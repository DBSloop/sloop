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

use crate::cli::{Fields, PasswordSource, SshFields};
use crate::consent::{Consent, Destroying};
use crate::engine::{Engine, Provisioning, Target};
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::registry::file::{Database, check_name};
use crate::registry::{Registries, Scope};
use crate::secret::{Lookup, Route, Secret, resolve};
use crate::ssh::tunnel::Tunnels;
use crate::ssh::{DEFAULT_PORT as SSH_PORT, Reach, Server, Through};
use crate::style;

/// Everything a `db` command needs from the outside.
pub struct Context<'a> {
    /// Both registries, already open.
    pub registries: Registries,
    /// `--password-command`, which outranks whatever route a record names.
    pub password_command: Option<&'a str>,
    /// The global store, for the sentence that says which registry was read.
    pub global: &'a Path,
    /// What this run was given permission to do — see [`crate::consent`].
    pub consent: Consent<'a>,
    /// Every SSH forward this session holds — see [`super::reach`]. Shared with every
    /// other command in the run, so a menu session authenticates once.
    pub tunnels: &'a Tunnels,
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
    ssh: &SshFields,
    test: bool,
) -> Outcome<Exit> {
    check_name(name)?;

    let scope = context.registries.writes_to();
    let replacing = context
        .registries
        .in_scope(scope)
        .and_then(|registry| registry.get(name))
        .cloned();

    if replacing.is_some() && !context.consent.forced() {
        return Err(Failure::usage(format!(
            "{name} is already registered in the {} registry",
            scope.label()
        ))
        .hint("`sloop db edit` changes it, and --force replaces it outright"));
    }

    let (draft, from_url) = draft(None, url, fields, ssh)?;
    let route = route_for(password, None)?;
    let database = draft.into_database(route.clone())?;

    // Everything that can fail without leaving a trace happens before anything is written:
    // the password is fetched, the connection is tried, and only then does the machine
    // change. A half-registered database is worse than an unregistered one.
    let secret = secret_for(&route, password, from_url, &database, context)?;
    let passphrase = ssh_secret_for(&database, ssh, context)?;
    if test {
        let reached = probe(&database, secret.as_ref(), context)?;
        announce_server(&reached.server, reached.at.through.as_deref());
    }

    if crate::report::would(&format!("register {name}")) {
        return Ok(Exit::Success);
    }

    // Replacing a record orphans whatever the old one pointed at, exactly as an edit does.
    let retire = replacing
        .as_ref()
        .map(|existing| Retiring::between(existing, &database))
        .unwrap_or_default();
    write(
        &mut context.registries,
        scope,
        name,
        &database,
        &Secrets {
            password: secret.as_ref(),
            ssh: passphrase.as_ref(),
        },
        &retire,
    )?;

    crate::report::result(serde_json::json!({
        "name": name,
        "registry": scope.label(),
        "connection": database.credential_key(),
        "password": route.describe(),
        "ssh": database.reach.through().map(Through::describe),
    }));
    crate::say!(
        "{} {} in the {} registry {}",
        style::paint("registered"),
        style::paint(name),
        scope.label(),
        style::dim(&format!(
            "({}), password from {}",
            context.registries.resolution().why(),
            route.describe()
        ))
    );
    crate::say!("  {}", style::dim(&database.credential_key()));
    announce_reach(&database);
    Ok(Exit::Success)
}

/// The sentence everybody needs once, printed where it cannot be missed.
///
/// **Said on the way in rather than only in `--help`.** A registration whose `--host` is the
/// laptop's idea of the address instead of the server's produces a tunnel to nowhere and an
/// error about the *database*, which is the least useful place to find out. So the record is
/// read back in words at the moment it is written.
fn announce_reach(database: &Database) {
    let Some(through) = database.reach.through() else {
        return;
    };

    crate::say!("  {}", style::dim(&through.describe()));
    crate::say!(
        "  {}",
        style::dim(&format!(
            "{}:{} is as {} sees it, not as this machine does",
            database.host, database.port, through.server.host
        ))
    );
}

// ---------------------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------------------

/// Everything `db create` was asked for, named rather than positional.
///
/// Ten arguments in a row is ten chances to pass the superuser's name where the new role's
/// goes, on a command whose whole job is to create one and use the other.
pub struct Creating<'a> {
    /// What to register it as.
    pub name: &'a str,
    /// Which engine, as typed.
    pub engine: &'a str,
    /// The server.
    pub host: &'a str,
    /// Its port, or the engine's default.
    pub port: Option<u16>,
    /// The account to create it with, or the engine's usual superuser.
    pub superuser: Option<&'a str>,
    /// Take that account's password from standard input.
    pub superuser_password_stdin: bool,
    /// Run this for that account's password.
    pub superuser_password_command: Option<&'a str>,
    /// The database's own name, or the label.
    pub database: Option<&'a str>,
    /// The role to create, or the database's name.
    pub role: Option<&'a str>,
    /// Take the new role's password from standard input rather than generating one.
    pub role_password_stdin: bool,
    /// Run this for the new role's password rather than generating one.
    pub role_password_command: Option<&'a str>,
}

/// The same thing, with the engine already settled rather than still a string.
///
/// **`mirror --create` never asks which engine**, because a copy is of something: the
/// destination is whatever the source is, and a flag that could disagree with that is a flag
/// that can produce a mirror which cannot be restored. So the one field `db create` reads
/// from the command line is the one field this does not have — see [`build`].
pub struct Building<'a> {
    /// What to register it as.
    pub name: &'a str,
    /// Which engine. Typed by `db create`, taken from the source by `mirror --create`.
    pub engine: Engine,
    /// The server.
    pub host: &'a str,
    /// Its port, or the engine's default.
    pub port: Option<u16>,
    /// The account to create it with, or the engine's usual superuser.
    pub superuser: Option<&'a str>,
    /// Take that account's password from standard input.
    pub superuser_password_stdin: bool,
    /// Run this for that account's password.
    pub superuser_password_command: Option<&'a str>,
    /// The database's own name, or the label.
    pub database: Option<&'a str>,
    /// The role to create, or the database's name.
    pub role: Option<&'a str>,
    /// Take the new role's password from standard input rather than generating one.
    pub role_password_stdin: bool,
    /// Run this for the new role's password rather than generating one.
    pub role_password_command: Option<&'a str>,
}

/// What [`build`] left on the server and in the registry, or `None` under `--dry-run`.
///
/// **The password comes back rather than being looked up again.** `mirror --create` needs to
/// connect as the role it just made, and reading it straight back out of the keyring would
/// turn one more failure — a locked keychain, a headless box — into a database that exists,
/// is registered, and cannot be copied into by the command that made it.
pub struct Built {
    /// The record as it was registered.
    pub record: Database,
    /// The new role's password, still in hand.
    pub secret: Secret,
}

/// Create a database, the role that owns it, and the grants that make the two usable.
///
/// **Two passwords, treated completely differently, and that is the whole shape of this
/// command.** The account it creates *with* is used for one connection and forgotten — never
/// written, never logged, never in `argv`. The password of the role being *created* is
/// generated, filed where this machine keeps secrets, and printed once so it can be put into
/// an application.
///
/// The work itself is one call per engine — see [`crate::engine::Adapter::provision`] —
/// because what "usable" means is PostgreSQL's business on PostgreSQL and MySQL's on MySQL,
/// while the flags a person types stay identical.
pub fn create(context: &mut Context<'_>, asked: &Creating<'_>) -> Outcome<Exit> {
    let engine = Engine::parse(asked.engine)?;
    build(
        &mut context.registries,
        context.global,
        context.consent,
        &Building {
            name: asked.name,
            engine,
            host: asked.host,
            port: asked.port,
            superuser: asked.superuser,
            superuser_password_stdin: asked.superuser_password_stdin,
            superuser_password_command: asked.superuser_password_command,
            database: asked.database,
            role: asked.role,
            role_password_stdin: asked.role_password_stdin,
            role_password_command: asked.role_password_command,
        },
    )?;
    Ok(Exit::Success)
}

/// The body of `db create`, reachable from any command that needs a database to exist.
///
/// **Three borrowed pieces rather than a [`Context`]**, because `mirror --create` is holding
/// a context of its own and the alternative is two commands passing one struct back and
/// forth around a call. These are exactly what creating a database touches: the registries
/// it is written to, the store the client tools live in, and whether a refusal was overridden
/// on purpose.
pub fn build(
    registries: &mut Registries,
    global: &Path,
    consent: Consent<'_>,
    asked: &Building<'_>,
) -> Outcome<Option<Built>> {
    check_name(asked.name)?;

    let engine = asked.engine;
    let port = asked.port.unwrap_or_else(|| engine.default_port());
    let Naming { database, role } = name_it(asked.name, asked.database, asked.role)?;
    let superuser = asked
        .superuser
        .map_or_else(|| usual_superuser(engine).to_owned(), str::to_owned);

    let scope = registries.writes_to();
    if registries
        .in_scope(scope)
        .and_then(|registry| registry.get(asked.name))
        .is_some()
        && !consent.forced()
    {
        return Err(Failure::usage(format!(
            "{} is already registered in the {} registry",
            asked.name,
            scope.label()
        ))
        .hint("--force replaces the record; the database this creates would be a new one"));
    }

    // **Everything missing, named at once, and before a word about what was going to
    // happen.** Two secrets are needed and only one of them can come down a pipe, so an
    // unattended run has a shape it has to take — and being told half of it, twice, under a
    // heading announcing work that is not going to happen, is the kind of thing that makes
    // a tool feel hostile.
    unattended_needs(asked)?;

    announce_what_is_being_made(engine, &role, asked.host, port, &database, &superuser);

    // The new role's password before anything is contacted: a run that cannot finish should
    // not first ask somebody for a superuser password.
    let (role_password, generated) = role_password(asked)?;
    let admin_password = superuser_password(asked, &superuser, asked.host, port)?;

    // **Everything above this line is reading and checking.** A rehearsal does all of it —
    // the name, the registry, the flags a run with no terminal is missing — and stops here.
    if crate::report::would(&format!(
        "create {}, owned by {role}",
        crate::engine::connection_string(engine, &role, asked.host, port, &database)
    )) {
        return Ok(None);
    }

    let adapter = super::adapter_for(engine, global);
    let maintenance = Target {
        engine,
        host: asked.host,
        port,
        database: engine.maintenance_database(),
        user: &superuser,
        password: &admin_password,
    };
    let server = adapter.probe(&maintenance)?;
    // `None`: `db create` makes a database on a server it can already reach, which is why
    // the record it writes below is `Reach::Direct`.
    announce_server(&server, None);

    let done = adapter.provision(
        &maintenance,
        &Provisioning {
            database: &database,
            role: &role,
            password: &role_password,
        },
    )?;

    announce_created(&database, &role, &done);

    // Registered last, once the server has actually done the work: a record pointing at a
    // database that does not exist is worse than no record.
    let route = kept_where();
    let record = Database {
        engine,
        host: asked.host.to_owned(),
        port,
        database: database.clone(),
        user: role.clone(),
        password: route.clone(),
        // `db create` makes a database on a server it can already reach, so there is no
        // tunnel to record. Creating one on the far side of an SSH server is a different
        // command's problem and not something to half-build here.
        reach: Reach::Direct,
    };
    write(
        registries,
        scope,
        asked.name,
        &record,
        &Secrets::just_the_password(&role_password),
        &[],
    )?;

    crate::say!(
        "  {}",
        style::dim(&format!(
            "registered as {} in the {} registry ({}), password in {}",
            asked.name,
            scope.label(),
            registries.resolution().why(),
            route.describe()
        ))
    );

    crate::report::result(serde_json::json!({
        "name": asked.name,
        "registry": scope.label(),
        "engine": engine.to_string(),
        "host": asked.host,
        "port": port,
        "database": database,
        "user": role,
        "role_existed": done.role_existed,
        "grants": done.grants,
        "password": route.describe(),
        // **The password itself is never a field.** Rule 3 does not have a JSON exception,
        // and a document is the easiest thing in this program to redirect into a file.
        "password_printed": generated && !done.role_existed,
    }));

    if generated && !done.role_existed {
        print_once(&role, &role_password, &route);
    }

    Ok(Some(Built {
        record,
        secret: role_password,
    }))
}

/// What the database and its owner are going to be called on the server.
pub struct Naming {
    /// The database's own name there.
    pub database: String,
    /// The role that will own it and connect as.
    pub role: String,
}

/// Settle both names, **asking for whichever was not given**.
///
/// **The label is what sloop files it under; these two are what exist on the server, and a
/// user who is never asked never finds that out.** `sloop db create --engine postgres orders`
/// used to make a database called `orders` owned by a role called `orders` without a word
/// about either — the owner's words: *"it must ask db user name, currently i see that it is
/// creating a role/user exactly same as db name"*. So it asks, with the old behaviour as the
/// default: pressing Enter twice is what the command used to do on its own, and now it is a
/// choice somebody made.
///
/// **Nothing is asked when there is no terminal.** Rule 4, and it is why the defaults stay
/// exactly what they were: a cron line that never named a role still gets one, and `--role`
/// is there for the one that wants to.
pub fn name_it(label: &str, database: Option<&str>, role: Option<&str>) -> Outcome<Naming> {
    let asking = std::io::stdin().is_terminal();

    let database = match database {
        Some(given) => given.to_owned(),
        None if asking => ask_for("Name the database itself", label)?,
        None => label.to_owned(),
    };
    let role = match role {
        Some(given) => given.to_owned(),
        None if asking => ask_for("Name the user that will own it", &database)?,
        None => database.clone(),
    };

    Ok(Naming { database, role })
}

/// Ask for one name, showing what will be used if nobody types one.
///
/// **The default is printed, not hidden.** A prompt that silently accepts something the user
/// cannot see is the defect this exists to fix, so the answer is either what they typed or
/// what they could read while deciding not to type anything.
fn ask_for(question: &str, default: &str) -> Outcome<String> {
    crate::report::ask(&format!(
        "{} {question} {} ",
        style::paint("?"),
        style::dim(&format!("[{default}]"))
    ));

    let mut given = String::new();
    std::io::stdin()
        .read_line(&mut given)
        .map_err(|error| Failure::usage(format!("could not read the answer: {error}")))?;

    answer_or_default(&given, default)
}

/// What a typed line means: the name in it, or the default when there is nothing in it.
///
/// **Split from the prompt so it can be tested.** The half that reads a terminal needs a
/// terminal; the half that decides what the line meant is where being wrong would be
/// expensive — an answer of two spaces silently becoming a database named two spaces, say.
fn answer_or_default(given: &str, default: &str) -> Outcome<String> {
    let given = given.trim();
    if given.is_empty() {
        return Ok(default.to_owned());
    }
    check_server_name(given).map(str::to_owned)
}

/// Check a name that is going to exist on a server rather than in the registry.
///
/// **Looser than [`check_name`] and for a different reason.** A label has to be typed back at
/// a command line, so it may not carry a colon; a database's own name only has to be quotable
/// — every engine here takes an identifier with almost anything in it, and refusing one
/// because sloop found it unusual would be sloop deciding what somebody may call their
/// database. What is refused is what would be invisible: nothing at all, or a name padded
/// with spaces that no listing would ever show.
fn check_server_name(name: &str) -> Outcome<&str> {
    let refuse = |why: &str| {
        Failure::usage(format!("{name} cannot be a name on the server: {why}"))
            .hint("letters, digits and the usual punctuation, with no stray spaces")
    };

    if name.is_empty() {
        return Err(refuse("it is empty"));
    }
    if name.trim() != name {
        return Err(refuse("it starts or ends with whitespace"));
    }
    if let Some(bad) = name.chars().find(|letter| letter.is_control()) {
        return Err(refuse(&format!(
            "{} is not something you could type back",
            bad.escape_debug()
        )));
    }

    Ok(name)
}

/// What is about to be created, and which account is creating it.
fn announce_what_is_being_made(
    engine: Engine,
    role: &str,
    host: &str,
    port: u16,
    database: &str,
    superuser: &str,
) {
    crate::say!(
        "{} {}",
        style::paint("creating"),
        style::dim(&crate::engine::connection_string(
            engine, role, host, port, database
        ))
    );
    crate::say!(
        "  {}",
        style::dim(&format!(
            "as {superuser}, whose password is used once and kept nowhere"
        ))
    );
}

/// Show a generated password, once.
///
/// **Only ever reached at a terminal** — `role_password` refuses to generate one when there
/// is nothing to print it to, because a scheduled run's standard error is a log file and
/// rule 3 does not have an exception for convenience. Standard error rather than standard
/// output, so that a person piping this command's output somewhere does not pipe the
/// password with it.
fn print_once(role: &str, password: &Secret, route: &Route) {
    crate::report::secret("");
    crate::report::secret(&format!(
        "{} {}",
        style::paint("the password for"),
        style::paint(role)
    ));
    crate::report::secret(&format!("  {}", password.expose()));
    crate::report::secret(&format!(
        "  {}",
        style::dim(&format!(
            "it is in {} and this is the only time it is printed — put it in your \
             application now",
            route.describe()
        ))
    ));
}

/// Where a password sloop just generated should live on *this* machine.
///
/// **The keyring, unless this machine has said otherwise.** A headless Linux box with no
/// Secret Service running has no keyring to file anything in, and a `db create` that
/// created a database and a role and then failed at the last step would be the worst
/// possible place to find that out. `SLOOP_PASSPHRASE` being set is how `R3` says "this
/// machine keeps secrets in the encrypted file", and the backup key already chooses the
/// same way — see `crypt::keep_somewhere`.
fn kept_where() -> Route {
    let [first, _] = crate::secret::preferred_routes();
    first
}

/// What the server did, in the order it did it.
fn announce_created(database: &str, role: &str, done: &crate::engine::Provisioned) {
    crate::say!("{} {database}", style::paint("created"));
    if done.role_existed {
        crate::say!(
            "  {}",
            style::dim(&format!(
                "{role} was already on the server, so it keeps the password it had"
            ))
        );
    } else {
        crate::say!("  {}", style::dim(&format!("role {role} created")));
    }
    for grant in &done.grants {
        crate::say!("  {}", style::dim(&format!("granted {grant}")));
    }
}

/// With no terminal, say everything that is missing in one go.
///
/// **Only one secret can come down a pipe**, so an unattended `db create` has exactly one
/// shape: the superuser's password from a command, the new role's from standard input. Being
/// told about one flag, then run again and told about the other, is how a tool earns the
/// reputation this project is trying not to have.
pub fn unattended_needs(asked: &Building<'_>) -> Outcome<()> {
    if std::io::stdin().is_terminal() {
        return Ok(());
    }

    let mut missing: Vec<&str> = Vec::new();
    if !asked.role_password_stdin && asked.role_password_command.is_none() {
        missing.push("--role-password-stdin");
    }
    if !asked.superuser_password_stdin && asked.superuser_password_command.is_none() {
        missing.push("--superuser-password-command");
    }
    if missing.is_empty() {
        return Ok(());
    }

    Err(Failure::new(
        Exit::Usage,
        format!(
            "there is no terminal to ask at, and {} {} missing",
            missing.join(" and "),
            if missing.len() == 1 { "is" } else { "are" }
        ),
    )
    .hint(
        "unattended, it looks like this: --superuser-password-command \"op read \
         op://vault/pg/root\" --role-password-stdin, with the new password piped in. Only          one of the two can use standard input, so the other takes a --…-password-command",
    ))
}

/// The password of the role being created: generated, or taken from standard input.
///
/// **A generated one is only ever printed at a terminal.** Rule 3 says no plaintext password
/// anywhere, and a scheduled run's standard error is a log file — so a run with nothing to
/// print to has to bring its own password, and is told which flag does that. That is also
/// the honest answer: whatever creates a database unattended already has to know the
/// password to configure anything with it.
fn role_password(asked: &Building<'_>) -> Outcome<(Secret, bool)> {
    if let Some(command) = asked.role_password_command {
        let resolved = resolve(
            &Route::Command(command.to_owned()),
            &Lookup {
                key: role_for(asked),
                vault: &crate::secret::sealed::Vault::File(Path::new("")),
            },
        )?;
        return Ok((resolved.secret, false));
    }

    if asked.role_password_stdin {
        return Ok((from_stdin()?, false));
    }

    if !std::io::stdin().is_terminal() {
        return Err(Failure::new(
            Exit::Usage,
            "a generated password could only be printed into a log here, and there is no \
             terminal to print it to",
        )
        .hint(
            "pipe the one you want in with --role-password-stdin, or have a password manager \
             print it with --role-password-command",
        ));
    }

    // **Typed, or generated — and the question is asked rather than assumed.** The owner
    // asked to be able to supply one: *"it will be good if we could also provide the
    // password in another flag so we can ignore generated password"*. At a terminal the
    // honest place for that is a hidden prompt, because the alternative they were reaching
    // for is a flag carrying the value, and every argument of every process on this machine
    // is readable in `ps`. See "Choosing the new user's password" in
    // `docs/OWNER-DECISIONS.md`.
    match crate::secret::typed_twice(role_for(asked))? {
        Some(typed) => Ok((typed, false)),
        None => Ok((Secret::new(crate::secret::generated_password()?), true)),
    }
}

/// The user this is all about, by whichever name it will end up with.
fn role_for<'a>(asked: &'a Building<'a>) -> &'a str {
    asked.role.or(asked.database).unwrap_or(asked.name)
}

/// The password for the account doing the creating.
///
/// It is never stored and never printed. Three ways in, in the order a person would expect:
/// a flag that names a command, standard input, or a hidden prompt — and with no terminal
/// and neither flag, the two flags are named in the error rather than guessed at.
fn superuser_password(
    asked: &Building<'_>,
    superuser: &str,
    host: &str,
    port: u16,
) -> Outcome<Secret> {
    if let Some(command) = asked.superuser_password_command {
        let resolved = resolve(
            &Route::Command(command.to_owned()),
            &Lookup {
                key: superuser,
                vault: &crate::secret::sealed::Vault::File(Path::new("")),
            },
        )?;
        return Ok(resolved.secret);
    }

    if asked.superuser_password_stdin {
        return from_stdin();
    }

    if !std::io::stdin().is_terminal() {
        return Err(Failure::new(
            Exit::Usage,
            format!("{superuser}'s password is needed and there is no terminal to ask at"),
        )
        .hint(
            "pipe it in with --superuser-password-stdin, or have a password manager print it \
             with --superuser-password-command",
        ));
    }

    let typed = rpassword::prompt_password(format!("Password for {superuser}@{host}:{port}: "))
        .map_err(|error| Failure::usage(format!("could not read the password: {error}")))?;
    Ok(Secret::new(typed))
}

/// The account an engine is usually administered as.
const fn usual_superuser(engine: Engine) -> &'static str {
    match engine {
        Engine::Postgres => "postgres",
        Engine::Mysql | Engine::Mariadb => "root",
    }
}

// ---------------------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------------------

/// One registered database, for `--json`.
///
/// **Nothing in here is a secret**, and that is a property of every field rather than a
/// habit: a password is a [`Route`], which is a direction, and every SSH field is something
/// `ps` shows the moment `ssh` runs.
fn as_json(
    scope: Scope,
    name: &str,
    database: &Database,
    context: &Context<'_>,
) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "registry": scope.label(),
        "engine": database.engine.to_string(),
        "host": database.host,
        "port": database.port,
        "database": database.database,
        "user": database.user,
        "password": database.password.overridden_by(context.password_command).describe(),
        "ssh": database.reach.through().map(|through| serde_json::json!({
            "host": through.server.host,
            "port": through.server.port,
            "user": through.server.user,
            "identity": through.server.identity
                .as_ref()
                .map(|path| path.display().to_string()),
            "passphrase": through.secret.as_ref().map(Route::describe),
        })),
    })
}

/// One line of `db list`, gathered before anything is printed so the columns can be
/// measured against every row rather than guessed at.
///
/// The last field is the tunnel, on a line of its own when there is one. It is not a column
/// because it is rare and long: widening every row of a listing for a field most of them do
/// not have is how a table stops being readable.
type Row = (String, Scope, String, String, Option<String>);

/// Show what is registered. Never a secret — see [`Route::describe`].
///
/// It cannot fail, and says so: the registries were read before this was called, and
/// there is no state of a registry that cannot be printed.
pub fn list(context: &Context<'_>) -> Exit {
    if context.registries.is_empty() {
        crate::report::result(serde_json::json!({ "databases": [] }));
        crate::say!(
            "{}",
            style::dim(&format!(
                "Nothing is registered in {}.",
                context.registries.resolution().describe(context.global)
            ))
        );
        crate::say!(
            "{}",
            style::dim("`sloop db add <name> --url postgres://user@host/database` starts one.")
        );
        return Exit::Success;
    }

    crate::report::result(serde_json::json!({
        "databases": context
            .registries
            .all()
            .map(|(scope, name, database)| as_json(scope, name, database, context))
            .collect::<Vec<_>>(),
    }));

    let rows: Vec<Row> = context
        .registries
        .all()
        .map(|(scope, name, database)| {
            (
                name.to_owned(),
                scope,
                // The connection alone, without the `through ssh://…` that
                // `Database::credential_key` appends: the tunnel gets its own line below,
                // where there is room to say what it means.
                crate::engine::connection_string(
                    database.engine,
                    &database.user,
                    &database.host,
                    database.port,
                    &database.database,
                ),
                // A route, never a value. Not one of these phrases can contain a
                // password, which is the property that lets this be printed at all.
                database
                    .password
                    .overridden_by(context.password_command)
                    .describe(),
                database.reach.through().map(Through::describe),
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

    for (name, scope, connection, route, over) in &rows {
        // A project entry shadows a global one of the same name. Saying so is the
        // difference between a confusing listing and an explanation.
        let shadowed = seen.contains(&name.as_str());
        seen.push(name);

        crate::say!(
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

        if let Some(over) = over {
            crate::say!(
                "{}{}  {}",
                " ".repeat(name.chars().count()),
                pad(name, name_column),
                style::dim(over)
            );
        }
    }

    crate::say!();
    crate::say!(
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
        crate::say!(
            "{}",
            style::dim("Nothing is registered, so nothing to test.")
        );
        return Ok(Exit::Success);
    }

    // Like `backup --all`, a failure is reported and the run carries on: knowing that
    // four of five are fine is worth more than stopping at the first one that is not.
    let mut worst = Exit::Success;
    let mut results = Vec::with_capacity(wanted.len());

    for (scope, name, database) in &wanted {
        crate::say!();
        crate::say!(
            "{}  {}  {}",
            style::paint(name),
            database.credential_key(),
            style::dim(scope.label())
        );

        let reached = match connect(database, context, *scope) {
            Ok(server) => {
                announce_server(&server.server, server.at.through.as_deref());
                Some(server)
            }
            Err(failure) => {
                failure.mention();
                if matches!(worst, Exit::Success) {
                    worst = failure.exit();
                }
                None
            }
        };
        results.push(serde_json::json!({
            "name": name,
            "registry": scope.label(),
            "connection": database.credential_key(),
            "reached": reached.is_some(),
            "engine": reached.as_ref().map(|it| it.server.engine.to_string()),
            "version": reached.as_ref().map(|it| it.server.version.to_string()),
            "tls": reached.as_ref().map(|it| it.server.tls),
            // **What actually secured it**, which over a tunnel is not what `tls` says. A
            // script watching a fleet should not have to infer that from two other fields.
            "through": reached.as_ref().and_then(|it| it.at.through.clone()),
        }));
    }

    crate::report::result(serde_json::json!({ "tested": results }));
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
    ssh: &SshFields,
    test: bool,
) -> Outcome<Exit> {
    let (scope, before) = context.registries.find(name)?;
    let before = before.clone();

    let (draft, from_url) = draft(Some(&before), url, fields, ssh)?;
    let route = route_for(password, Some(&before.password))?;
    let after = draft.into_database(route.clone())?;

    if after == before {
        crate::say!("{}", style::dim(&format!("{name} is already like that.")));
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

    let passphrase = ssh_carried_over(&before, &after, ssh, context, scope)?;

    if test {
        let reached = probe(&after, secret.as_ref(), context)?;
        announce_server(&reached.server, reached.at.through.as_deref());
    }

    if crate::report::would(&format!("change what {name} points at")) {
        return Ok(Exit::Success);
    }

    write(
        &mut context.registries,
        scope,
        name,
        &after,
        &Secrets {
            password: secret.as_ref(),
            ssh: passphrase.as_ref(),
        },
        &Retiring::between(&before, &after),
    )?;

    crate::say!(
        "{} {} {}",
        style::paint("changed"),
        style::paint(name),
        style::dim(&format!("in the {} registry", scope.label()))
    );
    crate::say!("  {}", style::dim(&before.credential_key()));
    crate::say!("  {}", after.credential_key());
    announce_reach(&after);
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

    if crate::report::would(&format!("rename {from} to {to}")) {
        return Ok(Exit::Success);
    }

    context
        .registries
        .update(scope, |registry| registry.rename(&stored, to.to_owned()))?;

    crate::report::result(serde_json::json!({ "from": stored, "to": to }));
    crate::say!(
        "{} {} {} {}",
        style::paint("renamed"),
        stored,
        style::dim("→"),
        style::paint(to)
    );
    crate::say!(
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
#[derive(Debug, Default)]
struct Draft {
    engine: Option<Engine>,
    host: Option<String>,
    port: Option<u16>,
    database: Option<String>,
    user: Option<String>,
    reach: Reach,
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
            reach: self.reach,
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
    ssh: &SshFields,
) -> Outcome<(Draft, Option<Secret>)> {
    let mut draft = Draft::default();
    let mut from_url = None;

    if let Some(existing) = existing {
        draft.engine = Some(existing.engine);
        draft.host = Some(existing.host.clone());
        draft.port = Some(existing.port);
        draft.database = Some(existing.database.clone());
        draft.user = Some(existing.user.clone());
        draft.reach = existing.reach.clone();
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

    draft.reach = reach_from(draft.reach, ssh)?;

    Ok((draft, from_url))
}

/// How this database is reached, after the `--ssh-*` flags have had their say.
///
/// **The flags edit what is there rather than replacing it**, the same way `--port` alone
/// leaves the host alone: `db edit prod --ssh-user deploy` changes the account and keeps the
/// server. `--no-ssh` is the one that replaces, and it is a separate flag rather than an
/// empty `--ssh-host ""` because turning a tunnel off is a decision somebody should have to
/// spell.
///
/// A URL has no say here at all. There is no spelling of an SSH server in a PostgreSQL URL,
/// and inventing one would be inventing a format nobody else reads.
fn reach_from(current: Reach, ssh: &SshFields) -> Outcome<Reach> {
    if ssh.no_ssh {
        return Ok(Reach::Direct);
    }

    let nothing_said = ssh.ssh_host.is_none()
        && ssh.ssh_port.is_none()
        && ssh.ssh_user.is_none()
        && ssh.ssh_identity.is_none()
        && !ssh.ssh_keyring
        && !ssh.ssh_encrypted_file
        && ssh.ssh_env.is_none()
        && ssh.ssh_passphrase_from.is_none()
        && !ssh.ssh_passphrase_stdin;

    if nothing_said {
        return Ok(current);
    }

    let mut through = match current {
        Reach::Over(through) => *through,
        // Nothing to edit, so there has to be a server to start from. Naming the flag
        // rather than guessing: `--ssh-user deploy` on a direct database could mean four
        // different servers and sloop knows none of them.
        Reach::Direct => {
            let host = ssh.ssh_host.clone().ok_or_else(|| {
                Failure::usage("--ssh-host says which server to go through, and it was not given")
                    .hint(
                        "`--ssh-host db.example.com`. --host is then the address that \
                         server sees, usually 127.0.0.1",
                    )
            })?;

            Through {
                server: Server {
                    host,
                    port: SSH_PORT,
                    user: None,
                    identity: None,
                },
                secret: None,
            }
        }
    };

    if let Some(host) = ssh.ssh_host.clone() {
        through.server.host = host;
    }
    if let Some(port) = ssh.ssh_port {
        through.server.port = port;
    }
    if let Some(user) = ssh.ssh_user.clone() {
        through.server.user = Some(user);
    }
    if let Some(identity) = ssh.ssh_identity.as_deref() {
        through.server.identity = Some(std::path::PathBuf::from(identity));
    }

    through.secret = ssh_route_for(ssh, through.secret.as_ref())?;

    check_ssh(&through.server)?;
    Ok(Reach::Over(Box::new(through)))
}

/// Where the key's passphrase comes from, or `None` for the agent.
///
/// **`None` is the default and stays the default.** Unlike a database password, which always
/// has to come from somewhere, an SSH key usually needs nothing from sloop at all — the
/// agent holds it, or the key has no passphrase. So a registration says nothing about a
/// passphrase unless one of these flags was passed.
fn ssh_route_for(chosen: &SshFields, existing: Option<&Route>) -> Outcome<Option<Route>> {
    if chosen.ssh_keyring {
        return Ok(Some(Route::Keyring));
    }
    if chosen.ssh_encrypted_file {
        return Ok(Some(Route::EncryptedFile));
    }
    if let Some(variable) = chosen.ssh_env.as_deref() {
        // The same reader the registry uses, so a flag and a hand-written field cannot
        // disagree about what a legal variable name is.
        return Ok(Some(Route::parse(&format!("${{{variable}}}"))?));
    }
    if let Some(command) = chosen.ssh_passphrase_from.as_deref() {
        return Ok(Some(Route::parse(&format!("command:{command}"))?));
    }
    if chosen.ssh_passphrase_stdin {
        // A value is being supplied with no route named for it, so it goes wherever this
        // machine keeps secrets — the same answer `db add` gives for a database password.
        return Ok(Some(existing.cloned().unwrap_or_else(kept_where)));
    }

    Ok(existing.cloned())
}

/// Check the server's own fields, before a record naming it is written.
///
/// Nothing here is about security — `known_hosts` is OpenSSH's and stays OpenSSH's. It is
/// about a record that would produce an `ssh` command line meaning something other than what
/// was typed: a host that is really `-o` would be an option, and a host with a space in it is
/// two arguments.
fn check_ssh(server: &Server) -> Outcome<()> {
    let refuse = |why: &str| {
        Failure::usage(format!("{} cannot be an SSH server: {why}", server.host))
            .hint("a host name or an address, as you would type it after `ssh`")
    };

    if server.host.trim().is_empty() {
        return Err(refuse("it is empty"));
    }
    if server.host.trim() != server.host {
        return Err(refuse("it starts or ends with whitespace"));
    }
    if server.host.contains(char::is_whitespace) {
        return Err(refuse(
            "it has a space in it, so `ssh` would read it as two arguments",
        ));
    }
    if server.host.starts_with('-') {
        return Err(refuse(
            "it starts with a dash, so `ssh` would read it as an option",
        ));
    }
    if let Some(user) = &server.user
        && (user.trim() != user || user.is_empty() || user.contains(char::is_whitespace))
    {
        return Err(Failure::usage(format!("{user} cannot be an SSH username"))
            .hint("the account on the server, with no spaces around it"));
    }

    Ok(())
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
        // **Nothing chosen: keep what the record already said, and default a new one to
        // wherever this machine keeps secrets.** Not the keyring unconditionally — that was
        // this function's bug, and it is the one place of three that had it. `db create`
        // asks `kept_where` and the backup key asks `crypt::keep_somewhere`, and both have
        // always read `SLOOP_PASSPHRASE` as "this machine keeps secrets in the encrypted
        // file". A headless Linux box that had said exactly that was still sent to a keyring
        // it deliberately does not run, and `db add` failed on a machine it was supposed to
        // work on. One rule, in `secret::preferred_routes`, and all three read it.
        return Ok(existing.cloned().unwrap_or_else(kept_where));
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
        let scope = context.registries.writes_to();
        let vault = context
            .registries
            .vault_in(scope)
            .unwrap_or_else(crate::secret::sealed::Vault::nowhere);

        match resolve(
            route,
            &Lookup {
                key: &key,
                vault: &vault,
            },
        ) {
            Ok(resolved) => {
                for note in &resolved.notes {
                    crate::note!("{}", style::dim(note));
                }
            }
            Err(failure) => crate::note!(
                "{}",
                style::dim(&format!(
                    "note: {} — registered anyway, because {} is read on every run rather \
                     than kept here",
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
        crate::note!(
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
        crate::note!("{}", style::dim(&note));
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

/// Try a route that keeps nothing, and say so if it answers with nothing.
fn note_if_it_does_not_answer(route: &Route, key: &str, context: &Context<'_>) {
    let scope = context.registries.writes_to();
    let vault = context
        .registries
        .vault_in(scope)
        .unwrap_or_else(crate::secret::sealed::Vault::nowhere);

    if let Err(failure) = resolve(route, &Lookup { key, vault: &vault }) {
        crate::note!(
            "{}",
            style::dim(&format!(
                "note: {} — registered anyway, because {} is read on every run rather than \
                 kept here",
                failure.message(),
                route.describe()
            ))
        );
    }
}

/// Get the SSH key's passphrase, if this registration needs sloop to hold one.
///
/// **`None` is the ordinary answer and the good one.** No SSH server, no route, or a route
/// that is a direction rather than a place — all three mean there is nothing for sloop to
/// keep, and with an agent holding the key that is every run.
fn ssh_secret_for(
    database: &Database,
    chosen: &SshFields,
    context: &Context<'_>,
) -> Outcome<Option<Secret>> {
    let Some(through) = database.reach.through() else {
        return Ok(None);
    };
    let Some(route) = &through.secret else {
        return Ok(None);
    };

    let asking_for = through.server.credential_key();

    if !route.is_stored() {
        // A direction rather than a place, so there is nothing to keep — but it is tried
        // once, for the reason `secret_for` tries the database's: a variable that is not set
        // or a command that is not installed should be mentioned now rather than found at
        // the first backup. A note and not a refusal, because these two routes exist for
        // machines configured later than they are registered.
        note_if_it_does_not_answer(route, &asking_for, context);
        return Ok(None);
    }

    let secret = if chosen.ssh_passphrase_stdin {
        from_stdin()?
    } else if std::io::stdin().is_terminal() {
        let typed = rpassword::prompt_password(format!("Passphrase for {asking_for}: "))
            .map_err(|error| Failure::usage(format!("could not read the passphrase: {error}")))?;
        Secret::new(typed)
    } else {
        // Rule 4. The flag that would have answered it is named, and so is the way to
        // register this database without sloop holding anything at all.
        return Err(Failure::new(
            Exit::Usage,
            "the SSH key's passphrase is needed and there is no terminal to ask at",
        )
        .hint(
            "pipe it in with --ssh-passphrase-stdin, or use --ssh-env VARIABLE or \
             --ssh-passphrase-from <command> — or put the key in an agent and pass none of \
             them",
        ));
    };

    for note in secret.notes() {
        crate::note!("{}", style::dim(&note));
    }

    Ok(Some(secret))
}

/// The passphrase an edit should file, carried over rather than asked for where it can be.
///
/// **Changing the SSH port should not cost somebody their passphrase.** It is filed under
/// the server, so moving the server moves the key it lives under — and the old one is read
/// from wherever it was and written under the new name, exactly as the database password is.
fn ssh_carried_over(
    before: &Database,
    after: &Database,
    chosen: &SshFields,
    context: &Context<'_>,
    scope: Scope,
) -> Outcome<Option<Secret>> {
    let Some(now) = after.reach.through() else {
        return Ok(None);
    };
    let Some(route) = &now.secret else {
        return Ok(None);
    };
    if !route.is_stored() {
        return Ok(None);
    }

    // Supplied on this command line, so there is nothing to carry.
    if chosen.ssh_passphrase_stdin {
        return ssh_secret_for(after, chosen, context);
    }

    let key = now.server.credential_key();

    if let Some(was) = before.reach.through()
        && let Some(old_route) = &was.secret
        && old_route.is_stored()
    {
        let old_key = was.server.credential_key();
        if old_key == key && old_route == route {
            // Nothing moved and nothing changed: what is already filed is still right, and
            // reading and rewriting it would be a keyring prompt for no reason.
            return Ok(None);
        }

        if let Some(vault) = context.registries.vault_in(scope)
            && let Ok(resolved) = resolve(
                old_route,
                &Lookup {
                    key: &old_key,
                    vault: &vault,
                },
            )
        {
            return Ok(Some(resolved.secret));
        }
    }

    // Nothing to carry — this database did not go over SSH before, or its passphrase was a
    // `${VAR}`, or there is simply nothing filed under the old name. Ask.
    ssh_secret_for(after, chosen, context).map_err(|failure| {
        failure.hint(
            "this edit needs the passphrase filed under the new server, and there was none \
             under the old one. Supply it with --ssh-passphrase-stdin, or at the prompt",
        )
    })
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
    // `?` on the Option rather than a failure: this whole function answers "is there an
    // old password worth carrying over", and no registry in that scope is a `no`.
    let vault = context.registries.vault_in(scope)?;

    resolve(
        &before.password,
        &Lookup {
            key: &key,
            vault: &vault,
        },
    )
    .ok()
    .map(|resolved| resolved.secret)
}

/// Connect, using whichever password applies.
fn connect(database: &Database, context: &Context<'_>, scope: Scope) -> Outcome<Reached> {
    let (at, secret) = super::open(
        database,
        scope,
        &context.registries,
        context.tunnels,
        context.password_command,
    )?;
    announce_tunnel(&at);

    let server = super::adapter_for(database.engine, context.global)
        .probe(&database.target_at(&secret, &at.host, at.port))?;
    Ok(Reached { server, at })
}

/// A server that answered, and the address it answered at.
///
/// **Both, because what secures the connection depends on the second.** Over a tunnel, the
/// client is talking to `127.0.0.1` and no certificate can be matched against that — so a
/// line that said "over TLS" and stopped would be describing a session nobody verified while
/// the thing actually securing it went unmentioned. See [`announce_server`].
struct Reached {
    /// What the server said it is.
    server: crate::engine::ServerInfo,
    /// Where the client program was pointed.
    at: super::At,
}

/// Say that a connection went through a tunnel, once, where it happened.
///
/// **The server, never the port.** A forward's port is different every run, so printing it
/// would be printing something nobody can reuse — and `R20` prints the `--ssh-*` flags for
/// the same reason.
fn announce_tunnel(at: &super::At) {
    if let Some(server) = &at.through {
        crate::say!("  {}", style::dim(&format!("through {server}")));
    }
}

/// Connect with a password that is in hand rather than stored, for `--test` on a record
/// that has not been written yet.
fn probe(database: &Database, secret: Option<&Secret>, context: &Context<'_>) -> Outcome<Reached> {
    match secret {
        Some(secret) => {
            let at = super::reach(
                database,
                context.tunnels,
                &context.registries,
                context.registries.writes_to(),
            )?;
            announce_tunnel(&at);

            let target = Target {
                engine: database.engine,
                host: &at.host,
                port: at.port,
                database: &database.database,
                user: &database.user,
                password: secret,
            };
            let server = super::adapter_for(database.engine, context.global).probe(&target)?;
            Ok(Reached { server, at })
        }
        // A `${VAR}` or `command:` route: nothing was kept, so go and ask for it the same
        // way every later run will.
        None => connect(database, context, context.registries.writes_to()),
    }
}

fn announce_server(server: &crate::engine::ServerInfo, through: Option<&str>) {
    let over_ssh = through.is_some();

    // **What is actually securing this connection, not what the client negotiated.** Over a
    // tunnel the client dialled `127.0.0.1`, so whatever certificate the server presented
    // cannot be matched against the name it was issued for — SSH is what authenticated the
    // far side, and saying "over TLS" while leaving that out would be describing the weaker
    // half of the truth. Nothing is turned off to make this message tidier: sloop sets no
    // TLS mode at all, on a tunnel or off one.
    let how = match (over_ssh, server.tls) {
        (true, _) => ", encrypted by SSH",
        (false, true) => ", over TLS",
        (false, false) => ", not encrypted",
    };

    crate::say!(
        "  {} {}{}",
        style::paint(&format!("{}", server.engine)),
        server.version,
        style::dim(how)
    );

    if over_ssh && server.tls {
        crate::say!(
            "  {}",
            style::dim(
                "the server offered TLS inside the tunnel as well, and no certificate can be \
                 matched against 127.0.0.1 — so it is the SSH connection that proves which \
                 machine this is"
            )
        );
    }
}

/// File the password, then the record — and undo the first if the second fails.
///
/// **The order is the recoverable one.** A stored password with no record pointing at it
/// is invisible clutter; a record with no password is a database that looks registered and
/// fails on the next backup. So the record goes last, and a failure there takes the
/// credential back out rather than leaving the pair half made.
fn write(
    registries: &mut Registries,
    scope: Scope,
    name: &str,
    database: &Database,
    secrets: &Secrets<'_>,
    retire: &[Retiring],
) -> Outcome<()> {
    // **Every secret this registration holds, each under its own key.** A database reached
    // over SSH has two — its own password and the passphrase that opens the key — and the
    // two go to different places under different names, because five databases behind one
    // bastion share the passphrase and share nothing else.
    let mut written: Vec<Retiring> = Vec::new();

    if let Some(secret) = secrets.password {
        let key = database.credential_key();
        store(&database.password, &key, secret, registries, scope)?;
        written.push(Retiring {
            route: database.password.clone(),
            key,
        });
    }

    if let Some((route, key, secret)) = ssh_credential(database, secrets.ssh) {
        if let Err(failure) = store(route, &key, secret, registries, scope) {
            // The database password may already be down. Nothing is half-written if this
            // fails, which is the same guarantee the single-secret version had.
            undo(&written, registries, scope);
            return Err(failure);
        }
        written.push(Retiring {
            route: route.clone(),
            key,
        });
    }

    let stored = crate::registry::Qualified::parse(name)?.name().to_owned();
    let entry = database.clone();
    let saved = registries.update(scope, move |registry| Ok(registry.insert(stored, entry)));

    if saved.is_err() {
        undo(&written, registries, scope);
    }
    saved?;

    // Only once the new record is safely on disk. A password nothing references any more
    // is clutter at best, and clearing it before the write would have been clutter plus a
    // lost password if the write then failed.
    for old in retire {
        if let Err(failure) = forget(&old.route, &old.key, registries, scope) {
            crate::note!(
                "{}",
                style::dim(&format!(
                    "the secret under the old key could not be removed: {}",
                    failure.message()
                ))
            );
        }
    }

    Ok(())
}

/// The SSH passphrase this registration is about to file, if it has one to file.
///
/// Three things have to line up: a secret in hand, a server to file it against, and a route
/// that keeps anything. `${VAR}` and `command:` are directions rather than places, so there
/// is nothing to write for either.
fn ssh_credential<'a>(
    database: &'a Database,
    secret: Option<&'a Secret>,
) -> Option<(&'a Route, String, &'a Secret)> {
    let secret = secret?;
    let through = database.reach.through()?;
    let route = through.secret.as_ref()?;
    Some((route, through.server.credential_key(), secret))
}

/// Put back what was just written, after something else in the same registration failed.
fn undo(written: &[Retiring], registries: &Registries, scope: Scope) {
    for one in written {
        let _ = forget(&one.route, &one.key, registries, scope);
    }
}

/// What a registration is holding, ready to be filed.
///
/// Named fields rather than two `Option<&Secret>` arguments in a row: the two are the same
/// type, they go to different keys, and passing them the wrong way round would file a
/// database password under an SSH server's name and be discovered at the next backup.
#[derive(Default)]
struct Secrets<'a> {
    /// The database's own password, when this run has one in hand.
    password: Option<&'a Secret>,
    /// The passphrase that opens the SSH key, when there is one.
    ssh: Option<&'a Secret>,
}

impl<'a> Secrets<'a> {
    /// A registration that holds a database password and nothing else — which is every
    /// direct one, and is what `db create` always makes.
    const fn just_the_password(password: &'a Secret) -> Self {
        Self {
            password: Some(password),
            ssh: None,
        }
    }
}

/// A password that is no longer referenced, and where it lives.
///
/// **Both halves, and the route is the half that was a bug.** Clearing the old key through
/// the *new* route looks right and is not: `db edit --encrypted-file` on a keyring record
/// writes the password into the file and then asks the file to forget a key it never had,
/// while the keyring quietly keeps the password for a connection that no longer exists.
#[derive(Debug)]
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
    ///
    /// **Both credentials, because an edit can move either.** `--ssh-identity` names a
    /// different key and orphans the passphrase filed under the old one; `--no-ssh` orphans
    /// it outright; and moving the tunnel moves the *database* password too, because
    /// [`Database::credential_key`] has the server in it.
    fn between(before: &Database, after: &Database) -> Vec<Self> {
        let mut retiring = Vec::new();

        let key = before.credential_key();
        if before.password.is_stored()
            && (key != after.credential_key() || before.password != after.password)
        {
            retiring.push(Self {
                route: before.password.clone(),
                key,
            });
        }

        if let Some(was) = before.reach.through()
            && let Some(route) = &was.secret
            && route.is_stored()
        {
            let key = was.server.credential_key();
            let kept = after.reach.through().is_some_and(|now| {
                now.server.credential_key() == key && now.secret.as_ref() == Some(route)
            });

            if !kept {
                retiring.push(Self {
                    route: route.clone(),
                    key,
                });
            }
        }

        retiring
    }
}

fn store(
    route: &Route,
    key: &str,
    secret: &Secret,
    registries: &Registries,
    scope: Scope,
) -> Outcome<()> {
    match route {
        Route::Keyring => crate::secret::os_keyring::set(key, secret),
        Route::EncryptedFile => {
            let vault = registries
                .vault_in(scope)
                .ok_or_else(|| Failure::usage("there is nowhere to put the encrypted password"))?;
            crate::secret::sealed::put(&vault, key, secret)
        }
        // Nothing to store: the route is the answer.
        Route::Environment(_) | Route::Command(_) => Ok(()),
    }
}

fn forget(route: &Route, key: &str, registries: &Registries, scope: Scope) -> Outcome<()> {
    match route {
        Route::Keyring => crate::secret::os_keyring::delete(key),
        Route::EncryptedFile => {
            let vault = registries.vault_in(scope).ok_or_else(|| {
                Failure::usage("there is nowhere to look for the encrypted password")
            })?;
            crate::secret::sealed::forget(&vault, key)
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
pub fn remove(context: &mut Context<'_>, name: &str) -> Outcome<Exit> {
    let (scope, record) = context.registries.find(name)?;
    let record = record.clone();

    crate::say!(
        "{} {}  {}",
        style::dim("forgetting"),
        style::paint(name),
        style::dim(&record.credential_key())
    );
    crate::say!(
        "  {}",
        style::dim(&format!(
            "the {} registry, and the password kept in {}",
            scope.label(),
            record.password.describe()
        ))
    );
    crate::say!(
        "  {}",
        style::dim("the database itself is not touched — that is `sloop db drop`")
    );

    if !context.consent.asked("Forget it?", "--yes")? {
        crate::say!("{}", style::dim("left alone."));
        return Ok(Exit::Success);
    }

    if crate::report::would(&format!("forget {name}")) {
        return Ok(Exit::Success);
    }

    // The record first, which is the opposite order from `write`. A password with no record
    // is clutter; a record whose password has been deleted is a database that looks
    // registered and cannot connect — so the half that makes it unreachable goes last.
    let stored = crate::registry::Qualified::parse(name)?.name().to_owned();
    context
        .registries
        .update(scope, move |registry| Ok(registry.remove(&stored)))?;

    if record.password.is_stored()
        && let Err(failure) = forget(
            &record.password,
            &record.credential_key(),
            &context.registries,
            scope,
        )
    {
        crate::note!(
            "{}",
            style::dim(&format!(
                "the record is gone; its password could not be removed: {}",
                failure.message()
            ))
        );
    }

    crate::report::result(serde_json::json!({ "name": name, "forgotten": true }));
    crate::say!("{} {}", style::paint("forgot"), style::paint(name));
    Ok(Exit::Success)
}

// ---------------------------------------------------------------------------------------
// drop
// ---------------------------------------------------------------------------------------

/// Destroy a database on the server.
///
/// **It writes nothing.** `R8` took a safety copy first; the owner reopened that on
/// 2026-09-16 — *"just drop nothing to write"* — and it is the same principle as a copy not
/// being a backup: `db drop` destroys a database, and a command that is not a backup command
/// should not leave a backup lying about. A dump it wrote had no manifest either, so it read
/// as an unfinished backup for ever and `restore` could not have used it.
///
/// So this cannot be undone, and every line it prints says so. `sloop backup <name>` first
/// is the way to keep a copy, and it is named in the warning, in the prompt and in `--help`.
/// See "`db drop` writes nothing" in `docs/OWNER-DECISIONS.md`.
pub fn drop(context: &mut Context<'_>, name: &str) -> Outcome<Exit> {
    let (scope, record) = context.registries.find(name)?;
    let record = record.clone();

    // Everything that can be answered before a socket is opened, is — a scheduled run that
    // named the wrong database, or has no terminal and no `--confirm`, is told so here
    // rather than after a password has been fetched for a connection it was never going to
    // be allowed to use. See `consent::Consent::checked_early`.
    let destroying = Destroying {
        named: &record.database,
        noun: "database",
        action: "dropping a database",
    };
    context.consent.checked_early(&destroying)?;

    // A name that resolves in the registry is not evidence that the database is there, and
    // "about to destroy X" had better be true before it is printed.
    let key = record.credential_key();
    let route = record.password.overridden_by(context.password_command);
    let vault = context
        .registries
        .vault_in(scope)
        .unwrap_or_else(crate::secret::sealed::Vault::nowhere);
    let resolved = resolve(
        &route,
        &Lookup {
            key: &key,
            vault: &vault,
        },
    )?;
    let at = super::reach(&record, context.tunnels, &context.registries, scope)?;
    announce_tunnel(&at);

    let target = record.target_at(&resolved.secret, &at.host, at.port);
    let adapter = super::adapter_for(record.engine, context.global);
    let server = adapter.probe(&target)?;

    crate::say!(
        "{} {}",
        style::paint("about to drop"),
        style::paint(&record.database)
    );
    crate::say!("  {}", style::dim(&key));
    crate::say!(
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
        crate::say!(
            "  {}",
            style::dim(&format!(
                "{} table(s), {rows} row(s) — all of it",
                counts.len()
            ))
        );
    }

    // **Said once, plainly, and before the question.** Nothing is kept, so the only honest
    // thing to do is name the command that would have kept something while there is still
    // time to run it.
    crate::say!(
        "  {}",
        style::dim(&format!(
            "nothing is kept and this cannot be undone — `sloop backup {}` first if you \
             want a copy",
            style::as_argument(name)
        ))
    );

    // **Typed, never clicked.** Rule 5, and what is typed is the database's own name on the
    // server rather than the label: the label is what sloop calls it, and the name is what
    // is about to stop existing.
    if !context.consent.typed(&destroying)?.granted() {
        crate::say!("{}", style::dim("left alone."));
        return Ok(Exit::Success);
    }

    let store = context.registries.root_in(scope).ok_or_else(|| {
        Failure::usage(format!(
            "there is no {} store to lock against",
            scope.label()
        ))
    })?;
    let _held = crate::lock::take(&store, name, "db drop")?;

    if crate::report::would(&format!("drop {}", target.describe())) {
        return Ok(Exit::Success);
    }

    let ended = adapter.terminate_connections(&target)?;
    if ended > 0 {
        crate::say!(
            "  {}",
            style::dim(&format!("ended {ended} other connection(s)"))
        );
    }

    adapter.drop_database(&target)?;
    crate::report::result(serde_json::json!({
        "name": name,
        "database": record.database,
        "dropped": true,
        "connections_ended": ended,
    }));
    crate::say!("{} {}", style::paint("dropped"), record.database);
    crate::say!(
        "  {}",
        style::dim(&format!(
            "{name} is still registered and now points at nothing — `sloop db remove {}` \
             forgets it",
            style::as_argument(name)
        ))
    );

    Ok(Exit::Success)
}

// ---------------------------------------------------------------------------------------
// Asking
// ---------------------------------------------------------------------------------------

#[cfg(test)]
#[path = "db_tests.rs"]
mod tests;
