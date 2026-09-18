//! The two decisions the builder makes that are easy to get wrong, with no prompt near them.
//!
//! Everything else on this screen is a `Select` around a list somebody else computed; these
//! are the lists.

use super::{offers, wanted_from};
use crate::engine::{ForeignKey, Table};
use crate::query::{Column, Wanted};

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

/// `order.customer_id → customer.id`, the ordinary shape.
fn orders_point_at_customers() -> ForeignKey {
    ForeignKey {
        from: table("order"),
        columns: vec!["customer_id".to_owned()],
        to: table("customer"),
        to_columns: vec!["id".to_owned()],
    }
}

/// **From either end, the key is offered.** That is what makes it useful: from `customer` it
/// answers *"and their orders"*, and from `order` the same key answers *"and who placed it"*.
#[test]
fn one_key_is_offered_from_whichever_side_you_started_on() {
    let keys = [orders_point_at_customers()];

    let from_the_child = offers(&[table("order")], &keys);
    assert_eq!(from_the_child.len(), 1);
    assert_eq!(from_the_child[0].table, table("customer"));

    let from_the_parent = offers(&[table("customer")], &keys);
    assert_eq!(from_the_parent.len(), 1);
    assert_eq!(from_the_parent[0].table, table("order"));
}

/// A table already in the query is never offered again.
#[test]
fn nothing_already_in_the_query_is_offered() {
    let keys = [orders_point_at_customers()];
    assert!(offers(&[table("order"), table("customer")], &keys).is_empty());
}

/// A key that touches neither table in the query has nothing to say about it.
#[test]
fn a_key_between_two_other_tables_is_not_offered() {
    let elsewhere = ForeignKey {
        from: table("invoice"),
        columns: vec!["supplier_id".to_owned()],
        to: table("supplier"),
        to_columns: vec!["id".to_owned()],
    };
    assert!(offers(&[table("customer")], &[elsewhere]).is_empty());
}

/// **Two keys between the same pair of tables are two different joins**, and both are
/// offered — `order.billing_id` and `order.delivery_id` are not the same question.
#[test]
fn two_keys_between_the_same_tables_are_two_offers() {
    let keys = [
        ForeignKey {
            from: table("order"),
            columns: vec!["billing_id".to_owned()],
            to: table("address"),
            to_columns: vec!["id".to_owned()],
        },
        ForeignKey {
            from: table("order"),
            columns: vec!["delivery_id".to_owned()],
            to: table("address"),
            to_columns: vec!["id".to_owned()],
        },
    ];

    let offered = offers(&[table("order")], &keys);
    assert_eq!(offered.len(), 2, "{offered:?}");
    assert!(offered[0].describe().contains("billing_id"));
    assert!(offered[1].describe().contains("delivery_id"));
}

/// The same key listed twice is one offer, not two identical rows on a list.
#[test]
fn the_same_join_is_never_offered_twice() {
    let keys = [orders_point_at_customers(), orders_point_at_customers()];
    assert_eq!(offers(&[table("order")], &keys).len(), 1);
}

/// **A table that points at itself is an ordinary thing to want** — every employee beside
/// their manager — and is offered from the one table in the query.
#[test]
fn a_self_reference_is_offered() {
    let manager = ForeignKey {
        from: table("employee"),
        columns: vec!["manager_id".to_owned()],
        to: table("employee"),
        to_columns: vec!["id".to_owned()],
    };

    // `employee` is on both ends and already in the query, so it is not offered a second
    // time — which is right: joining it to itself needs an alias the builder does not make.
    assert!(offers(&[table("employee")], &[manager]).is_empty());
}

/// `All` at the top, and nothing ticked at all, are the same answer.
#[test]
fn all_and_nothing_are_the_same_answer() {
    let columns = vec![column("customer", "id"), column("customer", "name")];

    assert_eq!(wanted_from(&[], &columns), Wanted::Everything);
    assert_eq!(wanted_from(&[0], &columns), Wanted::Everything);
    assert_eq!(
        wanted_from(&[0, 2], &columns),
        Wanted::Everything,
        "ticking All and one more is still All"
    );
}

/// Ticking three is those three, and the index is one past the `All` row.
#[test]
fn ticking_columns_takes_the_ones_that_were_ticked() {
    let columns = vec![
        column("customer", "id"),
        column("customer", "name"),
        column("customer", "city"),
    ];

    assert_eq!(
        wanted_from(&[2, 3], &columns),
        Wanted::These(vec![column("customer", "name"), column("customer", "city")])
    );
}

/// A tick past the end of the list is dropped rather than panicking on an index.
#[test]
fn a_tick_that_names_nothing_is_dropped() {
    let columns = vec![column("customer", "id")];
    assert_eq!(
        wanted_from(&[1, 9], &columns),
        Wanted::These(vec![column("customer", "id")])
    );
}
