import {
  ChangeDetectionStrategy,
  Component,
  DOCUMENT,
  PLATFORM_ID,
  inject,
  signal,
} from '@angular/core';
import { isPlatformBrowser } from '@angular/common';

type Os = 'windows' | 'unix';

/**
 * The install one-liner, on the operating system the visitor is actually using,
 * with the other one a click away.
 *
 * Both commands are in the DOM whichever tab is showing, so a crawler and a
 * reader on the wrong machine both get the whole answer. The detection only
 * decides which one is selected first.
 *
 * The copy button falls back to a range selection when the clipboard is refused
 * — which it is on an insecure origin, and in a browser that has not granted
 * the permission. Refusing to copy and saying nothing would be worse.
 */
@Component({
  selector: 'app-install-command',
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <div
      class="overflow-hidden rounded-xl border border-term-line bg-term-bg shadow-2"
      [class.shadow-glow]="copied()"
    >
      <div class="flex items-center gap-1 border-b border-term-line px-2 py-1.5">
        @for (option of options; track option.os) {
          <button
            type="button"
            [attr.aria-pressed]="os() === option.os"
            [class]="
              os() === option.os
                ? 'bg-term-line text-term-text'
                : 'text-term-dim hover:text-term-text'
            "
            class="rounded-md px-3 py-1.5 font-mono text-xs transition-colors duration-2 ease-out"
            (click)="os.set(option.os)"
          >
            {{ option.label }}
          </button>
        }
      </div>

      <div class="flex items-stretch gap-3">
        <div class="min-w-0 flex-1 overflow-x-auto px-4 py-3.5 sm:px-5">
          <pre class="m-0 font-mono text-xs leading-relaxed text-term-text sm:text-sm"><code
          ><span class="select-none text-brand">$ </span>{{ command() }}</code></pre>
        </div>

        <button
          type="button"
          class="flex shrink-0 items-center gap-2 border-l border-term-line px-4 font-mono text-xs text-term-dim transition-colors duration-2 ease-out hover:bg-term-line hover:text-term-text"
          [attr.aria-label]="'Copy the ' + label() + ' install command'"
          (click)="copy()"
        >
          @if (copied()) {
            <svg
              class="h-4 w-4 text-term-ok"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              stroke-width="1.6"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
              focusable="false"
            >
              <path d="M3 8.5 6.2 11.7 13 4.9" />
            </svg>
            <span class="hidden sm:inline">Copied</span>
          } @else {
            <svg
              class="h-4 w-4"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              stroke-width="1.5"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
              focusable="false"
            >
              <rect x="5.75" y="5.75" width="8.5" height="8.5" rx="2" />
              <path d="M10.25 2.75H3.75a1 1 0 0 0-1 1v6.5" />
            </svg>
            <span class="hidden sm:inline">Copy</span>
          }
        </button>
      </div>
    </div>
  `,
})
export class InstallCommand {
  private readonly document = inject(DOCUMENT);
  private readonly isBrowser = isPlatformBrowser(inject(PLATFORM_ID));

  protected readonly options = [
    { os: 'windows' as const, label: 'Windows' },
    { os: 'unix' as const, label: 'Linux / macOS' },
  ];

  protected readonly os = signal<Os>(this.detect());
  protected readonly copied = signal(false);

  protected label(): string {
    return this.os() === 'windows' ? 'Windows' : 'Linux and macOS';
  }

  protected command(): string {
    return this.os() === 'windows'
      ? 'irm https://dbsloop.github.io/install.ps1 | iex'
      : 'curl -fsSL https://dbsloop.github.io/install.sh | sh';
  }

  private detect(): Os {
    if (!this.isBrowser) {
      return 'unix';
    }
    const view = this.document.defaultView;
    const platform =
      (view?.navigator as { userAgentData?: { platform?: string } } | undefined)?.userAgentData
        ?.platform ??
      view?.navigator.platform ??
      view?.navigator.userAgent ??
      '';
    return /win/i.test(platform) ? 'windows' : 'unix';
  }

  protected async copy(): Promise<void> {
    const text = this.command();
    let ok = false;
    try {
      await this.document.defaultView?.navigator.clipboard.writeText(text);
      ok = true;
    } catch {
      // Insecure origin, or the permission was refused. Select the command
      // instead, so the keyboard shortcut still works and the visitor can see
      // what they are about to copy.
      const pre = this.document.querySelector('app-install-command pre');
      const selection = this.document.defaultView?.getSelection();
      if (pre && selection) {
        const range = this.document.createRange();
        range.selectNodeContents(pre);
        selection.removeAllRanges();
        selection.addRange(range);
      }
    }
    if (!ok) {
      return;
    }
    this.copied.set(true);
    this.document.defaultView?.setTimeout(() => this.copied.set(false), 1600);
  }
}
