//! Where every printed line goes, and what may follow it there.
//!
//! **One layer, decided once, so a flag does not have to be remembered at two hundred call
//! sites.** `--quiet`, `--json`, `--no-color` and `--log-file` are settings of the *run*
//! rather than arguments to a command, and a command that had to consult them would be a
//! command that forgets to. So every line in this program goes through [`say`] or [`note`],
//! and this module answers the four questions about it: is it printed, is it coloured, is it
//! written to a file, and does it have to be scrubbed first.
//!
//! **`--quiet` silences progress, never failures.** A run that has gone wrong says so on
//! standard error whatever the flags, because a scheduled backup whose failure is quiet is a
//! backup nobody finds out about. What `--quiet` removes is the running commentary.
//!
//! **`--json` replaces the commentary rather than joining it.** A command under `--json`
//! prints exactly one document on standard output and nothing else — otherwise the first
//! human line makes the output unparseable, which is the whole point of asking for JSON.
//!
//! **`--log-file` gets the same lines, without the colour and without the secrets.** Rule 3:
//! nothing is written to a log that could not be pasted into a public issue. Two things make
//! that true rather than hoped for — the one line that carries a real password goes through
//! [`secret`], which never logs, and everything else is scrubbed by [`redact`] on the way
//! past.

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

pub mod redact;

use std::io::Write as _;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};

/// What the global flags said, once the command line has been read.
///
/// Four booleans, and they are four independent answers a user gave rather than a state
/// machine with sixteen states — which is the thing clippy's lint is about. They are read
/// once, here, and never passed anywhere by position.
#[derive(Debug, Clone, Copy, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct Asked<'a> {
    /// `--quiet`: no running commentary.
    pub quiet: bool,
    /// `--json`: one document, and nothing else.
    pub json: bool,
    /// `--no-color`: never decorate, whatever the terminal says.
    pub no_color: bool,
    /// `--log-file`: append every line here as well.
    pub log_file: Option<&'a Path>,
    /// `--dry-run`: read everything, check everything, write nothing.
    pub dry_run: bool,
}

/// The settings this run is actually using.
struct Live {
    quiet: bool,
    json: bool,
    dry_run: bool,
    /// Held open for the whole run rather than reopened per line: a log that is appended to
    /// a thousand times should be a thousand writes, not a thousand opens.
    log: Option<Mutex<std::fs::File>>,
}

/// Decided once, in `main`, before a command runs.
static LIVE: OnceLock<Live> = OnceLock::new();

/// Read the global flags and settle how this run talks.
///
/// **Colour is turned off here rather than checked at every call.** `anstream` already
/// strips escapes when standard output is not a terminal and when `NO_COLOR` is set; what
/// `--no-color` adds is a way to say so on the command line, and writing the choice globally
/// means the same answer reaches clap's own help rendering.
///
/// Opening the log is the one thing that can fail, and it fails now — before a command has
/// done half its work and discovered it cannot say so.
pub fn settle(asked: &Asked<'_>) -> Outcome<()> {
    if asked.no_color {
        anstream::ColorChoice::Never.write_global();
    }

    let log = match asked.log_file {
        None => None,
        Some(path) => {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|error| {
                    Failure::new(
                        Exit::Usage,
                        format!("could not open {} to log to: {error}", path.display()),
                    )
                    .hint("--log-file needs a path in a directory that already exists")
                })?;
            Some(Mutex::new(file))
        }
    };

    // A second call is a bug rather than a state to handle: `main` settles this once.
    let _ = LIVE.set(Live {
        quiet: asked.quiet,
        json: asked.json,
        dry_run: asked.dry_run,
        log,
    });
    Ok(())
}

/// Is this run printing a JSON document instead of talking?
#[must_use]
pub fn is_json() -> bool {
    LIVE.get().is_some_and(|live| live.json)
}

/// Is this run keeping quiet?
#[must_use]
pub fn is_quiet() -> bool {
    LIVE.get().is_some_and(|live| live.quiet)
}

/// A line of ordinary output, on standard output.
pub fn say(line: &str) {
    log(line);
    if talking() {
        anstream::println!("{line}");
    }
}

/// A line of ordinary output with no newline after it, for a prompt.
///
/// **Never logged**, because half a line in a log file is noise, and the other half is
/// whatever somebody typed at it.
pub fn ask(line: &str) {
    if talking() {
        anstream::print!("{line}");
        let _ = std::io::stdout().flush();
    }
}

/// A line of commentary, on standard error.
///
/// Standard error rather than standard output so that a command whose output is being piped
/// somewhere does not have its notes land in the pipe.
pub fn note(line: &str) {
    log(line);
    if talking() {
        anstream::eprintln!("{line}");
    }
}

/// Something the user has to know happened, on standard error, whatever was asked for.
///
/// **Never silenced, and it is not commentary.** [`note`] is what a run says about the work
/// it is doing, and `--quiet` and `--json` are how somebody turns that off. This is for the
/// handful of things that happen *once* and move the user's data — today, the global store
/// being taken over from the path an older sloop kept it at. A scheduled `--json` run is
/// exactly the run that would relocate it, and a relocation nobody was told about is the
/// same as one that did not happen.
///
/// Standard error, so a `--json` consumer's stdout is still nothing but the document.
pub fn notice(line: &str) {
    log(line);
    anstream::eprintln!("{line}");
}

/// Something that went wrong, on standard error.
///
/// **Never silenced.** `--quiet` removes the commentary of a run that is working; a run that
/// is not working says so whatever was asked for, because the alternative is a scheduled job
/// that fails silently.
pub fn problem(line: &str) {
    log(line);
    anstream::eprintln!("{line}");
}

/// A line carrying a real secret, on standard error, and **never written to a log**.
///
/// One caller: the generated password `db create` prints once. Rule 3 says no plaintext
/// password anywhere, and a log file is an anywhere — so this path exists precisely to have
/// no logging in it, rather than to be scrubbed by [`redact`] and hoped about.
pub fn secret(line: &str) {
    if !is_quiet() {
        anstream::eprintln!("{line}");
    }
}

/// The one document a `--json` run prints, on standard output.
///
/// Printed with `println!` rather than through `anstream`: there is nothing to decorate, and
/// JSON that had been near a colour decision is JSON somebody has to check.
pub fn document(value: &serde_json::Value) {
    let rendered = serde_json::to_string_pretty(value)
        .unwrap_or_else(|_| "{\"error\":\"could not render this result\"}".to_owned());
    println!("{rendered}");
    log(&rendered);
}

/// **Stop here if this is a rehearsal**, having said what the next step would have been.
///
/// `--dry-run` reads everything and checks everything and then writes nothing, so every
/// command that writes has exactly one of these — placed at the moment it stops reading and
/// starts changing something. Placed any earlier the rehearsal would skip the checks that
/// make it worth having; placed any later it would not be a rehearsal.
///
/// Returns `true` when the caller must return without writing.
#[must_use]
pub fn would(what: &str) -> bool {
    if !LIVE.get().is_some_and(|live| live.dry_run) {
        return false;
    }

    result(serde_json::json!({ "dry_run": true, "would": what }));
    say(&format!(
        "{} {}",
        crate::style::paint("dry run"),
        crate::style::dim(&format!("would {what} — nothing was changed"))
    ));
    true
}

/// What this command did, for the one document a `--json` run prints.
///
/// **Recorded rather than returned**, because the alternative is every command's signature
/// carrying a `serde_json::Value` it does not otherwise care about. A command calls this once,
/// when it knows what it did; `finish` below puts the envelope round it.
static RESULT: Mutex<Option<serde_json::Value>> = Mutex::new(None);

/// Record what this command did. Called once, whatever the flags — building a small JSON
/// value costs nothing and a command that only did it under `--json` is a command whose JSON
/// is only exercised under `--json`.
pub fn result(value: serde_json::Value) {
    if let Ok(mut held) = RESULT.lock() {
        *held = Some(value);
    }
}

/// Print the document, if this run was asked for one.
///
/// **One envelope for every command**, so that a script can read `ok` and `exit` without
/// knowing which command it ran. What the command recorded goes under `result`; a command
/// that recorded nothing still produces a document that parses, which is `R16`'s *Done when*.
pub fn finish(command: &str, exit: Exit) {
    if !is_json() {
        return;
    }

    let result = RESULT.lock().ok().and_then(|mut held| held.take());
    document(&serde_json::json!({
        "ok": exit == Exit::Success,
        "exit": exit.code(),
        "command": command,
        "result": result.unwrap_or(serde_json::Value::Null),
    }));
}

/// Is there anybody to talk to, or has this run been asked for silence?
fn talking() -> bool {
    LIVE.get().is_none_or(|live| !live.quiet && !live.json)
}

/// Append a line to the log, stripped of colour and scrubbed of anything that looks like a
/// credential.
///
/// Best effort on purpose: a log that cannot be written to is not a reason to fail a backup
/// that is working. It is said once, on the first failure, and then the run carries on.
fn log(line: &str) {
    let Some(Live {
        log: Some(file), ..
    }) = LIVE.get()
    else {
        return;
    };

    let plain = anstream::adapter::strip_str(line).to_string();
    let safe = redact::secrets(&plain);

    if let Ok(mut file) = file.lock() {
        let _ = writeln!(file, "{safe}");
    }
}

/// Print a line, choosing the stream the way the old call did.
///
/// The macros below are what commands use; this is what they expand to.
pub fn to(stream: Stream, line: &str) {
    match stream {
        Stream::Out => say(line),
        Stream::Err => note(line),
    }
}

/// Which stream a line belongs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    /// Standard output: what the command was asked to produce.
    Out,
    /// Standard error: what it wants to say about producing it.
    Err,
}

/// A line on standard output. Silenced by `--quiet` and by `--json`.
#[macro_export]
macro_rules! say {
    () => {
        $crate::report::to($crate::report::Stream::Out, "")
    };
    ($($arg:tt)*) => {
        $crate::report::to($crate::report::Stream::Out, &format!($($arg)*))
    };
}

/// A line on standard error. Silenced by `--quiet` and by `--json`.
#[macro_export]
macro_rules! note {
    () => {
        $crate::report::to($crate::report::Stream::Err, "")
    };
    ($($arg:tt)*) => {
        $crate::report::to($crate::report::Stream::Err, &format!($($arg)*))
    };
}
