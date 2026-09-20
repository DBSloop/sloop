//! Going and getting the client tools — on demand, with consent, never silently.
//!
//! This is the one path in sloop that reaches the internet, and every part of its shape is
//! a consequence of that.
//!
//! **Nothing here is linked in.** The download is the system's own `curl`, or PowerShell's
//! `Invoke-WebRequest`, or `wget` — whichever the machine has. The archive is opened by the
//! system's own archiver. So `cargo tree` still shows no HTTP client, and the claim at the
//! top of the README survives somebody reading this file.
//!
//! **Nobody is asked twice.** A refusal is written down and honoured, so a tool that was
//! declined once does not nag on every command afterwards; it explains what to install and
//! gets out of the way.
//!
//! **Nobody is asked at all without a terminal.** A scheduled backup that stops to ask a
//! question nobody will ever see is the worst thing this tool can do, so without a terminal
//! this exits `2` with the instructions written out instead.
//!
//! **What arrives is proved before it is used.** The archive has to be the exact size and
//! the exact SHA-256 of a release pinned in [`releases`], or it is deleted and nothing is
//! installed. See that module for why the hash is pinned rather than fetched.

use std::collections::BTreeMap;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::engine::Engine;
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::style;

use super::{Inventory, Tool, releases};

/// How long a download is allowed to take before it is called a failure.
///
/// Generous: the PostgreSQL archive is a third of a gigabyte, and somebody on a slow line
/// should still get it. Bounded all the same, because a stalled transfer with no ceiling is
/// a command that never returns.
const DOWNLOAD_TIMEOUT_SECONDS: u32 = 1800;

/// Where sloop keeps what it fetched.
#[must_use]
pub fn fetched_dir(global: &Path) -> PathBuf {
    global.join("tools").join("bin")
}

/// Where the remembered answers live.
fn answers_file(global: &Path) -> PathBuf {
    global.join("tools").join("answers.toml")
}

/// What somebody said last time they were asked about an engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Answer {
    /// Yes, and it worked.
    Installed,
    /// No. Not to be asked again.
    Declined,
}

/// The file of remembered answers, one per engine.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Remembered {
    #[serde(default)]
    answers: BTreeMap<String, Answer>,
}

impl Remembered {
    fn load(global: &Path) -> Self {
        // A file that will not parse is a file from a future sloop or a half-written one,
        // and neither is worth failing a backup over. The worst an unreadable one can do is
        // ask a question that was already answered.
        std::fs::read_to_string(answers_file(global))
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn get(&self, engine: Engine) -> Option<Answer> {
        self.answers.get(engine.scheme()).copied()
    }

    fn set(&mut self, engine: Engine, answer: Answer, global: &Path) {
        self.answers.insert(engine.scheme().to_owned(), answer);

        let path = answers_file(global);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = toml::to_string_pretty(self) {
            let _ = std::fs::write(&path, text);
        }
    }
}

/// Write down an answer without asking for it. The tests use this to set up the case
/// where somebody has already said no.
#[cfg(test)]
pub fn remember(engine: Engine, answer: Answer, global: &Path) {
    Remembered::load(global).set(engine, answer, global);
}

/// Make sure this engine's tools are on the machine, asking once if they are not.
///
/// The inventory that comes back is the one to use: after an install it has been taken
/// again, so it names the programs that have just arrived rather than the absence that was
/// there a moment ago.
pub fn ensure(engine: Engine, global: &Path) -> Outcome<Inventory> {
    let fetched = fetched_dir(global);
    let inventory = Inventory::for_engine(engine, &fetched);
    if inventory.has_everything_for(engine) {
        return Ok(inventory);
    }

    let missing = inventory.missing_for(engine);
    let mut remembered = Remembered::load(global);

    if remembered.get(engine) == Some(Answer::Declined) {
        return Err(refused_before(engine, &missing));
    }

    if !std::io::stdin().is_terminal() {
        return Err(no_terminal(engine, &missing));
    }

    let Some(plan) = plan_for(engine) else {
        return Err(nothing_to_offer(engine, &missing));
    };

    crate::say!(
        "{} {} for {engine}, and sloop does not bundle them.",
        style::heading("Missing:"),
        missing
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    );
    crate::say!("{}", plan.describe());

    if !asked(&plan.question())? {
        remembered.set(engine, Answer::Declined, global);
        return Err(declined_now(engine, &missing));
    }

    plan.carry_out(&fetched)?;

    let after = Inventory::for_engine(engine, &fetched);
    if !after.has_everything_for(engine) {
        return Err(Failure::new(
            Exit::Usage,
            format!(
                "the install finished but {} still cannot be found",
                after
                    .missing_for(engine)
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
        .hint("`sloop doctor` says where it looked"));
    }

    remembered.set(engine, Answer::Installed, global);
    crate::say!("{} {engine}'s tools are ready.", style::heading("Done."));
    Ok(after)
}

/// Carry out the plan for an engine without asking first. Only the ignored end-to-end test
/// uses this; every other route goes through [`ensure`], which asks.
#[cfg(test)]
pub fn install_for_tests(engine: Engine, into: &Path) -> Outcome<()> {
    plan_for(engine)
        .ok_or_else(|| {
            Failure::new(Exit::Usage, "nothing to install on this platform")
                .hint("install the engine's client tools with this platform's package manager")
        })?
        .carry_out(into)
}

/// Get a whole PostgreSQL 18 — the server, not just the client programs.
///
/// **The same archive, the same checksum, a different set of members.** `R6` already proved
/// it received the pinned EnterpriseDB build intact; a server is what comes out when `share`
/// and `initdb` are taken out of it too. So there is one download path in sloop, one pinned
/// hash, and one place that shells out to the system's own `curl`.
///
/// Off Windows there is no such archive, and the answer is the machine's own package manager
/// — the same offer `R6` makes, for the package that carries the server rather than the
/// client.
pub fn postgres_server(global: &Path) -> Outcome<()> {
    let into = crate::server::fetched_dir(global);
    let wanted = crate::server::WANTED_MAJOR;

    if !std::io::stdin().is_terminal() {
        return Err(Failure::new(
            Exit::Usage,
            format!(
                "sloop keeps its own state in PostgreSQL {wanted}, this machine has none, and \
                 there is no terminal to ask about installing one at"
            ),
        )
        .hint("run `sloop setup` in a terminal once"));
    }

    let Some(plan) = server_plan() else {
        return Err(Failure::new(
            Exit::Usage,
            format!("sloop has no way to install PostgreSQL {wanted} on this machine"),
        )
        .hint(format!(
            "install PostgreSQL {wanted} yourself, then run `sloop setup` again"
        )));
    };

    crate::say!(
        "{} sloop keeps its own state in PostgreSQL {wanted}, and this machine has none.",
        style::heading("Needed:")
    );
    crate::say!("{}", plan.describe());

    if !asked(&plan.question())? {
        return Err(Failure::new(
            Exit::Usage,
            format!("PostgreSQL {wanted} is needed and that was declined"),
        )
        .hint(format!(
            "install PostgreSQL {wanted} yourself, then run `sloop setup` again"
        )));
    }

    match plan {
        Plan::FetchPostgresForWindows(release) => {
            std::fs::create_dir_all(&into).map_err(|error| {
                Failure::new(
                    Exit::Usage,
                    format!("could not create {}: {error}", into.display()),
                )
            })?;
            let archive = fetch_verified(release, &into)?;
            extract_server(&archive, &into)?;
            let _ = std::fs::remove_file(&archive);
            crate::say!("  {} {}", style::label("Installed into"), into.display());
            Ok(())
        }
        Plan::PackageManager(offer) => offer.run(),
    }
}

/// What this platform can offer for a PostgreSQL *server*.
///
/// The client-tool offer asks for `postgresql-client` and `libpq`, neither of which can hold
/// a database. This asks for the server package of the exact major sloop needs — pinned,
/// because "whatever `postgresql` means today" is how a machine ends up with 16.
fn server_plan() -> Option<Plan> {
    let wanted = crate::server::WANTED_MAJOR;

    if cfg!(windows) {
        let release = releases::newest();
        // The pinned archive has to actually be the major sloop wants. It is today, and this
        // is what makes the day it stops being true a refusal rather than a cluster on the
        // wrong version.
        return (release.major == wanted).then_some(Plan::FetchPostgresForWindows(release));
    }

    if cfg!(target_os = "macos") && on_path("brew") {
        return Some(Plan::PackageManager(Offer {
            manager: "Homebrew",
            command: vec![
                "brew".to_owned(),
                "install".to_owned(),
                format!("postgresql@{wanted}"),
            ],
        }));
    }
    if on_path("apt-get") {
        return Some(Plan::PackageManager(Offer {
            manager: "apt",
            command: vec![
                "sudo".to_owned(),
                "apt-get".to_owned(),
                "install".to_owned(),
                "-y".to_owned(),
                format!("postgresql-{wanted}"),
            ],
        }));
    }
    if on_path("dnf") {
        return Some(Plan::PackageManager(Offer {
            manager: "dnf",
            command: vec![
                "sudo".to_owned(),
                "dnf".to_owned(),
                "install".to_owned(),
                "-y".to_owned(),
                format!("postgresql{wanted}-server"),
            ],
        }));
    }

    None
}

/// What sloop would do about a missing engine on this machine.
pub(super) enum Plan {
    /// Download the pinned PostgreSQL archive and take three programs out of it.
    FetchPostgresForWindows(&'static releases::Release),
    /// Let the machine's own package manager do it.
    PackageManager(Offer),
}

/// A package manager, and the exact command that would be run.
pub(super) struct Offer {
    manager: &'static str,
    command: Vec<String>,
}

impl Plan {
    /// The question, and what answering yes commits to.
    pub(super) fn describe(&self) -> String {
        match self {
            Self::FetchPostgresForWindows(release) => format!(
                "  sloop can download the official {release} binaries for Windows \
                 ({} MB), check them against a SHA-256 built into this binary, and keep \
                 pg_dump, pg_restore and psql.\n  The download is the system's own curl. \
                 Nothing about this machine is sent anywhere.",
                release.bytes / 1_000_000
            ),
            Self::PackageManager(offer) => format!(
                "  sloop can ask {} to install them:\n    {}",
                offer.manager,
                offer.command.join(" ")
            ),
        }
    }

    pub(super) fn question(&self) -> String {
        match self {
            Self::FetchPostgresForWindows(_) => "Download and install them now?".to_owned(),
            Self::PackageManager(offer) => format!("Run that {} command now?", offer.manager),
        }
    }

    fn carry_out(&self, into: &Path) -> Outcome<()> {
        match self {
            Self::FetchPostgresForWindows(release) => install_postgres_archive(release, into),
            Self::PackageManager(offer) => offer.run(),
        }
    }
}

impl Offer {
    fn run(&self) -> Outcome<()> {
        let (program, arguments) = self
            .command
            .split_first()
            .expect("an offer always names a program");

        // Every stream inherited, on purpose: the package manager has its own progress to
        // show and `sudo` has a password to ask for, and neither works down a pipe.
        let status = Command::new(program)
            .args(arguments)
            .status()
            .map_err(|error| {
                Failure::new(Exit::Usage, format!("could not run {program}: {error}"))
            })?;

        if status.success() {
            return Ok(());
        }

        Err(Failure::new(
            Exit::Usage,
            format!("{} did not finish successfully", self.manager),
        )
        .hint(format!("run it yourself: {}", self.command.join(" "))))
    }
}

/// Is there anything sloop could actually do about this engine on this machine?
///
/// `doctor` asks before it offers. Calling [`ensure`] on an engine with no plan would print
/// a refusal in red for something that is not a failure — there is simply nothing to
/// install here, and that belongs in the report as a line about where to get them.
#[must_use]
pub fn can_offer(engine: Engine) -> bool {
    plan_for(engine).is_some()
}

/// What this platform can offer for this engine.
pub(super) fn plan_for(engine: Engine) -> Option<Plan> {
    if cfg!(windows) {
        // Only PostgreSQL. MySQL and MariaDB publish installers rather than a plain archive
        // with a stable name, and an installer is not something to run at somebody without
        // them watching — so those two are told where to go instead of being fetched.
        return match engine {
            Engine::Postgres => Some(Plan::FetchPostgresForWindows(releases::newest())),
            Engine::Mysql | Engine::Mariadb => None,
        };
    }

    package_manager_offer(engine).map(Plan::PackageManager)
}

/// The package manager this machine has, and what to ask it for.
///
/// The distribution's own package, not a third-party repository. It is the one that stays
/// working across upgrades, and on every distribution that matters it is new enough to dump
/// the servers that distribution ships. `sloop doctor` says the version afterwards, which is
/// where somebody finds out they want PGDG's newer one instead.
fn package_manager_offer(engine: Engine) -> Option<Offer> {
    let packages =
        |apt: &str, dnf: &str, brew: &str| (apt.to_owned(), dnf.to_owned(), brew.to_owned());

    let (apt, dnf, brew) = match engine {
        Engine::Postgres => packages("postgresql-client", "postgresql", "libpq"),
        Engine::Mysql => packages("mysql-client", "mysql", "mysql-client"),
        Engine::Mariadb => packages("mariadb-client", "mariadb", "mariadb"),
    };

    if cfg!(target_os = "macos") && on_path("brew") {
        return Some(Offer {
            manager: "Homebrew",
            command: vec!["brew".to_owned(), "install".to_owned(), brew],
        });
    }
    if on_path("apt-get") {
        return Some(Offer {
            manager: "apt",
            command: vec![
                "sudo".to_owned(),
                "apt-get".to_owned(),
                "install".to_owned(),
                "-y".to_owned(),
                apt,
            ],
        });
    }
    if on_path("dnf") {
        return Some(Offer {
            manager: "dnf",
            command: vec![
                "sudo".to_owned(),
                "dnf".to_owned(),
                "install".to_owned(),
                "-y".to_owned(),
                dnf,
            ],
        });
    }

    None
}

fn on_path(program: &str) -> bool {
    let name = if cfg!(windows) {
        format!("{program}.exe")
    } else {
        program.to_owned()
    };
    on_path_at(&name).is_some()
}

/// The same question, answered with *where* rather than *whether*.
///
/// **`R19d` needs the path, not the yes.** Verifying a MySQL archive runs `gpg`, and running
/// a program found on `PATH` by full path is what keeps it the one that was looked for.
/// `name` carries its own extension, because the caller already knows which it wants.
#[must_use]
pub fn on_path_at(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|directory| directory.join(name))
            .find(|candidate| candidate.is_file())
    })
}

/// Download, prove, unpack the client programs.
fn install_postgres_archive(release: &releases::Release, into: &Path) -> Outcome<()> {
    std::fs::create_dir_all(into).map_err(|error| {
        Failure::new(
            Exit::Usage,
            format!("could not create {}: {error}", into.display()),
        )
    })?;

    let archive = fetch_verified(release, into)?;
    extract(&archive, into)?;

    // A third of a gigabyte is not worth keeping for the 51 MB that came out of it.
    let _ = std::fs::remove_file(&archive);

    crate::say!("  {} {}", style::label("Installed into"), into.display());
    Ok(())
}

/// Download the pinned archive and prove it is the one, leaving it beside `into`.
///
/// **One path, two callers.** The client tools and the whole server come out of the same
/// archive, and a second copy of "download, check the size, check the hash" is a second
/// chance for one of them to skip a step.
fn fetch_verified(release: &releases::Release, into: &Path) -> Outcome<PathBuf> {
    let workspace = into
        .parent()
        .map_or_else(|| into.to_path_buf(), Path::to_path_buf)
        .join("download");
    std::fs::create_dir_all(&workspace).map_err(|error| {
        Failure::new(
            Exit::Usage,
            format!("could not create {}: {error}", workspace.display()),
        )
    })?;

    // The index, first and optionally. It decides nothing — see `releases` — so a machine
    // that cannot reach postgresql.org still installs, it just installs without a comment.
    if let Some(note) = index_note(&workspace, release) {
        crate::say!("  {}", style::dim(&note));
    }

    let archive = workspace.join(release.file_name());
    crate::note!("  {}", style::dim(&release.url()));
    fetching(&release.url(), &archive, "Downloading", Some(release.bytes))?;

    verify(&archive, release).inspect_err(|_| {
        // Nothing that failed its check is left lying about to be picked up by a later run
        // that might be less careful.
        let _ = std::fs::remove_file(&archive);
    })?;
    crate::say!("  {} SHA-256 matches", style::label("Verified"));

    Ok(archive)
}

/// Run the downloader, drawing how far it has got until it finishes.
///
/// **The file is watched, not the program.** Nothing here parses another tool's output: the
/// three downloaders draw three different meters, all of them to a terminal the menu is
/// holding, and none of them has a machine-readable mode worth relying on. What every one of
/// them does have is a file on disk that grows, and that is a number this can read every
/// tenth of a second without knowing which program is writing it.
///
/// Returns what it exited with and whatever it said about why, which is read back from a file
/// rather than a pipe: a pipe nobody drains while the child runs is a download that stops
/// when the buffer fills.
fn watched(
    program: &str,
    arguments: &[String],
    to: &Path,
    what: &str,
    total: Option<u64>,
) -> std::io::Result<(std::process::ExitStatus, String)> {
    let log = to.with_extension("sloop-download-log");
    let said = std::fs::File::create(&log).ok();

    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(said.map_or_else(Stdio::null, Stdio::from))
        .spawn()?;

    let step = crate::console::step(what, past(what));
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        let done = std::fs::metadata(to).map_or(0, |meta| meta.len());
        step.at(done, total, &rate(done, total, started));
        std::thread::sleep(POLL);
    };

    let landed = std::fs::metadata(to).map_or(0, |meta| meta.len());
    if status.success() {
        step.ok(&crate::console::bytes(landed));
    } else {
        step.bad("");
    }

    let complaint = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::remove_file(&log);
    Ok((status, complaint.trim().to_owned()))
}

/// How often the file on disk is measured.
///
/// **A tenth of a second**, which is a `stat` ten times a second against a download that
/// takes minutes — and slightly faster than the spinner, so the bar never shows a number the
/// frame beside it has already moved past.
const POLL: std::time::Duration = std::time::Duration::from_millis(100);

/// How fast it is going and how much longer it has, in the phrase beside the bar.
fn rate(done: u64, total: Option<u64>, since: std::time::Instant) -> String {
    let seconds = since.elapsed().as_secs_f64();
    if seconds < 1.0 || done == 0 {
        return String::new();
    }
    #[allow(clippy::cast_precision_loss)]
    let per_second = done as f64 / seconds;
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let speed = format!("{}/s", crate::console::bytes(per_second as u64));

    let Some(total) = total.filter(|total| *total > done) else {
        return speed;
    };
    #[allow(clippy::cast_precision_loss)]
    let left = (total - done) as f64 / per_second;
    if !left.is_finite() || left > 86_400.0 {
        return speed;
    }
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let left = left.round() as u64;
    format!("{speed} \u{00b7} {}m {:02}s left", left / 60, left % 60)
}

/// A present-tense label in the past tense, for the line the step settles into.
fn past(what: &str) -> &str {
    match what {
        "Downloading" => "Downloaded",
        other => other,
    }
}

/// Ask postgresql.org what it has, and turn that into one sentence or none.
fn index_note(workspace: &Path, release: &releases::Release) -> Option<String> {
    let index = workspace.join("versions.json");
    fetching(
        releases::INDEX_URL,
        &index,
        "Asking postgresql.org what it has",
        None,
    )
    .ok()?;
    let json = std::fs::read_to_string(&index).ok()?;
    let _ = std::fs::remove_file(&index);
    releases::what_the_index_adds(&releases::read_index(&json), release)
}

/// Fetch a URL to a file, using whatever this machine already has.
///
/// In order of preference, and every one of them is a program that was already installed:
/// `curl`, then PowerShell's `Invoke-WebRequest`, then `wget`. There is no fourth option and
/// there is deliberately no HTTP client in this binary to fall back on.
pub fn download(url: &str, to: &Path) -> Outcome<()> {
    fetching(url, to, "Downloading", None)
}

/// The same, saying what is being fetched and how big it is.
///
/// **Rule 6 of the owner's list** — *"it shows default curl ui of download, not a sloop
/// custom progressbar"*. `total` is what the catalogue says the archive weighs, which is the
/// only way to draw a bar for a download this binary is deliberately not performing itself:
/// the file on disk is watched as it grows, and the fraction is what has landed over what was
/// promised. A `None` total still gets a spinner and a running byte count, because the size
/// of the thing is not always known and *"is it moving"* is most of the question.
pub fn fetching(url: &str, to: &Path, what: &str, total: Option<u64>) -> Outcome<()> {
    let mut attempts: Vec<(&str, Vec<String>)> = Vec::new();

    if on_path("curl") {
        attempts.push((
            "curl",
            vec![
                "--fail".to_owned(),
                "--location".to_owned(),
                // **Its own meter off, because sloop draws one now.** `curl` writing a
                // progress bar to the terminal was fine when a download happened on the
                // terminal; the menu holds the screen, and two things drawing on it is one
                // too many. `--show-error` keeps the sentence it prints when it fails.
                "--silent".to_owned(),
                "--show-error".to_owned(),
                // https and nothing else, at both ends of a redirect chain.
                "--proto".to_owned(),
                "=https".to_owned(),
                "--proto-redir".to_owned(),
                "=https".to_owned(),
                "--tlsv1.2".to_owned(),
                "--max-time".to_owned(),
                DOWNLOAD_TIMEOUT_SECONDS.to_string(),
                "--output".to_owned(),
                to.display().to_string(),
                url.to_owned(),
            ],
        ));
    }

    if cfg!(windows) {
        attempts.push((
            "powershell",
            vec![
                "-NoProfile".to_owned(),
                "-NonInteractive".to_owned(),
                "-Command".to_owned(),
                format!(
                    "$ProgressPreference='SilentlyContinue'; \
                     Invoke-WebRequest -Uri '{url}' -OutFile '{}' -UseBasicParsing",
                    to.display()
                ),
            ],
        ));
    }

    if on_path("wget") {
        attempts.push((
            "wget",
            vec![
                "--https-only".to_owned(),
                // The same: no meter of its own. See the note on `curl` above.
                "--no-verbose".to_owned(),
                "--timeout".to_owned(),
                "60".to_owned(),
                "-O".to_owned(),
                to.display().to_string(),
                url.to_owned(),
            ],
        ));
    }

    if attempts.is_empty() {
        return Err(Failure::new(
            Exit::Usage,
            "this machine has no curl, no wget and no PowerShell, so there is nothing here \
             that can download anything",
        )
        .hint(format!("fetch {url} by hand and see `sloop doctor`")));
    }

    let mut last = String::new();
    for (program, arguments) in attempts {
        match watched(program, &arguments, to, what, total) {
            Ok((status, _)) if status.success() => return Ok(()),
            Ok((status, said)) if said.is_empty() => {
                last = format!("{program} exited with {status}");
            }
            Ok((_, said)) => last = format!("{program}: {said}"),
            Err(error) => last = format!("{program} would not run: {error}"),
        }
        let _ = std::fs::remove_file(to);
    }

    Err(
        Failure::new(Exit::Usage, format!("could not download {url}: {last}")).hint(
            "sloop has no network code of its own — it asks this machine's own downloader, so a \
         proxy or a firewall that blocks it blocks this. Installing the client tools by hand \
         works just as well; `sloop doctor` says where it looks for them",
        ),
    )
}

/// The size and then the hash, in that order.
///
/// Size first because it is free and it catches the common failure: a server that answers a
/// missing file with a courtesy page and a 200, which would otherwise be hashed in full
/// before anybody discovered it was HTML.
fn verify(archive: &Path, release: &releases::Release) -> Outcome<()> {
    let size = std::fs::metadata(archive)
        .map_err(|error| {
            Failure::new(
                Exit::Usage,
                format!(
                    "{} is not there after downloading it: {error}",
                    archive.display()
                ),
            )
        })?
        .len();

    if size != release.bytes {
        return Err(Failure::new(
            Exit::Usage,
            format!(
                "{} should be {} bytes and is {size}",
                release.file_name(),
                release.bytes
            ),
        )
        .hint(
            "that is what a download interrupted, or an error page served as a file, looks like",
        ));
    }

    let found = sha256_of(archive)?;
    if found != release.sha256 {
        return Err(Failure::new(
            Exit::Usage,
            format!(
                "{} does not match the checksum sloop holds for it",
                release.file_name()
            ),
        )
        .hint(format!("expected {}, got {found}", release.sha256)));
    }

    Ok(())
}

/// SHA-256 of a file, read a block at a time so a 300 MB archive is not held in memory.
pub fn sha256_of(path: &Path) -> Outcome<String> {
    let mut file = std::fs::File::open(path).map_err(|error| {
        Failure::new(
            Exit::Usage,
            format!("could not read {}: {error}", path.display()),
        )
    })?;

    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            Failure::new(
                Exit::Usage,
                format!("could not read {}: {error}", path.display()),
            )
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            use std::fmt::Write as _;
            let _ = write!(hex, "{byte:02x}");
            hex
        }))
}

/// Take the three programs and their libraries out of the archive, and nothing else.
///
/// The system's own archiver, for the same reason the download is the system's own curl:
/// a zip reader is a dependency, and this project's headline claim is about what is not in
/// its dependency graph. `--strip-components=2` drops the `pgsql/bin/` in front of every
/// name, so what lands is a flat directory of programs.
///
/// **Not whatever `tar` is on `PATH`.** On Windows that is very often MSYS's GNU tar, which
/// cannot read a zip at all; the one that can is the `bsdtar` Windows itself ships in
/// System32, and it is asked for by its full path.
fn extract(archive: &Path, into: &Path) -> Outcome<()> {
    unpack(
        archive,
        into,
        "--strip-components=2",
        &[
            "pgsql/bin/*.dll",
            "pgsql/bin/pg_dump*",
            "pgsql/bin/pg_restore*",
            "pgsql/bin/psql*",
        ],
    )
}

/// Take a whole PostgreSQL out of the same archive: the programs, the libraries they need,
/// and `share`, which `initdb` reads its templates out of and cannot create a cluster
/// without.
///
/// `--strip-components=1` rather than 2, because the layout has to survive: every one of
/// these programs finds `share` by walking up from its own directory, and a flat `bin` would
/// give an `initdb` that cannot find `postgres.bki`.
fn extract_server(archive: &Path, into: &Path) -> Outcome<()> {
    unpack(
        archive,
        into,
        "--strip-components=1",
        &["pgsql/bin", "pgsql/lib", "pgsql/share"],
    )
}

/// Unpack a whole archive, dropping the one directory the project wrapped it in.
///
/// **`R19d`'s installs, where the answer is "all of it".** The two above take four programs
/// out of a third of a gigabyte because that is all a dump needs; a *server* needs the
/// programs, the libraries, the share directory its bootstrap reads its templates out of and
/// the plugins it loads at startup — so naming members would be naming the ones today's
/// release happens to have, and a list one name short is a server that will not come up.
///
/// `--strip-components=1` because every one of these projects wraps everything in a single
/// directory named after the release: `mysql-8.4.11-winx64/`, `mariadb-11.4.4-winx64/`.
/// Keeping it would put the server one level deeper than the record says it is.
pub fn unpack_whole(archive: &Path, into: &Path) -> Outcome<()> {
    let archiver = system_archiver();

    let status = Command::new(&archiver)
        .current_dir(into)
        .arg("-xf")
        .arg(archive)
        .arg("--strip-components=1")
        .stdin(Stdio::null())
        .status()
        .map_err(|error| {
            Failure::new(
                Exit::Usage,
                format!("could not run {}: {error}", archiver.display()),
            )
            .hint(
                "unpacking the archive needs the system's own tar, which Windows 10 and later \
                 include",
            )
        })?;

    if !status.success() {
        return Err(Failure::new(
            Exit::Usage,
            format!("could not unpack {}", archive.display()),
        )
        .hint("nothing has been started"));
    }

    Ok(())
}

/// The system's own archiver, on the members named.
fn unpack(archive: &Path, into: &Path, strip: &str, members: &[&str]) -> Outcome<()> {
    let archiver = system_archiver();

    let status = Command::new(&archiver)
        .current_dir(into)
        .arg("-xf")
        .arg(archive)
        .arg(strip)
        // pgAdmin's own libraries, which are most of the archive and none of our business.
        .arg("--exclude")
        .arg("pgsql/bin/wx*")
        .arg("--exclude")
        .arg("pgsql/bin/testplug.dll")
        // Every library beside them rather than a list of the ones today's build happens to
        // need: the dependency set moves between releases, and a list that is one name short
        // produces a pg_dump that will not start.
        //
        // Every pattern here has to match something. The archiver treats one that matches
        // nothing as an error, which is a good property — a release that stopped shipping
        // `pg_restore` should fail loudly — but it means these lists are Windows-shaped on
        // purpose, because this archive is the Windows one and there is no `.so` in it.
        .args(members)
        .stdin(Stdio::null())
        .status()
        .map_err(|error| {
            Failure::new(
                Exit::Usage,
                format!("could not run {}: {error}", archiver.display()),
            )
            .hint(
                "unpacking the archive needs the system's own tar, which Windows 10 and \
                   later include",
            )
        })?;

    if !status.success() {
        return Err(Failure::new(
            Exit::Usage,
            format!("could not unpack {}", archive.display()),
        )
        .hint("the download may be truncated — delete it and run this again"));
    }

    Ok(())
}

/// The archiver that can read a zip.
fn system_archiver() -> PathBuf {
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot")
            .map_or_else(|| PathBuf::from("C:/Windows"), PathBuf::from);
        let bsdtar = root.join("System32").join("tar.exe");
        if bsdtar.is_file() {
            return bsdtar;
        }
    }
    PathBuf::from("tar")
}

/// Ask a yes-or-no question. Anything that is not a yes is a no.
fn asked(question: &str) -> Outcome<bool> {
    Ok(is_yes(&crate::console::ask(&format!("{question} [y/N] "))?))
}

/// Anything that is not plainly a yes is a no.
///
/// Separated from the reading so the rule can be checked without a terminal, and written
/// this way round on purpose: the question leads to a download, and a stray keypress or an
/// empty line must never be the thing that starts one.
#[must_use]
pub fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// The instructions somebody needs when sloop is not going to install anything.
pub fn how_to_install(engine: Engine) -> String {
    match (cfg!(windows), cfg!(target_os = "macos"), engine) {
        (true, _, Engine::Postgres) => {
            "install PostgreSQL's client tools, or let sloop fetch them: run `sloop doctor` \
             in a terminal"
                .to_owned()
        }
        (true, _, Engine::Mysql) => {
            "install MySQL's client tools from dev.mysql.com and put their `bin` on PATH".to_owned()
        }
        (true, _, Engine::Mariadb) => {
            "install MariaDB's client tools from mariadb.org and put their `bin` on PATH".to_owned()
        }
        (_, true, _) => package_manager_offer(engine).map_or_else(
            || format!("install {engine}'s client tools with Homebrew"),
            |offer| offer.command.join(" "),
        ),
        _ => package_manager_offer(engine).map_or_else(
            || format!("install {engine}'s client tools with this system's package manager"),
            |offer| offer.command.join(" "),
        ),
    }
}

fn names(missing: &[Tool]) -> String {
    missing
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn missing_failure(engine: Engine, missing: &[Tool], why: &str) -> Failure {
    Failure::new(
        Exit::Usage,
        format!("{engine} needs {}, and {why}", names(missing)),
    )
    .hint(how_to_install(engine))
}

fn refused_before(engine: Engine, missing: &[Tool]) -> Failure {
    missing_failure(
        engine,
        missing,
        "sloop was told once not to install them, so it has not asked again",
    )
}

fn declined_now(engine: Engine, missing: &[Tool]) -> Failure {
    missing_failure(engine, missing, "that was declined")
}

fn nothing_to_offer(engine: Engine, missing: &[Tool]) -> Failure {
    missing_failure(
        engine,
        missing,
        "sloop has no way to install them on this machine",
    )
}

fn no_terminal(engine: Engine, missing: &[Tool]) -> Failure {
    Failure::new(
        Exit::Usage,
        format!(
            "{engine} needs {}, and there is no terminal to ask about installing them at",
            names(missing)
        ),
    )
    .hint(format!(
        "run `sloop doctor` in a terminal once, or {}",
        how_to_install(engine)
    ))
}
