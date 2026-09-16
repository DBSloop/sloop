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

  // The navbar's primary tab points at /docs, which A5 builds. Until it does,
  // an unmatched path lands on the one page there is rather than throwing
  // NG04002 into a console this project requires to be empty. A5 adds the real
  // route and A12 adds real 404 handling; both replace this.
  {
    path: '**',
    redirectTo: '',
  },
];
