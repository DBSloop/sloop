//! The flag form of a run somebody did by hand — `R20`.
//!
//! **A session becomes a line somebody can schedule.** That is the whole of it: the menu is
//! how a person works something out, and the printed line is how they stop working it out
//! again next Tuesday at three in the morning. `CLAUDE.md` asks for it on every interactive
//! run, and the `Done when` is exact — *"pasting the printed line reproduces the run exactly,
//! non-interactively"*.
//!
//! **Printed after the alternate screen is handed back**, with the command's own output,
//! where it lands in the scrollback the user keeps. Anything printed inside the menu's screen
//! is gone the moment it redraws.
//!
//! **What it is not.** It is not a transcript, and it does not carry a password. Every
//! password in this program is asked for by the command on the terminal it was handed — see
//! `ui::flow`'s header — so there is nothing here to leave out: the line names the *route*
//! the password takes, exactly as the registry does, and running it asks again. That is not
//! a hole in *"reproduces the run exactly"*; it is the same question being asked the same way.
//!
//! **One place, and a test that parses what it writes.** The risk in a file like this is that
//! a flag is added to a command, wired into the menu, and forgotten here — so the line quietly
//! stops reproducing the run. `equivalent_tests` walks every [`Job`] there is, fills its
//! answers the way somebody pressing Enter would, and parses the line back through `clap`;
//! a job with no arm below fails to build, and a line that does not parse fails the test.

#[cfg(test)]
#[path = "equivalent_tests.rs"]
mod tests;

use crate::style;

use super::flow::{Answers, Doing, Job, field};

/// The line to print, or `None` for a job there is no useful flag form of.
///
/// **`db create` and `mirror --create` are the two that are `None`, and it is not an
/// omission.** Both ask for a password on the terminal and then *generate* one, print it
/// once and file it — so a second run of the same line does not repeat the run, it makes a
/// second database. A line that says "paste this to do it again" would be wrong about the
/// one command where doing it again is not the same thing.
#[must_use]
pub fn line(job: Job, answers: &Answers, world: &dyn Doing) -> Option<String> {
    let parts = argv(job, answers, world)?;
    Some(format!(
        "sloop {}",
        parts
            .iter()
            .map(|part| style::as_argument(part))
            .collect::<Vec<_>>()
            .join(" ")
    ))
}

/// The arguments themselves, unquoted, in the order the command line takes them.
///
/// Separate from [`line`] so a test can compare against what `clap` parses rather than
/// against a string somebody typed into an assertion twice.
#[must_use]
pub fn argv(job: Job, answers: &Answers, world: &dyn Doing) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    let named = |out: &mut Vec<String>, flag: &str, value: Option<&str>| {
        if let Some(value) = value {
            out.push(flag.to_owned());
            out.push(value.to_owned());
        }
    };
    let flag = |out: &mut Vec<String>, name: &str, on: bool| {
        if on {
            out.push(name.to_owned());
        }
    };

    match job {
        // **Four that print nothing, and it is not an omission.** `db create`, `setup` and
        // `server install` each generate a password, print it once and file it, so running
        // the line again would not repeat the run — it would make a second database, or a
        // second cluster. And `query` prints its own, carrying the statement the builder
        // wrote, which the menu never sees. See [`line`].
        Job::DbCreate | Job::Setup | Job::ServerInstall | Job::Query => return None,

        Job::DbAdd | Job::DbEdit => registering(&mut out, job, answers),

        Job::DbList => out.extend(["db".to_owned(), "list".to_owned()]),

        Job::DbTest => {
            out.extend(["db".to_owned(), "test".to_owned()]);
            out.extend(one_of_them(answers, field::WHICH));
        }

        Job::DbRename | Job::DbRemove | Job::DbDrop => {
            renaming_or_destroying(&mut out, job, answers, world);
        }

        Job::Backup | Job::BackupAll => {
            out.push("backup".to_owned());
            if job == Job::Backup {
                out.push(answers.text(field::NAME).to_owned());
            } else {
                out.push("--all".to_owned());
            }
            // Named either way: the default is `--sequential` today, and a line that leaned
            // on that would start meaning something else the day the default moved.
            out.push(if answers.text(field::MODE) == "replace" {
                "--replace".to_owned()
            } else {
                "--sequential".to_owned()
            });
        }

        Job::BackupsList => {
            out.extend(["backups".to_owned(), "list".to_owned()]);
            out.extend(one_of_them(answers, field::WHICH));
            flag(&mut out, "--check", answers.yes(field::CHECK));
        }

        Job::BackupsPrune => {
            out.extend(["backups".to_owned(), "prune".to_owned()]);
            out.extend(one_of_them(answers, field::WHICH));
            named(&mut out, "--keep", answers.some(field::KEEP));
            named(&mut out, "--older-than", answers.some(field::OLDER));
            flag(&mut out, "--include-broken", answers.yes(field::BROKEN));
            flag(&mut out, "--dry-run", answers.yes(field::DRY));
            // A rehearsal asks nothing; a real one asks *"Remove them?"* and `--yes` is
            // what answers it.
            flag(&mut out, "--yes", !answers.yes(field::DRY));
        }

        Job::Restore => {
            out.push("restore".to_owned());
            out.push(answers.text(field::NAME).to_owned());
            named(&mut out, "--from", answers.some(field::WHEN));
            confirming(&mut out, answers.some(field::NAME), world);
        }

        Job::Mirror | Job::Sync => {
            out.push(if job == Job::Mirror { "mirror" } else { "sync" }.to_owned());
            out.push(answers.text(field::SOURCE).to_owned());

            if answers.text(field::WHERE) == "new" {
                // A destination that does not exist yet is made, and making it generates a
                // password — the same reason `db create` has no line. See [`line`].
                return None;
            }
            named(&mut out, "--to", answers.some(field::TO));
            // The destination is what is replaced, so it is the destination that is named.
            confirming(&mut out, answers.some(field::TO), world);

            for pattern in answers.words(field::TABLES) {
                out.push("--table".to_owned());
                out.push(pattern);
            }
            flag(
                &mut out,
                "--with-references",
                answers.yes(field::REFERENCES),
            );
            flag(&mut out, "--safe", answers.yes(field::SAFE));
        }

        Job::KeyExport => out.extend(["key".to_owned(), "export".to_owned()]),
        Job::KeyImport => out.extend(["key".to_owned(), "import".to_owned()]),

        Job::Doctor => {
            out.push("doctor".to_owned());
            // The menu asks the question the other way round: *"ask the servers what each
            // role may do?"* is the opposite of `--offline`.
            flag(&mut out, "--offline", !answers.yes(field::OFFLINE));
        }

        Job::ServerConnection => {
            out.extend(["server".to_owned(), "connection".to_owned()]);
            flag(&mut out, "--show-password", answers.yes(field::PASSWORD));
        }

        Job::ServiceActivity => out.extend(["service".to_owned(), "activity".to_owned()]),
    }

    Some(out)
}

/// `db add` and `db edit`: the same vocabulary, and the same two halves under it.
///
/// One function because they *are* one command surface — `edit` is `add` with every field
/// optional — and because a flag added to one and forgotten in the other is the drift this
/// whole module exists to stop.
fn registering(out: &mut Vec<String>, job: Job, answers: &Answers) {
    out.push("db".to_owned());
    out.push(if job == Job::DbAdd { "add" } else { "edit" }.to_owned());
    out.push(answers.text(field::NAME).to_owned());

    let detail = answers.text(field::DETAIL);
    let about_the_connection = job == Job::DbAdd || (detail != "password" && detail != "reach");

    if job == Job::DbAdd && answers.text(field::HOW) == "url" {
        if let Some(url) = answers.some(field::URL) {
            out.push("--url".to_owned());
            out.push(url.to_owned());
        }
    } else if about_the_connection {
        connection(out, answers);
    }

    password_route(out, answers);

    // **`edit` changes one detail at a time**, so the SSH flags are only in the line when
    // that was the detail — every other field arriving as nothing is what leaves a record
    // alone. `add` writes a whole record, so they are always in it.
    if job == Job::DbAdd || detail == "reach" {
        reach(out, answers);
    }

    if answers.yes(field::TEST) {
        out.push("--test".to_owned());
    }
}

/// `db rename`, `db remove` and `db drop`: a name, and what has to travel with it.
///
/// **Two of the three are destructive, and rule 5 travels with the line.** Without a
/// terminal the only way to run one is `--confirm <name>` typed out — which is exactly what
/// a scheduled run needs, and exactly what the person just typed by hand.
fn renaming_or_destroying(out: &mut Vec<String>, job: Job, answers: &Answers, world: &dyn Doing) {
    out.push("db".to_owned());
    out.push(
        match job {
            Job::DbRename => "rename",
            Job::DbRemove => "remove",
            _ => "drop",
        }
        .to_owned(),
    );
    out.push(answers.text(field::NAME).to_owned());

    match job {
        Job::DbRename => out.push(answers.text(field::RENAMED).to_owned()),
        // **A question, so `--yes`.** `db remove` forgets a record and leaves the database
        // alone, so there is nothing named for rule 5 to make somebody type — it asks
        // *"Forget it?"*, and `--yes` is what answers that without a terminal.
        Job::DbRemove => out.push("--yes".to_owned()),
        // **A name, typed, so `--confirm`** — and the database's own name on the server
        // rather than the label, because that is the one the command asks for.
        _ => confirming(out, answers.some(field::NAME), world),
    }
}

/// `--confirm <name on the server>`, for a command that destroys something named.
///
/// **The label is not the name.** `--confirm` takes what the database is called on its
/// server, so that a cron line names the thing that stops existing and cannot be repointed
/// at another database by editing the label. A label nothing is registered under leaves the
/// flag out, and the pasted line then asks — which is the honest outcome for a record that
/// is no longer there.
fn confirming(out: &mut Vec<String>, label: Option<&str>, world: &dyn Doing) {
    if let Some(named) = label.and_then(|label| world.on_the_server(label)) {
        out.push("--confirm".to_owned());
        out.push(named);
    }
}

/// A name, unless the answer was *"every one of them"* — which the menu files as the empty
/// string and the flag surface says by leaving the argument out. [`Answers::some`] already
/// reads a blank answer as nothing, so the two agree without a sentinel between them.
fn one_of_them(answers: &Answers, which: &str) -> Vec<String> {
    answers
        .some(which)
        .map(|named| vec![named.to_owned()])
        .unwrap_or_default()
}

/// The connection fields, exactly as [`crate::commands::menu`] hands them over.
fn connection(out: &mut Vec<String>, answers: &Answers) {
    for (flag, value) in [
        ("--engine", answers.some(field::ENGINE)),
        ("--host", answers.some(field::HOST)),
        ("--port", answers.some(field::PORT)),
        ("--database", answers.some(field::DATABASE)),
        ("--user", answers.some(field::USER)),
    ] {
        if let Some(value) = value {
            out.push(flag.to_owned());
            out.push(value.to_owned());
        }
    }
}

/// Where the password is kept — the route, never the value.
fn password_route(out: &mut Vec<String>, answers: &Answers) {
    match answers.text(field::ROUTE) {
        "keyring" => out.push("--keyring".to_owned()),
        "file" => out.push("--encrypted-file".to_owned()),
        "env" => {
            if let Some(name) = answers.some(field::ENV) {
                out.push("--env".to_owned());
                out.push(name.to_owned());
            }
        }
        "command" => {
            if let Some(command) = answers.some(field::FROM_COMMAND) {
                out.push("--password-from".to_owned());
                out.push(command.to_owned());
            }
        }
        _ => {}
    }
}

/// How the database is reached — `R19e`'s flags, or `--no-ssh` for a direct one.
fn reach(out: &mut Vec<String>, answers: &Answers) {
    if !answers.asked(field::REACH) {
        return;
    }
    if answers.text(field::REACH) != "ssh" {
        out.push("--no-ssh".to_owned());
        return;
    }

    for (flag, value) in [
        ("--ssh-host", answers.some(field::SSH_HOST)),
        ("--ssh-port", answers.some(field::SSH_PORT)),
        ("--ssh-user", answers.some(field::SSH_USER)),
        ("--ssh-identity", answers.some(field::SSH_IDENTITY)),
    ] {
        if let Some(value) = value {
            out.push(flag.to_owned());
            out.push(value.to_owned());
        }
    }

    match answers.text(field::SSH_ROUTE) {
        "keyring" => out.push("--ssh-keyring".to_owned()),
        "file" => out.push("--ssh-encrypted-file".to_owned()),
        "env" => {
            if let Some(name) = answers.some(field::SSH_ENV) {
                out.push("--ssh-env".to_owned());
                out.push(name.to_owned());
            }
        }
        "command" => {
            if let Some(command) = answers.some(field::SSH_FROM_COMMAND) {
                out.push("--ssh-passphrase-from".to_owned());
                out.push(command.to_owned());
            }
        }
        _ => {}
    }
}
