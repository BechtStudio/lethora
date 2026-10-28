// Citations, labels and cross-references: the labels in the document (cached
// per document version, scanned lazily), `@` completion of source keys and
// labels (both views), and in the science view a hover card per reference
// and a subtle underline under references that match nothing.
// No Tauri imports: sources and formatting come in through `RefHost`.

import { acceptCompletion, autocompletion, type Completion, type CompletionContext, type CompletionResult, type CompletionSource } from "@codemirror/autocomplete";
import { syntaxTree } from "@codemirror/language";
import { EditorSelection, type EditorState, type Extension, Prec, type Range, StateEffect, type Text, type TransactionSpec } from "@codemirror/state";
import { Decoration, type DecorationSet, EditorView, hoverTooltip, type KeyBinding, keymap, ViewPlugin, type ViewUpdate } from "@codemirror/view";
import { type BibEntry, entryDetail } from "./bib.ts";
import { scanFigure } from "./figures.ts";
import { codePointBefore, wordy } from "./typst.ts";

// ---- Labels ----

export type LabelKind = "heading" | "equation" | "figure" | "table" | "other";

export interface DocLabel {
  name: string;
  kind: LabelKind;
  /** Position of the `<`. */
  pos: number;
  /** Heading text, figure caption, equation source, or the line (other). */
  text: string;
  /** Equations: display math. */
  display?: boolean;
}

export const KIND_NAMES: Record<LabelKind, string> = { heading: "Heading", equation: "Equation", figure: "Figure", table: "Table", other: "Label" };

const LABEL_OR_FIGURE = /<([\p{L}\p{N}_][\p{L}\p{N}_\-.:]*)>|#figure\(/gu;
const HEADING = /^(=+)\s+(.*)$/;

/** The text of the line around `i`. */
function lineAround(text: string, i: number): { start: number; end: number } {
  const start = text.lastIndexOf("\n", i - 1) + 1;
  const nl = text.indexOf("\n", i);
  return { start, end: nl < 0 ? text.length : nl };
}

/** The opening `$` of the equation closed by the `$` at `close` (escapes skipped), or -1. */
function mathOpen(text: string, close: number): number {
  for (let i = text.lastIndexOf("$", close - 1); i >= 0 && close - i < 8000; i = text.lastIndexOf("$", i - 1)) {
    let slashes = 0;
    while (text.charCodeAt(i - 1 - slashes) === 92) slashes++;
    if (slashes % 2 === 0) return i;
  }
  return -1;
}

/**
 * All labels in `text` with what they label: a heading (label at the end of
 * a heading line), an equation (label right after `$…$`), a figure (right
 * after `#figure(...)`; a table if its body is a `table` call, as Typst
 * decides), else "other". One regex pass; labels inside raw or
 * comments are not told apart (cheap and good enough for completion).
 */
export function scanLabels(text: string): DocLabel[] {
  const out: DocLabel[] = [];
  let figureEnd = -1;
  let figureCaption = "";
  let figureKind: LabelKind = "figure";
  LABEL_OR_FIGURE.lastIndex = 0;
  for (let m = LABEL_OR_FIGURE.exec(text); m; m = LABEL_OR_FIGURE.exec(text)) {
    const at = m.index;
    if (m[1] === undefined) {
      const span = scanFigure(text, at);
      if (span) {
        figureEnd = span.callEnd;
        figureCaption = span.caption ?? "";
        figureKind = /^\(\s*table\s*[([]/.test(text.slice(at + 7, at + 40)) ? "table" : "figure";
      }
      continue;
    }
    const name = m[1];
    let j = at;
    while (j > 0 && (text.charCodeAt(j - 1) === 32 || text.charCodeAt(j - 1) === 9)) j--;
    const before = text.charCodeAt(j - 1);
    if (j === figureEnd) {
      out.push({ name, kind: figureKind, pos: at, text: figureCaption });
      continue;
    }
    if (before === 36 /* $ */ && text.charCodeAt(j - 2) !== 92) {
      const open = mathOpen(text, j - 1);
      if (open >= 0) {
        const source = text.slice(open + 1, j - 1);
        const display = source.length >= 2 && /^\s/.test(source) && /\s$/.test(source);
        out.push({ name, kind: "equation", pos: at, text: source.trim(), display });
        continue;
      }
    }
    const line = lineAround(text, at);
    const lineText = text.slice(line.start, line.end);
    const heading = HEADING.exec(lineText);
    if (heading && !text.slice(LABEL_OR_FIGURE.lastIndex, line.end).trim()) {
      const title = lineText.slice(heading[1].length, at - line.start).trim();
      out.push({ name, kind: "heading", pos: at, text: title });
      continue;
    }
    out.push({ name, kind: "other", pos: at, text: lineText.trim().slice(0, 120) });
  }
  return out;
}

export interface LabelIndex {
  doc: Text;
  list: DocLabel[];
  /** By name (the first occurrence wins). */
  byName: Map<string, DocLabel>;
}

let lastIndex: LabelIndex | null = null;

/** The document's labels, scanned once per document version (the last one is cached). */
export function labelsFor(doc: Text): LabelIndex {
  if (lastIndex?.doc === doc) return lastIndex;
  const list = scanLabels(doc.toString());
  const byName = new Map<string, DocLabel>();
  for (const l of list) if (!byName.has(l.name)) byName.set(l.name, l);
  lastIndex = { doc, list, byName };
  return lastIndex;
}

// ---- `@` context ----

const KEY_CHAR = /[\p{L}\p{N}_\-.:]/u;
/** What completion keeps filtering locally while typing. */
export const VALID_FOR = /^[\p{L}\p{N}_\-.:]*$/u;

/** Token names (from the Typst tokenizer) where `@` is no reference. */
const NO_REFS = /(^|[_ ])(raw|comment|code|math|mathOpen|mathClose|label)([_ ]|$)/;

/** Whether the syntax tree allows a reference starting at `pos` (not in raw, comments, code, math or labels). */
export function refAllowedAt(state: EditorState, pos: number): boolean {
  const tree = syntaxTree(state);
  if (tree.length < pos) return true;
  const node = tree.resolveInner(pos, 1);
  return !NO_REFS.test(node.name);
}

export interface RefContext {
  /** Position after the `@`. */
  from: number;
  /** The key typed so far. */
  text: string;
}

/** The reference being typed before `pos` (`@` + key characters), or null (emails, raw, code, comments, math). */
export function refContext(state: EditorState, pos: number): RefContext | null {
  const line = state.doc.lineAt(pos);
  const head = pos - line.from;
  const text = line.text;
  const at = text.lastIndexOf("@", head - 1);
  if (at < 0 || head - at > 100) return null;
  for (let i = at + 1; i < head; ) {
    const cp = text.codePointAt(i)!;
    if (!KEY_CHAR.test(String.fromCodePoint(cp))) return null;
    i += cp > 0xffff ? 2 : 1;
  }
  if (wordy(codePointBefore(text, at))) return null;
  if (!refAllowedAt(state, line.from + at)) return null;
  return { from: line.from + at + 1, text: text.slice(at + 1, head) };
}

// ---- Completion ----

const sourceKeys = new WeakMap<readonly BibEntry[], Map<string, BibEntry>>();

/** Sources by key (cached per entries array). */
export function entriesByKey(entries: readonly BibEntry[]): Map<string, BibEntry> {
  let map = sourceKeys.get(entries);
  if (!map) {
    map = new Map(entries.map((e) => [e.key, e]));
    sourceKeys.set(entries, map);
  }
  return map;
}

const SOURCES = { name: "Sources", rank: 0 };
const LABELS = { name: "Labels", rank: 1 };

/** Completion options: sources (key, "Author Year", title), then labels (name, kind, text). */
export function completionOptions(entries: readonly BibEntry[], labels: readonly DocLabel[]): Completion[] {
  const out: Completion[] = [];
  for (const e of entries) {
    out.push({ label: e.key, detail: entryDetail(e), info: e.title ?? undefined, type: "source", section: SOURCES });
  }
  const seen = new Set<string>();
  for (const l of labels) {
    if (seen.has(l.name)) continue;
    seen.add(l.name);
    out.push({ label: l.name, detail: KIND_NAMES[l.kind], info: l.text || undefined, type: `label-${l.kind}`, section: LABELS });
  }
  return out;
}

/** The `@` completion source; `entries` gives the current sources. */
export function refCompletionSource(entries: () => readonly BibEntry[]): CompletionSource {
  return (ctx: CompletionContext): CompletionResult | null => {
    const c = refContext(ctx.state, ctx.pos);
    if (!c) return null;
    const options = completionOptions(entries(), labelsFor(ctx.state.doc).list);
    if (options.length === 0) return null;
    return { from: c.from, options, validFor: VALID_FOR };
  };
}

const completionTheme = EditorView.theme({
  ".cm-tooltip.cm-tooltip-autocomplete": {
    backgroundColor: "var(--paper-raised)",
    color: "var(--ink)",
    border: "1px solid var(--rule)",
    borderRadius: "6px",
    boxShadow: "var(--shadow)",
    fontFamily: "var(--ui)",
    fontSize: "13px",
    overflow: "hidden",
  },
  ".cm-tooltip.cm-tooltip-autocomplete > ul": { fontFamily: "var(--ui)", maxHeight: "16em", minWidth: "16em", maxWidth: "min(34rem, 80vw)" },
  ".cm-tooltip.cm-tooltip-autocomplete > ul > li": { padding: "0.2em 0.7em", lineHeight: "1.5" },
  ".cm-tooltip.cm-tooltip-autocomplete > ul > li[aria-selected]": { backgroundColor: "var(--chip)", color: "var(--accent-ink)" },
  ".cm-tooltip.cm-tooltip-autocomplete > ul > completion-section": {
    padding: "0.35em 0.7em 0.15em",
    fontSize: "11px",
    letterSpacing: "0.04em",
    textTransform: "uppercase",
    color: "var(--ink-faint)",
    borderBottom: "1px solid var(--rule)",
  },
  ".cm-completionLabel": { fontFamily: "var(--mono)", fontSize: "0.95em" },
  ".cm-completionMatchedText": { textDecoration: "none", color: "var(--accent-ink)", fontWeight: "600" },
  ".cm-completionDetail": { marginLeft: "0.8em", fontStyle: "normal", color: "var(--ink-soft)" },
  ".cm-tooltip.cm-completionInfo": {
    backgroundColor: "var(--paper-raised)",
    color: "var(--ink-soft)",
    border: "1px solid var(--rule)",
    borderRadius: "6px",
    boxShadow: "var(--shadow)",
    fontFamily: "var(--serif)",
    fontSize: "14px",
    padding: "0.4em 0.7em",
    maxWidth: "22rem",
  },
});

/** Tab accepts the selected completion like Enter (only while the list is open; else Tab is untouched). */
export const completionKeys: readonly KeyBinding[] = [{ key: "Tab", run: acceptCompletion }];

/** `@` completion of source keys and document labels (both views). */
export function refCompletion(entries: () => readonly BibEntry[]): Extension {
  return [
    autocompletion({ override: [refCompletionSource(entries)], icons: false, activateOnTyping: true, maxRenderedOptions: 200, aboveCursor: false }),
    Prec.highest(keymap.of([...completionKeys])),
    completionTheme,
  ];
}

// ---- Inserting a citation ----

/** `@key` after the main selection, with a space before unless at a line start, after whitespace, `(` or `[`. */
export function insertCitation(state: EditorState, key: string): TransactionSpec {
  const pos = state.selection.main.to;
  const prev = state.sliceDoc(Math.max(0, pos - 1), pos);
  const space = prev === "" || /[\s([]/.test(prev) ? "" : " ";
  const insert = `${space}@${key}`;
  return {
    changes: { from: pos, insert },
    selection: EditorSelection.cursor(pos + insert.length),
    scrollIntoView: true,
    userEvent: "input",
  };
}

// ---- References in the text ----

export interface RefSpan {
  /** Position of the `@`. */
  from: number;
  to: number;
  key: string;
}

/** The citation tokens overlapping `from..to` (as far as the document is parsed). */
export function findRefs(state: EditorState, from: number, to: number): RefSpan[] {
  const out: RefSpan[] = [];
  syntaxTree(state).iterate({
    from,
    to,
    enter(node) {
      if (/(^|[_ ])cite([_ ]|$)/.test(node.name)) out.push({ from: node.from, to: node.to, key: state.sliceDoc(node.from + 1, node.to) });
    },
  });
  return out;
}

/** The references matching neither a source key nor a label. */
export function unknownRefs(refs: RefSpan[], known: (key: string) => boolean): RefSpan[] {
  return refs.filter((r) => !known(r.key));
}

// ---- Science view: hover cards and unknown references ----

export interface RefHost {
  entries(): readonly BibEntry[];
  /** Whether the sources are loaded (until then nothing is marked unknown). */
  loaded(): boolean;
  /** The source as a formatted reference in the selected style. */
  format(key: string): Promise<string>;
  /** A rendered equation (CSS px), or null. */
  renderEquation(source: string, display: boolean): Promise<{ url: string; width: number; height: number } | null>;
  /** Calls `listener` when the sources change; returns the unsubscribe function. */
  subscribe(listener: () => void): () => void;
}

const refsChanged = StateEffect.define<null>();
const unknownMark = Decoration.mark({ class: "cm-ref-unknown", attributes: { title: "Unknown reference" } });
const LABEL_SCAN_DELAY_MS = 400;

function hoverCard(host: RefHost, state: EditorState, key: string): HTMLElement {
  const dom = document.createElement("div");
  dom.className = "cm-ref-card";
  const head = document.createElement("div");
  head.className = "cm-ref-card-head";
  const body = document.createElement("div");
  body.className = "cm-ref-card-body";
  dom.append(head, body);
  const entry = entriesByKey(host.entries()).get(key);
  if (entry) {
    head.textContent = entryDetail(entry) || key;
    body.textContent = "…";
    host.format(key).then(
      (text) => (body.textContent = text),
      (e) => {
        body.textContent = e instanceof Error ? e.message : String(e);
        body.classList.add("cm-ref-card-error");
      },
    );
    return dom;
  }
  const label = labelsFor(state.doc).byName.get(key);
  if (label) {
    head.textContent = KIND_NAMES[label.kind];
    if (label.kind === "equation" && label.text) {
      const source = document.createElement("code");
      source.textContent = label.text;
      body.append(source);
      host.renderEquation(label.text, !!label.display).then(
        (r) => {
          if (!r) return;
          const img = document.createElement("img");
          img.src = r.url;
          img.alt = label.text;
          img.width = Math.max(1, Math.round(r.width));
          img.height = Math.max(1, Math.round(r.height));
          source.replaceWith(img);
        },
        () => {},
      );
    } else {
      body.textContent = label.text || `<${label.name}>`;
    }
    return dom;
  }
  head.textContent = "Unknown reference";
  body.textContent = `No source or label is called “${key}”.`;
  body.classList.add("cm-ref-card-muted");
  return dom;
}

/** Hover cards for references and the unknown-reference underline (science view). */
export function refScience(host: RefHost): Extension {
  const hover = hoverTooltip(
    (view, pos, side) => {
      const tree = syntaxTree(view.state);
      if (tree.length < pos) return null;
      const node = tree.resolveInner(pos, side);
      if (!/(^|[_ ])cite([_ ]|$)/.test(node.name)) return null;
      const key = view.state.sliceDoc(node.from + 1, node.to);
      return { pos: node.from, end: node.to, above: true, create: () => ({ dom: hoverCard(host, view.state, key) }) };
    },
    // Close on edits, so a card never lingers over the text or a drawer.
    { hoverTime: 350, hideOnChange: true },
  );

  const unknown = ViewPlugin.fromClass(
    class {
      view: EditorView;
      decorations: DecorationSet = Decoration.none;
      labelNames: Set<string> | null = null;
      timer: ReturnType<typeof setTimeout> | undefined;
      unsubscribe: () => void;

      constructor(view: EditorView) {
        this.view = view;
        this.unsubscribe = host.subscribe(() => this.changed());
        this.scan(0);
      }

      changed() {
        this.view.dispatch({ effects: refsChanged.of(null) });
      }

      /** Rescans the labels after a pause (never per keystroke). */
      scan(delay: number) {
        clearTimeout(this.timer);
        this.timer = setTimeout(() => {
          const names = new Set(labelsFor(this.view.state.doc).byName.keys());
          const old = this.labelNames;
          if (!old || old.size !== names.size || [...names].some((n) => !old.has(n))) {
            this.labelNames = names;
            this.changed();
          }
        }, delay);
      }

      update(u: ViewUpdate) {
        if (u.docChanged) this.scan(LABEL_SCAN_DELAY_MS);
        if (
          u.docChanged ||
          u.viewportChanged ||
          syntaxTree(u.state) !== syntaxTree(u.startState) ||
          u.transactions.some((tr) => tr.effects.some((e) => e.is(refsChanged)))
        ) {
          this.decorations = this.build();
        }
      }

      build(): DecorationSet {
        const labels = this.labelNames;
        if (!labels || !host.loaded()) return Decoration.none;
        const keys = entriesByKey(host.entries());
        const known = (key: string) => keys.has(key) || labels.has(key);
        const ranges: Range<Decoration>[] = [];
        for (const { from, to } of this.view.visibleRanges) {
          for (const r of unknownRefs(findRefs(this.view.state, from, to), known)) ranges.push(unknownMark.range(r.from, r.to));
        }
        return Decoration.set(ranges, true);
      }

      destroy() {
        clearTimeout(this.timer);
        this.unsubscribe();
      }
    },
    { decorations: (v) => v.decorations },
  );

  const theme = EditorView.theme({
    ".cm-ref-unknown": {
      textDecoration: "underline wavy var(--ink-faint)",
      textDecorationSkipInk: "none",
      textUnderlineOffset: "0.25em",
    },
    ".cm-tooltip.cm-tooltip-hover": {
      backgroundColor: "var(--paper-raised)",
      color: "var(--ink)",
      border: "1px solid var(--rule)",
      borderRadius: "6px",
      boxShadow: "var(--shadow)",
    },
    ".cm-ref-card": { padding: "0.45rem 0.7rem", maxWidth: "min(30rem, 85vw)" },
    ".cm-ref-card-head": { fontFamily: "var(--ui)", fontSize: "11.5px", color: "var(--ink-faint)", marginBottom: "0.2rem" },
    ".cm-ref-card-body": { fontFamily: "var(--serif)", fontSize: "14.5px", lineHeight: "1.45", overflowX: "auto" },
    ".cm-ref-card-body code": { fontFamily: "var(--mono)", fontSize: "12.5px", whiteSpace: "pre-wrap" },
    ".cm-ref-card-body img": { display: "block" },
    ".cm-ref-card-error": { color: "var(--error)", fontFamily: "var(--ui)", fontSize: "12.5px" },
    ".cm-ref-card-muted": { color: "var(--ink-soft)", fontFamily: "var(--ui)", fontSize: "12.5px" },
  });

  return [hover, unknown, theme];
}
