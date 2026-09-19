# Security policy

## Reporting a vulnerability

**Please do not open a public issue.**

Report it through GitHub's private advisory form:

**https://github.com/DBSloop/sloop/security/advisories/new**

That opens a thread only the maintainers can see, and it is where a fix is coordinated and a
CVE requested if one is warranted.

Useful in a report:

- What an attacker can do, and what they need first — local access, a registry file, a
  network position.
- The version (`sloop --version`), the operating system, and the database engine.
- Steps to reproduce, ideally against a throwaway database.

You will get an acknowledgement within a week. If a week passes with no reply, the thread
may have been missed — open a public issue saying only that you are waiting on a security
response, with no details in it.

Please give us time to ship a fix before writing about the problem publicly. Credit goes in
the advisory and the release notes unless you would rather it did not.

## Supported versions

sloop is pre-1.0. Fixes go onto the latest release, and there are no backports to earlier
ones. After 1.0 this section will say which lines are maintained.

## In scope

- **A credential leaving the machine.** This is the claim the project is built on. Anything
  that sends, logs, or writes a password, a passphrase or a private key where it can be read
  is the most serious class of bug sloop has.
- **A plaintext password or key on disk**, including in a registry, a manifest, a dump
  filename, a log file or a crash message.
- **A password visible in `ps`** or in any other process listing.
- **A backup that decrypts without its key**, or encryption that is weaker than documented.
- **Writes to the source of a copy.** `mirror` and `sync` only ever read their source; a
  statement that writes to one is a correctness bug and a data-loss bug at once.
- **A destructive operation running without the confirmation it documents** — a drop, a
  restore or a mirror that proceeds without the name being typed or `--confirm` given.
- **Command or SQL injection** through a database name, a host, a label, a password or
  anything else sloop is handed.
- **Acquiring client tools without verification** — a downloaded archive accepted without
  its checksum matching, or fetched over plain HTTP.
- **Privilege escalation through the background service**, its unit file, its plist, its
  service definition, or the key file it reads its credentials from.
- **An installer that can be made to write outside its install directory**, or that runs
  something it did not verify.

## Not in scope

- Securing the machine or the network sloop runs on. That is the operator's job, and sloop
  documents rather than enforces it.
- A database server's own configuration, or a role granted more than it needs.
- The contents of a backup taken from a database that was already compromised.
- Anything requiring an attacker who already has your user account. If they can read your
  keyring, they can read everything that keyring protects.
- Missing hardening that no documented behaviour depends on, with no demonstrated impact.
- Unsigned release binaries. Code signing certificates have not been bought; the `curl | sh`
  and `irm | iex` install paths carry no quarantine attribute and are unaffected, and the
  workaround for a manual download is documented on the Releases page.

## What sloop does

Stated so that a report can point at the gap between this and what it found.

- **No HTTP client is in the dependency graph.** `ci/no-http-client.sh` fails the build if
  one appears. The only job needing the network shells out to the system's own downloader,
  after the user agrees.
- **Passwords are never stored in plaintext.** Four routes: the OS keyring, an
  XChaCha20-Poly1305 file with an Argon2id-derived key, an environment variable read at run
  time, or a command whose output is read through a pipe. A registry holds the route, never
  the value.
- **No password is passed in `argv`.** It reaches a child through a pipe or through that one
  child's environment.
- **Backups are encrypted with an age keypair.** The public key sits in the registry so a
  scheduled backup needs no secret; the private key lives in the keyring and only a restore
  reads it.
- **Logs are written to be pasteable.** `--log-file` strips colour and redacts anything that
  reads like a credential.
- **Configuration is parsed, never evaluated.** A password containing `\`, `$`, `>`, `#`, a
  quote or a space is a password, not a shell fragment.
