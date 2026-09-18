//! The `PATH` entry the installer wrote, and taking it back out.
//!
//! **`R21`'s half of `uninstall`.** `R19c6` built the part that knows what sloop *created* —
//! a PostgreSQL it downloaded, a database, a role, a registry — and left the installer's two
//! artefacts to here, because they are the installer's to describe: the directory it chose
//! and the line it appended. The entry's own words are the test — *"uninstall leaves nothing
//! behind including the `PATH` entry"* — and a `PATH` still naming a directory that no longer
//! exists is something left behind.
//!
//! **It is written twice, and that is deliberate rather than an oversight.** `uninstall.sh`
//! and `uninstall.ps1` do the same removal without the binary, because the case they exist
//! for is a binary that is gone or will not run, and a copy that can only be reached through
//! the thing being removed is not a fallback. The two are kept honest by both being driven:
//! `ci/install-roundtrip.sh` and `ci/install-roundtrip.ps1` install and then remove with the
//! script, while `cargo test` removes with this.
//!
//! **Windows goes through the registry rather than through
//! `[Environment]::SetEnvironmentVariable`, and the reason is not style.** That API reads the
//! value back *expanded* — a `Path` holding `%USERPROFILE%\bin` comes back as
//! `C:\Users\…\bin`, and writing that back replaces a variable somebody else's installer put
//! there with today's answer to it. The registry API can be told not to expand, and is.

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

use std::path::{Path, PathBuf};

/// The two lines the installer brackets its block with.
///
/// **A marked block is removable by a machine.** A bare `export PATH=…` appended to
/// somebody's `.bashrc` is removable only by a person reading it, which is near enough to
/// not being removable at all.
const BEGIN: &str = "# >>> sloop >>>";
const END: &str = "# <<< sloop <<<";

/// What the installer left on this machine, and how much of it is sloop's to delete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installed {
    /// A directory that is entirely the installer's, and goes whole. Windows, where the
    /// installer creates `%LOCALAPPDATA%\Programs\sloop` and nothing else writes into it.
    Tree(PathBuf),
    /// One file inside a directory that belongs to somebody else. Unix, where
    /// `~/.local/bin` is a person's own bin directory with a person's own programs in it.
    File(PathBuf),
}

/// One thing the removal changed, said in the words that fit what it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Touched {
    /// A startup file that had the block taken out of it and is otherwise unchanged.
    Block(PathBuf),
    /// A whole file that *was* the entry — fish keeps its own in `conf.d`, so removing it
    /// is a delete rather than an edit.
    File(PathBuf),
    /// An entry that came out of the user's `Path` in the registry, as it was spelled.
    ///
    /// Windows only. The enum stays one type on every platform so that the caller reporting
    /// what an uninstall did does not have to be written twice.
    #[cfg_attr(not(windows), allow(dead_code))]
    Registry(String),
}

impl Touched {
    /// The line for a report, written to follow a `PATH` label rather than to repeat it.
    #[must_use]
    pub fn said(&self) -> String {
        match self {
            Self::Block(path) => format!("block removed from {}", path.display()),
            Self::File(path) => format!("file removed: {}", path.display()),
            Self::Registry(entry) => format!("entry removed: {entry}"),
        }
    }
}

/// What a removal did, and what it could not do.
///
/// **Both halves, because neither alone is the truth.** An uninstall that says nothing about
/// the `.zshrc` it could not rewrite has left a line behind silently; one that fails on it
/// has thrown away everything it *did* manage.
#[derive(Debug, Default)]
pub struct Removal {
    /// What changed.
    pub touched: Vec<Touched>,
    /// What did not, in words somebody can act on.
    pub trouble: Vec<String>,
}

/// The directory the installer puts `sloop` in — the one that goes on `PATH`.
#[must_use]
pub fn bin_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        Some(
            PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
                .join("Programs")
                .join("sloop")
                .join("bin"),
        )
    } else {
        Some(
            PathBuf::from(std::env::var_os("HOME")?)
                .join(".local")
                .join("bin"),
        )
    }
}

/// What `uninstall` deletes of the installer's work.
#[must_use]
pub fn installed() -> Option<Installed> {
    let bin = bin_dir()?;
    if cfg!(windows) {
        // The directory above `bin`, because `Programs\sloop` is wholly the installer's.
        Some(Installed::Tree(bin.parent()?.to_path_buf()))
    } else {
        Some(Installed::File(bin.join("sloop")))
    }
}

/// Take the entry out, wherever this platform keeps it.
///
/// **Best effort, and it says what it could not do rather than failing.** Everything else has
/// already gone by the time this runs; a `.zshrc` that could not be rewritten is a line
/// somebody deletes by hand, not a reason to report that the uninstall failed.
#[must_use]
pub fn remove() -> Removal {
    #[cfg(windows)]
    {
        let Some(dir) = bin_dir() else {
            return Removal::default();
        };
        windows::remove(&dir)
    }

    #[cfg(not(windows))]
    {
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return Removal::default();
        };
        let zdotdir = std::env::var_os("ZDOTDIR").map(PathBuf::from);
        let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
        remove_from(&home, zdotdir.as_deref(), xdg.as_deref())
    }
}

/// Every startup file that could be holding the block, under one home directory.
///
/// **All of them, not just the current shell's.** Somebody who installed under bash and has
/// since moved to zsh has the block in `.bashrc`, and looking only at `$SHELL` would leave it
/// there naming a binary that no longer exists.
#[cfg_attr(windows, allow(dead_code))]
fn candidates(home: &Path, zdotdir: Option<&Path>, xdg: Option<&Path>) -> Vec<PathBuf> {
    vec![
        home.join(".bashrc"),
        home.join(".bash_profile"),
        home.join(".bash_login"),
        home.join(".profile"),
        home.join(".kshrc"),
        zdotdir.unwrap_or(home).join(".zshrc"),
        xdg.map_or_else(|| home.join(".config"), Path::to_path_buf)
            .join("fish")
            .join("conf.d")
            .join("sloop.fish"),
    ]
}

/// The rewrite, under a home directory this is told about rather than one it looks up.
///
/// **Told rather than looked up, so a test can prove it.** Reading `$HOME` here would mean the
/// only way to exercise the removal is to change the environment of the process running the
/// tests, which every other test running beside it shares.
#[cfg_attr(windows, allow(dead_code))]
fn remove_from(home: &Path, zdotdir: Option<&Path>, xdg: Option<&Path>) -> Removal {
    let mut removal = Removal::default();

    for path in candidates(home, zdotdir, xdg) {
        // fish is not a POSIX shell, so the installer gives it a file of its own in
        // `conf.d` rather than a block inside somebody else's startup file.
        if path.extension().is_some_and(|kind| kind == "fish") {
            if path.is_file() {
                match std::fs::remove_file(&path) {
                    Ok(()) => removal.touched.push(Touched::File(path)),
                    Err(error) => removal.trouble.push(format!("{}: {error}", path.display())),
                }
            }
            continue;
        }

        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(rewritten) = without_block(&text) else {
            continue;
        };

        match write_preserving_mode(&path, &rewritten) {
            Ok(()) => removal.touched.push(Touched::Block(path)),
            Err(error) => removal.trouble.push(format!("{}: {error}", path.display())),
        }
    }

    removal
}

/// One file's text without the marked block, or `None` when it did not have one.
///
/// **Line endings are kept as they were found.** A `.bashrc` edited on Windows and synced to a
/// Linux box has CRLF in it, and an uninstall that normalised every line ending would show up
/// as a whole-file change in somebody's dotfiles repository.
#[cfg_attr(windows, allow(dead_code))]
fn without_block(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    let mut found = false;

    for line in text.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);

        if bare == BEGIN {
            inside = true;
            found = true;
            continue;
        }
        if bare == END {
            inside = false;
            continue;
        }
        if !inside {
            out.push_str(line);
        }
    }

    // A file whose block was opened and never closed has had everything after the marker
    // taken out. That is the installer's own block either way, and half-removing it would
    // leave a `case` statement with no `esac`.
    found.then_some(out)
}

/// Write the file back with the permissions it already had.
///
/// **A temporary file beside the original, then a rename**, so a machine that loses power
/// halfway leaves a `.bashrc` rather than half of one. The mode is carried across because a
/// `.profile` that comes back world-readable is a change nobody asked for.
#[cfg_attr(windows, allow(dead_code))]
fn write_preserving_mode(path: &Path, text: &str) -> std::io::Result<()> {
    let name = path.file_name().map_or_else(
        || String::from("rc"),
        |name| name.to_string_lossy().into_owned(),
    );
    let temporary = path.with_file_name(format!(".{name}.sloop-uninstall.{}", std::process::id()));

    std::fs::write(&temporary, text)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(original) = std::fs::metadata(path) {
            let mode = original.permissions().mode();
            let _ = std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(mode));
        }
    }

    match std::fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            Err(error)
        }
    }
}

/// `HKCU\Environment`, through the one program every Windows has.
///
/// **Shelled out to rather than linked against, which is this project's habit for the
/// system's own jobs** — the download is `curl`, a database behind a firewall is `ssh`, and
/// the registry is PowerShell. It buys the same thing each time: nothing in `cargo tree` that
/// the guarantee then has to account for.
#[cfg(windows)]
mod windows {
    use std::path::Path;
    use std::process::Command;

    use super::{Removal, Touched};

    /// The removal, in the language of the machine it runs on.
    ///
    /// **Written to a file and run with `-File`, not handed to `-Command`.** `powershell.exe`
    /// re-parses the tail of its own command line for `-Command`, and a script holding both
    /// kinds of quote — this one has a C# snippet inside a here-string inside it — is exactly
    /// what that mangles. A file has no command line to survive.
    const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$sub = $env:SLOOP_PATH_KEY
$wanted = ([string] $env:SLOOP_PATH_DIR).TrimEnd('\')
if (-not $wanted) { exit 0 }

$key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($sub, $false)
if (-not $key) { exit 0 }
$value = $key.GetValue('Path', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
$kind = $null
if ($null -ne $value) { $kind = $key.GetValueKind('Path') }
$key.Close()
if ($null -eq $value) { exit 0 }

$kept = @()
$dropped = @()
foreach ($entry in ([string] $value -split ';')) {
    if ($entry -eq '') { continue }
    if ([Environment]::ExpandEnvironmentVariables($entry).TrimEnd('\') -ieq $wanted) {
        $dropped += $entry
    } else {
        $kept += $entry
    }
}
if ($dropped.Count -eq 0) { exit 0 }

$writable = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($sub, $true)
try { $writable.SetValue('Path', ($kept -join ';'), $kind) } finally { $writable.Close() }

# Explorer and everything launched from it read the environment once and cache it. Without
# the broadcast, a terminal opened from the Start menu still has the old PATH in it.
try {
    if (-not ('Sloop.Broadcast' -as [type])) {
        Add-Type -Namespace 'Sloop' -Name 'Broadcast' -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError = true, CharSet = System.Runtime.InteropServices.CharSet.Auto)]
public static extern System.IntPtr SendMessageTimeout(
    System.IntPtr hWnd, uint Msg, System.UIntPtr wParam, string lParam,
    uint fuFlags, uint uTimeout, out System.UIntPtr lpdwResult);
'@
    }
    $result = [System.UIntPtr]::Zero
    [Sloop.Broadcast]::SendMessageTimeout(
        [System.IntPtr] 0xffff, 0x1A, [System.UIntPtr]::Zero, 'Environment',
        0x0002, 1000, [ref] $result) | Out-Null
} catch { }

foreach ($entry in $dropped) { Write-Output $entry }
"#;

    /// Take the directory out of the user's `Path`.
    ///
    /// `SLOOP_TEST_PATH_KEY` is the same knob `install.ps1` reads, and for the same reason:
    /// a test that edits the real `HKCU\Environment` to prove itself is a test that can
    /// break the machine it runs on.
    pub fn remove(dir: &Path) -> Removal {
        let key = std::env::var("SLOOP_TEST_PATH_KEY")
            .ok()
            .filter(|key| !key.is_empty())
            .unwrap_or_else(|| String::from("Environment"));
        remove_in(dir, &key)
    }

    /// The removal against one named subkey of `HKEY_CURRENT_USER`, and the spellings of the
    /// directory that went.
    ///
    /// The directory arrives as an environment variable rather than inside the script,
    /// because `C:\Users\O'Brien\…` is a perfectly ordinary Windows path and a path pasted
    /// into PowerShell source is a path that has to survive PowerShell's own quoting.
    pub fn remove_in(dir: &Path, key: &str) -> Removal {
        let script = std::env::temp_dir().join(format!(
            "sloop-path-{}-{}.ps1",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_nanos())
        ));

        if let Err(error) = std::fs::write(&script, SCRIPT) {
            return still_there(dir, &error.to_string());
        }

        let run = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&script)
            .env("SLOOP_PATH_DIR", dir)
            .env("SLOOP_PATH_KEY", key)
            .output();

        let _ = std::fs::remove_file(&script);

        match run {
            Ok(output) if output.status.success() => Removal {
                touched: String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(|line| Touched::Registry(line.to_owned()))
                    .collect(),
                trouble: Vec::new(),
            },
            Ok(output) => still_there(dir, String::from_utf8_lossy(&output.stderr).trim()),
            Err(error) => still_there(dir, &error.to_string()),
        }
    }

    /// The one thing there is to say when the registry would not have it.
    fn still_there(dir: &Path, why: &str) -> Removal {
        let tail = if why.is_empty() {
            String::new()
        } else {
            format!(": {why}")
        };
        Removal {
            touched: Vec::new(),
            trouble: vec![format!(
                "{} is still on PATH in HKCU\\Environment{tail}",
                dir.display()
            )],
        }
    }
}
