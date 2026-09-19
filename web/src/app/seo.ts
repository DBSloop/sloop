import { DOCUMENT, Injectable, inject } from '@angular/core';
import { Meta, Title } from '@angular/platform-browser';
import { NavigationEnd, Router } from '@angular/router';
import { filter } from 'rxjs/operators';

import { DOCS_PAGES, pageAt } from './docs/nav';

/** Where the site lives, and how it describes itself when something asks. */
export const SITE = {
  /** No trailing slash. Everything below appends one. */
  origin: 'https://dbsloop.github.io',
  name: 'sloop',
  repository: 'https://github.com/DBSloop/sloop',
  /** The one sentence that is the product, used wherever nothing narrower fits. */
  tagline:
    'Register your databases once, then back them up, restore them, mirror them and sync them — on Windows, Linux and macOS, without your credentials ever leaving the machine.',
} as const;

/** One page, as a crawler and a model see it. */
export interface SitePage {
  /** The route, with no origin and no trailing slash. `''` is the landing page. */
  readonly path: string;
  /**
   * What the page is called — *Installation*, not *Install sloop — database
   * backup CLI for Windows, Linux, macOS*. A list of links wants this; the
   * `<title>` below is a different job, written to answer a typed query.
   */
  readonly name: string;
  readonly title: string;
  /** One sentence. For a docs page this is its `blurb` in `nav.ts`. */
  readonly description: string;
  /** In `sitemap.xml` and `llms.txt`, and not `noindex`. */
  readonly listed: boolean;
}

/**
 * The routes that are not in the docs tree.
 *
 * **Read by two things, and that is the point.** The service below uses them to
 * write a page's `<meta>`; `tools/site.mjs` parses this same array to build
 * `sitemap.xml`, `robots.txt` and `llms.txt`. A route described in one place and
 * not the other is exactly the drift `A23` exists to stop, and the tool fails
 * the build when a prerendered route appears in neither this list nor
 * `DOCS_NAV`.
 *
 * **`listed` is false twice, for two different reasons.** `/404` is the page a
 * mistake lands on and indexing it would put it in results; `/foundation` is the
 * token specimen `A1` built, reachable only by typing its URL and of no use to
 * anybody who did not build this site.
 */
export const EXTRA_PAGES: readonly SitePage[] = [
  {
    path: '',
    name: 'sloop',
    title: 'sloop — back up, restore, mirror and sync your databases',
    description:
      'A database operations CLI for PostgreSQL, MySQL and MariaDB. Register a database once, then back it up, restore it, mirror it or sync it — from a menu or from a flag, with no HTTP client anywhere in the binary.',
    listed: true,
  },
  {
    path: '/docs',
    name: 'Documentation',
    title: 'sloop docs — PostgreSQL, MySQL and MariaDB backup CLI',
    description:
      'Every page of the sloop documentation, by section: installing it, registering databases, backups and restores, copying, the background service, and the reference.',
    listed: true,
  },
  {
    path: '/foundation',
    name: 'Foundation',
    title: 'Foundation — sloop',
    description: 'The token specimen: every colour, size and duration the site is built from.',
    listed: false,
  },
  {
    path: '/404',
    name: 'Not found',
    title: 'Not found — sloop',
    description: 'There is nothing at this address.',
    listed: false,
  },
];

/** Every page on the site, the docs tree included, in sitemap order. */
export function sitePages(): readonly SitePage[] {
  const docs = DOCS_PAGES.map<SitePage>((page) => ({
    path: `/docs/${page.path}`,
    name: page.title,
    title: page.documentTitle,
    description: page.blurb,
    listed: true,
  }));
  return [...EXTRA_PAGES, ...docs];
}

/**
 * The head, per route.
 *
 * **This is the half of SEO that prerendering makes worth doing.** Before `A22`
 * the served body was an empty `<app-root>`, so nothing here would have been
 * read by anything that does not run JavaScript — which is most crawlers and
 * most model fetchers. Now every route is rendered to a file at build time, so
 * whatever this writes is *in* that file: description, canonical, Open Graph and
 * a JSON-LD graph, each one the page's own rather than the site's repeated
 * twenty-one times.
 *
 * It runs on `NavigationEnd` rather than in a resolver so that a client-side
 * navigation updates the head too. During a prerender there is exactly one
 * navigation, which is the one being rendered.
 */
@Injectable({ providedIn: 'root' })
export class Seo {
  private readonly router = inject(Router);
  private readonly meta = inject(Meta);
  private readonly title = inject(Title);
  private readonly document = inject(DOCUMENT);

  /** Called once, from the application shell. */
  start(): void {
    this.apply(this.router.url);
    this.router.events
      .pipe(filter((event): event is NavigationEnd => event instanceof NavigationEnd))
      .subscribe((event) => this.apply(event.urlAfterRedirects));
  }

  private apply(url: string): void {
    const path = url.split(/[?#]/)[0].replace(/\/+$/, '');
    const page = sitePages().find((candidate) => candidate.path === path) ?? notFound(path);

    // **The trailing slash is deliberate.** GitHub Pages serves a directory at
    // `/docs/security/` and answers `/docs/security` with a 301 to it, so the
    // canonical has to be the one that is a 200 — otherwise every page on the
    // site points its canonical at a redirect.
    const canonical = `${SITE.origin}${page.path}/`.replace(/([^:])\/\/+/g, '$1/');

    this.title.setTitle(page.title);
    this.set('name', 'description', page.description);
    this.set('property', 'og:title', page.title);
    this.set('property', 'og:description', page.description);
    this.set('property', 'og:url', canonical);
    this.set('name', 'twitter:title', page.title);
    this.set('name', 'twitter:description', page.description);

    if (page.listed) {
      this.meta.removeTag("name='robots'");
    } else {
      this.set('name', 'robots', 'noindex, follow');
    }

    this.link('canonical', canonical);
    this.jsonLd(page, canonical);
  }

  private set(kind: 'name' | 'property', key: string, content: string): void {
    this.meta.updateTag({ [kind]: key, content }, `${kind}='${key}'`);
  }

  /** One `<link rel>` in the head, created once and updated after that. */
  private link(rel: string, href: string): void {
    const head = this.document.head;
    let element = head.querySelector<HTMLLinkElement>(`link[rel="${rel}"]`);
    if (!element) {
      element = this.document.createElement('link');
      element.setAttribute('rel', rel);
      head.appendChild(element);
    }
    element.setAttribute('href', href);
  }

  /**
   * The structured description, as one `@graph`.
   *
   * A single script rather than several, because a graph lets the article point
   * at the site and the site at the organisation instead of three documents
   * repeating each other's identifiers.
   */
  private jsonLd(page: SitePage, canonical: string): void {
    const website = {
      '@type': 'WebSite',
      '@id': `${SITE.origin}/#website`,
      url: `${SITE.origin}/`,
      name: SITE.name,
      description: SITE.tagline,
    };

    // **On every page, not only the landing one.** A model that fetches one
    // docs page and nothing else should still come away knowing what the thing
    // being documented is, and a `TechArticle` whose `about` points at a node
    // defined on some other URL tells it nothing.
    const product = {
      '@type': 'SoftwareApplication',
      '@id': `${SITE.origin}/#sloop`,
      name: SITE.name,
      description: SITE.tagline,
      applicationCategory: 'DeveloperApplication',
      operatingSystem: 'Windows, Linux, macOS',
      url: `${SITE.origin}/`,
      codeRepository: SITE.repository,
      license: 'https://spdx.org/licenses/MIT.html',
      offers: { '@type': 'Offer', price: '0', priceCurrency: 'USD' },
    };

    const graph: unknown[] = [website, product];

    const docsPage = page.path.startsWith('/docs/')
      ? pageAt(page.path.slice('/docs/'.length))
      : undefined;

    if (docsPage) {
      graph.push(
        {
          '@type': 'TechArticle',
          '@id': `${canonical}#article`,
          headline: page.title,
          name: docsPage.title,
          description: page.description,
          url: canonical,
          isPartOf: { '@id': website['@id'] },
          about: { '@id': product['@id'] },
        },
        {
          // Three crumbs, and the last one is the page. The group it sits in —
          // *Reference*, *Backups* — is a heading in the sidebar and not a URL,
          // so naming it here would be a breadcrumb pointing at nothing.
          '@type': 'BreadcrumbList',
          '@id': `${canonical}#breadcrumbs`,
          itemListElement: [
            { '@type': 'ListItem', position: 1, name: 'sloop', item: `${SITE.origin}/` },
            {
              '@type': 'ListItem',
              position: 2,
              name: 'Documentation',
              item: `${SITE.origin}/docs/`,
            },
            { '@type': 'ListItem', position: 3, name: docsPage.title, item: canonical },
          ],
        },
      );
    }

    const script =
      this.document.head.querySelector<HTMLScriptElement>('script[type="application/ld+json"]') ??
      (() => {
        const made = this.document.createElement('script');
        made.setAttribute('type', 'application/ld+json');
        this.document.head.appendChild(made);
        return made;
      })();

    script.textContent = JSON.stringify({ '@context': 'https://schema.org', '@graph': graph });
  }
}

/**
 * What to say about a URL that is not a page.
 *
 * The wildcard route renders the not-found page under whatever was typed, so the
 * head has to describe *that* URL rather than `/404` — and it has to be
 * `noindex`, which is what `listed: false` produces above.
 */
function notFound(path: string): SitePage {
  return {
    path,
    name: 'Not found',
    title: 'Not found — sloop',
    description: 'There is nothing at this address.',
    listed: false,
  };
}
