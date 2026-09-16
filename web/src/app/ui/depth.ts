import {
  DOCUMENT,
  Directive,
  ElementRef,
  PLATFORM_ID,
  afterNextRender,
  inject,
  input,
} from '@angular/core';
import { isPlatformBrowser } from '@angular/common';

/**
 * Depth: scroll-driven parallax, plus a drift of the element's own.
 *
 * Two motions compose into one transform:
 *
 * - **Scroll.** The element drifts, turns and tilts according to where it sits
 *   in the viewport. Scrolling back runs it backwards. This is the transport.
 * - **Ambient.** A slow sine bob and roll on top, per element, with its own
 *   phase so neighbours never move in lockstep. This is the *"some things may
 *   move or rotate or animate by it's own"* half, and it is what makes a card
 *   read as floating rather than as parked at an angle.
 *
 * Three things this gets right that the first version did not:
 *
 * - **It owns `transform` alone.** `Reveal` animates the separate CSS `translate`
 *   property, so the two compose instead of overwriting each other — previously
 *   `Reveal`'s cleanup removed `transform` and stopped the parallax dead on any
 *   element that had both.
 * - **It measures the element's resting position once**, not its transformed
 *   box every frame. Reading `getBoundingClientRect` after writing a transform
 *   feeds the output back into the input, which damps the motion towards
 *   nothing — the reason the first version moved about 20px and looked static.
 * - **One loop drives every element.** The browser already stops delivering
 *   animation frames to a tab that is not being painted, so there is no separate
 *   visibility check to keep in step with it.
 *
 * `prefers-reduced-motion` opts out entirely: nothing is registered and no
 * transform is ever written.
 */
@Directive({
  selector: '[appDepth]',
})
export class Depth {
  /** Vertical drift in px across a screen of scrolling. Negative rises. */
  readonly appDepth = input(0, { transform: toNumber });
  /** Degrees of rotateY across the same distance. */
  readonly depthTurn = input(0, { transform: toNumber });
  /** Degrees of rotateX across the same distance. */
  readonly depthTilt = input(0, { transform: toNumber });
  /** A constant translateZ in px, so a card sits in front of its neighbours. */
  readonly depthLift = input(0, { transform: toNumber });
  /** Amplitude in px of the ambient bob. */
  readonly depthFloat = input(0, { transform: toNumber });
  /** Amplitude in degrees of the ambient roll. */
  readonly depthSpin = input(0, { transform: toNumber });
  /** Seconds of phase offset, so siblings drift out of step. */
  readonly depthPhase = input(0, { transform: toNumber });
  /**
   * The element's resting angles, in degrees. These belong here rather than in
   * a `transform` class on the element, because this directive writes the whole
   * `transform` and a class would simply be overwritten.
   */
  readonly depthAngle = input(0, { transform: toNumber });
  readonly depthPitch = input(0, { transform: toNumber });

  private readonly host = inject<ElementRef<HTMLElement>>(ElementRef);
  private readonly document = inject(DOCUMENT);

  constructor() {
    const isBrowser = isPlatformBrowser(inject(PLATFORM_ID));
    afterNextRender(() => {
      const view = this.document.defaultView;
      if (!isBrowser || !view) {
        return;
      }
      if (view.matchMedia?.('(prefers-reduced-motion: reduce)').matches) {
        return;
      }
      const element = this.host.nativeElement;
      element.style.willChange = 'transform';
      register(view, {
        element,
        drift: this.appDepth(),
        turn: this.depthTurn(),
        tilt: this.depthTilt(),
        lift: this.depthLift(),
        float: this.depthFloat(),
        spin: this.depthSpin(),
        phase: this.depthPhase(),
        angle: this.depthAngle(),
        pitch: this.depthPitch(),
        // Where the element's centre rests in the document, measured before a
        // transform has ever been written to it.
        anchor: element.getBoundingClientRect().top + view.scrollY + element.offsetHeight / 2,
      });
    });
  }
}

function toNumber(value: number | string): number {
  return Number(value) || 0;
}

interface Entry {
  readonly element: HTMLElement;
  readonly drift: number;
  readonly turn: number;
  readonly tilt: number;
  readonly lift: number;
  readonly float: number;
  readonly spin: number;
  readonly phase: number;
  readonly angle: number;
  readonly pitch: number;
  anchor: number;
}

const entries: Entry[] = [];
let running = false;

function register(view: Window, entry: Entry): void {
  entries.push(entry);
  if (running) {
    return;
  }
  running = true;

  // Re-measure resting positions when the layout changes. The transform has to
  // come off first or the old offset is baked into the new anchor.
  view.addEventListener(
    'resize',
    () => {
      for (const item of entries) {
        item.element.style.transform = '';
      }
      for (const item of entries) {
        item.anchor =
          item.element.getBoundingClientRect().top + view.scrollY + item.element.offsetHeight / 2;
      }
    },
    { passive: true },
  );

  const frame = (time: number) => {
    apply(view, time / 1000);
    view.requestAnimationFrame(frame);
  };
  view.requestAnimationFrame(frame);
}

function apply(view: Window, seconds: number): void {
  const height = view.innerHeight || 1;
  const middle = view.scrollY + height / 2;

  for (const entry of entries) {
    // +1 when the element's resting centre is a screen above the middle of the
    // viewport, -1 when it is a screen below. Independent of any transform
    // already written, so the motion cannot damp itself away.
    const progress = clamp((middle - entry.anchor) / height, -1.4, 1.4);
    if (progress < -1.35 || progress > 1.35) {
      continue; // Well off screen.
    }

    const wobble = entry.phase + seconds;
    const y = progress * entry.drift + Math.sin(wobble * 0.62) * entry.float;
    const turn = entry.angle + progress * entry.turn + Math.sin(wobble * 0.47) * entry.spin;
    const tilt = entry.pitch + progress * entry.tilt + Math.cos(wobble * 0.53) * entry.spin * 0.6;

    let transform = `translate3d(0, ${y.toFixed(2)}px, 0)`;
    if (entry.lift !== 0) {
      transform += ` translateZ(${entry.lift}px)`;
    }
    if (turn !== 0) {
      transform += ` rotateY(${turn.toFixed(3)}deg)`;
    }
    if (tilt !== 0) {
      transform += ` rotateX(${tilt.toFixed(3)}deg)`;
    }
    entry.element.style.transform = transform;
  }
}

function clamp(value: number, low: number, high: number): number {
  return value < low ? low : value > high ? high : value;
}
