//! What the menu offers, and — the half that decides whether it is honest — what it will not
//! offer.
//!
//! Nothing here reaches the network. The two index readers are given the real shape of what
//! the real endpoints return, because what is worth testing is the reading, not the fetching.

use super::{
    Build, ENGINES, Platform, mysql_builds, postgres_builds, read_mariadb_files,
    read_mariadb_versions, series_of,
};
use crate::engine::Engine;
use crate::tools::proof::Proof;

/// **The screen names the engines sloop does not speak.** A menu that silently omits MongoDB
/// reads as a tool that has never heard of it; one that says *"not yet"* reads as a tool that
/// knows where it stands — which is the whole of what the owner asked for in that line.
#[test]
fn the_list_names_what_sloop_cannot_do_as_well_as_what_it_can() {
    let supported: Vec<&str> = ENGINES
        .iter()
        .filter(|listed| listed.supported())
        .map(|listed| listed.name)
        .collect();
    assert_eq!(supported, ["PostgreSQL", "MySQL", "MariaDB"]);

    let not_yet: Vec<&str> = ENGINES
        .iter()
        .filter(|listed| !listed.supported())
        .map(|listed| listed.name)
        .collect();
    assert!(
        not_yet.contains(&"MongoDB") && not_yet.contains(&"SQL Server"),
        "{not_yet:?}"
    );

    // And every unsupported row says so where somebody reads it, rather than only in a field
    // the screen might forget to paint.
    for listed in ENGINES.iter().filter(|listed| !listed.supported()) {
        assert!(
            listed.blurb.contains("not yet"),
            "{} does not say it is not supported: {}",
            listed.name,
            listed.blurb
        );
    }
}

/// The three sloop speaks are exactly `Engine::ALL`, so an engine added to one is not missing
/// from the other.
#[test]
fn every_engine_sloop_speaks_is_on_the_list_once() {
    for engine in Engine::ALL {
        let rows = ENGINES
            .iter()
            .filter(|listed| listed.engine == Some(engine))
            .count();
        assert_eq!(rows, 1, "{engine} should be listed exactly once");
    }
}

/// `8.4.11` lives in `MySQL-8.4`, and something that is not a version resolves to nothing
/// rather than to a URL that would 404 halfway through a download.
#[test]
fn a_mysql_version_resolves_to_its_series_or_to_nothing() {
    assert_eq!(series_of("8.4.11").as_deref(), Some("8.4"));
    assert_eq!(series_of("9.5.0").as_deref(), Some("9.5"));
    assert_eq!(series_of("8.0.44").as_deref(), Some("8.0"));

    assert_eq!(series_of("8"), None, "a major alone is not a series");
    assert_eq!(series_of(""), None);
    assert_eq!(series_of("."), None);
}

/// Every MySQL build carries a signature and never a hash — which is the owner's decision
/// made unmissable rather than left in a comment.
#[test]
fn mysql_is_proved_by_a_signature_and_never_by_a_checksum() {
    for build in mysql_builds() {
        assert_eq!(build.engine, Engine::Mysql);
        match &build.proof {
            Proof::Signed {
                signature_url,
                keys,
            } => {
                assert_eq!(
                    *signature_url,
                    format!("{}.asc", build.url),
                    "the signature sits beside the archive"
                );
                assert!(
                    !keys.is_empty(),
                    "a signature with no key to check is not a check"
                );
                // The current Oracle key, so a rotation that is not carried fails a test
                // rather than failing a user's install.
                assert!(
                    keys.iter()
                        .any(|(key, _)| *key == "BCA43417C3B485DD128EC6D4B7B3B788A8D3785C"),
                    "Oracle's current release-engineering key should be accepted"
                );
            }
            other => panic!("MySQL must not be proved by {other:?}"),
        }
    }
}

/// Every PostgreSQL build is pinned, because EnterpriseDB publishes nothing to check against.
#[test]
fn postgres_is_proved_by_the_hash_this_build_carries() {
    for build in postgres_builds() {
        assert_eq!(build.engine, Engine::Postgres);
        assert!(
            matches!(build.proof, Proof::Pinned { .. }),
            "{:?}",
            build.proof
        );
    }
}

/// **A file with no usable hash is dropped, not offered unverified.** The one line in this
/// module that the guarantee rests on: an index that answers with `null`, or with something
/// that is not a SHA-256, must produce no offer at all.
#[test]
fn a_mariadb_file_without_a_real_hash_is_never_offered() {
    let good = "a".repeat(64);
    let json = format!(
        r#"{{"file_name": "mariadb-11.4.4-winx64.zip", "sha256sum": "{good}",
             "file_download_url": "https://example.test/good.zip"}},
           {{"file_name": "mariadb-11.4.4-winx64.zip", "sha256sum": null,
             "file_download_url": "https://example.test/null.zip"}},
           {{"file_name": "mariadb-11.4.4-winx64.zip", "sha256sum": "",
             "file_download_url": "https://example.test/empty.zip"}},
           {{"file_name": "mariadb-11.4.4-winx64.zip", "sha256sum": "not-a-hash",
             "file_download_url": "https://example.test/junk.zip"}},
           {{"file_name": "mariadb-11.4.4-winx64.zip",
             "file_download_url": "https://example.test/absent.zip"}}"#
    );

    let builds = read_mariadb_files(&json, Platform::Windows, "11.4.4");
    assert_eq!(builds.len(), 1, "only the one with a real hash: {builds:?}");
    assert_eq!(builds[0].url, "https://example.test/good.zip");
    assert!(matches!(
        &builds[0].proof,
        Proof::Published { sha256, .. } if *sha256 == good
    ));
}

/// The binary server archive for this platform, and not a source tarball, a debug build or a
/// directory entry.
#[test]
fn only_this_platforms_server_archive_is_offered() {
    let hash = "b".repeat(64);
    let row = |name: &str| {
        format!(
            r#"{{"file_name": "{name}", "sha256sum": "{hash}",
                 "file_download_url": "https://example.test/{name}"}}"#
        )
    };
    let json = [
        row("mariadb-11.4.4-winx64.zip"),
        row("mariadb-11.4.4-winx64-debug.zip"),
        row("mariadb-11.4.4-src.tar.gz"),
        row("mariadb-11.4.4-linux-systemd-x86_64.tar.gz"),
        row("yum/"),
    ]
    .join(",");

    let windows = read_mariadb_files(&json, Platform::Windows, "11.4.4");
    assert_eq!(
        windows
            .iter()
            .map(|b| b.file_name.clone())
            .collect::<Vec<_>>(),
        ["mariadb-11.4.4-winx64.zip"],
        "a debug build and a source tarball are not what somebody picked"
    );

    let linux = read_mariadb_files(&json, Platform::Linux, "11.4.4");
    assert_eq!(
        linux
            .iter()
            .map(|b| b.file_name.clone())
            .collect::<Vec<_>>(),
        ["mariadb-11.4.4-linux-systemd-x86_64.tar.gz"]
    );
}

/// **Newest first, by number and not by text.** Sorted as strings, `9.x` sits above `11.x`
/// and the menu offers the oldest release at the top — which is the kind of wrong that looks
/// right until somebody installs it.
#[test]
fn versions_are_offered_newest_first_and_stable_only() {
    let json = r#"
      {"release_id": "10.11.2", "release_status": "Stable"},
      {"release_id": "11.4.4", "release_status": "Stable"},
      {"release_id": "9.6.1", "release_status": "Stable"},
      {"release_id": "11.8.0", "release_status": "RC"},
      {"release_id": "11.2.3", "release_status": "Stable"}
    "#;

    assert_eq!(
        read_mariadb_versions(json),
        ["11.4.4", "11.2.3", "10.11.2", "9.6.1"],
        "an RC is not what somebody picking from a menu means by available"
    );
}

/// An index that arrives as nothing, or as an error document, offers nothing — rather than
/// panicking or inventing a version.
#[test]
fn an_index_that_did_not_arrive_offers_nothing() {
    for junk in ["", "null", "<html>503</html>", "{}", "not json at all"] {
        assert!(read_mariadb_versions(junk).is_empty(), "{junk}");
        assert!(
            read_mariadb_files(junk, Platform::Windows, "11.4.4").is_empty(),
            "{junk}"
        );
    }
}

/// Whatever is offered, it is offered for the machine it is running on.
#[test]
fn nothing_is_offered_for_a_platform_it_would_not_run_on() {
    let here = Platform::here();
    let all: Vec<Build> = postgres_builds()
        .into_iter()
        .chain(mysql_builds())
        .collect();

    for build in &all {
        assert!(
            here.is_some(),
            "{} was offered on a platform nothing is published for",
            build.file_name
        );
    }

    if here == Some(Platform::Windows) {
        assert!(
            all.iter()
                .any(|build| build.file_name.contains("windows")
                    || build.file_name.contains("winx64")),
            "on Windows the offers should be Windows builds: {all:?}"
        );
    }
}
