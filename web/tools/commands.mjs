#!/usr/bin/env node
/**
 * Build the command reference out of `sloop --help`, rather than out of memory.
 *
 * `A8`'s bar is that **a diff against the real `--help` output finds nothing
 * missing**, and the only way to hold that for forty commands is to stop
 * transcribing them. This walks the command tree by asking the binary what it
 * has, parses what it prints, and writes `commands.json`. The page renders that
 * file and nothing else, so a flag cannot be on the site unless the CLI printed
 * it, and cannot be missing from the site if the CLI printed it.
 *
 * It is the same arrangement as `web/privileges.json`, which a Rust test writes
 * from `cli/src/engine/privileges.rs`: **generated, never hand-edited.** Editing
 * the JSON by hand is editing something the next run overwrites.
 *
 * ## Two modes
 *
 * ```sh
 * node tools/commands.mjs            # write commands.json
 * node tools/commands.mjs --check    # exit 1 if it is out of date
 * ```
 *
 * `--check` is the half that matters. Rule 14: a concern that is only stated is
 * a concern that gets forgotten at release, so this exists to be run by CI
 * rather than remembered. `A22` owns the workflow that calls it.
 *
 * ## Which binary
 *
 * `SLOOP` in the environment, else the release build beside this repository.
 * **Build it first.** `A7` was written from a `target/release` binary four
 * features out of date and had to be recaptured; a generator pointed at a stale
 * binary produces a stale reference with no sign that anything is wrong.
 */
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const OUT = join(here, '..', 'src', 'app', 'docs', 'commands', 'commands.json');

function binary() {
  if (process.env.SLOOP) {
    return process.env.SLOOP;
  }
  const windows = process.platform === 'win32';
  const guess = resolve(
    here,
    '..',
    '..',
    'cli',
    'target',
    'release',
    windows ? 'sloop.exe' : 'sloop',
  );
  if (!existsSync(guess)) {
    fail(
      `no sloop binary at ${guess}`,
      '  build one:  cargo build --release --manifest-path cli/Cargo.toml --bin sloop',
      '  or point at one:  SLOOP=/path/to/sloop node tools/commands.mjs',
    );
  }
  return guess;
}

function fail(...lines) {
  console.error('');
  console.error('  commands.mjs failed.');
  console.error('');
  for (const line of lines) console.error(`  ${line}`);
  console.error('');
  process.exit(1);
}

/** Run the binary and give back what it printed, with CRLF normalised. */
function help(sloop, path, long) {
  const out = execFileSync(sloop, [...path, long ? '--help' : '-h'], {
    encoding: 'utf8',
    env: { ...process.env, NO_COLOR: '1' },
    maxBuffer: 8 * 1024 * 1024,
  });
  return out.replace(/\r\n/g, '\n').replace(/\s+$/, '');
}

/**
 * Split help output into its sections.
 *
 * clap prints `Usage:`, `Arguments:`, `Options:` and `Commands:` flush left,
 * and everything before the first of them is the description.
 */
function sections(text) {
  const lines = text.split('\n');
  const found = { description: [] };
  let current = 'description';
  for (const line of lines) {
    const header = /^([A-Z][A-Za-z ]*):\s*$/.exec(line);
    if (header) {
      current = header[1].toLowerCase();
      found[current] = [];
      continue;
    }
    // `Usage:` carries its value on the same line as the header.
    const usage = /^Usage:\s+(.*)$/.exec(line);
    if (usage) {
      found.usage = [usage[1]];
      current = 'usage';
      continue;
    }
    (found[current] ??= []).push(line);
  }
  return found;
}

/**
 * Turn an indented, column-aligned clap section into `{ spec, text }` entries.
 *
 * clap wraps a long description onto continuation lines indented to the column
 * the description starts at. The column is found from the first entry rather
 * than assumed, because it is computed per command from the longest flag in it.
 */
function entries(lines = []) {
  const out = [];
  let column = null;
  for (const raw of lines) {
    if (!raw.trim()) continue;
    const indent = raw.length - raw.trimStart().length;
    if (column !== null && indent >= column && out.length) {
      out[out.length - 1].text += ` ${raw.trim()}`;
      continue;
    }
    const split = /^(\s{2,})(\S.*?)(\s{2,})(\S.*)$/.exec(raw);
    if (!split) {
      // A spec too long for its own line: clap puts the description underneath.
      out.push({ spec: raw.trim(), text: '' });
      continue;
    }
    column = split[1].length + split[2].length + split[3].length;
    out.push({ spec: split[2], text: split[4] });
  }
  return out.map((entry) => ({ ...entry, text: entry.text.trim() }));
}

/** `-C, --project <PATH|NAME>` → the pieces a table wants. */
function option(spec) {
  const value = /<([^>]+)>/.exec(spec);
  const flags = spec
    .replace(/<[^>]+>/, '')
    .split(',')
    .map((part) => part.trim())
    .filter(Boolean);
  const short = flags.find((f) => /^-[^-]/.test(f)) ?? null;
  const long = flags.find((f) => /^--/.test(f)) ?? null;
  return { short, long, value: value ? value[1] : null, spec };
}

/** Walk the tree, asking each command what it has under it. */
function walk(sloop, path = []) {
  const short = sections(help(sloop, path, false));
  const long = sections(help(sloop, path, true));

  const children = entries(short.commands).filter((entry) => entry.spec !== 'help');

  const node = {
    path,
    name: path.length ? path.join(' ') : 'sloop',
    summary: (short.description ?? []).join(' ').replace(/\s+/g, ' ').trim(),
    // The long description is the prose: clap keeps its paragraph breaks, so
    // blank lines are kept and the page renders one paragraph each.
    description: paragraphs(long.description ?? []),
    usage: (short.usage ?? []).join(' ').trim(),
    arguments: entries(short.arguments).map((entry) => ({ name: entry.spec, text: entry.text })),
    options: entries(short.options).map((entry) => ({ ...option(entry.spec), text: entry.text })),
    children: children.map((entry) => entry.spec),
  };

  const tree = [node];
  for (const child of children) {
    tree.push(...walk(sloop, [...path, child.spec]));
  }
  return tree;
}

function paragraphs(lines) {
  const text = lines.join('\n').trim();
  if (!text) return [];
  return text
    .split(/\n\s*\n/)
    .map((block) =>
      block
        .split('\n')
        .map((l) => l.trim())
        .join(' ')
        .trim(),
    )
    .filter(Boolean);
}

function build() {
  const sloop = binary();
  const version = execFileSync(sloop, ['--version'], { encoding: 'utf8' }).trim();
  const all = walk(sloop);

  // **The global flags are the ones every command has.** They are not a list
  // kept somewhere; they are an intersection, so a flag that stops being global
  // stops being in this section without anybody editing anything.
  const leaves = all.filter((node) => node.children.length === 0);
  const global = leaves.length
    ? leaves[0].options.filter(
        (candidate) =>
          candidate.long &&
          candidate.long !== '--help' &&
          leaves.every((leaf) => leaf.options.some((o) => o.long === candidate.long)),
      )
    : [];
  const globalLongs = new Set(global.map((o) => o.long));

  return {
    $generated: 'web/tools/commands.mjs — do not edit by hand',
    version,
    // Every command keeps its full option list; the page filters the global
    // ones out for display, so nothing is lost from the data itself.
    globalOptions: global,
    commands: all.map((node) => ({
      ...node,
      // `--help` is on all forty of them and is a row nobody reads.
      ownOptions: node.options.filter(
        (o) => o.long !== '--help' && (!o.long || !globalLongs.has(o.long)),
      ),
    })),
  };
}

const data = build();
const json = `${JSON.stringify(data, null, 2)}\n`;

if (process.argv.includes('--check')) {
  const current = existsSync(OUT) ? readFileSync(OUT, 'utf8') : '';
  if (current === json) {
    console.log(
      `commands.json is up to date with ${data.version} (${data.commands.length} commands)`,
    );
    process.exit(0);
  }
  fail(
    'commands.json does not match what the binary prints.',
    '  regenerate it:  node tools/commands.mjs',
    `  binary: ${data.version}`,
  );
}

writeFileSync(OUT, json);
console.log(
  `wrote ${OUT}\n  ${data.version}, ${data.commands.length} commands, ${data.globalOptions.length} global flags`,
);
