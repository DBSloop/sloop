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

#[cfg(test)]
#[path = "mirror_tests.rs"]
mod tests;

use std::path::{Path, PathBuf};

use crate::consent::{Consent, Destroying};
use crate::engine::{Adapter, Target};
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::registry::file::Database;
use crate::registry::{Registries, Scope};
use crate::secret::{Lookup, Secret, resolve};
use crate::style;
use crate::tools::{Inventory, acquire};
use crate::verify::{self, Side};

use super::backup::{describe_bytes, plural};

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

/// Copy `source` over `destination`, exactly.
pub fn run(context: &Context<'_>, source: &str, destination: &str, safe: bool) -> Outcome<Exit> {
    let (from_scope, from) = context.registries.find(source)?;
    let from = from.clone();
    let (into_scope, into) = context.registries.find(destination)?;
    let into = into.clone();

    refuse_a_self_mirror(source, &from, destination, &into)?;

    if from.engine != into.engine {
        return Err(Failure::usage(format!(
            "{source} is {} and {destination} is {}",
            from.engine, into.engine
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

    let reading = secret_for(context, from_scope, &from)?;
    let writing = secret_for(context, into_scope, &into)?;
    let source_target = from.target(&reading);
    let destination_target = into.target(&writing);

    let fetched = acquire::fetched_dir(context.global);
    let adapter = Inventory::for_engine(from.engine, &fetched).adapter_for(from.engine);

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
    let before = announce_both(adapter.as_ref(), &source_target, &destination_target, mode)?;

    if !context.consent.typed(&destroying)?.granted() {
        anstream::println!("{}", style::dim("left alone."));
        return Ok(Exit::Success);
    }

    let cleared = adapter.clear_contents(&destination_target)?;
    if cleared > 0 {
        anstream::println!(
            "  {}",
            style::dim(&format!("cleared {}", plural(cleared, "table")))
        );
    }

    let started = std::time::Instant::now();
    let kept = if safe {
        Some(through_a_file(
            adapter.as_ref(),
            &source_target,
            &destination_target,
        )?)
    } else {
        // Straight down an OS pipe between the two clients — no file, and nothing through
        // this process. See `Adapter::copy_into`.
        adapter.copy_into(&source_target, &destination_target)?;
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
        &Side::counted(adapter.as_ref(), &destination_target, mode)?,
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
    if from.port != into.port
        || from.database != into.database
        || !same_host(&from.host, &into.host)
    {
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
    .hint("a mirror drops the destination's contents, so this would destroy the source"))
}

/// Is this the same machine, however the two were spelled?
fn same_host(one: &str, two: &str) -> bool {
    /// Every spelling of "this machine" that resolves to the same place.
    const HERE: [&str; 4] = ["localhost", "127.0.0.1", "::1", "[::1]"];

    let here = |host: &str| HERE.iter().any(|known| host.eq_ignore_ascii_case(known));
    one.eq_ignore_ascii_case(two) || (here(one) && here(two))
}

/// Resolve one end's password through whichever of the four routes its record names.
fn secret_for(context: &Context<'_>, scope: Scope, record: &Database) -> Outcome<Secret> {
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
