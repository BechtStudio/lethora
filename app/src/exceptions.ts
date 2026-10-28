// Anchored overrides ("exceptions" to the layout) in the publisher view: the
// store that loads and changes them, their labels and states, their
// parameter fields, and the Exceptions summary of the layout panel (the list
// itself is the drawer, exceptionsdrawer.ts).
//
// They are made and edited in the publisher view only (layering rule); the
// science view just shows figure placements as badges. An image quality
// changes only the exported PDF, never the pages. No Tauri imports: commands come in through `OverrideApi`. The
// labels, states and the store are pure and tested under `node --test`.

import type { CompileStatus, OverrideStatus } from "./status.ts";

export type OverrideKind = "page_break" | "column_break" | "space_before" | "figure_placement" | "image_quality" | "table_style";
export type Placement = "top" | "bottom" | "here" | "auto";
export const PLACEMENTS: readonly Placement[] = ["top", "bottom", "here", "auto"];
/** Resolution of images in the exported PDF (the layout's `export.image_quality`). */
export type ImageQuality = "original" | "print" | "screen" | "email";
export const QUALITIES: readonly ImageQuality[] = ["original", "print", "screen", "email"];
export const QUALITY_LABELS: Record<ImageQuality, string> = { original: "Original", print: "Print", screen: "Screen", email: "Email" };
/** The quality's label, or the value itself if it is not one. */
export const qualityLabel = (q: unknown): string => QUALITY_LABELS[q as ImageQuality] ?? String(q ?? "");
/** A table's look (the layout's `tables.style`, or a table's own). */
export type TableStyle = "plain" | "academic" | "grid" | "striped";
export const TABLE_STYLES: readonly TableStyle[] = ["plain", "academic", "grid", "striped"];
export const TABLE_STYLE_LABELS: Record<TableStyle, string> = { plain: "Plain", academic: "Academic", grid: "Grid", striped: "Striped" };
/** The style's label, or the value itself if it is not one. */
export const tableStyleLabel = (s: unknown): string => TABLE_STYLE_LABELS[s as TableStyle] ?? String(s ?? "");
export type Params = Record<string, unknown>;

/** What `overrides_get` lists. */
export interface OverrideInfo {
  id: string;
  /** `null`: the stored entries are invalid (see `error`). */
  kind: OverrideKind | null;
  params: Params;
  anchor_preview: string;
  pos_utf16: number | null;
  detached: boolean;
  /** Whether the last compile applied it (`null`: not compiled since). */
  applicable: boolean | null;
  error: string | null;
}

/** Where a click on a page leads (`page_jump`). */
export type JumpTarget =
  | { in: "content"; pos_utf16: number; start_utf16: number; end_utf16: number; block_start_utf16: number }
  | { in: "override"; id: string }
  | { in: "none" };

export interface JumpResult {
  target: JumpTarget;
  /** The figure call the click is in, if any (UTF-16). */
  figure_utf16: number | null;
  /** The image call the click is in, or the first in the clicked figure (UTF-16). */
  image_utf16: number | null;
  /** The table call the click is in, or the first in the clicked figure (UTF-16). */
  table_utf16: number | null;
}

export const isBlockKind = (kind: OverrideKind | null) => kind === "page_break" || kind === "column_break" || kind === "space_before";

/** "Page break · right-hand page", "Space before 18pt", "Figure: top", "Image quality: Screen", "Table: Grid". */
export function overrideLabel(o: Pick<OverrideInfo, "kind" | "params">): string {
  switch (o.kind) {
    case "page_break":
      return o.params.to_odd === true ? "Page break · right-hand page" : "Page break";
    case "column_break":
      return "Column break";
    case "space_before":
      return `Space before ${String(o.params.length ?? "")}`.trim();
    case "figure_placement":
      return `Figure: ${String(o.params.placement ?? "")}`;
    case "image_quality":
      return `Image quality: ${qualityLabel(o.params.quality)}`;
    case "table_style":
      return `Table: ${tableStyleLabel(o.params.style)}`;
    default:
      return "Invalid exception";
  }
}

/** A backend error without the `kind.param: ` prefix, capitalized. */
export function overrideError(e: unknown): string {
  const m = (e instanceof Error ? e.message : String(e)).replace(/^(page_break|column_break|space_before|figure_placement|image_quality|table_style)(\.\w+)?:\s*/, "");
  return m.charAt(0).toUpperCase() + m.slice(1);
}

export type BadgeKind = "detached" | "not-applied" | "invalid";

export interface Badge {
  kind: BadgeKind;
  text: string;
  title: string;
}

export interface RowState {
  label: string;
  badges: Badge[];
  /** Why it is not applied or invalid (shown under the row). */
  reason: string | null;
  /** Has parameters to edit (invalid ones can only be removed). */
  editable: boolean;
}

const NOT_APPLIED = "Not applied at this position.";

/**
 * How a row shows: the last compile's status (if any) wins over the listing;
 * `messages` are the diagnostics pointing at this override.
 */
export function rowState(o: OverrideInfo, status?: OverrideStatus, messages: readonly string[] = []): RowState {
  const label = overrideLabel(o);
  if (o.error !== null || o.kind === null) {
    const error = o.error ? overrideError(o.error) : "This exception is not valid.";
    return { label, badges: [{ kind: "invalid", text: "invalid", title: error }], reason: error, editable: false };
  }
  const badges: Badge[] = [];
  let reason: string | null = null;
  if (status?.detached ?? o.detached) {
    badges.push({ kind: "detached", text: "detached", title: "The text it was anchored to was deleted; it still applies at the nearest spot." });
  }
  if ((status ? status.applied : o.applicable) === false) {
    reason = messages.find((m) => /not applied/i.test(m)) ?? messages.find((m) => !/was deleted/i.test(m)) ?? NOT_APPLIED;
    badges.push({ kind: "not-applied", text: "not applied", title: reason });
  }
  return { label, badges, reason, editable: o.kind !== "column_break" };
}

// ---- The store ----

/** One operation of a batch (`overrides_batch`). */
export type BatchOp = { op: "update"; id: string; params: Params } | { op: "remove"; id: string };

export interface OverrideApi {
  list(): Promise<OverrideInfo[]>;
  add(kind: OverrideKind, params: Params, posUtf16: number): Promise<string>;
  remove(id: string): Promise<void>;
  update(id: string, params: Params): Promise<void>;
  /** Updates and removes several as one undo step (all or nothing). */
  batch(ops: BatchOp[]): Promise<void>;
  /** Sends pending text edits first, so undo order matches what the user did. */
  drain(): Promise<void>;
}

/**
 * The overrides as last listed, with the last compile's statuses and
 * messages. Requests run one at a time; every change drains the edit queue
 * first and reloads the list after (also when it fails).
 */
export class OverrideStore {
  list: OverrideInfo[] = [];
  private statuses = new Map<string, OverrideStatus>();
  private messages = new Map<string, string[]>();
  private chain: Promise<unknown> = Promise.resolve();
  private api: OverrideApi;
  private listeners = new Set<() => void>();
  /** Bumped by `reset`: answers for an earlier project are dropped. */
  private epoch = 0;

  constructor(api: OverrideApi) {
    this.api = api;
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  find(id: string): OverrideInfo | undefined {
    return this.list.find((o) => o.id === id);
  }

  label(id: string): string | null {
    const o = this.find(id);
    return o ? overrideLabel(o) : null;
  }

  state(o: OverrideInfo): RowState {
    return rowState(o, this.statuses.get(o.id), this.messages.get(o.id));
  }

  /** Reloads the list. */
  refresh(): Promise<void> {
    const epoch = this.epoch;
    return this.run(() => this.load(epoch));
  }

  /** A `compile-status` event: statuses and messages of that compile, then a reload. */
  onStatus(s: CompileStatus): void {
    if (s.state !== "done" && s.state !== "failed") return;
    this.statuses = new Map((s.overrides ?? []).map((o) => [o.id, o]));
    this.messages = new Map();
    for (const d of s.diagnostics) {
      if (d.location.in !== "override") continue;
      const list = this.messages.get(d.location.id) ?? [];
      list.push(d.message);
      this.messages.set(d.location.id, list);
    }
    this.emit();
    void this.refresh().catch(() => {});
  }

  add(kind: OverrideKind, params: Params, posUtf16: number): Promise<string> {
    return this.change(() => this.api.add(kind, params, posUtf16));
  }

  remove(id: string): Promise<void> {
    return this.change(() => this.api.remove(id));
  }

  update(id: string, params: Params): Promise<void> {
    return this.change(() => this.api.update(id, params));
  }

  batch(ops: BatchOp[]): Promise<void> {
    return this.change(() => this.api.batch(ops));
  }

  /** Another project: forget everything. */
  reset(): void {
    this.epoch++;
    this.list = [];
    this.statuses.clear();
    this.messages.clear();
    this.emit();
  }

  private change<T>(op: () => Promise<T>): Promise<T> {
    const epoch = this.epoch;
    return this.run(async () => {
      await this.api.drain();
      try {
        return await op();
      } catch (e) {
        throw new Error(overrideError(e));
      } finally {
        await this.load(epoch).catch(() => {});
      }
    });
  }

  private async load(epoch: number): Promise<void> {
    const list = await this.api.list();
    if (epoch !== this.epoch) return;
    this.list = list;
    this.emit();
  }

  private run<T>(op: () => Promise<T>): Promise<T> {
    const next = this.chain.then(op);
    this.chain = next.catch(() => {});
    return next;
  }

  private emit(): void {
    for (const l of this.listeners) l();
  }
}

// ---- Parameter fields and the layout panel's summary ----

function el<K extends keyof HTMLElementTagNameMap>(tag: K, props: Partial<HTMLElementTagNameMap[K]> = {}, ...children: Array<Node | string>): HTMLElementTagNameMap[K] {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...children);
  return e;
}

const PLACEMENT_LABELS: Record<Placement, string> = { top: "Top", bottom: "Bottom", here: "Here (in the text)", auto: "Auto" };

export interface ParamEditorOptions {
  /** Prefix of the controls' ids (unique per place the editor shows). */
  idPrefix: string;
  /** The length's text as last typed (`null`: the stored value). */
  draft: string | null;
  /** The last apply's error, shown under the fields. */
  error: string | null;
  onDraft(text: string): void;
  /** Applies new parameters (a change, Enter or Apply). */
  apply(params: Params): void;
  /** Esc in a control. */
  cancel(): void;
}

/**
 * The fields that edit `o`'s parameters (none for a column break or an
 * invalid one) and the error line: the Exceptions drawer's inline and bulk
 * editors and the page view's edit popover.
 */
export function paramEditor(o: OverrideInfo, opts: ParamEditorOptions): HTMLElement {
  const box = el("div", { className: "ex-edit" });
  const error = el("p", { className: "lp-error", hidden: opts.error === null, textContent: opts.error ?? "" });
  error.setAttribute("role", "alert");
  const apply = opts.apply;
  const escape = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    e.preventDefault();
    e.stopPropagation();
    opts.cancel();
  };
  const prefix = opts.idPrefix;
  if (o.kind === "page_break") {
    const id = `${prefix}-odd-${o.id}`;
    const box2 = el("input", { type: "checkbox", id, className: "lp-switch", checked: o.params.to_odd === true });
    box2.setAttribute("role", "switch");
    box2.dataset.focusKey = `${o.id}:edit-control`;
    box2.addEventListener("change", () => apply({ to_odd: box2.checked }));
    box2.addEventListener("keydown", escape);
    box.append(el("label", { htmlFor: id, textContent: "Start on a right-hand page" }), box2);
  } else if (o.kind === "space_before") {
    const id = `${prefix}-length-${o.id}`;
    const input = el("input", { type: "text", id, className: "lp-length", autocomplete: "off", spellcheck: false, value: opts.draft ?? String(o.params.length ?? "") });
    input.dataset.focusKey = `${o.id}:edit-control`;
    input.setAttribute("aria-describedby", `${id}-error`);
    if (opts.error !== null) input.setAttribute("aria-invalid", "true");
    error.id = `${id}-error`;
    input.addEventListener("input", () => opts.onDraft(input.value));
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        apply({ length: input.value.trim() });
      } else escape(e);
    });
    const ok = el("button", { type: "button", className: "ex-action", textContent: "Apply" });
    ok.addEventListener("click", () => apply({ length: input.value.trim() }));
    box.append(el("label", { htmlFor: id, textContent: "Space" }), input, ok);
  } else if (o.kind === "figure_placement") {
    const id = `${prefix}-place-${o.id}`;
    const select = el("select", { id });
    select.dataset.focusKey = `${o.id}:edit-control`;
    select.append(...PLACEMENTS.map((p) => el("option", { value: p, textContent: PLACEMENT_LABELS[p] })));
    select.value = String(o.params.placement ?? "auto");
    select.addEventListener("change", () => apply({ placement: select.value }));
    select.addEventListener("keydown", escape);
    box.append(el("label", { htmlFor: id, textContent: "Placement" }), select);
  } else if (o.kind === "image_quality") {
    const id = `${prefix}-quality-${o.id}`;
    const select = el("select", { id });
    select.dataset.focusKey = `${o.id}:edit-control`;
    select.append(...QUALITIES.map((q) => el("option", { value: q, textContent: QUALITY_LABELS[q] })));
    select.value = String(o.params.quality ?? "original");
    select.addEventListener("change", () => apply({ quality: select.value }));
    select.addEventListener("keydown", escape);
    box.append(el("label", { htmlFor: id, textContent: "Image quality" }), select);
  } else if (o.kind === "table_style") {
    const id = `${prefix}-table-${o.id}`;
    const select = el("select", { id });
    select.dataset.focusKey = `${o.id}:edit-control`;
    select.append(...TABLE_STYLES.map((t) => el("option", { value: t, textContent: TABLE_STYLE_LABELS[t] })));
    select.value = String(o.params.style ?? "plain");
    select.addEventListener("change", () => apply({ style: select.value }));
    select.addEventListener("keydown", escape);
    box.append(el("label", { htmlFor: id, textContent: "Table style" }), select);
  }
  return el("div", { className: "ex-editor" }, box, error);
}

/** Whether a row needs a look: detached, not applied or invalid. */
export const needsAttention = (state: RowState): boolean => state.badges.length > 0;

/**
 * The "Exceptions" group of the layout panel: how many there are, how many
 * need attention, and a button that opens the drawer listing them.
 */
export class ExceptionsSummary {
  readonly el: HTMLDetailsElement;
  private store: OverrideStore;
  private count: HTMLElement;
  private total: HTMLElement;
  private attention: HTMLElement;
  private open: HTMLButtonElement;
  private empty: HTMLElement;

  constructor(store: OverrideStore, openDrawer: () => void) {
    this.store = store;
    this.count = el("span", { className: "lp-count" });
    this.total = el("p", { className: "lp-note ex-total" });
    this.attention = el("p", { className: "lp-note ex-attention" });
    this.open = el("button", { type: "button", className: "ex-open-drawer", textContent: "Open (F7)", title: "List, filter and edit the exceptions in a drawer (F7)" });
    this.open.addEventListener("click", openDrawer);
    this.empty = el("p", { className: "lp-note ex-empty", textContent: "None. Right-click a page to add a page break, space before a block, a figure placement, an image quality or a table style." });
    this.el = el("details", { className: "lp-group lp-exceptions", open: true }, el("summary", {}, "Exceptions", this.count), this.empty, this.total, this.attention, this.open);
    store.subscribe(() => this.render());
    this.render();
  }

  private render(): void {
    const items = this.store.list;
    const attention = items.filter((o) => needsAttention(this.store.state(o))).length;
    this.count.textContent = items.length ? ` · ${items.length}` : "";
    this.empty.hidden = items.length > 0;
    this.total.hidden = this.open.hidden = items.length === 0;
    this.total.textContent = `${items.length.toLocaleString("en-US")} ${items.length === 1 ? "exception" : "exceptions"}`;
    this.attention.hidden = attention === 0;
    this.attention.textContent = `Needs attention: ${attention.toLocaleString("en-US")}`;
  }
}
