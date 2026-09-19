import {
  ChangeDetectionStrategy,
  Component,
  DOCUMENT,
  DestroyRef,
  ElementRef,
  PLATFORM_ID,
  afterNextRender,
  computed,
  inject,
  input,
  signal,
  viewChild,
} from '@angular/core';
import { isPlatformBrowser } from '@angular/common';

/** One line of a terminal block. */
export interface TerminalLine {
  /**
   * How the line is coloured, matching what `cli/src/style.rs` actually paints.
   *
   * One rule for all of them: **`tag` carries the colour, `text` is ordinary
   * terminal text, and `note` is dim.** So a line is written in the three
   * pieces the CLI writes it in, and none of them has to be faked.
   *
   * ```text
   * prompt   an accent caret, then the command in `text`
   * head     `style::heading` — the accent, bold
   * name     `style::paint`   — the accent, a value worth noticing
   * label    `style::label`   — dim, in front of a plain value
   * dim      `style::dim`     — the whole line
   * ok/warn/bad               — the three status colours
   * ```
   */
  readonly kind?: 'prompt' | 'out' | 'dim' | 'ok' | 'warn' | 'bad' | 'head' | 'name' | 'label';
  /** For every kind but `prompt`, `dim` and `out`, the part that carries the colour. */
  readonly tag?: string;
  /** A single space renders as a blank line; an empty string would collapse. */
  readonly text: string;
  /** Trailing dim text, for the `· 1,440 readings` half of a line. */
  readonly note?: string;
}

/** How long each line takes when the block is played rather than printed. */
const MS_PER_CHARACTER = 26;
const AFTER_A_COMMAND = 320;
const BETWEEN_OUTPUT_LINES = 90;
const BEFORE_REPLAY_IS_OFFERED = 900;

/**
 * A terminal block, and every character in it is real HTML text.
 *
 * Selectable, copyable, searchable, read by a crawler and by a model. Never an
 * image, never an SVG of text, never a screenshot — the owner's rule, and the
 * reason the site can be indexed at all.
 *
 * Lines are block-level spans with `whitespace-pre` rather than one `<pre>` full
 * of newline characters: leading indentation survives, copying still yields one
 * line per line, and the template needs no whitespace gymnastics to stop the
 * formatter from rewriting the output.
 *
 * The block stays dark in both themes. A terminal is dark; a CLI block that
 * turned into a grey card on the light site would stop reading as a terminal,
 * which is why `--ch-term-*` sits outside the theme switch in tokens.css.
 *
 * ## `play`
 *
 * With `play`, the block runs itself the first time it is scrolled to: the
 * command types, then its output arrives a line at a time, then the caret sits
 * and blinks the way a prompt does. A replay button appears once it has
 * finished.
 *
 * **Nothing is ever missing from the DOM.** The typed line is the whole string
 * in a span clipped to a width in `ch`, and an output line that has not arrived
 * yet is at `opacity: 0` — so a crawler, a model, a reader with scripting off
 * and anyone who hits `Ctrl+A` all get the complete transcript. The hidden
 * state is applied from script, never from the stylesheet, which is the same
 * rule `Reveal` follows and for the same reason.
 *
 * `prefers-reduced-motion` opts out entirely: the block is printed, finished,
 * with no caret and no replay.
 */
@Component({
  selector: 'app-terminal',
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <figure
      class="relative m-0 overflow-hidden rounded-xl border border-term-line bg-term-bg shadow-2"
    >
      <!-- The light along the top edge. A window has a lit edge where it
           catches the room; without it a rounded rectangle is a rectangle. -->
      <span
        aria-hidden="true"
        class="pointer-events-none absolute inset-x-0 top-0 h-px bg-gradient-to-r from-transparent via-term-dim/30 to-transparent"
      ></span>

      <figcaption
        class="flex items-center gap-2 border-b border-term-line px-4 py-2.5 font-mono text-xs text-term-dim"
      >
        <!-- Close, minimise, zoom, in the colours macOS uses. Chrome, not
             meaning: the palette's own ok/warn/bad say something inside the
             block and these three say "this is a window". -->
        <span class="flex shrink-0 items-center gap-2" aria-hidden="true">
          <span class="h-2.5 w-2.5 rounded-full bg-light-close"></span>
          <span class="h-2.5 w-2.5 rounded-full bg-light-min"></span>
          <span class="h-2.5 w-2.5 rounded-full bg-light-zoom"></span>
        </span>

        <span class="ml-1.5 truncate">{{ caption() }}</span>

        <span class="ml-auto flex shrink-0 items-center gap-1">
          @if (canReplay()) {
            <button
              type="button"
              class="flex shrink-0 items-center gap-1.5 rounded-md px-2 py-1 font-mono text-xs text-term-dim transition-colors duration-2 ease-out hover:bg-term-line hover:text-term-text"
              (click)="replay()"
            >
              <svg
                class="h-3.5 w-3.5"
                viewBox="0 0 16 16"
                fill="none"
                stroke="currentColor"
                stroke-width="1.5"
                stroke-linecap="round"
                stroke-linejoin="round"
                aria-hidden="true"
                focusable="false"
              >
                <path d="M13.5 8a5.5 5.5 0 1 1-1.9-4.16" />
                <path d="M13.2 2.3v2.9h-2.9" />
              </svg>
              Run it again
            </button>
          } @else if (meta()) {
            <span class="shrink-0 truncate text-term-dim/80">{{ meta() }}</span>
          }

          @if (canCopy()) {
            <button
              type="button"
              class="flex shrink-0 items-center gap-1.5 rounded-md px-2 py-1 font-mono text-xs transition-colors duration-2 ease-out hover:bg-term-line hover:text-term-text"
              [class]="copied() ? 'text-term-ok' : 'text-term-dim'"
              [attr.aria-label]="
                commands().length > 1
                  ? 'Copy all ' + commands().length + ' commands'
                  : 'Copy the command'
              "
              (click)="copyCommands()"
            >
              @if (copied()) {
                <svg
                  class="h-3.5 w-3.5"
                  viewBox="0 0 16 16"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.6"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                  aria-hidden="true"
                  focusable="false"
                >
                  <path d="M3 8.5 6.2 11.7 13 4.9" />
                </svg>
                Copied
              } @else {
                <svg
                  class="h-3.5 w-3.5"
                  viewBox="0 0 16 16"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.5"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                  aria-hidden="true"
                  focusable="false"
                >
                  <rect x="5.75" y="5.75" width="8.5" height="8.5" rx="2" />
                  <path d="M10.25 2.75H3.75a1 1 0 0 0-1 1v6.5" />
                </svg>
                Copy
              }
            </button>
          }
        </span>
      </figcaption>

      <div
        #scroller
        class="scroller overflow-x-auto px-4 py-4 sm:px-5"
        [class.is-clipped]="clipped()"
        (scroll)="onScroll()"
      >
        <code [class]="bodyClass()">
          @for (line of lines(); track $index; let i = $index) {
            <span
              class="block whitespace-pre transition-opacity duration-2 ease-out"
              [style.opacity]="opacityOf(i)"
            >
              @switch (line.kind) {
                @case ('prompt') {
                  <span class="select-none text-brand">$ </span>
                  <span
                    class="inline-block max-w-full overflow-hidden whitespace-pre align-bottom text-term-text"
                    [style.width]="widthOf(i, line)"
                    >{{ line.text }}</span
                  >
                  @if (caretOn() === i) {
                    <span
                      aria-hidden="true"
                      class="ml-px inline-block h-[1.05em] w-[0.55ch] translate-y-[0.18em] animate-blink bg-brand align-bottom"
                    ></span>
                  }
                }
                @case ('dim') {
                  <span class="text-term-dim">{{ line.text }}</span>
                }
                @case ('head') {
                  <span class="font-semibold text-brand">{{ line.tag }}</span>
                  <span class="text-term-text">{{ line.text }}</span>
                }
                @case ('name') {
                  <span class="text-brand">{{ line.tag }}</span>
                  <span class="text-term-text">{{ line.text }}</span>
                }
                @case ('label') {
                  <span class="text-term-dim">{{ line.tag }}</span>
                  <span class="text-term-text">{{ line.text }}</span>
                }
                @case ('ok') {
                  <span class="text-term-ok">{{ line.tag }}</span>
                  <span class="text-term-text">{{ line.text }}</span>
                }
                @case ('warn') {
                  <span class="text-term-warn">{{ line.tag }}</span>
                  <span class="text-term-text">{{ line.text }}</span>
                }
                @case ('bad') {
                  <span class="text-term-bad">{{ line.tag }}</span>
                  <span class="text-term-text">{{ line.text }}</span>
                }
                @default {
                  <span class="text-term-text">{{ line.text }}</span>
                }
              }
              @if (line.note) {
                <span class="text-term-dim">{{ line.note }}</span>
              }
            </span>
          }
        </code>
      </div>
    </figure>
  `,
  // The page's own scrollbar is sized for the page. Inside a terminal block it
  // is a pale capsule on near-black, so a transcript one character too wide
  // gains a bar heavier than the text above it. This one is the block's own
  // hairline, and it comes from the same `--ch-term-*` tokens as the frame.
  //
  // The `@supports` fence is base.css's, for base.css's reason: Chrome and Edge
  // drop ::-webkit-scrollbar entirely as soon as `scrollbar-color` is set, so
  // setting both would throw away the designed one. The test is false in every
  // engine that has the pseudo-elements.
  styles: `
    @supports not selector(::-webkit-scrollbar) {
      .scroller {
        scrollbar-width: thin;
        scrollbar-color: rgb(var(--ch-term-dim) / 0.55) transparent;
      }
    }

    .scroller::-webkit-scrollbar {
      height: 9px;
    }

    .scroller::-webkit-scrollbar-track {
      background-color: transparent;
    }

    /* Resting, this used to be --ch-term-line: 34,31,29 on a background of
       9,8,8. That is a contrast of about 1.1:1 — a scrollbar that is there and
       cannot be seen, which is worse than none, because the reader concludes
       the line is simply cut off. It is the block's own dim grey now, and
       brightens further on hover. */
    .scroller::-webkit-scrollbar-thumb {
      background-color: rgb(var(--ch-term-dim) / 0.55);
      border: 3px solid transparent;
      background-clip: content-box;
      border-radius: 999px;
      transition: background-color var(--dur-2) var(--ease-out);
    }

    .scroller:hover::-webkit-scrollbar-thumb {
      background-color: rgb(var(--ch-term-dim) / 0.85);
    }

    /* And the second half of the same problem: a scrollbar says there is more
       only to somebody who looks down. A line that runs off the right edge
       fades out, so it reads as continuing rather than as ending there. The
       class comes off as soon as the block is scrolled to its end. */
    .scroller.is-clipped {
      -webkit-mask-image: linear-gradient(to right, #000 calc(100% - 3rem), transparent 100%);
      mask-image: linear-gradient(to right, #000 calc(100% - 3rem), transparent 100%);
    }
  `,
})
export class Terminal {
  readonly caption = input('sloop');
  readonly lines = input.required<readonly TerminalLine[]>();
  /** A quiet note at the right of the title bar — a duration, a version. */
  readonly meta = input('');
  /** Type the commands and play the output the first time it is scrolled to. */
  readonly play = input(false, { transform: (value: boolean | string) => value !== false });
  /**
   * Hold the small size at every width.
   *
   * For the windows floating in the hero. They are ornament at 224–348px, and
   * a block that steps up to `text-sm` at 640px needs about a sixth more room
   * for the same line — which is a sixth of the stage the mark does not get.
   */
  readonly compact = input(false, { transform: (value: boolean | string) => value !== false });

  /**
   * Offer a copy button, and copy the commands rather than the transcript.
   *
   * **Opt-in, and deliberately not the default.** The owner's rule: *only use
   * copy button if user needs to copy that button, not for example shell
   * window.* Most blocks on this site are showing what something prints — a
   * listing, a status screen, a verified backup — and a copy button on one of
   * those offers to put somebody else's output on your clipboard, which is a
   * button that can only be pressed by mistake. A block sets this when the
   * commands in it are meant to be run as they stand.
   *
   * What it copies is the `prompt` lines, without the `$ `, one per line — so
   * a five-line sequence pastes as a five-line sequence and the output
   * interleaved between them is left behind. A block with no command in it
   * gets no button whatever this says.
   */
  readonly copy = input(false, { transform: (value: boolean | string) => value !== false });

  /**
   * `min-w-max` is load-bearing, and its absence was a real bug.
   *
   * Each line is a block-level span with `white-space: pre`. A block box takes
   * its containing block's width whatever is inside it, so a line longer than
   * the block **overflowed its own box without making the box any wider** —
   * `scrollWidth` stayed equal to `clientWidth`, the scroller had nothing to
   * scroll, and the end of a long command was simply clipped with no scrollbar
   * and no way to reach it. `min-width: max-content` sizes the body to its
   * longest line, so the overflow becomes real and the block scrolls.
   *
   * Not on `compact`: those are the hero's ornament windows, each already cut
   * to fit its own longest line, and a scrollbar on a 224px decoration is
   * clutter rather than an affordance.
   */
  protected readonly bodyClass = computed(() =>
    this.compact()
      ? 'block font-mono text-xs leading-relaxed'
      : 'block min-w-max font-mono text-xs leading-relaxed sm:text-sm',
  );

  /** The lines somebody would actually type, in order. */
  protected readonly commands = computed(() =>
    this.lines()
      .filter((line) => line.kind === 'prompt')
      .map((line) => line.text),
  );

  protected readonly canCopy = computed(
    () => this.copy() && !this.compact() && this.commands().length > 0,
  );
  protected readonly copied = signal(false);

  /**
   * True while the block is scrolled horizontally and there is more to the
   * right. Drives the fade at the right edge — see the note in `styles`.
   */
  protected readonly clipped = signal(false);

  /**
   * How far the run has got: the index of the line being written, and how many
   * characters of it are visible. `done` is the state everything starts in, so
   * a block that never plays — no script, reduced motion, an older browser —
   * is a finished transcript rather than an empty one.
   */
  private readonly at = signal<{ line: number; characters: number } | null>(null);
  private readonly finished = signal(true);

  protected readonly caretOn = computed(() => this.at()?.line ?? -1);
  protected readonly canReplay = computed(
    () => this.play() && this.finished() && this.everPlayed(),
  );
  private readonly everPlayed = signal(false);

  private readonly host = inject<ElementRef<HTMLElement>>(ElementRef);
  private readonly document = inject(DOCUMENT);
  private readonly scroller = viewChild.required<ElementRef<HTMLElement>>('scroller');
  private timer = 0;
  private copyTimer = 0;
  private watcher: ResizeObserver | undefined;

  /**
   * Put the commands on the clipboard.
   *
   * The fallback matters more here than it looks: the clipboard API is refused
   * on an insecure origin and in a browser that has not granted the
   * permission, and a Copy button that silently does nothing is worse than no
   * button. When it is refused, the block is selected instead, so the
   * keyboard shortcut still works and the reader can see what they are about
   * to take.
   */
  protected async copyCommands(): Promise<void> {
    const view = this.document.defaultView;
    const text = this.commands().join('\n');
    try {
      await view?.navigator.clipboard.writeText(text);
    } catch {
      const body = this.host.nativeElement.querySelector('code');
      const selection = view?.getSelection();
      if (body && selection) {
        const range = this.document.createRange();
        range.selectNodeContents(body);
        selection.removeAllRanges();
        selection.addRange(range);
      }
      return;
    }
    this.copied.set(true);
    view?.clearTimeout(this.copyTimer);
    this.copyTimer = view?.setTimeout(() => this.copied.set(false), 1600) ?? 0;
  }

  /** Is there more of this line to the right than is being shown? */
  protected onScroll(): void {
    const box = this.scroller().nativeElement;
    // A one-pixel allowance: fractional layout widths mean `scrollLeft +
    // clientWidth` lands just short of `scrollWidth` at the true end, and a
    // fade that never quite goes away reads as a rendering fault.
    this.clipped.set(box.scrollWidth - box.clientWidth - box.scrollLeft > 1);
  }

  constructor() {
    const isBrowser = isPlatformBrowser(inject(PLATFORM_ID));
    inject(DestroyRef).onDestroy(() => {
      const view = this.document.defaultView;
      view?.clearTimeout(this.timer);
      view?.clearTimeout(this.copyTimer);
      this.watcher?.disconnect();
    });

    // The block can start clipped, and can become clipped when the window
    // narrows or the OS tab swaps one command for a longer one — so it is
    // watched rather than measured once.
    afterNextRender(() => {
      const view = this.document.defaultView;
      if (!isBrowser || !view) {
        return;
      }
      this.onScroll();
      if (typeof view.ResizeObserver === 'function') {
        this.watcher = new view.ResizeObserver(() => this.onScroll());
        const box = this.scroller().nativeElement;
        this.watcher.observe(box);
        // And the body inside it: the box keeps its width while the content
        // changes under it — a webfont arriving, or the OS tab swapping one
        // command for a longer one — and only the second of those moves the
        // point at which the block is clipped.
        const body = box.firstElementChild;
        if (body) {
          this.watcher.observe(body);
        }
      }
    });

    afterNextRender(() => {
      const view = this.document.defaultView;
      if (!isBrowser || !view || !this.play()) {
        return;
      }
      if (view.matchMedia?.('(prefers-reduced-motion: reduce)').matches) {
        return;
      }
      if (typeof view.IntersectionObserver !== 'function') {
        return;
      }

      // Armed, not started. Hiding it now and starting only when it is looked
      // at is the whole point — a run that finished while the reader was four
      // sections above it is a run nobody saw.
      this.rewind();
      const observer = new view.IntersectionObserver(
        (entries) => {
          for (const entry of entries) {
            if (entry.isIntersecting) {
              observer.disconnect();
              this.start();
            }
          }
        },
        { rootMargin: '0px 0px -18% 0px', threshold: 0.2 },
      );
      observer.observe(this.host.nativeElement);
    });
  }

  /** A line that has not arrived yet is transparent, never absent. */
  protected opacityOf(index: number): number | null {
    const at = this.at();
    return at === null || index <= at.line ? null : 0;
  }

  /**
   * The width of a command line while it is being typed, in `ch`.
   *
   * `ch` is the advance of `0`, and in a monospace face every glyph has that
   * advance — so `width: 12ch` on an `overflow: hidden` span is exactly twelve
   * characters. The other thirty are still in the DOM behind it.
   */
  protected widthOf(index: number, line: TerminalLine): string | null {
    const at = this.at();
    if (at === null || index !== at.line) {
      return null;
    }
    return `${Math.min(at.characters, line.text.length)}ch`;
  }

  protected replay(): void {
    this.rewind();
    this.start();
  }

  private rewind(): void {
    this.document.defaultView?.clearTimeout(this.timer);
    this.finished.set(false);
    this.at.set({ line: 0, characters: 0 });
  }

  /**
   * One timeout per step rather than one interval for the whole run, so each
   * kind of line can take the time it actually takes: a command is typed, a
   * line of output lands whole, and a blank line is a beat.
   */
  private start(): void {
    const view = this.document.defaultView;
    if (!view) {
      return;
    }
    this.everPlayed.set(true);

    const step = () => {
      const at = this.at();
      if (at === null) {
        return;
      }
      const lines = this.lines();
      const line = lines[at.line];
      if (!line) {
        this.at.set(null);
        this.finished.set(true);
        return;
      }

      const typing = line.kind === 'prompt';
      if (typing && at.characters < line.text.length) {
        this.at.set({ line: at.line, characters: at.characters + 1 });
        this.timer = view.setTimeout(step, MS_PER_CHARACTER);
        return;
      }

      const next = at.line + 1;
      if (next >= lines.length) {
        // The caret stays on the last line and blinks for a moment, the way a
        // prompt does when a command has finished and nobody has typed yet.
        this.timer = view.setTimeout(() => {
          this.at.set(null);
          this.finished.set(true);
        }, BEFORE_REPLAY_IS_OFFERED);
        return;
      }

      this.at.set({ line: next, characters: 0 });
      this.timer = view.setTimeout(step, typing ? AFTER_A_COMMAND : BETWEEN_OUTPUT_LINES);
    };

    step();
  }
}
