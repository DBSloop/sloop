//! One SSH connection, held open, with a local port that comes out at the database.
//!
//! **Opened once and kept.** A menu session that backs up, lists, mirrors and queries is one
//! login, not four — which is the owner's whole reason for the entry. [`Tunnels`] keeps one
//! per remote server for the length of the process and hands the same one back to every
//! command that wants it.
//!
//! **Closed when sloop exits, by the type system rather than by remembering.** A [`Tunnel`]
//! kills its `ssh` on drop, so a command that fails half way through does not leave a
//! forwarded port open on somebody's machine.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::secret::Secret;

use super::{LOOPBACK, Through, askpass};

/// How long `ssh` is given to bind the forward before it is called a failure.
///
/// Generous enough for a login that needs a hardware key touched, bounded because a
/// connection that will not come up has to be a failure rather than a hang.
const OPEN_TIMEOUT: Duration = Duration::from_secs(45);

/// How often it is asked whether the forward is up yet.
const POLL_EVERY: Duration = Duration::from_millis(100);

/// One held connection, and the local port that comes out at the database.
#[derive(Debug)]
pub struct Tunnel {
    ssh: Child,
    local_port: u16,
    describes: String,
}

impl Tunnel {
    /// The port on `127.0.0.1` a client program should connect to.
    #[must_use]
    pub const fn local_port(&self) -> u16 {
        self.local_port
    }

    /// How it reads in a message: the server, never the port it happens to have taken.
    ///
    /// **`R20` prints the `--ssh-*` flags, not this port.** A tunnel's port is different every
    /// run, so a command line naming it would not reproduce anything.
    #[must_use]
    pub fn describe(&self) -> &str {
        &self.describes
    }

    /// Open one: pick a local port, spawn `ssh`, and wait until the forward is really bound.
    ///
    /// `helper` is the program `ssh` is pointed at for `SSH_ASKPASS`, which in a running sloop
    /// is sloop. It is a parameter rather than `current_exe()` read in here for the reason
    /// `catalogue::Reader` is one: inside a test binary `current_exe()` is the test harness,
    /// and a tunnel that could only be opened by the real binary could only be tested by
    /// shipping it first.
    pub fn open(
        through: &Through,
        database_host: &str,
        database_port: u16,
        secret: Option<&Secret>,
        helper: &std::path::Path,
    ) -> Outcome<Self> {
        let Some(program) = super::program() else {
            return Err(Failure::new(
                Exit::Usage,
                "this database is reached over SSH and this machine has no ssh",
            )
            .hint(
                "install OpenSSH — it ships with Windows 10 and later, and with every Linux \
                 and macOS — or reach that database directly",
            ));
        };

        let local_port = a_free_port()?;
        let arguments = through.arguments(local_port, database_host, database_port);

        let mut command = Command::new(&program);
        command
            .args(&arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        if secret.is_some() {
            // **`SSH_ASKPASS_REQUIRE=force`** is what makes `ssh` use the helper with no X
            // display and no terminal — which is exactly what a scheduled run has.
            command
                .env(askpass::MODE, "1")
                .env("SSH_ASKPASS", helper)
                .env("SSH_ASKPASS_REQUIRE", "force");
        } else {
            // No secret to send, so no helper — and `ssh` inherits neither from whatever the
            // user's environment happens to hold.
            command
                .env_remove(askpass::MODE)
                .env_remove("SSH_ASKPASS")
                .env_remove("SSH_ASKPASS_REQUIRE");
        }

        let mut ssh = command.spawn().map_err(|error| {
            Failure::usage(format!("could not run {}: {error}", program.display()))
        })?;

        // **Written and then closed, whether or not there is a secret.** The helper reads one
        // line from this pipe; leaving it open would make a helper that runs anyway — because
        // `ssh` asked something unexpected — wait for a line that never comes, and take the
        // whole connection's timeout with it.
        if let (Some(mut pipe), Some(secret)) = (ssh.stdin.take(), secret) {
            let _ = writeln!(pipe, "{}", secret.expose());
            let _ = pipe.flush();
        }

        let tunnel = Self {
            ssh,
            local_port,
            describes: through.server.describe(),
        };

        tunnel.wait_until_bound(through)
    }

    /// Wait for the forward to be bound, or say why it was not.
    ///
    /// **Two ways out and both are answers.** `ExitOnForwardFailure=yes` means `ssh` exits if
    /// it cannot bind, so a child that has gone is a failure with a reason; and a local port
    /// that can no longer be bound by this process is a forward that is up. Polling one
    /// without the other would hang on a login that was refused.
    fn wait_until_bound(mut self, through: &Through) -> Outcome<Self> {
        let deadline = Instant::now() + OPEN_TIMEOUT;

        loop {
            match self.ssh.try_wait() {
                Ok(Some(_)) => return Err(self.why_it_failed(through)),
                Err(error) => {
                    return Err(Failure::usage(format!(
                        "ssh could not be waited on: {error}"
                    )));
                }
                Ok(None) => {}
            }

            if !is_bindable(self.local_port) {
                return Ok(self);
            }

            if Instant::now() >= deadline {
                return Err(Failure::new(
                    Exit::Connect,
                    format!(
                        "{} did not open a forward within {} seconds",
                        through.server.describe(),
                        OPEN_TIMEOUT.as_secs()
                    ),
                )
                .hint(
                    "an agent that is not running, or a key that needs a passphrase sloop has \
                     not been given — `sloop doctor` says which",
                ));
            }

            std::thread::sleep(POLL_EVERY);
        }
    }

    /// What `ssh` said on the way out, as a failure.
    fn why_it_failed(&mut self, through: &Through) -> Failure {
        let mut said = String::new();
        if let Some(mut stderr) = self.ssh.stderr.take() {
            use std::io::Read as _;
            let _ = stderr.read_to_string(&mut said);
        }

        // **`ssh`'s own words, not a paraphrase.** A wrong host key, a refused key, a server
        // that is not there and a `ProxyJump` that failed all read differently, and the person
        // reading this needs the difference.
        Failure::new(
            Exit::Connect,
            format!("ssh could not reach {}", through.server.describe()),
        )
        .hint(if said.trim().is_empty() {
            "ssh exited without saying why".to_owned()
        } else {
            said.trim().to_owned()
        })
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.ssh.kill();
        let _ = self.ssh.wait();
    }
}

/// Every connection this session holds, one per server.
#[derive(Debug)]
pub struct Tunnels {
    held: BTreeMap<String, Tunnel>,
    /// What `ssh` is pointed at when it needs a passphrase. See [`Tunnel::open`].
    helper: std::path::PathBuf,
}

impl Tunnels {
    /// The set this process holds, with sloop itself as the askpass helper.
    pub fn new() -> Outcome<Self> {
        Ok(Self::with_helper(std::env::current_exe().map_err(
            |error| Failure::usage(format!("sloop cannot find its own binary: {error}")),
        )?))
    }

    /// The same, pointed at a particular helper — which is how the tests drive the real
    /// thing from inside a test binary.
    #[must_use]
    pub fn with_helper(helper: std::path::PathBuf) -> Self {
        Self {
            held: BTreeMap::new(),
            helper,
        }
    }
    /// The forward for this database, opening one if this session has not already.
    ///
    /// Keyed by [`super::Server::credential_key`], so two databases on one server share a
    /// login and two entries naming different keys do not.
    pub fn to(
        &mut self,
        through: &Through,
        database_host: &str,
        database_port: u16,
        secret: Option<&Secret>,
    ) -> Outcome<&Tunnel> {
        let key = through.server.credential_key();

        if !self.held.contains_key(&key) {
            let tunnel = Tunnel::open(through, database_host, database_port, secret, &self.helper)?;
            self.held.insert(key.clone(), tunnel);
        }

        self.held
            .get(&key)
            .ok_or_else(|| Failure::usage("the tunnel that was just opened is not there"))
    }

    /// How many are open, for the report and for the tests.
    #[must_use]
    pub fn count(&self) -> usize {
        self.held.len()
    }
}

/// A port on loopback that is free right now.
///
/// **Port 0, so the kernel picks.** This is not [`crate::install::port::choose`] and should
/// not be: a server somebody installed needs a *recognisable, stable* port they will type,
/// and a tunnel's port is invisible, throwaway, and different every run. Asking for an
/// ephemeral one is the right answer here and the wrong one there.
///
/// The listener is dropped before `ssh` is told to take it, which is a race in principle —
/// which is why `ExitOnForwardFailure=yes` is set and why the caller treats a child that
/// exited as a failure with a reason rather than retrying blindly.
fn a_free_port() -> Outcome<u16> {
    let taken = TcpListener::bind((LOOPBACK, 0))
        .map_err(|error| Failure::usage(format!("could not ask for a local port: {error}")))?;
    let port = taken
        .local_addr()
        .map_err(|error| Failure::usage(format!("that local port has no address: {error}")))?
        .port();
    drop(taken);
    Ok(port)
}

/// Could this process bind that port? A forward that is up means no.
pub(crate) fn is_bindable(port: u16) -> bool {
    TcpListener::bind((LOOPBACK, port)).is_ok()
}
