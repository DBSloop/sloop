import { Type } from '@angular/core';
import { Routes } from '@angular/router';

import { Activity } from './activity/activity';
import { BackupKey } from './backup-key/backup-key';
import { Backups } from './backups/backups';
import { Commands } from './commands/commands';
import { Databases } from './databases/databases';
import { Docs } from './docs';
import { MirrorAndSync } from './mirror-and-sync/mirror-and-sync';
import { DOCS_PAGES } from './nav';
import { GettingStarted } from './getting-started/getting-started';
import { Install } from './install/install';
import { Overview } from './overview/overview';
import { Pending } from './pending';
import { Postgres } from './postgres/postgres';
import { Query } from './query/query';
import { Service } from './service/service';

/**
 * The pages that have been written, by path.
 *
 * Everything not in here renders `Pending`. Adding a page is one entry plus the
 * `written: true` in `nav.ts` that A23 will read when it decides what belongs in
 * the sitemap.
 */
const WRITTEN: Readonly<Record<string, Type<unknown>>> = {
  install: Install,
  'getting-started': GettingStarted,
  commands: Commands,
  databases: Databases,
  backups: Backups,
  'backup-key': BackupKey,
  'mirror-and-sync': MirrorAndSync,
  query: Query,
  service: Service,
  activity: Activity,
  postgres: Postgres,
};

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
        // A page that has been written takes its own component; the rest render
        // `Pending`, which says so. One line per page, and it is the only edit
        // an entry makes here.
        component: WRITTEN[page.path] ?? Pending,
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
