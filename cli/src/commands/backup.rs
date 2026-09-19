//! `sloop backup` — take a dump, and write down what it is.
//!
//! **A backup is a dump plus its manifest, and neither half is optional.** The dump is
//! useless six months later if nothing says which server it came from, how big it was
//! meant to be, or what was in it; the manifest is written last, so its presence is what
//! tells `backups list` and `restore` that a directory holds a finished backup rather
//! than a run that was killed halfway. See [`crate::backup::manifest`].
//!
//! **The rows are counted before the dump starts.** Both engines take their snapshot at
//! the beginning of the dump — `pg_dump` in a repeatable-read transaction,
//! `mysqldump --single-transaction` in the same way — so counting immediately before is
//! the closest this can get to counting the snapshot itself. It also fails early: a role
//! that cannot read every table cannot count every table either, and finding that out
//! before twenty minutes of dumping is worth more than finding it out after.
//!
//! **`--all` never stops on a failure.** It runs to the end, says which databases failed
//! as it goes, and then says it again in one line, because a backup run that abandoned
//! four databases because the first one was unreachable is how people lose data without
//! being told. The exit code is the shared code of the failures when they agree, and `1`
//! when they do not — a monitor reading `3` learns "a server was down", and a monitor
//! reading `1` learns "read the output, it was more than one thing".
//!
//! **Nothing here asks a question**, so there is nothing for rule 4 to catch: a backup is
//! not destructive, and a scheduled run is the case this command is built for. A missing
//! name is usage and says so; everything else either works or fails with a code.
//!
//! **The lock is not here.** `R17` owns "one lockfile per database, exit 7 when it is
//! held", and a directory already holding a finished backup is refused rather than
//! overwritten in the meantime.

#[cfg(test)]
#[path = "backup_cluster_tests.rs"]
mod cluster_tests;
#[cfg(test)]
#[path = "backup_tests.rs"]
mod tests;

use std::collections::BTreeMap;
use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::backup::manifest::{self, Described, Manifest};
use crate::backup::stamp::Stamp;
use crate::backup::{self, directory_for, dump_file, store};
use crate::crypt::{self, PublicKey};
use crate::engine::{Adapter, ServerInfo, TableCount, Target};
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::registry::file::{Database, KeyKept};
use crate::registry::{Registries, Scope};
use crate::secret::{Lookup, resolve};
use crate::ssh::tunnel::Tunnels;
use crate::style;
use crate::tools::{Inventory, acquire};

/// Everything `backup` needs from the outside.
///
/// The registries are owned rather than borrowed because a backup can have to *write* one:
/// the first run against a registry with no backup key creates the keypair and records it.
/// That happens once, before any dumping starts — see [`sealing_for`].
pub struct Context<'a> {
    /// Both registries, already open.
    pub registries: Registries,
    /// The global store, which is where sloop's own copy of the client tools lives.
    pub global: &'a Path,
    /// `--password-command`, which outranks whatever route a record names.
    pub password_command: Option<&'a str>,
    /// Every SSH forward this session holds — see [`super::reach`]. Shared with every
    /// other command in the run, so a menu session authenticates once.
    pub tunnels: &'a Tunnels,
}

/// A backup that landed.
pub struct Taken {
    /// The directory it went in.
    pub directory: PathBuf,
    /// What was written beside the dump.
    pub manifest: Manifest,
}

impl Taken {
    /// The same backup, as a `--json` run reports it.
    ///
    /// The manifest is the record of what was taken, so this is the manifest's own fields —
    /// a script reading this and a person reading `backups list` are looking at one thing.
    #[must_use]
    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            "label": self.manifest.label,
            "directory": self.directory.display().to_string(),
            "engine": self.manifest.engine.to_string(),
            "database": self.manifest.database,
            "rows": self.manifest.rows,
            "tables": self.manifest.tables.len(),
            "bytes": self.manifest.dump.bytes,
            "sha256": self.manifest.dump.sha256,
            "encrypted": self.manifest.dump.encryption.is_some(),
            "taken_utc": self.manifest.taken.utc,
            "taken_local": self.manifest.taken.local,
            "seconds": self.manifest.took_seconds,
        })
    }

    /// The lines worth printing once it is done.
    ///
    /// Built rather than printed, so that what a person is shown is something a test can
    /// assert on — in particular that the time in it is the local one, which is a rule
    /// this project has and not an accident of formatting.
    #[must_use]
    pub fn describe(&self) -> Vec<String> {
        vec![
            format!("backed up to {}", self.directory.display()),
            format!(
                "{} in {:.1}s, sha256 {}",
                describe_bytes(self.manifest.dump.bytes),
                self.manifest.dump_seconds,
                short(&self.manifest.dump.sha256),
            ),
            format!(
                "taken {} ({})",
                self.manifest.taken_locally().readable(),
                self.manifest.taken.utc,
            ),
        ]
    }
}

/// Take this database's lock, from whichever store its registry lives in.
fn locked(context: &Context<'_>, scope: Scope, name: &str) -> Outcome<crate::lock::Held> {
    let store = context.registries.root_in(scope).ok_or_else(|| {
        Failure::new(
            Exit::Usage,
            format!("there is no {} store to lock against", scope.label()),
        )
        .hint("`sloop init` starts a registry in this directory; `--global` uses the global one")
    })?;
    crate::lock::take(&store, name, "backup")
}

/// Back up one database, or every one of them.
pub fn run(context: &mut Context<'_>, name: Option<&str>, all: bool, mode: Mode) -> Outcome<Exit> {
    match (name, all) {
        (Some(name), false) => {
            let (scope, engine) = {
                let (scope, database) = context.registries.find(name)?;
                (scope, database.engine)
            };

            // **The lock first, before the key.** `sealing_for` *creates* a keypair on a
            // registry that has none, and a run that is about to be refused has no business
            // creating one — nor contacting anything. The lock is the cheapest question
            // there is, so it is the first one asked.
            let _held = locked(context, scope, name)?;

            // Before anything is dumped: is there a key, and is there a copy of it?
            let sealing = sealing_for(context, scope)?;
            let inventory = Inventory::for_engine(engine, &fetched(context));

            let (_, database) = context.registries.find(name)?;
            let Some(taken) = one(
                context,
                &inventory,
                sealing.as_ref(),
                scope,
                name,
                database,
                mode,
            )?
            else {
                return Ok(Exit::Success);
            };
            for line in taken.describe() {
                crate::say!("  {}", style::dim(&line));
            }
            crate::report::result(serde_json::json!({ "backups": [taken.as_json()] }));
            Ok(Exit::Success)
        }
        (None, true) => every(context, mode),
        // clap refuses both together, so this is only reachable if that ever changes.
        (Some(_), true) => Err(
            Failure::usage("--all backs up everything, so it takes no name")
                .hint("`sloop backup <name>` for one, `sloop backup --all` for all of them"),
        ),
        (None, false) => Err(Failure::usage("backup needs a database to back up").hint(
            "`sloop backup <name>`, or `sloop backup --all` — `sloop db list` shows the names",
        )),
    }
}

/// Every registered database, whatever happens to any one of them.
fn every(context: &mut Context<'_>, mode: Mode) -> Outcome<Exit> {
    let registered = context.registries.len();
    if registered == 0 {
        return Err(Failure::usage(format!(
            "nothing is registered in {}, so --all has nothing to back up",
            context.registries.resolution().describe(context.global)
        ))
        .hint("`sloop db add <name> --url postgres://user@host/database` registers one"));
    }

    // **Every key, before the first dump.** A project entry and a global one are encrypted
    // to their own registry's key, so both may need setting up — and the question "is there
    // a copy of this key" is asked once, at the start, rather than thirty databases in.
    let scopes: Vec<Scope> = {
        let mut seen: Vec<Scope> = Vec::new();
        for (scope, _, _) in context.registries.all() {
            if !seen.contains(&scope) {
                seen.push(scope);
            }
        }
        seen
    };
    let mut sealing: BTreeMap<Scope, Option<PublicKey>> = BTreeMap::new();
    for scope in scopes {
        sealing.insert(scope, sealing_for(context, scope)?);
    }

    // Collected after that, so the loop below is not iterating a borrow it also has to read
    // the registry through — and so "0 of 7" is a number that exists before the work does.
    let jobs: Vec<(Scope, &str, &Database)> = context.registries.all().collect();

    let inventory = Inventory::everything(&fetched(context));
    let mut failures: Vec<(String, Failure)> = Vec::new();
    let mut landed: Vec<serde_json::Value> = Vec::new();
    let mut done = 0_usize;

    for (index, (scope, name, database)) in jobs.iter().enumerate() {
        if index > 0 {
            crate::say!();
        }
        let sealed_to = sealing.get(scope).and_then(Option::as_ref);

        // **A locked database is reported and stepped over, not a reason to stop.** `--all`
        // never abandons the rest of the run over one database, and a backup that is already
        // running is the least alarming thing that can go wrong with one.
        let _held = match locked(context, *scope, name) {
            Ok(held) => held,
            Err(failure) => {
                let failure = failure.prefixed(*name);
                failure.mention();
                failures.push(((*name).to_owned(), failure));
                continue;
            }
        };

        match one(context, &inventory, sealed_to, *scope, name, database, mode) {
            Ok(Some(taken)) => {
                done += 1;
                for line in taken.describe() {
                    crate::say!("  {}", style::dim(&line));
                }
                landed.push(taken.as_json());
            }
            // A rehearsal: it said what it would have done and wrote nothing.
            Ok(None) => {}
            // Said now as well as at the end. A run over thirty databases should not keep
            // the first failure to itself until the last one has finished.
            Err(failure) => {
                let failure = failure.prefixed(*name);
                failure.mention();
                failures.push(((*name).to_owned(), failure));
            }
        }
    }

    crate::report::result(serde_json::json!({
        "backups": landed,
        "attempted": jobs.len(),
        "failed": failures
            .iter()
            .map(|(name, failure)| serde_json::json!({
                "name": name,
                "error": failure.message(),
                "exit": failure.exit().code(),
            }))
            .collect::<Vec<_>>(),
    }));

    // One line, whatever the length of the run: what a person scrolls to the bottom for,
    // and the only place every failed database is named together.
    crate::say!();
    let counted = style::paint(&format!("backed up {done} of {}", jobs.len()));
    if failures.is_empty() {
        crate::say!("{counted}");
    } else {
        crate::say!(
            "{counted} {}",
            style::dim(&format!(
                "— {} failed: {}",
                failures.len(),
                failures
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        );
    }

    Ok(verdict(&failures))
}

/// What `--all` leaves the process with.
///
/// The failures' own code when they agree on one, and `1` when they do not. A run where
/// three servers were unreachable is a `3` somebody can act on; a run where one server was
/// down and another role had its password moved is not any single one of those, and
/// picking the first would tell a monitoring system something that is not true.
fn verdict(failures: &[(String, Failure)]) -> Exit {
    let mut codes = failures.iter().map(|(_, failure)| failure.exit());
    match codes.next() {
        None => Exit::Success,
        Some(first) if codes.all(|code| code == first) => first,
        Some(_) => Exit::Failure,
    }
}

/// One database, start to finish.
fn one(
    context: &Context<'_>,
    inventory: &Inventory,
    sealed_to: Option<&PublicKey>,
    scope: Scope,
    name: &str,
    database: &Database,
    mode: Mode,
) -> Outcome<Option<Taken>> {
    let key = database.credential_key();
    crate::say!("{}  {}", style::paint(name), style::dim(&key));

    // The encrypted file sits beside the registry that names the database, so a project
    // entry and a global one of the same name read from two different stores.
    let route = database.password.overridden_by(context.password_command);
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
    for note in &resolved.notes {
        crate::say!("  {}", style::dim(note));
    }

    let root = context.registries.root_in(scope).ok_or_else(|| {
        Failure::usage("there is nowhere to put the backup")
            .hint("run this against a project registry, or the global store")
    })?;

    // **The forward is opened here, inside the per-database work.** `backup --all` never
    // stops on a failure, so a server that cannot be reached over SSH fails one database and
    // the rest of the run carries on — which is what that rule has always meant, and would
    // not be true if the tunnel were opened once at the top.
    let at = super::reach(database, context.tunnels, &context.registries, scope)?;
    if let Some(server) = &at.through {
        crate::say!("  {}", style::dim(&format!("through {server}")));
    }

    let target = database.target_at(&resolved.secret, &at.host, at.port);
    let adapter = inventory.adapter_for(database.engine);

    let started = Instant::now();
    let server = adapter.probe(&target)?;
    let counts = adapter.row_counts(&target)?;
    let rows: u64 = counts.iter().map(|count| count.rows).sum();
    crate::say!(
        "  {}",
        style::dim(&format!(
            "{} {}{}, {}, {}",
            server.engine,
            server.version,
            if server.tls { ", TLS" } else { "" },
            plural(u64::try_from(counts.len()).unwrap_or(u64::MAX), "table"),
            plural(rows, "row"),
        ))
    );

    // **Everything above this is a read.** The server has been reached, the key has been
    // found and the rows have been counted; a rehearsal stops before the first directory is
    // made, which is the first thing this leaves behind.
    if crate::report::would(&format!("back up {name} to {}", root.display())) {
        return Ok(None);
    }

    let taken = Stamp::now();
    let (directory, swap_into) = destination(&root, database.engine, name, taken, mode)?;

    let existed = directory.is_dir();
    std::fs::create_dir_all(&directory).map_err(|error| {
        Failure::new(
            Exit::Dump,
            format!("could not create {}: {error}", directory.display()),
        )
    })?;

    // From here on a failure leaves a directory behind, and a directory with half a dump
    // in it is worse than none: it is what somebody finds when they go looking for a
    // backup. So everything below either finishes or clears up after itself.
    let outcome = write_everything(
        adapter.as_ref(),
        &target,
        &Writing {
            label: name,
            key: &key,
            directory: &directory,
            taken,
            started,
            server: &server,
            counts: &counts,
            sealed_to,
        },
    );

    match outcome {
        Ok(manifest) => {
            // **`R27a`: the one figure in this tool that really is bytes.** sloop read this
            // dump, so it knows exactly how many — unlike anything about traffic on the wire,
            // which no engine reports per database. Recorded here rather than in the
            // scheduler, because a backup taken by hand moved exactly as many bytes as one
            // the service took and the screen answers about both.
            //
            // Best effort: a machine whose store cannot be written to has just taken a
            // backup, and failing it to record a statistic would be the tail wagging the dog.
            if let Some(store) = context.registries.store() {
                let _ = crate::service::traffic::moved(
                    store,
                    scope,
                    name,
                    i64::try_from(manifest.dump.bytes).unwrap_or(0),
                    0,
                );
            }

            // **The swap is the last thing that happens, and only now.** Everything above
            // wrote into `latest.writing`, so up to this line the previous copy is still
            // the one at `latest` and still complete — which is what makes a `--replace`
            // that is killed mid-dump cost nothing at all.
            if let Some(latest) = swap_into {
                store::swap_in(&directory, &latest)?;
                return Ok(Some(Taken {
                    directory: latest,
                    manifest,
                }));
            }
            Ok(Some(Taken {
                directory,
                manifest,
            }))
        }
        Err(failure) => {
            if !existed {
                // Best effort, and only the directory this run created: it is named after
                // a second that has passed, or it is this run's own `latest.writing`, so
                // nothing else can have put anything in it.
                let _ = std::fs::remove_dir_all(&directory);
            }
            Err(failure)
        }
    }
}

/// How a backup is written, and it is a flag rather than a mode the tool remembers.
///
/// A scheduled run honours both identically: there is no interactive half to either, and
/// nothing about the choice is recorded in the registry. What a crontab says is what it
/// gets, every time, which is the only way a retention policy and a cron line can be
/// reasoned about together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// A new `<utc-timestamp>/` directory per run. The default, and `R9`'s behaviour.
    #[default]
    Sequential,
    /// One directory per label, overwritten — `backups/<engine>/<label>/latest`.
    Replace,
}

/// Where this run writes, and what it has to be swapped into afterwards.
///
/// Sequential returns the timestamped directory and nothing to swap. Replace returns
/// `latest.writing` and the `latest` it will become — because a replace that writes straight
/// into `latest` would spend the length of a dump with neither a finished backup nor a
/// recoverable one on the disk.
fn destination(
    root: &Path,
    engine: crate::engine::Engine,
    name: &str,
    taken: Stamp,
    mode: Mode,
) -> Outcome<(PathBuf, Option<PathBuf>)> {
    if mode == Mode::Sequential {
        let directory = directory_for(root, engine, name, taken);
        refuse_to_overwrite(&directory, name)?;
        return Ok((directory, None));
    }

    let label_dir = backup::label_dir(root, engine, name);
    std::fs::create_dir_all(&label_dir).map_err(|error| {
        Failure::new(
            Exit::Dump,
            format!("could not create {}: {error}", label_dir.display()),
        )
    })?;

    // Anything left over from a run that was killed is put right before this one starts,
    // and out loud: a directory that repaired itself in silence teaches nobody anything.
    for note in store::recover_replace(&label_dir)? {
        crate::say!("  {}", style::dim(&note));
    }

    Ok((
        label_dir.join(backup::WRITING),
        Some(label_dir.join(backup::LATEST)),
    ))
}

/// **A finished backup is never overwritten.**
///
/// Two runs in the same second land on the same UTC directory name, and the second one
/// silently replacing the first is how a good backup disappears. `R17`'s lockfile stops
/// two runs at once; this stops two in a row.
///
/// A directory with no manifest in it is a run that died halfway, and that one *is*
/// written over — there is nothing there worth keeping and refusing would leave somebody
/// unable to back up until they deleted it by hand.
fn refuse_to_overwrite(directory: &Path, name: &str) -> Outcome<()> {
    if !manifest::completed(directory) {
        return Ok(());
    }

    Err(Failure::new(
        Exit::Failure,
        format!(
            "a backup of {name} taken this second is already at {}",
            directory.display()
        ),
    )
    .hint("a backup is named for the second it was taken in — wait one and run it again"))
}

/// What [`write_everything`] is working on, so the argument list stays readable.
struct Writing<'a> {
    /// The name the database is registered under.
    label: &'a str,
    /// The connection string, which has no password in it.
    key: &'a str,
    /// The backup's own directory, already created.
    directory: &'a Path,
    /// The moment the directory is named after.
    taken: Stamp,
    /// When the whole thing started, counting included.
    started: Instant,
    /// What the server said when it was asked.
    server: &'a ServerInfo,
    /// The source's exact row counts, taken before the dump.
    counts: &'a [TableCount],
    /// The key to encrypt to, when this registry has one.
    sealed_to: Option<&'a PublicKey>,
}

/// Dump, hash, describe — the half that has to be undone if any of it fails.
fn write_everything(
    adapter: &dyn Adapter,
    target: &Target<'_>,
    writing: &Writing<'_>,
) -> Outcome<Manifest> {
    let plain = dump_file(writing.directory);
    let dumping = Instant::now();

    // **Encrypted on the way out, not afterwards.** The dump program's output goes through
    // the age writer and lands as ciphertext; there is no moment where the plaintext exists
    // on disk, and no second pass over a file that may be a hundred gigabytes.
    let file = match writing.sealed_to {
        Some(recipient) => {
            let sealed = crypt::sealed_name(&plain);
            crypt::sealed_to(&sealed, recipient, |sink| {
                adapter.dump_into(target, sink, &[])
            })?;
            sealed
        }
        None => plain,
    };
    let dump_took = dumping.elapsed();

    let bytes = std::fs::metadata(&file)
        .map(|meta| meta.len())
        .map_err(|error| {
            Failure::new(
                Exit::Dump,
                format!("the dump is not at {}: {error}", file.display()),
            )
        })?;
    if bytes == 0 {
        return Err(Failure::new(
            Exit::Dump,
            format!("the dump wrote nothing to {}", file.display()),
        )
        .hint(
            "an empty database still dumps to something, so this is the role being allowed \
             to see none of it — `sloop doctor` reports what it may do",
        ));
    }

    // The checksum is of what is actually on the disk, encrypted or not, so `backups list`
    // can tell an intact backup from a corrupted one without needing the key.
    let sha256 = manifest::checksum(&file)?;
    let written = file.file_name().unwrap_or_default().to_string_lossy();

    let manifest = Manifest::of(Described {
        label: writing.label,
        source: writing.key,
        database: target.database,
        server: writing.server,
        taken: writing.taken,
        took: writing.started.elapsed(),
        dump_took,
        dump_file: &written,
        bytes,
        sha256,
        sealed_to: writing.sealed_to,
        counts: writing.counts,
    });

    manifest.write(writing.directory)?;
    Ok(manifest)
}

/// The key this scope's backups are encrypted to, setting one up if there is none.
///
/// **Encryption is the default, and this is where it starts.** The first backup against a
/// registry with no keypair creates one: the public half goes in the registry so that every
/// run after this needs no secret at all, and the private half goes in the keyring.
///
/// **A machine that cannot keep a private key gets an unencrypted backup and is told so.**
/// A headless Linux box with no keyring and no `SLOOP_PASSPHRASE` has nowhere to put one,
/// and refusing to back it up would be this tool failing at its job in order to protect a
/// feature. The backup happens; the sentence explaining why it is not encrypted happens too.
fn sealing_for(context: &mut Context<'_>, scope: Scope) -> Outcome<Option<PublicKey>> {
    let existing = context
        .registries
        .in_scope(scope)
        .and_then(|registry| registry.encryption())
        .cloned();

    let encryption = if let Some(encryption) = existing {
        encryption
    } else {
        let vault = context
            .registries
            .vault_in(scope)
            .unwrap_or_else(crate::secret::sealed::Vault::nowhere);

        match crate::commands::key::create(&mut context.registries, scope, &vault) {
            Ok(encryption) => encryption,
            Err(failure) => {
                crate::note!(
                    "{} {}",
                    style::dim("note: this backup is not encrypted —"),
                    style::dim(failure.message())
                );
                crate::note!(
                    "  {}",
                    style::dim(
                        "a machine with no keyring can keep a key in the Argon2id file \
                         instead: set SLOOP_PASSPHRASE and run `sloop key export`"
                    )
                );
                return Ok(None);
            }
        }
    };

    if encryption.key_kept.is_none() {
        keep_a_copy_first(context, scope, &encryption.public_key)?;
    }

    Ok(Some(encryption.public_key))
}

/// Said once, hard: an encrypted backup is worth nothing without the key.
///
/// The one place this tool stops to make somebody read something. It is not a hardening
/// preference — it is the difference between an archive and a pile of noise, and the moment
/// to find out is now rather than the day the machine it was taken on is gone.
///
/// **Typed, not clicked**, because rule 5 applies to anything this irreversible. Without a
/// terminal it exits `2` naming the command that answers, because a scheduled run must never
/// hang on a question nobody is there to read.
fn keep_a_copy_first(context: &mut Context<'_>, scope: Scope, public: &PublicKey) -> Outcome<()> {
    crate::note!();
    crate::note!("{}", style::paint("this registry has a new backup key"));
    crate::note!("  {}", style::dim(&public.to_string()));
    crate::note!(
        "  {}",
        style::dim(
            "backups from here on are encrypted to it. The private half is on this machine \
             and nowhere else, so if this machine is lost, every one of those backups is \
             unreadable — there is no recovery, no reset and nobody to ask."
        )
    );
    crate::note!(
        "  {}",
        style::dim("`sloop key export > backup-key.txt` writes it out. Keep that somewhere else.")
    );

    if !std::io::stdin().is_terminal() {
        return Err(Failure::new(
            Exit::Usage,
            "the backup key has not been copied anywhere yet, and there is no terminal to ask at",
        )
        .hint("run `sloop key export` once, then this run will go through"));
    }

    anstream::eprint!(
        "{} {} ",
        style::paint("?"),
        style::dim(
            "type `decline` to take encrypted backups with no copy of the key, or \
                    anything else to stop:"
        )
    );
    let _ = std::io::Write::flush(&mut std::io::stderr());

    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|error| Failure::usage(format!("could not read the answer: {error}")))?;

    if answer.trim() != "decline" {
        return Err(Failure::new(
            Exit::Usage,
            "the backup key has not been copied anywhere yet",
        )
        .hint("run `sloop key export` once, then this run will go through"));
    }

    // Remembered, so nobody is asked twice.
    crate::commands::key::remember(&mut context.registries, scope, KeyKept::Declined)
}

/// Where sloop keeps the client tools it fetched itself.
fn fetched(context: &Context<'_>) -> PathBuf {
    acquire::fetched_dir(context.global)
}

/// `1 table`, `2 tables`, `0 rows`.
pub fn plural(count: u64, thing: &str) -> String {
    if count == 1 {
        format!("{count} {thing}")
    } else {
        format!("{count} {thing}s")
    }
}

/// A size somebody can read at a glance.
///
/// Powers of 1024, labelled the way they are actually measured. A dump is compared against
/// the last one rather than against a disk quota, so being exactly right about the unit
/// matters less than the two numbers being comparable — but calling 1024 bytes a kilobyte
/// in a tool that also prints a checksum would be sloppy in a place people look for rigour.
pub fn describe_bytes(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let mut size = bytes as f64;
    let mut unit = "B";

    for next in ["KiB", "MiB", "GiB", "TiB"] {
        if size < 1024.0 {
            break;
        }
        size /= 1024.0;
        unit = next;
    }

    if unit == "B" {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {unit}")
    }
}

/// The front of a checksum, which is all a line of output has room for.
///
/// Enough to tell two dumps apart by eye; the whole thing is in the manifest, which is
/// what anything actually verifying it reads.
pub fn short(sha256: &str) -> String {
    sha256.chars().take(12).collect()
}
