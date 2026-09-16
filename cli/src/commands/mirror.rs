//! `sloop mirror` — make one database an exact copy of another.
//!
//! **A copy is not a backup, so this leaves nothing behind.** The dump goes from the
//! source's client straight into the destination's through a pipe, and no file is produced
//! at all — which is the owner's decision recorded as "A copy is not a backup" in
//! `docs/OWNER-DECISIONS.md`. The cost is the retry: a restore that dies halfway has nothing
//! to replay from, so `--safe` is the opt-in net, deleted the moment verification passes and
//! kept with its path printed when anything fails.
//!
//! **The source is only ever read.** Rule 6, and it is the reason the direction is in the
//! command's shape rather than in a flag pair somebody can get the wrong way round:
//! `sloop mirror live --to staging` reads `live` and writes `staging`, and nothing in this
//! module issues a statement against the source that changes anything — not even to collect
//! statistics.
//!
//! **The destination's database, owner and credentials are never touched.** What is replaced
//! is its contents: every schema that is not the system's on PostgreSQL, every table, view,
//! routine and event on the MySQL family. An application configured against that connection
//! string keeps working — see [`crate::engine::Adapter::clear_contents`], which `R13`'s
//! `restore` already needed.
//!
//! **Mirroring a database onto itself is refused**, on host, port and database together, and
//! `localhost` is treated as the same host as `127.0.0.1` because it is: a guard that can be
//! walked past by spelling the host differently is a guard that fails exactly when somebody
//! is in a hurry.
//!
//! **The destination does not have to exist yet** — `R14a`. With `--create` the database, the
//! role that owns it and the grants are all made first, by the same [`db::build`] that
//! `sloop db create` is, and then the copy runs into it. Three things follow from that, and
//! each one is load-bearing:
//!
//! - **The engine is never asked for.** A copy is *of* something, so the destination is
//!   whatever the source is. A flag that could disagree would be a flag for producing a
//!   mirror that cannot be read back.
//! - **Nothing has to be typed to confirm it**, because nothing is being destroyed. Rule 5
//!   protects data that exists; a database this command made two seconds ago holds none, and
//!   demanding its name anyway is the ceremony that teaches people to type names without
//!   reading them.
//! - **The new role owns and writes.** `R6a`'s privilege matrix is about a role that *reads*,
//!   for backups; this one is the owner of a database. The two matrices stay separate — see
//!   [`crate::engine::Adapter::provision`].
//!
//! The source is proved readable *before* anything is created, so the ordinary failure — a
//! password that has rotated, a server that is down — leaves no empty database and no
//! registry entry behind. Once the destination does exist it stays: dropping it again would
//! be a destructive act nobody asked for, so a copy that fails after that point says what is
//! now there rather than quietly tidying it away.

#[cfg(test)]
#[path = "mirror_tests.rs"]
mod tests;

use std::io::{IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};

use crate::consent::{Consent, Destroying};
use crate::engine::{Adapter, Target};
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::registry::file::{Database, check_name};
use crate::registry::{Registries, Scope};
use crate::secret::{Lookup, Secret, resolve};
use crate::style;
use crate::verify::{self, Side};

use super::backup::{describe_bytes, plural};
use super::db;

/// Everything `mirror` needs from the outside.
pub struct Context<'a> {
    /// Both registries, already open.
    pub registries: Registries,
    /// The global store, which is where sloop's own copy of the client tools lives.
    pub global: &'a Path,
    /// `--password-command`, which outranks whatever route a record names.
    pub password_command: Option<&'a str>,
    /// What this run was given permission to do — see [`crate::consent`].
    pub consent: Consent<'a>,
}

/// Everything `mirror` was asked for, named rather than positional.
pub struct Mirroring<'a> {
    /// The registered database to copy. Only ever read.
    pub source: &'a str,
    /// `--to`: a destination that is already registered.
    pub to: Option<&'a str>,
    /// `--create`: a destination that is not there yet, and what to call it here.
    pub create: Option<&'a str>,
    /// `--safe`: dump to a file first, so a copy that fails can be retried.
    pub safe: bool,
    /// The flags that only mean something when one is being made.
    pub new: New<'a>,
}

/// How to build a destination that does not exist yet — [`crate::cli::NewDestination`],
/// borrowed.
#[derive(Default)]
pub struct New<'a> {
    /// The server to make it on. The source's own when left out.
    pub host: Option<&'a str>,
    /// Its port.
    pub port: Option<u16>,
    /// The account to make it with.
    pub superuser: Option<&'a str>,
    /// Take that account's password from standard input.
    pub superuser_password_stdin: bool,
    /// Run this for that account's password.
    pub superuser_password_command: Option<&'a str>,
    /// The new database's own name on the server.
    pub database: Option<&'a str>,
    /// The role to create and hand it to.
    pub role: Option<&'a str>,
    /// Take the new role's password from standard input rather than generating one.
    pub role_password_stdin: bool,
}

impl<'a> From<&'a crate::cli::NewDestination> for New<'a> {
    fn from(flags: &'a crate::cli::NewDestination) -> Self {
        Self {
            host: flags.host.as_deref(),
            port: flags.port,
            superuser: flags.superuser.as_deref(),
            superuser_password_stdin: flags.superuser_password_stdin,
            superuser_password_command: flags.superuser_password_command.as_deref(),
            database: flags.database.as_deref(),
            role: flags.role.as_deref(),
            role_password_stdin: flags.role_password_stdin,
        }
    }
}

/// Which database this copy is going into.
enum Destination {
    /// One that is already registered. `R14`, unchanged.
    Registered(String),
    /// One that has to be made first. `R14a`.
    Made(String),
}

/// Copy `source` over a destination — making the destination first, if asked to.
pub fn run(context: &mut Context<'_>, asked: &Mirroring<'_>) -> Outcome<Exit> {
    let (from_scope, from) = context.registries.find(asked.source)?;
    let (from_scope, from) = (from_scope, from.clone());

    let Some(destination) = decide(context, asked)? else {
        anstream::println!("{}", style::dim("left alone."));
        return Ok(Exit::Success);
    };

    match destination {
        Destination::Registered(name) => {
            into_a_registered_database(context, asked, from_scope, &from, &name)
        }
        Destination::Made(name) => into_a_new_database(context, asked, from_scope, &from, &name),
    }
}

/// Where this copy is going, from the flags or from the person running it.
///
/// **The two flags mean different things and neither quietly becomes the other.** `--to` on a
/// name nothing is registered under is a typo, not an instruction to create a database, so it
/// stays a usage error — one that names `--create` rather than leaving somebody to find it.
/// `--create` on a name that *is* registered is the same mistake the other way round.
///
/// With neither flag and somebody there to ask, it asks. With neither flag and nobody there,
/// it exits `2` naming both — rule 4, and the half of `R14a`'s *Done when* that is about not
/// hanging.
fn decide(context: &Context<'_>, asked: &Mirroring<'_>) -> Outcome<Option<Destination>> {
    if let Some(name) = asked.create {
        if context.registries.find(name).is_ok() {
            return Err(Failure::usage(format!("{name} is already registered")).hint(format!(
                "--to {name} copies into it; --create is for a destination that is not there yet"
            )));
        }
        return Ok(Some(Destination::Made(name.to_owned())));
    }

    if let Some(name) = asked.to {
        return match context.registries.find(name) {
            Ok(_) => Ok(Some(Destination::Registered(name.to_owned()))),
            Err(failure) => Err(failure.hint(format!(
                "--create {name} would make it and copy into it, and `sloop db list` shows \
                 what is registered"
            ))),
        };
    }

    if !std::io::stdin().is_terminal() {
        return Err(Failure::new(
            Exit::Usage,
            format!(
                "{} has nowhere to be copied to, and there is no terminal to ask at",
                asked.source
            ),
        )
        .hint(
            "--to <NAME> copies into a database that is already registered, and \
             --create <NAME> makes one first",
        ));
    }

    let Some(typed) = ask_where(asked.source)? else {
        return Ok(None);
    };

    if context.registries.find(&typed).is_ok() {
        return Ok(Some(Destination::Registered(typed)));
    }

    // **Offered rather than refused.** The one moment somebody is standing there able to say
    // yes is a strange moment to tell them to run the command again with another flag.
    let question = format!("{typed} is not registered. Make it, and copy into it?");
    if !context
        .consent
        .asked(&question, &format!("--create {typed}"))?
    {
        return Ok(None);
    }
    Ok(Some(Destination::Made(typed)))
}

/// Ask which database this is going into. `None` when nothing was named.
fn ask_where(source: &str) -> Outcome<Option<String>> {
    anstream::print!(
        "{} copy {} into which database? ",
        style::paint("?"),
        style::paint(source)
    );
    let _ = std::io::stdout().flush();

    let mut given = String::new();
    std::io::stdin()
        .read_line(&mut given)
        .map_err(|error| Failure::usage(format!("could not read the answer: {error}")))?;

    let given = given.trim();
    Ok((!given.is_empty()).then(|| given.to_owned()))
}

/// `--to`: the destination is already there, and this is `R14` exactly.
fn into_a_registered_database(
    context: &Context<'_>,
    asked: &Mirroring<'_>,
    from_scope: Scope,
    from: &Database,
    destination: &str,
) -> Outcome<Exit> {
    let (into_scope, into) = context.registries.find(destination)?;
    let into = into.clone();

    refuse_a_self_mirror(asked.source, from, destination, &into)?;

    if from.engine != into.engine {
        return Err(Failure::usage(format!(
            "{} is {} and {destination} is {}",
            asked.source, from.engine, into.engine
        ))
        .hint("R29 is where copying across engines gets decided; today it is refused"));
    }

    // The paperwork before a password is fetched or a socket opened.
    let destroying = Destroying {
        named: &into.database,
        noun: "database",
        action: "replacing a database's contents",
    };
    context.consent.checked_early(&destroying)?;

    let reading = secret_for(
        &context.registries,
        context.password_command,
        from_scope,
        from,
    )?;
    let writing = secret_for(
        &context.registries,
        context.password_command,
        into_scope,
        &into,
    )?;

    copy(
        context,
        asked,
        &from.target(&reading),
        &into.target(&writing),
        Some(&destroying),
    )
}

/// `--create`: the destination is made first, and then copied into.
///
/// **The order is the one that leaves least behind when it goes wrong.** The label is
/// checked, the self-mirror guard runs against the database that is *about* to exist, the
/// source's password is fetched and the source is contacted — all before a superuser password
/// is asked for, and all before one statement runs on the destination's server. Nothing is
/// created until there is something proven to put in it.
fn into_a_new_database(
    context: &mut Context<'_>,
    asked: &Mirroring<'_>,
    from_scope: Scope,
    from: &Database,
    label: &str,
) -> Outcome<Exit> {
    check_name(label)?;

    // The engine and the server are the source's, so the connection this *will* be is known
    // before anything is asked to make it — which is what lets the guard fire on a database
    // that does not exist yet.
    let proposed = proposed(from, label, &asked.new);
    refuse_the_same_connection(
        asked.source,
        from,
        label,
        &proposed.host,
        proposed.port,
        &proposed.database,
        MIRRORING_ITSELF,
    )?;

    let building = db::Building {
        name: label,
        engine: from.engine,
        host: &proposed.host,
        port: Some(proposed.port),
        superuser: asked.new.superuser,
        superuser_password_stdin: asked.new.superuser_password_stdin,
        superuser_password_command: asked.new.superuser_password_command,
        database: Some(&proposed.database),
        role: Some(&proposed.role),
        role_password_stdin: asked.new.role_password_stdin,
    };

    // **Every flag that is missing, named before a socket is opened.** [`db::build`] asks
    // this too, and would ask it in time — but by then the source's password has been
    // fetched and its server contacted, so an unattended run would hear about a failed
    // connection first and about the two flags it was actually missing second. Reading the
    // flags costs nothing, so it goes first.
    db::unattended_needs(&building)?;

    let reading = secret_for(
        &context.registries,
        context.password_command,
        from_scope,
        from,
    )?;
    super::adapter_for(from.engine, context.global).probe(&from.target(&reading))?;

    let built = db::build(
        &mut context.registries,
        context.global,
        context.consent,
        &building,
    )?;

    // **No `Destroying`.** There is nothing in there to destroy — see the note at the top of
    // this module.
    copy(
        context,
        asked,
        &from.target(&reading),
        &built.record.target(&built.secret),
        None,
    )
}

/// The connection a `--create` is about to bring into existence.
struct Proposed {
    /// The server it goes on.
    host: String,
    /// Its port.
    port: u16,
    /// The database's own name there.
    database: String,
    /// The role that will own it.
    role: String,
}

/// Work out what `--create` is actually asking for, filling in what was not said.
///
/// **The source's own server unless told otherwise**, because a staging copy beside the live
/// database is the case this exists for, and a default that made somebody type `--host` every
/// time would be a default chosen for symmetry rather than for use.
///
/// **The port follows the host.** The source's when it is the source's machine; the engine's
/// own default when it is somebody else's — a source listening on 5433 says nothing about
/// what any other machine listens on.
fn proposed(from: &Database, label: &str, new: &New<'_>) -> Proposed {
    let host = new.host.unwrap_or(&from.host).to_owned();
    let port = new.port.unwrap_or_else(|| {
        if same_host(&host, &from.host) {
            from.port
        } else {
            from.engine.default_port()
        }
    });
    let database = new.database.unwrap_or(label).to_owned();
    let role = new.role.unwrap_or(&database).to_owned();

    Proposed {
        host,
        port,
        database,
        role,
    }
}

/// The copy itself, once both ends are settled.
///
/// `destroying` is `Some` when the destination held something before this run started and
/// `None` when this run made it. By the time the two paths meet here that is the only
/// difference left between them, and it is exactly one question: does anybody have to type a
/// name?
fn copy(
    context: &Context<'_>,
    asked: &Mirroring<'_>,
    source_target: &Target<'_>,
    destination_target: &Target<'_>,
    destroying: Option<&Destroying<'_>>,
) -> Outcome<Exit> {
    let adapter = super::adapter_for(source_target.engine, context.global);

    anstream::println!(
        "{} {}",
        style::paint("mirroring"),
        style::dim(&format!(
            "{} → {}",
            source_target.describe(),
            destination_target.describe()
        ))
    );

    // **Counted before the dump, on the source.** That is the moment the dump describes,
    // and it is the only number `verify` can call drift rather than loss afterwards.
    let mode = verify::Mode::from_environment();
    let before = announce_both(adapter.as_ref(), source_target, destination_target, mode)?;

    if let Some(destroying) = destroying {
        if !context.consent.typed(destroying)?.granted() {
            anstream::println!("{}", style::dim("left alone."));
            return Ok(Exit::Success);
        }
    }

    let cleared = adapter.clear_contents(destination_target)?;
    if cleared > 0 {
        anstream::println!(
            "  {}",
            style::dim(&format!("cleared {}", plural(cleared, "table")))
        );
    }

    let started = std::time::Instant::now();
    let kept = if asked.safe {
        Some(through_a_file(
            adapter.as_ref(),
            source_target,
            destination_target,
        )?)
    } else {
        // Straight down an OS pipe between the two clients — no file, and nothing through
        // this process. See `Adapter::copy_into`.
        adapter.copy_into(source_target, destination_target)?;
        None
    };
    anstream::println!(
        "  {}",
        style::dim(&format!(
            "copied in {:.1}s",
            started.elapsed().as_secs_f64()
        ))
    );

    let comparison = verify::Comparison::of(
        mode,
        &before,
        &Side::counted(adapter.as_ref(), destination_target, mode)?,
    );
    for line in comparison.describe() {
        anstream::println!("  {}", style::dim(&line));
    }

    // **The net goes only when the copy is proved.** `--safe` exists for the run that does
    // not land; deleting the dump before the counts agreed would be deleting it exactly when
    // it was wanted.
    if let Some(dump) = kept {
        if comparison.landed() {
            discard(&dump);
        } else {
            anstream::eprintln!(
                "{}",
                style::dim(&format!(
                    "--safe: the dump is kept at {} — the counts did not agree",
                    dump.display()
                ))
            );
        }
    }

    Ok(comparison.exit())
}

/// Say what is being copied and what is in the way, and hand back the source's counts.
///
/// **Both ends are probed before anything is asked for**, so a destination that cannot be
/// reached is a connection failure rather than a question somebody answers and then watches
/// fail. The source's counts come from the same pass, because they have to be taken before
/// the dump either way — that is the moment the dump describes, and the only reading
/// `verify` can call drift rather than loss afterwards.
fn announce_both(
    adapter: &dyn Adapter,
    source: &Target<'_>,
    destination: &Target<'_>,
    mode: verify::Mode,
) -> Outcome<Side> {
    let from = adapter.probe(source)?;
    let into = adapter.probe(destination)?;
    anstream::println!(
        "  {}",
        style::dim(&format!(
            "{} {} → {} {}",
            from.engine, from.version, into.engine, into.version
        ))
    );

    let before = Side::counted(adapter, source, mode)?;
    let rows: u64 = before.counts.iter().map(|count| count.rows).sum();
    anstream::println!(
        "  {}",
        style::dim(&format!(
            "copying {}, {}",
            plural(count(before.counts.len()), "table"),
            plural(rows, "row")
        ))
    );

    announce_destination(adapter, destination);

    Ok(before)
}

/// Dump to a file first, then restore from it, and hand the path back.
///
/// `--safe`, and the cost is stated where somebody can act on it: for as long as the copy
/// runs there is an **unencrypted** dump of the source on this machine's disk. That is the
/// trade the flag is asking for — a copy that can be retried — and it is deleted the moment
/// the counts agree.
fn through_a_file(
    adapter: &dyn Adapter,
    source: &Target<'_>,
    destination: &Target<'_>,
) -> Outcome<PathBuf> {
    let directory = std::env::temp_dir().join(format!(
        "sloop-mirror-{}-{}",
        std::process::id(),
        crate::backup::stamp::Stamp::now().utc_path()
    ));
    std::fs::create_dir_all(&directory).map_err(|error| {
        Failure::new(
            Exit::Dump,
            format!("could not create {}: {error}", directory.display()),
        )
    })?;

    let dump = crate::backup::dump_file(&directory);
    anstream::eprintln!(
        "{}",
        style::dim(&format!(
            "--safe: dumping to {} first — it is not encrypted, and it goes when the counts \
             agree",
            dump.display()
        ))
    );

    let written = adapter.dump(source, &dump);
    let summary = match written {
        Ok(summary) => summary,
        Err(failure) => {
            discard(&directory);
            return Err(failure);
        }
    };
    anstream::println!(
        "  {}",
        style::dim(&format!("dumped {}", describe_bytes(summary.bytes)))
    );

    if let Err(failure) = adapter.restore(destination, &dump) {
        anstream::eprintln!(
            "{}",
            style::dim(&format!(
                "--safe: the dump is kept at {} — the restore did not finish",
                dump.display()
            ))
        );
        return Err(failure);
    }

    Ok(directory)
}

/// Remove a temporary dump, and say so if it will not go.
///
/// Best effort on purpose: the copy has landed and been verified by the time this runs, so a
/// file that cannot be deleted is untidiness rather than a failure — but it is untidiness
/// holding a plaintext dump, so it is never silent.
fn discard(directory: &Path) {
    if let Err(error) = std::fs::remove_dir_all(directory) {
        anstream::eprintln!(
            "{}",
            style::dim(&format!(
                "the temporary dump at {} could not be removed: {error}",
                directory.display()
            ))
        );
    }
}

/// **Refuse to mirror a database over itself.**
///
/// Host, port and database together, and `localhost` counts as `127.0.0.1`: one command
/// where the two names differ but the connection does not would drop every table in the
/// source and restore them from a dump of themselves — which works, right up until the dump
/// fails halfway.
fn refuse_a_self_mirror(
    source: &str,
    from: &Database,
    destination: &str,
    into: &Database,
) -> Outcome<()> {
    refuse_the_same_connection(
        source,
        from,
        destination,
        &into.host,
        into.port,
        &into.database,
        MIRRORING_ITSELF,
    )
}

/// What a mirror onto itself would have done, which is the half worth reading.
const MIRRORING_ITSELF: &str =
    "a mirror drops the destination's contents, so this would destroy the source";

/// The same guard, against a destination that may not exist yet.
///
/// **`--create` needs this before it creates anything**, and at that point there is no record
/// to compare against — only the host, port and name the new database is *going* to have. So
/// the comparison takes those three rather than a second [`Database`], and the guard is one
/// implementation serving both paths instead of a copy for each.
///
/// `consequence` is what the caller would have done, because that is the half of the message
/// worth reading: a mirror onto itself destroys the source, and a sync onto itself merely
/// wastes an afternoon. `sync` is the third caller — see [`super::sync`].
pub(super) fn refuse_the_same_connection(
    source: &str,
    from: &Database,
    destination: &str,
    host: &str,
    port: u16,
    database: &str,
    consequence: &str,
) -> Outcome<()> {
    if from.port != port || from.database != database || !same_host(&from.host, host) {
        return Ok(());
    }

    Err(Failure::usage(format!(
        "{source} and {destination} are the same database — {}",
        crate::engine::connection_string(
            from.engine,
            &from.user,
            &from.host,
            from.port,
            &from.database
        )
    ))
    .hint(consequence))
}

/// Is this the same machine, however the two were spelled?
fn same_host(one: &str, two: &str) -> bool {
    /// Every spelling of "this machine" that resolves to the same place.
    const HERE: [&str; 4] = ["localhost", "127.0.0.1", "::1", "[::1]"];

    let here = |host: &str| HERE.iter().any(|known| host.eq_ignore_ascii_case(known));
    one.eq_ignore_ascii_case(two) || (here(one) && here(two))
}

/// Resolve one end's password through whichever of the four routes its record names.
///
/// **The pieces rather than a [`Context`]**, because `sync` holds a context of its own shape
/// and needs exactly this: the registries the record came out of, and whatever
/// `--password-command` said.
pub(super) fn secret_for(
    registries: &Registries,
    password_command: Option<&str>,
    scope: Scope,
    record: &Database,
) -> Outcome<Secret> {
    let key = record.credential_key();
    let route = record.password.overridden_by(password_command);
    let sealed = registries.sealed_in(scope).unwrap_or_default();
    let resolved = resolve(
        &route,
        &Lookup {
            key: &key,
            sealed_file: &sealed,
        },
    )?;
    for note in &resolved.notes {
        anstream::println!("  {}", style::dim(note));
    }
    Ok(resolved.secret)
}

/// Say what is in the destination now, before the question rather than after it.
fn announce_destination(adapter: &dyn Adapter, target: &Target<'_>) {
    // Best effort: a role that cannot count is about to find that out from the clearing,
    // and refusing to mirror because the tables could not be listed would be the wrong way
    // round.
    let existing = adapter.row_counts(target).unwrap_or_default();
    if existing.is_empty() {
        anstream::println!("  {}", style::dim("the destination is empty"));
        return;
    }

    let rows: u64 = existing.iter().map(|count| count.rows).sum();
    anstream::println!(
        "  {}",
        style::dim(&format!(
            "replacing what the destination has now — {}, {}",
            plural(count(existing.len()), "table"),
            plural(rows, "row")
        ))
    );
}

/// A count as the width [`plural`] takes.
fn count(of: usize) -> u64 {
    u64::try_from(of).unwrap_or(u64::MAX)
}
