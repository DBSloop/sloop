/**
 * The docs tree — eight groups, seventeen pages, in one place.
 *
 * The sidebar, the route table, the overview's directory, the prev/next footer
 * and the filter all read this file, so adding a page is one edit and the five
 * of them cannot part company. `docs/ANGULAR-TASK.md` says which pages there
 * are; this is that list, turned into something the site can render.
 *
 * **The groups are the CLI's own headings.** `sloop` with no arguments opens a
 * menu whose home screen reads DATABASES, BACKUPS, COPYING, READING, BACKUP
 * KEY, THIS MACHINE, THE BACKGROUND SERVICE — so somebody who learned the tool
 * from the menu finds the docs already arranged the way they think. Start here
 * and Reference are the two the menu has no equivalent of, and they are first
 * and last for the same reason they would be in a book.
 *
 * `blurb` and `covers` are written from `docs/TOTAL_FEATURE_LIST.md`, which is
 * the whole of what the CLI does. They describe a page's subject and are never
 * a substitute for the page: the entry that writes it checks every flag against
 * `sloop <command> --help`, which is the only authority there is.
 */
export interface DocsPageEntry {
  /** The child path under `/docs`. */
  readonly path: string;
  /** The `<h1>`. */
  readonly title: string;
  /**
   * The `<title>`, which is a different job from the `<h1>` and is written as
   * one.
   *
   * A heading sits inside a page that has already said what it is about; a
   * title is a lone line in a list of ten results and has to answer the
   * question that was typed. So *Installation* is the heading and
   * *Install sloop — database backup CLI for Windows, Linux and macOS* is the
   * title, and neither is a worse version of the other.
   *
   * Kept near sixty characters, which is about where a result stops being
   * shown in full, and each one written as something a person would actually
   * search for rather than as a list of words they might.
   */
  readonly documentTitle: string;
  /** The sidebar label, where the full title is too long for a 16rem rail. */
  readonly short?: string;
  /** One sentence, under the heading and beside the link on the overview. */
  readonly blurb: string;
  /** What the page holds. Shown as an outline until the page is written. */
  readonly covers: readonly string[];
  /** Set by the entry that writes the page. Until then the site says so. */
  readonly written?: boolean;
}

export interface DocsGroup {
  readonly title: string;
  /** One line under the group heading on the overview page. */
  readonly line: string;
  readonly pages: readonly DocsPageEntry[];
}

export const DOCS_NAV: readonly DocsGroup[] = [
  {
    title: 'Start here',
    line: 'Get sloop onto the machine, and a verified backup out of it.',
    pages: [
      {
        path: 'install',
        documentTitle: 'Install sloop — database backup CLI for Windows, Linux, macOS',
        title: 'Installation',
        blurb:
          'The one-liner for your platform, what it does to PATH, and both ways to take sloop off again.',
        written: true,
        covers: [
          'The install one-liner for each platform, one block per platform and never two at once.',
          'Where the binary lands, how PATH is changed — and that the open shell has to be restarted.',
          'The installer options, and the environment variables that do the same thing.',
          'Taking a release archive by hand and checking it against SHA256SUMS.',
          'cargo install dbsloop, for a machine that already has Rust.',
          'The SmartScreen and Gatekeeper warning on a manual download, and why the one-liner is unaffected.',
          'sloop uninstall and uninstall.sh, and that neither ever deletes a backup.',
        ],
      },
      {
        path: 'getting-started',
        documentTitle: 'Getting started — your first verified PostgreSQL backup',
        title: 'Getting started',
        written: true,
        blurb: 'From an empty machine to a verified backup you can restore, in six commands.',
        covers: [
          'sloop setup: finding or installing the PostgreSQL that sloop keeps its own state in.',
          'Registering the first database, by URL or field by field.',
          'sloop key export — a required step, not an aside. The first encrypted backup refuses to run until it has happened.',
          'The first backup, and what it counts on both sides before it calls itself done.',
          'sloop backups list, and reading a manifest.',
          'Restoring it, and what the confirmation asks you to type.',
        ],
      },
    ],
  },
  {
    title: 'Databases',
    line: 'Registering them, and knowing which registry a bare name resolves to.',
    pages: [
      {
        path: 'databases',
        documentTitle: 'Connect PostgreSQL, MySQL and MariaDB databases to sloop',
        title: 'Databases, registries and projects',
        short: 'Databases and registries',
        written: true,
        blurb:
          'Register a database once, then name it. Where that name is looked up, and what happens when two registries both hold it.',
        covers: [
          'db add, by URL or field by field, and the four routes a password can come from.',
          'db create, which makes the database and the role that owns it, then registers it.',
          'db test, db list and db edit — and that list never prints a secret.',
          'db rename, which changes the name sloop files it under and not the name on the server.',
          'db remove against db drop: forgetting one, and destroying one.',
          'The global registry and project registries, and sloop init.',
          'The five-step resolution order, why a name in both belongs to the project, and what global:orders forces.',
          'Where the store lives on each platform, and that the registry is a database rather than a file.',
        ],
      },
      {
        path: 'ssh',
        documentTitle: 'Back up a database through an SSH tunnel — sloop',
        title: 'Reaching a database over SSH',
        short: 'Over SSH',
        written: true,
        blurb:
          'A database whose port is closed, reached by running the machine’s own ssh — never a client built into the binary.',
        covers: [
          '--ssh-host, --ssh-port, --ssh-user, --ssh-identity, and --no-ssh.',
          'The four routes a key passphrase can come from, which are the four a password comes from.',
          'One forward per menu session rather than one per command, and that every forward closes with the session.',
          'Why sloop shells out: an embedded SSH client is a socket in the binary, and the guarantee is that there is not one.',
        ],
      },
    ],
  },
  {
    title: 'Backups',
    line: 'Taking them, proving them, keeping them, and putting one back.',
    pages: [
      {
        path: 'backups',
        documentTitle: 'Database backup, retention and restore — sloop',
        title: 'Backups, retention and restoring',
        short: 'Backups and restoring',
        written: true,
        blurb:
          'Take one, prove every row arrived, delete what is past its retention, and put it back when you need it.',
        covers: [
          'sloop backup, and --all, which never stops on a failure and reports every database that did.',
          'Sequential against --replace: a directory per run, or one backup kept current.',
          'What is written to disk, and everything the manifest records.',
          'backups list, and --check, which hashes every dump and compares it with its manifest.',
          'Pruning by count with --keep and by age with --older-than, and what --include-broken adds.',
          'restore, --from, and what it replaces.',
          'One lockfile per database, and what exit 7 means for the run that finds it held.',
          'Verification: exact counts on both sides, why a planner estimate is not proof, and what SLOOP_VERIFY=fast changes.',
        ],
      },
      {
        path: 'backup-key',
        documentTitle: 'Encrypted database backups and the age key — sloop',
        title: 'The backup key',
        written: true,
        blurb:
          'An age keypair, and the asymmetry is the point: a scheduled backup needs no secret, and only a restore does.',
        covers: [
          'The public key in the registry, so an unattended backup has nothing to unlock.',
          'The private key in the OS keyring, or in the encrypted file on a machine without one.',
          'sloop key export and sloop key import, and moving to another machine.',
          'The refusal that stops the first encrypted backup until a copy exists, and what it does with no terminal to ask at.',
          'What you lose by losing the machine, and what you lose by losing the key.',
        ],
      },
    ],
  },
  {
    title: 'Copying',
    line: 'Two commands that are not the same command.',
    pages: [
      {
        path: 'mirror-and-sync',
        documentTitle: 'Mirror vs sync: copy or merge a database — sloop',
        title: 'Mirror and sync',
        written: true,
        blurb:
          'One makes the destination identical to the source. The other merges into what is already there. Picking the wrong one is the mistake this page exists to stop.',
        covers: [
          'The difference, first and in a box: mirror replaces, sync merges.',
          '--to for a registered destination, --create to make one first.',
          'Scoping a copy with --table and --with-references, and what --safe buys you.',
          'That the source of a copy is only ever read — no statement, not even ANALYZE, writes to it.',
          'The rules sync states as rules: a primary key per table or the table is skipped and named; sequences and identity columns reset; source-deleted rows that linger; an outright refusal on circular foreign keys.',
          'That both are same-engine, and that PostgreSQL to MySQL is refused rather than attempted.',
        ],
      },
    ],
  },
  {
    title: 'Reading',
    line: 'Looking inside a database without writing any SQL.',
    pages: [
      {
        path: 'query',
        documentTitle: 'Read a database table without writing SQL — sloop query',
        title: 'Reading a database',
        written: true,
        blurb:
          'Pick a table, tick the columns, choose the test. Nothing it runs can write, and the server is what guarantees that.',
        covers: [
          'The builder: a table, its columns and a test, as a screen of questions.',
          'The results grid, two hundred rows at a time.',
          '--sql, for one statement instead of the builder.',
          'That everything it runs — --sql included — runs inside a transaction the server has been told is read-only, which is a guarantee from the engine rather than a list of forbidden words.',
          'What it does with no terminal to build a query at.',
        ],
      },
    ],
  },
  {
    title: 'The background service',
    line: 'The feature that replaces cron, on all three platforms.',
    pages: [
      {
        path: 'service',
        documentTitle: 'Scheduled database backups without cron — sloop service',
        title: 'The background service',
        short: 'The service',
        written: true,
        blurb:
          'sloop registers itself with your machine and backs your databases up on a schedule, with nobody logged in.',
        covers: [
          'What gets registered: a systemd unit, a launchd daemon, or a real Windows service.',
          'service install and its flags, and service uninstall, which deletes nothing it recorded.',
          'Attaching and detaching a database, picked up without a restart.',
          'service schedule, with --every, --keep, --keep-for-days and --off.',
          'service status, service start and service stop.',
          'That nothing is written to crontab, Task Scheduler or a systemd timer.',
          'A missed run caught up once and labelled late, and a collision with a manual run that exits 7 and stands aside.',
          'Which four commands need an administrator or sudo, and that sloop asks before it does anything.',
        ],
      },
      {
        path: 'activity',
        documentTitle: 'Database activity by day, week and month — sloop',
        title: 'Activity and monitoring',
        short: 'Activity',
        written: true,
        blurb:
          'Rows in, rows out and size, per database per day — and the one number no engine will give you.',
        covers: [
          'What the service records, per database per UTC day.',
          'How a day, a week, a month and a year are all a sum over a range.',
          'sloop service activity, and why every window carries its count of readings.',
          'Moved: real bytes sloop itself read or wrote during a dump or a restore.',
          'That bytes on the wire are not available per database on any of the three engines, said in those words rather than drawn as a zero.',
          'A period with no readings shown as having none, never as zero.',
        ],
      },
    ],
  },
  {
    title: 'This machine',
    line: 'What sloop puts on it, and how to take all of it off.',
    pages: [
      {
        path: 'postgres',
        documentTitle: 'sloop setup and its own PostgreSQL — server and connection',
        title: 'sloop’s own PostgreSQL',
        short: 'sloop’s PostgreSQL',
        written: true,
        blurb:
          'The registry, the schedules and the service’s readings live in a PostgreSQL 18 of sloop’s own. Here is how to open it yourself.',
        covers: [
          'Why sloop keeps its state in a database rather than a file, and what is in it.',
          'sloop setup, and its flags for a run with nobody to ask.',
          'server install and server list, and installing a database server without administrator rights.',
          'server connection, for psql, DataGrip or anything else — and --show-password, which only works on a terminal.',
          'That everything sloop installs goes under ~/.sloop, and nothing goes in Program Files or /usr/local.',
          'What origin records, and why it decides how much reset destroys.',
        ],
      },
      {
        path: 'reset',
        documentTitle: 'Reset and uninstall sloop without deleting backups',
        title: 'Reset and uninstall',
        written: true,
        blurb:
          'Put the machine back to the moment sloop was installed — and the one thing that can still be lost.',
        covers: [
          'What reset removes and what it leaves.',
          'How origin decides whether a PostgreSQL is destroyed or merely left alone.',
          'That backups are never deleted. Not by reset, not by uninstall, not ever.',
          'The one thing that can still be lost: an encrypted backup needs a private key, and the Argon2id vault holding it is inside the database reset destroys.',
          'sloop uninstall, and the uninstall script — including what its --binary-only leaves behind.',
        ],
      },
    ],
  },
  {
    title: 'Reference',
    line: 'Every flag, every exit code, and how to check the claim yourself.',
    pages: [
      {
        path: 'commands',
        documentTitle: 'sloop command reference — every command and every flag',
        title: 'Command reference',
        written: true,
        blurb: 'Every command and every flag, checked against --help rather than remembered.',
        covers: [
          'Forty commands across eight groups, two paragraphs each and then the table.',
          'The global flags every command takes.',
          'Why --yes, --force and --confirm are three different things and none stands in for another.',
        ],
      },
      {
        path: 'security',
        documentTitle: 'Security — your database credentials never leave the machine',
        title: 'Security',
        written: true,
        blurb: 'The guarantee, expanded — and how to check it yourself in thirty seconds.',
        covers: [
          'No HTTP client and no SSH client in the dependency graph, and the CI script that fails the build if one appears.',
          'The four password routes, and that a registry holds a route and never a value.',
          'Nothing in argv, so nothing in ps output.',
          'Config parsed and never sourced or evaluated, so a password holding a $ or a backtick still works.',
          'Logs written to be pasted into a public issue.',
          'The cargo tree check, one copy away from working.',
        ],
      },
      {
        path: 'privileges',
        documentTitle: 'PostgreSQL and MySQL backup user privileges and GRANTs',
        title: 'The privileges a backup role needs',
        short: 'Role privileges',
        written: true,
        blurb:
          'What a role has to be able to do per engine, what breaks silently without it, and the GRANT that fixes it.',
        covers: [
          'The two silent failures first: short of one grant, MySQL and MariaDB write a dump with every stored routine and every trigger missing from it, say nothing, and exit 0.',
          'The minimum per engine, read from the same table sloop doctor checks a live connection against.',
          'sloop doctor, sloop doctor --offline, and why a missing grant exits 8 rather than 2.',
          'The three privileges MySQL’s manual asks a backup role for that sloop does not need, and why.',
        ],
      },
      {
        path: 'automation',
        documentTitle: 'Automate database backups — exit codes, JSON, dry runs',
        title: 'Automation',
        written: true,
        blurb:
          'Running sloop from something that is not a person: exit codes, non-interactive flags, and output a script can read.',
        covers: [
          'All nine exit codes, what each means to a scheduler, and why 6 is the one worth wiring up.',
          '-y, --force and --confirm, and why none stands in for another.',
          '--json, --quiet, --log-file and --dry-run.',
          'The lockfile, and what a run does when it finds one held.',
          'Passwords under a service account, where the keyring will not work.',
          'sloop’s own service first, then cron, systemd timers and Task Scheduler for anyone who wants their existing scheduler to drive it.',
        ],
      },
      {
        path: 'roadmap',
        documentTitle: 'sloop roadmap — planned queries, DDL and row editing',
        title: 'Roadmap',
        blurb: 'What is planned. No dates, and nothing that reads as a promise.',
        covers: [
          'Writing and saving queries, beyond the read-only builder sloop query has today.',
          'DDL: creating and altering tables from sloop.',
          'Editing rows in place.',
          'A local interface that reads sloop’s own PostgreSQL the way the CLI does.',
          'And that cross-engine migration is not on it.',
        ],
      },
    ],
  },
];

/** Every page, in sidebar order. What prev/next walks. */
export const DOCS_PAGES: readonly DocsPageEntry[] = DOCS_NAV.flatMap((group) => group.pages);

/** The group a page sits in, for the eyebrow above its heading. */
export function groupOf(path: string): DocsGroup | undefined {
  return DOCS_NAV.find((group) => group.pages.some((page) => page.path === path));
}

export function pageAt(path: string): DocsPageEntry | undefined {
  return DOCS_PAGES.find((page) => page.path === path);
}

/** The sidebar label: the short form where there is one. */
export function labelOf(page: DocsPageEntry): string {
  return page.short ?? page.title;
}

export function urlOf(page: DocsPageEntry): string {
  return `/docs/${page.path}`;
}
