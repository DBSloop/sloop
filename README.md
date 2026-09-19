# sloop

Register your databases once, then back them up, restore them, mirror them and sync them —
from a menu or from a flag, on Windows, Linux and macOS.

**Your credentials never leave your machine.** No telemetry, no analytics, no update pings.
`cargo tree` on this repository shows no HTTP client in the dependency graph — there is
nothing in the binary that *could* phone home.

[![cli](https://github.com/DBSloop/sloop/actions/workflows/cli.yml/badge.svg)](https://github.com/DBSloop/sloop/actions/workflows/cli.yml)
[![crates.io](https://img.shields.io/crates/v/dbsloop.svg)](https://crates.io/crates/dbsloop)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

PostgreSQL, MySQL and MariaDB. Documentation at **[dbsloop.github.io](https://dbsloop.github.io)**.

---

## Install

```sh
# Linux and macOS
curl -fsSL https://dbsloop.github.io/install.sh | sh

# Windows
irm https://dbsloop.github.io/install.ps1 | iex
```

Restart your shell afterwards — the installer adds the binary to `PATH`, and an open shell
does not see that until it restarts.

| Platform | Where it goes |
|---|---|
| Linux, macOS | `~/.local/bin`, with a line appended to your shell's startup file |
| Windows | `%LOCALAPPDATA%\Programs\sloop\bin`, via `HKCU\Environment` |

Installer options — pass them after `sh -s --`, or as PowerShell parameters:

| Option | What it does |
|---|---|
| `--version <x.y.z>` | a release other than the latest |
| `--dir <path>` | somewhere other than the default |
| `--from <url\|dir>` | fetch from there instead of GitHub releases |
| `--no-modify-path` | do not touch `PATH` or any startup file |

```sh
curl -fsSL https://dbsloop.github.io/install.sh | sh -s -- --version 1.2.3
```

### Or take the binary yourself

Every [release](https://github.com/DBSloop/sloop/releases) carries one archive per platform
and a single `SHA256SUMS` over all of them: `x86_64` and `aarch64` for Linux (musl), macOS
and Windows. Unpack one and put `sloop` somewhere on your `PATH`.

### Or build it

```sh
cargo install dbsloop --version 0.1.0-rc.2

git clone https://github.com/DBSloop/sloop
cd sloop/cli && cargo install --path .
```

The crate is `dbsloop` and the command is `sloop` — `sloop` on crates.io belongs to an
abandoned 2019 project. The version is named because sloop is on release candidates until
1.0, and Cargo does not pick one of those up on its own.

---

## Thirty seconds

```sh
sloop setup                          # once per machine
sloop db add orders --url postgres://app@db.internal:5432/orders
sloop key export > backup-key.txt    # once. Keep it somewhere else
sloop backup orders                  # dumps it, counts every row, writes a manifest
sloop backups list
sloop restore orders                 # puts the newest whole backup back
```

The password is asked for, not typed on the command line. `--url` is one way to give the
rest; `--engine --host --port --user --database` is the other.

**The key export is not optional and not a formality.** Backups are encrypted, the private
half of the key is on this machine and nowhere else, and losing the machine without a copy
means losing every backup it took. The first backup stops and says so; without a terminal to
say it to, it refuses and exits `2`.

Or run `sloop` with no arguments and answer the questions. The menu covers registering,
backing up, restoring, mirroring, syncing, reading and the backup key; installing and
scheduling the background service is by flag for now. Every interactive run ends by printing
the flag form of what it just did, so a session becomes a line you can schedule.

---

## Commands

Run `sloop <command> --help` for the flags of any one of them.

### Databases

| Command | What it does |
|---|---|
| `sloop db add <name>` | register a database, by URL or field by field |
| `sloop db create <name>` | create a database and the role that owns it, then register it |
| `sloop db list` | every registered database. Never prints a secret |
| `sloop db test [name]` | open a connection and report what came back |
| `sloop db edit <name>` | change host, port, user, database or password |
| `sloop db rename <from> <to>` | change the name sloop files it under, not the server's |
| `sloop db remove <name>` | forget it. The database itself is untouched |
| `sloop db drop <name>` | destroy the database on the server. Keeps nothing |

`db add` and `db edit`:

| Flag | What it does |
|---|---|
| `--url <URL>` | the whole connection in one string |
| `--engine <ENGINE>` | `postgres`, `mysql` or `mariadb` |
| `--host`, `--port`, `--database`, `--user` | field by field instead |
| `--keyring` | keep the password in the OS keyring. The default |
| `--encrypted-file` | keep it in the Argon2id-encrypted file, for a machine with no keyring |
| `--env <VARIABLE>` | read it from that variable at run time |
| `--password-from <COMMAND>` | run that and read the password from its output |
| `--password-stdin` | take it from standard input |
| `--test` | connect before saving, and refuse to save if it fails |

`db create` also takes `--superuser`, `--superuser-password-stdin`, `--role` and
`--role-password-stdin`.

### Backups

| Command | What it does |
|---|---|
| `sloop backup [name]` | dump it, verify every row arrived, write a manifest |
| `sloop backup --all` | every database, carrying on past a failure |
| `sloop backups list [name]` | what was taken, when, how big, whether it still checks out |
| `sloop backups prune [name]` | delete what is past a retention limit |
| `sloop restore <name>` | put a backup back, replacing what is there now |

| Flag | What it does |
|---|---|
| `backup --sequential` | one directory per run, named for the moment it was taken. The default |
| `backup --replace` | one backup per database, at `backups/<engine>/<name>/latest` |
| `backups list --check` | hash every dump and compare it with its manifest |
| `backups prune --keep <N>` | keep this many of the newest |
| `backups prune --older-than <AGE>` | `12h`, `30d`, `6w` |
| `backups prune --include-broken` | unfinished and damaged directories too |
| `restore --from <WHEN>` | a particular backup, by its directory's own name |

### Copying

| Command | What it does |
|---|---|
| `sloop mirror <source> --to <dest>` | an exact copy. The destination ends up identical |
| `sloop sync <source> --to <dest>` | a merge. Rows added and replaced, destination-only rows kept |

The source is only ever read. No statement sloop sends to it writes anything.

| Flag | What it does |
|---|---|
| `--to <NAME>` | a registered database to copy into |
| `--create <NAME>` | make the destination first, then copy into it |
| `--table <PATTERN>` | only this table. Repeatable, schema-qualified, takes a wildcard |
| `--with-references` | bring in the tables the named ones point at |
| `--safe` | dump to a file first, so a copy that fails can be retried |

`sync` needs a primary key on every table, resets sequences and identity columns afterwards,
names the source-deleted rows that will linger, and refuses on circular foreign keys.

### Reading

| Command | What it does |
|---|---|
| `sloop query [name]` | pick a table, tick the columns, choose the test. No SQL |
| `sloop query <name> --sql <STATEMENT>` | run one statement instead |

Everything this command runs, `--sql` included, runs inside a transaction the server has been
told is read-only.

### This machine

| Command | What it does |
|---|---|
| `sloop setup` | prepare the PostgreSQL 18 sloop keeps its own state in |
| `sloop doctor` | which client tools are present, and what each registered role may do |
| `sloop doctor --offline` | the same without connecting to anything |
| `sloop server install [engine]` | download, install, configure and start a database server |
| `sloop server list` | every server sloop installed here, and how to reach each |
| `sloop server connection` | where sloop keeps its own state, for `psql` or anything else |
| `sloop reset` | back to the moment sloop was installed |
| `sloop uninstall` | remove sloop from this machine |

### Backup key

Backups are encrypted with an [age](https://age-encryption.org) keypair, and the asymmetry
is the point: the public key sits in the registry, so a scheduled backup needs no secret to
run. The private key goes to the OS keyring, or to the encrypted file on a machine without
one, and only a restore reads it.

| Command | What it does |
|---|---|
| `sloop key export` | write the private key out. Losing it loses every backup |
| `sloop key import` | take a private key exported from another machine |

### The background service

| Command | What it does |
|---|---|
| `sloop service install` | register sloop with this machine, so it starts at boot |
| `sloop service uninstall` | stop it and take it off, keeping what it recorded |
| `sloop service attach <name>` | watch a database. Picked up without a restart |
| `sloop service detach <name>` | stop watching one |
| `sloop service schedule <name>` | back it up on a schedule, with no cron line anywhere |
| `sloop service start` \| `stop` | now, rather than at the next boot |
| `sloop service status` | installed, running, when it last ran and when it runs next |
| `sloop service activity` | rows in, rows out and size, by day, week and month |

```sh
sloop service install
sloop service attach orders
sloop service schedule orders --every 1d --keep 7 --keep-for-days 30
```

Nothing is written to crontab, Task Scheduler or a systemd timer. The service takes the
backup, applies the retention policy and prunes what falls outside it.

| Flag | What it does |
|---|---|
| `service install --no-start` | register it and leave it stopped |
| `service install --user <USER>` | run as that account. Linux and macOS |
| `service install --interval <SECONDS>` | between one round of readings and the next. 60 by default |
| `service schedule --every <INTERVAL>` | `30m`, `6h`, `1d`, `2w` |
| `service schedule --keep <COUNT>` | keep this many of the newest backups |
| `service schedule --keep-for-days <DAYS>` | prune anything older |
| `service schedule --off` | stop backing it up. It stays attached and sampled |

---

## Global flags

Every command takes these.

| Flag | What it does |
|---|---|
| `--global` | use the global registry, whichever directory you are in |
| `-C, --project <PATH\|NAME>` | work on that project |
| `--password-command <COMMAND>` | take the password from that command's output |
| `-y, --yes` | answer a question this would stop and ask |
| `--force` | override a refusal that protects something |
| `--confirm <NAME>` | the name of the thing being destroyed, typed out |
| `-q, --quiet` | say nothing but what went wrong |
| `--json` | print one JSON document instead of talking |
| `--no-color` | never colour the output |
| `--log-file <PATH>` | append everything to a file as well, credentials removed |
| `--dry-run` | say what would happen and change nothing |

`--yes`, `--force` and `--confirm` are three different things and none stands in for another.
`--yes` answers questions. `--force` overrides a refusal and answers none. `--confirm` is the
only way to run a destructive command without a terminal, and it has to match the name
exactly — so a scheduled line names what it destroys and cannot be repointed by editing one
flag.

With no terminal to ask at, a command that needs an answer exits `2` and names the flag that
would have given it. It never hangs on a question nobody is there to read.

---

## Passwords

Four routes, and none of them is a plaintext password in a file. A registry holds a *route*,
never a value, and there is no spelling of a password that a registry will accept.

| Route | Where the password lives |
|---|---|
| `keyring` | the OS keyring: Credential Manager, Keychain, Secret Service. The default |
| `encrypted-file` | XChaCha20-Poly1305 with Argon2id, for a headless Linux box |
| `${VARIABLE}` | an environment variable, read at run time, for automation |
| `command:<command line>` | run something and read its output, for a team password manager |

```sh
sloop db add orders --url postgres://app@db:5432/orders \
  --password-from "op read op://vault/db/password"

sloop db add orders --url postgres://app@db:5432/orders --env PGPASSWORD
```

Nothing is ever passed in `argv`, so nothing shows in `ps`.

---

## Registries

A registry is the set of databases sloop knows. There is one global registry and any number
of project ones.

```sh
sloop init            # start a project registry in this directory
```

Which one a command uses, in order:

1. `--global` — the global registry, full stop.
2. `-C <path|name>` — that project.
3. `SLOOP_PROJECT` — the same thing from the environment.
4. The nearest `.sloop` at or above the working directory, the way git finds `.git`.
5. Otherwise, the global registry.

A name registered in both belongs to the project. `--global` forces the other one, and
`global:orders` is the explicit qualifier for a single command.

The global registry lives at `~/.sloop` — the same path on all three platforms. A store left
in one of the older locations (`%APPDATA%\sloop`, `~/.config/sloop`,
`~/Library/Application Support/sloop`) is moved there once, rather than ignored.

The registry itself is a database in the PostgreSQL `sloop setup` prepares, not a file you
edit. `sloop server connection` prints how to open it.

---

## Backups on disk

Inside the registry they belong to — `~/.sloop/backups/` for the global one, `.sloop/backups/`
for a project's.

```
backups/<engine>/<label>/<utc-timestamp>/
    dump.age          the dump, encrypted
    manifest.json     engine version, size, checksum, duration, per-table row counts,
                      both timestamps and the offset
```

Directory names are UTC — `20260916T031500Z` — so they sort correctly and survive a daylight
saving change. Every display shows local time.

**Verification counts rows.** Both sides, every table, with one statement. A planner estimate
is not proof, and has been seen reporting double the real figure; `SLOOP_VERIFY=fast` asks for
the estimate instead and every number it produces is labelled as one. A dump taken from a live
database drifts while it runs, and sloop says so in those words. A table that did not arrive
at all is a failure.

---

## Exit codes

Frozen at 1.0. Automation depends on them: adding one is a feature, changing one is a
breaking release.

| Code | Meaning |
|---|---|
| `0` | success |
| `1` | something else went wrong |
| `2` | bad usage, an unknown name, or a question with no terminal to ask it at |
| `3` | the connection failed |
| `4` | the dump failed |
| `5` | the restore failed |
| `6` | it finished, and the row counts disagreed |
| `7` | another run holds the lock on that database |
| `8` | `doctor` found something that will break a backup |

`6` is the one worth wiring up: finished but not verified is neither success nor a crash.

---

## Environment

| Variable | What it does |
|---|---|
| `SLOOP_PROJECT` | which project registry to use. `-C` outranks it |
| `SLOOP_VERIFY` | `fast` for planner estimates instead of exact counts |
| `SLOOP_PASSPHRASE` | opens the encrypted password file without a prompt |
| `SLOOP_PASSPHRASE_FILE` | a file holding it, for a run with nobody to ask |
| `NO_COLOR` | the same as `--no-color` |

---

## Client tools

`pg_dump`, `pg_restore`, `psql`, `mysqldump`, `mysql`, `mariadb-dump`. They are not bundled —
`mysqldump` is GPL v2, and bundling would put about 150 MB in every release. sloop acquires
them on demand, once, after you say yes:

| Platform | How they arrive |
|---|---|
| Windows | the official PostgreSQL binary zip, checksum verified, only the needed files extracted |
| Linux | offers to run `apt` or `dnf`, from the PGDG repository |
| macOS | offers to run `brew install libpq` |

Never without a terminal — there it exits and prints what to install.

The one job that needs the network shells out to the system's own `curl` or
`Invoke-WebRequest`. The binary holds no network code, which is what keeps the claim at the
top of this file checkable by anyone.

---

## Roadmap

Planned. No dates.

- Writing and saving queries, beyond the read-only builder `sloop query` has today.
- DDL: creating and altering tables from sloop.
- Editing rows in place.
- A local interface that reads sloop's own PostgreSQL the way the CLI does.

---

## Building

```sh
cd cli
cargo build
cargo test
```

Rust 1.89 or newer. See [CONTRIBUTING.md](CONTRIBUTING.md) for the full loop, including the
throwaway database clusters the tests build and tear down.

```
cli/        the Rust CLI. Crate dbsloop, binary sloop
web/        the documentation site
ci/         the checks the workflows run
install/    install.sh, install.ps1 and their uninstallers
```

---

## Security

Report a vulnerability privately — see [SECURITY.md](SECURITY.md). Please do not open a
public issue for one.

## License

MIT OR Apache-2.0, at your option.
See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).

Unless you state otherwise, any contribution you intentionally submit for inclusion in this
work shall be dual licensed as above, with no additional terms or conditions.
