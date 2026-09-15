//! A throwaway world to run `sloop` in.
//!
//! Every integration test goes through here, and it exists because of a mistake worth not
//! repeating: the first run of these tests created a `.sloop` inside the repository and
//! registered a project in the real `%APPDATA%\sloop`. A test that can reach the machine
//! it is running on will eventually change it.
//!
//! So the sandbox hands the child process a `HOME`, an `APPDATA` and an `XDG_CONFIG_HOME`
//! of its own, all inside a temporary directory, and runs it in a working directory of its
//! own. Those three variables are the only inputs `Locations` has, so the global store
//! lands inside the sandbox on all three platforms with nothing mocked.
//!
//! Temporary directories are made by hand rather than with `tempfile`, because a
//! four-crate dependency for twenty lines is not a trade this project makes.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
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
        let home = root.join("home");
        let work = root.join("work");

        for dir in [&home, &work] {
            std::fs::create_dir_all(dir).expect("a temporary directory should be creatable");
        }

        Self { root, home, work }
    }

    /// The working directory the commands run in unless told otherwise.
    #[must_use]
    pub fn work(&self) -> &Path {
        &self.work
    }

    /// Where the global store will be, following the same rule the binary follows.
    #[must_use]
    pub fn global_dir(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.home
                .join("Library")
                .join("Application Support")
                .join("sloop")
        } else {
            // Linux reads XDG_CONFIG_HOME, Windows reads APPDATA, and the sandbox points
            // both at the same place.
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
            // The three variables the store location is computed from.
            .env("HOME", &self.home)
            .env("APPDATA", &self.home)
            .env("XDG_CONFIG_HOME", &self.home)
            // Anything the developer's shell happens to export must not reach a test.
            .env_remove("SLOOP_PROJECT")
            .env_remove("CLICOLOR_FORCE")
            .env_remove("CLICOLOR")
            .env_remove("NO_COLOR");

        Invocation {
            command,
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
    shown: String,
}

impl Invocation {
    /// Add an environment variable.
    #[must_use]
    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.command.env(key, value);
        self
    }

    /// Run it.
    pub fn run(mut self) -> Run {
        let output = self
            .command
            .output()
            .expect("the binary these tests were built alongside should run");
        Run {
            output,
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
