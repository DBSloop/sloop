import { ChangeDetectionStrategy, Component, computed, input } from '@angular/core';
import { RouterLink } from '@angular/router';

import { DocsToc } from './docs-toc';
import { DOCS_PAGES, labelOf, urlOf, type DocsPageEntry } from './nav';

/**
 * The frame every docs page sits in: the heading block, the article, the
 * on-page contents, and the two links out of it.
 *
 * A page supplies its prose as projected content and nothing else, so the type
 * rhythm, the measure and the contents behave the same on all seventeen of them
 * — which is the difference between a docs site and seventeen pages that happen
 * to share a navbar.
 *
 * The article carries `.docs-prose`, whose element rules live in
 * `src/styles/docs.css`. A page writes `<h2 id="...">` and `<p>` and gets the
 * scale; it reaches for a utility class only where it wants something the scale
 * does not give it.
 */
@Component({
  selector: 'app-docs-page',
  imports: [DocsToc, RouterLink],
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <div class="grid gap-10 pb-20 pt-9 sm:pt-11 xl:grid-cols-[minmax(0,1fr)_13.5rem] xl:gap-12">
      <div class="min-w-0">
        <header>
          @if (eyebrow()) {
            <p class="flex items-center gap-3 font-mono text-xs text-accent">
              <span aria-hidden="true" class="h-px w-6 bg-accent/45"></span>{{ eyebrow() }}
            </p>
          }
          <!-- 4xl and not 5xl. The display sizes were scaled for a landing
               page with the width of the window to fill; here the column is
               680px and the heading below it is 2xl, and a 55px title over a
               27px one is a shout rather than a hierarchy. -->
          <h1 class="mt-3 max-w-[22ch] text-3xl font-semibold sm:text-4xl">{{ heading() }}</h1>
          <p class="mt-5 max-w-measure text-lg text-muted">{{ lede() }}</p>
        </header>

        <div class="mt-9 xl:hidden">
          <app-docs-toc [within]="body" variant="inline" [depth]="tocDepth()" />
        </div>

        <article #body class="docs-prose mt-10">
          <ng-content />
        </article>

        @if (previous() || next()) {
          <nav
            class="mt-16 grid gap-4 border-t border-line pt-8 sm:grid-cols-2"
            aria-label="Nearby pages"
          >
            @if (previous(); as page) {
              <a
                [routerLink]="url(page)"
                class="group rounded-xl border border-line bg-surface px-5 py-4 transition-colors duration-2 ease-out hover:border-line-strong"
              >
                <span class="font-mono text-xs text-faint">← Previous</span>
                <span
                  class="mt-1 block text-sm font-semibold text-text transition-colors duration-2 group-hover:text-accent"
                  >{{ label(page) }}</span
                >
              </a>
            } @else {
              <span class="hidden sm:block"></span>
            }

            @if (next(); as page) {
              <a
                [routerLink]="url(page)"
                class="group rounded-xl border border-line bg-surface px-5 py-4 text-right transition-colors duration-2 ease-out hover:border-line-strong sm:col-start-2"
              >
                <span class="font-mono text-xs text-faint">Next →</span>
                <span
                  class="mt-1 block text-sm font-semibold text-text transition-colors duration-2 group-hover:text-accent"
                  >{{ label(page) }}</span
                >
              </a>
            }
          </nav>
        }
      </div>

      <aside class="hidden xl:block">
        <div class="sticky max-h-[calc(100dvh-var(--docs-rail,134px)-4rem)] overflow-y-auto">
          <app-docs-toc [within]="body" [depth]="tocDepth()" />
        </div>
      </aside>
    </div>
  `,
  styles: `
    :host {
      display: block;
      min-width: 0;
    }

    /* The rail clears the navbar and the toolbar, both of which are measured at
       runtime by the shell — see docs.ts. The fallback is what the two measure
       to at 1440, so a rail is right even in the one frame before the observer
       has run. */
    aside > div {
      top: calc(var(--docs-rail, 134px) + 2rem);
    }

    /* The page's own scrollbar is an 11px capsule, which is right for the page
       and heavy inside a 216px contents rail. Same @supports fence as base.css:
       Chrome and Edge drop ::-webkit-scrollbar entirely once the standard
       scrollbar-color is set, so setting both throws the designed one away. */
    @supports not selector(::-webkit-scrollbar) {
      aside > div {
        scrollbar-width: thin;
        scrollbar-color: rgb(var(--ch-line)) transparent;
      }
    }

    aside > div::-webkit-scrollbar {
      width: 8px;
    }

    aside > div::-webkit-scrollbar-track {
      background-color: transparent;
    }

    aside > div::-webkit-scrollbar-thumb {
      background-color: rgb(var(--ch-line));
      border: 2px solid transparent;
      background-clip: content-box;
      border-radius: 999px;
      transition: background-color var(--dur-2) var(--ease-out);
    }

    aside > div:hover::-webkit-scrollbar-thumb {
      background-color: rgb(var(--ch-line-strong));
    }
  `,
})
export class DocsPage {
  readonly heading = input.required<string>();
  readonly lede = input.required<string>();
  /** The group this page sits in. Empty on the overview, which is in none. */
  readonly eyebrow = input('');
  /**
   * The page's own path under `/docs`, which is what prev and next walk from.
   * The overview passes the empty string and therefore has a next but no
   * previous.
   */
  readonly path = input('');
  /** Passed through to the on-page contents; see `DocsToc.depth`. */
  readonly tocDepth = input(3, { transform: (value: number | string) => Number(value) || 3 });

  protected readonly previous = computed<DocsPageEntry | null>(() => {
    const at = this.index();
    return at > 0 ? DOCS_PAGES[at - 1] : null;
  });

  protected readonly next = computed<DocsPageEntry | null>(() => {
    const at = this.index();
    return at + 1 < DOCS_PAGES.length ? DOCS_PAGES[at + 1] : null;
  });

  /** `-1` for the overview, which sits before the first page rather than in it. */
  private readonly index = computed(() =>
    DOCS_PAGES.findIndex((page) => page.path === this.path()),
  );

  protected label = labelOf;
  protected url = urlOf;
}
