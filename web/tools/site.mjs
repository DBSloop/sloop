#!/usr/bin/env node
/**
 * Finish the built site, and refuse to hand over one that is wrong.
 *
 * `ng build` prerenders every route to `<route>/index.html`, which is most of
 * what `A22` needs. Here is the rest, and none of it is anything anybody should
 * have to remember:
 *
 * ```text
 * 404.html            GitHub Pages serves this, with a real 404 status, for a
 *                     path that has no file. Angular built the page as
 *                     404/index.html; it is moved here and the directory goes,
 *                     because /404/ as a page that returns 200 is a lie.
 * install.sh          the one-liners in the README, in `sloop --help`, on the
 * install.ps1         landing page and on /docs/reset fetch these four from
 * uninstall.sh        this origin. They are copied from install/, and every
 * uninstall.ps1       dbsloop.github.io URL in the source is then checked
 *                     against what the output actually serves.
 * sitemap.xml         all three generated from the route list — `nav.ts` for
 * robots.txt          the docs tree, `EXTRA_PAGES` in `seo.ts` for the rest —
 * llms.txt            so adding a page adds it to all of them with no other
 *                     edit. `llms.txt`'s prose is tools/llms-copy.mjs.
 * .nojekyll           GitHub Pages runs Jekyll otherwise, which silently drops
 *                     every path beginning with an underscore.
 * ```
 *
 * **And then it checks, which is the half that matters.** Rule 14: a concern
 * that is only stated is a concern that gets forgotten at release. So this
 * fails the build rather than printing a warning, on any of:
 *
 * - a route that has no `index.html`, or one that came out as an empty shell
 * - a page whose `<title>` or `<h1>` is not the one `nav.ts` names for it
 * - a page whose meta description or canonical is not the one `seo.ts` names,
 *   or that has no JSON-LD at all
 * - a prerendered route that nothing describes, so nothing could list it
 * - a `listed: false` route that is in the sitemap anyway, or is missing its
 *   `noindex`
 * - a sitemap that does not hold exactly the routes that were built
 * - a script missing, or a `dbsloop.github.io` URL the output does not serve
 * - `404.html` missing, or not actually the not-found page
 *
 * Two of those are worth the trouble on their own. **A prerender that renders
 * the wrong component** still writes a file of the right name and a plausible
 * size; comparing the heading against the tree is what catches it. And **a
 * sitemap is the one file nobody ever looks at**, so it is the one most likely
 * to be quietly wrong — which is why it is read back off disk and compared
 * rather than simply written.
 *
 * ```sh
 * node tools/site.mjs            # finish dist/web/browser, then check it
 * node tools/site.mjs --check    # check it, writing nothing
 * ```
 */
import { cpSync, existsSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { LLMS_COPY } from './llms-copy.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const web = resolve(here, '..');
const repo = resolve(web, '..');
const out = join(web, 'dist', 'web', 'browser');
const manifest = join(web, 'dist', 'web', 'prerendered-routes.json');
const navFile = join(web, 'src', 'app', 'docs', 'nav.ts');
const seoFile = join(web, 'src', 'app', 'seo.ts');

/** Where the site is served from. Must match `SITE.origin` in `seo.ts`. */
const ORIGIN = 'https://dbsloop.github.io';

/** Verify what was built without writing anything, so a hand edit goes red. */
const CHECK_ONLY = process.argv.includes('--check');

/** Matches `SITE.name` in `seo.ts`, and heads `llms.txt`. */
const SITE_NAME = 'sloop';

/** The two routes outside the docs tree, and the one that becomes `404.html`. */
const OUTSIDE = ['/', '/foundation', '/404'];

/**
 * The scripts `install/` holds, served raw from the root of the site.
 *
 * **All four, not the two `A22` names.** The install one-liners are the reason
 * this entry exists, but `/docs/reset` prints
 * `curl -fsSL https://dbsloop.github.io/uninstall.sh | sh` as the way to take
 * sloop off a machine whose binary is gone. A site that documents a URL and
 * does not serve it is a site with a broken page on it, and the check below
 * would have failed the build over exactly that.
 */
const SCRIPTS = ['install.sh', 'install.ps1', 'uninstall.sh', 'uninstall.ps1'];

function fail(...lines) {
  console.error(`site: ${lines[0]}`);
  for (const line of lines.slice(1)) {
    console.error(line);
  }
  process.exit(1);
}

/**
 * Every docs page, read out of `nav.ts`.
 *
 * **Parsed rather than imported**, because `nav.ts` is TypeScript and this is a
 * plain Node script — and because the alternative, a second list of pages kept
 * here, is exactly the drift `nav.ts` exists to prevent. The shape it looks for
 * is the one the file is written in: `path`, then `documentTitle`, then
 * `title`, in that order, once per page.
 *
 * **`blurb` is found separately**, because `short` and `written` sit between it
 * and `title` on some entries and not others, and a regex that tries to hold
 * that shape is a regex that breaks the next time a field is added. Instead the
 * first `blurb` after a page's `path` is taken to be that page's, which is what
 * a reader of the file would assume too.
 */
function pagesFromNav() {
  const source = readFileSync(navFile, 'utf8');
  const entry =
    /path:\s*'([^']+)',\s*\n\s*documentTitle:\s*'((?:[^'\\]|\\.)*)',\s*\n\s*title:\s*'((?:[^'\\]|\\.)*)',/g;
  const blurbs = [...source.matchAll(/blurb:\s*\n?\s*'((?:[^'\\]|\\.)*)',/g)];

  const pages = [];
  for (const found of source.matchAll(entry)) {
    const blurb = blurbs.find((one) => one.index > found.index);
    if (!blurb) {
      fail(`${found[1]} in nav.ts has no blurb after it`);
    }
    pages.push({
      path: found[1],
      documentTitle: unquote(found[2]),
      title: unquote(found[3]),
      blurb: unquote(blurb[1]),
    });
  }

  if (pages.length === 0) {
    fail(
      `found no pages in ${navFile}`,
      '  the shape this looks for is path, then documentTitle, then title',
    );
  }
  return pages;
}

/** Every `.ts` and `.html` file under a directory. */
function sources(dir) {
  const found = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      found.push(...sources(path));
    } else if (/\.(ts|html)$/.test(entry.name)) {
      found.push(path);
    }
  }
  return found;
}

/**
 * The routes that are not in the docs tree, read out of `seo.ts`.
 *
 * Parsed for the same reason `nav.ts` is: the running site needs this list at
 * runtime, so it has to be TypeScript, and keeping a second copy here is the
 * drift the whole arrangement exists to prevent.
 */
function pagesFromSeo() {
  const source = readFileSync(seoFile, 'utf8');
  const entry =
    /path:\s*'((?:[^'\\]|\\.)*)',\s*\n\s*name:\s*'((?:[^'\\]|\\.)*)',\s*\n\s*title:\s*'((?:[^'\\]|\\.)*)',\s*\n\s*description:\s*\n?\s*'((?:[^'\\]|\\.)*)',\s*\n\s*listed:\s*(true|false)/g;

  const found = [...source.matchAll(entry)].map((one) => ({
    path: unquote(one[1]),
    name: unquote(one[2]),
    title: unquote(one[3]),
    description: unquote(one[4]),
    listed: one[5] === 'true',
  }));

  if (found.length === 0) {
    fail(
      `found no pages in ${seoFile}`,
      '  the shape this looks for is path, name, title, description, listed',
    );
  }
  return found;
}

/** A single-quoted TypeScript string literal, as its text. */
function unquote(literal) {
  return literal.replaceAll("\\'", "'").replaceAll('\\\\', '\\');
}

/** The docs groups, in order, each with the paths that sit in it. */
function groupsFromNav() {
  const source = readFileSync(navFile, 'utf8');
  const heads = [
    ...source.matchAll(/title:\s*'((?:[^'\\]|\\.)*)',\s*\n\s*line:\s*'((?:[^'\\]|\\.)*)',/g),
  ];
  const entries = [...source.matchAll(/path:\s*'([^']+)',\s*\n\s*documentTitle:/g)];

  return heads.map((head, at) => {
    const from = head.index;
    const to = at + 1 < heads.length ? heads[at + 1].index : source.length;
    return {
      title: unquote(head[1]),
      line: unquote(head[2]),
      paths: entries
        .filter((entry) => entry.index > from && entry.index < to)
        .map((entry) => entry[1]),
    };
  });
}

/** The handful of entities Angular writes into an attribute. */
function decode(text) {
  return text
    .replaceAll('&quot;', '"')
    .replaceAll('&#39;', "'")
    .replaceAll('&lt;', '<')
    .replaceAll('&gt;', '>')
    .replaceAll('&amp;', '&');
}

/** The five characters XML will not take raw. */
function xml(text) {
  return text
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&apos;');
}

/**
 * `sitemap.xml`.
 *
 * **No `lastmod`, and that is a decision rather than an omission.** The only
 * date this build knows is the moment it ran, which would tell a crawler that
 * all twenty pages changed every time any one of them did — and a sitemap that
 * cries wolf on every deploy is one a crawler learns to discount. `changefreq`
 * and `priority` are left out for the same reason: Google ignores both.
 */
function sitemap(listed) {
  const urls = listed
    .map((page) => `  <url>\n    <loc>${xml(urlOf(page))}</loc>\n  </url>`)
    .join('\n');
  return [
    '<?xml version="1.0" encoding="UTF-8"?>',
    '<!-- Generated by web/tools/site.mjs from nav.ts and seo.ts. Do not edit. -->',
    '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">',
    urls,
    '</urlset>',
    '',
  ].join('\n');
}

/**
 * `robots.txt`.
 *
 * Everything is allowed, because everything here is documentation that exists
 * to be found. The two unlisted routes carry `noindex` in their own head, which
 * is the instruction that works — a `Disallow` would stop a crawler reading the
 * page and therefore stop it seeing the `noindex` at all.
 */
function robots() {
  return [
    '# Generated by web/tools/site.mjs. Do not edit.',
    'User-agent: *',
    'Allow: /',
    '',
    `Sitemap: ${ORIGIN}/sitemap.xml`,
    '',
  ].join('\n');
}

/**
 * `llms.txt`.
 *
 * The page list is generated from the same routes as the sitemap; the prose is
 * `tools/llms-copy.mjs`. The shape — a `>` summary, what people get wrong, what
 * it is, the pages, the questions, contact — is the owner's, from a worked
 * example he supplied.
 */
function llms(listed) {
  const out = [];
  const say = (...lines) => out.push(...lines);

  say(`# ${SITE_NAME}`, '', `> ${LLMS_COPY.summary}`, '');

  say('## What people get wrong first', '');
  for (const item of LLMS_COPY.gotWrong) {
    say(`### ${item.heading}`, '', ...item.lines, '');
  }

  say('## What it is', '', ...LLMS_COPY.what, '');

  say('## Pages', '');
  const byPath = new Map(listed.map((page) => [page.path, page]));
  for (const page of listed.filter((one) => !one.path.startsWith('/docs/'))) {
    say(`- [${page.name}](${urlOf(page)}): ${page.description}`);
  }
  say('');
  for (const group of groupsFromNav()) {
    const inGroup = group.paths.map((path) => byPath.get(`/docs/${path}`)).filter(Boolean);
    if (inGroup.length === 0) {
      continue;
    }
    say(`### ${group.title}`, '', `${group.line}`, '');
    for (const page of inGroup) {
      say(`- [${page.name}](${urlOf(page)}): ${page.description}`);
    }
    say('');
  }

  say('## Questions', '');
  for (const item of LLMS_COPY.questions) {
    say(`### ${item.q}`, '', ...item.a, '');
  }

  say('## Contact', '', ...LLMS_COPY.contact, '');

  return `${out.join('\n')}`;
}

/** The text of one element, or null when the document has none. */
function tagText(html, tag) {
  const found = html.match(new RegExp(`<${tag}(?:\\s[^>]*)?>(.*?)</${tag}>`, 's'));
  return found ? found[1].trim() : null;
}

function read(route) {
  const file = join(out, route === '/' ? '' : route, 'index.html');
  if (!existsSync(file)) {
    fail(
      `${route} was not prerendered`,
      `  expected ${file}`,
      '  every route on this site is a real file — see A22 in docs/ANGULAR-TASK.md',
    );
  }
  return { file, html: readFileSync(file, 'utf8') };
}

/**
 * The not-found page, from wherever it is by now.
 *
 * **This script is idempotent on purpose.** It moves `404/index.html` to
 * `404.html` and deletes the directory, so a second run against the same
 * output has to find the page where the first run left it rather than
 * reporting a route that was never missing. Running a build step twice is
 * something people do; failing the second one teaches them not to trust it.
 */
function readNotFound() {
  const built = join(out, '404', 'index.html');
  const moved = join(out, '404.html');
  const file = existsSync(built) ? built : moved;
  if (!existsSync(file)) {
    fail(
      '/404 was not prerendered',
      `  expected ${built} or ${moved}`,
      '  the 404 route is in src/app/app.routes.ts and is built like every other page',
    );
  }
  return { file, html: readFileSync(file, 'utf8') };
}

// ── 1. Every route is a real page ──────────────────────────────────────────

const pages = pagesFromNav();
const extras = pagesFromSeo();
const expected = [...OUTSIDE, '/docs', ...pages.map((page) => `/docs/${page.path}`)].sort();

if (!existsSync(manifest)) {
  fail(
    `no ${manifest}`,
    '  run `ng build` first — this finishes what that produced, it does not replace it',
  );
}
const built = Object.keys(JSON.parse(readFileSync(manifest, 'utf8')).routes).sort();

const missing = expected.filter((route) => !built.includes(route));
const extra = built.filter((route) => !expected.includes(route));
if (missing.length > 0 || extra.length > 0) {
  fail(
    'the prerendered routes and the docs tree disagree',
    ...missing.map((route) => `  in nav.ts, never built:  ${route}`),
    ...extra.map((route) => `  built, not in nav.ts:    ${route}`),
  );
}

for (const route of built) {
  if (route === '/404') {
    // Checked by its own step below, which knows both places it can be.
    continue;
  }
  const { file, html } = read(route);
  if (!tagText(html, 'h1')) {
    fail(
      `${route} rendered without an <h1>`,
      `  ${file}`,
      '  that is the shape of an empty shell rather than a page: the prerender produced',
      '  the document but not the component inside it',
    );
  }
}

// ── 2. Each docs page is the page it is supposed to be ─────────────────────

for (const page of pages) {
  const route = `/docs/${page.path}`;
  const { file, html } = read(route);

  const title = tagText(html, 'title');
  if (title !== page.documentTitle) {
    fail(
      `${route} has the wrong <title>`,
      `  nav.ts says  ${page.documentTitle}`,
      `  the file has ${title}`,
      `  ${file}`,
    );
  }

  const heading = tagText(html, 'h1');
  if (heading !== page.title) {
    fail(
      `${route} has the wrong <h1>`,
      `  nav.ts says  ${page.title}`,
      `  the file has ${heading}`,
      `  ${file}`,
      '  a prerender that renders the wrong component still writes a file of the right',
      '  name and a plausible size, and this is what catches it',
    );
  }
}

// ── 3. 404.html, where GitHub Pages looks for it ───────────────────────────

const notFound = readNotFound();
if (!CHECK_ONLY) {
  writeFileSync(join(out, '404.html'), notFound.html);
  rmSync(join(out, '404'), { recursive: true, force: true });
}

const notFoundHeading = tagText(notFound.html, 'h1');
if (!notFoundHeading || !/nothing at this address/i.test(notFoundHeading)) {
  fail(
    '404.html is not the not-found page',
    `  its <h1> is ${notFoundHeading}`,
    '  the /404 route should render NotFound — see src/app/app.routes.ts',
  );
}

// ── 4. The scripts, copied from install/ and verified on the way ───────────

for (const name of SCRIPTS) {
  const source = join(repo, 'install', name);
  if (!existsSync(source)) {
    fail(`no ${source}`, '  a one-liner on this site fetches this file from its root');
  }
  const target = join(out, name);
  if (!CHECK_ONLY) {
    cpSync(source, target);
  }
  if (!existsSync(target)) {
    fail(`${name} is not on the site`, `  expected ${target}`);
  }

  // Cheap, and it catches the one thing that can go wrong between those two
  // lines: a copy that truncated, or a tool that helpfully rewrote the line
  // endings of a file `sh` is going to read.
  if (!readFileSync(target).equals(readFileSync(source))) {
    fail(`${name} did not copy cleanly`, `  ${source}`, `  ${target}`);
  }
}

// ── 5. sitemap.xml, robots.txt and llms.txt ───────────────────────────────
//
// **Generated from the route list, never hand-maintained.** A sitemap somebody
// has to remember is a sitemap that goes stale the first time a route is added.
// The docs half comes out of `nav.ts` and the rest out of `EXTRA_PAGES` in
// `seo.ts` — the same two files the running site reads — so adding a page adds
// it to all three of these with no other edit.
//
// And then they are read back off disk and checked against that same route
// list, which is what makes a hand-edited sitemap fail the build rather than
// ship. `node tools/site.mjs --check` runs the checks without writing anything.

// Both halves carry `name` as well as `title`: the head wants the `<title>`,
// and a list of links wants what the page is called. `seo.ts` says why.
const described = [
  ...extras,
  ...pages.map((page) => ({
    path: `/docs/${page.path}`,
    title: page.documentTitle,
    name: page.title,
    description: page.blurb,
    listed: true,
  })),
];

for (const route of built) {
  if (!described.some((page) => page.path === (route === '/' ? '' : route))) {
    fail(
      `${route} is prerendered and nothing describes it`,
      '  a page needs a title and one sentence before it can be in the sitemap, the',
      '  Open Graph tags or llms.txt',
      '  add it to EXTRA_PAGES in src/app/seo.ts, or to DOCS_NAV in src/app/docs/nav.ts',
    );
  }
}

const listed = described.filter((page) => page.listed);
const urlOf = (page) => `${ORIGIN}${page.path}/`.replace(/([^:])\/\/+/g, '$1/');

if (!CHECK_ONLY) {
  writeFileSync(join(out, 'sitemap.xml'), sitemap(listed));
  writeFileSync(join(out, 'robots.txt'), robots());
  writeFileSync(join(out, 'llms.txt'), llms(listed));
}

// Read back, not remembered. Everything below is about the bytes on disk.
for (const name of ['sitemap.xml', 'robots.txt', 'llms.txt']) {
  if (!existsSync(join(out, name))) {
    fail(`${name} was not generated`, `  expected ${join(out, name)}`);
  }
}

const sitemapText = readFileSync(join(out, 'sitemap.xml'), 'utf8');
const inSitemap = [...sitemapText.matchAll(/<loc>([^<]+)<\/loc>/g)].map((found) => found[1]);
const wanted = listed.map(urlOf);

const absent = wanted.filter((url) => !inSitemap.includes(url));
const unexpected = inSitemap.filter((url) => !wanted.includes(url));
if (absent.length > 0 || unexpected.length > 0) {
  fail(
    'sitemap.xml does not match the routes that were built',
    ...absent.map((url) => `  built, missing from the sitemap:  ${url}`),
    ...unexpected.map((url) => `  in the sitemap, never built:      ${url}`),
    '  it is generated — `node tools/site.mjs` rewrites it from nav.ts and seo.ts',
  );
}

const llmsText = readFileSync(join(out, 'llms.txt'), 'utf8');
for (const page of listed) {
  if (!llmsText.includes(urlOf(page))) {
    fail(`llms.txt has no entry for ${urlOf(page)}`, '  it is generated from the same list');
  }
}

if (!readFileSync(join(out, 'robots.txt'), 'utf8').includes(`${ORIGIN}/sitemap.xml`)) {
  fail('robots.txt does not point at the sitemap');
}

// The pages that must not be indexed must also not be advertised.
for (const page of described.filter((candidate) => !candidate.listed)) {
  const url = urlOf(page);
  if (inSitemap.includes(url) || llmsText.includes(url)) {
    fail(`${page.path} is listed:false and is still advertised`, `  ${url}`);
  }
  const { file, html } = page.path === '/404' ? readNotFound() : read(page.path);
  if (!/<meta name="robots" content="noindex/.test(html)) {
    fail(`${page.path} is listed:false and is missing its noindex`, `  ${file}`);
  }
}

// And every page's head has to be the head `seo.ts` says it is. Two writers
// touch a title — the router's own strategy and `Seo` — so this is what proves
// they agree rather than one quietly winning.
for (const page of described) {
  const { file, html } = page.path === '/404' ? readNotFound() : read(page.path);
  const url = urlOf(page);

  const description = html.match(/<meta name="description" content="([^"]*)"/);
  if (!description || decode(description[1]) !== page.description) {
    fail(
      `${page.path || '/'} has the wrong meta description`,
      `  seo.ts says  ${page.description}`,
      `  the file has ${description ? decode(description[1]) : '(none)'}`,
      `  ${file}`,
    );
  }

  const canonical = html.match(/<link rel="canonical" href="([^"]*)"/);
  const expected = page.path === '/404' ? undefined : url;
  if (expected && (!canonical || canonical[1] !== expected)) {
    fail(
      `${page.path || '/'} has the wrong canonical`,
      `  expected ${expected}`,
      `  the file has ${canonical ? canonical[1] : '(none)'}`,
      `  ${file}`,
    );
  }

  if (!/<script type="application\/ld\+json">/.test(html)) {
    fail(`${page.path || '/'} has no JSON-LD`, `  ${file}`);
  }
}

// ── 6. Every URL this site publishes is a URL this site serves ─────────────
//
// The check with teeth, and the one that turns `A22`'s *done when* into
// something a build can decide. The install one-liners are in the README, in
// `sloop --help`, on the landing page and on two docs pages; the uninstall
// pair is on `/docs/reset`. Rename a file or mistype a path and every one of
// those keeps looking right while fetching a 404 — so the source is read for
// what it promises, and the output is asked whether it can keep it.

const published = new Set();
for (const file of sources(join(web, 'src'))) {
  const text = readFileSync(file, 'utf8');
  for (const found of text.matchAll(/dbsloop\.github\.io(\/[A-Za-z0-9._/-]*)?/g)) {
    published.add(found[1] ?? '/');
  }
}

for (const path of [...published].sort()) {
  const asFile = join(out, path);
  const asPage = join(out, path, 'index.html');
  if (!existsSync(asFile) && !existsSync(asPage)) {
    fail(
      `the site links to https://dbsloop.github.io${path} and does not serve it`,
      `  looked for ${asFile}`,
      `  and for    ${asPage}`,
      '  either publish the file or stop printing the URL',
    );
  }
}

// ── 6a. Every privilege row is on the privileges page ─────────────────────
//
// `web/privileges.json` is written by a Rust test from the table `sloop doctor`
// checks against, and `/docs/privileges` renders it. The page typed the phase
// as two values and filtered the rows into two hand-written lists, so when the
// CLI grew a third phase the row for it matched neither filter and left the
// page — with the build green, because a filter that matches nothing is not an
// error anywhere.
//
// So the built page is asked. Every row of the engine the page opens on has to
// be in the HTML by its own id, which is only true if it was rendered. Rule 14:
// the concern is a check that fails rather than a sentence somebody remembers.
//
// The other engines are behind a button and are not prerendered, so they cannot
// be checked here — but nothing on that page groups by a phase name typed into
// it any more, and this catches the whole class of failure on the engine that
// is served.

const privilegesFile = join(web, 'privileges.json');
const privilegeTable = JSON.parse(readFileSync(privilegesFile, 'utf8'));
const shown = privilegeTable[0];

if (!shown || !Array.isArray(shown.requires) || shown.requires.length === 0) {
  fail(
    'web/privileges.json has no rows for the first engine',
    `  ${privilegesFile}`,
    '  it is generated by engine::privileges::tests in the CLI — regenerate it',
  );
}

const privilegesPage = read('/docs/privileges').html;
const unrendered = shown.requires.filter((row) => !privilegesPage.includes(`id="${row.id}"`));

if (unrendered.length > 0) {
  fail(
    `/docs/privileges leaves out ${unrendered.length} row(s) of web/privileges.json`,
    ...unrendered.map((row) => `  ${shown.engine}/${row.phase}: ${row.id} — ${row.title}`),
    '  every row in that file belongs on the page — see A25 in docs/ANGULAR-TASK.md.',
    '  a row goes missing when the page groups by a phase name typed into it rather',
    '  than by the phases the file actually carries.',
  );
}

// ── 6b. One place says which sloop this is ────────────────────────────
//
// The site printed `0.1.0` in seventeen places while `0.1.1` was the release,
// because every one of them was typed into a transcript by hand. They read
// `src/app/version.ts` now, which reads the `version` the generated command
// reference carries — and that comes from asking the binary.
//
// So this fails the build on a version literal anywhere else under `src/`. A
// release becomes `npm run commands` and nothing else; forgetting it turns the
// build red rather than leaving a number nobody notices.

const versionFile = join(web, 'src', 'app', 'version.ts');
const versionSource = readFileSync(versionFile, 'utf8');

// The two numbers that are sloop's own. Everything else with dots in it —
// `postgres 17.9`, `mysql 8.4.11` — is a fact about a database in a capture and
// belongs exactly where it is.
const release = JSON.parse(
  readFileSync(join(web, 'src', 'app', 'docs', 'commands', 'commands.json'), 'utf8'),
).version.replace(/^\D+/, '');
const captured = versionSource.match(/CAPTURED_AT = '([^']+)'/)?.[1];

if (!/^\d+\.\d+\.\d+$/.test(release) || !captured) {
  fail(
    'could not read which sloop this site documents',
    `  release, from commands.json: ${release}`,
    `  CAPTURED_AT, from version.ts: ${captured}`,
  );
}

for (const file of sources(join(web, 'src'))) {
  if (file === versionFile) {
    continue;
  }
  const text = readFileSync(file, 'utf8');
  const typed = [release, captured].filter((one) => text.includes(one));
  if (typed.length > 0) {
    fail(
      `${file.slice(web.length + 1)} has sloop's version typed into it: ${typed.join(', ')}`,
      '  src/app/version.ts is the one place that may hold one — RELEASE for the',
      '  release this site documents, read from the generated command reference,',
      '  and CAPTURED_AT for the build its transcripts were recorded on. Import',
      '  one of those, so the next release is one command and not a hunt.',
    );
  }
}

// ── 7. Jekyll, and the shell nobody asked for ──────────────────────────────

if (!CHECK_ONLY) {
  writeFileSync(join(out, '.nojekyll'), '');
}

// Angular emits this beside `index.html` as the client-side-rendered shell. On
// a static site nothing serves it, and a crawler that finds it finds a page
// with no content on it — so it goes rather than sitting there.
if (!CHECK_ONLY) {
  rmSync(join(out, 'index.csr.html'), { force: true });
}

const files = readdirSync(out).length;
console.log(
  `site: ${built.length} routes prerendered and checked, ${SCRIPTS.length} scripts published, ` +
    `${published.size} published URLs resolved, ${shown.requires.length} privilege rows rendered, ` +
    `${files} entries in ${out}`,
);
