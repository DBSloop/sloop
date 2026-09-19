import { Routes } from '@angular/router';

import { Docs } from './docs';
import { DOCS_PAGES } from './nav';
import { Overview } from './overview/overview';
import { Pending } from './pending';

/**
 * `/docs`, and a child route for every page in the tree.
 *
 * The children are generated from `DOCS_PAGES`, so the sidebar and the router
 * cannot disagree about what exists — a link in the rail that leads nowhere is
 * the first thing anybody notices on a documentation site, and this is the one
 * arrangement in which it cannot happen.
 *
 * **Each page is its own route object rather than one `:slug` route**, and that
 * is not decoration. Angular re-creates a component when the activated route
 * config changes and reuses it when only a parameter does — so a single
 * parameterised route would keep one instance alive across every page, and the
 * on-page contents, which read the article's headings once after render, would
 * still be showing the previous page's sections.
 *
 * Every page starts on `Pending`, which says so. The entry that writes a page
 * swaps its `component` for the real one; nothing else here changes, and A23
 * generates `sitemap.xml` and `llms.txt` from this same list.
 */
export const docsRoutes: Routes = [
  {
    path: '',
    component: Docs,
    children: [
      {
        path: '',
        component: Overview,
        title: 'sloop docs — PostgreSQL, MySQL and MariaDB backup CLI',
      },
      ...DOCS_PAGES.map((page) => ({
        path: page.path,
        component: Pending,
        // Read by the shell for the breadcrumb, and by the page itself for
        // everything on it.
        data: { page: page.path },
        // Not the heading. A `<title>` is a lone line in a list of ten results
        // and has to answer the question somebody typed; `nav.ts` says why each
        // one is worded the way it is.
        title: page.documentTitle,
      })),
    ],
  },
];
