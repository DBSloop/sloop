//! What can be checked without a network and without a database.
//!
//! The install itself is driven by hand against a real download — it is a third of a
//! gigabyte from somebody else's server, which is not a thing to put in `cargo test`. What
//! *is* here is every decision that install rests on: which programs an engine needs, which
//! copy wins when there are several, what the pinned table says, how the index is read, and
//! the two refusals that must happen before anybody is ever asked anything.

use std::path::{Path, PathBuf};

use super::acquire::{self, Answer};
use super::releases::{self, Major, Release};
use super::{Candidate, Inventory, Source, Tool};
use crate::engine::{Engine, Version, mysql};

/// A captured cut of `https://www.postgresql.org/versions.json`, trimmed to both ends of
/// the range that matters: a `9.6`-style major, an end-of-life one, and the current one.
const INDEX_SAMPLE: &str = r#"[
{"current": false, "eolDate": "2021-11-11", "firstRelDate": "2016-09-29", "latestMinor": "24", "major": "9.6", "relDate": "2021-11-11", "supported": false},
{"current": false, "eolDate": "2025-11-13", "firstRelDate": "2020-09-24", "latestMinor": "23", "major": "13", "relDate": "2025-11-13", "supported": false},
{"current": false, "eolDate": "2029-11-08", "firstRelDate": "2024-09-26", "latestMinor": "11", "major": "17", "relDate": "2026-08-13", "supported": true},
{"current": true, "eolDate": "2030-11-14", "firstRelDate": "2025-09-25", "latestMinor": "6", "major": "18", "relDate": "2026-08-13", "supported": true}
]"#;

#[test]
fn every_engine_names_the_tools_it_cannot_work_without() {
    for engine in Engine::ALL {
        let needed = Tool::needed_by(engine);
        assert!(!needed.is_empty(), "{engine} needs something");

        for &tool in needed {
            assert_eq!(tool.engine(), engine, "{tool} does not belong to {engine}");
        }
    }

    // Every tool is wanted by exactly one engine, so nothing can be silently unreachable.
    for tool in Tool::ALL {
        let owners: Vec<_> = Engine::ALL
            .into_iter()
            .filter(|&engine| Tool::needed_by(engine).contains(&tool))
            .collect();
        assert_eq!(owners.len(), 1, "{tool} is claimed by {owners:?}");
    }
}

/// MariaDB renamed its programs at 10.5. A machine with an older one has `mysqldump` and no
/// `mariadb-dump`, and calling that "MariaDB is not installed" would be wrong.
#[test]
fn mariadbs_older_names_are_still_looked_for() {
    let names = Tool::MariadbDump.file_names();
    assert!(names.iter().any(|name| name.starts_with("mariadb-dump")));
    assert!(names.iter().any(|name| name.starts_with("mysqldump")));

    let names = Tool::Mariadb.file_names();
    assert!(names.iter().any(|name| name.starts_with("mariadb")));
    assert!(names.iter().any(|name| name.starts_with("mysql")));

    // PostgreSQL has never renamed anything, and inventing an alias for it would have
    // `psql` answering for something that is not PostgreSQL.
    assert!(Tool::Psql.also_known_as().is_empty());
    assert_eq!(Tool::Psql.file_names().len(), 1);
}

/// Each family is read by its own parser. One parser for both gets MariaDB wrong, because
/// its tools print their own version before the one that matters.
#[test]
fn a_version_is_read_the_way_that_family_prints_it() {
    assert_eq!(
        Tool::PgDump.read_version("pg_dump (PostgreSQL) 18.6"),
        Some(Version::new(18, 6))
    );
    assert_eq!(
        Tool::MysqlDump.read_version("mysqldump  Ver 8.4.3 for Win64 on x86_64"),
        Some(Version::new(8, 4))
    );
    assert_eq!(
        Tool::MariadbDump
            .read_version("mysqldump  Ver 10.19 Distrib 10.6.16-MariaDB, for Linux (x86_64)"),
        Some(Version::new(10, 6)),
        "10.19 is the tool's own version, not the server's"
    );
    assert_eq!(Tool::Psql.read_version("command not found"), None);
}

/// The rule that decides which copy runs. Newest wins wherever it came from, because a
/// tool sloop fetched *because* the system one was too old must not then lose to it.
#[test]
fn the_newest_copy_wins_and_a_tie_goes_to_the_one_on_path() {
    let candidate = |version: Option<(u32, u32)>, source: Source, path: &str| Candidate {
        path: PathBuf::from(path),
        source,
        version: version.map(|(major, minor)| Version::new(major, minor)),
    };

    let mut found = [
        candidate(Some((14, 2)), Source::OnPath, "/usr/bin/pg_dump"),
        candidate(None, Source::Installed, "/broken/pg_dump"),
        candidate(Some((18, 6)), Source::Fetched, "/sloop/pg_dump"),
        candidate(Some((16, 1)), Source::Installed, "/opt/pg16/pg_dump"),
        candidate(Some((18, 6)), Source::Installed, "/opt/pg18/pg_dump"),
    ];
    found.sort_by_key(Candidate::rank);

    let order: Vec<_> = found
        .iter()
        .map(|found| found.path.display().to_string())
        .collect();
    assert_eq!(
        order,
        vec![
            // 18.6 twice: the one sloop fetched outranks an installed one, and neither is
            // on PATH.
            "/sloop/pg_dump",
            "/opt/pg18/pg_dump",
            "/opt/pg16/pg_dump",
            "/usr/bin/pg_dump",
            // A copy that would not say its version sorts last rather than being dropped:
            // it is still a real program, and `doctor` has to be able to show it.
            "/broken/pg_dump",
        ]
    );
}

/// Discovery is pointed at a directory of its own, so the assertion is about the rule and
/// not about whatever this machine happens to have installed.
#[test]
fn a_program_in_the_fetched_directory_is_found_there() {
    let sandbox = Sandbox::new("fetched");
    let name = if cfg!(windows) {
        "pg_dump.exe"
    } else {
        "pg_dump"
    };
    std::fs::write(sandbox.path().join(name), b"not really a program").expect("writing a file");

    let inventory = Inventory::for_engine(Engine::Postgres, sandbox.path());
    let ours = inventory
        .every(Tool::PgDump)
        .iter()
        .find(|candidate| candidate.path.starts_with(sandbox.path()))
        .expect("the one that was just put there");

    assert_eq!(ours.source, Source::Fetched);
    // It is not a program, so it cannot say what version it is — and that is reported
    // rather than swallowed.
    assert_eq!(ours.version, None);
}

/// An engine with nothing found says so, and names every missing piece rather than the
/// first one.
#[test]
fn a_missing_engine_names_everything_it_is_short_of() {
    let sandbox = Sandbox::new("empty");
    let inventory = Inventory::of(&[], sandbox.path());

    for engine in Engine::ALL {
        assert!(!inventory.has_everything_for(engine));
        assert_eq!(inventory.missing_for(engine), Tool::needed_by(engine));
    }
}

/// The seam R4 was shaped for: the inventory says where the programs are, and the engine
/// is handed paths without ever learning that discovery exists.
#[test]
fn the_inventory_builds_an_adapter_for_every_engine() {
    let sandbox = Sandbox::new("adapters");
    let inventory = Inventory::of(&[], sandbox.path());

    for engine in Engine::ALL {
        assert_eq!(inventory.adapter_for(engine).engine(), engine);
    }

    // Nothing was found, so each adapter falls back to the bare name — which is what
    // `PATH` lookup at spawn time does, and is why a machine this module could not see
    // into still works.
    assert_eq!(inventory.postgres().dump, Path::new("pg_dump"));
    assert_eq!(
        inventory.mysql_family(mysql::Family::Mariadb).dump,
        Path::new("mariadb-dump")
    );
}

#[test]
fn the_pinned_releases_are_usable_and_the_newest_is_the_newest() {
    assert!(
        !releases::PINNED.is_empty(),
        "there has to be one to install"
    );

    for release in releases::PINNED {
        assert_eq!(release.sha256.len(), 64, "{release} has no SHA-256");
        assert!(
            release
                .sha256
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "{release}'s hash is not lower-case hex"
        );
        assert!(release.bytes > 0, "{release} has no size to check");
        assert!(
            release.url().starts_with("https://"),
            "{release} would be fetched over something other than https"
        );
        assert_eq!(
            std::path::Path::new(&release.file_name())
                .extension()
                .and_then(std::ffi::OsStr::to_str),
            Some("zip")
        );
    }

    let newest = releases::newest();
    for release in releases::PINNED {
        assert!(
            (release.major, release.minor, release.build)
                <= (newest.major, newest.minor, newest.build)
        );
    }
}

#[test]
fn the_index_is_read_without_a_json_library() {
    let majors = releases::read_index(INDEX_SAMPLE);

    assert_eq!(majors.len(), 4);
    assert_eq!(
        majors[0],
        Major {
            number: 9,
            latest_minor: 24,
            supported: false
        },
        "a `9.6`-style major is read as 9"
    );
    assert_eq!(
        majors[3],
        Major {
            number: 18,
            latest_minor: 6,
            supported: true
        }
    );
    assert_eq!(
        majors.iter().filter(|major| major.supported).count(),
        2,
        "13 and 9.6 are past end of life"
    );

    // Nothing usable in, nothing out — the index is advice, and advice that arrives
    // mangled has to be the same as advice that never arrived.
    assert!(releases::read_index("").is_empty());
    assert!(releases::read_index("<html>not json</html>").is_empty());
}

#[test]
fn the_index_only_ever_adds_a_sentence() {
    let majors = releases::read_index(INDEX_SAMPLE);
    let pinned = |major, minor| Release {
        major,
        minor,
        build: 1,
        bytes: 1,
        sha256: "",
    };

    // Up to date with the index: nothing to say.
    assert_eq!(releases::what_the_index_adds(&majors, &pinned(18, 6)), None);

    // A newer minor is out.
    let said = releases::what_the_index_adds(&majors, &pinned(18, 2)).expect("a sentence");
    assert!(said.contains("18.6"), "{said}");

    // A whole major behind.
    let said = releases::what_the_index_adds(&majors, &pinned(17, 11)).expect("a sentence");
    assert!(said.contains("18"), "{said}");
    assert!(
        said.contains("up to PostgreSQL 17"),
        "it has to say the pinned one still works: {said}"
    );

    // Pinned to something upstream has dropped.
    let said = releases::what_the_index_adds(&majors, &pinned(13, 23)).expect("a sentence");
    assert!(said.contains("no longer supports"), "{said}");

    // No index at all is not a sentence, it is silence.
    assert_eq!(releases::what_the_index_adds(&[], &pinned(1, 0)), None);
}

/// Rule 4, one layer down. `cargo test` has no terminal, which is exactly the situation a
/// scheduled backup is in, so this is the real thing rather than a simulation of it.
#[test]
fn without_a_terminal_nothing_is_asked_and_the_exit_code_says_usage() {
    let sandbox = Sandbox::new("no-tty");

    for engine in Engine::ALL {
        if Inventory::for_engine(engine, &acquire::fetched_dir(sandbox.path()))
            .has_everything_for(engine)
        {
            // This machine has them, so there is nothing to be asked about.
            continue;
        }

        let failure = acquire::ensure(engine, sandbox.path())
            .expect_err("it cannot install without being asked");

        assert_eq!(failure.exit().code(), 2, "{engine}: {failure:?}");
        assert!(
            failure.message().contains("no terminal"),
            "{engine}: {failure:?}"
        );
        assert!(
            failure.hint_text().is_some(),
            "{engine}: it has to say what to do instead"
        );
    }
}

/// A no is remembered, and a remembered no is checked before anything else — so a tool that
/// was declined once explains itself instead of asking again on every command.
#[test]
fn a_refusal_is_remembered_and_never_becomes_a_second_question() {
    let sandbox = Sandbox::new("declined");

    let Some(engine) = Engine::ALL.into_iter().find(|&engine| {
        !Inventory::for_engine(engine, &acquire::fetched_dir(sandbox.path()))
            .has_everything_for(engine)
    }) else {
        eprintln!("skipping: this machine has every engine's tools");
        return;
    };

    acquire::remember(engine, Answer::Declined, sandbox.path());

    let failure = acquire::ensure(engine, sandbox.path()).expect_err("it was told not to");

    assert_eq!(failure.exit().code(), 2, "{failure:?}");
    assert!(
        failure.message().contains("told once"),
        "it has to say why it did not ask: {failure:?}"
    );
    assert!(
        !failure.message().contains("no terminal"),
        "the remembered answer has to be read before the terminal is looked for: {failure:?}"
    );
    assert!(failure.hint_text().is_some_and(|hint| !hint.is_empty()));
}

/// A temporary directory, removed when it goes out of scope. Built by hand for the same
/// reason as the one in `tests/support`: four crates for twenty lines is not a trade this
/// project makes.
struct Sandbox(PathBuf);

impl Sandbox {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);

        let root = std::env::temp_dir().join(format!(
            "sloop-tools-{}-{label}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a temporary directory");
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The words somebody is shown before they decide. A prompt that does not say what it is
/// about to do is not consent, so the size, the source and the verification are all in it.
#[test]
fn the_offer_says_what_it_is_going_to_do() {
    let Some(plan) = acquire::plan_for(Engine::Postgres) else {
        eprintln!("skipping: this platform offers nothing for PostgreSQL");
        return;
    };

    let said = plan.describe();
    assert!(
        said.contains("SHA-256") || said.contains("install"),
        "{said}"
    );
    assert!(plan.question().ends_with('?'), "{}", plan.question());

    if cfg!(windows) {
        let release = releases::newest();
        assert!(said.contains(&release.major.to_string()), "{said}");
        // The size matters most: a third of a gigabyte is not something to start on
        // somebody's behalf without telling them first.
        assert!(
            said.contains(&(release.bytes / 1_000_000).to_string()),
            "{said}"
        );
        assert!(said.contains("curl"), "{said}");
    }
}

/// Only a plain yes starts a download.
#[test]
fn anything_that_is_not_a_yes_is_a_no() {
    for yes in ["y", "Y", "yes", "YES", " yes ", "Yes\n"] {
        assert!(acquire::is_yes(yes), "{yes:?}");
    }
    for no in ["", "\n", "n", "no", "sure", "ok", "yep", "1", "yeah"] {
        assert!(!acquire::is_yes(no), "{no:?}");
    }
}

/// The whole install, against the real download.
///
/// Ignored by default, and deliberately: it is a third of a gigabyte from somebody else's
/// server, which does not belong in a run of `cargo test`. Run it by hand with
/// `cargo test --bin sloop installs_postgresql -- --ignored --nocapture`, which is how the
/// pinned checksum in `releases` is confirmed to still be the right one.
#[test]
#[ignore = "downloads a third of a gigabyte from get.enterprisedb.com"]
fn installs_postgresql_and_then_has_nothing_left_to_ask() {
    if !cfg!(windows) {
        eprintln!("skipping: the archive install is the Windows path");
        return;
    }

    let sandbox = Sandbox::new("install");
    let global = sandbox.path();
    let fetched = acquire::fetched_dir(global);

    // Before: the sandbox has nothing in it.
    assert!(
        !fetched.join("pg_dump.exe").is_file(),
        "the sandbox started out with something in it"
    );

    acquire::install_for_tests(Engine::Postgres, &fetched).expect("installing PostgreSQL");

    // The three programs, and a version that matches what was pinned.
    let release = releases::newest();
    let inventory = Inventory::for_engine(Engine::Postgres, &fetched);
    for tool in [Tool::PgDump, Tool::PgRestore, Tool::Psql] {
        let ours = inventory
            .every(tool)
            .iter()
            .find(|candidate| candidate.path.starts_with(&fetched))
            .unwrap_or_else(|| panic!("{tool} was not installed into {}", fetched.display()));

        assert_eq!(ours.source, Source::Fetched);
        assert_eq!(
            ours.version,
            Some(Version::new(release.major, release.minor)),
            "{tool} is not the version that was pinned"
        );
    }

    // The archive is not kept: 51 MB was wanted out of 330, and the rest is not worth
    // leaving on somebody's disk.
    assert!(
        !fetched
            .parent()
            .expect("a parent")
            .join("download")
            .join(release.file_name())
            .is_file(),
        "the archive was left behind"
    );

    // And a second run has nothing to ask about — which is the half of "asked once" that a
    // remembered answer cannot prove on its own.
    assert!(
        Inventory::for_engine(Engine::Postgres, &fetched).has_everything_for(Engine::Postgres),
        "the second run would still think PostgreSQL was missing"
    );
    acquire::ensure(Engine::Postgres, global)
        .expect("a second run finds them and returns without asking anything");
}
