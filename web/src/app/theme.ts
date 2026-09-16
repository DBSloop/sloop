import { DOCUMENT, Injectable, PLATFORM_ID, inject, signal } from '@angular/core';
import { isPlatformBrowser } from '@angular/common';

/**
 * Three positions, and one of them is not a theme.
 *
 * `system` follows `prefers-color-scheme` from then on, in both directions;
 * tokens.css matches it with `:root:not([data-theme='dark'])` inside the
 * media query. A first visit stores nothing and is dark whatever the OS says.
 */
export type ThemeChoice = 'dark' | 'system' | 'light';

/** Read by the inline script in index.html before the first paint. */
const STORAGE_KEY = 'sloop-theme';

@Injectable({ providedIn: 'root' })
export class Theme {
  private readonly document = inject(DOCUMENT);
  private readonly isBrowser = isPlatformBrowser(inject(PLATFORM_ID));

  private readonly current = signal<ThemeChoice>('dark');

  /** What the control renders as selected. */
  readonly choice = this.current.asReadonly();

  constructor() {
    if (!this.isBrowser) {
      return;
    }
    // The inline script has already resolved this and written it onto <html>,
    // before anything painted. Reading the attribute back rather than deriving
    // the choice a second time means the two can never disagree — there is one
    // resolution, and this is a read of its result.
    const written = this.document.documentElement.getAttribute('data-theme');
    if (written === 'light' || written === 'system' || written === 'dark') {
      this.current.set(written);
    }
  }

  select(choice: ThemeChoice): void {
    this.current.set(choice);
    if (!this.isBrowser) {
      return;
    }
    this.document.documentElement.setAttribute('data-theme', choice);
    try {
      this.document.defaultView?.localStorage.setItem(STORAGE_KEY, choice);
    } catch {
      // Site data blocked, or a private window that refuses writes. The choice
      // still applies to this page; it just will not survive the next load.
      // Failing the click over that would be worse than forgetting it.
    }
  }
}
