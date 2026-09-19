import { ChangeDetectionStrategy, Component } from '@angular/core';
import { RouterLink } from '@angular/router';

import { DocsPage } from '../docs-page';

/** One thing that is planned, and the thing that exists in its place today. */
interface Planned {
  readonly title: string;
  /** What it would be. Written as an intention, never as a commitment. */
  readonly what: string;
  /** What sloop does today that this would build on. Present tense, and true. */
  readonly today: string;
  /** Where on this site that today is documented. */
  readonly link: { readonly to: string; readonly label: string };
}

/** Something deliberately absent from the list above, and why. */
interface NotPlanned {
  readonly title: string;
  readonly why: readonly string[];
}

/**
 * Roadmap.
 *
 * ## There are no transcripts on this page, on purpose
 *
 * Every other page on this site leads with a captured run, because every other
 * page documents something that exists. This one documents four things that do
 * not, and a terminal block showing unbuilt software would be the single most
 * dishonest thing this site could publish. So the only blocks of evidence here
 * are links to the pages where the *existing* half is demonstrated.
 *
 * ## Where the four items come from
 *
 * `docs/TOTAL_FEATURE_LIST.md`, "What is not built", which is the owner's list
 * and not this session's. Rule 12: entries come from the owner, and that applies
 * to a roadmap more than to anything else — a documentation session inventing a
 * feature and publishing it as planned is how a promise nobody made ends up on a
 * website.
 *
 * The `today` line on each card is the part that has to be checked rather than
 * written, because it is the only sentence on the page making a claim about
 * software that exists. All four are from the feature list and are documented on
 * the pages they link to.
 *
 * ## Cross-engine migration
 *
 * On hold by the owner's decision, and the reasoning is `R29` in
 * `docs/OWNER-DECISIONS.md` — measured on this machine against PostgreSQL 17.9
 * and MySQL 8.4.11 rather than recalled. What that entry establishes, and what
 * this page says in fewer words: sloop has no type model and no value
 * representation, because every copy it performs is performed by the engines'
 * own client programs. Cross-engine, sloop becomes the thing moving the bytes,
 * and every guarantee that is currently free has to be re-earned.
 *
 * **The refusal is quoted rather than captured.** It is on the
 * [mirror and sync page](/docs/mirror-and-sync) as a real run, and repeating a
 * capture on a second page is how two pages start disagreeing. The words here
 * are `mirror.rs` and `sync.rs`'s own hint, which is the same text that block
 * shows.
 */
@Component({
  selector: 'app-docs-roadmap',
  imports: [DocsPage, RouterLink],
  templateUrl: './roadmap.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Roadmap {
  protected readonly planned: readonly Planned[] = [
    {
      title: 'Writing and saving queries',
      what: 'Writing a query rather than building one, and keeping it under a name to run again.',
      today:
        'sloop query builds a read-only query from a screen of questions, and --sql runs one statement. Nothing is saved, and nothing either of them runs can write.',
      link: { to: '/docs/query', label: 'Reading a database' },
    },
    {
      title: 'DDL',
      what: 'Creating and altering tables from sloop.',
      today:
        'sloop db create makes a database and the role that owns it, then registers it. Inside a database, the only schema sloop writes is one a dump carried into a restore or a mirror — never one you described to it.',
      link: { to: '/docs/databases', label: 'Databases and registries' },
    },
    {
      title: 'Editing rows in place',
      what: 'Changing a value in the results grid, and having that reach the database.',
      today:
        'The grid shows two hundred rows at a time and is read-only — enforced by the server, which has been told the transaction cannot write.',
      link: { to: '/docs/query', label: 'Reading a database' },
    },
    {
      title: 'A local interface',
      what: "Something with a window, reading sloop's own PostgreSQL the way the CLI reads it.",
      today:
        'sloop server connection prints host, port, user and database, so psql, DataGrip or anything else can open that database today.',
      link: { to: '/docs/postgres', label: "sloop's own PostgreSQL" },
    },
  ];

  protected readonly notPlanned: readonly NotPlanned[] = [
    {
      title: 'Copying between engines',
      why: [
        'PostgreSQL to MySQL has been worked through, measured against real servers rather than recalled, and it is on hold. It is not on this list, and there is no date on which it will be.',
        'The reason is structural. sloop has no type model, because it has never needed one: every copy it performs is performed by the engines’ own client programs, and no value passes through sloop. Cross-engine, sloop becomes the thing moving the bytes, and every guarantee that is free today — exact row counts, a source that is only read, a failed copy that leaves nothing behind — has to be re-earned by hand.',
        'So mirror, sync and restore all refuse across engines today, and each says why rather than pointing at a ticket.',
      ],
    },
  ];

  /** The refusal's own words, from `mirror.rs` and `sync.rs`. */
  protected readonly refusal =
    'copying between engines is not built: the type systems do not map cleanly, and a copy that quietly rounds or truncates is worse than one that refuses.';

  /** What is not changing, which is the other half of an honest roadmap. */
  protected readonly fixed: readonly { readonly title: string; readonly detail: string }[] = [
    {
      title: 'The guarantee',
      detail:
        'No HTTP client in the dependency graph, and no SSH client either. CI fails the build on the day one appears, so nothing on the list above can arrive by adding one.',
    },
    {
      title: 'The exit codes',
      detail:
        'Frozen at 1.0. Adding a code is a feature; changing what one means is a breaking release, and a test in the source asserts every number and says so.',
    },
    {
      title: 'Never prompting without a terminal',
      detail:
        'Anything added here has to hold that too. A screen of questions is only ever an alternative to a flag, never the only way to say something.',
    },
  ];
}
