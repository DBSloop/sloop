import { RenderMode, type ServerRoute } from '@angular/ssr';

/**
 * How each route is produced. There is one answer and it is the same for all of
 * them: **build it now, to a file.**
 *
 * `outputMode: "static"` means nothing renders at request time, so
 * `RenderMode.Server` and `RenderMode.Client` have nothing to run on — GitHub
 * Pages serves files and does not execute anything. Every route is therefore
 * `Prerender`, and a route that could not be prerendered would fail the build
 * rather than quietly fall back to an empty shell.
 *
 * **The wildcard is listed before it**, and this is the one entry that is not
 * obvious. `**` in `app.routes.ts` renders the 404 page for a bad link followed
 * inside the site; it matches no real URL, so there is nothing to prerender and
 * asking Angular to try is an error. `Client` here means *this one is left to
 * the browser* — which is exactly right, because the only way to reach it is to
 * already be running in one. The real 404, the file GitHub Pages serves, comes
 * from the concrete `404` route below it and is built like every other page.
 */
export const serverRoutes: ServerRoute[] = [
  { path: '**', renderMode: RenderMode.Prerender },
];
