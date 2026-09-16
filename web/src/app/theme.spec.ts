import { TestBed } from '@angular/core/testing';

import { Theme, type ThemeChoice } from './theme';

/**
 * The three-way choice is the part worth pinning down: `system` is stored and
 * written like a theme but resolves in CSS, and anything unrecognised has to
 * land on dark rather than on whatever was in storage.
 */
describe('Theme', () => {
  const root = document.documentElement;
  let original: string | null;

  beforeEach(() => {
    original = root.getAttribute('data-theme');
    localStorage.removeItem('sloop-theme');
  });

  afterEach(() => {
    if (original === null) {
      root.removeAttribute('data-theme');
    } else {
      root.setAttribute('data-theme', original);
    }
    localStorage.removeItem('sloop-theme');
  });

  function serviceWith(written: string | null): Theme {
    if (written === null) {
      root.removeAttribute('data-theme');
    } else {
      root.setAttribute('data-theme', written);
    }
    TestBed.resetTestingModule();
    return TestBed.inject(Theme);
  }

  it('reads back what the inline script already resolved', () => {
    for (const written of ['dark', 'light', 'system'] as const) {
      expect(serviceWith(written).choice()).toBe(written);
    }
  });

  it('falls back to dark when the attribute is missing or nonsense', () => {
    expect(serviceWith(null).choice()).toBe('dark');
    expect(serviceWith('chartreuse').choice()).toBe('dark');
  });

  it('writes the choice to the element and to storage', () => {
    const theme = serviceWith('dark');
    for (const choice of ['light', 'system', 'dark'] as ThemeChoice[]) {
      theme.select(choice);
      expect(theme.choice()).toBe(choice);
      expect(root.getAttribute('data-theme')).toBe(choice);
      expect(localStorage.getItem('sloop-theme')).toBe(choice);
    }
  });
});
