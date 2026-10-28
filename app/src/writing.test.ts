// Tests for focus paragraphs, word counting and checkpoint times. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { Text } from "@codemirror/state";
import { paragraphAt } from "./focus.ts";
import { countWords } from "./words.ts";
import { relativeTime } from "./checkpoints.ts";

const doc = (s: string) => Text.of(s.split("\n"));

test("paragraph = run of non-blank lines around the cursor", () => {
  const d = doc("= Title\n\nOne\ntwo\nthree\n  \nFour");
  assert.deepEqual(paragraphAt(d, 0), { from: 1, to: 1 });
  assert.deepEqual(paragraphAt(d, d.line(4).from + 1), { from: 3, to: 5 });
  assert.deepEqual(paragraphAt(d, d.line(3).from), { from: 3, to: 5 });
  assert.deepEqual(paragraphAt(d, d.length), { from: 7, to: 7 });
});

test("a blank line is its own paragraph", () => {
  const d = doc("a\n\nb");
  assert.deepEqual(paragraphAt(d, 2), { from: 2, to: 2 });
  const ws = doc("a\n \t\nb");
  assert.deepEqual(paragraphAt(ws, 3), { from: 2, to: 2 });
});

test("paragraph scan is bounded", () => {
  const d = doc(Array.from({ length: 50 }, (_, i) => `line ${i}`).join("\n"));
  assert.deepEqual(paragraphAt(d, d.line(25).from, 10), { from: 15, to: 35 });
  assert.deepEqual(paragraphAt(d, 0), { from: 1, to: 50 });
});

test("word count: whitespace-delimited", () => {
  assert.equal(countWords(doc("")), 0);
  assert.equal(countWords(doc("   ")), 0);
  assert.equal(countWords(doc("one")), 1);
  assert.equal(countWords(doc("  one  two\tthree\n\nfour ")), 4);
  assert.equal(countWords(doc("= Title\nA *bold* move.")), 5);
  assert.equal(countWords(doc("non breaking em space 😀")), 5);
});

test("word count over a large multi-chunk document", () => {
  const para = "Lorem ipsum dolor sit amet, consectetur adipiscing elit.";
  const d = doc(Array.from({ length: 20000 }, () => para).join("\n\n"));
  assert.equal(countWords(d), 20000 * 8);
});

test("relative checkpoint times", () => {
  const now = Date.UTC(2026, 0, 10, 12, 0, 0);
  const ago = (s: number) => relativeTime(now / 1000 - s, now);
  assert.equal(ago(5), "just now");
  assert.equal(ago(-30), "just now");
  assert.equal(ago(60), "1 min ago");
  assert.equal(ago(5 * 60), "5 min ago");
  assert.equal(ago(3600), "1 hour ago");
  assert.equal(ago(5 * 3600), "5 hours ago");
  assert.equal(ago(26 * 3600), "yesterday");
  assert.equal(ago(4 * 86400), "4 days ago");
});
