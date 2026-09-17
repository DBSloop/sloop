//! sloop, being its own askpass helper.
//!
//! **How a passphrase reaches `ssh` without touching argv or a file.** sloop runs `ssh` with
//! `SSH_ASKPASS` pointing at its own binary and `SSH_ASKPASS_REQUIRE=force`, and writes the
//! secret into `ssh`'s standard input. `ssh` spawns the helper with its own stdin inherited,
//! so the helper reads the secret from there and prints it. It is in memory, in one process,
//! for one connection.
//!
//! An environment variable would be readable from `/proc/<pid>/environ` by the same user, and
//! argv by anybody at all. A pipe is neither. **The variables sloop does set carry no secret**
//! — `SSH_ASKPASS` is a path to this binary and [`MODE`] is the word `1`.
//!
//! **This was tested against a real `ssh` before it was written, and the test found the thing
//! that matters.** With `SSH_ASKPASS_REQUIRE=force`, OpenSSH routes *every* prompt to the
//! helper — including the host key question:
//!
//! ```text
//! askpass called with: Enter passphrase for key 'key_pass':
//! askpass called with: The authenticity of host '[…]:2222' can't be established.
//!                      ED25519 key fingerprint is: SHA256:DVeB7mWFtZ5e…
//!                      Are you sure you want to continue connecting (yes/no/[fingerprint])?
//! ```
//!
//! **A helper that answered the second one would silently trust an unknown host.** That is
//! the one thing in this area that is catastrophic to get subtly wrong, and it is why
//! [`is_asking_for_a_secret`] exists and why it is written as a list of things that must
//! *not* be answered as well as a list of things that may be. A prompt this does not
//! recognise gets no answer and a non-zero exit, so `ssh` fails with *"Host key verification
//! failed"* — which is the correct outcome and what a person would get without sloop.
//!
//! **Rule 0d does not apply here.** Host key verification is the operator's security, and
//! sloop leaves it alone rather than stepping around it to make a message go away.

use std::io::{BufRead as _, Write as _};

use crate::exit::Exit;

/// The variable that says "you are being run as an askpass helper, not as sloop".
///
/// **Not a subcommand**, because the top-level command list is a frozen, tested surface and
/// this is not a command anybody runs — it is `ssh` calling back into the same binary. An
/// environment variable is checked before `clap` ever parses, so `sloop --help` is unchanged
/// and the agreed command list stays the agreed command list.
pub const MODE: &str = "SLOOP_ASKPASS";

/// Is this process being run as the helper rather than as sloop?
#[must_use]
pub fn is_the_helper() -> bool {
    std::env::var_os(MODE).is_some_and(|value| value == "1")
}

/// Answer the one prompt this process was spawned for, or refuse it.
///
/// Reads the secret from the standard input `ssh` handed down and prints it. Everything it
/// declines to answer exits non-zero with nothing on standard output, which is what makes
/// `ssh` give up rather than proceed.
#[must_use]
pub fn respond() -> Exit {
    let prompt: String = std::env::args().skip(1).collect::<Vec<_>>().join(" ");

    if !is_asking_for_a_secret(&prompt) {
        // **Deliberately silent about what it was asked.** This runs as a child of `ssh` and
        // whatever it writes to standard output is taken as the answer, so the refusal is the
        // exit code and nothing else. `ssh` prints its own reason, which is the one somebody
        // should read.
        return Exit::Usage;
    }

    let mut secret = String::new();
    if std::io::stdin().lock().read_line(&mut secret).is_err() {
        return Exit::Usage;
    }

    // Nothing was sent, which is a run with no secret configured reaching a prompt it cannot
    // answer. Refusing is right: `ssh` then fails and says what it wanted.
    let secret = secret.trim_end_matches(['\r', '\n']);
    if secret.is_empty() {
        return Exit::Usage;
    }

    let mut out = std::io::stdout().lock();
    if writeln!(out, "{secret}").is_err() || out.flush().is_err() {
        return Exit::Usage;
    }

    Exit::Success
}

/// Is this prompt asking for a passphrase or a password, and nothing else?
///
/// **Written as a refusal first**, because the dangerous case is answering something that was
/// not a request for a secret. The host key question contains the word "fingerprint" and asks
/// for `yes/no`; the follow-up — *"Please type 'yes', 'no' or the fingerprint:"* — contains
/// neither "passphrase" nor "password", so it fails the first test too. Both are real
/// OpenSSH prompts, captured from a real connection.
#[must_use]
pub fn is_asking_for_a_secret(prompt: &str) -> bool {
    let asked = prompt.to_ascii_lowercase();

    // Nothing at all is not a question this should answer. A helper that treated an empty
    // prompt as "give me the passphrase" would hand the secret to whatever asked.
    if asked.trim().is_empty() {
        return false;
    }

    // Anything that smells of host key confirmation, whatever else it also says.
    for refused in [
        "authenticity",
        "yes/no",
        "fingerprint",
        "known_hosts",
        "continue connecting",
    ] {
        if asked.contains(refused) {
            return false;
        }
    }

    asked.contains("passphrase") || asked.contains("password")
}
