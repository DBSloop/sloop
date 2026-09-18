//! What gets written to disk, for each of the three things that run programs at boot.
//!
//! **Rendering is separated from installing so that all of it can be tested from one
//! machine.** A systemd unit cannot be checked on Windows and a `sc.exe` command line
//! cannot be checked on Linux — but the *text* of each can be checked anywhere, and the text
//! is where the mistakes live: a path with a space in it, an account name with an `&`, a
//! `binPath=` whose quoting collapses the moment somebody's username has a space. The
//! platform halves that follow this file do as little as possible beyond handing these
//! strings to `systemctl`, `launchctl` and `sc.exe`.
//!
//! **Three things are in every definition and none of them are guessed at run time.**
//!
//! ```text
//! program    the sloop binary, absolute -- a service manager has no PATH worth the name
//! store      where sloop's own state is, because a service does not inherit a home
//! key file   where the passphrase comes from, readable by the service account and nobody
//! ```
//!
//! The second is the one that surprises people. A Windows service runs as `LocalSystem`,
//! whose profile is `C:\Windows\System32\config\systemprofile` and not the profile of whoever
//! installed sloop; a systemd unit has no `HOME` at all unless it is given one. So the store
//! is written into the definition at install time rather than looked up at run time, and
//! `sloop service run --store <path>` is how the daemon is told.
//!
//! **No password is ever written into any of these files.** A unit file is world-readable by
//! design on all three platforms — `systemctl cat` prints it to anybody — so what goes in is
//! the *path* to a key file, and the key file is what carries the passphrase under
//! permissions only the service account satisfies. That is rule 3, in the one place where
//! getting it wrong would be invisible.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// What Windows and systemd call it. Lowercase and short, because it is what somebody types
/// into `systemctl status` and `sc query` at three in the morning.
pub const SERVICE_NAME: &str = "sloop";

/// What launchd calls it. Reverse-DNS is not a convention there, it is the rule: `launchctl`
/// refuses a label it does not like, and every other agent on the machine is named this way.
pub const LAUNCHD_LABEL: &str = "io.github.dbsloop.sloop";

/// The one line that says what it is, in every one of the three.
pub const DESCRIPTION: &str = "sloop - scheduled database backups and monitoring";

/// Everything the three renderers need, worked out once by `install` and written down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    /// The sloop binary, absolute.
    pub program: PathBuf,
    /// The store the daemon works from.
    pub store: PathBuf,
    /// The account it runs as. `None` on Windows, which uses `LocalSystem`.
    pub account: Option<String>,
    /// The file the passphrase is read from.
    pub key_file: PathBuf,
}

impl Definition {
    /// The arguments after the program name, shared by all three platforms.
    ///
    /// One place, so the three cannot drift into starting the daemon three different ways.
    #[must_use]
    pub fn arguments(&self) -> Vec<String> {
        vec![
            String::from("service"),
            String::from("run"),
            String::from("--store"),
            self.store.display().to_string(),
        ]
    }

    /// The systemd unit.
    ///
    /// **Almost none of systemd's hardening is here, and that is rule 0d rather than
    /// laziness.** `PrivateTmp=true` gives the service its own `/tmp`, and a PostgreSQL
    /// reached over a unix socket lives at `/tmp/.s.PGSQL.5432` — so the most commonly
    /// recommended line in every systemd hardening guide would make sloop unable to reach the
    /// database it was installed to watch, and the failure would read as a connection error.
    /// `ProtectHome=` does the same to the store and to a backup directory under a home.
    /// `NoNewPrivileges=` stays because nothing here ever needs to gain any.
    ///
    /// `Restart=on-failure` with a delay, not `always`: a daemon that exits cleanly because
    /// it was told to stop should stay stopped, and one that is crashing should not spin.
    #[must_use]
    pub fn systemd_unit(&self) -> String {
        let mut command = quoted_command(&self.program, &self.arguments());
        command.push('\n');

        let mut unit = String::new();
        unit.push_str("# Written by `sloop service install`. Changes here are lost the next\n");
        unit.push_str("# time it runs; `sloop service uninstall` removes the file.\n\n");
        unit.push_str("[Unit]\n");
        let _ = writeln!(unit, "Description={DESCRIPTION}");
        unit.push_str("Documentation=https://dbsloop.github.io\n");
        // `network-online` rather than `network`: a database on another host is not reachable
        // when an interface merely exists.
        unit.push_str("After=network-online.target\n");
        unit.push_str("Wants=network-online.target\n\n");

        unit.push_str("[Service]\n");
        unit.push_str("Type=simple\n");
        let _ = write!(unit, "ExecStart={command}");
        if let Some(account) = &self.account {
            let _ = writeln!(unit, "User={account}");
        }
        let _ = writeln!(
            unit,
            "Environment=SLOOP_PASSPHRASE_FILE={}",
            self.key_file.display()
        );
        unit.push_str("Restart=on-failure\n");
        unit.push_str("RestartSec=30\n");
        unit.push_str("NoNewPrivileges=true\n\n");

        unit.push_str("[Install]\n");
        unit.push_str("WantedBy=multi-user.target\n");
        unit
    }

    /// The launchd property list.
    ///
    /// `KeepAlive` with `SuccessfulExit=false` is launchd's way of saying the same thing as
    /// `Restart=on-failure`: bring it back if it died, leave it alone if it was told to go.
    #[must_use]
    pub fn launchd_plist(&self) -> String {
        let mut plist = String::new();
        plist.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        plist.push_str(
            "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
        );
        plist.push_str("<plist version=\"1.0\">\n<dict>\n");
        let _ = writeln!(
            plist,
            "    <!-- Written by `sloop service install`. -->\n    \
             <key>Label</key>\n    <string>{LAUNCHD_LABEL}</string>"
        );

        plist.push_str("    <key>ProgramArguments</key>\n    <array>\n");
        let _ = writeln!(
            plist,
            "        <string>{}</string>",
            escape_xml(&self.program.display().to_string())
        );
        for argument in self.arguments() {
            let _ = writeln!(plist, "        <string>{}</string>", escape_xml(&argument));
        }
        plist.push_str("    </array>\n");

        if let Some(account) = &self.account {
            let _ = writeln!(
                plist,
                "    <key>UserName</key>\n    <string>{}</string>",
                escape_xml(account)
            );
        }

        plist.push_str("    <key>EnvironmentVariables</key>\n    <dict>\n");
        let _ = writeln!(
            plist,
            "        <key>SLOOP_PASSPHRASE_FILE</key>\n        <string>{}</string>",
            escape_xml(&self.key_file.display().to_string())
        );
        plist.push_str("    </dict>\n");

        plist.push_str("    <key>RunAtLoad</key>\n    <true/>\n");
        plist.push_str("    <key>KeepAlive</key>\n    <dict>\n");
        plist.push_str("        <key>SuccessfulExit</key>\n        <false/>\n");
        plist.push_str("    </dict>\n");
        plist.push_str("</dict>\n</plist>\n");
        plist
    }

    /// What goes after `binPath=` in `sc.exe create`.
    ///
    /// **The quoting here is the whole of it.** `sc.exe` hands this string to the Service
    /// Control Manager as a single command line, which Windows then splits again with
    /// `CommandLineToArgvW` — so the program path needs its own quotes or
    /// `C:\Program Files\sloop\sloop.exe` starts something called `C:\Program`, and so does
    /// every argument that might hold a space. A store under `C:\Users\Jo Smith\.sloop` is an
    /// ordinary path and has to survive.
    #[must_use]
    pub fn windows_binary_path(&self) -> String {
        quoted_command(&self.program, &self.arguments())
    }
}

/// A program and its arguments as one command line, each part quoted when it needs to be.
///
/// Used for systemd's `ExecStart` and for `sc.exe`'s `binPath=`, which want the same shape
/// for the same reason: one string that something else splits back into words.
fn quoted_command(program: &Path, arguments: &[String]) -> String {
    let mut line = quote(&program.display().to_string());
    for argument in arguments {
        line.push(' ');
        line.push_str(&quote(argument));
    }
    line
}

/// One word, quoted if a space or a quote would otherwise split it in two.
///
/// Left bare when it does not need quoting, because `ExecStart=/usr/bin/sloop` is what a
/// person expects to read in a unit file and `ExecStart="/usr/bin/sloop"` makes them wonder
/// what is special about it.
fn quote(word: &str) -> String {
    if !word.is_empty() && !word.contains([' ', '\t', '"', '\\']) {
        return word.to_owned();
    }
    // A backslash is only an escape inside these quotes when it precedes a quote, but
    // doubling every one of them is correct in both readings and needs no lookahead.
    let escaped = word.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// XML's five, of which a plist can really meet three: a path with an `&` in it, an account
/// name with an angle bracket, and the quote that would end an attribute.
fn escape_xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(character),
        }
    }
    out
}
