// Rendered math for the science view: formulas outside the selection show as
// images (SVG from the core's Typst renderer, as `<img>` data URLs — the SVG
// markup never enters the DOM); with the cursor in or next to a formula its
// source stays editable and a tooltip shows a live preview.
//
// Only spans near the viewport are found (via the syntax tree) and rendered.
// Decorations live in a StateField (replacements may cover line breaks, which
// view plugins may not do); a view plugin feeds it the viewport and style and
// asks the cache for renders. No Tauri imports: the renderer is injected.

import { syntaxTree } from "@codemirror/language";
import { type EditorState, type Extension, type Range, StateEffect, StateField } from "@codemirror/state";
import { Decoration, type DecorationSet, EditorView, showTooltip, type Tooltip, type TooltipView, ViewPlugin, type ViewUpdate, WidgetType } from "@codemirror/view";
import { KIND, nodeKind } from "./typst.ts";

// ---- Finding math spans ----

export interface MathSpan {
  /** Position of the opening `$`. */
  from: number;
  /** Position after the closing `$`. */
  to: number;
  /** The text between the delimiters. */
  source: string;
  display: boolean;
}

/** Typst: an equation is display math when its content starts and ends with whitespace. */
export const isDisplay = (source: string) => source.length >= 2 && /^\s/.test(source) && /\s$/.test(source);

/** How far before/after the range to look for the delimiters of spans crossing its edges. */
const MARGIN = 4000;

/** The closed math spans overlapping `from..to` (as far as the document is parsed). */
export function findMathSpans(state: EditorState, from: number, to: number): MathSpan[] {
  const spans: MathSpan[] = [];
  const { doc } = state;
  let open = -1;
  syntaxTree(state).iterate({
    from: Math.max(0, from - MARGIN),
    to: Math.min(doc.length, to + MARGIN),
    enter(node) {
      const { flags } = nodeKind(node.type);
      if (flags & KIND.mathOpen) open = node.from;
      else if (flags & KIND.mathClose && open >= 0) {
        if (node.to >= from && open <= to) {
          const source = doc.sliceString(open + 1, node.to - 1);
          spans.push({ from: open, to: node.to, source, display: isDisplay(source) });
        }
        open = -1;
      }
    },
  });
  return spans;
}

/** The math span with `pos` strictly inside (between its delimiters), if any. */
export function mathAt(state: EditorState, pos: number): MathSpan | null {
  return findMathSpans(state, pos, pos).find((s) => s.from < pos && pos < s.to) ?? null;
}

// ---- Render cache ----

export interface MathStyle {
  /** Text size in pt. */
  sizePt: number;
  /** `#rrggbb`. */
  color: string;
}

export interface MathRequest extends MathStyle {
  source: string;
  display: boolean;
}

/** What the `render_math` command returns (sizes in pt, baseline from the top). */
export type MathReply = { ok: { svg: string; width: number; height: number; baseline: number } } | { error: string };

export type Rendered = { url: string; width: number; height: number; baseline: number } | { error: string };

export type RenderFn = (request: MathRequest) => Promise<MathReply>;

export const mathKey = (r: MathRequest) => `${r.display ? "D" : "I"}|${r.sizePt}|${r.color}|${r.source}`;

export const svgDataUrl = (svg: string) => `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;

/**
 * Rendered formulas by request (bounded, least recently used dropped), with
 * deduplicated requests and at most `concurrency` renders in flight. Requests
 * not yet started are replaced by each `want` call, so scrolling past many
 * formulas never piles up work.
 */
export class MathCache {
  private render: RenderFn;
  private onResult: () => void;
  private capacity: number;
  private concurrency: number;
  private done = new Map<string, Rendered>();
  private running = new Set<string>();
  private queue: MathRequest[] = [];

  constructor(render: RenderFn, onResult: () => void, options: { capacity?: number; concurrency?: number } = {}) {
    this.render = render;
    this.onResult = onResult;
    this.capacity = options.capacity ?? 400;
    this.concurrency = options.concurrency ?? 4;
  }

  /** The finished result, if any (marks it as recently used). */
  get(request: MathRequest): Rendered | undefined {
    const key = mathKey(request);
    const hit = this.done.get(key);
    if (hit) {
      this.done.delete(key);
      this.done.set(key, hit);
    }
    return hit;
  }

  /** Renders these (in priority order) unless done or running; drops queued requests not listed. */
  want(requests: MathRequest[]): void {
    const seen = new Set<string>();
    this.queue = requests.filter((r) => {
      const key = mathKey(r);
      if (seen.has(key) || this.done.has(key) || this.running.has(key)) return false;
      seen.add(key);
      return true;
    });
    this.pump();
  }

  /** Requests in flight and waiting (for tests). */
  get pending(): { running: number; queued: number } {
    return { running: this.running.size, queued: this.queue.length };
  }

  get size(): number {
    return this.done.size;
  }

  private pump(): void {
    while (this.running.size < this.concurrency && this.queue.length > 0) this.start(this.queue.shift()!);
  }

  private start(request: MathRequest): void {
    const key = mathKey(request);
    this.running.add(key);
    let result: Promise<MathReply>;
    try {
      result = this.render(request);
    } catch (e) {
      result = Promise.reject(e);
    }
    result
      .then(
        (reply): Rendered =>
          "ok" in reply
            ? { url: svgDataUrl(reply.ok.svg), width: reply.ok.width, height: reply.ok.height, baseline: reply.ok.baseline }
            : { error: reply.error },
        (e): Rendered => ({ error: e instanceof Error ? e.message : String(e) }),
      )
      .then((rendered) => {
        this.running.delete(key);
        this.done.set(key, rendered);
        while (this.done.size > this.capacity) this.done.delete(this.done.keys().next().value!);
        this.pump();
        this.onResult();
      });
  }
}

// ---- Colours ----

/** `#rrggbb` from a CSS colour in `#rgb`, `#rrggbb` or `rgb()` form; null otherwise. */
export function toHex(css: string): string | null {
  const s = css.trim().toLowerCase();
  if (/^#[0-9a-f]{6}$/.test(s)) return s;
  if (/^#[0-9a-f]{3}$/.test(s)) return `#${s[1]}${s[1]}${s[2]}${s[2]}${s[3]}${s[3]}`;
  const m = /^rgba?\(\s*(\d+)[\s,]+(\d+)[\s,]+(\d+)/.exec(s);
  if (!m) return null;
  return `#${m.slice(1, 4).map((n) => Math.min(255, Number(n)).toString(16).padStart(2, "0")).join("")}`;
}

// ---- Editor extension ----

const PX_PER_PT = 4 / 3;
const PREVIEW_DELAY_MS = 150;

const setRange = StateEffect.define<{ from: number; to: number }>();
const setStyle = StateEffect.define<MathStyle>();
const rendered = StateEffect.define<null>();

interface MathView {
  /** The document range searched for spans (viewport plus slack), null until the view reports it. */
  range: { from: number; to: number } | null;
  style: MathStyle | null;
  spans: MathSpan[];
  /** The span the main selection touches. */
  active: MathSpan | null;
  deco: DecorationSet;
  tooltip: Tooltip | null;
}

const request = (span: MathSpan, style: MathStyle): MathRequest => ({ source: span.source, display: span.display, ...style });

const touches = (state: EditorState, span: MathSpan) => state.selection.ranges.some((r) => r.from <= span.to && r.to >= span.from);

class MathWidget extends WidgetType {
  readonly image: { url: string; width: number; height: number; baseline: number };
  readonly source: string;
  readonly display: boolean;

  constructor(image: MathWidget["image"], source: string, display: boolean) {
    super();
    this.image = image;
    this.source = source;
    this.display = display;
  }

  eq(other: MathWidget): boolean {
    return other.image === this.image && other.display === this.display;
  }

  toDOM(view: EditorView): HTMLElement {
    const wrap = document.createElement("span");
    wrap.className = this.display ? "cm-math-render cm-math-display" : "cm-math-render";
    const img = document.createElement("img");
    img.src = this.image.url;
    img.alt = this.source;
    img.title = this.source.trim();
    img.draggable = false;
    img.width = Math.max(1, Math.round(this.image.width * PX_PER_PT));
    img.height = Math.max(1, Math.round(this.image.height * PX_PER_PT));
    if (!this.display) img.style.verticalAlign = `${-((this.image.height - this.image.baseline) * PX_PER_PT)}px`;
    wrap.append(img);
    // Clicking a formula puts the cursor into its source.
    wrap.addEventListener("mousedown", (e) => {
      if (e.button !== 0) return;
      e.preventDefault();
      const pos = view.posAtDOM(wrap);
      view.dispatch({ selection: { anchor: Math.min(pos + 1, view.state.doc.length) }, userEvent: "select.pointer" });
      view.focus();
    });
    return wrap;
  }

  ignoreEvent(): boolean {
    return true;
  }
}

/**
 * The science view's rendered math. `render` calls the core (`render_math`);
 * one instance can serve successive editor states (its cache persists).
 */
export function mathRendering(render: RenderFn): Extension {
  let notify: (() => void) | null = null;
  const cache = new MathCache(render, () => notify?.());

  function decorations(state: EditorState, spans: MathSpan[], style: MathStyle | null): DecorationSet {
    if (!style) return Decoration.none;
    const ranges: Range<Decoration>[] = [];
    for (const span of spans) {
      if (!span.source.trim() || touches(state, span)) continue;
      const r = cache.get(request(span, style));
      if (!r) continue; // pending: the styled source stays
      if ("error" in r) {
        ranges.push(Decoration.mark({ class: "cm-math-error", attributes: { title: r.error } }).range(span.from, span.to));
      } else {
        ranges.push(Decoration.replace({ widget: new MathWidget(r, span.source, span.display) }).range(span.from, span.to));
      }
    }
    return Decoration.set(ranges, true);
  }

  const field: StateField<MathView> = StateField.define<MathView>({
    create: () => ({ range: null, style: null, spans: [], active: null, deco: Decoration.none, tooltip: null }),
    update(value, tr) {
      let { range, style } = value;
      let respan = false;
      let redeco = !!tr.selection;
      if (tr.docChanged && range) {
        range = { from: tr.changes.mapPos(range.from, -1), to: tr.changes.mapPos(range.to, 1) };
        respan = true;
      }
      for (const e of tr.effects) {
        if (e.is(setRange)) {
          range = e.value;
          respan = true;
        } else if (e.is(setStyle)) {
          style = e.value;
          redeco = true;
        } else if (e.is(rendered)) {
          redeco = true;
        }
      }
      if (range && syntaxTree(tr.state) !== syntaxTree(tr.startState)) respan = true;
      if (!respan && !redeco) return tr.docChanged ? { ...value, deco: value.deco.map(tr.changes) } : value;

      const state = tr.state;
      const spans = respan && range ? findMathSpans(state, range.from, range.to) : value.spans;
      const main = state.selection.main;
      const active = spans.find((s) => main.from <= s.to && main.to >= s.from && s.source.trim()) ?? null;
      let tooltip: Tooltip | null = null;
      if (active) {
        const old = value.tooltip;
        // Same object while the formula's start stays put, so the preview isn't rebuilt per keystroke.
        tooltip =
          old && old.pos === active.from && tr.changes.mapPos(old.pos) === old.pos
            ? old
            : { pos: active.from, above: true, strictSide: false, create: createPreview };
      }
      return { range, style, spans, active, deco: decorations(state, spans, style), tooltip };
    },
    provide: (f) => [EditorView.decorations.from(f, (v) => v.deco), showTooltip.from(f, (v) => v.tooltip)],
  });

  function createPreview(view: EditorView): TooltipView {
    const dom = document.createElement("div");
    dom.className = "cm-math-preview";
    dom.hidden = true;
    const img = document.createElement("img");
    img.alt = "";
    const message = document.createElement("div");
    message.className = "cm-math-preview-error";
    dom.append(img, message);
    let shown: Rendered | null = null;
    const show = (state: EditorState) => {
      const { active, style } = state.field(field);
      const r = active && style ? cache.get(request(active, style)) : undefined;
      if (!r || r === shown) return; // pending: keep the last preview
      shown = r;
      dom.hidden = false;
      if ("error" in r) {
        img.hidden = true;
        message.hidden = false;
        message.textContent = r.error;
      } else {
        message.hidden = true;
        img.hidden = false;
        img.src = r.url;
        img.width = Math.max(1, Math.round(r.width * PX_PER_PT));
        img.height = Math.max(1, Math.round(r.height * PX_PER_PT));
      }
    };
    show(view.state);
    return { dom, update: (u) => show(u.state) };
  }

  const plugin = ViewPlugin.fromClass(
    class {
      view: EditorView;
      style: MathStyle | null = null;
      timer: ReturnType<typeof setTimeout> | undefined;
      syncTimer: ReturnType<typeof setTimeout> | undefined;
      renderedTimer: ReturnType<typeof setTimeout> | undefined;
      /** The active formula's request key and since when it has been unchanged. */
      activeKey = "";
      activeSince = 0;
      media = window.matchMedia("(prefers-color-scheme: dark)");
      onTheme = () => {
        this.style = null;
        this.schedule();
      };

      notify = () => {
        if (this.renderedTimer === undefined) {
          this.renderedTimer = setTimeout(() => {
            this.renderedTimer = undefined;
            if (this.view.state.field(field, false)) this.view.dispatch({ effects: rendered.of(null) });
          }, 0);
        }
      };

      constructor(view: EditorView) {
        this.view = view;
        notify = this.notify;
        this.media.addEventListener("change", this.onTheme);
        this.schedule();
      }

      update(u: ViewUpdate) {
        // The text size may have changed (Settings → Editor): the formulas follow.
        if (u.geometryChanged) this.style = null;
        if (u.docChanged || u.viewportChanged || u.geometryChanged || u.selectionSet || u.transactions.some((tr) => tr.effects.length > 0)) this.schedule();
      }

      schedule() {
        if (this.syncTimer === undefined) {
          this.syncTimer = setTimeout(() => {
            this.syncTimer = undefined;
            this.sync();
          }, 0);
        }
      }

      readStyle(): MathStyle {
        const px = parseFloat(getComputedStyle(this.view.contentDOM).fontSize) || 19.5;
        const ink = getComputedStyle(document.documentElement).getPropertyValue("--ink");
        const color = toHex(ink) ?? toHex(getComputedStyle(this.view.contentDOM).color) ?? "#000000";
        return { sizePt: Math.round(px * 75) / 100, color };
      }

      sync() {
        const value = this.view.state.field(field, false);
        if (!value) return;
        this.style ??= this.readStyle();
        const effects: StateEffect<unknown>[] = [];
        const vp = this.view.viewport;
        const r = value.range;
        if (!r || vp.from < r.from || vp.to > r.to || r.to - r.from > 3 * (vp.to - vp.from) + 4000) {
          effects.push(setRange.of({ from: Math.max(0, vp.from - 1000), to: Math.min(this.view.state.doc.length, vp.to + 1000) }));
        }
        const s = value.style;
        if (!s || s.sizePt !== this.style.sizePt || s.color !== this.style.color) effects.push(setStyle.of(this.style));
        if (effects.length) {
          this.view.dispatch({ effects }); // update() schedules the next sync
          return;
        }

        const wanted: MathRequest[] = [];
        const { active } = value;
        const style = this.style;
        if (active) {
          const req = request(active, style);
          const key = mathKey(req);
          const now = Date.now();
          if (key !== this.activeKey) {
            this.activeKey = key;
            this.activeSince = now;
          }
          if (!cache.get(req)) {
            if (now - this.activeSince >= PREVIEW_DELAY_MS) wanted.push(req);
            else {
              clearTimeout(this.timer);
              this.timer = setTimeout(() => this.schedule(), PREVIEW_DELAY_MS - (now - this.activeSince));
            }
          }
        } else {
          this.activeKey = "";
        }
        const state = this.view.state;
        for (const span of value.spans) {
          if (span.to < vp.from || span.from > vp.to || !span.source.trim() || touches(state, span)) continue;
          wanted.push(request(span, style));
        }
        cache.want(wanted);
      }

      destroy() {
        clearTimeout(this.timer);
        clearTimeout(this.syncTimer);
        clearTimeout(this.renderedTimer);
        this.media.removeEventListener("change", this.onTheme);
        if (notify === this.notify) notify = null;
      }
    },
  );

  return [field, plugin];
}
