// Figures: finding `#figure(...)` expressions (balanced-bracket scan that
// respects strings, comments, raw, math and content blocks), the insert
// snippet, image bytes as cached Blob URLs, and the science view's preview
// (image + caption + label while the cursor is elsewhere; source inside).
// The preview sizes images against the layout's text column like Typst does
// and shows the figure's placement (or a publisher exception's) as a badge.
// No Tauri imports: image bytes, the layout and the exceptions are injected.

import { syntaxTree } from "@codemirror/language";
import { EditorSelection, type EditorState, type Extension, type Range, StateEffect, StateField, type Text, type TransactionSpec } from "@codemirror/state";
import { Decoration, type DecorationSet, EditorView, ViewPlugin, type ViewUpdate, WidgetType } from "@codemirror/view";
import { ASSET_TYPES } from "./assettypes.ts";

// ---- Scanning ----

/** A figure expression found in the text (offsets relative to the scanned string plus `offset`). */
export interface FigureSpan {
  /** Position of `#figure(`. */
  from: number;
  /** End of the expression, including a trailing label if there is one. */
  to: number;
  /** End of the call (after its `)`). */
  callEnd: number;
  /** The label after the call (`fig:x`), if any. */
  label: string | null;
  /** The image path, when it is a literal relative path under `assets/` with a supported extension. */
  path: string | null;
  /** The caption's markup (trimmed, whitespace collapsed), if any. */
  caption: string | null;
  /** Start of the caption's content inside the brackets (for placing the cursor). */
  captionFrom: number | null;
  /** The literal `width:`, `height:` and `fit:` of the `image(...)` call. */
  image: ImageArgs;
  /** The figure's literal `placement:`, if any. */
  placement: FigurePlacement | null;
}

/** A literal length argument. */
export interface Length {
  value: number;
  unit: "%" | "pt" | "mm" | "cm" | "in" | "em";
}

/** A size argument: a literal length, `auto`, or null (absent, or not a literal length). */
export type Size = Length | "auto" | null;
export type Fit = "cover" | "contain" | "stretch";
export type FigurePlacement = "top" | "bottom" | "auto" | "none";

export interface ImageArgs {
  width: Size;
  height: Size;
  fit: Fit | null;
}

/** Longest figure expression considered (unclosed ones end the scan early). */
export const MAX_FIGURE = 20_000;

const FIGURE = "#figure(";
const LABEL_AFTER = /[ \t]*<([\p{L}\p{N}_][\p{L}\p{N}_\-.:]*)>/uy;
const IMAGE = /^\s*image\(\s*"([^"\\\n]*)"/;
const CAPTION = /(?:^|[,(\s])caption\s*:\s*\[/g;
const IDENT = /[\p{ID_Start}_][\p{ID_Continue}\-]*/uy;
const NAMED = /^\s*([\p{ID_Start}_][\p{ID_Continue}\-]*)\s*:([\s\S]*)$/u;
const LENGTH = /^(\d+(?:\.\d*)?|\.\d+)(%|pt|mm|cm|in|em)$/;

/** End of the string starting at the quote `i` (after the closing quote), or -1. */
export function stringEnd(text: string, i: number, limit: number): number {
  for (let j = i + 1; j < limit; j++) {
    const c = text.charCodeAt(j);
    if (c === 92 /* \ */) j++;
    else if (c === 34 /* " */) return j + 1;
  }
  return -1;
}

/** End of the equation opened at `i` (after its closing `$`), or -1. */
export function mathEnd(text: string, i: number, limit: number): number {
  for (let j = i + 1; j < limit; j++) {
    const c = text.charCodeAt(j);
    if (c === 92) j++;
    else if (c === 34) {
      const end = stringEnd(text, j, limit);
      if (end < 0) return -1;
      j = end - 1;
    } else if (c === 36 /* $ */) return j + 1;
  }
  return -1;
}

/** End of the raw span opened by the backtick run at `i`, or -1. */
export function rawEnd(text: string, i: number, limit: number): number {
  let n = 0;
  while (text.charCodeAt(i + n) === 96) n++;
  if (n === 2) return i + 2;
  const fence = "`".repeat(n);
  for (let j = text.indexOf(fence, i + n); j >= 0 && j < limit; j = text.indexOf(fence, j + 1)) {
    let run = 0;
    while (text.charCodeAt(j + run) === 96) run++;
    if (run === n) return j + n;
    j += run - 1;
  }
  return -1;
}

/** End of a comment at `i` (line comment to the line end, block comments nested), or -1 if none starts there. */
export function commentEnd(text: string, i: number, limit: number): number {
  if (text.charCodeAt(i) !== 47 /* / */) return -1;
  const next = text.charCodeAt(i + 1);
  if (next === 47) {
    if (/https?:$/.test(text.slice(Math.max(0, i - 6), i))) return -1;
    const nl = text.indexOf("\n", i);
    return nl < 0 || nl > limit ? limit : nl;
  }
  if (next !== 42 /* * */) return -1;
  let depth = 1;
  for (let j = i + 2; j < limit - 1; j++) {
    if (text.charCodeAt(j) === 47 && text.charCodeAt(j + 1) === 42) {
      depth++;
      j++;
    } else if (text.charCodeAt(j) === 42 && text.charCodeAt(j + 1) === 47) {
      j++;
      if (--depth === 0) return j + 1;
    }
  }
  return limit;
}

/**
 * Index after the bracket closing the one at `open` (`(`, `[` or `{`), or -1
 * if it doesn't close within `limit`. `(`/`{` contain code (strings,
 * comments, nested brackets), `[` contains markup (escapes, raw, math,
 * `#expr(...)`, nested `[...]`).
 */
export function closeBracket(text: string, open: number, limit = Math.min(text.length, open + MAX_FIGURE)): number {
  const closer: Record<number, string> = { 40: ")", 91: "]", 123: "}" };
  const first = closer[text.charCodeAt(open)];
  if (!first) return -1;
  const stack: string[] = [first];
  let i = open + 1;
  while (i < limit) {
    const top = stack[stack.length - 1];
    const c = text.charCodeAt(i);
    const comment = c === 47 ? commentEnd(text, i, limit) : -1;
    if (comment >= 0) {
      i = comment;
      continue;
    }
    if (c === 96 /* ` */) {
      i = rawEnd(text, i, limit);
      if (i < 0) return -1;
      continue;
    }
    if (c === 36 /* $ */) {
      i = mathEnd(text, i, limit);
      if (i < 0) return -1;
      continue;
    }
    if (top === "]") {
      // Markup.
      if (c === 92) i += 2;
      else if (c === 91) {
        stack.push("]");
        i++;
      } else if (c === 93) {
        stack.pop();
        i++;
        if (stack.length === 0) return i;
      } else if (c === 35 /* # */) {
        IDENT.lastIndex = i + 1;
        const end = IDENT.test(text) ? IDENT.lastIndex : i + 1;
        const next = text.charCodeAt(end);
        if (next === 40 || next === 123) {
          stack.push(closer[next]);
          i = end + 1;
        } else i = end;
      } else i++;
    } else {
      // Code.
      if (c === 34) {
        i = stringEnd(text, i, limit);
        if (i < 0) return -1;
      } else if (c === 40 || c === 91 || c === 123) {
        stack.push(closer[c]);
        i++;
      } else if (c === 41 || c === 125 || c === 93) {
        if (String.fromCharCode(c) !== top) return -1;
        stack.pop();
        i++;
        if (stack.length === 0) return i;
      } else i++;
    }
  }
  return -1;
}

/** The named arguments of the call whose `(` is at `open` and that ends at `end`: name → value source (trimmed). */
export function namedArgs(text: string, open: number, end: number): Map<string, string> {
  const out = new Map<string, string>();
  let start = open + 1;
  for (let i = start; i < end; ) {
    const c = text.charCodeAt(i);
    if (c === 44 /* , */ || i === end - 1) {
      const m = NAMED.exec(text.slice(start, i));
      if (m) out.set(m[1], m[2].trim());
      start = ++i;
      continue;
    }
    let next = i + 1;
    if (c === 47) next = Math.max(next, commentEnd(text, i, end));
    else if (c === 34) next = stringEnd(text, i, end);
    else if (c === 40 || c === 91 || c === 123) next = closeBracket(text, i, end);
    if (next < 0) break;
    i = next;
  }
  return out;
}

/** A size argument's source as a literal length or `auto` (null otherwise, e.g. `1fr` or `50% - 1cm`). */
function parseSize(source: string | undefined): Size {
  if (source === "auto") return "auto";
  const m = source === undefined ? null : LENGTH.exec(source);
  return m ? { value: Number(m[1]), unit: m[2] as Length["unit"] } : null;
}

/** The literal sizing arguments of the image call whose `(` is at `open`. */
function imageArgs(text: string, open: number, limit: number): ImageArgs {
  const end = closeBracket(text, open, limit);
  const args = end < 0 ? new Map<string, string>() : namedArgs(text, open, end);
  const fit = /^"(cover|contain|stretch)"$/.exec(args.get("fit") ?? "");
  return { width: parseSize(args.get("width")), height: parseSize(args.get("height")), fit: fit ? (fit[1] as Fit) : null };
}

/** Image types the preview (and Typst 0.15) supports, by lowercase extension. */
const MIME: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  svg: "image/svg+xml",
  webp: "image/webp",
};

export const IMAGE_EXTENSIONS = Object.keys(MIME);

/** The MIME type for an image path by extension (Typst matches extensions case-sensitively, so lowercase only); null if unsupported. */
export function mimeFor(path: string): string | null {
  const m = /\.([a-z0-9]+)$/.exec(path);
  return (m && MIME[m[1]]) ?? null;
}

/** A literal project path under `assets/` (no `..`, `.`, empty parts, backslashes or colons) with an image extension. */
export function isAssetPath(path: string): boolean {
  if (!path.startsWith("assets/") || /[\\:\0]/.test(path)) return false;
  const parts = path.split("/");
  return parts.length >= 2 && parts.every((p) => p !== "" && p !== "." && p !== "..") && mimeFor(path) !== null;
}

/** The figure expression starting with `#figure(` at `start` in `text`, or null if it doesn't close. */
export function scanFigure(text: string, start: number, offset = 0): FigureSpan | null {
  const open = start + FIGURE.length - 1;
  const end = closeBracket(text, open);
  if (end < 0) return null;
  const body = text.slice(open + 1, end - 1);
  let label: string | null = null;
  let to = end;
  LABEL_AFTER.lastIndex = end;
  const m = LABEL_AFTER.exec(text);
  if (m) {
    label = m[1];
    to = LABEL_AFTER.lastIndex;
  }
  const image = IMAGE.exec(body);
  const path = image && isAssetPath(image[1]) ? image[1] : null;
  const args = image ? imageArgs(text, open + 1 + image[0].indexOf("("), end) : { width: null, height: null, fit: null };
  const placed = /^(top|bottom|auto|none)$/.exec(namedArgs(text, open, end).get("placement") ?? "");
  let caption: string | null = null;
  let captionFrom: number | null = null;
  CAPTION.lastIndex = 0;
  const c = CAPTION.exec(body);
  if (c) {
    const bracket = open + 1 + c.index + c[0].length - 1;
    const close = closeBracket(text, bracket, end);
    if (close > 0) {
      caption = text.slice(bracket + 1, close - 1).replace(/\s+/g, " ").trim();
      captionFrom = offset + bracket + 1;
    }
  }
  const placement = placed ? (placed[1] as FigurePlacement) : null;
  return { from: offset + start, to: offset + to, callEnd: offset + end, label, path, caption, captionFrom, image: args, placement };
}

/** Whether the syntax tree marks `pos` as code (so `#figure(` there isn't inside raw, a comment or math); true where unparsed. */
export function codeAt(state: EditorState, pos: number): boolean {
  const tree = syntaxTree(state);
  if (tree.length < pos + 1) return true;
  const name = tree.resolveInner(pos + 1, 1).name;
  return /(^|[_ ])code([_ ]|$)/.test(name);
}

/** Figures overlapping `from..to` that start at most `MAX_FIGURE` before it. */
export function findFigures(state: EditorState, from: number, to: number): FigureSpan[] {
  const doc = state.doc;
  const start = Math.max(0, from - MAX_FIGURE);
  const end = Math.min(doc.length, to + MAX_FIGURE);
  const text = doc.sliceString(start, end);
  const out: FigureSpan[] = [];
  const last = to - start;
  for (let i = text.indexOf(FIGURE); i >= 0 && i <= last; i = text.indexOf(FIGURE, i + 1)) {
    const span = scanFigure(text, i, start);
    if (!span || span.to < from || !codeAt(state, span.from)) continue;
    out.push(span);
    i = span.to - start - 1;
  }
  return out;
}

// ---- Inserting ----

/** A label stem from a file name: lowercase ASCII letters, digits and `-`. */
export function labelStem(path: string): string {
  const name = path.split("/").pop() ?? "";
  const stem = name.replace(/\.[^.]*$/, "");
  return (
    stem
      .normalize("NFKD")
      .replace(/[̀-ͯ]/g, "")
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 40)
      .replace(/-+$/, "") || "figure"
  );
}

/** `fig:<stem>`, or `fig:<stem>-2`, `-3`, ... if taken. */
export function uniqueLabel(stem: string, taken: (name: string) => boolean): string {
  if (!taken(`fig:${stem}`)) return `fig:${stem}`;
  for (let n = 2; ; n++) if (!taken(`fig:${stem}-${n}`)) return `fig:${stem}-${n}`;
}

/** A figure to insert: the asset's path and the figure's label. */
export interface NewFigure {
  path: string;
  label: string;
}

/**
 * The markup replacing `from..to` in `doc` with `figures`, each its own
 * block (blank lines between them and around them, as neighbouring text
 * needs): the text and the offset of the first "Caption" in it.
 */
export function figureText(doc: Text, from: number, to: number, figures: readonly NewFigure[]): { insert: string; caption: number } {
  const line = doc.lineAt(from);
  const endLine = doc.lineAt(to);
  const blank = (n: number) => n < 1 || n > doc.lines || !doc.line(n).text.trim();
  // Separated from neighbouring text by a blank line.
  const before = line.text.slice(0, from - line.from).trim() ? "\n\n" : blank(line.number - 1) ? "" : "\n";
  const after = endLine.text.slice(to - endLine.from).trim() ? "\n\n" : to === doc.length ? "\n" : blank(endLine.number + 1) ? "" : "\n";
  const body = figures.map((f) => `#figure(\n  ${ASSET_TYPES.image.snippet(f.path)},\n  caption: [Caption],\n) <${f.label}>`).join("\n\n");
  return { insert: before + body + after, caption: before.length + body.indexOf("Caption") };
}

/** Labels for figures showing `paths` (`uniqueLabel` of each stem, none taken twice). */
export function figureLabels(paths: readonly string[], taken: (name: string) => boolean): NewFigure[] {
  const chosen = new Set<string>();
  return paths.map((path) => {
    const label = uniqueLabel(labelStem(path), (name) => chosen.has(name) || taken(name));
    chosen.add(label);
    return { path, label };
  });
}

/** The figure markup for `path` at the main cursor, on its own lines, with "Caption" selected. */
export function insertFigure(state: EditorState, path: string, label: string): TransactionSpec {
  const { from, to } = state.selection.main;
  const { insert, caption } = figureText(state.doc, from, to, [{ path, label }]);
  return {
    changes: { from, to, insert },
    selection: EditorSelection.single(from + caption, from + caption + "Caption".length),
    scrollIntoView: true,
    userEvent: "input",
  };
}

// ---- Sizing ----

/**
 * What images are sized against: the layout's text area (`layout_get`'s
 * `text_area`) and the default image width (percent of the column), which
 * the generated preamble sets for every image (`#set image(width: ..)`).
 */
export interface FigureLayout {
  column_width_pt: number;
  height_pt: number;
  font_size_pt: number;
  default_width: number;
}

/** How the preview draws an image (the editor's column stands for the layout's text column). */
export interface ImageBox {
  /** Width as a fraction of the column (at most 1). */
  width: number;
  /** The box's width/height when it isn't the image's own aspect ratio (the image fills it per `fit`). */
  ratio: number | null;
  fit: Fit;
}

const PT_PER: Record<string, number> = { pt: 1, mm: 72 / 25.4, cm: 72 / 2.54, in: 72 };

/** `l` in points; `%` of `whole` points, `em` of the body text size. */
export function lengthPt(l: Length, layout: FigureLayout, whole: number): number {
  if (l.unit === "%") return (l.value / 100) * whole;
  if (l.unit === "em") return l.value * layout.font_size_pt;
  return l.value * PT_PER[l.unit];
}

/**
 * The image's box, like Typst lays it out in a column: a width (else the
 * layout's default width) and no height keeps the aspect ratio; a width and
 * a height make a box the image fits per `fit` (default cover); `width:
 * auto` with a height scales from the height; `width: auto` alone is the
 * natural size. Wider than the column is capped. `natural` (points) is
 * needed only for `width: auto`: null if it's needed but unknown.
 */
export function imageBox(args: ImageArgs, layout: FigureLayout, natural: { width: number; height: number } | null): ImageBox | null {
  const col = layout.column_width_pt;
  const fit = args.fit ?? "cover";
  const height = args.height && args.height !== "auto" ? lengthPt(args.height, layout, layout.height_pt) : null;
  if (args.width !== "auto") {
    const width = lengthPt(args.width ?? { value: layout.default_width, unit: "%" }, layout, col);
    if (!(width > 0)) return null;
    return { width: Math.min(width / col, 1), ratio: height && height > 0 ? width / height : null, fit };
  }
  if (!natural || !(natural.width > 0 && natural.height > 0)) return null;
  if (height === null) return { width: Math.min(natural.width / col, 1), ratio: null, fit };
  if (!(height > 0)) return null;
  const scaled = (height * natural.width) / natural.height;
  return scaled <= col ? { width: scaled / col, ratio: null, fit } : { width: 1, ratio: col / height, fit };
}

/** The pixel density Typst reads from an image's metadata (EXIF, else JFIF or PNG `pHYs`), or null. */
export function imageDpi(bytes: Uint8Array): number | null {
  try {
    const d = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    let exif: number | null = null;
    let other: number | null = null;
    if (d.getUint32(0) === 0x89504e47) {
      // PNG chunks up to the image data.
      for (let i = 8; i + 8 <= d.byteLength; i += 12 + d.getUint32(i)) {
        const type = d.getUint32(i + 4);
        if (type === 0x49444154 /* IDAT */) break;
        if (type === 0x70485973 /* pHYs */ && d.getUint8(i + 16) === 1) other = Math.max(d.getUint32(i + 8), d.getUint32(i + 12)) * 0.0254;
        if (type === 0x65584966 /* eXIf */) exif = tiffDpi(d, i + 8);
      }
    } else if (d.getUint16(0) === 0xffd8) {
      // JPEG segments up to the scan.
      if (d.getUint16(2) === 0xffe0 && d.getUint16(4) >= 16 && d.getUint32(6) === 0x4a464946 /* JFIF */ && d.getUint8(10) === 0 && d.getUint8(11) === 1) {
        const dpu = Math.max(d.getUint16(14), d.getUint16(16));
        const unit = d.getUint8(13);
        other = unit === 1 ? dpu : unit === 2 ? dpu * 2.54 : null;
      }
      for (let i = 2; i + 4 <= d.byteLength && d.getUint8(i) === 0xff; i += 2 + d.getUint16(i + 2)) {
        const marker = d.getUint8(i + 1);
        if (marker === 0xda) break;
        if (marker === 0xe1 && d.getUint32(i + 4) === 0x45786966 /* Exif */) {
          exif = tiffDpi(d, i + 10);
          break;
        }
      }
    }
    const dpi = exif ?? other;
    return dpi !== null && dpi > 0 ? dpi : null;
  } catch {
    return null;
  }
}

/** The larger of the X/Y resolutions in the EXIF (TIFF) block at `start`, or null. */
function tiffDpi(d: DataView, start: number): number | null {
  const le = d.getUint16(start) === 0x4949;
  const ifd = start + d.getUint32(start + 4, le);
  let dpi: number | null = null;
  for (let k = 0, n = d.getUint16(ifd, le); k < n; k++) {
    const e = ifd + 2 + k * 12;
    const tag = d.getUint16(e, le);
    if ((tag === 0x011a || tag === 0x011b) && d.getUint16(e + 2, le) === 5 /* rational */) {
      const at = start + d.getUint32(e + 8, le);
      const den = d.getUint32(at + 4, le);
      if (den > 0) dpi = Math.max(dpi ?? 0, d.getUint32(at, le) / den);
    }
  }
  return dpi;
}

/** An image's natural size in points (as Typst sizes it: pixels at its DPI, else 72; SVG at 96). */
function naturalPt(img: HTMLImageElement, svg: boolean, dpi: number | null): { width: number; height: number } | null {
  if (!img.naturalWidth || !img.naturalHeight) return null;
  const scale = 72 / (svg ? 96 : (dpi ?? 72));
  return { width: img.naturalWidth * scale, height: img.naturalHeight * scale };
}

// ---- Placement ----

/** A figure placement exception (`overrides_get`) as the science view needs it. */
export interface PlacementException {
  /** The anchor (UTF-16, current text). */
  pos: number;
  placement: string;
  detached: boolean;
}

/** The figure placement exceptions among the overrides that `overrides_get` lists. */
export function placementExceptions(list: readonly { kind: string | null; params: Record<string, unknown>; pos_utf16: number | null; detached: boolean }[]): PlacementException[] {
  return list.flatMap((o) => (o.kind === "figure_placement" && o.pos_utf16 !== null ? [{ pos: o.pos_utf16, placement: String(o.params.placement ?? ""), detached: o.detached }] : []));
}

/**
 * Which figure each exception applies to, like the core's `figure_at`: the
 * call containing the anchor, else the first one starting on its line.
 * `spans` are in document order.
 */
export function matchExceptions(doc: Text, spans: readonly FigureSpan[], exceptions: readonly PlacementException[]): Map<FigureSpan, PlacementException> {
  const out = new Map<FigureSpan, PlacementException>();
  for (const e of exceptions) {
    if (e.pos > doc.length) continue;
    // The call starts after the `#`.
    let span = spans.find((s) => s.from + 1 <= e.pos && e.pos < s.callEnd);
    if (!span) {
      const line = doc.lineAt(e.pos);
      span = spans.find((s) => s.from + 1 >= line.from && s.from + 1 < line.to);
    }
    if (span && !out.has(span)) out.set(span, e);
  }
  return out;
}

export interface PlacementBadge {
  text: string;
  kind: "own" | "exception" | "detached";
}

const PLACEMENT_NAMES: Record<string, string> = { top: "Top", bottom: "Bottom", auto: "Auto", none: "Here", here: "Here" };

/** The badge of a figure: an exception's placement, else its own `placement:` argument, else none. */
export function placementBadge(span: FigureSpan, exception: PlacementException | undefined): PlacementBadge | null {
  if (exception) {
    const name = PLACEMENT_NAMES[exception.placement] ?? exception.placement;
    return { text: `${name} · exception`, kind: exception.detached ? "detached" : "exception" };
  }
  return span.placement ? { text: PLACEMENT_NAMES[span.placement], kind: "own" } : null;
}

// ---- Image cache ----

export type AssetBytes = ArrayBuffer | Uint8Array | number[];
export type LoadAsset = (path: string) => Promise<AssetBytes>;
export type AssetState = { url: string; dpi: number | null } | { error: string };

/**
 * Images by project path as Blob URLs (bounded; evicted URLs are revoked),
 * loaded on demand, at most `concurrency` at a time.
 */
export class AssetCache {
  private load: LoadAsset;
  private onResult: () => void;
  private capacity: number;
  private done = new Map<string, AssetState>();
  private running = new Set<string>();
  private queue: string[] = [];
  private concurrency = 2;

  constructor(load: LoadAsset, onResult: () => void, capacity = 40) {
    this.load = load;
    this.onResult = onResult;
    this.capacity = capacity;
  }

  get(path: string): AssetState | undefined {
    const hit = this.done.get(path);
    if (hit) {
      this.done.delete(path);
      this.done.set(path, hit);
    }
    return hit;
  }

  want(paths: string[]): void {
    this.queue = [...new Set(paths)].filter((p) => !this.done.has(p) && !this.running.has(p));
    this.pump();
  }

  /** Drops `path` (its image was replaced), so it loads again when wanted. */
  forget(path: string): void {
    const s = this.done.get(path);
    if (s && "url" in s) URL.revokeObjectURL(s.url);
    this.done.delete(path);
  }

  /** Drops everything (e.g. another project), revoking the URLs. */
  clear(): void {
    for (const s of this.done.values()) if ("url" in s) URL.revokeObjectURL(s.url);
    this.done.clear();
    this.queue = [];
  }

  get size(): number {
    return this.done.size;
  }

  private pump(): void {
    while (this.running.size < this.concurrency && this.queue.length > 0) this.start(this.queue.shift()!);
  }

  private start(path: string): void {
    this.running.add(path);
    const mime = mimeFor(path);
    const loaded: Promise<AssetState> = mime
      ? Promise.resolve()
          .then(() => this.load(path))
          .then((data) => {
            const bytes = data instanceof ArrayBuffer || ArrayBuffer.isView(data) ? data : new Uint8Array(data);
            const view = bytes instanceof ArrayBuffer ? new Uint8Array(bytes) : new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength);
            return { url: URL.createObjectURL(new Blob([bytes as BlobPart], { type: mime })), dpi: mime === "image/svg+xml" ? null : imageDpi(view) };
          })
      : Promise.resolve({ error: `Unsupported image type: ${path}` });
    loaded
      .catch((): AssetState => ({ error: `Image not found: ${path}` }))
      .then((state) => {
        this.running.delete(path);
        this.done.set(path, state);
        while (this.done.size > this.capacity) {
          const [oldest, s] = this.done.entries().next().value!;
          this.done.delete(oldest);
          if ("url" in s) URL.revokeObjectURL(s.url);
        }
        this.pump();
        this.onResult();
      });
  }
}

// ---- Editor extension ----

const setRange = StateEffect.define<{ from: number; to: number }>();
const loaded = StateEffect.define<null>();
const setLayout = StateEffect.define<FigureLayout | null>();
const setExceptions = StateEffect.define<PlacementException[]>();

interface FigureView {
  range: { from: number; to: number } | null;
  spans: FigureSpan[];
  deco: DecorationSet;
  /** Null until known: images then show at their natural size, capped at the column. */
  layout: FigureLayout | null;
  /** Positions mapped through text changes until the next list. */
  exceptions: PlacementException[];
}

const OBJECT_FIT: Record<Fit, string> = { cover: "cover", contain: "contain", stretch: "fill" };

const BADGE_TITLES: Record<PlacementBadge["kind"], string> = {
  own: "Placement (the figure's placement argument)",
  exception: "Placement set by an exception in the publisher view",
  detached: "Placement set by an exception whose anchor text was deleted; it applies at the nearest figure",
};

const touches = (state: EditorState, span: FigureSpan) => state.selection.ranges.some((r) => r.from <= span.to && r.to >= span.from);

class FigureWidget extends WidgetType {
  readonly span: FigureSpan;
  readonly image: AssetState | undefined;
  readonly block: boolean;
  readonly layout: FigureLayout | null;
  readonly badge: PlacementBadge | null;
  /** What the image's size depends on. */
  readonly sizing: string;

  constructor(span: FigureSpan, image: AssetState | undefined, block: boolean, layout: FigureLayout | null, badge: PlacementBadge | null) {
    super();
    this.span = span;
    this.image = image;
    this.block = block;
    this.layout = layout;
    this.badge = badge;
    this.sizing = JSON.stringify([span.image, layout]);
  }

  eq(other: FigureWidget): boolean {
    const a = this.span;
    const b = other.span;
    return (
      other.image === this.image &&
      other.block === this.block &&
      a.path === b.path &&
      a.caption === b.caption &&
      a.label === b.label &&
      other.sizing === this.sizing &&
      other.badge?.text === this.badge?.text &&
      other.badge?.kind === this.badge?.kind
    );
  }

  get estimatedHeight(): number {
    return 320;
  }

  toDOM(view: EditorView): HTMLElement {
    const wrap: HTMLElement = document.createElement(this.block ? "div" : "span");
    wrap.className = this.block ? "cm-figure" : "cm-figure cm-figure-inline";
    const image = this.image;
    if (image && "url" in image) {
      const img = document.createElement("img");
      img.src = image.url;
      img.alt = this.span.caption ?? this.span.path ?? "";
      img.draggable = false;
      // Sized once the layout is known (natural sizes once loaded).
      const size = () => {
        if (!this.layout) return true;
        const box = imageBox(this.span.image, this.layout, naturalPt(img, mimeFor(this.span.path ?? "") === "image/svg+xml", image.dpi));
        if (!box) return false;
        // The caption keeps the full column; an inline figure is sized as a whole.
        (this.block ? img : wrap).style.width = `${box.width * 100}%`;
        if (!this.block) img.style.width = "100%";
        if (box.ratio !== null) {
          img.style.aspectRatio = String(box.ratio);
          img.style.objectFit = OBJECT_FIT[box.fit];
        }
        return true;
      };
      const sized = size();
      img.addEventListener("load", () => {
        if (!sized) size();
        view.requestMeasure();
      });
      img.addEventListener("error", () => {
        img.replaceWith(placeholder(`Image could not be shown: ${this.span.path}`));
        view.requestMeasure();
      });
      wrap.append(img);
    } else {
      wrap.append(placeholder(image ? image.error : "Loading image…", !image));
    }
    const caption = document.createElement(this.block ? "div" : "span");
    caption.className = "cm-figure-caption";
    caption.textContent = this.span.caption ?? "";
    if (this.span.label) {
      const label = document.createElement("span");
      label.className = "cm-figure-label";
      label.textContent = `<${this.span.label}>`;
      caption.append(" ", label);
    }
    if (this.badge) {
      const badge = document.createElement("span");
      badge.className = `cm-figure-badge ${this.badge.kind}`;
      badge.textContent = this.badge.text;
      badge.title = BADGE_TITLES[this.badge.kind];
      caption.append(" ", badge);
    }
    wrap.append(caption);
    wrap.title = "Click to edit the figure";
    // Clicking a figure puts the cursor into its caption (or its source).
    wrap.addEventListener("mousedown", (e) => {
      if (e.button !== 0) return;
      e.preventDefault();
      const start = view.posAtDOM(wrap);
      const offset = (this.span.captionFrom ?? this.span.from + 1) - this.span.from;
      view.dispatch({ selection: { anchor: Math.min(start + offset, view.state.doc.length) }, userEvent: "select.pointer" });
      view.focus();
    });
    return wrap;
  }

  ignoreEvent(): boolean {
    return true;
  }
}

function placeholder(text: string, loading = false): HTMLElement {
  const el = document.createElement("span");
  el.className = loading ? "cm-figure-missing cm-figure-loading" : "cm-figure-missing";
  el.textContent = text;
  return el;
}

/**
 * The science view's figure previews; `load` returns an asset's bytes
 * (`asset_bytes`). `clear` drops the cached images and the layout (another
 * project), `reload` the images only (F5, undo, restore: they may differ
 * now), `invalidate` one image (replaced); `setLayout` and `setExceptions`
 * pass on the layout and the figure placement exceptions (positions in the
 * current text).
 */
export function figurePreview(load: LoadAsset): {
  extension: Extension;
  clear(): void;
  reload(): void;
  invalidate(path: string): void;
  setLayout(layout: FigureLayout | null): void;
  setExceptions(list: PlacementException[]): void;
} {
  let notify: (() => void) | null = null;
  let current: { view: EditorView } | null = null;
  // Kept for when the view (re)gains the extension.
  let layout: FigureLayout | null = null;
  let exceptions: PlacementException[] = [];
  const cache = new AssetCache(load, () => notify?.());

  function decorations(state: EditorState, value: Omit<FigureView, "deco">): DecorationSet {
    const ranges: Range<Decoration>[] = [];
    const matched = matchExceptions(state.doc, value.spans, value.exceptions);
    for (const span of value.spans) {
      if (!span.path || touches(state, span)) continue;
      const first = state.doc.lineAt(span.from);
      const last = state.doc.lineAt(span.to);
      const block = first.from === span.from && last.to === span.to;
      const widget = new FigureWidget(span, cache.get(span.path), block, value.layout, placementBadge(span, matched.get(span)));
      ranges.push(Decoration.replace({ widget, block }).range(span.from, span.to));
    }
    return Decoration.set(ranges, true);
  }

  const field = StateField.define<FigureView>({
    create: () => ({ range: null, spans: [], deco: Decoration.none, layout, exceptions }),
    update(value, tr) {
      let { range, layout, exceptions } = value;
      let respan = false;
      let redeco = !!tr.selection;
      if (tr.docChanged) {
        if (range) {
          range = { from: tr.changes.mapPos(range.from, -1), to: tr.changes.mapPos(range.to, 1) };
          respan = true;
        }
        exceptions = exceptions.map((e) => ({ ...e, pos: tr.changes.mapPos(e.pos, 1) }));
      }
      for (const e of tr.effects) {
        if (e.is(setRange)) {
          range = e.value;
          respan = true;
        } else if (e.is(setLayout)) {
          layout = e.value;
          redeco = true;
        } else if (e.is(setExceptions)) {
          exceptions = e.value;
          redeco = true;
        } else if (e.is(loaded)) redeco = true;
      }
      if (range && syntaxTree(tr.state) !== syntaxTree(tr.startState)) respan = true;
      if (!respan && !redeco) return tr.docChanged ? { ...value, exceptions, deco: value.deco.map(tr.changes) } : value;
      const spans = respan && range ? findFigures(tr.state, range.from, range.to) : value.spans;
      const next = { range, spans, layout, exceptions };
      return { ...next, deco: decorations(tr.state, next) };
    },
    provide: (f) => EditorView.decorations.from(f, (v) => v.deco),
  });

  const plugin = ViewPlugin.fromClass(
    class {
      view: EditorView;
      timer: ReturnType<typeof setTimeout> | undefined;
      notifyTimer: ReturnType<typeof setTimeout> | undefined;

      constructor(view: EditorView) {
        this.view = view;
        notify = this.notify;
        current = this;
        this.schedule();
      }

      notify = () => {
        if (this.notifyTimer !== undefined) return;
        this.notifyTimer = setTimeout(() => {
          this.notifyTimer = undefined;
          if (this.view.state.field(field, false)) this.view.dispatch({ effects: loaded.of(null) });
        }, 0);
      };

      update(u: ViewUpdate) {
        if (u.docChanged || u.viewportChanged || u.transactions.some((tr) => tr.effects.length > 0)) this.schedule();
      }

      schedule() {
        if (this.timer !== undefined) return;
        this.timer = setTimeout(() => {
          this.timer = undefined;
          this.sync();
        }, 0);
      }

      sync() {
        const value = this.view.state.field(field, false);
        if (!value) return;
        const vp = this.view.viewport;
        const r = value.range;
        if (!r || vp.from < r.from || vp.to > r.to || r.to - r.from > 3 * (vp.to - vp.from) + 4000) {
          const len = this.view.state.doc.length;
          this.view.dispatch({ effects: setRange.of({ from: Math.max(0, vp.from - 1000), to: Math.min(len, vp.to + 1000) }) });
          return;
        }
        cache.want(value.spans.filter((s) => s.path && s.to >= vp.from && s.from <= vp.to).map((s) => s.path!));
      }

      destroy() {
        clearTimeout(this.timer);
        clearTimeout(this.notifyTimer);
        if (notify === this.notify) notify = null;
        if (current === this) current = null;
      }
    },
  );

  /** Sends `effect` to the view if it shows figure previews. */
  const send = (effect: StateEffect<unknown>) => {
    if (current?.view.state.field(field, false)) current.view.dispatch({ effects: effect });
  };

  return {
    extension: [field, plugin],
    clear: () => {
      cache.clear();
      layout = null;
      send(setLayout.of(null));
    },
    reload: () => {
      cache.clear();
      // Redraws the figures, which asks for their images again.
      send(setLayout.of(layout));
    },
    invalidate: (path) => {
      cache.forget(path);
      send(setLayout.of(layout));
    },
    setLayout: (next) => {
      if (JSON.stringify(next) === JSON.stringify(layout)) return;
      layout = next;
      send(setLayout.of(next));
    },
    setExceptions: (list) => {
      exceptions = list;
      send(setExceptions.of(list));
    },
  };
}
