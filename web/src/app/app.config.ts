import { ApplicationConfig, provideBrowserGlobalErrorListeners } from '@angular/core';
import { provideClientHydration, withEventReplay } from '@angular/platform-browser';
import { provideRouter, withInMemoryScrolling } from '@angular/router';

import { routes } from './app.routes';

export const appConfig: ApplicationConfig = {
  providers: [
    provideBrowserGlobalErrorListeners(),
    provideClientHydration(withEventReplay()),
    provideRouter(
      routes,
      // Angular does not move the scroll position on its own, which is fine
      // with one long page and wrong the moment there are seventeen: following
      // a sidebar link from halfway down a page would open the next one
      // halfway down as well.
      //
      //   scrollPositionRestoration  a new page starts at the top, and going
      //                              back returns to where you were reading
      //   anchorScrolling            a URL with a fragment lands on that
      //                              heading instead of at the top
      //
      // Both honour `scroll-behavior`, which tokens.css collapses under
      // `prefers-reduced-motion`.
      withInMemoryScrolling({
        scrollPositionRestoration: 'enabled',
        anchorScrolling: 'enabled',
      }),
    ),
  ],
};
