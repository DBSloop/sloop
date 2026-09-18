//! sloop running in the background, with nobody logged in.
//!
//! **`R24`, and the shape is deliberately thin.** What a service *does* is `R26`'s and
//! `R27a`'s; what is here is the part every one of those needs first and none of them can
//! build on its own — getting sloop registered with whatever this machine uses to start
//! things at boot, started, stopped, asked about, and taken off again leaving nothing.
//!
//! **`R25` gave it something to watch.** [`watch`] is the list of databases the daemon reads
//! on every round — a table, so attaching reaches a service that is already running without
//! anything being restarted.
//!
//! ```text
//! Linux      a systemd unit in /etc/systemd/system
//! macOS      a launchd daemon in /Library/LaunchDaemons
//! Windows    a real service, registered with the Service Control Manager
//! ```
//!
//! **"A real Windows service" is the phrase that decides the whole design.** A program the
//! Service Control Manager starts has about thirty seconds to call back and say it is
//! running, and one that does not is killed — so a service cannot be an ordinary binary that
//! merely happens to be launched by `sc.exe`. It has to speak the SCM's protocol, answer
//! stop, and report its state. That is what `windows-service` is in the dependency list for,
//! and it is why `service run` exists as a hidden command rather than the daemon being a
//! flag on something else.
//!
//! **It never opens a port.** Not for a status endpoint, not for metrics, not for a local
//! socket that "is only bound to loopback". `ci/no-http-client.sh` would fail the build, and
//! the guarantee at the top of the README forbids it before that check gets a chance to.
//! Everything the daemon records goes into sloop's own PostgreSQL and is read back by a CLI
//! running on the same machine.

pub mod credentials;
pub mod daemon;
pub mod key;
pub mod manage;
pub mod mechanism;
pub mod sample;
pub mod unit;
pub mod watch;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
mod cluster_tests;
