//! What the removal does to a startup file, and what it leaves alone.
//!
//! **A home directory made for the test, never the one the test runs in.** `remove_from`
//! takes the home it works under as an argument for exactly this reason: proving the
//! rewrite by editing the environment of the process running the tests would mean editing
//! something every other test running beside it shares — and the file it would then rewrite
//! is the developer's own `.bashrc`.

use std::path::{Path, PathBuf};

use super::{BEGIN, END, Touched, candidates, remove_from, without_block};

/// A home directory on disk, thrown away when it goes out of scope.
struct Home(PathBuf);

impl Home {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "sloop-pathentry-{}-{label}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a temporary directory should be creatable");
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn put(&self, relative: &str, what: &str) -> PathBuf {
        let path = self.0.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("creatable");
        }
        std::fs::write(&path, what).expect("writable");
        path
    }

    fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.0.join(relative)).expect("readable")
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What `install.sh` appends, written the way it writes it.
fn block() -> String {
    format!(
        "\n{BEGIN}\n\
         # Added by the sloop installer. `sloop uninstall` removes this block.\n\
         case \":$PATH:\" in\n\
         \x20   *\":$HOME/.local/bin:\"*) ;;\n\
         \x20   *) PATH=\"$HOME/.local/bin:$PATH\" ;;\n\
         esac\n\
         export PATH\n\
         {END}\n"
    )
}

#[test]
fn a_file_without_the_block_is_not_touched() {
    let text = "export EDITOR=vi\nexport PATH=\"$HOME/bin:$PATH\"\n";
    assert_eq!(without_block(text), None);
}

#[test]
fn the_block_goes_and_nothing_else_does() {
    let before = format!("export EDITOR=vi\n{}alias ll='ls -l'\n", block());

    let after = without_block(&before).expect("the block is there");

    assert_eq!(after, "export EDITOR=vi\n\nalias ll='ls -l'\n");
    assert!(!after.contains("sloop"));
}

#[test]
fn a_file_that_is_only_the_block_comes_back_empty() {
    let after = without_block(&block()).expect("the block is there");
    assert_eq!(after, "\n");
}

/// A `.bashrc` edited on Windows and synced to a Linux box has CRLF in it, and an uninstall
/// that rewrote every line ending would show up as a whole-file change in somebody's
/// dotfiles repository.
#[test]
fn crlf_endings_survive_the_rewrite() {
    let before =
        format!("export EDITOR=vi\r\n{BEGIN}\r\nexport PATH=x\r\n{END}\r\nalias ll=ls\r\n");

    let after = without_block(&before).expect("the block is there");

    assert_eq!(after, "export EDITOR=vi\r\nalias ll=ls\r\n");
}

/// A file with no trailing newline is a file somebody wrote by hand, and it comes back the
/// way it went in.
#[test]
fn a_missing_final_newline_stays_missing() {
    let before = format!("{BEGIN}\nexport PATH=x\n{END}\nalias ll=ls");

    let after = without_block(&before).expect("the block is there");

    assert_eq!(after, "alias ll=ls");
}

/// Two installs, or an install and a hand-copied block: both go.
#[test]
fn two_blocks_both_go() {
    let before = format!("first\n{}second\n{}third\n", block(), block());

    let after = without_block(&before).expect("the blocks are there");

    assert!(!after.contains("sloop"));
    assert_eq!(after, "first\n\nsecond\n\nthird\n");
}

/// The markers have to be the whole line. A `.bashrc` that mentions them in prose — a
/// comment about this very uninstall, say — is not a block.
#[test]
fn a_marker_inside_a_longer_line_is_not_a_marker() {
    let before = format!("# the installer writes {BEGIN} around its block\nexport PATH=x\n");

    assert_eq!(without_block(&before), None);
}

#[test]
fn every_shell_that_could_hold_one_is_looked_at() {
    let home = Path::new("/home/someone");
    let found = candidates(home, None, None);

    for expected in [
        "/home/someone/.bashrc",
        "/home/someone/.bash_profile",
        "/home/someone/.bash_login",
        "/home/someone/.profile",
        "/home/someone/.kshrc",
        "/home/someone/.zshrc",
        "/home/someone/.config/fish/conf.d/sloop.fish",
    ] {
        assert!(
            found.iter().any(|path| path == Path::new(expected)),
            "{expected} is not among {found:?}"
        );
    }
}

#[test]
fn zdotdir_and_xdg_are_honoured() {
    let home = Path::new("/home/someone");
    let found = candidates(
        home,
        Some(Path::new("/home/someone/.config/zsh")),
        Some(Path::new("/home/someone/.settings")),
    );

    assert!(
        found
            .iter()
            .any(|path| path == Path::new("/home/someone/.config/zsh/.zshrc"))
    );
    assert!(
        found
            .iter()
            .any(|path| path == Path::new("/home/someone/.settings/fish/conf.d/sloop.fish"))
    );
}

#[test]
fn the_block_comes_out_of_every_file_that_had_one() {
    let home = Home::new("many");
    home.put(".bashrc", &format!("export EDITOR=vi\n{}", block()));
    home.put(".zshrc", &format!("{}setopt autocd\n", block()));
    home.put(".profile", "umask 022\n");

    let removal = remove_from(home.path(), None, None);

    assert!(removal.trouble.is_empty(), "{:?}", removal.trouble);
    assert_eq!(removal.touched.len(), 2, "{:?}", removal.touched);
    assert!(
        removal
            .touched
            .iter()
            .all(|touched| matches!(touched, Touched::Block(_)))
    );

    assert_eq!(home.read(".bashrc"), "export EDITOR=vi\n\n");
    assert_eq!(home.read(".zshrc"), "\nsetopt autocd\n");
    assert_eq!(home.read(".profile"), "umask 022\n");
}

/// fish gets a file of its own, so the removal is a delete — and the rest of `conf.d` is
/// somebody else's.
#[test]
fn the_fish_file_goes_and_its_neighbours_stay() {
    let home = Home::new("fish");
    home.put(
        ".config/fish/conf.d/sloop.fish",
        &format!("{BEGIN}\nfish_add_path $HOME/.local/bin\n{END}\n"),
    );
    let neighbour = home.put(".config/fish/conf.d/nvm.fish", "set -x NVM_DIR ~/.nvm\n");

    let removal = remove_from(home.path(), None, None);

    assert_eq!(
        removal.touched,
        vec![Touched::File(
            home.path().join(".config/fish/conf.d/sloop.fish")
        )]
    );
    assert!(!home.path().join(".config/fish/conf.d/sloop.fish").exists());
    assert!(neighbour.exists(), "somebody else's conf.d file went");
}

/// A machine that never installed sloop, or one where the uninstall already ran: nothing to
/// do, and nothing said about it.
#[test]
fn a_home_with_no_block_reports_nothing() {
    let home = Home::new("clean");
    home.put(".bashrc", "export EDITOR=vi\n");

    let removal = remove_from(home.path(), None, None);

    assert!(removal.touched.is_empty());
    assert!(removal.trouble.is_empty());
    assert_eq!(home.read(".bashrc"), "export EDITOR=vi\n");
}

/// Running it twice is running it once. An uninstall that is re-run after a failure
/// elsewhere must not report a second removal it did not make.
#[test]
fn removing_twice_changes_nothing_the_second_time() {
    let home = Home::new("twice");
    home.put(".bashrc", &format!("export EDITOR=vi\n{}", block()));

    let first = remove_from(home.path(), None, None);
    let second = remove_from(home.path(), None, None);

    assert_eq!(first.touched.len(), 1);
    assert!(second.touched.is_empty());
    assert!(second.trouble.is_empty());
}

/// The mode of a startup file is not the uninstaller's to change: a `.profile` that comes
/// back world-readable is a change nobody asked for.
#[cfg(unix)]
#[test]
fn the_file_keeps_the_permissions_it_had() {
    use std::os::unix::fs::PermissionsExt;

    let home = Home::new("mode");
    let rc = home.put(".bashrc", &format!("export EDITOR=vi\n{}", block()));
    std::fs::set_permissions(&rc, std::fs::Permissions::from_mode(0o600)).expect("chmod");

    let removal = remove_from(home.path(), None, None);
    assert_eq!(removal.touched.len(), 1);

    let mode = std::fs::metadata(&rc)
        .expect("still there")
        .permissions()
        .mode();
    assert_eq!(
        mode & 0o777,
        0o600,
        "the mode changed to {:o}",
        mode & 0o777
    );
}

/// The whole round trip on the platform that keeps its `PATH` in the registry: seed a
/// throwaway key the way the installer writes one, take it out with the real code, and
/// prove the entries that were not sloop's are still there and still unexpanded.
#[cfg(windows)]
#[test]
fn the_registry_entry_goes_and_the_others_keep_their_spelling() {
    use std::process::Command;

    let key = format!("Environment-sloop-test-{}", std::process::id());
    let dir = format!(
        "{}\\Programs\\sloop\\bin",
        std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA")
    );

    // Seeded through PowerShell rather than through the module under test, so the test does
    // not prove the removal with the removal. `%USERPROFILE%\bin` is there to catch the
    // failure this code exists to avoid: reading the value back expanded and writing today's
    // answer over somebody else's variable.
    let seed = format!(
        "$k = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('{key}', $true); \
         $k.SetValue('Path', 'C:\\Windows;%USERPROFILE%\\bin;%LOCALAPPDATA%\\Programs\\sloop\\bin', \
         [Microsoft.Win32.RegistryValueKind]::ExpandString); $k.Close()"
    );
    let seeded = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &seed])
        .output()
        .expect("powershell should run on Windows");
    assert!(
        seeded.status.success(),
        "seeding failed: {}",
        String::from_utf8_lossy(&seeded.stderr)
    );

    // The key is handed to the removal rather than put in this process's environment.
    // `set_var` is unsafe in this edition and the crate forbids unsafe, and a test that
    // edits the environment edits it for every test running beside it.
    let removal = super::windows::remove_in(std::path::Path::new(&dir), &key);

    let read = format!(
        "$k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('{key}', $false); \
         Write-Output $k.GetValue('Path', '', \
         [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames); $k.Close(); \
         [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKey('{key}')"
    );
    let after = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &read])
        .output()
        .expect("powershell should run on Windows");
    let left = String::from_utf8_lossy(&after.stdout).trim().to_owned();

    assert!(removal.trouble.is_empty(), "{:?}", removal.trouble);
    assert_eq!(
        removal.touched,
        vec![Touched::Registry(
            "%LOCALAPPDATA%\\Programs\\sloop\\bin".to_owned()
        )]
    );
    assert_eq!(left, "C:\\Windows;%USERPROFILE%\\bin");
}
