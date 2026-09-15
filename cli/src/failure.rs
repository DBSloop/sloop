//! A failure worth showing the user, carrying the exit code it leaves with.
//!
//! Every error in this tool has to answer "and what does the scheduler see?", so the exit
//! code is part of the error rather than something chosen at the top of `main` by whoever
//! remembers to. The codes themselves are frozen in [`crate::exit`].

use crate::exit::Exit;
use crate::style;

/// Something went wrong, said in a sentence, with the code it exits on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    exit: Exit,
    message: String,
    hint: Option<String>,
}

/// What every command returns.
pub type Outcome<T> = Result<T, Failure>;

impl Failure {
    /// A failure with a code chosen deliberately.
    pub fn new(exit: Exit, message: impl Into<String>) -> Self {
        Self {
            exit,
            message: message.into(),
            hint: None,
        }
    }

    /// Bad usage, an unknown name, or a question that could not be asked. Frozen at `2`.
    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(Exit::Usage, message)
    }

    /// The line that tells the user what to do about it. Worth writing every time.
    #[must_use]
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// The code this leaves the process with.
    #[must_use]
    pub const fn exit(&self) -> Exit {
        self.exit
    }

    /// Print it the way clap prints its own errors, so the two never look like they came
    /// from different programs.
    pub fn report(&self) {
        anstream::eprintln!("{} {}", style::error_prefix(), self.message);
        if let Some(hint) = &self.hint {
            anstream::eprintln!("{} {hint}", style::label("  hint:"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Failure;
    use crate::exit::Exit;

    #[test]
    fn a_usage_failure_carries_the_frozen_code() {
        assert_eq!(Failure::usage("nope").exit(), Exit::Usage);
        assert_eq!(Failure::usage("nope").exit().code(), 2);
    }

    #[test]
    fn a_hint_is_optional_and_additive() {
        let bare = Failure::usage("nope");
        let helped = Failure::usage("nope").hint("try this");
        assert_ne!(bare, helped);
        assert_eq!(bare.exit(), helped.exit());
    }
}
