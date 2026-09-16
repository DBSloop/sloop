import { ChangeDetectionStrategy, Component, input } from '@angular/core';

/**
 * The background: strata, not a grid.
 *
 * A two-axis hairline grid is the default backdrop of roughly every generated
 * landing page, and the owner said so. This is the alternative, and it is not
 * arbitrary — it is the navbar's wave, repeated and receding.
 *
 * Every band is the same kind of curve that draws the waterline under the mark:
 * half-periods of uneven length and height, so no two lines are alike and the
 * field never resolves into a pattern. They tighten and fade towards the
 * horizon, which is what gives a flat backdrop a distance.
 *
 * Generated in the component rather than written out: fifteen hand-typed path
 * strings would be unreadable and unchangeable, and these are deterministic —
 * the phases come from an integer sequence, not from `Math.random`, so the
 * markup is the same on every render and on a prerender.
 */
@Component({
  selector: 'app-strata',
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <svg
      class="h-full w-full"
      [attr.viewBox]="'0 0 1200 ' + height()"
      preserveAspectRatio="none"
      aria-hidden="true"
      focusable="false"
    >
      @for (band of bands; track band.y) {
        <path
          [attr.d]="band.d"
          fill="none"
          stroke="currentColor"
          [attr.stroke-width]="band.width"
          [attr.stroke-opacity]="band.opacity"
        />
      }
    </svg>
  `,
  styles: `
    :host {
      display: block;
    }
  `,
})
export class Strata {
  /** The viewBox height. The bands are distributed across it. */
  readonly height = input(560);
  /** How many lines. */
  readonly count = input(16);

  protected get bands(): readonly Band[] {
    const total = this.count();
    const height = this.height();
    const out: Band[] = [];

    for (let index = 0; index < total; index += 1) {
      // Eased so the bands crowd towards the top, the way a receding surface
      // does. A linear spread reads as a list; this reads as distance.
      const t = index / Math.max(1, total - 1);
      const y = height * (0.06 + 0.94 * t * t);
      out.push({
        y,
        d: wave(y, index),
        // Nearer bands are heavier and more present; far ones are a whisper.
        width: (0.7 + t * 1.1).toFixed(2),
        opacity: (0.09 + t * 0.34).toFixed(3),
      });
    }
    return out;
  }
}

interface Band {
  readonly y: number;
  readonly d: string;
  readonly width: string;
  readonly opacity: string;
}

/**
 * One band: a run of half-periods of uneven length and height, the same shape
 * as the waterline in the navbar.
 *
 * The lengths and amplitudes come off two small co-prime cycles offset by the
 * band index, so consecutive bands never line up and the whole field stays
 * deterministic.
 */
function wave(y: number, seed: number): string {
  const lengths = [96, 74, 118, 82, 104, 68];
  const amplitudes = [5.5, 3.2, 7.1, 4.4, 2.6, 6.3];

  let d = `M0 ${y.toFixed(2)}`;
  let x = 0;
  let step = 0;
  let up = seed % 2 === 0;

  while (x < 1200) {
    const length = lengths[(seed + step) % lengths.length];
    const amplitude = amplitudes[(seed * 3 + step) % amplitudes.length] * (up ? -1 : 1);
    // Control points at a quarter and three quarters of the span, at 1.32x the
    // amplitude, put the crest of the cubic on the sine.
    d +=
      `c${(length * 0.25).toFixed(2)} ${(amplitude * 1.32).toFixed(2)} ` +
      `${(length * 0.75).toFixed(2)} ${(amplitude * 1.32).toFixed(2)} ` +
      `${length} 0`;
    x += length;
    step += 1;
    up = !up;
  }
  return d;
}
