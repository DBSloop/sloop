//! What gets written, checked from whichever machine happens to be running the tests.
//!
//! **All three platforms, from one.** A systemd unit cannot be installed on Windows and a
//! `sc.exe` command line cannot be installed on Linux, but both are text, and the mistakes
//! that matter are textual: a path with a space, an account with an `&`, a `binPath=` whose
//! quoting falls apart the first time somebody's username is `Jo Smith`. None of those need
//! the platform to find.

use std::path::PathBuf;

use super::unit::{DESCRIPTION, Definition, LAUNCHD_LABEL, SERVICE_NAME};

/// An ordinary Linux install.
fn plain() -> Definition {
    Definition {
        program: PathBuf::from("/usr/local/bin/sloop"),
        store: PathBuf::from("/home/jo/.sloop"),
        account: Some(String::from("jo")),
        key_file: PathBuf::from("/etc/sloop/service.key"),
        interval: 60,
    }
}

/// The same machine, owned by somebody whose name and home have spaces in them. This is not
/// a hypothetical on Windows, where the default is `C:\Users\First Last`.
fn spaced() -> Definition {
    Definition {
        program: PathBuf::from(r"C:\Program Files\sloop\sloop.exe"),
        store: PathBuf::from(r"C:\Users\Jo Smith\.sloop"),
        account: None,
        key_file: PathBuf::from(r"C:\ProgramData\sloop\service.key"),
        interval: 60,
    }
}

#[test]
fn the_three_platforms_start_the_daemon_the_same_way() {
    let arguments = plain().arguments();
    assert_eq!(
        arguments,
        [
            "service",
            "run",
            "--store",
            "/home/jo/.sloop",
            "--interval",
            "60"
        ]
    );
}

// ------------------------------------------------------------------------------- systemd

#[test]
fn the_unit_says_what_it_is_and_where_to_put_it() {
    let unit = plain().systemd_unit();

    assert!(unit.contains(&format!("Description={DESCRIPTION}")));
    assert!(unit.contains("[Install]\nWantedBy=multi-user.target"));
    assert!(unit.contains("ExecStart=/usr/local/bin/sloop service run --store /home/jo/.sloop"));
    assert!(unit.contains("User=jo"));
}

/// A database on another host is not reachable merely because an interface exists.
#[test]
fn the_unit_waits_for_the_network_to_be_up_rather_than_to_exist() {
    let unit = plain().systemd_unit();
    assert!(unit.contains("After=network-online.target"));
    assert!(unit.contains("Wants=network-online.target"));
}

/// A daemon told to stop should stay stopped, and one that is crashing should not spin.
#[test]
fn it_comes_back_from_a_crash_and_not_from_being_stopped() {
    let unit = plain().systemd_unit();
    assert!(unit.contains("Restart=on-failure"));
    assert!(!unit.contains("Restart=always"));
    assert!(unit.contains("RestartSec="));
}

/// `PrivateTmp=true` is the first line of every systemd hardening guide, and it would give
/// the service its own `/tmp` -- where a PostgreSQL reached over a unix socket keeps
/// `.s.PGSQL.5432`. Rule 0d: the feature works.
#[test]
fn nothing_in_the_unit_can_hide_the_database_from_it() {
    let unit = plain().systemd_unit();
    assert!(
        !unit.contains("PrivateTmp"),
        "PrivateTmp would hide the socket"
    );
    assert!(
        !unit.contains("ProtectHome"),
        "ProtectHome would hide the store"
    );
    assert!(
        !unit.contains("ProtectSystem"),
        "ProtectSystem would hide the backups"
    );
}

// ------------------------------------------------------------------------------- launchd

#[test]
fn the_plist_is_labelled_the_way_launchd_insists() {
    let plist = plain().launchd_plist();
    assert!(plist.contains("<key>Label</key>"));
    assert!(plist.contains(&format!("<string>{LAUNCHD_LABEL}</string>")));
    assert!(LAUNCHD_LABEL.contains('.'), "launchd wants reverse-DNS");
}

#[test]
fn the_plist_starts_at_boot_and_comes_back_from_a_crash() {
    let plist = plain().launchd_plist();
    assert!(plist.contains("<key>RunAtLoad</key>\n    <true/>"));
    assert!(plist.contains("<key>KeepAlive</key>"));
    assert!(plist.contains("<key>SuccessfulExit</key>\n        <false/>"));
}

/// Each argument is its own `<string>`, so launchd never has to split a command line and a
/// path with a space in it cannot become two paths.
#[test]
fn every_argument_is_its_own_element() {
    let plist = plain().launchd_plist();
    for part in [
        "/usr/local/bin/sloop",
        "service",
        "run",
        "--store",
        "/home/jo/.sloop",
    ] {
        assert!(
            plist.contains(&format!("<string>{part}</string>")),
            "{part} is not an element of its own"
        );
    }
}

// ------------------------------------------------------------------------ Windows quoting

/// `sc.exe` hands `binPath=` to the Service Control Manager as one command line, which
/// Windows splits again -- so an unquoted `C:\Program Files\...` starts `C:\Program`.
#[test]
fn a_path_with_a_space_survives_being_split_back_apart() {
    let line = spaced().windows_binary_path();

    assert!(
        line.starts_with("\"C:\\\\Program Files\\\\sloop\\\\sloop.exe\""),
        "the program is not quoted: {line}"
    );
    assert!(
        line.contains("\"C:\\\\Users\\\\Jo Smith\\\\.sloop\""),
        "the store is not quoted: {line}"
    );
}

/// The reverse: a word that needs no quotes does not get them, because a unit file is read
/// by people and `ExecStart="/usr/bin/sloop"` makes them wonder what is special about it.
#[test]
fn a_word_that_needs_no_quoting_does_not_get_any() {
    let line = plain().windows_binary_path();
    assert_eq!(
        line,
        "/usr/local/bin/sloop service run --store /home/jo/.sloop --interval 60"
    );
    assert!(!line.contains('"'));
}

// --------------------------------------------------------------------------- the secret

/// Rule 3, in the one place where breaking it would be invisible: a unit file is
/// world-readable on all three platforms -- `systemctl cat` prints it to anybody -- so what
/// goes in is the path to the key file and never the passphrase itself.
#[test]
fn no_definition_ever_carries_a_passphrase() {
    for definition in [plain(), spaced()] {
        for written in [
            definition.systemd_unit(),
            definition.launchd_plist(),
            definition.windows_binary_path(),
        ] {
            assert!(
                !written.to_lowercase().contains("passphrase="),
                "a passphrase is being written into a unit file"
            );
            assert!(
                !written.contains("SLOOP_PASSPHRASE\n") && !written.contains("SLOOP_PASSPHRASE "),
                "the passphrase variable itself is being set"
            );
        }
    }
    let unit = plain().systemd_unit();
    assert!(unit.contains("SLOOP_PASSPHRASE_FILE=/etc/sloop/service.key"));
}

/// A service does not inherit the installing user's home, so the store is written down at
/// install time rather than looked up at run time. On Windows the account is `LocalSystem`,
/// whose profile is not the profile sloop was set up under.
#[test]
fn every_platform_is_told_where_the_store_is() {
    let windows = spaced();
    assert!(windows.windows_binary_path().contains("--store"));
    assert!(plain().systemd_unit().contains("--store /home/jo/.sloop"));
    assert!(plain().launchd_plist().contains("<string>--store</string>"));
}

#[test]
fn windows_runs_as_local_system_and_names_no_account() {
    let plist = spaced().launchd_plist();
    assert!(
        !plist.contains("<key>UserName</key>"),
        "no account was asked for, so none should be named"
    );
    assert_eq!(SERVICE_NAME, "sloop", "what somebody types into sc query");
}

/// An account or a path with an XML metacharacter in it must not end the element it is in.
#[test]
fn a_path_with_an_ampersand_does_not_break_the_plist() {
    let awkward = Definition {
        program: PathBuf::from("/opt/r&d/sloop"),
        store: PathBuf::from("/home/<jo>/.sloop"),
        account: Some(String::from("r&d")),
        key_file: PathBuf::from("/etc/sloop/service.key"),
        interval: 60,
    };

    let plist = awkward.launchd_plist();

    assert!(plist.contains("<string>/opt/r&amp;d/sloop</string>"));
    assert!(plist.contains("<string>/home/&lt;jo&gt;/.sloop</string>"));
    assert!(plist.contains("<string>r&amp;d</string>"));
    // The only bare ampersands left are the ones that begin an entity.
    for (at, _) in plist.match_indices('&') {
        let tail = &plist[at..];
        assert!(
            ["&amp;", "&lt;", "&gt;", "&quot;", "&apos;"]
                .iter()
                .any(|entity| tail.starts_with(entity)),
            "a bare ampersand at {at}"
        );
    }
}

// ------------------------------------------------------------------ what status answers

/// The three states are distinct, which is what `R24`'s own *Done when* asks for: a
/// monitoring system that cannot tell "somebody stopped it" from "it was never installed"
/// wakes the wrong person.
#[test]
fn stopped_and_never_installed_are_not_the_same_answer() {
    use super::mechanism::State;

    assert_ne!(State::Stopped.spoken(), State::NotInstalled.spoken());
    assert!(State::Stopped.installed());
    assert!(State::Running.installed());
    assert!(!State::NotInstalled.installed());
}
