//! `sloop doctor` — what this machine can and cannot do, and an offer to fix it.
//!
//! **It reads the machine and nothing else.** No connection is opened, no registry is
//! consulted and no version is looked up on the internet, so `doctor` is safe to run
//! anywhere and cannot be the "update ping" this project promised never to have. The one
//! thing that reaches the network is the install, and only after somebody says yes to it.
//!
//! **"Too old" is said as a fact, not as an opinion.** A version number on its own tells
//! nobody anything; what matters is the rule that actually bites, which is that PostgreSQL
//! refuses to let an older `pg_dump` read a newer server. So each PostgreSQL tool is
//! reported with the highest server it can dump — a sentence that is still true in five
//! years, where a hard-coded "supported since" table would have quietly rotted.

use std::path::Path;

use crate::engine::Engine;
use crate::exit::Exit;
use crate::style;
use crate::tools::{Candidate, Inventory, Tool, acquire};

/// Report, then offer.
///
/// It cannot fail, and says so. `doctor` exists to describe a machine, and there is no
/// state of a machine it cannot describe — a missing tool is the report, not an error in
/// producing it. What it returns is the exit code, which is a different question.
pub fn run(global: &Path, may_install: bool) -> Exit {
    let fetched = acquire::fetched_dir(global);
    let inventory = Inventory::everything(&fetched);

    print_report(&inventory, &fetched);

    let short_of = Engine::ALL
        .into_iter()
        .filter(|&engine| !inventory.has_everything_for(engine))
        .collect::<Vec<_>>();

    if short_of.is_empty() {
        return Exit::Success;
    }

    // Only the ones something can actually be done about. An engine sloop cannot install
    // here is not a failed offer, it is a line in the report saying where to get it, and
    // printing a refusal in red for it would be crying wolf on every single run.
    let can_offer: Vec<_> = short_of
        .iter()
        .copied()
        .filter(|&engine| acquire::can_offer(engine))
        .collect();

    if may_install && !can_offer.is_empty() {
        // One engine at a time, and a refusal on one is not a reason to stop asking about
        // the next: somebody may well want PostgreSQL fetched and MariaDB left alone.
        for engine in can_offer {
            anstream::println!();
            if let Err(failure) = acquire::ensure(engine, global) {
                failure.report();
            }
        }
        // The report above is now out of date wherever an install succeeded, and a stale
        // report is worse than no report. Take it again.
        anstream::println!();
        let after = Inventory::everything(&fetched);
        print_report(&after, &fetched);
        return verdict(&after);
    }

    verdict(&inventory)
}

/// Nothing usable at all is a machine that cannot run a single command, and automation
/// should hear about it. One engine short of three is not: most people use one.
fn verdict(inventory: &Inventory) -> Exit {
    if Engine::ALL
        .into_iter()
        .any(|engine| inventory.has_everything_for(engine))
    {
        Exit::Success
    } else {
        Exit::Usage
    }
}

fn print_report(inventory: &Inventory, fetched: &Path) {
    anstream::println!("{}", style::heading("Client tools"));
    anstream::println!(
        "{}",
        style::dim(
            "  sloop drives the tools each engine ships. It does not bundle them: mysqldump \
             is GPL v2, and a bundled pg_dump older than your server simply fails."
        )
    );

    for engine in Engine::ALL {
        anstream::println!();
        print_engine(inventory, engine);
    }

    anstream::println!();
    anstream::println!(
        "{} {}",
        style::label("  sloop keeps what it fetches in"),
        fetched.display()
    );
}

fn print_engine(inventory: &Inventory, engine: Engine) {
    let ready = inventory.has_everything_for(engine);
    let mark = if ready { "ready" } else { "not ready" };
    anstream::println!(
        "  {} {}",
        style::paint(&format!("{engine}")),
        style::dim(&format!("— {mark}"))
    );

    if !ready {
        // What to do about it, said in the report rather than only in an error somebody
        // has to provoke. `doctor` is the place people come to find this out.
        let what = if acquire::can_offer(engine) {
            "sloop can install these — run `sloop doctor` in a terminal".to_owned()
        } else {
            acquire::how_to_install(engine)
        };
        anstream::println!("    {}", style::dim(&format!("→ {what}")));
    }

    for &tool in Tool::needed_by(engine) {
        let every = inventory.every(tool);
        match every.split_first() {
            None => anstream::println!(
                "    {:<14} {}",
                tool.to_string(),
                style::dim(&format!("missing — {}", tool.what_for()))
            ),
            Some((best, rest)) => {
                anstream::println!("    {:<14} {}", tool.to_string(), describe(tool, best));
                for other in rest {
                    anstream::println!(
                        "    {:<14} {}",
                        "",
                        style::dim(&format!("also {}", describe_plain(other)))
                    );
                }
            }
        }
    }
}

fn describe(tool: Tool, candidate: &Candidate) -> String {
    let mut line = describe_plain(candidate);

    // The rule that actually bites, stated as a fact about this copy rather than as a
    // version number somebody has to interpret. It is said only where it is true: PostgreSQL
    // will not let an older `pg_dump` read a newer server, and `pg_restore` cannot read an
    // archive a newer one wrote — but `psql` has no such limit, and MySQL has no such rule
    // at all, so claiming one there would invent a restriction the engine does not have.
    if let Some(version) = candidate.version {
        use std::fmt::Write as _;
        let _ = match tool {
            Tool::PgDump => write!(line, " — dumps servers up to PostgreSQL {}", version.major),
            Tool::PgRestore => write!(line, " — reads dumps up to PostgreSQL {}", version.major),
            _ => Ok(()),
        };
    }

    line
}

fn describe_plain(candidate: &Candidate) -> String {
    let version = candidate
        .version
        .map_or_else(|| "version unreadable".to_owned(), |v| v.to_string());

    format!(
        "{version}  {}  {}",
        style::dim(candidate.source.describe()),
        style::dim(&candidate.path.display().to_string())
    )
}

#[cfg(test)]
mod tests {
    use super::verdict;
    use crate::engine::Engine;
    use crate::tools::Inventory;

    /// What automation reads. A machine that cannot run a single command should say so;
    /// a machine missing one engine of three should not, because most people use one.
    #[test]
    fn nothing_usable_is_the_only_thing_that_exits_non_zero() {
        let nowhere = std::env::temp_dir().join("sloop-doctor-verdict-that-does-not-exist");

        let nothing = Inventory::default();
        assert_eq!(
            verdict(&nothing).code(),
            2,
            "a machine with no tools at all"
        );

        // Whatever this machine really has: if any engine is complete, the code is 0.
        let everything = Inventory::everything(&nowhere);
        let expected = u8::from(
            !Engine::ALL
                .into_iter()
                .any(|engine| everything.has_everything_for(engine)),
        ) * 2;
        assert_eq!(verdict(&everything).code(), expected);

        // And the rule is about whole engines, not about tool counts: an inventory that
        // only ever looked for PostgreSQL decides the verdict on PostgreSQL alone.
        let one_engine = Inventory::for_engine(Engine::Postgres, &nowhere);
        assert_eq!(
            verdict(&one_engine).code(),
            u8::from(!one_engine.has_everything_for(Engine::Postgres)) * 2
        );
    }
}
