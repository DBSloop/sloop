//! The order a merge goes in, which is the part of `sync` that has to be right before a
//! single row moves.
//!
//! **A wrong order is not a wrong answer, it is half a database.** A child loaded before its
//! parent fails on the foreign key, and by then the tables before it are already in — so the
//! sort and the cycle refusal are tested here, against shapes rather than a server, because
//! being right about them is cheap and being wrong about them is expensive.

use super::{describe, in_dependency_order, report};
use crate::engine::{Merged, Table, TableShape};
use crate::exit::Exit;

/// A table in the one schema these tests use.
fn table(name: &str) -> Table {
    Table {
        schema: "public".to_owned(),
        name: name.to_owned(),
    }
}

/// A shape with a primary key and the parents it points at.
fn shape(name: &str, parents: &[&str]) -> TableShape {
    TableShape {
        table: table(name),
        columns: vec!["id".to_owned(), "body".to_owned()],
        server_assigned: false,
        primary_key: vec!["id".to_owned()],
        references: parents.iter().map(|parent| table(parent)).collect(),
    }
}

/// The order that came back, as names.
fn names(shapes: &[TableShape]) -> Vec<String> {
    shapes
        .iter()
        .map(|shape| shape.table.name.clone())
        .collect()
}

/// Where each table ended up, so a test can say "before" without knowing the whole order.
fn at(shapes: &[TableShape], name: &str) -> usize {
    names(shapes)
        .iter()
        .position(|found| found == name)
        .unwrap_or_else(|| panic!("{name} is not in the order at all: {:?}", names(shapes)))
}

/// **Parents first**, however the tables happened to be listed.
#[test]
fn a_table_comes_after_everything_it_points_at() {
    let ordered = in_dependency_order(vec![
        shape("line_items", &["orders", "products"]),
        shape("orders", &["customers"]),
        shape("products", &[]),
        shape("customers", &[]),
    ])
    .expect("this set has an order");

    assert_eq!(ordered.len(), 4);
    assert!(at(&ordered, "customers") < at(&ordered, "orders"));
    assert!(at(&ordered, "orders") < at(&ordered, "line_items"));
    assert!(at(&ordered, "products") < at(&ordered, "line_items"));
}

/// **The same order twice.** Two runs against one database have to do the same thing in the
/// same sequence, or a log from one cannot be read against a log from the next.
#[test]
fn tables_with_nothing_between_them_come_back_alphabetically() {
    let ordered = in_dependency_order(vec![
        shape("zebra", &[]),
        shape("apple", &[]),
        shape("mango", &[]),
    ])
    .expect("nothing points at anything");

    assert_eq!(names(&ordered), ["apple", "mango", "zebra"]);
}

/// **A cycle is refused, and the message names the tables in it** — the `Done when` of R15.
#[test]
fn foreign_keys_that_point_at_each_other_are_refused_by_name() {
    let failure = in_dependency_order(vec![
        shape("alone", &[]),
        shape("chickens", &["eggs"]),
        shape("eggs", &["chickens"]),
    ])
    .expect_err("there is no order that loads those two");

    assert_eq!(failure.exit(), Exit::Usage);
    let said = failure.message();
    assert!(said.contains("public.chickens"), "{said}");
    assert!(said.contains("public.eggs"), "{said}");
    assert!(
        !said.contains("public.alone"),
        "a table that is not in the cycle must not be blamed for it: {said}"
    );
    assert!(
        failure
            .hint_text()
            .is_some_and(|hint| hint.contains("mirror")),
        "the refusal has to say what does work: {:?}",
        failure.hint_text()
    );
}

/// A table pointing at itself is an employee with a manager, not a cycle.
#[test]
fn a_table_that_points_at_itself_is_not_a_cycle() {
    let ordered =
        in_dependency_order(vec![shape("employees", &["employees"])]).expect("that is ordinary");

    assert_eq!(names(&ordered), ["employees"]);
}

/// A key pointing at something that is not being copied cannot constrain the order of the
/// things that are.
#[test]
fn a_reference_to_a_table_that_is_not_here_is_ignored() {
    let ordered = in_dependency_order(vec![
        shape("orders", &["customers"]),
        shape("notes", &["somewhere_else"]),
    ])
    .expect("one of those parents is not in this set");

    assert_eq!(ordered.len(), 2);
}

/// One merged table, as a run would have counted it.
fn merged(name: &str, loaded: u64, inserted: u64, kept: u64, before: u64) -> Merged {
    Merged {
        table: table(name),
        loaded,
        inserted,
        updated: loaded - inserted,
        kept,
        before,
        after: before + inserted,
    }
}

/// **The three numbers R15 is about, in one line.** Added, replaced, and the ones that stayed
/// because the source has no row for them.
#[test]
fn a_tables_line_says_what_arrived_and_what_stayed() {
    let said = describe(&merged("orders", 10, 4, 3, 6));

    assert!(said.contains("public.orders"), "{said}");
    assert!(said.contains("10 in"), "{said}");
    assert!(said.contains("4 new"), "{said}");
    assert!(said.contains("6 replaced"), "{said}");
    assert!(
        said.contains("3 kept that the source has no row for"),
        "the notification is the point of the command: {said}"
    );
}

/// Nothing to keep, nothing said about keeping — a line that reports zero of something is a
/// line somebody has to read to find out it says nothing.
#[test]
fn nothing_kept_is_not_mentioned() {
    let said = describe(&merged("orders", 10, 10, 0, 0));
    assert!(!said.contains("kept"), "{said}");
}

/// **Exit 6 when the arithmetic disagrees.** The destination has to end with the rows it
/// started with plus the ones that were new; anything else means something else was writing
/// to it while this ran.
#[test]
fn a_table_that_does_not_add_up_is_a_mismatch_rather_than_a_success() {
    assert_eq!(report(&[merged("orders", 10, 4, 3, 6)]), Exit::Success);

    let drifted = Merged {
        after: 99,
        ..merged("orders", 10, 4, 3, 6)
    };
    assert!(!drifted.adds_up());
    assert_eq!(report(&[drifted]), Exit::Mismatch);

    let said = describe(&Merged {
        after: 99,
        ..merged("orders", 10, 4, 3, 6)
    });
    assert!(said.contains("does not add up"), "{said}");
}

/// An empty merge is a success and not a mismatch: nothing was loaded, so nothing can
/// disagree.
#[test]
fn a_table_that_had_nothing_to_merge_still_adds_up() {
    assert_eq!(report(&[merged("orders", 0, 0, 5, 5)]), Exit::Success);
}
