//! What the builder produces, in each engine's spelling.
//!
//! **The statement is the whole contract.** Everything above it is a list somebody pointed
//! at; everything below it is a client program running what it was handed. If the text is
//! right the query is right, and these check the text against both engines without either
//! one being installed.

use super::{Built, Column, Condition, Join, Joiner, Operator, Wanted, escape_wildcards};
use crate::engine::{Adapter, Engine, ForeignKey, Table, adapter_for};

fn table(name: &str) -> Table {
    Table {
        schema: "public".to_owned(),
        name: name.to_owned(),
    }
}

fn column(table_name: &str, name: &str) -> Column {
    Column {
        table: table(table_name),
        name: name.to_owned(),
    }
}

fn postgres() -> Box<dyn Adapter> {
    adapter_for(Engine::Postgres)
}

fn mysql() -> Box<dyn Adapter> {
    adapter_for(Engine::Mysql)
}

/// Picking a table and ticking `All` is `select *`, and nothing else.
#[test]
fn a_table_and_all_of_it_is_select_star() {
    let built = Built::everything_in(table("customer"));
    assert_eq!(
        built.sql(&*postgres(), 0),
        "SELECT * FROM \"public\".\"customer\" LIMIT 201 OFFSET 0"
    );
}

/// **MySQL has no schema below the database**, so a table there is named on its own — a
/// qualified name would be a cross-database reference the connected user may not hold.
#[test]
fn each_engine_names_a_table_its_own_way() {
    let built = Built::everything_in(table("customer"));
    assert!(built.sql(&*mysql(), 0).contains("FROM `customer`"));
    assert!(!built.sql(&*mysql(), 0).contains("public"));
}

/// Ticking three columns is those three, in the order they were offered.
#[test]
fn ticking_three_columns_asks_for_those_three() {
    let mut built = Built::everything_in(table("customer"));
    built.columns = Wanted::These(vec![
        column("customer", "id"),
        column("customer", "name"),
        column("customer", "city"),
    ]);

    assert_eq!(
        built.sql(&*postgres(), 0),
        "SELECT \"public\".\"customer\".\"id\", \"public\".\"customer\".\"name\", \
         \"public\".\"customer\".\"city\" FROM \"public\".\"customer\" LIMIT 201 OFFSET 0"
    );
}

/// **`All` stops being `*` the moment something is joined in.** Two tables with an `id`
/// each would otherwise come back as two columns called `id`, and a grid with two
/// identically-named columns is a grid nobody can read.
#[test]
fn all_of_a_join_names_every_column_rather_than_starring() {
    let mut built = Built::everything_in(table("customer"));
    built.joins = vec![Join {
        table: table("order"),
        on: vec![(column("customer", "id"), column("order", "customer_id"))],
    }];

    let sql = built.sql(&*postgres(), 0);
    assert!(
        sql.contains("SELECT \"public\".\"customer\".*, \"public\".\"order\".*"),
        "{sql}"
    );
    assert!(!sql.contains("SELECT *"), "{sql}");
}

/// A named column in a join is aliased to the label the screen already used, so the grid's
/// headings say which table each column came from.
#[test]
fn a_joined_column_carries_the_label_the_screen_showed() {
    let mut built = Built::everything_in(table("customer"));
    built.joins = vec![Join {
        table: table("order"),
        on: vec![(column("customer", "id"), column("order", "customer_id"))],
    }];
    built.columns = Wanted::These(vec![column("customer", "name"), column("order", "total")]);

    let sql = built.sql(&*postgres(), 0);
    assert!(sql.contains("AS \"customer.name\""), "{sql}");
    assert!(sql.contains("AS \"order.total\""), "{sql}");
}

/// **The `Done when`'s first clause, as text**: a two-table join with a condition on each
/// side, with no SQL typed but the two values.
#[test]
fn a_two_table_join_with_a_condition_on_each_side() {
    let built = Built {
        from: table("customer"),
        columns: Wanted::These(vec![column("customer", "name"), column("order", "total")]),
        joins: vec![Join {
            table: table("order"),
            on: vec![(column("customer", "id"), column("order", "customer_id"))],
        }],
        conditions: vec![
            Condition {
                column: column("customer", "city"),
                operator: Operator::Is,
                value: "London".to_owned(),
            },
            Condition {
                column: column("order", "total"),
                operator: Operator::Above,
                value: "50".to_owned(),
            },
        ],
        joiner: Joiner::All,
    };

    assert_eq!(
        built.sql(&*postgres(), 0),
        "SELECT \"public\".\"customer\".\"name\" AS \"customer.name\", \
         \"public\".\"order\".\"total\" AS \"order.total\" \
         FROM \"public\".\"customer\" \
         LEFT JOIN \"public\".\"order\" \
         ON \"public\".\"customer\".\"id\" = \"public\".\"order\".\"customer_id\" \
         WHERE \"public\".\"customer\".\"city\" = 'London' \
         AND \"public\".\"order\".\"total\" > '50' \
         LIMIT 201 OFFSET 0"
    );
}

/// `any of them` is `OR`, and it is a chosen word rather than a typed one.
#[test]
fn the_joiner_is_the_word_that_was_chosen() {
    let mut built = Built::everything_in(table("customer"));
    built.conditions = vec![
        Condition {
            column: column("customer", "city"),
            operator: Operator::Is,
            value: "London".to_owned(),
        },
        Condition {
            column: column("customer", "city"),
            operator: Operator::Is,
            value: "Paris".to_owned(),
        },
    ];
    built.joiner = Joiner::Any;

    assert!(
        built.sql(&*postgres(), 0).contains("' OR "),
        "{}",
        built.sql(&*postgres(), 0)
    );
}

/// The two operators that ask about the absence of a value take none.
#[test]
fn asking_whether_something_is_there_needs_no_value() {
    for (operator, expected) in [
        (Operator::IsEmpty, "IS NULL"),
        (Operator::IsNotEmpty, "IS NOT NULL"),
    ] {
        assert!(!operator.takes_a_value());
        let mut built = Built::everything_in(table("customer"));
        built.conditions = vec![Condition {
            column: column("customer", "city"),
            operator,
            value: String::new(),
        }];
        assert!(built.sql(&*postgres(), 0).contains(expected));
    }
}

/// **A value is the one thing anybody types, so it is the one thing that is escaped.**
/// The read-only transaction is what makes a value that got through harmless; this is what
/// stops one getting through.
#[test]
fn a_value_with_a_quote_in_it_cannot_close_the_literal() {
    let mut built = Built::everything_in(table("customer"));
    built.conditions = vec![Condition {
        column: column("customer", "name"),
        operator: Operator::Is,
        value: "o'brien'; DROP TABLE customer; --".to_owned(),
    }];

    let sql = built.sql(&*postgres(), 0);
    assert!(
        sql.contains("'o''brien''; DROP TABLE customer; --'"),
        "{sql}"
    );
    // The apostrophes are doubled, so every one of them is inside the literal: the statement
    // has exactly the quotes it started with plus the pair around the value.
    assert_eq!(sql.matches('\'').count() % 2, 0, "{sql}");
}

/// **Every wildcard is one sloop added.** A search for `100%` means those four characters,
/// not "anything beginning with 100".
#[test]
fn a_percent_in_a_search_is_looked_for_rather_than_obeyed() {
    assert_eq!(escape_wildcards("100%"), "100\\%");
    assert_eq!(escape_wildcards("a_b"), "a\\_b");
    assert_eq!(escape_wildcards("back\\slash"), "back\\\\slash");

    let mut built = Built::everything_in(table("customer"));
    built.conditions = vec![Condition {
        column: column("customer", "name"),
        operator: Operator::Contains,
        value: "100%".to_owned(),
    }];
    let sql = built.sql(&*postgres(), 0);
    assert!(sql.contains("LIKE '%100\\%%' ESCAPE"), "{sql}");
}

/// Paging is in the statement, and it asks for one row past the page so that *"is there
/// another"* costs nothing to answer.
#[test]
fn every_page_is_a_limit_and_an_offset() {
    let built = Built::everything_in(table("customer"));
    assert!(built.sql(&*postgres(), 0).ends_with("LIMIT 201 OFFSET 0"));
    assert!(built.sql(&*postgres(), 1).ends_with("LIMIT 201 OFFSET 200"));
    assert!(
        built
            .sql(&*postgres(), 7)
            .ends_with("LIMIT 201 OFFSET 1400")
    );
}

/// A key points one way; both ways are offered, because a person wants either.
#[test]
fn a_foreign_key_is_offered_from_both_ends() {
    let key = ForeignKey {
        from: table("order"),
        columns: vec!["customer_id".to_owned()],
        to: table("customer"),
        to_columns: vec!["id".to_owned()],
    };

    let forwards = Join::from_key(&key);
    assert_eq!(forwards.table, table("customer"));
    assert_eq!(
        forwards.on,
        vec![(column("order", "customer_id"), column("customer", "id"))]
    );

    let backwards = Join::from_key_backwards(&key);
    assert_eq!(backwards.table, table("order"));
    assert_eq!(
        backwards.on,
        vec![(column("customer", "id"), column("order", "customer_id"))]
    );
}

/// A two-column key lines its two lists up rather than crossing them.
#[test]
fn a_key_of_two_columns_pairs_them_in_order() {
    let key = ForeignKey {
        from: table("line"),
        columns: vec!["shop".to_owned(), "order_id".to_owned()],
        to: table("order"),
        to_columns: vec!["shop".to_owned(), "id".to_owned()],
    };

    assert_eq!(
        Join::from_key(&key).on,
        vec![
            (column("line", "shop"), column("order", "shop")),
            (column("line", "order_id"), column("order", "id")),
        ]
    );
}

/// Every operator reads as a person would say it, and none of them is SQL.
#[test]
fn no_operator_on_the_list_is_spelled_the_way_sql_spells_it() {
    for operator in Operator::ALL {
        let label = operator.label();
        assert!(
            label
                .chars()
                .all(|letter| letter.is_ascii_lowercase() || letter == ' '),
            "{label} is not plain language"
        );
    }
}
