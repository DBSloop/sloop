//! What a backup role actually has to be able to do, per engine.
//!
//! **This module exists because of one afternoon.** R5's MySQL fixture failed with
//! `alpha has insufficient privileges to SHOW CREATE FUNCTION widget_count`, and finding
//! out why took longer than writing the adapter did. The general form of that problem —
//! *what is the real minimum, and how would anybody know before their backup is already
//! broken* — is what this answers.
//!
//! **The list is checked, not recited.** Every row below was established against a real
//! server by granting exactly that much and watching what happened, and every row has a
//! check in its adapter that asks the live connection rather than inferring from a version
//! number. `sloop doctor` runs those checks against each registered database, so a role
//! that is one grant short is a sentence on a health check instead of a discovery halfway
//! through a dump.
//!
//! **One list, two readers.** The site has to publish the same minimums, and a second copy
//! of them would drift within a release. So this table is the only copy, and `web/` gets a
//! generated file rather than a transcription: the tests below write it, compare it, and
//! fail when the two disagree. Rule 14 — a concern that is only stated is a concern that
//! gets forgotten, so this one is a test that goes red in CI instead of a note in a
//! checklist.
//!
//! **The worst row is the quiet one.** On MySQL 9.4 a role without the global
//! `SHOW_ROUTINE` privilege does not fail: `mysqldump --routines` exits `0` and writes a
//! dump with every stored routine silently missing from it. Nothing in the output says so.
//! That single behaviour is reason enough for this whole module — an error somebody can
//! read is survivable, and a backup that is quietly incomplete is not.

use std::collections::BTreeSet;
use std::fmt;

use serde::Serialize;

use super::{Engine, ServerInfo};

/// Which half of the job needs a privilege.
///
/// A registered database is usually one or the other, and a role that can only ever be
/// dumped from is perfectly healthy — which is why the report says which phase a gap is
/// in rather than calling every gap a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Reading the source, which is all `backup` and the source half of `mirror` do.
    Dump,
    /// Writing the destination — `restore`, and the destination half of `mirror`.
    Restore,
}

impl Phase {
    /// The phrase the report heads this half with.
    #[must_use]
    pub const fn heading(self) -> &'static str {
        match self {
            Self::Dump => "to back this database up",
            Self::Restore => "to restore into it",
        }
    }
}

/// Every phase, for the places that report both.
pub const PHASES: [Phase; 2] = [Phase::Dump, Phase::Restore];

impl fmt::Display for Phase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Dump => "dump",
            Self::Restore => "restore",
        })
    }
}

/// Whether a requirement applies to every database or only to some.
///
/// The distinction is the difference between a useful report and a wall of red. A database
/// with no stored routines does not need `SHOW_ROUTINE`, and telling its owner that they
/// are missing it would teach them to ignore the whole section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "subject", rename_all = "snake_case")]
pub enum Applies {
    /// Every database of this engine, every time.
    Always,
    /// Only when the database contains the thing named.
    OnlyWith(&'static str),
}

/// One thing a role must be able to do, and what breaks when it cannot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Requirement {
    /// A stable key. The site addresses rows by it, so it outlives any wording change.
    pub id: &'static str,
    /// What the role has to be able to do, in the report's own words.
    pub title: &'static str,
    /// The privileges that grant it, exactly as an engine spells them.
    pub privileges: &'static [&'static str],
    /// What goes wrong without it. The half people actually read.
    pub consequence: &'static str,
    /// Dump, or restore.
    pub phase: Phase,
    /// Every database, or only some.
    pub applies: Applies,
    /// The statement that grants it. `{role}` and `{database}` are filled in.
    pub grant: &'static str,
}

impl Requirement {
    /// The grant statement with this role and database written into it.
    #[must_use]
    pub fn grant_for(&self, role: &str, database: &str) -> String {
        self.grant
            .replace("{role}", role)
            .replace("{database}", database)
    }
}

/// A privilege the engine's own manual asks for that sloop does not need.
///
/// Worth publishing beside the list above, because it is the half a DBA cannot work out
/// for themselves: the manual's list is written for `mysqldump` with default options, and
/// sloop does not run it with default options. Three privileges come off, and a backup
/// role that is granted them anyway is holding more than it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Waived {
    /// The privilege the manual asks for.
    pub privilege: &'static str,
    /// The argument sloop passes that makes it unnecessary.
    pub because: &'static str,
}

/// What a role turned out to hold, for one requirement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The role can do it.
    Held,
    /// The role cannot, and this database needs it.
    Missing(String),
    /// Nothing in this database needs it.
    NotNeeded(String),
    /// The server could not be asked, and guessing would be worse than saying so.
    Unknown(String),
}

impl Verdict {
    /// Is this a gap that will break something?
    #[must_use]
    pub const fn is_missing(&self) -> bool {
        matches!(self, Self::Missing(_))
    }
}

/// Turn what a check found out into a verdict.
///
/// Shared by all three adapters, which answer the same two questions — *does this database
/// need it* and *can this role do it* — by entirely different routes. Keeping the decision
/// here is what stops two engines disagreeing about what "not needed" means.
#[must_use]
pub fn verdict_from(applies: Applies, applicable: bool, held: bool, detail: String) -> Verdict {
    match (applies, applicable, held) {
        (_, _, true) => Verdict::Held,
        (Applies::OnlyWith(subject), false, _) => {
            Verdict::NotNeeded(format!("this database has no {subject}"))
        }
        // `Always` is applicable by definition, so a false here is a check that did not
        // run rather than a requirement that does not apply. Saying so beats reporting a
        // privilege as missing on the strength of a question nobody answered.
        (Applies::Always, false, _) => Verdict::Unknown("the check did not run".to_owned()),
        (_, true, false) => Verdict::Missing(detail),
    }
}

/// One requirement, and how this role measured against it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Which requirement.
    pub requirement: &'static Requirement,
    /// What the server said.
    pub verdict: Verdict,
}

/// Everything a check found out about one connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The role the server says is connected — not the one the registry named, which can
    /// differ: MySQL resolves `bk` against its host patterns and answers `bk@%`.
    pub role: String,
    /// What answered.
    pub server: ServerInfo,
    /// One per requirement, in the table's order.
    pub findings: Vec<Finding>,
}

impl Report {
    /// The gaps, in the order they were checked.
    pub fn gaps(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|finding| finding.verdict.is_missing())
    }

    /// Gaps in one phase. A source-only database with no restore privileges is fine.
    pub fn gaps_in(&self, phase: Phase) -> impl Iterator<Item = &Finding> {
        self.gaps()
            .filter(move |finding| finding.requirement.phase == phase)
    }

    /// Can this role take a backup of this database, complete and correct?
    #[must_use]
    pub fn can_dump(&self) -> bool {
        self.gaps_in(Phase::Dump).next().is_none()
    }

    /// The statements that would close one phase's gaps, deduplicated and in order.
    ///
    /// **One phase at a time, and that is not tidiness.** A database registered as a backup
    /// source is only ever read from, and handing its owner a `GRANT CREATE` to paste —
    /// because the same role also could not restore into it, which it is never asked to —
    /// is advice that widens a privilege nothing needs. Rule 6 in a print statement.
    ///
    /// Nothing here is ever run by sloop either: granting a privilege is a thing an
    /// administrator does deliberately, and a backup tool that edits the permission table
    /// is a backup tool nobody installs.
    #[must_use]
    pub fn remedy_for(&self, phase: Phase, database: &str) -> Vec<String> {
        let quoted = quote_database(self.server.engine, database);
        let mut seen = BTreeSet::new();
        self.gaps_in(phase)
            .map(|finding| finding.requirement.grant_for(&self.role, &quoted))
            .filter(|statement| seen.insert(statement.clone()))
            .collect()
    }
}

/// A database name, as this engine's `GRANT` syntax needs it.
///
/// Every statement in a report is meant to be pasted, and `GRANT SELECT ON sales-eu.* …`
/// is a syntax error on a database somebody quite reasonably called `sales-eu`. Left alone
/// where it is already a plain lower-case identifier, because quoting everything makes the
/// common line noisier for the sake of the rare one.
#[must_use]
fn quote_database(engine: Engine, name: &str) -> String {
    let plain = !name.is_empty()
        && !name.starts_with(|first: char| first.is_ascii_digit())
        && name
            .chars()
            .all(|letter| letter.is_ascii_lowercase() || letter.is_ascii_digit() || letter == '_');

    if plain {
        return name.to_owned();
    }

    match engine {
        // PostgreSQL folds an unquoted identifier to lower case; MySQL does not fold but
        // needs backticks for anything outside its unquoted set. Both double the quote
        // character to escape it.
        Engine::Postgres => format!("\"{}\"", name.replace('"', "\"\"")),
        Engine::Mysql | Engine::Mariadb => format!("`{}`", name.replace('`', "``")),
    }
}

/// The requirements for an engine.
///
/// No catch-all arm, deliberately, the same way [`super::adapter_for`] has none: a fourth
/// engine stops this compiling until somebody has worked out what its backup role needs.
/// Getting that wrong quietly is exactly the failure this module exists to prevent.
#[must_use]
pub const fn for_engine(engine: Engine) -> &'static [Requirement] {
    match engine {
        Engine::Postgres => POSTGRES,
        Engine::Mysql => MYSQL,
        Engine::Mariadb => MARIADB,
    }
}

/// What this engine's manual asks for that sloop does not need, and why.
#[must_use]
pub const fn waived_by(engine: Engine) -> &'static [Waived] {
    match engine {
        Engine::Postgres => &[],
        Engine::Mysql | Engine::Mariadb => MYSQL_WAIVED,
    }
}

// ---------------------------------------------------------------------------------------
// PostgreSQL
//
// Established against PostgreSQL 17.9 by granting exactly this much and no more. Two of
// the four dump rows are things `pg_read_all_data` does not cover and its documentation
// does not mention, and both were found by watching `pg_dump` fail.
// ---------------------------------------------------------------------------------------

/// PostgreSQL's minimum.
const POSTGRES: &[Requirement] = &[
    Requirement {
        id: "pg-connect",
        title: "reach the database",
        privileges: &["CONNECT"],
        consequence: "nothing can connect at all",
        phase: Phase::Dump,
        applies: Applies::Always,
        grant: "GRANT CONNECT ON DATABASE {database} TO {role};",
    },
    Requirement {
        id: "pg-read-everything",
        title: "read every table, view and sequence",
        privileges: &["USAGE on each schema", "SELECT on each table and sequence"],
        consequence: "pg_dump stops on the first table it cannot lock, and the dump is \
                      not written",
        phase: Phase::Dump,
        applies: Applies::Always,
        // The predefined role covers schema USAGE and table, view and sequence SELECT in
        // one statement, and keeps covering objects created after it was granted — which
        // the explicit form does not, without ALTER DEFAULT PRIVILEGES as well. Added in
        // PostgreSQL 14; the hint in `doctor` names the older route on an older server.
        grant: "GRANT pg_read_all_data TO {role};",
    },
    Requirement {
        id: "pg-bypass-row-security",
        title: "see every row of a table with row-level security",
        privileges: &["BYPASSRLS"],
        // pg_read_all_data explicitly does not carry BYPASSRLS, which its own
        // documentation says and which is easy to read past.
        consequence: "pg_dump sets row_security to off so that it dumps whole tables, and \
                      a role that cannot bypass the policy fails rather than silently \
                      dumping a subset",
        phase: Phase::Dump,
        applies: Applies::OnlyWith("a table that has row-level security enabled"),
        grant: "ALTER ROLE {role} BYPASSRLS;",
    },
    Requirement {
        id: "pg-read-large-objects",
        title: "read the large objects",
        privileges: &["SELECT on each large object"],
        // There is no GRANT ... ON ALL LARGE OBJECTS, and pg_read_all_data does not reach
        // them: it covers tables, views and sequences and stops there.
        consequence: "pg_dump carries large objects by default and stops on the first one \
                      it cannot open",
        phase: Phase::Dump,
        applies: Applies::OnlyWith("a large object"),
        grant: "-- per object: GRANT SELECT ON LARGE OBJECT <oid> TO {role};",
    },
    Requirement {
        id: "pg-create-in-database",
        title: "create a schema",
        privileges: &["CREATE on the database"],
        consequence: "a restore stops at the first CREATE SCHEMA in the dump",
        phase: Phase::Restore,
        applies: Applies::Always,
        grant: "GRANT CREATE ON DATABASE {database} TO {role};",
    },
    Requirement {
        id: "pg-create-in-schemas",
        title: "enter and create in every schema",
        // Both, and the pair is not obvious: `CREATE` without `USAGE` gets as far as the
        // tables and then fails on the first `ALTER TABLE` against one, because altering
        // an object means naming it, and naming it needs USAGE on the schema it is in.
        privileges: &["USAGE on each schema", "CREATE on each schema"],
        // PostgreSQL 15 revoked CREATE on `public` from PUBLIC. A restore role that worked
        // on 14 stops working on 15 with nothing about the upgrade in the error.
        consequence: "a restore stops partway with `permission denied for schema` — on \
                      PostgreSQL 15 and newer this bites on `public`, which no longer \
                      grants CREATE to everybody",
        phase: Phase::Restore,
        applies: Applies::Always,
        grant: "GRANT USAGE, CREATE ON SCHEMA public TO {role};",
    },
    Requirement {
        id: "pg-untrusted-extensions",
        title: "create the extensions this database uses",
        privileges: &["superuser"],
        consequence: "a dump of this database carries CREATE EXTENSION for an extension \
                      PostgreSQL does not mark trusted, and only a superuser can run that \
                      — restoring elsewhere needs the extension installed first",
        phase: Phase::Restore,
        applies: Applies::OnlyWith("an extension that is not trusted"),
        grant: "-- a superuser runs: CREATE EXTENSION <name>; on the destination first",
    },
];

// ---------------------------------------------------------------------------------------
// MySQL and MariaDB
//
// Established against MySQL 9.4.0 and MariaDB 11.4.5 by granting one privilege at a time
// and dumping after each. The two engines differ in exactly one row — how a role is
// allowed to read a routine it did not define — which is why they are two tables.
// ---------------------------------------------------------------------------------------

/// Shared wording for the row that matters most.
///
/// "The dump program" rather than `mysqldump`, here and in the rows below, because
/// MariaDB shares most of this table and its program is `mariadb-dump`. Both behave the
/// same way, and naming the wrong binary in a report is how somebody concludes the
/// warning is about a different server.
const ROUTINE_CONSEQUENCE: &str = "the dump program exits 0 and writes a dump with every stored routine missing from \
     it. Nothing in the output says so: a backup that looks fine has quietly lost every \
     function and procedure";

/// MySQL's minimum.
const MYSQL: &[Requirement] = &[
    Requirement {
        id: "my-select",
        title: "read every table",
        privileges: &["SELECT"],
        consequence: "the dump program cannot read the rows, and fails before writing anything",
        phase: Phase::Dump,
        applies: Applies::Always,
        grant: "GRANT SELECT ON {database}.* TO {role};",
    },
    Requirement {
        id: "my-show-view",
        title: "read the view definitions",
        privileges: &["SHOW VIEW"],
        consequence: "the dump program stops on the first view with `SHOW VIEW command denied`",
        phase: Phase::Dump,
        applies: Applies::OnlyWith("a view"),
        grant: "GRANT SHOW VIEW ON {database}.* TO {role};",
    },
    Requirement {
        id: "my-trigger-dump",
        title: "read the trigger definitions",
        privileges: &["TRIGGER"],
        // The second silent one. Views and events fail loudly; triggers and routines do
        // not, and the two halves of that are worth telling apart.
        consequence: "the dump program exits 0 and the dump carries no triggers. Nothing in the \
                      output says so",
        phase: Phase::Dump,
        applies: Applies::OnlyWith("a trigger"),
        grant: "GRANT TRIGGER ON {database}.* TO {role};",
    },
    Requirement {
        id: "my-event-dump",
        title: "read the event definitions",
        privileges: &["EVENT"],
        consequence: "the dump program stops on `show events` with `Access denied`",
        phase: Phase::Dump,
        applies: Applies::OnlyWith("an event"),
        grant: "GRANT EVENT ON {database}.* TO {role};",
    },
    Requirement {
        id: "my-show-routine",
        title: "read a stored routine it did not define",
        // Global only. MySQL refuses `GRANT SHOW_ROUTINE ON db.*` with
        // `Illegal privilege level specified for SHOW_ROUTINE`.
        privileges: &["SHOW_ROUTINE, granted globally"],
        consequence: ROUTINE_CONSEQUENCE,
        phase: Phase::Dump,
        applies: Applies::OnlyWith("a stored routine defined by another account"),
        grant: "GRANT SHOW_ROUTINE ON *.* TO {role};",
    },
    Requirement {
        id: "my-load",
        title: "create the tables and load the rows",
        privileges: &[
            "SELECT",
            "INSERT",
            "CREATE",
            "DROP",
            "ALTER",
            "REFERENCES",
            "LOCK TABLES",
        ],
        // Each one earns its place in a real dump: DROP for `DROP TABLE IF EXISTS`, ALTER
        // for the deferred `ADD CONSTRAINT`, REFERENCES for the foreign key itself, and
        // LOCK TABLES because mysqldump writes `LOCK TABLES` around its own inserts.
        consequence: "the restore stops partway, leaving the destination half built",
        phase: Phase::Restore,
        applies: Applies::Always,
        grant: "GRANT SELECT, INSERT, CREATE, DROP, ALTER, REFERENCES, LOCK TABLES \
                ON {database}.* TO {role};",
    },
    Requirement {
        id: "my-create-view",
        title: "create the views",
        privileges: &["CREATE VIEW"],
        consequence: "the restore stops at the first view",
        phase: Phase::Restore,
        applies: Applies::OnlyWith("a view"),
        grant: "GRANT CREATE VIEW ON {database}.* TO {role};",
    },
    Requirement {
        id: "my-trigger-restore",
        title: "create the triggers",
        privileges: &["TRIGGER"],
        consequence: "the restore stops at the first trigger",
        phase: Phase::Restore,
        applies: Applies::OnlyWith("a trigger"),
        grant: "GRANT TRIGGER ON {database}.* TO {role};",
    },
    Requirement {
        id: "my-routine-restore",
        title: "create the stored routines",
        // ALTER ROUTINE as well as CREATE ROUTINE: the dump opens each routine with
        // `DROP FUNCTION IF EXISTS`, and dropping one is an ALTER ROUTINE.
        privileges: &["CREATE ROUTINE", "ALTER ROUTINE"],
        consequence: "the restore stops at the first routine",
        phase: Phase::Restore,
        applies: Applies::OnlyWith("a stored routine"),
        grant: "GRANT CREATE ROUTINE, ALTER ROUTINE ON {database}.* TO {role};",
    },
    Requirement {
        id: "my-event-restore",
        title: "create the events",
        privileges: &["EVENT"],
        consequence: "the restore stops at the first event",
        phase: Phase::Restore,
        applies: Applies::OnlyWith("an event"),
        grant: "GRANT EVENT ON {database}.* TO {role};",
    },
    Requirement {
        id: "my-function-binlog",
        title: "create a stored function while binary logging is on",
        privileges: &["SUPER, granted globally"],
        // Binary logging is on by default from MySQL 8, and
        // log_bin_trust_function_creators is off by default, so this is the common case
        // rather than an exotic one.
        consequence: "the restore stops with `You do not have the SUPER privilege and \
                      binary logging is enabled`. The alternative is for a DBA to set \
                      log_bin_trust_function_creators on the destination server",
        phase: Phase::Restore,
        applies: Applies::OnlyWith(
            "a stored function, while the server has binary logging on and \
             log_bin_trust_function_creators off",
        ),
        grant: "GRANT SUPER ON *.* TO {role};",
    },
];

/// MariaDB's minimum.
///
/// The same as MySQL's but for one row, and the difference is real rather than cosmetic:
/// MariaDB never adopted `SHOW_ROUTINE` and still keeps routine definitions in
/// `mysql.proc`, so the privilege that lets a backup role read somebody else's routine is
/// `SELECT` on that table.
const MARIADB: &[Requirement] = &[
    MYSQL[0],
    MYSQL[1],
    MYSQL[2],
    MYSQL[3],
    Requirement {
        id: "maria-show-create-routine",
        title: "read a stored routine it did not define",
        // Two routes, and the check accepts either. MariaDB 11.3 added the narrow one;
        // before that the only way in was read access to the table routines live in.
        privileges: &[
            "SHOW CREATE ROUTINE (MariaDB 11.3 and newer)",
            "or SELECT on mysql.proc",
        ],
        consequence: ROUTINE_CONSEQUENCE,
        phase: Phase::Dump,
        applies: Applies::OnlyWith("a stored routine defined by another account"),
        grant: "GRANT SHOW CREATE ROUTINE ON *.* TO {role};  -- before 11.3: \
                GRANT SELECT ON mysql.proc TO {role};",
    },
    MYSQL[5],
    MYSQL[6],
    MYSQL[7],
    MYSQL[8],
    MYSQL[9],
    MYSQL[10],
];

/// Where `web/` reads the table from, relative to `cli/`.
///
/// A generated file, committed, so the site's build needs no Rust toolchain — and checked
/// by a test, so it cannot quietly fall behind the code it came from.
pub const PUBLISHED: &str = "../web/privileges.json";

/// One engine's whole entry, as the site receives it.
#[derive(Debug, Clone, Copy, Serialize)]
struct Published {
    /// `postgres`, `mysql`, `mariadb`.
    engine: Engine,
    /// What a role must be able to do.
    requires: &'static [Requirement],
    /// What this engine's manual asks for that sloop does not need.
    waives: &'static [Waived],
}

/// The whole table as JSON, exactly as the committed file holds it.
///
/// Test-only: `serde_json` is a dev-dependency, so none of this reaches the binary. The
/// site reads the committed file; the binary reads the tables above; neither needs a JSON
/// parser at run time.
#[cfg(test)]
fn published_json() -> String {
    let all: Vec<Published> = Engine::ALL
        .into_iter()
        .map(|engine| Published {
            engine,
            requires: for_engine(engine),
            waives: waived_by(engine),
        })
        .collect();

    // Pretty, and with a trailing newline: this file is read by people in diffs at least
    // as often as by a build.
    let mut json = serde_json::to_string_pretty(&all).expect("the table is plain data");
    json.push('\n');
    json
}

/// What the MySQL manual asks a backup role for that sloop does not need.
const MYSQL_WAIVED: &[Waived] = &[
    Waived {
        privilege: "LOCK TABLES",
        because: "sloop dumps with --single-transaction, which takes no locks on the source",
    },
    Waived {
        privilege: "PROCESS",
        because: "sloop dumps with --no-tablespaces, so nothing reads INFORMATION_SCHEMA.FILES",
    },
    Waived {
        privilege: "RELOAD or FLUSH_TABLES",
        because: "sloop dumps with --set-gtid-purged=OFF, so a GTID-enabled server needs \
                  neither",
    },
];

#[cfg(test)]
mod tests {
    use super::{Applies, Engine, Phase, Requirement, for_engine, waived_by};
    use std::collections::BTreeSet;

    /// Every engine has a list, and every list covers both halves of the job. An engine
    /// that could be backed up but never restored is a list somebody forgot to finish.
    #[test]
    fn every_engine_covers_both_phases() {
        for engine in Engine::ALL {
            let table = for_engine(engine);
            assert!(!table.is_empty(), "{engine} has no requirements");
            for phase in [Phase::Dump, Phase::Restore] {
                assert!(
                    table.iter().any(|row| row.phase == phase),
                    "{engine} has nothing for {phase}"
                );
            }
        }
    }

    /// The id is what the site addresses a row by, so two rows sharing one would make the
    /// page show the same entry twice and silently drop the other.
    #[test]
    fn every_id_is_unique_across_every_engine() {
        let mut seen = BTreeSet::new();
        for engine in Engine::ALL {
            for row in for_engine(engine) {
                // The same row may legitimately appear in two engines' tables — MariaDB
                // shares most of MySQL's — so a repeat is only wrong within one engine.
                assert!(
                    seen.insert((engine, row.id)),
                    "{engine} lists {} twice",
                    row.id
                );
            }
        }
    }

    /// Every row has to say what it costs and how to fix it. A row with a title and
    /// nothing else is a row that sends somebody to a search engine.
    #[test]
    fn every_row_is_complete() {
        for engine in Engine::ALL {
            for row in for_engine(engine) {
                let Requirement {
                    id,
                    title,
                    privileges,
                    consequence,
                    grant,
                    ..
                } = row;
                assert!(!id.is_empty() && !title.is_empty(), "a row has no name");
                assert!(!privileges.is_empty(), "{id} names no privilege");
                assert!(consequence.len() > 20, "{id} does not say what breaks");
                // Every row has a remedy, and nearly all of them are a statement naming
                // the role. The exception is real rather than an oversight: an untrusted
                // extension is not fixed by granting anything to the backup role, it is
                // fixed by a superuser creating the extension on the destination. A row
                // like that says so in a comment instead, and never silently.
                assert!(
                    grant.contains("{role}") || grant.starts_with("--"),
                    "{id} has no remedy, and does not explain why"
                );
            }
        }
    }

    /// A conditional row has to name the condition, because the report prints it as the
    /// reason a requirement was skipped.
    #[test]
    fn a_conditional_row_names_its_condition() {
        for engine in Engine::ALL {
            for row in for_engine(engine) {
                if let Applies::OnlyWith(subject) = row.applies {
                    assert!(!subject.is_empty(), "{} applies to nothing", row.id);
                }
            }
        }
    }

    /// MariaDB is MySQL with one row swapped. If that stops being true the difference is
    /// worth a deliberate edit here rather than a surprise on somebody's server.
    #[test]
    fn mariadb_differs_from_mysql_in_exactly_the_routine_row() {
        let mysql = for_engine(Engine::Mysql);
        let mariadb = for_engine(Engine::Mariadb);
        assert_eq!(mysql.len(), mariadb.len());

        let differing: Vec<_> = mysql
            .iter()
            .zip(mariadb)
            .filter(|(one, other)| one != other)
            .map(|(one, other)| (one.id, other.id))
            .collect();

        assert_eq!(
            differing,
            [("my-show-routine", "maria-show-create-routine")]
        );
    }

    /// PostgreSQL waives nothing: sloop runs `pg_dump` with the options it was going to
    /// need anyway, so there is no privilege the manual asks for that this avoids.
    #[test]
    fn only_the_mysql_family_waives_anything() {
        assert!(waived_by(Engine::Postgres).is_empty());
        assert_eq!(waived_by(Engine::Mysql).len(), 3);
        assert_eq!(waived_by(Engine::Mariadb).len(), 3);
    }

    /// The site publishes these minimums, and a page that says something different from
    /// what the code checks is worse than a page that says nothing — somebody grants
    /// exactly what it lists and their backup is still quietly incomplete.
    ///
    /// So the file is generated and this goes red the moment the two part company. The
    /// fix is a command rather than an edit, which is the point: there is no way to change
    /// the table and update the site wrongly.
    #[test]
    fn the_site_publishes_exactly_what_the_code_checks() {
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(super::PUBLISHED);
        let committed = std::fs::read_to_string(&here).unwrap_or_default();

        assert_eq!(
            committed.replace("\r\n", "\n"),
            super::published_json(),
            "\n\n  {} is out of date.\n  Run: cargo test regenerate_what_the_site_publishes -- \
             --ignored\n  and commit the result.\n",
            here.display()
        );
    }

    /// Write it. Ignored, because a test that edits the working tree should only ever run
    /// when somebody asked it to.
    #[test]
    #[ignore = "writes web/privileges.json; run it after changing the table"]
    fn regenerate_what_the_site_publishes() {
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(super::PUBLISHED);
        if let Some(parent) = here.parent() {
            std::fs::create_dir_all(parent).expect("somewhere to write it");
        }
        std::fs::write(&here, super::published_json()).expect("writing the published table");
        println!("wrote {}", here.display());
    }

    #[test]
    fn a_grant_statement_is_filled_in_with_the_real_names() {
        let row = for_engine(Engine::Mysql)
            .iter()
            .find(|row| row.id == "my-select")
            .expect("MySQL needs SELECT");

        assert_eq!(
            row.grant_for("`bk`@`%`", "shop"),
            "GRANT SELECT ON shop.* TO `bk`@`%`;"
        );
    }
}
