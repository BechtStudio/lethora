// The Exceptions drawer of the page view (F7, or "Exceptions" in the
// Publisher tab's bar and the split pane's head): every exception in
// document order, grouped by chapter (level-1 heading), with its page,
// label, anchored text and state. Filters by kind, "Needs attention" and
// search; a multi-select tree for the keyboard (Up/Down with the pages
// following, Enter edits, Delete removes) and the mouse (Ctrl/Shift+click);
// bulk remove and "Set value…" go through `batch` as one undo step.
//
// In the Publisher tab it covers the layout panel, in the split pane the
// editor beside the pages. Grouping, filtering, ordering and range
// selection are pure and tested under `node --test`.

import { needsAttention, type OverrideInfo, type OverrideKind, type OverrideStore, paramEditor, type Params, type RowState } from "./exceptions.ts";

// ---- Pure parts ----

/** A level-1 heading: where its line starts (UTF-16), its text, a key that survives edits elsewhere. */
export interface Chapter {
  pos: number;
  title: string;
  key: string;
}

const CHAPTER = /^=[ \t]+(.*?)(?:\s*<[^<>]*>)?\s*$/;

/** The level-1 headings (`= ` lines) of `text`, outside raw blocks. */
export function chapterStarts(text: string): Chapter[] {
  const out: Chapter[] = [];
  const seen = new Map<string, number>();
  let raw = false;
  for (let start = 0; start <= text.length; ) {
    let end = text.indexOf("\n", start);
    if (end < 0) end = text.length;
    const line = text.slice(start, end);
    const fence = line.trimStart();
    if (fence.startsWith("```")) {
      // A fence that closes on the same line is raw text of its own.
      if (!fence.slice(3).includes("```")) raw = !raw;
    } else if (!raw && line.charCodeAt(0) === 61) {
      const m = CHAPTER.exec(line);
      if (m) {
        const title = m[1] || "Untitled chapter";
        const n = (seen.get(title) ?? 0) + 1;
        seen.set(title, n);
        out.push({ pos: start, title, key: `ch:${n}:${title}` });
      }
    }
    start = end + 1;
  }
  return out;
}

/** Index of the chapter holding UTF-16 offset `pos` (-1: before the first). */
export function chapterAt(chapters: readonly Chapter[], pos: number): number {
  let lo = 0;
  let hi = chapters.length - 1;
  let found = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (chapters[mid].pos <= pos) {
      found = mid;
      lo = mid + 1;
    } else hi = mid - 1;
  }
  return found;
}

export interface RowGroup {
  key: string;
  title: string;
  items: OverrideInfo[];
}

export const BEFORE_KEY = "before";
export const INVALID_KEY = "invalid";

/**
 * `list` in document order, grouped under its chapters (only those with
 * exceptions); before the first one in "Before the first chapter", those
 * without a position (invalid) last.
 */
export function groupByChapter(list: readonly OverrideInfo[], chapters: readonly Chapter[]): RowGroup[] {
  const sorted = [...list].sort((a, b) => Number(a.pos_utf16 === null) - Number(b.pos_utf16 === null) || (a.pos_utf16 ?? 0) - (b.pos_utf16 ?? 0) || a.id.localeCompare(b.id));
  const groups: RowGroup[] = [];
  let at = -2;
  for (const o of sorted) {
    const index = o.pos_utf16 === null ? -3 : chapterAt(chapters, o.pos_utf16);
    if (index !== at || groups.length === 0) {
      at = index;
      groups.push(
        index === -3
          ? { key: INVALID_KEY, title: "Invalid", items: [] }
          : index === -1
            ? { key: BEFORE_KEY, title: "Before the first chapter", items: [] }
            : { key: chapters[index].key, title: chapters[index].title, items: [] },
      );
    }
    groups[groups.length - 1].items.push(o);
  }
  return groups;
}

/** The kind chips: breaks (page and column), space, figure placement, image quality, table style. */
export type KindFilter = "break" | "space" | "figure" | "image" | "table";
export const KIND_FILTERS: ReadonlyArray<{ id: KindFilter; label: string; kinds: readonly OverrideKind[] }> = [
  { id: "break", label: "Break", kinds: ["page_break", "column_break"] },
  { id: "space", label: "Space", kinds: ["space_before"] },
  { id: "figure", label: "Figure", kinds: ["figure_placement"] },
  { id: "image", label: "Image", kinds: ["image_quality"] },
  { id: "table", label: "Table", kinds: ["table_style"] },
];

export interface Filter {
  /** None: every kind (invalid ones too). */
  kinds: ReadonlySet<KindFilter>;
  attention: boolean;
  query: string;
}

export const isFiltered = (f: Filter): boolean => f.kinds.size > 0 || f.attention || f.query.trim() !== "";

/** Whether `o` (shown as `state`) passes `f`: any chosen kind, needing attention, the query in its anchored text or label. */
export function matches(o: OverrideInfo, state: RowState, f: Filter): boolean {
  if (f.kinds.size > 0 && !KIND_FILTERS.some((k) => f.kinds.has(k.id) && o.kind !== null && k.kinds.includes(o.kind))) return false;
  if (f.attention && !needsAttention(state)) return false;
  const q = f.query.trim().toLowerCase();
  return q === "" || o.anchor_preview.toLowerCase().includes(q) || state.label.toLowerCase().includes(q);
}

/** The ids from `from` to `to` in `order`, both included (just `to` if `from` is not there). */
export function rangeIds(order: readonly string[], from: string | null, to: string): string[] {
  const a = from === null ? -1 : order.indexOf(from);
  const b = order.indexOf(to);
  if (b < 0) return [];
  if (a < 0) return [to];
  return order.slice(Math.min(a, b), Math.max(a, b) + 1);
}

/** Whether the overrides of `ids` can take one value together: two or more, all of one kind with parameters. */
export function bulkKind(list: readonly OverrideInfo[], ids: ReadonlySet<string>): OverrideKind | null {
  if (ids.size < 2) return null;
  let kind: OverrideKind | null = null;
  let n = 0;
  for (const o of list) {
    if (!ids.has(o.id)) continue;
    if (o.kind === null || o.kind === "column_break" || o.error !== null || (kind !== null && o.kind !== kind)) return null;
    kind = o.kind;
    n++;
  }
  return n === ids.size ? kind : null;
}

// ---- The drawer ----

export interface DrawerHost {
  /** The current text (a new object per change, so headings are scanned once per version). */
  doc(): { toString(): string };
  /** Page (0-based) of each override on the pages shown (the last compile). */
  pages(): ReadonlyMap<string, number>;
  /** Scrolls the pages to override `id`'s marker and flashes it. */
  showOnPage(id: string): void;
  /** Shows UTF-16 offset `pos` in the text (the drawer has closed). */
  openInText(pos: number): void;
  /** The drawer opened or closed. */
  shown(open: boolean): void;
  /** It closed with the focus in it and nowhere to return it. */
  closed(): void;
  error(e: unknown): void;
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, props: Partial<HTMLElementTagNameMap[K]> = {}, ...children: Array<Node | string>): HTMLElementTagNameMap[K] {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...children);
  return e;
}

const groupKey = (key: string) => `g:${key}`;
const rowKey = (id: string) => `r:${id}`;
const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));
const count = (n: number) => n.toLocaleString("en-US");

export class ExceptionsDrawer {
  readonly el: HTMLElement;
  private store: OverrideStore;
  private host: DrawerHost;
  private open = false;
  private countEl: HTMLElement;
  private search: HTMLInputElement;
  private chips = new Map<KindFilter, HTMLButtonElement>();
  private attentionChip: HTMLButtonElement;
  private bulk: HTMLElement;
  private bulkText: HTMLElement;
  private bulkSet: HTMLButtonElement;
  private bulkEditor: HTMLElement;
  private scroller: HTMLElement;
  private tree: HTMLUListElement;
  private empty: HTMLElement;
  private filter: { kinds: Set<KindFilter>; attention: boolean; query: string } = { kinds: new Set(), attention: false, query: "" };
  private collapsed = new Set<string>();
  private selected = new Set<string>();
  /** Where a Shift+click range starts. */
  private anchor: string | null = null;
  /** The current item's key (`g:` group or `r:` row). */
  private current: string | null = null;
  /** The current item's index in `order`, for when it disappears. */
  private currentIndex = 0;
  /** Items in navigation order, and the rows' ids as shown. */
  private order: string[] = [];
  private shownIds: string[] = [];
  /** The row edited inline: rebuilt only for its own changes (others wait until it ends). */
  private editing: string | null = null;
  private draft: string | null = null;
  private editError: string | null = null;
  private bulkEditing = false;
  private bulkDraft: string | null = null;
  private bulkError: string | null = null;
  private chapterDoc: unknown = null;
  private chapters: Chapter[] = [];
  /** Where the focus was before opening. */
  private opener: HTMLElement | null = null;

  constructor(store: OverrideStore, host: DrawerHost, parent: HTMLElement) {
    this.store = store;
    this.host = host;
    const close = el("button", { type: "button", className: "ex-action exd-close", textContent: "×", title: "Close (Esc, F7)" });
    close.setAttribute("aria-label", "Close the exceptions");
    close.addEventListener("click", () => this.close());
    this.countEl = el("span", { className: "exd-count" });
    this.countEl.setAttribute("aria-live", "polite");
    const head = el("div", { className: "exd-head" }, el("h2", { className: "exd-title", textContent: "Exceptions" }), this.countEl, close);

    this.search = el("input", { type: "search", className: "exd-search", placeholder: "Search anchored text or label…", autocomplete: "off", spellcheck: false });
    this.search.setAttribute("aria-label", "Search the exceptions by anchored text or label");
    this.search.addEventListener("input", () => {
      this.filter.query = this.search.value;
      this.render();
    });
    // Down from the search box goes to the list.
    this.search.addEventListener("keydown", (e) => {
      if (e.key !== "ArrowDown" || e.altKey || e.ctrlKey || e.metaKey) return;
      e.preventDefault();
      this.focusCurrent();
    });
    const chips = el("div", { className: "exd-chips" });
    chips.setAttribute("role", "group");
    chips.setAttribute("aria-label", "Filter");
    for (const k of KIND_FILTERS) {
      const chip = this.chip(k.label, () => {
        if (!this.filter.kinds.delete(k.id)) this.filter.kinds.add(k.id);
      });
      this.chips.set(k.id, chip);
      chips.append(chip);
    }
    this.attentionChip = this.chip("Needs attention", () => (this.filter.attention = !this.filter.attention));
    this.attentionChip.classList.add("attention");
    this.attentionChip.title = "Only those detached, not applied or invalid";
    chips.append(this.attentionChip);
    const filters = el("div", { className: "exd-filters" }, this.search, chips);

    this.bulkText = el("span", { className: "exd-bulk-text" });
    const remove = el("button", { type: "button", className: "ex-action", textContent: "Remove", title: "Remove the selected exceptions (Delete; one undo step)" });
    remove.addEventListener("click", () => this.removeSelected());
    this.bulkSet = el("button", { type: "button", className: "ex-action", textContent: "Set value…", title: "Give all selected exceptions one value (one undo step)" });
    this.bulkSet.addEventListener("click", () => this.startBulkEdit());
    const clear = el("button", { type: "button", className: "ex-action", textContent: "Clear selection" });
    clear.addEventListener("click", () => {
      this.selected.clear();
      this.render();
      this.focusCurrent();
    });
    this.bulkEditor = el("div", { className: "exd-bulk-editor", hidden: true });
    this.bulk = el("div", { className: "exd-bulk", hidden: true }, el("div", { className: "exd-bulk-bar" }, this.bulkText, remove, this.bulkSet, clear), this.bulkEditor);

    this.tree = el("ul", { className: "exd-tree" });
    this.tree.setAttribute("role", "tree");
    this.tree.setAttribute("aria-multiselectable", "true");
    this.tree.setAttribute("aria-label", "Exceptions by chapter");
    this.tree.addEventListener("keydown", (e) => this.onKey(e));
    this.tree.addEventListener("click", (e) => this.onClick(e));
    this.empty = el("div", { className: "exd-empty", hidden: true });
    this.scroller = el("div", { className: "exd-scroll" }, this.tree, this.empty);

    this.el = el("aside", { className: "exd", hidden: true }, head, filters, this.bulk, this.scroller);
    this.el.setAttribute("aria-label", "Exceptions");
    parent.append(this.el);
    store.subscribe(() => this.render());
  }

  get isOpen(): boolean {
    return this.open;
  }

  toggle(): void {
    if (this.open) this.close();
    else this.show();
  }

  /** Opens it with the focus in the list (or the search box if the list is empty). */
  show(): void {
    if (!this.open) {
      const active = document.activeElement;
      this.opener = active instanceof HTMLElement && active !== document.body ? active : null;
      this.open = true;
      this.el.hidden = false;
      this.host.shown(true);
      this.render();
    }
    if (this.order.length) this.focusCurrent();
    else this.search.focus();
  }

  /** Closes it (`restore`: if the focus was in it, back where it was before). */
  close(restore = true): void {
    if (!this.open) return;
    const focused = this.el.contains(document.activeElement);
    this.open = false;
    this.el.hidden = true;
    this.stopEdit();
    this.stopBulkEdit();
    this.host.shown(false);
    if (!restore || !focused) return;
    const opener = this.opener;
    this.opener = null;
    if (opener?.isConnected && opener.offsetParent !== null) opener.focus({ preventScroll: true });
    else this.host.closed();
  }

  /** Esc: ends an inline or bulk edit, else closes it; true if it was open. */
  escape(): boolean {
    if (!this.open) return false;
    if (this.editing !== null) {
      const id = this.editing;
      this.stopEdit();
      this.render();
      this.focusItem(rowKey(id));
    } else if (this.bulkEditing) {
      this.stopBulkEdit();
      this.bulkSet.focus();
    } else this.close();
    return true;
  }

  /** Makes override `id` the current, selected row (a marker was clicked); the focus stays. */
  reveal(id: string): void {
    if (!this.open || !this.store.find(id)) return;
    this.stopEdit();
    this.selected = new Set([id]);
    this.anchor = id;
    this.current = rowKey(id);
    this.collapsed.delete(this.groupOf(id));
    this.render();
    this.itemEl(this.current)?.scrollIntoView({ block: "nearest" });
  }

  /** Another project: forget the selection and edits (the filters stay). */
  reset(): void {
    this.close(false);
    this.selected.clear();
    this.collapsed.clear();
    this.anchor = this.current = null;
    this.chapterDoc = null;
    this.chapters = [];
  }

  // ---- Rendering ----

  private chip(label: string, toggle: () => void): HTMLButtonElement {
    const chip = el("button", { type: "button", className: "exd-chip", textContent: label });
    chip.setAttribute("aria-pressed", "false");
    chip.addEventListener("click", () => {
      toggle();
      this.render();
    });
    return chip;
  }

  private chaptersNow(): Chapter[] {
    const doc = this.host.doc();
    if (doc !== this.chapterDoc) {
      this.chapterDoc = doc;
      this.chapters = chapterStarts(doc.toString());
    }
    return this.chapters;
  }

  private groupOf(id: string): string {
    const o = this.store.find(id);
    if (!o || o.pos_utf16 === null) return INVALID_KEY;
    const chapters = this.chaptersNow();
    const i = chapterAt(chapters, o.pos_utf16);
    return i < 0 ? BEFORE_KEY : chapters[i].key;
  }

  private render(): void {
    if (!this.open) return;
    const all = this.store.list;
    if (this.editing !== null && !this.store.find(this.editing)) this.stopEdit();
    const states = new Map(all.map((o) => [o.id, this.store.state(o)]));
    const shown = all.filter((o) => matches(o, states.get(o.id)!, this.filter));
    // Only what the filters show stays selected.
    const visible = new Set(shown.map((o) => o.id));
    for (const id of this.selected) if (!visible.has(id)) this.selected.delete(id);

    this.countEl.textContent = isFiltered(this.filter) ? `${count(shown.length)} of ${count(all.length)}` : all.length ? count(all.length) : "";
    for (const [id, chip] of this.chips) chip.setAttribute("aria-pressed", String(this.filter.kinds.has(id)));
    this.attentionChip.setAttribute("aria-pressed", String(this.filter.attention));
    this.renderBulk();

    // While a row is edited the list waits (its fields keep the focus and the text).
    if (this.editing !== null) return;
    const active = document.activeElement;
    const hadFocus = this.tree.contains(active);
    const refocus = hadFocus && active instanceof HTMLElement ? (active.dataset.focusKey ?? null) : null;
    const scrollTop = this.scroller.scrollTop;

    const groups = groupByChapter(shown, this.chaptersNow());
    const pages = this.host.pages();
    this.order = [];
    this.shownIds = [];
    const items: HTMLElement[] = [];
    for (const g of groups) {
      const open = !this.collapsed.has(g.key);
      this.order.push(groupKey(g.key));
      const rows: HTMLElement[] = [];
      if (open) {
        for (const o of g.items) {
          this.order.push(rowKey(o.id));
          this.shownIds.push(o.id);
          rows.push(this.row(o, states.get(o.id)!, pages.get(o.id)));
        }
      }
      items.push(this.group(g, open, rows));
    }
    // The current item stays, or the one now at its place.
    let index = this.current === null ? -1 : this.order.indexOf(this.current);
    if (index < 0 && this.order.length) {
      index = Math.min(this.currentIndex, this.order.length - 1);
      this.current = this.order[index];
    } else if (index < 0) this.current = null;
    this.currentIndex = Math.max(0, index);
    this.tree.replaceChildren(...items);
    this.markCurrent();

    this.empty.hidden = shown.length > 0;
    if (shown.length === 0) this.renderEmpty(all.length > 0);
    this.scroller.scrollTop = scrollTop;
    if (hadFocus) {
      const target = refocus ? this.tree.querySelector<HTMLElement>(`[data-focus-key="${CSS.escape(refocus)}"]`) : null;
      if (target) target.focus({ preventScroll: true });
      else this.focusCurrent();
    }
  }

  private renderEmpty(filtered: boolean): void {
    if (!filtered) {
      this.empty.replaceChildren(el("p", { textContent: "No exceptions yet. Right-click a page to add a page break, space before a block, a figure placement or an image quality." }));
      return;
    }
    const clear = el("button", { type: "button", className: "ex-action", textContent: "Clear the filters" });
    clear.addEventListener("click", () => {
      this.filter = { kinds: new Set(), attention: false, query: "" };
      this.search.value = "";
      this.render();
      this.focusCurrent();
    });
    this.empty.replaceChildren(el("p", { textContent: "No exceptions match." }), clear);
  }

  private group(g: RowGroup, open: boolean, rows: HTMLElement[]): HTMLElement {
    const key = groupKey(g.key);
    const head = el(
      "div",
      { className: "exd-ghead" },
      el("span", { className: "exd-caret", textContent: open ? "▾" : "▸" }),
      el("span", { className: "exd-gtitle", textContent: g.title }),
      el("span", { className: "exd-gcount", textContent: count(g.items.length) }),
    );
    const li = el("li", { className: "exd-group", tabIndex: -1 }, head);
    li.dataset.key = key;
    li.dataset.focusKey = key;
    li.setAttribute("role", "treeitem");
    li.setAttribute("aria-level", "1");
    li.setAttribute("aria-expanded", String(open));
    li.setAttribute("aria-label", `${g.title}, ${g.items.length} ${g.items.length === 1 ? "exception" : "exceptions"}`);
    if (open) {
      const list = el("ul", { className: "exd-rows" }, ...rows);
      list.setAttribute("role", "group");
      li.append(list);
    }
    return li;
  }

  private row(o: OverrideInfo, state: RowState, page: number | undefined): HTMLElement {
    const key = rowKey(o.id);
    const selected = this.selected.has(o.id);
    const invalid = state.badges.some((b) => b.kind === "invalid");
    const li = el("li", { className: `exd-row${selected ? " selected" : ""}${invalid ? " invalid" : ""}`, tabIndex: -1 });
    li.dataset.key = key;
    li.dataset.id = o.id;
    li.dataset.focusKey = key;
    li.setAttribute("role", "treeitem");
    li.setAttribute("aria-level", "2");
    li.setAttribute("aria-selected", String(selected));

    const pageEl = el("span", { className: "exd-page", textContent: page === undefined ? "–" : String(page + 1) });
    pageEl.title = page === undefined ? "Not on the pages shown" : `Page ${page + 1}`;
    const badges = el("span", { className: "ex-badges" });
    for (const b of state.badges) badges.append(el("span", { className: `ex-badge ${b.kind}`, textContent: b.text, title: b.title }));
    const anchor = el("span", { className: "ex-anchor", textContent: o.anchor_preview.trim() || (o.pos_utf16 === null ? "" : "(empty line)") });
    anchor.title = o.anchor_preview;
    const body = el("span", { className: "exd-body" }, el("span", { className: "ex-head" }, el("span", { className: "ex-label", textContent: state.label }), badges), anchor);
    li.setAttribute("aria-label", `${state.label}${page === undefined ? "" : `, page ${page + 1}`}${state.badges.map((b) => `, ${b.text}`).join("")}: ${o.anchor_preview.trim()}`);

    const actions = el("span", { className: "ex-actions" });
    const action = (text: string, name: string, label: string, run: () => void) => {
      const b = el("button", { type: "button", className: "ex-action", textContent: text, tabIndex: -1 });
      b.dataset.focusKey = `${o.id}:${name}`;
      b.dataset.action = name;
      b.setAttribute("aria-label", label);
      b.addEventListener("click", run);
      actions.append(b);
      return b;
    };
    if (state.editable) {
      action("Edit", "edit", `Edit ${state.label}`, () => {
        this.current = key;
        if (this.editing === o.id) this.escape();
        else this.startEdit(o.id);
      }).setAttribute("aria-expanded", String(this.editing === o.id));
    }
    if (o.pos_utf16 !== null) {
      const pos = o.pos_utf16;
      action("Show in text", "show", `Show ${state.label} in the text`, () => {
        this.close(false);
        this.host.openInText(pos);
      }).title = "Show where it is anchored in the text";
    }
    action("Remove", "remove", `Remove ${state.label}`, () => this.remove([o.id]));
    li.append(pageEl, body, actions);
    if (state.reason) li.append(el("p", { className: "ex-reason", textContent: state.reason }));
    if (this.editing === o.id) li.append(this.editor(o));
    return li;
  }

  /** The inline editor of `o`'s parameters. */
  private editor(o: OverrideInfo): HTMLElement {
    return paramEditor(o, {
      idPrefix: "exd",
      draft: this.draft,
      error: this.editError,
      onDraft: (text) => (this.draft = text),
      apply: (params) => {
        this.store.update(o.id, params).then(
          () => {
            if (this.editing !== o.id) return;
            this.stopEdit();
            this.render();
            this.focusItem(rowKey(o.id));
          },
          (e: unknown) => {
            if (this.editing !== o.id) return;
            this.editError = errorText(e);
            this.rebuildEdited();
          },
        );
      },
      cancel: () => this.escape(),
    });
  }

  private startEdit(id: string): void {
    this.stopBulkEdit();
    this.stopEdit();
    this.render();
    this.editing = id;
    this.rebuildEdited();
  }

  /** Redraws the edited row alone (its error changed), keeping the focus in its fields. */
  private rebuildEdited(): void {
    const id = this.editing;
    const o = id === null ? undefined : this.store.find(id);
    const old = id === null ? null : this.itemEl(rowKey(id));
    if (!o || !old) {
      this.stopEdit();
      this.render();
      return;
    }
    const li = this.row(o, this.store.state(o), this.host.pages().get(o.id));
    old.replaceWith(li);
    this.markCurrent();
    li.scrollIntoView({ block: "nearest" });
    li.querySelector<HTMLElement>(".ex-edit :is(input, select)")?.focus({ preventScroll: true });
  }

  private stopEdit(): void {
    this.editing = null;
    this.draft = null;
    this.editError = null;
  }

  // ---- Bulk ----

  private renderBulk(): void {
    const n = this.selected.size;
    this.bulk.hidden = n < 2;
    this.bulkText.textContent = `${count(n)} selected`;
    const kind = bulkKind(this.store.list, this.selected);
    this.bulkSet.hidden = kind === null;
    if (this.bulkEditing && kind === null) this.stopBulkEdit();
  }

  private startBulkEdit(): void {
    if (bulkKind(this.store.list, this.selected) === null) return;
    if (this.editing !== null) {
      this.stopEdit();
      this.render();
    }
    this.bulkEditing = true;
    this.bulkDraft = null;
    this.bulkError = null;
    this.renderBulkEditor();
    this.bulkEditor.querySelector<HTMLElement>(".ex-edit :is(input, select)")?.focus();
  }

  private renderBulkEditor(): void {
    const first = this.store.list.find((o) => this.selected.has(o.id));
    if (!first) {
      this.stopBulkEdit();
      return;
    }
    const n = this.selected.size;
    const editor = paramEditor(first, {
      idPrefix: "exd-bulk",
      draft: this.bulkDraft,
      error: this.bulkError,
      onDraft: (text) => (this.bulkDraft = text),
      apply: (params) => this.applyBulk(params),
      cancel: () => this.escape(),
    });
    this.bulkEditor.replaceChildren(el("p", { className: "exd-bulk-note", textContent: `One value for all ${count(n)}:` }), editor);
    this.bulkEditor.hidden = false;
  }

  private applyBulk(params: Params): void {
    const ops = [...this.selected].map((id) => ({ op: "update" as const, id, params }));
    this.store.batch(ops).then(
      () => {
        this.stopBulkEdit();
        this.focusCurrent();
      },
      (e: unknown) => {
        if (!this.bulkEditing) return;
        this.bulkError = errorText(e);
        this.renderBulkEditor();
        this.bulkEditor.querySelector<HTMLElement>(".ex-edit :is(input, select)")?.focus();
      },
    );
  }

  private stopBulkEdit(): void {
    this.bulkEditing = false;
    this.bulkDraft = null;
    this.bulkError = null;
    this.bulkEditor.hidden = true;
    this.bulkEditor.replaceChildren();
  }

  /** Removes the selected rows, or the current one (one undo step). */
  private removeSelected(): void {
    const ids = this.selected.size ? [...this.selected] : this.current?.startsWith("r:") ? [this.current.slice(2)] : [];
    this.remove(ids);
  }

  private remove(ids: string[]): void {
    if (ids.length === 0) return;
    if (this.editing !== null && ids.includes(this.editing)) this.stopEdit();
    const hadFocus = this.el.contains(document.activeElement);
    for (const id of ids) this.selected.delete(id);
    this.store.batch(ids.map((id) => ({ op: "remove" as const, id }))).then(() => {
      // The rows are gone: the focus goes to the one now at their place.
      if (hadFocus && (document.activeElement === document.body || !document.activeElement)) this.focusCurrent();
    }, this.host.error);
  }

  // ---- Current item, selection, keys ----

  private itemEl(key: string | null): HTMLElement | null {
    return key === null ? null : this.tree.querySelector<HTMLElement>(`[data-key="${CSS.escape(key)}"]`);
  }

  /** The current item takes Tab (and its row's buttons); the others only arrows. */
  private markCurrent(): void {
    for (const e of this.tree.querySelectorAll<HTMLElement>(".current")) {
      e.classList.remove("current");
      e.tabIndex = -1;
      for (const b of e.querySelectorAll<HTMLElement>(":scope > .ex-actions button")) b.tabIndex = -1;
    }
    const cur = this.itemEl(this.current) ?? this.itemEl(this.order[0] ?? null);
    if (!cur) return;
    cur.classList.add("current");
    cur.tabIndex = 0;
    for (const b of cur.querySelectorAll<HTMLElement>(":scope > .ex-actions button")) b.tabIndex = 0;
  }

  private focusItem(key: string | null): void {
    const e = this.itemEl(key);
    if (!e) return;
    e.focus({ preventScroll: true });
    e.scrollIntoView({ block: "nearest" });
  }

  private focusCurrent(): void {
    if (this.current === null && this.order.length) this.current = this.order[0];
    this.markCurrent();
    this.focusItem(this.current);
  }

  /** Makes `key` current; a row is selected alone (`extend`: the range from the anchor) and shown on the pages. */
  private moveTo(key: string, extend = false): void {
    this.current = key;
    this.currentIndex = Math.max(0, this.order.indexOf(key));
    if (key.startsWith("r:")) {
      const id = key.slice(2);
      if (extend) this.selected = new Set(rangeIds(this.shownIds, this.anchor, id));
      else {
        this.selected = new Set([id]);
        this.anchor = id;
      }
      this.host.showOnPage(id);
      this.updateSelection();
    }
    this.markCurrent();
    this.focusItem(key);
  }

  /** Shows the selection without rebuilding the rows. */
  private updateSelection(): void {
    for (const row of this.tree.querySelectorAll<HTMLElement>(".exd-row")) {
      const on = this.selected.has(row.dataset.id!);
      row.classList.toggle("selected", on);
      row.setAttribute("aria-selected", String(on));
    }
    this.renderBulk();
  }

  private setCollapsed(key: string, collapsed: boolean): void {
    const g = key.slice(2);
    if (collapsed === this.collapsed.has(g)) return;
    if (collapsed) this.collapsed.add(g);
    else this.collapsed.delete(g);
    this.current = key;
    this.render();
    this.focusItem(key);
  }

  private onKey(e: KeyboardEvent): void {
    const target = e.target as HTMLElement;
    // The inline editor's fields keep their keys.
    if (target.closest(".ex-editor")) return;
    const onButton = target instanceof HTMLButtonElement;
    const ctrl = e.ctrlKey || e.metaKey;
    if (e.altKey) return;
    // The focused item is the current one.
    const focused = target.closest<HTMLElement>("[data-key]")?.dataset.key;
    if (focused) this.current = focused;
    const key = this.current;
    const index = key === null ? -1 : this.order.indexOf(key);
    const isGroup = key?.startsWith("g:") ?? false;
    let handled = true;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      const next = this.order[Math.max(0, Math.min(this.order.length - 1, index + (e.key === "ArrowDown" ? 1 : -1)))];
      if (next) this.moveTo(next, e.shiftKey);
    } else if ((e.key === "Home" || e.key === "End") && !ctrl) {
      const next = e.key === "Home" ? this.order[0] : this.order[this.order.length - 1];
      if (next) this.moveTo(next, e.shiftKey);
    } else if (e.key === "ArrowLeft" && key !== null) {
      if (isGroup) this.setCollapsed(key, true);
      else {
        // To the row's chapter.
        for (let i = index; i >= 0; i--) {
          if (this.order[i].startsWith("g:")) {
            this.moveTo(this.order[i]);
            break;
          }
        }
      }
    } else if (e.key === "ArrowRight" && key !== null && isGroup) {
      this.setCollapsed(key, false);
    } else if (e.key === "Enter" && !onButton && key !== null) {
      if (isGroup) this.setCollapsed(key, !this.collapsed.has(key.slice(2)));
      else {
        const o = this.store.find(key.slice(2));
        if (o && this.store.state(o).editable) this.startEdit(o.id);
      }
    } else if (e.key === " " && !onButton && key !== null && !isGroup) {
      // Space toggles the current row in the selection.
      const id = key.slice(2);
      if (!this.selected.delete(id)) this.selected.add(id);
      this.anchor = id;
      this.updateSelection();
    } else if (e.key === "Delete") {
      this.removeSelected();
    } else if (ctrl && e.key.toLowerCase() === "a" && !e.shiftKey) {
      this.selected = new Set(this.shownIds);
      this.updateSelection();
    } else handled = false;
    if (handled) {
      e.preventDefault();
      e.stopPropagation();
    }
  }

  private onClick(e: MouseEvent): void {
    const target = e.target as HTMLElement;
    if (target.closest("button, input, select, label, .ex-editor")) return;
    const row = target.closest<HTMLElement>(".exd-row");
    if (!row) {
      const group = target.closest(".exd-ghead")?.parentElement?.dataset.key;
      if (group) this.setCollapsed(group, !this.collapsed.has(group.slice(2)));
      return;
    }
    const key = row.dataset.key!;
    const id = row.dataset.id!;
    if (e.shiftKey) {
      this.current = key;
      this.selected = new Set(rangeIds(this.shownIds, this.anchor, id));
      this.updateSelection();
      this.markCurrent();
      this.focusItem(key);
    } else if (e.ctrlKey || e.metaKey) {
      this.current = key;
      if (!this.selected.delete(id)) this.selected.add(id);
      this.anchor = id;
      this.updateSelection();
      this.markCurrent();
      this.focusItem(key);
    } else this.moveTo(key);
  }
}
