//! `sloop db …` — registering a database, and everything that reads or edits a record.
//!
//! **A URL and a field-by-field registration produce the same record, and that is checked
//! rather than intended.** `--url` is read into the same fields the flags fill in, the
//! flags then override whatever the URL said, and the result goes through one constructor.
//! There is no second code path for a URL to drift down.
//!
//! **There is no `--password` flag, and there will not be one.** Every argument of every
//! process on the machine is in `ps`, so a password that arrives that way is a password
//! that has already been read by anyone who wanted it. The value is typed at a hidden
//! prompt, piped in with `--password-stdin`, or never handled at all — which is what the
//! `${VAR}` and `command:` routes are for.
//!
//! **A URL that carries a password is taken, used and complained about.** Refusing it
//! would protect nothing: by the time sloop is running, that password is already in the
//! shell's history and was already in `ps`, and the only thing a refusal achieves is that
//! the paste somebody just did does not work. So it is accepted, filed under a real route,
//! never written back, and the user is told plainly where it has already been seen.
//!
//! **Nothing here prompts without a terminal.** A registration that needs a password and
//! has no way to ask for one exits `2` naming `--password-stdin`, because a scheduled run
//! that hangs on an invisible question is the worst failure this tool can have.

use std::io::{IsTerminal as _, Read as _};
use std::path::Path;

use crate::cli::{Fields, PasswordSource};
use crate::engine::{Engine, Target, adapter_for};
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::registry::file::{Database, check_name};
use crate::registry::{Registries, Scope};
use crate::secret::{Lookup, Route, Secret, resolve};
use crate::style;

/// Everything a `db` command needs from the outside.
pub struct Context<'a> {
    /// Both registries, already open.
    pub registries: Registries,
    /// `--password-command`, which outranks whatever route a record names.
    pub password_command: Option<&'a str>,
    /// The global store, for the sentence that says which registry was read.
    pub global: &'a Path,
}

// ---------------------------------------------------------------------------------------
// add
// ---------------------------------------------------------------------------------------

/// Register a database.
pub fn add(
    context: &mut Context<'_>,
    name: &str,
    url: Option<&str>,
    fields: &Fields,
    password: &PasswordSource,
    test: bool,
    force: bool,
) -> Outcome<Exit> {
    check_name(name)?;

    let scope = context.registries.writes_to();
    let replacing = context
        .registries
        .in_scope(scope)
        .and_then(|registry| registry.get(name))
        .cloned();

    if replacing.is_some() && !force {
        return Err(Failure::usage(format!(
            "{name} is already registered in the {} registry",
            scope.label()
        ))
        .hint("`sloop db edit` changes it, and --force replaces it outright"));
    }

    let (draft, from_url) = draft(None, url, fields)?;
    let route = route_for(password, None)?;
    let database = draft.into_database(route.clone())?;

    // Everything that can fail without leaving a trace happens before anything is written:
    // the password is fetched, the connection is tried, and only then does the machine
    // change. A half-registered database is worse than an unregistered one.
    let secret = secret_for(&route, password, from_url, &database, context)?;
    if test {
        let reached = probe(&database, secret.as_ref(), context)?;
        announce_server(&reached);
    }

    // Replacing a record orphans whatever the old one pointed at, exactly as an edit does.
    let retire = replacing
        .as_ref()
        .and_then(|existing| Retiring::between(existing, &database));
    write(context, scope, name, &database, secret.as_ref(), retire)?;

    anstream::println!(
        "{} {} in the {} registry, {}",
        style::paint("registered"),
        style::paint(name),
        scope.label(),
        style::dim(&format!("password from {}", route.describe()))
    );
    anstream::println!("  {}", style::dim(&database.credential_key()));
    Ok(Exit::Success)
}

// ---------------------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------------------

/// One line of `db list`, gathered before anything is printed so the columns can be
/// measured against every row rather than guessed at.
type Row = (String, Scope, String, String);

/// Show what is registered. Never a secret — see [`Route::describe`].
///
/// It cannot fail, and says so: the registries were read before this was called, and
/// there is no state of a registry that cannot be printed.
pub fn list(context: &Context<'_>) -> Exit {
    if context.registries.is_empty() {
        anstream::println!(
            "{}",
            style::dim(&format!(
                "Nothing is registered in {}.",
                context.registries.resolution().describe(context.global)
            ))
        );
        anstream::println!(
            "{}",
            style::dim("`sloop db add <name> --url postgres://user@host/database` starts one.")
        );
        return Exit::Success;
    }

    let rows: Vec<Row> = context
        .registries
        .all()
        .map(|(scope, name, database)| {
            (
                name.to_owned(),
                scope,
                database.credential_key(),
                // A route, never a value. Not one of these phrases can contain a
                // password, which is the property that lets this be printed at all.
                database
                    .password
                    .overridden_by(context.password_command)
                    .describe(),
            )
        })
        .collect();

    // **Padded before it is painted, not after.** `style::paint` wraps the text in escape
    // sequences, and a width specifier counts those — so `{:<9}` on a painted six-letter
    // name pads to nothing at all and the columns collapse. The terminal never sees the
    // escapes as width, so the spaces have to be counted on the plain text.
    let pad = |text: &str, to: usize| " ".repeat(to.saturating_sub(text.chars().count()));
    let widest = |column: fn(&Row) -> &String| {
        rows.iter()
            .map(|row| column(row).chars().count())
            .max()
            .unwrap_or(0)
    };
    let name_column = widest(|row| &row.0);
    let connection_column = widest(|row| &row.2);

    let mut seen: Vec<&str> = Vec::new();

    for (name, scope, connection, route) in &rows {
        // A project entry shadows a global one of the same name. Saying so is the
        // difference between a confusing listing and an explanation.
        let shadowed = seen.contains(&name.as_str());
        seen.push(name);

        anstream::println!(
            "{}{}  {}{}  {}",
            style::paint(name),
            pad(name, name_column),
            connection,
            pad(connection, connection_column),
            style::dim(&format!(
                "{}{} · {route}",
                scope.label(),
                if shadowed { ", shadowed" } else { "" }
            )),
        );
    }

    anstream::println!();
    anstream::println!(
        "{}",
        style::dim(&format!(
            "A bare name is looked for in {}.",
            context
                .registries
                .resolution()
                .describe_lookup(context.global)
        ))
    );
    Exit::Success
}

// ---------------------------------------------------------------------------------------
// test
// ---------------------------------------------------------------------------------------

/// Open a connection and say what answered.
///
/// **It connects and stops there.** Whether the role can actually take a backup is a
/// different and much longer question, and `sloop doctor` is where it is answered — two
/// commands that both half-answered it would be two places to keep in step.
pub fn test(context: &Context<'_>, name: Option<&str>) -> Outcome<Exit> {
    let wanted: Vec<(Scope, String, Database)> = match name {
        Some(name) => {
            let (scope, database) = context.registries.find(name)?;
            vec![(scope, name.to_owned(), database.clone())]
        }
        None => context
            .registries
            .all()
            .map(|(scope, name, database)| (scope, name.to_owned(), database.clone()))
            .collect(),
    };

    if wanted.is_empty() {
        anstream::println!(
            "{}",
            style::dim("Nothing is registered, so nothing to test.")
        );
        return Ok(Exit::Success);
    }

    // Like `backup --all`, a failure is reported and the run carries on: knowing that
    // four of five are fine is worth more than stopping at the first one that is not.
    let mut worst = Exit::Success;

    for (scope, name, database) in &wanted {
        anstream::println!();
        anstream::println!(
            "{}  {}  {}",
            style::paint(name),
            database.credential_key(),
            style::dim(scope.label())
        );

        match connect(database, context, *scope) {
            Ok(server) => announce_server(&server),
            Err(failure) => {
                failure.report();
                if matches!(worst, Exit::Success) {
                    worst = failure.exit();
                }
            }
        }
    }

    Ok(worst)
}

// ---------------------------------------------------------------------------------------
// edit
// ---------------------------------------------------------------------------------------

/// Change a registered database's details.
pub fn edit(
    context: &mut Context<'_>,
    name: &str,
    url: Option<&str>,
    fields: &Fields,
    password: &PasswordSource,
    test: bool,
) -> Outcome<Exit> {
    let (scope, before) = context.registries.find(name)?;
    let before = before.clone();

    let (draft, from_url) = draft(Some(&before), url, fields)?;
    let route = route_for(password, Some(&before.password))?;
    let after = draft.into_database(route.clone())?;

    if after == before {
        anstream::println!("{}", style::dim(&format!("{name} is already like that.")));
        return Ok(Exit::Success);
    }

    // **The key moves with the connection**, and this is the whole reason `edit` is more
    // than three lines. A stored password is filed under the connection rather than the
    // label, so changing the host changes where the password lives — and an edit that did
    // not carry it across would leave a record pointing at a credential that is not there.
    let moved = before.credential_key() != after.credential_key();
    let route_changed = route != before.password;
    let supplied = password.password_stdin || from_url.is_some();

    let secret = if supplied {
        secret_for(&route, password, from_url, &after, context)?
    } else if route.is_stored() && (moved || route_changed) {
        // **Carried across rather than asked for again.** Changing a port should not cost
        // somebody their password, and both stored routes can be read as well as written —
        // so the old one is fetched from wherever it was and filed under the new key. It is
        // also how `--encrypted-file` migrates a keyring entry into the file.
        match carry_over(&before, context, scope) {
            Some(existing) => Some(existing),
            // Nothing to carry: the record was on `${VAR}` or `command:` before, or there
            // is simply no password under the old key. Ask, and say why.
            None => secret_for(&route, password, from_url, &after, context).map_err(|failure| {
                failure.hint(
                    "this edit needs the password filed under the new connection, and \
                         there was none under the old one. Supply it with --password-stdin, \
                         or at the prompt",
                )
            })?,
        }
    } else {
        None
    };

    if test {
        let reached = probe(&after, secret.as_ref(), context)?;
        announce_server(&reached);
    }

    write(
        context,
        scope,
        name,
        &after,
        secret.as_ref(),
        Retiring::between(&before, &after),
    )?;

    anstream::println!(
        "{} {} {}",
        style::paint("changed"),
        style::paint(name),
        style::dim(&format!("in the {} registry", scope.label()))
    );
    anstream::println!("  {}", style::dim(&before.credential_key()));
    anstream::println!("  {}", after.credential_key());
    Ok(Exit::Success)
}

// ---------------------------------------------------------------------------------------
// rename
// ---------------------------------------------------------------------------------------

/// Give a registered database a different name.
///
/// **Only the label moves.** The password is filed under the connection — see
/// [`Database::credential_key`] — precisely so that this command cannot orphan one, which
/// is why it is four lines and `edit` is forty.
pub fn rename(context: &mut Context<'_>, from: &str, to: &str) -> Outcome<Exit> {
    check_name(to)?;
    let (scope, _) = context.registries.find(from)?;

    if context
        .registries
        .in_scope(scope)
        .is_some_and(|registry| registry.get(to).is_some())
    {
        return Err(Failure::usage(format!(
            "{to} is already registered in the {} registry",
            scope.label()
        )));
    }

    // The qualifier is not part of the name on disk: `global:staging` names the entry
    // `staging`, and renaming it has to take the qualifier off first.
    let stored = crate::registry::Qualified::parse(from)?.name().to_owned();
    context
        .registries
        .update(scope, |registry| registry.rename(&stored, to.to_owned()))?;

    anstream::println!(
        "{} {} {} {}",
        style::paint("renamed"),
        stored,
        style::dim("→"),
        style::paint(to)
    );
    anstream::println!(
        "  {}",
        style::dim("the password is filed under the connection, so it did not move")
    );
    Ok(Exit::Success)
}

// ---------------------------------------------------------------------------------------
// The parts the five share
// ---------------------------------------------------------------------------------------

/// A connection being built up, before it is complete enough to be a [`Database`].
///
/// One of these whether the details came from a URL, from flags, or from an existing
/// record being edited — which is what makes the three indistinguishable afterwards.
#[derive(Default)]
struct Draft {
    engine: Option<Engine>,
    host: Option<String>,
    port: Option<u16>,
    database: Option<String>,
    user: Option<String>,
}

impl Draft {
    /// Everything that is there, and a sentence about whatever is not.
    fn into_database(self, password: Route) -> Outcome<Database> {
        let need = |what: &str, flag: &str| {
            Failure::usage(format!("no {what} was given")).hint(format!(
                "pass {flag}, or give the whole connection with --url \
                 postgres://user@host:5432/database"
            ))
        };

        let engine = self.engine.ok_or_else(|| need("engine", "--engine"))?;

        Ok(Database {
            host: self.host.ok_or_else(|| need("host", "--host"))?,
            port: self.port.unwrap_or_else(|| engine.default_port()),
            database: self
                .database
                .ok_or_else(|| need("database", "--database"))?,
            user: self.user.ok_or_else(|| need("role", "--user"))?,
            engine,
            password,
        })
    }
}

/// Build a draft from, in order: what is already registered, then the URL, then the flags.
///
/// **The flags win**, because they are the more specific instruction — the same reason
/// `-C` outranks `SLOOP_PROJECT`. It also means a URL with a password in it can be
/// corrected without retyping the parts that were right.
fn draft(
    existing: Option<&Database>,
    url: Option<&str>,
    fields: &Fields,
) -> Outcome<(Draft, Option<Secret>)> {
    let mut draft = Draft::default();
    let mut from_url = None;

    if let Some(existing) = existing {
        draft.engine = Some(existing.engine);
        draft.host = Some(existing.host.clone());
        draft.port = Some(existing.port);
        draft.database = Some(existing.database.clone());
        draft.user = Some(existing.user.clone());
    }

    if let Some(url) = url {
        let parsed = crate::registry::url::parse(url)?;
        draft.engine = Some(parsed.engine);
        draft.host = Some(parsed.host);
        // A URL without a port means "the engine's default", not "keep the old one":
        // moving a record to a new host and keeping the old host's odd port would be a
        // connection nobody asked for.
        draft.port = parsed.port;
        if parsed.database.is_some() {
            draft.database = parsed.database;
        }
        if parsed.user.is_some() {
            draft.user = parsed.user;
        }
        from_url = parsed.password;
    }

    if let Some(engine) = fields.engine.as_deref() {
        draft.engine = Some(Engine::parse(engine)?);
    }
    if let Some(host) = fields.host.clone() {
        draft.host = Some(host);
    }
    if let Some(port) = fields.port {
        draft.port = Some(port);
    }
    if let Some(database) = fields.database.clone() {
        draft.database = Some(database);
    }
    if let Some(user) = fields.user.clone() {
        draft.user = Some(user);
    }

    Ok((draft, from_url))
}

/// Which of the four routes this registration will use.
fn route_for(chosen: &PasswordSource, existing: Option<&Route>) -> Outcome<Route> {
    let route = if chosen.keyring {
        Route::Keyring
    } else if chosen.encrypted_file {
        Route::EncryptedFile
    } else if let Some(variable) = chosen.env.as_deref() {
        // Parsed through the same reader the registry file uses, so `--env PGPASSWORD`
        // and a hand-edited `password = "${PGPASSWORD}"` cannot disagree about what is a
        // legal variable name.
        Route::parse(&format!("${{{variable}}}"))?
    } else if let Some(command) = chosen.password_from.as_deref() {
        Route::parse(&format!("command:{command}"))?
    } else {
        // Nothing chosen: keep what the record already said, and default a new one to the
        // keyring, which is the route that needs no configuration.
        return Ok(existing.cloned().unwrap_or(Route::Keyring));
    };

    Ok(route)
}

/// Get the password, if this route is one that has to hold one.
///
/// `None` for `${VAR}` and `command:`, where the route *is* the answer and there is
/// nothing for sloop to keep.
fn secret_for(
    route: &Route,
    source: &PasswordSource,
    from_url: Option<Secret>,
    database: &Database,
    context: &Context<'_>,
) -> Outcome<Option<Secret>> {
    if !route.is_stored() {
        // Try it once, so a variable that is not set or a command that is not installed is
        // mentioned now rather than discovered during a backup.
        //
        // **A note and not a refusal, deliberately.** These two routes exist for machines
        // that are configured later than they are registered: a provisioning step adds the
        // database and the scheduler supplies `CI_DB_PASSWORD` at run time, and a `db add`
        // that insisted the variable already be set would make the useful case impossible.
        // `--test` is the way to say "and prove it works", and that path does fail.
        let key = database.credential_key();
        let sealed = context
            .registries
            .sealed_in(context.registries.writes_to())
            .unwrap_or_default();

        match resolve(
            route,
            &Lookup {
                key: &key,
                sealed_file: &sealed,
            },
        ) {
            Ok(resolved) => {
                for note in &resolved.notes {
                    anstream::eprintln!("{}", style::dim(note));
                }
            }
            Err(failure) => anstream::eprintln!(
                "{}",
                style::dim(&format!(
                    "note: {} — registered anyway, because {} is read on every run rather                      than kept here",
                    failure.message(),
                    route.describe()
                ))
            ),
        }

        return Ok(None);
    }

    if let Some(secret) = from_url {
        // Said plainly rather than refused. See the module comment: by the time this
        // runs, that password has already been in `ps` and is already in the history.
        anstream::eprintln!(
            "{}",
            style::dim(
                "note: the password came from the URL, so it was visible in `ps` and is in \
                 your shell history while that command line lives. sloop has filed it and \
                 will not write it anywhere — `--password-stdin` avoids the exposure next time."
            )
        );
        return Ok(Some(secret));
    }

    let secret = if source.password_stdin {
        from_stdin()?
    } else if std::io::stdin().is_terminal() {
        let typed =
            rpassword::prompt_password(format!("Password for {}: ", database.credential_key()))
                .map_err(|error| Failure::usage(format!("could not read the password: {error}")))?;
        Secret::new(typed)
    } else {
        // Rule 4, and the flag is named because an error that does not say what to do
        // instead is an error somebody has to go and look up.
        return Err(Failure::new(
            Exit::Usage,
            "a password is needed and there is no terminal to ask at",
        )
        .hint(
            "pipe it in with --password-stdin, or register the database with --env VARIABLE \
             or --password-from <command> so that nothing has to be stored",
        ));
    };

    for note in secret.notes() {
        anstream::eprintln!("{}", style::dim(&note));
    }

    Ok(Some(secret))
}

/// Read a password from standard input.
///
/// **One trailing newline comes off and nothing else does.** A password is allowed to end
/// in a space — `Secret::notes` warns about it rather than fixing it — so trimming would
/// silently register something different from what was piped in. Everything is read, not
/// just the first line, because a `\n` inside a password is legal even if it is unusual.
fn from_stdin() -> Outcome<Secret> {
    let mut typed = String::new();
    std::io::stdin()
        .read_to_string(&mut typed)
        .map_err(|error| Failure::usage(format!("could not read the password: {error}")))?;

    if typed.ends_with('\n') {
        typed.pop();
        if typed.ends_with('\r') {
            typed.pop();
        }
    }

    Ok(Secret::new(typed))
}

/// Fetch the password a record already had, so an edit can move it rather than ask again.
///
/// `None` rather than an error on every failure, deliberately: the old route may be a
/// `${VAR}` holding nothing that is sloop's to move, the keyring may have no entry because
/// the record was written before a password ever was, or the encrypted file may not exist.
/// All three mean the same thing here — there is nothing to carry — and the caller asks
/// for a password instead, which is more use than a stack of excuses.
fn carry_over(before: &Database, context: &Context<'_>, scope: Scope) -> Option<Secret> {
    if !before.password.is_stored() {
        return None;
    }

    let key = before.credential_key();
    let sealed = context.registries.sealed_in(scope).unwrap_or_default();

    resolve(
        &before.password,
        &Lookup {
            key: &key,
            sealed_file: &sealed,
        },
    )
    .ok()
    .map(|resolved| resolved.secret)
}

/// Connect, using whichever password applies.
fn connect(
    database: &Database,
    context: &Context<'_>,
    scope: Scope,
) -> Outcome<crate::engine::ServerInfo> {
    let key = database.credential_key();
    let route = database.password.overridden_by(context.password_command);
    let sealed = context.registries.sealed_in(scope).unwrap_or_default();

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

    adapter_for(database.engine).probe(&database.target(&resolved.secret))
}

/// Connect with a password that is in hand rather than stored, for `--test` on a record
/// that has not been written yet.
fn probe(
    database: &Database,
    secret: Option<&Secret>,
    context: &Context<'_>,
) -> Outcome<crate::engine::ServerInfo> {
    match secret {
        Some(secret) => {
            let target = Target {
                engine: database.engine,
                host: &database.host,
                port: database.port,
                database: &database.database,
                user: &database.user,
                password: secret,
            };
            adapter_for(database.engine).probe(&target)
        }
        // A `${VAR}` or `command:` route: nothing was kept, so go and ask for it the same
        // way every later run will.
        None => connect(database, context, context.registries.writes_to()),
    }
}

fn announce_server(server: &crate::engine::ServerInfo) {
    anstream::println!(
        "  {} {}{}",
        style::paint(&format!("{}", server.engine)),
        server.version,
        style::dim(if server.tls {
            ", over TLS"
        } else {
            ", not encrypted"
        })
    );
}

/// File the password, then the record — and undo the first if the second fails.
///
/// **The order is the recoverable one.** A stored password with no record pointing at it
/// is invisible clutter; a record with no password is a database that looks registered and
/// fails on the next backup. So the record goes last, and a failure there takes the
/// credential back out rather than leaving the pair half made.
fn write(
    context: &mut Context<'_>,
    scope: Scope,
    name: &str,
    database: &Database,
    secret: Option<&Secret>,
    retire: Option<Retiring>,
) -> Outcome<()> {
    let key = database.credential_key();

    if let Some(secret) = secret {
        store(&database.password, &key, secret, context, scope)?;
    }

    let stored = crate::registry::Qualified::parse(name)?.name().to_owned();
    let entry = database.clone();
    let saved = context
        .registries
        .update(scope, move |registry| Ok(registry.insert(stored, entry)));

    if saved.is_err() && secret.is_some() {
        let _ = forget(&database.password, &key, context, scope);
    }
    saved?;

    // Only once the new record is safely on disk. A password nothing references any more
    // is clutter at best, and clearing it before the write would have been clutter plus a
    // lost password if the write then failed.
    if let Some(old) = retire {
        if let Err(failure) = forget(&old.route, &old.key, context, scope) {
            anstream::eprintln!(
                "{}",
                style::dim(&format!(
                    "the password under the old key could not be removed: {}",
                    failure.message()
                ))
            );
        }
    }

    Ok(())
}

/// A password that is no longer referenced, and where it lives.
///
/// **Both halves, and the route is the half that was a bug.** Clearing the old key through
/// the *new* route looks right and is not: `db edit --encrypted-file` on a keyring record
/// writes the password into the file and then asks the file to forget a key it never had,
/// while the keyring quietly keeps the password for a connection that no longer exists.
struct Retiring {
    /// Where the old password was kept.
    route: Route,
    /// What it was filed under.
    key: String,
}

impl Retiring {
    /// What a change from `before` to `after` leaves behind, if anything.
    ///
    /// A move of the key or a change of route both orphan the old entry; a change of
    /// neither leaves nothing to do. The new route is not consulted at all — even a move
    /// to `${VAR}`, which stores nothing, has to clear what the old route was holding.
    fn between(before: &Database, after: &Database) -> Option<Self> {
        let key = before.credential_key();
        let orphaned = key != after.credential_key() || before.password != after.password;

        (orphaned && before.password.is_stored()).then(|| Self {
            route: before.password.clone(),
            key,
        })
    }
}

fn store(
    route: &Route,
    key: &str,
    secret: &Secret,
    context: &Context<'_>,
    scope: Scope,
) -> Outcome<()> {
    match route {
        Route::Keyring => crate::secret::os_keyring::set(key, secret),
        Route::EncryptedFile => {
            let path = context.registries.sealed_in(scope).ok_or_else(|| {
                Failure::usage("there is nowhere to put the encrypted password file")
            })?;
            crate::secret::sealed::put(&path, key, secret)
        }
        // Nothing to store: the route is the answer.
        Route::Environment(_) | Route::Command(_) => Ok(()),
    }
}

fn forget(route: &Route, key: &str, context: &Context<'_>, scope: Scope) -> Outcome<()> {
    match route {
        Route::Keyring => crate::secret::os_keyring::delete(key),
        Route::EncryptedFile => {
            let path = context.registries.sealed_in(scope).ok_or_else(|| {
                Failure::usage("there is nowhere to look for the encrypted password file")
            })?;
            crate::secret::sealed::forget(&path, key)
        }
        Route::Environment(_) | Route::Command(_) => Ok(()),
    }
}

// ---------------------------------------------------------------------------------------
// remove
// ---------------------------------------------------------------------------------------

/// Forget a registered database.
///
/// **It does not touch the server, and that is the entire difference from `db drop`.** Two
/// commands rather than one with a flag, because the two are not degrees of the same thing:
/// one edits a file on this machine and the other destroys somebody's data. A flag is too
/// easy to type by accident for a distinction that large.
///
/// The stored password goes with the record. A credential nothing references is the clutter
/// `db edit` was leaking until R7, and keeping it would be keeping a password for a
/// database sloop no longer knows about.
pub fn remove(context: &mut Context<'_>, name: &str, yes: bool) -> Outcome<Exit> {
    let (scope, record) = context.registries.find(name)?;
    let record = record.clone();

    anstream::println!(
        "{} {}  {}",
        style::dim("forgetting"),
        style::paint(name),
        style::dim(&record.credential_key())
    );
    anstream::println!(
        "  {}",
        style::dim(&format!(
            "the {} registry, and the password kept in {}",
            scope.label(),
            record.password.describe()
        ))
    );
    anstream::println!(
        "  {}",
        style::dim("the database itself is not touched — that is `sloop db drop`")
    );

    if !confirmed(yes, "Forget it?", "--yes")? {
        anstream::println!("{}", style::dim("left alone."));
        return Ok(Exit::Success);
    }

    // The record first, which is the opposite order from `write`. A password with no record
    // is clutter; a record whose password has been deleted is a database that looks
    // registered and cannot connect — so the half that makes it unreachable goes last.
    let stored = crate::registry::Qualified::parse(name)?.name().to_owned();
    context
        .registries
        .update(scope, move |registry| Ok(registry.remove(&stored)))?;

    if record.password.is_stored() {
        if let Err(failure) = forget(&record.password, &record.credential_key(), context, scope) {
            anstream::eprintln!(
                "{}",
                style::dim(&format!(
                    "the record is gone; its password could not be removed: {}",
                    failure.message()
                ))
            );
        }
    }

    anstream::println!("{} {}", style::paint("forgot"), style::paint(name));
    Ok(Exit::Success)
}

// ---------------------------------------------------------------------------------------
// drop
// ---------------------------------------------------------------------------------------

/// Destroy a database on the server, after backing it up.
pub fn drop(
    context: &mut Context<'_>,
    name: &str,
    confirm: Option<&str>,
    no_backup: bool,
) -> Outcome<Exit> {
    let (scope, record) = context.registries.find(name)?;
    let record = record.clone();

    // **A typo is caught before a socket is opened.** `--confirm` is a string comparison
    // and cannot be made more certain by connecting first, so a script that names the
    // wrong database gets told so without sloop touching a server at all. The interactive
    // prompt is the other way round — it comes after the probe, because somebody typing a
    // name by hand should be looking at how many rows are about to go when they do.
    //
    // **Rule 4 is checked here too, and for the same reason.** With no terminal and no
    // `--confirm`, this run cannot finish however well everything else goes — so it says
    // so before fetching a password and opening a connection it was only ever going to
    // refuse to use. It also makes the error the same one every time, rather than
    // whichever step happened to fail first.
    match confirm {
        None if !std::io::stdin().is_terminal() => {
            return Err(Failure::new(
                Exit::Usage,
                "dropping a database needs its name typed, and there is no terminal to type at",
            )
            .hint(format!(
                "pass --confirm {} to say it up front",
                record.database
            )));
        }
        Some(given) if given != record.database => {
            return Err(Failure::new(
                Exit::Usage,
                format!(
                    "--confirm says {given}, and the database is {}",
                    record.database
                ),
            )
            .hint("nothing was contacted and nothing was changed. The two have to match exactly"));
        }
        // `--confirm` matched, or there is a terminal to ask at.
        _ => {}
    }

    // A name that resolves in the registry is not evidence that the database is there, and
    // "about to destroy X" had better be true before it is printed.
    let key = record.credential_key();
    let route = record.password.overridden_by(context.password_command);
    let sealed = context.registries.sealed_in(scope).unwrap_or_default();
    let resolved = resolve(
        &route,
        &Lookup {
            key: &key,
            sealed_file: &sealed,
        },
    )?;
    let target = record.target(&resolved.secret);
    let adapter = adapter_for(record.engine);
    let server = adapter.probe(&target)?;

    anstream::println!(
        "{} {}",
        style::paint("about to drop"),
        style::paint(&record.database)
    );
    anstream::println!("  {}", style::dim(&key));
    anstream::println!(
        "  {}",
        style::dim(&format!(
            "{} {}, registered here as {name}",
            server.engine, server.version
        ))
    );

    // How much is about to go, in the terms somebody weighs it in. Best effort: a role that
    // cannot count is about to find that out anyway, and refusing to drop because the
    // tables could not be listed would be the wrong way round.
    if let Ok(counts) = adapter.row_counts(&target) {
        let rows: u64 = counts.iter().map(|count| count.rows).sum();
        anstream::println!(
            "  {}",
            style::dim(&format!(
                "{} table(s), {rows} row(s) — all of it",
                counts.len()
            ))
        );
    }

    // **Typed, never clicked.** Rule 5, and what is typed is the database's own name on the
    // server rather than the label: the label is what sloop calls it, and the name is what
    // is about to stop existing.
    if !typed_the_name(&record.database, confirm)? {
        anstream::println!("{}", style::dim("left alone."));
        return Ok(Exit::Success);
    }

    // Before anything is destroyed, and a failure here stops the drop. That ordering is the
    // whole point of the safety copy.
    if no_backup {
        anstream::eprintln!(
            "{}",
            style::dim("--no-backup: nothing is being kept, and this cannot be undone")
        );
    } else {
        let root = context.registries.root_in(scope).ok_or_else(|| {
            Failure::usage("there is nowhere to put the safety backup")
                .hint("run this against a project registry, or the global store")
        })?;
        let kept = safety_backup(adapter.as_ref(), &target, &root, name)?;
        anstream::println!("  {} {}", style::paint("backed up to"), kept.display());
    }

    let ended = adapter.terminate_connections(&target)?;
    if ended > 0 {
        anstream::println!(
            "  {}",
            style::dim(&format!("ended {ended} other connection(s)"))
        );
    }

    adapter.drop_database(&target)?;
    anstream::println!("{} {}", style::paint("dropped"), record.database);
    anstream::println!(
        "  {}",
        style::dim(&format!(
            "{name} is still registered and now points at nothing — `sloop db remove {name}` \
             forgets it"
        ))
    );

    Ok(Exit::Success)
}

/// Dump the whole database into the settled backup layout, before it is destroyed.
///
/// Returns where it went, which is the one thing somebody wants from this command five
/// minutes after running it.
fn safety_backup(
    adapter: &dyn crate::engine::Adapter,
    target: &Target<'_>,
    root: &Path,
    label: &str,
) -> Outcome<std::path::PathBuf> {
    let taken = crate::backup::stamp::Stamp::now();
    let directory = crate::backup::directory_for(root, target.engine, label, taken);
    let file = crate::backup::dump_file(&directory);

    anstream::println!(
        "  {}",
        style::dim(&format!("dumping first, to {}", file.display()))
    );

    let summary = adapter.dump(target, &file)?;
    anstream::println!(
        "  {}",
        style::dim(&format!(
            "{} bytes in {:.1}s, taken {}",
            summary.bytes,
            summary.took.as_secs_f64(),
            taken.readable_utc()
        ))
    );

    Ok(file)
}

// ---------------------------------------------------------------------------------------
// Asking
// ---------------------------------------------------------------------------------------

/// A yes-or-no question, for the things that do not destroy data.
///
/// Rule 4: with no terminal there is nobody to ask, so it exits `2` naming the flag that
/// would have answered instead of waiting for somebody who is not there.
fn confirmed(already: bool, question: &str, flag: &str) -> Outcome<bool> {
    if already {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        return Err(Failure::new(
            Exit::Usage,
            format!("{question} — and there is no terminal to ask at"),
        )
        .hint(format!("pass {flag} to answer it up front")));
    }

    anstream::print!("{} {question} ", style::paint("?"));
    let _ = std::io::Write::flush(&mut std::io::stdout());

    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|error| Failure::usage(format!("could not read the answer: {error}")))?;

    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// The confirmation for something that destroys data: the name, typed out.
///
/// **Rule 5, and it holds in a script too.** `--confirm <DATABASE>` is the same typing done
/// in advance, so automation still has to name what it is destroying — there is no spelling
/// of "yes, whichever database that was". Compared exactly, because a database name is
/// case-sensitive on most of the platforms this runs against, and "close enough" is not a
/// standard to destroy data by.
fn typed_the_name(expected: &str, given: Option<&str>) -> Outcome<bool> {
    // Already checked, before anything was contacted — see the top of `drop`. Reaching
    // here with a value at all means it matched.
    if given.is_some() {
        return Ok(true);
    }

    if !std::io::stdin().is_terminal() {
        return Err(Failure::new(
            Exit::Usage,
            "dropping a database needs its name typed, and there is no terminal to type at",
        )
        .hint(format!("pass --confirm {expected} to say it up front")));
    }

    anstream::print!(
        "{} type {} to destroy it, or anything else to stop: ",
        style::paint("?"),
        style::paint(expected)
    );
    let _ = std::io::Write::flush(&mut std::io::stdout());

    let mut typed = String::new();
    std::io::stdin()
        .read_line(&mut typed)
        .map_err(|error| Failure::usage(format!("could not read the answer: {error}")))?;

    // Only the line ending comes off. A name with a trailing space is a name somebody would
    // have to type a trailing space for, and trimming would quietly accept a different name
    // than the one on the server.
    Ok(typed.trim_end_matches(['\n', '\r']) == expected)
}

#[cfg(test)]
#[path = "db_tests.rs"]
mod tests;
