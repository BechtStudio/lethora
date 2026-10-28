// Tests for the Exceptions drawer's chapters, grouping, filters and selection. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { type OverrideInfo, rowState } from "./exceptions.ts";
import { BEFORE_KEY, bulkKind, chapterAt, chapterStarts, type Filter, groupByChapter, INVALID_KEY, isFiltered, matches, rangeIds } from "./exceptionsdrawer.ts";

const TEXT = [
  "Preface text.", // 0
  "", // 14
  "= The Keeper <ch:keeper>", // 15
  "", // 40
  "Mara counted the steps.", // 41
  "== A Section", // 65
  "```", // 78
  "= not a chapter", // 82
  "```", // 98
  "= Second", // 102
  "", // 111
  "= The Keeper", // 112
].join("\n");

const info = (over: Partial<OverrideInfo>): OverrideInfo => ({
  id: "a1",
  kind: "page_break",
  params: { to_odd: false },
  anchor_preview: "= The Keeper",
  pos_utf16: 15,
  detached: false,
  applicable: true,
  error: null,
  ...over,
});

test("chapters: level-1 headings outside raw blocks, labels dropped, keys unique", () => {
  const chapters = chapterStarts(TEXT);
  assert.deepEqual(
    chapters.map((c) => [c.pos, c.title]),
    [
      [15, "The Keeper"],
      [102, "Second"],
      [112, "The Keeper"],
    ],
  );
  assert.equal(TEXT.slice(102, 110), "= Second");
  assert.equal(new Set(chapters.map((c) => c.key)).size, 3);
  // The same text gives the same keys (collapsed groups stay collapsed).
  assert.deepEqual(chapterStarts(`More.\n${TEXT}`).map((c) => c.key), chapters.map((c) => c.key));
  assert.deepEqual(chapterStarts("=\n= \n==x\n=x").map((c) => c.title), ["Untitled chapter"]);
  assert.deepEqual(chapterStarts("```raw```\n= After"), [{ pos: 10, title: "After", key: "ch:1:After" }]);
  assert.deepEqual(chapterStarts(""), []);
});

test("chapter of a position: binary search, the heading line itself included", () => {
  const chapters = chapterStarts(TEXT);
  assert.equal(chapterAt(chapters, 0), -1);
  assert.equal(chapterAt(chapters, 14), -1);
  assert.equal(chapterAt(chapters, 15), 0);
  assert.equal(chapterAt(chapters, 101), 0);
  assert.equal(chapterAt(chapters, 102), 1);
  assert.equal(chapterAt(chapters, 10_000), 2);
  assert.equal(chapterAt([], 5), -1);
});

test("groups: document order under chapters, before the first one, invalid last", () => {
  const chapters = chapterStarts(TEXT);
  const list = [
    info({ id: "inv", kind: null, pos_utf16: null, error: "bad" }),
    info({ id: "c", pos_utf16: 112 }),
    info({ id: "b2", pos_utf16: 41 }),
    info({ id: "b1", pos_utf16: 15 }),
    info({ id: "pre", pos_utf16: 0 }),
    info({ id: "b0", pos_utf16: 41 }),
  ];
  const groups = groupByChapter(list, chapters);
  assert.deepEqual(
    groups.map((g) => [g.key, g.title, g.items.map((o) => o.id)]),
    [
      [BEFORE_KEY, "Before the first chapter", ["pre"]],
      [chapters[0].key, "The Keeper", ["b1", "b0", "b2"]],
      [chapters[2].key, "The Keeper", ["c"]],
      [INVALID_KEY, "Invalid", ["inv"]],
    ],
  );
  assert.deepEqual(groupByChapter([], chapters), []);
  // No chapters: everything is before the first one.
  assert.deepEqual(groupByChapter([info({})], []).map((g) => g.key), [BEFORE_KEY]);
});

test("filters: kind chips, needs attention, search over anchored text and label", () => {
  const f = (over: Partial<Filter>): Filter => ({ kinds: new Set(), attention: false, query: "", ...over });
  const pb = info({});
  const cb = info({ id: "cb", kind: "column_break", params: {}, detached: true, anchor_preview: "The keepers before them" });
  const sp = info({ id: "sp", kind: "space_before", params: { length: "18pt" }, anchor_preview: "== Light" });
  const img = info({ id: "iq", kind: "image_quality", params: { quality: "screen" }, applicable: false, anchor_preview: 'image("a.png")' });
  const bad = info({ id: "zz", kind: null, params: {}, anchor_preview: "", pos_utf16: null, error: "bad" });
  const all = [pb, cb, sp, img, bad];
  const pick = (filter: Filter) => all.filter((o) => matches(o, rowState(o), filter)).map((o) => o.id);

  assert.deepEqual(pick(f({})), ["a1", "cb", "sp", "iq", "zz"]);
  assert.equal(isFiltered(f({})), false);
  assert.equal(isFiltered(f({ query: "  " })), false);
  assert.deepEqual(pick(f({ kinds: new Set(["break"]) })), ["a1", "cb"]);
  assert.deepEqual(pick(f({ kinds: new Set(["space", "image"]) })), ["sp", "iq"]);
  assert.deepEqual(pick(f({ kinds: new Set(["figure"]) })), []);
  assert.deepEqual(pick(f({ attention: true })), ["cb", "iq", "zz"]);
  assert.deepEqual(pick(f({ attention: true, kinds: new Set(["break"]) })), ["cb"]);
  assert.deepEqual(pick(f({ query: "KEEPER" })), ["a1", "cb"]);
  assert.deepEqual(pick(f({ query: "18pt" })), ["sp"]);
  assert.deepEqual(pick(f({ query: " screen " })), ["iq"]);
  assert.ok(isFiltered(f({ attention: true })));
});

test("range selection and bulk kinds", () => {
  const order = ["a", "b", "c", "d"];
  assert.deepEqual(rangeIds(order, "b", "d"), ["b", "c", "d"]);
  assert.deepEqual(rangeIds(order, "d", "b"), ["b", "c", "d"]);
  assert.deepEqual(rangeIds(order, "c", "c"), ["c"]);
  assert.deepEqual(rangeIds(order, null, "c"), ["c"]);
  assert.deepEqual(rangeIds(order, "gone", "a"), ["a"]);
  assert.deepEqual(rangeIds(order, "a", "gone"), []);

  const list = [
    info({ id: "s1", kind: "space_before", params: { length: "1em" } }),
    info({ id: "s2", kind: "space_before", params: { length: "2em" } }),
    info({ id: "p1" }),
    info({ id: "c1", kind: "column_break", params: {} }),
    info({ id: "c2", kind: "column_break", params: {} }),
    info({ id: "zz", kind: null, params: {}, error: "bad" }),
  ];
  assert.equal(bulkKind(list, new Set(["s1", "s2"])), "space_before");
  assert.equal(bulkKind(list, new Set(["s1"])), null);
  assert.equal(bulkKind(list, new Set(["s1", "p1"])), null);
  assert.equal(bulkKind(list, new Set(["c1", "c2"])), null);
  assert.equal(bulkKind(list, new Set(["s1", "zz"])), null);
  assert.equal(bulkKind(list, new Set(["s1", "gone"])), null);
});
