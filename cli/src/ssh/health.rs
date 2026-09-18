//! What `doctor` can say about reaching a database over SSH, without opening anything.
//!
//! **Four local facts, and not one of them is a connection.** Is there an `ssh` and which
//! one; is an agent running and holding anything; is the key a record names actually
//! readable; and does the record name a key at all. Whether the *server* answers is a
//! different question and a much more expensive one — `doctor` answers that by opening the
//! forward it would open anyway, per database, in `commands::doctor`.
//!
//! **Nothing here can prompt.** `ssh -V` prints a version and exits; `ssh-add -l` lists what
//! an agent holds and exits. Neither reads a terminal, which is what makes them safe to run
//! inside a report that a scheduled `sloop doctor` also produces.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Whether an agent is there, and whether it is any use.
///
/// **Three states, not two, because the middle one is the confusing one.** An agent that is
/// running and holding no keys looks exactly like a working setup right up until a
/// connection asks for a passphrase nobody is there to type — which in a scheduled run is
/// rule 4's failure. It is worth its own line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    /// Running, and holding at least one key.
    Holding(usize),
    /// Running, and holding nothing.
    Empty,
    /// Not running, or not reachable from here.
    Absent,
}

impl Agent {
    /// How it reads in the report.
    #[must_use]
    pub fn describe(self) -> String {
        match self {
            Self::Holding(1) => "running, holding 1 key".to_owned(),
            Self::Holding(keys) => format!("running, holding {keys} keys"),
            Self::Empty => "running, holding no keys".to_owned(),
            Self::Absent => "not running".to_owned(),
        }
    }

    /// Would a key with a passphrase go through without anybody typing it?
    #[must_use]
    pub const fn can_unlock_a_key(self) -> bool {
        matches!(self, Self::Holding(_))
    }
}

/// One private key a registration names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    /// The path, as the record holds it.
    pub path: PathBuf,
    /// Could this process read it? **Read, not parse** — sloop never opens a private key
    /// and never copies one; `ssh` does that. What is checked is the mistake people
    /// actually make, which is a path that has moved or a file this account cannot see.
    pub readable: bool,
}

/// What this machine can do about SSH.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Where `ssh` is, if it is anywhere.
    pub program: Option<PathBuf>,
    /// What it says it is: `OpenSSH_10.2p1, OpenSSL 3.5.4`.
    pub version: Option<String>,
    /// Whether an agent is holding keys for it.
    pub agent: Agent,
    /// Every private key the registry names, in order, with no duplicates.
    pub keys: Vec<Key>,
}

impl Report {
    /// Is there enough here to open a forward at all?
    #[must_use]
    pub const fn usable(&self) -> bool {
        self.program.is_some()
    }
}

/// Look at this machine.
///
/// `named` is every private key path the registry mentions. It is passed in rather than read
/// from a registry here for the reason every other check in `doctor` takes its inputs: this
/// module answers a question about the machine, and which databases exist is not one.
#[must_use]
pub fn look(named: &[PathBuf]) -> Report {
    let program = super::program();

    let mut keys: Vec<Key> = Vec::new();
    for path in named {
        if keys.iter().any(|seen| seen.path == *path) {
            continue;
        }
        keys.push(Key {
            readable: is_readable(path),
            path: path.clone(),
        });
    }

    Report {
        version: program.as_deref().and_then(version_of),
        agent: agent(),
        program,
        keys,
    }
}

/// What `ssh -V` says, on one line.
///
/// **It writes to standard error**, which is not a quirk to work around but the reason this
/// reads both streams: a version check that read only stdout would report every OpenSSH on
/// earth as having no version.
fn version_of(program: &Path) -> Option<String> {
    let said = Command::new(program)
        .arg("-V")
        .stdin(Stdio::null())
        .output()
        .ok()?;

    let text = if said.stderr.is_empty() {
        String::from_utf8_lossy(&said.stdout)
    } else {
        String::from_utf8_lossy(&said.stderr)
    };

    let line = text.lines().next()?.trim().to_owned();
    (!line.is_empty()).then_some(line)
}

/// Ask the agent what it is holding.
///
/// **`ssh-add -l`'s exit codes are the answer and they are documented**: `0` is an agent with
/// keys, `1` is an agent with none, and `2` is no agent to talk to. Reading the code rather
/// than the message keeps this working in a locale sloop has never seen.
fn agent() -> Agent {
    let name = if cfg!(windows) {
        "ssh-add.exe"
    } else {
        "ssh-add"
    };
    let Some(program) = crate::tools::acquire::on_path_at(name) else {
        return Agent::Absent;
    };

    let Ok(said) = Command::new(program)
        .arg("-l")
        .stdin(Stdio::null())
        .output()
    else {
        return Agent::Absent;
    };

    match said.status.code() {
        Some(0) => Agent::Holding(
            String::from_utf8_lossy(&said.stdout)
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count()
                .max(1),
        ),
        Some(1) => Agent::Empty,
        _ => Agent::Absent,
    }
}

/// Can this process read that file?
///
/// Opened rather than stat'd: a key that exists and belongs to another account is the case
/// worth catching, and `exists()` says yes to it.
fn is_readable(path: &Path) -> bool {
    std::fs::File::open(path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::{Agent, look};

    /// **Three states, and the middle one is the whole reason there are three.** An agent
    /// holding nothing looks like a working setup right up until a connection asks for a
    /// passphrase nobody is there to type, which in a scheduled run is rule 4's failure.
    #[test]
    fn an_agent_holding_nothing_is_not_an_agent_that_can_unlock_a_key() {
        assert!(Agent::Holding(1).can_unlock_a_key());
        assert!(Agent::Holding(4).can_unlock_a_key());
        assert!(!Agent::Empty.can_unlock_a_key());
        assert!(!Agent::Absent.can_unlock_a_key());

        // And each says which it is, in words a person reads rather than a state name.
        assert_eq!(Agent::Holding(1).describe(), "running, holding 1 key");
        assert_eq!(Agent::Holding(3).describe(), "running, holding 3 keys");
        assert!(Agent::Empty.describe().contains("no keys"));
        assert!(Agent::Absent.describe().contains("not running"));
    }

    /// A key that is there is readable, one that is not is not, and five databases naming
    /// one key produce one line rather than five.
    #[test]
    fn a_key_is_reported_once_and_by_whether_it_can_actually_be_read() {
        let here = std::env::temp_dir().join(format!(
            "sloop-health-key-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::write(&here, b"not really a key").expect("writing a file");
        let nowhere = here.with_extension("missing");

        let found = look(&[here.clone(), nowhere.clone(), here.clone()]);
        let _ = std::fs::remove_file(&here);

        assert_eq!(
            found.keys.len(),
            2,
            "the same key twice is one key: {found:?}"
        );
        assert_eq!(found.keys[0].path, here);
        assert!(found.keys[0].readable);
        assert_eq!(found.keys[1].path, nowhere);
        assert!(
            !found.keys[1].readable,
            "a key that is not there cannot be read"
        );
    }

    /// Whatever this machine has, the report is about it and says so consistently: an `ssh`
    /// that is there has a version, and one that is not makes the whole thing unusable.
    #[test]
    fn what_it_says_about_ssh_agrees_with_itself() {
        let found = look(&[]);

        assert_eq!(found.usable(), found.program.is_some());
        if found.program.is_some() {
            // Every OpenSSH prints something for `-V`; what matters here is that it was
            // read at all, which is the half that needed standard error.
            assert!(
                found.version.is_some(),
                "there is an ssh and it said nothing: {found:?}"
            );
        } else {
            assert!(found.version.is_none());
        }
    }
}
