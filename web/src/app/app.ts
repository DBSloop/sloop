import { ChangeDetectionStrategy, Component, inject } from '@angular/core';
import { RouterOutlet } from '@angular/router';

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
  constructor() {
    inject(Seo).start();
  }
}
