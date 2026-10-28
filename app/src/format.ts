// Word-like formatting for the author and science views, written as Typst
// markup: inline formats (bold `*…*`, italic `_…_`, `#underline[…]`, ...)
// and paragraph kinds (headings, lists) at the line start. Pure functions
// from the editor state to a transaction (`{}` where nothing applies); the
// edits sync to the core like typing. No DOM.
//
// Inline formats are line-bounded, as in the editor's highlighting: spans
// are found per line with the markup tokenizer (typst.ts).

import { StringStream, syntaxTree } from "@codemirror/language";
import { type ChangeSpec, EditorSelection, type EditorState, type Line, type TransactionSpec } from "@codemirror/state";
import { codePointBefore, modeAt, typstParser, type TypstState, wordy } from "./typst.ts";

export type InlineFormat = "bold" | "italic" | "underline" | "strike" | "super" | "sub";

export const INLINE_FORMATS: readonly InlineFormat[] = ["bold", "italic", "underline", "strike", "super", "sub"];

/** Each format's delimiter (bold and italic) and function. */
const FORMS: Record<InlineFormat, { delim?: string; fn: string }> = {
  bold: { delim: "*", fn: "strong" },
  italic: { delim: "_", fn: "emph" },
  underline: { fn: "underline" },
  strike: { fn: "strike" },
  super: { fn: "super" },
  sub: { fn: "sub" },
};

const BY_FN: Record<string, InlineFormat> = { strong: "bold", emph: "italic", underline: "underline", strike: "strike", super: "super", sub: "sub" };
const CALL = /^#(strong|emph|underline|strike|super|sub)$/;
/** A character style's call up to its `[`, the id as one plain string (see styles.ts). */
const STYLED_CALL = /#styled\("([^"\\]*)"\)\[/y;
/** Where a styled span's id starts after its `from`. */
const STYLED_ID = '#styled("'.length;

type Frame = TypstState["frames"][number];

/** An inline format, or a character style (`#styled("<id>")[…]`). */
export type SpanFormat = InlineFormat | "styled";

/** A formatted span: `from` `*` or `#name[`, content, `]` or `*` up to `to` (document positions). */
export interface Span {
  format: SpanFormat;
  /** A styled span's style id. */
  style?: string;
  from: number;
  contentFrom: number;
  contentTo: number;
  to: number;
}

/** The formatted spans closed on `line`, inner ones first (tokenized from the line start as plain markup). */
export function lineSpans(line: Line): Span[] {
  const spans: Span[] = [];
  const text = line.text;
  const state = typstParser.startState!(2);
  const stream = new StringStream(text, 4, 2);
  /** Open `*`/`_` by their markup frame. */
  const opened = new Map<Frame, { bold: number; italic: number }>();
  /** Open formatting calls by the frame of their content. */
  const calls = new Map<Frame, { format: SpanFormat; style?: string; from: number; contentFrom: number }>();
  /** A call whose `[` comes at `end` (a styled call's argument tokens come first). */
  let call: { format: SpanFormat; style?: string; from: number; end: number } | null = null;
  while (!stream.eol()) {
    stream.start = stream.pos;
    let token: string | null = null;
    for (let tries = 0; tries < 10 && stream.pos === stream.start; tries++) token = typstParser.token(stream, state);
    if (stream.pos === stream.start) break;
    const value = stream.current();
    const top = state.frames[state.frames.length - 1];
    const pending = call;
    if (!call || stream.start >= call.end) call = null;
    if (!token) continue;
    if (token.includes("mk") && (value === "*" || value === "_") && top.k === "markup") {
      const format = value === "*" ? "bold" : "italic";
      const open = opened.get(top) ?? { bold: -1, italic: -1 };
      opened.set(top, open);
      if (value === "*" ? top.strong : top.emph) open[format] = stream.start;
      else if (open[format] >= 0) {
        spans.push({ format, from: line.from + open[format], contentFrom: line.from + open[format] + 1, contentTo: line.from + stream.start, to: line.from + stream.pos });
        open[format] = -1;
      }
    } else if (token === "code") {
      const name = CALL.exec(value);
      if (name) call = { format: BY_FN[name[1]], from: stream.start, end: stream.pos };
      else if (value === "#styled") {
        STYLED_CALL.lastIndex = stream.start;
        const styled = STYLED_CALL.exec(text);
        if (styled) call = { format: "styled", style: styled[1], from: stream.start, end: STYLED_CALL.lastIndex - 1 };
      } else if (value === "[" && pending && pending.end === stream.start && top.k === "markup" && top.block) {
        calls.set(top, { format: pending.format, style: pending.style, from: pending.from, contentFrom: stream.pos });
      } else if (value === "]") {
        for (const [frame, c] of calls) {
          if (state.frames.includes(frame)) continue;
          spans.push({ format: c.format, style: c.style, from: line.from + c.from, contentFrom: line.from + c.contentFrom, contentTo: line.from + stream.start, to: line.from + stream.pos });
          calls.delete(frame);
        }
      }
    }
  }
  return spans;
}

/** Whether `pos` is in markup (not math, raw text, code or a comment). */
const inMarkup = (state: EditorState, pos: number) => modeAt(state, pos) === "markup";

/** Tokens a format never cuts into: code (calls and their brackets), citations, labels, math, raw text, comments. */
const WHOLE = /(?:^|_)(?:code|cite|label|math|mathOpen|mathClose|raw|comment)(?:_|$)/;

/** Whether a format may start or end at `pos`: in markup, not inside such a token (`#und|erline`, `@wo|olf`). */
function canCut(state: EditorState, pos: number): boolean {
  if (!inMarkup(state, pos)) return false;
  const tree = syntaxTree(state);
  const before = tree.resolveInner(pos, -1);
  const after = tree.resolveInner(pos, 1);
  return !(before.to >= pos && before.from < pos && WHOLE.test(before.name) && after.from <= pos && after.to > pos && WHOLE.test(after.name));
}

/** Whether `[` and `]` (unescaped) pair up in `text`. */
function balanced(text: string): boolean {
  let depth = 0;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (c === "\\") i++;
    else if (c === "[") depth++;
    else if (c === "]" && --depth < 0) return false;
  }
  return depth === 0;
}

/** A selected part of one line, without the whitespace around it. */
export interface Segment {
  line: Line;
  from: number;
  to: number;
}

/** Where a line's heading or list marker ends (0 without one): inline formats start after it. */
function markerEnd(text: string): number {
  const p = prefixOf(text);
  return p.heading || p.list ? p.end : 0;
}

/** The non-blank parts of `from..to`, line by line (never a heading or list marker). */
export function* segments(state: EditorState, from: number, to: number): Generator<Segment> {
  const last = state.doc.lineAt(to).number;
  for (let n = state.doc.lineAt(from).number; n <= last; n++) {
    const line = state.doc.line(n);
    const b = Math.min(to, line.to) - line.from;
    const a = Math.max(Math.max(from, line.from) - line.from, markerEnd(line.text));
    const part = line.text.slice(a, b);
    const start = part.length - part.trimStart().length;
    const end = part.trimEnd().length;
    if (end > start) yield { line, from: line.from + a + start, to: line.from + a + end };
  }
}

/** The innermost span of `format` holding the segment (a cursor: in its content). */
function enclosing(spans: readonly Span[], format: SpanFormat, from: number, to: number): Span | undefined {
  // Inner spans come first.
  return spans.find((s) => s.format === format && (from < to ? s.from <= from && to <= s.to : s.contentFrom <= from && from <= s.contentTo));
}

/** Removes a span's delimiters. */
const unwrap = (s: Span): { from: number; to: number }[] => [
  { from: s.from, to: s.contentFrom },
  { from: s.contentTo, to: s.to },
];

/**
 * Wraps `from..to` on `line` in `format`: as a call (`#strong[…]`) where a
 * letter or digit touches it outside (Typst reads `*`/`_` inside a word as
 * text) or the `content` (as it will be) holds the delimiter, else with the
 * delimiter.
 */
function wrap(line: Line, format: InlineFormat, from: number, to: number, content = line.text.slice(from - line.from, to - line.from)): { from: number; insert: string }[] {
  const { delim, fn } = FORMS[format];
  const text = line.text;
  const call = !delim || wordy(codePointBefore(text, from - line.from)) || wordy(text.codePointAt(to - line.from)) || content.includes(delim);
  return [
    { from, insert: call ? `#${fn}[` : delim },
    { from: to, insert: call ? "]" : delim },
  ];
}

/** The selection after `changes`, kept on the same text (inserted delimiters stay outside). */
function keepSelection(state: EditorState, changes: ChangeSpec[]): TransactionSpec {
  const set = state.changes(changes);
  const { anchor, head, empty } = state.selection.main;
  const forward = anchor <= head;
  return {
    changes: set,
    selection: empty ? EditorSelection.cursor(set.mapPos(head)) : EditorSelection.single(set.mapPos(anchor, forward ? 1 : -1), set.mapPos(head, forward ? -1 : 1)),
    scrollIntoView: true,
    userEvent: "input",
  };
}

/**
 * Toggles `format` at the main selection, as Word does. A selection all
 * inside spans of it loses them (the enclosing spans' delimiters go);
 * otherwise each line's selected part is wrapped (spans of it touching the
 * part merge into one). A cursor in a span removes the span, in a word
 * formats the word, elsewhere inserts an empty pair to type into. Nothing
 * in math, raw text or code.
 */
export function toggleInline(state: EditorState, format: InlineFormat): TransactionSpec {
  const range = state.selection.main;
  if (!canCut(state, range.from) || !canCut(state, range.to)) return {};
  if (range.empty) return atCursor(state, format, range.head);
  const parts: { seg: Segment; spans: Span[]; covered: Span | undefined }[] = [];
  for (const seg of segments(state, range.from, range.to)) {
    // Lines of a multi-line formula or code in between are left alone.
    if (!canCut(state, seg.from) || !canCut(state, seg.to)) continue;
    const spans = lineSpans(seg.line);
    parts.push({ seg, spans, covered: enclosing(spans, format, seg.from, seg.to) });
  }
  if (!parts.length) return {};
  const changes: ChangeSpec[] = [];
  if (parts.every((p) => p.covered)) {
    const done = new Set<number>();
    for (const { covered } of parts) {
      if (done.has(covered!.from)) continue;
      done.add(covered!.from);
      changes.push(...unwrap(covered!));
    }
    return keepSelection(state, changes);
  }
  for (const { seg, spans, covered } of parts) {
    if (covered) continue;
    const own = spans.filter((s) => s.format === format);
    let from = seg.from;
    let to = seg.to;
    // Spans overlapping the part become part of it.
    for (let grown = true; grown; ) {
      grown = false;
      for (const s of own) {
        if (s.from < to && s.to > from && (s.from < from || s.to > to)) {
          from = Math.min(from, s.from);
          to = Math.max(to, s.to);
          grown = true;
        }
      }
    }
    // A part crossing another format's span, or a call around unbalanced brackets, would break the markup.
    if (spans.some((s) => s.from < to && from < s.to && !(s.from <= from && to <= s.to) && !(from <= s.from && s.to <= to))) continue;
    // Spans inside it go (their delimiters).
    const gone = own.filter((s) => from <= s.from && s.to <= to).flatMap(unwrap);
    gone.sort((x, y) => x.from - y.from);
    let content = "";
    let at = from;
    for (const g of gone) {
      content += state.sliceDoc(at, g.from);
      at = g.to;
    }
    content += state.sliceDoc(at, to);
    const delims = wrap(seg.line, format, from, to, content);
    if (delims[1].insert === "]" && !balanced(content)) continue;
    changes.push(...gone, ...delims);
  }
  return changes.length ? keepSelection(state, changes) : {};
}

/** The word (letters and digits) `pos` is inside of, not at an end of; null between words. */
function wordAround(line: Line, pos: number): { from: number; to: number } | null {
  const text = line.text;
  let a = pos - line.from;
  let b = a;
  for (let cp = codePointBefore(text, a); wordy(cp); cp = codePointBefore(text, a)) a -= cp! > 0xffff ? 2 : 1;
  for (let cp = text.codePointAt(b); wordy(cp); cp = text.codePointAt(b)) b += cp! > 0xffff ? 2 : 1;
  return a < pos - line.from && pos - line.from < b ? { from: line.from + a, to: line.from + b } : null;
}

function atCursor(state: EditorState, format: InlineFormat, pos: number): TransactionSpec {
  const line = state.doc.lineAt(pos);
  // In a heading or list marker: after it.
  pos = Math.max(pos, line.from + markerEnd(line.text));
  const span = enclosing(lineSpans(line), format, pos, pos);
  if (span) return keepSelection(state, unwrap(span));
  const word = wordAround(line, pos);
  if (word) return keepSelection(state, wrap(line, format, word.from, word.to));
  // An empty pair (a call next to a letter or digit).
  const [open, close] = wrap(line, format, pos, pos);
  return {
    changes: { from: pos, insert: open.insert + close.insert },
    selection: EditorSelection.cursor(pos + open.insert.length),
    scrollIntoView: true,
    userEvent: "input",
  };
}

/** The inline formats holding the whole main selection (a cursor: in their content), for pressed buttons. */
export function activeFormats(state: EditorState): Set<InlineFormat> {
  const range = state.selection.main;
  const active = new Set<InlineFormat>();
  if (range.empty) {
    const spans = lineSpans(state.doc.lineAt(range.head));
    for (const f of INLINE_FORMATS) if (enclosing(spans, f, range.head, range.head)) active.add(f);
    return active;
  }
  let formats: readonly InlineFormat[] | null = null;
  for (const seg of segments(state, range.from, range.to)) {
    const spans = lineSpans(seg.line);
    const here: InlineFormat[] = (formats ?? INLINE_FORMATS).filter((f) => enclosing(spans, f, seg.from, seg.to));
    if (formats && here.length === formats.length) continue;
    // Lines of a multi-line formula or code don't count (as for toggling).
    if (!inMarkup(state, seg.from) || !inMarkup(state, seg.to)) continue;
    formats = here;
    if (!formats.length) break;
  }
  return new Set(formats ?? []);
}

// ---- Character styles (`#styled("<id>")[…]`; the styles are in the layout layer, see styles.ts) ----

/** A styled span's opening, up to its `[`. */
const styledOpen = (id: string) => `#styled(${JSON.stringify(id)})[`;

/** Gives a styled span another style (its id replaced), or unwraps it (null). */
function repoint(span: Span, id: string | null): ChangeSpec[] {
  return id === null ? unwrap(span) : [{ from: span.from + STYLED_ID, to: span.contentFrom - '")['.length, insert: id }];
}

/**
 * Gives the main selection's text another character style, as Word does
 * with fonts: `idFor(current)` gets the style of the styled span the text
 * is in (null: none) and answers the one it should have (null: default
 * text). Per line, a selected part covering a span's content (but for
 * whitespace) re-points or unwraps the span; a part strictly inside one gets
 * a span of its own inside it (default text: the span is split around it);
 * else the part is wrapped in `#styled("<id>")[…]` (spans inside it stay:
 * Typst applies the inner ones). A cursor in a span re-points or unwraps
 * it, in a word styles the word, elsewhere inserts an empty span to type
 * into. Nothing in math, raw text or code, nor across another format's span.
 */
export function restyle(state: EditorState, idFor: (current: string | null) => string | null): TransactionSpec {
  const range = state.selection.main;
  if (!canCut(state, range.from) || !canCut(state, range.to)) return {};
  if (range.empty) {
    const line = state.doc.lineAt(range.head);
    const pos = Math.max(range.head, line.from + markerEnd(line.text));
    const span = enclosing(lineSpans(line), "styled", pos, pos);
    if (span) {
      const id = idFor(span.style!);
      return id === span.style ? {} : keepSelection(state, repoint(span, id));
    }
    const id = idFor(null);
    if (id === null) return {};
    const word = wordAround(line, pos);
    if (word) return keepSelection(state, [{ from: word.from, insert: styledOpen(id) }, { from: word.to, insert: "]" }]);
    const open = styledOpen(id);
    return { changes: { from: pos, insert: `${open}]` }, selection: EditorSelection.cursor(pos + open.length), scrollIntoView: true, userEvent: "input" };
  }
  /** Whether `a..b` holds only whitespace. */
  const blank = (a: number, b: number) => a >= b || !state.sliceDoc(a, b).trim();
  const changes: ChangeSpec[] = [];
  for (const seg of segments(state, range.from, range.to)) {
    // Lines of a multi-line formula or code in between are left alone.
    if (!canCut(state, seg.from) || !canCut(state, seg.to)) continue;
    const spans = lineSpans(seg.line);
    const span = enclosing(spans, "styled", seg.from, seg.to);
    const current = span ? span.style! : null;
    const id = idFor(current);
    if (id === current) continue;
    if (span && blank(span.contentFrom, seg.from) && blank(seg.to, span.contentTo)) {
      changes.push(...repoint(span, id));
      continue;
    }
    // A part crossing another format's span, or around unbalanced brackets, would break the markup.
    if (spans.some((s) => s.from < seg.to && seg.from < s.to && !(s.from <= seg.from && seg.to <= s.to) && !(seg.from <= s.from && s.to <= seg.to))) continue;
    if (!balanced(state.sliceDoc(seg.from, seg.to))) continue;
    if (id !== null) {
      changes.push({ from: seg.from, insert: styledOpen(id) }, { from: seg.to, insert: "]" });
    } else if (span) {
      // Split around the part: the span ends before it and starts again after it.
      changes.push(blank(span.contentFrom, seg.from) ? { from: span.from, to: span.contentFrom } : { from: seg.from, insert: "]" });
      changes.push(blank(seg.to, span.contentTo) ? { from: span.contentTo, to: span.to } : { from: seg.to, insert: state.sliceDoc(span.from, span.contentFrom) });
    }
  }
  return changes.length ? keepSelection(state, changes) : {};
}

// ---- Paragraph kinds ----

/** Indentation, then a heading (`== `) or list marker (`- `, `+ `, `1. `) with the spaces after it. */
const PREFIX = /^([ \t]*)(?:(=+)|([-+]|\d+\.))(?=[ \t]|$)[ \t]*/;
const INDENT = /^[ \t]*/;
const BLANK = /^\s*$/;

export type ListKind = "bullet" | "numbered";

interface Prefix {
  /** Heading level, 0 if none. */
  heading: number;
  list: ListKind | null;
  /** Length of the indentation, and of the whole prefix. */
  indent: number;
  end: number;
}

function prefixOf(text: string): Prefix {
  const m = PREFIX.exec(text);
  if (!m) return { heading: 0, list: null, indent: INDENT.exec(text)![0].length, end: INDENT.exec(text)![0].length };
  return { heading: m[2]?.length ?? 0, list: m[3] ? (m[3] === "-" ? "bullet" : "numbered") : null, indent: m[1].length, end: m[0].length };
}

/** The non-blank lines the main selection touches (not one it only ends at the start of). */
function* touchedLines(state: EditorState): Generator<Line> {
  const { from, to } = state.selection.main;
  const { doc } = state;
  const first = doc.lineAt(from).number;
  let last = doc.lineAt(to).number;
  if (to > from && last > first && doc.line(last).from === to) last--;
  for (let n = first; n <= last; n++) {
    const line = doc.line(n);
    if (!BLANK.test(line.text)) yield line;
  }
}

/** The touched lines in markup (not in a formula, raw block or code). */
const markupLines = (state: EditorState) => [...touchedLines(state)].filter((l) => inMarkup(state, l.from));

/** The selection after line-start changes: positions at a change move past what it inserts. */
function lineEdit(state: EditorState, changes: ChangeSpec[]): TransactionSpec {
  if (!changes.length) return {};
  const set = state.changes(changes);
  const { anchor, head } = state.selection.main;
  return { changes: set, selection: EditorSelection.single(set.mapPos(anchor, 1), set.mapPos(head, 1)), scrollIntoView: true, userEvent: "input" };
}

/**
 * Makes the touched lines headings of `level` (1–6, `= ` … `====== `), or
 * plain paragraphs (0: the heading marker goes). A list item's marker is
 * replaced by the heading's.
 */
export function setHeading(state: EditorState, level: number): TransactionSpec {
  const changes: ChangeSpec[] = [];
  for (const line of markupLines(state)) {
    const p = prefixOf(line.text);
    if (p.heading === level) continue;
    if (level === 0) {
      if (p.heading) changes.push({ from: line.from, to: line.from + p.end });
    } else {
      changes.push({ from: line.from, to: line.from + (p.heading || p.list ? p.end : p.indent), insert: `${"=".repeat(level)} ` });
    }
  }
  return lineEdit(state, changes);
}

/**
 * Bulleted (`- `) or numbered (`+ `) list on the touched lines: removed if
 * every one has it already, else set (replacing a heading or the other
 * list's marker), after the indentation.
 */
export function toggleList(state: EditorState, kind: ListKind): TransactionSpec {
  const lines = markupLines(state);
  const all = lines.length > 0 && lines.every((l) => prefixOf(l.text).list === kind);
  const marker = kind === "bullet" ? "- " : "+ ";
  const changes: ChangeSpec[] = [];
  for (const line of lines) {
    const p = prefixOf(line.text);
    if (all) changes.push({ from: line.from + p.indent, to: line.from + p.end });
    else if (p.list !== kind) changes.push({ from: line.from + (p.heading ? 0 : p.indent), to: line.from + (p.heading || p.list ? p.end : p.indent), insert: marker });
  }
  return lineEdit(state, changes);
}

export interface ParagraphKind {
  /** The heading level of every touched line (0: none), null if they differ. */
  heading: number | null;
  /** Every touched line is a bulleted / numbered list item. */
  bullet: boolean;
  numbered: boolean;
}

/** What the touched lines are (for the heading box and the list buttons). */
export function paragraphKind(state: EditorState): ParagraphKind {
  let kind: ParagraphKind | null = null;
  for (const line of touchedLines(state)) {
    const p = prefixOf(line.text);
    if (!kind) kind = { heading: p.heading, bullet: p.list === "bullet", numbered: p.list === "numbered" };
    if (kind.heading !== p.heading) kind.heading = null;
    if (p.list !== "bullet") kind.bullet = false;
    if (p.list !== "numbered") kind.numbered = false;
    // Settled (a long selection is not read to its end).
    if (kind.heading === null && !kind.bullet && !kind.numbered) break;
  }
  return kind ?? { heading: 0, bullet: false, numbered: false };
}
