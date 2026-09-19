# Contributing to sloop

Thanks for looking. Issues, discussion and pull requests are all welcome.

## Before a big change

Open an issue first. A design that has been talked through is quicker to review than a
branch that has to be unpicked, and it saves you writing something that was already decided
another way.

Small fixes — a typo, a wrong message, a missing `--help` line — need no issue. Send them.

## Building

```sh
git clone https://github.com/DBSloop/sloop
cd sloop/cli
cargo build
```

Rust 1.89 or newer; the exact minimum is `rust-version` in `cli/Cargo.toml` and CI holds the
crate to it.

```
cli/         the Rust CLI. Crate dbsloop, binary sloop
web/         the documentation site, Angular
ci/          the scripts the workflows run
install/     install.sh, install.ps1 and their uninstallers
```

## The loop, before you open a pull request

```sh
cd cli
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

All three have to pass. Warnings are denied in CI, so a warning is a failure.

The shell scripts are linted too, and CI's shellcheck is older than the one your package
manager will give you — a newer one passes things 0.9 rejects. Match it:

```sh
pip install shellcheck-py==0.9.0.6
shellcheck --shell=sh --severity=style install/*.sh ci/*.sh
```

### And the one that is not about the code

```sh
cargo tree | grep -Ei 'reqwest|hyper|ureq|isahc|curl'
```

This must print nothing. sloop's headline claim is that no credential can leave the machine
because there is no HTTP client in the binary, and `ci/no-http-client.sh` fails the build the
day one appears. If your change needs the network, it shells out to the system's own `curl`
or `Invoke-WebRequest` after the user has agreed — see `cli/src/tools/acquire.rs`.

## Tests

Most of the suite is unit tests beside the code. The interesting part is not.

**Tests build real database servers.** `cargo test` starts throwaway PostgreSQL, MySQL and
MariaDB clusters in temporary directories, on ports nothing else uses, and tears them down
afterwards. Nothing is installed on your machine and no database of yours is touched.

You need the server programs on `PATH`, or pointed at:

| Variable | What it points at |
|---|---|
| `SLOOP_TEST_PG_BIN` | the directory holding `initdb`, `pg_ctl`, `postgres`, `psql` |
| `SLOOP_TEST_MYSQL_BIN` | the directory holding the MySQL client programs |
| `SLOOP_TEST_MARIADB_BIN` | the same for MariaDB |

A machine with no server skips those tests and says so. **A skip proves nothing** — if you
changed an engine adapter, run its tests for real or say in the pull request that you could
not. CI runs all three on every push, plus PostgreSQL 15, 16, 17 and 18.

Integration tests in `cli/tests/` drive the compiled binary inside a sandbox that has its own
`HOME`, `APPDATA` and working directory. A test that can reach the machine it runs on will
change it eventually, so none of them can.

## What a pull request needs

- One thing. A branch that fixes a bug and renames a module is two reviews.
- The three commands above, green.
- A test that fails before your change and passes after it.
- A commit message whose first line says what changed, in the present tense, and stays
  short. The body explains why, not what — the diff already says what.
- Documentation, when the change is visible: `--help` text lives in `cli/src/cli.rs`, the
  README's command tables live here, and the site lives in `web/`.

## Things the code holds to

These are checked, so it is quicker to know them than to be told by a failing build.

**Every command says what it does, and every error says what to do about it.** A `Failure`
sloop phrases itself carries a `.hint(...)` naming the flag, the command or the step that
fixes it. The exception is a message ending in `: {error}` — that is the operating system or
a client tool talking, and there is nothing to add. `failure::plain_language` reads the
sources and fails the build otherwise.

**No password in `argv`, ever.** `ps` shows it. Passwords go through a pipe or a
single-process environment variable. No plaintext password is written anywhere — a registry
holds a *route*, and there is no spelling of a password it will accept.

**Never prompt when stdin is not a terminal.** A scheduled run that hangs on a question is
the worst failure this tool can have. Use `std::io::IsTerminal`, and exit with an error
naming the flag that would have answered it.

**The source of a copy is only ever read.** No statement `mirror` or `sync` sends to the
source writes anything, not even `ANALYZE`.

**Destructive means typing the name**, not pressing `y`.

**Exit codes are frozen.** They are listed in the README and in `cli/src/exit.rs`. Adding one
is a feature; changing what one means is a breaking release.

**No `unwrap()` on anything a user can influence.** A refusal with a sentence, not a panic.

## Reporting a bug

Use the issue template — it asks for the version, the OS and the engine, which are the three
things a report is unusable without. `sloop --version` and `sloop doctor` between them have
everything.

`sloop --log-file <path>` writes everything a run printed, without colour and with anything
that looks like a credential removed, so the file is safe to attach.

## Security

Do not open a public issue for a vulnerability. [SECURITY.md](SECURITY.md) has how to report
one privately.

## License

By contributing you agree that your work is dual licensed under MIT and Apache-2.0, matching
the project, with no additional terms.
