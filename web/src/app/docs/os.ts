import { DOCUMENT, Injectable, PLATFORM_ID, inject, signal } from '@angular/core';
import { isPlatformBrowser } from '@angular/common';

/** The three platforms sloop ships for. */
export type OsName = 'windows' | 'linux' | 'macos';

export const OS_ORDER: readonly OsName[] = ['windows', 'linux', 'macos'];

export const OS_LABEL: Readonly<Record<OsName, string>> = {
  windows: 'Windows',
  linux: 'Linux',
  macos: 'macOS',
};

/** Alongside `sloop-theme`, which index.html reads before the first paint. */
const STORAGE_KEY = 'sloop-os';

/**
 * Which operating system the docs are being read on — **global state, not
 * per-page.**
 *
 * `CLAUDE.md`: *pick Windows once and every sidebar page stays on Windows.*
 * That is the whole reason this is a root service rather than a signal inside
 * the tab strip: a reader picking macOS on the installation page and finding
 * Linux again on the automation page has been told the site does not remember
 * anything, three pages before they stop trusting it.
 *
 * Three states, and the third is what makes the first visit right:
 *
 * ```text
 * stored     an explicit choice, and it outranks everything
 * detected   a first visit, resolved from the browser's own platform string
 * linux      no browser to ask — a prerender, or a platform nothing matched
 * ```
 *
 * The choice is resolved in the constructor, which runs while the docs shell
 * is being constructed and therefore before anything is painted — so a reader
 * on a Mac never sees a frame of Windows instructions.
 *
 * `localStorage` throws rather than returning `null` in a few configurations —
 * Safari's private mode historically, any browser with site data blocked — so
 * both the read and the write are guarded. A blocked write costs the choice at
 * the next reload; failing the click over it would cost the click.
 */
@Injectable({ providedIn: 'root' })
export class OsChoice {
  private readonly document = inject(DOCUMENT);
  private readonly isBrowser = isPlatformBrowser(inject(PLATFORM_ID));

  private readonly current = signal<OsName>('linux');

  /** What the tab strip renders as selected, and what a page branches on. */
  readonly os = this.current.asReadonly();

  constructor() {
    if (!this.isBrowser) {
      return;
    }
    this.current.set(this.stored() ?? this.detect());
  }

  select(os: OsName): void {
    this.current.set(os);
    if (!this.isBrowser) {
      return;
    }
    try {
      this.document.defaultView?.localStorage.setItem(STORAGE_KEY, os);
    } catch {
      // Site data blocked. The choice still holds for this session.
    }
  }

  private stored(): OsName | null {
    let value: string | null = null;
    try {
      value = this.document.defaultView?.localStorage.getItem(STORAGE_KEY) ?? null;
    } catch {
      return null;
    }
    return value === 'windows' || value === 'linux' || value === 'macos' ? value : null;
  }

  /**
   * The same read `InstallCommand` does on the landing page, widened to three:
   * `userAgentData.platform` where the browser has it, then the deprecated
   * `navigator.platform`, then the user agent string, which every browser
   * still has something in.
   *
   * An iPhone or an iPad reports a Mac-ish platform and nobody is installing a
   * CLI on either, but macOS is a better guess for that reader than Linux is.
   */
  private detect(): OsName {
    const view = this.document.defaultView;
    const platform =
      (view?.navigator as { userAgentData?: { platform?: string } } | undefined)?.userAgentData
        ?.platform ??
      view?.navigator.platform ??
      view?.navigator.userAgent ??
      '';
    if (/win/i.test(platform)) {
      return 'windows';
    }
    if (/mac|darwin|iphone|ipad|ipod/i.test(platform)) {
      return 'macos';
    }
    return 'linux';
  }
}
