import { ChangeDetectionStrategy, Component, input } from '@angular/core';

/** One line of a terminal block. */
export interface TerminalLine {
  /** `prompt` prefixes an accent caret; the status kinds tint `tag`. */
  readonly kind?: 'prompt' | 'out' | 'dim' | 'ok' | 'warn' | 'bad';
  /** For the status kinds, the word that carries the colour. */
  readonly tag?: string;
  /** A single space renders as a blank line; an empty string would collapse. */
  readonly text: string;
}

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
 */
@Component({
  selector: 'app-terminal',
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <figure class="m-0 overflow-hidden rounded-xl border border-term-line bg-term-bg shadow-2">
      <figcaption
        class="flex items-center gap-2 border-b border-term-line px-4 py-2.5 font-mono text-xs text-term-dim"
      >
        <span class="h-2.5 w-2.5 rounded-full bg-term-dim/40" aria-hidden="true"></span>
        <span class="h-2.5 w-2.5 rounded-full bg-term-dim/40" aria-hidden="true"></span>
        <span class="h-2.5 w-2.5 rounded-full bg-term-dim/40" aria-hidden="true"></span>
        <span class="ml-1.5">{{ caption() }}</span>
      </figcaption>

      <div class="overflow-x-auto px-4 py-4 sm:px-5">
        <code class="block font-mono text-xs leading-relaxed sm:text-sm">
          @for (line of lines(); track $index) {
            <span class="block whitespace-pre">
              @switch (line.kind) {
                @case ('prompt') {
                  <span class="select-none text-brand">$ </span>
                  <span class="text-term-text">{{ line.text }}</span>
                }
                @case ('dim') {
                  <span class="text-term-dim">{{ line.text }}</span>
                }
                @case ('ok') {
                  <span class="text-term-ok">{{ line.tag }}</span>
                  <span class="text-term-dim">{{ line.text }}</span>
                }
                @case ('warn') {
                  <span class="text-term-warn">{{ line.tag }}</span>
                  <span class="text-term-dim">{{ line.text }}</span>
                }
                @case ('bad') {
                  <span class="text-term-bad">{{ line.tag }}</span>
                  <span class="text-term-dim">{{ line.text }}</span>
                }
                @default {
                  <span class="text-term-text">{{ line.text }}</span>
                }
              }
            </span>
          }
        </code>
      </div>
    </figure>
  `,
})
export class Terminal {
  readonly caption = input('sloop');
  readonly lines = input.required<readonly TerminalLine[]>();
}
