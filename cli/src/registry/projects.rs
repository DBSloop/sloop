//! The index of named projects, so `-C <name>` reaches one from anywhere on the disk.
//!
//! One file per project: `<global>/projects/<name>`, holding that project's absolute path
//! and nothing else. There is no format, so there is nothing to mis-parse — the same
//! reason the registry itself will never be a file that gets `source`d. The path is read
//! back with a carriage return stripped, because a Windows editor will add one and the
//! "no such project" it causes is invisible in the file.

use std::path::{Path, PathBuf};

use crate::failure::{Failure, Outcome};

/// Where the pointers live inside the global store.
#[must_use]
pub fn dir(global: &Path) -> PathBuf {
    global.join("projects")
}

/// Characters a project name may not contain, because the name becomes a filename.
///
/// The separators and the colon are the ones that would let a name escape the index
/// directory; the rest are simply illegal in a Windows filename, and a name that works on
/// one machine and not another is worse than a name that is refused everywhere.
const FORBIDDEN: [char; 9] = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// Check that a project name can be a filename, because that is what it becomes.
///
/// The default name is the project directory's own name, so this refuses only what a
/// directory name could never be anyway — and refusing it here means the index can never
/// hold an entry that points outside itself.
pub fn check_name(name: &str) -> Outcome<&str> {
    let refuse = |why: String| {
        Failure::usage(format!("{name} cannot be a project name: {why}"))
            .hint("a project name becomes a filename, so it has to look like one")
    };

    if name.is_empty() {
        return Err(refuse("it is empty".to_owned()));
    }
    if name == "." || name == ".." {
        return Err(refuse(
            "it names a directory rather than a project".to_owned(),
        ));
    }
    if let Some(bad) = name
        .chars()
        .find(|c| FORBIDDEN.contains(c) || c.is_control())
    {
        return Err(refuse(format!(
            "{} is not allowed in one",
            bad.escape_debug()
        )));
    }
    if name.trim() != name {
        return Err(refuse("it starts or ends with whitespace".to_owned()));
    }

    Ok(name)
}

/// Where the pointer for `name` would be.
pub fn path_for(global: &Path, name: &str) -> Outcome<PathBuf> {
    Ok(dir(global).join(check_name(name)?))
}

/// The project `name` points at, if the index knows it and the pointer is readable.
///
/// A pointer that cannot be read counts as absent rather than as an error: the index is a
/// convenience, and `-C <path>` still has to work when it is broken.
#[must_use]
pub fn read(global: &Path, name: &str) -> Option<PathBuf> {
    let pointer = path_for(global, name).ok()?;
    let contents = std::fs::read_to_string(pointer).ok()?;
    parse_pointer(&contents)
}

/// Record that `project` is called `name`.
pub fn write(global: &Path, name: &str, project: &Path) -> Outcome<()> {
    let pointer = path_for(global, name)?;
    let parent = pointer.parent().unwrap_or(global);

    std::fs::create_dir_all(parent).map_err(|error| {
        Failure::usage(format!("could not create {}: {error}", parent.display()))
    })?;

    // A trailing newline, so it is a well-behaved text file. Reading strips it again.
    let line = format!("{}\n", project.display());
    std::fs::write(&pointer, line)
        .map_err(|error| Failure::usage(format!("could not write {}: {error}", pointer.display())))
}

/// The first non-empty line of a pointer file, with a carriage return and any trailing
/// whitespace taken off.
fn parse_pointer(contents: &str) -> Option<PathBuf> {
    contents
        .lines()
        .map(|line| line.trim_end_matches('\r').trim_end())
        .find(|line| !line.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{check_name, dir, parse_pointer, path_for};

    #[test]
    fn pointers_live_under_projects() {
        assert_eq!(dir(Path::new("/store")), PathBuf::from("/store/projects"));
        assert_eq!(
            path_for(Path::new("/store"), "demo").unwrap(),
            PathBuf::from("/store/projects/demo")
        );
    }

    #[test]
    fn a_name_that_could_escape_its_directory_is_refused() {
        let refused = [
            "", ".", "..", "a/b", "a\\b", "c:name", "a*b", "a?b", "a|b", "a<b", "a>b", "a\"b",
            " demo", "demo ", "de\tmo", "de\nmo",
        ];

        for name in refused {
            let failure = check_name(name).expect_err("should be refused");
            assert_eq!(failure.exit().code(), 2, "{name:?}");
        }

        assert!(path_for(Path::new("/store"), "../escape").is_err());
    }

    #[test]
    fn an_ordinary_directory_name_is_a_fine_project_name() {
        for name in ["demo", "my-app", "my_app", "app.v2", "Ünïcödé", "a"] {
            assert_eq!(check_name(name).unwrap(), name);
        }
    }

    #[test]
    fn a_pointer_survives_crlf_and_trailing_whitespace() {
        // A Windows editor adds the carriage return, and the failure it causes looks like
        // a missing project rather than like a stray byte.
        for contents in [
            "/home/me/demo",
            "/home/me/demo\n",
            "/home/me/demo\r\n",
            "/home/me/demo   \r\n",
            "\n\n/home/me/demo\r\n",
        ] {
            assert_eq!(
                parse_pointer(contents),
                Some(PathBuf::from("/home/me/demo")),
                "{contents:?}"
            );
        }
    }

    #[test]
    fn a_path_with_spaces_in_it_is_kept_whole() {
        assert_eq!(
            parse_pointer("C:\\Users\\me\\My Projects\\demo\r\n"),
            Some(PathBuf::from("C:\\Users\\me\\My Projects\\demo"))
        );
    }

    #[test]
    fn an_empty_pointer_is_no_pointer() {
        for contents in ["", "\n", "   \r\n", "\r\n\r\n"] {
            assert_eq!(parse_pointer(contents), None, "{contents:?}");
        }
    }
}
