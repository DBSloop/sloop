import { ChangeDetectionStrategy, Component, computed, inject } from '@angular/core';
import { RouterLink } from '@angular/router';

import { DocsPage } from '../docs-page';
import { OS_LABEL, OsChoice } from '../os';
import { Terminal, type TerminalLine } from '../../ui/terminal';
import { RELEASE } from '../../version';

/** One exit code, for the pair of cards that tell `0` and `6` apart. */
interface Ending {
  readonly code: string;
  readonly title: string;
  readonly means: string;
  readonly then: string;
}

/**
 * Backups, retention and restoring.
 *
 * ## How these blocks were made
 *
 * A throwaway PostgreSQL 17.9 cluster on port 5443, four databases registered in
 * a project registry of its own, and every command below run against it on
 * 2026-09-19 with the binary built from this commit. Torn down afterwards.
 *
 * **The awkward ones were produced rather than described**, because they are the
 * ones a reader will not believe on assertion:
 *
 * - **`--all` continuing past a failure** — a database called `gone` was
 *   registered against a database that does not exist on the server. The run
 *   backed up the other three, named the one that failed, and exited `3`.
 * - **A damaged dump** — one byte of an `archive` dump was overwritten, and
 *   `backups list --check` was run against it. It names both hashes and the
 *   command exits `6`.
 * - **Exit `7`** — two backups of a 2,000,000-row table were started at the same
 *   moment. The block is what the one that lost printed, naming the winner's
 *   process id and the lock file.
 * - **`SLOOP_VERIFY=fast`** — a real restore under the variable, so the page can
 *   show the sentence sloop actually prints about an estimate.
 *
 * ## The substitutions, named rather than hidden
 *
 * **The project's path.** The registry was built in a scratch directory to be
 * captured; on the page it is `C:\Users\you\projects\acme-api` and its POSIX
 * equivalents, the same substitution `A9` makes. Every path under it — the
 * backup directories, the lock file — is the real one with that prefix replaced.
 *
 * **The `age` keys are replaced** with keys of the same shape and length, for
 * `A7`'s reason: publishing a real private key hands over every backup it
 * guards. The public key in the `key export` block is the only other one on the
 * page and it is replaced to match.
 *
 * Nothing else is edited. Sizes, durations, hashes, row counts and timestamps
 * are what the run printed.
 *
 * ## What is not here
 *
 * The flag tables, which are generated from `--help` on the
 * [command reference](/docs/commands). And the key itself, which is
 * [its own page](/docs/backup-key) — this one only shows the refusal that sends
 * you there.
 */
@Component({
  selector: 'app-docs-backups',
  imports: [DocsPage, RouterLink, Terminal],
  templateUrl: './backups.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Backups {
  private readonly choice = inject(OsChoice);

  protected readonly os = this.choice.os;
  protected readonly osLabel = computed(() => OS_LABEL[this.os()]);

  private readonly slash = computed(() => (this.os() === 'windows' ? '\\' : '/'));

  /** The project whose registry these backups belong to. */
  protected readonly project = computed(() => {
    switch (this.os()) {
      case 'windows':
        return 'C:\\Users\\you\\projects\\acme-api';
      case 'macos':
        return '/Users/you/projects/acme-api';
      default:
        return '/home/you/projects/acme-api';
    }
  });

  /** `~/.sloop`, where a global registry's backups go instead. */
  protected readonly globalStore = computed(() => {
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
    return [this.project(), '.sloop', ...parts].join(this.slash());
  }

  protected readonly backupRoot = computed(() => this.path('backups'));
  protected readonly lockFile = computed(() => this.path('locks', 'archive.lock'));

  // ── The key comes first ───────────────────────────────────────────────────

  /** The first encrypted backup will not run until the key has been copied out. */
  protected readonly refusal: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop backup shop' },
    { text: ' ' },
    { kind: 'head', tag: 'this registry has a new backup key', text: '' },
    {
      kind: 'name',
      tag: '  age1q7v3mx0kz9rw2ld5htn8jc4pf6ys0ueg3a2xk9msv4rzq3ltdn6shw8cjp',
      text: '',
    },
    {
      kind: 'dim',
      text: '  backups from here on are encrypted to it. The private half is on this machine and',
    },
    {
      kind: 'dim',
      text: '  nowhere else, so if this machine is lost, every one of those backups is unreadable —',
    },
    { kind: 'dim', text: '  there is no recovery, no reset and nobody to ask.' },
    {
      kind: 'dim',
      text: '  `sloop key export > backup-key.txt` writes it out. Keep that somewhere else.',
    },
    {
      kind: 'bad',
      tag: 'error:',
      text: ' the backup key has not been copied anywhere yet, and there is no terminal to ask at',
    },
    { kind: 'dim', text: '  hint: run `sloop key export` once, then this run will go through' },
  ];

  // ── Taking one ────────────────────────────────────────────────────────────

  protected readonly first = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop backup shop' },
    { kind: 'name', tag: 'shop', text: '', note: '  postgres://app@127.0.0.1:5443/shop' },
    { kind: 'step', mark: 'ok', text: 'Connected', note: 'postgres 17.9, 2 tables, 10601 rows' },
    { kind: 'step', mark: 'ok', text: 'Dumped 2 tables', note: '67.7 kB, encrypted' },
    { kind: 'step', mark: 'ok', text: 'Checked', note: 'e68d1629c48f' },
    {
      kind: 'dim',
      text: `  backed up to ${this.path('backups', 'postgres', 'shop', '20260919T140939Z')}`,
    },
    { kind: 'dim', text: '  66.1 KiB in 0.4s, sha256 e68d1629c48f' },
    { kind: 'dim', text: '  taken 2026-09-19 20:09:39 +06:00 (2026-09-19T14:09:39Z)' },
  ]);

  /** Four databases, one of them pointing at a database that is not there. */
  protected readonly all = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop backup --all' },
    { kind: 'name', tag: 'archive', text: '', note: '  postgres://app@127.0.0.1:5443/archive' },
    { kind: 'step', mark: 'ok', text: 'Connected', note: 'postgres 17.9, 0 tables, 0 rows' },
    { kind: 'step', mark: 'ok', text: 'Dumped 0 tables', note: '1.1 kB, encrypted' },
    { kind: 'step', mark: 'ok', text: 'Checked', note: '77d875ac2fcb' },
    {
      kind: 'dim',
      text: `  backed up to ${this.path('backups', 'postgres', 'archive', '20260919T140947Z')}`,
    },
    { kind: 'dim', text: '  1.1 KiB in 0.3s, sha256 77d875ac2fcb' },
    { text: ' ' },
    { kind: 'name', tag: 'gone', text: '', note: '  postgres://app@127.0.0.1:5443/gone' },
    {
      kind: 'step',
      mark: 'bad',
      text: 'gone: psql.exe failed against postgres://app@127.0.0.1:5443/gone: psql: error:',
    },
    {
      text: '     connection to server at "127.0.0.1", port 5443 failed: FATAL:  database "gone" does not exist',
    },
    {
      kind: 'dim',
      text: '  hint: check the host, port, user and password sloop was given: `sloop db list` shows',
    },
    { kind: 'dim', text: '  them and `sloop db test <name>` tries them' },
    { text: ' ' },
    { kind: 'name', tag: 'ledger', text: '', note: '  postgres://app@127.0.0.1:5443/ledger' },
    { kind: 'step', mark: 'ok', text: 'Connected', note: 'postgres 17.9, 1 table, 4210 rows' },
    { kind: 'step', mark: 'ok', text: 'Dumped 1 table', note: '23.2 kB, encrypted' },
    { kind: 'step', mark: 'ok', text: 'Checked', note: 'b9726982f168' },
    { kind: 'dim', text: '  22.7 KiB in 0.2s, sha256 b9726982f168' },
    { text: ' ' },
    { kind: 'name', tag: 'shop', text: '', note: '  postgres://app@127.0.0.1:5443/shop' },
    { kind: 'step', mark: 'ok', text: 'Connected', note: 'postgres 17.9, 2 tables, 10601 rows' },
    { kind: 'step', mark: 'ok', text: 'Dumped 2 tables', note: '67.6 kB, encrypted' },
    { kind: 'step', mark: 'ok', text: 'Checked', note: '2698a4bdaec9' },
    { kind: 'dim', text: '  66.0 KiB in 0.2s, sha256 2698a4bdaec9' },
    { text: ' ' },
    { kind: 'name', tag: 'backed up 3 of 4', text: '', note: ' — 1 failed: gone' },
  ]);

  /** `--replace` keeps one copy at a fixed path. */
  protected readonly replace = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop backup ledger --replace' },
    { kind: 'name', tag: 'ledger', text: '', note: '  postgres://app@127.0.0.1:5443/ledger' },
    { kind: 'step', mark: 'ok', text: 'Connected', note: 'postgres 17.9, 1 table, 4210 rows' },
    { kind: 'step', mark: 'ok', text: 'Dumped 1 table', note: '23.2 kB, encrypted' },
    { kind: 'step', mark: 'ok', text: 'Checked', note: '07daf4371ce1' },
    {
      kind: 'dim',
      text: `  backed up to ${this.path('backups', 'postgres', 'ledger', 'latest')}`,
    },
    { kind: 'dim', text: '  22.7 KiB in 0.2s, sha256 07daf4371ce1' },
    { kind: 'dim', text: '  taken 2026-09-19 20:10:06 +06:00 (2026-09-19T14:10:06Z)' },
  ]);

  // ── On disk ───────────────────────────────────────────────────────────────

  protected readonly layout = computed<readonly TerminalLine[]>(() => {
    const s = this.slash();
    return [
      { kind: 'prompt', text: `cd ${this.path('backups')}` },
      { kind: 'prompt', text: 'find . -type f' },
      { text: `.${s}postgres${s}archive${s}20260919T140947Z${s}dump.age` },
      { text: `.${s}postgres${s}archive${s}20260919T140947Z${s}manifest.json` },
      { text: `.${s}postgres${s}ledger${s}20260919T140948Z${s}dump.age` },
      { text: `.${s}postgres${s}ledger${s}20260919T140948Z${s}manifest.json` },
      { text: `.${s}postgres${s}shop${s}20260919T140939Z${s}dump.age` },
      { text: `.${s}postgres${s}shop${s}20260919T140939Z${s}manifest.json` },
      { text: `.${s}postgres${s}shop${s}20260919T140948Z${s}dump.age` },
      { text: `.${s}postgres${s}shop${s}20260919T140948Z${s}manifest.json` },
    ];
  });

  /** The manifest beside that first `shop` dump, as it was written. */
  protected readonly manifest: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'cat manifest.json' },
    { text: '{' },
    { kind: 'dim', text: '  "version": 1,' },
    { kind: 'dim', text: `  "sloop": "${RELEASE}",` },
    { kind: 'dim', text: '  "label": "shop",' },
    { kind: 'dim', text: '  "engine": "postgres",' },
    { kind: 'dim', text: '  "source": "postgres://app@127.0.0.1:5443/shop",' },
    { kind: 'dim', text: '  "database": "shop",' },
    { kind: 'dim', text: '  "server": { "version": "17.9", "tls": false },' },
    { kind: 'dim', text: '  "taken": {' },
    { kind: 'dim', text: '    "utc": "2026-09-19T14:09:39Z",' },
    { kind: 'dim', text: '    "local": "2026-09-19T20:09:39+06:00",' },
    { kind: 'dim', text: '    "offset": "+06:00", "offset_seconds": 21600,' },
    { kind: 'dim', text: '    "unix": 1789826979' },
    { kind: 'dim', text: '  },' },
    { kind: 'dim', text: '  "took_seconds": 0.534,' },
    { kind: 'dim', text: '  "dump_seconds": 0.402,' },
    { kind: 'dim', text: '  "dump": {' },
    { kind: 'dim', text: '    "file": "dump.age",' },
    { kind: 'dim', text: '    "bytes": 67650,' },
    { kind: 'dim', text: '    "sha256": "e68d1629c48fce316f2e0cff63e81ceb77d268390265...",' },
    { kind: 'dim', text: '    "encryption": { "format": "age", "recipient": "age1q7v3mx0..." }' },
    { kind: 'dim', text: '  },' },
    { kind: 'dim', text: '  "rows": 10601,' },
    { kind: 'dim', text: '  "tables": [' },
    { kind: 'dim', text: '    { "schema": "public", "name": "customers", "rows": 1284 },' },
    { kind: 'dim', text: '    { "schema": "public", "name": "orders",    "rows": 9317 }' },
    { kind: 'dim', text: '  ]' },
    { text: '}' },
  ];

  // ── Seeing what is there ──────────────────────────────────────────────────

  protected readonly list: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop backups list' },
    { kind: 'dim', text: 'project' },
    { kind: 'name', tag: 'ledger', text: '', note: '  postgres · 2 backups, 45.4 KiB' },
    {
      kind: 'dim',
      text: '  2026-09-19 20:10:06 +06:00  22.7 KiB, encrypted · 4210 rows · replace',
    },
    { kind: 'dim', text: '  2026-09-19 20:09:48 +06:00  22.7 KiB, encrypted · 4210 rows' },
    { kind: 'name', tag: 'shop', text: '', note: '  postgres · 2 backups, 132.0 KiB' },
    { kind: 'dim', text: '  2026-09-19 20:10:22 +06:00  66.0 KiB, encrypted · 10601 rows' },
    { kind: 'dim', text: '  2026-09-19 20:10:20 +06:00  66.0 KiB, encrypted · 10601 rows' },
    { text: ' ' },
    { kind: 'dim', text: '4 backups, 177.4 KiB · sizes checked, --check hashes them' },
  ];

  /** One byte of a dump was overwritten. `--check` is what notices. */
  protected readonly check: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop backups list archive --check' },
    { kind: 'dim', text: 'project' },
    {
      kind: 'name',
      tag: 'archive',
      text: '',
      note: '  postgres · 4 backups, 25.3 MiB · 1 damaged',
    },
    { kind: 'dim', text: '  2026-09-19 20:12:01 +06:00  6.3 MiB, encrypted · 2000000 rows' },
    { kind: 'dim', text: '  2026-09-19 20:11:47 +06:00  6.3 MiB, encrypted · 2000000 rows' },
    { kind: 'dim', text: '  2026-09-19 20:11:28 +06:00  6.3 MiB, encrypted · 2000000 rows' },
    {
      kind: 'bad',
      tag: '  2026-09-19 20:09:47 +06:00',
      text: '  dump.age hashes to a25f8c3cb5c6 and the manifest says 77d875ac2fcb',
    },
    { text: ' ' },
    { kind: 'dim', text: '4 backups, 25.3 MiB · 1 damaged · every dump hashed' },
    { text: ' ' },
    { kind: 'prompt', text: 'echo exit $?' },
    { text: 'exit 6' },
  ];

  // ── Retention ─────────────────────────────────────────────────────────────

  protected readonly prune: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop backups prune shop --keep 2 --dry-run' },
    {
      kind: 'name',
      tag: '  remove',
      text: ' 2026-09-19 20:10:18 +06:00  66.0 KiB, encrypted · 10601 rows  20260919T141018Z',
    },
    {
      kind: 'name',
      tag: '  remove',
      text: ' 2026-09-19 20:09:48 +06:00  66.0 KiB, encrypted · 10601 rows  20260919T140948Z',
    },
    {
      kind: 'name',
      tag: '  remove',
      text: ' 2026-09-19 20:09:39 +06:00  66.1 KiB, encrypted · 10601 rows  20260919T140939Z',
    },
    { kind: 'dim', text: '  2 backups kept by the policy' },
    { text: ' ' },
    { kind: 'dim', text: '3 backups would go, freeing 198.1 KiB' },
    { kind: 'dim', text: '--dry-run: nothing was removed.' },
    { text: ' ' },
    { kind: 'prompt', text: 'sloop backups prune shop --keep 2 --yes' },
    { kind: 'dim', text: '  … the same three, named again' },
    { kind: 'name', tag: 'removed 3 backups', text: ', freed 198.1 KiB' },
  ];

  /** `--replace` copies are skipped by retention, and it says so. */
  protected readonly pruneAge: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop backups prune --older-than 30d --yes' },
    {
      kind: 'dim',
      text: '    keep 2026-09-19 20:10:06 +06:00  replace-mode: one copy at a fixed path, so retention skips it',
    },
    { kind: 'dim', text: '  4 backups kept by the policy' },
    { kind: 'dim', text: 'nothing to prune — the policy removes none of what is there' },
  ];

  // ── Putting one back ──────────────────────────────────────────────────────

  protected readonly restoreRefusal: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop restore shop  # from a scheduled run' },
    {
      kind: 'bad',
      tag: 'error:',
      text: " replacing a database's contents needs its name typed, and there is no terminal to type at",
    },
    { kind: 'dim', text: '  hint: pass --confirm shop to say it up front' },
  ];

  protected readonly restore = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop restore shop' },
    {
      text: `restoring ${this.path('backups', 'postgres', 'shop', '20260919T141022Z')}`,
    },
    { kind: 'dim', text: '  taken 2026-09-19 20:10:22 +06:00 (2026-09-19T14:10:22Z)' },
    {
      kind: 'dim',
      text: '  66.0 KiB of postgres 17.9, encrypted, 10601 rows, sha256 2657e422a4af',
    },
    { kind: 'dim', text: '  into shop — postgres 17.9' },
    { kind: 'dim', text: '  replacing what is there now — 2 tables, 10601 rows' },
    { kind: 'name', tag: '?', text: ' type shop to destroy it, or anything else to stop: shop' },
    { kind: 'dim', text: '  cleared 2 tables' },
    { kind: 'dim', text: '  loaded in 0.2s' },
    { kind: 'dim', text: '  checking it against the counts the manifest recorded' },
    { kind: 'step', mark: 'ok', text: 'Verified', note: 'exact count(*) on both sides' },
    { kind: 'dim', text: '  verifying — exact count(*) on both sides' },
    { kind: 'dim', text: '  public.customers  1284 → 1284' },
    { kind: 'dim', text: '  public.orders     9317 → 9317' },
    { kind: 'dim', text: '  2 of 2 tables matched' },
  ]);

  protected readonly from: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop restore shop --from 20260919T141020Z' },
    { kind: 'dim', text: '  … the same shape, from the backup you named' },
    { text: ' ' },
    { kind: 'prompt', text: 'sloop restore shop --from 20200101T000000Z' },
    { kind: 'bad', tag: 'error:', text: ' shop has no backup called 20200101T000000Z' },
    {
      kind: 'dim',
      text: "  hint: `--from` takes the directory's own name, as `sloop backups list` shows it",
    },
  ];

  // ── The lock ──────────────────────────────────────────────────────────────

  protected readonly lock = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop backup archive   # while another run of it is going' },
    {
      kind: 'bad',
      tag: 'error:',
      text: ' another sloop run is working on archive: `sloop backup`, process 24424,',
    },
    { kind: 'bad', tag: '', text: '  since 20260919T141146Z' },
    {
      kind: 'dim',
      text: '  hint: exit 7 means this run did not start rather than that it failed. The lock is at',
    },
    { kind: 'dim', text: `  ${this.lockFile()}, and it goes when that run does` },
  ]);

  // ── Verification ──────────────────────────────────────────────────────────

  protected readonly fast: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'SLOOP_VERIFY=fast sloop restore ledger --confirm ledger' },
    { kind: 'dim', text: '  … loaded in 0.1s' },
    { kind: 'dim', text: '  checking it against the counts the manifest recorded' },
    {
      kind: 'step',
      mark: 'ok',
      text: 'Verified',
      note: "estimates from the engine's own statistics — not proof",
    },
    {
      kind: 'dim',
      text: "  verifying — estimates from the engine's own statistics — not proof",
    },
    { kind: 'dim', text: '  public.entries  4210 → 4210' },
    {
      kind: 'dim',
      text: '  1 of 1 tables matched — from estimates, so this is a glance and not a proof',
    },
  ];

  /**
   * `0` and `6`, side by side.
   *
   * The entry's bar is that a reader could explain the difference to somebody
   * else, and a pair of cards is the shape that survives being half-read — the
   * paragraph underneath says the rest.
   */
  protected readonly endings: readonly Ending[] = [
    {
      code: '0',
      title: 'It finished, and it was checked',
      means:
        'Every table the manifest names arrived, and the counts on both sides agree. The copy is one you can act on.',
      then: 'Nothing to do.',
    },
    {
      code: '6',
      title: 'It finished, and then the numbers disagreed',
      means:
        'The work completed — nothing crashed, nothing timed out. What failed was the proof afterwards: a table arrived empty, or did not arrive, or a dump no longer hashes to what its manifest says.',
      then: 'Neither success nor a crash, so automation has to treat it as its own case.',
    },
  ];
}
