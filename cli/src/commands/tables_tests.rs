//! Which tables `--table` names, and what it refuses to leave out.
//!
//! **The glob and the foreign keys are where being wrong is expensive.** A pattern that
//! matches one table too many on a scoped `mirror` drops a table nobody named, and a selection
//! missing a parent fails halfway through a load — so both are settled here, against shapes
//! rather than a server.

use super::{Selection, glob, matches};
use crate::engine::{Table, TableShape};
use crate::exit::Exit;

fn table(schema: &str, name: &str) -> Table {
    Table {
        schema: schema.to_owned(),
        name: name.to_owned(),
    }
}

/// A shape in `public`, with the parents it points at.
fn shape(name: &str, parents: &[&str]) -> TableShape {
    TableShape {
        table: table("public", name),
        columns: vec!["id".to_owned()],
        server_assigned: false,
        primary_key: vec!["id".to_owned()],
        references: parents
            .iter()
            .map(|parent| table("public", parent))
            .collect(),
    }
}

/// The fixture: a chain three deep, plus two unrelated tables.
fn source() -> Vec<TableShape> {
    vec![
        shape("customers", &[]),
        shape("orders", &["customers"]),
        shape("line_items", &["orders"]),
        shape("audit_logins", &[]),
        shape("audit_changes", &[]),
    ]
}

/// What came back, by name.
fn names(chosen: &[TableShape]) -> Vec<String> {
    chosen
        .iter()
        .map(|shape| shape.table.name.clone())
        .collect()
}

/// Nothing named means everything, and the list comes back as it went in.
#[test]
fn no_pattern_is_every_table() {
    let all = Selection {
        patterns: &[],
        with_references: false,
    };
    assert!(all.is_everything());
    assert_eq!(names(&all.choose(&source()).unwrap()).len(), 5);
}

/// **A glob, not a regex.** `audit_*` is what people mean.
#[test]
fn a_glob_picks_the_tables_it_looks_like_it_picks() {
    let patterns = vec!["audit_*".to_owned()];
    let chosen = Selection {
        patterns: &patterns,
        with_references: false,
    }
    .choose(&source())
    .expect("both audit tables stand alone");

    // The order is the source's own; putting them in load order is the caller's job.
    assert_eq!(names(&chosen), ["audit_logins", "audit_changes"]);
}

/// Repeatable, and the same table named twice is still one table.
#[test]
fn patterns_add_up_without_doubling() {
    let patterns = vec![
        "customers".to_owned(),
        "audit_*".to_owned(),
        "customers".to_owned(),
    ];
    let chosen = Selection {
        patterns: &patterns,
        with_references: false,
    }
    .choose(&source())
    .expect("none of those has a parent");

    assert_eq!(
        names(&chosen),
        ["customers", "audit_logins", "audit_changes"]
    );
}

/// A schema in front of the pattern narrows it; without one, any schema matches.
#[test]
fn a_pattern_with_a_dot_names_a_schema() {
    let elsewhere = vec![
        shape("orders", &[]),
        TableShape {
            table: table("archive", "orders"),
            ..shape("orders", &[])
        },
    ];

    let qualified = vec!["archive.orders".to_owned()];
    let chosen = Selection {
        patterns: &qualified,
        with_references: false,
    }
    .choose(&elsewhere)
    .expect("one of the two");
    assert_eq!(chosen.len(), 1);
    assert_eq!(chosen[0].table.schema, "archive");

    let bare = vec!["orders".to_owned()];
    assert_eq!(
        Selection {
            patterns: &bare,
            with_references: false,
        }
        .choose(&elsewhere)
        .expect("both, one per schema")
        .len(),
        2
    );
}

/// **A typo must not quietly copy nothing.** The run would succeed and say it did nothing.
#[test]
fn a_pattern_that_matches_nothing_is_a_usage_error_naming_it() {
    let patterns = vec!["audit_*".to_owned(), "ordrs".to_owned()];
    let failure = Selection {
        patterns: &patterns,
        with_references: false,
    }
    .choose(&source())
    .expect_err("nothing is called ordrs");

    assert_eq!(failure.exit(), Exit::Usage);
    assert!(
        failure.message().contains("--table ordrs"),
        "{}",
        failure.message()
    );
}

/// **The `Done when` of R15a.** A child named alone is refused, naming its parents.
#[test]
fn a_child_without_its_parents_is_refused_and_they_are_named() {
    let patterns = vec!["orders".to_owned()];
    let failure = Selection {
        patterns: &patterns,
        with_references: false,
    }
    .choose(&source())
    .expect_err("orders points at customers");

    assert_eq!(failure.exit(), Exit::Usage);
    assert!(
        failure.message().contains("public.customers"),
        "{}",
        failure.message()
    );
    assert!(
        failure
            .hint_text()
            .is_some_and(|hint| hint.contains("--with-references")),
        "{:?}",
        failure.hint_text()
    );
}

/// And `--with-references` completes the same run, the whole way up the chain.
#[test]
fn with_references_follows_the_chain_to_the_top() {
    let patterns = vec!["line_items".to_owned()];
    let chosen = Selection {
        patterns: &patterns,
        with_references: true,
    }
    .choose(&source())
    .expect("the parents come too");

    // line_items -> orders -> customers, two levels up rather than one.
    assert_eq!(names(&chosen), ["customers", "orders", "line_items"]);
}

/// A parent that is already named is not missing, so nothing is refused.
#[test]
fn naming_the_parents_yourself_is_enough() {
    let patterns = vec!["orders".to_owned(), "customers".to_owned()];
    let chosen = Selection {
        patterns: &patterns,
        with_references: false,
    }
    .choose(&source())
    .expect("both ends are named");

    assert_eq!(names(&chosen), ["customers", "orders"]);
}

/// A table pointing outside the source cannot constrain a selection inside it.
#[test]
fn a_parent_that_is_not_in_the_source_is_not_demanded() {
    let orphan = vec![TableShape {
        references: vec![table("public", "somewhere_else")],
        ..shape("orders", &[])
    }];
    let patterns = vec!["orders".to_owned()];

    Selection {
        patterns: &patterns,
        with_references: false,
    }
    .choose(&orphan)
    .expect("that parent is not here to be named");
}

/// The matcher itself: `*`, `?`, case, and the anchoring that keeps a glob from being a
/// substring search.
#[test]
fn the_glob_matches_what_a_person_would_expect() {
    assert!(glob("audit_*", "audit_logins"));
    assert!(glob("*_logins", "audit_logins"));
    assert!(glob("*audit*", "the_audit_table"));
    assert!(glob("audit_?", "audit_1"));
    assert!(
        glob("orders", "ORDERS"),
        "names are matched case-insensitively"
    );
    assert!(glob("*", "anything"));
    assert!(glob("a*b*c", "axxbyyc"));

    assert!(
        !glob("audit", "audit_logins"),
        "a glob is anchored at both ends"
    );
    assert!(!glob("audit_?", "audit_12"));
    assert!(!glob("audit_*", "logins_audit"));
    assert!(!glob("a*b*c", "axxbyy"));

    // The pathological one: this has to finish, not hang.
    assert!(!glob("a*a*a*a*a*b", &"a".repeat(64)));
}

/// A pattern is matched against the parts it names, and only those.
#[test]
fn a_bare_pattern_never_matches_a_schema_by_accident() {
    assert!(matches("orders", &table("public", "orders")));
    assert!(!matches("public", &table("public", "orders")));
    assert!(matches("pub*.ord*", &table("public", "orders")));
}
