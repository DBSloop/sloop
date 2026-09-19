//! `sloop restore` — put a stored backup back into a registered database.
//!
//! **It replaces the contents and never drops the database.** The database somebody is
//! restoring into has an owner, a set of grants and a connection string that an application
//! is configured with; dropping and recreating it would take all three away to solve a
//! problem that is about tables. So the destination is emptied — every schema that is not
//! the system's on PostgreSQL, every table, view, routine and event on the MySQL family —
//! and the dump is loaded into what is left. See [`crate::engine::Adapter::clear_contents`].
//!
//! **Nothing plaintext touches the disk.** An encrypted backup is decrypted in memory and
//! written straight into the client's standard input, so the thing `R11` protected is never
//! written out in the clear to restore it. The cost is that a PostgreSQL archive arriving
//! down a pipe cannot be restored in parallel; an unencrypted dump is already a file and
//! takes the parallel path.
//!
//! **The dump is hashed before anything is replaced.** A restore is rare, high-stakes, and
//! irreversible from the destination's point of view — reading the file twice to know it is
//! the file the manifest describes is the cheapest insurance this command can buy. A
//! backup that fails that check is refused, and `--force` is the way past it, because
//! somebody in a disaster with one damaged backup should be allowed to try it.
//!
//! **It verifies afterwards, against the manifest.** The per-table counts in the manifest
//! were taken from the source immediately before the dump, which is exactly what the dump
//! describes — a better comparison than asking a live source now. `R10`'s [`crate::verify`]
//! does the comparing, so `restore`, `mirror` and `sync` all say the same words about the
//! same findings.

#[cfg(test)]
#[path = "restore_tests.rs"]
mod tests;

use std::path::Path;

use crate::backup::manifest::Manifest;
use crate::backup::store::{self, Check, State, Stored};
use crate::consent::{Consent, Destroying};
use crate::crypt::{self, PrivateKey};
use crate::engine::{Adapter, Target};
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::registry::file::Database;
use crate::registry::{Registries, Scope};
use crate::secret::{Lookup, resolve};
use crate::ssh::tunnel::Tunnels;
use crate::style;
use crate::tools::{Inventory, acquire};
use crate::verify::{self, Side};

use super::backup::{describe_bytes, plural, short};

/// Everything `restore` needs from the outside.
pub struct Context<'a> {
    /// Both registries, already open.
    pub registries: Registries,
    /// The global store, which is where sloop's own copy of the client tools lives.
    pub global: &'a Path,
    /// `--password-command`, which outranks whatever route a record names.
    pub password_command: Option<&'a str>,
    /// What this run was given permission to do — see [`crate::consent`].
    pub consent: Consent<'a>,
    /// Every SSH forward this session holds — see [`super::reach`]. Shared with every
    /// other command in the run, so a menu session authenticates once.
    pub tunnels: &'a Tunnels,
}

/// Put a backup back.
/// Where this scope keeps its encrypted passwords, or nowhere for a route that needs none.
fn vault_for(context: &Context<'_>, scope: Scope) -> crate::secret::sealed::Vault<'static> {
    context
        .registries
        .vault_in(scope)
        .unwrap_or_else(crate::secret::sealed::Vault::nowhere)
}

pub fn run(context: &Context<'_>, name: &str, from: Option<&str>) -> Outcome<Exit> {
    let (scope, record) = context.registries.find(name)?;
    let record = record.clone();

    // **The paperwork first, before a password is fetched or a socket opened.** A scheduled
    // restore that named the wrong database should be told so without contacting anything.
    let destroying = Destroying {
        named: &record.database,
        noun: "database",
        action: "replacing a database's contents",
    };
    context.consent.checked_early(&destroying)?;

    // **One run at a time per database.** A restore racing the backup that is still writing
    // the copy it is reading would be the worst possible pair of runs to allow.
    let _held = locked(context, scope, name, "restore")?;

    let root = context.registries.root_in(scope).ok_or_else(|| {
        Failure::usage("there is nowhere to look for backups")
            .hint("run this against a project registry, or the global store")
    })?;

    let chosen = choose(&root, name, from, context.consent.forced())?;
    let manifest = manifest_of(&chosen)?;

    crate::say!(
        "{} {}",
        style::paint("restoring"),
        style::dim(&chosen.directory.display().to_string())
    );
    for line in describe(&manifest) {
        crate::say!("  {}", style::dim(&line));
    }

    same_engine(&manifest, &record, name)?;

    let key = record.credential_key();
    let route = record.password.overridden_by(context.password_command);
    let vault = vault_for(context, scope);
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

    // **Before the destination is announced**, because what is announced is a server that
    // answered — and over SSH, answering means the forward is up.
    let at = super::reach(&record, context.tunnels, &context.registries, scope)?;
    if let Some(server) = &at.through {
        crate::say!("  {}", style::dim(&format!("through {server}")));
    }

    let target = record.target_at(&resolved.secret, &at.host, at.port);
    let adapter = Inventory::for_engine(record.engine, &acquire::fetched_dir(context.global))
        .adapter_for(record.engine);
    announce_destination(adapter.as_ref(), &target, &record.database)?;

    // The private key, before anything is cleared: a restore that empties a database and
    // then finds it cannot read the dump is the worst possible order to do this in.
    let opening = key_for(context, scope, &manifest)?;

    if !context.consent.typed(&destroying)?.granted() {
        crate::say!("{}", style::dim("left alone."));
        return Ok(Exit::Success);
    }

    if crate::report::would(&format!(
        "replace the contents of {} from {}",
        target.describe(),
        chosen.directory.display()
    )) {
        return Ok(Exit::Success);
    }

    replace(
        adapter.as_ref(),
        &target,
        &Loading {
            label: name,
            database: &record.database,
            chosen: &chosen,
            manifest: &manifest,
            private: opening.as_ref(),
        },
    )?;

    // **Against the manifest**, whose counts came from the source at the moment of the dump.
    crate::say!(
        "  {}",
        style::dim("checking it against the counts the manifest recorded")
    );
    let mode = verify::Mode::from_environment();
    let comparison = verify::Comparison::of(
        mode,
        &Side::from_manifest(&manifest),
        &Side::counted(adapter.as_ref(), &target, mode)?,
    );
    for line in comparison.describe() {
        crate::say!("  {}", style::dim(&line));
    }

    record_what_it_moved(context, scope, name, manifest.dump.bytes);

    crate::report::result(serde_json::json!({
        "name": name,
        "destination": target.describe(),
        "from": chosen.directory.display().to_string(),
        "taken": manifest.taken.utc,
        "verified": comparison.landed(),
        "tables": comparison.as_json(),
    }));
    Ok(comparison.exit())
}

/// **`R27a`: a restore being written is bytes out**, and sloop wrote them, so the figure is
/// exact — unlike anything about traffic on the wire, which no engine reports per database.
///
/// Best effort, for the reason the backup side gives: the restore has already landed, and
/// failing it to record a statistic would be the tail wagging the dog.
fn record_what_it_moved(context: &Context<'_>, scope: Scope, name: &str, bytes: u64) {
    if let Some(store) = context.registries.store() {
        let _ = crate::service::traffic::moved(
            store,
            scope,
            name,
            0,
            i64::try_from(bytes).unwrap_or(0),
        );
    }
}

/// The engine a backup came from has to be the engine it is going into.
///
/// A PostgreSQL archive fed to `mysql` is a wall of syntax errors, and saying so here is
/// kinder than letting the client say it.
fn same_engine(manifest: &Manifest, record: &Database, name: &str) -> Outcome<()> {
    if manifest.engine == record.engine {
        return Ok(());
    }

    Err(Failure::usage(format!(
        "that backup is {} and {name} is {}",
        manifest.engine, record.engine
    ))
    .hint(
        "restoring across engines is not built: a dump is written in its own engine's dialect, \
         and reinterpreting it would corrupt quietly rather than fail loudly. Restore this \
         backup into a database on the engine it was taken from",
    ))
}

/// What the chosen backup says about itself, or why it cannot say anything.
///
/// **A directory with no manifest is a run that did not finish**, so nothing recorded the
/// engine, the row counts or the checksum — and a restore from one would be putting back
/// something nothing has ever checked.
fn manifest_of(chosen: &Stored) -> Outcome<Manifest> {
    chosen.manifest.clone().ok_or_else(|| {
        Failure::new(
            Exit::Usage,
            format!(
                "{} has no manifest to restore from",
                chosen.directory.display()
            ),
        )
        .hint(
            "a backup with no manifest is one whose run did not finish — `sloop backups \
             list` marks the ones that did",
        )
    })
}

/// Which backup this run is putting back.
///
/// Take this database's lock, from whichever store its registry lives in.
fn locked(
    context: &Context<'_>,
    scope: Scope,
    name: &str,
    doing: &str,
) -> Outcome<crate::lock::Held> {
    let store = context.registries.root_in(scope).ok_or_else(|| {
        Failure::new(
            Exit::Usage,
            format!("there is no {} store to lock against", scope.label()),
        )
        .hint("`sloop init` starts a registry in this directory; `--global` uses the global one")
    })?;
    crate::lock::take(&store, name, doing)
}

/// `--from` takes the directory's own name — `20260916T031500Z`, or `latest` for the copy
/// `--replace` keeps — because that is what a listing shows and what a person can copy out
/// of it. Left out, it is the newest complete backup, which is what somebody in a hurry
/// means.
fn choose(root: &Path, label: &str, from: Option<&str>, forced: bool) -> Outcome<Stored> {
    let wanted = store::label_for(label);
    // Hashed, not stat-ed: see this module's note on why a restore pays for that.
    let found = store::scan(root, Check::Checksum)?;
    let mine: Vec<Stored> = found
        .backups
        .into_iter()
        .filter(|stored| stored.label == wanted)
        .collect();

    if mine.is_empty() {
        return Err(
            Failure::usage(format!("nothing has been backed up under {label}")).hint(
                "`sloop backups list` shows what there is, and `sloop backup <name>` takes one",
            ),
        );
    }

    let chosen = match from {
        Some(asked) => mine
            .into_iter()
            .find(|stored| {
                stored
                    .directory
                    .file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .is_some_and(|name| name == asked)
            })
            .ok_or_else(|| {
                Failure::usage(format!("{label} has no backup called {asked}")).hint(
                    "`--from` takes the directory's own name, as `sloop backups list` shows it",
                )
            })?,
        // Newest first out of `scan`, so the first complete one is the newest complete one.
        None => mine
            .iter()
            .find(|stored| stored.is_complete())
            .or_else(|| mine.first())
            .cloned()
            .ok_or_else(|| {
                Failure::usage(format!("nothing has been backed up under {label}"))
                    .hint(format!("take the first one: `sloop backup {label}`"))
            })?,
    };

    match &chosen.state {
        State::Complete => Ok(chosen),
        _ if forced => {
            crate::note!(
                "{} {}",
                style::dim("--force: restoring a backup that did not check out —"),
                style::dim(chosen.problem().unwrap_or("it is not a finished backup"))
            );
            // Said before the question, because it is the thing worth changing your mind
            // over: the destination is emptied before the dump is loaded, so a dump that
            // will not load leaves nothing behind it.
            crate::note!(
                "  {}",
                style::dim(
                    "if it will not load, the destination will be left empty — the clearing \
                     happens first"
                )
            );
            Ok(chosen)
        }
        State::Unfinished => Err(Failure::new(
            Exit::Usage,
            format!(
                "{} is not a finished backup — it has no manifest",
                chosen.directory.display()
            ),
        )
        .hint("`sloop backups list` shows which ones are whole; --force restores it anyway")),
        State::Unreadable(why) | State::Damaged(why) => Err(Failure::new(
            Exit::Usage,
            format!("{} cannot be trusted: {why}", chosen.directory.display()),
        )
        .hint("--force restores it anyway, which is the right call when it is all there is")),
    }
}

/// The private key this backup needs, when it needs one.
///
/// Fetched **before** the destination is touched. A restore that clears a database and then
/// discovers the key is on another machine has turned a recoverable situation into a lost
/// one, so the order here is load-bearing rather than tidy.
fn key_for(
    context: &Context<'_>,
    scope: Scope,
    manifest: &Manifest,
) -> Outcome<Option<PrivateKey>> {
    let Some(encrypted) = &manifest.dump.encryption else {
        return Ok(None);
    };

    let encryption = context
        .registries
        .in_scope(scope)
        .and_then(|registry| registry.encryption())
        .ok_or_else(|| {
            Failure::new(
                Exit::Restore,
                format!(
                    "that backup is encrypted to {} and this registry has no key",
                    encrypted.recipient
                ),
            )
            .hint("`sloop key import` takes the key exported from the machine that made it")
        })?
        .clone();

    if encryption.public_key.to_string() != encrypted.recipient {
        return Err(Failure::new(
            Exit::Restore,
            format!(
                "that backup is encrypted to {} and this registry's key is {}",
                encrypted.recipient, encryption.public_key
            ),
        )
        .hint("the key that made a backup is the only key that can open it — `sloop key import`"));
    }

    let vault = context
        .registries
        .vault_in(scope)
        .unwrap_or_else(crate::secret::sealed::Vault::nowhere);
    crypt::Store {
        route: &encryption.private_key,
        vault: &vault,
    }
    .fetch(&encryption.public_key)
    .map(Some)
}

/// Feed the dump into the destination, decrypting on the way if it is encrypted.
fn load(
    adapter: &dyn Adapter,
    target: &Target<'_>,
    chosen: &Stored,
    manifest: &Manifest,
    private: Option<&PrivateKey>,
) -> Outcome<()> {
    let dump = chosen.directory.join(&manifest.dump.file);

    match private {
        // **Decrypted into the client, not onto the disk.** See this module's note.
        Some(key) => {
            let mut plaintext = crypt::opened(&dump, key)?;
            adapter.restore_into(target, &mut plaintext)
        }
        // Nothing to protect and the file is already there, so take the fast path — which
        // for PostgreSQL means a parallel restore.
        None => adapter.restore(target, &dump),
    }
}

/// What the manifest says about the backup being put back.
fn describe(manifest: &Manifest) -> Vec<String> {
    vec![
        format!(
            "taken {} ({})",
            manifest.taken_locally().readable(),
            manifest.taken.utc
        ),
        format!(
            "{} of {} {}{}, {}, sha256 {}",
            describe_bytes(manifest.dump.bytes),
            manifest.engine,
            manifest.server.version,
            if manifest.dump.encryption.is_some() {
                ", encrypted"
            } else {
                ""
            },
            plural(manifest.rows, "row"),
            short(&manifest.dump.sha256)
        ),
    ]
}

/// A count as the width [`plural`] takes.
fn count(of: usize) -> u64 {
    u64::try_from(of).unwrap_or(u64::MAX)
}

/// Say where this is going, and what is in the way.
///
/// **Before the question rather than after it.** Somebody agreeing to replace a database's
/// contents should be looking at how many rows are in it while they agree, and a destination
/// that turns out not to be empty is exactly the thing worth finding out one line earlier.
fn announce_destination(adapter: &dyn Adapter, target: &Target<'_>, database: &str) -> Outcome<()> {
    let server = adapter.probe(target)?;
    crate::say!(
        "  {}",
        style::dim(&format!(
            "into {database} — {} {}{}",
            server.engine,
            server.version,
            if server.tls { ", TLS" } else { "" }
        ))
    );

    // Best effort: a role that cannot count is about to find that out from the restore
    // itself, and refusing to restore because the tables could not be listed would be the
    // wrong way round.
    let existing = adapter.row_counts(target).unwrap_or_default();
    if existing.is_empty() {
        crate::say!("  {}", style::dim("it is empty"));
        return Ok(());
    }

    let rows: u64 = existing.iter().map(|count| count.rows).sum();
    crate::say!(
        "  {}",
        style::dim(&format!(
            "replacing what is there now — {}, {}",
            plural(count(existing.len()), "table"),
            plural(rows, "row")
        ))
    );

    Ok(())
}

/// What [`replace`] is working on, named rather than positional.
struct Loading<'a> {
    /// The name the database is registered under.
    label: &'a str,
    /// Its own name on the server, for the sentence about what state it is in.
    database: &'a str,
    /// The backup being put back.
    chosen: &'a Stored,
    /// What that backup says about itself.
    manifest: &'a Manifest,
    /// The key, when the dump is encrypted.
    private: Option<&'a PrivateKey>,
}

/// Empty the destination and load the dump into it.
///
/// **The two halves are together because the gap between them is the dangerous part.** From
/// the moment the clearing succeeds until the moment the load does, the database has nothing
/// in it — so the one thing this must never do is fail quietly in between and leave somebody
/// reading a decryption error with no idea their database is now empty.
fn replace(adapter: &dyn Adapter, target: &Target<'_>, loading: &Loading<'_>) -> Outcome<()> {
    let cleared = adapter.clear_contents(target)?;
    if cleared > 0 {
        crate::say!(
            "  {}",
            style::dim(&format!("cleared {}", plural(cleared, "table")))
        );
    }

    let started = std::time::Instant::now();
    if let Err(failure) = load(
        adapter,
        target,
        loading.chosen,
        loading.manifest,
        loading.private,
    ) {
        crate::note!(
            "{}",
            style::paint(&format!(
                "{} was cleared and the restore did not finish — it is empty now",
                loading.database
            ))
        );
        crate::note!(
            "  {}",
            style::dim(&format!(
                "`sloop backups list {}` shows the other backups there are",
                style::as_argument(loading.label)
            ))
        );
        return Err(failure);
    }

    crate::say!(
        "  {}",
        style::dim(&format!(
            "loaded in {:.1}s",
            started.elapsed().as_secs_f64()
        ))
    );

    Ok(())
}
