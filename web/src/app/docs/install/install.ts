import { ChangeDetectionStrategy, Component, computed, inject } from '@angular/core';
import { RouterLink } from '@angular/router';

import { DocsPage } from '../docs-page';
import { OS_LABEL, OsChoice } from '../os';
import { Terminal, type TerminalLine } from '../../ui/terminal';

/**
 * Installation.
 *
 * **One platform at a time, and that is the entry's bar rather than a
 * preference:** *a reader on any one OS never sees an instruction for another*.
 * So every command block on this page is behind the OS strip in the toolbar,
 * and the three platforms are named once, in a sentence, so somebody comparing
 * still knows what there is.
 *
 * ## Where the transcripts come from
 *
 * Every one of them was produced by running the thing, and none was written
 * from an idea of what it prints.
 *
 * ```text
 * Windows install     ci/install-roundtrip.ps1, run on this machine against a
 *                     release laid out locally — 26 of 26 checks passed
 * Linux install       the same round trip on ubuntu-latest, from CI run
 *                     35432962875, job "install (ubuntu-latest)" — 33 of 33
 * macOS install       the same, job "install (macos-latest)" — 33 of 33
 * by hand, Unix       run here in a shell against the real v0.1.0 release
 * by hand, Windows    run here in PowerShell against the real v0.1.0 release
 * removal             the uninstall half of the same round trips
 * ```
 *
 * **The one substitution, and it is named rather than hidden.** The round trips
 * install into a temporary directory so they cannot touch the machine they run
 * on, so the paths in their output are temporary paths. Those are replaced here
 * by the defaults a person actually gets — `~/.local/bin`,
 * `%LOCALAPPDATA%\Programs\sloop\bin`, and the startup file `pick_rc` in
 * `install/install.sh` chooses for each login shell. They are the only variable
 * in those lines; every other character is what the installer printed.
 *
 * macOS shows `.zshrc` where CI shows `.bash_profile`, because the CI runner
 * logs in with bash and macOS has shipped zsh as the default since Catalina.
 * Both come out of the same `pick_rc`.
 */
@Component({
  selector: 'app-docs-install',
  imports: [DocsPage, RouterLink, Terminal],
  templateUrl: './install.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Install {
  private readonly choice = inject(OsChoice);

  protected readonly os = this.choice.os;
  protected readonly osLabel = computed(() => OS_LABEL[this.os()]);

  /** The target triple this platform's release archive is named for. */
  protected readonly target = computed(() => {
    switch (this.os()) {
      case 'windows':
        return 'x86_64-pc-windows-msvc';
      case 'macos':
        return 'aarch64-apple-darwin';
      default:
        return 'x86_64-unknown-linux-musl';
    }
  });

  protected readonly archive = computed(() =>
    this.os() === 'windows' ? `sloop-${this.target()}.zip` : `sloop-${this.target()}.tar.gz`,
  );

  /** Where the installer puts the binary when it is not told otherwise. */
  protected readonly installDir = computed(() => {
    switch (this.os()) {
      case 'windows':
        return '%LOCALAPPDATA%\\Programs\\sloop\\bin';
      default:
        return '~/.local/bin';
    }
  });

  // ── The one-liner, and what it prints ─────────────────────────────────────

  protected readonly installing = computed<readonly TerminalLine[]>(() => {
    switch (this.os()) {
      case 'windows':
        return WINDOWS_INSTALL;
      case 'macos':
        return MACOS_INSTALL;
      default:
        return LINUX_INSTALL;
    }
  });

  protected readonly installCaption = computed(() => {
    switch (this.os()) {
      case 'windows':
        return 'PowerShell, on Windows';
      case 'macos':
        return 'a terminal, on macOS';
      default:
        return 'a shell, on Linux';
    }
  });

  // ── Passing an option through the pipe ────────────────────────────────────

  protected readonly withOptions = computed<readonly TerminalLine[]>(() =>
    this.os() === 'windows'
      ? [
          {
            kind: 'prompt',
            text: '& ([scriptblock]::Create((irm https://dbsloop.github.io/install.ps1))) -Version 1.2.3 -Dir D:\\tools\\sloop',
          },
        ]
      : [
          {
            kind: 'prompt',
            text: 'curl -fsSL https://dbsloop.github.io/install.sh | sh -s -- --version 1.2.3 --dir ~/bin',
          },
        ],
  );

  // ── Taking the archive by hand ────────────────────────────────────────────

  protected readonly byHand = computed<readonly TerminalLine[]>(() => {
    switch (this.os()) {
      case 'windows':
        return WINDOWS_BY_HAND;
      case 'macos':
        return MACOS_BY_HAND;
      default:
        return LINUX_BY_HAND;
    }
  });

  // ── Taking it off again ───────────────────────────────────────────────────

  /**
   * The command to run, in a block of its own and above the fallback script.
   *
   * The owner, on seeing the section with only the script in it: *everyone will
   * think that they have to use the long command instead of just
   * `sloop uninstall`, so this must be inside another shell, so that they
   * understand, because the most highlighted part here is the shell box.*
   *
   * That is right, and it is a general point about this site rather than a note
   * on one section. A terminal block is the heaviest thing on a page; a reader
   * finds one, reads what is in it, and acts. A paragraph saying *run this
   * other thing first* above a block holding something else is a paragraph that
   * loses. The primary command gets the primary box.
   *
   * No output beside it, because none was captured: running `sloop uninstall`
   * for a screenshot means destroying a real machine's sloop, and an invented
   * transcript is the one thing this site does not do.
   */
  protected readonly uninstallCommand: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop uninstall' },
  ];

  protected readonly removing = computed<readonly TerminalLine[]>(() =>
    this.os() === 'windows' ? WINDOWS_REMOVE : UNIX_REMOVE,
  );

  /**
   * `cargo install`, which is the same command everywhere and differs only in
   * what the binary ends up called.
   *
   * Run here against the real crates.io release: 1m 49s, and the `sloop` it
   * produced reports `sloop 0.1.0`. The four hundred-odd `Compiling` lines in
   * the middle are cut, and the cut is marked.
   */
  protected readonly withCargo = computed<readonly TerminalLine[]>(() => {
    const windows = this.os() === 'windows';
    const binary = windows ? 'sloop.exe' : 'sloop';
    const where = windows ? 'C:\\Users\\you\\.cargo\\bin\\' : '/home/you/.cargo/bin/';
    return [
      { kind: 'prompt', text: 'cargo install dbsloop' },
      { text: '    Updating crates.io index' },
      { text: '  Downloaded dbsloop v0.1.0' },
      { text: '  Installing dbsloop v0.1.0' },
      { text: '   Compiling dbsloop v0.1.0' },
      { kind: 'dim', text: '   …' },
      { text: '    Finished `release` profile [optimized] target(s) in 1m 49s' },
      { text: `  Installing ${where}${binary}` },
      { text: `   Installed package \`dbsloop v0.1.0\` (executable \`${binary}\`)` },
    ];
  });
}

// The blocks themselves. Module constants rather than fields, because they never
// depend on anything: each is one platform's real output, kept verbatim.

const WINDOWS_INSTALL: readonly TerminalLine[] = [
  { kind: 'prompt', text: 'irm https://dbsloop.github.io/install.ps1 | iex' },
  { text: 'sloop latest for x86_64-pc-windows-msvc' },
  { text: '  checksum verified' },
  { text: '  installed C:\\Users\\you\\AppData\\Local\\Programs\\sloop\\bin\\sloop.exe' },
  { text: '  PATH entry added: %LOCALAPPDATA%\\Programs\\sloop\\bin' },
  { text: ' ' },
  { text: 'sloop 0.1.0 is installed.' },
  { text: ' ' },
  { text: 'Restart your shell, or open a new terminal, before running sloop.' },
  { text: 'This one read its PATH when it started and will not see the new entry.' },
  { text: ' ' },
  { text: 'To use it in this window without restarting:' },
  {
    kind: 'dim',
    text: '    $env:Path = "C:\\Users\\you\\AppData\\Local\\Programs\\sloop\\bin;$env:Path"',
  },
  { text: ' ' },
  { text: 'Then: sloop setup' },
];

const LINUX_INSTALL: readonly TerminalLine[] = [
  { kind: 'prompt', text: 'curl -fsSL https://dbsloop.github.io/install.sh | sh' },
  { text: 'sloop latest for x86_64-unknown-linux-musl' },
  { text: '  checksum verified' },
  { text: '  installed /home/you/.local/bin/sloop' },
  { text: '  PATH entry added to /home/you/.bashrc' },
  { text: ' ' },
  { text: 'sloop 0.1.0 is installed.' },
  { text: ' ' },
  { text: 'Restart your shell, or start a new terminal, before running sloop.' },
  { text: 'A shell reads /home/you/.bashrc once, when it starts -- this one has already read it.' },
  { text: ' ' },
  { text: 'To use it in this shell without restarting:' },
  { kind: 'dim', text: '    export PATH="/home/you/.local/bin:$PATH"' },
  { text: ' ' },
  { text: 'Then: sloop setup' },
];

const MACOS_INSTALL: readonly TerminalLine[] = [
  { kind: 'prompt', text: 'curl -fsSL https://dbsloop.github.io/install.sh | sh' },
  { text: 'sloop latest for aarch64-apple-darwin' },
  { text: '  checksum verified' },
  { text: '  installed /Users/you/.local/bin/sloop' },
  { text: '  PATH entry added to /Users/you/.zshrc' },
  { text: ' ' },
  { text: 'sloop 0.1.0 is installed.' },
  { text: ' ' },
  { text: 'Restart your shell, or start a new terminal, before running sloop.' },
  { text: 'A shell reads /Users/you/.zshrc once, when it starts -- this one has already read it.' },
  { text: ' ' },
  { text: 'To use it in this shell without restarting:' },
  { kind: 'dim', text: '    export PATH="/Users/you/.local/bin:$PATH"' },
  { text: ' ' },
  { text: 'Then: sloop setup' },
];

const LINUX_BY_HAND: readonly TerminalLine[] = [
  { kind: 'prompt', text: 'base=https://github.com/DBSloop/sloop/releases/latest/download' },
  { kind: 'prompt', text: 'curl -fsSLO "$base/sloop-x86_64-unknown-linux-musl.tar.gz"' },
  { kind: 'prompt', text: 'curl -fsSLO "$base/SHA256SUMS"' },
  {
    kind: 'prompt',
    text: 'grep sloop-x86_64-unknown-linux-musl.tar.gz SHA256SUMS | sha256sum -c -',
  },
  { text: 'sloop-x86_64-unknown-linux-musl.tar.gz: OK' },
  { kind: 'prompt', text: 'tar -xzf sloop-x86_64-unknown-linux-musl.tar.gz' },
  { kind: 'prompt', text: 'install -m 755 sloop ~/.local/bin/sloop' },
  { kind: 'prompt', text: 'sloop --version' },
  { text: 'sloop 0.1.0' },
];

const MACOS_BY_HAND: readonly TerminalLine[] = [
  { kind: 'prompt', text: 'base=https://github.com/DBSloop/sloop/releases/latest/download' },
  { kind: 'prompt', text: 'curl -fsSLO "$base/sloop-aarch64-apple-darwin.tar.gz"' },
  { kind: 'prompt', text: 'curl -fsSLO "$base/SHA256SUMS"' },
  {
    kind: 'prompt',
    text: 'grep sloop-aarch64-apple-darwin.tar.gz SHA256SUMS | shasum -a 256 -c -',
  },
  { text: 'sloop-aarch64-apple-darwin.tar.gz: OK' },
  { kind: 'prompt', text: 'tar -xzf sloop-aarch64-apple-darwin.tar.gz' },
  { kind: 'prompt', text: 'install -m 755 sloop ~/.local/bin/sloop' },
  { kind: 'prompt', text: 'sloop --version' },
  { text: 'sloop 0.1.0' },
];

const WINDOWS_BY_HAND: readonly TerminalLine[] = [
  {
    kind: 'prompt',
    text: "$base  = 'https://github.com/DBSloop/sloop/releases/latest/download'",
  },
  { kind: 'prompt', text: "$asset = 'sloop-x86_64-pc-windows-msvc.zip'" },
  { kind: 'prompt', text: 'irm "$base/$asset"     -OutFile $asset' },
  { kind: 'prompt', text: 'irm "$base/SHA256SUMS" -OutFile SHA256SUMS' },
  {
    kind: 'prompt',
    text: "$want = ((Select-String SHA256SUMS -Pattern $asset).Line -split '\\s+')[0]",
  },
  { kind: 'prompt', text: '$got  = (Get-FileHash $asset -Algorithm SHA256).Hash' },
  { kind: 'prompt', text: 'if ($got -ieq $want) { Expand-Archive $asset -DestinationPath . }' },
  { kind: 'prompt', text: '.\\sloop.exe --version' },
  { text: 'sloop 0.1.0' },
];

const WINDOWS_REMOVE: readonly TerminalLine[] = [
  { kind: 'prompt', text: 'irm https://dbsloop.github.io/uninstall.ps1 | iex' },
  { text: '  removed C:\\Users\\you\\AppData\\Local\\Programs\\sloop\\bin\\sloop.exe' },
  { text: '  removed PATH entry: %LOCALAPPDATA%\\Programs\\sloop\\bin' },
  { text: ' ' },
  { text: 'sloop is off this machine.' },
  { text: 'Open shells still have the old PATH; the next one will not.' },
];

const UNIX_REMOVE: readonly TerminalLine[] = [
  { kind: 'prompt', text: 'curl -fsSL https://dbsloop.github.io/uninstall.sh | sh' },
  { text: '  removed /home/you/.local/bin/sloop' },
  { text: "  removed sloop's PATH block from /home/you/.bashrc" },
  { text: ' ' },
  { text: 'sloop is off this machine.' },
  { text: 'Open shells still have the old PATH; the next one will not.' },
];
