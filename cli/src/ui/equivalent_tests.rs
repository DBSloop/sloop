//! **The `Done when`: pasting the printed line reproduces the run exactly.**
//!
//! Which is a property of the *parser*, not of a string — so these do not compare the line
//! with one somebody typed into an assertion twice. They hand it back to `clap` and look at
//! what comes out, which is the same thing `main` looks at.

use clap::Parser as _;

use super::{argv, line};
use crate::cli::{BackupsCommand, Cli, Command, DbCommand};
use crate::ui::flow::{Answers, Doing, How, Job, Next, field};

/// Every job the menu can run, so a new one cannot be added without an arm here.
const EVERY_JOB: &[Job] = &[
    Job::DbAdd,
    Job::DbCreate,
    Job::DbList,
    Job::DbTest,
    Job::DbEdit,
    Job::DbRename,
    Job::DbRemove,
    Job::DbDrop,
    Job::Backup,
    Job::BackupAll,
    Job::BackupsList,
    Job::Restore,
    Job::BackupsPrune,
    Job::Mirror,
    Job::Sync,
    Job::KeyExport,
    Job::KeyImport,
    Job::Query,
    Job::Doctor,
    Job::Setup,
    Job::ServerInstall,
    Job::ServerConnection,
];

/// A world with a few databases and a backup in it, so every question has an answer.
#[derive(Default)]
struct Bench;

impl Doing for Bench {
    fn databases(&self) -> Vec<String> {
        vec!["orders".to_owned(), "staging".to_owned()]
    }

    fn backups_of(&self, _name: &str) -> Vec<String> {
        vec!["20260916T031500Z".to_owned()]
    }

    /// The label is not the name. `orders` is filed under that label and is called
    /// `orders_live` on its server, which is exactly the difference `--confirm` is about.
    fn on_the_server(&self, label: &str) -> Option<String> {
        Some(format!("{label}_live"))
    }

    fn run(&mut self, _job: Job, _answers: &Answers) -> crate::failure::Outcome<crate::exit::Exit> {
        unreachable!("nothing is run here")
    }
}

/// Answer every question the way somebody pressing Enter answers it: the first choice on a
/// list, and whatever is already in a box.
fn pressing_enter(job: Job) -> Answers {
    let world = Bench;
    let mut answers = Answers::default();

    for _ in 0..40 {
        match job.next(&answers, &world) {
            Next::Ask(step) => {
                let value = match &step.how {
                    How::Pick { values, .. } => values.first().cloned().unwrap_or_default(),
                    How::Type {
                        initial, needed, ..
                    } if *needed && initial.trim().is_empty() => "orders".to_owned(),
                    How::Type { initial, .. } => initial.clone(),
                };
                answers.put(step.field, value);
            }
            _ => return answers,
        }
    }
    panic!("{job:?} never stopped asking");
}

/// The line, parsed back through the real command surface.
fn parsed(job: Job, answers: &Answers) -> Option<Cli> {
    let parts = argv(job, answers, &Bench)?;
    let mut words = vec!["sloop".to_owned()];
    words.extend(parts);
    Some(Cli::try_parse_from(&words).unwrap_or_else(|why| {
        panic!("{job:?} printed a line clap will not take: {why}\n{words:?}")
    }))
}

/// **Every job's line parses**, which is the half of the `Done when` a person cannot check
/// by reading. A flag renamed on the command surface and forgotten here fails right there.
#[test]
fn every_line_the_menu_prints_is_one_the_binary_accepts() {
    for job in EVERY_JOB {
        let answers = pressing_enter(*job);
        let Some(cli) = parsed(*job, &answers) else {
            continue;
        };
        assert!(
            cli.command.is_some(),
            "{job:?} printed a line with no command in it"
        );
    }
}

/// And the line is the one it claims to be: the same command, with the same name in it.
#[test]
fn the_line_names_the_command_the_menu_ran() {
    let cases: &[(Job, &str)] = &[
        (Job::DbAdd, "db add"),
        (Job::DbList, "db list"),
        (Job::DbTest, "db test"),
        (Job::DbEdit, "db edit"),
        (Job::DbRename, "db rename"),
        (Job::DbRemove, "db remove"),
        (Job::DbDrop, "db drop"),
        (Job::Backup, "backup"),
        (Job::BackupAll, "backup"),
        (Job::BackupsList, "backups list"),
        (Job::BackupsPrune, "backups prune"),
        (Job::Restore, "restore"),
        (Job::Mirror, "mirror"),
        (Job::Sync, "sync"),
        (Job::KeyExport, "key export"),
        (Job::KeyImport, "key import"),
        (Job::Doctor, "doctor"),
        (Job::ServerConnection, "server connection"),
    ];

    for (job, path) in cases {
        let answers = pressing_enter(*job);
        let cli = parsed(*job, &answers).unwrap_or_else(|| panic!("{job:?} printed nothing"));
        assert_eq!(
            cli.command.as_ref().map(Command::path),
            Some(*path),
            "{job:?}"
        );
    }
}

/// **The three that print nothing, and why.** Two make a database and generate a password
/// for it, so running the line again would make a second one rather than repeat the run;
/// `query` prints its own, carrying the statement, which the menu never sees.
#[test]
fn the_jobs_with_no_useful_line_print_none() {
    for job in [Job::DbCreate, Job::Setup, Job::ServerInstall, Job::Query] {
        let answers = pressing_enter(job);
        assert!(
            line(job, &answers, &Bench).is_none(),
            "{job:?} printed a line"
        );
    }
}

/// A job that destroys something named carries `--confirm <name on the server>`, because
/// that is the only way to run one without a terminal — rule 5, travelling with the line.
#[test]
fn a_destructive_line_confirms_the_name_it_destroys() {
    for job in [Job::DbDrop, Job::Restore] {
        let mut answers = pressing_enter(job);
        answers.put(field::NAME, "orders".to_owned());

        let parts =
            argv(job, &answers, &Bench).unwrap_or_else(|| panic!("{job:?} printed nothing"));
        let at = parts
            .iter()
            .position(|part| part == "--confirm")
            .unwrap_or_else(|| panic!("{job:?} does not confirm: {parts:?}"));

        // **The name on the server, not the label.** A line that confirmed the label would
        // be refused by the command, which is what makes this the sharpest half of
        // "reproduces the run exactly".
        assert_eq!(parts.get(at + 1).map(String::as_str), Some("orders_live"));

        let cli = parsed(job, &answers).expect("it printed a line");
        assert_eq!(cli.confirm.as_deref(), Some("orders_live"), "{job:?}");
    }
}

/// **A label with a space in it survives being pasted**, which is the defect
/// `style::as_argument` exists for — see "A suggested command has to survive being pasted".
#[test]
fn a_name_with_a_space_in_it_is_quoted() {
    let mut answers = pressing_enter(Job::DbRemove);
    answers.put(field::NAME, "Test Sloop DB 2".to_owned());

    let printed = line(Job::DbRemove, &answers, &Bench).expect("it printed a line");
    assert!(printed.contains("'Test Sloop DB 2'"), "{printed}");

    // And what it becomes when a shell takes it apart is one argument, not four.
    let cli = parsed(Job::DbRemove, &answers).expect("it printed a line");
    let Some(Command::Db {
        command: DbCommand::Remove { name },
    }) = cli.command
    else {
        panic!("that is not db remove");
    };
    assert_eq!(name, "Test Sloop DB 2");
}

/// *"Every one of them"* is the menu's way of saying what the flag surface says by leaving
/// the argument out, and the two have to agree.
#[test]
fn every_one_of_them_leaves_the_name_out() {
    let mut answers = pressing_enter(Job::BackupsList);
    answers.put(field::WHICH, String::new());

    let cli = parsed(Job::BackupsList, &answers).expect("it printed a line");
    let Some(Command::Backups {
        command: BackupsCommand::List { name, .. },
    }) = cli.command
    else {
        panic!("that is not backups list");
    };
    assert_eq!(name, None);
}

/// How a backup is kept is always named, never left to the default — a line that leaned on
/// the default would start meaning something else the day the default moved.
#[test]
fn a_backup_line_says_how_it_keeps_them() {
    for (mode, expected) in [("replace", "--replace"), ("sequential", "--sequential")] {
        let mut answers = pressing_enter(Job::Backup);
        answers.put(field::MODE, mode.to_owned());

        let parts = argv(Job::Backup, &answers, &Bench).expect("it printed a line");
        assert!(parts.iter().any(|part| part == expected), "{parts:?}");
    }
}

/// `doctor`'s question is asked the other way round from its flag, and the line has to turn
/// it back: *"ask the servers what each role may do?"* is the opposite of `--offline`.
#[test]
fn doctors_question_is_the_opposite_of_its_flag() {
    let mut answers = pressing_enter(Job::Doctor);

    answers.put(field::OFFLINE, "yes".to_owned());
    let asked = argv(Job::Doctor, &answers, &Bench).expect("it printed a line");
    assert!(!asked.iter().any(|part| part == "--offline"), "{asked:?}");

    let mut answers = pressing_enter(Job::Doctor);
    answers.put(field::OFFLINE, "no".to_owned());
    let quiet = argv(Job::Doctor, &answers, &Bench).expect("it printed a line");
    assert!(quiet.iter().any(|part| part == "--offline"), "{quiet:?}");
}

/// A password is never in the line — the route is, exactly as the registry holds it.
#[test]
fn no_line_ever_carries_a_password() {
    for job in EVERY_JOB {
        let answers = pressing_enter(*job);
        let Some(printed) = line(*job, &answers, &Bench) else {
            continue;
        };
        assert!(
            !printed.contains("--password ") && !printed.contains("--password="),
            "{job:?} printed {printed}"
        );
        assert!(
            !printed.contains("-stdin"),
            "{job:?} printed a flag for a run with no terminal: {printed}"
        );
    }
}

/// **The other half of the `Done when`, spelled out on the command it matters most for.**
/// Not just that the line parses — that what it parses *into* is the run that happened.
#[test]
fn what_the_line_parses_into_is_the_run_that_happened() {
    let mut answers = pressing_enter(Job::DbAdd);
    answers.put(field::NAME, "orders".to_owned());
    answers.put(field::HOW, "fields".to_owned());
    answers.put(field::ENGINE, "postgres".to_owned());
    answers.put(field::HOST, "db.internal".to_owned());
    answers.put(field::PORT, "5433".to_owned());
    answers.put(field::DATABASE, "orders_live".to_owned());
    answers.put(field::USER, "app".to_owned());
    answers.put(field::ROUTE, "env".to_owned());
    answers.put(field::ENV, "ORDERS_PW".to_owned());
    answers.put(field::REACH, "ssh".to_owned());
    answers.put(field::SSH_HOST, "bastion.internal".to_owned());
    answers.put(field::SSH_PORT, "2222".to_owned());
    answers.put(field::SSH_USER, "deploy".to_owned());
    answers.put(field::SSH_ROUTE, "keyring".to_owned());
    answers.put(field::TEST, "yes".to_owned());

    let cli = parsed(Job::DbAdd, &answers).expect("it printed a line");
    let Some(Command::Db {
        command:
            DbCommand::Add {
                name,
                url,
                fields,
                password,
                ssh,
                test,
            },
    }) = cli.command
    else {
        panic!("that is not db add");
    };

    assert_eq!(name, "orders");
    assert_eq!(url, None, "the fields were answered, not a URL");
    assert_eq!(fields.engine.as_deref(), Some("postgres"));
    assert_eq!(fields.host.as_deref(), Some("db.internal"));
    assert_eq!(fields.port, Some(5433));
    assert_eq!(fields.database.as_deref(), Some("orders_live"));
    assert_eq!(fields.user.as_deref(), Some("app"));

    // The route, never the value — and never the flag for a run with no terminal.
    assert_eq!(password.env.as_deref(), Some("ORDERS_PW"));
    assert!(!password.keyring && !password.encrypted_file);
    assert!(!password.password_stdin);

    assert_eq!(ssh.ssh_host.as_deref(), Some("bastion.internal"));
    assert_eq!(ssh.ssh_port, Some(2222));
    assert_eq!(ssh.ssh_user.as_deref(), Some("deploy"));
    assert!(ssh.ssh_keyring);
    assert!(!ssh.no_ssh);
    assert!(!ssh.ssh_passphrase_stdin);

    assert!(test);
}

/// And "straight at it" is `--no-ssh` rather than silence: somebody who moved a tunnelled
/// database to a direct one said something, and the line has to say it too.
#[test]
fn a_direct_database_says_no_ssh_rather_than_nothing() {
    let mut answers = pressing_enter(Job::DbAdd);
    answers.put(field::REACH, "direct".to_owned());

    let cli = parsed(Job::DbAdd, &answers).expect("it printed a line");
    let Some(Command::Db {
        command: DbCommand::Add { ssh, .. },
    }) = cli.command
    else {
        panic!("that is not db add");
    };
    assert!(ssh.no_ssh);
}

/// Print the line every job would produce, for a person checking them against a real
/// binary. `cargo test --bin sloop print_every_line -- --nocapture --ignored`.
#[test]
#[ignore = "prints rather than asserts"]
fn print_every_line() {
    for job in EVERY_JOB {
        let answers = pressing_enter(*job);
        match line(*job, &answers, &Bench) {
            Some(printed) => println!("{job:?}\t{printed}"),
            None => println!("{job:?}\t(no line)"),
        }
    }
}

/// **`db remove` is a question, not a name.** It forgets a record and leaves the database
/// alone, so there is nothing for rule 5 to make anybody type — and a line that carried
/// `--confirm` instead of `--yes` would stop on *"Forget it?"* with no terminal to answer at.
/// Found by running it, not by reading it.
#[test]
fn forgetting_a_record_is_answered_with_yes() {
    let answers = pressing_enter(Job::DbRemove);
    let parts = argv(Job::DbRemove, &answers, &Bench).expect("it printed a line");

    assert!(parts.iter().any(|part| part == "--yes"), "{parts:?}");
    assert!(!parts.iter().any(|part| part == "--confirm"), "{parts:?}");

    let cli = parsed(Job::DbRemove, &answers).expect("it printed a line");
    assert!(cli.yes);
}

/// `backups prune` asks *"Remove them?"* — unless it is a rehearsal, which asks nothing.
#[test]
fn pruning_answers_its_question_only_when_it_is_going_to_remove_something() {
    let mut rehearsing = pressing_enter(Job::BackupsPrune);
    rehearsing.put(field::DRY, "yes".to_owned());
    let parts = argv(Job::BackupsPrune, &rehearsing, &Bench).expect("it printed a line");
    assert!(parts.iter().any(|part| part == "--dry-run"), "{parts:?}");
    assert!(!parts.iter().any(|part| part == "--yes"), "{parts:?}");

    let mut really = pressing_enter(Job::BackupsPrune);
    really.put(field::DRY, "no".to_owned());
    let parts = argv(Job::BackupsPrune, &really, &Bench).expect("it printed a line");
    assert!(!parts.iter().any(|part| part == "--dry-run"), "{parts:?}");
    assert!(parts.iter().any(|part| part == "--yes"), "{parts:?}");
}
