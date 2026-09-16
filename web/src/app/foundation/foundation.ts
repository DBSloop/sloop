import { ChangeDetectionStrategy, Component } from '@angular/core';

/** One step of the type scale, as it is shown on the specimen. */
interface TypeStep {
  /** The literal Tailwind class. Written out because the JIT scanner reads
   *  source text — a class assembled at runtime would never be generated. */
  readonly cls: string;
  readonly token: string;
  readonly sample: string;
}

interface Swatch {
  readonly cls: string;
  readonly token: string;
  readonly note?: string;
}

interface SwatchGroup {
  readonly title: string;
  readonly blurb: string;
  readonly swatches: readonly Swatch[];
}

@Component({
  selector: 'app-foundation',
  templateUrl: './foundation.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Foundation {
  /** The figlet block the owner fixed as the CLI's wordmark, 26 columns wide.
   *  Here it does a second job: it is the hardest thing on the site for a
   *  monospace face to render, so if the terminal block below looks right,
   *  the font stack and the terminal tokens are both correct. */
  protected readonly wordmark = [
    '      _                   ',
    '     | |                  ',
    '  ___| | ___   ___  _ __  ',
    " / __| |/ _ \\ / _ \\| '_ \\ ",
    ' \\__ \\ | (_) | (_) | |_) |',
    ' |___/_|\\___/ \\___/| .__/ ',
    '                   | |    ',
    '                   |_|    ',
  ].join('\n');

  protected readonly typeScale: readonly TypeStep[] = [
    { cls: 'text-7xl', token: '7xl', sample: 'Back up. Restore.' },
    { cls: 'text-6xl', token: '6xl', sample: 'Mirror and sync' },
    { cls: 'text-5xl', token: '5xl', sample: 'Register once, run anywhere' },
    { cls: 'text-4xl', token: '4xl', sample: 'Your credentials never leave' },
    { cls: 'text-3xl', token: '3xl', sample: 'PostgreSQL, MySQL and MariaDB' },
    { cls: 'text-2xl', token: '2xl', sample: 'A menu, or a flag. Both do the same work.' },
    { cls: 'text-xl', token: 'xl', sample: 'Windows, Linux and macOS, from one binary.' },
    {
      cls: 'text-lg',
      token: 'lg',
      sample: 'Every interactive run ends by printing the flag equivalent.',
    },
    {
      cls: 'text-base',
      token: 'base',
      sample:
        'Body copy sits here, and stops at seventy characters however wide the window gets, because a line a reader has to track back across is a line they read twice.',
    },
    {
      cls: 'text-sm',
      token: 'sm',
      sample: 'Secondary copy, captions and the explanatory line under a heading.',
    },
    { cls: 'text-xs', token: 'xs', sample: 'LABELS, METADATA AND TABLE HEADS' },
  ];

  protected readonly palette: readonly SwatchGroup[] = [
    {
      title: 'Surfaces',
      blurb: 'The page, what sits on it, and the lines between.',
      swatches: [
        { cls: 'bg-bg', token: 'bg', note: 'the page' },
        { cls: 'bg-surface', token: 'surface', note: 'a panel' },
        { cls: 'bg-surface-2', token: 'surface-2', note: 'hover' },
        { cls: 'bg-line', token: 'line', note: 'hairline' },
        { cls: 'bg-line-strong', token: 'line-strong', note: 'seen' },
      ],
    },
    {
      title: 'Text',
      blurb: 'Three tiers, each measured against the page rather than guessed at.',
      swatches: [
        { cls: 'bg-text', token: 'text', note: '15.8:1' },
        { cls: 'bg-muted', token: 'muted', note: '7.4:1' },
        { cls: 'bg-faint', token: 'faint', note: '4.8:1' },
      ],
    },
    {
      title: 'Accent',
      blurb:
        'brand is Claude Code orange and never moves. accent is the same orange on dark and a darkened terracotta on light, because #D97757 reads at 3:1 on a white page and that fails.',
      swatches: [
        { cls: 'bg-brand', token: 'brand', note: '#D97757' },
        { cls: 'bg-accent', token: 'accent', note: 'readable' },
        { cls: 'bg-accent-soft', token: 'accent-soft', note: 'tint' },
        { cls: 'bg-brand-ink', token: 'brand-ink', note: 'on brand' },
      ],
    },
    {
      title: 'Status',
      blurb: 'A run succeeded, a run needs looking at, a run failed.',
      swatches: [
        { cls: 'bg-ok', token: 'ok' },
        { cls: 'bg-warn', token: 'warn' },
        { cls: 'bg-bad', token: 'bad' },
      ],
    },
    {
      title: 'Terminal',
      blurb:
        'Outside the theme switch on purpose. A terminal is dark in both themes, so a CLI block on the light site still reads as a terminal rather than a grey box — and the statuses printed inside one have to stay outside the switch with it, or the light palette puts a 3.2:1 green on a near-black background.',
      swatches: [
        { cls: 'bg-term-bg', token: 'term-bg' },
        { cls: 'bg-term-line', token: 'term-line' },
        { cls: 'bg-term-text', token: 'term-text' },
        { cls: 'bg-term-dim', token: 'term-dim' },
        { cls: 'bg-term-ok', token: 'term-ok' },
        { cls: 'bg-term-warn', token: 'term-warn' },
        { cls: 'bg-term-bad', token: 'term-bad' },
      ],
    },
  ];

  protected readonly radii: readonly { cls: string; token: string }[] = [
    { cls: 'rounded-sm', token: 'sm' },
    { cls: 'rounded-md', token: 'md' },
    { cls: 'rounded-lg', token: 'lg' },
    { cls: 'rounded-xl', token: 'xl' },
    { cls: 'rounded-2xl', token: '2xl' },
  ];

  protected readonly depths: readonly { cls: string; token: string; note: string }[] = [
    { cls: 'shadow-1', token: 'shadow-1', note: 'a resting card' },
    { cls: 'shadow-2', token: 'shadow-2', note: 'lifted, hovered' },
    { cls: 'shadow-3', token: 'shadow-3', note: 'a dialog' },
    { cls: 'shadow-glow', token: 'shadow-glow', note: 'the accent, once' },
  ];

  protected readonly motion: readonly { cls: string; token: string; note: string }[] = [
    { cls: 'duration-1', token: 'dur-1', note: 'a colour change under the cursor' },
    { cls: 'duration-2', token: 'dur-2', note: 'a button, a tab' },
    { cls: 'duration-3', token: 'dur-3', note: 'a panel opening' },
    { cls: 'duration-4', token: 'dur-4', note: 'something arriving on scroll' },
  ];
}
