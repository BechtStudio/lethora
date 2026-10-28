// Tables: the markup that the formatting row's Table button (and Edit →
// Insert table…) inserts, the next free `<tab:N>` label, and the Word-like
// grid picker that chooses columns × rows; editing a table on the code
// (moving between cells, rows, columns, the header, merged cells). The text
// functions are pure and tested under `node --test`; the picker is the only
// DOM part. How tables look is the layout layer's (`tables.*`), never the
// inserted text's.

import { EditorSelection, type EditorState, type Extension, Prec, type Text, type TransactionSpec } from "@codemirror/state";
import { type EditorView, keymap } from "@codemirror/view";
import { closeBracket, codeAt, commentEnd, findFigures, MAX_FIGURE, mathEnd, rawEnd, stringEnd } from "./figures.ts";
import type { MenuItem } from "./pagemenu.ts";

/** The picker's largest table. */
export const MAX_COLUMNS = 10;
export const MAX_ROWS = 8;

/**
 * A labelled table figure of `rows` rows of `columns` empty cells, the
 * first of them a `table.header(...)` if `header` (so 3 rows with a header
 * are the header and two body rows), with an empty caption: the text and
 * the offset inside its first cell's `[]`.
 */
export function tableText(columns: number, rows: number, header: boolean, label: string): { text: string; cell: number } {
  const cols = Math.max(1, Math.min(MAX_COLUMNS, Math.round(columns)));
  const count = Math.max(1, Math.min(MAX_ROWS, Math.round(rows)));
  const cells = Array<string>(cols).fill("[]").join(", ");
  const lines = ["#figure(", "  table(", `    columns: ${cols},`];
  if (header) lines.push(`    table.header(${cells}),`);
  for (let i = header ? 1 : 0; i < count; i++) lines.push(`    ${cells},`);
  lines.push("  ),", "  caption: [],", `) <${label}>`);
  const text = lines.join("\n");
  // Nothing before the cells has brackets.
  return { text, cell: text.indexOf("[]") + 1 };
}

/** `tab:N` for the largest N of the `tab:<number>` labels in `names`, plus one (`tab:1` if none). */
export function nextTableLabel(names: Iterable<string>): string {
  let max = 0;
  for (const name of names) {
    const m = /^tab:(\d{1,9})$/.exec(name);
    if (m) max = Math.max(max, Number(m[1]));
  }
  return `tab:${max + 1}`;
}

/**
 * Where a table inserted with the cursor at `pos` goes, as a block of its
 * own: on the cursor's line if it is empty (blank lines added towards
 * neighbouring text), else after the paragraph holding it (the lines up to
 * the next empty one), after a blank line.
 */
export function tablePlace(doc: Text, pos: number): { from: number; to: number; before: string; after: string } {
  const blank = (n: number) => n >= 1 && n <= doc.lines && doc.line(n).text.trim() === "";
  const line = doc.lineAt(pos);
  if (line.text.trim() === "") {
    const before = line.number > 1 && !blank(line.number - 1) ? "\n" : "";
    const after = line.number === doc.lines ? "\n" : blank(line.number + 1) ? "" : "\n";
    return { from: line.from, to: line.to, before, after };
  }
  let end = line;
  while (end.number < doc.lines && !blank(end.number + 1)) end = doc.line(end.number + 1);
  // Before the empty line that follows, if any (it stays the separator).
  return { from: end.to, to: end.to, before: "\n\n", after: end.number === doc.lines ? "\n" : "" };
}

/** Inserts a table figure (see `tableText`, `tablePlace`) at the main cursor, the cursor in its first cell: one transaction. */
export function insertTable(state: EditorState, columns: number, rows: number, header: boolean, label: string): TransactionSpec {
  const { from, to, before, after } = tablePlace(state.doc, state.selection.main.head);
  const { text, cell } = tableText(columns, rows, header, label);
  return {
    changes: { from, to, insert: before + text + after },
    selection: EditorSelection.cursor(from + before.length + cell),
    scrollIntoView: true,
    userEvent: "input",
  };
}

// ---- Editing a table on the code ----
//
// The `table(...)` call holding the cursor is read into a grid: `columns`
// (a number, a track list or missing), the named arguments as written, a
// `table.header(...)` (its rows are the header rows), and the cells as
// written (`[...]`, any expression, `table.cell(colspan:, rowspan:)`),
// placed like Typst's auto-placement. A command changes the grid and prints
// the whole call back (one row per line, merged cells where they start; the
// cells and named arguments as written), sent as the span that changed, so
// an exception anchored at `table(` stays. Lines, footers, cells placed by
// x/y and comments between the arguments keep rows and columns as they are
// (moving between cells still works).

/** How far from the cursor a table call is looked for. */
const REACH = MAX_FIGURE;
const TABLE_CALL = "table(";
const NAME = /^([\p{ID_Start}_][\p{ID_Continue}\-]*)\s*:/u;
const COUNT = /^\d+$/;
const TRACK = /^(?:auto|\d+(?:\.\d+)?(?:fr|pt|mm|cm|in|em|%))$/;
const LINE = /^table\.[hv]line\b/;

const FIXED = "Rows and columns can't be changed here: this table";
const COMMENTS = `${FIXED} has comments between its cells.`;
const LINES = `${FIXED} has lines (table.hline, table.vline).`;
const FOOTER = `${FIXED} has a footer.`;
const PLACED = `${FIXED} places cells by x or y.`;
const SPANS = `${FIXED} has a merged cell whose size isn't a number, or doesn't fit.`;
const COLUMNS = `${FIXED}'s columns are neither a number nor a list.`;
const HEADER = `${FIXED}'s header is not its first part.`;
const OPAQUE = `${FIXED} has a cell that can't be read.`;

/** A cell (positions in the document). */
interface Cell {
  /** As written. */
  src: string;
  from: number;
  to: number;
  /** What Tab selects: inside the content's brackets, else the whole expression. */
  selFrom: number;
  selTo: number;
  /** A `table.cell(...)`: its other arguments (as written) and its body (a content block after the call, or an argument). */
  call: { args: string[]; body: string; trailing: boolean } | null;
  colspan: number;
  rowspan: number;
  /** Its first slot (−1 where the table isn't laid out). */
  row: number;
  col: number;
  /** Changed: printed anew. */
  dirty: boolean;
}

type Columns = { kind: "none" } | { kind: "count" } | { kind: "tracks"; tracks: string[] };

/** A `table(...)` call read into a grid. */
export interface Table {
  /** `table` … after its `)` (document positions). */
  from: number;
  end: number;
  /** Every cell in source order (the header's, the body's, a footer's). */
  cells: Cell[];
  /** Why rows and columns can't be changed, or null. */
  fixed: string | null;
  /** The named arguments as written, in order. */
  named: { name: string; src: string }[];
  /** The header's own named arguments (`repeat: false`), null without a header. */
  header: string[] | null;
  columns: Columns;
  cols: number;
  rows: number;
  headRows: number;
  /** The indentation of the call's line, and of its arguments. */
  indent: string;
  inner: string;
}

interface Piece {
  from: number;
  to: number;
}

/**
 * The arguments of the call whose `(` is at `open` and that ends at `end`
 * (after its `)`): their ranges without whitespace and comments around
 * them, and whether there are comments; null if it doesn't scan.
 */
function args(text: string, open: number, end: number): { pieces: Piece[]; comments: boolean } | null {
  const close = end - 1;
  const pieces: Piece[] = [];
  let comments = false;
  let from = -1;
  let to = -1;
  for (let i = open + 1; i < close; ) {
    const c = text.charCodeAt(i);
    if (c === 44 /* , */) {
      if (from >= 0) pieces.push({ from, to });
      from = to = -1;
      i++;
      continue;
    }
    if (c === 32 || c === 9 || c === 10 || c === 13) {
      i++;
      continue;
    }
    const comment = commentEnd(text, i, close);
    if (comment >= 0) {
      comments = true;
      i = comment;
      continue;
    }
    const next = c === 34 ? stringEnd(text, i, close) : c === 40 || c === 91 || c === 123 ? closeBracket(text, i, close) : c === 96 ? rawEnd(text, i, close) : c === 36 ? mathEnd(text, i, close) : i + 1;
    if (next < 0) return null;
    if (from < 0) from = i;
    to = i = next;
  }
  if (from >= 0) pieces.push({ from, to });
  return { pieces, comments };
}

/** Whether `a..b` of `text` is one content block `[...]`. */
const isBlock = (text: string, a: number, b: number) => text.charCodeAt(a) === 91 && closeBracket(text, a, b) === b;

/** The cell written at `p` (positions plus `base`), and why it fixes the table's rows and columns, if it does. */
function readCell(text: string, p: Piece, base: number): { cell: Cell; fixed: string | null } {
  const src = text.slice(p.from, p.to);
  const cell: Cell = { src, from: base + p.from, to: base + p.to, selFrom: base + p.from, selTo: base + p.to, call: null, colspan: 1, rowspan: 1, row: -1, col: -1, dirty: false };
  const select = (a: number, b: number) => {
    if (isBlock(text, a, b)) {
      cell.selFrom = base + a + 1;
      cell.selTo = base + b - 1;
    }
  };
  if (!src.startsWith("table.cell(")) {
    select(p.from, p.to);
    return { cell, fixed: null };
  }
  const open = p.from + "table.cell".length;
  const close = closeBracket(text, open, p.to);
  const list = close < 0 ? null : args(text, open, close);
  if (!list || (close < p.to && !isBlock(text, close, p.to))) return { cell, fixed: OPAQUE };
  let fixed: string | null = list.comments ? COMMENTS : null;
  const call = { args: [] as string[], body: "", trailing: close < p.to };
  let body: Piece | null = call.trailing ? { from: close, to: p.to } : null;
  for (const a of list.pieces) {
    const s = text.slice(a.from, a.to);
    const m = NAME.exec(s);
    if (!m) {
      if (body) fixed ??= OPAQUE;
      body = a;
      continue;
    }
    const value = s.slice(m[0].length).trim();
    if (m[1] === "colspan" || m[1] === "rowspan") {
      if (COUNT.test(value) && Number(value) >= 1) cell[m[1]] = Number(value);
      else fixed ??= SPANS;
    } else if (m[1] === "x" || m[1] === "y") fixed ??= PLACED;
    else call.args.push(s);
  }
  if (!body) return { cell, fixed: OPAQUE };
  call.body = text.slice(body.from, body.to);
  select(body.from, body.to);
  cell.call = call;
  return { cell, fixed };
}

/**
 * Places `cells` from row `start` as Typst's auto-placement does (each in
 * the next free slots, left to right, skipping slots of earlier merged
 * cells): the row after the last one they take, or −1 if one is wider than
 * the table.
 */
function place(cells: Cell[], cols: number, start: number, taken: Set<number>): number {
  let next = start * cols;
  let end = start;
  for (const c of cells) {
    if (c.colspan > cols) return -1;
    for (let at = next; ; at++) {
      const row = Math.floor(at / cols);
      const col = at % cols;
      if (col + c.colspan > cols) continue;
      let free = true;
      for (let r = row; free && r < row + c.rowspan; r++) for (let k = col; free && k < col + c.colspan; k++) free = !taken.has(r * cols + k);
      if (!free) continue;
      for (let r = row; r < row + c.rowspan; r++) for (let k = col; k < col + c.colspan; k++) taken.add(r * cols + k);
      c.row = row;
      c.col = col;
      next = at + c.colspan;
      end = Math.max(end, row + c.rowspan);
      break;
    }
  }
  return end;
}

/** The table call `from` (`table`) … `end` (after `)`) in `text`, positions plus `base`; null if it can't be read. */
function readTable(text: string, from: number, end: number, base: number): Table | null {
  const open = from + TABLE_CALL.length - 1;
  const list = args(text, open, end);
  if (!list) return null;
  let fixed: string | null = list.comments ? COMMENTS : null;
  const fix = (why: string | null) => {
    fixed ??= why;
  };
  const named: Table["named"] = [];
  const cells: Cell[] = [];
  const head: Cell[] = [];
  const body: Cell[] = [];
  let header: string[] | null = null;
  for (const p of list.pieces) {
    const s = text.slice(p.from, p.to);
    if (s.startsWith("..")) return null;
    const m = NAME.exec(s);
    if (m) {
      named.push({ name: m[1], src: s });
      continue;
    }
    if (LINE.test(s)) {
      fix(LINES);
      continue;
    }
    const group = /^table\.(header|footer)\(/.exec(s);
    if (!group) {
      const { cell, fixed: why } = readCell(text, p, base);
      fix(why);
      cells.push(cell);
      body.push(cell);
      continue;
    }
    const gOpen = p.from + group[0].length - 1;
    const inner = closeBracket(text, gOpen, p.to) === p.to ? args(text, gOpen, p.to) : null;
    if (!inner) return null;
    if (inner.comments) fix(COMMENTS);
    const isHeader = group[1] === "header" && header === null && body.length === 0;
    if (group[1] === "footer") fix(FOOTER);
    else if (!isHeader) fix(HEADER);
    const own: string[] = [];
    for (const q of inner.pieces) {
      const t = text.slice(q.from, q.to);
      if (t.startsWith("..")) return null;
      if (NAME.test(t)) own.push(t);
      else if (LINE.test(t)) fix(LINES);
      else {
        const { cell, fixed: why } = readCell(text, q, base);
        fix(why);
        cells.push(cell);
        if (isHeader) head.push(cell);
      }
    }
    if (isHeader) header = own;
  }

  let columns: Columns = { kind: "none" };
  let cols = 1;
  const given = named.find((n) => n.name === "columns");
  if (given) {
    const v = given.src.slice(NAME.exec(given.src)![0].length).trim();
    const tracks = v.startsWith("(") && closeBracket(v, 0, v.length) === v.length ? args(v, 0, v.length) : null;
    if (COUNT.test(v) && Number(v) >= 1) {
      columns = { kind: "count" };
      cols = Number(v);
    } else if (TRACK.test(v)) columns = { kind: "tracks", tracks: [v] };
    else if (tracks && !tracks.comments && tracks.pieces.length) {
      const list = tracks.pieces.map((q) => v.slice(q.from, q.to));
      if (list.some((t) => t.startsWith("..") || NAME.test(t))) fix(COLUMNS);
      columns = { kind: "tracks", tracks: list };
      cols = list.length;
    } else fix(COLUMNS);
  }

  let headRows = 0;
  let rows = 0;
  if (fixed === null) {
    const taken = new Set<number>();
    headRows = place(head, cols, 0, taken);
    rows = headRows < 0 ? -1 : place(body, cols, headRows, taken);
    if (rows < 0) {
      fix(SPANS);
      headRows = rows = 0;
    }
  }

  const lineStart = text.lastIndexOf("\n", from - 1) + 1;
  const indent = /^[ \t]*/.exec(text.slice(lineStart, from))![0];
  let innerIndent = `${indent}  `;
  const first = list.pieces[0];
  if (first) {
    const nl = text.lastIndexOf("\n", first.from - 1);
    const ws = text.slice(nl + 1, first.from);
    if (nl > open && /^[ \t]*$/.test(ws) && ws.length > indent.length) innerIndent = ws;
  }
  return { from: base + from, end: base + end, cells, fixed, named, header, columns, cols, rows, headRows, indent, inner: innerIndent };
}

/**
 * The `table(...)` call (parenthesised; not one with trailing content
 * blocks) whose parentheses hold `pos`, read into a grid; null if there is
 * none or it can't be read (e.g. cells spread in with `..`).
 */
export function tableAt(state: EditorState, pos = state.selection.main.head): Table | null {
  const doc = state.doc;
  const base = Math.max(0, pos - REACH);
  const text = doc.sliceString(base, Math.min(doc.length, pos + REACH));
  const at = pos - base;
  for (let i = at - TABLE_CALL.length; i >= 0; i--) {
    i = text.lastIndexOf(TABLE_CALL, i);
    if (i < 0) break;
    if (i > 0 && /[\p{ID_Continue}\-.]/u.test(text[i - 1])) continue;
    const end = closeBracket(text, i + TABLE_CALL.length - 1, text.length);
    if (end < 0 || at >= end || !codeAt(state, base + i)) continue;
    if (text.charCodeAt(end) === 91) return null;
    return readTable(text, i, end, base);
  }
  return null;
}

/** The index of the cell holding `pos` (its ends included), −1 if none. */
const cellAt = (t: Table, pos: number) => t.cells.findIndex((c) => c.from <= pos && pos <= c.to);

/** Selects a cell's content (Tab). */
const selectCell = (c: Cell): TransactionSpec => ({ selection: EditorSelection.single(c.selFrom, c.selTo), scrollIntoView: true, userEvent: "select" });

/**
 * Tab in a table: the next cell's content (from between cells: the cell
 * after); in the last cell a new empty row, the cursor in its first cell.
 * Nothing else in a table (`{}`: Tab stays in the editor); null outside a
 * table and while read only (Tab does what it does elsewhere).
 */
export function nextCell(state: EditorState): TransactionSpec | null {
  if (state.readOnly) return null;
  const t = tableAt(state);
  if (!t) return null;
  const pos = state.selection.main.head;
  const i = cellAt(t, pos);
  const next = i >= 0 ? i + 1 : t.cells.findIndex((c) => c.from >= pos);
  if (next >= 0 && next < t.cells.length) return selectCell(t.cells[next]);
  if (i >= 0 && !t.fixed) {
    const g = gridOf(t);
    return commit(state, t, g, insertRow(g, g.rows));
  }
  return {};
}

/** Shift+Tab in a table: the previous cell's content; nothing in the first cell (`{}`); null as `nextCell`. */
export function previousCell(state: EditorState): TransactionSpec | null {
  if (state.readOnly) return null;
  const t = tableAt(state);
  if (!t) return null;
  const pos = state.selection.main.head;
  const i = cellAt(t, pos);
  let prev = i - 1;
  if (i < 0) for (let k = t.cells.length - 1; k >= 0 && prev < 0; k--) if (t.cells[k].to <= pos) prev = k;
  return prev >= 0 ? selectCell(t.cells[prev]) : {};
}

// ---- The grid's commands ----

/** The table's cells and size being changed (cells copied from the table, same order). */
interface Grid {
  cells: Cell[];
  cols: number;
  rows: number;
  headRows: number;
  columns: Columns;
  header: string[] | null;
}

interface Slot {
  row: number;
  col: number;
}

const gridOf = (t: Table): Grid => ({
  cells: t.cells.map((c) => ({ ...c, call: c.call && { ...c.call, args: [...c.call.args] } })),
  cols: t.cols,
  rows: t.rows,
  headRows: t.headRows,
  columns: t.columns.kind === "tracks" ? { kind: "tracks", tracks: [...t.columns.tracks] } : t.columns,
  header: t.header && [...t.header],
});

/** A cell's content as markup (a content block's inside, else the expression as `#(...)`), for merging. */
function contentOf(c: Cell): string {
  const body = c.call ? c.call.body : c.src;
  if (isBlock(body, 0, body.length)) return body.slice(1, -1).trim();
  return `#${body.startsWith("(") ? body : `(${body})`}`;
}

/** A cell's source: as written, or printed anew with its spans. */
function cellSource(c: Cell): string {
  if (!c.dirty) return c.src;
  const body = c.call ? c.call.body : c.src;
  const list = [...(c.call?.args ?? [])];
  if (c.colspan > 1) list.push(`colspan: ${c.colspan}`);
  if (c.rowspan > 1) list.push(`rowspan: ${c.rowspan}`);
  if (!list.length) return body;
  const trailing = c.call ? c.call.trailing : isBlock(body, 0, body.length);
  return trailing ? `table.cell(${list.join(", ")})${body}` : `table.cell(${[...list, body].join(", ")})`;
}

const emptyCell = (row: number, col: number): Cell => ({ src: "[]", from: -1, to: -1, selFrom: -1, selTo: -1, call: null, colspan: 1, rowspan: 1, row, col, dirty: false });

/** The table call printed from the grid: one row per line, empty cells in free slots. */
function print(t: Table, g: Grid): string {
  const rows = g.rows;
  const taken = new Set<number>();
  for (const c of g.cells) for (let r = c.row; r < c.row + c.rowspan; r++) for (let k = c.col; k < c.col + c.colspan; k++) taken.add(r * g.cols + k);
  const cells = [...g.cells];
  for (let r = 0; r < rows; r++) for (let k = 0; k < g.cols; k++) if (!taken.has(r * g.cols + k)) cells.push(emptyCell(r, k));
  cells.sort((a, b) => a.row - b.row || a.col - b.col);
  const byRow: string[][] = Array.from({ length: rows }, () => []);
  for (const c of cells) byRow[c.row].push(cellSource(c));
  const columns = g.cols === t.cols ? null : g.columns.kind === "tracks" ? (g.columns.tracks.length === 1 ? `(${g.columns.tracks[0]},)` : `(${g.columns.tracks.join(", ")})`) : String(g.cols);
  const lines = ["table("];
  if (columns && !t.named.some((n) => n.name === "columns")) lines.push(`${t.inner}columns: ${columns},`);
  for (const n of t.named) lines.push(`${t.inner}${n.name === "columns" && columns ? `columns: ${columns}` : n.src},`);
  const head = byRow.slice(0, g.headRows).filter((r) => r.length);
  if (g.headRows > 0) {
    const own = g.header ?? [];
    if (head.length === 1) lines.push(`${t.inner}table.header(${[...own, ...head[0]].join(", ")}),`);
    else {
      const deeper = t.inner + (t.inner.length > t.indent.length ? t.inner.slice(t.indent.length) : "  ");
      lines.push(`${t.inner}table.header(`);
      if (own.length) lines.push(`${deeper}${own.join(", ")},`);
      for (const r of head) lines.push(`${deeper}${r.join(", ")},`);
      lines.push(`${t.inner}),`);
    }
  }
  for (const r of byRow.slice(g.headRows)) if (r.length) lines.push(`${t.inner}${r.join(", ")},`);
  lines.push(`${t.indent})`);
  return lines.join("\n");
}

/**
 * The transaction printing the grid in place of the table: only the span
 * that changed (so text and anchors before and after stay), the cursor at
 * the end of the content of the cell at `target`.
 */
function commit(state: EditorState, t: Table, g: Grid, target: Slot): TransactionSpec {
  const old = state.sliceDoc(t.from, t.end);
  const text = print(t, g);
  const max = Math.min(old.length, text.length);
  let a = 0;
  while (a < max && old.charCodeAt(a) === text.charCodeAt(a)) a++;
  let b = 0;
  while (b < max - a && old.charCodeAt(old.length - 1 - b) === text.charCodeAt(text.length - 1 - b)) b++;
  const after = readTable(text, 0, text.length, t.from);
  const cell = after?.cells.find((c) => c.row <= target.row && target.row < c.row + c.rowspan && c.col <= target.col && target.col < c.col + c.colspan);
  return {
    changes: { from: t.from + a, to: t.end - b, insert: text.slice(a, text.length - b) },
    selection: EditorSelection.cursor(cell ? cell.selTo : t.from + a),
    scrollIntoView: true,
    userEvent: "input",
  };
}

/** A new empty row `at` (a header row if it goes among the header's): merged cells across it grow; its first free slot. */
function insertRow(g: Grid, at: number): Slot {
  const across = new Set<number>();
  for (const c of g.cells) {
    if (c.row >= at) c.row++;
    else if (c.row + c.rowspan > at) {
      c.rowspan++;
      c.dirty = true;
      for (let k = c.col; k < c.col + c.colspan; k++) across.add(k);
    }
  }
  g.rows++;
  if (at < g.headRows) g.headRows++;
  let col = 0;
  while (col < g.cols && across.has(col)) col++;
  return { row: at, col: col < g.cols ? col : 0 };
}

/** A new empty column `at` (an `auto` track in a track list): merged cells across it grow. */
function insertColumn(g: Grid, at: number, row: number): Slot {
  for (const c of g.cells) {
    if (c.col >= at) c.col++;
    else if (c.col + c.colspan > at) {
      c.colspan++;
      c.dirty = true;
    }
  }
  g.cols++;
  if (g.columns.kind === "tracks") g.columns.tracks.splice(at, 0, "auto");
  return { row, col: at };
}

/** Removes rows `r0..r1`: merged cells across them shrink, cells only there go. */
function deleteRows(g: Grid, r0: number, r1: number, col: number): Slot {
  g.cells = g.cells.filter((c) => {
    const overlap = Math.max(0, Math.min(c.row + c.rowspan, r1) - Math.max(c.row, r0));
    if (overlap === c.rowspan) return false;
    if (overlap) {
      c.rowspan -= overlap;
      c.dirty = true;
    }
    if (c.row >= r1) c.row -= r1 - r0;
    else if (c.row >= r0) c.row = r0;
    return true;
  });
  g.rows -= r1 - r0;
  g.headRows -= Math.max(0, Math.min(g.headRows, r1) - r0);
  if (g.headRows === 0) g.header = null;
  return { row: Math.max(0, Math.min(r0, g.rows - 1)), col };
}

/** Removes columns `c0..c1` (their tracks too): merged cells across them shrink, cells only there go. */
function deleteColumns(g: Grid, c0: number, c1: number, row: number): Slot {
  g.cells = g.cells.filter((c) => {
    const overlap = Math.max(0, Math.min(c.col + c.colspan, c1) - Math.max(c.col, c0));
    if (overlap === c.colspan) return false;
    if (overlap) {
      c.colspan -= overlap;
      c.dirty = true;
    }
    if (c.col >= c1) c.col -= c1 - c0;
    else if (c.col >= c0) c.col = c0;
    return true;
  });
  g.cols -= c1 - c0;
  if (g.columns.kind === "tracks") g.columns.tracks.splice(c0, c1 - c0);
  return { row, col: Math.max(0, Math.min(c0, g.cols - 1)) };
}

interface Rect {
  r0: number;
  r1: number;
  c0: number;
  c1: number;
}

/**
 * Merges the cells of `rect` into its top-left one (their contents joined
 * with spaces): only if they fill it exactly, are two or more and are all
 * header or all body cells; null otherwise.
 */
function merge(g: Grid, rect: Rect): Slot | null {
  const { r0, r1, c0, c1 } = rect;
  const inside = g.cells.filter((c) => c.row < r1 && c.row + c.rowspan > r0 && c.col < c1 && c.col + c.colspan > c0);
  if (inside.length < 2 || !(r1 <= g.headRows || r0 >= g.headRows)) return null;
  if (inside.some((c) => c.row < r0 || c.row + c.rowspan > r1 || c.col < c0 || c.col + c.colspan > c1)) return null;
  if (inside.reduce((n, c) => n + c.rowspan * c.colspan, 0) !== (r1 - r0) * (c1 - c0)) return null;
  inside.sort((a, b) => a.row - b.row || a.col - b.col);
  const first = inside[0];
  const joined = inside.map(contentOf).filter((s) => s !== "");
  first.call = { args: first.call?.args ?? [], body: `[${joined.join(" ")}]`, trailing: true };
  first.colspan = c1 - c0;
  first.rowspan = r1 - r0;
  first.dirty = true;
  g.cells = g.cells.filter((c) => c === first || !inside.includes(c));
  return { row: r0, col: c0 };
}

/** The selected cells' rectangle (the anchor's and the head's cells, whole) and the head's cell index; null outside cells or in a fixed table. */
function selected(state: EditorState, t: Table): { rect: Rect; head: number } | null {
  if (t.fixed) return null;
  const { anchor, head } = state.selection.main;
  const h = cellAt(t, head);
  if (h < 0) return null;
  const a = cellAt(t, anchor);
  const ends = a >= 0 ? [t.cells[h], t.cells[a]] : [t.cells[h]];
  return {
    head: h,
    rect: {
      r0: Math.min(...ends.map((c) => c.row)),
      r1: Math.max(...ends.map((c) => c.row + c.rowspan)),
      c0: Math.min(...ends.map((c) => c.col)),
      c1: Math.max(...ends.map((c) => c.col + c.colspan)),
    },
  };
}

/**
 * Deletes the table: the whole figure (with its label) if the table is the
 * figure's body (a figure without it would be broken), else the call (and
 * its `#`); whole lines when it has its lines to itself (one blank line
 * kept between the text around it).
 */
function deleteTable(state: EditorState, t: Table): TransactionSpec {
  const doc = state.doc;
  const figure = findFigures(state, t.from, t.end).find((f) => f.from < t.from && t.end <= f.callEnd && !state.sliceDoc(f.from + "#figure(".length, t.from).trim());
  let from = figure ? figure.from : t.from - (state.sliceDoc(t.from - 1, t.from) === "#" ? 1 : 0);
  let to = figure ? figure.to : t.end;
  const first = doc.lineAt(from);
  const last = doc.lineAt(to);
  if (!state.sliceDoc(first.from, from).trim() && !state.sliceDoc(to, last.to).trim()) {
    from = first.from;
    to = last.to;
    if (last.number < doc.lines) {
      to++;
      const next = doc.line(last.number + 1);
      const blankBefore = first.number === 1 || !doc.line(first.number - 1).text.trim();
      if (blankBefore && !next.text.trim()) to = next.number < doc.lines ? next.to + 1 : next.to;
    } else if (first.number > 1) from--;
  }
  return { changes: { from, to }, selection: EditorSelection.cursor(from), scrollIntoView: true, userEvent: "delete" };
}

export type TableAction = "row-above" | "row-below" | "column-left" | "column-right" | "delete-row" | "delete-column" | "delete-table" | "header" | "merge" | "split";

/**
 * A table command at the selection, one transaction; null where it doesn't
 * apply (outside a table, read only, outside cells, a fixed table, ...).
 * Rows and columns are the selected cells' (merged cells whole): rows are
 * inserted above the first / below the last, deleted all; deleting every
 * row or column deletes the table. Merge takes the rectangle from the
 * anchor's cell to the head's; split makes a merged cell plain again
 * (empty cells in the freed slots).
 */
export function tableCommand(state: EditorState, action: TableAction): TransactionSpec | null {
  if (state.readOnly) return null;
  const t = tableAt(state);
  if (!t) return null;
  if (action === "delete-table") return deleteTable(state, t);
  const g = gridOf(t);
  if (action === "header") {
    if (t.fixed || t.rows === 0) return null;
    if (g.headRows > 0) {
      g.headRows = 0;
      g.header = null;
    } else {
      if (g.cells.some((c) => c.row === 0 && c.rowspan > 1)) return null;
      g.headRows = 1;
      g.header = [];
    }
    const at = cellAt(t, state.selection.main.head);
    return commit(state, t, g, at >= 0 ? t.cells[at] : { row: 0, col: 0 });
  }
  const sel = selected(state, t);
  if (!sel) return null;
  const { rect } = sel;
  const head = g.cells[sel.head];
  switch (action) {
    case "row-above":
      return commit(state, t, g, insertRow(g, rect.r0));
    case "row-below":
      return commit(state, t, g, insertRow(g, rect.r1));
    case "column-left":
      return commit(state, t, g, insertColumn(g, rect.c0, head.row));
    case "column-right":
      return commit(state, t, g, insertColumn(g, rect.c1, head.row));
    case "delete-row":
      return rect.r0 === 0 && rect.r1 >= t.rows ? deleteTable(state, t) : commit(state, t, g, deleteRows(g, rect.r0, rect.r1, head.col));
    case "delete-column":
      return rect.c0 === 0 && rect.c1 >= t.cols ? deleteTable(state, t) : commit(state, t, g, deleteColumns(g, rect.c0, rect.c1, head.row));
    case "merge": {
      const at = merge(g, rect);
      return at && commit(state, t, g, at);
    }
    case "split":
      if (head.colspan === 1 && head.rowspan === 1) return null;
      head.colspan = head.rowspan = 1;
      head.dirty = true;
      return commit(state, t, g, head);
  }
}

const MENU: Array<[string, TableAction] | null> = [
  ["Insert row above", "row-above"],
  ["Insert row below", "row-below"],
  ["Insert column left", "column-left"],
  ["Insert column right", "column-right"],
  null,
  ["Delete row", "delete-row"],
  ["Delete column", "delete-column"],
  ["Delete table", "delete-table"],
  null,
  ["Header row", "header"],
  ["Merge cells", "merge"],
  ["Split cell", "split"],
];

/** The table commands as menu items for the selection (none outside a table); those that don't apply are greyed out. */
export function tableMenu(state: EditorState): MenuItem<TableAction>[] {
  const t = tableAt(state);
  if (!t) return [];
  return MENU.map((entry): MenuItem<TableAction> => {
    if (!entry) return { type: "separator" };
    const [label, action] = entry;
    const act = tableCommand(state, action) ? action : null;
    const item: Extract<MenuItem<TableAction>, { type: "item" }> = { type: "item", label, act };
    if (action === "header") item.checked = t.header !== null;
    if (!act && t.fixed && action !== "delete-table") item.title = t.fixed;
    return item;
  });
}

const keyRun = (command: (state: EditorState) => TransactionSpec | null) => (view: EditorView) => {
  const spec = command(view.state);
  if (!spec) return false;
  view.dispatch(spec);
  return true;
};

/** Tab / Shift+Tab between a table's cells (below the `@` completion's Tab). */
export const tableKeys: Extension = Prec.high(keymap.of([{ key: "Tab", run: keyRun(nextCell), shift: keyRun(previousCell) }]));

/** "3 × 4 table" (columns × rows, as Word shows it). */
export const sizeLabel = (columns: number, rows: number) => `${columns} × ${rows} table`;

// ---- The grid picker ----

export interface PickerHost {
  /** Inserts a table of `columns` × `rows` (the picker is closed first). */
  insert(columns: number, rows: number, header: boolean): void;
  /** The picker closed without inserting (Esc, a click outside): the focus goes back. */
  closed(): void;
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, props: Partial<HTMLElementTagNameMap[K]> = {}, ...children: Array<Node | string>): HTMLElementTagNameMap[K] {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...children);
  return e;
}

/**
 * The popup grid: hovering highlights columns × rows from the top left,
 * a click inserts; with the keyboard the arrows move the highlight, Enter
 * inserts, Esc closes. A "Header row" checkbox (on by default) makes the
 * first row the header.
 */
export class TablePicker {
  private root: HTMLElement | null = null;
  private host: PickerHost | null = null;
  private cells: HTMLElement[] = [];
  private label: HTMLElement | null = null;
  private header: HTMLInputElement | null = null;
  private columns = 1;
  private rows = 1;
  private onOutside = (e: Event) => {
    if (this.root && !this.root.contains(e.target as Node)) this.close();
  };
  private onResize = () => this.close();

  get isOpen(): boolean {
    return this.root !== null;
  }

  /** Opens below `anchor` (viewport pixels), the grid focused at 1 × 1. */
  open(anchor: { left: number; top: number; bottom: number }, host: PickerHost): void {
    this.close(false);
    this.host = host;
    this.columns = this.rows = 1;
    const grid = el("div", { className: "tp-grid", tabIndex: 0 });
    grid.setAttribute("role", "group");
    grid.setAttribute("aria-label", "Table size, columns × rows: the arrow keys change it, Enter inserts");
    grid.setAttribute("aria-describedby", "tp-label");
    grid.style.setProperty("--tp-cols", String(MAX_COLUMNS));
    this.cells = [];
    for (let r = 1; r <= MAX_ROWS; r++) {
      for (let c = 1; c <= MAX_COLUMNS; c++) {
        const cell = el("div", { className: "tp-cell" });
        cell.addEventListener("pointerenter", () => this.set(c, r));
        cell.addEventListener("click", () => this.insert());
        this.cells.push(cell);
        grid.append(cell);
      }
    }
    grid.addEventListener("keydown", (e) => this.key(e));
    const label = el("div", { className: "tp-label", id: "tp-label" });
    label.setAttribute("aria-live", "polite");
    const header = el("input", { type: "checkbox", id: "tp-header", checked: true });
    header.addEventListener("keydown", (e) => {
      if (e.key === "Escape") this.escapeKey(e);
      else if (e.key === "Enter") {
        e.preventDefault();
        this.insert();
      }
    });
    header.addEventListener("change", () => this.set(this.columns, this.rows));
    const option = el("label", { className: "tp-option", htmlFor: "tp-header" }, header, " Header row");
    const root = el("div", { className: "table-picker" }, grid, label, option);
    root.setAttribute("role", "dialog");
    root.setAttribute("aria-label", "Insert table");
    root.addEventListener("contextmenu", (e) => e.preventDefault());
    document.body.append(root);
    this.root = root;
    this.label = label;
    this.header = header;
    this.set(1, 1);
    const w = root.offsetWidth;
    const h = root.offsetHeight;
    const top = anchor.bottom + 4 + h + 4 <= window.innerHeight ? anchor.bottom + 4 : Math.max(4, anchor.top - 4 - h);
    root.style.left = `${Math.max(4, Math.min(anchor.left, window.innerWidth - w - 4))}px`;
    root.style.top = `${top}px`;
    window.addEventListener("pointerdown", this.onOutside, true);
    window.addEventListener("resize", this.onResize);
    window.addEventListener("blur", this.onResize);
    grid.focus();
  }

  /** Closes the picker (`restore`: tell the host, which puts focus back). */
  close(restore = true): void {
    if (!this.root) return;
    this.root.remove();
    this.root = this.label = this.header = null;
    this.cells = [];
    window.removeEventListener("pointerdown", this.onOutside, true);
    window.removeEventListener("resize", this.onResize);
    window.removeEventListener("blur", this.onResize);
    const host = this.host;
    this.host = null;
    if (restore) host?.closed();
  }

  /** Esc: closes the picker; true if it was open. */
  escape(): boolean {
    if (!this.root) return false;
    this.close();
    return true;
  }

  private set(columns: number, rows: number): void {
    this.columns = Math.max(1, Math.min(MAX_COLUMNS, columns));
    this.rows = Math.max(1, Math.min(MAX_ROWS, rows));
    const header = this.header?.checked ?? true;
    this.cells.forEach((cell, i) => {
      const c = (i % MAX_COLUMNS) + 1;
      const r = Math.floor(i / MAX_COLUMNS) + 1;
      cell.classList.toggle("on", c <= this.columns && r <= this.rows);
      cell.classList.toggle("head", header && r === 1 && c <= this.columns);
    });
    if (this.label) this.label.textContent = sizeLabel(this.columns, this.rows);
  }

  private insert(): void {
    const host = this.host;
    const { columns, rows } = this;
    const header = this.header?.checked ?? true;
    this.close(false);
    host?.insert(columns, rows, header);
  }

  private escapeKey(e: KeyboardEvent): void {
    e.preventDefault();
    e.stopPropagation();
    this.close();
  }

  private key(e: KeyboardEvent): void {
    const move: Record<string, [number, number]> = { ArrowRight: [1, 0], ArrowLeft: [-1, 0], ArrowDown: [0, 1], ArrowUp: [0, -1] };
    const step = move[e.key];
    if (step) {
      e.preventDefault();
      this.set(this.columns + step[0], this.rows + step[1]);
    } else if (e.key === "Home") {
      e.preventDefault();
      this.set(1, 1);
    } else if (e.key === "End") {
      e.preventDefault();
      this.set(MAX_COLUMNS, MAX_ROWS);
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      this.insert();
    } else if (e.key === "Escape") this.escapeKey(e);
  }
}
