// Softened Typst markup (a subset) as a CodeMirror stream language: decorations
// only, the text is never changed. The tokenizer keeps a stack of modes
// (markup, code, math, raw, block comment), so constructs may span lines.
// StreamLanguage parses lazily up to the viewport, so book-size documents stay
// fast.
//
// Strong/emphasis stay line-bounded (an opener needs its closer on the same
// line, checked by a look-ahead), so a stray `*` never restyles a paragraph.
// The content of `#underline[…]`, `#strike[…]`, `#super[…]`, `#sub[…]`,
// `#strong[…]` and `#emph[…]` is styled likewise when its `]` is on the line;
// a character style's `#styled("<id>")[…]` keeps the styles around it (its
// font and size are decorations, see styles.ts).

import { getIndentation, HighlightStyle, StreamLanguage, type StreamParser, StringStream, syntaxHighlighting, syntaxTree } from "@codemirror/language";
import { type EditorState, type Line, type Range } from "@codemirror/state";
import { Decoration, type DecorationSet, type EditorView, ViewPlugin, type ViewUpdate } from "@codemirror/view";
import { Tag, tags } from "@lezer/highlight";

// ---- Token kinds ----

const mathTag = Tag.define();
const mathOpenTag = Tag.define();
const mathCloseTag = Tag.define();
const citeTag = Tag.define();
const codeTag = Tag.define();
const underlineTag = Tag.define();
const superTag = Tag.define();
const subTag = Tag.define();

/** Token names (a token is one or more of these, space-separated). */
const tokenTable = {
  mk: tags.processingInstruction,
  strong: tags.strong,
  emph: tags.emphasis,
  underline: underlineTag,
  strike: tags.strikethrough,
  super: superTag,
  sub: subTag,
  raw: tags.monospace,
  math: mathTag,
  mathOpen: mathOpenTag,
  mathClose: mathCloseTag,
  cite: citeTag,
  label: tags.labelName,
  comment: tags.comment,
  code: codeTag,
  h1: tags.heading1,
  h2: tags.heading2,
  h3: tags.heading3,
  h4: tags.heading4,
  h5: tags.heading5,
  h6: tags.heading6,
};

export type TokenKind = keyof typeof tokenTable;

// ---- Tokenizer state ----

interface MarkupFrame {
  k: "markup";
  /** A content block `[...]` (closed by an unbalanced `]`), not the document. */
  block: boolean;
  /** Plain `[` opened inside the block. */
  depth: number;
  /** Token for the closing `]`. */
  closeTag: string;
  strong: boolean;
  emph: boolean;
  term: boolean;
  /** The previous token was a citation (a `[` now opens its supplement). */
  cited: boolean;
  /** Inline style tags of a formatting function's content (`#underline[…]`), with the ones around it. */
  style: string;
}

interface CodeFrame {
  k: "code";
  /** A `#let`/`#set`/... statement: ends at the end of the line (brackets balanced). */
  line: boolean;
  /** Open `(` and `{`. */
  stack: string;
  /** The style tag a content block of this call gets (`#underline` → "underline"; "-": only the ones around it), or "". */
  fn: string;
}

interface MathFrame {
  k: "math";
  pre: string;
}

interface RawFrame {
  k: "raw";
  ticks: number;
  pre: string;
}

interface CommentFrame {
  k: "comment";
  depth: number;
  pre: string;
}

type Frame = MarkupFrame | CodeFrame | MathFrame | RawFrame | CommentFrame;

export interface TypstState {
  frames: Frame[];
}

const markupFrame = (block: boolean, closeTag = "", style = ""): MarkupFrame => ({
  k: "markup",
  block,
  depth: 0,
  closeTag,
  strong: false,
  emph: false,
  term: false,
  cited: false,
  style,
});

/** Formatting functions and the style tag of their content (a character style's `#styled` adds none, see styles.ts). */
const STYLED: Record<string, string> = { underline: "underline", strike: "strike", super: "super", sub: "sub", strong: "strong", emph: "emph", styled: "-" };

// ---- Character helpers ----

const BACKSLASH = 92;
const SLASH = 47;
const BACKTICK = 96;
const DOLLAR = 36;
const STAR = 42;
const UNDERSCORE = 95;
const QUOTE = 34;
const LBRACKET = 91;
const RBRACKET = 93;
const AT = 64;
const LT = 60;
const GT = 62;

const isHigh = (c: number) => c >= 0xd800 && c < 0xdc00;
const LETTER_OR_DIGIT = /[\p{L}\p{N}]/u;

export function wordy(cp: number | undefined): boolean {
  if (cp === undefined) return false;
  if (cp < 128) return (cp >= 48 && cp <= 57) || (cp >= 65 && cp <= 90) || (cp >= 97 && cp <= 122);
  return LETTER_OR_DIGIT.test(String.fromCodePoint(cp));
}

/** The code point ending just before `i`. */
export function codePointBefore(text: string, i: number): number | undefined {
  if (i <= 0) return undefined;
  const c = text.charCodeAt(i - 1);
  if (c >= 0xdc00 && c < 0xe000 && i >= 2 && isHigh(text.charCodeAt(i - 2))) return text.codePointAt(i - 2);
  return c;
}

/** Typst treats `*` and `_` between two letters/digits as plain text (`snake_case`). */
const inWord = (text: string, i: number) => wordy(codePointBefore(text, i)) && wordy(text.codePointAt(i + 1));

/** Characters of a label or reference key. */
function labelChar(cp: number): boolean {
  return wordy(cp) || cp === UNDERSCORE || cp === 45 /* - */ || cp === 46 /* . */ || cp === 58 /* : */;
}

/** End of a run of label characters starting at `i`. */
function labelEnd(text: string, i: number): number {
  while (i < text.length) {
    const cp = text.codePointAt(i)!;
    if (!labelChar(cp)) break;
    i += cp > 0xffff ? 2 : 1;
  }
  return i;
}

/** Position after an escape (`\` plus one code point, or just `\` at the end). */
const afterEscape = (text: string, i: number) => Math.min(text.length, i + (isHigh(text.charCodeAt(i + 1)) ? 3 : 2));

/** `//` starts a comment unless it is part of `http://` / `https://`. */
const isLineComment = (text: string, i: number) =>
  text.charCodeAt(i) === SLASH && text.charCodeAt(i + 1) === SLASH && !/https?:$/.test(text.slice(Math.max(0, i - 6), i));

const isBlockComment = (text: string, i: number) => text.charCodeAt(i) === SLASH && text.charCodeAt(i + 1) === STAR;

function backtickRun(text: string, i: number): number {
  let n = 0;
  while (i + n < text.length && text.charCodeAt(i + n) === BACKTICK) n++;
  return n;
}

/** End of a raw span opened by the backtick run at `i`, or -1 if not closed on this line. */
function rawEnd(text: string, i: number): number {
  const n = backtickRun(text, i);
  if (n === 2) return i + 2; // `` is empty raw
  for (let j = i + n; j < text.length; ) {
    if (text.charCodeAt(j) !== BACKTICK) {
      j++;
      continue;
    }
    const run = backtickRun(text, j);
    if (run === n) return j + n;
    j += run;
  }
  return -1;
}

/** End of a string starting at the quote `i` (escapes skipped); the line end if unclosed. */
function stringEnd(text: string, i: number): number {
  for (let j = i + 1; j < text.length; ) {
    const c = text.charCodeAt(j);
    if (c === BACKSLASH) j = afterEscape(text, j);
    else if (c === QUOTE) return j + 1;
    else j++;
  }
  return text.length;
}

/** End of an equation opened at `i`, or -1 if not closed on this line. */
function mathEnd(text: string, i: number): number {
  for (let j = i + 1; j < text.length; ) {
    const c = text.charCodeAt(j);
    if (c === BACKSLASH) j = afterEscape(text, j);
    else if (c === QUOTE) j = stringEnd(text, j);
    else if (c === DOLLAR) return j + 1;
    else if (isLineComment(text, j) || isBlockComment(text, j)) return -1;
    else j++;
  }
  return -1;
}

/**
 * Whether the strong/emphasis opener before `start` closes on this line: the
 * next `delim` outside escapes, raw and math, before a comment, before the
 * enclosing delimiter `stop` (if one is open) and before the end of the
 * content block.
 */
function closes(text: string, start: number, delim: number, stop: number, inBlock: boolean): boolean {
  let depth = 0;
  for (let i = start; i < text.length; ) {
    const c = text.charCodeAt(i);
    if (c === BACKSLASH) {
      i = afterEscape(text, i);
    } else if (c === BACKTICK) {
      i = rawEnd(text, i);
      if (i < 0) return false;
    } else if (c === DOLLAR) {
      i = mathEnd(text, i);
      if (i < 0) return false;
    } else if (isLineComment(text, i) || isBlockComment(text, i)) {
      return false;
    } else if (c === LBRACKET) {
      depth++;
      i++;
    } else if (c === RBRACKET) {
      if (depth === 0 && inBlock) return false;
      depth = Math.max(0, depth - 1);
      i++;
    } else if ((c === delim || c === stop) && !inWord(text, i)) {
      return c === delim;
    } else {
      i++;
    }
  }
  return false;
}

/**
 * Whether the content block opened before `start` closes on this line: an
 * unbalanced `]` outside escapes, raw and math, before a comment.
 */
function blockCloses(text: string, start: number): boolean {
  let depth = 0;
  for (let i = start; i < text.length; ) {
    const c = text.charCodeAt(i);
    if (c === BACKSLASH) {
      i = afterEscape(text, i);
    } else if (c === BACKTICK) {
      i = rawEnd(text, i);
      if (i < 0) return false;
    } else if (c === DOLLAR) {
      i = mathEnd(text, i);
      if (i < 0) return false;
    } else if (isLineComment(text, i) || isBlockComment(text, i)) {
      return false;
    } else {
      if (c === LBRACKET) depth++;
      else if (c === RBRACKET && depth-- === 0) return true;
      i++;
    }
  }
  return false;
}

/** Matches a sticky regexp at the stream position and consumes it (no slicing of the line). */
function eat(stream: StringStream, re: RegExp): boolean {
  re.lastIndex = stream.pos;
  if (!re.test(stream.string)) return false;
  stream.pos = re.lastIndex;
  return true;
}

const PLAIN = /[^\\/`$*_@<#[\]:]+/y;
const HEADING = /(=+) /y;
const LIST = /(?:[-+]|\d+\.)(?=[ \t]|$)/y;
const TERM = /\/ /y;
const INDENT = /[ \t]*/y;
const IDENT_START = /[\p{ID_Start}_]/uy;
const IDENT = /[\p{ID_Start}_](?:\p{ID_Continue}|-(?=\p{ID_Continue}))*/uy;
const KEYWORD = /(?:let|set|show|import|include|if|for|while|context|return)(?!\p{ID_Continue}|-)/uy;
const CODE_PLAIN = /[^\\/"`$[\](){}]+/y;
const MATH_PLAIN = /[^$\\"/]+/y;
const RAW_PLAIN = /[^`]+/y;
const COMMENT_PLAIN = /[^*/]+/y;

const at = (re: RegExp, text: string, i: number) => {
  re.lastIndex = i;
  return re.test(text);
};

/** `base` plus the inline style tags (`pre`) in effect. */
const tok = (base: string, pre: string) => (pre ? `${base} ${pre}` : base);

function inlineStyle(f: MarkupFrame): string {
  const parts: string[] = [];
  if (f.strong || f.term) parts.push("strong");
  if (f.emph) parts.push("emph");
  if (f.style) for (const p of f.style.split(" ")) if (!parts.includes(p)) parts.push(p);
  return parts.join(" ");
}

const top = (s: TypstState) => s.frames[s.frames.length - 1];

function pop(s: TypstState): void {
  if (s.frames.length > 1) s.frames.pop();
}

/** Starts the frames opened by `$`, a backtick run or `/*` (shared by markup and code). */
function openNested(stream: StringStream, s: TypstState, pre: string): string | null {
  const text = stream.string;
  const i = stream.pos;
  const c = text.charCodeAt(i);
  if (c === DOLLAR) {
    stream.pos++;
    s.frames.push({ k: "math", pre });
    return tok("mathOpen", pre);
  }
  if (c === BACKTICK) {
    const n = backtickRun(text, i);
    stream.pos += n;
    if (n === 2) return tok("raw", pre);
    if (n >= 3) eat(stream, IDENT); // language tag
    s.frames.push({ k: "raw", ticks: n, pre });
    return tok("raw", pre);
  }
  if (isBlockComment(text, i)) {
    stream.pos += 2;
    s.frames.push({ k: "comment", depth: 1, pre: "" });
    return "comment";
  }
  return null;
}

function markupToken(stream: StringStream, s: TypstState, f: MarkupFrame): string | null {
  const text = stream.string;
  if (stream.pos === 0) {
    f.strong = f.emph = f.term = f.cited = false;
    HEADING.lastIndex = 0;
    const heading = HEADING.exec(text);
    if (heading) {
      stream.pos = heading[1].length;
      return `h${Math.min(heading[1].length, 6)} mk`;
    }
    if (eat(stream, INDENT)) {
      if (stream.pos > 0) return null; // list markers may follow the indentation
    }
  }
  const p = stream.pos;
  if (p === 0 || (p < 256 && (text[p - 1] === " " || text[p - 1] === "\t") && /^[ \t]*$/.test(text.slice(0, p)))) {
    if (eat(stream, LIST)) return "mk";
    if (at(TERM, text, stream.pos)) {
      stream.pos++;
      f.term = true;
      return "mk";
    }
  }

  const cited = f.cited;
  f.cited = false;
  const pre = inlineStyle(f);
  const plain = pre || null;
  const i = stream.pos;
  const c = text.charCodeAt(i);

  if (eat(stream, PLAIN)) return plain;

  if (c === BACKSLASH) {
    // A forced line break (`\` before whitespace or the line end) is markup.
    const next = text.charCodeAt(i + 1);
    if (Number.isNaN(next) || next === 32 || next === 9) {
      stream.pos++;
      return tok("mk", pre);
    }
    stream.pos = afterEscape(text, i);
    return plain;
  }
  if (isLineComment(text, i)) {
    stream.skipToEnd();
    return "comment";
  }
  const nested = openNested(stream, s, pre);
  if (nested) return nested;

  if (c === STAR || c === UNDERSCORE) {
    stream.pos++;
    if (inWord(text, i)) return plain;
    const open = c === STAR ? f.strong : f.emph;
    if (open) {
      if (c === STAR) f.strong = false;
      else f.emph = false;
      return tok("mk", pre);
    }
    const other = c === STAR ? (f.emph ? UNDERSCORE : -1) : f.strong ? STAR : -1;
    if (!closes(text, i + 1, c, other, f.block)) return plain;
    if (c === STAR) f.strong = true;
    else f.emph = true;
    return tok("mk", inlineStyle(f));
  }
  if (c === AT) {
    stream.pos++;
    if (wordy(codePointBefore(text, i))) return plain;
    let end = labelEnd(text, i + 1);
    // A trailing `.` or `:` ends the sentence, not the key.
    while (end > i + 1 && (text.charCodeAt(end - 1) === 46 || text.charCodeAt(end - 1) === 58)) end--;
    if (end === i + 1) return plain;
    stream.pos = end;
    f.cited = true;
    return tok("cite", pre);
  }
  if (c === LT) {
    stream.pos++;
    const end = labelEnd(text, i + 1);
    if (end > i + 1 && text.charCodeAt(end) === GT) {
      stream.pos = end + 1;
      return "label";
    }
    return plain;
  }
  if (c === 35 /* # */) {
    const next = text.charCodeAt(i + 1);
    if (at(IDENT_START, text, i + 1)) {
      stream.pos++;
      const line = eat(stream, KEYWORD);
      if (!line) eat(stream, IDENT);
      const fn = line ? "" : (STYLED[text.slice(i + 1, stream.pos)] ?? "");
      s.frames.push({ k: "code", line, stack: "", fn });
      return "code";
    }
    if (next === 40 /* ( */ || next === 123 /* { */ || next === LBRACKET) {
      stream.pos++;
      s.frames.push({ k: "code", line: false, stack: "", fn: "" });
      return "code";
    }
    stream.pos++;
    return plain;
  }
  if (c === LBRACKET) {
    stream.pos++;
    if (cited) {
      s.frames.push(markupFrame(true, "mk"));
      return "mk";
    }
    if (f.block) f.depth++;
    return plain;
  }
  if (c === RBRACKET) {
    stream.pos++;
    if (!f.block) return plain;
    if (f.depth > 0) {
      f.depth--;
      return plain;
    }
    pop(s);
    return f.closeTag;
  }
  if (c === 58 /* : */ && f.term) {
    stream.pos++;
    f.term = false;
    return tok("mk", inlineStyle(f));
  }
  stream.pos++;
  return plain;
}

const OPEN = "({";
const CLOSE = ")}";

function codeToken(stream: StringStream, s: TypstState, f: CodeFrame): string | null {
  const text = stream.string;
  const i = stream.pos;
  const c = text.charCodeAt(i);
  if (f.stack === "") {
    if (stream.sol()) {
      pop(s); // an expression or statement ends with its line
      return null;
    }
    if (!f.line) {
      // Embedded expression: continues with calls, content blocks and field access only.
      if (c === 46 /* . */ && at(IDENT_START, text, i + 1)) {
        stream.pos++;
        eat(stream, IDENT);
        f.fn = "";
        return "code";
      }
      if (c !== 40 && c !== 123 && c !== LBRACKET) {
        pop(s);
        if (c === 59 /* ; */) {
          stream.pos++;
          return "code";
        }
        return null;
      }
    }
  }
  if (isLineComment(text, i)) {
    stream.skipToEnd();
    return "comment";
  }
  const nested = openNested(stream, s, "");
  if (nested) return nested;
  stream.pos++;
  const open = OPEN.indexOf(text[i]);
  if (open >= 0) {
    f.stack += text[i];
  } else if (CLOSE.indexOf(text[i]) >= 0) {
    f.stack = f.stack.slice(0, -1);
  } else if (c === LBRACKET) {
    // A formatting call's content: styled (with the styles around the call) when it closes on this line.
    let style = "";
    if (f.fn && f.stack === "" && blockCloses(text, i + 1)) {
      const outer = s.frames[s.frames.length - 2];
      const around = outer?.k === "markup" ? inlineStyle(outer) : "";
      const own = f.fn === "-" ? "" : f.fn;
      style = around && own ? `${around} ${own}` : around || own;
    }
    s.frames.push(markupFrame(true, "code", style));
  } else if (c === QUOTE) {
    stream.pos = stringEnd(text, i);
  } else if (c === BACKSLASH) {
    stream.pos = afterEscape(text, i);
  } else {
    eat(stream, CODE_PLAIN);
  }
  return "code";
}

function mathToken(stream: StringStream, s: TypstState, f: MathFrame): string {
  const text = stream.string;
  const i = stream.pos;
  const c = text.charCodeAt(i);
  const math = tok("math", f.pre);
  if (c === DOLLAR) {
    stream.pos++;
    pop(s);
    return tok("mathClose", f.pre);
  }
  if (c === BACKSLASH) {
    stream.pos = afterEscape(text, i);
  } else if (c === QUOTE) {
    stream.pos = stringEnd(text, i);
  } else if (isLineComment(text, i)) {
    stream.skipToEnd();
    return "comment math";
  } else if (isBlockComment(text, i)) {
    stream.pos += 2;
    s.frames.push({ k: "comment", depth: 1, pre: "math" });
    return "comment math";
  } else if (!eat(stream, MATH_PLAIN)) {
    stream.pos++;
  }
  return math;
}

function rawToken(stream: StringStream, s: TypstState, f: RawFrame): string {
  const raw = tok("raw", f.pre);
  if (eat(stream, RAW_PLAIN)) return raw;
  const n = backtickRun(stream.string, stream.pos);
  stream.pos += n;
  if (n === f.ticks) pop(s);
  return raw;
}

function commentToken(stream: StringStream, s: TypstState, f: CommentFrame): string {
  const comment = tok("comment", f.pre);
  if (eat(stream, COMMENT_PLAIN)) return comment;
  const text = stream.string;
  const i = stream.pos;
  if (isBlockComment(text, i)) {
    stream.pos += 2;
    f.depth++;
  } else if (text.charCodeAt(i) === STAR && text.charCodeAt(i + 1) === SLASH) {
    stream.pos += 2;
    if (--f.depth === 0) pop(s);
  } else {
    stream.pos++;
  }
  return comment;
}

export const typstParser: StreamParser<TypstState> = {
  name: "typst",
  startState: () => ({ frames: [markupFrame(false)] }),
  copyState: (s) => ({ frames: s.frames.map((f) => ({ ...f })) }),
  token(stream, s) {
    const f = top(s);
    switch (f.k) {
      case "markup":
        return markupToken(stream, s, f);
      case "code":
        return codeToken(stream, s, f);
      case "math":
        return mathToken(stream, s, f);
      case "raw":
        return rawToken(stream, s, f);
      case "comment":
        return commentToken(stream, s, f);
    }
  },
  blankLine(s) {
    for (let f = top(s); f.k === "code" && f.stack === "" && s.frames.length > 1; f = top(s)) pop(s);
    const f = top(s);
    if (f.k === "markup") f.strong = f.emph = f.term = f.cited = false;
  },
  // CodeMirror computes indentation from the state at the position asked
  // for; `modeAt` reads it here. `null` keeps the line's own indentation.
  indent(s) {
    probed = s;
    return null;
  },
  tokenTable,
};

export const typstLanguage = StreamLanguage.define(typstParser);

// ---- Mode at a position ----

let probed: TypstState | null = null;

export type Mode = "markup" | "math" | "code" | "raw";

/**
 * The mode at `pos`: markup, math, code (comments count as code) or raw
 * text, from the tokenizer state there (CodeMirror resumes from its nearest
 * cached state). An embedded expression that a line break ends (`#name`,
 * `#f(x)` after its last bracket) counts as the markup around it. Markup if
 * the text there is not parsed yet.
 */
export function modeAt(state: EditorState, pos: number): Mode {
  const frames = framesAt(state, pos);
  for (let i = frames.length - 1; i >= 0; i--) {
    const f = frames[i];
    if (f.k === "code" && f.stack === "") continue;
    return f.k === "comment" ? "code" : f.k;
  }
  return "markup";
}

/**
 * Whether the start of the line at `pos` is in the document's own markup:
 * not inside a content block `[...]`, a call's arguments, math, raw text or
 * a comment (as `modeAt`, ended expressions don't count). True if the text
 * there is not parsed yet.
 */
export function topLevelAt(state: EditorState, pos: number): boolean {
  return framesAt(state, pos).every((f, i) => (i === 0 && f.k === "markup" && !f.block) || (f.k === "code" && f.stack === ""));
}

/** The tokenizer's frames at the start of the line at `pos` ([] if not parsed). */
function framesAt(state: EditorState, pos: number): Frame[] {
  probed = null;
  getIndentation(state, pos);
  const frames = (probed as TypstState | null)?.frames ?? [];
  probed = null;
  return frames;
}

// ---- Styling ----

const typstHighlight = HighlightStyle.define([
  { tag: tags.processingInstruction, class: "cm-mk" },
  { tag: tags.strong, class: "cm-strong" },
  { tag: tags.emphasis, class: "cm-emph" },
  { tag: underlineTag, class: "cm-underline" },
  { tag: tags.strikethrough, class: "cm-strike" },
  { tag: superTag, class: "cm-super" },
  { tag: subTag, class: "cm-sub" },
  { tag: tags.monospace, class: "cm-raw" },
  { tag: mathTag, class: "cm-math" },
  { tag: mathOpenTag, class: "cm-math cm-math-open" },
  { tag: mathCloseTag, class: "cm-math cm-math-close" },
  { tag: citeTag, class: "cm-cite" },
  { tag: tags.labelName, class: "cm-label" },
  { tag: tags.comment, class: "cm-comment" },
  { tag: codeTag, class: "cm-code" },
]);

// ---- Node kinds (for headings, spell check and math spans) ----

export const KIND = {
  quiet: 1, // no spell check (spell.ts): math, raw, code, citations, labels
  mathOpen: 2,
  mathClose: 4,
  math: 8,
} as const;

const QUIET = new Set(["math", "mathOpen", "mathClose", "raw", "code", "cite", "label"]);
const kindCache = new Map<number, { flags: number; heading: number }>();

/** Flags and heading level (0 = none) of a token node type. */
export function nodeKind(type: { id: number; name: string }): { flags: number; heading: number } {
  let k = kindCache.get(type.id);
  if (!k) {
    let flags = 0;
    let heading = 0;
    for (const part of type.name.split("_")) {
      if (QUIET.has(part)) flags |= KIND.quiet;
      if (part === "mathOpen") flags |= KIND.mathOpen;
      else if (part === "mathClose") flags |= KIND.mathClose;
      else if (part === "math") flags |= KIND.math;
      else if (/^h[1-6]$/.test(part)) heading = Number(part[1]);
    }
    k = { flags, heading };
    kindCache.set(type.id, k);
  }
  return k;
}

const headingLines = [1, 2, 3, 4, 5, 6].map((n) => Decoration.line({ class: `cm-h${n}` }));
const blankLine = Decoration.line({ class: "cm-blank" });

const BLANK = /^\s*$/;
const FORCED_BREAK = /(^|\s)\\\s*$/;
const NOT_MARKUP = new Set(["math", "mathOpen", "raw", "code", "comment"]);

/** Whether blank line `line` directly follows markup text (not a forced line break `\`, not math, raw text or code). */
function followsText(state: EditorState, line: Line): boolean {
  if (line.number === 1 || !BLANK.test(line.text)) return false;
  const prev = state.doc.line(line.number - 1);
  if (BLANK.test(prev.text) || FORCED_BREAK.test(prev.text)) return false;
  const end = prev.from + prev.text.trimEnd().length;
  const token = syntaxTree(state).resolveInner(end, -1).type.name.split("_");
  return !token.some((part) => NOT_MARKUP.has(part)) || modeAt(state, line.from) === "markup";
}

/**
 * Whether blank line `line` is a paragraph break to draw compact: it
 * follows text, or it is the break before the next paragraph and the line
 * above it is a full-height empty line (the one Enter or Shift+Enter leaves
 * for typing). Either way the typing line keeps its height and the break
 * stays compact when the first letter goes in.
 */
function isParagraphGap(state: EditorState, line: Line): boolean {
  if (followsText(state, line)) return true;
  const { doc } = state;
  if (line.number < 2 || line.number === doc.lines || !BLANK.test(line.text)) return false;
  const prev = doc.line(line.number - 1);
  return BLANK.test(prev.text) && !followsText(state, prev) && !BLANK.test(doc.line(line.number + 1).text) && modeAt(state, line.from) === "markup";
}

/**
 * Heading line classes and the compact blank-line class (see
 * `isParagraphGap`), visible ranges only.
 */
function buildExtras(view: EditorView): DecorationSet {
  const ranges: Range<Decoration>[] = [];
  const { doc } = view.state;
  let next = 0;
  for (const { from, to } of view.visibleRanges) {
    for (let pos = Math.max(from, next); pos <= to; ) {
      const line = doc.lineAt(pos);
      if (isParagraphGap(view.state, line)) ranges.push(blankLine.range(line.from));
      pos = next = line.to + 1;
    }
  }
  const tree = syntaxTree(view.state);
  for (const { from, to } of view.visibleRanges) {
    tree.iterate({
      from,
      to,
      enter(node) {
        const { heading } = nodeKind(node.type);
        if (heading) {
          const line = doc.lineAt(node.from);
          if (line.from === node.from) ranges.push(headingLines[heading - 1].range(line.from));
        }
      },
    });
  }
  return Decoration.set(ranges, true);
}

const markupExtras = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = buildExtras(view);
    }
    update(update: ViewUpdate) {
      if (update.docChanged || update.viewportChanged || syntaxTree(update.state) !== syntaxTree(update.startState)) {
        this.decorations = buildExtras(update.view);
      }
    }
  },
  { decorations: (v) => v.decorations },
);

/** Softened Typst markup: tokenizer, styles and heading lines. */
export const typstMarkup = [typstLanguage, syntaxHighlighting(typstHighlight), markupExtras];

// ---- Testing helper ----

/**
 * Tokenizes `text` line by line without a DOM or editor (as the stream
 * language does); calls `emit` for every non-null token with its line index.
 */
export function tokenize(text: string, emit: (token: string, value: string, line: number, from: number) => void): TypstState {
  const state = typstParser.startState!(2);
  text.split("\n").forEach((line, n) => {
    const stream = new StringStream(line, 4, 2);
    if (stream.eol()) {
      typstParser.blankLine!(state, 2);
      return;
    }
    while (!stream.eol()) {
      stream.start = stream.pos;
      let token: string | null = null;
      for (let tries = 0; tries < 10 && stream.pos === stream.start; tries++) token = typstParser.token(stream, state);
      if (stream.pos === stream.start) throw new Error(`tokenizer stuck at ${n}:${stream.pos}`);
      if (token) emit(token, stream.current(), n, stream.start);
    }
  });
  return state;
}
