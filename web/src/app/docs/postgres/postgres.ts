import { ChangeDetectionStrategy, Component, computed, inject } from '@angular/core';
import { RouterLink } from '@angular/router';

import { DocsPage } from '../docs-page';
import { OS_LABEL, OsChoice } from '../os';
import { Terminal, type TerminalLine } from '../../ui/terminal';
import { RELEASE } from '../../version';

/** One of the two things `server.toml` can say about whose server it is. */
interface Whose {
  readonly value: string;
  readonly means: string;
  readonly reset: string;
}

/** One table in sloop's own database, and what it holds. */
interface Held {
  readonly table: string;
  readonly holds: string;
}

/**
 * sloop's own PostgreSQL.
 *
 * ## How these blocks were made
 *
 * All four are real runs on 2026-09-19, with the binary built from this commit,
 * against the PostgreSQL 18 sloop installed for itself on this machine:
 * `server connection`, `server list`, `setup` on a machine that is already set
 * up, and `reset --dry-run`.
 *
 * The table list is read from `sloop_database` itself —
 * `information_schema.tables` — rather than from the migration source, so it is
 * what is actually there at schema version 9.
 *
 * ## The substitution
 *
 * One: **the home directory**, `C:\Users\Saad\` written as `C:\Users\you\`, with
 * the POSIX spellings behind the OS strip. Ports, versions, role names, database
 * names and every word of every message are as printed.
 *
 * ## What is described rather than captured
 *
 * **`server install`**, which downloads and installs a database server — hundreds
 * of megabytes, and it would leave one on the machine this session runs on. Its
 * one flag is read from the generated command reference, and
 * [getting started](/docs/getting-started) has the captured first-run `setup`
 * offer, cut at the question, for what an acquisition actually looks like.
 */
@Component({
  selector: 'app-docs-postgres',
  imports: [DocsPage, RouterLink, Terminal],
  templateUrl: './postgres.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Postgres {
  private readonly choice = inject(OsChoice);

  protected readonly os = this.choice.os;
  protected readonly osLabel = computed(() => OS_LABEL[this.os()]);

  private readonly slash = computed(() => (this.os() === 'windows' ? '\\' : '/'));

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

  /** Where an installer would have put it instead, on this platform. */
  protected readonly systemPlace = computed(() =>
    this.os() === 'windows' ? 'Program Files' : '/usr/local',
  );

  protected readonly elevation = computed(() =>
    this.os() === 'windows' ? 'an administrator' : 'sudo',
  );

  private path(...parts: readonly string[]): string {
    return [this.store(), ...parts].join(this.slash());
  }

  /** What is actually in `sloop_database`, read from the database. */
  protected readonly tables: readonly Held[] = [
    {
      table: 'registered_database',
      holds: 'Every database you have registered — and its password route, never a password.',
    },
    {
      table: 'project',
      holds: 'The project registries, by directory, so -C acme-api can find one by name.',
    },
    { table: 'engine', holds: 'Which engine each registration speaks.' },
    {
      table: 'backup_key',
      holds:
        'The public half of the age keypair, per registry. The private half is in the keyring.',
    },
    {
      table: 'backup_run',
      holds: 'Every backup taken: when, how big, how long, and whether it verified.',
    },
    { table: 'restore_run', holds: 'The same for every restore.' },
    {
      table: 'monitored_database',
      holds: 'What the background service is attached to, and what it is scheduled to do.',
    },
    {
      table: 'service',
      holds: 'The service itself — when it last read the list, and when it last ran.',
    },
    { table: 'activity_hour', holds: 'Rows in and rows out per database per UTC day.' },
    {
      table: 'bandwidth_day',
      holds: 'Bytes sloop itself moved. Not bytes on the wire — no engine reports those.',
    },
    {
      table: 'sealed_vault',
      holds: 'The encrypted password file, for a machine with no keyring running.',
    },
    {
      table: 'schema_migration',
      holds: `Which version of its own schema sloop has applied. Nine, at ${RELEASE}.`,
    },
  ];

  /** What `origin` can be, and what each costs at `reset`. */
  protected readonly origins: readonly Whose[] = [
    {
      value: 'sloop',
      means: 'sloop created the cluster, under its own store, on a port it chose.',
      reset:
        "Removes it. It was sloop's to make, so it is sloop's to take away — and sloop setup builds it again.",
    },
    {
      value: 'machine',
      means: 'A PostgreSQL 18 that was already here. sloop is a guest on it.',
      reset:
        "Leaves it alone entirely. sloop never starts, stops or reconfigures somebody else's server.",
    },
  ];

  // ── Opening it ────────────────────────────────────────────────────────────

  protected readonly connection: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop server connection' },
    { text: ' ' },
    { kind: 'head', tag: "sloop's own database answering", text: '' },
    { kind: 'label', tag: '  Host      ', text: '127.0.0.1' },
    { kind: 'label', tag: '  Port      ', text: '5433' },
    { kind: 'label', tag: '  Database  ', text: 'sloop_database' },
    { kind: 'label', tag: '  User      ', text: 'sloop_db_admin' },
    { kind: 'label', tag: '  Password  ', text: 'kept in the OS keyring' },
    {
      kind: 'label',
      tag: '  URL       ',
      text: 'postgres://sloop_db_admin@127.0.0.1:5433/sloop_database',
    },
    { text: ' ' },
    {
      kind: 'dim',
      text: '  Open it with psql, DataGrip, or anything else that speaks PostgreSQL.',
    },
    {
      kind: 'dim',
      text: '  `sloop server connection --show-password` prints the password on this terminal.',
    },
  ];

  // ── setup ─────────────────────────────────────────────────────────────────

  /** What `setup` prints on a machine that is already set up. */
  protected readonly setupAgain = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop setup' },
    { kind: 'name', tag: '  Already there', text: ' sloop_database' },
    { kind: 'head', tag: 'Using:', text: " sloop's own cluster" },
    {
      kind: 'dim',
      text: `  postgres://postgres@127.0.0.1:5433/postgres  binaries at ${this.path('postgres', '18', 'bin')}`,
    },
    { kind: 'dim', text: `  cluster at ${this.path('postgres', 'data')}` },
    {
      kind: 'head',
      tag: 'State:',
      text: ' postgres://sloop_db_admin@127.0.0.1:5433/sloop_database',
    },
    { kind: 'dim', text: '  owned by sloop_db_admin, whose password is kept in the OS keyring' },
    { kind: 'dim', text: '  Schema already at version 9 in sloop_database' },
  ]);

  protected readonly serverList: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop server list' },
    {
      kind: 'head',
      tag: 'None:',
      text: ' sloop has not installed a database server on this machine.',
    },
    {
      kind: 'dim',
      text: '  `sloop server install` walks through engine, version and one confirmation.',
    },
  ];

  // ── reset ─────────────────────────────────────────────────────────────────

  /** `origin` deciding what goes, named file by file. */
  protected readonly reset = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop reset --dry-run' },
    { kind: 'head', tag: 'This will remove:', text: '' },
    {
      kind: 'fact',
      tag: '  PostgreSQL',
      text: ' the whole server sloop installed, on port 5433',
    },
    {
      kind: 'dim',
      text: '  sloop installed it, so sloop takes it away. `sloop setup` downloads and installs it again.',
    },
    { kind: 'label', tag: '  Gone', text: ` ${this.path('server.toml')}` },
    { kind: 'label', tag: '  Gone', text: ` ${this.path('registry.toml')}` },
    { kind: 'label', tag: '  Gone', text: ` ${this.path('projects')}` },
    { kind: 'label', tag: '  Gone', text: ` ${this.path('locks')}` },
    { kind: 'label', tag: '  Gone', text: ` ${this.path('postgres')}` },
    { text: ' ' },
    { kind: 'head', tag: 'This will keep:', text: '' },
    { kind: 'fact', tag: '  Backups', text: ` ${this.path('backups')}` },
    {
      kind: 'dim',
      text: '  a backup is the one thing here that cannot be made again, so reset never deletes one',
    },
    {
      kind: 'dim',
      text: '  an encrypted backup still needs its key. If the private key was never exported and',
    },
    {
      kind: 'dim',
      text: '  this machine keeps secrets in the encrypted file, it goes with the database and those',
    },
    { kind: 'dim', text: '  backups become unreadable.' },
    { text: ' ' },
    {
      kind: 'bad',
      tag: 'error:',
      text: ' resetting sloop needs its name typed, and there is no terminal to type at',
    },
    { kind: 'dim', text: '  hint: pass --confirm sloop_database to say it up front' },
  ]);
}
