#!/usr/bin/env node
/**
 * Finish the built site, and refuse to hand over one that is wrong.
 *
 * `ng build` prerenders every route to `<route>/index.html`, which is most of
 * what `A22` needs. Three things it does not do, and one thing nobody should
 * have to remember:
 *
 * ```text
 * 404.html            GitHub Pages serves this, with a real 404 status, for a
 *                     path that has no file. Angular built the page as
 *                     404/index.html; it is moved here and the directory goes,
 *                     because /404/ as a page that returns 200 is a lie.
 * install.sh          the two one-liners in the README, in `sloop --help` and
 * install.ps1         on the landing page fetch these from this origin. They
 *                     are copied from install/ and then compared byte for byte,
 *                     so a published installer can never drift from the one in
 *                     the repository.
 * .nojekyll           GitHub Pages runs Jekyll otherwise, which silently drops
 *                     every path beginning with an underscore.
 * ```
 *
 * **And then it checks, which is the half that matters.** Rule 14: a concern
 * that is only stated is a concern that gets forgotten at release. So this
 * fails the build rather than printing a warning, on any of:
 *
 * - a route in the docs tree that has no `index.html`
 * - an `index.html` that came out as an empty shell rather than a page
 * - a page whose `<title>` or `<h1>` is not the one `nav.ts` names for it
 * - an install script missing, or differing from the one in `install/`
 * - `404.html` missing, or not actually the not-found page
 *
 * The third of those is the one worth having. A prerender that silently
 * produces the wrong component for a route still produces a file of the right
 * name and a plausible size; comparing the heading against the tree catches it,
 * and nothing else does.
 *
 * ```sh
 * node tools/site.mjs          # finish dist/web/browser and check it
 * ```
 */
import { cpSync, existsSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const web = resolve(here, '..');
const repo = resolve(web, '..');
const out = join(web, 'dist', 'web', 'browser');
const manifest = join(web, 'dist', 'web', 'prerendered-routes.json');
const navFile = join(web, 'src', 'app', 'docs', 'nav.ts');

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
 */
function pagesFromNav() {
  const source = readFileSync(navFile, 'utf8');
  const entry =
    /path:\s*'([^']+)',\s*\n\s*documentTitle:\s*'((?:[^'\\]|\\.)*)',\s*\n\s*title:\s*'((?:[^'\\]|\\.)*)',/g;

  const pages = [];
  for (const found of source.matchAll(entry)) {
    pages.push({
      path: found[1],
      documentTitle: found[2].replaceAll("\\'", "'"),
      title: found[3].replaceAll("\\'", "'"),
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
writeFileSync(join(out, '404.html'), notFound.html);
rmSync(join(out, '404'), { recursive: true, force: true });

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
  cpSync(source, target);

  // Cheap, and it catches the one thing that can go wrong between those two
  // lines: a copy that truncated, or a tool that helpfully rewrote the line
  // endings of a file `sh` is going to read.
  if (!readFileSync(target).equals(readFileSync(source))) {
    fail(
      `${name} did not copy cleanly`,
      `  ${source}`,
      `  ${target}`,
    );
  }
}

// ── 5. Every URL this site publishes is a URL this site serves ─────────────
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

// ── 5. Jekyll, and the shell nobody asked for ──────────────────────────────

writeFileSync(join(out, '.nojekyll'), '');

// Angular emits this beside `index.html` as the client-side-rendered shell. On
// a static site nothing serves it, and a crawler that finds it finds a page
// with no content on it — so it goes rather than sitting there.
rmSync(join(out, 'index.csr.html'), { force: true });

const files = readdirSync(out).length;
console.log(
  `site: ${built.length} routes prerendered and checked, ${SCRIPTS.length} scripts published, ` +
    `${published.size} published URLs resolved, ${files} entries in ${out}`,
);
