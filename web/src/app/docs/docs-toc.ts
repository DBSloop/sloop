import {
  type AfterViewInit,
  ChangeDetectionStrategy,
  ChangeDetectorRef,
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
export class DocsToc implements AfterViewInit {
  /** The article to read headings out of. */
  readonly within = input.required<HTMLElement>();
  readonly variant = input<'rail' | 'inline'>('rail');
  /**
   * The deepest heading level to list: `2` for `h2` only, `3` for `h2` and `h3`.
   *
   * Three is right for a page of prose. The command reference is not one — it
   * is eight groups holding thirty-nine commands, and listing every command
   * would put a scroll bar on the contents of a page whose whole job is to be
   * scanned. It asks for two, and the commands are reached from the index at
   * the top of it instead.
   */
  readonly depth = input(3, { transform: (value: number | string) => Number(value) || 3 });

  protected readonly items = signal<readonly TocItem[]>([]);
  protected readonly active = signal('');

  private readonly document = inject(DOCUMENT);
  private readonly cdr = inject(ChangeDetectorRef);
  private readonly isBrowser = isPlatformBrowser(inject(PLATFORM_ID));
  private headings: HTMLElement[] = [];
  private frame = 0;

  /**
   * Read the headings during the render itself, browser or not.
   *
   * **`A24`, and it was worth a CLS of 0.41 on every docs page.** The collection
   * used to happen only in `afterNextRender`, which is browser-only by design —
   * so a prerendered page shipped this component empty and then grew a list of
   * eight links the moment the bundle booted. On a phone the inline variant sits
   * *above* the article, so the whole page jumped down by the height of that
   * list on every first load.
   *
   * `ngAfterViewInit` runs on the server too, and by then the projected article
   * is in the DOM with its headings in it — the same DOM the browser will read a
   * moment later, which is why hydration finds what it expects. The observers
   * and the scroll listener stay where they were: those are browser work, and
   * there is nothing to observe during a render that happens once.
   */
  ngAfterViewInit(): void {
    this.collect();
    if (!this.isBrowser) {
      // The write above happens after this view has been checked, and a
      // prerender serialises what was checked — so on the server the list has
      // to be rendered explicitly or it is serialised empty, which is the bug
      // this whole hook exists to fix. In the browser `afterNextRender` runs
      // inside the normal cycle and needs none of this.
      this.cdr.detectChanges();
    }
  }

  constructor() {
    const isBrowser = this.isBrowser;
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

    let watcher: MutationObserver | undefined;

    inject(DestroyRef).onDestroy(() => {
      view?.removeEventListener('scroll', onScroll);
      watcher?.disconnect();
      if (this.frame) {
        view?.cancelAnimationFrame(this.frame);
      }
    });

    afterNextRender(() => {
      if (!isBrowser || !view) {
        return;
      }
      // `ngAfterViewInit` has already read them; this is the re-read after
      // hydration, which costs nothing and keeps the two paths identical.
      this.collect();

      // **A page's headings can change without a navigation**, and on the
      // installation page they do: the OS strip is global state, the article
      // renders one platform's sections, and picking macOS removes the
      // SmartScreen heading and adds the Gatekeeper one. Reading the headings
      // once after the first render left the contents describing a page that
      // was no longer on screen.
      //
      // So the article is watched rather than sampled. It is the same contract
      // as before — the contents are whatever the article says they are — held
      // for the whole time the page is open instead of for one frame of it.
      if (typeof view.MutationObserver === 'function') {
        watcher = new view.MutationObserver(() => {
          // Coalesced into a frame: one `@if` switching platforms fires a
          // stream of records, and re-reading the DOM for each of them is
          // work for the same answer.
          if (this.frame) {
            return;
          }
          this.frame = view.requestAnimationFrame(() => {
            this.frame = 0;
            this.collect();
          });
        });
        // **`characterData` too, and leaving it out was a bug the installation
        // page could never have shown.** There the OS strip adds and removes
        // whole headings, which is a `childList` change; on `sloop's own
        // PostgreSQL` a heading is `Nothing here needs {{ elevation() }}` and
        // picking Linux rewrites its text node in place. Without this the
        // article said *sudo* while the contents rail beside it still said
        // *an administrator* — the rail describing a page that is no longer on
        // screen, which is the exact failure this observer exists to prevent.
        watcher.observe(this.within(), {
          childList: true,
          subtree: true,
          characterData: true,
        });
      }

      // The rail is the only variant that highlights. The inline disclosure is
      // closed most of the time, and marking something inside a closed box is
      // work for nobody.
      if (this.variant() !== 'rail') {
        return;
      }
      view.addEventListener('scroll', onScroll, { passive: true });
    });
  }

  /** Read the article's headings, and say which one the reader is in. */
  private collect(): void {
    const wanted = this.depth() >= 3 ? 'h2[id], h3[id]' : 'h2[id]';
    this.headings = Array.from(this.within().querySelectorAll<HTMLElement>(wanted)).filter(
      (heading) => heading.textContent?.trim(),
    );

    this.items.set(
      this.headings.map((heading) => ({
        id: heading.id,
        text: heading.textContent?.trim() ?? '',
        level: heading.tagName === 'H3' ? 3 : 2,
      })),
    );

    // **Only in the browser**, because this is the one part of the collection
    // that measures rather than reads. `ngAfterViewInit` runs on the server too
    // — deliberately, so the list is in the prerendered HTML — and the server's
    // DOM has no `getBoundingClientRect`, so calling it there threw inside the
    // render pass and left every block after this component on the page
    // serialised empty. There is nothing to highlight in a render that happens
    // once and is never scrolled, and `afterNextRender` marks it a moment later
    // in the browser anyway.
    if (this.isBrowser && this.variant() === 'rail' && this.headings.length > 1) {
      this.mark();
    }
  }

  /**
   * The current section is the last heading whose top has passed the line the
   * sticky chrome ends on — which is the same heading a reader would say they
   * are "in", and the reason this is measured rather than observed. An
   * IntersectionObserver reports crossings, and a section taller than the
   * viewport crosses nothing for as long as it is being read.
   *
   * **The line is the one the router scrolls to, read from the same place.**
   * It was a hardcoded 140 while the shell was scrolling anchored headings to
   * its own measured `--docs-rail` plus 24 — 158 at 1440. So a heading reached
   * from this very list landed eighteen pixels *below* the line that decides
   * what is current, the test failed on it, and the highlight sat on the
   * heading above the one that had just been clicked. Two numbers for one line
   * is the bug; there is one number now, and `docs.ts` owns it.
   */
  private mark(): void {
    const view = this.document.defaultView;
    if (!view) {
      return;
    }
    const line = this.railLine(view);
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

  /**
   * Where the sticky chrome ends, from the property the shell measures into.
   *
   * `docs.ts` writes `--docs-rail` on its host as the navbar and toolbar
   * resize, and adds 24 before handing it to the router's `ViewportScroller`.
   * The same sum is used here, plus two pixels of tolerance: a heading the
   * router has just placed sits *exactly* on the line, and sub-pixel layout
   * would otherwise decide the comparison at random.
   *
   * The fallback is the same 134 `docs.ts` starts from — what the two bars
   * measure to at 1440, so the frame before the observer has run is already
   * right.
   */
  private railLine(view: Window): number {
    const raw = view.getComputedStyle(this.within()).getPropertyValue('--docs-rail');
    const rail = Number.parseFloat(raw);
    return (Number.isFinite(rail) ? rail : 134) + 24 + 2;
  }
}
