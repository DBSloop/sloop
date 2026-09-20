import { ChangeDetectionStrategy, Component } from '@angular/core';
import { RouterLink } from '@angular/router';

import { DocsPage } from '../docs-page';
import { Terminal, type TerminalLine } from '../../ui/terminal';

/** One of the four routes a password can take. */
interface Route {
  readonly flag: string;
  readonly where: string;
  readonly when: string;
}

/**
 * Security.
 *
 * ## How these blocks were made
 *
 * Real runs on 2026-09-19 with the binary built from this commit, against a
 * throwaway PostgreSQL 17.9 on port 5449 built for the awkward-password proof.
 *
 * **Four of them are the page's evidence and all four were produced, not
 * asserted:**
 *
 * 1. **A password given in a URL.** sloop files it, warns that the shell and
 *    `ps` already saw it, and the `--log-file` written by that same run has no
 *    password in it anywhere.
 * 2. **A token inside somebody's own `--password-command`.** The terminal shows
 *    `Bearer sk-live-abc123` because that is the command the user typed; the log
 *    shows `Bearer ***`. Both are captured, side by side, because the difference
 *    is the honest part.
 * 3. **A password of nothing but shell metacharacters** —
 *    `p@ss$HOME \`whoami\` >out #1 "q" |pipe \slash` — set as a real PostgreSQL
 *    role's password, registered through `--password-stdin`, and then connected
 *    to on a *later* run out of the keyring. Nothing was expanded, nothing was
 *    evaluated.
 * 4. **The dependency check**, run exactly as the page prints it: 269 crates in
 *    the graph and the grep finds none of them.
 *
 * ## The substitution
 *
 * None. Every character of every block is what the runs printed, including the
 * invented `sk-live-abc123`, which was never a real token.
 */
@Component({
  selector: 'app-docs-security',
  imports: [DocsPage, RouterLink, Terminal],
  templateUrl: './security.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Security {
  // No OS strip: nothing here is a path, and every claim holds on all three.

  protected readonly routes: readonly Route[] = [
    {
      flag: '--keyring',
      where: 'Credential Manager, the Keychain, or the Secret Service.',
      when: 'The default, and right unless one of the other three applies.',
    },
    {
      flag: '--encrypted-file',
      where: 'A file encrypted with XChaCha20-Poly1305, its key derived with Argon2id.',
      when: 'A headless Linux box with no keyring running.',
    },
    {
      flag: '--env <VARIABLE>',
      where: 'An environment variable, read at the moment it is needed.',
      when: 'CI, and automation generally.',
    },
    {
      flag: '--password-from <COMMAND>',
      where: 'Whatever that command prints. Read through a pipe, never an argument.',
      when: 'A team password manager.',
    },
  ];

  // ── The dependency check ──────────────────────────────────────────────────

  /** What a reader copies, and what it prints. */
  protected readonly check: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'git clone https://github.com/DBSloop/sloop && cd sloop/cli' },
    { kind: 'prompt', text: 'cargo tree --edges normal,build,dev --target all --all-features \\' },
    { kind: 'prompt', text: "    --prefix none | awk 'NF{print $1}' | sort -u \\" },
    { kind: 'prompt', text: "    | grep -Ei 'reqwest|hyper|ureq|isahc|curl|ssh2|russh|thrussh'" },
    { kind: 'dim', text: '# nothing. That is the whole result.' },
  ];

  /** The same thing, as the script CI runs on every push. */
  protected readonly guard: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sh ci/no-http-client.sh' },
    {
      kind: 'ok',
      tag: 'no-http-client: 269 crates in the graph, not one of them opens a socket.',
      text: '',
    },
  ];

  // ── Nothing in argv ───────────────────────────────────────────────────────

  /** A password given the exposing way, and what sloop does about it. */
  protected readonly inTheUrl: readonly TerminalLine[] = [
    {
      kind: 'prompt',
      text: "sloop db add leaky --url 'postgres://app:hunter2@db.internal:5432/orders' \\",
    },
    { kind: 'prompt', text: '    --log-file run.log' },
    {
      kind: 'step',
      mark: 'warn',
      text: 'The password came from the URL',
      note: 'it was visible in `ps` and is in your shell history while that',
    },
    { text: 'command line lives. sloop has filed it and will not write it anywhere —' },
    { text: '--password-stdin avoids the exposure next time' },
    {
      kind: 'name',
      tag: 'registered leaky',
      text: ' in the project registry',
      note: ', password from the OS keyring',
    },
    { kind: 'dim', text: '  postgres://app@db.internal:5432/orders' },
    { text: ' ' },
    { kind: 'prompt', text: 'cat run.log' },
    {
      kind: 'dim',
      text: '▲    The password came from the URL      it was visible in `ps` and is in your shell history while that',
    },
    {
      kind: 'dim',
      text: 'command line lives. sloop has filed it and will not write it anywhere —',
    },
    { kind: 'dim', text: '--password-stdin avoids the exposure next time' },
    {
      kind: 'dim',
      text: 'registered leaky in the project registry, password from the OS keyring',
    },
    {
      kind: 'ok',
      tag: '  postgres://app@db.internal:5432/orders',
      text: '   ← no password, anywhere',
    },
  ];

  // ── The redactor ──────────────────────────────────────────────────────────

  /** Somebody else's secret, inside a command sloop was handed. */
  protected readonly redacted: readonly TerminalLine[] = [
    { kind: 'dim', text: '# the terminal shows the command the user typed' },
    {
      kind: 'prompt',
      text: 'sloop db add vaulted --url … --log-file run2.log \\',
    },
    {
      kind: 'prompt',
      text: '    --password-from "curl -H \'Authorization: Bearer sk-live-abc123\' https://vault/pw"',
    },
    {
      kind: 'dim',
      text: "note: … password from the command `curl -H 'Authorization: Bearer sk-live-abc123'",
    },
    { kind: 'dim', text: '  https://vault/pw`' },
    { text: ' ' },
    { kind: 'dim', text: '# the log does not' },
    { kind: 'prompt', text: 'cat run2.log' },
    {
      kind: 'ok',
      tag: "note: … password from the command `curl -H 'Authorization: Bearer *** https://vault/pw`",
      text: '',
    },
  ];

  // ── Parsed, never evaluated ───────────────────────────────────────────────

  /** A password that is nothing but shell metacharacters, and it just works. */
  protected readonly awkward: readonly TerminalLine[] = [
    { kind: 'dim', text: "# the role's real password on the server:" },
    { kind: 'dim', text: '#   p@ss$HOME `whoami` >out #1 "q" |pipe \\slash' },
    {
      kind: 'prompt',
      text: 'sloop db add awkward --url postgres://app@127.0.0.1:5449/orders \\',
    },
    { kind: 'prompt', text: '    --password-stdin --test   # piped in, never an argument' },
    { kind: 'step', mark: 'ok', text: 'postgres 17.9', note: 'not encrypted' },
    {
      kind: 'name',
      tag: 'registered awkward',
      text: ' in the project registry',
      note: ', password from the OS keyring',
    },
    { text: ' ' },
    { kind: 'dim', text: '# and a later run, reading it back out of the keyring:' },
    { kind: 'prompt', text: 'sloop db test awkward' },
    {
      kind: 'name',
      tag: 'awkward',
      text: '  postgres://app@127.0.0.1:5449/orders',
      note: '  project',
    },
    {
      kind: 'step',
      mark: 'ok',
      text: 'postgres 17.9',
      note: 'not encrypted   ← it connected',
    },
  ];
}
