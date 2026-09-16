//! Which registry a command reads, and in what order.
//!
//! ```text
//! --global             the global store, full stop
//! -C <path|name>       that project
//! SLOOP_PROJECT        the same, from the environment
//! walk up from cwd     the nearest .sloop, the way git finds .git
//! otherwise            the global store
//! ```
//!
//! Once a project is in play, a bare name is looked for in the project **first** and the
//! global store second, so the nearer answer wins — the rule that makes a repository's
//! `.git/config` beat `~/.gitconfig`. `global:name` forces the far one.
//!
//! Every branch is reachable from a unit test, because the two things `resolve` asks about
//! the world — is there a `.sloop` here, and does this name point anywhere — arrive through
//! [`World`] rather than straight off the disk.

pub mod file;
pub mod locations;
pub mod projects;
pub mod url;

#[cfg(test)]
mod tests;

use std::path::{Component, Path, PathBuf};

use crate::failure::{Failure, Outcome};
use file::Registry;
use locations::PROJECT_DIR;

/// One of the two registries.
///
/// Ordered, because a command that works over both keeps a map keyed by scope — and the
/// order is the search order: the project first, the global store second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    /// The `.sloop` beside the code.
    Project,
    /// The one in the user's config directory.
    Global,
}

/// Why the registry resolved the way it did.
///
/// Worth carrying around: half the confusing moments with a tool like this are "which
/// config did it just read", and the answer should never require guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// `--global` was given.
    GlobalFlag,
    /// `-C` named it.
    Flag,
    /// `SLOOP_PROJECT` named it.
    EnvVar,
    /// A `.sloop` was found at or above the working directory.
    WalkUp,
    /// Nothing was found, so the global store it is.
    NoProject,
}

/// Where a bare name will be looked for, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    project: Option<PathBuf>,
    reason: Reason,
}

impl Resolution {
    /// The project's registry directory.
    #[must_use]
    pub fn registry_dir(&self) -> Option<PathBuf> {
        self.project.as_ref().map(|dir| dir.join(PROJECT_DIR))
    }

    /// The scopes a bare name is looked for in, nearest first.
    #[must_use]
    pub fn search_order(&self) -> &'static [Scope] {
        if self.project.is_some() {
            &[Scope::Project, Scope::Global]
        } else {
            &[Scope::Global]
        }
    }

    /// Which scope answers for `name`.
    ///
    /// `holds` says whether a scope has an entry called `name`; this decides the order it
    /// is asked in. The split is deliberate — the order is settled here, once, and the
    /// registry file format can arrive later without moving it.
    // Nothing takes a database name until R7, so nothing calls this yet. The order and
    // the collision rule are settled here regardless, with tests, because deciding them
    // is R2's job and re-deriving them inside `db list` would be how they drift.
    #[allow(dead_code)]
    pub fn scope_for(
        &self,
        name: &Qualified<'_>,
        mut holds: impl FnMut(Scope) -> bool,
    ) -> Option<Scope> {
        match name.scope() {
            // `global:name` skips the project even when there is one.
            Some(forced) => holds(forced).then_some(forced),
            None => self
                .search_order()
                .iter()
                .copied()
                .find(|&scope| holds(scope)),
        }
    }

    /// The registry this resolved to, and in the same breath how it was chosen. Both
    /// halves matter: a tool that reads the wrong config is only confusing until it says
    /// which one it read.
    #[must_use]
    pub fn describe(&self, global_dir: &Path) -> String {
        let where_ = self
            .registry_dir()
            .unwrap_or_else(|| global_dir.to_path_buf());

        format!("{} — {}", where_.display(), self.why())
    }

    /// Why this registry and not the other one, in a phrase.
    ///
    /// **Its own method because a command that writes a record has to say it too.** Being
    /// told *"registered in the global registry"* when you never asked for the global
    /// registry reads like a bug — the answer is that there was no project to put it in, and
    /// that is a sentence rather than something to work out.
    #[must_use]
    pub fn why(&self) -> String {
        match self.reason {
            Reason::GlobalFlag => "--global".to_owned(),
            Reason::Flag => "named by -C".to_owned(),
            Reason::EnvVar => "named by SLOOP_PROJECT".to_owned(),
            Reason::WalkUp => {
                format!("the nearest {PROJECT_DIR} at or above the working directory")
            }
            Reason::NoProject => format!("no {PROJECT_DIR} at or above the working directory"),
        }
    }

    /// Where a bare name would be looked for, in the order it would be looked for in.
    #[must_use]
    pub fn describe_lookup(&self, global_dir: &Path) -> String {
        self.search_order()
            .iter()
            .map(|scope| match scope {
                Scope::Project => "this project".to_owned(),
                Scope::Global => format!("the global store at {}", global_dir.display()),
            })
            .collect::<Vec<_>>()
            .join(", then ")
    }
}

/// A name as the user typed it, with the one qualifier this tool understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Qualified<'a> {
    scope: Option<Scope>,
    name: &'a str,
}

impl<'a> Qualified<'a> {
    /// Split `global:name` from `name`.
    ///
    /// Anything else before a colon is refused rather than folded into the name. A
    /// silently misread qualifier would point a command at the wrong database, which is
    /// the one mistake this tool must never make quietly.
    pub fn parse(input: &'a str) -> Outcome<Self> {
        match input.split_once(':') {
            Some(("global", name)) if !name.is_empty() => Ok(Self {
                scope: Some(Scope::Global),
                name,
            }),
            Some(("global", _)) => {
                Err(Failure::usage("global: needs a name after it")
                    .hint("for example global:staging"))
            }
            Some((qualifier, _)) => Err(Failure::usage(format!("{qualifier}: is not a qualifier"))
                .hint("global: is the only one — a bare name searches the project first")),
            None => Ok(Self {
                scope: None,
                name: input,
            }),
        }
    }

    /// The scope the qualifier forced, if it forced one.
    #[must_use]
    pub const fn scope(&self) -> Option<Scope> {
        self.scope
    }

    /// The name with the qualifier taken off.
    #[must_use]
    pub const fn name(&self) -> &'a str {
        self.name
    }
}

impl Scope {
    /// The word a listing puts beside an entry.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Global => "global",
        }
    }
}

/// Both registries, open at once.
///
/// **Both, because that is what R2 promised.** A bare name is looked for in the project
/// first and in the global store second, and `global:name` forces the far one — a rule
/// that cannot be implemented by a command holding one registry and guessing. So every
/// command that takes a name gets both, and the search order lives in [`Resolution`],
/// where it was settled, rather than being re-derived here.
pub struct Registries {
    project: Option<(PathBuf, Registry)>,
    global: (PathBuf, Registry),
    resolution: Resolution,
}

impl Registries {
    /// Read whichever of the two exist. A registry that is not there yet is an empty one,
    /// not an error: that is the state of every machine before the first `db add`.
    pub fn open(resolution: Resolution, global_dir: &Path) -> Outcome<Self> {
        let read = |dir: &Path| Registry::load(&dir.join(file::FILE));

        let project = match resolution.registry_dir() {
            Some(dir) => {
                let registry = read(&dir)?;
                Some((dir, registry))
            }
            None => None,
        };

        Ok(Self {
            project,
            global: (global_dir.to_path_buf(), read(global_dir)?),
            resolution,
        })
    }

    /// How the registry was chosen, for the commands that say so.
    #[must_use]
    pub const fn resolution(&self) -> &Resolution {
        &self.resolution
    }

    /// The registry in one scope, if it is in play at all.
    #[must_use]
    pub fn in_scope(&self, scope: Scope) -> Option<&Registry> {
        match scope {
            Scope::Project => self.project.as_ref().map(|(_, registry)| registry),
            Scope::Global => Some(&self.global.1),
        }
    }

    /// Where a scope's encrypted password file is.
    #[must_use]
    pub fn sealed_in(&self, scope: Scope) -> Option<PathBuf> {
        self.dir_of(scope).map(|dir| dir.join(file::SEALED_FILE))
    }

    /// The directory a scope keeps everything in — its registry, its sealed passwords and
    /// its backups. `backups/` hangs off this, so a project's copies stay with the project.
    #[must_use]
    pub fn root_in(&self, scope: Scope) -> Option<PathBuf> {
        self.dir_of(scope)
    }

    fn dir_of(&self, scope: Scope) -> Option<PathBuf> {
        match scope {
            Scope::Project => self.project.as_ref().map(|(dir, _)| dir.clone()),
            Scope::Global => Some(self.global.0.clone()),
        }
    }

    /// The scope a new entry goes in: the project when there is one, the global store
    /// otherwise — and always the global store under `--global`.
    #[must_use]
    pub const fn writes_to(&self) -> Scope {
        if self.project.is_some() {
            Scope::Project
        } else {
            Scope::Global
        }
    }

    /// Find a name, honouring the qualifier and the search order.
    pub fn find(&self, input: &str) -> Outcome<(Scope, &file::Database)> {
        let qualified = Qualified::parse(input)?;

        let scope = self
            .resolution
            .scope_for(&qualified, |scope| {
                self.in_scope(scope)
                    .is_some_and(|registry| registry.get(qualified.name()).is_some())
            })
            .ok_or_else(|| file::unknown(input))?;

        let database = self
            .in_scope(scope)
            .and_then(|registry| registry.get(qualified.name()))
            .ok_or_else(|| file::unknown(input))?;

        Ok((scope, database))
    }

    /// Every registered database, nearest scope first.
    ///
    /// The order matters for a listing: a project entry shadows a global one of the same
    /// name, and showing the shadowed one second is how somebody works out why.
    pub fn all(&self) -> impl Iterator<Item = (Scope, &str, &file::Database)> {
        self.resolution
            .search_order()
            .iter()
            .copied()
            .filter_map(move |scope| {
                self.in_scope(scope)
                    .map(move |registry| (scope, registry.entries()))
            })
            .flat_map(|(scope, entries)| {
                entries.map(move |(name, database)| (scope, name, database))
            })
    }

    /// How many are registered, across both.
    #[must_use]
    pub fn len(&self) -> usize {
        self.all().count()
    }

    /// Is there nothing registered anywhere?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.all().next().is_none()
    }

    /// Read a scope's registry, change it, and write it back.
    ///
    /// The read-modify-write is in one place because the write half has to be atomic —
    /// see [`Registry::save`] — and a second copy of this is a second chance to forget.
    pub fn update<T>(
        &mut self,
        scope: Scope,
        change: impl FnOnce(&mut Registry) -> Outcome<T>,
    ) -> Outcome<T> {
        let (dir, registry) = match scope {
            Scope::Project => self
                .project
                .as_mut()
                .ok_or_else(|| Failure::usage("there is no project registry here"))?,
            Scope::Global => &mut self.global,
        };

        let outcome = change(registry)?;
        registry.save(&dir.join(file::FILE))?;
        Ok(outcome)
    }
}

/// The two questions resolution asks of the world outside the process.
pub trait World {
    /// Is this a directory?
    fn is_directory(&self, path: &Path) -> bool;
    /// Does this directory hold a `.sloop`?
    fn is_project(&self, dir: &Path) -> bool;
    /// Where does this project name point?
    fn project_named(&self, name: &str) -> Option<PathBuf>;
}

/// Which registry to read, given the flags, the environment and the working directory.
pub fn resolve(
    cwd: &Path,
    world: &dyn World,
    global_flag: bool,
    flag: Option<&str>,
    env: Option<&str>,
) -> Outcome<Resolution> {
    if global_flag {
        return Ok(Resolution {
            project: None,
            reason: Reason::GlobalFlag,
        });
    }

    if let Some(argument) = flag.filter(|value| !value.is_empty()) {
        return named(cwd, world, argument, Reason::Flag);
    }

    // An empty variable is how a shell says "unset". `SLOOP_PROJECT=` must not be an error.
    if let Some(argument) = env.filter(|value| !value.is_empty()) {
        return named(cwd, world, argument, Reason::EnvVar);
    }

    Ok(match walk_up(cwd, world) {
        Some(project) => Resolution {
            project: Some(project),
            reason: Reason::WalkUp,
        },
        None => Resolution {
            project: None,
            reason: Reason::NoProject,
        },
    })
}

/// Resolve an argument that is either a path or a registered name.
///
/// The path reading is tried first, so a bare word behaves the way `git -C` makes it
/// behave and what the user can see beats what they cannot. A directory is walked up from,
/// again like `git -C`, so pointing at a subdirectory of a project finds the project.
fn named(cwd: &Path, world: &dyn World, argument: &str, reason: Reason) -> Outcome<Resolution> {
    let source = if matches!(reason, Reason::EnvVar) {
        "SLOOP_PROJECT"
    } else {
        "-C"
    };

    let as_path = normalize(&cwd.join(argument));
    if world.is_directory(&as_path)
        && let Some(project) = walk_up(&as_path, world)
    {
        return Ok(Resolution {
            project: Some(project),
            reason,
        });
    }

    if let Some(project) = world.project_named(argument) {
        return Ok(Resolution {
            project: Some(project),
            reason,
        });
    }

    Err(Failure::usage(format!(
        "{source} says {argument}, which is neither a project directory nor a project sloop knows"
    ))
    .hint(format!(
        "run sloop init in it, or point {source} at a directory with a {PROJECT_DIR} at or above it"
    )))
}

/// The nearest project at or above `start`, the way git finds `.git`.
fn walk_up(start: &Path, world: &dyn World) -> Option<PathBuf> {
    let mut candidate = Some(start);
    while let Some(dir) = candidate {
        if world.is_project(dir) {
            return Some(dir.to_path_buf());
        }
        candidate = dir.parent();
    }
    None
}

/// Resolve `.` and `..` without touching the disk.
///
/// `canonicalize` would need the path to exist and would hand back a verbatim prefix on
/// Windows, which then shows up in output and in the index. Doing it lexically keeps the
/// path looking like the one the user typed.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if out.as_os_str().is_empty() {
                    out.push("..");
                }
                // Above a root or a drive prefix there is nothing, so the `..` is dropped.
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The real world: the filesystem, and the index in the global store.
pub struct Disk {
    global: PathBuf,
}

impl Disk {
    /// Look at the real disk, with the project index in `global`.
    #[must_use]
    pub fn new(global: impl Into<PathBuf>) -> Self {
        Self {
            global: global.into(),
        }
    }
}

impl World for Disk {
    fn is_directory(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn is_project(&self, dir: &Path) -> bool {
        dir.join(PROJECT_DIR).is_dir()
    }

    fn project_named(&self, name: &str) -> Option<PathBuf> {
        projects::read(&self.global, name)
    }
}
