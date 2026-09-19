import { ChangeDetectionStrategy, Component } from '@angular/core';
import { RouterLink } from '@angular/router';

import { DocsPage } from '../docs-page';
import { Terminal, type TerminalLine } from '../../ui/terminal';

/** One reason sloop runs the machine's `ssh` instead of linking a client. */
interface Reason {
  readonly title: string;
  readonly detail: string;
}

/**
 * Reaching a database over SSH.
 *
 * ## How these blocks were made
 *
 * Real runs on 2026-09-19 with the binary built from this commit: two
 * registrations through an SSH server, `db list` showing the forward,
 * `db edit --no-ssh` taking it off, and a `db test` that fails. The CI guard
 * block is `sh ci/no-http-client.sh` in this repository.
 *
 * **There is no live tunnel on this page and the reason is honest**: this
 * machine has no `sshd` to tunnel to, and checking further needed elevation. So
 * `bastion.example.com` does not resolve — which turns out to be the better
 * capture, because the error is *OpenSSH's own words relayed verbatim*
 * (`ssh: Could not resolve hostname …`), and that is the page's whole argument
 * in one line: sloop is running your `ssh`, not imitating it.
 *
 * ## What is quoted rather than captured
 *
 * The four reasons for shelling out, from the module note at the top of
 * `cli/src/ssh/mod.rs`, and the one-forward-per-session rule from the same
 * place. The denied crate families are read from `ci/no-http-client.sh`.
 */
@Component({
  selector: 'app-docs-ssh',
  imports: [DocsPage, RouterLink, Terminal],
  templateUrl: './ssh.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Ssh {
  // No OS strip: every path on this page belongs to the SSH server or to
  // `~/.ssh`, which is spelled the same way by OpenSSH everywhere.

  protected readonly registering: readonly TerminalLine[] = [
    {
      kind: 'prompt',
      text: 'sloop db add remote --url postgres://app@127.0.0.1:5432/orders \\',
    },
    { kind: 'prompt', text: '    --ssh-host bastion.example.com --ssh-user deploy' },
    {
      kind: 'name',
      tag: 'registered remote',
      text: ' in the project registry',
      note: ' (the nearest .sloop at or above the working directory), password from the OS keyring',
    },
    {
      kind: 'dim',
      text: '  postgres://app@127.0.0.1:5432/orders through ssh://deploy@bastion.example.com:22',
    },
    { kind: 'dim', text: '  over deploy@bastion.example.com, from the agent' },
    {
      kind: 'warn',
      tag: '  127.0.0.1:5432 is as bastion.example.com sees it',
      text: ', not as this machine does',
    },
  ];

  protected readonly listed: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop db list' },
    {
      kind: 'name',
      tag: 'remote',
      text: '  postgres://app@127.0.0.1:5432/orders',
      note: '  project · the OS keyring',
    },
    { kind: 'dim', text: '        over deploy@bastion.example.com, from the agent' },
  ];

  /** OpenSSH's own words, relayed. */
  protected readonly failing: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop db test remote' },
    { text: ' ' },
    {
      kind: 'name',
      tag: 'remote',
      text: '  postgres://app@127.0.0.1:5432/orders through ssh://deploy@bastion.example.com:22',
      note: '  project',
    },
    { kind: 'bad', tag: 'error:', text: ' ssh could not reach deploy@bastion.example.com' },
    {
      kind: 'dim',
      text: '  hint: ssh: Could not resolve hostname bastion.example.com: Name or service not known',
    },
  ];

  protected readonly withIdentity: readonly TerminalLine[] = [
    {
      kind: 'prompt',
      text: 'sloop db add keyed --url postgres://app@127.0.0.1:5432/orders \\',
    },
    {
      kind: 'prompt',
      text: '    --ssh-host bastion.example.com --ssh-user deploy \\',
    },
    {
      kind: 'prompt',
      text: '    --ssh-identity ~/.ssh/id_ed25519 \\',
    },
    {
      kind: 'prompt',
      text: '    --ssh-passphrase-from "op read op://vault/ssh/passphrase"',
    },
    {
      kind: 'dim',
      text: 'note: the password command exited with 1: op read op://vault/ssh/passphrase —',
    },
    {
      kind: 'dim',
      text: '  registered anyway, because the command is read on every run rather than kept here',
    },
    {
      kind: 'name',
      tag: 'registered keyed',
      text: ' in the project registry',
      note: ', password from the OS keyring',
    },
    {
      kind: 'dim',
      text: '  postgres://app@127.0.0.1:5432/orders through ssh://deploy@bastion.example.com:22#C:/Users/you/.ssh/id_ed25519',
    },
    {
      kind: 'dim',
      text: '  over deploy@bastion.example.com, unlocked from the command `op read op://vault/ssh/passphrase`',
    },
  ];

  protected readonly removing: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop db edit remote --no-ssh' },
    { kind: 'name', tag: 'changed remote', text: '', note: '  in the project registry' },
    {
      kind: 'dim',
      text: '  postgres://app@127.0.0.1:5432/orders through ssh://deploy@bastion.example.com:22',
    },
    { text: '  postgres://app@127.0.0.1:5432/orders' },
  ];

  /** The guarantee, checked rather than asserted. */
  protected readonly guard: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sh ci/no-http-client.sh' },
    {
      kind: 'ok',
      tag: 'no-http-client: 269 crates in the graph, not one of them opens a socket.',
      text: '',
    },
  ];

  /** Why the machine's own `ssh`, from the module note that decided it. */
  protected readonly reasons: readonly Reason[] = [
    {
      title: 'The guarantee survives literally',
      detail:
        'An embedded SSH client is a socket in the binary. The promise above the fold would stop being true while a check that only looked for HTTP clients still passed — so the check is not narrow, and ssh2, libssh2-sys, russh, thrussh, async-ssh2-*, ssh-rs and makiko all fail the build alongside reqwest and hyper.',
    },
    {
      title: 'Your own SSH already works',
      detail:
        'ProxyJump through a bastion, IdentityFile, the agent, a hardware key, Match blocks, a corporate CA. Anybody whose ssh user@server works gets a working sloop with nothing else configured, because it is the same ssh.',
    },
    {
      title: 'known_hosts stays OpenSSH’s',
      detail:
        'sloop does not weaken host checking by a single option. The first connection to an unknown server is OpenSSH asking you, exactly as it would if you had typed the command yourself.',
    },
    {
      title: 'Not one adapter changes',
      detail:
        'sloop binds a local port that comes out at the database on the far side. psql, pg_dump, mysqldump and the rest connect to 127.0.0.1 and know nothing about SSH at all — the only thing that changed is which address they were handed.',
    },
  ];
}
