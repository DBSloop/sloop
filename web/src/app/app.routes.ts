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
];
