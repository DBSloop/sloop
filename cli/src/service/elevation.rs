//! Whether this run can change what the machine does at boot, asked before it tries.
//!
//! **Because the alternative is finding out halfway.** `service install` writes a key file
//! under `%ProgramData%` or `/etc`, copies credentials into it, and only then asks the
//! service manager to register a unit. Without elevation the first of those fails — after
//! two lines have already been printed saying installation is under way, with an error from
//! the operating system about a path nobody typed. Everything before that point was real:
//! a directory may exist that did not before. A refusal at the door leaves the machine
//! exactly as it was and says what to do about it in one sentence.
//!
//! **Shelled out, like every other question this project asks the operating system.**
//! `systemctl`, `launchctl`, `sc.exe`, `curl` and `ssh` are all reached the same way, and
//! the two programs here are on every machine that could run the commands that call this.
//! Nothing is added to the dependency graph to answer a yes-or-no question.
//!
//! **A machine that will not answer is allowed to carry on**, which is rule 0d: a check
//! that cannot tell must not be the reason a feature refuses. Only a clear *no* refuses.

use std::process::{Command, Stdio};

use crate::exit::Exit;
use crate::failure::{Failure, Outcome};

use super::mechanism::Mechanism;

/// Refuse now if this run certainly cannot register or control a service.
///
/// `Ok(())` covers both *yes* and *cannot tell*; see the module note.
pub fn require(mechanism: Mechanism) -> Outcome<()> {
    if enough() == Some(false) {
        return Err(Failure::new(
            Exit::Failure,
            format!(
                "changing what this machine runs at boot needs more than this account has, \
                 and {} will refuse",
                mechanism.spoken()
            ),
        )
        .hint(advice()));
    }
    Ok(())
}

/// What to do about it, in this machine's own words.
fn advice() -> &'static str {
    if cfg!(windows) {
        "open a terminal with 'Run as administrator' and run it again. Nothing has been \
         changed by this run."
    } else {
        "run it again with sudo. Nothing has been changed by this run."
    }
}

/// Does this run have what it takes? `None` when the machine would not say.
fn enough() -> Option<bool> {
    if cfg!(windows) {
        windows_integrity()
    } else {
        effective_uid_is_root()
    }
}

/// Unix: `id -u`, which is `0` for root and for anything running under `sudo`.
fn effective_uid_is_root() -> Option<bool> {
    let said = said_by(Command::new("id").arg("-u"))?;
    Some(said.trim() == "0")
}

/// Windows: the process's integrity level, out of `whoami /groups`.
///
/// **The SID, not the label.** `whoami` prints *Mandatory Label\\High Mandatory Level* in
/// English and something else in every other install language, while `S-1-16-12288` is the
/// same eleven characters everywhere.
///
/// **The level, not two literals.** Reading it as a number is what lets *no integrity SID in
/// the output at all* mean **cannot tell** rather than **no** — a locked-down `whoami`, a
/// future level, or an output shape nobody here has seen would otherwise refuse an account
/// that is perfectly able to install a service.
///
/// Checked rather than membership of Administrators, because that group is on the token of
/// an un-elevated administrator too — which is exactly the account this refusal is for.
fn windows_integrity() -> Option<bool> {
    let said = said_by(Command::new("whoami").arg("/groups"))?;
    Some(integrity_level(&said)? >= HIGH)
}

/// `S-1-16-12288`, the level a process has once it is elevated. System is 16384, which is
/// what a Windows service itself runs as and is also enough.
const HIGH: u32 = 12288;

/// The highest `S-1-16-<n>` in some text, or `None` where there is not one.
///
/// **The highest, because a token can carry more than one.** What decides what a process may
/// do is the level it is running at, and that is the largest of them.
fn integrity_level(said: &str) -> Option<u32> {
    said.split("S-1-16-")
        .skip(1)
        .filter_map(|rest| {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<u32>().ok()
        })
        .max()
}

/// Run it and hand back its standard output, or `None` if it would not run.
fn said_by(command: &mut Command) -> Option<String> {
    let output = command.stdin(Stdio::null()).stderr(Stdio::null()).output();

    match output {
        Ok(output) if output.status.success() => {
            Some(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        // A machine with no `id`, no `whoami`, or one that answered with a failure, has not
        // said no. See the module note.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Mechanism, advice, enough, require};

    /// **Whatever this machine is, the answer is a real one and the check agrees with it.**
    /// A test that asserted *elevated* or *not elevated* would pass on one developer's
    /// machine and fail on the next; what is worth holding is that asking does not panic,
    /// and that a run allowed to proceed is exactly a run this did not refuse.
    #[test]
    fn asking_the_machine_agrees_with_what_it_answered() {
        let answered = enough();
        let allowed = require(Mechanism::of_this_machine().unwrap_or(Mechanism::Systemd)).is_ok();

        assert_eq!(
            allowed,
            answered != Some(false),
            "the refusal and the answer disagree: {answered:?}"
        );
    }

    /// **The level is read, not matched against two spellings.** Text with no integrity SID
    /// in it has not said the account is unprivileged; it has said nothing, and the refusal
    /// this gates is too blunt to fire on nothing.
    #[test]
    fn an_integrity_level_is_read_out_of_what_whoami_prints() {
        use super::integrity_level;

        let medium = "Mandatory Label\\Medium Mandatory Level  Label  S-1-16-8192";
        let high = "Mandatory Label\\High Mandatory Level    Label  S-1-16-12288";
        let system = "Mandatory Label\\System Mandatory Level  Label  S-1-16-16384";

        assert_eq!(integrity_level(medium), Some(8192));
        assert_eq!(integrity_level(high), Some(12288));
        assert_eq!(integrity_level(system), Some(16384));

        // A token carrying several: the one that decides is the highest.
        assert_eq!(
            integrity_level(&format!("{medium}\n{high}")),
            Some(12288),
            "the highest level on the token is the one in force"
        );

        // Nothing to read is not a no.
        assert_eq!(
            integrity_level("BUILTIN\\Administrators  S-1-5-32-544"),
            None
        );
        assert_eq!(integrity_level(""), None);
        assert_eq!(integrity_level("S-1-16-"), None);
    }

    /// The sentence under the refusal is the whole point of it, so it names the step.
    #[test]
    fn the_refusal_names_the_way_out() {
        let said = advice();
        assert!(
            said.contains("Nothing has been changed"),
            "a refusal at the door should say the machine is untouched: {said}"
        );
        if cfg!(windows) {
            assert!(said.contains("Run as administrator"), "{said}");
        } else {
            assert!(said.contains("sudo"), "{said}");
        }
    }
}
