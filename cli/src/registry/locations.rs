//! Where the stores live, on each operating system.
//!
//! The paths are not guessed from a crate — they are the three the owner named, and they
//! are computed from the environment so a test can ask for all three from one machine.
//! A path this tool cannot work out is a failure with a sentence, never a panic.

use std::path::{Path, PathBuf};

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};

/// The directory a project keeps its registry in, beside the code it belongs to.
pub const PROJECT_DIR: &str = ".sloop";

/// Which set of conventions to follow. Separated from `cfg!` so every branch is reachable
/// from every host: the macOS path has to be verifiable without a Mac.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// `$XDG_CONFIG_HOME/sloop`, or `~/.config/sloop`.
    Linux,
    /// `~/Library/Application Support/sloop`.
    MacOs,
    /// `%APPDATA%\sloop`.
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
}

/// Everything outside the process that decides where the global store is.
#[derive(Debug, Clone, Default)]
pub struct Locations {
    platform: Platform,
    home: Option<PathBuf>,
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
    /// shell that hands over `C:/Users/me/AppData/Roaming` produces a working but
    /// half-and-half path once something is joined onto it. It works; it reads like a bug.
    pub fn global_dir(&self) -> Outcome<PathBuf> {
        self.raw_global_dir().map(|path| super::normalize(&path))
    }

    fn raw_global_dir(&self) -> Outcome<PathBuf> {
        match self.platform {
            Platform::Linux => {
                // The XDG spec says a relative `XDG_CONFIG_HOME` must be ignored, and it
                // is worth honouring: a relative one would put the store wherever the
                // command happened to be run from.
                if let Some(base) = self.xdg_config_home.as_deref().filter(rooted) {
                    return Ok(base.join("sloop"));
                }
                Ok(self.home()?.join(".config").join("sloop"))
            }
            Platform::MacOs => Ok(self
                .home()?
                .join("Library")
                .join("Application Support")
                .join("sloop")),
            Platform::Windows => self
                .appdata
                .clone()
                .map(|base| base.join("sloop"))
                .ok_or_else(|| {
                    Failure::new(
                        Exit::Usage,
                        "APPDATA is not set, so there is nowhere to keep the global store",
                    )
                    .hint("set APPDATA, or work inside a project created with `sloop init`")
                }),
        }
    }

    fn home(&self) -> Outcome<&Path> {
        self.home.as_deref().ok_or_else(|| {
            Failure::new(
                Exit::Usage,
                "HOME is not set, so there is nowhere to keep the global store",
            )
            .hint("set HOME, or work inside a project created with `sloop init`")
        })
    }
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
/// and treating `HOME=` as a real path produces a store at `/.config/sloop`.
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
        locations
            .global_dir()
            .expect("these fixtures all have what they need")
            .to_string_lossy()
            .replace('\\', "/")
    }

    #[test]
    fn linux_uses_xdg_config_home_when_it_is_absolute() {
        let locations = Locations::new(Platform::Linux)
            .with_home("/home/me")
            .with_xdg_config_home("/home/me/cfg");
        assert_eq!(global(&locations), "/home/me/cfg/sloop");
    }

    #[test]
    fn linux_falls_back_to_dot_config() {
        let locations = Locations::new(Platform::Linux).with_home("/home/me");
        assert_eq!(global(&locations), "/home/me/.config/sloop");
    }

    #[test]
    fn linux_ignores_a_relative_xdg_config_home() {
        // A relative value would follow the working directory around, which is exactly
        // what a global store must not do.
        let locations = Locations::new(Platform::Linux)
            .with_home("/home/me")
            .with_xdg_config_home("cfg");
        assert_eq!(global(&locations), "/home/me/.config/sloop");
    }

    #[test]
    fn macos_uses_application_support() {
        let locations = Locations::new(Platform::MacOs).with_home("/Users/me");
        assert_eq!(
            global(&locations),
            "/Users/me/Library/Application Support/sloop"
        );
    }

    #[test]
    fn macos_ignores_xdg_config_home() {
        let locations = Locations::new(Platform::MacOs)
            .with_home("/Users/me")
            .with_xdg_config_home("/Users/me/cfg");
        assert_eq!(
            global(&locations),
            "/Users/me/Library/Application Support/sloop"
        );
    }

    #[test]
    fn windows_uses_appdata() {
        let locations = Locations::new(Platform::Windows)
            .with_appdata(PathBuf::from(r"C:\Users\me\AppData\Roaming"));
        assert_eq!(global(&locations), "C:/Users/me/AppData/Roaming/sloop");
    }

    /// Windows only, because that is the only place the defect exists: a Git Bash shell
    /// exports `APPDATA` with forward slashes, and joining onto it gives a path that opens
    /// fine and reads like a bug.
    #[cfg(windows)]
    #[test]
    fn a_store_path_never_comes_out_half_and_half() {
        let locations =
            Locations::new(Platform::Windows).with_appdata("C:/Users/me/AppData/Roaming");
        let path = locations.global_dir().unwrap();

        assert!(
            !path.to_string_lossy().contains('/'),
            "mixed separators in {}",
            path.display()
        );
        assert_eq!(global(&locations), "C:/Users/me/AppData/Roaming/sloop");
    }

    #[test]
    fn a_missing_home_is_a_sentence_and_not_a_panic() {
        for platform in [Platform::Linux, Platform::MacOs] {
            let failure = Locations::new(platform)
                .global_dir()
                .expect_err("no HOME, no store");
            assert_eq!(failure.exit().code(), 2);
        }
        let failure = Locations::new(Platform::Windows)
            .global_dir()
            .expect_err("no APPDATA, no store");
        assert_eq!(failure.exit().code(), 2);
    }
}
