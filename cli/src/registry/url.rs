//! Reading a connection URL, because that is the form people already have one in.
//!
//! **Hand-rolled, and the reason is the dependency graph.** A URL crate would be a
//! reasonable choice in any other project; here the whole product claim is that
//! `cargo tree` is short enough to read, and a general-purpose URL parser brings a
//! percent-encoding crate, an IDNA crate and a Unicode table with it. What is needed is
//! one scheme family, and that fits in this file.
//!
//! **Everything after a `%` is decoded, once.** A password is allowed to contain `@`, `:`,
//! `/` and `#`, and the only way those survive a URL is percent-encoded — so `p%40ss` is
//! the password `p@ss` and not the six characters somebody typed. Decoding happens exactly
//! once, on the userinfo and on the path, and never on a value that has already been
//! decoded: a password of `100%25` means `100%`, and running the decoder twice would turn
//! it into something the server has never heard of.
//!
//! **The password comes out as a [`Secret`] and stops there.** It is never written back
//! into the registry, never logged and never put in the argv of anything — `db add` takes
//! it, files it under whichever of the four routes was chosen, and drops it.

use crate::engine::Engine;
use crate::failure::{Failure, Outcome};
use crate::secret::Secret;

/// What a connection URL turned out to say.
///
/// Every field is optional except the ones a URL cannot omit, because a URL is allowed to
/// be partial — `postgres://host/app` names no user, and the caller fills that in from a
/// flag or from the machine's own account.
///
/// `Debug` is safe to derive: the one field that could say anything is a [`Secret`], whose
/// own `Debug` prints `Secret(<redacted>)` and never the value.
#[derive(Debug)]
pub struct Parsed {
    /// From the scheme.
    pub engine: Engine,
    /// Host name or address. Never empty.
    pub host: String,
    /// The port, when the URL gave one.
    pub port: Option<u16>,
    /// The database, from the path.
    pub database: Option<String>,
    /// The role, from the userinfo.
    pub user: Option<String>,
    /// The password, when the URL carried one — which is a thing to warn about rather
    /// than a thing to refuse. See `db add`.
    pub password: Option<Secret>,
}

/// Read a connection URL.
///
/// The shapes, all of which turn up in the wild:
///
/// ```text
/// postgres://app@db.internal:5432/orders
/// postgresql://app:p%40ss@db.internal/orders
/// mysql://root@127.0.0.1:3306/shop
/// mariadb://db.internal/shop
/// postgres://app@[2001:db8::1]:5432/orders
/// ```
pub fn parse(input: &str) -> Outcome<Parsed> {
    let refuse = |why: &str| {
        Failure::usage(format!("{input} is not a connection URL: {why}"))
            .hint("it looks like postgres://user@host:5432/database — mysql:// and mariadb:// too")
    };

    let (scheme, rest) = input.split_once("://").ok_or_else(|| refuse("no scheme"))?;
    let engine = engine_for(scheme).ok_or_else(|| {
        Failure::usage(format!("{scheme} is not an engine sloop knows")).hint(
            "postgres:// or postgresql://, mysql://, mariadb:// — those are the three engines",
        )
    })?;

    // Query and fragment are cut before anything else, so a `?sslmode=require` cannot end
    // up inside a database name. Nothing in a query string is honoured: how sloop connects
    // is settled by the adapter, and quietly ignoring half a URL is worse than dropping it.
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);

    // The last `@` splits userinfo from the host, not the first: a password may contain an
    // un-encoded `@` and still be what somebody pasted.
    let (userinfo, hostpath) = match rest.rsplit_once('@') {
        Some((userinfo, hostpath)) => (Some(userinfo), hostpath),
        None => (None, rest),
    };

    let (authority, path) = match hostpath.split_once('/') {
        Some((authority, path)) => (authority, Some(path)),
        None => (hostpath, None),
    };

    let (host, port) =
        split_host_and_port(authority).ok_or_else(|| refuse("the port is not a number"))?;
    if host.is_empty() {
        return Err(refuse("no host"));
    }

    let (user, password) = match userinfo {
        None => (None, None),
        Some(userinfo) => {
            let (user, password) = match userinfo.split_once(':') {
                Some((user, password)) => (user, Some(password)),
                None => (userinfo, None),
            };
            (
                non_empty(decode(user)),
                password.map(|value| Secret::new(decode(value))),
            )
        }
    };

    Ok(Parsed {
        engine,
        host: decode(&host),
        port,
        database: path.map(decode).and_then(non_empty),
        user,
        password,
    })
}

/// The engine a scheme names.
///
/// `postgresql://` as well as `postgres://`, because libpq accepts both and every
/// connection string people already have uses one or the other.
fn engine_for(scheme: &str) -> Option<Engine> {
    match scheme.to_ascii_lowercase().as_str() {
        "postgres" | "postgresql" => Some(Engine::Postgres),
        "mysql" => Some(Engine::Mysql),
        "mariadb" => Some(Engine::Mariadb),
        _ => None,
    }
}

/// Split `host:port`, `host`, `[::1]:port` or `[::1]`.
///
/// The brackets are what make this more than a `rsplit_once(':')`: an IPv6 address is
/// full of colons, and the last one is part of the address rather than a port marker.
/// `None` means there was a port and it was not a number — which is a typo worth
/// reporting, not a host called `db:x`.
fn split_host_and_port(authority: &str) -> Option<(String, Option<u16>)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (address, after) = rest.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some("") | None => None,
            Some(digits) => Some(digits.parse().ok()?),
        };
        return Some((address.to_owned(), port));
    }

    match authority.rsplit_once(':') {
        Some((host, "")) => Some((host.to_owned(), None)),
        Some((host, digits)) => Some((host.to_owned(), Some(digits.parse().ok()?))),
        None => Some((authority.to_owned(), None)),
    }
}

/// Percent-decoding, and nothing else.
///
/// `+` is **not** a space. That convention belongs to HTML form bodies, and a password of
/// `a+b` is a password of `a+b` — silently turning it into `a b` would be an authentication
/// failure with no visible cause, which is the exact class of bug this codebase keeps
/// writing comments about.
///
/// A `%` that is not followed by two hex digits is left alone, because a password is
/// allowed to contain one and refusing the URL over it would be worse than taking it
/// literally.
fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        let pair = (index + 2 < bytes.len())
            .then(|| {
                let high = (bytes[index] == b'%').then(|| hex(bytes[index + 1]))??;
                let low = hex(bytes[index + 2])?;
                Some(high * 16 + low)
            })
            .flatten();

        if let Some(byte) = pair {
            out.push(byte);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }

    // A percent-encoded sequence can spell any byte, including one that is not UTF-8 on
    // its own. Lossy rather than an error: a password sloop cannot represent is a password
    // the user will be told about when the server refuses it, and refusing to read the URL
    // at all would be less useful than trying.
    String::from_utf8_lossy(&out).into_owned()
}

const fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::{decode, parse, split_host_and_port};
    use crate::engine::Engine;

    #[test]
    fn the_shapes_people_actually_paste() {
        let parsed = parse("postgres://app@db.internal:5432/orders").unwrap();
        assert_eq!(parsed.engine, Engine::Postgres);
        assert_eq!(parsed.host, "db.internal");
        assert_eq!(parsed.port, Some(5432));
        assert_eq!(parsed.user.as_deref(), Some("app"));
        assert_eq!(parsed.database.as_deref(), Some("orders"));
        assert!(parsed.password.is_none());

        // libpq takes both spellings, so every connection string in the wild uses one.
        assert_eq!(parse("postgresql://h/d").unwrap().engine, Engine::Postgres);
        assert_eq!(parse("mysql://h/d").unwrap().engine, Engine::Mysql);
        assert_eq!(parse("mariadb://h/d").unwrap().engine, Engine::Mariadb);
        // The scheme is not case-sensitive anywhere else, and should not be here.
        assert_eq!(parse("POSTGRES://h/d").unwrap().engine, Engine::Postgres);
    }

    /// A URL is allowed to be partial, and the caller fills the rest in from flags. A
    /// parser that demanded every field would refuse `postgres://host/db`, which is a
    /// perfectly ordinary thing to have.
    #[test]
    fn everything_but_the_host_may_be_absent() {
        let parsed = parse("mysql://db.internal").unwrap();
        assert_eq!(parsed.host, "db.internal");
        assert_eq!(parsed.port, None);
        assert_eq!(parsed.user, None);
        assert_eq!(parsed.database, None);

        let with_path = parse("mysql://db.internal/").unwrap();
        assert_eq!(with_path.database, None, "an empty path is no database");
    }

    /// The whole reason this file decodes anything: a real password contains the
    /// characters a URL uses as punctuation.
    #[test]
    fn a_password_survives_the_punctuation_it_is_made_of() {
        let parsed = parse("postgres://app:p%40ss%3Aword%2F%23%21@host/db").unwrap();
        assert_eq!(parsed.user.as_deref(), Some("app"));
        assert_eq!(parsed.password.unwrap().expose(), "p@ss:word/#!");

        // The last `@` wins, so an un-encoded one in a password still parses.
        let sloppy = parse("postgres://app:p@ss@host/db").unwrap();
        assert_eq!(sloppy.host, "host");
        assert_eq!(sloppy.password.unwrap().expose(), "p@ss");
    }

    /// `+` is a space in an HTML form body and nowhere else. Turning `a+b` into `a b`
    /// would be an authentication failure with nothing on screen to explain it.
    #[test]
    fn a_plus_is_a_plus() {
        let parsed = parse("postgres://u:a+b@host/db").unwrap();
        assert_eq!(parsed.password.unwrap().expose(), "a+b");
    }

    /// Decoding happens once. `100%25` is `100%`, not the start of another escape.
    #[test]
    fn decoding_happens_exactly_once() {
        assert_eq!(decode("100%25"), "100%");
        assert_eq!(decode("%2525"), "%25");
        // A stray percent is a character, not a parse error: passwords contain them.
        assert_eq!(decode("50%"), "50%");
        assert_eq!(decode("%zz"), "%zz");
        assert_eq!(decode("%4"), "%4");
        assert_eq!(decode(""), "");
        // And a multi-byte character spelled out byte by byte comes back whole.
        assert_eq!(decode("%C3%BC"), "ü");
    }

    #[test]
    fn an_ipv6_address_is_not_a_host_with_six_ports() {
        assert_eq!(
            split_host_and_port("[2001:db8::1]:5432"),
            Some(("2001:db8::1".to_owned(), Some(5432)))
        );
        assert_eq!(split_host_and_port("[::1]"), Some(("::1".to_owned(), None)));

        let parsed = parse("postgres://app@[2001:db8::1]:5432/orders").unwrap();
        assert_eq!(parsed.host, "2001:db8::1");
        assert_eq!(parsed.port, Some(5432));
        assert_eq!(parsed.database.as_deref(), Some("orders"));
    }

    /// A query string is dropped rather than half-honoured. Reading `?sslmode=require`
    /// and then ignoring it would be worse than not reading it.
    #[test]
    fn a_query_string_does_not_become_part_of_the_database_name() {
        let parsed = parse("postgres://app@host:5432/orders?sslmode=require").unwrap();
        assert_eq!(parsed.database.as_deref(), Some("orders"));

        let fragment = parse("mysql://host/shop#notes").unwrap();
        assert_eq!(fragment.database.as_deref(), Some("shop"));
    }

    #[test]
    fn what_is_refused_and_why() {
        for (input, expected) in [
            ("db.internal/orders", "no scheme"),
            ("redis://host/0", "not an engine"),
            ("postgres:///orders", "no host"),
            ("postgres://host:not-a-port/db", "not a number"),
        ] {
            let failure = parse(input).expect_err(input);
            assert_eq!(failure.exit().code(), 2, "{input}");
            assert!(
                failure.message().contains(expected),
                "{input}: {}",
                failure.message()
            );
        }
    }
}
