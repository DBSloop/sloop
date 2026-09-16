//! How a command gets permission — and what each way of giving it may not do.
//!
//! Three flags, and the whole point is that they are **not** interchangeable:
//!
//! | flag | what it does | what it cannot do |
//! |---|---|---|
//! | `-y`, `--yes` | answers a question that would have been asked | destroy a named thing, or override a refusal |
//! | `--force` | overrides a refusal that is there to protect something | answer any question |
//! | `--confirm <NAME>` | is the name, typed | anything, unless it matches exactly |
//!
//! **A bare `-y` is never enough to destroy something.** That is rule 5 holding in a
//! crontab exactly as it holds at a prompt: `sloop db drop app --confirm app_production`
//! names what it is about to destroy, so a cron line cannot be repointed at another
//! database by editing one flag, and a `--yes` copied from somewhere else cannot answer for
//! a name it has never seen. There is no spelling of "yes, whichever database that was".
//!
//! **Rule 4 lives here too.** With nothing to ask at, a command that needs an answer exits
//! `2` naming the flag that would have given it, rather than waiting for somebody who is not
//! there. Every branch below that reads stdin is guarded by that check first.
//!
//! **What `--yes` deliberately cannot answer:** whether a backup key has been copied
//! somewhere safe. `R11` asks for the word `decline`, and a flag that meant "I accept losing
//! every backup on this machine" is not a flag this tool is going to have. See
//! `commands::backup`.
//!
//! This module arrived as `R12b`, pulled out of `R16` on purpose: `R13`, `R14` and `R15` are
//! the destructive commands, and each has to be born automatable rather than retrofitted
//! after somebody has already written a cron line against it.

#[cfg(test)]
#[path = "consent_tests.rs"]
mod tests;

use std::io::IsTerminal as _;

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::style;

/// The permission this run was given, once, from the global flags.
#[derive(Debug, Clone, Copy, Default)]
pub struct Consent<'a> {
    /// `-y`, `--yes`.
    yes: bool,
    /// `--force`.
    force: bool,
    /// `--confirm <NAME>`.
    confirm: Option<&'a str>,
}

/// What came back when permission was asked for.
///
/// **Declining is not a failure.** Somebody who types the wrong name, or changes their mind,
/// has used the tool correctly — so the command says nothing happened and exits `0`. A
/// `--confirm` that names the *wrong* database is the other thing entirely: that is a
/// scheduled run pointed at something it did not mean, and it exits `2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consented {
    /// A flag said so before the run started.
    ByFlag,
    /// Somebody typed it at a terminal.
    AtTheTerminal,
    /// Nobody agreed, and nothing should happen.
    Declined,
}

impl Consented {
    /// Did permission actually arrive?
    #[must_use]
    pub const fn granted(self) -> bool {
        !matches!(self, Self::Declined)
    }
}

impl<'a> Consent<'a> {
    /// What the command line said.
    #[must_use]
    pub const fn given(yes: bool, force: bool, confirm: Option<&'a str>) -> Self {
        Self {
            yes,
            force,
            confirm,
        }
    }

    /// Was a refusal overridden on purpose?
    ///
    /// `--force` and nothing else. A refusal is a thing sloop decided not to do; answering a
    /// question with `--yes` has never been a reason to do it anyway, and a command that
    /// treated the two as one would turn "are you sure?" into a way past every safety check
    /// in the tool.
    #[must_use]
    pub const fn forced(self) -> bool {
        self.force
    }

    /// A yes-or-no question, for the things that do not destroy a database.
    ///
    /// `flag` is what a scheduled run would have passed instead — named in the error,
    /// because rule 4 means a question that cannot be asked has to say how to answer it in
    /// advance.
    pub fn asked(self, question: &str, flag: &str) -> Outcome<bool> {
        if self.yes {
            return Ok(true);
        }

        if !std::io::stdin().is_terminal() {
            return Err(Failure::new(
                Exit::Usage,
                format!("{question} — and there is no terminal to ask at"),
            )
            .hint(format!("pass {flag} to answer it up front")));
        }

        crate::report::ask(&format!("{} {question} ", style::paint("?")));

        let mut answer = String::new();
        std::io::stdin()
            .read_line(&mut answer)
            .map_err(|error| Failure::usage(format!("could not read the answer: {error}")))?;

        Ok(matches!(
            answer.trim().to_ascii_lowercase().as_str(),
            "y" | "yes"
        ))
    }

    /// Permission to destroy something: its name, typed out.
    ///
    /// **`--yes` is not consulted.** Rule 5 says a destructive operation is typed rather
    /// than clicked, and a flag that answered for a name nobody typed would be the click.
    pub fn typed(self, destroying: &Destroying<'_>) -> Outcome<Consented> {
        self.checked_early(destroying)?;

        if self.confirm.is_some() {
            // `checked_early` has already compared it, so a value here is a value that
            // matched.
            return Ok(Consented::ByFlag);
        }

        crate::report::ask(&format!(
            "{} type {} to destroy it, or anything else to stop: ",
            style::paint("?"),
            style::paint(destroying.named)
        ));

        let mut given = String::new();
        std::io::stdin()
            .read_line(&mut given)
            .map_err(|error| Failure::usage(format!("could not read the answer: {error}")))?;

        // Only the line ending comes off. A name with a trailing space is a name somebody
        // would have to type a trailing space for, and trimming would quietly accept a
        // different name than the one on the server.
        if given.trim_end_matches(['\n', '\r']) == destroying.named {
            Ok(Consented::AtTheTerminal)
        } else {
            Ok(Consented::Declined)
        }
    }

    /// The half of [`Consent::typed`] that can be answered before a socket is opened.
    ///
    /// **A typo is caught before anything is contacted.** `--confirm` is a string comparison
    /// and connecting first cannot make it more certain, so a scheduled run that names the
    /// wrong database is told so without sloop touching a server — and a run with no
    /// terminal and no `--confirm` is told the one thing it needs before a password is
    /// fetched for a connection it was only ever going to refuse to use.
    ///
    /// The prompt is deliberately *not* here: somebody typing a name by hand should be
    /// looking at how much is about to go when they type it, which is after the probe.
    pub fn checked_early(self, destroying: &Destroying<'_>) -> Outcome<()> {
        match self.confirm {
            Some(given) if given == destroying.named => Ok(()),
            Some(given) => Err(Failure::new(
                Exit::Usage,
                format!(
                    "--confirm says {given}, and the {} is {}",
                    destroying.noun, destroying.named
                ),
            )
            .hint("nothing was contacted and nothing was changed. The two have to match exactly")),
            None if !std::io::stdin().is_terminal() => Err(Failure::new(
                Exit::Usage,
                format!(
                    "{} needs its name typed, and there is no terminal to type at",
                    destroying.action
                ),
            )
            .hint(format!(
                "pass --confirm {} to say it up front",
                style::as_argument(destroying.named)
            ))),
            None => Ok(()),
        }
    }
}

/// What is about to be destroyed, in the words the messages need.
///
/// Three strings rather than one, because the same permission produces three sentences and
/// each has to read like English: *"--confirm says order, and the database is orders"*,
/// *"dropping a database needs its name typed"*, *"type orders to destroy it"*. A single
/// generic phrase would make one of the three read like a machine, and these are the last
/// words somebody sees before a database stops existing.
pub struct Destroying<'a> {
    /// The name that has to be typed — the database's own name on the server, not the label
    /// sloop files it under. The label is what sloop calls it; this is what stops existing.
    pub named: &'a str,
    /// What kind of thing it is: `database`.
    pub noun: &'a str,
    /// What is about to happen to it: `dropping a database`.
    pub action: &'a str,
}
