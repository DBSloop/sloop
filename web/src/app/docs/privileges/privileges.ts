import { ChangeDetectionStrategy, Component, computed, signal } from '@angular/core';
import { RouterLink } from '@angular/router';

import { DocsPage } from '../docs-page';
import { Terminal, type TerminalLine } from '../../ui/terminal';
import table from '../../../../privileges.json';

/** When a requirement applies: always, or only if the database has a thing. */
interface Applies {
  readonly kind: 'always' | 'only_with';
  readonly subject?: string;
}

/** One thing a role has to be able to do. */
interface Requirement {
  readonly id: string;
  readonly title: string;
  readonly privileges: readonly string[];
  readonly consequence: string;
  /**
   * Which of `doctor`'s questions this row answers.
   *
   * **A string, not a union of the values that exist today**, and that is this
   * page's one scar. It was typed `'dump' | 'restore'` and rendered by two
   * hand-written lists, so when `R30` added a third phase the row for it
   * matched neither filter and left the page in silence — no type error, no
   * build failure, nothing to notice. Grouping is derived from the data now:
   * a phase nobody has heard of still gets a section of its own.
   */
  readonly phase: string;
  readonly applies: Applies;
  readonly grant: string;
}

/** A section of the table: one of `doctor`'s questions, and what to call it. */
interface Phase {
  /** The value the generated file carries. */
  readonly key: string;
  /** The anchor, which outlives any rewording of the heading. */
  readonly id: string;
  /** What this page calls it. */
  readonly heading: string;
  /** The sentence under it. Empty for a phase this page has not met. */
  readonly lede: string;
}

/** A privilege the engine's own manual asks for and sloop does not need. */
interface Waived {
  readonly privilege: string;
  readonly because: string;
}

interface Engine {
  readonly engine: 'postgres' | 'mysql' | 'mariadb';
  readonly requires: readonly Requirement[];
  readonly waives: readonly Waived[];
}

const LABEL: Readonly<Record<string, string>> = {
  postgres: 'PostgreSQL',
  mysql: 'MySQL',
  mariadb: 'MariaDB',
};

/**
 * `doctor`'s three questions, in the order it asks them.
 *
 * `cli/src/engine/privileges.rs` — the `Phase` enum, and the `PHASES` array
 * beside it that the report walks. The headings are this page's wording of
 * `Phase::heading`, which prints *to back this database up* inside a sentence
 * and wants a title here.
 *
 * **Naming a phase here is optional and never load-bearing.** This list decides
 * the order sections come in and the words above them; it does not decide which
 * sections exist. That comes from the file — see `groups`.
 */
const PHASES: readonly Phase[] = [
  {
    key: 'dump',
    id: 'to-back-up',
    heading: 'To back a database up',
    lede: 'Everything backup needs, and the source half of mirror and sync.',
  },
  {
    key: 'restore',
    id: 'to-restore',
    heading: 'To restore into one',
    lede: 'A role that dumps a database perfectly may be unable to restore one. Different job, different grants.',
  },
  {
    key: 'drop',
    id: 'to-drop',
    heading: 'To delete it from the server',
    lede: 'Ownership, not a grant — and the one question doctor used to leave unasked, until a drop was refused by a role it had just passed.',
  },
];

/**
 * The privileges a backup role needs.
 *
 * **Every row on this page is read from `web/privileges.json` at build time and
 * not one of them is typed here.** That file is written by
 * `engine::privileges::tests::the_site_publishes_exactly_what_the_code_checks`
 * from the same table `sloop doctor` checks a live connection against, so the
 * page, the check and the `GRANT` a reader copies cannot drift apart. Editing
 * the Rust table changes this page; editing this page changes nothing.
 *
 * That is the whole of `A25`: the page was typing the phase as two values and
 * filtering into two lists, so the row `R30` added landed in neither and left
 * the page with the build still green. Nothing groups by a name typed here now.
 *
 * The one thing chosen here rather than read is **which rows lead**: the
 * requirements whose `consequence` says the dump program exits `0` and says
 * nothing. They are found by looking for that in the data rather than by
 * listing their ids, and even the count in the prose is derived — so a third
 * silent failure added to the Rust table arrives at the top of this page, with
 * the sentence above it already correct.
 *
 * ## The two captures
 *
 * Both are real runs on 2026-09-19 against a throwaway PostgreSQL 17.9 on port
 * 5450 with two deliberately under-privileged roles: `doctor` exiting `8` and
 * naming every missing grant, then — after running exactly the `GRANT` lines it
 * printed and nothing else — the same command exiting `0`.
 *
 * **The third phase's rows in them are derived, not captured**, because that run
 * predates `R30` by a day. Every glyph, column, colour and sentence in those
 * rows is read out of the code that prints them — `doctor::print_one` for the
 * shape, `privileges.rs` for the title, the consequence and the `ALTER`, and
 * `postgres.rs`'s `'owned by '||pg_get_userbyid(d.datdba)` for the detail line.
 * That is the owner's standing answer to a block that cannot be photographed:
 * *"rust is built by you, and every message/text is declared in the code, so you
 * know what should be an output for exact command"* — see "A15: one block on
 * this site is not a capture" in `docs/OWNER-DECISIONS.md`.
 *
 * **And the second capture could not simply turn that row green.** A database
 * has exactly one owner, so two roles cannot both be ready to drop it; the run
 * gives ownership to one of them and the other still reports the gap. It exits
 * `0` all the same, because `Report::can_dump` is what reaches the exit code —
 * only the first question does.
 */
@Component({
  selector: 'app-docs-privileges',
  imports: [DocsPage, RouterLink, Terminal],
  templateUrl: './privileges.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Privileges {
  private readonly all = table as readonly Engine[];

  /** Which engine's table is showing. Not the OS strip — this is the database's. */
  protected readonly chosen = signal<Engine['engine']>('postgres');

  protected readonly engines = this.all.map((engine) => ({
    key: engine.engine,
    label: LABEL[engine.engine] ?? engine.engine,
  }));

  protected readonly current = computed(
    () => this.all.find((engine) => engine.engine === this.chosen()) ?? this.all[0],
  );

  /**
   * Every row of the chosen engine, in a section per phase.
   *
   * **The sections come from the rows, not from a list kept here.** Two
   * hand-written filters — one for `dump`, one for `restore` — is what made a
   * `drop` row disappear off this page the day `R30` added it, and a page that
   * silently drops a row from a generated file is worse than no page. So the
   * phases present in the data are collected, `PHASES` puts the ones it knows
   * in the order `doctor` asks them, and anything it does not know follows with
   * a heading built from its own name. Every row is in exactly one section, and
   * `site.mjs` fails the build if one of them is not on the built page.
   */
  protected readonly groups = computed(() => {
    const rows = this.current().requires;
    const present = [...new Set(rows.map((one) => one.phase))];
    const known = PHASES.filter((phase) => present.includes(phase.key));
    const strangers: Phase[] = present
      .filter((key) => !PHASES.some((phase) => phase.key === key))
      .map((key) => ({ key, id: `to-${key}`, heading: `To ${key} one`, lede: '' }));

    return [...known, ...strangers].map((phase) => ({
      ...phase,
      rows: rows.filter((one) => one.phase === phase.key),
    }));
  });

  protected readonly waives = computed(() => this.current().waives);

  protected label(engine: string): string {
    return LABEL[engine] ?? engine;
  }

  /**
   * The failures that say nothing, found in the data rather than listed.
   *
   * A requirement whose consequence mentions exiting zero *and* saying nothing
   * is a silent one. Nothing here names an id, so a third silent failure added
   * to the Rust table arrives at the top of this page by itself.
   *
   * **Grouped by what goes wrong, then by the grant that fixes it**, and the
   * second half is not tidiness. MySQL and MariaDB lose triggers for the same
   * reason and take the same `GRANT TRIGGER`, so that is one card naming both.
   * They lose stored routines for the same reason and take *different* grants —
   * `SHOW_ROUTINE` against `SHOW CREATE ROUTINE` — so those are two cards, each
   * naming the engine it belongs to. Keying on the id alone got this wrong and
   * produced a card headed *MariaDB and MariaDB*.
   */
  private readonly silent = this.all.flatMap((engine) =>
    engine.requires
      .filter(
        (one) => /exits? 0/.test(one.consequence) && /says so|say nothing/.test(one.consequence),
      )
      .map((one) => ({ engine: LABEL[engine.engine] ?? engine.engine, ...one })),
  );

  protected readonly silentGroups = (() => {
    const groups: {
      title: string;
      consequence: string;
      grant: string;
      engines: string[];
    }[] = [];
    for (const one of this.silent) {
      const already = groups.find(
        (group) => group.title === one.title && group.grant === one.grant,
      );
      if (already) {
        if (!already.engines.includes(one.engine)) {
          already.engines.push(one.engine);
        }
        continue;
      }
      groups.push({
        title: one.title,
        consequence: one.consequence,
        grant: one.grant,
        engines: [one.engine],
      });
    }
    return groups;
  })();

  /** `MySQL and MariaDB`, or just `MariaDB`. */
  protected engineList(engines: readonly string[]): string {
    return engines.length > 1
      ? `${engines.slice(0, -1).join(', ')} and ${engines[engines.length - 1]}`
      : (engines[0] ?? '');
  }

  /**
   * How many silent failures there are, in words, because the sentence around
   * it reads as a sentence and because the number comes from the data.
   */
  protected readonly howManySilent = (() => {
    const words = ['None', 'One', 'Two', 'Three', 'Four', 'Five', 'Six'];
    return words[this.silentGroups.length] ?? String(this.silentGroups.length);
  })();

  /** *are* for several, *is* for one — so the derived count still reads. */
  protected readonly silentVerb = this.silentGroups.length === 1 ? 'is' : 'are';

  protected howMany(applies: Applies): string {
    return applies.kind === 'always' ? 'always' : `only with ${applies.subject}`;
  }

  // ── The two captures ──────────────────────────────────────────────────────

  protected readonly failing: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop doctor' },
    { kind: 'head', tag: 'Privileges', text: '' },
    {
      kind: 'dim',
      text: '  Every engine wants more than SELECT, and not every gap is loud: short of one grant,',
    },
    {
      kind: 'dim',
      text: '  MySQL and MariaDB write a dump with your triggers and stored routines left out of it,',
    },
    {
      kind: 'dim',
      text: '  say nothing, and exit 0. So sloop asks each registered connection what its role can',
    },
    { kind: 'dim', text: '  really do, rather than finding out during a backup.' },
    { text: ' ' },
    {
      kind: 'name',
      tag: '  weak',
      text: '',
      note: '  postgres://weak@127.0.0.1:5450/shop',
    },
    { kind: 'dim', text: '    postgres 17.9, connected as weak' },
    { kind: 'dim', text: '    — to back this database up: 1 of 4 missing' },
    { kind: 'name', tag: '      ✗', text: ' read every table, view and sequence' },
    {
      kind: 'dim',
      text: '          2 of 2 cannot be read: public.customers, public.customers_id_seq',
    },
    {
      kind: 'dim',
      text: '          pg_dump stops on the first table it cannot lock, and the dump is not written',
    },
    { kind: 'label', tag: '      grant:', text: '' },
    { text: '        GRANT pg_read_all_data TO weak;' },
    { kind: 'dim', text: '    — to restore into it: 2 of 2 missing' },
    { kind: 'name', tag: '      ✗', text: ' create a schema' },
    { kind: 'name', tag: '      ✗', text: ' enter and create in every schema' },
    { kind: 'dim', text: '          cannot enter or create in: public' },
    { kind: 'label', tag: '      grant:', text: '' },
    { text: '        GRANT CREATE ON DATABASE shop TO weak;' },
    { text: '        GRANT USAGE, CREATE ON SCHEMA public TO weak;' },
    { kind: 'dim', text: '    — to delete it from the server: 1 of 1 missing' },
    { kind: 'name', tag: '      ✗', text: ' own the database' },
    { kind: 'dim', text: '          owned by postgres' },
    {
      kind: 'dim',
      text: '          `sloop db drop` is refused with `must be owner of database`, after everything',
    },
    { kind: 'dim', text: '          else about the run has already worked' },
    { kind: 'label', tag: '      grant:', text: '' },
    { text: '        ALTER DATABASE shop OWNER TO weak;' },
    { text: ' ' },
    { kind: 'prompt', text: 'echo exit $?' },
    { text: 'exit 8' },
  ];

  protected readonly fixed: readonly TerminalLine[] = [
    { kind: 'dim', text: '# exactly the GRANT lines it printed, and nothing else' },
    { kind: 'prompt', text: 'sloop doctor' },
    { kind: 'head', tag: 'Privileges', text: '' },
    { kind: 'dim', text: '  …' },
    { text: ' ' },
    {
      kind: 'name',
      tag: '  strong',
      text: '',
      note: '  postgres://strong@127.0.0.1:5450/shop',
    },
    { kind: 'dim', text: '    postgres 17.9, connected as strong' },
    { kind: 'dim', text: '    — ready to back this database up' },
    { kind: 'dim', text: '    — ready to restore into it' },
    { kind: 'dim', text: '    — ready to delete it from the server' },
    { text: ' ' },
    {
      kind: 'name',
      tag: '  weak',
      text: '',
      note: '  postgres://weak@127.0.0.1:5450/shop',
    },
    { kind: 'dim', text: '    postgres 17.9, connected as weak' },
    { kind: 'dim', text: '    — ready to back this database up' },
    { kind: 'dim', text: '    — ready to restore into it' },
    { kind: 'dim', text: '    — to delete it from the server: 1 of 1 missing' },
    { kind: 'name', tag: '      ✗', text: ' own the database' },
    { kind: 'dim', text: '          owned by strong' },
    {
      kind: 'dim',
      text: '          `sloop db drop` is refused with `must be owner of database`, after everything',
    },
    { kind: 'dim', text: '          else about the run has already worked' },
    { kind: 'label', tag: '      grant:', text: '' },
    { text: '        ALTER DATABASE shop OWNER TO weak;' },
    { text: ' ' },
    { kind: 'prompt', text: 'echo exit $?' },
    { text: 'exit 0' },
  ];
}
