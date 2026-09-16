//! `sloop init` — start a project registry beside the code it belongs to.
//!
//! Two things happen. A `.sloop` directory appears in the target directory, holding a
//! `.gitignore` that ignores everything including itself, so the whole thing is invisible
//! to git without a single line being added to the user's own `.gitignore`. And the
//! project is recorded under a name in the global store, so `-C <name>` reaches it from
//! anywhere on the disk.

use std::path::{Path, PathBuf};

use crate::failure::{Failure, Outcome};
use crate::registry::locations::PROJECT_DIR;
use crate::registry::projects;
use crate::style;
use crate::wordmark;

/// Ignores everything in `.sloop`, including this file.
///
/// The comment is for whoever opens it; git never sees it, because the `*` below covers
/// this file too. That is the whole trick: the directory disappears from `git status`
/// without the user's own `.gitignore` being touched.
const GITIGNORE: &str = "\
# sloop keeps this project's registry here, and none of it belongs in a repository.
# This file ignores everything beside it, itself included.
*
";

/// What a name ended up being, or why it did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Naming {
    /// The project answers to this name from anywhere.
    Registered(String),
    /// It already did, and still does.
    AlreadyMine(String),
    /// Another project got there first. This one is still reachable by path.
    Taken { name: String, other: PathBuf },
    /// The directory is called something that cannot be a filename in the index.
    Unusable { name: String },
    /// A drive root or similar: there was no directory name to take.
    Nameless,
}

/// What `init` found or made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Initialised {
    /// The directory the registry belongs to.
    pub project: PathBuf,
    /// The registry directory itself.
    pub registry: PathBuf,
    /// True when there was already one here.
    pub existed: bool,
    /// How the project is named in the global store.
    pub naming: Naming,
}

/// Create the registry, register the name, and say what happened.
pub fn run(target: &Path, global: &Path) -> Outcome<()> {
    let report = create(target, global)?;
    crate::report::result(serde_json::json!({
        "project": report.project.display().to_string(),
        "registry": report.registry.display().to_string(),
        "existed": report.existed,
    }));
    print(&report, global);
    Ok(())
}

/// Do the work. Kept apart from the printing so the behaviour can be checked without
/// reading a screen.
pub fn create(target: &Path, global: &Path) -> Outcome<Initialised> {
    if !target.is_dir() {
        return Err(
            Failure::usage(format!("{} is not a directory", target.display()))
                .hint("sloop init works on a directory that already exists"),
        );
    }

    let registry = target.join(PROJECT_DIR);
    if registry.exists() && !registry.is_dir() {
        return Err(Failure::usage(format!(
            "{} exists and is not a directory",
            registry.display()
        ))
        .hint("move it out of the way, then run sloop init again"));
    }

    let existed = registry.is_dir();
    std::fs::create_dir_all(&registry).map_err(|error| {
        Failure::usage(format!("could not create {}: {error}", registry.display()))
    })?;

    // Only written when it is missing. If someone has edited it, that was on purpose.
    let ignore = registry.join(".gitignore");
    if !ignore.exists() {
        std::fs::write(&ignore, GITIGNORE).map_err(|error| {
            Failure::usage(format!("could not write {}: {error}", ignore.display()))
        })?;
    }

    Ok(Initialised {
        naming: register(target, global),
        project: target.to_path_buf(),
        registry,
        existed,
    })
}

/// Record the project under its directory's name, without ever stealing a name that is
/// already pointing somewhere else.
fn register(target: &Path, global: &Path) -> Naming {
    let Some(name) = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
    else {
        return Naming::Nameless;
    };

    if projects::check_name(&name).is_err() {
        return Naming::Unusable { name };
    }

    match projects::read(global, &name) {
        Some(existing) if existing == target => Naming::AlreadyMine(name),
        Some(other) => Naming::Taken { name, other },
        None => match projects::write(global, &name, target) {
            Ok(()) => Naming::Registered(name),
            // The registry itself is made; a name is a convenience on top of it, and
            // failing to write one is not a reason to call the whole thing a failure.
            Err(_) => Naming::Unusable { name },
        },
    }
}

fn print(report: &Initialised, global: &Path) {
    let headline = if report.existed {
        "There was already a project registry here"
    } else {
        "Project registry created"
    };

    crate::say!();
    for line in wordmark::render(terminal_columns()).lines() {
        // One space, so the leftmost glyph of the block lands in the same column as
        // the text under it. The block is drawn flush left; everything else is not.
        crate::say!(" {line}");
    }
    crate::say!();
    crate::say!("  {}", style::heading(headline));
    crate::say!();
    crate::say!(
        "{}",
        row("registry", &report.registry.display().to_string())
    );
    for line in name_rows(&report.naming, &report.project) {
        crate::say!("{line}");
    }
    crate::say!("{}", row("global", &global.display().to_string()));
    crate::say!();
    crate::say!(
        "  {}",
        style::dim(&format!(
            "{PROJECT_DIR} ignores itself, so git will never see it."
        ))
    );
    crate::say!();
}

fn name_rows(naming: &Naming, project: &Path) -> Vec<String> {
    match naming {
        Naming::Registered(name) | Naming::AlreadyMine(name) => vec![
            row("name", name),
            note(&format!("reachable anywhere as: sloop -C {name}")),
        ],
        Naming::Taken { name, other } => vec![
            row("name", &format!("{name} is already {}", other.display())),
            note(&format!(
                "reach this one as: sloop -C {}",
                project.display()
            )),
        ],
        Naming::Unusable { name } => vec![
            row("name", &format!("{name} cannot be one")),
            note(&format!("reach it as: sloop -C {}", project.display())),
        ],
        Naming::Nameless => vec![note(&format!(
            "reach it as: sloop -C {}",
            project.display()
        ))],
    }
}

/// A label and its value, aligned. Padded before it is styled, so the invisible escape
/// bytes never count towards the column.
fn row(label: &str, value: &str) -> String {
    format!("  {}  {value}", style::label(&format!("{label:<8}")))
}

/// A quieter line under a row.
fn note(text: &str) -> String {
    format!("  {}  {}", " ".repeat(8), style::dim(text))
}

/// The terminal width, or `None` when there is no terminal — a pipe or a file, where the
/// full wordmark is right because nothing is going to wrap it.
fn terminal_columns() -> Option<usize> {
    terminal_size::terminal_size().map(|(terminal_size::Width(columns), _)| usize::from(columns))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{GITIGNORE, Naming, name_rows, note, row};

    #[test]
    fn the_ignore_file_ignores_everything_including_itself() {
        let lines: Vec<_> = GITIGNORE
            .lines()
            .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
            .collect();
        assert_eq!(lines, ["*"], "one rule, and it covers this file too");
    }

    #[test]
    fn a_row_pads_before_it_paints() {
        // The escapes are invisible, so padding after styling would leave the column
        // ragged by exactly the length of an ANSI sequence.
        let painted = row("name", "demo");
        let plain: String = strip(&painted);
        assert_eq!(plain, "  name      demo");
    }

    #[test]
    fn every_naming_still_tells_the_user_how_to_reach_the_project() {
        let project = PathBuf::from("/work/demo");
        let namings = [
            Naming::Registered("demo".to_owned()),
            Naming::AlreadyMine("demo".to_owned()),
            Naming::Taken {
                name: "demo".to_owned(),
                other: PathBuf::from("/other/demo"),
            },
            Naming::Unusable {
                name: "de:mo".to_owned(),
            },
            Naming::Nameless,
        ];

        for naming in namings {
            let rows = name_rows(&naming, &project);
            let text: String = strip(&rows.join("\n"));
            assert!(
                text.contains("sloop -C"),
                "{naming:?} left no way in: {text}"
            );
        }
    }

    #[test]
    fn a_note_lines_up_under_its_row() {
        assert_eq!(strip(&note("x")).len(), strip(&row("name", "x")).len());
    }

    fn strip(painted: &str) -> String {
        let mut plain = String::new();
        let mut rest = painted;
        while let Some(start) = rest.find('\x1b') {
            plain.push_str(&rest[..start]);
            let after = &rest[start..];
            let end = after.find('m').expect("an SGR sequence ends in m");
            rest = &after[end + 1..];
        }
        plain.push_str(rest);
        plain
    }
}
