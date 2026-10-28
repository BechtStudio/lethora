// The Review panel (View → Review, or "Review" in the bar), right of the text
// in the author and science views: a card per review item beside its line,
// Word-style (pushed down to clear each other, the active card pulled to its
// line; see `stackCards`), moving with the editor's scroll. A card shows the
// thread (oldest first), the active one also the reply box and its actions
// (resolve or reopen, edit and delete messages, recolour, remove) as the
// rights allow (`itemRights`, `messageRights`), a suggestion's Accept and
// Reject (`suggestionRights`). Clicking a card selects its text; putting the
// caret in highlighted text activates its card. Highlights and suggestions
// show only while the panel does, for the items the filter shows; in suggest
// mode keystrokes show at once (`suggested`) until the core lists them.
//
// The pure parts are in `review.ts`. No Tauri imports: commands come in
// through `ReviewApi`, which sends pending edits first (see `Sync.call`).

import { EditorView, type ViewUpdate } from "@codemirror/view";
import {
  CARD_GAP,
  type CardBox,
  COLOUR_LABELS,
  DRAFT,
  FILTERS,
  isColour,
  isDone,
  itemRights,
  MARK_COLOURS,
  type MarkColour,
  markAt,
  marksOf,
  messageRights,
  passes,
  type Person,
  personOf,
  relativeTime,
  REVIEW_SELECT,
  type ReviewAccess,
  type ReviewAuthor,
  type ReviewFilter,
  type ReviewItem,
  type ReviewList,
  type ReviewMark,
  type ReviewMessage,
  reviewField,
  reviewState,
  setReviewActive,
  setReviewMarks,
  stackCards,
  suggestionAt,
  type Suggesting,
  suggestionRights,
  suggestionText,
} from "./review.ts";

/** A range of the editor's text, asked for when the request runs (edits typed meanwhile are sent first). */
export type EditorRange = () => { from: number; to: number } | null;

export interface ReviewApi {
  /** `review_list`, its offsets already mapped to the editor's text. */
  list(): Promise<ReviewList | null>;
  /** The members' names and colours by user (a server-shared project), else none. */
  people(): Promise<Map<string, Person>>;
  /** Resolve to the new item's id (null if nothing was sent). */
  comment(range: EditorRange, text: string): Promise<number | null>;
  mark(range: EditorRange, colour: MarkColour): Promise<number | null>;
  reply(item: number, text: string): Promise<unknown>;
  edit(item: number, message: number, text: string): Promise<unknown>;
  deleteMessage(item: number, message: number): Promise<unknown>;
  resolve(item: number, resolved: boolean): Promise<unknown>;
  colour(item: number, colour: MarkColour): Promise<unknown>;
  remove(item: number): Promise<unknown>;
  /** A `main` edit: the editor gets the change through the edit queue. */
  accept(item: number): Promise<unknown>;
  reject(item: number): Promise<unknown>;
}

export interface ReviewHost {
  view: EditorView;
  api: ReviewApi;
  /** The panel's × button. */
  close(): void;
  error(e: unknown): void;
}

/** How long after an edit the items are listed again (detached ones, other devices' items). */
export const REFRESH_DELAY_MS = 500;
const FILTER_KEY = "lethora.reviewFilter";

function el<K extends keyof HTMLElementTagNameMap>(tag: K, props: Partial<HTMLElementTagNameMap[K]> = {}, ...children: Array<Node | string>): HTMLElementTagNameMap[K] {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...children);
  return e;
}

/** A button; `key` finds it again after a render (the focus stays), `write` puts it under the read-only guard. */
function button(textContent: string, onClick: () => void, opts: { key?: string; write?: boolean; className?: string; title?: string } = {}): HTMLButtonElement {
  const b = el("button", { type: "button", textContent, className: opts.className ?? "", title: opts.title ?? "" });
  if (opts.key) b.dataset.key = opts.key;
  if (opts.write) b.dataset.write = "";
  b.addEventListener("click", onClick);
  return b;
}

function loadFilter(): ReviewFilter {
  try {
    const f = localStorage.getItem(FILTER_KEY);
    return f === "resolved" || f === "all" ? f : "open";
  } catch {
    return "open";
  }
}

export class ReviewPanel {
  private root: HTMLElement;
  private host: ReviewHost;
  private cardsEl: HTMLElement;
  private track: HTMLElement;
  private emptyEl: HTMLElement;
  private filterButtons = new Map<ReviewFilter, HTMLButtonElement>();

  private shown = false;
  private list: ReviewList | null = null;
  private people = new Map<string, Person>();
  private access: ReviewAccess = { main: true, review: true };
  private filter = loadFilter();
  /** The active card's item, as the editor's review field says. */
  private active: number | null = null;
  /** A new comment's card is open (its range is the field's `DRAFT` mark). */
  private draft = false;
  private editing: { item: number; message: number } | null = null;
  /** What is typed in the boxes, by box key (kept over renders). */
  private texts = new Map<string, string>();
  /** The item to activate once listed (just added). */
  private activateNext: number | null = null;
  private refreshTimer: ReturnType<typeof setTimeout> | undefined;
  /** Drops list replies overtaken by a newer request or a project switch. */
  private generation = 0;
  /** Suggest calls sent but not answered: lists replied meanwhile miss them (shown as `suggested` left them). */
  private unconfirmed = 0;

  constructor(root: HTMLElement, host: ReviewHost) {
    this.root = root;
    this.host = host;
    // Keeps the focus when the focused card goes (review undo): Ctrl+Z goes on in the panel.
    this.root.tabIndex = -1;
    const filters = el("div", { className: "rv-filter" });
    filters.setAttribute("role", "group");
    filters.setAttribute("aria-label", "Show");
    for (const { filter, label } of FILTERS) {
      const b = button(label, () => this.setFilter(filter));
      b.setAttribute("aria-pressed", "false");
      this.filterButtons.set(filter, b);
      filters.append(b);
    }
    const close = button("×", () => host.close(), { className: "rv-close", title: "Close the Review panel" });
    close.setAttribute("aria-label", "Close the Review panel");
    this.cardsEl = el("div", { className: "rv-cards" });
    this.track = el("div", { className: "rv-track" }, this.cardsEl);
    this.emptyEl = el("p", { className: "rv-empty" });
    root.append(el("div", { className: "rv-head" }, el("h2", { textContent: "Review" }), filters, close), this.emptyEl, this.track);

    this.cardsEl.addEventListener("click", (e) => {
      const target = e.target as HTMLElement;
      const card = target.closest<HTMLElement>(".rv-card");
      if (card && !target.closest("button, textarea")) this.select(Number(card.dataset.id));
    });
    this.cardsEl.addEventListener("keydown", (e) => {
      const target = e.target as HTMLElement;
      if (e.key === "Enter" && target.classList.contains("rv-card")) {
        e.preventDefault();
        this.select(Number(target.dataset.id));
      }
    });
    this.cardsEl.addEventListener("input", (e) => {
      const box = e.target as HTMLTextAreaElement;
      if (box.dataset.key) this.texts.set(box.dataset.key, box.value);
      const send = box.parentElement?.querySelector<HTMLButtonElement>("[data-send]");
      if (send) send.disabled = !box.value.trim();
    });
    // The cards move with the text: the wheel over them scrolls the editor.
    this.track.addEventListener(
      "wheel",
      (e) => {
        if ((e.target as HTMLElement).closest("textarea") || e.ctrlKey) return;
        host.view.scrollDOM.scrollTop += e.deltaY;
        e.preventDefault();
      },
      { passive: false },
    );
    host.view.scrollDOM.addEventListener("scroll", () => this.shift());
    this.showFilter();
  }

  get isShown(): boolean {
    return this.shown;
  }

  /** Whether review undo and redo can act (as last listed). */
  get canUndo(): boolean {
    return this.list?.undo ?? false;
  }

  get canRedo(): boolean {
    return this.list?.redo ?? false;
  }

  /** The items as last listed (or shown in suggest mode), offsets of the editor's text. */
  get current(): ReviewList | null {
    return this.list;
  }

  contains(target: EventTarget | null): boolean {
    return target instanceof Node && this.root.contains(target);
  }

  /** Shows the panel and the highlights (or hides both); shown, the items are listed anew. */
  setShown(on: boolean): void {
    if (on === this.shown) return;
    this.shown = on;
    this.root.hidden = !on;
    if (!on) {
      this.dropDraft();
      this.editing = null;
      this.reshow();
      return;
    }
    this.render();
    void this.refresh();
  }

  /** The role's rights (live role changes): a reader's write controls are disabled by the caller's guard. */
  setAccess(access: ReviewAccess): void {
    if (access.main === this.access.main && access.review === this.access.review) return;
    this.access = access;
    if (!access.review) {
      this.dropDraft();
      this.editing = null;
    }
    this.render();
  }

  /** Forgets the project (another one opens, or none). */
  reset(): void {
    this.generation++;
    clearTimeout(this.refreshTimer);
    this.list = null;
    this.people = new Map();
    this.draft = false;
    this.editing = null;
    this.texts.clear();
    this.activateNext = null;
    this.active = null;
    this.unconfirmed = 0;
    this.render();
  }

  /**
   * Suggest mode: an edit's suggest calls were sent (`calls` of them); the
   * items show as they leave them (if listed) until the core answers all
   * (`suggestSettled`) and lists them.
   */
  suggested(s: Suggesting): void {
    this.unconfirmed += s.calls.length;
    if (!s.items || !this.list) return;
    this.setList({ ...this.list, items: s.items }, true);
  }

  /** A suggest call was answered (or failed): once all are, the items are listed at once. */
  suggestSettled(): void {
    this.unconfirmed = Math.max(0, this.unconfirmed - 1);
    if (this.unconfirmed === 0) void this.refresh();
  }

  /** The open suggestion at `pos` shown in the text, with what this role may do with it. */
  suggestionAt(pos: number): { id: number; accept: boolean; why: string; reject: boolean } | null {
    const mark = suggestionAt(reviewState(this.host.view.state).marks, pos);
    const item = mark && mark.id > 0 ? this.list?.items.find((i) => i.id === mark.id) : undefined;
    return item ? { id: item.id, ...suggestionRights(item, this.list!.me, this.access) } : null;
  }

  accept(id: number): void {
    void this.write(() => this.host.api.accept(id));
  }

  reject(id: number): void {
    void this.write(() => this.host.api.reject(id));
  }

  /** Lists the items again after `delay` ms (the latest request wins). */
  requestRefresh(delay = REFRESH_DELAY_MS): void {
    clearTimeout(this.refreshTimer);
    if (this.shown) this.refreshTimer = setTimeout(() => void this.refresh(), delay);
  }

  /** Lists the items (with the members' names) and shows them. */
  async refresh(): Promise<void> {
    if (!this.shown) return;
    clearTimeout(this.refreshTimer);
    const generation = ++this.generation;
    try {
      const people = await this.host.api.people();
      if (generation !== this.generation) return;
      this.people = people;
      const list = await this.host.api.list();
      // Placed at once: the offsets match the editor's text right now. Suggestions sent after it are missing: the next list has them.
      if (generation !== this.generation || !list || this.unconfirmed > 0) return;
      this.setList(list);
    } catch (e) {
      this.host.error(e);
    }
  }

  /** The editor changed (call from its update listener): cards follow the text, the active card the caret. */
  update(u: ViewUpdate): void {
    const before = u.startState.field(reviewField, false);
    const after = reviewState(u.state);
    if (before?.active !== after.active) {
      this.active = after.active;
      this.render();
    }
    // The listed ranges follow the text too (suggest mode merges on them before the next list).
    if (u.docChanged && this.list) {
      const map = (i: ReviewItem) => {
        const from = u.changes.mapPos(i.from, 1);
        return { ...i, from, to: Math.max(from, u.changes.mapPos(i.to, -1)) };
      };
      this.list = { ...this.list, items: this.list.items.map(map) };
    }
    if (!this.shown) return;
    if (u.docChanged) this.requestRefresh();
    if (u.docChanged || u.geometryChanged || u.viewportChanged || u.heightChanged || before?.marks !== after.marks) this.layout();
  }

  /** Ctrl+Alt+M: a new comment's card on the selection, its box focused; nothing is created until it is sent. */
  newComment(): void {
    const { view } = this.host;
    const { from, to } = view.state.selection.main;
    if (from === to || !this.shown) return;
    this.draft = true;
    this.texts.delete("draft");
    const marks: ReviewMark[] = [...reviewState(view.state).marks.filter((m) => m.id !== DRAFT), { id: DRAFT, from, to, kind: "draft", colour: null, shown: true }];
    view.dispatch({ effects: [setReviewMarks(marks), setReviewActive(DRAFT)] });
    this.render();
    this.focusKey("draft");
  }

  /** Highlights the selection in `colour`. */
  addMark(colour: MarkColour): void {
    const { view } = this.host;
    if (view.state.selection.main.empty) return;
    this.write(async () => {
      this.activateNext = await this.host.api.mark(() => view.state.selection.main, colour);
    });
  }

  /** Esc in the panel: ends editing a message, or drops the new comment. */
  escape(): boolean {
    if (!this.contains(document.activeElement)) return false;
    if (this.editing) {
      this.editing = null;
      this.render();
      return true;
    }
    if (this.draft) {
      this.dropDraft();
      this.host.view.focus();
      return true;
    }
    return false;
  }

  // ---- State ----

  /** Shows `list`; `typed` (suggest mode, before the core lists it): the card at the caret becomes the active one. */
  private setList(list: ReviewList, typed = false): void {
    this.list = list;
    const { view } = this.host;
    const marks = marksOf(list.items, (p) => p, view.state.doc.length, (i) => this.shown && passes(i, this.filter), (a) => this.who(a));
    const draft = reviewState(view.state).marks.find((m) => m.id === DRAFT);
    if (this.draft && draft) marks.push(draft);
    // Just added: its card opens (also when the filter hides it, it stays listed).
    let activate = this.activateNext !== null && list.items.some((i) => i.id === this.activateNext) ? this.activateNext : null;
    this.activateNext = null;
    // A suggestion shown before it was listed has another id now.
    const gone = this.active !== null && this.active < 0 && !marks.some((m) => m.id === this.active);
    if (activate === null && (typed || gone)) activate = markAt(marks, view.state.selection.main.head)?.id ?? null;
    view.dispatch({ effects: activate === null && !gone ? setReviewMarks(marks) : [setReviewMarks(marks), setReviewActive(activate)] });
    this.render();
  }

  /** The highlights follow the filter (or go while the panel is hidden). */
  private reshow(): void {
    const { view } = this.host;
    const byId = new Map((this.list?.items ?? []).map((i) => [i.id, i]));
    const marks = reviewState(view.state).marks.map((m) => {
      const item = byId.get(m.id);
      return { ...m, shown: this.shown && (m.id === DRAFT || (item !== undefined && passes(item, this.filter))) };
    });
    view.dispatch({ effects: setReviewMarks(marks) });
  }

  private setFilter(filter: ReviewFilter): void {
    this.filter = filter;
    try {
      localStorage.setItem(FILTER_KEY, filter);
    } catch {
      // not persisted
    }
    this.showFilter();
    this.reshow();
    this.render();
  }

  private showFilter(): void {
    for (const [f, b] of this.filterButtons) b.setAttribute("aria-pressed", String(f === this.filter));
  }

  private dropDraft(): void {
    if (!this.draft) return;
    this.draft = false;
    this.texts.delete("draft");
    const { view } = this.host;
    const state = reviewState(view.state);
    view.dispatch({ effects: [setReviewMarks(state.marks.filter((m) => m.id !== DRAFT)), ...(state.active === DRAFT ? [setReviewActive(null)] : [])] });
    this.render();
  }

  /** Runs a review write, then lists the items again; errors go to the bar. */
  private write(action: () => Promise<unknown>): Promise<void> {
    return action()
      .then(() => this.refresh())
      .catch((e) => this.host.error(e));
  }

  /** Selects item `id`'s text (scrolled into view) and opens its card. */
  private select(id: number): void {
    const { view } = this.host;
    const mark = reviewState(view.state).marks.find((m) => m.id === id);
    if (!mark) return;
    view.dispatch({
      selection: { anchor: mark.from, head: mark.to },
      userEvent: REVIEW_SELECT,
      effects: [setReviewActive(id), EditorView.scrollIntoView(mark.from, { y: "nearest", yMargin: 80 })],
    });
  }

  // ---- Cards ----

  private render(): void {
    if (!this.shown) return;
    const focused = document.activeElement instanceof HTMLElement && this.root.contains(document.activeElement) ? document.activeElement : null;
    const key = focused?.dataset.key;
    const range = focused instanceof HTMLTextAreaElement ? [focused.selectionStart, focused.selectionEnd] : null;

    const items = (this.list?.items ?? []).filter((i) => passes(i, this.filter) || i.id === this.active);
    const cards = items.map((i) => this.card(i));
    if (this.draft) cards.unshift(this.draftCard());
    this.cardsEl.replaceChildren(...cards);
    this.emptyEl.hidden = cards.length > 0 || this.list === null;
    this.emptyEl.textContent = this.list?.items.length
      ? this.filter === "resolved"
        ? "Nothing resolved yet."
        : "Nothing open. Show Resolved or All for the rest."
      : "No comments yet. Select some text, then New comment (Ctrl+Alt+M) or Highlight, in the Edit menu or on right-click.";

    if (key) {
      const again = this.root.querySelector<HTMLElement>(`[data-key="${CSS.escape(key)}"]`);
      (again ?? this.root).focus({ preventScroll: true });
      if (again instanceof HTMLTextAreaElement && range) again.setSelectionRange(range[0], range[1]);
    }
    this.layout();
  }

  private who(author: ReviewAuthor | null): { label: string; colour: string } {
    return personOf(author, this.list?.me ?? "", this.people);
  }

  /** "Name · 5 min ago · edited", the name in its colour. */
  private header(author: ReviewAuthor | null, created: number, edited: number | null): HTMLElement {
    const who = this.who(author);
    const name = el("span", { className: "rv-who", textContent: who.label });
    name.style.setProperty("--who", who.colour);
    const when = el("span", { className: "rv-when", textContent: relativeTime(created, Date.now() / 1000) + (edited ? " · edited" : "") });
    when.title = new Date((edited ?? created) * 1000).toLocaleString();
    return el("div", { className: "rv-header" }, name, when);
  }

  private card(item: ReviewItem): HTMLElement {
    const me = this.list?.me ?? "";
    const active = item.id === this.active;
    const rights = itemRights(item, me, this.access);
    const card = el("article", { className: "rv-card", tabIndex: 0 });
    card.dataset.id = String(item.id);
    card.dataset.key = `card-${item.id}`;
    card.classList.toggle("active", active);
    const s = item.suggestion;
    card.classList.toggle("done", isDone(item));
    card.style.setProperty("--who", this.who(item.author).colour);
    if (isColour(item.colour)) card.dataset.colour = item.colour;
    const tag = s?.state === "accepted" ? "Accepted" : s?.state === "rejected" ? "Rejected" : item.resolved ? "Resolved" : "";
    if (tag) card.append(el("span", { className: "rv-tag", textContent: tag }));
    if (item.messages.length === 0 || item.kind === "suggestion") card.append(this.header(item.author, item.created, null));
    if (s) card.append(el("p", { className: "rv-suggestion", textContent: suggestionText(s) }));
    if (item.detached) card.append(el("p", { className: "rv-note", textContent: "Its text was deleted." }));
    else if (s?.state === "open" && s.stale) card.append(el("p", { className: "rv-note", textContent: "The text under it changed since." }));
    if (isColour(item.colour)) card.append(this.colours(item, active && rights.recolour));

    const shown = active ? item.messages : item.messages.slice(0, 1);
    for (const m of shown) card.append(this.message(item, m, active));
    if (!active && item.messages.length > 1) {
      const more = item.messages.length - 1;
      card.append(el("p", { className: "rv-more", textContent: more === 1 ? "1 reply" : `${more} replies` }));
    }
    if (!active) return card;

    if (rights.reply) card.append(this.replyBox(item));
    const actions = el("div", { className: "rv-actions" });
    // (Not before the core listed it.)
    if (s?.state === "open" && item.id > 0) {
      const may = suggestionRights(item, me, this.access);
      const accept = button("Accept", () => this.accept(item.id), { key: `accept-${item.id}`, className: "primary", title: may.why || "Make the change in the text (Ctrl+Z in the text undoes it)" });
      accept.disabled = !may.accept;
      actions.append(accept);
      if (may.reject) actions.append(button("Reject", () => this.reject(item.id), { key: `reject-${item.id}`, write: true, title: "Leave the text as it is (Ctrl+Z in this panel undoes it)" }));
    }
    if (rights.resolve) {
      actions.append(
        item.resolved
          ? button("Reopen", () => this.write(() => this.host.api.resolve(item.id, false)), { key: `reopen-${item.id}`, write: true })
          : button("Resolve", () => this.write(() => this.host.api.resolve(item.id, true)), { key: `resolve-${item.id}`, write: true, title: "Mark as done (Resolved shows it again)" }),
      );
    }
    if (rights.remove) {
      const label = item.kind === "mark" ? "Remove highlight" : "Delete thread";
      actions.append(button(label, () => this.write(() => this.host.api.remove(item.id)), { key: `remove-${item.id}`, write: true, className: "rv-quiet", title: `${label} (Ctrl+Z in this panel undoes it)` }));
    }
    if (actions.childElementCount) card.append(actions);
    return card;
  }

  /** The highlight's colour; its author picks another on the active card. */
  private colours(item: ReviewItem, pick: boolean): HTMLElement {
    const row = el("div", { className: "rv-colours" });
    if (!pick) {
      const swatch = el("span", { className: "rv-swatch", title: COLOUR_LABELS[item.colour as MarkColour] });
      swatch.dataset.colour = item.colour!;
      row.append(swatch, el("span", { textContent: item.kind === "mark" ? "Highlight" : COLOUR_LABELS[item.colour as MarkColour] }));
      return row;
    }
    for (const c of MARK_COLOURS) {
      const b = button("", () => this.write(() => this.host.api.colour(item.id, c)), { key: `colour-${item.id}-${c}`, write: true, className: "rv-swatch", title: COLOUR_LABELS[c] });
      b.dataset.colour = c;
      b.setAttribute("aria-label", COLOUR_LABELS[c]);
      b.setAttribute("aria-pressed", String(c === item.colour));
      row.append(b);
    }
    return row;
  }

  private message(item: ReviewItem, m: ReviewMessage, active: boolean): HTMLElement {
    const box = el("div", { className: "rv-message" }, this.header(m.author, m.created, m.edited));
    const key = `edit-${item.id}-${m.id}`;
    if (this.editing?.item === item.id && this.editing.message === m.id) {
      const save = () => {
        const text = this.texts.get(key)?.trim();
        if (!text) return;
        this.write(async () => {
          await this.host.api.edit(item.id, m.id, text);
          this.editing = null;
          this.texts.delete(key);
        });
      };
      const cancel = () => {
        this.editing = null;
        this.texts.delete(key);
        this.render();
      };
      box.append(this.textBox(key, "Edit the message…", save), el("div", { className: "rv-actions" }, button("Save", save, { key: `${key}-save`, write: true, className: "primary" }), button("Cancel", cancel, { key: `${key}-cancel` })));
      return box;
    }
    box.append(m.deleted ? el("p", { className: "rv-text rv-deleted", textContent: "Message deleted" }) : el("p", { className: active ? "rv-text" : "rv-text rv-clamp", textContent: m.text }));
    const rights = messageRights(item, m, this.list?.me ?? "", this.access);
    if (active && (rights.edit || rights.delete)) {
      const actions = el("div", { className: "rv-message-actions" });
      if (rights.edit) {
        actions.append(
          button(
            "Edit",
            () => {
              this.editing = { item: item.id, message: m.id };
              this.texts.set(key, m.text);
              this.render();
              this.focusKey(key);
            },
            { key: `${key}-edit`, write: true, className: "rv-link" },
          ),
        );
      }
      if (rights.delete) actions.append(button("Delete", () => this.write(() => this.host.api.deleteMessage(item.id, m.id)), { key: `delete-${item.id}-${m.id}`, write: true, className: "rv-link" }));
      box.append(actions);
    }
    return box;
  }

  /** A text box keeping what is typed over renders; Ctrl+Enter sends. */
  private textBox(key: string, placeholder: string, send: () => void): HTMLTextAreaElement {
    const box = el("textarea", { rows: 2, placeholder, value: this.texts.get(key) ?? "" });
    box.dataset.key = key;
    box.dataset.write = "";
    box.setAttribute("aria-label", placeholder.replace(/…$/, ""));
    box.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        send();
      }
    });
    return box;
  }

  private replyBox(item: ReviewItem): HTMLElement {
    const key = `reply-${item.id}`;
    const note = item.kind === "mark";
    const send = () => {
      const text = this.texts.get(key)?.trim();
      if (!text) return;
      this.write(async () => {
        await this.host.api.reply(item.id, text);
        this.texts.delete(key);
      });
    };
    const sendButton = button(note ? "Add note" : "Reply", send, { key: `${key}-send`, write: true, className: "primary", title: "Ctrl+Enter" });
    sendButton.dataset.send = "";
    sendButton.disabled = !this.texts.get(key)?.trim();
    return el("div", { className: "rv-reply" }, this.textBox(key, note ? "Add a note…" : "Reply…", send), sendButton);
  }

  private draftCard(): HTMLElement {
    const card = el("article", { className: "rv-card active rv-draft" });
    card.dataset.id = String(DRAFT);
    card.style.setProperty("--who", this.who(this.list ? { user: this.list.me, device: "" } : null).colour);
    const send = () => {
      const text = this.texts.get("draft")?.trim();
      if (!text) return;
      const { view } = this.host;
      let id: number | null = null;
      // The new card keeps the focus (its box is gone): Ctrl+Z in the panel undoes it.
      void this.write(async () => {
        const range = () => reviewState(view.state).marks.find((m) => m.id === DRAFT) ?? null;
        id = this.activateNext = await this.host.api.comment(range, text);
        this.dropDraft();
      }).then(() => id !== null && this.focusKey(`card-${id}`));
    };
    const box = this.textBox("draft", "Comment…", send);
    // Left empty: nothing is created.
    box.addEventListener("blur", () =>
      setTimeout(() => {
        // (A render replaces the card: the one shown now counts.)
        const shown = this.root.querySelector(".rv-draft");
        if (this.draft && !this.texts.get("draft")?.trim() && !shown?.contains(document.activeElement)) this.dropDraft();
      }),
    );
    const sendButton = button("Comment", send, { key: "draft-send", write: true, className: "primary", title: "Ctrl+Enter" });
    sendButton.dataset.send = "";
    sendButton.disabled = !this.texts.get("draft")?.trim();
    const cancel = button("Cancel", () => {
      this.dropDraft();
      this.host.view.focus();
    });
    card.append(el("div", { className: "rv-header" }, el("span", { className: "rv-who", textContent: "New comment" })), el("div", { className: "rv-reply" }, box, el("div", { className: "rv-actions" }, sendButton, cancel)));
    return card;
  }

  private focusKey(key: string): void {
    this.root.querySelector<HTMLElement>(`[data-key="${CSS.escape(key)}"]`)?.focus();
  }

  // ---- Placement ----

  /** Places the cards beside their lines (measured with the editor's own read/write cycle). */
  private layout(): void {
    if (!this.shown) return;
    this.host.view.requestMeasure({
      key: this,
      read: (view) => {
        const marks = new Map(reviewState(view.state).marks.map((m) => [m.id, m]));
        const boxes: CardBox[] = [];
        for (const card of this.cardsEl.children) {
          if (!(card instanceof HTMLElement)) continue;
          const m = marks.get(Number(card.dataset.id));
          if (m) boxes.push({ id: m.id, want: lineTop(view, m.from), height: card.offsetHeight });
        }
        const limit = Math.max(view.contentHeight, view.scrollDOM.clientHeight);
        return { tops: stackCards(boxes, this.active, CARD_GAP, limit), offset: this.offset() };
      },
      write: ({ tops, offset }) => {
        for (const card of this.cardsEl.children) {
          if (card instanceof HTMLElement) card.style.transform = `translateY(${tops.get(Number(card.dataset.id)) ?? 0}px)`;
        }
        this.cardsEl.style.transform = `translateY(${offset}px)`;
      },
    });
  }

  /** Where the document's top is, relative to the cards' track. */
  private offset(): number {
    return this.host.view.documentTop - this.track.getBoundingClientRect().top;
  }

  /** The editor scrolled: the cards move with it. */
  private shift(): void {
    if (this.shown) this.cardsEl.style.transform = `translateY(${this.offset()}px)`;
  }
}

/** The top of the visual line holding `pos`, px from the document's top (estimated off screen). */
function lineTop(view: EditorView, pos: number): number {
  const drawn = pos >= view.viewport.from && pos <= view.viewport.to ? view.coordsAtPos(pos, 1) : null;
  return drawn ? drawn.top - view.documentTop : view.lineBlockAt(pos).top;
}
