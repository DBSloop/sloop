import { ChangeDetectionStrategy, Component, inject } from '@angular/core';

import { OS_LABEL, OS_ORDER, OsChoice, type OsName } from './os';

/**
 * The OS tab strip.
 *
 * It sits in the docs toolbar, which is chrome above every page rather than
 * content inside one — because the choice it holds is global. Picking macOS on
 * the installation page and finding Linux again on the automation page would
 * tell a reader the site forgets things, and they would stop trusting the rest
 * of it too.
 *
 * **Buttons in a group, not `role="tablist"`.** A tablist promises arrow-key
 * navigation with a roving tabindex, and a tabpanel for each tab; this strip
 * changes fragments scattered through a document rather than swapping one
 * panel, and promising the keyboard contract without implementing it is worse
 * than not claiming it. Three buttons, each reachable with Tab, each announcing
 * whether it is the one in effect — the same call the navbar's theme control
 * made, for the same reason.
 */
@Component({
  selector: 'app-os-tabs',
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <div class="flex items-center gap-2.5">
      <span class="hidden shrink-0 font-mono text-xs text-faint lg:inline">Showing for</span>
      <div
        class="flex shrink-0 items-center gap-0.5 rounded-full border border-line bg-surface p-1"
        role="group"
        aria-label="Operating system"
      >
        @for (name of order; track name) {
          <button
            type="button"
            [attr.aria-pressed]="os() === name"
            [class]="
              os() === name
                ? 'bg-accent-soft text-accent ring-1 ring-accent/25'
                : 'text-faint hover:bg-surface-2 hover:text-muted'
            "
            class="rounded-full px-2.5 py-1 text-xs font-medium transition-[color,background-color] duration-2 ease-out sm:px-3.5 sm:py-1.5"
            (click)="select(name)"
          >
            {{ label[name] }}
          </button>
        }
      </div>
    </div>
  `,
})
export class OsTabs {
  private readonly choice = inject(OsChoice);

  protected readonly order = OS_ORDER;
  protected readonly label = OS_LABEL;
  protected readonly os = this.choice.os;

  protected select(name: OsName): void {
    this.choice.select(name);
  }
}
