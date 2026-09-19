import { ApplicationConfig, mergeApplicationConfig } from '@angular/core';
import { provideServerRendering, withRoutes } from '@angular/ssr';

import { appConfig } from './app.config';
import { serverRoutes } from './app.routes.server';

/**
 * The browser configuration plus the one thing rendering off a browser needs.
 *
 * **Deliberately thin.** Everything that differs between a prerender and a
 * browser is already handled where it happens: every component that reaches
 * for `window`, `localStorage` or a `ResizeObserver` does it inside
 * `afterNextRender` or behind `isPlatformBrowser`, and reads the document
 * through the `DOCUMENT` token rather than the global. So there is no
 * server-only replacement to provide here, and a page renders the same whether
 * it was built at 3am on a runner or opened in a tab.
 */
const serverConfig: ApplicationConfig = {
  providers: [provideServerRendering(withRoutes(serverRoutes))],
};

export const config = mergeApplicationConfig(appConfig, serverConfig);
