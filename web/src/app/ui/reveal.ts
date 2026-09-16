import {
  DOCUMENT,
  Directive,
  ElementRef,
  PLATFORM_ID,
  inject,
  input,
  afterNextRender,
} from '@angular/core';
import { isPlatformBrowser } from '@angular/common';

/**
 * Arrive on scroll.
 *
 * The element starts translated down and transparent and settles when it first
 * crosses into the viewport — once, never again, because a section that
 * re-animates every time it is scrolled past is a section nobody can read.
 *
 * Three things make it safe rather than decorative:
 *
 * It animates the CSS `translate` property rather than `transform`, which the
 * `Depth` directive owns. The two are separate properties and compose, so an
 * element can arrive and drift at the same time.
 *
 * - **It only ever hides what it will show.** The hidden state is applied from
 *   script, so a visitor with JavaScript off, or a crawler, gets the page with
 *   every section already visible. Hiding content in CSS and revealing it in JS
 *   is how a page ends up blank for the one reader who cannot run it.
 * - **`prefers-reduced-motion` skips it entirely** — nothing is hidden and
 *   nothing transitions.
 * - **No IntersectionObserver, no problem.** The element is shown immediately.
 */
@Directive({
  selector: '[appReveal]',
})
export class Reveal {
  /** Stagger within a group, in milliseconds. */
  readonly appReveal = input(0, { transform: (value: number | string) => Number(value) || 0 });

  private readonly host = inject<ElementRef<HTMLElement>>(ElementRef);
  private readonly document = inject(DOCUMENT);

  constructor() {
    const isBrowser = isPlatformBrowser(inject(PLATFORM_ID));

    afterNextRender(() => {
      if (!isBrowser) {
        return;
      }
      const element = this.host.nativeElement;
      const view = this.document.defaultView;

      const reduced = view?.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;
      if (reduced || typeof view?.IntersectionObserver !== 'function') {
        return;
      }

      // `translate`, not `transform`. They are separate CSS properties that
      // compose, and `Depth` owns `transform` — writing both here meant the two
      // directives overwrote each other, and this one's cleanup stopped the
      // parallax dead on every element that had both.
      element.style.opacity = '0';
      element.style.translate = '0 22px';
      element.style.transition =
        'opacity var(--dur-4) var(--ease-out), translate var(--dur-4) var(--ease-out)';
      element.style.transitionDelay = `${this.appReveal()}ms`;

      // Once it has arrived, every inline style comes back off. `translate3d`
      // promotes the element to its own compositor layer, and leaving one behind
      // on every paragraph of a long page costs memory for the rest of the
      // session — and, as this page proved, keeps text on a layer that does not
      // always end up in a screenshot. The element finishes as plain markup.
      const settle = () => {
        // The transition comes off FIRST. Removing `opacity` while a transition
        // is still declared starts a second animation from wherever the element
        // currently is towards the stylesheet's value — which never finishes if
        // the tab is not painting, and leaves the text invisible until it is.
        // With the transition gone, the removals below take effect instantly and
        // the element is correct whether or not a frame ever runs.
        element.style.removeProperty('transition');
        element.style.removeProperty('transition-delay');
        element.style.removeProperty('opacity');
        element.style.removeProperty('translate');
      };

      // The safety net, checked exactly once — and only for an element that is
      // already in view.
      //
      // IntersectionObserver callbacks are delivered per frame, so a document
      // that is not painting never fires one, and the element would keep the
      // `opacity: 0` it was given at setup. Timers still run when frames do not,
      // so this catches that.
      //
      // The viewport test is the whole point of it. An earlier version armed
      // this unconditionally, which revealed every section on the page two and a
      // half seconds after load — so by the time the reader scrolled down,
      // everything was already showing and there was no arrival left to see. Off
      // screen, this does nothing and the observer is left to do its job.
      view.setTimeout(() => {
        const box = element.getBoundingClientRect();
        if (box.top < view.innerHeight && box.bottom > 0) {
          settle();
        }
      }, 2500);

      const observer = new view.IntersectionObserver(
        (entries) => {
          for (const entry of entries) {
            if (!entry.isIntersecting) {
              continue;
            }
            element.style.opacity = '1';
            element.style.translate = '0 0';
            element.addEventListener('transitionend', settle, { once: true });
            // transitionend does not fire if the transition never starts —
            // a display:none ancestor, a tab backgrounded mid-flight — so the
            // element is never left stuck part-way.
            view.setTimeout(settle, this.appReveal() + 1200);
            observer.disconnect();
          }
        },
        // A little into the viewport, so the movement is seen rather than
        // finished by the time the section is actually looked at.
        { rootMargin: '0px 0px -12% 0px', threshold: 0.01 },
      );
      observer.observe(element);
    });
  }
}
