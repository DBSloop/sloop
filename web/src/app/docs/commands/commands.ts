import { ChangeDetectionStrategy, Component } from '@angular/core';
import { RouterLink } from '@angular/router';

import { DocsPage } from '../docs-page';
import reference from './commands.json';

/** One flag, as `sloop <command> -h` printed it. */
interface Flag {
  readonly short: string | null;
  readonly long: string | null;
  readonly value: string | null;
  readonly spec: string;
  readonly text: string;
}

interface Command {
  readonly path: readonly string[];
  readonly name: string;
  readonly summary: string;
  readonly description: readonly string[];
  readonly usage: string;
  readonly arguments: readonly { readonly name: string; readonly text: string }[];
  readonly ownOptions: readonly Flag[];
  readonly children: readonly string[];
}

/** A run of text, split so the template can render it without `innerHTML`. */
interface Piece {
  readonly text: string;
  readonly kind: 'plain' | 'code' | 'strong';
}

/**
 * The eight groups, and the top-level commands in each.
 *
 * **This is the one editorial thing on the page, and it is deliberately not
 * generated**, because `--help` has no opinion about which commands belong
 * together — `sloop help` lists fifteen of them in declaration order. The
 * groups are the CLI menu's own headings, so the reference is arranged the way
 * somebody who learned the tool from the menu already thinks.
 *
 * A top-level command that is not named here still appears, in *Everything
 * else* at the bottom. That fallback is the point: a command added to the CLI
 * and forgotten here shows up looking out of place, which is noticeable, rather
 * than silently vanishing from the reference, which is not.
 */
const GROUPS: readonly { id: string; title: string; line: string; members: readonly string[] }[] = [
  {
    id: 'getting-set-up',
    title: 'Getting set up',
    line: 'Preparing a machine, and a project registry in a directory.',
    members: ['setup', 'init'],
  },
  {
    id: 'databases',
    title: 'Databases',
    line: 'Telling sloop about a database, and changing or forgetting what it knows.',
    members: ['db'],
  },
  {
    id: 'backups',
    title: 'Backups',
    line: 'Taking a copy, seeing what is stored, pruning it, and putting one back.',
    members: ['backup', 'backups', 'restore'],
  },
  {
    id: 'copying',
    title: 'Copying',
    line: 'Two commands that are not the same command.',
    members: ['mirror', 'sync'],
  },
  {
    id: 'reading',
    title: 'Reading',
    line: 'Looking inside a database without writing SQL.',
    members: ['query'],
  },
  {
    id: 'the-backup-key',
    title: 'The backup key',
    line: 'Moving the key that every encrypted backup needs between machines.',
    members: ['key'],
  },
  {
    id: 'the-background-service',
    title: 'The background service',
    line: 'Registering sloop with the machine, and what it does once it is there.',
    members: ['service'],
  },
  {
    id: 'this-machine',
    title: 'This machine',
    line: 'The servers sloop installed, what it can check, and taking it all off.',
    members: ['server', 'doctor', 'reset', 'uninstall'],
  },
];

/**
 * The command reference — **generated, not written.**
 *
 * Every command, every argument and every flag on this page comes out of
 * `commands.json`, which `web/tools/commands.mjs` writes by walking the binary's
 * own `--help`. `A8`'s bar is that *a diff against the real `--help` output
 * finds nothing missing*, and that is held by construction here rather than by
 * proofreading: a flag cannot be on this page unless the CLI printed it, and
 * cannot be absent from it if the CLI printed it.
 *
 * So **nothing on this page is edited when the CLI changes.** The generator is
 * re-run, the JSON changes, and the page follows. `node tools/commands.mjs
 * --check` fails when the two have parted company, which is the half that keeps
 * it true once nobody is looking; `A22` owns wiring that into CI.
 *
 * Two things here are not generated, and both are marked where they appear: the
 * eight groups above, and the short prose introducing the global flags. Those
 * are judgements about arrangement, which `--help` does not have and should not.
 *
 * The descriptions arrive with the CLI's own light markup in them — backticks
 * around code, asterisks around emphasis. `pieces()` splits that into segments
 * the template renders as real elements, so nothing goes through `innerHTML`.
 */
@Component({
  selector: 'app-docs-commands',
  imports: [DocsPage, RouterLink],
  templateUrl: './commands.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Commands {
  protected readonly version = reference.version;
  protected readonly globalOptions = reference.globalOptions as readonly Flag[];

  private readonly all = reference.commands as readonly Command[];

  /** Every command, in its group, with the ones nobody grouped at the end. */
  protected readonly groups = (() => {
    const real = this.all.filter((command) => command.path.length > 0);
    const taken = new Set<string>();

    const grouped = GROUPS.map((group) => {
      const members = group.members.flatMap((top) => {
        const found = real.filter((command) => command.path[0] === top);
        for (const command of found) taken.add(command.name);
        return found;
      });
      return { ...group, members };
    });

    const rest = real.filter((command) => !taken.has(command.name));
    return rest.length
      ? [
          ...grouped,
          {
            id: 'everything-else',
            title: 'Everything else',
            line: 'Commands the CLI has that this page has not been told where to put.',
            members: rest,
          },
        ]
      : grouped;
  })();

  protected readonly count = this.all.filter((command) => command.path.length > 0).length;

  /** `sloop db add` → `db-add`, which is what the index links to. */
  protected id(command: Command): string {
    return command.path.join('-');
  }

  /** How a flag is written in a table cell, short form first. */
  protected spec(flag: Flag): string {
    const names = [flag.short, flag.long].filter(Boolean).join(', ');
    return flag.value ? `${names} <${flag.value}>` : names;
  }

  /**
   * Split the CLI's own light markup into renderable pieces.
   *
   * The descriptions are written for a terminal and carry `` `code` `` and
   * `**emphasis**`. Rendering them raw would show the punctuation; rendering
   * them through `innerHTML` would be a habit worth not having on a site that
   * will later read generated files. This returns segments instead, and the
   * template gives each one an element.
   */
  protected pieces(text: string): readonly Piece[] {
    const out: Piece[] = [];
    const pattern = /`([^`]+)`|\*\*([^*]+)\*\*/g;
    let at = 0;
    let match: RegExpExecArray | null;
    while ((match = pattern.exec(text)) !== null) {
      if (match.index > at) {
        out.push({ text: text.slice(at, match.index), kind: 'plain' });
      }
      out.push(
        match[1] !== undefined
          ? { text: match[1], kind: 'code' }
          : { text: match[2] ?? '', kind: 'strong' },
      );
      at = match.index + match[0].length;
    }
    if (at < text.length) {
      out.push({ text: text.slice(at), kind: 'plain' });
    }
    return out;
  }
}
