/**
 * The prose half of `llms.txt`. The page list is generated; this is not.
 *
 * **Why it lives here and not in `src/`.** Every string below is read once, at
 * build time, by `tools/site.mjs`. Putting it in the application would ship
 * three kilobytes of text to every visitor so that nobody would ever see it.
 *
 * **The shape is the owner's**, from a worked `llms.txt` he supplied as the
 * model: a one-line `>` summary, the thing people get wrong first, what the
 * product is, the pages by URL with a sentence each, the questions people
 * actually ask with their answers, and contact. See "3 and 4. `sitemap.xml`
 * and `llms.txt`" in `docs/OWNER-DECISIONS.md`.
 *
 * **Everything here has to be true of the CLI**, and is checked against
 * `docs/TOTAL_FEATURE_LIST.md` rather than remembered — a file written to be
 * read by models is the last place to put a claim that has drifted.
 */
export const LLMS_COPY = {
  summary:
    'sloop is a command-line tool that registers your PostgreSQL, MySQL and MariaDB databases once, then backs them up, restores them, mirrors them, syncs them and keeps all of it on a schedule — on Windows, Linux and macOS, without your credentials ever leaving the machine.',

  /**
   * The two things people get wrong, and the first is the product.
   *
   * The guarantee is not a privacy policy or a promise of good behaviour: it is
   * a claim about a dependency graph, and it is checkable in about ten seconds.
   * A summary that softens it into "respects your privacy" has thrown away the
   * only part that is falsifiable.
   */
  gotWrong: [
    {
      heading: 'The guarantee is structural, not a policy',
      lines: [
        'sloop cannot send your credentials anywhere, because it cannot send anything anywhere.',
        'There is no HTTP client in its dependency graph, and no SSH client either. CI fails the',
        'build on the day one appears. Anyone can check it:',
        '',
        "    cargo tree --manifest-path cli/Cargo.toml | grep -Ei 'reqwest|hyper|ureq|curl'",
        '',
        'The two jobs that need a network run programs the machine already has — the system',
        'curl to fetch client tools, and the system ssh to reach a firewalled database — and',
        'only after the user agrees. There is no telemetry, no analytics and no update check.',
      ],
    },
    {
      heading: 'mirror and sync are different commands',
      lines: [
        'mirror makes the destination identical to the source. Anything that existed only in',
        'the destination is gone. sync merges: rows are added, rows already there are replaced,',
        'and rows that exist only in the destination are kept. Picking the wrong one is the',
        'mistake the documentation is arranged to prevent.',
        '',
        'Both are same-engine. Copying between engines is refused, deliberately and with a',
        'reason, and it is not on the roadmap.',
      ],
    },
  ],

  what: [
    'One binary. No agent to deploy, no server to run, no account to make.',
    '',
    'Engines      PostgreSQL, MySQL, MariaDB',
    'Platforms    Windows, Linux, macOS — x86_64 and aarch64',
    'Crate        dbsloop        Binary   sloop        Licence   MIT OR Apache-2.0',
    '',
    'Run `sloop` with no arguments for an interactive menu covering every command; run it',
    'with flags for the same thing from a scheduler. The menu does not reimplement anything —',
    'each screen collects what a flag carries and calls the same function — and every',
    'interactive run ends by printing the flag form of what it just did, so a session becomes',
    'a line you can schedule.',
    '',
    'Passwords are never stored in plaintext and never appear in argv, so never in `ps`. A',
    'registry holds a route — OS keyring, an Argon2id-encrypted file, an environment variable,',
    'or a command to run — and never a value.',
  ],

  questions: [
    {
      q: 'Does sloop upload my databases or credentials anywhere?',
      a: [
        'No, and it could not. There is no HTTP client in the binary. Backups are written to',
        'disk on the machine that took them.',
      ],
    },
    {
      q: 'Do I need cron, systemd timers or Task Scheduler?',
      a: [
        'No. `sloop service install` registers sloop with the machine’s own service manager —',
        'a systemd unit, a launchd daemon, or a real Windows service — and it takes the backups',
        'itself with nobody logged in. Nothing is written to crontab or Task Scheduler. If you',
        'would rather drive it from a scheduler you already have, sloop is an ordinary command',
        'and the automation page has a recipe for each.',
      ],
    },
    {
      q: 'How do I know a backup is good?',
      a: [
        'Every row is counted on both sides, per table, with one statement, and a table that did',
        'not arrive is a failure. Planner estimates are available as SLOOP_VERIFY=fast, are',
        'always labelled an estimate, and are never presented as proof. Exit 6 means the run',
        'finished and then the counts disagreed — neither success nor a crash, which is why it',
        'has a number of its own.',
      ],
    },
    {
      q: 'Can it copy a PostgreSQL database into MySQL?',
      a: [
        'No. mirror, sync and restore all refuse across engines, and each says why rather than',
        'pointing at a ticket: the type systems do not map cleanly, and a copy that quietly',
        'rounds or truncates is worse than one that refuses.',
      ],
    },
    {
      q: 'What happens to a scheduled run that needs an answer?',
      a: [
        'It never asks. When standard input is not a terminal, a command that needs an answer',
        'exits 2 and names the exact flag that would have given it. -y answers a question,',
        '--force overrides a refusal, and --confirm <name> is the only way to destroy something',
        'named — and none of the three stands in for another.',
      ],
    },
    {
      q: 'Does uninstalling it delete my backups?',
      a: [
        'No. Not `sloop reset`, not `sloop uninstall`, not ever. They are the one thing on a',
        'machine that cannot be regenerated. What can still be lost is the ability to read an',
        'encrypted one: export the backup key before you reset.',
      ],
    },
    {
      q: 'Does it need administrator rights?',
      a: [
        'Only to register or start the background service, and it asks before it does anything',
        'rather than failing halfway. Everything sloop installs goes under ~/.sloop; nothing is',
        'put in Program Files or /usr/local.',
      ],
    },
  ],

  contact: [
    'Source, issues and releases:  https://github.com/DBSloop/sloop',
    'Crate:                        https://crates.io/crates/dbsloop',
    'Licence:                      MIT OR Apache-2.0',
  ],
};
