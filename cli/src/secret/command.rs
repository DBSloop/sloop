//! The password-command route, for teams whose passwords live in a password manager.
//!
//! `sloop --password-command "op read op://vault/db/password"`. The command is run, its
//! standard output is the password, and nothing else about it is interpreted.
//!
//! The password never appears in `argv`: what is passed to the shell is the command line
//! the user wrote, and the password only ever comes back through a pipe. Anyone running
//! `ps` while this happens sees the name of a password manager, not a password.

use std::process::{Command, Stdio};

use zeroize::Zeroize;

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::secret::Secret;

/// Run `command` and take its output as the password.
pub fn run(command: &str) -> Outcome<Secret> {
    let output = shell(command).output().map_err(|error| {
        Failure::new(
            Exit::Usage,
            format!("could not run the password command: {error}"),
        )
        .hint(
            "the command is run by the system shell, so it has to be something the shell can find",
        )
    })?;

    if !output.status.success() {
        return Err(failed(command, output.status, &output.stderr));
    }

    let mut text = String::from_utf8(output.stdout).map_err(|_| {
        Failure::new(
            Exit::Usage,
            "the password command produced bytes that are not text",
        )
        .hint("sloop expects the password on standard output, as UTF-8")
    })?;

    let secret = Secret::new(trim_one_newline(&text).to_owned());
    text.zeroize();
    Ok(secret)
}

/// The command line, handed to the system shell.
///
/// A shell rather than a bare `exec`, because these commands come with pipes and quoting
/// in them and every password manager writes its documentation assuming one. The string
/// is the user's own, out of their own configuration; there is nothing here to inject
/// into that they did not already write themselves.
fn shell(command: &str) -> Command {
    let mut process = interpreter(command);

    // Standard input stays closed. A password command that stops to ask a question would
    // hang a scheduled run, which is the one failure this tool must not have.
    process.stdin(Stdio::null());
    process
}

/// `cmd.exe`, given the command line verbatim.
///
/// `raw_arg` rather than `arg`, because Rust escapes arguments the way the MSVC C runtime
/// reads them and `cmd.exe` does not read them that way at all: a quoted path arrives
/// mangled and comes back as "the filename, directory name, or volume label syntax is
/// incorrect". `/S` then says to strip exactly the outer pair of quotes and take
/// everything between them literally, which is the one predictable rule cmd has.
#[cfg(windows)]
fn interpreter(command: &str) -> Command {
    use std::os::windows::process::CommandExt as _;

    let mut process = Command::new("cmd");
    process.raw_arg(format!("/S /C \"{command}\""));
    process
}

#[cfg(not(windows))]
fn interpreter(command: &str) -> Command {
    let mut process = Command::new("sh");
    process.arg("-c").arg(command);
    process
}

/// Take off the one line ending a command adds, and nothing more.
///
/// Every password manager prints a trailing newline, so removing exactly one is right.
/// Anything beyond that is left alone and reported by [`Secret::notes`] instead: a
/// password is allowed to end in a space, and stripping one that was deliberate would
/// break a login in a way nobody would ever find.
fn trim_one_newline(text: &str) -> &str {
    text.strip_suffix('\n')
        .map_or(text, |rest| rest.strip_suffix('\r').unwrap_or(rest))
}

fn failed(command: &str, status: std::process::ExitStatus, stderr: &[u8]) -> Failure {
    let code = status
        .code()
        .map_or_else(|| "a signal".to_owned(), |code| code.to_string());

    // The password manager's own complaint, which is the only useful thing to show. It is
    // about the command, not about the password — a command that prints its own secret to
    // standard error is already broken in a way sloop cannot fix.
    let said: String = String::from_utf8_lossy(stderr)
        .lines()
        .take(3)
        .collect::<Vec<_>>()
        .join("; ");

    let failure = Failure::new(
        Exit::Usage,
        format!("the password command exited with {code}: {command}"),
    );

    if said.trim().is_empty() {
        failure.hint("it printed nothing to explain itself; try running it yourself")
    } else {
        failure.hint(said)
    }
}

#[cfg(test)]
mod tests {
    use super::trim_one_newline;

    #[test]
    fn exactly_one_line_ending_comes_off() {
        assert_eq!(trim_one_newline("hunter2"), "hunter2");
        assert_eq!(trim_one_newline("hunter2\n"), "hunter2");
        assert_eq!(trim_one_newline("hunter2\r\n"), "hunter2");
        // Two newlines means the second one was in the password, however odd that is.
        assert_eq!(trim_one_newline("hunter2\n\n"), "hunter2\n");
    }

    #[test]
    fn trailing_spaces_are_left_where_they_are() {
        // A password is allowed to end in a space. Stripping one that was deliberate
        // breaks a login in a way nobody ever finds, so this only reports it.
        assert_eq!(trim_one_newline("hunter2  \n"), "hunter2  ");
        assert_eq!(trim_one_newline("hunter2\t"), "hunter2\t");
    }

    #[test]
    fn an_empty_output_stays_empty() {
        assert_eq!(trim_one_newline(""), "");
        assert_eq!(trim_one_newline("\n"), "");
        assert_eq!(trim_one_newline("\r\n"), "");
    }
}
