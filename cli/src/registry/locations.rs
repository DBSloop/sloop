//! Where the stores live, on each operating system.
//!
//! **One path, on all three:** `~/.sloop`, in the user's home directory, the way `~/.claude`
//! sits in theirs. `C:\Users\<name>\.sloop` on Windows, `/home/<name>/.sloop` on Linux,
//! `/Users/<name>/.sloop` on macOS. It replaced three platform conventions — `%APPDATA%\sloop`,
//! `~/.config/sloop` and `~/Library/Application Support/sloop` — and a store left in one of
//! those is moved here once, by [`super::adopt`], rather than quietly ignored.
//!
//! The paths are not guessed from a crate — they are computed from the environment so a test
//! can ask for all three from one machine. A path this tool cannot work out is a failure with
//! a sentence, never a panic.

use std::path::{Path, PathBuf};

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};

/// The directory a project keeps its registry in, beside the code it belongs to.
pub const PROJECT_DIR: &str = ".sloop";

/// Which set of conventions to follow. Separated from `cfg!` so every branch is reachable
/// from every host: the macOS path has to be verifiable without a Mac.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// Home from `HOME`. An older store at `$XDG_CONFIG_HOME/sloop` or `~/.config/sloop`.
    Linux,
    /// Home from `HOME`. An older store at `~/Library/Application Support/sloop`.
    MacOs,
    /// Home from `USERPROFILE`. An older store at `%APPDATA%\sloop`.
    Windows,
}

impl Platform {
    /// The platform this binary was built for.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }

    /// The variable a user would set to fix a missing home directory.
    const fn home_variable(self) -> &'static str {
        match self {
            Self::Windows => "USERPROFILE",
            Self::Linux | Self::MacOs => "HOME",
        }
    }
}

/// Everything outside the process that decides where the global store is.
#[derive(Debug, Clone, Default)]
pub struct Locations {
    platform: Platform,
    home: Option<PathBuf>,
    userprofile: Option<PathBuf>,
    xdg_config_home: Option<PathBuf>,
    appdata: Option<PathBuf>,
}

impl Default for Platform {
    fn default() -> Self {
        Self::current()
    }
}

impl Locations {
    /// Read the environment this process was given.
    #[must_use]
    pub fn from_process() -> Self {
        Self {
            platform: Platform::current(),
            home: var_path("HOME"),
            userprofile: var_path("USERPROFILE"),
            xdg_config_home: var_path("XDG_CONFIG_HOME"),
            appdata: var_path("APPDATA"),
        }
    }

    /// Build one by hand, for the platforms this host is not. The macOS path has to
    /// be checkable without a Mac, and the Linux one without Linux.
    #[cfg(test)]
    #[must_use]
    pub fn new(platform: Platform) -> Self {
        Self {
            platform,
            ..Self::default()
        }
    }

    /// Set `HOME`.
    #[cfg(test)]
    #[must_use]
    pub fn with_home(mut self, home: impl Into<PathBuf>) -> Self {
        self.home = Some(home.into());
        self
    }

    /// Set `USERPROFILE`.
    #[cfg(test)]
    #[must_use]
    pub fn with_userprofile(mut self, home: impl Into<PathBuf>) -> Self {
        self.userprofile = Some(home.into());
        self
    }

    /// Set `XDG_CONFIG_HOME`.
    #[cfg(test)]
    #[must_use]
    pub fn with_xdg_config_home(mut self, dir: impl Into<PathBuf>) -> Self {
        self.xdg_config_home = Some(dir.into());
        self
    }

    /// Set `APPDATA`.
    #[cfg(test)]
    #[must_use]
    pub fn with_appdata(mut self, dir: impl Into<PathBuf>) -> Self {
        self.appdata = Some(dir.into());
        self
    }

    /// The global store: the registry used when no project is in play.
    ///
    /// Normalised on the way out. The value arrives from an environment variable, and a
    /// shell that hands over `C:/Users/me` produces a working but half-and-half path once
    /// something is joined onto it. It works; it reads like a bug.
    pub fn global_dir(&self) -> Outcome<PathBuf> {
        Ok(super::normalize(&self.home_dir()?.join(PROJECT_DIR)))
    }

    /// The user's home directory — and so the line the walk up the tree stops at.
    ///
    /// **Both facts come from here on purpose.** With the global store at `~/.sloop`, a
    /// `git`-style walk that ran past the home directory would find the global store and
    /// treat it as a project, from anywhere under the home folder. The boundary and the
    /// store are the same answer, so they are computed in one place rather than two.
    pub fn home_dir(&self) -> Outcome<PathBuf> {
        Ok(super::normalize(self.raw_home()?))
    }

    fn raw_home(&self) -> Outcome<&Path> {
        // Windows first reads `USERPROFILE`, because a Git Bash shell exports `HOME` as
        // `/c/Users/me` — an MSYS path a native Windows binary cannot open. `HOME` stays as
        // the fallback for a shell that sets only that.
        let ordered: [&Option<PathBuf>; 2] = match self.platform {
            Platform::Windows => [&self.userprofile, &self.home],
            Platform::Linux | Platform::MacOs => [&self.home, &self.userprofile],
        };

        ordered
            .into_iter()
            .find_map(|value| value.as_deref())
            .ok_or_else(|| {
                let variable = self.platform.home_variable();
                Failure::new(
                    Exit::Usage,
                    format!("{variable} is not set, so there is nowhere to keep the global store"),
                )
                .hint(format!(
                    "set {variable}, or work inside a project created with `sloop init`"
                ))
            })
    }

    /// Where a store written by an older sloop would be, if this platform had one there.
    ///
    /// `None` when the environment does not say — a Windows machine with no `APPDATA` has
    /// no old store to find, which is a fact rather than a failure. Nothing here touches
    /// the disk; whether the directory exists is [`super::adopt`]'s question.
    #[must_use]
    pub fn legacy_dir(&self) -> Option<PathBuf> {
        let path = match self.platform {
            Platform::Linux => {
                // The XDG spec says a relative `XDG_CONFIG_HOME` must be ignored, and it
                // is worth honouring: a relative one would name a directory that follows
                // the working directory around.
                match self.xdg_config_home.as_deref().filter(rooted) {
                    Some(base) => base.join("sloop"),
                    None => self.raw_home().ok()?.join(".config").join("sloop"),
                }
            }
            Platform::MacOs => self
                .raw_home()
                .ok()?
                .join("Library")
                .join("Application Support")
                .join("sloop"),
            Platform::Windows => self.appdata.as_deref()?.join("sloop"),
        };

        Some(super::normalize(&path))
    }

    /// The store a whole machine shares, when this platform has such a place.
    ///
    /// **`~/.sloop` cannot be the answer for a machine.** Root's home is `0700` on every
    /// Linux built this decade, so a store inside it is unreadable to every other account and
    /// unreachable even to the postmaster sloop starts — which is how `R31` began, with a
    /// cluster that could not be created under `/root`. `/var/lib/<name>` is where the
    /// filesystem standard puts state an installed program keeps, and it is where Debian's
    /// own PostgreSQL keeps its clusters.
    ///
    /// **`None` on Windows**, which has no uid to drop to and no refusal to work around, so
    /// nothing there changes.
    #[must_use]
    pub fn machine_dir(&self) -> Option<PathBuf> {
        let path = match self.platform {
            Platform::Linux => PathBuf::from("/var/lib/sloop"),
            Platform::MacOs => PathBuf::from("/Library/Application Support/sloop"),
            Platform::Windows => return None,
        };

        Some(super::normalize(&path))
    }
}

/// Is this the store a whole machine shares, rather than one account's own?
///
/// Asked by the two places that create files in a store and have to know which kind they are
/// writing into — see [`crate::account::prepare_store`].
#[must_use]
pub fn is_machine_store(dir: &Path) -> bool {
    Locations::from_process()
        .machine_dir()
        .is_some_and(|machine| machine == dir)
}

/// Does this path start at the root, by POSIX rules?
///
/// Not `Path::is_absolute`: that answers for the host, and a Linux path checked from a
/// Windows machine is not absolute by Windows rules. The XDG branch has to mean what it
/// means on Linux whichever machine is asking, which is also what makes the rule testable
/// from here.
fn rooted(path: &&Path) -> bool {
    path.as_os_str().as_encoded_bytes().first() == Some(&b'/')
}

/// A variable that is set and not empty. An empty variable is how a shell says "unset",
/// and treating `HOME=` as a real path produces a store at `/.sloop`.
fn var_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Locations, Platform};

    fn global(locations: &Locations) -> String {
        slashes(
            &locations
                .global_dir()
                .expect("these fixtures all have what they need"),
        )
    }

    fn legacy(locations: &Locations) -> String {
        slashes(
            &locations
                .legacy_dir()
                .expect("these fixtures all have what they need"),
        )
    }

    fn slashes(path: &std::path::Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }

    #[test]
    fn linux_shares_a_machine_at_var_lib() {
        assert_eq!(
            Locations::new(Platform::Linux)
                .machine_dir()
                .as_deref()
                .map(slashes),
            Some("/var/lib/sloop".to_owned())
        );
    }

    #[test]
    fn macos_shares_a_machine_under_library() {
        // `~/Library/Application Support` is one account's. Without the `~` it is the
        // machine's, and that difference is the whole of the choice.
        assert_eq!(
            Locations::new(Platform::MacOs)
                .machine_dir()
                .as_deref()
                .map(slashes),
            Some("/Library/Application Support/sloop".to_owned())
        );
    }

    #[test]
    fn windows_shares_nothing_because_nothing_there_refuses_root() {
        // `None` is what keeps the rest of `R31` off Windows: no relocated store, no service
        // account, no dropped uid.
        assert_eq!(Locations::new(Platform::Windows).machine_dir(), None);
    }

    #[test]
    fn the_machine_store_does_not_follow_the_home_directory() {
        // It belongs to the machine, so `HOME` has nothing to say about it. One that moved
        // with `HOME` would be a different store under `sudo` than under `sudo -H`.
        assert_eq!(
            Locations::new(Platform::Linux)
                .with_home("/home/somebody-else")
                .machine_dir(),
            Locations::new(Platform::Linux).machine_dir()
        );
    }

    #[test]
    fn every_platform_keeps_the_global_store_in_the_home_directory() {
        assert_eq!(
            global(&Locations::new(Platform::Linux).with_home("/home/me")),
            "/home/me/.sloop"
        );
        assert_eq!(
            global(&Locations::new(Platform::MacOs).with_home("/Users/me")),
            "/Users/me/.sloop"
        );
        assert_eq!(
            global(&Locations::new(Platform::Windows).with_userprofile(r"C:\Users\me")),
            "C:/Users/me/.sloop"
        );
    }

    /// The store used to follow `XDG_CONFIG_HOME`, and no longer does. The variable still
    /// decides where an *old* store is looked for, which is a different question.
    #[test]
    fn xdg_config_home_no_longer_moves_the_store() {
        let locations = Locations::new(Platform::Linux)
            .with_home("/home/me")
            .with_xdg_config_home("/home/me/cfg");

        assert_eq!(global(&locations), "/home/me/.sloop");
        assert_eq!(legacy(&locations), "/home/me/cfg/sloop");
    }

    #[test]
    fn appdata_no_longer_moves_the_store_either() {
        let locations = Locations::new(Platform::Windows)
            .with_userprofile(r"C:\Users\me")
            .with_appdata(PathBuf::from(r"C:\Users\me\AppData\Roaming"));

        assert_eq!(global(&locations), "C:/Users/me/.sloop");
        assert_eq!(legacy(&locations), "C:/Users/me/AppData/Roaming/sloop");
    }

    /// A Git Bash shell exports `HOME` as an MSYS path a Windows binary cannot open, and
    /// exports `USERPROFILE` as the real one. The real one has to win.
    #[test]
    fn windows_prefers_userprofile_to_a_shell_provided_home() {
        let locations = Locations::new(Platform::Windows)
            .with_home("/c/Users/me")
            .with_userprofile(r"C:\Users\me");
        assert_eq!(global(&locations), "C:/Users/me/.sloop");
    }

    #[test]
    fn windows_falls_back_to_home_when_userprofile_is_not_set() {
        let locations = Locations::new(Platform::Windows).with_home(r"C:\Users\me");
        assert_eq!(global(&locations), "C:/Users/me/.sloop");
    }

    #[test]
    fn the_old_macos_store_is_the_one_apple_puts_there() {
        let locations = Locations::new(Platform::MacOs).with_home("/Users/me");
        assert_eq!(
            legacy(&locations),
            "/Users/me/Library/Application Support/sloop"
        );
    }

    #[test]
    fn the_old_linux_store_ignores_a_relative_xdg_config_home() {
        // A relative value would follow the working directory around, which is exactly
        // what a store must not do.
        let locations = Locations::new(Platform::Linux)
            .with_home("/home/me")
            .with_xdg_config_home("cfg");
        assert_eq!(legacy(&locations), "/home/me/.config/sloop");
    }

    #[test]
    fn with_nothing_in_the_environment_there_is_no_old_store_to_find() {
        for platform in [Platform::Linux, Platform::MacOs, Platform::Windows] {
            assert_eq!(Locations::new(platform).legacy_dir(), None);
        }
    }

    /// Windows only, because that is the only place the defect exists: a Git Bash shell
    /// exports these variables with forward slashes, and joining onto one gives a path that
    /// opens fine and reads like a bug.
    #[cfg(windows)]
    #[test]
    fn a_store_path_never_comes_out_half_and_half() {
        let locations = Locations::new(Platform::Windows)
            .with_userprofile("C:/Users/me")
            .with_appdata("C:/Users/me/AppData/Roaming");

        for path in [
            locations.global_dir().unwrap(),
            locations.home_dir().unwrap(),
            locations.legacy_dir().unwrap(),
        ] {
            assert!(
                !path.to_string_lossy().contains('/'),
                "mixed separators in {}",
                path.display()
            );
        }
        assert_eq!(global(&locations), "C:/Users/me/.sloop");
    }

    #[test]
    fn a_missing_home_is_a_sentence_and_not_a_panic() {
        for platform in [Platform::Linux, Platform::MacOs, Platform::Windows] {
            let failure = Locations::new(platform)
                .global_dir()
                .expect_err("no home, no store");
            assert_eq!(failure.exit().code(), 2);
        }
    }

    /// The Windows sentence has to name the variable a Windows user would set. `HOME` is
    /// not one of them, and being told to set it is being told to do the wrong thing.
    #[test]
    fn the_sentence_names_the_variable_this_platform_reads() {
        let windows = Locations::new(Platform::Windows)
            .global_dir()
            .expect_err("no home, no store");
        assert!(
            windows.message().contains("USERPROFILE"),
            "{}",
            windows.message()
        );

        let linux = Locations::new(Platform::Linux)
            .global_dir()
            .expect_err("no home, no store");
        assert!(linux.message().contains("HOME"), "{}", linux.message());
    }
}
