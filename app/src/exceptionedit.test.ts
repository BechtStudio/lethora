// Tests for the edit popover's placement. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { popoverPlace } from "./exceptionedit.ts";

const view = { width: 800, height: 600 };
const size = { width: 300, height: 200 };
const at = (left: number, top: number, w = 40, h = 16) => ({ left, top, right: left + w, bottom: top + h });

test("placement: below the anchor, left edges aligned", () => {
  assert.deepEqual(popoverPlace(at(100, 100), size, view), { left: 100, top: 120 });
});

test("placement: above the anchor when it does not fit below", () => {
  assert.deepEqual(popoverPlace(at(100, 500), size, view), { left: 100, top: 296 });
});

test("placement: fits neither below nor above, as low as fits", () => {
  assert.deepEqual(popoverPlace(at(100, 150), { width: 300, height: 500 }, view), { left: 100, top: 96 });
  assert.deepEqual(popoverPlace(at(100, 150), { width: 300, height: 700 }, view), { left: 100, top: 4 });
});

test("placement: kept inside the view horizontally", () => {
  assert.equal(popoverPlace(at(700, 100), size, view).left, 496);
  assert.equal(popoverPlace(at(-20, 100), size, view).left, 4);
  assert.equal(popoverPlace(at(100, 100), { width: 900, height: 200 }, view).left, 4);
});

test("placement: a point anchor (no marker shown)", () => {
  assert.deepEqual(popoverPlace({ left: 200, top: 300, right: 200, bottom: 300 }, size, view), { left: 200, top: 304 });
});
