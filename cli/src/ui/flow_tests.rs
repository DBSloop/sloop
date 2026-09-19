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
    /// What the service has been told to watch. Its own list, because `R23a`'s pickers read
    /// it rather than the registry — a database can be registered and unattached, and the
    /// screens that offer one have to tell those apart.
    watched: Vec<String>,
    /// Which of them are in the **global** registry. Its own list for the same reason: a
    /// machine can have a project full of databases and nothing a service could watch, and
    /// that is the case `service attach` has a sentence of its own for.
    globally: Vec<String>,
}

impl Known {
    fn with(databases: &[&str]) -> Self {
        Self {
            databases: databases.iter().map(|name| (*name).to_owned()).collect(),
            backups: vec!["20260916T031500Z".to_owned(), "20260915T031500Z".to_owned()],
            watched: databases.iter().map(|name| (*name).to_owned()).collect(),
            globally: databases.iter().map(|name| (*name).to_owned()).collect(),
        }
    }

    fn empty() -> Self {
        Self {
            databases: Vec::new(),
            backups: Vec::new(),
            watched: Vec::new(),
            globally: Vec::new(),
        }
    }

    /// Databases registered, and a service watching none of them.
    fn none_attached(databases: &[&str]) -> Self {
        Self {
            watched: Vec::new(),
            ..Self::with(databases)
        }
    }

    /// Databases registered in a project, and none in the global store — which is the only
    /// one a service can see.
    fn none_global(databases: &[&str]) -> Self {
        Self {
            globally: Vec::new(),
            watched: Vec::new(),
            ..Self::with(databases)
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
    fn on_the_server(&self, label: &str) -> Option<String> {
        Some(format!("{label}_live"))
    }

    fn databases(&self) -> Vec<String> {
        self.databases.clone()
    }

    fn backups_of(&self, _name: &str) -> Vec<String> {
        self.backups.clone()
    }

    fn globally_registered(&self) -> Vec<String> {
        self.globally.clone()
    }

    fn watched(&self) -> Vec<String> {
        self.watched.clone()
    }

    fn run(&mut self, _job: Job, _answers: &Answers) -> Outcome<Exit> {
        unreachable!("these tests never run anything")
    }
}

/// Every job, so a flow added later cannot quietly go untested.
///
/// **`Job::every()`, not a list kept here.** This one was eighteen entries long while the
/// enum had twenty-three, so `query`, `setup`, `server install`, `server connection` and
/// `service activity` were all outside a constant named `EVERY`. One list now, guarded by an
/// exhaustive match in `flow.rs`.
fn every() -> &'static [Job] {
    Job::every()
}

/// **`R23a`, and `R28`'s rule reaching the menu.** A screen with an empty list says what to
/// do about it, and what to do is not the same thing for all three: a database has to be
/// registered globally before the service can watch it, and watched before it can be
/// scheduled or detached. The generic sentence — *register another database first* — would
/// send somebody who has done exactly that back to do it again.
#[test]
fn a_service_screen_with_nothing_to_pick_names_the_step_that_fixes_it() {
    let nothing_watched = Known::none_attached(&["orders"]);

    for (job, expected) in [
        (Job::ServiceDetach, "not watching anything"),
        (Job::ServiceSchedule, "not watching anything"),
    ] {
        match job.next(&Answers::default(), &nothing_watched) {
            Next::Blocked(said) => assert!(
                said.contains(expected),
                "{job:?} said {said:?}, which does not name the step"
            ),
            other => panic!("{job:?} offered a list of nothing: {other:?}"),
        }
    }

    // A project full of databases and nothing in the global store, which is the only one a
    // service can see. `Known::empty()` would not reach this: with nothing registered at all
    // the flow blocks a step earlier, on the sentence every job shares.
    let nothing_global = Known::none_global(&["orders"]);
    match Job::ServiceAttach.next(&Answers::default(), &nothing_global) {
        Next::Blocked(said) => assert!(said.contains("global registry"), "{said:?}"),
        other => panic!("attach offered a list of nothing: {other:?}"),
    }
}

#[test]
fn the_list_of_every_job_is_every_job() {
    for job in every() {
        assert!(job.in_the_list(), "{job:?} is missing from Job::every()");
    }
}

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

    for job in every() {
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

    for job in every() {
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

    for job in every() {
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

    // **Checked against the rule rather than against a list of exceptions.** The list
    // version named five jobs and swept the rest into `_`, which was right while `EVERY`
    // held eighteen of them and wrong the moment it held all thirty-one: `setup`,
    // `server install` and most of `service` are answerable on a machine with nothing
    // registered, and being swept into "must be blocked" would have made this test demand
    // the opposite of what those commands are for.
    for job in every() {
        let asked = job.next(&Answers::default(), &world);
        if job.needs_a_database() {
            assert!(
                matches!(asked, Next::Blocked(_)),
                "{job:?} offered a list of nothing"
            );
        } else {
            assert!(
                !matches!(asked, Next::Blocked(_)),
                "{job:?} refused to start on a machine with nothing registered, and it does \
                 not need anything registered"
            );
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

/// **A blank is an answer for some boxes and not for others**, and pressing Enter on an
/// empty name must not reach the command as a run with no name.
#[test]
fn a_box_says_whether_leaving_it_blank_is_an_answer() {
    let world = Known::with(&["orders", "orders_staging"]);

    // Everything a command cannot do without.
    for (job, field) in [
        (Job::DbAdd, field::NAME),
        (Job::DbCreate, field::NAME),
        (Job::DbRename, field::RENAMED),
    ] {
        let needed = walk_it(job, &world)
            .1
            .into_iter()
            .find(|step| step.field == field)
            .unwrap_or_else(|| panic!("{job:?} never asked for {field}"));
        assert!(
            matches!(needed.how, How::Type { needed: true, .. }),
            "{job:?} would take a blank {field}"
        );
    }

    // And everything where a blank means "you decide", which is a real answer.
    for (job, field) in [
        (Job::DbCreate, field::PORT),
        (Job::DbCreate, field::SUPERUSER),
        (Job::BackupsPrune, field::KEEP),
        (Job::BackupsPrune, field::OLDER),
    ] {
        let loose = walk_it(job, &world)
            .1
            .into_iter()
            .find(|step| step.field == field)
            .unwrap_or_else(|| panic!("{job:?} never asked for {field}"));
        assert!(
            matches!(loose.how, How::Type { needed: false, .. }),
            "{job:?} insists on {field}"
        );
    }
}

/// A box with something already in it is one Enter is a right answer to, so it is never
/// one of the required ones that could be left empty by accident.
#[test]
fn every_box_that_opens_full_is_one_enter_can_answer() {
    let world = Known::with(&["orders", "orders_staging"]);

    for job in every() {
        for step in walk_it(*job, &world).1 {
            let How::Type {
                initial, needed, ..
            } = &step.how
            else {
                continue;
            };
            assert!(
                !initial.trim().is_empty() || !needed || step.field != field::PORT,
                "{job:?}: {} opens empty and insists on an answer",
                step.field
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// reaching it over SSH — R19e
// ---------------------------------------------------------------------------------------

/// **The default answer asks nothing more.** Most databases are reached straight at their
/// address, and a registration that went through four SSH questions to say so would be a
/// screen everybody learns to press Enter through.
#[test]
fn registering_asks_about_ssh_once_and_stops_there() {
    let world = Known::empty();
    let (answers, asked) = walk_it(Job::DbAdd, &world);

    assert_eq!(
        answers.text(field::REACH),
        "direct",
        "the first option is the usual one"
    );
    for field in [
        field::SSH_HOST,
        field::SSH_PORT,
        field::SSH_USER,
        field::SSH_IDENTITY,
        field::SSH_ROUTE,
    ] {
        assert!(
            !asked.iter().any(|step| step.field == field),
            "{field} was asked although nothing goes over SSH: {asked:?}"
        );
    }
}

/// And saying so opens exactly the questions a tunnel needs — including the sentence that
/// everybody gets backwards once, on the question where it matters.
#[test]
fn saying_it_goes_over_ssh_asks_for_the_server_and_says_which_host_is_which() {
    let world = Known::empty();
    let mut answers = Answers::default();
    answers.put(field::NAME, "prod");
    answers.put(field::HOW, "url");
    answers.put(field::URL, "postgres://app@127.0.0.1/orders");
    answers.put(field::ROUTE, "keyring");
    answers.put(field::REACH, "ssh");

    let Next::Ask(step) = Job::DbAdd.next(&answers, &world) else {
        panic!("the server was not asked for");
    };
    assert_eq!(step.field, field::SSH_HOST);
    let How::Type { help, .. } = &step.how else {
        panic!("a server is typed, not picked");
    };
    assert!(
        help.contains("THAT machine sees"),
        "the one sentence everybody needs is not under the question: {help}"
    );

    // The passphrase question comes after the key, and its first answer is the agent —
    // which is the documented default and the one where sloop holds no secret at all.
    answers.put(field::SSH_HOST, "bastion.internal");
    answers.put(field::SSH_PORT, "");
    answers.put(field::SSH_USER, "deploy");
    answers.put(field::SSH_IDENTITY, "");

    let Next::Ask(step) = Job::DbAdd.next(&answers, &world) else {
        panic!("the passphrase route was not asked for");
    };
    assert_eq!(step.field, field::SSH_ROUTE);
    let How::Pick { values, .. } = &step.how else {
        panic!("it has to be a list");
    };
    assert_eq!(values.first().map(String::as_str), Some("agent"));
}

/// A passphrase route that holds a value asks where the value is, and one that does not
/// asks nothing more — the same shape the database password has.
#[test]
fn only_a_passphrase_route_that_needs_a_value_asks_for_one() {
    let world = Known::empty();
    let mut answers = Answers::default();
    answers.put(field::NAME, "prod");
    answers.put(field::HOW, "url");
    answers.put(field::URL, "postgres://app@127.0.0.1/orders");
    answers.put(field::ROUTE, "keyring");
    answers.put(field::REACH, "ssh");
    answers.put(field::SSH_HOST, "bastion.internal");
    answers.put(field::SSH_PORT, "");
    answers.put(field::SSH_USER, "");
    answers.put(field::SSH_IDENTITY, "");

    for (route, wanted) in [
        ("env", Some(field::SSH_ENV)),
        ("command", Some(field::SSH_FROM_COMMAND)),
        ("agent", None),
        ("keyring", None),
        ("file", None),
    ] {
        let mut asking = answers.clone();
        asking.put(field::SSH_ROUTE, route);

        let Next::Ask(step) = Job::DbAdd.next(&asking, &world) else {
            panic!("{route} ended the plan early");
        };
        match wanted {
            Some(field) => assert_eq!(step.field, field, "{route}"),
            // Nothing more about SSH: the next question is the last one `db add` asks.
            None => assert_eq!(step.field, field::TEST, "{route}"),
        }
    }
}

/// `db edit` can change how a database is reached, and that is its own detail rather than
/// something smuggled into "the server" — which is the database's address, not the tunnel's.
#[test]
fn editing_offers_how_it_is_reached_as_a_detail_of_its_own() {
    let world = Known::with(&["prod"]);
    let mut answers = Answers::default();
    answers.put(field::NAME, "prod");
    answers.put(field::DETAIL, "reach");

    let Next::Ask(step) = Job::DbEdit.next(&answers, &world) else {
        panic!("changing the reach asked nothing");
    };
    assert_eq!(step.field, field::REACH);
}
