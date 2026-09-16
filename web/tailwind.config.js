/**
 * Tailwind 3. This file names things; it does not define them.
 *
 * Every value below is a `var()` pointing at `src/styles/tokens.css`, which is
 * the one place a colour, a size, a radius, a shadow or a duration is written.
 * A number appearing here would be a second place, and the two would drift.
 *
 * Colours are declared as `rgb(var(--ch-x) / <alpha-value>)` so the opacity
 * modifiers still work from a custom property: `bg-surface/60`, `border-line/40`,
 * `text-accent/70`.
 *
 * There is deliberately no `dark:` variant. Theming happens once, in the token
 * layer, by re-pointing `--ch-*`; a component that wrote `bg-white dark:bg-black`
 * would be a second theming mechanism, and the first one to go out of sync.
 * Per the owner (2026-09-16): `clamp()` sizes type, Tailwind's breakpoints do
 * layout, and there are no hand-written `@media` blocks for layout anywhere.
 *
 * @type {import('tailwindcss').Config}
 */
const channel = (name) => `rgb(var(--ch-${name}) / <alpha-value>)`;

module.exports = {
  content: ['./src/**/*.{html,ts}'],
  theme: {
    extend: {
      colors: {
        bg: channel('bg'),
        surface: channel('surface'),
        'surface-2': channel('surface-2'),
        line: channel('line'),
        'line-strong': channel('line-strong'),

        text: channel('text'),
        muted: channel('muted'),
        faint: channel('faint'),

        accent: channel('accent'),
        'accent-soft': channel('accent-soft'),
        brand: channel('brand'),
        'brand-ink': channel('brand-ink'),

        ok: channel('ok'),
        warn: channel('warn'),
        bad: channel('bad'),

        scrim: channel('scrim'),

        'term-bg': channel('term-bg'),
        'term-line': channel('term-line'),
        'term-text': channel('term-text'),
        'term-dim': channel('term-dim'),
        'term-ok': channel('term-ok'),
        'term-warn': channel('term-warn'),
        'term-bad': channel('term-bad'),
      },

      fontFamily: {
        sans: 'var(--font-sans)',
        mono: 'var(--font-mono)',
      },

      // Each step carries its own leading and tracking, so `text-5xl` is a
      // decision about display type rather than only a font-size.
      fontSize: {
        xs: ['var(--fs-xs)', { lineHeight: 'var(--lh-xs)', letterSpacing: 'var(--tr-xs)' }],
        sm: ['var(--fs-sm)', { lineHeight: 'var(--lh-sm)', letterSpacing: 'var(--tr-sm)' }],
        base: ['var(--fs-base)', { lineHeight: 'var(--lh-base)', letterSpacing: 'var(--tr-base)' }],
        lg: ['var(--fs-lg)', { lineHeight: 'var(--lh-lg)', letterSpacing: 'var(--tr-lg)' }],
        xl: ['var(--fs-xl)', { lineHeight: 'var(--lh-xl)', letterSpacing: 'var(--tr-xl)' }],
        '2xl': ['var(--fs-2xl)', { lineHeight: 'var(--lh-2xl)', letterSpacing: 'var(--tr-2xl)' }],
        '3xl': ['var(--fs-3xl)', { lineHeight: 'var(--lh-3xl)', letterSpacing: 'var(--tr-3xl)' }],
        '4xl': ['var(--fs-4xl)', { lineHeight: 'var(--lh-4xl)', letterSpacing: 'var(--tr-4xl)' }],
        '5xl': ['var(--fs-5xl)', { lineHeight: 'var(--lh-5xl)', letterSpacing: 'var(--tr-5xl)' }],
        '6xl': ['var(--fs-6xl)', { lineHeight: 'var(--lh-6xl)', letterSpacing: 'var(--tr-6xl)' }],
        '7xl': ['var(--fs-7xl)', { lineHeight: 'var(--lh-7xl)', letterSpacing: 'var(--tr-7xl)' }],
      },

      borderRadius: {
        sm: 'var(--r-sm)',
        md: 'var(--r-md)',
        lg: 'var(--r-lg)',
        xl: 'var(--r-xl)',
        '2xl': 'var(--r-2xl)',
      },

      boxShadow: {
        1: 'var(--shadow-1)',
        2: 'var(--shadow-2)',
        3: 'var(--shadow-3)',
        glow: 'var(--shadow-glow)',
        none: 'none',
      },

      maxWidth: {
        // Rule 9's capped measure. `max-w-measure` on any block of prose.
        measure: 'var(--measure)',
      },

      transitionDuration: {
        1: 'var(--dur-1)',
        2: 'var(--dur-2)',
        3: 'var(--dur-3)',
        4: 'var(--dur-4)',
      },

      transitionTimingFunction: {
        out: 'var(--ease-out)',
        'in-out': 'var(--ease-in-out)',
        spring: 'var(--ease-spring)',
      },
    },
  },
  plugins: [],
};
