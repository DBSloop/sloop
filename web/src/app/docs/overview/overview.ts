import { ChangeDetectionStrategy, Component, computed, inject } from '@angular/core';
import { RouterLink } from '@angular/router';

import { DocsPage } from '../docs-page';
import { OsChoice } from '../os';
import { DOCS_NAV, labelOf, urlOf } from '../nav';
import { Terminal, type TerminalLine } from '../../ui/terminal';

/**
 * The front page of the documentation.
 *
 * It does three things and deliberately not a fourth: it states the guarantee,
 * which `CLAUDE.md` puts at the top of the docs as well as at the top of the
 * README and above the fold on the landing page; it says what sloop is in
 * enough prose to be worth indexing; and it hands the reader the map.
 *
 * The install block under *Install it* is driven by the OS tab strip in the
 * toolbar, which is how this page demonstrates that the choice is global rather
 * than claiming it. The installation page itself covers what the installer
 * does, per platform, in full.
 */
@Component({
  selector: 'app-docs-overview',
  imports: [DocsPage, RouterLink, Terminal],
  templateUrl: './overview.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Overview {
  private readonly choice = inject(OsChoice);

  protected readonly groups = DOCS_NAV;
  protected readonly label = labelOf;
  protected readonly url = urlOf;

  protected readonly os = this.choice.os;

  protected readonly installCaption = computed(() => {
    switch (this.os()) {
      case 'windows':
        return 'PowerShell, on Windows';
      case 'macos':
        return 'a terminal, on macOS';
      default:
        return 'a shell, on Linux';
    }
  });

  /**
   * One command, and it is the command for the platform in the toolbar. Both
   * are on the site whichever is selected — the installation page shows every
   * platform's block — so nothing here is hidden from a reader on the other
   * machine, or from a crawler.
   */
  protected readonly installLines = computed<readonly TerminalLine[]>(() =>
    this.os() === 'windows'
      ? [{ kind: 'prompt', text: 'irm https://dbsloop.github.io/install.ps1 | iex' }]
      : [{ kind: 'prompt', text: 'curl -fsSL https://dbsloop.github.io/install.sh | sh' }],
  );

  /** What thirty seconds of checking the guarantee actually looks like. */
  protected readonly proofLines: readonly TerminalLine[] = [
    { kind: 'prompt', text: "cargo tree | grep -Ei 'reqwest|hyper|ureq|curl'" },
    { kind: 'dim', text: '(no output)' },
  ];
}
