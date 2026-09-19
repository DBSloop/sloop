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

    /// An invariant that fired, which means there is nothing the user did and nothing they
    /// can change.
    ///
    /// **Still a hint, because *report it* is a fix.** These are the refusals rule 8 asks for
    /// in place of an `unwrap` — *there is always a global store*, *the sealed vault came back
    /// as something not hex* — and the alternative to this line is somebody reading one of
    /// those sentences with nowhere to go. The repository comes from the manifest, so it
    /// cannot drift from the one on crates.io.
    #[must_use]
    pub fn report_a_bug(self) -> Self {
        self.hint(concat!(
            "this is not something you did — it should not be able to happen. Please report ",
            "it at ",
            env!("CARGO_PKG_REPOSITORY"),
            "/issues"
        ))
    }

    /// The same failure, reported as a different kind of failure.
    ///
    /// **For a statement that means one thing in one command and another elsewhere.** A
    /// `DROP SCHEMA` that will not run is a connection problem when `doctor` asks about it
    /// and a restore failure when `restore` is clearing a destination with it — same
    /// sentence, different code, and the code is what a scheduler acts on. The message and
    /// the hint are kept, because what went wrong has not changed.
    #[must_use]
    pub fn at(mut self, exit: Exit) -> Self {
        self.exit = exit;
        self
    }

    /// The sentence itself.
    ///
    /// Read by the tests today and by R16's `--json`, which has to put the message and
    /// the hint in fields of their own rather than in the middle of a printed line.
    #[allow(dead_code)]
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The same failure with something in front of it — a file name, a database name.
    ///
    /// The hint and the exit code come along. Rebuilding the failure instead loses the
    /// hint, which is the half that tells the user what to do.
    #[must_use]
    pub fn prefixed(mut self, prefix: impl std::fmt::Display) -> Self {
        self.message = format!("{prefix}: {}", self.message);
        self
    }

    /// The line that says what to do about it, when there is one. See `message`.
    #[allow(dead_code)]
    ///
    /// Named apart from the builder above, which took the good name first and deserves
    /// it: `.hint("...")` at thirty call sites reads better than `.with_hint("...")`.
    #[must_use]
    pub fn hint_text(&self) -> Option<&str> {
        self.hint.as_deref()
    }

    /// The code this leaves the process with.
    #[must_use]
    pub const fn exit(&self) -> Exit {
        self.exit
    }

    /// Print it the way clap prints its own errors, so the two never look like they came
    /// from different programs.
    /// Say this went wrong without ending the run, and without claiming to be its result.
    ///
    /// **For the commands that carry on.** `backup --all`, `db test` and `doctor` each work
    /// through a list and report the ones that failed as they go — and under `--json` every
    /// one of those calling [`Failure::report`] would print a document of its own, which is
    /// several documents on one standard output and therefore not JSON at all. There is one
    /// document per run, and it is the envelope `main` prints at the end.
    pub fn mention(&self) {
        crate::report::problem(&format!("{} {}", style::error_prefix(), self.message));
        if let Some(hint) = &self.hint {
            crate::report::problem(&format!("{} {hint}", style::label("  hint:")));
        }
    }

    pub fn report(&self) {
        // **Through `problem`, which `--quiet` cannot silence.** Everything else this tool
        // prints is commentary somebody may not want; this is the sentence that says the run
        // did not do what it was asked, and a scheduled job that swallows it is worse than
        // one that says too much.
        //
        // Under `--json` the same failure is a document instead — see `report::failed`.
        if crate::report::is_json() {
            crate::report::document(&serde_json::json!({
                "ok": false,
                "exit": self.exit.code(),
                "error": self.message,
                "hint": self.hint,
            }));
            return;
        }

        crate::report::problem(&format!("{} {}", style::error_prefix(), self.message));
        if let Some(hint) = &self.hint {
            crate::report::problem(&format!("{} {hint}", style::label("  hint:")));
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

    /// **The link is the whole of the hint, so it has to be a link.** It comes from the
    /// manifest rather than a string here, which is what keeps it the same address crates.io
    /// and the README show.
    #[test]
    fn a_bug_report_names_where_to_report_it() {
        let failure = Failure::usage("there is always a global store").report_a_bug();
        let hint = failure.hint_text().expect("a reported bug carries a hint");

        assert!(hint.contains(env!("CARGO_PKG_REPOSITORY")), "{hint}");
        assert!(hint.ends_with("/issues"), "{hint}");
        assert!(
            hint.contains("not something you did"),
            "an invariant is not the user's mistake, and the line should say so: {hint}"
        );
    }

    #[test]
    fn a_hint_is_optional_and_additive() {
        let bare = Failure::usage("nope");
        let helped = Failure::usage("nope").hint("try this");
        assert_ne!(bare, helped);
        assert_eq!(bare.exit(), helped.exit());
    }
}

/// **The sweep that keeps `R28` true after `R28`.**
///
/// A plain-language pass done once is a plain-language pass that rots on the next task, so
/// the rule lives here as a test over the source rather than in anybody's memory — rule 14.
/// It reads the crate's own `.rs` files and holds every `Failure` in them to one sentence:
///
/// > **A failure sloop explains in its own words says what to do about it.**
///
/// The exemption is narrow and it is the only one: a message ending in `: {error}` is
/// quoting somebody else — the operating system, a client tool, a server — and sloop has
/// nothing to add to *Access is denied.* Those are the ones `map_err` hands an error to,
/// and they already read as `could not create …: Access is denied.`
///
/// Everything else is sloop's own sentence. An invariant a user should never be able to
/// reach is not exempt either: if one ever fires, *report it* is the fix, and somebody
/// staring at *the sealed vault came back as something not hex* with nowhere to go is
/// exactly the failure this project says it does not ship.
#[cfg(test)]
mod plain_language {
    use std::path::{Path, PathBuf};

    /// One `Failure::…(…)` expression, found in the source.
    struct Built {
        file: String,
        line: usize,
        /// The whole expression: constructor through the last chained call.
        expression: String,
    }

    /// Every `.rs` file this crate compiles, test modules of their own excluded.
    fn sources() -> Vec<PathBuf> {
        let mut found = Vec::new();
        walk(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut found,
        );
        found.sort();
        found
    }

    fn walk(dir: &Path, into: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, into);
                continue;
            }
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            // A test constructs whatever failure it is checking the handling of, and holding
            // a fixture to the standard of a sentence somebody reads is noise.
            if name.ends_with(".rs") && !name.ends_with("tests.rs") {
                into.push(path);
            }
        }
    }

    /// The source with its inline `#[cfg(test)] mod … { … }` blocks blanked out.
    ///
    /// Blanked rather than removed, and newlines kept, so the line numbers reported below
    /// still point at the file on disk.
    fn without_tests(source: &str) -> String {
        let mut kept = source.to_owned();
        let mut from = 0;
        while let Some(offset) = kept[from..].find("#[cfg(test)]\nmod ") {
            let at = from + offset;
            let Some(open) = kept[at..].find('{').map(|by| at + by) else {
                break;
            };
            // rustfmt closes a module at column zero, so the first `\n}` after it is the end.
            let Some(close) = kept[open..].find("\n}").map(|by| open + by + 2) else {
                break;
            };
            // Space per *byte*, not per char: a `…` in a doc comment is three of them, and
            // shortening the replacement would slide every offset after it.
            let blanked: String = kept[at..close]
                .chars()
                .map(|c| {
                    if c == '\n' {
                        "\n".to_owned()
                    } else {
                        " ".repeat(c.len_utf8())
                    }
                })
                .collect();
            kept.replace_range(at..close, &blanked);
            from = close;
        }
        kept
    }

    /// Every `Failure::new(…)` and `Failure::usage(…)` in one file, chained calls included.
    fn built_in(file: &str, source: &str) -> Vec<Built> {
        let chars: Vec<char> = source.chars().collect();
        let mut found = Vec::new();
        let mut at = 0;
        while at < chars.len() {
            let ahead: String = chars[at..].iter().take(15).collect();
            let Some(paren) = ["Failure::new(", "Failure::usage("]
                .iter()
                .find(|form| ahead.starts_with(*form))
                .map(|form| at + form.len() - 1)
            else {
                at += 1;
                continue;
            };
            let end = expression_ending_at(&chars, paren);
            found.push(Built {
                file: file.to_owned(),
                line: chars[..at].iter().filter(|c| **c == '\n').count() + 1,
                expression: chars[at..end].iter().collect(),
            });
            at = end;
        }
        found
    }

    /// Where the expression that opens at `open` finishes, chained `.method(…)` calls and all.
    fn expression_ending_at(chars: &[char], open: usize) -> usize {
        let mut depth = 0usize;
        let mut in_string = false;
        let mut escaped = false;
        let mut at = open;
        while at < chars.len() {
            let c = chars[at];
            if in_string {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    in_string = false;
                }
            } else if c == '"' {
                in_string = true;
            } else if c == '(' {
                depth += 1;
            } else if c == ')' {
                depth -= 1;
                if depth == 0 {
                    // A chained call continues the same expression; anything else ends it.
                    // Comments are skipped along with the whitespace, because this codebase
                    // explains a hint above the line that adds it more often than not.
                    let mut next = at + 1;
                    loop {
                        while next < chars.len() && chars[next].is_whitespace() {
                            next += 1;
                        }
                        if chars.get(next) == Some(&'/') && chars.get(next + 1) == Some(&'/') {
                            while next < chars.len() && chars[next] != '\n' {
                                next += 1;
                            }
                            continue;
                        }
                        break;
                    }
                    if chars.get(next) == Some(&'.') && chars.get(next + 1) != Some(&'.') {
                        at = next;
                        continue;
                    }
                    return at + 1;
                }
            }
            at += 1;
        }
        chars.len()
    }

    /// Whether the sentence ends in an error somebody else wrote.
    fn quotes_someone_else(flattened: &str) -> bool {
        flattened.contains(": {error}\"")
    }

    /// Whether a failure built into a binding is hinted a few lines further down.
    ///
    /// **Three of them are built and then hinted according to what went wrong** — the tool's
    /// stderr, the service manager's refusal — which reads better at those sites than a
    /// `match` returning two whole failures. The hint is still there; it is just not in the
    /// same expression.
    fn helped_later(source: &str, built: &Built) -> bool {
        let Some(name) = source
            .lines()
            .nth(built.line - 1)
            .and_then(|line| line.trim().strip_prefix("let "))
            .map(|rest| rest.trim_start_matches("mut "))
            .and_then(|rest| rest.split([' ', ':', '=']).next())
            .filter(|name| !name.is_empty())
        else {
            return false;
        };
        let hinted = format!("{name}.hint(");
        source
            .lines()
            .skip(built.line)
            .take(20)
            .any(|line| line.contains(&hinted))
    }

    /// One line of the expression, with the whitespace rustfmt spread it over taken out.
    fn flatten(expression: &str) -> String {
        expression.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn every_failure_in_sloops_own_words_names_the_fix() {
        let mut unhelped = Vec::new();
        for path in sources() {
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            let shown = path
                .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                .unwrap_or(&path)
                .display()
                .to_string()
                .replace('\\', "/");
            let stripped = without_tests(&source);
            for built in built_in(&shown, &stripped) {
                let flattened = flatten(&built.expression);
                if flattened.contains(".hint(")
                    || flattened.contains(".report_a_bug()")
                    || quotes_someone_else(&flattened)
                    || helped_later(&stripped, &built)
                {
                    continue;
                }
                let shortened: String = flattened.chars().take(110).collect();
                unhelped.push(format!("{}:{}\n      {shortened}", built.file, built.line));
            }
        }

        assert!(
            unhelped.is_empty(),
            "{} failure{} what went wrong without saying what to do about it.\n\
             Give each one a `.hint(…)` naming the flag, the command or the step that fixes \
             it — or, for an invariant nobody should be able to reach, the hint that says so \
             and where to report it.\n\n  {}\n",
            unhelped.len(),
            if unhelped.len() == 1 {
                " says"
            } else {
                "s say"
            },
            unhelped.join("\n  ")
        );
    }
}
