import { ChangeDetectionStrategy, Component, computed, inject } from '@angular/core';
import { RouterLink } from '@angular/router';

import { DocsPage } from '../docs-page';
import { OS_LABEL, OsChoice } from '../os';
import { Terminal, type TerminalLine } from '../../ui/terminal';

/** One command, for the table of what needs an elevated terminal and what does not. */
interface Duty {
  readonly command: string;
  readonly does: string;
  readonly elevated: boolean;
}

/**
 * The background service.
 *
 * ## How these blocks were made
 *
 * A throwaway PostgreSQL 17.9 cluster on port 5447 with two databases, on
 * 2026-09-19, with the binary built from this commit. `attach`, `detach`,
 * `schedule`, `schedule --off`, `status` and `activity` were all run for real
 * and are captured verbatim, on Windows.
 *
 * ## What was deliberately not run
 *
 * **`service install` was never completed**, because it registers a real
 * service with the machine this session is running on, and that is the owner's
 * machine rather than a throwaway. What *is* captured is the run refusing:
 * unelevated, it stops before touching anything and says so, which is the block
 * this page most needs anyway.
 *
 * So the page shows an uninstalled machine throughout, and every line of every
 * block is consistent with that — `no sloop service is installed here` appears
 * under `attach` and `schedule` because that is what those runs printed. It is
 * an honest state rather than a staged one, and it is also the state a reader
 * is in while following the page.
 *
 * ## What is quoted rather than captured
 *
 * **The Linux and macOS spellings.** This session is on Windows, so the systemd
 * unit path, the launchd label and the `sudo` wording are read from
 * `cli/src/service/mechanism.rs`, `unit.rs` and `elevation.rs` — named here so
 * the quoting is visible. `SERVICE_NAME` is `sloop`, `LAUNCHD_LABEL` is
 * `io.github.dbsloop.sloop`, and `Mechanism::spoken` is what puts *systemd*,
 * *launchd* or *the Service Control Manager* into the refusal.
 *
 * The Windows elevation refusal below is a real run.
 *
 * ## One thing worth knowing before reading the page
 *
 * **The service can only watch the global registry**, and that is not an
 * omission — a service has no working directory, so there is no project for it
 * to be standing in. `service attach` against a project-only database is a real
 * captured error, and it is on the page because it is the first thing that goes
 * wrong for somebody who registered everything in a project.
 */
@Component({
  selector: 'app-docs-service',
  imports: [DocsPage, RouterLink, Terminal],
  templateUrl: './service.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Service {
  private readonly choice = inject(OsChoice);

  protected readonly os = this.choice.os;
  protected readonly osLabel = computed(() => OS_LABEL[this.os()]);

  /** What this platform's service manager is called, in sloop's own words. */
  protected readonly manager = computed(() => {
    switch (this.os()) {
      case 'windows':
        return 'the Service Control Manager';
      case 'macos':
        return 'launchd';
      default:
        return 'systemd';
    }
  });

  /** What gets registered, and where it lives. */
  protected readonly registers = computed(() => {
    switch (this.os()) {
      case 'windows':
        return 'a real service, registered with the Service Control Manager';
      case 'macos':
        return 'a launchd daemon at /Library/LaunchDaemons/io.github.dbsloop.sloop.plist';
      default:
        return 'a systemd unit at /etc/systemd/system/sloop.service';
    }
  });

  /** How this platform's reader gets an elevated terminal. */
  protected readonly elevate = computed(() =>
    this.os() === 'windows' ? "open a terminal with 'Run as administrator'" : 'run it with sudo',
  );

  protected readonly elevatedRun = computed(() =>
    this.os() === 'windows' ? 'sloop service install' : 'sudo sloop service install',
  );

  /**
   * The scheduler this platform has and sloop does not write to.
   *
   * Named per platform rather than all three at once, for `A7`'s reason: a
   * reader on Windows has no `systemd` timer to be reassured about, and a
   * sentence that mentions one is a sentence written for somebody else.
   */
  protected readonly scheduler = computed(() => {
    switch (this.os()) {
      case 'windows':
        return 'Task Scheduler';
      case 'macos':
        return 'crontab';
      default:
        return 'crontab or a systemd timer';
    }
  });

  // ── Elevation ─────────────────────────────────────────────────────────────

  /** A real unelevated run. It stops before doing anything. */
  protected readonly refusal = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop service install' },
    {
      kind: 'bad',
      tag: 'error:',
      text: ` changing what this machine runs at boot needs more than this account has, and`,
    },
    { kind: 'bad', tag: '', text: `  ${this.manager()} will refuse` },
    {
      kind: 'dim',
      text:
        this.os() === 'windows'
          ? "  hint: open a terminal with 'Run as administrator' and run it again. Nothing has been"
          : '  hint: run it again with sudo. Nothing has been changed by this run.',
    },
    ...(this.os() === 'windows' ? [{ kind: 'dim' as const, text: '  changed by this run.' }] : []),
  ]);

  protected readonly duties: readonly Duty[] = [
    { command: 'service install', does: 'registers it with the machine', elevated: true },
    { command: 'service uninstall', does: 'takes it off again', elevated: true },
    { command: 'service start', does: 'starts it now', elevated: true },
    { command: 'service stop', does: 'stops it now', elevated: true },
    { command: 'service attach', does: 'watch a database', elevated: false },
    { command: 'service detach', does: 'stop watching one', elevated: false },
    { command: 'service schedule', does: 'back one up on a schedule', elevated: false },
    { command: 'service status', does: 'installed, running, last run, next run', elevated: false },
    { command: 'service activity', does: 'what it has recorded', elevated: false },
  ];

  // ── Attaching ─────────────────────────────────────────────────────────────

  /** The first thing that goes wrong, and the reason it does. */
  protected readonly projectOnly: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop service attach orders   # registered in a project' },
    {
      kind: 'bad',
      tag: 'error:',
      text: ' there is no database called orders in the global registry',
    },
    {
      kind: 'dim',
      text: '  hint: the service has no working directory, so it can only watch the global store.',
    },
    {
      kind: 'dim',
      text: '  `sloop db list --global` shows what is in it, and `sloop db add --global` puts',
    },
    { kind: 'dim', text: '  something there.' },
  ];

  protected readonly attach: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop service attach orders' },
    { kind: 'name', tag: 'Attached.', text: ' the service watches orders from its next round' },
    {
      kind: 'dim',
      text: '  no sloop service is installed here — `sloop service install` registers one and starts it',
    },
  ];

  protected readonly detach: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop service detach reports' },
    {
      kind: 'name',
      tag: 'Detached.',
      text: ' the service stops watching reports at its next round',
    },
    { kind: 'dim', text: '  nothing had been recorded about it yet' },
  ];

  // ── Scheduling ────────────────────────────────────────────────────────────

  protected readonly schedule: readonly TerminalLine[] = [
    {
      kind: 'prompt',
      text: 'sloop service schedule orders --every 1d --keep 7 --keep-for-days 30',
    },
    { kind: 'name', tag: 'Scheduled.', text: ' orders is backed up every day' },
    { kind: 'dim', text: '  the newest 7 are kept, and anything older than 30 days is pruned' },
    { kind: 'ok', tag: '  no cron line, no scheduled task — the service takes it', text: '' },
    {
      kind: 'dim',
      text: '  no sloop service is installed here — `sloop service install` registers one and starts it',
    },
  ];

  protected readonly scheduleOff: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop service schedule orders --off' },
    {
      kind: 'name',
      tag: 'Stopped.',
      text: ' orders is still attached and still sampled — nothing backs it up now',
    },
    { text: ' ' },
    { kind: 'prompt', text: 'sloop service schedule orders --every 6h --keep 10' },
    { kind: 'name', tag: 'Scheduled.', text: ' orders is backed up every 6 hours' },
    { kind: 'dim', text: '  the newest 10 are kept, the rest are pruned' },
  ];

  // ── Status ────────────────────────────────────────────────────────────────

  protected readonly statusFresh = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop service status' },
    { kind: 'head', tag: 'Service', text: '' },
    { kind: 'label', tag: '  State ', text: 'not installed' },
    { kind: 'label', tag: '  Managed by ', text: this.manager() },
    { kind: 'dim', text: '  `sloop service install` registers it with this machine' },
    { text: ' ' },
    {
      kind: 'dim',
      text: 'Watching nothing — `sloop service attach <name>` attaches a database',
    },
  ]);

  protected readonly statusWatching = computed<readonly TerminalLine[]>(() => [
    { kind: 'prompt', text: 'sloop service status' },
    { kind: 'head', tag: 'Service', text: '' },
    { kind: 'label', tag: '  State ', text: 'not installed' },
    { kind: 'label', tag: '  Managed by ', text: this.manager() },
    { text: ' ' },
    { kind: 'head', tag: 'Watching 2 databases', text: '' },
    {
      kind: 'name',
      tag: '  orders',
      text: '',
      note: '  attached 2026-09-19 20:59:58 +06:00 · not picked up yet · no activity recorded',
    },
    {
      kind: 'dim',
      text: '      backed up every day · never run · next 2026-09-19 20:59:59 +06:00',
    },
    {
      kind: 'name',
      tag: '  reports',
      text: '',
      note: '  attached 2026-09-19 20:59:58 +06:00 · not picked up yet · no activity recorded',
    },
    {
      kind: 'dim',
      text: '      no backup schedule — `sloop service schedule <name> --every 1d`',
    },
    { kind: 'dim', text: '  no service has ever read this list' },
  ]);

  protected readonly activity: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop service activity' },
    { kind: 'head', tag: 'Activity', text: '', note: ' 2 databases attached' },
    { text: ' ' },
    { kind: 'name', tag: 'orders', text: '' },
    { kind: 'dim', text: '  nothing recorded yet — attached 2026-09-19 20:59:58 +06:00' },
    { kind: 'dim', text: '  no running service has picked it up yet' },
    { text: ' ' },
    { kind: 'name', tag: 'reports', text: '' },
    { kind: 'dim', text: '  nothing recorded yet — attached 2026-09-19 20:59:58 +06:00' },
    { kind: 'dim', text: '  no running service has picked it up yet' },
    { text: ' ' },
    {
      kind: 'dim',
      text: '  rows are what the server counted; Moved is real bytes sloop read or wrote. Nothing',
    },
    {
      kind: 'dim',
      text: '  here is bytes on the wire — no engine reports those per database.',
    },
  ];
}
