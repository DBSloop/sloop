import { ChangeDetectionStrategy, Component } from '@angular/core';
import { RouterLink } from '@angular/router';

import { DocsPage } from '../docs-page';
import { Terminal, type TerminalLine } from '../../ui/terminal';

/** One figure on the screen, and where it actually comes from. */
interface Measure {
  readonly name: string;
  readonly is: string;
  readonly from: string;
}

/**
 * Activity and monitoring.
 *
 * ## How these blocks were made, and the one that could not be
 *
 * A throwaway PostgreSQL 17.9 cluster on port 5448 with two databases, attached
 * and scheduled, on 2026-09-19. The three `service activity` blocks showing an
 * unsampled machine — two attached, one attached, none attached — are real runs
 * and include the honesty sentence verbatim.
 *
 * **The populated screen is a shape, not a capture, and the page says so where
 * it appears.** Activity rows are written only by a *running installed service*:
 * `sloop service run` is the body the service manager starts and refuses to run
 * from a terminal — *this is not running as a Windows service* — and a manual
 * `sloop backup` does not feed those tables either. Both were tried. Installing
 * a real service on the owner's machine to photograph a screen is not a trade
 * worth making, so the block carries invented numbers in sloop's exact layout,
 * captioned as illustrative, with every *word* around them quoted from
 * `cli/src/commands/activity.rs`.
 *
 * That file is also where the three windows, the `Size` line, the `Moved` line
 * and `no readings in this period` are read from.
 *
 * ## The rule this page is measured against
 *
 * *Nothing on the page implies sloop measures network traffic.* So `Moved` is
 * described as what it is — bytes sloop itself read or wrote during a dump or a
 * restore — the row counts are described as the server's own counters, and the
 * sentence saying bytes on the wire are not available per database is on the
 * page twice: once in sloop's own words inside a captured block, and once in
 * the prose.
 */
@Component({
  selector: 'app-docs-activity',
  imports: [DocsPage, RouterLink, Terminal],
  templateUrl: './activity.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Activity {
  // No OS strip: not a path on the page, and what is recorded is the same
  // everywhere.

  /** Every figure the screen shows, and its provenance. */
  protected readonly measures: readonly Measure[] = [
    {
      name: 'rows in',
      is: 'Rows written into the database over the period.',
      from: "The engine's own counters, read once a round and differenced. sloop does not count them itself.",
    },
    {
      name: 'rows out',
      is: 'Rows read out of it over the period.',
      from: 'The same counters, the same way.',
    },
    {
      name: 'readings',
      is: 'How many rounds the service actually sampled in that period.',
      from: 'A count of samples. It is shown because a window with two readings and a window with two thousand are not the same evidence.',
    },
    {
      name: 'Size',
      is: 'How much space the database takes on disk, and when that was last read.',
      from: "The engine's own size functions, at the moment of the last reading.",
    },
    {
      name: 'Moved',
      is: 'Bytes sloop itself read or wrote, over 30 days.',
      from: 'sloop read the dump and wrote the restore, so it knows exactly how many. This is the only figure on the screen sloop measures directly — which is why it is a line of its own rather than a column beside the rows.',
    },
  ];

  // ── Real captures ─────────────────────────────────────────────────────────

  protected readonly nothingAttached: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop service activity' },
    {
      kind: 'head',
      tag: 'Activity',
      text: '',
      note: ' nothing is attached — `sloop service attach <name>` attaches a database',
    },
  ];

  protected readonly attachedUnsampled: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop service activity' },
    { kind: 'head', tag: 'Activity', text: '', note: ' 2 databases attached' },
    { text: ' ' },
    { kind: 'name', tag: 'analytics', text: '' },
    { kind: 'dim', text: '  nothing recorded yet — attached 2026-09-19 21:06:26 +06:00' },
    { kind: 'dim', text: '  no running service has picked it up yet' },
    { text: ' ' },
    { kind: 'name', tag: 'orders', text: '' },
    { kind: 'dim', text: '  nothing recorded yet — attached 2026-09-19 21:06:26 +06:00' },
    { kind: 'dim', text: '  no running service has picked it up yet' },
    { text: ' ' },
    {
      kind: 'dim',
      text: '  rows are what the server counted; Moved is real bytes sloop read or wrote. Nothing',
    },
    { kind: 'dim', text: '  here is bytes on the wire — no engine reports those per database.' },
    {
      kind: 'dim',
      text: '  no service has ever read this list — `sloop service install` starts one',
    },
  ];

  // ── The shape, with invented numbers ──────────────────────────────────────

  /**
   * What the same screen looks like once a service has been running.
   *
   * **The numbers are invented and the page says so beside the block.** Every
   * word, every label and every unit is `cli/src/commands/activity.rs` — what
   * could not be captured here is data, not wording.
   *
   * The formatters were read rather than guessed at, which caught two things:
   * `Stamp::readable` prints a full `2026-09-19 21:04:12 +06:00` and not a
   * time of day, and `describe_bytes` prints `0 B` rather than `0.0 B` at
   * zero. `grouped` is the comma form; `plural` is what makes it *readings*.
   */
  protected readonly populated: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop service activity' },
    { kind: 'head', tag: 'Activity', text: '', note: ' 2 databases attached' },
    { text: ' ' },
    { kind: 'name', tag: 'analytics', text: '' },
    {
      kind: 'label',
      tag: '  Today   ',
      text: '4,102 rows in · 61,330 out',
      note: ' · 288 readings',
    },
    {
      kind: 'label',
      tag: '  7 days  ',
      text: '26,884 rows in · 402,551 out',
      note: ' · 2,016 readings',
    },
    {
      kind: 'label',
      tag: '  30 days ',
      text: '119,470 rows in · 1,744,902 out',
      note: ' · 8,640 readings',
    },
    {
      kind: 'label',
      tag: '  Size    ',
      text: '412.6 MiB',
      note: ' on disk, read 2026-09-19 21:04:12 +06:00',
    },
    { text: ' ' },
    { kind: 'name', tag: 'orders', text: '' },
    // Today is the empty window, not the 30-day one: thirty days contains the
    // seven, so a populated week inside an empty month is a screen sloop could
    // never print. A service stopped this morning is what this shape means.
    { kind: 'dim', text: '  Today   no readings in this period' },
    {
      kind: 'label',
      tag: '  7 days  ',
      text: '71,205 rows in · 84,330 out',
      note: ' · 1,728 readings',
    },
    {
      kind: 'label',
      tag: '  30 days ',
      text: '284,902 rows in · 341,770 out',
      note: ' · 8,352 readings',
    },
    {
      kind: 'label',
      tag: '  Size    ',
      text: '1.4 GiB',
      note: ' on disk, read 2026-09-19 21:04:12 +06:00',
    },
    {
      kind: 'label',
      tag: '  Moved   ',
      text: '3.1 GiB in · 0 B out',
      note: ' · real bytes sloop moved, over 30 days',
    },
    { text: ' ' },
    {
      kind: 'dim',
      text: '  rows are what the server counted; Moved is real bytes sloop read or wrote. Nothing',
    },
    { kind: 'dim', text: '  here is bytes on the wire — no engine reports those per database.' },
    { kind: 'dim', text: '  the service last read this list at 2026-09-19 21:04:12 +06:00' },
  ];
}
