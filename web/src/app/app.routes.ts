import { Routes } from '@angular/router';

import { Foundation } from './foundation/foundation';
import { Landing } from './landing/landing';

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

  // An unmatched path lands on the one page every visitor can use rather than
  // throwing NG04002 into a console this project requires to be empty. A22 owns
  // real 404 handling, and replaces this.
  {
    path: '**',
    redirectTo: '',
  },
];
