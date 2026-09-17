//! A throwaway world to run `sloop` in.
//!
//! Every integration test goes through here, and it exists because of a mistake worth not
//! repeating: the first run of these tests created a `.sloop` inside the repository and
//! registered a project in the real `%APPDATA%\sloop`. A test that can reach the machine
//! it is running on will eventually change it.
//!
//! So the sandbox hands the child process a home directory of its own — `USERPROFILE` and
//! `HOME`, plus the `APPDATA` and `XDG_CONFIG_HOME` an older store would have been under —
//! all inside a temporary directory. Those variables are the only inputs `Locations` has, so
//! the global store lands inside the sandbox on all three platforms with nothing mocked.
//!
//! **The working directory sits inside that home directory, and that is the containment.**
//! `R19b` stops the walk up the tree at the home directory, so a run started anywhere under
//! the sandbox's home can no longer walk out of the sandbox at all. It is not a precaution:
//! `%TEMP%` on Windows sits under `%USERPROFILE%`, and a `.sloop` left in the real home
//! directory by a harness whose `cd` had failed made every one of these tests resolve to it
//! and write into it. [`Sandbox::new`] now asserts the containment rather than assuming it.
//!
//! Temporary directories are made by hand rather than with `tempfile`, because a
//! four-crate dependency for twenty lines is not a trade this project makes.

#![allow(dead_code)]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A temporary directory tree, removed when it goes out of scope.
pub struct Sandbox {
    root: PathBuf,
    home: PathBuf,
    work: PathBuf,
}

impl Sandbox {
    /// A fresh sandbox. `label` only has to make the directory name readable if a test
    /// ever leaves one behind.
    #[must_use]
    pub fn new(label: &str) -> Self {
        let unique = format!(
            "sloop-test-{}-{label}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let root = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&root).expect("a temporary directory should be creatable");

        // macOS puts its temporary directory under `/var`, which is a symlink to
        // `/private/var`, and `getcwd` in the child process resolves it. Without this the
        // sandbox and the binary hold two spellings of the same directory, and every path
        // assertion fails on exactly one platform. Not on Windows, where `canonicalize`
        // hands back a verbatim `\\?\` path that `getcwd` never produces — and where
        // nothing is symlinked anyway.
        #[cfg(not(windows))]
        let root = std::fs::canonicalize(&root).unwrap_or(root);

        let home = root.join("home");
        let work = home.join("work");

        for dir in [&home, &work] {
            std::fs::create_dir_all(dir).expect("a temporary directory should be creatable");
        }

        let sandbox = Self { root, home, work };
        sandbox.check_containment();
        sandbox
    }

    /// Fail loudly, here, if a run could reach outside the sandbox.
    ///
    /// The walk up the tree stops at the home directory, so a working directory inside the
    /// sandbox's home can never resolve to a project outside it. That holds only while the
    /// working directory really is inside the home directory — so it is checked rather than
    /// assumed, and the panic says why, because the alternative is a test suite that silently
    /// reads and writes the machine it is running on.
    fn check_containment(&self) {
        assert!(
            self.home.starts_with(&self.root),
            "the sandbox home {} is not inside the sandbox {}",
            self.home.display(),
            self.root.display()
        );
        assert!(
            self.work.starts_with(&self.home),
            "the sandbox working directory {} is not inside the sandbox home {}, so a run \
             started in it can walk out of the sandbox and into the real home directory",
            self.work.display(),
            self.home.display()
        );

        // Nothing between the working directory and the boundary may already be a project,
        // or a test would resolve to it without having asked for it.
        let mut dir = self.work.as_path();
        while dir != self.home {
            assert!(
                !dir.join(".sloop").exists(),
                "{} already holds a .sloop, so a fresh sandbox is not fresh",
                dir.display()
            );
            dir = dir.parent().expect("work is under home, so it has parents");
        }
    }

    /// The working directory the commands run in unless told otherwise.
    #[must_use]
    pub fn work(&self) -> &Path {
        &self.work
    }

    /// The sandbox's home directory — where the global store goes, and the line the walk
    /// up the tree stops at.
    #[must_use]
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Where the global store will be, following the same rule the binary follows:
    /// `~/.sloop`, on all three platforms.
    #[must_use]
    pub fn global_dir(&self) -> PathBuf {
        self.home.join(".sloop")
    }

    /// Where a store written by an older sloop would be, so a test can leave one there.
    ///
    /// The same three conventions the binary looks in, computed from the same variables the
    /// sandbox hands the child.
    #[must_use]
    pub fn legacy_dir(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.home
                .join("Library")
                .join("Application Support")
                .join("sloop")
        } else {
            // Linux reads XDG_CONFIG_HOME, Windows reads APPDATA, and the sandbox points
            // both at the home directory.
            self.home.join("sloop")
        }
    }

    /// The pointer file the index would hold for `name`.
    #[must_use]
    pub fn pointer(&self, name: &str) -> PathBuf {
        self.global_dir().join("projects").join(name)
    }

    /// Make a directory inside the sandbox and return it.
    ///
    /// `relative` is always written with forward slashes and pushed one component at a
    /// time, so the path that comes back uses this platform's separator throughout. A
    /// path with one stray slash left in the middle still opens fine on Windows but
    /// prints differently from the one the binary reports, which makes a passing test
    /// look like a failing one.
    pub fn make_dir(&self, relative: &str) -> PathBuf {
        let mut path = self.work.clone();
        for part in relative.split('/') {
            path.push(part);
        }
        std::fs::create_dir_all(&path).expect("a sandbox directory should be creatable");
        path
    }

    /// Run `sloop` in the sandbox's working directory.
    pub fn sloop(&self, args: &[&str]) -> Run {
        self.sloop_in(&self.work.clone(), args)
    }

    /// Run `sloop` somewhere specific inside the sandbox.
    pub fn sloop_in(&self, cwd: &Path, args: &[&str]) -> Run {
        self.command(cwd, args).run()
    }

    /// A command to add environment to before running it.
    #[must_use]
    pub fn command(&self, cwd: &Path, args: &[&str]) -> Invocation {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sloop"));
        command
            .args(args)
            .current_dir(cwd)
            // Every variable the store location is computed from — the home directory the
            // store lives in, and the three an older store would have been found under.
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("APPDATA", &self.home)
            .env("XDG_CONFIG_HOME", &self.home)
            // **The backup key goes in the sandbox too.** A registry with no keypair gets
            // one the first time `backup` or `key export` runs, and without this it would
            // land in the machine's own Credential Manager or Keychain — which no sandbox
            // can reach in to clean up. `SLOOP_PASSPHRASE` tells sloop this machine keeps
            // secrets in the Argon2id file, and that file is inside the sandbox.
            .env("SLOOP_PASSPHRASE", "a test passphrase")
            // Anything the developer's shell happens to export must not reach a test.
            .env_remove("SLOOP_PROJECT")
            .env_remove("CLICOLOR_FORCE")
            .env_remove("CLICOLOR")
            .env_remove("NO_COLOR");

        Invocation {
            command,
            stdin: None,
            shown: args.to_vec().join(" "),
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // Best effort. On Windows an open handle can keep a file alive for a moment, and
        // a test that has already passed should not fail over a temporary directory.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A prepared command, so a test can add to the environment before running it.
pub struct Invocation {
    command: Command,
    stdin: Option<Vec<u8>>,
    shown: String,
}

impl Invocation {
    /// Add an environment variable.
    #[must_use]
    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.command.env(key, value);
        self
    }

    /// Write these bytes to the child's standard input.
    ///
    /// Bytes rather than a string, and written verbatim: `--password-stdin` is supposed to
    /// take exactly what it was given, so a helper that appended a newline of its own would
    /// be testing something other than what a caller does.
    #[must_use]
    pub fn stdin(mut self, bytes: &[u8]) -> Self {
        self.stdin = Some(bytes.to_vec());
        self
    }

    /// Run it.
    ///
    /// **Standard input is closed unless a test opened it.** `Command::output` does that
    /// by itself, and it is the state that matters most here: it is what a scheduled run
    /// looks like, and rule 4 says nothing may stop and ask a question in it.
    pub fn run(mut self) -> Run {
        let Some(bytes) = self.stdin.take() else {
            let output = self
                .command
                .output()
                .expect("the binary these tests were built alongside should run");
            return Run {
                output,
                shown: self.shown,
            };
        };

        let mut child = self
            .command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the binary these tests were built alongside should run");

        // Dropped straight after, so the child sees end of file rather than waiting for
        // more — a pipe left open is the other way to make a command hang forever.
        {
            let mut pipe = child.stdin.take().expect("stdin was piped");
            pipe.write_all(&bytes).expect("writing to the child");
        }

        Run {
            output: child.wait_with_output().expect("the child should finish"),
            shown: self.shown,
        }
    }
}

/// What a run produced.
pub struct Run {
    output: Output,
    shown: String,
}

impl Run {
    /// The exit code, or `None` if a signal ended it.
    #[must_use]
    pub fn code(&self) -> Option<i32> {
        self.output.status.code()
    }

    /// Standard output as text.
    #[must_use]
    pub fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.output.stdout).into_owned()
    }

    /// Standard error as text.
    #[must_use]
    pub fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.output.stderr).into_owned()
    }

    /// Both streams, with runs of whitespace collapsed.
    ///
    /// Output is wrapped to a width and padded into columns, so a phrase in it can be
    /// split by a newline or spread by alignment at any moment. This asks what the command
    /// *said*, not what shape it happened to say it in.
    #[must_use]
    pub fn said(&self) -> String {
        let both = format!("{}\n{}", self.stdout(), self.stderr());
        both.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// Fail the test unless it exited with `code`, showing what it printed.
    pub fn expect_code(&self, code: i32) -> &Self {
        assert_eq!(
            self.code(),
            Some(code),
            "`sloop {}` should exit {code}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            self.shown,
            self.stdout(),
            self.stderr()
        );
        self
    }

    /// Fail the test unless the output contains `text`, ignoring how it was wrapped.
    pub fn expect_said(&self, text: &str) -> &Self {
        assert!(
            self.said().contains(text),
            "`sloop {}` never said {text:?}\n--- said ---\n{}",
            self.shown,
            self.said()
        );
        self
    }

    /// Fail the test if it exited with `code`.
    ///
    /// For a claim shaped "it got past the thing that would have stopped it": what matters
    /// is that the run was *not* refused, and pinning the code it did produce would be
    /// pinning something the machine decides.
    pub fn expect_not_code(&self, code: i32) -> &Self {
        assert_ne!(
            self.code(),
            Some(code),
            "`sloop {}` should not have exited {code}
--- stdout ---
{}
--- stderr ---
{}",
            self.shown,
            self.stdout(),
            self.stderr()
        );
        self
    }

    /// Fail the test if the output contains `text`.
    pub fn expect_silent_about(&self, text: &str) -> &Self {
        assert!(
            !self.said().contains(text),
            "`sloop {}` mentioned {text:?} and should not have\n--- said ---\n{}",
            self.shown,
            self.said()
        );
        self
    }
}

/// Is there a PostgreSQL client on this machine at all?
///
/// **A test that wants exit `3` has to say it needs one.** A connection that is *refused* is
/// exit `3`; a machine with no `psql` cannot refuse a connection, it can only fail to start
/// one, which is exit `2` and a different promise. That difference is what kept CI red on
/// Linux and macOS for twelve commits while passing on Windows, whose runner happens to ship
/// PostgreSQL — the tests meant "given a client" and never said so.
///
/// `PATH` only. sloop itself also looks where a package manager hides them, so a machine with
/// `psql` off `PATH` skips something it could have run — which is a visible skip rather than a
/// false pass, and CI installs a client so it never happens there.
#[must_use]
pub fn a_postgres_client_is_installed() -> bool {
    let name = if cfg!(windows) { "psql.exe" } else { "psql" };

    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|directory| directory.join(name).is_file())
    })
}

/// Say why an assertion is not being made, so a skip is something somebody can see.
pub fn skipping(what: &str) {
    eprintln!("skipping {what}: this machine has no psql, so a connection cannot be refused");
}
