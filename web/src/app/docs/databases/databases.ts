import { ChangeDetectionStrategy, Component, computed, inject } from '@angular/core';
import { RouterLink } from '@angular/router';

import { DocsPage } from '../docs-page';
import { OS_LABEL, OsChoice } from '../os';
import { Terminal, type TerminalLine } from '../../ui/terminal';

/** One rung of the resolution ladder, rendered as the numbered list on the page. */
interface Step {
  readonly rule: string;
  readonly text: string;
  /** Shown in the monospace column beside the rule. Empty where there is no flag. */
  readonly how: string;
}

/**
 * Databases, registries and projects.
 *
 * **The page `A9` exists for**, and it exists because nothing on the list covered
 * how sloop decides which database a bare name means. Everything else assumes
 * that question is settled; this is where it is settled.
 *
 * ## How these blocks were made
 *
 * A throwaway PostgreSQL 17.9 cluster was built with `initdb` on port 5442 and a
 * throwaway MySQL 8.4.11 on 3311 — the portable one under
 * `%LOCALAPPDATA%\sloop-test-engines` — on 2026-09-19, and every command below
 * was run against them with the binary built from this commit. Both were torn
 * down afterwards. Nothing here is transcribed from `TOTAL_FEATURE_LIST.md` or
 * remembered: a block on this page is output that was produced.
 *
 * **The binary matters.** `A7` was written from a `target/release` build four
 * features out of date and every transcript on it had to be recaptured. This one
 * was captured from a build of `20a6f21`, which is `sloop 0.1.0`.
 *
 * ## The substitutions, named rather than hidden
 *
 * Two, and both are the same kind `A6` and `A7` make — a path spelled the way
 * the reader's platform spells it, and a home directory that is the reader's
 * rather than the author's.
 *
 * 1. **`C:\Users\Saad\` becomes `C:\Users\you\`**, and the global store is
 *    spelled `~/.sloop` the way each platform spells it. That path is real and
 *    the same on all three — it is only the spelling that moves.
 * 2. **The project directory is written as the reader's.** It was created in a
 *    scratch directory to be captured; on the page it is
 *    `C:\Users\you\projects\acme-api` and its POSIX equivalents. The
 *    `name      acme-api` line `init` prints is the directory's own name, so on
 *    any path ending in `acme-api` it is the line that was printed.
 *
 * **One line is quoted from source rather than taken from a log**, the way `A7`
 * quotes two: the typed confirmation `db drop` draws. A prompt is written in raw
 * mode and read straight off the terminal, so a redirected run never records
 * one. It is `Consent::typed` in `cli/src/consent.rs`, with the answer that was
 * given. Everything else below was captured from a redirected run.
 *
 * ## What is *not* on this page
 *
 * The full flag tables. They are generated from `--help` on the
 * [command reference](/docs/commands) and there is exactly one of them, on
 * purpose — a second hand-maintained copy is the one that goes stale. This page
 * names the flags it is explaining and links there for the rest.
 */
@Component({
  selector: 'app-docs-databases',
  imports: [DocsPage, RouterLink, Terminal],
  templateUrl: './databases.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Databases {
  private readonly choice = inject(OsChoice);

  protected readonly os = this.choice.os;
  protected readonly osLabel = computed(() => OS_LABEL[this.os()]);

  /** `~/.sloop`, spelled the way this platform spells it. */
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

  /** A project directory, spelled the same way. */
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

  protected readonly projectStore = computed(() =>
    this.os() === 'windows' ? `${this.project()}\\.sloop` : `${this.project()}/.sloop`,
  );

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

  /** How this platform sets an environment variable for one command. */
  protected readonly setVariable = computed(() => {
    switch (this.os()) {
      case 'windows':
        return '$env:SHOP_PASSWORD = "…"; sloop backup nightly';
      default:
        return 'SHOP_PASSWORD=… sloop backup nightly';
    }
  });

  // ── Registering ───────────────────────────────────────────────────────────

  /** `db add`, the whole connection in one string. */
  protected readonly addByUrl: readonly TerminalLine[] = [
    {
      kind: 'prompt',
      text: 'sloop db add shop --global --url postgres://app@127.0.0.1:5442/shop --test',
    },
    { kind: 'step', mark: 'ok', text: 'postgres 17.9', note: 'not encrypted' },
    {
      kind: 'name',
      tag: 'registered shop',
      text: ' in the global registry',
      note: ' (--global), password from the OS keyring',
    },
    { kind: 'dim', text: '  postgres://app@127.0.0.1:5442/shop' },
  ];

  /** The same registration, field by field. */
  protected readonly addByFields: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop db add ledger --global --engine postgres \\' },
    { kind: 'prompt', text: '    --host 127.0.0.1 --port 5442 --database ledger --user app' },
    {
      kind: 'name',
      tag: 'registered ledger',
      text: ' in the global registry',
      note: ' (--global), password from the OS keyring',
    },
    { kind: 'dim', text: '  postgres://app@127.0.0.1:5442/ledger' },
  ];

  /** Neither `--engine` nor `--url`: it says so and stops, rather than asking. */
  protected readonly addWithNeither: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop db add nowhere --global' },
    { kind: 'bad', tag: 'error:', text: ' no engine was given' },
    {
      kind: 'dim',
      text: '  hint: pass --engine, or give the whole connection with --url postgres://user@host:5432/database',
    },
  ];

  /** A MySQL registration, so the page is not three PostgreSQL blocks. */
  protected readonly addMysql: readonly TerminalLine[] = [
    {
      kind: 'prompt',
      text: 'sloop db add sessions --global --url mysql://app@127.0.0.1:3311/sessions --test',
    },
    { kind: 'step', mark: 'ok', text: 'mysql 8.4', note: 'over TLS' },
    {
      kind: 'name',
      tag: 'registered sessions',
      text: ' in the global registry',
      note: ' (--global), password from the OS keyring',
    },
    { kind: 'dim', text: '  mysql://app@127.0.0.1:3311/sessions' },
  ];

  // ── The four password routes ──────────────────────────────────────────────

  /** Three more registrations, one per remaining route. */
  protected readonly addRoutes: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop db add headless --global --url … --encrypted-file' },
    {
      kind: 'name',
      tag: 'registered headless',
      text: ' in the global registry',
      note: ' (--global), password from the encrypted file',
    },
    { text: ' ' },
    { kind: 'prompt', text: 'sloop db add nightly --global --url … --env SHOP_PASSWORD' },
    {
      kind: 'dim',
      text: 'note: SHOP_PASSWORD is not set, and the registry says the password is in it — registered',
    },
    {
      kind: 'dim',
      text: '  anyway, because the environment variable SHOP_PASSWORD is read on every run rather',
    },
    { kind: 'dim', text: '  than kept here' },
    {
      kind: 'name',
      tag: 'registered nightly',
      text: ' in the global registry',
      note: ' (--global), password from the environment variable SHOP_PASSWORD',
    },
    { text: ' ' },
    {
      kind: 'prompt',
      text: 'sloop db add vaulted --global --url … --password-from "op read op://vault/db/password"',
    },
    {
      kind: 'dim',
      text: 'note: the password command exited with 1: op read op://vault/db/password — registered',
    },
    {
      kind: 'dim',
      text: '  anyway, because the command `op read op://vault/db/password` is read on every run',
    },
    { kind: 'dim', text: '  rather than kept here' },
    {
      kind: 'name',
      tag: 'registered vaulted',
      text: ' in the global registry',
      note: ' (--global), password from the command `op read op://vault/db/password`',
    },
  ];

  /** `db list` with all four routes in it, and not a secret anywhere. */
  protected readonly list = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop db list --global' },
    {
      kind: 'name',
      tag: 'accounts',
      text: '  postgres://app@127.0.0.1:5442/ledger            ',
      note: '  global · the OS keyring',
    },
    {
      kind: 'name',
      tag: 'headless',
      text: '  postgres://app@127.0.0.1:5442/shop              ',
      note: '  global · the encrypted file',
    },
    {
      kind: 'name',
      tag: 'nightly ',
      text: '  postgres://app@127.0.0.1:5442/shop              ',
      note: '  global · the environment variable SHOP_PASSWORD',
    },
    {
      kind: 'name',
      tag: 'sessions',
      text: '  mysql://app@127.0.0.1:3311/sessions             ',
      note: '  global · the OS keyring',
    },
    {
      kind: 'name',
      tag: 'shop    ',
      text: '  postgres://app@127.0.0.1:5442/shop              ',
      note: '  global · the OS keyring',
    },
    {
      kind: 'name',
      tag: 'vaulted ',
      text: '  postgres://app@127.0.0.1:5442/ledger            ',
      note: '  global · the command `op read op://vault/db/password`',
    },
    { text: ' ' },
    {
      kind: 'dim',
      text: `A bare name is looked for in the global store at ${this.store()}.`,
    },
  ]);

  // ── db create ─────────────────────────────────────────────────────────────

  /** `db create`, which makes the database *and* the role that owns it. */
  protected readonly create: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop db create reports --global --engine postgres \\' },
    {
      kind: 'prompt',
      text: '    --host 127.0.0.1 --port 5442 --database reports --role reports_owner \\',
    },
    {
      kind: 'prompt',
      text: '    --superuser postgres --superuser-password-command "op read op://vault/pg/root" \\',
    },
    { kind: 'prompt', text: '    --role-password-stdin' },
    {
      kind: 'name',
      tag: 'creating',
      text: '',
      note: ' postgres://reports_owner@127.0.0.1:5442/reports',
    },
    { kind: 'dim', text: '  as postgres, whose password is used once and kept nowhere' },
    { kind: 'step', mark: 'ok', text: 'postgres 17.9', note: 'not encrypted' },
    { kind: 'name', tag: 'created reports', text: '' },
    { kind: 'dim', text: '  role reports_owner created' },
    { kind: 'dim', text: '  granted USAGE, CREATE ON SCHEMA public TO reports_owner' },
    { kind: 'dim', text: '  granted default privileges on tables and sequences to reports_owner' },
    {
      kind: 'dim',
      text: '  registered as reports in the global registry (--global), password in the OS keyring',
    },
  ];

  /** Both passwords cannot come down the same pipe, and it says which flag fixes it. */
  protected readonly createNoTerminal: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop db create reports --global … --superuser-password-stdin' },
    {
      kind: 'bad',
      tag: 'error:',
      text: ' there is no terminal to ask at, and --role-password-stdin is missing',
    },
    {
      kind: 'dim',
      text: '  hint: unattended, it looks like this: --superuser-password-command "op read',
    },
    {
      kind: 'dim',
      text: '  op://vault/pg/root" --role-password-stdin, with the new password piped in. Only one of',
    },
    {
      kind: 'dim',
      text: '  the two can use standard input, so the other takes a --…-password-command',
    },
  ];

  // ── Checking and changing ─────────────────────────────────────────────────

  protected readonly test: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop db test accounts --global' },
    { text: ' ' },
    {
      kind: 'name',
      tag: 'accounts',
      text: '  postgres://app@127.0.0.1:5442/ledger',
      note: '  global',
    },
    { kind: 'step', mark: 'ok', text: 'postgres 17.9', note: 'not encrypted' },
    { text: ' ' },
    { kind: 'prompt', text: 'sloop db test sessions --global' },
    { text: ' ' },
    {
      kind: 'name',
      tag: 'sessions',
      text: '  mysql://app@127.0.0.1:3311/sessions',
      note: '  global',
    },
    { kind: 'step', mark: 'ok', text: 'mysql 8.4', note: 'over TLS' },
  ];

  /** A route that cannot be resolved fails here, before anything is dumped. */
  protected readonly testBadRoute: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop db test vaulted --global' },
    { text: ' ' },
    {
      kind: 'name',
      tag: 'vaulted',
      text: '  postgres://app@127.0.0.1:5442/ledger',
      note: '  global',
    },
    {
      kind: 'step',
      mark: 'bad',
      text: 'the password command exited with 1: op read op://vault/db/password',
    },
    {
      kind: 'dim',
      text: "  hint: 'op' is not recognized as an internal or external command,; operable program or batch file.",
    },
  ];

  protected readonly edit: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop db edit ledger --global --host db.internal' },
    { kind: 'name', tag: 'changed ledger', text: '', note: '  in the global registry' },
    { kind: 'dim', text: '  postgres://app@127.0.0.1:5442/ledger' },
    { text: '  postgres://app@db.internal:5442/ledger' },
  ];

  protected readonly rename: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop db rename ledger accounts --global' },
    { kind: 'name', tag: 'renamed', text: ' ledger → accounts' },
    { kind: 'dim', text: '  the password is filed under the connection, so it did not move' },
  ];

  // ── Forgetting against destroying ─────────────────────────────────────────

  // `forgetting` is dim, the name is the accent and the connection is dim again
  // — three pieces where a line here has a coloured `tag`, ordinary `text` and a
  // dim `note`. So the name takes the ordinary slot and loses its accent; every
  // character is the one the CLI printed, and the two dim runs are dim.
  protected readonly remove: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop db remove shop --global --yes' },
    {
      kind: 'label',
      tag: 'forgetting ',
      text: 'shop',
      note: '  postgres://app@127.0.0.1:5442/shop',
    },
    { kind: 'dim', text: '  the global registry, and the password kept in the OS keyring' },
    { kind: 'dim', text: '  the database itself is not touched — that is `sloop db drop`' },
    { kind: 'name', tag: 'forgot shop', text: '' },
  ];

  protected readonly drop: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop db drop reports --global' },
    { kind: 'name', tag: 'about to drop reports', text: '' },
    { kind: 'dim', text: '  postgres://reports_owner@127.0.0.1:5442/reports' },
    { kind: 'dim', text: '  postgres 17.9, registered here as reports' },
    { kind: 'dim', text: '  0 table(s), 0 row(s) — all of it' },
    {
      kind: 'dim',
      text: '  nothing is kept and this cannot be undone — `sloop backup reports` first if you want a copy',
    },
    {
      kind: 'name',
      tag: '?',
      text: ' type reports to destroy it, or anything else to stop: reports',
    },
    { kind: 'name', tag: 'dropped reports', text: '' },
    {
      kind: 'dim',
      text: '  reports is still registered and now points at nothing — `sloop db remove reports` forgets it',
    },
  ];

  /** No terminal, and a `--confirm` that does not match. Two refusals, both exit 2. */
  protected readonly dropRefusals: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop db drop reports --global  # from a scheduled run' },
    {
      kind: 'bad',
      tag: 'error:',
      text: ' dropping a database needs its name typed, and there is no terminal to type at',
    },
    { kind: 'dim', text: '  hint: pass --confirm reports to say it up front' },
    { text: ' ' },
    { kind: 'prompt', text: 'sloop db drop reports --global --confirm wrong' },
    { kind: 'bad', tag: 'error:', text: ' --confirm says wrong, and the database is reports' },
    {
      kind: 'dim',
      text: '  hint: nothing was contacted and nothing was changed. The two have to match exactly',
    },
  ];

  // ── Registries ────────────────────────────────────────────────────────────

  protected readonly init = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop init' },
    { text: ' ' },
    { kind: 'name', tag: '       _                   ', text: '' },
    { kind: 'name', tag: '      | |                  ', text: '' },
    { kind: 'name', tag: '   ___| | ___   ___  _ __  ', text: '' },
    { kind: 'name', tag: "  / __| |/ _ \\ / _ \\| '_ \\ ", text: '' },
    { kind: 'name', tag: '  \\__ \\ | (_) | (_) | |_) |', text: '' },
    { kind: 'name', tag: '  |___/_|\\___/ \\___/| .__/ ', text: '' },
    { kind: 'name', tag: '                    | |    ', text: '' },
    { kind: 'name', tag: '                    |_|    ', text: '' },
    { text: ' ' },
    { kind: 'head', tag: '  Project registry created', text: '' },
    { text: ' ' },
    { kind: 'label', tag: '  registry  ', text: this.projectStore() },
    { kind: 'label', tag: '  name      ', text: 'acme-api' },
    { kind: 'dim', text: '            reachable anywhere as: sloop -C acme-api' },
    { kind: 'label', tag: '  global    ', text: this.store() },
    { text: ' ' },
    { kind: 'dim', text: '  .sloop ignores itself, so git will never see it.' },
  ]);

  /** The same name in both registries, and `db list` saying which one wins. */
  protected readonly shadowed = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: `cd ${this.project()}` },
    { kind: 'prompt', text: 'sloop db add shop --url postgres://app@127.0.0.1:5442/shop_staging' },
    {
      kind: 'name',
      tag: 'registered shop',
      text: ' in the project registry',
      note: ' (the nearest .sloop at or above the working directory), password from the OS keyring',
    },
    { kind: 'dim', text: '  postgres://app@127.0.0.1:5442/shop_staging' },
    { text: ' ' },
    { kind: 'prompt', text: 'sloop db list' },
    {
      kind: 'name',
      tag: 'shop    ',
      text: '  postgres://app@127.0.0.1:5442/shop_staging     ',
      note: '  project · the OS keyring',
    },
    {
      kind: 'name',
      tag: 'accounts',
      text: '  postgres://app@127.0.0.1:5442/ledger           ',
      note: '  global · the OS keyring',
    },
    {
      kind: 'name',
      tag: 'shop    ',
      text: '  postgres://app@127.0.0.1:5442/shop             ',
      note: '  global, shadowed · the OS keyring',
    },
    { text: ' ' },
    {
      kind: 'dim',
      text: `A bare name is looked for in this project, then the global store at ${this.store()}.`,
    },
  ]);

  /** Which one a bare `shop` reaches, and how to reach the other. */
  protected readonly qualified: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop db test shop' },
    { text: ' ' },
    {
      kind: 'name',
      tag: 'shop',
      text: '  postgres://app@127.0.0.1:5442/shop_staging',
      note: '  project',
    },
    { kind: 'step', mark: 'ok', text: 'postgres 17.9', note: 'not encrypted' },
    { text: ' ' },
    { kind: 'prompt', text: 'sloop db test global:shop' },
    { text: ' ' },
    {
      kind: 'name',
      tag: 'global:shop',
      text: '  postgres://app@127.0.0.1:5442/shop',
      note: '  global',
    },
    { kind: 'step', mark: 'ok', text: 'postgres 17.9', note: 'not encrypted' },
  ];

  /** `-C` and `SLOOP_PROJECT`, from a directory that is not the project. */
  protected readonly fromElsewhere = computed<readonly TerminalLine[]>(() => [
    {
      kind: 'dim',
      text: '# standing anywhere else — neither of these needs the project directory',
    },
    { kind: 'prompt', text: 'sloop db list -C acme-api' },
    { kind: 'dim', text: '  … the project registry, exactly as above' },
    { text: ' ' },
    {
      kind: 'prompt',
      text:
        this.os() === 'windows'
          ? '$env:SLOOP_PROJECT = "acme-api"; sloop db list'
          : 'SLOOP_PROJECT=acme-api sloop db list',
    },
    { kind: 'dim', text: '  … the same thing. -C outranks it when both are given' },
  ]);

  /** Where sloop keeps its own state, which is where the registry actually is. */
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

  /**
   * The resolution order, as the CLI applies it.
   *
   * Five rungs and the first one that matches wins — which is the whole of the
   * rule, and the reason it is a numbered list here rather than a paragraph.
   * `Registries::resolve` in `cli/src/registry.rs` is the authority; the
   * `(…)` sentence `db add` prints names the rung that was taken, so a run says
   * which of these five it used rather than leaving it to be worked out.
   */
  protected readonly steps: readonly Step[] = [
    {
      how: '--global',
      rule: 'The global registry, full stop.',
      text: 'Nothing else is consulted — not a project you are standing in, not the variable.',
    },
    {
      how: '-C <path|name>',
      rule: 'That project.',
      text: 'A directory, or a name sloop init recorded. Works from anywhere on the machine.',
    },
    {
      how: 'SLOOP_PROJECT',
      rule: 'The same thing, from the environment.',
      text: '-C outranks it when both are given, so a flag on the line always beats the shell it ran in.',
    },
    {
      how: '.sloop',
      rule: 'The nearest one at or above the working directory.',
      text: 'The way git finds .git — walk up until a .sloop turns up, and stop at the first.',
    },
    {
      how: '',
      rule: 'Otherwise, the global registry.',
      text: 'Which is where a machine with no projects on it lives all the time.',
    },
  ];
}
