//! What the menu runs, which is the commands, unchanged.
//!
//! **Not a second implementation of anything.** Every job below builds the same argument
//! struct `main` builds from `argv` and calls the same function — `db::add`, `backup::run`,
//! `mirror::run`. A menu with its own copy of what `mirror` does is a menu that is subtly
//! wrong about one of them within a release, and the two surfaces would then have to be
//! tested twice and would still disagree. So the flows in [`crate::ui::flow`] collect
//! exactly what a flag carries, this turns the answers back into flags, and there is one
//! implementation of every command.
//!
//! **The registries are opened per job, not held.** A menu is a long session and a command
//! changes the registry underneath it — `db add` on the second screen, `db remove` on the
//! fifth. Reading them once at the top and keeping them would mean every screen after the
//! first showing a world that no longer exists.

use std::path::{Path, PathBuf};

use crate::cli::{Fields, PasswordSource, SshFields};
use crate::commands;
use crate::consent::Consent;
use crate::exit::Exit;
use crate::failure::{Failure, Outcome};
use crate::registry::locations::Locations;
use crate::registry::{Disk, Registries, Resolution, resolve};
use crate::ssh::tunnel::Tunnels;
use crate::ui::flow::{Answers, Doing, Job, field};

/// Everything a job needs to be able to open the world again.
pub struct Machine {
    /// The global store.
    global: PathBuf,
    /// Where `sloop` was run.
    cwd: PathBuf,
    /// Which registry this session resolved to, settled once at the start: a menu that
    /// re-resolved per job could walk into a different project halfway through a session.
    resolution: Resolution,
    /// `--password-command`, which outranks whatever route a record names.
    password_command: Option<String>,
    /// Every SSH forward this session holds.
    ///
    /// **Owned by the machine, not by a job, and that is the owner's whole sentence about
    /// this feature.** The registry is reopened per job; the forwards are not. Ten
    /// operations against one server in one menu session authenticate once — *"if he wants
    /// to query or something like that then it will login after each task which is not a
    /// proper apprach"* — and every one of them is closed when this value is dropped, which
    /// is when the session ends.
    tunnels: Tunnels,
}

impl Machine {
    /// Work out which registry this session reads, and keep what is needed to reopen it.
    pub fn new(
        locations: &Locations,
        global: bool,
        project: Option<&str>,
        environment: Option<&str>,
        password_command: Option<&str>,
    ) -> Outcome<Self> {
        let store = crate::registry::adopt::global(locations)?;
        let home = locations.home_dir()?;
        let cwd = std::env::current_dir().map_err(|error| {
            Failure::usage(format!("cannot read the working directory: {error}"))
        })?;
        let resolution = resolve(&cwd, &Disk::new(&store, home), global, project, environment)?;

        Ok(Self {
            global: store,
            cwd,
            resolution,
            password_command: password_command.map(ToOwned::to_owned),
            tunnels: Tunnels::new()?,
        })
    }

    /// Where the registry is.
    #[must_use]
    pub fn global(&self) -> &Path {
        &self.global
    }

    /// Where `sloop` was run.
    #[must_use]
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// Which registry, and why.
    #[must_use]
    pub const fn resolution(&self) -> &Resolution {
        &self.resolution
    }

    /// Find or install sloop's PostgreSQL, make its database, and create its tables.
    ///
    /// The same `server::set_up` the `sloop setup` command runs, with the same reporting —
    /// the menu has already handed the terminal back, so every line it prints lands in the
    /// scrollback the user keeps.
    ///
    /// **No flags, because a menu has a terminal.** `--superuser-password-command` and
    /// `--superuser-password-stdin` exist for a run with nobody to ask; here there is
    /// somebody, and `rpassword` asks them.
    fn set_up(&self) -> Outcome<Exit> {
        let settled = crate::server::set_up(
            &self.global,
            &crate::server::make::Asking {
                command: None,
                stdin: false,
            },
            &crate::server::own::Choosing::unsupplied(),
        )?;
        crate::server::announce(&settled.ready);
        crate::server::announce_own(&settled);
        Ok(Exit::Success)
    }

    /// Both registries, read fresh.
    fn open(&self) -> Outcome<Registries> {
        Registries::open(self.resolution.clone(), &self.global)
    }

    /// The registry this session writes to, as a directory — where backups live.
    fn store_root(&self) -> Option<PathBuf> {
        let registries = self.open().ok()?;
        registries.root_in(registries.writes_to())
    }

    /// What a `db` command is handed.
    ///
    /// **Consent is `--yes` and nothing more, and that is deliberate.** A menu cannot carry
    /// `--force`, because overriding a refusal is a thing somebody should have to say in so
    /// many words rather than find behind a menu item; and it cannot carry `--confirm`,
    /// because rule 5 says a destructive operation is typed. The command asks for the name
    /// itself, on the terminal it has just been handed, exactly as it does from a shell.
    fn context(&self, registries: Registries) -> commands::db::Context<'_> {
        commands::db::Context {
            registries,
            password_command: self.password_command.as_deref(),
            global: &self.global,
            consent: Consent::given(false, false, None),
            tunnels: &self.tunnels,
        }
    }
}

impl Doing for Machine {
    fn databases(&self) -> Vec<String> {
        let Ok(registries) = self.open() else {
            return Vec::new();
        };

        // **This project's, then the global store's, and no other project's.** The order is
        // `Registries::all`'s already — sorting the two scopes together would throw it away,
        // and a picker that offers `staging` above the `staging` you are standing in is a
        // picker that runs the wrong command. Names within a scope are alphabetical because
        // the registry is a `BTreeMap`; the scopes stay in search order.
        let mut names: Vec<String> = Vec::new();
        for (_, name, _) in registries.all() {
            // A project entry shadows a global one of the same name, so the far one is not
            // a second choice — it is the same choice, resolving somewhere else.
            if !names.iter().any(|seen| seen == name) {
                names.push(name.to_owned());
            }
        }
        names
    }

    fn on_the_server(&self, label: &str) -> Option<String> {
        let registries = self.open().ok()?;
        let (_, database) = registries.find(label).ok()?;
        Some(database.database.clone())
    }

    fn backups_of(&self, name: &str) -> Vec<String> {
        let Some(root) = self.store_root() else {
            return Vec::new();
        };
        let Ok(found) = crate::backup::store::scan(&root, crate::backup::store::Check::Size) else {
            return Vec::new();
        };

        let label = crate::backup::store::label_for(name);
        found
            .backups
            .iter()
            .filter(|stored| stored.label == label && stored.is_complete())
            .filter_map(|stored| {
                stored
                    .directory
                    .file_name()
                    .map(|taken| taken.to_string_lossy().into_owned())
            })
            .collect()
    }

    fn run(&mut self, job: Job, answers: &Answers) -> Outcome<Exit> {
        if let Some(done) = self.before_the_registry(job, answers) {
            return done;
        }

        let registries = self.open()?;

        match job {
            Job::Setup | Job::ServerInstall | Job::ServerConnection => {
                unreachable!("handled above, before the registries are opened")
            }
            Job::DbAdd => self.add(registries, answers),
            Job::DbCreate => self.create(registries, answers),
            Job::DbList => Ok(commands::db::list(&self.context(registries))),
            Job::DbTest => {
                commands::db::test(&self.context(registries), answers.some(field::WHICH))
            }
            Job::DbEdit => self.edit(registries, answers),
            Job::DbRename => commands::db::rename(
                &mut self.context(registries),
                answers.text(field::NAME),
                answers.text(field::RENAMED),
            ),
            Job::DbRemove => {
                commands::db::remove(&mut self.context(registries), answers.text(field::NAME))
            }
            Job::DbDrop => {
                commands::db::drop(&mut self.context(registries), answers.text(field::NAME))
            }

            Job::Backup => self.backup(registries, answers.some(field::NAME), answers),
            Job::BackupAll => self.backup(registries, None, answers),

            Job::BackupsList => commands::backups::list(
                &commands::backups::Context {
                    registries,
                    global: &self.global,
                    consent: Consent::given(false, false, None),
                },
                answers.some(field::WHICH),
                answers.yes(field::CHECK),
            ),

            Job::BackupsPrune => commands::backups::prune(
                &commands::backups::Context {
                    registries,
                    global: &self.global,
                    consent: Consent::given(false, false, None),
                },
                &commands::backups::Pruning {
                    name: answers.some(field::WHICH),
                    keep: answers.number(field::KEEP),
                    older_than: answers.some(field::OLDER),
                    dry_run: answers.yes(field::DRY),
                    include_broken: answers.yes(field::BROKEN),
                },
            ),

            Job::Restore => commands::restore::run(
                &commands::restore::Context {
                    registries,
                    global: &self.global,
                    password_command: self.password_command.as_deref(),
                    consent: Consent::given(false, false, None),
                    tunnels: &self.tunnels,
                },
                answers.text(field::NAME),
                answers.some(field::WHEN),
            ),

            Job::Mirror | Job::Sync => self.copy(job, registries, answers),

            Job::KeyExport => commands::key::export(&mut commands::key::Context {
                registries,
                global: &self.global,
            }),
            Job::KeyImport => commands::key::import(&mut commands::key::Context {
                registries,
                global: &self.global,
            }),

            Job::Query => commands::query::run(
                &commands::query::Context {
                    registries,
                    password_command: self.password_command.as_deref(),
                    global: &self.global,
                    tunnels: &self.tunnels,
                },
                &commands::query::Asking {
                    name: answers.some(field::NAME),
                    sql: None,
                },
            ),

            Job::Doctor => Ok(commands::doctor::run(
                &self.global,
                true,
                &commands::doctor::Registered {
                    registries: &registries,
                    from: self.resolution.describe(&self.global),
                    password_command: self.password_command.as_deref(),
                    offline: !answers.yes(field::OFFLINE),
                    tunnels: &self.tunnels,
                },
            )),
        }
    }
}

impl Machine {
    /// The three jobs that run **before** the registries are opened, and `None` for the rest.
    ///
    /// **Each one would otherwise fail with "run `sloop setup`" on the way in.** Setup is the
    /// job whose whole purpose is to make the thing every other job opens. Installing a
    /// server is what somebody reaches for on a machine with nothing on it yet, and declining
    /// to help because it has nothing on it yet would be the wrong answer. And where sloop's
    /// own database *is* comes out of `server.toml`, not out of the registry inside it — so
    /// it can be answered on a machine whose server is not even running.
    fn before_the_registry(&self, job: Job, answers: &Answers) -> Option<Outcome<Exit>> {
        match job {
            Job::Setup => Some(self.set_up()),
            Job::ServerInstall => Some(commands::server::install(
                &self.global,
                &commands::server::Installing {
                    engine: None,
                    version: None,
                    yes: false,
                },
            )),
            // A menu is a terminal, so the refusal that stops a password reaching a pipe
            // cannot fire and `--force` is never what answers this one.
            Job::ServerConnection => Some(commands::server::connection(
                &self.global,
                &commands::server::Showing {
                    password: answers.yes(field::PASSWORD),
                    force: false,
                },
            )),
            _ => None,
        }
    }

    /// `db add`, from the answers.
    fn add(&self, registries: Registries, answers: &Answers) -> Outcome<Exit> {
        let by_url = answers.text(field::HOW) == "url";
        commands::db::add(
            &mut self.context(registries),
            answers.text(field::NAME),
            by_url.then(|| answers.text(field::URL)),
            &fields(answers, !by_url),
            &password_source(answers),
            &ssh_fields(answers, true),
            answers.yes(field::TEST),
        )
    }

    /// `db create`, from the answers. The two names on the server and both passwords are
    /// the command's own questions, asked on the terminal it has just been handed.
    fn create(&self, registries: Registries, answers: &Answers) -> Outcome<Exit> {
        commands::db::create(
            &mut self.context(registries),
            &commands::db::Creating {
                name: answers.text(field::NAME),
                engine: answers.text(field::ENGINE),
                host: answers.some(field::HOST).unwrap_or("127.0.0.1"),
                port: answers.number(field::PORT),
                superuser: answers.some(field::SUPERUSER),
                superuser_password_stdin: false,
                superuser_password_command: None,
                database: None,
                role: None,
                role_password_stdin: false,
                role_password_command: None,
            },
        )
    }

    /// `db edit`, from the answers. One detail at a time, so every other field arrives as
    /// `None` and is left exactly as it was.
    fn edit(&self, registries: Registries, answers: &Answers) -> Outcome<Exit> {
        let detail = answers.text(field::DETAIL);
        commands::db::edit(
            &mut self.context(registries),
            answers.text(field::NAME),
            None,
            &fields(answers, detail != "password" && detail != "reach"),
            &password_source(answers),
            &ssh_fields(answers, detail == "reach"),
            answers.yes(field::TEST),
        )
    }

    /// `backup`, either one or all of them.
    fn backup(
        &self,
        registries: Registries,
        name: Option<&str>,
        answers: &Answers,
    ) -> Outcome<Exit> {
        let mode = if answers.text(field::MODE) == "replace" {
            commands::backup::Mode::Replace
        } else {
            commands::backup::Mode::Sequential
        };
        commands::backup::run(
            &mut commands::backup::Context {
                registries,
                global: &self.global,
                password_command: self.password_command.as_deref(),
                tunnels: &self.tunnels,
            },
            name,
            name.is_none(),
            mode,
        )
    }

    /// `mirror` and `sync`, which take the same answers.
    fn copy(&self, job: Job, registries: Registries, answers: &Answers) -> Outcome<Exit> {
        let making = answers.text(field::WHERE) == "new";
        let new = commands::mirror::New {
            host: making.then(|| answers.some(field::HOST)).flatten(),
            port: making.then(|| answers.number(field::PORT)).flatten(),
            superuser: making.then(|| answers.some(field::SUPERUSER)).flatten(),
            superuser_password_stdin: false,
            superuser_password_command: None,
            database: None,
            role: None,
            role_password_stdin: false,
            role_password_command: None,
        };
        // Owned here and borrowed into `Selection`, which holds a slice because from the
        // command line the patterns are `argv` and live as long as the process does.
        let patterns = answers.words(field::TABLES);
        let only = commands::tables::Selection {
            patterns: &patterns,
            with_references: answers.yes(field::REFERENCES),
        };

        let source = answers.text(field::SOURCE);
        let to = (!making).then(|| answers.text(field::TO));
        let create = making.then(|| answers.text(field::CREATE));
        let safe = answers.yes(field::SAFE);

        if job == Job::Mirror {
            commands::mirror::run(
                &mut commands::mirror::Context {
                    registries,
                    global: &self.global,
                    password_command: self.password_command.as_deref(),
                    consent: Consent::given(false, false, None),
                    tunnels: &self.tunnels,
                },
                &commands::mirror::Mirroring {
                    source,
                    to,
                    create,
                    safe,
                    new,
                    only,
                },
            )
        } else {
            commands::sync::run(
                &mut commands::sync::Context {
                    registries,
                    global: &self.global,
                    password_command: self.password_command.as_deref(),
                    consent: Consent::given(false, false, None),
                    tunnels: &self.tunnels,
                },
                &commands::sync::Syncing {
                    source,
                    to,
                    create,
                    safe,
                    new,
                    only,
                },
            )
        }
    }
}

/// The connection fields, as `db add` and `db edit` take them.
///
/// `wanted` is false when the answers were about something else entirely — a URL, or a
/// password — and then every field is `None`, which is what leaves a record alone.
fn fields(answers: &Answers, wanted: bool) -> Fields {
    if !wanted {
        return Fields {
            engine: None,
            host: None,
            port: None,
            database: None,
            user: None,
        };
    }

    Fields {
        engine: answers.some(field::ENGINE).map(ToOwned::to_owned),
        host: answers.some(field::HOST).map(ToOwned::to_owned),
        port: answers.number(field::PORT),
        database: answers.some(field::DATABASE).map(ToOwned::to_owned),
        user: answers.some(field::USER).map(ToOwned::to_owned),
    }
}

/// How the database is reached, as the flag surface names it — `R19e`.
///
/// `wanted` is false when the answers were about something else entirely, and then every
/// field is unset, which is what leaves a record's [`crate::ssh::Reach`] exactly as it was.
/// Written out field by field for the reason [`fields`] is: a flag added to `SshFields` and
/// silently defaulted here would be a question the menu quietly stopped asking.
///
/// **`--ssh-passphrase-stdin` is never set**, for the reason `password_stdin` never is: it
/// is the flag for a run with no terminal, and a menu is the opposite of that. The command
/// asks, hidden, on the terminal it has been handed.
fn ssh_fields(answers: &Answers, wanted: bool) -> SshFields {
    let blank = SshFields {
        ssh_host: None,
        ssh_port: None,
        ssh_user: None,
        ssh_identity: None,
        no_ssh: false,
        ssh_keyring: false,
        ssh_encrypted_file: false,
        ssh_env: None,
        ssh_passphrase_from: None,
        ssh_passphrase_stdin: false,
    };

    if !wanted {
        return blank;
    }

    // **"Straight at it" is `--no-ssh`, not silence.** Somebody who changes a tunnelled
    // database to a direct one has said something, and a menu that turned that into "leave
    // it alone" would be a screen that does nothing.
    if answers.text(field::REACH) != "ssh" {
        return SshFields {
            no_ssh: true,
            ..blank
        };
    }

    let route = answers.text(field::SSH_ROUTE);
    SshFields {
        ssh_host: answers.some(field::SSH_HOST).map(ToOwned::to_owned),
        ssh_port: answers.number(field::SSH_PORT),
        ssh_user: answers.some(field::SSH_USER).map(ToOwned::to_owned),
        ssh_identity: answers.some(field::SSH_IDENTITY).map(ToOwned::to_owned),
        no_ssh: false,
        ssh_keyring: route == "keyring",
        ssh_encrypted_file: route == "file",
        ssh_env: (route == "env")
            .then(|| answers.some(field::SSH_ENV))
            .flatten()
            .map(ToOwned::to_owned),
        ssh_passphrase_from: (route == "command")
            .then(|| answers.some(field::SSH_FROM_COMMAND))
            .flatten()
            .map(ToOwned::to_owned),
        ssh_passphrase_stdin: false,
    }
}

/// Where the password is kept, as the four routes the flag surface names.
///
/// **`password_stdin` is never set.** It is the flag for a run with no terminal, and a menu
/// is the opposite of that: the command asks, hidden, on the terminal it has been handed.
fn password_source(answers: &Answers) -> PasswordSource {
    let route = answers.text(field::ROUTE);
    PasswordSource {
        keyring: route == "keyring",
        encrypted_file: route == "file",
        env: (route == "env")
            .then(|| answers.some(field::ENV))
            .flatten()
            .map(ToOwned::to_owned),
        password_from: (route == "command")
            .then(|| answers.some(field::FROM_COMMAND))
            .flatten()
            .map(ToOwned::to_owned),
        password_stdin: false,
    }
}

#[cfg(test)]
#[path = "menu_tests.rs"]
mod tests;
