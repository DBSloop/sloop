import {
  ChangeDetectionStrategy,
  Component,
  DOCUMENT,
  DestroyRef,
  PLATFORM_ID,
  afterNextRender,
  inject,
  input,
  signal,
} from '@angular/core';
import { isPlatformBrowser } from '@angular/common';
import { RouterLink } from '@angular/router';

interface TocItem {
  readonly id: string;
  readonly text: string;
  /** `2` or `3`. A level-three entry is indented under the one above it. */
  readonly level: number;
}

/**
 * On-page contents.
 *
 * It reads the headings out of the article it is given rather than asking each
 * page to declare them twice — so a page cannot grow a section that the
 * contents do not know about, which is the failure mode of every hand-written
 * table of contents.
 *
 * **It is navigation, not content.** Every heading it lists is already in the
 * document, in order, as a real `<h2>` or `<h3>`; this is a shortcut to them.
 * That is why building it from the DOM after render costs nothing a crawler
 * cares about — there is nothing here that is not in the page itself.
 *
 * Only `h2` and `h3` with an `id`, because an anchor without one goes nowhere.
 * Fewer than two of them and the component renders nothing at all: a contents
 * list of one item is furniture.
 *
 * **The links are `routerLink` with a `fragment`, and a bare `href="#id"` is a
 * bug rather than a shortcut.** `index.html` carries `<base href="/">`, and a
 * fragment-only href resolves against the *base* URL rather than the current
 * one — so on `/docs/backups` an `href="#verification"` navigates to
 * `/#verification`, which is the landing page. It was written that way first
 * and every contents link threw the reader off the documentation entirely.
 *
 * Going through the router also means one mechanism handles both cases: a
 * click here, and somebody opening a shared `/docs/backups#verification` from
 * outside. `withInMemoryScrolling({anchorScrolling: 'enabled'})` in
 * `app.config.ts` does the scrolling, and the shell tells its `ViewportScroller`
 * how far the sticky bars reach so the heading lands below them rather than
 * underneath them.
 */
@Component({
  selector: 'app-docs-toc',
  imports: [RouterLink],
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    @if (items().length > 1) {
      @if (variant() === 'rail') {
        <nav aria-label="On this page">
          <p class="font-mono text-[0.6875rem] uppercase tracking-[0.14em] text-faint">
            On this page
          </p>
          <ul class="mt-3 border-l border-line">
            @for (item of items(); track item.id) {
              <li>
                <a
                  [routerLink]="[]"
                  [fragment]="item.id"
                  [class]="
                    active() === item.id
                      ? 'border-accent text-accent'
                      : 'border-transparent text-muted hover:border-line-strong hover:text-text'
                  "
                  class="-ml-px block border-l py-1.5 text-sm transition-colors duration-2 ease-out"
                  [style.padding-left.rem]="item.level === 3 ? 1.75 : 1"
                  [attr.title]="item.text"
                  ><!-- Clamped, because a rail is 216px wide and a heading
                       phrased as a question is not. Nothing is lost: the
                       heading itself is in the document a scroll away, and the
                       full text is on the link. -->
                  <span class="line-clamp-2">{{ item.text }}</span></a
                >
              </li>
            }
          </ul>
        </nav>
      } @else {
        <!-- Below xl there is no room for a rail, so the same list becomes a
             disclosure at the top of the article. A real details element and
             not a scripted panel: it opens with the keyboard, it opens with
             scripting off, and it is closed by default so it never pushes the
             first paragraph off a phone screen. -->
        <details class="group rounded-xl border border-line bg-surface/60">
          <summary
            class="flex cursor-pointer list-none items-center gap-2 px-4 py-3 text-sm font-medium text-muted transition-colors duration-2 ease-out hover:text-text"
          >
            <svg
              class="h-4 w-4 shrink-0 transition-transform duration-2 ease-out group-open:rotate-90"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              stroke-width="1.6"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
              focusable="false"
            >
              <path d="m6 3.5 5 4.5-5 4.5" />
            </svg>
            On this page
            <span class="ml-auto font-mono text-xs text-faint">{{ items().length }}</span>
          </summary>
          <ul class="border-t border-line px-4 py-3">
            @for (item of items(); track item.id) {
              <li>
                <a
                  [routerLink]="[]"
                  [fragment]="item.id"
                  class="block py-1.5 text-sm text-muted transition-colors duration-2 ease-out hover:text-text"
                  [style.padding-left.rem]="item.level === 3 ? 1 : 0"
                  >{{ item.text }}</a
                >
              </li>
            }
          </ul>
        </details>
      }
    }
  `,
})
export class DocsToc {
  /** The article to read headings out of. */
  readonly within = input.required<HTMLElement>();
  readonly variant = input<'rail' | 'inline'>('rail');

  protected readonly items = signal<readonly TocItem[]>([]);
  protected readonly active = signal('');

  private readonly document = inject(DOCUMENT);
  private headings: HTMLElement[] = [];
  private frame = 0;

  constructor() {
    const isBrowser = isPlatformBrowser(inject(PLATFORM_ID));
    const view = this.document.defaultView;

    const onScroll = () => {
      // One read per frame. A scroll event fires far more often than the page
      // paints, and `getBoundingClientRect` in each of them is layout work
      // nobody sees.
      if (this.frame || !view) {
        return;
      }
      this.frame = view.requestAnimationFrame(() => {
        this.frame = 0;
        this.mark();
      });
    };

    inject(DestroyRef).onDestroy(() => {
      view?.removeEventListener('scroll', onScroll);
      if (this.frame) {
        view?.cancelAnimationFrame(this.frame);
      }
    });

    afterNextRender(() => {
      if (!isBrowser || !view) {
        return;
      }
      this.headings = Array.from(
        this.within().querySelectorAll<HTMLElement>('h2[id], h3[id]'),
      ).filter((heading) => heading.textContent?.trim());

      this.items.set(
        this.headings.map((heading) => ({
          id: heading.id,
          text: heading.textContent?.trim() ?? '',
          level: heading.tagName === 'H3' ? 3 : 2,
        })),
      );

      // The rail is the only variant that highlights. The inline disclosure is
      // closed most of the time, and marking something inside a closed box is
      // work for nobody.
      if (this.variant() !== 'rail' || this.headings.length < 2) {
        return;
      }
      this.mark();
      view.addEventListener('scroll', onScroll, { passive: true });
    });
  }

  /**
   * The current section is the last heading whose top has passed the line the
   * sticky chrome ends on — which is the same heading a reader would say they
   * are "in", and the reason this is measured rather than observed. An
   * IntersectionObserver reports crossings, and a section taller than the
   * viewport crosses nothing for as long as it is being read.
   */
  private mark(): void {
    const view = this.document.defaultView;
    if (!view) {
      return;
    }
    const line = 140;
    let current = this.headings[0];
    for (const heading of this.headings) {
      if (heading.getBoundingClientRect().top <= line) {
        current = heading;
      }
    }
    // At the very bottom of a page the last section can never reach the line,
    // so the reader would watch the highlight stop one short of where they are.
    const atEnd =
      view.innerHeight + view.scrollY >= (this.document.documentElement.scrollHeight ?? 0) - 2;
    this.active.set(atEnd ? (this.headings.at(-1)?.id ?? '') : (current?.id ?? ''));
  }
}
