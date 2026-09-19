import { ChangeDetectionStrategy, Component } from '@angular/core';
import { RouterLink } from '@angular/router';

import { DocsPage } from '../docs-page';
import { Terminal, type TerminalLine } from '../../ui/terminal';

/** One step of the builder, for the list that stands in for a screen recording. */
interface Step {
  readonly asks: string;
  readonly detail: string;
}

/**
 * Reading a database.
 *
 * ## How these blocks were made
 *
 * A throwaway PostgreSQL 17.9 cluster on port 5446 and a throwaway MySQL 8.4.11
 * on 3312 — the portable one under `%LOCALAPPDATA%\sloop-test-engines` — on
 * 2026-09-19, with the binary built from this commit. Both torn down after.
 *
 * **The read-only guarantee was attacked rather than asserted**, which is the
 * whole point of the page. Six real runs: `UPDATE`, `DELETE` and `CREATE TABLE`
 * through `--sql` on PostgreSQL, the same on MySQL, and — the one that matters —
 * a write hidden behind an allowed first word on *both* engines:
 *
 * ```sql
 * WITH doomed AS (DELETE FROM customers RETURNING id) SELECT count(*) FROM doomed
 * ```
 *
 * That statement begins with `WITH`. Any keyword blacklist lets it through, and
 * both servers refused it anyway. That is the difference between a guarantee and
 * a list, and it is on the page as two captured errors rather than as a claim.
 *
 * **The per-engine difference is real and the page says so.** On PostgreSQL the
 * refusal is entirely the server's. On MySQL and MariaDB the read-only
 * transaction catches every data change but **not** DDL, because `CREATE TABLE`
 * commits implicitly before it runs — so sloop adds a first-word fence there,
 * and its message says plainly which of the two refused. Verified on both
 * engines rather than taken from `cli/src/engine/mysql.rs`, though that file
 * says the same thing.
 *
 * ## What is quoted rather than captured
 *
 * **The builder's questions.** It is a screen of prompts drawn in raw mode, so a
 * redirected run never records one — the same reason `A7` quotes two prompts
 * from source. The six questions, the eleven tests and the two hundred rows a
 * page are read from `cli/src/commands/query.rs` and `cli/src/query/mod.rs`,
 * named here so the quoting is visible rather than implied. Every terminal block
 * below is a real run.
 */
@Component({
  selector: 'app-docs-query',
  imports: [DocsPage, RouterLink, Terminal],
  templateUrl: './query.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Query {
  // No OS strip on this page: nothing it shows is a path, and the two engines it
  // distinguishes are the database's, not the reader's machine.

  /** The builder, question by question, in the order it asks them. */
  protected readonly steps: readonly Step[] = [
    {
      asks: 'Which database?',
      detail:
        'Skipped when you named one on the command line. Otherwise every database this registry knows, as a list.',
    },
    {
      asks: 'Which table?',
      detail:
        'The tables that are actually there, read from the catalogue. Not typed, so it cannot be mistyped.',
    },
    {
      asks: 'Which columns?',
      detail: 'A tick list of that table’s columns. Tick none and you get all of them.',
    },
    {
      asks: 'Narrow it down?',
      detail:
        'No, and every row comes back. Yes, and it asks the next three — then offers another condition.',
    },
    {
      asks: 'Which column? / And it…',
      detail:
        'The test, from a list of eleven: is, is not, contains, starts with, ends with, is more than, is less than, is at least, is at most, is empty, is not empty.',
    },
    {
      asks: 'What are you looking for?',
      detail:
        'The only free text in the whole builder, and it is a value rather than a fragment of a statement. The tests that need no value — is empty, is not empty — skip it.',
    },
  ];

  // ── --sql ─────────────────────────────────────────────────────────────────

  protected readonly noTerminal: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop query shop   # from a script' },
    { kind: 'label', tag: 'Reading: ', text: 'shop', note: '  project' },
    {
      kind: 'bad',
      tag: 'error:',
      text: ' building a query is a screen of questions, and there is no terminal to ask at',
    },
    {
      kind: 'dim',
      text: '  hint: give the statement instead: `sloop query <name> --sql "select ..."`',
    },
  ];

  protected readonly sql: readonly TerminalLine[] = [
    {
      kind: 'prompt',
      text: 'sloop query shop --sql "select city, count(*) as customers,',
    },
    { kind: 'prompt', text: '    round(avg(spend),2) as avg_spend from customers group by city"' },
    { kind: 'label', tag: 'Reading: ', text: 'shop', note: '  project' },
    { text: ' ' },
    { kind: 'head', tag: 'city    customers  avg_spend', text: '' },
    { text: 'Lisbon  321        237.54' },
    { text: 'Dhaka   321        238.28' },
    { text: 'Berlin  321        237.17' },
    { text: 'Osaka   321        237.91' },
    { text: ' ' },
    { kind: 'label', tag: 'Rows: ', text: '4' },
  ];

  // ── The guarantee ─────────────────────────────────────────────────────────

  /** PostgreSQL refusing three different writes, in its own words. */
  protected readonly postgresRefuses: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop query shop --sql "update customers set city = \'Nowhere\'"' },
    { kind: 'label', tag: 'Reading: ', text: 'shop', note: '  project' },
    {
      kind: 'bad',
      tag: 'error:',
      text: ' psql.exe failed against postgres://app@127.0.0.1:5446/shop:',
    },
    { kind: 'bad', tag: '', text: '  ERROR:  cannot execute UPDATE in a read-only transaction' },
    { text: ' ' },
    { kind: 'prompt', text: 'sloop query shop --sql "delete from orders"' },
    { kind: 'bad', tag: '', text: '  ERROR:  cannot execute DELETE in a read-only transaction' },
    { text: ' ' },
    { kind: 'prompt', text: 'sloop query shop --sql "create table sneaky (id int)"' },
    {
      kind: 'bad',
      tag: '',
      text: '  ERROR:  cannot execute CREATE TABLE in a read-only transaction',
    },
  ];

  /** The statement a keyword list cannot catch, refused by both servers anyway. */
  protected readonly hidden: readonly TerminalLine[] = [
    { kind: 'dim', text: '# it begins with WITH, so no list of forbidden first words sees it' },
    {
      kind: 'prompt',
      text: 'sloop query shop --sql "with doomed as (delete from customers returning id)',
    },
    { kind: 'prompt', text: '    select count(*) from doomed"' },
    { kind: 'label', tag: 'Reading: ', text: 'shop', note: '  project' },
    {
      kind: 'bad',
      tag: 'error:',
      text: ' psql.exe failed against postgres://app@127.0.0.1:5446/shop:',
    },
    { kind: 'bad', tag: '', text: '  ERROR:  cannot execute SELECT in a read-only transaction' },
    { text: ' ' },
    { kind: 'dim', text: '# the MySQL shape of the same trick, on MySQL 8.4.11' },
    {
      kind: 'prompt',
      text: 'sloop query mshop --sql "with doomed as (select id from customers)',
    },
    {
      kind: 'prompt',
      text: '    delete customers from customers join doomed on doomed.id = customers.id"',
    },
    {
      kind: 'bad',
      tag: 'error:',
      text: ' mysql.exe failed against mysql://app@127.0.0.1:3312/shop:',
    },
    {
      kind: 'bad',
      tag: '',
      text: '  ERROR 1792 (25006) at line 1: Cannot execute statement in a READ ONLY transaction.',
    },
  ];

  /** Where MySQL's own fence stops, and sloop says so. */
  protected readonly mysqlDdl: readonly TerminalLine[] = [
    { kind: 'prompt', text: 'sloop query mshop --sql "create table sneaky (id int)"' },
    { kind: 'label', tag: 'Reading: ', text: 'mshop', note: '  project' },
    {
      kind: 'bad',
      tag: 'error:',
      text: ' sloop only reads, and CREATE is not a statement that reads',
    },
    {
      kind: 'dim',
      text: '  hint: this engine commits a transaction before it runs a statement like that, so its',
    },
    {
      kind: 'dim',
      text: '  own read-only transaction cannot refuse it and sloop does. Statements that read:',
    },
    {
      kind: 'dim',
      text: '  SELECT, WITH, SHOW, EXPLAIN, DESCRIBE, DESC, TABLE, VALUES, ANALYZE',
    },
  ];

  /** A large result, printed rather than paged, because a flag form goes in a script. */
  protected readonly many: readonly TerminalLine[] = [
    {
      kind: 'prompt',
      text: 'sloop query shop --sql "select id, name, city, spend from customers"',
    },
    { kind: 'label', tag: 'Reading: ', text: 'shop', note: '  project' },
    { text: ' ' },
    { kind: 'head', tag: 'id    name           city    spend', text: '' },
    { text: '1     Customer 1     Berlin  0.37' },
    { text: '2     Customer 2     Lisbon  0.74' },
    { text: '3     Customer 3     Osaka   1.11' },
    { kind: 'dim', text: '…' },
    { text: '1283  Customer 1283  Osaka   474.71' },
    { text: '1284  Customer 1284  Dhaka   475.08' },
    { text: ' ' },
    { kind: 'label', tag: 'Rows: ', text: '1284' },
  ];
}
