// Review tools in the author and science views, the pure parts: the review
// items as the core lists them (`review_list`; crates/core/src/review.rs),
// who may do what with them, the Open / Resolved / All filter, their ranges
// in the editor with the highlights and suggestions (a CodeMirror field),
// which card is active, how the cards stack beside their lines
// (reviewpanel.ts draws them), and suggest mode (edits become suggestions,
// `suggestMode`). Offsets from the core are mapped to the editor's by the
// edit queue (see `Sync.toEditor`).
//
// No Tauri imports and no DOM (but in widgets drawn): this runs under `node --test`.

import { type ChangeSet, EditorSelection, EditorState, type Extension, type Range, StateEffect, StateField, type Transaction } from "@codemirror/state";
import { Decoration, type DecorationSet, EditorView, WidgetType } from "@codemirror/view";
import { UNKNOWN_COLOUR } from "./authors.ts";
import { displayName, safeColour } from "./presence.ts";
import { fromCore, OWN_STEP } from "./sync.ts";

/** The highlight colours (the core's `MARK_COLOURS`). */
export const MARK_COLOURS = ["yellow", "green", "blue", "pink"] as const;
export type MarkColour = (typeof MARK_COLOURS)[number];
export const COLOUR_LABELS: Record<MarkColour, string> = { yellow: "Yellow", green: "Green", blue: "Blue", pink: "Pink" };

export type ReviewKind = "mark" | "comment" | "suggestion";

/** Who made an item or a message; null for an unknown peer. */
export interface ReviewAuthor {
  user: string;
  device: string;
}

export interface ReviewMessage {
  id: number;
  author: ReviewAuthor | null;
  /** Empty once deleted. */
  text: string;
  /** Unix seconds. */
  created: number;
  edited: number | null;
  deleted: boolean;
}

export interface Suggestion {
  insert: string;
  original: string;
  state: "open" | "accepted" | "rejected";
  stale: boolean;
}

/** `review_list`: an item, its range in the core's text (empty where its text was when detached). */
export interface ReviewItem {
  id: number;
  kind: ReviewKind;
  from: number;
  to: number;
  detached: boolean;
  colour: string | null;
  resolved: boolean;
  author: ReviewAuthor | null;
  created: number;
  /** Oldest first. */
  messages: ReviewMessage[];
  suggestion: Suggestion | null;
}

/** `review_list`: this device's user, the items in text order, and whether review undo/redo can act. */
export interface ReviewList {
  me: string;
  items: ReviewItem[];
  undo: boolean;
  redo: boolean;
}

/** What this device's role lets it change: `main` (Owner, Editor, a project of its own) and `review` (all but Readers). */
export interface ReviewAccess {
  main: boolean;
  review: boolean;
}

export const isColour = (c: string | null): c is MarkColour => (MARK_COLOURS as readonly string[]).includes(c ?? "");

// ---- Filter ----

export type ReviewFilter = "open" | "resolved" | "all";
export const FILTERS: ReadonlyArray<{ filter: ReviewFilter; label: string }> = [
  { filter: "open", label: "Open" },
  { filter: "resolved", label: "Resolved" },
  { filter: "all", label: "All" },
];

/** Resolved, or a suggestion accepted or rejected. */
export const isDone = (item: ReviewItem): boolean => item.resolved || (item.suggestion !== null && item.suggestion.state !== "open");

export function passes(item: ReviewItem, filter: ReviewFilter): boolean {
  return filter === "all" || isDone(item) === (filter === "resolved");
}

// ---- Rights (the core's permission table; review.rs, "Permissions") ----

const own = (author: ReviewAuthor | null, me: string) => author !== null && author.user === me;

/**
 * What the item's card offers. Writing `review` at all is the role's: the
 * panel keeps these controls disabled for readers (`WriteGuard`). A
 * suggestion offers Accept and Reject instead (`suggestionRights`).
 */
export function itemRights(item: ReviewItem, me: string, access: ReviewAccess): { reply: boolean; resolve: boolean; remove: boolean; recolour: boolean } {
  const plain = item.kind !== "suggestion";
  return {
    reply: plain,
    resolve: plain,
    // Its author, an Owner or an Editor.
    remove: plain && (own(item.author, me) || access.main),
    recolour: plain && isColour(item.colour) && own(item.author, me),
  };
}

/** What a message offers: Edit for its author only, Delete also for an Owner or Editor. */
export function messageRights(item: ReviewItem, message: ReviewMessage, me: string, access: ReviewAccess): { edit: boolean; delete: boolean } {
  const live = item.kind !== "suggestion" && !message.deleted;
  return { edit: live && own(message.author, me), delete: live && (own(message.author, me) || access.main) };
}

/**
 * An open suggestion's Accept (whoever writes `main`, never a reviewer; not
 * when stale or detached, `why` says why not) and Reject (its author, an
 * Owner or an Editor).
 */
export function suggestionRights(item: ReviewItem, me: string, access: ReviewAccess): { accept: boolean; why: string; reject: boolean } {
  const s = item.suggestion;
  if (!s || s.state !== "open") return { accept: false, why: "", reject: false };
  const why = !access.main
    ? "Only an Owner or Editor accepts suggestions."
    : item.detached
      ? "Its text was deleted: it can't be accepted."
      : s.stale
        ? "The text under it changed since: it can't be accepted."
        : "";
  return { accept: why === "", why, reject: access.review && (own(item.author, me) || access.main) };
}

/**
 * Where Ctrl+Z and Ctrl+Y go: a text field's own history, the review's (the
 * focus in the Review panel, suggest mode, or everywhere for a reviewer, who
 * doesn't write `main`), else `main`'s.
 */
export function historyTarget(at: { field: boolean; panel: boolean; suggest?: boolean }, access: ReviewAccess): "field" | "review" | "main" {
  if (at.field) return "field";
  if (at.panel || at.suggest || (!access.main && access.review)) return "review";
  return "main";
}

// ---- People and words ----

/** A member as the panel names them (`server_members`). */
export interface Person {
  name?: string;
  colour: string;
}

/** The default colour of this device's user without a members list (a project of its own). */
export const OWN_COLOUR = "#0072b2";

/** An author's label and colour: "You", a member's name, "Someone" (not a member), or "Unknown". */
export function personOf(author: ReviewAuthor | null, me: string, people: ReadonlyMap<string, Person>): { label: string; colour: string } {
  if (!author) return { label: "Unknown", colour: UNKNOWN_COLOUR };
  const p = people.get(author.user);
  if (author.user === me) return { label: "You", colour: safeColour(p?.colour ?? OWN_COLOUR) };
  return { label: displayName(p ?? {}), colour: p ? safeColour(p.colour) : UNKNOWN_COLOUR };
}

const dateOnly = new Intl.DateTimeFormat(undefined, { dateStyle: "medium" });

/** Unix seconds `secs` relative to `now`: "just now", "5 min ago", "3 h ago", "yesterday", "4 days ago", else the date. */
export function relativeTime(secs: number, now: number, date: (secs: number) => string = (s) => dateOnly.format(new Date(s * 1000))): string {
  const d = Math.max(0, now - secs);
  if (d < 60) return "just now";
  if (d < 3600) return `${Math.floor(d / 60)} min ago`;
  if (d < 86400) return `${Math.floor(d / 3600)} h ago`;
  if (d < 2 * 86400) return "yesterday";
  if (d < 7 * 86400) return `${Math.floor(d / 86400)} days ago`;
  return date(secs);
}

const quoted = (s: string, n = 60) => {
  const t = s.replace(/\s+/g, " ").trim();
  return `“${t.length <= n ? t : `${t.slice(0, n - 1).trimEnd()}…`}”`;
};

/** A suggestion's card text: "Suggestion: replace “X” with “Y”" (insert or delete if one side is empty). */
export function suggestionText(s: Suggestion): string {
  if (!s.insert) return `Suggestion: delete ${quoted(s.original)}`;
  if (!s.original) return `Suggestion: insert ${quoted(s.insert)}`;
  return `Suggestion: replace ${quoted(s.original)} with ${quoted(s.insert)}`;
}

// ---- Ranges and highlights in the editor ----

/** The id of a new comment's card before it exists (review ids are never 0). */
export const DRAFT = 0;
/** The user event of selections the panel makes (they don't change the active card). */
export const REVIEW_SELECT = "select.review";

/** An item's range in the editor's text. */
export interface ReviewMark {
  id: number;
  from: number;
  to: number;
  kind: ReviewKind | "draft";
  colour: MarkColour | null;
  /** Highlighted (and found by the caret): the panel is open and the filter shows it. */
  shown: boolean;
  /** An open suggestion's text: what it inserts, and its author's colour and hover text. */
  suggestion?: { insert: string; colour: string; title: string };
}

/** Who made an item, as the panel names them (`personOf`). */
export type Who = (author: ReviewAuthor | null) => { label: string; colour: string };

/** The items' ranges mapped by `toEditor` (core → editor offsets) and kept within `length`; detached ones empty; open suggestions with their text (`who` names their authors). */
export function marksOf(items: readonly ReviewItem[], toEditor: (pos: number) => number, length: number, shown: (item: ReviewItem) => boolean, who?: Who): ReviewMark[] {
  const place = (pos: number) => Math.max(0, Math.min(length, toEditor(pos)));
  return items.map((item) => {
    const from = place(item.from);
    const to = item.detached ? from : Math.max(from, place(item.to));
    const mark: ReviewMark = { id: item.id, from, to, kind: item.kind, colour: isColour(item.colour) ? item.colour : null, shown: shown(item) };
    // Only open suggestions show in the text.
    const s = item.suggestion;
    if (s?.state === "open" && who) {
      const person = who(item.author);
      mark.suggestion = { insert: s.insert, colour: person.colour, title: `${person.label} · ${suggestionText(s)}` };
    }
    return mark;
  });
}

/** The innermost shown range holding `pos` (its ends included), or null: non-empty, or a suggestion's insertion point. */
export function markAt(marks: readonly ReviewMark[], pos: number): ReviewMark | null {
  let best: ReviewMark | null = null;
  for (const m of marks) {
    if (!m.shown || (m.from >= m.to && !m.suggestion) || pos < m.from || pos > m.to) continue;
    if (!best || m.to - m.from < best.to - best.from || (m.to - m.from === best.to - best.from && m.from > best.from)) best = m;
  }
  return best;
}

export interface ReviewState {
  marks: readonly ReviewMark[];
  /** The active card's item (`DRAFT` for a new comment), or null. */
  active: number | null;
}

const setMarks = StateEffect.define<ReviewMark[]>();
const setActive = StateEffect.define<number | null>();

/** Replaces the items' ranges (positions in the editor's current text). */
export const setReviewMarks = (marks: ReviewMark[]): StateEffect<ReviewMark[]> => setMarks.of(marks);
/** Makes `id`'s card the active one (null: none). */
export const setReviewActive = (id: number | null): StateEffect<number | null> => setActive.of(id);

/** The shown open suggestion at `pos` (its ends included; the innermost), or null. */
export function suggestionAt(marks: readonly ReviewMark[], pos: number): ReviewMark | null {
  return markAt(
    marks.filter((m) => m.suggestion),
    pos,
  );
}

/** How a suggestion shows: its range struck through (if any), its text inserted at the range's end (if any). */
export function suggestionParts(m: ReviewMark): { strike: { from: number; to: number } | null; insert: { at: number; text: string } | null } {
  if (!m.suggestion) return { strike: null, insert: null };
  return { strike: m.from < m.to ? { from: m.from, to: m.to } : null, insert: m.suggestion.insert ? { at: m.to, text: m.suggestion.insert } : null };
}

/** A suggestion's inserted text, underlined in its author's colour (line breaks as ¶). */
class InsertWidget extends WidgetType {
  readonly text: string;
  readonly colour: string;
  readonly title: string;
  readonly active: boolean;

  constructor(text: string, colour: string, title: string, active: boolean) {
    super();
    this.text = text;
    this.colour = colour;
    this.title = title;
    this.active = active;
  }

  eq(other: InsertWidget): boolean {
    return other.text === this.text && other.colour === this.colour && other.title === this.title && other.active === this.active;
  }

  toDOM(): HTMLElement {
    const span = document.createElement("span");
    span.className = `cm-suggest-ins${this.active ? " cm-review-active" : ""}`;
    span.style.setProperty("--who", this.colour);
    span.title = this.title;
    span.textContent = this.text.replace(/\n/g, "¶");
    return span;
  }
}

function decorations(state: ReviewState): DecorationSet {
  const ranges: Range<Decoration>[] = [];
  for (const m of state.marks) {
    if (!m.shown) continue;
    if (m.suggestion) {
      const { strike, insert } = suggestionParts(m);
      const active = m.id === state.active;
      const attributes = { style: `--who: ${m.suggestion.colour}`, title: m.suggestion.title };
      if (strike) ranges.push(Decoration.mark({ class: `cm-suggest-del${active ? " cm-review-active" : ""}`, attributes }).range(strike.from, strike.to));
      // Before the caret at the range's end: typing there goes on with it.
      if (insert) ranges.push(Decoration.widget({ widget: new InsertWidget(insert.text, m.suggestion.colour, m.suggestion.title, active), side: -1 }).range(insert.at));
      continue;
    }
    if (m.from >= m.to || m.kind === "suggestion") continue;
    const kind = m.colour ? `cm-review-${m.colour}` : "cm-review-comment";
    ranges.push(Decoration.mark({ class: `cm-review ${kind}${m.id === state.active ? " cm-review-active" : ""}` }).range(m.from, m.to));
  }
  return Decoration.set(ranges, true);
}

/**
 * The items' ranges, mapped over edits (text typed at either end stays
 * outside), and the active card: a selection the user makes activates the
 * item under its head, or none.
 */
export const reviewField = StateField.define<ReviewState>({
  create: () => ({ marks: [], active: null }),
  update(value, tr) {
    let { marks, active } = value;
    if (tr.docChanged) {
      marks = marks.map((m) => {
        const from = tr.changes.mapPos(m.from, 1);
        return { ...m, from, to: Math.max(from, tr.changes.mapPos(m.to, -1)) };
      });
    }
    let chosen = false;
    for (const effect of tr.effects) {
      if (effect.is(setMarks)) marks = effect.value;
      if (effect.is(setActive)) {
        active = effect.value;
        chosen = true;
      }
    }
    if (tr.selection && !chosen && !tr.isUserEvent(REVIEW_SELECT)) active = markAt(marks, tr.selection.main.head)?.id ?? null;
    return marks === value.marks && active === value.active ? value : { marks, active };
  },
  provide: (field) => EditorView.decorations.from(field, decorations),
});

export const reviewState = (state: EditorState): ReviewState => state.field(reviewField);

// ---- Cards beside their lines ----

/** A card to place: its item, where its line is (px from the document's top) and its height. */
export interface CardBox {
  id: number;
  want: number;
  height: number;
}

/** Space between stacked cards, px. */
export const CARD_GAP = 8;

/**
 * Card tops (px from the document's top), Word-style: each card at its line,
 * pushed down to clear the one above; the active card stays at its line and
 * the ones above it move up to make room (as far as the top allows). Cards
 * that would end past `limit` move up as far as there is room.
 */
export function stackCards(cards: readonly CardBox[], active: number | null, gap = CARD_GAP, limit = Infinity): Map<number, number> {
  const sorted = [...cards].sort((a, b) => a.want - b.want);
  const n = sorted.length;
  const top = sorted.map((c) => Math.max(0, c.want));
  const down = (start: number) => {
    for (let i = Math.max(1, start); i < n; i++) top[i] = Math.max(top[i], top[i - 1] + sorted[i - 1].height + gap);
  };
  const up = (start: number) => {
    for (let i = start; i >= 0; i--) top[i] = Math.min(top[i], top[i + 1] - gap - sorted[i].height);
  };
  const a = sorted.findIndex((c) => c.id === active);
  if (a < 0) down(1);
  else {
    down(a + 1);
    up(a - 1);
  }
  if (n && top[n - 1] + sorted[n - 1].height > limit) {
    top[n - 1] = Math.min(top[n - 1], Math.max(0, limit - sorted[n - 1].height));
    up(n - 2);
  }
  if (n && top[0] < 0) {
    top[0] = 0;
    down(1);
  }
  return new Map(sorted.map((c, i) => [c.id, top[i]]));
}

// ---- Suggest mode (review.rs, "Suggest mode") ----

/** A `review_suggest` call: replace `from..to` with `insert` (in the text before the edit, which suggest mode never changes). */
export interface SuggestCall {
  from: number;
  to: number;
  insert: string;
}

/** A transaction's changes as suggest calls, last first (so each one's offsets hold as the core takes them). */
export function suggestCalls(changes: ChangeSet): SuggestCall[] {
  const calls: SuggestCall[] = [];
  changes.iterChanges((from, to, _fromB, _toB, inserted) => calls.push({ from, to, insert: inserted.toString() }));
  return calls.reverse();
}

/** How the core takes a suggest call: nothing, a new suggestion, or rule 1 (typing), 2 (a backspace after typed text) or 3 (widening) on the own one. */
export type SuggestRule = "none" | "new" | "type" | "shrink" | "widen";

/** A suggest call's outcome: the own suggestion it merged into (if any) and the suggestion made or changed (null once removed). */
export interface Suggested {
  rule: SuggestRule;
  own: number | null;
  item: ReviewItem | null;
}

const chars = (s: string) => [...s].length;

/**
 * The core's merge rules (`Document::suggest_change`) on the listed items,
 * for the caret, for showing keystrokes before the core lists them, and for
 * the dev mock. `me` makes new suggestions; `slice` reads the text; `fresh`
 * gives a new item's id and time.
 */
export function suggestInto(items: readonly ReviewItem[], me: ReviewAuthor, call: SuggestCall, slice: (from: number, to: number) => string, fresh: () => { id: number; created: number }): Suggested {
  const { from, to, insert } = call;
  if (from === to && !insert) return { rule: "none", own: null, item: null };
  const deleted = chars(slice(from, to));
  // The newest own open suggestion next to the edit.
  let own: ReviewItem | null = null;
  for (const i of items) {
    const s = i.suggestion;
    if (!s || s.state !== "open" || i.resolved || i.detached || i.author?.user !== me.user) continue;
    const near = from === to ? from === i.to || (i.from < from && from <= i.to) : (!insert && to === i.to && chars(s.insert) >= deleted) || (from <= i.to && to >= i.from);
    if (near && (!own || i.created > own.created || (i.created === own.created && i.id > own.id))) own = i;
  }
  const s = own?.suggestion;
  if (!own || !s) {
    const { id, created } = fresh();
    const item: ReviewItem = {
      id,
      kind: "suggestion",
      from,
      to,
      detached: false,
      colour: null,
      resolved: false,
      author: me,
      created,
      messages: [],
      suggestion: { insert, original: slice(from, to), state: "open", stale: false },
    };
    return { rule: "new", own: null, item };
  }
  // Rule 1: typing at its end or inside it.
  if (from === to) return { rule: "type", own: own.id, item: { ...own, suggestion: { ...s, insert: s.insert + insert } } };
  // Rule 2: a backspace after typed text.
  if (!insert && to === own.to && chars(s.insert) >= deleted) {
    const shrunk = [...s.insert].slice(0, chars(s.insert) - deleted).join("");
    return { rule: "shrink", own: own.id, item: !shrunk && own.from === own.to ? null : { ...own, suggestion: { ...s, insert: shrunk } } };
  }
  // Rule 3: widen the range over the deletion or replacement.
  const lo = Math.min(from, own.from);
  const hi = Math.max(to, own.to);
  const original = slice(lo, own.from) + s.original + slice(own.to, hi);
  return { rule: "widen", own: own.id, item: { ...own, from: lo, to: hi, suggestion: { ...s, original, insert: s.insert + insert } } };
}

/** The items after a suggest call, in the core's order. */
export function withSuggested(items: readonly ReviewItem[], s: Suggested): ReviewItem[] {
  const out = s.own === null ? [...items] : items.filter((i) => i.id !== s.own);
  if (s.item) out.push(s.item);
  return out.sort((a, b) => a.from - b.from || a.to - b.to || a.created - b.created || a.id - b.id);
}

/**
 * Where the caret goes after a suggest call (the text stays as it is): after
 * typing at the insertion point (the next key goes on there, the typed text
 * shows before the caret), after a replacement at the range's end; after
 * Backspace at the deletion's start (repeating widens the struck range)
 * unless it took back typed text, after Delete at its end.
 */
export function suggestCaret(call: SuggestCall, rule: SuggestRule, forward: boolean): number {
  return call.insert || rule === "shrink" || forward ? call.to : call.from;
}

/** What a suggest-mode transaction carries instead of its changes. */
export interface Suggesting {
  calls: SuggestCall[];
  /** The listed items as the calls leave them (null while nothing is listed). */
  items: ReviewItem[] | null;
  /** Starts a new review undo step (a paste, a cut, ...: see `OWN_STEP`). */
  split: boolean;
}

const suggestEffect = StateEffect.define<Suggesting>();

/** The suggest calls a transaction carries, if it was an edit in suggest mode. */
export function suggestionsOf(tr: Transaction): Suggesting | null {
  for (const e of tr.effects) if (e.is(suggestEffect)) return e.value;
  return null;
}

/** Ids of suggestions shown before the core lists them (the core's are positive). */
let tempId = 0;

/**
 * Suggest mode: every edit of the editor (typing, deleting, paste, cut,
 * formatting, Replace, ...) becomes suggest calls, carried by a transaction
 * that only moves the caret (`suggestionsOf`; the caller sends them). The
 * core's changes (`fromCore`) pass. `list` is what the panel last listed
 * (the merge rules decide the caret after Backspace). With several changes
 * at once (Replace all, formatting both ends) the selection stays.
 */
export function suggestMode(list: () => ReviewList | null): Extension {
  return EditorState.transactionFilter.of((tr) => {
    if (!tr.docChanged || tr.annotation(fromCore)) return tr;
    const calls = suggestCalls(tr.changes);
    const listed = list();
    const me: ReviewAuthor = { user: listed?.me ?? "", device: "" };
    let items: ReviewItem[] = listed?.items ?? [];
    let caret: number | null = null;
    for (const call of calls) {
      const s = suggestInto(
        items,
        me,
        call,
        (f, t) => tr.startState.sliceDoc(f, t),
        () => ({ id: --tempId, created: Math.floor(Date.now() / 1000) }),
      );
      items = withSuggested(items, s);
      if (calls.length === 1) caret = suggestCaret(call, s.rule, tr.isUserEvent("delete.forward"));
    }
    return {
      selection: caret === null ? tr.startState.selection : EditorSelection.cursor(caret),
      effects: [...StateEffect.mapEffects(tr.effects, tr.changes.invertedDesc), suggestEffect.of({ calls, items: listed ? items : null, split: OWN_STEP.some((e) => tr.isUserEvent(e)) })],
      scrollIntoView: tr.scrollIntoView,
    };
  });
}
