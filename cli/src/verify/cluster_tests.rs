//! Verification against two real databases: a copy taken for the test, then broken in
//! each of the ways that matter, on a cluster created and destroyed here.
//!
//! The comparison rules are unit-tested next door with numbers made up on the spot. This
//! file is about the other half — that the numbers being compared are the ones a real
//! PostgreSQL really holds, and that `n_live_tup` behaves the way the fast mode is written
//! to assume rather than the way it is documented to.

use super::{Comparison, Finding, Mode, Side};
use crate::engine::cluster_tests::{ALPHA_PASSWORD, BETA_PASSWORD, Cluster, skip};
use crate::engine::postgres::Postgres;
use crate::engine::{Adapter, Target};
use crate::exit::Exit;
use crate::secret::Secret;

/// Count both ends the way `mode` says to, and print what a person would be shown.
///
/// The printing is not decoration: [`super::print`] is the only user-visible half of this
/// module, and a report nothing ever renders is a report that is broken the first time
/// something asks for it. `cargo test` swallows it unless `--nocapture` is passed, which
/// is also how it gets looked at by eye.
fn compare(
    adapter: &dyn Adapter,
    source: &Target<'_>,
    destination: &Target<'_>,
    mode: Mode,
) -> Comparison {
    let left = Side::counted(adapter, source, mode).expect("counting the source");
    let right = Side::counted(adapter, destination, mode).expect("counting the destination");
    let comparison = Comparison::of(mode, &left, &right);
    super::print(&comparison);
    comparison
}

fn finding(comparison: &Comparison, table: &str) -> Finding {
    comparison
        .rows
        .iter()
        .find(|row| row.table == table)
        .unwrap_or_else(|| panic!("{table} is not in the comparison: {:?}", comparison.rows))
        .finding
}

/// A copy that is demonstrably complete, glanced at rather than counted.
///
/// A restored table's statistics are whatever the collector has got round to, which can be
/// nothing at all for the first half-second and can equally be the right number. Either
/// answer is fine; what must never happen is a perfect copy exiting `6` because a statistic
/// had not caught up.
fn an_estimate_never_condemns_this_copy(
    adapter: &dyn Adapter,
    from: &Target<'_>,
    into: &Target<'_>,
) {
    let glanced = compare(adapter, from, into, Mode::Fast);
    assert!(
        glanced.landed(),
        "an estimate failed a copy that is demonstrably complete: {:?}",
        glanced.rows
    );
    assert_eq!(glanced.exit(), Exit::Success);

    let said = glanced.describe().join("\n");
    assert!(said.contains("estimates"), "{said}");
    assert!(said.contains("not a proof"), "{said}");
}

/// R10's "Done when", against a copy that really was taken: drift described, a table that
/// did not arrive failed, and an estimate never doing either.
#[test]
fn a_real_copy_is_verified_and_each_way_of_breaking_it_is_told_apart() {
    let Some(cluster) = Cluster::start("verify") else {
        skip("initdb is not on this machine");
        return;
    };

    let adapter = Postgres::new(cluster.tools());
    let alpha = Secret::new(ALPHA_PASSWORD.to_owned());
    let beta = Secret::new(BETA_PASSWORD.to_owned());
    let source = cluster.database("source_db", "alpha");
    let destination = cluster.database("target_db", "beta");
    let from = source.target(&alpha);
    let into = destination.target(&beta);

    // --- take a real copy -------------------------------------------------------------
    let dump = cluster.root.join("verify.dump");
    adapter.dump(&from, &dump).expect("dumping the source");
    adapter.restore(&into, &dump).expect("restoring the copy");

    // --- what a command calls, in whatever mode this shell asked for --------------------
    // `against` reads `SLOOP_VERIFY` itself, which is the point of it. Asserted without
    // assuming what the variable holds, because a developer who has exported it should get
    // the run they asked for and not a failing test suite — and because a copy this fresh
    // passes either way: exact counts match, and an estimate never fails anything.
    let front_door = super::against(&adapter, &from, &into).expect("counting both sides");
    super::print(&front_door);
    assert_eq!(
        front_door.mode,
        Mode::from_environment(),
        "`against` is not honouring {}",
        super::VARIABLE
    );
    assert!(front_door.landed(), "{:?}", front_door.rows);

    // --- exact: it landed --------------------------------------------------------------
    // Asked for by name from here on, so that every assertion below is about the copy and
    // never about the environment the tests happen to be running in.
    let landed = compare(&adapter, &from, &into, Mode::Exact);
    assert!(
        landed.landed(),
        "a copy that was just taken did not verify: {:?}",
        landed.rows
    );
    assert_eq!(landed.exit(), Exit::Success);
    assert_eq!(
        finding(&landed, "app.widgets"),
        Finding::Matched { rows: 500 }
    );
    assert_eq!(
        finding(&landed, "public.notes"),
        Finding::Matched { rows: 7 }
    );

    // --- fast: the same copy, and an estimate is not allowed to condemn it -------------
    an_estimate_never_condemns_this_copy(&adapter, &from, &into);

    // --- rows written to the source afterwards are drift, not an error ------------------
    cluster
        .psql(
            "source_db",
            "INSERT INTO app.widgets (name) SELECT 'late ' || g FROM generate_series(1, 5) g;",
        )
        .expect("writing to the source");

    let drifted = compare(&adapter, &from, &into, Mode::Exact);
    assert_eq!(
        finding(&drifted, "app.widgets"),
        Finding::Drifted {
            source: 505,
            destination: 500
        }
    );
    assert!(drifted.landed(), "drift failed the run");
    assert_eq!(drifted.exit(), Exit::Success);
    assert!(
        drifted.describe().join("\n").contains("drift"),
        "{:?}",
        drifted.describe()
    );

    // --- a table emptied on the destination is a failure --------------------------------
    cluster
        .psql("target_db", "DELETE FROM public.notes;")
        .expect("emptying a table on the copy");

    let emptied = compare(&adapter, &from, &into, Mode::Exact);
    assert_eq!(
        finding(&emptied, "public.notes"),
        Finding::Empty { source: 7 }
    );
    assert!(!emptied.landed());
    assert_eq!(emptied.exit(), Exit::Mismatch);
    assert_eq!(emptied.exit().code(), 6);
    assert!(
        emptied.describe().join("\n").contains("nothing arrived"),
        "{:?}",
        emptied.describe()
    );
    // And the drift from a moment ago is still only drift, in the same report.
    assert!(matches!(
        finding(&emptied, "app.widgets"),
        Finding::Drifted { .. }
    ));

    // --- and so is a table that is not there at all -------------------------------------
    cluster
        // `CASCADE` because the fixture has a view over this table, and a view is not
        // something the comparison counts either way.
        .psql("target_db", "DROP TABLE app.widgets CASCADE;")
        .expect("dropping a table on the copy");

    let missing = compare(&adapter, &from, &into, Mode::Exact);
    assert_eq!(
        finding(&missing, "app.widgets"),
        Finding::Missing { source: 505 }
    );
    assert_eq!(missing.exit(), Exit::Mismatch);
    assert!(
        missing
            .describe()
            .join("\n")
            .contains("the table is not there"),
        "{:?}",
        missing.describe()
    );

    // A missing table is the one finding a statistic cannot explain away, so even a fast
    // run calls it: the catalogue is not an estimate.
    let missing_fast = compare(&adapter, &from, &into, Mode::Fast);
    assert_eq!(missing_fast.exit(), Exit::Mismatch);
}

/// The two modes list the same tables, which is what stops a fast run appearing to lose
/// one — and what makes the exact answer and the estimate comparable at all.
#[test]
fn both_modes_see_the_same_tables() {
    let Some(cluster) = Cluster::start("verify-tables") else {
        skip("initdb is not on this machine");
        return;
    };

    let adapter = Postgres::new(cluster.tools());
    let alpha = Secret::new(ALPHA_PASSWORD.to_owned());
    let source = cluster.database("source_db", "alpha");
    let target = source.target(&alpha);

    let named = |counts: &[crate::engine::TableCount]| -> Vec<String> {
        counts.iter().map(|count| count.table.to_string()).collect()
    };

    let exact = adapter.row_counts(&target).expect("counting");
    let estimated = adapter.estimated_row_counts(&target).expect("estimating");

    assert_eq!(named(&exact), named(&estimated));
    assert!(
        named(&exact).contains(&"app.widgets".to_owned()),
        "{:?}",
        named(&exact)
    );
    // A view is not a table in either mode.
    assert!(!named(&estimated).contains(&"app.recent".to_owned()));

    // The exact count is the truth whatever the statistics happen to say today.
    assert_eq!(
        exact
            .iter()
            .find(|count| count.table.to_string() == "app.widgets")
            .map(|count| count.rows),
        Some(500)
    );
}
