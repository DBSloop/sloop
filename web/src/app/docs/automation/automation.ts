import { ChangeDetectionStrategy, Component, computed, inject } from '@angular/core';
import { RouterLink } from '@angular/router';

import { DocsPage } from '../docs-page';
import { OS_LABEL, OsChoice } from '../os';
import { Terminal, type TerminalLine } from '../../ui/terminal';

/** One of the nine exit codes, as a scheduler has to think about it. */
interface Code {
  readonly code: number;
  /** Which of the three status colours the number is painted in. */
  readonly tone: 'ok' | 'warn' | 'bad';
  readonly meaning: string;
  /** What the thing calling `sloop` should do about it. */
  readonly scheduler: string;
  /** The one worth wiring up, which gets the band. */
  readonly lead?: boolean;
}

/** One of the three ways of saying yes, and the two things it cannot do. */
interface Saying {
  readonly flag: string;
  readonly does: string;
  /** The same three questions asked of all three, so the columns line up. */
  readonly can: readonly { readonly label: string; readonly yes: boolean }[];
}

/**
 * Automation.
 *
 * ## How these blocks were made
 *
 * Real runs on 2026-09-19, with the binary built from this commit, against a
 * throwaway PostgreSQL 17.9 cluster on port 5451 built for them and torn down
 * afterwards: three tables, 12,752 rows, a role called `app`, a project registry
 * in a scratch directory.
 *
 * **Every transcript on this page is captured output**, and the ones that
 * matter most are the failures, because a page about automation that only shows
 * things working is a page about the easy half:
 *
 * 1. **`restore` four times over** — bare, with `-y`, with `--force`, and with
 *    `--confirm orders`. The first three exit `2` with the same message, which
 *    is the whole argument of the "none stands in for another" section made by
 *    the program rather than by the page.
 * 2. **`--confirm ordres`**, a typo, refusing before it contacts anything.
 * 3. **Exit `7`**, produced by racing two real backups at the same database —
 *    one started, one found the lock and stood aside naming the process that
 *    held it.
 * 4. **`service schedule` refusing** a schedule whose first run would have
 *    stopped to ask about the backup key at three in the morning. That refusal
 *    is the rule about prompts enforced a day early, and it is the best single
 *    thing on this page.
 * 5. **`backup --all` with one database pointed at a dead port**, plain and as
 *    `--json`, showing it carry on to the end and then report what failed.
 * 6. **`--env` with the variable unset**, which is the shape of every
 *    first-morning failure under a service account.
 *
 * ## The substitutions, named rather than hidden
 *
 * **The project's path.** The registry was built in a scratch directory; on the
 * page it is `C:\Users\you\projects\acme-api` and its POSIX equivalents, the
 * same substitution the backups page makes. Every path under it — the backup
 * directories, the lock file, the log — is the real one with that prefix
 * replaced.
 *
 * **Long lines are broken where a terminal would break them.** sloop does not
 * wrap: it prints one line and lets the emulator soft-wrap it, and a capture
 * redirected to a file therefore holds lines of 170 characters. They are shown
 * here wrapped the way a terminal shows them. No word is changed, added or
 * removed.
 *
 * **Two elisions, and both are marked as comments rather than as output.** The
 * second and third `restore` runs printed the same two lines as the first, and
 * the block says so in a `#` comment instead of repeating them — the point of
 * those two runs is that they are identical, and six identical lines make it
 * harder to see rather than easier. The `--all --json` document's `backups`
 * array holds the two objects the block above it already shows in full, so it
 * is `[ … two of them … ]`. Nothing sloop printed is paraphrased into a
 * coloured line anywhere on the page.
 *
 * Nothing else is edited. Row counts, sizes, durations, checksums, process
 * numbers and timestamps are what the runs printed.
 *
 * ## What is written rather than captured
 *
 * The three scheduler recipes at the foot — the crontab line, the systemd unit
 * and timer, and the `schtasks` command — plus the two exit-code snippets. They
 * are things a reader writes, not things sloop prints, so there is nothing to
 * capture. Each was written against `sloop --help` and the flags on this page,
 * and each is a single paste.
 */
@Component({
  selector: 'app-docs-automation',
  imports: [DocsPage, RouterLink, Terminal],
  templateUrl: './automation.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Automation {
  private readonly choice = inject(OsChoice);

  protected readonly os = this.choice.os;
  protected readonly osLabel = computed(() => OS_LABEL[this.os()]);

  private readonly slash = computed(() => (this.os() === 'windows' ? '\\' : '/'));

  /** The project whose registry every block below belongs to. */
  private readonly project = computed(() => {
    switch (this.os()) {
      case 'windows':
        return 'C:\\Users\\you\\projects\\acme-api';
      case 'macos':
        return '/Users/you/projects/acme-api';
      default:
        return '/home/you/projects/acme-api';
    }
  });

  private path(...parts: readonly string[]): string {
    return [this.project(), '.sloop', ...parts].join(this.slash());
  }

  /** Where the binary lands, which a scheduler has to be told in full. */
  protected readonly binary = computed(() => {
    switch (this.os()) {
      case 'windows':
        return '%LOCALAPPDATA%\\Programs\\sloop\\bin\\sloop.exe';
      case 'macos':
        return '/Users/you/.local/bin/sloop';
      default:
        return '/home/you/.local/bin/sloop';
    }
  });

  /** This platform's own scheduler, named for the reader who has one. */
  protected readonly scheduler = computed(() => {
    switch (this.os()) {
      case 'windows':
        return 'Task Scheduler';
      case 'macos':
        return 'cron';
      default:
        return 'cron or a systemd timer';
    }
  });

  // ── The service, which is the answer first ────────────────────────────────

  /** Attach, then schedule. Both captured. */
  protected readonly scheduling: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop service attach orders' },
    { kind: 'head', tag: 'Attached.', text: ' the service watches orders from its next round' },
    { text: ' ' },
    {
      kind: 'prompt',
      text: 'sloop service schedule orders --every 1d --keep 7 --keep-for-days 30',
    },
    { kind: 'head', tag: 'Scheduled.', text: ' orders is backed up every day' },
    { kind: 'dim', text: '  the newest 7 are kept, and anything older than 30 days is pruned' },
    { kind: 'dim', text: '  no cron line, no scheduled task — the service takes it' },
  ];

  /** What `service status` gives a monitoring system to read. */
  protected readonly status: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop service status --json' },
    { kind: 'dim', text: '{' },
    { kind: 'dim', text: '  "command": "service status",' },
    { kind: 'dim', text: '  "exit": 0,' },
    { kind: 'dim', text: '  "ok": true,' },
    { kind: 'dim', text: '  "result": {' },
    { kind: 'dim', text: '    "last_seen": null,' },
    { kind: 'dim', text: '    "running": true,' },
    { kind: 'dim', text: '    "watching": [' },
    { kind: 'dim', text: '      {' },
    { kind: 'dim', text: '        "attached_at": "2026-09-19T22:23:52+06:00",' },
    { kind: 'dim', text: '        "days_of_history": 0,' },
    { kind: 'name', tag: '        "label": "orders",', text: '' },
    { kind: 'dim', text: '        "schedule": null,' },
    { kind: 'dim', text: '        "seen_at": null' },
    { kind: 'dim', text: '      }' },
    { kind: 'dim', text: '    ]' },
    { kind: 'dim', text: '  }' },
    { kind: 'dim', text: '}' },
  ];

  // ── A scheduled run never stops to ask ────────────────────────────────────

  /**
   * The best thing on the page: a schedule refused a day before it would have
   * hung, because its first run would have had a question and nobody to ask.
   */
  protected readonly refusedEarly: readonly TerminalLine[] = [
    {
      kind: 'prompt',
      text: 'sloop service schedule orders --every 1d --keep 7 --keep-for-days 30',
    },
    {
      kind: 'bad',
      tag: 'error:',
      text: ' this registry has no backup key yet, and the first scheduled backup would',
    },
    {
      kind: 'bad',
      tag: '',
      text: '  make one and then stop to ask about it with no terminal to ask at',
    },
    {
      kind: 'dim',
      text: '  hint: `sloop backup <name>` once by hand makes the key and asks the question,',
    },
    { kind: 'dim', text: '  or `sloop key export` makes it and writes it out' },
  ];

  /** Four runs. Three of them are the argument. */
  protected readonly confirming: readonly TerminalLine[] = [
    { kind: 'dim', text: '# no terminal — this is a cron job, or CI, or a service' },
    { kind: 'prompt', text: 'sloop restore orders' },
    {
      kind: 'bad',
      tag: 'error:',
      text: " replacing a database's contents needs its name typed, and there is no",
    },
    { kind: 'bad', tag: '', text: '  terminal to type at' },
    { kind: 'dim', text: '  hint: pass --confirm orders to say it up front' },
    { kind: 'dim', text: '  → exit 2' },
    { text: ' ' },
    { kind: 'prompt', text: 'sloop restore orders -y' },
    { kind: 'dim', text: '# the same two lines, and the same exit 2' },
    { text: ' ' },
    { kind: 'prompt', text: 'sloop restore orders --force' },
    { kind: 'dim', text: '# the same two lines again, and the same exit 2' },
    { text: ' ' },
    { kind: 'prompt', text: 'sloop restore orders --confirm ordres' },
    { kind: 'bad', tag: 'error:', text: ' --confirm says ordres, and the database is orders' },
    {
      kind: 'dim',
      text: '  hint: nothing was contacted and nothing was changed. The two have to match',
    },
    { kind: 'dim', text: '  exactly' },
    { text: ' ' },
    { kind: 'prompt', text: 'sloop restore orders --confirm orders' },
    { kind: 'ok', tag: '  3 of 3 tables matched', text: '   ← exit 0' },
  ];

  /** The three ways of saying yes, and what each one cannot do. */
  protected readonly sayings: readonly Saying[] = [
    {
      flag: '-y, --yes',
      does: 'Answers a question this would otherwise have stopped to ask.',
      can: [
        { label: 'Answers a question', yes: true },
        { label: 'Overrides a refusal', yes: false },
        { label: 'Destroys a named thing', yes: false },
      ],
    },
    {
      flag: '--force',
      does: 'Overrides a refusal that is there to protect something.',
      can: [
        { label: 'Answers a question', yes: false },
        { label: 'Overrides a refusal', yes: true },
        { label: 'Destroys a named thing', yes: false },
      ],
    },
    {
      flag: '--confirm <NAME>',
      does: 'The name of the thing being destroyed, typed out and matching exactly.',
      can: [
        { label: 'Answers a question', yes: false },
        { label: 'Overrides a refusal', yes: false },
        { label: 'Destroys a named thing', yes: true },
      ],
    },
  ];

  // ── The exit codes ────────────────────────────────────────────────────────

  /**
   * All nine, from `cli/src/exit.rs`, where a test asserts each number and says
   * that editing it to pass is a breaking release.
   */
  protected readonly codes: readonly Code[] = [
    {
      code: 0,
      tone: 'ok',
      meaning: 'It did what it said it would.',
      scheduler: 'Nothing. This is the quiet case, and most runs are it.',
    },
    {
      code: 1,
      tone: 'bad',
      meaning: 'Something went wrong that none of the others describe.',
      scheduler: 'Alert. It is also what a process gets from a panic, so nothing pretends a crash was a category.',
    },
    {
      code: 2,
      tone: 'bad',
      meaning:
        'Bad usage, an unknown database name, or a question with no terminal to ask it at.',
      scheduler:
        'Fix the command, not the machine. This is the code a scheduled line gets on its first morning, and the message names the flag that answers it.',
    },
    {
      code: 3,
      tone: 'bad',
      meaning: 'The database refused, or never answered.',
      scheduler: 'Alert, and look at the database rather than at sloop. Nothing was written.',
    },
    {
      code: 4,
      tone: 'bad',
      meaning: 'The dump failed.',
      scheduler: 'Alert. There is no backup from this run.',
    },
    {
      code: 5,
      tone: 'bad',
      meaning: 'The restore failed.',
      scheduler: 'Alert loudly. A restore that failed halfway has left a database part-loaded.',
    },
    {
      code: 6,
      tone: 'warn',
      lead: true,
      meaning: 'It finished, and then the row counts disagreed.',
      scheduler:
        'Alert, and keep the backup. It is neither success nor a crash, and a check that only tests for zero calls this a success.',
    },
    {
      code: 7,
      tone: 'warn',
      meaning: 'Another run holds the lock on this database.',
      scheduler:
        'Usually nothing. The previous run overran; this one stood aside rather than racing it. Alert only if it happens twice.',
    },
    {
      code: 8,
      tone: 'warn',
      meaning: '`doctor` found something that will break a backup.',
      scheduler:
        'Fix the grant. Distinct from 2 on purpose: a role short of a privilege is not bad usage, and alerting as though it were wakes the wrong person.',
    },
  ];

  /** Reading the code, in whichever shell this reader has. */
  protected readonly branching = computed<readonly TerminalLine[]>(() =>
    this.os() === 'windows' ? this.branchingPowerShell() : this.branchingSh(),
  );

  private branchingSh(): readonly TerminalLine[] {
    return [
      { kind: 'prompt', text: 'sloop backup --all --global -q --log-file /var/log/sloop.log' },
      { kind: 'prompt', text: 'status=$?' },
      { text: ' ' },
      { kind: 'prompt', text: 'case $status in' },
      { kind: 'prompt', text: '  0) ;;' },
      {
        kind: 'prompt',
        text: '  6) alert "sloop: it finished, and the row counts disagreed" ;;',
      },
      { kind: 'prompt', text: '  7) ;;                       # an earlier run is still going' },
      { kind: 'prompt', text: '  *) alert "sloop failed with exit $status" ;;' },
      { kind: 'prompt', text: 'esac' },
    ];
  }

  private branchingPowerShell(): readonly TerminalLine[] {
    return [
      {
        kind: 'prompt',
        text: 'sloop backup --all --global -q --log-file "$env:LOCALAPPDATA\\sloop\\backup.log"',
      },
      { text: ' ' },
      { kind: 'prompt', text: 'switch ($LASTEXITCODE) {' },
      { kind: 'prompt', text: '  0 { }' },
      {
        kind: 'prompt',
        text: '  6 { Send-Alert "sloop: it finished, and the row counts disagreed" }',
      },
      { kind: 'prompt', text: '  7 { }                       # an earlier run is still going' },
      { kind: 'prompt', text: '  default { Send-Alert "sloop failed with exit $LASTEXITCODE" }' },
      { kind: 'prompt', text: '}' },
    ];
  }

  // ── The lock ──────────────────────────────────────────────────────────────

  /** Two real backups at one database, a second apart. */
  protected readonly locked = computed<readonly TerminalLine[]>(() => [
    { kind: 'dim', text: '# the nightly run is still going when the hourly one starts' },
    { kind: 'prompt', text: 'sloop backup orders' },
    {
      kind: 'bad',
      tag: 'error:',
      text: ' another sloop run is working on orders: `sloop backup`, process 18220,',
    },
    { kind: 'bad', tag: '', text: '  since 20260919T162205Z' },
    {
      kind: 'dim',
      text: '  hint: exit 7 means this run did not start rather than that it failed. The lock',
    },
    { kind: 'dim', text: `  is at ${this.path('locks', 'orders.lock')}, and it goes` },
    { kind: 'dim', text: '  when that run does' },
  ]);

  // ── Output a script can read ──────────────────────────────────────────────

  /** One document, and it is the whole of standard output. */
  protected readonly json = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop backup orders --json' },
    { kind: 'dim', text: '{' },
    { kind: 'dim', text: '  "command": "backup",' },
    { kind: 'dim', text: '  "exit": 0,' },
    { kind: 'dim', text: '  "ok": true,' },
    { kind: 'dim', text: '  "result": {' },
    { kind: 'dim', text: '    "backups": [' },
    { kind: 'dim', text: '      {' },
    { kind: 'dim', text: '        "bytes": 83724,' },
    { kind: 'dim', text: '        "database": "orders",' },
    {
      kind: 'dim',
      text: `        "directory": "${this.jsonPath('backups', 'postgres', 'orders', '20260919T162109Z')}",`,
    },
    { kind: 'dim', text: '        "encrypted": true,' },
    { kind: 'dim', text: '        "engine": "postgres",' },
    { kind: 'dim', text: '        "label": "orders",' },
    { kind: 'name', tag: '        "rows": 12752,', text: '' },
    { kind: 'dim', text: '        "seconds": 0.418,' },
    {
      kind: 'dim',
      text: '        "sha256": "fc9f985fd6294d232fcf97a09302c74049d60dc0dcbade729295754559f184db",',
    },
    { kind: 'dim', text: '        "tables": 3,' },
    { kind: 'dim', text: '        "taken_local": "2026-09-19T22:21:09+06:00",' },
    { kind: 'dim', text: '        "taken_utc": "2026-09-19T16:21:09Z"' },
    { kind: 'dim', text: '      }' },
    { kind: 'dim', text: '    ]' },
    { kind: 'dim', text: '  }' },
    { kind: 'dim', text: '}' },
  ]);

  /** A failure is the same document with the other half filled in. */
  protected readonly jsonError: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop backup orders --json' },
    { kind: 'dim', text: '{' },
    {
      kind: 'bad',
      tag: '  "error": "the backup key has not been copied anywhere yet, and there is no',
      text: '',
    },
    { kind: 'bad', tag: '    terminal to ask at",', text: '' },
    { kind: 'dim', text: '  "exit": 2,' },
    { kind: 'dim', text: '  "hint": "run `sloop key export` once, then this run will go through",' },
    { kind: 'dim', text: '  "ok": false' },
    { kind: 'dim', text: '}' },
  ];

  /** Silent on the terminal, complete in the file. */
  protected readonly quietLog = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop backup orders --quiet --log-file nightly.log' },
    { kind: 'dim', text: '# nothing. That is what --quiet means.' },
    { text: ' ' },
    { kind: 'prompt', text: 'cat nightly.log' },
    { kind: 'name', tag: 'orders', text: '  postgres://app@127.0.0.1:5451/orders' },
    { kind: 'dim', text: '  postgres 17.9, 3 tables, 12752 rows' },
    {
      kind: 'dim',
      text: `  backed up to ${this.path('backups', 'postgres', 'orders', '20260919T162303Z')}`,
    },
    { kind: 'dim', text: '  82.1 KiB in 0.3s, sha256 0b2b7d79c729' },
    { kind: 'dim', text: '  taken 2026-09-19 22:23:03 +06:00 (2026-09-19T16:23:03Z)' },
  ]);

  /** A real rehearsal: everything read and checked, nothing written. */
  protected readonly dryRun = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop backup orders --dry-run' },
    { kind: 'name', tag: 'orders', text: '  postgres://app@127.0.0.1:5451/orders' },
    { kind: 'dim', text: '  postgres 17.9, 3 tables, 12752 rows' },
    {
      kind: 'ok',
      tag: `dry run would back up orders to ${this.path()} — nothing was changed`,
      text: '',
    },
  ]);

  // ── --all carries on ──────────────────────────────────────────────────────

  /** One database pointed at a dead port, and it still finished the other two. */
  protected readonly all = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop backup --all' },
    { kind: 'name', tag: 'archive', text: '  postgres://app@127.0.0.1:5999/archive' },
    {
      kind: 'bad',
      tag: 'error:',
      text: ' archive: psql.exe failed against postgres://app@127.0.0.1:5999/archive:',
    },
    {
      kind: 'bad',
      tag: '',
      text: '  psql: error: connection to server at "127.0.0.1", port 5999 failed:',
    },
    { kind: 'bad', tag: '', text: '  Connection refused (0x0000274D/10061)' },
    {
      kind: 'dim',
      text: '  hint: check the host, port, user and password sloop was given: `sloop db list`',
    },
    { kind: 'dim', text: '  shows them and `sloop db test <name>` tries them' },
    { text: ' ' },
    { kind: 'name', tag: 'nightly', text: '  postgres://app@127.0.0.1:5451/orders' },
    { kind: 'dim', text: '  postgres 17.9, 3 tables, 12752 rows' },
    { kind: 'dim', text: '  82.0 KiB in 0.3s, sha256 52549a64b0ee' },
    { text: ' ' },
    { kind: 'name', tag: 'orders', text: '  postgres://app@127.0.0.1:5451/orders' },
    { kind: 'dim', text: '  postgres 17.9, 3 tables, 12752 rows' },
    { kind: 'dim', text: '  82.1 KiB in 0.2s, sha256 cfd64780e06e' },
    { text: ' ' },
    { kind: 'warn', tag: 'backed up 2 of 3 — 1 failed: archive', text: '   ← exit 3' },
  ]);

  /** The same run, as the document a script reads instead. */
  protected readonly allJson: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop backup --all --json' },
    { kind: 'dim', text: '{' },
    { kind: 'dim', text: '  "command": "backup",' },
    { kind: 'warn', tag: '  "exit": 3,', text: '' },
    { kind: 'warn', tag: '  "ok": false,', text: '' },
    { kind: 'dim', text: '  "result": {' },
    { kind: 'dim', text: '    "attempted": 3,' },
    { kind: 'dim', text: '    "backups": [ … two of them … ],' },
    { kind: 'dim', text: '    "failed": [' },
    { kind: 'dim', text: '      {' },
    { kind: 'dim', text: '        "error": "archive: psql.exe failed against …",' },
    { kind: 'dim', text: '        "exit": 3,' },
    { kind: 'name', tag: '        "name": "archive"', text: '' },
    { kind: 'dim', text: '      }' },
    { kind: 'dim', text: '    ]' },
    { kind: 'dim', text: '  }' },
    { kind: 'dim', text: '}' },
  ];

  // ── Passwords with nobody logged in ───────────────────────────────────────

  /** The first-morning failure, and the two routes that avoid it. */
  protected readonly envMissing: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop backup nightly' },
    { kind: 'name', tag: 'nightly', text: '  postgres://app@127.0.0.1:5451/orders' },
    {
      kind: 'bad',
      tag: 'error:',
      text: ' SLOOP_DB_PW is not set, and the registry says the password is in it',
    },
    {
      kind: 'dim',
      text: '  hint: export SLOOP_DB_PW before running this, or change the password route',
    },
    { kind: 'dim', text: '  → exit 2' },
  ];

  /** The route is in the listing, so which one a record takes is never a guess. */
  protected readonly listing: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop db list' },
    {
      kind: 'name',
      tag: 'nightly',
      text: '  postgres://app@127.0.0.1:5451/orders',
      note: '  project · the environment variable SLOOP_DB_PW',
    },
    {
      kind: 'name',
      tag: 'orders ',
      text: '  postgres://app@127.0.0.1:5451/orders',
      note: '  project · the OS keyring',
    },
  ];

  /** The keyring's own words when there is no session to open it in. */
  protected readonly noKeyring: readonly TerminalLine[] = [
    {
      kind: 'bad',
      tag: 'error:',
      text: ' the OS keyring is not usable here: platform secure storage failure',
    },
    {
      kind: 'dim',
      text: '  hint: on a machine with no keyring — a headless Linux box, or a scheduled job —',
    },
    {
      kind: 'dim',
      text: '  use encrypted-file, ${A_VARIABLE}, or command:<command line> instead',
    },
  ];

  // ── Somebody else's scheduler ─────────────────────────────────────────────

  /** A crontab line, for Linux and macOS. */
  protected readonly cron = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'crontab -e' },
    { text: ' ' },
    { kind: 'dim', text: '# 03:00 every day. The route was set once, at db add:' },
    {
      kind: 'dim',
      text: '#   sloop db add orders --url … --password-from "cat /etc/sloop/orders.pw"',
    },
    {
      kind: 'out',
      text: `0 3 * * * ${this.binary()} backup --all --global -q --log-file ${this.logPath()}`,
    },
  ]);

  /** The unit and the timer, for a Linux box that has systemd. */
  protected readonly unit: readonly TerminalLine[] = [
    { kind: 'dim', text: '# /etc/systemd/system/sloop-backup.service' },
    { kind: 'out', text: '[Unit]' },
    { kind: 'out', text: 'Description=sloop nightly backup' },
    { text: ' ' },
    { kind: 'out', text: '[Service]' },
    { kind: 'out', text: 'Type=oneshot' },
    { kind: 'out', text: 'User=sloop' },
    { kind: 'out', text: 'Environment=SLOOP_PASSPHRASE_FILE=/etc/sloop/passphrase' },
    {
      kind: 'out',
      text: 'ExecStart=/usr/local/bin/sloop backup --all --global -q --log-file /var/log/sloop.log',
    },
  ];

  protected readonly timer: readonly TerminalLine[] = [
    { kind: 'dim', text: '# /etc/systemd/system/sloop-backup.timer' },
    { kind: 'out', text: '[Unit]' },
    { kind: 'out', text: 'Description=Run sloop every night at three' },
    { text: ' ' },
    { kind: 'out', text: '[Timer]' },
    { kind: 'out', text: 'OnCalendar=*-*-* 03:00:00' },
    { kind: 'out', text: 'Persistent=true' },
    { text: ' ' },
    { kind: 'out', text: '[Install]' },
    { kind: 'out', text: 'WantedBy=timers.target' },
    { text: ' ' },
    { kind: 'prompt', text: 'systemctl enable --now sloop-backup.timer' },
  ];

  /**
   * A one-line batch file, and a task that runs it.
   *
   * **Not one long `/tr`**, and that is the whole reason this is two steps:
   * `schtasks` takes the command as a single quoted argument, so a path with a
   * space in it needs quotes inside quotes, which is where every recipe for
   * this goes wrong. A `.cmd` file has none of that problem, and it is also the
   * thing you can run by hand once before trusting it to three in the morning.
   */
  protected readonly batch: readonly TerminalLine[] = [
    { kind: 'dim', text: '@rem  %USERPROFILE%\\sloop-nightly.cmd' },
    {
      kind: 'out',
      text: '"%LOCALAPPDATA%\\Programs\\sloop\\bin\\sloop.exe" backup --all --global -q --log-file "%LOCALAPPDATA%\\sloop\\backup.log"',
    },
  ];

  /** The task that runs it, as you, at three. One line, so there is no `^`. */
  protected readonly schtasks: readonly TerminalLine[] = [
    {
      kind: 'prompt',
      text: 'schtasks /create /tn "sloop nightly" /sc daily /st 03:00 /tr "%USERPROFILE%\\sloop-nightly.cmd" /ru "%USERNAME%" /rp',
    },
  ];

  /** Where a scheduled run's log goes on this platform. */
  private logPath(): string {
    return this.os() === 'macos' ? '/Users/you/.local/state/sloop.log' : '/var/log/sloop.log';
  }

  /** The same path as `path()`, with backslashes escaped the way JSON prints them. */
  private jsonPath(...parts: readonly string[]): string {
    return this.path(...parts).replaceAll('\\', '\\\\');
  }
}
