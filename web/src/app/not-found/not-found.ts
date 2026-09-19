import { ChangeDetectionStrategy, Component } from '@angular/core';
import { RouterLink } from '@angular/router';

import { DOCS_NAV } from '../docs/nav';

/** Where somebody who mistyped a URL most likely meant to be. */
interface Destination {
  readonly to: string;
  readonly title: string;
  readonly line: string;
}

/**
 * The page a genuine typo lands on.
 *
 * **It is a real 404, and that is the whole design.** Every route on this site
 * is prerendered to its own `index.html`, so GitHub Pages serves a real `200`
 * for every page that exists and falls back to `404.html` only for a path that
 * does not. This is the page that becomes `404.html`, and it is not an SPA
 * fallback: a URL reaching it is wrong, the address bar keeps it, and the page
 * says so rather than quietly showing the home page instead.
 *
 * `A22` prerenders it from the `404` route and `tools/site.mjs` moves the file
 * to where GitHub Pages looks for it. The wildcard route renders it too, so a
 * bad link followed inside the site behaves the same way as one typed into the
 * address bar.
 */
@Component({
  selector: 'app-not-found',
  imports: [RouterLink],
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <!-- 65px is the navbar: a 36px mark, py-3.5 either side, and the hairline
         under it. Filling the rest of the viewport keeps this page from
         scrolling on a desktop without pinning it there on a phone, where the
         section list is taller than the screen and a scrollbar is correct. -->
    <main class="mx-auto flex min-h-[calc(100dvh-65px)] max-w-3xl flex-col justify-center px-6 py-16">
      <p class="flex items-center gap-3 font-mono text-xs text-accent">
        <span aria-hidden="true" class="h-px w-6 bg-accent/45"></span>404
      </p>
      <h1 class="mt-4 max-w-[18ch] text-3xl font-semibold sm:text-4xl">
        There is nothing at this address.
      </h1>
      <p class="mt-5 max-w-measure text-lg text-muted">
        Every page on this site is a real file, so this is a genuine typo rather than something
        that moved. Here is the whole of it, by section:
      </p>

      <ul class="mt-9 grid gap-px overflow-hidden rounded-xl border border-line sm:grid-cols-2">
        @for (place of destinations; track place.to) {
          <!-- Nine destinations into two columns leaves one cell empty, and an
               empty cell inside a bordered grid reads as a missing item. The
               first one spans instead: it is the one anybody who landed here
               most likely wants, and eight after it pair up exactly. -->
          <li class="bg-surface" [class.sm:col-span-2]="$first">
            <a
              [routerLink]="place.to"
              class="group block h-full p-5 transition-colors duration-2 ease-out hover:bg-surface-2"
            >
              <span
                class="block text-sm font-semibold text-text transition-colors duration-2 group-hover:text-accent"
                >{{ place.title }}</span
              >
              <span class="mt-1 block text-sm text-muted">{{ place.line }}</span>
            </a>
          </li>
        }
      </ul>

      <p class="mt-9 text-sm text-muted">
        Or start at <a routerLink="/" class="text-accent underline decoration-accent/35 underline-offset-[3px] transition-colors duration-2 hover:decoration-accent">the home page</a>.
      </p>
    </main>
  `,
})
export class NotFound {
  /**
   * The docs tree's own groups, plus the two pages that are not in it.
   *
   * Read from `DOCS_NAV` rather than typed out, for the reason the sidebar and
   * the route table read it: a page added later appears here without anybody
   * remembering to come back, and a link on a 404 page that leads to another
   * 404 is a special kind of insult.
   */
  protected readonly destinations: readonly Destination[] = [
    { to: '/docs', title: 'Documentation', line: 'All seventeen pages, and where to start.' },
    ...DOCS_NAV.map((group) => ({
      to: `/docs/${group.pages[0].path}`,
      title: group.title,
      line: group.line,
    })),
  ];
}
