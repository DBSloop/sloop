import { ChangeDetectionStrategy, Component, computed, inject } from '@angular/core';
import { RouterLink } from '@angular/router';

import { DocsPage } from '../docs-page';
import { OS_LABEL, OsChoice } from '../os';
import { Terminal, type TerminalLine } from '../../ui/terminal';

/** One half of the keypair, for the panel that sets the two side by side. */
interface Half {
  readonly name: string;
  readonly kind: string;
  readonly lives: string;
  readonly needed: string;
  readonly lost: string;
}

/**
 * The backup key.
 *
 * ## How these blocks were made
 *
 * Two project registries against one throwaway PostgreSQL 17.9 cluster on port
 * 5444, on 2026-09-19, with the binary built from this commit. The second
 * registry stands in for a second machine — it is the same thing from sloop's
 * point of view, a store with its own key and its own backups, and it can be
 * shown honestly on one machine where "buy another laptop" cannot.
 *
 * The whole move was performed rather than described: back up on the first,
 * copy the backup directory across, **fail to restore**, `key import`, restore.
 * The failure block is a real exit `5`, and it is the one a reader needs most —
 * it is what losing the key looks like from the inside.
 *
 * Two refusals are real runs too: importing a second, different key into a
 * registry that already has backups, and a malformed key.
 *
 * ## The substitutions, named rather than hidden
 *
 * **The keys are replaced**, every one of them, with keys of the same shape and
 * length — 62 characters for a public key, 74 for a private one. `A7` set this
 * rule and it is not negotiable: a real `AGE-SECRET-KEY-…` on a public page
 * hands over every backup it guards.
 *
 * **The two registries' paths are written as the reader's**, the same
 * substitution `A9` and `A10` make: `C:\Users\you\projects\acme-api` and
 * `C:\Users\you\projects\acme-api` on the new machine, with the POSIX spellings
 * behind the OS strip.
 *
 * Sizes, hashes, row counts, timestamps and every word of every message are
 * what the runs printed.
 */
@Component({
  selector: 'app-docs-backup-key',
  imports: [DocsPage, RouterLink, Terminal],
  templateUrl: './backup-key.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class BackupKey {
  private readonly choice = inject(OsChoice);

  protected readonly os = this.choice.os;
  protected readonly osLabel = computed(() => OS_LABEL[this.os()]);

  private readonly slash = computed(() => (this.os() === 'windows' ? '\\' : '/'));

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

  private readonly home = computed(() => {
    switch (this.os()) {
      case 'windows':
        return 'C:\\Users\\you';
      case 'macos':
        return '/Users/you';
      default:
        return '/home/you';
    }
  });

  /** The registry the backups were taken in. */
  protected readonly laptop = computed(() =>
    [this.home(), 'projects', 'acme-api', '.sloop'].join(this.slash()),
  );

  /** The same project checked out on the machine you are moving to. */
  protected readonly newMachine = computed(() =>
    [this.home(), 'projects', 'acme-api', '.sloop'].join(this.slash()),
  );

  /** Public keys are 62 characters; this one is not a real one. */
  private readonly publicKey = 'age1t6mzq0xv9k2wr5ldh8nj3cp7yfs4ueg0a2xk9msv4rzq3ltdn6shqw8cjp';

  // ── Exporting ─────────────────────────────────────────────────────────────

  /** The first export, which is also where the key is made. */
  protected readonly exportFirst = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop key export' },
    { text: `a new backup key for ${this.laptop()}` },
    { kind: 'label', tag: '  public  ', text: this.publicKey },
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
    { text: 'AGE-SECRET-KEY-1K9XW3MT25G5TU6LTSN07M55JPT9Y0T24MDNQZ2DXW202RULF43W44CQ' },
  ]);

  /**
   * The same command again, and the first word is the whole difference.
   *
   * *a new backup key* on the run that made it, *the backup key* on every run
   * after. Worth showing, because a reader who runs it twice needs to know the
   * second run did not replace anything.
   */
  protected readonly exportAgain = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop key export' },
    { text: `the backup key for ${this.laptop()}` },
    { kind: 'label', tag: '  public  ', text: this.publicKey },
    { kind: 'label', tag: '  private ', text: 'the OS keyring, and on standard output below' },
    { kind: 'dim', text: '  …' },
  ]);

  /** What `backup` does before the key has been copied anywhere. */
  protected readonly refusal: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop backup shop' },
    { text: ' ' },
    { kind: 'head', tag: 'this registry has a new backup key', text: '' },
    {
      kind: 'name',
      tag: '  age1t6mzq0xv9k2wr5ldh8nj3cp7yfs4ueg0a2xk9msv4rzq3ltdn6shqw8cjp',
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

  // ── Moving a machine ──────────────────────────────────────────────────────

  /** The backup the old machine took, and the copy across. */
  protected readonly taken = computed<readonly TerminalLine[]>(() => {
    const s = this.slash();
    return [
      { kind: 'dim', text: '# on the machine that has the backups' },
      { kind: 'prompt', text: 'sloop backup shop' },
      { kind: 'name', tag: 'shop', text: '', note: '  postgres://app@127.0.0.1:5444/shop' },
      { kind: 'dim', text: '  postgres 17.9, 2 tables, 10601 rows' },
      {
        kind: 'dim',
        text: `  backed up to ${this.laptop()}${s}backups${s}postgres${s}shop${s}20260919T142852Z`,
      },
      { kind: 'dim', text: '  66.0 KiB in 0.3s, sha256 42958e81e612' },
    ];
  });

  /** The new machine can see the backup perfectly well. That is not the problem. */
  protected readonly seenThere: readonly TerminalLine[] = [
    { kind: 'dim', text: '# on the new machine, after copying the backups directory across' },
    { kind: 'prompt', text: 'sloop backups list' },
    { kind: 'dim', text: 'project' },
    { kind: 'name', tag: 'shop', text: '', note: '  postgres · 1 backup, 66.0 KiB' },
    { kind: 'dim', text: '  2026-09-19 20:28:52 +06:00  66.0 KiB, encrypted · 10601 rows' },
    { text: ' ' },
    { kind: 'dim', text: '1 backup, 66.0 KiB · sizes checked, --check hashes them' },
  ];

  /** And this is what it is worth without the key. */
  protected readonly withoutKey = computed<readonly TerminalLine[]>(() => {
    const s = this.slash();
    return [
      { kind: 'prompt', text: 'sloop restore shop --confirm shop' },
      {
        text: `restoring ${this.newMachine()}${s}backups${s}postgres${s}shop${s}20260919T142852Z`,
      },
      { kind: 'dim', text: '  taken 2026-09-19 20:28:52 +06:00 (2026-09-19T14:28:52Z)' },
      {
        kind: 'dim',
        text: '  66.0 KiB of postgres 17.9, encrypted, 10601 rows, sha256 42958e81e612',
      },
      { kind: 'dim', text: '  into shop — postgres 17.9' },
      { kind: 'dim', text: '  it is empty' },
      {
        kind: 'bad',
        tag: 'error:',
        text: ` that backup is encrypted to ${this.publicKey}`,
      },
      { kind: 'bad', tag: '', text: '  and this registry has no key' },
      {
        kind: 'dim',
        text: '  hint: `sloop key import` takes the key exported from the machine that made it',
      },
    ];
  });

  protected readonly importing = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop key import < backup-key.txt' },
    { kind: 'name', tag: 'imported', text: '', note: ` ${this.publicKey}` },
    {
      kind: 'dim',
      text: `  kept in the OS keyring, and backups under ${this.newMachine()} are encrypted to it`,
    },
    { kind: 'dim', text: '  from now on' },
  ]);

  protected readonly restored = computed<readonly TerminalLine[]>(() => {
    const s = this.slash();
    return [
      { kind: 'prompt', text: 'sloop restore shop --confirm shop' },
      {
        text: `restoring ${this.newMachine()}${s}backups${s}postgres${s}shop${s}20260919T142852Z`,
      },
      { kind: 'dim', text: '  taken 2026-09-19 20:28:52 +06:00 (2026-09-19T14:28:52Z)' },
      {
        kind: 'dim',
        text: '  66.0 KiB of postgres 17.9, encrypted, 10601 rows, sha256 42958e81e612',
      },
      { kind: 'dim', text: '  into shop — postgres 17.9' },
      { kind: 'dim', text: '  it is empty' },
      { kind: 'dim', text: '  loaded in 0.1s' },
      { kind: 'dim', text: '  checking it against the counts the manifest recorded' },
      { kind: 'dim', text: '  verifying — exact count(*) on both sides' },
      { kind: 'ok', tag: '  public.customers  1284 → 1284', text: '' },
      { kind: 'ok', tag: '  public.orders     9317 → 9317', text: '' },
      { kind: 'ok', tag: '  2 of 2 tables matched', text: '' },
    ];
  });

  // ── One key per registry ──────────────────────────────────────────────────

  /** Importing the same key twice is a no-op, and says so. */
  protected readonly sameAgain = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop key import < backup-key.txt' },
    { kind: 'name', tag: 'already the key here', text: '', note: ` ${this.publicKey}` },
  ]);

  /** A second, different key is refused — the old backups would become unreadable. */
  protected readonly differentKey = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop key import < a-different-key.txt' },
    {
      kind: 'bad',
      tag: 'error:',
      text: ` this registry is already encrypting to ${this.publicKey},`,
    },
    { kind: 'bad', tag: '', text: '  and that is a different key' },
    {
      kind: 'dim',
      text: '  hint: every backup already taken can only be read with the key it was written for.',
    },
    {
      kind: 'dim',
      text: '  To move to a new key, take the `[encryption]` block out of the registry by hand',
    },
    { kind: 'dim', text: '  first — and keep the old key, or those backups are gone' },
  ]);

  /** A key that is not one. */
  protected readonly malformed: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop key import < notes.txt' },
    {
      kind: 'bad',
      tag: 'error:',
      text: ' that is not an age private key: invalid Bech32 encoding',
    },
    {
      kind: 'dim',
      text: '  hint: it is the `AGE-SECRET-KEY-1…` line that `sloop key export` printed',
    },
  ];

  /** The two halves, set against each other. */
  protected readonly halves = computed<readonly Half[]>(() => [
    {
      name: 'The public half',
      kind: 'age1t6mzq0xv9k2wr5ldh8nj3cp7yfs4ueg0a2xk9msv4rzq3ltdn6shqw8cjp',
      lives: 'In the registry, in plain sight. It is on the manifest of every backup it encrypted.',
      needed:
        'Every backup. It is all a backup needs, which is why a scheduled run can take one at 3am with nothing unlocked and no secret anywhere near it.',
      lost: 'Nothing. It is derived from the private half and it is not a secret — publishing it costs you nothing at all.',
    },
    {
      name: 'The private half',
      kind: 'AGE-SECRET-KEY-1K9XW3MT25G5TU6LTSN07M55JPT9Y0T24MDNQZ2DXW202RULF43W44CQ',
      lives: `In ${this.keyring()} — or in the Argon2id-encrypted file, on a machine with no keyring running.`,
      needed:
        'Only a restore. Nothing else on this page, and nothing a schedule ever does, reads it.',
      lost: 'Every backup it ever encrypted, at once and for good. There is no recovery, no reset and nobody to ask.',
    },
  ]);
}
