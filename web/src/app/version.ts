import reference from './docs/commands/commands.json';

/**
 * Which sloop this site documents, and the only place on it that says so.
 *
 * **Read from the generated reference rather than typed.** `web/tools/commands.mjs`
 * builds `commands.json` by asking the binary, so its `version` field is whatever
 * `sloop --version` printed — and CI regenerates it against a freshly built binary
 * and fails when the two have parted company. Every transcript on this site that
 * shows a version interpolates this, so a release is one command rather than a
 * hunt through seventeen pages.
 *
 * The field arrives the way `--version` prints it — `sloop`, a space, the number
 * — and this is the number on its own.
 */
export const RELEASE = reference.version.replace(/^\D+/, '');

/** The same with a `v` in front, for the places that print one. */
export const RELEASE_V = `v${RELEASE}`;

/**
 * The release the transcripts on this site were captured from.
 *
 * **A fact about the past, so it does not move with [`RELEASE`].** The install
 * and backup blocks are real runs taken at `0.1.0`, and the version strings
 * inside them are interpolated so that the pages describe the current release —
 * but the provenance notes in each component say which build was actually run,
 * and this is that build. It changes only when the blocks are recaptured.
 *
 * `tools/site.mjs` allows a version literal in this file and nowhere else under
 * `src/`, which is what stops the next release leaving a stale number behind.
 */
export const CAPTURED_AT = '0.1.0';
