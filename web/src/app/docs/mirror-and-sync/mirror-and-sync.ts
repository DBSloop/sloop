import { ChangeDetectionStrategy, Component, computed, inject } from '@angular/core';
import { RouterLink } from '@angular/router';

import { DocsPage } from '../docs-page';
import { OS_LABEL, OsChoice } from '../os';
import { Terminal, type TerminalLine } from '../../ui/terminal';

/** One of the two commands, for the panel that has to be read before anything else. */
interface Command {
  readonly name: string;
  readonly verb: string;
  readonly ends: string;
  readonly destinationOnly: string;
  readonly reach: string;
  readonly use: string;
}

/** One of `sync`'s rules, stated as a rule. */
interface Rule {
  readonly rule: string;
  readonly detail: string;
}

/**
 * Mirror and sync.
 *
 * ## How these blocks were made
 *
 * One throwaway PostgreSQL 17.9 cluster on port 5445, on 2026-09-19, with the
 * binary built from this commit: a `live` source of 10,841 rows across three
 * tables, and a `staging` destination seeded with **different** data — 600 stale
 * customers, 400 stale orders, and one row that exists nowhere else. Every
 * command below was run against them, and the destination was reset between
 * the `mirror` run and the `sync` run so both are shown doing their real work
 * on the same starting state.
 *
 * **The destination-only row is the whole demonstration.** `mirror` removes it
 * and `sync` keeps it, and the two blocks say so in sloop's own words rather
 * than in mine — *replacing what the destination has now* against *1 kept that
 * the source has no row for*.
 *
 * `audit_log` has no primary key **on purpose**, so the `sync` block shows the
 * skip happening rather than describing it. The circular foreign keys are a
 * second database built for the refusal: `teams.lead_id → people.id` and
 * `people.team_id → teams.id`.
 *
 * ## The substitutions, named rather than hidden
 *
 * One: **the temporary path in the `--safe` block** is written as this
 * platform's temp directory rather than the author's. Everything else — the row
 * counts, the sizes, the durations, the sequence names, every word of every
 * message — is what the runs printed.
 *
 * ## The cross-engine refusal was reworded before this page shipped
 *
 * It used to end *"R29 is where copying across engines gets decided"* — `R29`
 * being an entry in the project's own Rust build list and meaning nothing to a
 * reader. Raised in the `A12` report and fixed in `cli/` the same day, in
 * `mirror`, `sync` and `restore`, which all three printed it. The block below is
 * the reworded one, captured from a binary built after the change.
 */
@Component({
  selector: 'app-docs-mirror-and-sync',
  imports: [DocsPage, RouterLink, Terminal],
  templateUrl: './mirror-and-sync.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class MirrorAndSync {
  private readonly choice = inject(OsChoice);

  protected readonly os = this.choice.os;
  protected readonly osLabel = computed(() => OS_LABEL[this.os()]);

  /** Where `--safe` puts the dump it takes on the way past. */
  protected readonly tempDump = computed(() => {
    switch (this.os()) {
      case 'windows':
        return 'C:\\Users\\you\\AppData\\Local\\Temp\\sloop-mirror-23020-20260919T143745Z\\dump';
      case 'macos':
        return '/var/folders/sloop-mirror-23020-20260919T143745Z/dump';
      default:
        return '/tmp/sloop-mirror-23020-20260919T143745Z/dump';
    }
  });

  // ── The difference ────────────────────────────────────────────────────────

  protected readonly commands: readonly Command[] = [
    {
      name: 'mirror',
      verb: 'Replaces',
      ends: 'The destination ends up identical to the source.',
      destinationOnly:
        'Gone. Anything that existed only in the destination is removed with everything else.',
      reach: 'Dump and restore. The destination is cleared and rewritten.',
      use: 'Refreshing a staging copy from production. You want the two to match and you do not care what staging had.',
    },
    {
      name: 'sync',
      verb: 'Merges',
      ends: 'The destination ends up holding both — the source’s rows and its own.',
      destinationOnly:
        'Kept, and counted. sloop tells you how many rows it left that the source has no row for.',
      reach: 'COPY into temporary tables, then upsert in foreign-key order, parents first.',
      use: 'Topping up a database that has rows of its own worth keeping, or bringing in new rows without disturbing local ones.',
    },
  ];

  // ── mirror ────────────────────────────────────────────────────────────────

  protected readonly mirrorRefusal: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop mirror live --to staging   # from a scheduled run' },
    {
      kind: 'bad',
      tag: 'error:',
      text: " replacing a database's contents needs its name typed, and there is no terminal to type at",
    },
    { kind: 'dim', text: '  hint: pass --confirm staging to say it up front' },
  ];

  protected readonly mirrorDry: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop mirror live --to staging --dry-run' },
    {
      kind: 'name',
      tag: 'mirroring',
      text: ' postgres://app@127.0.0.1:5445/live → postgres://app@127.0.0.1:5445/staging',
    },
    { kind: 'dim', text: '  postgres 17.9 → postgres 17.9' },
    { kind: 'dim', text: '  copying 3 tables, 10841 rows' },
    { kind: 'dim', text: '  replacing what the destination has now — 3 tables, 1001 rows' },
    {
      kind: 'dim',
      text: 'dry run would replace the contents of postgres://app@127.0.0.1:5445/staging — nothing was changed',
    },
  ];

  protected readonly mirrorReal: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop mirror live --to staging' },
    {
      kind: 'name',
      tag: 'mirroring',
      text: ' postgres://app@127.0.0.1:5445/live → postgres://app@127.0.0.1:5445/staging',
    },
    { kind: 'dim', text: '  postgres 17.9 → postgres 17.9' },
    { kind: 'dim', text: '  copying 3 tables, 10841 rows' },
    { kind: 'dim', text: '  replacing what the destination has now — 3 tables, 1001 rows' },
    {
      kind: 'name',
      tag: '?',
      text: ' type staging to destroy it, or anything else to stop: staging',
    },
    { kind: 'dim', text: '  cleared 3 tables' },
    { kind: 'dim', text: '  copied in 0.3s' },
    { kind: 'step', mark: 'ok', text: 'Verified', note: 'exact count(*) on both sides' },
    { kind: 'dim', text: '  verifying — exact count(*) on both sides' },
    { kind: 'dim', text: '  public.audit_log   240 → 240' },
    { kind: 'dim', text: '  public.customers  1284 → 1284' },
    { kind: 'dim', text: '  public.orders     9317 → 9317' },
    { kind: 'dim', text: '  3 of 3 tables matched' },
  ];

  // ── sync ──────────────────────────────────────────────────────────────────

  /** Every one of `sync`'s rules, visible in one run. */
  protected readonly syncReal: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop sync live --to staging --confirm staging' },
    {
      kind: 'name',
      tag: 'syncing',
      text: ' postgres://app@127.0.0.1:5445/live → postgres://app@127.0.0.1:5445/staging',
    },
    { kind: 'dim', text: '  postgres 17.9 → postgres 17.9' },
    { kind: 'dim', text: '  merging 2 tables, parents first' },
    {
      kind: 'dim',
      text: '  skipping public.audit_log — it has no primary key, so there is no way to tell which row is which',
    },
    { kind: 'dim', text: '  the destination has 3 tables' },
    {
      kind: 'dim',
      text: '  public.customers  1284 in, 684 new, 600 replaced, 1 kept that the source has no row for',
    },
    { kind: 'dim', text: '  public.orders  9317 in, 8917 new, 400 replaced' },
    { kind: 'dim', text: '  merged in 0.3s' },
    { kind: 'dim', text: '  public.customers_id_seq now hands out 99002 next' },
    { kind: 'dim', text: '  public.orders_id_seq now hands out 9318 next' },
    { kind: 'dim', text: '  2 tables merged: 9601 new, 1000 replaced, 1 kept' },
  ];

  protected readonly rules: readonly Rule[] = [
    {
      rule: 'A primary key per table, or the table is skipped and named',
      detail:
        'A merge has to know which destination row a source row replaces, and without a key there is no answer. sloop skips that table, says which one and why, and carries on with the rest rather than refusing the whole run.',
    },
    {
      rule: 'Parents first, in foreign-key order',
      detail:
        'Rows are copied into temporary tables and then upserted in dependency order, so a child row never arrives before the row it points at.',
    },
    {
      rule: 'Sequences and identity columns are reset afterwards',
      detail:
        'A merged table whose sequence still points at the destination’s old high-water mark hands out an id that already exists. sloop moves each one past the highest value in the table and prints where it now stands.',
    },
    {
      rule: 'Rows the source has deleted are kept, and counted',
      detail:
        'A merge adds and replaces; it does not delete. Rows that exist only in the destination stay, and sloop says how many — so a row that lingers is a row you were told about rather than one you find later.',
    },
    {
      rule: 'Circular foreign keys are refused outright',
      detail:
        'Two tables pointing at each other have no load order at all. sloop refuses the run rather than loading half of it and leaving the rest to fail.',
    },
  ];

  protected readonly circular: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop sync loop_demo --to loop_dest --confirm loop_dest' },
    {
      kind: 'name',
      tag: 'syncing',
      text: ' postgres://app@127.0.0.1:5445/loop_demo → postgres://app@127.0.0.1:5445/loop_dest',
    },
    { kind: 'dim', text: '  postgres 17.9 → postgres 17.9' },
    {
      kind: 'bad',
      tag: 'error:',
      text: " these tables' foreign keys point at each other, so there is no order that loads",
    },
    { kind: 'bad', tag: '', text: '  them: public.people, public.teams' },
    {
      kind: 'dim',
      text: '  hint: sync refuses rather than loading half of them. `sloop mirror` replaces the',
    },
    { kind: 'dim', text: '  destination outright and has no such ordering to get right' },
  ];

  // ── Scoping ───────────────────────────────────────────────────────────────

  protected readonly tableAlone: readonly TerminalLine[] = [
    {
      kind: 'prompt',
      text: 'sloop mirror live --to staging --table public.orders --confirm staging',
    },
    {
      kind: 'name',
      tag: 'mirroring',
      text: ' postgres://app@127.0.0.1:5445/live → postgres://app@127.0.0.1:5445/staging',
    },
    {
      kind: 'bad',
      tag: 'error:',
      text: ' 1 table is named, and it points at 1 table: public.customers',
    },
    {
      kind: 'dim',
      text: '  hint: rows cannot be loaded into a table whose parents are not there. Name them too,',
    },
    { kind: 'dim', text: '  or pass --with-references to pull them in' },
  ];

  protected readonly withReferences: readonly TerminalLine[] = [
    {
      kind: 'prompt',
      text: 'sloop mirror live --to staging --table public.orders --with-references \\',
    },
    { kind: 'prompt', text: '    --confirm staging' },
    {
      kind: 'name',
      tag: 'mirroring',
      text: ' postgres://app@127.0.0.1:5445/live → postgres://app@127.0.0.1:5445/staging',
    },
    { kind: 'dim', text: '  only 2 tables: public.customers, public.orders' },
    {
      kind: 'dim',
      text: '  those tables are dropped and recreated; everything else in the destination is left',
    },
    { kind: 'dim', text: '  exactly as it is, so it will not end up identical to the source' },
    { kind: 'dim', text: '  postgres 17.9 → postgres 17.9' },
    { kind: 'dim', text: '  copying 2 tables, 10601 rows' },
    { kind: 'dim', text: '  cleared 2 tables' },
    { kind: 'dim', text: '  copied in 0.4s' },
    { kind: 'dim', text: '  2 of 2 tables matched' },
  ];

  protected readonly noMatch: readonly TerminalLine[] = [
    {
      kind: 'prompt',
      text: 'sloop mirror live --to staging --table public.nope --confirm staging',
    },
    { kind: 'bad', tag: 'error:', text: ' --table public.nope matches no table in the source' },
    {
      kind: 'dim',
      text: '  hint: a name, or a glob like `audit_*`, optionally with a schema in front.',
    },
    { kind: 'dim', text: '  `sloop db test <name>` says what is there' },
  ];

  protected readonly safe = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop mirror live --to staging --safe --confirm staging' },
    {
      kind: 'name',
      tag: 'mirroring',
      text: ' postgres://app@127.0.0.1:5445/live → postgres://app@127.0.0.1:5445/staging',
    },
    { kind: 'dim', text: '  copying 3 tables, 10841 rows' },
    { kind: 'dim', text: '  cleared 3 tables' },
    {
      kind: 'dim',
      text: `--safe: dumping to ${this.tempDump()} first — it is not encrypted,`,
    },
    { kind: 'dim', text: '  and it goes when the counts agree' },
    { kind: 'dim', text: '  dumped 66.7 KiB' },
    { kind: 'dim', text: '  copied in 0.8s' },
    { kind: 'dim', text: '  3 of 3 tables matched' },
  ]);

  // ── Making the destination ────────────────────────────────────────────────

  protected readonly create: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop mirror live --create rehearsal \\' },
    {
      kind: 'prompt',
      text: '    --superuser postgres --superuser-password-command "op read op://vault/pg/root" \\',
    },
    { kind: 'prompt', text: '    --role-password-stdin' },
    {
      kind: 'name',
      tag: 'creating',
      text: '',
      note: ' postgres://rehearsal@127.0.0.1:5445/rehearsal',
    },
    { kind: 'dim', text: '  as postgres, whose password is used once and kept nowhere' },
    { kind: 'name', tag: 'created rehearsal', text: '' },
    { kind: 'dim', text: '  role rehearsal created' },
    { kind: 'dim', text: '  granted USAGE, CREATE ON SCHEMA public TO rehearsal' },
    { kind: 'dim', text: '  registered as rehearsal in the project registry' },
    {
      kind: 'name',
      tag: 'mirroring',
      text: ' postgres://app@127.0.0.1:5445/live → postgres://rehearsal@127.0.0.1:5445/rehearsal',
    },
    { kind: 'dim', text: '  copying 3 tables, 10841 rows' },
    { kind: 'dim', text: '  the destination is empty' },
    { kind: 'dim', text: '  copied in 0.3s' },
    { kind: 'dim', text: '  3 of 3 tables matched' },
  ];

  // ── The two refusals that protect you ─────────────────────────────────────

  protected readonly sameDatabase: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop mirror live --to live --confirm live' },
    {
      kind: 'bad',
      tag: 'error:',
      text: ' live and live are the same database — postgres://app@127.0.0.1:5445/live',
    },
    {
      kind: 'dim',
      text: "  hint: a mirror drops the destination's contents, so this would destroy the source",
    },
  ];

  protected readonly crossEngine: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop mirror live --to mysql_side --confirm sessions' },
    { kind: 'bad', tag: 'error:', text: ' live is postgres and mysql_side is mysql' },
    {
      kind: 'dim',
      text: '  hint: copying between engines is not built: the type systems do not map cleanly, and a',
    },
    {
      kind: 'dim',
      text: '  copy that quietly rounds or truncates is worse than one that refuses. Mirror between two',
    },
    { kind: 'dim', text: '  databases on the same engine' },
  ];
}
