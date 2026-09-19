import { ChangeDetectionStrategy, Component, inject } from '@angular/core';
import { ActivatedRoute, RouterLink } from '@angular/router';

import { DocsPage } from './docs-page';
import { groupOf, pageAt, type DocsPageEntry } from './nav';

/**
 * A page that has a place in the tree and no prose in it yet.
 *
 * Every page in `docs/ANGULAR-TASK.md` is its own entry, built and confirmed
 * one at a time, so for a while this shell has more routes than it has written
 * pages. The alternative to this component was leaving those routes out of the
 * sidebar until their turn came — which would have made the tree a moving
 * target and left `A5` untestable against its own *Done when*, since that asks
 * for every page in the list to be reachable.
 *
 * **So it says what it is.** It does not pretend to be documentation, it does
 * not fill the space with restated marketing, and it does not guess at flags —
 * it shows the outline the entry that writes it will follow, and points at the
 * two things that are authoritative right now: the binary's own `--help`, and
 * the tour on the home page.
 *
 * One component rather than seventeen stubs: the route carries the page's path
 * in its `data`, and everything shown comes out of `nav.ts`. When a page is
 * written, its route swaps this component for the real one and nothing else
 * changes.
 */
@Component({
  selector: 'app-docs-pending',
  imports: [DocsPage, RouterLink],
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <app-docs-page [heading]="page.title" [lede]="page.blurb" [eyebrow]="group" [path]="page.path">
      <div class="docs-bare rounded-xl border border-warn/30 bg-surface p-5 sm:p-6">
        <p class="flex items-center gap-2.5 font-mono text-xs text-warn">
          <span aria-hidden="true" class="h-1.5 w-1.5 rounded-full bg-warn"></span>
          Not written yet
        </p>
        <p class="mt-3 max-w-measure text-sm text-muted">
          This page has a place in the documentation and no prose in it yet. The outline below is
          what it will hold — it is not a summary standing in for the page, and nothing on it has
          been shortened into a claim.
        </p>
      </div>

      <h2 id="what-this-page-will-cover">What this page will cover</h2>
      <ul>
        @for (item of page.covers; track item) {
          <li>{{ item }}</li>
        }
      </ul>

      <h2 id="until-it-is-written">Until it is written</h2>
      <p>
        The binary documents itself and is always current, which no page here can claim to be:
        <code>sloop --help</code> lists every command, and
        <code>sloop &lt;command&gt; --help</code> gives that command's own flags. Running
        <code>sloop</code> with no arguments opens a menu covering all of them, and every run
        finishes by printing the flag form of what it just did.
      </p>
      <p>
        <a routerLink="/">The home page</a> is a tour of the whole tool — the guarantee, the two
        surfaces, backups and verification, the background service, copying, reading, and what
        happens to your credentials. <a routerLink="/docs">The documentation index</a> has the rest
        of the map.
      </p>
    </app-docs-page>
  `,
})
export class Pending {
  private readonly path =
    (inject(ActivatedRoute).snapshot.data['page'] as string | undefined) ?? '';

  /**
   * Routes are generated from the same list this reads, so a miss is not
   * reachable — but a fallback is still cheaper than a non-null assertion, and
   * it degrades into a page rather than into a crash.
   */
  protected readonly page: DocsPageEntry = pageAt(this.path) ?? {
    path: this.path,
    title: 'Documentation',
    documentTitle: 'sloop documentation',
    blurb: 'This page is not in the documentation tree.',
    covers: [],
  };

  protected readonly group = groupOf(this.path)?.title ?? '';
}
