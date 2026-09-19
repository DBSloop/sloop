import { ChangeDetectionStrategy, Component, computed, inject } from '@angular/core';
import { RouterLink } from '@angular/router';

import { DocsPage } from '../docs-page';
import { OS_LABEL, OsChoice } from '../os';
import { Terminal, type TerminalLine } from '../../ui/terminal';

/**
 * Getting started: six commands, and the page shows one platform at a time.
 *
 * **One platform at a time is the rule for the whole docs section, not just for
 * installation.** The first version of this page put the Windows transcript in
 * the block and then listed apt, dnf and Homebrew in the prose underneath it —
 * so a reader on the Windows tab was handed two package managers they cannot
 * run, which is the exact thing the OS strip exists to prevent. Every command
 * and every path below is behind that strip now, and the mechanical check that
 * `A6` uses applies here too: with one platform selected the article must
 * contain no marker belonging to another.
 *
 * ## How these were made
 *
 * A throwaway PostgreSQL 17.9 cluster was built with `initdb` on port 5441,
 * seeded with two tables — 1,284 customers and 9,317 orders — and every command
 * below was run against it, in order, on 2026-09-19, with the binary built from
 * this commit. The cluster was torn down afterwards.
 *
 * **The binary matters, and getting it wrong is how this page was wrong once.**
 * The first version used whatever was sitting in `cli/target/release`, which
 * turned out to predate `R19a`, `R24`, `R27a` and `R28` — so the transcripts were
 * what an older sloop printed. It was caught because that binary's `--help`
 * listed neither `query` nor `service`, and confirmed when a rebuild refused to
 * read the registry the old one had written: schema version 5 where the current
 * binary wants 9. Re-running every command against a fresh build showed the six
 * data blocks were word-for-word correct and only their numbers had moved; the
 * `setup` block was not, and is dealt with below.
 *
 * **Rebuild before capturing. A stale binary is a page that lies about a version
 * nobody is running.**
 *
 * ## What differs per platform, and how each difference is honoured
 *
 * **Six of the seven blocks differ only in a path.** `db add`, `db test`,
 * `key export`, `backup`, `backups list` and `restore` print the same words
 * everywhere; only the store they name changes. That is the same named
 * substitution `A6` makes — `~/.sloop` spelled the way each platform spells it.
 *
 * **`setup` differs on every platform, and every block stops at the question.**
 * Windows is offered a download, Linux and macOS their own package manager. Each
 * block holds the command, the offer and the question and nothing after it, with
 * the cut marked. On Windows those lines came from a real run; on the other two
 * they are `Plan::describe()`, `Plan::question()` and `server_plan()` in
 * `cli/src/tools/acquire.rs`, quoted. What happens after the question is prose,
 * because on Linux and macOS it was never observed and on Windows re-capturing
 * it would cost a `sloop reset` and another 344 MB download to photograph
 * something a reader sees once.
 *
 * ## The rest of the substitutions, named rather than hidden
 *
 * 1. **`C:\Users\Saad\` becomes `C:\Users\you\`.** A home directory is the
 *    reader's, not the author's.
 * 2. **The two keys are replaced** with keys of the same shape and length —
 *    62 characters for the public one, 74 for the private — because publishing
 *    a real `age` private key would hand every backup this machine holds to
 *    anybody reading.
 * 3. **Two prompt lines are quoted from the source rather than taken from the
 *    log.** A prompt is drawn in raw mode and read straight off the terminal, so
 *    `--log-file` never sees one. They are `Plan::question()` and the typed
 *    confirmation in `cli/src/consent.rs`, each followed by the answer that was
 *    actually given.
 *
 * The password prompt `db add` draws is **not** reproduced, for the same rule
 * read the other way: it is named for `credential_key()`, whose exact spelling
 * this page has not seen printed, and a guessed prompt is a lie in a block whose
 * whole value is that it is not one. It is described in prose instead.
 */
@Component({
  selector: 'app-docs-getting-started',
  imports: [DocsPage, RouterLink, Terminal],
  templateUrl: './getting-started.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class GettingStarted {
  private readonly choice = inject(OsChoice);

  protected readonly os = this.choice.os;
  protected readonly osLabel = computed(() => OS_LABEL[this.os()]);

  /** Where sloop keeps everything, spelled the way this platform spells it. */
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

  /** The name this platform's reader knows their secret store by. */
  protected readonly keyring = computed(() => {
    switch (this.os()) {
      case 'windows':
        return 'Credential Manager';
      case 'macos':
        return 'the Keychain';
      default:
        return 'the Secret Service';
    }
  });

  private readonly backupPath = computed(() => {
    const parts = ['backups', 'postgres', 'shop', '20260919T124114Z'];
    return this.os() === 'windows'
      ? `${this.store()}\\${parts.join('\\')}`
      : `${this.store()}/${parts.join('/')}`;
  });

  protected readonly shell = computed(() => {
    switch (this.os()) {
      case 'windows':
        return 'PowerShell, on Windows';
      case 'macos':
        return 'a terminal, on macOS';
      default:
        return 'a shell, on Linux';
    }
  });

  // ── 1 · setup ─────────────────────────────────────────────────────────────

  protected readonly setup = computed<readonly TerminalLine[]>(() => {
    switch (this.os()) {
      case 'windows':
        return WINDOWS_SETUP;
      case 'macos':
        return offerSetup('Homebrew', 'brew install postgresql@18');
      default:
        return offerSetup('apt', 'sudo apt-get install -y postgresql-18');
    }
  });

  // ── 2 · db add, db test ───────────────────────────────────────────────────

  protected readonly register: readonly TerminalLine[] = [
    {
      kind: 'prompt',
      text: 'sloop db add shop --global --url postgres://app@127.0.0.1:5441/shop',
    },
    { text: 'registered shop in the global registry (--global), password from the OS keyring' },
    { kind: 'dim', text: '  postgres://app@127.0.0.1:5441/shop' },
  ];

  protected readonly test: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop db test shop --global' },
    { text: ' ' },
    {
      kind: 'name',
      tag: 'shop',
      text: '',
      note: '  postgres://app@127.0.0.1:5441/shop  global',
    },
    { kind: 'dim', text: '  postgres 17.9, not encrypted' },
  ];

  // ── 3 · key export ────────────────────────────────────────────────────────

  protected readonly exportKey = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop key export --global' },
    { text: `a new backup key for ${this.store()}` },
    {
      kind: 'label',
      tag: '  public  ',
      text: 'age1w4k7mz0qx9v2lr5htn3jd8pc6yfs0ueg7a2xk9msv4rzq3ltdn6sh8wcjp',
    },
    { kind: 'label', tag: '  private ', text: 'the OS keyring, and on standard output below' },
    {
      kind: 'warn',
      tag: '  keep that line somewhere sloop cannot reach',
      text: ' — a password manager, a safe.',
    },
    {
      kind: 'dim',
      text: '  Every encrypted backup this registry takes needs it, and nothing else can replace it.',
    },
    { text: ' ' },
    { text: 'AGE-SECRET-KEY-1QF8TZ3KXW9MV0RJ5H2LYD7NPS4UGC6AE8QT7XZ3KMW9VRJ5H2LYDQ7X4NP' },
  ]);

  // ── 4 · backup ────────────────────────────────────────────────────────────

  protected readonly firstBackup = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop backup shop --global' },
    { kind: 'name', tag: 'shop', text: '', note: '  postgres://app@127.0.0.1:5441/shop' },
    { kind: 'dim', text: '  postgres 17.9, 2 tables, 10601 rows' },
    { kind: 'dim', text: `  backed up to ${this.backupPath()}` },
    { kind: 'dim', text: '  88.4 KiB in 0.5s, sha256 1c90608b129c' },
    { kind: 'dim', text: '  taken 2026-09-19 18:41:14 +06:00 (2026-09-19T12:41:14Z)' },
  ]);

  // ── 5 · backups list ──────────────────────────────────────────────────────

  protected readonly listBackups: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop backups list --global' },
    { kind: 'name', tag: 'shop', text: '', note: '  postgres · 1 backup, 88.4 KiB' },
    { kind: 'dim', text: '  2026-09-19 18:41:14 +06:00  88.4 KiB, encrypted · 10601 rows' },
    { text: ' ' },
    { kind: 'dim', text: '1 backup, 88.4 KiB · sizes checked, --check hashes them' },
    { text: ' ' },
    { kind: 'prompt', text: 'sloop backups list --global --check' },
    { kind: 'name', tag: 'shop', text: '', note: '  postgres · 1 backup, 88.4 KiB' },
    { kind: 'dim', text: '  2026-09-19 18:41:14 +06:00  88.4 KiB, encrypted · 10601 rows' },
    { text: ' ' },
    { kind: 'dim', text: '1 backup, 88.4 KiB · every dump hashed' },
  ];

  // ── 6 · restore ───────────────────────────────────────────────────────────

  protected readonly restore = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop restore shop --global' },
    { text: `restoring ${this.backupPath()}` },
    { kind: 'dim', text: '  taken 2026-09-19 18:41:14 +06:00 (2026-09-19T12:41:14Z)' },
    {
      kind: 'dim',
      text: '  88.4 KiB of postgres 17.9, encrypted, 10601 rows, sha256 1c90608b129c',
    },
    { kind: 'dim', text: '  into shop — postgres 17.9' },
    { kind: 'warn', tag: '  replacing what is there now', text: ' — 2 tables, 10601 rows' },
    { kind: 'name', tag: '?', text: ' type shop to destroy it, or anything else to stop: shop' },
    { kind: 'dim', text: '  cleared 2 tables' },
    { kind: 'dim', text: '  loaded in 0.1s' },
    { kind: 'dim', text: '  checking it against the counts the manifest recorded' },
    { kind: 'dim', text: '  verifying — exact count(*) on both sides' },
    { kind: 'ok', tag: '  public.customers  1284 → 1284', text: '' },
    { kind: 'ok', tag: '  public.orders     9317 → 9317', text: '' },
    { kind: 'ok', tag: '  2 of 2 tables matched', text: '' },
  ]);

  /** The manifest filed beside that dump, as it was written. */
  protected readonly manifest: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'cat manifest.json' },
    { text: '{' },
    { kind: 'dim', text: '  "version": 1,' },
    { kind: 'dim', text: '  "sloop": "0.1.0",' },
    { kind: 'dim', text: '  "label": "shop",' },
    { kind: 'dim', text: '  "engine": "postgres",' },
    { kind: 'dim', text: '  "source": "postgres://app@127.0.0.1:5441/shop",' },
    { kind: 'dim', text: '  "server": { "version": "17.9", "tls": false },' },
    { kind: 'dim', text: '  "taken": {' },
    { kind: 'dim', text: '    "utc": "2026-09-19T12:41:14Z",' },
    { kind: 'dim', text: '    "local": "2026-09-19T18:41:14+06:00",' },
    { kind: 'dim', text: '    "offset": "+06:00", "offset_seconds": 21600,' },
    { kind: 'dim', text: '    "unix": 1789821674' },
    { kind: 'dim', text: '  },' },
    { kind: 'dim', text: '  "took_seconds": 0.765,' },
    { kind: 'dim', text: '  "dump_seconds": 0.487,' },
    { kind: 'dim', text: '  "dump": {' },
    { kind: 'dim', text: '    "file": "dump.age",' },
    { kind: 'dim', text: '    "bytes": 90525,' },
    { kind: 'dim', text: '    "sha256": "1c90608b129ccc8f39cc61395ad374f99ea...",' },
    { kind: 'dim', text: '    "encryption": { "format": "age", "recipient": "age1w4k7mz0..." }' },
    { kind: 'dim', text: '  },' },
    { kind: 'dim', text: '  "rows": 10601,' },
    { kind: 'dim', text: '  "tables": [' },
    { kind: 'dim', text: '    { "schema": "public", "name": "customers", "rows": 1284 },' },
    { kind: 'dim', text: '    { "schema": "public", "name": "orders",    "rows": 9317 }' },
    { kind: 'dim', text: '  ]' },
    { text: '}' },
  ];
}

/**
 * The Windows offer, captured from a real `sloop setup` on this machine.
 *
 * **Cut at the question, exactly like the other two platforms**, and that cut is
 * a correction rather than a style choice. The first version of this page ran
 * the whole install and printed every line of it — but it ran against a
 * `target/release` binary built before `R19a`, `R24`, `R27a` and `R28`, so those
 * lines were what an older sloop printed. Re-running proved it: the schema it
 * laid down was version 5 where the current binary wants 9, and `R28`'s
 * plain-language pass had since rewritten *kept where this machine keeps
 * secrets* into *kept in the OS keyring*.
 *
 * Re-capturing the install sequence would mean `sloop reset` and another 344 MB
 * download on the owner's machine to photograph something a reader sees once.
 * So the block stops where the reader has to decide, every line in it was
 * checked against a current run, and what happens after the question is prose.
 */
const WINDOWS_SETUP: readonly TerminalLine[] = [
  { kind: 'prompt', text: 'sloop setup' },
  {
    kind: 'head',
    tag: 'Already here:',
    text: ' PostgreSQL 17.9 at C:\\Program Files\\PostgreSQL\\17\\bin, which sloop leaves exactly as it is.',
  },
  {
    kind: 'head',
    tag: 'Needed:',
    text: ' sloop keeps its own state in PostgreSQL 18, and this machine has none.',
  },
  {
    kind: 'dim',
    text: '  sloop can download the official PostgreSQL 18.6 binaries for Windows (344 MB), check them against a SHA-256 built into this binary, and keep pg_dump, pg_restore and psql.',
  },
  {
    kind: 'dim',
    text: "  The download is the system's own curl. Nothing about this machine is sent anywhere.",
  },
  { kind: 'name', tag: '?', text: ' Download and install them now? y' },
  { kind: 'dim', text: '  …' },
];

/**
 * What `setup` prints on a platform with a package manager, **up to the question
 * and no further**.
 *
 * Every line is `Plan::describe()` and `Plan::question()` in
 * `cli/src/tools/acquire.rs`, with the manager and command from `server_plan()`
 * in the same file. What comes after the question depends on what the package
 * manager leaves running, which this session did not observe on either platform
 * — so the block stops, and the cut is marked the way every excerpt on this site
 * is marked.
 */
function offerSetup(manager: string, command: string): readonly TerminalLine[] {
  return [
    { kind: 'prompt', text: 'sloop setup' },
    {
      kind: 'head',
      tag: 'Needed:',
      text: ' sloop keeps its own state in PostgreSQL 18, and this machine has none.',
    },
    { kind: 'dim', text: `  sloop can ask ${manager} to install them:` },
    { kind: 'dim', text: `    ${command}` },
    { kind: 'name', tag: '?', text: ` Run that ${manager} command now? y` },
    { kind: 'dim', text: '  …' },
  ];
}
