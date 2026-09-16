//! The questions, and the order they come in.
//!
//! These drive [`Job::next`] directly with a made-up world, so every branch of every flow
//! is walked without a terminal, a registry or a database anywhere near it. What each job
//! then *does* with the answers is `commands::menu`'s, and is tested there.

use super::{Answers, Doing, How, Job, Next, Step, field};
use crate::exit::Exit;
use crate::failure::Outcome;

/// A world with some databases in it and some backups of one of them.
struct Known {
    databases: Vec<String>,
    backups: Vec<String>,
}

impl Known {
    fn with(databases: &[&str]) -> Self {
        Self {
            databases: databases.iter().map(|name| (*name).to_owned()).collect(),
            backups: vec!["20260916T031500Z".to_owned(), "20260915T031500Z".to_owned()],
        }
    }

    fn empty() -> Self {
        Self {
            databases: Vec::new(),
            backups: Vec::new(),
        }
    }

    fn without_backups(databases: &[&str]) -> Self {
        Self {
            backups: Vec::new(),
            ..Self::with(databases)
        }
    }
}

impl Doing for Known {
    fn databases(&self) -> Vec<String> {
        self.databases.clone()
    }

    fn backups_of(&self, _name: &str) -> Vec<String> {
        self.backups.clone()
    }

    fn run(&mut self, _job: Job, _answers: &Answers) -> Outcome<Exit> {
        unreachable!("these tests never run anything")
    }
}

/// Every job, so a flow added later cannot quietly go untested.
const EVERY: &[Job] = &[
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
    Job::Doctor,
];

/// Answer every question the way somebody pressing Enter would: the first item of a list,
/// or whatever is already in the box. Returns what was asked, in order.
fn walk_it(job: Job, world: &dyn Doing) -> (Answers, Vec<Step>) {
    let mut answers = Answers::default();
    let mut asked = Vec::new();

    for _ in 0..40 {
        match job.next(&answers, world) {
            Next::Ask(step) => {
                let value = match &step.how {
                    How::Pick { values, .. } => values
                        .first()
                        .cloned()
                        .expect("a question with nothing to pick is not a question"),
                    How::Type { initial, .. } => initial.clone(),
                };
                answers.put(step.field, value);
                asked.push(*step);
            }
            Next::Ready | Next::Blocked(_) => return (answers, asked),
        }
    }
    panic!("{job:?} never stopped asking");
}

/// **The `Done when`: every command is reachable and completable without touching a flag.**
///
/// Reachable is the tree, which `ui::tests` walks. This is completable: every job, answered
/// the way somebody pressing Enter answers it, ends up ready to run.
#[test]
fn every_job_can_be_answered_to_the_end() {
    let world = Known::with(&["orders", "orders_staging"]);

    for job in EVERY {
        let (answers, asked) = walk_it(*job, &world);
        assert_eq!(
            job.next(&answers, &world),
            Next::Ready,
            "{job:?} was not ready after {} questions: {:?}",
            asked.len(),
            asked.iter().map(|step| step.field).collect::<Vec<_>>()
        );
    }
}

/// No question is asked twice, and none is asked with nothing to answer it.
#[test]
fn no_question_is_asked_twice_and_none_is_empty() {
    let world = Known::with(&["orders", "orders_staging"]);

    for job in EVERY {
        let (_, asked) = walk_it(*job, &world);

        let mut seen: Vec<&str> = Vec::new();
        for step in &asked {
            assert!(
                !seen.contains(&step.field),
                "{job:?} asked for {} twice",
                step.field
            );
            seen.push(step.field);

            assert!(
                step.question.ends_with('?') || step.question.ends_with('.'),
                "{job:?} asked {:?}, which is not a sentence",
                step.question
            );
            if let How::Pick { items, values } = &step.how {
                assert_eq!(items.len(), values.len(), "{job:?}: {}", step.field);
                assert!(
                    !items.is_empty(),
                    "{job:?} asked {} with no answers",
                    step.field
                );
            }
        }
    }
}

/// **Nothing that is a password is ever asked for here.** Rule 3 lives in the commands,
/// which ask for it themselves on the terminal the menu has just handed back — and a flow
/// that collected one would be a second place for a password to exist.
#[test]
fn no_flow_ever_asks_for_a_password() {
    let world = Known::with(&["orders"]);

    for job in EVERY {
        let (_, asked) = walk_it(*job, &world);
        for step in &asked {
            let question = step.question.to_lowercase();
            assert!(
                !question.contains("password?") && !question.starts_with("the password"),
                "{job:?} asked for a password: {:?}",
                step.question
            );
        }
    }
}

/// **Nor does one ever ask somebody to type the name of what is about to be destroyed.**
/// Rule 5, in `consent`, once — the menu picks *which* database, and the command asks for
/// the name in the same words it uses from a shell.
#[test]
fn dropping_a_database_is_a_pick_and_the_typing_is_the_commands() {
    let world = Known::with(&["orders", "orders_staging"]);
    let (answers, asked) = walk_it(Job::DbDrop, &world);

    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(asked[0].field, field::NAME);
    assert!(matches!(asked[0].how, How::Pick { .. }));
    assert_eq!(answers.text(field::NAME), "orders");
}

/// A job that needs a registered database says so instead of drawing an empty list.
#[test]
fn a_job_that_needs_a_database_says_so_when_there_are_none() {
    let world = Known::empty();

    for job in EVERY {
        let asked = job.next(&Answers::default(), &world);
        match job {
            Job::DbAdd | Job::DbCreate => {
                assert!(matches!(asked, Next::Ask(_)), "{job:?} refused to start");
            }
            Job::DbList | Job::KeyExport | Job::KeyImport => {
                assert_eq!(asked, Next::Ready, "{job:?}");
            }
            Job::Doctor => assert!(matches!(asked, Next::Ask(_)), "{job:?}"),
            _ => assert!(
                matches!(asked, Next::Blocked(_)),
                "{job:?} offered a list of nothing"
            ),
        }
    }
}

/// And a restore says the useful thing rather than the general one: there is nothing to
/// put back, and where to get something.
#[test]
fn restoring_with_no_backups_names_what_to_do_first() {
    let world = Known::without_backups(&["orders"]);
    let mut answers = Answers::default();
    answers.put(field::NAME, "orders");

    let Next::Blocked(why) = Job::Restore.next(&answers, &world) else {
        panic!("a restore with no backups should not offer a list");
    };
    assert!(why.contains("orders"), "{why}");
    assert!(why.contains("Back one up"), "{why}");
}

/// **The branch that makes a plan a plan rather than a list.** `db add` asks for a URL or
/// for five fields, and changing the answer two questions back changes what comes next.
#[test]
fn changing_an_answer_changes_the_questions_after_it() {
    let world = Known::with(&["orders"]);

    let mut answers = Answers::default();
    answers.put(field::NAME, "orders");
    answers.put(field::HOW, "url");
    let Next::Ask(step) = Job::DbAdd.next(&answers, &world) else {
        panic!("a URL was not asked for");
    };
    assert_eq!(step.field, field::URL);

    // Back one question, and answer it the other way.
    assert!(answers.undo());
    answers.put(field::HOW, "fields");
    let Next::Ask(step) = Job::DbAdd.next(&answers, &world) else {
        panic!("the fields were not asked for");
    };
    assert_eq!(step.field, field::ENGINE);

    // And the name given before the branch is still given.
    assert_eq!(answers.text(field::NAME), "orders");
}

/// Going back drops one answer and no more.
#[test]
fn going_back_drops_exactly_one_answer() {
    let mut answers = Answers::default();
    answers.put(field::NAME, "orders");
    answers.put(field::HOW, "fields");
    answers.put(field::ENGINE, "postgres");

    assert!(answers.undo());
    assert_eq!(answers.get(field::ENGINE), None);
    assert_eq!(answers.text(field::HOW), "fields");
    assert_eq!(answers.text(field::NAME), "orders");

    assert!(answers.undo());
    assert!(answers.undo());
    assert!(!answers.undo(), "there was nothing left to drop");
}

/// A copy never offers the source as its own destination.
#[test]
fn a_copy_cannot_be_pointed_at_itself() {
    let world = Known::with(&["orders", "orders_staging", "orders_ci"]);

    for job in [Job::Mirror, Job::Sync] {
        let mut answers = Answers::default();
        answers.put(field::SOURCE, "orders");
        answers.put(field::WHERE, "known");

        let Next::Ask(step) = job.next(&answers, &world) else {
            panic!("{job:?} did not ask where it was going");
        };
        assert_eq!(step.field, field::TO);
        let How::Pick { values, .. } = step.how else {
            panic!("a destination should be picked, not typed");
        };
        assert!(!values.contains(&"orders".to_owned()), "{values:?}");
        assert_eq!(values.len(), 2, "{values:?}");
    }
}

/// Making a destination asks about the destination; using one that exists does not.
#[test]
fn a_copy_only_asks_how_to_build_a_destination_it_is_building() {
    let world = Known::with(&["orders", "orders_staging"]);

    let mut existing = Answers::default();
    existing.put(field::SOURCE, "orders");
    existing.put(field::WHERE, "known");
    existing.put(field::TO, "orders_staging");
    let Next::Ask(step) = Job::Mirror.next(&existing, &world) else {
        panic!("nothing else was asked");
    };
    assert_eq!(
        step.field,
        field::SCOPE,
        "an existing destination was asked how to build it"
    );

    let mut making = Answers::default();
    making.put(field::SOURCE, "orders");
    making.put(field::WHERE, "new");
    let Next::Ask(step) = Job::Mirror.next(&making, &world) else {
        panic!("nothing was asked about the new destination");
    };
    assert_eq!(step.field, field::CREATE);
}

/// Every table, or some. Only "some" asks which.
#[test]
fn only_a_partial_copy_is_asked_which_tables() {
    let world = Known::with(&["orders", "orders_staging"]);

    let mut answers = Answers::default();
    answers.put(field::SOURCE, "orders");
    answers.put(field::WHERE, "known");
    answers.put(field::TO, "orders_staging");
    answers.put(field::SCOPE, "all");

    let Next::Ask(step) = Job::Sync.next(&answers, &world) else {
        panic!("nothing else was asked");
    };
    assert_eq!(
        step.field,
        field::SAFE,
        "a whole-database copy was asked about tables"
    );
}

/// A blank answer means "you decide", everywhere, and a list of patterns comes apart the
/// way somebody would write one.
#[test]
fn a_blank_answer_declines_and_a_list_comes_apart() {
    let mut answers = Answers::default();
    answers.put(field::PORT, "  ");
    answers.put(field::TABLES, " orders, public.audit_* log_?  ");
    answers.put(field::KEEP, "7");

    assert_eq!(answers.some(field::PORT), None);
    assert_eq!(answers.number::<u16>(field::PORT), None);
    assert_eq!(answers.number::<usize>(field::KEEP), Some(7));
    assert_eq!(
        answers.words(field::TABLES),
        ["orders", "public.audit_*", "log_?"]
    );
    assert!(answers.words(field::PORT).is_empty());
}

/// A yes-or-no question puts the likelier answer first, so Enter is usually right — and a
/// no is a real answer rather than an unanswered question.
#[test]
fn a_yes_or_no_question_leads_with_the_likely_answer() {
    let world = Known::with(&["orders"]);
    let mut answers = Answers::default();
    answers.put(field::WHICH, "orders");

    let Next::Ask(step) = Job::BackupsList.next(&answers, &world) else {
        panic!("the check was not asked about");
    };
    let How::Pick { items, values } = &step.how else {
        panic!("yes or no is a pick");
    };
    assert_eq!(
        items[0].title, "No",
        "hashing every dump is not the default"
    );
    assert_eq!(values, &["no", "yes"]);

    answers.put(field::CHECK, "no");
    assert!(!answers.yes(field::CHECK));
    assert!(answers.asked(field::CHECK), "a no is still an answer");
}

/// The summary reads back what was said, in the order it was asked, and says plainly when
/// a blank meant "you decide".
#[test]
fn the_screen_reads_back_what_it_was_told() {
    let mut answers = Answers::default();
    answers.put(field::NAME, "orders");
    answers.put(field::ENGINE, "postgres");
    answers.put(field::HOST, "db.internal");
    answers.put(field::PORT, "");

    let said = Job::so_far(&answers);
    assert_eq!(
        said,
        vec![
            ("name", "orders".to_owned()),
            ("engine", "postgres".to_owned()),
            ("host", "db.internal".to_owned()),
            ("port", "sloop decides".to_owned()),
        ]
    );

    let mut all = Answers::default();
    all.put(field::WHICH, "");
    assert_eq!(
        Job::so_far(&all),
        vec![("database", "every one of them".to_owned())]
    );
}
