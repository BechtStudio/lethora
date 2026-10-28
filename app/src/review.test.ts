// Tests for the review tools' pure parts: rights, filters, undo routing, names, ranges, the active card, card stacking and suggest mode. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { ChangeSet, EditorSelection, EditorState } from "@codemirror/state";
import { fromCore } from "./sync.ts";
import { UNKNOWN_COLOUR } from "./authors.ts";
import {
  type CardBox,
  DRAFT,
  historyTarget,
  itemRights,
  markAt,
  marksOf,
  messageRights,
  passes,
  personOf,
  relativeTime,
  REVIEW_SELECT,
  type ReviewAccess,
  type ReviewItem,
  reviewField,
  setReviewActive,
  setReviewMarks,
  stackCards,
  type SuggestCall,
  suggestCalls,
  suggestCaret,
  suggestInto,
  suggestionAt,
  suggestionParts,
  suggestionRights,
  suggestionsOf,
  suggestionText,
  suggestMode,
  withSuggested,
} from "./review.ts";

const ME = "me";
const BEA = "bea";
const author = (user: string) => ({ user, device: `${user}-device` });

function item(over: Partial<ReviewItem> = {}): ReviewItem {
  return { id: 1, kind: "comment", from: 0, to: 5, detached: false, colour: null, resolved: false, author: author(ME), created: 100, messages: [], suggestion: null, ...over };
}

const message = (user: string | null, over: Partial<ReviewItem["messages"][number]> = {}) => ({ id: 7, author: user ? author(user) : null, text: "Hi", created: 100, edited: null, deleted: false, ...over });

const OWNER: ReviewAccess = { main: true, review: true };
const REVIEWER: ReviewAccess = { main: false, review: true };
const READER: ReviewAccess = { main: false, review: false };

test("rights: edit and recolour for the author only, delete and remove also for owners and editors", () => {
  const mine = item({ colour: "yellow" });
  const beas = item({ author: author(BEA), colour: "green" });
  const unknown = item({ author: null });
  for (const access of [OWNER, REVIEWER, READER]) {
    assert.deepEqual(itemRights(mine, ME, access), { reply: true, resolve: true, remove: true, recolour: true });
    assert.equal(itemRights(beas, ME, access).recolour, false);
    assert.equal(itemRights(unknown, ME, access).recolour, false);
    assert.deepEqual(messageRights(mine, message(ME), ME, access), { edit: true, delete: true });
    assert.equal(messageRights(beas, message(BEA), ME, access).edit, false);
    assert.equal(messageRights(beas, message(null), ME, access).edit, false);
  }
  // Moderators: Owner and Editor (main writers), never a reviewer or reader.
  assert.equal(itemRights(beas, ME, OWNER).remove, true);
  assert.equal(itemRights(beas, ME, REVIEWER).remove, false);
  assert.equal(itemRights(unknown, ME, REVIEWER).remove, false);
  assert.equal(messageRights(beas, message(BEA), ME, OWNER).delete, true);
  assert.equal(messageRights(beas, message(BEA), ME, REVIEWER).delete, false);
  // A comment without a colour can't be recoloured; a deleted message offers nothing.
  assert.equal(itemRights(item(), ME, OWNER).recolour, false);
  assert.deepEqual(messageRights(mine, message(ME, { deleted: true }), ME, OWNER), { edit: false, delete: false });
  // Suggestions offer Accept and Reject instead.
  const suggestion = item({ kind: "suggestion", suggestion: { insert: "a", original: "b", state: "open", stale: false } });
  assert.deepEqual(itemRights(suggestion, ME, OWNER), { reply: false, resolve: false, remove: false, recolour: false });
  assert.deepEqual(messageRights(suggestion, message(ME), ME, OWNER), { edit: false, delete: false });
});

test("filters: open, resolved (also settled suggestions), all", () => {
  const open = item();
  const resolved = item({ resolved: true });
  const accepted = item({ kind: "suggestion", suggestion: { insert: "a", original: "", state: "accepted", stale: false } });
  const pending = item({ kind: "suggestion", suggestion: { insert: "a", original: "", state: "open", stale: false } });
  const all = [open, resolved, accepted, pending];
  assert.deepEqual(all.filter((i) => passes(i, "open")), [open, pending]);
  assert.deepEqual(all.filter((i) => passes(i, "resolved")), [resolved, accepted]);
  assert.deepEqual(all.filter((i) => passes(i, "all")), all);
});

test("undo goes to a field, the review (panel focus, or a reviewer anywhere) or main", () => {
  assert.equal(historyTarget({ field: true, panel: true }, REVIEWER), "field");
  assert.equal(historyTarget({ field: false, panel: true }, OWNER), "review");
  assert.equal(historyTarget({ field: false, panel: false }, OWNER), "main");
  assert.equal(historyTarget({ field: false, panel: false }, REVIEWER), "review");
  // A reader has nothing to undo; main's undo does nothing while read only.
  assert.equal(historyTarget({ field: false, panel: false }, READER), "main");
});

test("names: you, members, someone, unknown", () => {
  const people = new Map([
    [ME, { name: "Max", colour: "#d55e00" }],
    [BEA, { name: "Bea", colour: "#009e73" }],
  ]);
  assert.deepEqual(personOf(author(ME), ME, people), { label: "You", colour: "#d55e00" });
  assert.deepEqual(personOf(author(ME), ME, new Map()), { label: "You", colour: "#0072b2" });
  assert.deepEqual(personOf(author(BEA), ME, people), { label: "Bea", colour: "#009e73" });
  assert.deepEqual(personOf(author("gone"), ME, people), { label: "Someone", colour: UNKNOWN_COLOUR });
  assert.deepEqual(personOf(null, ME, people), { label: "Unknown", colour: UNKNOWN_COLOUR });
});

test("times and suggestion texts", () => {
  const now = 1_000_000;
  assert.equal(relativeTime(now - 5, now), "just now");
  assert.equal(relativeTime(now + 30, now), "just now");
  assert.equal(relativeTime(now - 125, now), "2 min ago");
  assert.equal(relativeTime(now - 3 * 3600, now), "3 h ago");
  assert.equal(relativeTime(now - 30 * 3600, now), "yesterday");
  assert.equal(relativeTime(now - 4 * 86400, now), "4 days ago");
  assert.equal(relativeTime(now - 9 * 86400, now, (s) => `d${s}`), `d${now - 9 * 86400}`);
  assert.equal(suggestionText({ insert: "light", original: "lamp", state: "open", stale: false }), "Suggestion: replace “lamp” with “light”");
  assert.equal(suggestionText({ insert: "", original: "very  old", state: "open", stale: false }), "Suggestion: delete “very old”");
  assert.equal(suggestionText({ insert: "new", original: "", state: "open", stale: false }), "Suggestion: insert “new”");
  assert.match(suggestionText({ insert: "x".repeat(100), original: "y", state: "open", stale: false }), /x…”$/);
});

test("ranges: mapped to the editor, clamped, detached ones empty, colours checked", () => {
  const items = [item({ id: 1, from: 2, to: 6, colour: "blue" }), item({ id: 2, from: 8, to: 8, detached: true }), item({ id: 3, from: 4, to: 50, colour: "purple", resolved: true })];
  const marks = marksOf(items, (p) => p + 1, 20, (i) => !i.resolved);
  assert.deepEqual(marks, [
    { id: 1, from: 3, to: 7, kind: "comment", colour: "blue", shown: true },
    { id: 2, from: 9, to: 9, kind: "comment", colour: null, shown: true },
    { id: 3, from: 5, to: 20, kind: "comment", colour: null, shown: false },
  ]);
  // The innermost shown range at a position, ends included; hidden and empty ones never.
  const nested = [...marks, { id: 4, from: 4, to: 6, kind: "mark" as const, colour: "pink" as const, shown: true }];
  assert.equal(markAt(nested, 5)?.id, 4);
  assert.equal(markAt(nested, 3)?.id, 1);
  assert.equal(markAt(nested, 7)?.id, 1);
  assert.equal(markAt(nested, 9), null);
  assert.equal(markAt(nested, 15), null);
});

test("the field maps ranges over edits (typing at the ends stays outside) and follows the caret", () => {
  let state = EditorState.create({ doc: "Hello brave new world", extensions: reviewField });
  state = state.update({ effects: setReviewMarks([{ id: 5, from: 6, to: 11, kind: "comment", colour: null, shown: true }]) }).state;
  const range = () => state.field(reviewField).marks[0];
  state = state.update({ changes: [{ from: 6, insert: "very " }, { from: 11, insert: "!" }] }).state;
  assert.equal(state.sliceDoc(range().from, range().to), "brave");
  state = state.update({ changes: { from: 13, insert: "st" } }).state;
  assert.equal(state.sliceDoc(range().from, range().to), "brstave");
  // All of its text deleted: an empty range where it was.
  state = state.update({ changes: { from: 11, to: 18 } }).state;
  assert.deepEqual([range().from, range().to], [11, 11]);

  // The caret activates the item under it, a selection elsewhere none; the panel's own selections and effects decide themselves.
  state = state.update({ effects: setReviewMarks([{ id: 5, from: 0, to: 5, kind: "mark", colour: "yellow", shown: true }]) }).state;
  state = state.update({ selection: EditorSelection.cursor(2) }).state;
  assert.equal(state.field(reviewField).active, 5);
  state = state.update({ selection: EditorSelection.cursor(9) }).state;
  assert.equal(state.field(reviewField).active, null);
  state = state.update({ selection: EditorSelection.cursor(9), effects: setReviewActive(DRAFT) }).state;
  assert.equal(state.field(reviewField).active, DRAFT);
  state = state.update({ selection: EditorSelection.cursor(1), userEvent: REVIEW_SELECT }).state;
  assert.equal(state.field(reviewField).active, DRAFT);
});

const box = (id: number, want: number, height = 50): CardBox => ({ id, want, height });
const tops = (m: Map<number, number>) => [...m.entries()].sort((a, b) => a[0] - b[0]).map(([, top]) => top);

test("cards stack at their lines, pushed down to clear the one above", () => {
  assert.deepEqual(tops(stackCards([box(1, 0), box(2, 200), box(3, 400)], null, 8)), [0, 200, 400]);
  assert.deepEqual(tops(stackCards([box(1, 100), box(2, 110), box(3, 120)], null, 8)), [100, 158, 216]);
  // Order by line, not by input; never above the top.
  assert.deepEqual(tops(stackCards([box(2, 110), box(1, -20)], null, 8)), [0, 110]);
  assert.deepEqual(stackCards([], null), new Map());
});

test("the active card stays at its line, the ones above move up as far as the top allows", () => {
  assert.deepEqual(tops(stackCards([box(1, 100), box(2, 110), box(3, 120)], 3, 8)), [4, 62, 120]);
  // Not enough room above: from the top down, the active card as close as it gets.
  assert.deepEqual(tops(stackCards([box(1, 10), box(2, 20), box(3, 30)], 3, 8)), [0, 58, 116]);
  // Cards below the active one are pushed down.
  assert.deepEqual(tops(stackCards([box(1, 100), box(2, 110), box(3, 120)], 2, 8)), [52, 110, 168]);
});

test("cards past the end move up as far as there is room", () => {
  assert.deepEqual(tops(stackCards([box(1, 0), box(2, 380), box(3, 390)], null, 8, 400)), [0, 292, 350]);
  // More cards than room: they start at the top and run past the end.
  assert.deepEqual(tops(stackCards([box(1, 10), box(2, 20), box(3, 30)], null, 8, 100)), [0, 58, 116]);
});

// ---- Suggest mode ----

const open = (insert: string, original: string, over: Partial<ReviewItem> = {}): ReviewItem =>
  item({ kind: "suggestion", suggestion: { insert, original, state: "open", stale: false }, ...over });

test("rights: Accept for owners and editors on fresh suggestions, Reject for the author too", () => {
  const beas = open("a", "b", { author: author(BEA) });
  const mine = open("a", "b");
  assert.deepEqual(suggestionRights(beas, ME, OWNER), { accept: true, why: "", reject: true });
  // A reviewer never accepts (with a reason), rejects its own only.
  assert.equal(suggestionRights(mine, ME, REVIEWER).accept, false);
  assert.match(suggestionRights(mine, ME, REVIEWER).why, /Owner or Editor/);
  assert.equal(suggestionRights(mine, ME, REVIEWER).reject, true);
  assert.equal(suggestionRights(beas, ME, REVIEWER).reject, false);
  assert.deepEqual(suggestionRights(mine, ME, READER), { accept: false, why: "Only an Owner or Editor accepts suggestions.", reject: false });
  // Stale or detached: no Accept, still Reject.
  const stale = open("a", "b", { suggestion: { insert: "a", original: "b", state: "open", stale: true } });
  assert.deepEqual([suggestionRights(stale, ME, OWNER).accept, suggestionRights(stale, ME, OWNER).reject], [false, true]);
  assert.match(suggestionRights(stale, ME, OWNER).why, /changed/);
  assert.match(suggestionRights(open("a", "b", { detached: true }), ME, OWNER).why, /deleted/);
  // Settled ones and other items offer nothing.
  const accepted = item({ kind: "suggestion", suggestion: { insert: "a", original: "b", state: "accepted", stale: false } });
  assert.deepEqual(suggestionRights(accepted, ME, OWNER), { accept: false, why: "", reject: false });
  assert.deepEqual(suggestionRights(item(), ME, OWNER), { accept: false, why: "", reject: false });
});

test("undo goes to the review in suggest mode", () => {
  assert.equal(historyTarget({ field: false, panel: false, suggest: true }, OWNER), "review");
  assert.equal(historyTarget({ field: true, panel: false, suggest: true }, OWNER), "field");
  assert.equal(historyTarget({ field: false, panel: false, suggest: false }, OWNER), "main");
});

test("a transaction's changes become suggest calls, last first", () => {
  const changes = ChangeSet.of([{ from: 1, insert: "x" }, { from: 4, to: 6 }, { from: 8, to: 9, insert: "yz" }], 10);
  assert.deepEqual(suggestCalls(changes), [
    { from: 8, to: 9, insert: "yz" },
    { from: 4, to: 6, insert: "" },
    { from: 1, to: 1, insert: "x" },
  ]);
});

const TEXT = "Hello brave new world";
const slice = (f: number, t: number) => TEXT.slice(f, t);
let fresh = 100;
const next = () => ({ id: ++fresh, created: 500 });
/** Runs suggest calls through the merge rules from `items`. */
function suggestAll(items: ReviewItem[], calls: SuggestCall[]): ReviewItem[] {
  for (const c of calls) items = withSuggested(items, suggestInto(items, author(ME), c, slice, next));
  return items;
}
const brief = (items: ReviewItem[]) => items.map((i) => [i.from, i.to, i.suggestion?.insert, i.suggestion?.original]);

test("merge rules: typing appends, backspace after typed text takes it back, deleting next to it widens", () => {
  // Typing at the end (rule 1); the first key makes a new suggestion.
  assert.deepEqual(brief(suggestAll([], [{ from: 6, to: 6, insert: "very " }, { from: 6, to: 6, insert: "!" }])), [[6, 6, "very !", ""]]);
  // A backspace after typed text shrinks it (rule 2); all of it: gone.
  let items = suggestAll([], [{ from: 6, to: 6, insert: "ab" }, { from: 5, to: 6, insert: "" }]);
  assert.deepEqual(brief(items), [[6, 6, "a", ""]]);
  assert.deepEqual(suggestInto(items, author(ME), { from: 5, to: 6, insert: "" }, slice, next), { rule: "shrink", own: items[0].id, item: null });
  // Repeated Backspace over real text widens the struck range (rule 3), Delete too.
  items = suggestAll([], [{ from: 10, to: 11, insert: "" }, { from: 9, to: 10, insert: "" }, { from: 11, to: 12, insert: "" }]);
  assert.deepEqual(brief(items), [[9, 12, "", "ve "]]);
  // Typing inside or at the end of a deletion: its replacement.
  assert.deepEqual(brief(suggestAll(items, [{ from: 12, to: 12, insert: "ry" }])), [[9, 12, "ry", "ve "]]);
  // Replacing a selection: one suggestion; typing at its end goes on with it.
  assert.deepEqual(brief(suggestAll([], [{ from: 6, to: 11, insert: "b" }, { from: 11, to: 11, insert: "old" }])), [[6, 11, "bold", "brave"]]);
  // Deleting inside the range changes nothing.
  items = suggestAll([], [{ from: 6, to: 11, insert: "" }]);
  assert.deepEqual(brief(suggestAll(items, [{ from: 7, to: 8, insert: "" }])), [[6, 11, "", "brave"]]);
  // Nothing at all: no call.
  assert.equal(suggestInto([], author(ME), { from: 3, to: 3, insert: "" }, slice, next).rule, "none");
});

test("merge rules: only own open suggestions merge", () => {
  const beas = open("x", "", { id: 1, from: 6, to: 6, author: author(BEA) });
  const resolved = open("x", "", { id: 2, from: 6, to: 6, resolved: true });
  const rejected = item({ id: 3, kind: "suggestion", from: 6, to: 6, suggestion: { insert: "x", original: "", state: "rejected", stale: false } });
  const detached = open("", "brave", { id: 4, from: 6, to: 6, detached: true });
  for (const other of [beas, resolved, rejected, detached]) assert.equal(suggestInto([other], author(ME), { from: 6, to: 6, insert: "a" }, slice, next).rule, "new");
  // The newest own one wins.
  const older = open("a", "", { id: 5, from: 6, to: 6, created: 10 });
  const newer = open("b", "", { id: 6, from: 6, to: 6, created: 20 });
  assert.equal(suggestInto([older, newer], author(ME), { from: 6, to: 6, insert: "c" }, slice, next).own, 6);
});

test("the caret: at the insertion point after typing, at the deletion's start after Backspace (unless typed text went), at its end after Delete", () => {
  assert.equal(suggestCaret({ from: 6, to: 6, insert: "a" }, "new", false), 6);
  assert.equal(suggestCaret({ from: 6, to: 11, insert: "b" }, "new", false), 11);
  assert.equal(suggestCaret({ from: 5, to: 6, insert: "" }, "widen", false), 5);
  assert.equal(suggestCaret({ from: 5, to: 6, insert: "" }, "shrink", false), 6);
  assert.equal(suggestCaret({ from: 6, to: 7, insert: "" }, "new", true), 7);
});

test("suggest mode: edits leave the text, move the caret and carry the calls; the core's changes pass", () => {
  const list = { me: ME, items: [] as ReviewItem[], undo: false, redo: false };
  let state = EditorState.create({ doc: TEXT, selection: EditorSelection.cursor(6), extensions: [suggestMode(() => list), reviewField] });
  let tr = state.update({ changes: { from: 6, insert: "a" }, selection: EditorSelection.cursor(7), userEvent: "input.type" });
  assert.equal(tr.state.doc.toString(), TEXT);
  assert.equal(tr.state.selection.main.head, 6);
  const typed = suggestionsOf(tr)!;
  assert.deepEqual(typed.calls, [{ from: 6, to: 6, insert: "a" }]);
  assert.deepEqual(brief(typed.items!), [[6, 6, "a", ""]]);
  assert.equal(typed.split, false);
  list.items = typed.items!;
  state = tr.state;
  // Backspace right after typed text: the caret stays; over real text: to the deletion's start.
  tr = state.update({ changes: { from: 5, to: 6 }, selection: EditorSelection.cursor(5), userEvent: "delete.backward" });
  assert.equal(tr.state.selection.main.head, 6);
  list.items = [];
  tr = state.update({ changes: { from: 5, to: 6 }, selection: EditorSelection.cursor(5), userEvent: "delete.backward" });
  assert.equal(tr.state.selection.main.head, 5);
  tr = state.update({ changes: { from: 6, to: 7 }, userEvent: "delete.forward" });
  assert.equal(tr.state.selection.main.head, 7);
  // A paste starts a step of its own; several changes: the selection stays.
  tr = state.update({ changes: [{ from: 0, insert: "*" }, { from: 5, insert: "*" }], selection: EditorSelection.range(0, 7), userEvent: "input.paste" });
  assert.deepEqual(
    suggestionsOf(tr)!.calls.map((c) => c.from),
    [5, 0],
  );
  assert.equal(suggestionsOf(tr)!.split, true);
  assert.deepEqual([tr.state.selection.main.from, tr.state.selection.main.to], [6, 6]);
  // The core's changes pass; selection-only transactions carry nothing.
  tr = state.update({ changes: { from: 0, insert: "Oh. " }, annotations: fromCore.of(true) });
  assert.equal(tr.state.doc.toString(), "Oh. " + TEXT);
  assert.equal(suggestionsOf(tr), null);
  assert.equal(suggestionsOf(state.update({ selection: EditorSelection.cursor(2) })), null);
  // Nothing listed yet: the calls go, no items to show.
  assert.equal(suggestionsOf(EditorState.create({ doc: TEXT, extensions: suggestMode(() => null) }).update({ changes: { from: 0, insert: "a" } }))!.items, null);
});

test("only open suggestions show in the text: struck range, inserted text after it, the author on hover", () => {
  const who = (a: { user: string } | null) => ({ label: a?.user === ME ? "You" : "Bea", colour: a?.user === ME ? "#111111" : "#222222" });
  const items = [
    open("reached", "travelled", { id: 1, from: 2, to: 11, author: author(BEA) }),
    open("", "very ", { id: 2, from: 12, to: 17 }),
    open("new ", "", { id: 3, from: 20, to: 20 }),
    item({ id: 4, kind: "suggestion", from: 22, to: 25, suggestion: { insert: "x", original: "abc", state: "rejected", stale: false } }),
  ];
  const marks = marksOf(items, (p) => p, 40, () => true, who);
  assert.deepEqual(marks[0].suggestion, { insert: "reached", colour: "#222222", title: "Bea · Suggestion: replace “travelled” with “reached”" });
  assert.equal(marks[3].suggestion, undefined);
  assert.deepEqual(suggestionParts(marks[0]), { strike: { from: 2, to: 11 }, insert: { at: 11, text: "reached" } });
  assert.deepEqual(suggestionParts(marks[1]), { strike: { from: 12, to: 17 }, insert: null });
  assert.deepEqual(suggestionParts(marks[2]), { strike: null, insert: { at: 20, text: "new " } });
  assert.deepEqual(suggestionParts(marks[3]), { strike: null, insert: null });
  // Found at the click or caret, an insertion point too; settled and hidden ones never.
  assert.equal(suggestionAt(marks, 5)?.id, 1);
  assert.equal(suggestionAt(marks, 20)?.id, 3);
  assert.equal(suggestionAt(marks, 23), null);
  assert.equal(suggestionAt(marksOf(items, (p) => p, 40, () => false, who), 5), null);
  // An insertion point activates its card too.
  assert.equal(markAt(marks, 20)?.id, 3);
});
