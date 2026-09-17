//! Choosing a port that collides with nothing.
//!
//! `R19d` asks for it in those words, and the reason is that the obvious answer is wrong.
//! A machine that already runs PostgreSQL has 5432; installing a second one on 5432 gives
//! either a server that will not start or, worse, one that starts and shadows the other for
//! whichever of them bound first. Guessing an offset — 5433, 5434 — is the same mistake one
//! step along, because the guess is made without looking.
//!
//! **So the operating system is asked.** Binding a port is the only thing that actually
//! answers "is this free", and it is asked of both `127.0.0.1` — where the server is about
//! to listen — and `0.0.0.0`, which catches something already holding that port on another
//! interface.
//!
//! **A bind and a drop, and not one byte sent.** This is the whole of sloop's use of a
//! socket: no connection is opened, nothing is read, nothing is written, and `cargo tree`
//! is unchanged, because a listener is `std::net` and not an HTTP client. Everything that
//! talks to a database still does it through the database's own programs.
//!
//! **It is a race and it is treated as one.** A port that was free a moment ago can be taken
//! by something else before the server binds it, which is why the server starting is what
//! proves the choice and this only narrows it down.

use std::net::TcpListener;

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};

/// How far above the engine's usual port to look before giving up.
///
/// Far enough that a machine running a handful of servers still gets one, near enough that
/// the number stays recognisable: somebody who sees 5434 knows what it is, and somebody who
/// sees 34517 does not.
const HOW_FAR: u16 = 64;

/// The first free port at or above `wanted`, skipping any this machine has already given out.
pub fn choose(wanted: u16, spoken_for: &[u16]) -> Outcome<u16> {
    for port in wanted..=wanted.saturating_add(HOW_FAR) {
        if spoken_for.contains(&port) {
            continue;
        }
        if is_free(port) {
            return Ok(port);
        }
    }

    Err(Failure::new(
        Exit::Usage,
        format!(
            "every port from {wanted} to {} is in use on this machine",
            wanted.saturating_add(HOW_FAR)
        ),
    )
    .hint("stop something, or install that server yourself on a port you choose"))
}

/// Can something bind this port, on loopback and on every interface?
///
/// Both, because either one alone gives a wrong answer. A server bound to one address on
/// this port would leave the other still bindable on some platforms, and a port that looked
/// free would then refuse the moment the real server tried to take it.
#[must_use]
pub fn is_free(port: u16) -> bool {
    ["127.0.0.1", "0.0.0.0"]
        .iter()
        .all(|address| TcpListener::bind((*address, port)).is_ok())
}
