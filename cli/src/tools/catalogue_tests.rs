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
                        .any(|key| key.fingerprint == "BCA43417C3B485DD128EC6D4B7B3B788A8D3785C"),
                    "Oracle's current release-engineering key should be accepted"
                );
                // And the key itself, not only its fingerprint. A fingerprint is what you
                // compare a key against; on its own it can never verify anything, which the
                // first real run of this found out the expensive way.
                assert!(
                    keys.iter().all(|key| !key.armored.is_empty()),
                    "a carried fingerprint with no key behind it cannot check a signature"
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

/// One release document, in the shape `/rest-api/mariadb/<major>/latest/` really returns —
/// which is not the shape this module first assumed. The SHA-256 is **nested under
/// `checksum`**, beside an MD5 and a SHA-1, and `file_download_url` comes after that object.
fn a_release(version: &str, files: &str) -> String {
    format!(
        r#"{{"major_release_id": "11.4", "major_release_status": "Stable",
             "releases": {{"{version}": {{
                "release_id": "{version}",
                "release_name": "MariaDB Server {version}",
                "files": [{files}]}}}}}}"#
    )
}

/// One file entry, with its hash where the real endpoint puts it.
fn a_file(name: &str, os: &str, sha256: &str) -> String {
    format!(
        r#"{{"file_id": 1, "file_name": "{name}", "package_type": "ZIP file",
             "os": "{os}", "cpu": "x86_64",
             "checksum": {{"md5sum": "0123", "sha1sum": "4567", "sha256sum": {sha256}}},
             "file_download_url": "http://downloads.mariadb.org/rest-api/{name}",
             "signature": "-----BEGIN PGP SIGNATURE-----",
             "checksum_url": "http://downloads.mariadb.org/x/checksum/",
             "signature_url": "http://downloads.mariadb.org/x/signature/"}}"#
    )
}

/// **A file with no usable hash is dropped, not offered unverified.** The one line in this
/// module that the guarantee rests on: an index that answers with `null`, or with something
/// that is not a SHA-256, must produce no offer at all.
#[test]
fn a_mariadb_file_without_a_real_hash_is_never_offered() {
    let good = "a".repeat(64);
    let json = a_release(
        "11.4.4",
        &[
            a_file(
                "mariadb-11.4.4-winx64.zip",
                "Windows",
                &format!("\"{good}\""),
            ),
            a_file("mariadb-11.4.4-winx64.zip", "Windows", "null"),
            a_file("mariadb-11.4.4-winx64.zip", "Windows", "\"\""),
            a_file("mariadb-11.4.4-winx64.zip", "Windows", "\"not-a-hash\""),
        ]
        .join(","),
    );

    let builds = read_mariadb_files(&json, Platform::Windows, "11.4.4");
    assert_eq!(builds.len(), 1, "only the one with a real hash: {builds:?}");
    assert!(matches!(
        &builds[0].proof,
        Proof::Published { sha256, .. } if *sha256 == good
    ));
}

/// **The index hands out `http://` for its own downloads, and sloop corrects it.** `acquire`
/// refuses anything that is not https at both ends of a redirect chain, deliberately — so the
/// scheme is fixed rather than the refusal relaxed.
#[test]
fn an_http_download_url_is_taken_over_tls_instead() {
    let json = a_release(
        "11.4.4",
        &a_file(
            "mariadb-11.4.4-winx64.zip",
            "Windows",
            &format!("\"{}\"", "c".repeat(64)),
        ),
    );

    let builds = read_mariadb_files(&json, Platform::Windows, "11.4.4");
    assert_eq!(builds.len(), 1);
    assert!(
        builds[0].url.starts_with("https://"),
        "the bytes should not cross the network in the clear: {}",
        builds[0].url
    );
}

/// The binary server archive for this platform, and not a source tarball, a debug build or a
/// directory entry.
#[test]
fn only_this_platforms_server_archive_is_offered() {
    let hash = format!("\"{}\"", "b".repeat(64));
    let json = a_release(
        "11.4.4",
        &[
            a_file("mariadb-11.4.4-winx64.zip", "Windows", &hash),
            a_file("mariadb-11.4.4-winx64-debugsymbols.zip", "Windows", &hash),
            a_file("mariadb-11.4.4-src.tar.gz", "Source", &hash),
            a_file("mariadb-11.4.4-linux-systemd-x86_64.tar.gz", "Linux", &hash),
            a_file("yum/", "Linux", &hash),
        ]
        .join(","),
    );

    let names = |platform| {
        read_mariadb_files(&json, platform, "11.4.4")
            .iter()
            .map(|build| build.file_name.clone())
            .collect::<Vec<_>>()
    };

    assert_eq!(
        names(Platform::Windows),
        ["mariadb-11.4.4-winx64.zip"],
        "a debug build and a source tarball are not what somebody picked"
    );
    assert_eq!(
        names(Platform::Linux),
        ["mariadb-11.4.4-linux-systemd-x86_64.tar.gz"]
    );
    assert!(names(Platform::MacOs).is_empty(), "nothing macOS here");
}

/// A release that is not the one asked for offers nothing, so a document that came back for
/// some other version cannot be installed as this one.
#[test]
fn only_the_release_that_was_asked_for_is_read() {
    let json = a_release(
        "11.4.4",
        &a_file(
            "mariadb-11.4.4-winx64.zip",
            "Windows",
            &format!("\"{}\"", "e".repeat(64)),
        ),
    );

    assert!(read_mariadb_files(&json, Platform::Windows, "11.4.3").is_empty());
    assert_eq!(
        read_mariadb_files(&json, Platform::Windows, "11.4.4").len(),
        1
    );
}

/// **Newest first, by number and not by text.** Sorted as strings, `9.x` sits above `11.x`
/// and the menu offers the oldest release at the top — which is the kind of wrong that looks
/// right until somebody installs it.
#[test]
fn versions_are_offered_newest_first_and_stable_only() {
    let json = r#"{"releases": {
        "10.11.2": {"release_id": "10.11.2", "release_status": "Stable", "files": []},
        "11.4.4":  {"release_id": "11.4.4",  "release_status": "Stable", "files": []},
        "9.6.1":   {"release_id": "9.6.1",   "release_status": "Stable", "files": []},
        "11.8.0":  {"release_id": "11.8.0",  "release_status": "RC",     "files": []},
        "11.2.3":  {"release_id": "11.2.3",  "release_status": "Stable", "files": []}
    }}"#;

    assert_eq!(
        read_mariadb_versions(json),
        ["11.4.4", "11.2.3", "10.11.2", "9.6.1"],
        "an RC is not what somebody picking from a menu means by available"
    );
}

/// The release endpoint leaves `release_status` out altogether, which is not a reason to
/// refuse the only release it returned.
#[test]
fn a_release_that_does_not_say_what_it_is_is_still_a_release() {
    let json = a_release("11.4.4", "");
    assert_eq!(read_mariadb_versions(&json), ["11.4.4"]);
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

// ---------------------------------------------------------------------------------------
// Resolving an engine into a list, and a list row into a build
// ---------------------------------------------------------------------------------------

use super::{Choice, choices, resolve, why_nothing_is_offered};
use crate::failure::Outcome;

/// What `downloads.mariadb.org/rest-api/mariadb/` really answers with, trimmed to the fields
/// that are read. Two stable majors, one release candidate and one older one out of order.
const MAJORS: &str = r#"{
  "major_releases": [
    {"release_id": "12.0", "release_name": "12.0",
     "release_status": "RC", "release_support_type": "Rolling", "release_eol_date": null},
    {"release_id": "10.11", "release_name": "10.11",
     "release_status": "Stable", "release_support_type": "Long Term Support",
     "release_eol_date": "2028-02-16"},
    {"release_id": "11.8", "release_name": "11.8",
     "release_status": "Stable", "release_support_type": "Long Term Support",
     "release_eol_date": "2030-02-13"},
    {"release_id": "9.6", "release_name": "9.6",
     "release_status": "Stable", "release_support_type": "Short Term Support",
     "release_eol_date": null}
  ]
}"#;

/// What `…/11.8/latest/` really answers with, built out of the same two helpers the readers
/// above are tested with — so there is one description of the real document, not two.
fn latest(version: &str, hash: &str) -> String {
    let files = [
        a_file(
            &format!("mariadb-{version}-winx64.zip"),
            "Windows",
            &format!("\"{hash}\""),
        ),
        a_file(
            &format!("mariadb-{version}-linux-systemd-x86_64.tar.gz"),
            "Linux",
            &format!("\"{hash}\""),
        ),
        a_file(
            &format!("mariadb-{version}-macos-arm64.tar.gz"),
            "macOS",
            &format!("\"{hash}\""),
        ),
    ]
    .join(",");
    a_release(version, &files)
}

/// A reader that answers from a table, so the real resolution runs against the real shapes
/// without anything reaching the network.
fn from_a_table<'a>(pairs: &'a [(&'a str, String)]) -> impl Fn(&str) -> Outcome<String> + 'a {
    move |url: &str| {
        // **Matched on the end of the URL, not on any part of it.** `rest-api/mariadb/` is a
        // prefix of every one of these, so a table keyed on "contains" would answer the
        // release endpoint with the list of majors — which is exactly the kind of wrong that
        // looks right.
        pairs
            .iter()
            .find(|(held, _)| url.ends_with(held))
            .map(|(_, text)| text.clone())
            .ok_or_else(|| crate::failure::Failure::usage(format!("nothing is published at {url}")))
    }
}

/// **MariaDB's majors come back newest first, stable only, each with what it is.** A release
/// candidate is not what somebody picking a server out of a menu means by "available".
#[test]
fn mariadb_offers_its_stable_majors_newest_first() {
    let table = [("rest-api/mariadb/", MAJORS.to_owned())];
    let offered = choices(Engine::Mariadb, &from_a_table(&table)).expect("the index was readable");

    let versions: Vec<&str> = offered
        .iter()
        .map(|choice| choice.version.as_str())
        .collect();
    assert_eq!(versions, ["11.8", "10.11", "9.6"], "12.0 is an RC");

    assert_eq!(offered[0].note, "Long Term Support");
    assert!(
        offered.iter().all(|choice| choice.build.is_none()),
        "a major resolves to a build only once it has been chosen"
    );
}

/// **And choosing one reads its files in the same breath as its version**, so what gets
/// installed is the release whose hash was read rather than one resolved separately.
#[test]
fn choosing_a_major_resolves_the_release_inside_it_and_its_checksum() {
    let Some(platform) = Platform::here() else {
        eprintln!("skipping: nothing is published for this machine");
        return;
    };

    let hash = "d".repeat(64);
    let table = [
        ("rest-api/mariadb/", MAJORS.to_owned()),
        ("11.8/latest/", latest("11.8.2", &hash)),
    ];
    let read = from_a_table(&table);

    let offered = choices(Engine::Mariadb, &read).expect("readable");
    let build = resolve(Engine::Mariadb, &offered[0], &read).expect("11.8 has a release in it");

    assert_eq!(build.engine, Engine::Mariadb);
    assert_eq!(build.version, "11.8.2", "the release, not the major");
    assert!(
        matches!(&build.proof, Proof::Published { sha256, from }
                 if *sha256 == hash && from == "downloads.mariadb.org"),
        "{:?}",
        build.proof
    );
    assert!(
        build.url.starts_with("https://"),
        "the url comes out of the index: {}",
        build.url
    );
    // And it is this machine's archive, not one of the other two in the same release.
    let wanted = match platform {
        Platform::Windows => "winx64.zip",
        Platform::Linux => "linux-systemd-x86_64.tar.gz",
        Platform::MacOs => "macos-arm64.tar.gz",
    };
    assert!(build.file_name.ends_with(wanted), "{}", build.file_name);
}

/// **A major that lists no release for this machine is a sentence, not a download.** This is
/// the refusal that keeps "the checksum is verified before anything is run" honest when the
/// index has nothing to verify against.
#[test]
fn a_release_with_nothing_publishable_refuses_rather_than_offering() {
    let table = [
        ("rest-api/mariadb/", MAJORS.to_owned()),
        // A release whose files have no usable hash, which `read_mariadb_files` drops.
        ("11.8/latest/", latest("11.8.2", "not-a-hash")),
    ];
    let read = from_a_table(&table);

    let offered = choices(Engine::Mariadb, &read).expect("readable");
    let failure = resolve(Engine::Mariadb, &offered[0], &read)
        .expect_err("a file with no usable hash is never offered");
    assert!(
        failure.message().contains("checksum"),
        "{}",
        failure.message()
    );
}

/// A build that is already known needs no index at all — which is what makes PostgreSQL and
/// MySQL resolvable on a machine with no network.
#[test]
fn an_engine_whose_versions_are_a_table_never_reads_an_index() {
    let read = |url: &str| -> Outcome<String> {
        panic!("nothing should have been fetched, and something asked for {url}")
    };

    for engine in [Engine::Postgres, Engine::Mysql] {
        let offered = choices(engine, &read).expect("a table needs no index");
        for choice in &offered {
            let build = resolve(engine, choice, &read).expect("already known");
            assert_eq!(build.version, choice.version);
        }
    }
}

/// **Nothing to offer is answered with a reason, not with silence.** A Linux machine asking
/// for PostgreSQL has a perfectly good answer, and the whole reason this screen exists is
/// that somebody should not have to go and find it out for themselves.
#[test]
fn an_engine_with_nothing_to_offer_still_says_what_to_do() {
    for engine in Engine::ALL {
        let said = why_nothing_is_offered(engine);
        assert!(!said.is_empty(), "{engine} says nothing");
        assert!(
            said.ends_with('.'),
            "{engine} should read as a sentence: {said}"
        );
    }
}

/// Whatever else it is, a `Choice` is the thing the screen prints — so it has to carry a
/// version somebody can read.
#[test]
fn every_offer_names_a_version() {
    let read = |_: &str| -> Outcome<String> { Ok(MAJORS.to_owned()) };

    for engine in Engine::ALL {
        for choice in choices(engine, &read).expect("readable") {
            let Choice { version, .. } = &choice;
            assert!(!version.is_empty(), "{engine} offered a nameless version");
        }
    }
}

/// **The majors document is read as the real endpoint writes it**, including the fields this
/// module ignores — an index that grows a column must not stop being readable.
#[test]
fn the_majors_document_is_read_as_it_is_really_published() {
    let majors = super::read_mariadb_majors(MAJORS);

    let versions: Vec<&str> = majors
        .iter()
        .map(|choice| choice.version.as_str())
        .collect();
    assert_eq!(versions, ["11.8", "10.11", "9.6"], "12.0 is an RC");
    assert_eq!(majors[0].note, "Long Term Support");
    assert!(super::read_mariadb_majors("{}").is_empty());
}
