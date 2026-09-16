import { ChangeDetectionStrategy, Component, inject } from '@angular/core';
import { RouterLink, RouterLinkActive } from '@angular/router';

import { Mark } from '../mark';
import { Theme, type ThemeChoice } from '../theme';

interface ThemeOption {
  readonly value: ThemeChoice;
  readonly label: string;
  /** Stroke-based, drawn on a 16px grid, one style across the three. */
  readonly path: string;
  readonly extra?: string;
}

@Component({
  selector: 'app-navbar',
  imports: [Mark, RouterLink, RouterLinkActive],
  templateUrl: './navbar.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Navbar {
  private readonly theme = inject(Theme);

  protected readonly choice = this.theme.choice;

  protected readonly themes: readonly ThemeOption[] = [
    {
      value: 'light',
      label: 'Light',
      path: 'M8 1.5v1.7M8 12.8v1.7M14.5 8h-1.7M3.2 8H1.5M12.6 3.4l-1.2 1.2M4.6 11.4l-1.2 1.2M12.6 12.6l-1.2-1.2M4.6 4.6L3.4 3.4',
      extra: 'M8 5.2a2.8 2.8 0 1 0 0 5.6a2.8 2.8 0 0 0 0-5.6z',
    },
    {
      value: 'system',
      label: 'System',
      path: 'M2 4.25a1.5 1.5 0 0 1 1.5-1.5h9a1.5 1.5 0 0 1 1.5 1.5v5.5a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 2 9.75z',
      extra: 'M6 13.75h4M8 11.25v2.5',
    },
    {
      value: 'dark',
      label: 'Dark',
      path: 'M13.4 9.7A5.9 5.9 0 0 1 6.3 2.6a5.9 5.9 0 1 0 7.1 7.1z',
    },
  ];

  protected select(choice: ThemeChoice): void {
    this.theme.select(choice);
  }
}
