import { ChangeDetectionStrategy, Component, computed, inject } from '@angular/core';
import { RouterLink } from '@angular/router';

import { DocsPage } from '../docs-page';
import { OS_LABEL, OsChoice } from '../os';
import { Terminal, type TerminalLine } from '../../ui/terminal';

/** One line of the two columns: something that goes, or something that stays. */
interface Item {
  readonly what: string;
  readonly detail: string;
}

/**
 * Reset and uninstall.
 *
 * ## How these blocks were made
 *
 * **Every transcript is a real run, including the two that destroy things.** They were
 * taken on 2026-09-19 with the binary built from this commit, against an
 * isolated machine assembled for them and torn down afterwards — never the
 * machine this session is running on, which is the owner's:
 *
 * ```text
 * USERPROFILE           a scratch home, so ~/.sloop was a scratch store
 * LOCALAPPDATA          a scratch tree, so uninstall deleted a scratch binary
 * SLOOP_TEST_PATH_KEY   a scratch HKCU subkey holding a Path value, so the PATH
 *                       entry that came out was a real registry edit to a key
 *                       made for it. It was deleted afterwards and HKCU\Environment
 *                       was never opened for writing.
 * ```
 *
 * The PostgreSQL was a throwaway 17.9 cluster on port 5452 with five databases on
 * it — `sloop_database`, `payments`, `analytics` and the two templates — and two
 * login roles besides the superuser. That is why the before-and-after block can
 * prove the claim rather than assert it: `payments`, `analytics` and `reporting`
 * are still there afterwards, and they are on the same server.
 *
 * What was captured:
 *
 * 1. **`reset` with no terminal** — the whole announcement, then exit `2` naming
 *    the flag.
 * 2. **`--confirm sloop_databse`**, a typo, refusing before anything is contacted.
 * 3. **`reset --confirm sloop_database` on a machine-owned server**, and the
 *    `psql` before and after.
 * 4. **`reset --confirm sloop_database` on a sloop-owned server** — see below.
 * 5. **`reset` again on the machine it had just cleaned**, which says there is
 *    nothing to do.
 * 6. **`uninstall --confirm sloop_database`**, binary, tree and PATH entry.
 * 7. **`uninstall.sh` and `uninstall.ps1`**, both refusing while state is present,
 *    and both with `--binary-only` / `-BinaryOnly`.
 *
 * ## The one assembled state, named rather than hidden
 *
 * **`origin = sloop` was set by hand.** Reaching it the real way means letting
 * sloop download and install a PostgreSQL 18, which is about a third of a
 * gigabyte and, on this machine, would have been the owner's own. So the record
 * was written with `origin = "sloop"` and the directory it names was created with
 * placeholder files in the real shape — `postgres/18/bin`, `postgres/data`. Reset
 * then ran against it for real: it surveyed that state, announced it, and deleted
 * the directory whole. Every character of that block is what the run printed. What
 * is standing in is the contents of the directory, not the behaviour.
 *
 * ## The substitutions
 *
 * **The scratch paths.** `~/.sloop` on the page is the global store, and the
 * install tree is `%LOCALAPPDATA%\Programs\sloop` and `~/.local/bin/sloop`. Both
 * are the real paths with the scratch prefix replaced, the same substitution the
 * backups page makes.
 *
 * **Long lines are wrapped where a terminal wraps them**, for the reason the
 * automation page gives: sloop prints one line and lets the emulator fold it, and
 * a capture redirected to a file holds lines of 190 characters. No word is
 * changed, added or removed.
 *
 * ## What is read rather than captured
 *
 * **The Linux and macOS spellings of the two `PATH` lines the CLI prints.** This
 * session is on Windows, so `block removed from …` and `file removed: …` come from
 * `Touched::said` in `cli/src/pathentry/mod.rs`. The `uninstall.sh` transcripts
 * beside them are real runs and show the same wording from the script's own side.
 */
@Component({
  selector: 'app-docs-reset',
  imports: [DocsPage, RouterLink, Terminal],
  templateUrl: './reset.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Reset {
  private readonly choice = inject(OsChoice);

  protected readonly os = this.choice.os;
  protected readonly osLabel = computed(() => OS_LABEL[this.os()]);

  private readonly slash = computed(() => (this.os() === 'windows' ? '\\' : '/'));

  /** `~/.sloop`, where the global store lives on this reader's platform. */
  protected readonly store = computed(() => {
    switch (this.os()) {
      case 'windows':
        return 'C:\\Users\\you\\.sloop';
      case 'macos':
        return '/Users/you/.sloop';
      default:
        return '/home/you/.sloop';
    }
  });

  private path(...parts: readonly string[]): string {
    return [this.store(), ...parts].join(this.slash());
  }

  /** What the installer put on this machine, and what `uninstall` takes off. */
  protected readonly installed = computed(() => {
    switch (this.os()) {
      case 'windows':
        return '%LOCALAPPDATA%\\Programs\\sloop';
      case 'macos':
        return '/Users/you/.local/bin/sloop';
      default:
        return '/home/you/.local/bin/sloop';
    }
  });

  /** The uninstall one-liner for this platform. */
  protected readonly script = computed(() =>
    this.os() === 'windows'
      ? 'irm https://dbsloop.github.io/uninstall.ps1 | iex'
      : 'curl -fsSL https://dbsloop.github.io/uninstall.sh | sh',
  );

  /** The same, told to leave sloop's own state alone. */
  protected readonly binaryOnly = computed(() =>
    this.os() === 'windows'
      ? '& ([scriptblock]::Create((irm https://dbsloop.github.io/uninstall.ps1))) -BinaryOnly'
      : 'curl -fsSL https://dbsloop.github.io/uninstall.sh | sh -s -- --binary-only',
  );

  // ── The two columns ───────────────────────────────────────────────────────

  protected readonly goes: readonly Item[] = [
    {
      what: 'sloop_database',
      detail: 'The registry, the schedules, the service’s readings and the vault. All of it.',
    },
    {
      what: 'sloop_db_admin',
      detail: 'The role that owned it, dropped in the same run.',
    },
    {
      what: 'server.toml',
      detail: 'The record that said which PostgreSQL was sloop’s, and on what port.',
    },
    {
      what: 'projects/ and locks/',
      detail: 'Where projects were registered, and any lockfile left behind by a run.',
    },
    {
      what: 'The PostgreSQL — sometimes',
      detail: 'Only if sloop installed it. The next section is the whole of that question.',
    },
    {
      what: 'The background service',
      detail:
        'Taken off the machine before anything else goes, because it wakes on a timer and opens the store every round.',
    },
    {
      what: 'Every server sloop installed',
      detail:
        'Each one stopped first, then its directory — a running server holds its own files open, on Windows especially.',
    },
    {
      what: 'Every password sloop filed',
      detail:
        'Its own superuser and role, the servers it installed, your registered databases and the SSH passphrases beside them — out of the keyring, out of the encrypted file.',
    },
  ];

  protected readonly stays: readonly Item[] = [
    {
      what: 'Every backup',
      detail:
        'Not by reset, not by uninstall, not ever. They are the one thing here that cannot be made again.',
    },
    {
      what: 'Every other database',
      detail: 'On a PostgreSQL sloop did not install, only sloop’s own two objects go.',
    },
    {
      what: 'Every other role',
      detail: 'Two statements run, and both name something sloop created.',
    },
    {
      what: 'Your databases themselves',
      detail: 'Registering one never touched it, and forgetting all of them does not either.',
    },
    {
      what: 'Passwords sloop never held',
      detail:
        'A ${VAR} route or a --password-command points at a secret somebody else keeps. There is no copy here to remove, and it says so rather than pretending.',
    },
  ];

  // ── The announcement ──────────────────────────────────────────────────────

  /** Said before anything happens, and before the name is typed. */
  protected readonly announcement = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop reset' },
    { kind: 'head', tag: 'This will remove:', text: '' },
    {
      kind: 'fact',
      tag: '  Database',
      text: ' sloop_database and its role, on the PostgreSQL already at port 5452',
    },
    {
      kind: 'dim',
      text: '  that server was here before sloop and stays exactly as it is — no other',
    },
    { kind: 'dim', text: '  database on it is touched' },
    { kind: 'label', tag: '  Gone', text: ` ${this.path('server.toml')}` },
    { kind: 'label', tag: '  Gone', text: ` ${this.path('locks')}` },
    { kind: 'fact', tag: '  Password', text: " postgres, the superuser of sloop's PostgreSQL" },
    {
      kind: 'fact',
      tag: '  Password',
      text: " sloop_db_admin, which owns sloop's own database",
    },
    { kind: 'fact', tag: '  Password', text: ' the password for shop' },
    {
      kind: 'dim',
      text: '  these are forgotten wherever this machine keeps them, so the next `sloop setup`',
    },
    { kind: 'dim', text: '  starts with nothing left over' },
    { text: ' ' },
    { kind: 'head', tag: 'This will keep:', text: '' },
    { kind: 'fact', tag: '  Backups', text: ` ${this.path('backups')}` },
    {
      kind: 'dim',
      text: '  a backup is the one thing here that cannot be made again, so reset never',
    },
    { kind: 'dim', text: '  deletes one' },
    {
      kind: 'dim',
      text: '  an encrypted backup still needs its key. If the private key was never exported',
    },
    {
      kind: 'dim',
      text: '  and this machine keeps secrets in the encrypted file, it goes with the database',
    },
    { kind: 'dim', text: '  and those backups become unreadable.' },
    { text: ' ' },
    {
      kind: 'bad',
      tag: 'error:',
      text: ' resetting sloop needs its name typed, and there is no terminal to type at',
    },
    { kind: 'dim', text: '  hint: pass --confirm sloop_database to say it up front' },
  ]);

  /**
   * Asked before the list, before the question, before anything.
   *
   * **The owner's instruction, in their words:** *"even for reset and uninstall
   * if it needs elevated administrator rights, then ask it first without
   * proceeding"*. Taking a service off the machine is the one thing here that
   * needs more than an ordinary account, so a run that cannot do it says so
   * while the machine is still whole — rather than deleting the cluster and
   * then discovering it cannot finish.
   *
   * The wording is `service::elevation::require` and `advice`, which is the
   * same refusal `service install` gives, one command earlier.
   */
  protected readonly notElevated = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop reset' },
    {
      kind: 'bad',
      tag: 'error:',
      text: ' changing what this machine runs at boot needs more than this account has, and',
    },
    { kind: 'bad', tag: '', text: `  ${this.manager()} will refuse` },
    {
      kind: 'dim',
      text:
        this.os() === 'windows'
          ? "  hint: open a terminal with 'Run as administrator' and run it again. Nothing has"
          : '  hint: run it again with sudo. Nothing has been changed by this run.',
    },
    ...(this.os() === 'windows'
      ? [{ kind: 'dim' as const, text: '  been changed by this run.' }]
      : []),
  ]);

  /** What this platform's service manager is called, in sloop's own words. */
  protected readonly manager = computed(() => {
    switch (this.os()) {
      case 'windows':
        return 'the Service Control Manager';
      case 'macos':
        return 'launchd';
      default:
        return 'systemd';
    }
  });

  /** How this reader gets an elevated terminal. */
  protected readonly elevate = computed(() =>
    this.os() === 'windows' ? "a terminal opened with 'Run as administrator'" : 'sudo',
  );

  /** A typo, caught before a socket is opened. */
  protected readonly typo: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop reset --confirm sloop_databse' },
    { kind: 'dim', text: '# …the same announcement, and then:' },
    {
      kind: 'bad',
      tag: 'error:',
      text: ' --confirm says sloop_databse, and the database sloop keeps its state in is',
    },
    { kind: 'bad', tag: '', text: '  sloop_database' },
    {
      kind: 'dim',
      text: '  hint: nothing was contacted and nothing was changed. The two have to match',
    },
    { kind: 'dim', text: '  exactly' },
  ];

  // ── Who owns the PostgreSQL ───────────────────────────────────────────────

  /** `origin = machine`: sloop was a guest, and leaves as one. */
  protected readonly guest = computed<readonly TerminalLine[]>(() => [
    { kind: 'head', tag: 'This will remove:', text: '' },
    {
      kind: 'fact',
      tag: '  Database',
      text: ' sloop_database and its role, on the PostgreSQL already at port 5452',
    },
    {
      kind: 'dim',
      text: '  that server was here before sloop and stays exactly as it is — no other',
    },
    { kind: 'dim', text: '  database on it is touched' },
    { kind: 'label', tag: '  Gone', text: ` ${this.path('server.toml')}` },
    { kind: 'label', tag: '  Gone', text: ` ${this.path('locks')}` },
  ]);

  /** `origin = sloop`: sloop brought it, so sloop takes it away. */
  protected readonly owner = computed<readonly TerminalLine[]>(() => [
    { kind: 'head', tag: 'This will remove:', text: '' },
    {
      kind: 'fact',
      tag: '  PostgreSQL',
      text: ' the whole server sloop installed, on port 5433',
    },
    {
      kind: 'dim',
      text: '  sloop installed it, so sloop takes it away. `sloop setup` downloads and',
    },
    { kind: 'dim', text: '  installs it again.' },
    { kind: 'label', tag: '  Gone', text: ` ${this.path('server.toml')}` },
    { kind: 'label', tag: '  Gone', text: ` ${this.path('projects')}` },
    { kind: 'label', tag: '  Gone', text: ` ${this.path('locks')}` },
    { kind: 'label', tag: '  Gone', text: ` ${this.path('postgres')}` },
  ]);

  /** The claim, tested: one server, five databases, and what was left after. */
  protected readonly survivors: readonly TerminalLine[] = [
    { kind: 'dim', text: '# before — the server sloop was a guest on' },
    { kind: 'prompt', text: 'psql -p 5452 -U postgres -c "select datname from pg_database"' },
    { kind: 'dim', text: ' analytics' },
    { kind: 'dim', text: ' payments' },
    { kind: 'dim', text: ' postgres' },
    { kind: 'name', tag: ' sloop_database', text: '' },
    { kind: 'dim', text: ' template0' },
    { kind: 'dim', text: ' template1' },
    { text: ' ' },
    { kind: 'prompt', text: 'sloop reset --confirm sloop_database' },
    { kind: 'label', tag: '  Dropped', text: ' sloop_database and sloop_db_admin' },
    { text: ' ' },
    { kind: 'dim', text: '# after' },
    { kind: 'prompt', text: 'psql -p 5452 -U postgres -c "select datname from pg_database"' },
    { kind: 'ok', tag: ' analytics', text: '' },
    { kind: 'ok', tag: ' payments', text: '' },
    { kind: 'dim', text: ' postgres' },
    { kind: 'dim', text: ' template0' },
    { kind: 'dim', text: ' template1' },
    { text: ' ' },
    { kind: 'prompt', text: 'psql -p 5452 -U postgres -c "select rolname from pg_roles ..."' },
    { kind: 'dim', text: ' postgres' },
    { kind: 'ok', tag: ' reporting', text: '   ← still there, and it never was sloop’s' },
  ];

  /** What was left in the store afterwards, which is one directory. */
  protected readonly afterwards = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: `ls ${this.store()}` },
    { kind: 'ok', tag: 'backups', text: '   ← and nothing else' },
    { text: ' ' },
    { kind: 'prompt', text: 'sloop reset' },
    {
      kind: 'dim',
      text: 'there is nothing to reset — this machine has no sloop state on it',
    },
  ]);

  // ── uninstall ─────────────────────────────────────────────────────────────

  /**
   * The same run, plus the binary.
   *
   * The two `PATH` lines are the Windows capture; the POSIX spellings are
   * `Touched::said`'s, named in the class comment.
   */
  protected readonly uninstalling = computed<readonly TerminalLine[]>(() => {
    const tail: TerminalLine[] =
      this.os() === 'windows'
        ? [
            { kind: 'label', tag: '  Removed', text: ' %LOCALAPPDATA%\\Programs\\sloop' },
            {
              kind: 'label',
              tag: '  PATH',
              text: ' entry removed: %LOCALAPPDATA%\\Programs\\sloop\\bin',
            },
          ]
        : [
            { kind: 'label', tag: '  Removed', text: ` ${this.installed()}` },
            {
              kind: 'label',
              tag: '  PATH',
              text: ` block removed from ${this.os() === 'macos' ? '/Users/you/.zshrc' : '/home/you/.bashrc'}`,
            },
          ];

    return [
      { kind: 'prompt', text: 'sloop uninstall --confirm sloop_database' },
      { kind: 'dim', text: '# …the same announcement, and the same two columns, and then:' },
      { kind: 'label', tag: '  Dropped', text: ' sloop_database and sloop_db_admin' },
      { kind: 'label', tag: '  Removed', text: ` ${this.path('server.toml')}` },
      ...tail,
      {
        kind: 'dim',
        text: '  open shells still have the old PATH; the next one will not',
      },
      { text: ' ' },
      { kind: 'head', tag: 'Done.', text: ' sloop is off this machine.' },
    ];
  });

  /** The one-liner on its own, so the block has something to copy. */
  protected readonly scriptLine = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: this.script() },
  ]);

  // ── The scripts ───────────────────────────────────────────────────────────

  /** The script refuses to strand state behind a binary that could remove it. */
  protected readonly scriptRefusal = computed<readonly TerminalLine[]>(() => {
    // `--` rather than an em dash inside the quoted block: both scripts are
    // written in ASCII so that they print the same in every console, and this
    // is their output rather than the site's prose.
    return [
      { kind: 'prompt', text: this.script() },
      { text: ' ' },
      { kind: 'bad', tag: '  sloop uninstall stopped.', text: '' },
      { text: ' ' },
      {
        kind: 'out',
        text: `  sloop still has state on this machine, and ${this.binaryPath()} can`,
      },
      { kind: 'out', text: '  still remove it:' },
      { text: ' ' },
      { kind: 'name', tag: `    ${this.store()}`, text: '' },
      { text: ' ' },
      {
        kind: 'out',
        text: '    Run this first -- it removes the PostgreSQL sloop installed, its database,',
      },
      { kind: 'out', text: '    its role and its registry, then the binary and the PATH entry:' },
      { text: ' ' },
      { kind: 'name', tag: `        ${this.binaryPath()} uninstall`, text: '' },
      { text: ' ' },
      { kind: 'out', text: '    Or leave that state where it is and take only the binary:' },
      { text: ' ' },
      { kind: 'name', tag: `        ${this.binaryOnly()}`, text: '' },
      { text: ' ' },
      { kind: 'ok', tag: '    Backups are not deleted by either.', text: '' },
    ];
  });

  /** And what it does once it has been told to. */
  protected readonly binaryOnlyRun = computed<readonly TerminalLine[]>(() =>
    this.os() === 'windows'
      ? [
          { kind: 'prompt', text: this.binaryOnly() },
          {
            kind: 'label',
            tag: '  removed',
            text: ' %LOCALAPPDATA%\\Programs\\sloop\\bin\\sloop.exe',
          },
          { kind: 'label', tag: '  removed', text: ' %LOCALAPPDATA%\\Programs\\sloop' },
          {
            kind: 'label',
            tag: '  removed',
            text: ' PATH entry: %LOCALAPPDATA%\\Programs\\sloop\\bin',
          },
          { text: ' ' },
          { kind: 'head', tag: 'sloop is off this machine.', text: '' },
          { kind: 'dim', text: 'Open shells still have the old PATH; the next one will not.' },
        ]
      : [
          { kind: 'prompt', text: this.binaryOnly() },
          { kind: 'label', tag: '  removed', text: ` ${this.binaryPath()}` },
          {
            kind: 'label',
            tag: '  removed',
            text: ` sloop's PATH block from ${this.os() === 'macos' ? '/Users/you/.zshrc' : '/home/you/.bashrc'}`,
          },
          { text: ' ' },
          { kind: 'head', tag: 'sloop is off this machine.', text: '' },
          { kind: 'dim', text: 'Open shells still have the old PATH; the next one will not.' },
        ],
  );

  /** The binary itself, as the script names it back at you. */
  private binaryPath(): string {
    return this.os() === 'windows'
      ? '%LOCALAPPDATA%\\Programs\\sloop\\bin\\sloop.exe'
      : this.installed();
  }
}
