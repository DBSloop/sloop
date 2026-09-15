//! `sloop` — register your databases once, then back them up, restore them, mirror them
//! and sync them.
//!
//! This build is the workspace and the command surface. Every command parses, every
//! command documents itself, and every command says plainly that its body has not been
//! written yet rather than pretending to have done something.

mod cli;
mod exit;
mod style;

use std::io::Write as _;
use std::process::ExitCode;

use clap::Parser as _;

use crate::cli::Cli;
use crate::exit::Exit;

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            // `--help` and `--version` arrive here too. clap already knows which of the
            // two streams each belongs on; the exit code is ours, and a usage error is
            // frozen at 2 whatever clap's own default happens to be next year.
            let usage_error = error.use_stderr();
            let _ = error.print();
            return if usage_error {
                Exit::Usage.into()
            } else {
                Exit::Success.into()
            };
        }
    };

    match cli.command {
        Some(command) => unimplemented(&format!("'{}'", command.path())),
        None => unimplemented("the interactive menu"),
    }
}

/// Say the honest thing and exit non-zero.
///
/// A stub that exits 0 is a stub that a script believes. This one does not use any of the
/// frozen codes either — none of them describes "nothing happened", and 1 already means
/// exactly that everywhere else.
fn unimplemented(what: &str) -> ExitCode {
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "sloop: {what} is not implemented yet.");
    let _ = writeln!(
        stderr,
        "       This is an early build. https://github.com/DBSloop/sloop tracks what works."
    );
    Exit::Failure.into()
}
