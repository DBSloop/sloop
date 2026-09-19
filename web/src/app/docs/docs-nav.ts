import {
  ChangeDetectionStrategy,
  Component,
  DOCUMENT,
  ElementRef,
  computed,
  inject,
  output,
  signal,
  viewChild,
} from '@angular/core';
import { RouterLink, RouterLinkActive } from '@angular/router';

import { DOCS_NAV, DOCS_PAGES, labelOf, urlOf, type DocsGroup } from './nav';

/**
 * The sidebar tree, and the filter over it.
 *
 * Rendered twice — once in the rail at `lg` and up, once inside the disclosure
 * panel below it — so the two can never hold different trees. Each instance
 * owns its own filter text, which is right: the panel is opened, used and
 * closed, and inheriting whatever was typed in a rail nobody can see would be
 * a surprise.
 *
 * **Why a filter and not a search.** The owner's call, asked before this was
 * built. Seventeen pages is enough that scanning eight groups for *the one
 * about keys* is work, and not nearly enough to justify a build-time index of
 * page prose — which would also, today, be an index of pages that have not been
 * written. A filter needs no index, cannot go stale, and covers a page the
 * moment it is added to `nav.ts`.
 *
 * It matches the page's title, its sidebar label, its one-line summary and its
 * group's name, so *key* finds The backup key, *cron* finds The background
 * service through its summary, and *reference* finds the whole Reference group.
 */
@Component({
  selector: 'app-docs-nav',
  imports: [RouterLink, RouterLinkActive],
  changeDetection: ChangeDetectionStrategy.OnPush,
  // The tree scrolls inside a 256px rail, and the page's own 11px capsule is
  // heavier there than the links beside it. The @supports fence is base.css's,
  // for base.css's reason: Chrome and Edge drop ::-webkit-scrollbar entirely
  // once `scrollbar-color` is set on the element.
  styles: `
    @supports not selector(::-webkit-scrollbar) {
      nav {
        scrollbar-width: thin;
        scrollbar-color: rgb(var(--ch-line)) transparent;
      }
    }

    nav::-webkit-scrollbar {
      width: 8px;
    }

    nav::-webkit-scrollbar-track {
      background-color: transparent;
    }

    nav::-webkit-scrollbar-thumb {
      background-color: rgb(var(--ch-line));
      border: 2px solid transparent;
      background-clip: content-box;
      border-radius: 999px;
      transition: background-color var(--dur-2) var(--ease-out);
    }

    nav:hover::-webkit-scrollbar-thumb {
      background-color: rgb(var(--ch-line-strong));
    }
  `,
  template: `
    <div class="flex h-full min-h-0 flex-col">
      <!-- The filter. A search input rather than a text one: it is what the
           control is, it gets the platform's own semantics, and a password
           manager never offers to fill it. -->
      <div class="relative shrink-0">
        <svg
          class="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-faint"
          viewBox="0 0 16 16"
          fill="none"
          stroke="currentColor"
          stroke-width="1.6"
          stroke-linecap="round"
          aria-hidden="true"
          focusable="false"
        >
          <circle cx="7.1" cy="7.1" r="4.6" />
          <path d="m10.6 10.6 3 3" />
        </svg>
        <input
          #box
          type="search"
          autocomplete="off"
          spellcheck="false"
          [attr.aria-label]="'Filter ' + total + ' documentation pages'"
          placeholder="Filter pages"
          class="w-full rounded-lg border border-line bg-surface py-2 pl-9 pr-12 text-sm text-text placeholder:text-faint transition-colors duration-2 ease-out hover:border-line-strong focus:border-accent/60 [&::-webkit-search-cancel-button]:hidden"
          [value]="query()"
          (input)="query.set(asValue($event))"
          (keydown.escape)="clear()"
        />
        <!-- The hint is decoration on a control the keyboard already reaches;
             it disappears the moment there is something to say instead. -->
        @if (query()) {
          <button
            type="button"
            class="absolute right-2 top-1/2 -translate-y-1/2 rounded-md px-2 py-1 font-mono text-[0.6875rem] text-faint transition-colors duration-2 ease-out hover:bg-surface-2 hover:text-text"
            (click)="clear()"
          >
            clear
          </button>
        } @else {
          <kbd
            aria-hidden="true"
            class="pointer-events-none absolute right-2.5 top-1/2 -translate-y-1/2 rounded border border-line bg-surface-2 px-1.5 py-0.5 font-mono text-[0.6875rem] font-normal text-faint"
            >{{ hint }}</kbd
          >
        }
      </div>

      <nav class="mt-5 min-h-0 flex-1 overflow-y-auto pb-2" aria-label="Documentation">
        @if (!query()) {
          <a
            routerLink="/docs"
            [routerLinkActiveOptions]="{ exact: true }"
            routerLinkActive="!text-accent !border-accent"
            class="-ml-px flex items-center gap-2 border-l border-transparent py-1.5 pl-4 text-sm font-medium text-muted transition-colors duration-2 ease-out hover:text-text"
            (click)="picked.emit()"
            >Overview</a
          >
        }

        @for (group of groups(); track group.title; let first = $first) {
          <!-- The first group sits straight under the filter box when the tree
               is filtered, and under the Overview link when it is not. -->
          <div [class]="first && query() ? '' : 'mt-6'">
            <p class="pl-4 font-mono text-[0.6875rem] uppercase tracking-[0.14em] text-faint">
              {{ group.title }}
            </p>
            <ul class="mt-2 border-l border-line">
              @for (page of group.pages; track page.path) {
                <li>
                  <a
                    [routerLink]="url(page)"
                    routerLinkActive="!text-accent !border-accent !font-semibold"
                    class="-ml-px block border-l border-transparent py-1.5 pl-4 text-sm text-muted transition-colors duration-2 ease-out hover:border-line-strong hover:text-text"
                    (click)="picked.emit()"
                    >{{ label(page) }}</a
                  >
                </li>
              }
            </ul>
          </div>
        }

        @if (query()) {
          <p class="mt-6 pl-4 font-mono text-xs text-faint">
            @if (matches() === 0) {
              Nothing matches <span class="text-muted">{{ query() }}</span
              >.
            } @else {
              {{ matches() }} of {{ total }} pages
            }
          </p>
        }
      </nav>
    </div>
  `,
})
export class DocsNav {
  /** A link was followed — the panel below `lg` closes itself on it. */
  readonly picked = output<void>();

  protected readonly query = signal('');
  protected readonly total = DOCS_PAGES.length;

  private readonly box = viewChild.required<ElementRef<HTMLInputElement>>('box');

  /**
   * Ctrl on Windows and Linux, ⌘ on a Mac — read from the browser rather than
   * from the docs' own OS tab, because the shortcut belongs to the keyboard in
   * front of the reader and not to the platform they are reading about.
   */
  protected readonly hint = /mac|iphone|ipad/i.test(
    inject(DOCUMENT).defaultView?.navigator.userAgent ?? '',
  )
    ? '⌘K'
    : 'Ctrl K';

  protected readonly groups = computed<readonly DocsGroup[]>(() => {
    const needle = this.query().trim().toLowerCase();
    if (!needle) {
      return DOCS_NAV;
    }
    return DOCS_NAV.flatMap((group) => {
      // A group whose own name matches keeps all of its pages: somebody typing
      // "backups" means the group, and hiding four of its five pages because
      // the word is not in their titles would be the filter arguing back.
      if (group.title.toLowerCase().includes(needle)) {
        return [group];
      }
      const pages = group.pages.filter((page) =>
        [page.title, page.short ?? '', page.blurb].join(' ').toLowerCase().includes(needle),
      );
      return pages.length ? [{ ...group, pages }] : [];
    });
  });

  protected readonly matches = computed(() =>
    this.groups().reduce((count, group) => count + group.pages.length, 0),
  );

  /** Called by the shell when the reader presses the shortcut. */
  focus(): void {
    const input = this.box().nativeElement;
    input.focus();
    input.select();
  }

  protected clear(): void {
    this.query.set('');
    this.box().nativeElement.value = '';
  }

  protected asValue(event: Event): string {
    return (event.target as HTMLInputElement).value;
  }

  protected label = labelOf;
  protected url = urlOf;
}
