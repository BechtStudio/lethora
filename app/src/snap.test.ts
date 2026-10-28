// Tests for Snap Layouts on the maximize button (chrome.ts). Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { snapLayouts, type SnapRect } from "./chrome.ts";

/** A fake window (ratio, resize and ratio-change listeners) and maximize button. */
function setup(dpr: number, rect: { left: number; top: number; right: number; bottom: number }) {
  const g = globalThis as Record<string, unknown>;
  const resize: Array<() => void> = [];
  const ratio: Array<() => void> = [];
  g.devicePixelRatio = dpr;
  g.addEventListener = (_: string, f: () => void) => resize.push(f);
  g.matchMedia = () => ({ addEventListener: (_: string, f: () => void) => ratio.push(f) });
  g.ResizeObserver = class {
    observe() {}
  };
  const classes = new Set<string>();
  const button = {
    clicks: 0,
    classList: { add: (...c: string[]) => c.forEach((n) => classes.add(n)), remove: (...c: string[]) => c.forEach((n) => classes.delete(n)) },
    getBoundingClientRect: () => rect,
    click: () => button.clicks++,
  };
  const reports: SnapRect[] = [];
  let handler: (state: string) => void = () => {};
  snapLayouts(
    button as unknown as HTMLElement,
    async (r) => reports.push(r),
    (h) => (handler = h),
    (e) => assert.fail(String(e)),
  );
  return { g, resize, ratio, classes, button, reports, state: (s: string) => handler(s) };
}

test("reports the button in physical pixels, edges rounded, once per change", () => {
  const t = setup(1.25, { left: 100.3, top: 0, right: 146.3, bottom: 32 });
  assert.deepEqual(t.reports, [{ x: 125, y: 0, width: 58, height: 40 }]);
  t.resize.forEach((f) => f());
  assert.equal(t.reports.length, 1);
  // A new ratio (monitor DPI or zoom) reports again and watches the next one.
  t.g.devicePixelRatio = 1.5;
  t.ratio.shift()!();
  assert.deepEqual(t.reports[1], { x: 150, y: 0, width: 69, height: 48 });
  assert.equal(t.ratio.length, 1);
});

test("the overlay's states show on the button; a click clicks it", () => {
  const t = setup(1, { left: 0, top: 0, right: 46, bottom: 32 });
  t.state("hover");
  t.state("press");
  assert.deepEqual([...t.classes].sort(), ["hover", "pressed"]);
  t.state("click");
  assert.deepEqual([...t.classes], ["hover"]);
  assert.equal(t.button.clicks, 1);
  t.state("press");
  t.state("leave");
  assert.deepEqual([...t.classes], []);
  assert.equal(t.button.clicks, 1);
});
