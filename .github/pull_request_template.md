<!--
Thanks for the pull request.

A security fix does not start here — report it privately first, through
https://github.com/DBSloop/sloop/security/advisories/new
-->

## What this changes

<!-- One or two sentences. The diff says what; say why. -->

Closes #

## How it was tested

<!--
Name the engines you actually ran against. A test that skipped because the server was not
there proved nothing, and saying so is better than leaving it to be assumed.
-->

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] `cargo test`
- [ ] A test fails without this change and passes with it
- [ ] Ran against a real PostgreSQL / MySQL / MariaDB, or said below which I could not

## The constraints

<!-- Delete any line that has nothing to do with this change. -->

- [ ] `cargo tree` still shows no HTTP client
- [ ] No password reaches `argv`, a log, a filename or a config file
- [ ] Nothing prompts when stdin is not a terminal
- [ ] `mirror` and `sync` still only read their source
- [ ] No exit code changed meaning
- [ ] Every new failure names the fix, and `--help` covers every new command and flag

## Anything else

<!-- A decision you were unsure about, a follow-up you left, a thing to look at closely. -->
