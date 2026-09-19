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
 *
 * **A section ends at the next flush-left line, not at the end of the output.**
 * clap's `after_help` is printed at column zero with no header of its own, and
 * without this it was swallowed by whichever section came last — bare `sloop`
 * grew an option called *Run `sloop` with no command for the interactive menu.*
 * Every real entry inside a section is indented by at least two, so column zero
 * is the boundary.
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
    if (current !== 'description' && line.trim() && !/^\s/.test(line)) {
      current = 'after';
      found.after ??= [];
    }
    (found[current] ??= []).push(line);
  }
  return found;
}

/** A line holding a flag, an argument or a subcommand name and nothing else. */
const SPEC_ONLY =
  /^(?:-{1,2}[^\s,]+(?:,\s*-{1,2}[^\s,]+)*(?:[ =](?:<[^>]+>|\[[^\]]+\]))?|<[^>]+>|\[[^\]]+\])$/;

/** Two columns on one line: the spec, a run of spaces, then the description. */
const TWO_COLUMN = /^(\s{2,})(\S.*?)(\s{2,})(\S.*)$/;

/**
 * Turn an indented clap section into `{ spec, text }` entries.
 *
 * **clap prints two layouts and the difference is not cosmetic.** When every
 * flag in a command is short enough, the description sits on the same line in a
 * second column. When one is not, clap drops the whole block to a vertical
 * layout: the spec alone on its line, the description indented underneath it.
 *
 * `sloop db create`, `sloop setup` and bare `sloop` print the second one, and
 * reading them with the first one's rule turned every flag into two useless
 * rows — a spec with no description, then prose with no flag. That is what
 * `A8` shipped: 32 of 32 rows wrong on `db create`, 21 of 21 on `setup`. The
 * layout is now detected per section rather than assumed, because a command can
 * print one layout for `Arguments:` and the other for `Options:` — `db create`
 * does exactly that.
 *
 * In the two-column layout a wrapped description continues on a line indented
 * to the description column, which is computed per command from the longest
 * flag in it and so is read from the first entry rather than assumed.
 *
 * In the vertical layout the spec's own indentation is **not** a usable signal:
 * clap indents `-C, --project` by two and `--engine` by six so the long forms
 * line up. So a line opens a new entry when the whole of it is a spec and
 * nothing else, which is true of every spec line in that layout and false of
 * every description — including one that happens to begin with a dash.
 */
function entries(lines = []) {
  const kept = lines.filter((line) => line.trim());
  const out = [];

  if (!kept.some((line) => TWO_COLUMN.test(line))) {
    for (const raw of kept) {
      const text = raw.trim();
      if (!out.length || SPEC_ONLY.test(text)) {
        out.push({ spec: text, text: '' });
        continue;
      }
      const last = out[out.length - 1];
      last.text += `${last.text ? ' ' : ''}${text}`;
    }
    return out.map((entry) => ({ ...entry, text: entry.text.trim() }));
  }

  let column = null;
  for (const raw of kept) {
    const indent = raw.length - raw.trimStart().length;
    if (column !== null && indent >= column && out.length) {
      out[out.length - 1].text += ` ${raw.trim()}`;
      continue;
    }
    const split = TWO_COLUMN.exec(raw);
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

/** The sections that are never a list of flags. */
const NOT_FLAGS = new Set(['description', 'usage', 'arguments', 'commands', 'after']);

/**
 * Every flag a command takes, including the ones under a heading of its own.
 *
 * **clap's `help_heading` puts flags in a section with any name at all**, and
 * three commands here use it: *Reaching it over SSH* on `db add` and `db edit`,
 * and *Making the destination* on `mirror` and `sync`. Reading only `Options:`
 * dropped all of them — eleven `--ssh-*` flags absent from a reference whose
 * whole promise is that a diff against `--help` finds nothing missing. That is
 * what `A8` shipped, and `A17` found it while writing the SSH page.
 *
 * So every section that is not one of the known non-flag ones is parsed too,
 * and kept only for the entries that actually *are* flags. That last part
 * matters: `Examples:` is a heading as well, on `db add` and `query`, and it
 * holds prose. Filtering on the shape rather than on a list of headings means a
 * heading nobody anticipated is handled without anybody editing this file.
 */
function optionsOf(found) {
  const out = [];
  for (const [name, lines] of Object.entries(found)) {
    if (NOT_FLAGS.has(name)) {
      continue;
    }
    for (const entry of entries(lines)) {
      const flag = { ...option(entry.spec), text: entry.text };
      if (flag.long || flag.short) {
        out.push(flag);
      }
    }
  }
  return out;
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
    options: optionsOf(short),
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

/**
 * Refuse to write a reference that parsed badly.
 *
 * **Rule 14: a concern that is only stated is a concern that gets forgotten.**
 * `--check` catches the JSON drifting from the binary, which is a different
 * failure — the first version of this file parsed one layout, wrote rows with
 * no flag and no description for three commands, and `--check` was perfectly
 * happy because the JSON matched what the parser produced. So the parser now
 * checks its own output, and a clap layout nobody anticipated fails the build
 * with the command and the row named instead of shipping.
 */
function audit(all) {
  const wrong = [];
  for (const node of all) {
    for (const flag of node.options) {
      if (!flag.long && !flag.short) {
        wrong.push(`${node.name}: "${flag.spec}" is not a flag`);
      } else if (!flag.text) {
        wrong.push(`${node.name}: ${flag.spec} has no description`);
      }
    }
    for (const argument of node.arguments) {
      if (!/^[<[]/.test(argument.name)) {
        wrong.push(`${node.name}: "${argument.name}" is not an argument`);
      }
    }
  }
  if (wrong.length) {
    fail(
      `${wrong.length} row(s) did not parse. The help layout has changed:`,
      ...wrong.slice(0, 12).map((line) => `  ${line}`),
      ...(wrong.length > 12 ? [`  … and ${wrong.length - 12} more`] : []),
      '',
      'Nothing was written. Fix entries()/sections() rather than the JSON.',
    );
  }
}

function build() {
  const sloop = binary();
  const version = execFileSync(sloop, ['--version'], { encoding: 'utf8' }).trim();
  const all = walk(sloop);
  audit(all);

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
