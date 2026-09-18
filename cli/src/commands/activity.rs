//! `sloop service activity` — what the service has recorded, on a screen — `R27`.
//!
//! **Every number says what it is.** Rows the server counted, and a size on disk. Nothing here
//! is bytes on the wire, because per-database bytes on the wire do not exist, and the line that
//! says so is printed rather than left to a manual.
//!
//! **A period with no readings reads as having none.** `R27`'s *Done when*, and the reason each
//! window carries its count of readings: an idle database and an unwatched one are different
//! facts and this screen never draws them the same.
//!
//! **Local time throughout**, including which day "today" is — see
//! [`crate::service::activity::read`].

use crate::backup::stamp::Stamp;
use crate::commands::backup::{describe_bytes, plural};
use crate::exit::Exit;
use crate::failure::Outcome;
use crate::registry::locations::Locations;
use crate::registry::store::Store;
use crate::service::activity::{self, Activity, Recorded, Window};
use crate::style;

/// Show it, for a command line that has to work out where the store is.
pub fn run(locations: &Locations) -> Outcome<Exit> {
    show(&crate::registry::adopt::global(locations)?)
}

/// The same, for the menu, which settled where the store is when the session started.
pub fn show(global: &std::path::Path) -> Outcome<Exit> {
    let recorded = activity::read(&Store::require(global)?)?;

    crate::report::result(as_json(&recorded));
    say(&recorded);
    Ok(Exit::Success)
}

/// The whole screen.
fn say(recorded: &Recorded) {
    crate::say!(
        "{} {}",
        style::heading("Activity"),
        style::dim(&match recorded.databases.len() {
            0 => String::from(
                "nothing is attached — `sloop service attach <name>` attaches a \
                               database"
            ),
            1 => String::from("1 database attached"),
            many => format!("{many} databases attached"),
        })
    );

    if recorded.databases.is_empty() {
        return;
    }

    for database in &recorded.databases {
        crate::say!("");
        one(database);
    }

    crate::say!("");
    crate::say!(
        "  {}",
        style::dim(
            "rows are what the server counted, not bytes on the wire — no engine reports \
             bytes per database"
        )
    );
    crate::say!(
        "  {}",
        style::dim(&match recorded.last_seen {
            Some(stamp) => format!(
                "the service last read this list at {}",
                stamp.local().readable()
            ),
            None => String::from(
                "no service has ever read this list — `sloop service install` starts one"
            ),
        })
    );

    if !recorded.detached_with_history.is_empty() {
        crate::say!(
            "  {}",
            style::dim(&format!(
                "still recorded but no longer attached: {}",
                recorded.detached_with_history.join(", ")
            ))
        );
    }
}

/// One database, and its three windows.
fn one(database: &Activity) {
    crate::say!("{}", style::paint(&database.label));

    if !database.ever_watched() {
        crate::say!(
            "  {}",
            style::dim(&format!(
                "nothing recorded yet — attached {}",
                Stamp::from_unix_seconds(database.attached_at)
                    .local()
                    .readable()
            ))
        );
        crate::say!(
            "  {}",
            style::dim(match database.seen_at {
                Some(_) => "the service has picked it up, so the first readings are coming",
                None => "no running service has picked it up yet",
            })
        );
        return;
    }

    // **Padded to one width, because these four are read as a column.** A ragged left edge
    // makes somebody's eye do work the screen should have done.
    crate::say!("  {} {}", style::label("Today  "), window(&database.today));
    crate::say!("  {} {}", style::label("7 days "), window(&database.week));
    crate::say!("  {} {}", style::label("30 days"), window(&database.month));

    crate::say!(
        "  {} {}",
        style::label("Size   "),
        match (database.size_bytes, database.size_taken_at()) {
            (Some(bytes), Some(when)) => format!(
                "{} {}",
                describe_bytes(u64::try_from(bytes).unwrap_or(0)),
                style::dim(&format!("on disk, read {}", when.local().readable()))
            ),
            _ => style::dim("not recorded yet"),
        }
    );
}

/// One window, as one phrase.
///
/// **`R27`'s `Done when` lives in the first branch.** No readings means nobody was watching,
/// and that is said in words — a zero here would be a claim that nothing happened.
fn window(window: &Window) -> String {
    if !window.watched() {
        return style::dim("no readings in this period");
    }

    format!(
        "{} rows in · {} out {}",
        grouped(window.rows_in),
        grouped(window.rows_out),
        style::dim(&format!(
            "· {}",
            plural(u64::try_from(window.readings).unwrap_or(0), "reading")
        ))
    )
}

/// `1,250,000` — a count somebody can read at a glance.
///
/// **Grouped rather than shortened.** `1.2M` is easier to scan and loses the difference
/// between 1,200,000 and 1,249,999, which on a screen somebody is using to decide whether a
/// database is being hammered is the difference they came for. One format for every size, so
/// two lines of a column can be compared by eye.
pub fn grouped(count: i64) -> String {
    let digits = count.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);

    if count < 0 {
        out.push('-');
    }
    for (seen, digit) in digits.chars().enumerate() {
        if seen > 0 && (digits.len() - seen).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }

    out
}

/// The same thing, for a script.
///
/// **Every window says how many readings it had**, so a reader can tell an idle database from
/// an unwatched one without parsing a sentence — which is the same distinction the screen makes
/// in words.
fn as_json(recorded: &Recorded) -> serde_json::Value {
    let window = |window: &Window| {
        serde_json::json!({
            "rows_in": window.rows_in,
            "rows_out": window.rows_out,
            "readings": window.readings,
            "watched": window.watched(),
        })
    };

    serde_json::json!({
        // Said in the document too, so nothing downstream has to guess what the numbers are.
        "measures": "rows counted by the server, and size on disk. Never bytes on the wire.",
        "last_seen": recorded.last_seen.map(|stamp| stamp.local().iso()),
        "detached_with_history": recorded.detached_with_history,
        "databases": recorded
            .databases
            .iter()
            .map(|database| serde_json::json!({
                "label": database.label,
                "today": window(&database.today),
                "week": window(&database.week),
                "month": window(&database.month),
                "size_bytes": database.size_bytes,
                "size_at": database.size_taken_at().map(|stamp| stamp.local().iso()),
                "attached_at": Stamp::from_unix_seconds(database.attached_at).local().iso(),
                "seen_at": database.seen_at.map(|at| Stamp::from_unix_seconds(at).local().iso()),
            }))
            .collect::<Vec<_>>(),
    })
}
