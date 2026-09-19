import { ChangeDetectionStrategy, Component, inject, signal } from '@angular/core';
import { NavigationEnd, Router, RouterOutlet } from '@angular/router';
import { filter } from 'rxjs/operators';

import { Navbar } from './navbar/navbar';
import { Seo } from './seo';

/**
 * The application shell: the navbar, which every route shares, and the outlet.
 *
 * It also starts [`Seo`], which is the one thing here that is not visible. The
 * head has to be written for whichever route is being rendered, and the shell is
 * the only component that exists for all of them — so it is where the one
 * subscription belongs rather than in twenty-one pages.
 */
@Component({
  selector: 'app-root',
  imports: [Navbar, RouterOutlet],
  templateUrl: './app.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class App {
  /**
   * Where the skip link points, which is this page and not the home page.
   *
   * **A bare `href="#content"` does not work here, and it fails silently.**
   * `index.html` carries `<base href="/">` because the router needs it, and a
   * fragment-only URL resolves against the base rather than the current
   * document — so from `/docs/security` the skip link went to `/#content`, the
   * landing page, with focus correctly on *its* main. It looked right and read
   * wrong, which is the worst kind of broken.
   *
   * Written out in full it is an ordinary same-document link again: the browser
   * moves focus to `main#content` (which carries `tabindex="-1"` for exactly
   * this) and scrolls, with no script involved and nothing to go wrong when the
   * bundle has not loaded yet.
   */
  protected readonly skipTo = signal('#content');

  constructor() {
    inject(Seo).start();

    const router = inject(Router);
    const point = () => this.skipTo.set(`${router.url.split('#')[0]}#content`);
    point();
    router.events
      .pipe(filter((event): event is NavigationEnd => event instanceof NavigationEnd))
      .subscribe(point);
  }
}
