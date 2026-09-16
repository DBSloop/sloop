import { Routes } from '@angular/router';

import { Foundation } from './foundation/foundation';

export const routes: Routes = [
  // A4 takes this path for the landing page. The specimen keeps its own route
  // from that point — it is the thing to open when a colour looks wrong.
  {
    path: '',
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
