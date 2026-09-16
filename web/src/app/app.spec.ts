import { TestBed } from '@angular/core/testing';
import { provideRouter } from '@angular/router';

import { App } from './app';

describe('App', () => {
  beforeEach(async () => {
    await TestBed.configureTestingModule({
      imports: [App],
      // The shell renders the navbar, which links with routerLink.
      providers: [provideRouter([])],
    }).compileComponents();
  });

  it('bootstraps the shell with the navbar in it', () => {
    const fixture = TestBed.createComponent(App);
    fixture.detectChanges();
    expect(fixture.nativeElement.querySelector('app-navbar')).toBeTruthy();
  });
});
