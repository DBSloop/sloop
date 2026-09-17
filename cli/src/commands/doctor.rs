//! `sloop doctor` — what this machine can and cannot do, and an offer to fix it.
//!
//! **Two questions, and they are different ones.** Are the client tools here, and can the
//! registered roles actually do the job. The first reads the machine; the second opens each
//! registered connection and asks it, which is the only way to answer it — a privilege is a
//! fact about a server, not about this laptop. `--offline` skips the second half.
//!
//! **Connecting is not phoning home.** The connections `doctor` opens are the user's own
//! databases, named in the user's own registry, and nothing leaves for anywhere else: no
//! version lookup, no telemetry, no update ping. The one path that reaches the internet is
//! still the tool install, still only after somebody says yes to it.
//!
//! **"Too old" is said as a fact, not as an opinion.** A version number on its own tells
//! nobody anything; what matters is the rule that actually bites, which is that PostgreSQL
//! refuses to let an older `pg_dump` read a newer server. So each PostgreSQL tool is
//! reported with the highest server it can dump — a sentence that is still true in five
//! years, where a hard-coded "supported since" table would have quietly rotted.
//!
//! **Exit `8` when something will break a backup, and only then.** The whole reason this
//! command checks grants is that a role one privilege short writes a dump with your stored
//! routines missing and says nothing; a health check that answered `0` in precisely that
//! case would be worse than not having one. So `8` — added rather than borrowed, because
//! `2` means the command was called wrongly and this is not that.
//!
//! **Two things deliberately do not make it red**, on the same principle that keeps the
//! install offer quiet about engines it cannot install — a check that cries wolf is a check
//! people stop reading.
//!
//! - **A database that could not be reached.** Transient and environmental: a laptop off
//!   the VPN is not a broken backup, and `backup` itself exits `3` when it matters.
//! - **A gap on the restore side.** Almost every registered database is a backup source,
//!   and almost none of their roles can restore into them — correctly, since nothing asks
//!   them to. It is reported; `restore` is where it counts.
//!
//! What is left is what runs unattended and fails where nobody is watching: this machine
//! having no usable client tools, and a role that cannot take a complete dump.

use std::path::Path;

use crate::engine::Engine;
use crate::engine::privileges::{self, Phase, Report, Verdict};
use crate::exit::Exit;
use crate::failure::Failure;
use crate::registry::file::Database;
use crate::registry::{Registries, Scope};
use crate::secret::{self, Lookup};
use crate::style;
use crate::tools::{Candidate, Inventory, Tool, acquire};

/// What `doctor` needs in order to check the second half.
pub struct Registered<'a> {
    /// The databases to ask. Both scopes, because a name that resolves is a name whose
    /// role has to work — and a global entry is just as real from inside a project.
    pub registries: &'a Registries,
    /// Which registry was resolved, and why — the same sentence every other command prints.
    pub from: String,
    /// `--password-command`, which outranks whatever route the file names.
    pub password_command: Option<&'a str>,
    /// Do not open a connection at all.
    pub offline: bool,
}

/// Report, then offer.
///
/// It cannot fail, and says so. `doctor` exists to describe a machine, and there is no
/// state of a machine it cannot describe — a missing tool is the report, not an error in
/// producing it. A database that refuses a connection is reported the same way, because a
/// server being down is a thing to say rather than a reason to stop saying anything else.
/// What it returns is the exit code, which is a different question.
pub fn run(global: &Path, may_install: bool, registered: &Registered<'_>) -> Exit {
    let fetched = acquire::fetched_dir(global);
    let mut inventory = Inventory::everything(&fetched);

    print_report(&inventory, &fetched);

    let short_of = Engine::ALL
        .into_iter()
        .filter(|&engine| !inventory.has_everything_for(engine))
        // Only the ones something can actually be done about. An engine sloop cannot
        // install here is not a failed offer, it is a line in the report saying where to
        // get it, and printing a refusal in red for it would be crying wolf every run.
        .filter(|&engine| acquire::can_offer(engine))
        .collect::<Vec<_>>();

    if may_install && !short_of.is_empty() {
        // One engine at a time, and a refusal on one is not a reason to stop asking about
        // the next: somebody may well want PostgreSQL fetched and MariaDB left alone.
        for engine in short_of {
            crate::say!();
            if let Err(failure) = acquire::ensure(engine, global) {
                failure.mention();
            }
        }
        // The report above is now out of date wherever an install succeeded, and a stale
        // report is worse than no report. Take it again.
        crate::say!();
        inventory = Inventory::everything(&fetched);
        print_report(&inventory, &fetched);
    }

    // Last, and after any install: checking a role needs the client tools that were just
    // fetched, and a section that said "no tools" above an install that just finished
    // would be answering a question nobody still has.
    crate::say!();
    let (roles_can_dump, roles) = print_privileges(registered, &inventory);

    let exit = verdict(&inventory, roles_can_dump);
    crate::report::result(serde_json::json!({
        "tools": tools_as_json(&inventory, &fetched),
        "roles": roles,
        // The two things `verdict` acts on, said out loud rather than left to be inferred
        // from the exit code — a script that has to work out *which* of them was wrong from
        // an 8 is a script that guesses.
        "every_engine_ready": Engine::ALL
            .into_iter()
            .all(|engine| inventory.has_everything_for(engine)),
        "every_role_can_dump": roles_can_dump,
    }));
    exit
}

/// What was found of every engine's tools, for a `--json` run.
///
/// **The same three facts the printed report leads with**, per engine: whether it is usable
/// at all, what is missing, and which copy of each program sloop would actually run. A
/// machine that cannot back up is a machine where one of these is wrong, and a script
/// watching a fleet wants to know which.
fn tools_as_json(inventory: &Inventory, fetched: &Path) -> serde_json::Value {
    serde_json::json!({
        "fetched_into": fetched.display().to_string(),
        "engines": Engine::ALL
            .into_iter()
            .map(|engine| serde_json::json!({
                "engine": engine.to_string(),
                "ready": inventory.has_everything_for(engine),
                "missing": inventory
                    .missing_for(engine)
                    .iter()
                    .map(|tool| tool.program())
                    .collect::<Vec<_>>(),
                "using": Tool::needed_by(engine)
                    .iter()
                    .filter_map(|&tool| {
                        let found = inventory.best(tool)?;
                        Some(serde_json::json!({
                            "tool": tool.program(),
                            "path": found.path.display().to_string(),
                            "version": found.version.map(|version| version.to_string()),
                        }))
                    })
                    .collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>(),
    })
}

/// One checked connection, for a `--json` run.
fn role_as_json(name: &str, database: &Database, report: &Report) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "connection": database.credential_key(),
        "role": report.role,
        "engine": report.server.engine.to_string(),
        "version": report.server.version.to_string(),
        "can_dump": report.can_dump(),
        "checked": true,
        "findings": report
            .findings
            .iter()
            .map(|finding| serde_json::json!({
                "id": finding.requirement.id,
                "title": finding.requirement.title,
                "phase": match finding.requirement.phase {
                    Phase::Dump => "dump",
                    Phase::Restore => "restore",
                },
                "verdict": match finding.verdict {
                    Verdict::Held => "held",
                    Verdict::Missing(_) => "missing",
                    Verdict::NotNeeded(_) => "not_needed",
                    Verdict::Unknown(_) => "unknown",
                },
                "consequence": finding.requirement.consequence,
            }))
            .collect::<Vec<_>>(),
    })
}

/// What automation reads.
///
/// Two things are wrong enough to be worth waking somebody for, and both share the same
/// property: they break a backup that runs unattended, and nothing else will say so until
/// the backup is needed. Everything else `doctor` prints is information.
fn verdict(inventory: &Inventory, roles_can_dump: bool) -> Exit {
    // Nothing usable at all is a machine that cannot run a single command. One engine
    // short of three is not: most people use one.
    let any_engine_works = Engine::ALL
        .into_iter()
        .any(|engine| inventory.has_everything_for(engine));

    if any_engine_works && roles_can_dump {
        Exit::Success
    } else {
        Exit::Unhealthy
    }
}

fn print_report(inventory: &Inventory, fetched: &Path) {
    crate::say!("{}", style::heading("Client tools"));
    crate::say!(
        "{}",
        style::dim(
            "  sloop drives the tools each engine ships. It does not bundle them: mysqldump \
             is GPL v2, and a bundled pg_dump older than your server simply fails."
        )
    );

    for engine in Engine::ALL {
        crate::say!();
        print_engine(inventory, engine);
    }

    crate::say!();
    crate::say!(
        "{} {}",
        style::label("  sloop keeps what it fetches in"),
        fetched.display()
    );
}

fn print_engine(inventory: &Inventory, engine: Engine) {
    let ready = inventory.has_everything_for(engine);
    let mark = if ready { "ready" } else { "not ready" };
    crate::say!(
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
        crate::say!("    {}", style::dim(&format!("→ {what}")));
    }

    for &tool in Tool::needed_by(engine) {
        let every = inventory.every(tool);
        match every.split_first() {
            None => crate::say!(
                "    {:<14} {}",
                tool.to_string(),
                style::dim(&format!("missing — {}", tool.what_for()))
            ),
            Some((best, rest)) => {
                crate::say!("    {:<14} {}", tool.to_string(), describe(tool, best));
                for other in rest {
                    crate::say!(
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

// ---------------------------------------------------------------------------------------
// Privileges
// ---------------------------------------------------------------------------------------

/// The second half of the report: can each registered role actually do the job.
///
/// Returns whether every role that was checked can take a complete dump — which is the
/// half of this that reaches the exit code. A database that could not be reached is not
/// counted against it; see the module comment for why.
fn print_privileges(
    registered: &Registered<'_>,
    inventory: &Inventory,
) -> (bool, Vec<serde_json::Value>) {
    crate::say!("{}", style::heading("Privileges"));
    crate::say!(
        "{}",
        style::dim(
            "  Every engine wants more than SELECT, and not every gap is loud: short of one \
             grant, MySQL and MariaDB write a dump with your triggers and stored routines \
             left out of it, say nothing, and exit 0. So sloop asks each registered \
             connection what its role can really do, rather than finding out during a backup."
        )
    );

    if registered.registries.is_empty() {
        crate::say!();
        crate::say!(
            "{}",
            style::dim(&format!(
                "  Nothing is registered in {}, so there is no role to check. \
                 What each engine needs:",
                registered.from
            ))
        );
        print_the_documented_minimum(inventory);
        // Nothing was asked, so nothing was found wanting. A machine with no databases
        // registered yet is not an unhealthy one.
        return (true, Vec::new());
    }

    if registered.offline {
        crate::say!();
        crate::say!(
            "{}",
            style::dim("  --offline, so nothing was asked. What each engine needs:")
        );
        print_the_documented_minimum(inventory);
        return (true, Vec::new());
    }

    let mut every_role_can_dump = true;
    let mut checked: Vec<serde_json::Value> = Vec::new();

    for (scope, name, database) in registered.registries.all() {
        crate::say!();
        match check(database, registered, scope, inventory) {
            Ok(report) => {
                every_role_can_dump &= report.can_dump();
                checked.push(role_as_json(name, database, &report));
                print_one(name, database, &report);
            }
            // A server that is down, a keyring that is locked, a password command that is
            // not installed: all of them stop this one check and none of them is a reason
            // to abandon the rest of the report, or the databases after it.
            Err(failure) => {
                crate::say!(
                    "  {}  {}",
                    style::paint(name),
                    style::dim(&database.credential_key())
                );
                crate::say!("    {}", style::dim("could not be checked"));
                checked.push(serde_json::json!({
                    "name": name,
                    "connection": database.credential_key(),
                    "checked": false,
                    "error": failure.message(),
                }));
                failure.mention();
            }
        }
    }

    (every_role_can_dump, checked)
}

/// Open the connection and ask it.
fn check(
    database: &Database,
    registered: &Registered<'_>,
    scope: Scope,
    inventory: &Inventory,
) -> Result<Report, Failure> {
    let key = database.credential_key();
    let route = database.password.overridden_by(registered.password_command);
    // The encrypted file sits beside the registry that names the database, so a project
    // entry and a global one of the same name read from two different stores.
    let vault = registered
        .registries
        .vault_in(scope)
        .unwrap_or_else(crate::secret::sealed::Vault::nowhere);
    let resolved = secret::resolve(
        &route,
        &Lookup {
            key: &key,
            vault: &vault,
        },
    )?;

    for note in &resolved.notes {
        crate::say!("    {}", style::dim(note));
    }

    // The inventory this command has already built and already printed, rather than
    // whatever `PATH` happens to hold: a report that says a tool was found and then cannot
    // find it is a report nobody can act on.
    inventory
        .adapter_for(database.engine)
        .check_privileges(&database.target(&resolved.secret))
}

/// One database's findings.
fn print_one(name: &str, database: &Database, report: &Report) {
    crate::say!(
        "  {}  {}",
        style::paint(name),
        style::dim(&database.credential_key())
    );
    crate::say!(
        "    {}",
        style::dim(&format!(
            "{} {}{}, connected as {}",
            report.server.engine,
            report.server.version,
            if report.server.tls { ", TLS" } else { "" },
            report.role
        ))
    );

    for phase in privileges::PHASES {
        let rows: Vec<_> = report
            .findings
            .iter()
            .filter(|finding| finding.requirement.phase == phase)
            .collect();

        let needed = rows
            .iter()
            .filter(|finding| !matches!(finding.verdict, Verdict::NotNeeded(_)))
            .count();
        let gaps = rows
            .iter()
            .filter(|finding| finding.verdict.is_missing())
            .count();

        let summary = if gaps == 0 {
            format!("— ready {}", phase.heading())
        } else {
            format!("— {}: {gaps} of {needed} missing", phase.heading())
        };
        crate::say!("    {}", style::dim(&summary));

        // Only the gaps get elaborated. A held privilege is a line nobody needs to read,
        // and printing eleven of them is how the two that matter get scrolled past.
        for finding in rows.iter().filter(|row| row.verdict.is_missing()) {
            let Verdict::Missing(detail) = &finding.verdict else {
                continue;
            };
            crate::say!("      {} {}", style::paint("✗"), finding.requirement.title);
            if !detail.is_empty() {
                crate::say!("          {}", style::dim(detail));
            }
            crate::say!("          {}", style::dim(finding.requirement.consequence));
        }

        // Anything that could not be established at all. Rare, and never guessed at.
        for finding in &rows {
            if let Verdict::Unknown(why) = &finding.verdict {
                crate::say!("      {} {}", style::dim("?"), finding.requirement.title);
                crate::say!("          {}", style::dim(why));
            }
        }

        // Under the gaps they close, and never merged with the other phase's — see
        // `Report::remedy_for` for why that distinction is worth the extra lines.
        let remedy = report.remedy_for(phase, &database.database);
        if !remedy.is_empty() {
            crate::say!("      {}", style::dim("grant:"));
            for statement in remedy {
                crate::say!("        {statement}");
            }
        }
    }
}

/// The table, with no server to measure against.
///
/// Printed where there is nothing registered — which is every machine until `db add`
/// exists — and under `--offline`. Only for the engines whose tools are actually here:
/// a machine with no MySQL client has no use for MySQL's grant list today.
fn print_the_documented_minimum(inventory: &Inventory) {
    let present: Vec<_> = Engine::ALL
        .into_iter()
        .filter(|&engine| inventory.has_everything_for(engine))
        .collect();

    // A machine with no engine's tools at all still gets the lists; the section would
    // otherwise be a heading, a sentence and nothing under it.
    let showing = if present.is_empty() {
        Engine::ALL.to_vec()
    } else {
        present
    };

    for engine in showing {
        crate::say!();
        crate::say!("  {}", style::paint(&format!("{engine}")));

        for phase in [Phase::Dump, Phase::Restore] {
            crate::say!("    {}", style::dim(&format!("— {}", phase.heading())));
            // Title first and the grants under it, rather than two columns: a row like
            // "USAGE on each schema, SELECT on each table and sequence" is wider than any
            // column worth having, and the thing worth reading is what it lets you do.
            for row in privileges::for_engine(engine)
                .iter()
                .filter(|row| row.phase == phase)
            {
                crate::say!("      {}", row.title);
                crate::say!("        {}", style::dim(&row.privileges.join(", ")));
            }
        }

        if !privileges::waived_by(engine).is_empty() {
            crate::say!(
                "    {}",
                style::dim("— what the manual asks for and sloop does not need")
            );
            for waived in privileges::waived_by(engine) {
                crate::say!("      {}", waived.privilege);
                crate::say!("        {}", style::dim(waived.because));
            }
        }
    }
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
    fn nothing_usable_is_a_finding() {
        let nowhere = std::env::temp_dir().join("sloop-doctor-verdict-that-does-not-exist");

        let nothing = Inventory::default();
        assert_eq!(
            verdict(&nothing, true).code(),
            8,
            "a machine with no tools at all"
        );

        // Whatever this machine really has: if any engine is complete, the code is 0.
        let everything = Inventory::everything(&nowhere);
        let expected = u8::from(
            !Engine::ALL
                .into_iter()
                .any(|engine| everything.has_everything_for(engine)),
        ) * 8;
        assert_eq!(verdict(&everything, true).code(), expected);

        // And the rule is about whole engines, not about tool counts: an inventory that
        // only ever looked for PostgreSQL decides the verdict on PostgreSQL alone.
        let one_engine = Inventory::for_engine(Engine::Postgres, &nowhere);
        assert_eq!(
            verdict(&one_engine, true).code(),
            u8::from(!one_engine.has_everything_for(Engine::Postgres)) * 8
        );
    }

    /// The other half of the same code, and the reason it was added: a machine with every
    /// tool it needs, whose backup role cannot take a complete dump, is not healthy.
    #[test]
    fn a_role_that_cannot_dump_is_a_finding_even_with_every_tool_installed() {
        let nowhere = std::env::temp_dir().join("sloop-doctor-verdict-that-does-not-exist");
        let everything = Inventory::everything(&nowhere);

        // Only meaningful on a machine that has the tools; elsewhere the first rule
        // already decides it, and this assertion would be testing nothing.
        if Engine::ALL
            .into_iter()
            .any(|engine| everything.has_everything_for(engine))
        {
            assert_eq!(verdict(&everything, true).code(), 0);
            assert_eq!(verdict(&everything, false).code(), 8);
        }

        // And with no tools either, it stays a finding rather than becoming two.
        assert_eq!(verdict(&Inventory::default(), false).code(), 8);
    }
}
