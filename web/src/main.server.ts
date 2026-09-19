import { bootstrapApplication, type BootstrapContext } from '@angular/platform-browser';

import { App } from './app/app';
import { config } from './app/app.config.server';

/**
 * The entry the prerenderer boots, once per route, at build time.
 *
 * **Nothing here ever runs on a server**, and that is the point: `outputMode`
 * is `static`, so this is used to render every route to an `index.html` on
 * disk and is then thrown away. What ships is HTML, JavaScript and CSS on
 * GitHub Pages — there is no Node process at the other end of a request, which
 * is also why `@angular/ssr` and `@angular/platform-server` are development
 * dependencies rather than dependencies.
 *
 * **The `context` argument is not optional off a browser.** Angular 21 needs it
 * to know which platform it is bootstrapping on; without it the route
 * extraction fails with `NG0401` before a single page is rendered.
 */
const bootstrap = (context: BootstrapContext) => bootstrapApplication(App, config, context);

export default bootstrap;
