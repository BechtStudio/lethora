// Character styles for the formatting row and the editor (Milestone 6
// step 2): content says `#styled("<id>")[…]`, the style's font and size live
// in the layout layer (`styles.<id>.font|size|name`). The row shows the
// font, size and style at the selection; a font or size picked for a
// selection reuses an unnamed style with exactly those settings or creates
// one (named styles are never reused silently); the text edits themselves
// are `restyle` (format.ts). The editor shows styled text in its font at a
// size relative to the body's, and marks references to missing styles.
//
// Pure functions and an editor extension; no DOM, tested under `node --test`.

import { type EditorState, type Extension, type Range } from "@codemirror/state";
import { Decoration, type DecorationSet, type EditorView, ViewPlugin, type ViewUpdate } from "@codemirror/view";
import { cssFamily } from "./fontpicker.ts";
import { lineSpans, segments, type Span } from "./format.ts";
import { type CharStyle, type LayoutState, valueOf } from "./layoutpanel.ts";
import { modeAt } from "./typst.ts";

/** A style's font and size (it may set one of them), or a change of one. */
export interface Settings {
  font?: string;
  size?: string;
}

/** What the row and the editor need from the layout: the body's font and size, the styles by id. */
export interface TextStyles {
  body: { font: string; size: string };
  styles: Record<string, CharStyle>;
  /** Every id the layout knows (also removed styles and stray fields): never given to a new style. */
  ids: ReadonlySet<string>;
}

export function textStylesOf(s: LayoutState): TextStyles {
  const styles = s.layout.styles ?? {};
  const ids = new Set(Object.keys(styles));
  for (const key of Object.keys(s.overrides)) {
    const m = /^styles\.([^.]+)\./.exec(key);
    if (m) ids.add(m[1]);
  }
  return { body: { font: String(valueOf(s, "text.font") ?? ""), size: String(valueOf(s, "text.size") ?? "") }, styles, ids };
}

// ---- Sizes ----

const PT_PER: Record<string, number> = { pt: 1, mm: 72 / 25.4, cm: 72 / 2.54, in: 72 };

/** The size box's list, in points. */
export const SIZES = [8, 9, 10, 10.5, 11, 12, 14, 16, 18, 20, 24, 28, 36, 48, 72];

/** An absolute size (`14pt`, `5mm`, `1.2cm`, `0.5in`) in points; null if it is none. */
export function toPt(size: string | undefined): number | null {
  const m = /^\s*(\d+(?:\.\d+)?|\.\d+)\s*(pt|mm|cm|in)\s*$/i.exec(size ?? "");
  return m ? Number(m[1]) * PT_PER[m[2].toLowerCase()] : null;
}

/** A size as the size box shows it: points without the unit, at most one decimal ("11", "10.5", "14.2" for 5mm). */
export function sizeLabel(size: string): string {
  const pt = toPt(size);
  return pt === null ? size : String(Math.round(pt * 10) / 10);
}

/** The size box's text as a size: a number is points (a decimal comma too), a length with a unit stays; null if it is none. */
export function parseSize(text: string): string | null {
  const t = text.trim().replace(",", ".");
  if (/^(\d+(\.\d+)?|\.\d+)$/.test(t)) return `${Number(t)}pt`;
  return toPt(t) === null ? null : t.replace(/\s+/g, "").toLowerCase();
}

// ---- Comparing and naming ----

const sameFont = (a: string, b: string) => a.trim().toLowerCase() === b.trim().toLowerCase();

function sameSize(a: string, b: string): boolean {
  const x = toPt(a);
  const y = toPt(b);
  return x !== null && y !== null ? Math.abs(x - y) < 0.005 : a.trim() === b.trim();
}

/** Whether `a` and `b` set the same fields to the same values. */
export function sameSettings(a: Settings, b: Settings): boolean {
  const same = (x: string | undefined, y: string | undefined, eq: (x: string, y: string) => boolean) => (x === undefined || y === undefined ? x === y : eq(x, y));
  return same(a.font, b.font, sameFont) && same(a.size, b.size, sameSize);
}

/** Only the fields `s` sets (no empty ones). */
function fields(s: Settings): Settings {
  const out: Settings = {};
  if (s.font) out.font = s.font;
  if (s.size) out.size = s.size;
  return out;
}

/** An unnamed style's name from its settings: "EB Garamond 14 pt", "14 pt", "DejaVu Sans Mono". */
export function computedName(s: Settings): string {
  return [s.font?.trim(), s.size ? `${sizeLabel(s.size)} pt` : ""].filter(Boolean).join(" ");
}

export interface StyleEntry {
  id: string;
  label: string;
  font?: string;
}

/** The Style box's styles: named ones by name, then unnamed ones by their computed name ("Custom N" if none can be made). */
export function styleEntries(info: TextStyles): StyleEntry[] {
  const named: StyleEntry[] = [];
  const unnamed: StyleEntry[] = [];
  let custom = 0;
  for (const id of Object.keys(info.styles).sort()) {
    const s = info.styles[id];
    if (s.name) named.push({ id, label: s.name, font: s.font });
    else unnamed.push({ id, label: computedName(s) || `Custom ${++custom}`, font: s.font });
  }
  const byLabel = (a: StyleEntry, b: StyleEntry) => a.label.localeCompare(b.label, undefined, { numeric: true });
  return [...named.sort(byLabel), ...unnamed.sort(byLabel)];
}

/** The Style box's text for a `StyleState.id`: empty when mixed, a missing style's id. */
export function styleLabel(info: TextStyles, id: string | null | undefined): string {
  if (id === undefined) return "";
  if (id === null) return "Default text";
  return styleEntries(info).find((e) => e.id === id)?.label ?? `“${id}” (missing)`;
}

// ---- Creating styles ----

/** The longest style id. */
const MAX_ID = 48;

/** `text` as an id part: lower-case ASCII letters and digits, runs of anything else one `-`. */
export function slug(text: string): string {
  return text
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

/** A readable id for a style of `settings` (`eb-garamond-14pt`, `10-5pt`), with `-2`, `-3`, … while `taken`. */
export function newStyleId(settings: Settings, taken: (id: string) => boolean): string {
  const base = [settings.font && slug(settings.font), settings.size && slug(settings.size)].filter(Boolean).join("-") || "style";
  for (let n = 1; ; n++) {
    const suffix = n === 1 ? "" : `-${n}`;
    const id = base.slice(0, MAX_ID - suffix.length).replace(/-+$/, "") + suffix;
    if (!taken(id)) return id;
  }
}

/** Styles to create first, by id. */
export type Creations = Map<string, Settings>;

/**
 * The style for text with `target` settings: null if they are the body's
 * (or set nothing: default text); else an unnamed style with exactly these
 * settings, else a new one (added to `creations`).
 */
export function styleFor(target: Settings, info: TextStyles, creations: Creations): string | null {
  const t = fields(target);
  if (sameFont(t.font ?? info.body.font, info.body.font) && sameSize(t.size ?? info.body.size, info.body.size)) return null;
  for (const [id, s] of creations) if (sameSettings(s, t)) return id;
  for (const id of Object.keys(info.styles).sort()) {
    const s = info.styles[id];
    if (!s.name && sameSettings(fields(s), t)) return id;
  }
  const id = newStyleId(t, (id) => info.ids.has(id) || id in info.styles || creations.has(id));
  creations.set(id, t);
  return id;
}

/** A style's own settings (none for a missing one or default text). */
export const settingsOf = (info: TextStyles, id: string | null): Settings => (id !== null && info.styles[id] ? fields(info.styles[id]) : {});

/** The `layout_set` changes creating `creations`: whole styles (a stray name or removal of the id goes). */
export function styleChanges(creations: Creations): Record<string, string | null> {
  const changes: Record<string, string | null> = {};
  for (const [id, s] of creations) {
    changes[`styles.${id}.font`] = s.font ?? null;
    changes[`styles.${id}.size`] = s.size ?? null;
    changes[`styles.${id}.name`] = null;
    changes[`styles.${id}.removed`] = null;
  }
  return changes;
}

// ---- The layout panel's Character styles ----

/** How often the text uses each style: every `#styled("<id>")[` (also in code or math; one pass, any line). */
export function styleUses(text: string): Map<string, number> {
  const uses = new Map<string, number>();
  for (const m of text.matchAll(/#styled\("([^"\\]*)"\)\[/g)) uses.set(m[1], (uses.get(m[1]) ?? 0) + 1);
  return uses;
}

/**
 * A new named style's name ("Style N", the first N no style has) and id
 * (from the name, `style-1`): never one the layout knows or the text still
 * uses (a deleted style's references would show the new one).
 */
export function newNamedStyle(info: TextStyles, uses: ReadonlyMap<string, number> = new Map()): { id: string; name: string } {
  const names = new Set(Object.values(info.styles).map((s) => s.name?.trim().toLowerCase()));
  let n = 1;
  while (names.has(`style ${n}`)) n++;
  const name = `Style ${n}`;
  return { id: newStyleId({ font: name }, (id) => info.ids.has(id) || uses.has(id)), name };
}

/** The unnamed styles the text doesn't use ("Remove unused" keeps named ones on purpose). */
export function unusedStyles(info: TextStyles, uses: ReadonlyMap<string, number>): string[] {
  return Object.keys(info.styles).filter((id) => !info.styles[id].name && !uses.get(id));
}

/**
 * The `layout_set` changes deleting styles `ids`: every field cleared, and
 * `removed` for a preset's style (one with a resolved field that is not
 * stored, i.e. not among the overrides).
 */
export function deleteChanges(s: LayoutState, ids: Iterable<string>): Record<string, true | null> {
  const changes: Record<string, true | null> = {};
  for (const id of ids) {
    const key = (field: string) => `styles.${id}.${field}`;
    const preset = Object.keys(s.layout.styles[id] ?? {}).some((field) => !Object.prototype.hasOwnProperty.call(s.overrides, key(field)));
    for (const field of ["font", "size", "name"]) changes[key(field)] = null;
    changes[key("removed")] = preset ? true : null;
  }
  return changes;
}

// ---- The style at the selection ----

export interface StyleState {
  /** The innermost style the text is in; null: default text; undefined: mixed. */
  id: string | null | undefined;
  /** The font and size in effect (the body's where no style sets them); undefined: mixed. */
  font: string | undefined;
  size: string | undefined;
}

/** The styled spans holding `from..to` (a cursor: in their content), outer ones first. */
function holding(spans: readonly Span[], from: number, to: number): Span[] {
  return spans.filter((s) => s.format === "styled" && (from < to ? s.from <= from && to <= s.to : s.contentFrom <= from && from <= s.contentTo)).reverse();
}

/** What the text held by `chain` (outer first) has: the inner style's fields over the outer's over the body's. */
function stateOf(info: TextStyles, chain: readonly Span[]): StyleState {
  let s: Settings = {};
  for (const span of chain) s = { ...s, ...settingsOf(info, span.style!) };
  return { id: chain.length ? chain[chain.length - 1].style! : null, font: s.font ?? info.body.font, size: s.size ?? info.body.size };
}

/**
 * The style, font and size of the main selection's text (a cursor: where
 * it is), for the row: a field that differs within it (also from styled
 * text inside it) is undefined.
 */
export function styleAt(state: EditorState, info: TextStyles): StyleState {
  const range = state.selection.main;
  if (range.empty) return stateOf(info, holding(lineSpans(state.doc.lineAt(range.head)), range.head, range.head));
  let out: StyleState | null = null;
  for (const seg of segments(state, range.from, range.to)) {
    const spans = lineSpans(seg.line);
    const here = [stateOf(info, holding(spans, seg.from, seg.to))];
    for (const s of spans) {
      if (s.format === "styled" && s.from < seg.to && seg.from < s.to && !(s.from <= seg.from && seg.to <= s.to)) here.push(stateOf(info, holding(spans, s.contentFrom, s.contentTo)));
    }
    for (const h of here) {
      if (!out) {
        out = h;
        continue;
      }
      const merged: StyleState = {
        id: out.id === h.id ? out.id : undefined,
        font: out.font !== undefined && h.font !== undefined && sameFont(out.font, h.font) ? out.font : undefined,
        size: out.size !== undefined && h.size !== undefined && sameSize(out.size, h.size) ? out.size : undefined,
      };
      // Lines of a multi-line formula or code don't count (as for restyling).
      if ((merged.id !== out.id || merged.font !== out.font || merged.size !== out.size) && modeAt(state, seg.from) !== "markup") break;
      out = merged;
    }
    // Settled (a long selection is not read to its end).
    if (out && out.id === undefined && out.font === undefined && out.size === undefined) break;
  }
  return out ?? stateOf(info, []);
}

// ---- The editor ----

/** The inline CSS of a style's text: its font, its size relative to the body's (`bodyPt`) on the editor's text size. */
export function styledCss(style: Settings, bodyPt: number | null): string {
  const css: string[] = [];
  if (style.font) css.push(`font-family: ${cssFamily(style.font)}`);
  const pt = toPt(style.size);
  if (pt !== null && bodyPt) css.push(`font-size: calc(var(--text-size) * ${Math.round((pt / bodyPt) * 1000) / 1000})`);
  return css.join("; ");
}

/**
 * Shows the content of `#styled("<id>")[…]` (its `]` on the line) in the
 * style's font at its size relative to the body (absolute, so nested spans
 * don't multiply); a missing style's content is marked. Visible lines only.
 */
export function styledText(info: TextStyles): Extension {
  const bodyPt = toPt(info.body.size);
  const marks = new Map<string, Decoration>();
  const mark = (id: string) => {
    let m = marks.get(id);
    if (!m) {
      const style = info.styles[id];
      m = style
        ? Decoration.mark({ class: "cm-styled", attributes: { style: styledCss(style, bodyPt) } })
        : Decoration.mark({ class: "cm-styled-missing", attributes: { title: `Missing style “${id}”` } });
      marks.set(id, m);
    }
    return m;
  };
  const build = (view: EditorView): DecorationSet => {
    const ranges: Range<Decoration>[] = [];
    const { doc } = view.state;
    let next = 0;
    for (const { from, to } of view.visibleRanges) {
      for (let pos = Math.max(from, next); pos <= to; ) {
        const line = doc.lineAt(pos);
        if (line.text.includes("#styled(")) {
          for (const s of lineSpans(line)) if (s.format === "styled" && s.contentTo > s.contentFrom) ranges.push(mark(s.style!).range(s.contentFrom, s.contentTo));
        }
        pos = next = line.to + 1;
      }
    }
    return Decoration.set(ranges, true);
  };
  return ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;
      constructor(view: EditorView) {
        this.decorations = build(view);
      }
      update(update: ViewUpdate) {
        if (update.docChanged || update.viewportChanged) this.decorations = build(update.view);
      }
    },
    { decorations: (v) => v.decorations },
  );
}
