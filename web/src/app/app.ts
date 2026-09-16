import { ChangeDetectionStrategy, Component } from '@angular/core';
import { RouterOutlet } from '@angular/router';

import { Navbar } from './navbar/navbar';

/** The application shell: the navbar, which every route shares, and the outlet. */
@Component({
  selector: 'app-root',
  imports: [Navbar, RouterOutlet],
  templateUrl: './app.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class App {}
