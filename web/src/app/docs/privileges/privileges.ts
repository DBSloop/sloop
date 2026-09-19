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
  readonly phase: 'dump' | 'restore';
  readonly applies: Applies;
  readonly grant: string;
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
 * The privileges a backup role needs.
 *
 * **Every row on this page is read from `web/privileges.json` at build time and
 * not one of them is typed here.** That file is written by
 * `engine::privileges::tests::the_site_publishes_exactly_what_the_code_checks`
 * from the same table `sloop doctor` checks a live connection against, so the
 * page, the check and the `GRANT` a reader copies cannot drift apart. Editing
 * the Rust table changes this page; editing this page changes nothing.
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

  protected readonly toBackUp = computed(() =>
    this.current().requires.filter((one) => one.phase === 'dump'),
  );

  protected readonly toRestore = computed(() =>
    this.current().requires.filter((one) => one.phase === 'restore'),
  );

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
      text: '  postgres://weak@127.0.0.1:5450/shop',
    },
    { kind: 'dim', text: '    postgres 17.9, connected as weak' },
    { kind: 'warn', tag: '    — to back this database up: 1 of 4 missing', text: '' },
    { kind: 'bad', tag: '      ✗ read every table, view and sequence', text: '' },
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
    { kind: 'warn', tag: '    — to restore into it: 2 of 2 missing', text: '' },
    { kind: 'bad', tag: '      ✗ create a schema', text: '' },
    { kind: 'bad', tag: '      ✗ enter and create in every schema', text: '' },
    { kind: 'dim', text: '          cannot enter or create in: public' },
    { kind: 'label', tag: '      grant:', text: '' },
    { text: '        GRANT CREATE ON DATABASE shop TO weak;' },
    { text: '        GRANT USAGE, CREATE ON SCHEMA public TO weak;' },
    { text: ' ' },
    { kind: 'prompt', text: 'echo exit $?' },
    { kind: 'warn', tag: 'exit 8', text: '' },
  ];

  protected readonly fixed: readonly TerminalLine[] = [
    { kind: 'dim', text: '# exactly the GRANT lines it printed, and nothing else' },
    { kind: 'prompt', text: 'sloop doctor' },
    { kind: 'head', tag: 'Privileges', text: '' },
    { text: ' ' },
    { kind: 'name', tag: '  strong', text: '  postgres://strong@127.0.0.1:5450/shop' },
    { kind: 'dim', text: '    postgres 17.9, connected as strong' },
    { kind: 'ok', tag: '    — ready to back this database up', text: '' },
    { kind: 'ok', tag: '    — ready to restore into it', text: '' },
    { text: ' ' },
    { kind: 'name', tag: '  weak', text: '  postgres://weak@127.0.0.1:5450/shop' },
    { kind: 'dim', text: '    postgres 17.9, connected as weak' },
    { kind: 'ok', tag: '    — ready to back this database up', text: '' },
    { kind: 'ok', tag: '    — ready to restore into it', text: '' },
    { text: ' ' },
    { kind: 'prompt', text: 'echo exit $?' },
    { kind: 'ok', tag: 'exit 0', text: '' },
  ];
}
