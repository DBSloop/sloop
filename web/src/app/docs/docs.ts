import {
  ChangeDetectionStrategy,
  Component,
  DOCUMENT,
  DestroyRef,
  ElementRef,
  PLATFORM_ID,
  afterNextRender,
  computed,
  inject,
  signal,
  viewChild,
} from '@angular/core';
import { ViewportScroller, isPlatformBrowser } from '@angular/common';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { NavigationEnd, Router, RouterLink, RouterOutlet } from '@angular/router';
import { filter } from 'rxjs';

import { DocsNav } from './docs-nav';
import { OsTabs } from './os-tabs';
import { groupOf, pageAt, type DocsPageEntry } from './nav';

/**
 * The docs shell: the toolbar, the rail and the column every page renders into.
 *
 * ## Why two heights are measured instead of written down
 *
 * The rail and the on-page contents are sticky, and both have to clear the
 * navbar *and* this component's own toolbar. Neither bar has a fixed height —
 * the navbar's comes from the theme control inside it, the toolbar's from the
 * OS strip inside this one — so a number in a stylesheet would be a copy of an
 * arithmetic result: correct until somebody changes a padding two components
 * away, and nothing tells them.
 *
 * So both are measured with one `ResizeObserver` and published as custom
 * properties on this host:
 *
 * ```text
 * --docs-nav    the navbar
 * --docs-bar    this toolbar
 * --docs-rail   the two together, which is what everything sticky offsets by
 * ```
 *
 * Every consumer declares a fallback of what they measure to today, so the one
 * frame before the observer first runs is already about right, and a page with
 * no script at all is a few pixels out rather than broken.
 */
@Component({
  selector: 'app-docs',
  imports: [DocsNav, OsTabs, RouterLink, RouterOutlet],
  templateUrl: './docs.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
  host: {
    '(document:keydown)': 'onKeydown($event)',
  },
  styles: `
    :host {
      display: block;
    }

    .docs-bar {
      top: var(--docs-nav, 71px);
    }

    .docs-rail {
      top: var(--docs-rail, 134px);
    }
  `,
})
export class Docs {
  /** Open below `lg`, where there is no rail to open instead. */
  protected readonly panel = signal(false);

  /** The page the outlet is showing, for the breadcrumb. */
  protected readonly here = signal<DocsPageEntry | null>(null);
  protected readonly group = computed(() => groupOf(this.here()?.path ?? '')?.title ?? '');

  // Read explicitly as the component rather than left to the default, because
  // both of these are template references on the same component type and an
  // implicit read is one refactor away from returning an ElementRef instead.
  private readonly rail = viewChild.required('rail', { read: DocsNav });
  private readonly panelNav = viewChild('panelNav', { read: DocsNav });
  private readonly bar = viewChild.required<ElementRef<HTMLElement>>('bar');

  private readonly document = inject(DOCUMENT);
  private readonly router = inject(Router);
  private readonly destroyRef = inject(DestroyRef);
  private readonly isBrowser = isPlatformBrowser(inject(PLATFORM_ID));
  private readonly host = inject<ElementRef<HTMLElement>>(ElementRef);
  private readonly scroller = inject(ViewportScroller);

  /** How far down the page the sticky chrome reaches. Measured, see below. */
  private sticky = 134;

  constructor() {
    this.sync();

    // Where the router puts a heading when it scrolls to a fragment — either
    // from a contents link or from somebody opening a shared /docs/x#y.
    //
    // It has to be told: Angular's scroller positions an element from
    // `getBoundingClientRect` and a configured offset, and takes no notice of
    // the `scroll-margin-top` in docs.css. Without this the heading lands
    // exactly underneath the two bars that are covering it.
    this.scroller.setOffset(() => [0, this.sticky + 24]);
    // The offset is on a root service, and the landing page's chrome is a
    // different height. It goes back when the docs are left.
    this.destroyRef.onDestroy(() => this.scroller.setOffset([0, 0]));

    this.router.events
      .pipe(
        filter((event) => event instanceof NavigationEnd),
        takeUntilDestroyed(),
      )
      .subscribe(() => {
        this.sync();
        // A panel left open over the page somebody just asked for is a panel
        // they have to close before they can read anything.
        this.panel.set(false);
      });

    afterNextRender(() => {
      // Again, and not instead: the constructor runs while the router is still
      // activating, and this runs once it certainly has finished.
      this.sync();
      this.measure();
    });
  }

  /**
   * Ctrl+K, or ⌘K on a Mac — the shortcut every docs site has, pointed at the
   * filter rather than at a search dialog this one does not have.
   *
   * Below `lg` the rail is `display: none`, and focusing something inside a
   * hidden subtree does nothing at all — so the panel is opened first and the
   * copy of the tree inside *it* takes the focus.
   */
  protected onKeydown(event: KeyboardEvent): void {
    if (event.key === 'Escape' && this.panel()) {
      this.panel.set(false);
      return;
    }
    if (event.key !== 'k' || !(event.metaKey || event.ctrlKey) || event.altKey) {
      return;
    }
    const view = this.document.defaultView;
    if (!view) {
      return;
    }
    event.preventDefault();

    if (view.matchMedia('(min-width: 1024px)').matches) {
      this.rail().focus();
      return;
    }
    this.panel.set(true);
    // The panel's tree does not exist until the view has been updated for the
    // signal above, so the focus waits a turn for it.
    view.setTimeout(() => this.panelNav()?.focus());
  }

  /** The deepest activated route is the page, and its `data.page` is the path. */
  private sync(): void {
    let route = this.router.routerState.snapshot.root;
    while (route.firstChild) {
      route = route.firstChild;
    }
    this.here.set(pageAt((route.data['page'] as string | undefined) ?? '') ?? null);
  }

  private measure(): void {
    const view = this.document.defaultView;
    if (!this.isBrowser || !view || typeof view.ResizeObserver !== 'function') {
      return;
    }

    // The navbar belongs to the application shell rather than to this
    // component, so it is found rather than injected. `app-navbar` itself is an
    // inline element whose own box says nothing useful; the `<header>` inside
    // it is the bar.
    const navbar = this.document.querySelector('app-navbar header');
    // The toolbar's own row, not the sticky box around it — that box grows by
    // the height of the panel when the panel is open, and an offset that moved
    // when a menu opened would be a rail that jumped.
    const bar = this.bar().nativeElement;

    const style = this.host.nativeElement.style;
    const write = () => {
      const nav = Math.round(navbar?.getBoundingClientRect().height ?? 0);
      // + 1 for the hairline under the toolbar, which is a border on the
      // sticky box and so is not part of the row's own height.
      const own = Math.round(bar.getBoundingClientRect().height) + 1;
      style.setProperty('--docs-nav', `${nav}px`);
      style.setProperty('--docs-bar', `${own}px`);
      style.setProperty('--docs-rail', `${nav + own}px`);
      this.sticky = nav + own;
    };

    const observer = new view.ResizeObserver(write);
    if (navbar) {
      observer.observe(navbar);
    }
    observer.observe(bar);
    this.destroyRef.onDestroy(() => observer.disconnect());
    write();
  }
}
