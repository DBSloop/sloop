import { Routes } from '@angular/router';

import { Foundation } from './foundation/foundation';
import { Landing } from './landing/landing';
import { NotFound } from './not-found/not-found';

export const routes: Routes = [
  {
    path: '',
    component: Landing,
    title: 'sloop — back up, restore, mirror and sync your databases',
  },

  // The token specimen A1 built. It keeps earning its place: it is the thing to
  // open when a colour looks wrong in one theme and right in the other.
  {
    path: 'foundation',
    component: Foundation,
    title: 'Foundation — sloop',
  },

  // The documentation, and everything under it.
  //
  // Lazy, for the reason three.js is lazy on the landing page and the reverse
  // of it: the docs are the largest route set on the site by count, and
  // somebody who came for the home page should not download seventeen route
  // definitions to read it. The split also keeps the landing bundle — which
  // owns the 3D — out of the docs.
  {
    path: 'docs',
    loadChildren: () => import('./docs/docs.routes').then((m) => m.docsRoutes),
  },

  // **A real 404, and prerendered like everything else.** `A22` builds this
  // route to its own `index.html` and `tools/site.mjs` moves it to `404.html`,
  // which is the file GitHub Pages serves — with a real `404` status — for a
  // path that has no file. Every path that *does* exist has one, so nothing
  // real ever reaches this.
  {
    path: '404',
    component: NotFound,
    title: 'Not found — sloop',
  },

  // The same component for a bad link followed inside the site. Not a redirect:
  // sending a typo to the home page tells somebody the page exists and they
  // arrived somewhere else, which is a worse lie than saying it does not.
  {
    path: '**',
    component: NotFound,
    title: 'Not found — sloop',
  },
];
