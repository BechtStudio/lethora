// Tests for the split page pane's width math. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { clampFraction, fractionAt, paneWidth, parseFraction, SPLIT_MIN } from "./split.ts";

test("stored fraction: valid values kept, anything else is half", () => {
  assert.equal(parseFraction("0.35"), 0.35);
  assert.equal(parseFraction(null), 0.5);
  assert.equal(parseFraction(""), 0.5);
  assert.equal(parseFraction("abc"), 0.5);
  assert.equal(parseFraction("0"), 0.5);
  assert.equal(parseFraction("1"), 0.5);
  assert.equal(parseFraction("-0.2"), 0.5);
});

test("pane width: the fraction of the window within both minimums", () => {
  assert.equal(paneWidth(0.5, 1600), 800);
  assert.equal(paneWidth(0.1, 1600), SPLIT_MIN);
  assert.equal(paneWidth(0.95, 1600), 1600 - SPLIT_MIN);
  assert.equal(paneWidth(NaN, 1600), 800);
  // Exactly room for both minimums.
  assert.equal(paneWidth(0.9, 2 * SPLIT_MIN), SPLIT_MIN);
});

test("pane width: half each when both minimums do not fit", () => {
  assert.equal(paneWidth(0.8, 500), 250);
  assert.equal(paneWidth(0.2, 500), 250);
  assert.equal(paneWidth(0.5, 0), 0);
});

test("clamped fraction and the fraction at a handle position", () => {
  assert.equal(clampFraction(0.5, 1000), 0.5);
  assert.equal(clampFraction(0.9, 1000), 0.68);
  assert.equal(clampFraction(0.1, 1000), 0.32);
  assert.equal(clampFraction(0.7, 0), 0.5);
  assert.equal(fractionAt(400, 1000), 0.6);
  assert.equal(fractionAt(100, 1000), 0.68);
  assert.equal(fractionAt(990, 1000), 0.32);
  assert.equal(fractionAt(100, 0), 0.5);
});
