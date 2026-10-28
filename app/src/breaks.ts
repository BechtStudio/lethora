// Enter, Shift+Enter, Backspace and Delete in Typst terms. In markup, Enter
// starts a new paragraph (a blank line) or continues a list, Shift+Enter is
// a forced line break (`\`), and Backspace/Delete over either break remove
// it whole; in math, Shift+Enter is math's line break. Elsewhere (code, raw
// text, Enter in math) the keys work as before.

import { insertNewlineAndIndent } from "@codemirror/commands";
import { EditorSelection, type EditorState, type TransactionSpec } from "@codemirror/state";
import { type Command, keymap } from "@codemirror/view";
import { modeAt } from "./typst.ts";

/** A list (`-`), enum (`+`, `1.`) or term (`/ `) item marker, with its indentation and the spaces after it. */
const ITEM = /^([ \t]*)(?:([-+]|(\d+)\.)(?=[ \t]|$)|\/(?= ))[ \t]*/;
const BLANK = /^\s*$/;
const SPACES = /^[ \t]*/;

/** Spaces and tabs after `pos` on its line. */
function spacesAfter(state: EditorState, pos: number): number {
  const line = state.doc.lineAt(pos);
  return SPACES.exec(line.text.slice(pos - line.from))![0].length;
}

/** The item marker of a line and whether `pos` is after it, else null. */
function itemAt(text: string, col: number): RegExpExecArray | null {
  const item = ITEM.exec(text);
  return item && col >= item[0].length ? item : null;
}

/**
 * Enter: in markup a blank line ("\n\n", only "\n" on a blank line), with
 * the line's indentation; on a list, enum or term item the next item, and
 * on an empty item the end of the list (its marker becomes a blank line).
 * `null` outside markup (insert a plain line break).
 */
export function paragraphBreak(state: EditorState): TransactionSpec | null {
  if (modeAt(state, state.selection.main.from) !== "markup") return null;
  return state.changeByRange((range) => {
    const line = state.doc.lineAt(range.from);
    const col = range.from - line.from;
    const item = itemAt(line.text, col);
    if (item && range.empty && item[0].length === line.length) {
      return { changes: { from: line.from, to: line.to, insert: "\n" }, range: EditorSelection.cursor(line.from + 1) };
    }
    let from = range.from;
    let insert: string;
    if (item) {
      const marker = item[3] ? `${Number(item[3]) + 1}.` : (item[2] ?? "/");
      insert = `\n${item[1]}${marker} `;
    } else if (range.empty && BLANK.test(line.text)) {
      insert = "\n";
    } else {
      // Trailing spaces stay out of the paragraph before.
      while (from > line.from && /[ \t]/.test(line.text[from - line.from - 1]) && /\S/.test(line.text.slice(0, from - line.from))) from--;
      insert = `\n\n${SPACES.exec(line.text)![0]}`;
    }
    const to = range.to + spacesAfter(state, range.to);
    return { changes: { from, to, insert }, range: EditorSelection.cursor(from + insert.length) };
  });
}

/**
 * Shift+Enter: in markup and math a forced line break, `\` then a line
 * break (after a space unless one is there) and the line's indentation (on
 * an item line: aligned with the item's text, so the break stays in the
 * item). `null` in code and raw text (insert a plain line break).
 */
export function lineBreak(state: EditorState): TransactionSpec | null {
  const mode = modeAt(state, state.selection.main.from);
  if (mode !== "markup" && mode !== "math") return null;
  return state.changeByRange((range) => {
    const line = state.doc.lineAt(range.from);
    const col = range.from - line.from;
    const item = mode === "markup" ? itemAt(line.text, col) : null;
    const indent = item ? " ".repeat(item[0].length) : SPACES.exec(line.text)![0];
    const space = col > 0 && !/\s/.test(line.text[col - 1]) ? " " : "";
    const insert = `${space}\\\n${indent}`;
    const to = range.to + spacesAfter(state, range.to);
    return { changes: { from: range.from, to, insert }, range: EditorSelection.cursor(range.from + insert.length) };
  });
}

/** A forced line break (`\` after whitespace or at the line start) ending a line. */
const FORCED = /(^|\s)\\\s*$/;

/**
 * Joins line `upper` with line `lower` below it (the lines between are
 * blank): a forced line break between them becomes a space, a paragraph
 * break (blank lines) goes away and the two run together, as in Word. `null`
 * if they are neither (an ordinary line break; the default applies).
 */
function join(state: EditorState, upper: number, lower: number): { from: number; to: number; insert: string } | null {
  const { doc } = state;
  const up = doc.line(upper);
  const down = doc.line(lower);
  const forced = FORCED.test(up.text);
  if (BLANK.test(up.text) || (lower === upper + 1 && !forced)) return null;
  const text = forced ? up.text.replace(/\s*\\\s*$/, "") : up.text.trimEnd();
  const from = up.from + text.length;
  const to = down.from + SPACES.exec(down.text)![0].length;
  const insert = forced && lower === upper + 1 && text !== "" && !BLANK.test(down.text) ? " " : "";
  return { from, to, insert };
}

/** The nearest non-blank line above line `n` (0 if none). */
function textAbove(state: EditorState, n: number): number {
  while (--n >= 1 && BLANK.test(state.doc.line(n).text));
  return n;
}

/**
 * Backspace in markup: at the start of a line (only indentation before the
 * cursor), or right after a forced line break `\`, removes the whole break
 * to the text before: a forced line break or the blank lines of a paragraph
 * break. `null` elsewhere (the default applies).
 */
export function joinBackward(state: EditorState): TransactionSpec | null {
  const range = state.selection.main;
  if (!range.empty || state.selection.ranges.length > 1 || modeAt(state, range.head) !== "markup") return null;
  const line = state.doc.lineAt(range.head);
  const before = line.text.slice(0, range.head - line.from);
  const after = line.text.slice(range.head - line.from);
  let change: ReturnType<typeof join> = null;
  if (BLANK.test(before)) {
    const upper = textAbove(state, line.number);
    if (upper >= 1) change = join(state, upper, line.number);
    if (change) change.to = Math.max(change.to, range.head);
  } else if (FORCED.test(before) && BLANK.test(after) && line.number < state.doc.lines && !BLANK.test(state.doc.line(line.number + 1).text)) {
    change = join(state, line.number, line.number + 1);
  }
  return change && { changes: change, selection: EditorSelection.cursor(change.from + change.insert.length) };
}

/**
 * Delete in markup at the end of a line (only spaces after the cursor):
 * removes a forced line break or a paragraph break to the text below, like
 * Backspace from there. `null` elsewhere (the default applies).
 */
export function joinForward(state: EditorState): TransactionSpec | null {
  const range = state.selection.main;
  if (!range.empty || state.selection.ranges.length > 1 || modeAt(state, range.head) !== "markup") return null;
  const line = state.doc.lineAt(range.head);
  if (!BLANK.test(line.text.slice(range.head - line.from))) return null;
  let lower = line.number + 1;
  while (lower <= state.doc.lines && BLANK.test(state.doc.line(lower).text)) lower++;
  if (lower > state.doc.lines) return null;
  const change = join(state, line.number, lower);
  return change && { changes: change, selection: EditorSelection.cursor(change.from + change.insert.length) };
}

/** Runs `spec` as a typed edit, or inserts a plain line break where it does not apply. */
const breakCommand =
  (spec: (state: EditorState) => TransactionSpec | null): Command =>
  (view) => {
    const tr = spec(view.state);
    if (!tr) return insertNewlineAndIndent(view);
    view.dispatch(view.state.update(tr, { scrollIntoView: true, userEvent: "input" }));
    return true;
  };

/** Runs `spec` as a delete, or leaves the key to the default keymap where it does not apply. */
const joinCommand =
  (spec: (state: EditorState) => TransactionSpec | null): Command =>
  (view) => {
    const tr = spec(view.state);
    if (!tr) return false;
    view.dispatch(view.state.update(tr, { scrollIntoView: true, userEvent: "delete.join" }));
    return true;
  };

/** Enter, Shift+Enter, and Backspace/Delete over a line or paragraph break (before the default keymap). */
export const breakKeys = keymap.of([
  { key: "Enter", run: breakCommand(paragraphBreak), shift: breakCommand(lineBreak) },
  { key: "Backspace", run: joinCommand(joinBackward) },
  { key: "Delete", run: joinCommand(joinForward) },
]);
