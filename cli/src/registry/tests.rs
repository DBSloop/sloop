//! Every branch of the resolution order, against a world that is a list rather than a
//! disk. The disk gets exercised end to end from `tests/registry.rs`; what matters here is
//! that no branch can quietly stop being reachable.

use std::path::{Path, PathBuf};

use super::{Qualified, Reason, Resolution, Scope, World, normalize, resolve};

/// A world made of three lists.
#[derive(Default)]
struct Fake {
    directories: Vec<PathBuf>,
    projects: Vec<PathBuf>,
    named: Vec<(String, PathBuf)>,
}

impl Fake {
    /// A directory that exists but holds no `.sloop`.
    fn dir(mut self, path: &str) -> Self {
        self.directories.push(PathBuf::from(path));
        self
    }

    /// A directory that exists and holds a `.sloop`.
    fn project(mut self, path: &str) -> Self {
        self.directories.push(PathBuf::from(path));
        self.projects.push(PathBuf::from(path));
        self
    }

    /// A name in the index, pointing somewhere.
    fn name(mut self, name: &str, path: &str) -> Self {
        self.named.push((name.to_owned(), PathBuf::from(path)));
        self
    }
}

impl World for Fake {
    fn is_directory(&self, path: &Path) -> bool {
        self.directories.iter().any(|known| known == path)
    }

    fn is_project(&self, dir: &Path) -> bool {
        self.projects.iter().any(|known| known == dir)
    }

    fn project_named(&self, name: &str) -> Option<PathBuf> {
        self.named
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, path)| path.clone())
    }
}

fn at(cwd: &str) -> PathBuf {
    PathBuf::from(cwd)
}

/// A project path with forward slashes, so one set of expectations covers every host.
fn project_of(resolution: &Resolution) -> Option<String> {
    resolution.project.as_deref().map(slashes)
}

fn slashes(path: &Path) -> String {
    path.to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

#[test]
fn global_wins_even_when_a_project_is_right_here() {
    let world = Fake::default().project("/work/demo");
    let resolved = resolve(&at("/work/demo"), &world, true, None, None).unwrap();

    assert_eq!(project_of(&resolved), None);
    assert_eq!(resolved.reason, Reason::GlobalFlag);
    assert_eq!(resolved.search_order(), [Scope::Global]);
}

#[test]
fn a_flag_pointing_at_a_project_directory_takes_it() {
    let world = Fake::default().project("/work/demo");
    let resolved = resolve(&at("/elsewhere"), &world, false, Some("/work/demo"), None).unwrap();

    assert_eq!(project_of(&resolved).as_deref(), Some("/work/demo"));
    assert_eq!(resolved.reason, Reason::Flag);
}

#[test]
fn a_flag_pointing_inside_a_project_walks_up_the_way_git_does() {
    let world = Fake::default()
        .project("/work/demo")
        .dir("/work/demo/src/deep");
    let resolved = resolve(
        &at("/elsewhere"),
        &world,
        false,
        Some("/work/demo/src/deep"),
        None,
    )
    .unwrap();

    assert_eq!(project_of(&resolved).as_deref(), Some("/work/demo"));
}

#[test]
fn a_relative_flag_is_read_against_the_working_directory() {
    let world = Fake::default().project("/work/other");
    let resolved = resolve(&at("/work/demo"), &world, false, Some("../other"), None).unwrap();

    assert_eq!(project_of(&resolved).as_deref(), Some("/work/other"));
}

#[test]
fn a_flag_that_is_a_registered_name_is_found_from_anywhere() {
    let world = Fake::default()
        .project("/work/demo")
        .name("demo", "/work/demo");
    let resolved = resolve(&at("/somewhere/else"), &world, false, Some("demo"), None).unwrap();

    assert_eq!(project_of(&resolved).as_deref(), Some("/work/demo"));
    assert_eq!(resolved.reason, Reason::Flag);
}

#[test]
fn a_visible_directory_beats_a_registered_name_of_the_same_spelling() {
    // `git -C demo` means the directory called demo. What the user can see wins, and the
    // resolution is printed back to them either way.
    let world = Fake::default()
        .project("/work/demo")
        .name("demo", "/far/away");
    let resolved = resolve(&at("/work"), &world, false, Some("demo"), None).unwrap();

    assert_eq!(project_of(&resolved).as_deref(), Some("/work/demo"));
}

#[test]
fn a_directory_with_no_project_above_it_falls_through_to_the_name() {
    let world = Fake::default()
        .dir("/work/plain")
        .name("plain", "/work/demo");
    let resolved = resolve(&at("/work"), &world, false, Some("plain"), None).unwrap();

    assert_eq!(project_of(&resolved).as_deref(), Some("/work/demo"));
}

#[test]
fn a_flag_that_is_neither_is_a_usage_error_naming_the_flag() {
    let world = Fake::default();
    let failure = resolve(&at("/work"), &world, false, Some("nope"), None).unwrap_err();

    assert_eq!(failure.exit().code(), 2);
}

#[test]
fn the_environment_variable_does_what_the_flag_does() {
    let world = Fake::default()
        .project("/work/demo")
        .name("demo", "/work/demo");
    let resolved = resolve(&at("/elsewhere"), &world, false, None, Some("demo")).unwrap();

    assert_eq!(project_of(&resolved).as_deref(), Some("/work/demo"));
    assert_eq!(resolved.reason, Reason::EnvVar);
}

#[test]
fn the_environment_variable_names_itself_when_it_is_wrong() {
    let world = Fake::default();
    let failure = resolve(&at("/work"), &world, false, None, Some("nope")).unwrap_err();

    assert_eq!(failure.exit().code(), 2);
}

#[test]
fn the_flag_beats_the_environment_variable() {
    let world = Fake::default()
        .project("/work/flag")
        .project("/work/env")
        .name("flag", "/work/flag")
        .name("env", "/work/env");
    let resolved = resolve(&at("/elsewhere"), &world, false, Some("flag"), Some("env")).unwrap();

    assert_eq!(project_of(&resolved).as_deref(), Some("/work/flag"));
    assert_eq!(resolved.reason, Reason::Flag);
}

#[test]
fn an_empty_flag_or_variable_is_how_a_shell_says_unset() {
    let world = Fake::default().project("/work/demo");

    for (flag, env) in [(Some(""), None), (None, Some("")), (Some(""), Some(""))] {
        let resolved = resolve(&at("/work/demo/src"), &world, false, flag, env).unwrap();
        assert_eq!(project_of(&resolved).as_deref(), Some("/work/demo"));
        assert_eq!(resolved.reason, Reason::WalkUp);
    }
}

#[test]
fn walking_up_finds_the_project_from_a_subdirectory() {
    let world = Fake::default().project("/work/demo");
    let resolved = resolve(&at("/work/demo/a/b/c"), &world, false, None, None).unwrap();

    assert_eq!(project_of(&resolved).as_deref(), Some("/work/demo"));
    assert_eq!(resolved.reason, Reason::WalkUp);
}

#[test]
fn walking_up_stops_at_the_nearest_one() {
    let world = Fake::default().project("/work").project("/work/demo");
    let resolved = resolve(&at("/work/demo/src"), &world, false, None, None).unwrap();

    assert_eq!(project_of(&resolved).as_deref(), Some("/work/demo"));
}

#[test]
fn a_project_directory_is_its_own_project() {
    let world = Fake::default().project("/work/demo");
    let resolved = resolve(&at("/work/demo"), &world, false, None, None).unwrap();

    assert_eq!(project_of(&resolved).as_deref(), Some("/work/demo"));
}

#[test]
fn nothing_above_the_working_directory_means_the_global_store() {
    let world = Fake::default().project("/somewhere/else");
    let resolved = resolve(&at("/work/demo/src"), &world, false, None, None).unwrap();

    assert_eq!(project_of(&resolved), None);
    assert_eq!(resolved.reason, Reason::NoProject);
    assert_eq!(resolved.search_order(), [Scope::Global]);
}

#[test]
fn a_project_is_searched_before_the_global_store() {
    let world = Fake::default().project("/work/demo");
    let resolved = resolve(&at("/work/demo"), &world, false, None, None).unwrap();

    assert_eq!(resolved.search_order(), [Scope::Project, Scope::Global]);
}

/// The collision rule: a name in both registries resolves to the project.
#[test]
fn a_name_in_both_registries_resolves_to_the_project() {
    let world = Fake::default().project("/work/demo");
    let resolved = resolve(&at("/work/demo"), &world, false, None, None).unwrap();
    let name = Qualified::parse("staging").unwrap();

    assert_eq!(resolved.scope_for(&name, |_| true), Some(Scope::Project));
}

#[test]
fn a_name_only_the_global_store_has_still_resolves() {
    let world = Fake::default().project("/work/demo");
    let resolved = resolve(&at("/work/demo"), &world, false, None, None).unwrap();
    let name = Qualified::parse("staging").unwrap();

    let found = resolved.scope_for(&name, |scope| scope == Scope::Global);
    assert_eq!(found, Some(Scope::Global));
}

#[test]
fn the_global_qualifier_reaches_past_a_project_that_has_the_name() {
    let world = Fake::default().project("/work/demo");
    let resolved = resolve(&at("/work/demo"), &world, false, None, None).unwrap();
    let name = Qualified::parse("global:staging").unwrap();

    assert_eq!(resolved.scope_for(&name, |_| true), Some(Scope::Global));
    assert_eq!(name.name(), "staging");
}

#[test]
fn the_global_qualifier_does_not_invent_an_entry_that_is_not_there() {
    let world = Fake::default().project("/work/demo");
    let resolved = resolve(&at("/work/demo"), &world, false, None, None).unwrap();
    let name = Qualified::parse("global:staging").unwrap();

    assert_eq!(
        resolved.scope_for(&name, |scope| scope == Scope::Project),
        None
    );
}

#[test]
fn a_name_in_neither_registry_resolves_to_nothing() {
    let world = Fake::default().project("/work/demo");
    let resolved = resolve(&at("/work/demo"), &world, false, None, None).unwrap();
    let name = Qualified::parse("staging").unwrap();

    assert_eq!(resolved.scope_for(&name, |_| false), None);
}

#[test]
fn under_global_a_bare_name_never_reaches_a_project() {
    let world = Fake::default().project("/work/demo");
    let resolved = resolve(&at("/work/demo"), &world, true, None, None).unwrap();
    let name = Qualified::parse("staging").unwrap();

    assert_eq!(resolved.scope_for(&name, |_| true), Some(Scope::Global));
}

#[test]
fn a_bare_name_carries_no_qualifier() {
    let name = Qualified::parse("staging").unwrap();
    assert_eq!(name.scope(), None);
    assert_eq!(name.name(), "staging");
}

#[test]
fn an_unknown_qualifier_is_refused_rather_than_read_as_a_name() {
    for input in ["project:staging", "local:staging", ":staging", "global:"] {
        let failure = Qualified::parse(input).unwrap_err();
        assert_eq!(failure.exit().code(), 2, "{input}");
    }
}

/// Every reason has to name itself. A user who cannot tell which registry was read is
/// the failure mode this whole module exists to avoid.
#[test]
fn describing_the_resolution_names_both_the_registry_and_the_reason() {
    let world = Fake::default().project("/work/demo");
    let global = Path::new("/store");
    let said = |resolution: &Resolution| {
        resolution
            .describe(global)
            .replace(std::path::MAIN_SEPARATOR, "/")
    };

    let walked = resolve(&at("/work/demo/src"), &world, false, None, None).unwrap();
    assert!(
        said(&walked).contains("/work/demo/.sloop"),
        "{}",
        said(&walked)
    );
    assert!(said(&walked).contains("nearest .sloop"));

    let flagged = resolve(&at("/elsewhere"), &world, false, Some("/work/demo"), None).unwrap();
    assert!(said(&flagged).contains("-C"));

    let from_env = resolve(&at("/elsewhere"), &world, false, None, Some("/work/demo")).unwrap();
    assert!(said(&from_env).contains("SLOOP_PROJECT"));

    let forced = resolve(&at("/work/demo"), &world, true, None, None).unwrap();
    assert!(said(&forced).contains("--global"));
    assert!(said(&forced).contains("/store"));

    let nowhere = resolve(&at("/nothing/here"), &world, false, None, None).unwrap();
    assert!(said(&nowhere).contains("/store"));
    assert!(said(&nowhere).contains("no .sloop"));
}

#[test]
fn describing_the_lookup_puts_the_project_first_and_says_so() {
    let world = Fake::default().project("/work/demo");
    let global = Path::new("/store");

    let inside = resolve(&at("/work/demo"), &world, false, None, None).unwrap();
    let said = inside
        .describe_lookup(global)
        .replace(std::path::MAIN_SEPARATOR, "/");
    assert_eq!(said, "this project, then the global store at /store");

    let outside = resolve(&at("/nothing/here"), &world, false, None, None).unwrap();
    let said = outside
        .describe_lookup(global)
        .replace(std::path::MAIN_SEPARATOR, "/");
    assert_eq!(said, "the global store at /store");
}

#[test]
fn the_registry_directory_hangs_off_the_project() {
    let world = Fake::default().project("/work/demo");
    let resolved = resolve(&at("/work/demo"), &world, false, None, None).unwrap();

    assert_eq!(
        resolved.registry_dir().as_deref().map(slashes),
        Some("/work/demo/.sloop".to_owned())
    );
}

#[test]
fn normalising_resolves_dot_and_dot_dot_without_touching_the_disk() {
    let cases = [
        ("/a/b/../c", "/a/c"),
        ("/a/./b", "/a/b"),
        ("/a/b/../../c", "/c"),
        ("/..", "/"),
        ("/a/../..", "/"),
        ("a/b/../c", "a/c"),
        ("../a", "../a"),
    ];

    for (input, want) in cases {
        let got = slashes(&normalize(Path::new(input)));
        assert_eq!(got, want, "{input}");
    }
}

#[cfg(windows)]
#[test]
fn normalising_leaves_a_drive_prefix_alone() {
    let got = normalize(Path::new(r"C:\a\b\..\c"));
    assert_eq!(got, PathBuf::from(r"C:\a\c"));

    let above_the_drive = normalize(Path::new(r"C:\.."));
    assert_eq!(above_the_drive, PathBuf::from(r"C:\"));
}
