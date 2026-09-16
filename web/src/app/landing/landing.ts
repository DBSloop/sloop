import { ChangeDetectionStrategy, Component } from '@angular/core';
import { RouterLink } from '@angular/router';

import { Mark } from '../mark';
import { InstallCommand } from '../ui/install-command';
import { Depth } from '../ui/depth';
import { Reveal } from '../ui/reveal';
import { Terminal, type TerminalLine } from '../ui/terminal';
import { Stones } from './stones';
import { Strata } from './strata';

interface ExitCode {
  readonly code: string;
  readonly meaning: string;
}

/**
 * The landing page.
 *
 * Only two kinds of thing live here: the exit-code table, which is genuinely
 * tabular data, and the terminal transcripts, which are genuinely data too. All
 * the prose is literal markup in the template — so a phrase can carry a `<code>`
 * or a `<strong>` where it needs one, which a string interpolated as text
 * cannot.
 */
@Component({
  selector: 'app-landing',
  imports: [Depth, InstallCommand, Mark, Reveal, RouterLink, Stones, Strata, Terminal],
  templateUrl: './landing.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Landing {
  /** Frozen at 1.0, because automation depends on them. */
  protected readonly exitCodes: readonly ExitCode[] = [
    { code: '0', meaning: 'It worked.' },
    { code: '2', meaning: 'Bad usage, or a database name that is not registered.' },
    { code: '3', meaning: 'The connection failed.' },
    { code: '4', meaning: 'The dump failed.' },
    { code: '5', meaning: 'The restore failed.' },
    { code: '6', meaning: 'It finished, and the row counts did not agree.' },
    { code: '7', meaning: 'Another run holds the lock.' },
    { code: '8', meaning: 'doctor found something that will break a backup.' },
  ];

  /**
   * Real output. The shape, the wording and the field order come from
   * `cli/src/commands/backup.rs`, not from an idea of what a backup might print.
   */
  protected readonly backupOutput: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop backup shop' },
    { text: 'shop  postgres://app@db.internal:5432/orders' },
    { kind: 'dim', text: '  postgres 16.4, TLS, 48 tables, 1284003 rows' },
    { kind: 'dim', text: '  backed up to backups/postgres/shop/20260916T031500Z' },
    { kind: 'dim', text: '  412.6 MB in 12.8s, sha256 9f3ac1d2e8b0' },
    { kind: 'dim', text: '  taken 2026-09-16 09:15:00 +06:00 (20260916T031500Z)' },
    { text: ' ' },
    { text: 'backed up 1 of 1' },
  ];

  /**
   * The three windows that float around the stack.
   *
   * Short on purpose: each one has to fit its window at 260-300px without
   * clipping, and a truncated command reads as a broken page rather than as a
   * long one. The status tags are padded so every message starts at column 5.
   */
  protected readonly registerSnippet: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop db add shop' },
    { kind: 'dim', text: '  connected, 48 tables' },
    { text: 'registered shop' },
  ];

  /** What a scheduled run leaves behind. */
  protected readonly nightlySnippet: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop backup --all' },
    { kind: 'ok', tag: 'ok', text: '   shop      412.6 MB' },
    { kind: 'ok', tag: 'ok', text: '   billing   204.1 MB' },
    { kind: 'bad', tag: 'fail', text: ' archive   refused' },
    { text: 'backed up 2 of 3' },
  ];

  /** Checking the machine before trusting it with anything. */
  protected readonly doctorSnippet: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop doctor' },
    { kind: 'ok', tag: 'ok', text: '   pg_dump 16.4' },
    { kind: 'ok', tag: 'ok', text: '   mysqldump 8.0.39' },
    { kind: 'warn', tag: 'warn', text: ' shop: 2 schemas' },
  ];

  /** The claim, and the thirty seconds it takes to check it. */
  protected readonly guaranteeProof: readonly TerminalLine[] = [
    { kind: 'prompt', text: "cargo tree | grep -Ei 'reqwest|hyper|ureq|isahc|curl'" },
    { kind: 'dim', text: '# nothing. there is no HTTP client in the graph.' },
    { text: ' ' },
    { kind: 'prompt', text: 'echo $?' },
    { text: '1' },
  ];
}
