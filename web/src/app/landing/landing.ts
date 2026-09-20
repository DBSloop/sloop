import { ChangeDetectionStrategy, Component } from '@angular/core';
import { RouterLink } from '@angular/router';

import { Mark } from '../mark';
import { InstallCommand } from '../ui/install-command';
import { Depth } from '../ui/depth';
import { Reveal } from '../ui/reveal';
import { Terminal, type TerminalLine } from '../ui/terminal';
import { Stones } from './stones';
import { Strata } from './strata';

interface ExitCode {
  readonly code: string;
  readonly meaning: string;
  /**
   * The three colours the CLI itself prints in, used for the one thing on this
   * page they genuinely describe. `0` worked. `6` finished and then the numbers
   * disagreed, which is the whole reason it is not `0` and not a crash. Every
   * other code is a run that did not do what it was asked.
   */
  readonly tone: 'ok' | 'warn' | 'bad';
}

/** One cell of the band under the hero: what sloop does, at a glance. */
interface Capability {
  readonly command: string;
  readonly title: string;
  readonly line: string;
}

/** One of the three steps that take a machine from nothing to a nightly backup. */
interface Step {
  readonly step: string;
  readonly command: string;
  readonly title: string;
  readonly line: string;
}

/** One step of the order a bare database name is resolved in. */
interface Rule {
  readonly flag: string;
  readonly means: string;
}

/**
 * The landing page.
 *
 * Two kinds of thing live here and nothing else: tabular data — the exit codes,
 * the capability band, the three service steps — and the terminal transcripts.
 * All the prose is literal markup in the template, so a phrase can carry a
 * `<code>` or a `<strong>` where it needs one, which a string interpolated as
 * text cannot.
 *
 * **Every transcript below was composed from the format strings in `cli/`, not
 * from an idea of what the command might print.** Where one is an excerpt — a
 * hero window is 250px and a real line of sloop's output is not — the cut is
 * marked with a dim `…` rather than left to look like the whole thing.
 * Sources, so the next person can check them again:
 *
 * ```text
 * backup            commands/backup.rs  — one(), Taken::describe(), describe_bytes()
 * verify            verify/mod.rs       — Comparison::describe(), Mode::describe()
 * db list           commands/db.rs      — list(), secret::Route::describe()
 * service status    commands/service.rs — status(), State::spoken()
 * service schedule  commands/service.rs — schedule(), schedule::every_reads_as()
 * service activity  commands/activity.rs— say(), one(), window()
 * query --sql       commands/query.rs   — run(), print()
 * the menu          ui/screen.rs        — HOME, the seven Shelf headings
 * the printed line  ui/mod.rs           — Stage::equivalent()
 * ```
 *
 * Sizes are `MiB`/`GiB` because `describe_bytes` divides by 1024 and labels it
 * that way. A page that rounded them to `MB` would be the only place in this
 * project that says something the binary does not.
 */
/** One door on the home screen, for the panel that says what is behind them. */
interface Door {
  readonly title: string;
  readonly command: string;
  readonly holds: string;
}

@Component({
  selector: 'app-landing',
  imports: [Depth, InstallCommand, Mark, Reveal, RouterLink, Stones, Strata, Terminal],
  templateUrl: './landing.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Landing {
  // ── The band under the hero ───────────────────────────────────────────────
  //
  // The eight things A4a's scope asks to land at or near the fold, each as the
  // command that does it. No icons: on a page about a command-line tool the
  // commands *are* the iconography, and eight invented glyphs would be eight
  // more things to look at rather than one more thing to understand.

  protected readonly capabilities: readonly Capability[] = [
    {
      command: 'sloop backup',
      title: 'Backups that prove themselves',
      line: 'Every row counted on both sides, and a manifest filed beside every dump.',
    },
    {
      command: 'sloop restore',
      title: 'Put one back',
      line: 'Replaces what is in the database now. Typed out, never clicked, then verified.',
    },
    {
      command: 'sloop mirror',
      title: 'An exact copy',
      line: 'The destination ends up identical to the source. Anything else there is gone.',
    },
    {
      command: 'sloop sync',
      title: 'A merge, not a copy',
      line: 'Rows added and replaced, and rows only the destination had are kept.',
    },
    {
      command: 'sloop query',
      title: 'Read one without SQL',
      line: 'Pick a table, tick the columns. The server is the thing that refuses writes.',
    },
    {
      command: 'sloop service schedule',
      title: 'A schedule, with no cron',
      line: 'Nothing is written to crontab, Task Scheduler or a systemd timer.',
    },
    {
      command: 'sloop service activity',
      title: 'What each one has been doing',
      line: 'Rows in, rows out and size, by day, by week and by month.',
    },
    {
      command: 'sloop doctor',
      title: 'Before you trust any of it',
      line: 'The grants a backup role really needs, checked against the live connection.',
    },
  ];

  /** Nothing to install, nothing to configure, and no cron line at the end of it. */
  protected readonly steps: readonly Step[] = [
    {
      step: '01',
      command: 'sloop service install',
      title: 'Register it with the machine',
      line: 'A systemd unit, a launchd daemon, or a real Windows service. It comes back at boot. Needs an administrator or sudo, and says so before it touches anything.',
    },
    {
      step: '02',
      command: 'sloop service attach shop',
      title: 'Point it at a database',
      line: 'Picked up at the next round, with nothing restarted. Detaching stops the readings and deletes none of what was already recorded.',
    },
    {
      step: '03',
      // Broken with a shell continuation rather than left to the browser,
      // which takes a hyphen as a wrap opportunity and splits `--keep 7`
      // into `--` and `keep 7`.
      command: 'sloop service schedule shop \\\n    --every 1d --keep 7',
      title: 'Say how often, and how many to keep',
      line: 'It runs the same backup and prune you would have typed, prunes only after a backup that worked, and catches a missed run up once rather than a hundred times.',
    },
  ];

  /**
   * The five steps, in order, and the first that applies wins.
   *
   * `registry::Resolution` in the CLI, which also carries *why* it answered
   * the way it did — which is what every command prints beside the answer.
   */
  protected readonly resolution: readonly Rule[] = [
    { flag: '--global', means: 'the global registry, whichever directory you are standing in.' },
    { flag: '-C <path|name>', means: 'that project — a directory, or a name sloop init recorded.' },
    { flag: 'SLOOP_PROJECT', means: 'the same thing from the environment. -C outranks it.' },
    {
      flag: 'the nearest .sloop',
      means: 'at or above this directory, stopping short of your home.',
    },
    { flag: 'otherwise', means: 'the global registry.' },
  ];

  /**
   * The six doors, and what each one holds.
   *
   * `ui::screen::DOORS`, and the commands behind each are that door's `leaves`.
   * The front page carries none of these words — it is six lines — and typing
   * any of them still finds the door, because `Door::finds` matches on names
   * the screen never draws.
   */
  protected readonly doors: readonly Door[] = [
    {
      title: 'Databases',
      command: 'sloop db …',
      holds: 'add, create, list, test, edit, rename, remove, drop',
    },
    {
      title: 'Backups',
      command: 'sloop backup …',
      holds: 'back one up, back all up, list, prune, restore',
    },
    { title: 'Copy a database', command: 'sloop mirror …', holds: 'mirror, sync' },
    { title: 'Look inside one', command: 'sloop query …', holds: 'query' },
    {
      title: 'Background service',
      command: 'sloop service …',
      holds: 'install, attach, schedule, activity, status, detach, start, stop, uninstall',
    },
    {
      title: 'This machine',
      command: 'sloop doctor',
      holds:
        "install a database server, open sloop's own database, doctor, export the backup key, import one",
    },
  ];

  /** Frozen at 1.0, because automation depends on them. `cli/src/exit.rs`. */
  protected readonly exitCodes: readonly ExitCode[] = [
    { code: '0', tone: 'ok', meaning: 'It worked.' },
    {
      code: '1',
      tone: 'bad',
      meaning: 'Something went wrong that none of the codes below describes.',
    },
    {
      code: '2',
      tone: 'bad',
      meaning: 'Bad usage, an unknown name, or a question with no terminal to ask it at.',
    },
    { code: '3', tone: 'bad', meaning: 'The connection failed.' },
    { code: '4', tone: 'bad', meaning: 'The dump failed.' },
    { code: '5', tone: 'bad', meaning: 'The restore failed.' },
    { code: '6', tone: 'warn', meaning: 'It finished, and then the row counts did not agree.' },
    { code: '7', tone: 'warn', meaning: 'Another run holds the lock on that database.' },
    { code: '8', tone: 'warn', meaning: 'doctor found something that will break a backup.' },
  ];

  /** The palette the CLI prints these in, so the page and the tool agree. */
  protected readonly toneClass = {
    ok: 'border-ok/35 bg-ok/10 text-ok',
    warn: 'border-warn/35 bg-warn/10 text-warn',
    bad: 'border-bad/35 bg-bad/10 text-bad',
  } as const;

  // ── The three windows that float around the stack ─────────────────────────
  //
  // Real output at 272–300px, so the longest lines run off the right edge and
  // are faded there rather than cut short. One command each: the menu, the
  // service, and a backup.

  /**
   * `sloop` with no arguments opens the menu, and this is its front page.
   *
   * **Six doors**, in a window narrow enough that the CLI draws the titles and
   * nothing else — `paint::option`'s last branch, where a row under about
   * twenty-four columns of room carries its title alone. The commands behind
   * each door are one keystroke away, and typing `mirror` here still finds the
   * door that holds it, because `Door::finds` carries the word the screen does
   * not draw.
   */
  protected readonly menuSnippet: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop' },
    { kind: 'name', tag: '?', text: ' What would you like to do?' },
    { kind: 'row', text: 'Databases', here: true },
    { kind: 'row', text: 'Backups' },
    { kind: 'row', text: 'Copy a database' },
    { kind: 'row', text: 'Look inside one' },
    { kind: 'row', text: 'Background service' },
    { kind: 'row', text: 'This machine' },
  ];

  /**
   * The same screen with room to be itself, at the eighty-eight columns the
   * design was drawn on.
   *
   * **The mark beside the facts, not stacked under them.** Stacked, this header
   * took eight rows of art, a rule and three rows of facts — twenty of a
   * twenty-four-row terminal, which left the list a five-row keyhole. Side by
   * side they take eight rows together. The `lead` on each line is the wordmark
   * column, exactly as `paint::marked` lays it out.
   *
   * **Three columns in every row**, because that is `paint::option`: the title,
   * then the thing worth knowing in the colour it deserves — green for a thing
   * that is working, amber for one that wants attention, grey for a phrase that
   * is only a description — then the flag form in grey on the right. The
   * columns are measured across the whole list rather than per row, which is
   * most of what stops a list looking cluttered.
   */
  protected readonly homeScreen: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop' },
    { text: ' ' },
    { lead: '       _', text: '' },
    {
      lead: '      | |                        ',
      kind: 'head',
      tag: 'sloop',
      text: '',
      note: '  v0.1.1',
    },
    {
      lead: '   ___| | ___   ___  _ __        ',
      text: 'your databases, backed up and moved about',
    },
    { lead: "  / __| |/ _ \\ / _ \\| '_ \\", text: '' },
    {
      lead: '  \\__ \\ | (_) | (_) | |_) |      ',
      kind: 'chips',
      text: '',
      chips: [
        { mark: 'ok', text: '3 databases' },
        { mark: 'ok', text: 'last backup 2h ago' },
      ],
    },
    {
      lead: '  |___/_|\\___/ \\___/| .__/       ',
      kind: 'chips',
      text: '',
      chips: [{ mark: 'ok', text: 'service running' }],
    },
    {
      lead: '                    | |          ',
      kind: 'dim',
      text: 'C:\\Users\\you\\.sloop  ·  the global registry',
    },
    { lead: '                    |_|', text: '' },
    { text: ' ' },
    { kind: 'name', tag: '?', text: ' What would you like to do?' },
    {
      kind: 'row',
      here: true,
      text: 'Databases',
      middle: '3 registered',
      hue: 'ok',
      note: 'sloop db …',
    },
    {
      kind: 'row',
      text: 'Backups',
      middle: '12 kept · newest 2h ago',
      hue: 'ok',
      note: 'sloop backup …',
    },
    {
      kind: 'row',
      text: 'Copy a database',
      middle: 'mirror exactly, or merge with sync',
      note: 'sloop mirror …',
    },
    {
      kind: 'row',
      text: 'Look inside one',
      middle: 'tables and rows, with no SQL to type',
      note: 'sloop query …',
    },
    {
      kind: 'row',
      text: 'Background service',
      middle: 'running',
      hue: 'ok',
      note: 'sloop service …',
    },
    {
      kind: 'row',
      text: 'This machine',
      middle: 'servers, the backup key, and a health check',
      note: 'sloop doctor',
    },
    { text: ' ' },
    { kind: 'row', text: 'Quit', at: 2 },
    { text: ' ' },
    {
      kind: 'dim',
      text: '  ↑↓ to move, enter to choose, esc to go back, or type a few letters to filter',
    },
  ];

  /** Installed, running, and coming back at boot — three separate questions. */
  protected readonly serviceSnippet: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop service status' },
    { kind: 'head', tag: 'Service', text: '' },
    { kind: 'label', tag: '  State ', text: 'running' },
    { kind: 'label', tag: '  Managed by ', text: 'systemd' },
    { kind: 'label', tag: '  At boot ', text: 'starts by itself' },
  ];

  /**
   * A schedule, set once.
   *
   * The command is broken with a shell continuation and the last output line —
   * *no cron line, no scheduled task — the service takes it* — is the window's
   * caption instead, marked here by the `…` the other windows use. Fifty-five
   * characters is 470px of monospace, and a 470px card would be most of the
   * stage.
   */
  protected readonly scheduleSnippet: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop service schedule shop \\' },
    { text: '      --every 1d --keep 7' },
    { kind: 'head', tag: 'Scheduled.', text: '', note: ' shop is backed up every day' },
    { kind: 'dim', text: '  …' },
  ];

  /** What the machine has, before anything is trusted to it. */
  protected readonly doctorSnippet: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop doctor' },
    { kind: 'head', tag: 'Client tools', text: '' },
    { kind: 'name', tag: '  postgres', text: '' },
    { kind: 'label', tag: '    pg_dump        ', text: '17.9' },
    { kind: 'name', tag: '  mysql', text: '' },
    { kind: 'label', tag: '    mysqldump      ', text: '8.4.11' },
    { kind: 'name', tag: '  mariadb', text: '' },
    { kind: 'label', tag: '    mariadb-dump   ', text: '11.8.9' },
  ];

  /**
   * The block that types itself, under the band.
   *
   * Three commands and nothing between them: register a database, take a
   * backup that proves it landed, put it on a schedule. It is the whole
   * product in nine seconds, and every character of it is in the DOM from the
   * first paint — the run only decides when each one becomes visible.
   */
  protected readonly firstRun: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop db add shop --url postgres://app@db.internal:5432/orders' },
    {
      kind: 'name',
      tag: 'registered',
      text: ' shop in the project registry',
      note: ' (a .sloop at or above the working directory), password from the OS keyring',
    },
    { kind: 'dim', text: '  postgres://app@db.internal:5432/orders' },
    { text: ' ' },
    { kind: 'prompt', text: 'sloop backup shop' },
    { kind: 'name', tag: 'shop', text: '', note: '  postgres://app@db.internal:5432/orders' },
    {
      kind: 'step',
      mark: 'ok',
      text: 'Connected',
      note: 'postgres 17.9, TLS, 48 tables, 1284003 rows',
    },
    { kind: 'step', mark: 'ok', text: 'Dumped 48 tables', note: '432.6 MB, encrypted' },
    { kind: 'step', mark: 'ok', text: 'Checked', note: '9f3ac1d2e8b0' },
    { kind: 'dim', text: '  backed up to backups/postgres/shop/20260919T031500Z' },
    { kind: 'dim', text: '  412.6 MiB in 12.8s, sha256 9f3ac1d2e8b0' },
    { kind: 'dim', text: '  taken 2026-09-19 09:15:00 +06:00 (20260919T031500Z)' },
    { text: ' ' },
    { kind: 'prompt', text: 'sloop service schedule shop --every 1d --keep 7' },
    { kind: 'head', tag: 'Scheduled.', text: '', note: ' shop is backed up every day' },
    { kind: 'dim', text: '  the newest 7 are kept, the rest are pruned' },
    { kind: 'dim', text: '  no cron line, no scheduled task — the service takes it' },
  ];

  // ── The transcripts in the sections ───────────────────────────────────────

  /** The claim, and the thirty seconds it takes to check it. */
  protected readonly guaranteeProof: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'cd cli' },
    { kind: 'prompt', text: "cargo tree | grep -Ei 'reqwest|hyper|ureq|curl'" },
    { kind: 'dim', text: '# nothing. no HTTP client is in this graph.' },
    { text: ' ' },
    { kind: 'prompt', text: "cargo tree | grep -Ei 'ssh2|russh|libssh2'" },
    { kind: 'dim', text: '# nothing either. no socket of any kind is in it.' },
  ];

  /** What an interactive run leaves behind, which is the point of it. */
  /**
   * Where a finished job lands, and the reason there is such a screen.
   *
   * Nothing drawn inside the alternate buffer survives it, so what a job said
   * while it ran is gone the moment it returns — this is what is left, and it
   * holds the whole of it: the verdict, every step it took on a rail in the
   * colour it settled with, and the flag form of what just ran, so a session
   * somebody worked out by hand becomes a line they can schedule.
   *
   * **Its way out is `← Home`, not `← Back`**, and the list is otherwise empty:
   * the thing it had just done used to sit there selected, so Enter on the
   * screen saying a database had been created created another one.
   */
  protected readonly resultScreen: readonly TerminalLine[] = [
    { text: ' ' },
    { kind: 'trail', at: 2, text: '', crumbs: ['Backups', 'Back one up now'] },
    { text: ' ' },
    {
      kind: 'verdict',
      at: 2,
      mark: 'ok',
      text: 'DONE',
      tag: 'Back one up now',
      note: '4.2s',
    },
    { text: ' ' },
    {
      kind: 'step',
      rail: true,
      at: 2,
      mark: 'ok',
      text: 'Connected',
      note: 'postgres 17.9, 2 tables, 10601 rows',
    },
    {
      kind: 'step',
      rail: true,
      at: 2,
      mark: 'ok',
      text: 'Dumped 2 tables',
      note: '67.6 kB, encrypted',
    },
    { kind: 'step', rail: true, at: 2, mark: 'ok', text: 'Checked', note: '9f2a1c4e77b0' },
    {
      kind: 'step',
      rail: true,
      at: 2,
      mark: 'ok',
      text: 'Verified',
      note: 'exact count(*) on both sides',
    },
    { text: ' ' },
    { kind: 'dim', text: '  the same thing, from a shell' },
    { kind: 'name', tag: '  sloop backup shop --yes', text: '' },
    { text: ' ' },
    { kind: 'name', tag: '?', text: ' That is done.' },
    { kind: 'row', here: true, text: '← Home', at: 2 },
  ];

  /** A route, never a value. Not one of these phrases can hold a password. */
  protected readonly registrySnippet: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop db list' },
    {
      kind: 'name',
      tag: 'shop   ',
      text: '  postgres://app@db.internal:5432/orders',
      note: '  project · the OS keyring',
    },
    {
      kind: 'name',
      tag: 'billing',
      text: '  mysql://svc@10.0.0.9:3306/billing      ',
      note: '  global · the environment variable ${BILLING_PW}',
    },
    {
      kind: 'name',
      tag: 'reports',
      text: '  postgres://ro@127.0.0.1:5432/reports   ',
      note: '  global · the command `op read op://vault/db/pw`',
    },
    { kind: 'dim', text: '         through ssh://deploy@bastion.internal:22' },
  ];

  /**
   * A backup, and then a copy of it proved row by row.
   *
   * The `verifying — exact count(*) on both sides` heading and the padded
   * `source → destination` columns are `verify::Comparison::describe()`.
   */
  protected readonly backupOutput: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop backup shop' },
    { kind: 'name', tag: 'shop', text: '', note: '  postgres://app@db.internal:5432/orders' },
    {
      kind: 'step',
      mark: 'ok',
      text: 'Connected',
      note: 'postgres 17.9, TLS, 48 tables, 1284003 rows',
    },
    { kind: 'step', mark: 'ok', text: 'Dumped 48 tables', note: '432.6 MB, encrypted' },
    { kind: 'step', mark: 'ok', text: 'Checked', note: '9f3ac1d2e8b0' },
    { kind: 'dim', text: '  backed up to backups/postgres/shop/20260919T031500Z' },
    { kind: 'dim', text: '  412.6 MiB in 12.8s, sha256 9f3ac1d2e8b0' },
    { kind: 'dim', text: '  taken 2026-09-19 09:15:00 +06:00 (20260919T031500Z)' },
    { text: ' ' },
    { kind: 'prompt', text: 'sloop mirror shop --to shopcopy --confirm shopcopy' },
    { kind: 'dim', text: '  copied in 2.4s' },
    {
      kind: 'step',
      mark: 'warn',
      text: 'Verified',
      note: '1 table moved — the source was live',
    },
    { kind: 'dim', text: '  verifying — exact count(*) on both sides' },
    { kind: 'dim', text: '    orders        500 → 500' },
    { kind: 'dim', text: '    line_items   1284 → 1284' },
    { kind: 'dim', text: '    audit_log    9120 → 9163   drift: the source is live' },
    { kind: 'dim', text: '    3 of 3 tables matched, 1 drifted' },
  ];

  /** Rows the server counted, and bytes sloop moved. Never the two added up. */
  protected readonly activityOutput: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop service activity' },
    { kind: 'head', tag: 'Activity', text: '', note: ' 2 databases attached' },
    { text: ' ' },
    { kind: 'name', tag: 'shop', text: '' },
    {
      kind: 'label',
      tag: '  Today   ',
      text: '184,220 rows in · 1,905,633 out',
      note: ' · 1,440 readings',
    },
    {
      kind: 'label',
      tag: '  7 days  ',
      text: '1,284,004 rows in · 13,402,118 out',
      note: ' · 10,080 readings',
    },
    {
      kind: 'label',
      tag: '  30 days ',
      text: '5,610,882 rows in · 57,884,203 out',
      note: ' · 43,200 readings',
    },
    {
      kind: 'label',
      tag: '  Size    ',
      text: '412.6 MiB',
      note: ' on disk, read 2026-09-19 09:41:02 +06:00',
    },
    {
      kind: 'label',
      tag: '  Moved   ',
      text: '11.8 GiB in · 0 B out',
      note: ' · real bytes sloop moved, over 30 days',
    },
    { text: ' ' },
    { kind: 'name', tag: 'billing', text: '' },
    { kind: 'label', tag: '  Today   ', text: '', note: 'no readings in this period' },
    {
      kind: 'label',
      tag: '  7 days  ',
      text: '48,110 rows in · 211,904 out',
      note: ' · 3,902 readings',
    },
    { text: ' ' },
    { kind: 'dim', text: '  rows are what the server counted; Moved is real bytes sloop read or' },
    { kind: 'dim', text: '  wrote. Nothing here is bytes on the wire — no engine reports those' },
    { kind: 'dim', text: '  per database.' },
  ];

  /**
   * The flag form of the builder. It prints rather than opening the grid, so
   * the output can be piped somewhere — `commands::query::straight`.
   */
  protected readonly querySnippet: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop query shop --sql "select name, state from orders"' },
    { kind: 'head', tag: 'Reading:', text: ' shop', note: '  project' },
    { text: ' ' },
    { kind: 'head', tag: 'name          state', text: '' },
    { text: 'Ada Lovelace  shipped' },
    { text: 'Grace Hopper  packing' },
    { text: 'Alan Turing   shipped' },
    { text: ' ' },
    { kind: 'head', tag: 'Rows:', text: ' 3' },
  ];
}
