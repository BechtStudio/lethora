// Tests for the page preview math and the page image cache. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  anchorAt,
  layoutPages,
  METRICS,
  PageCache,
  type PageCacheDeps,
  pageAt,
  pagePoint,
  type PageSize,
  PX_PER_PT,
  scrollFor,
  stepZoom,
  visibleRange,
  wantedPages,
  WHEEL_STEP,
  wheelZoom,
  zoomScale,
} from "./pages.ts";

const A4: PageSize = [595.28, 841.89];
const LETTER: PageSize = [612, 792];
const TRADE: PageSize = [432, 648];
const pages = (n: number, size: PageSize = A4): PageSize[] => Array.from({ length: n }, () => size);

test("layout: pages stacked with labels and gaps, padding around", () => {
  const m = METRICS;
  const l = layoutPages([A4, TRADE, A4], 1, m);
  assert.equal(l.tops[0], m.padY);
  assert.equal(l.tops[1], m.padY + A4[1] + m.label + m.gap);
  assert.equal(l.tops[2], l.tops[1] + TRADE[1] + m.label + m.gap);
  assert.equal(l.total, l.tops[2] + A4[1] + m.label + m.padY);
  assert.equal(l.width, A4[0] + 2 * m.padX);
  assert.deepEqual(layoutPages([], 1).tops.length, 0);
});

test("zoom: fit width, fixed factors clamped to 50-200 %, steps", () => {
  const fit = zoomScale("fit", pages(3), 800);
  assert.ok(Math.abs(fit - (800 - 2 * METRICS.padX) / A4[0]) < 1e-9);
  assert.equal(zoomScale(1, pages(3), 800), PX_PER_PT);
  assert.equal(zoomScale(5, pages(3), 800), 2 * PX_PER_PT);
  assert.equal(zoomScale(0.1, pages(3), 800), 0.5 * PX_PER_PT);
  // Without pages, fit width assumes A4.
  assert.ok(zoomScale("fit", [], 800) > 0);
  assert.equal(stepZoom(1, 1), 1.1);
  assert.equal(stepZoom(1, -1), 0.9);
  assert.equal(stepZoom(1.03, 1), 1.1);
  assert.equal(stepZoom(1.03, -1), 1);
  assert.equal(stepZoom(2, 1), 2);
  assert.equal(stepZoom(0.5, -1), 0.5);
  assert.equal(stepZoom(0.3, 1), 0.5);
});

test("ctrl+wheel: one step per notch, small deltas add up, a turn starts over", () => {
  // A mouse notch (100 px): one step each way (wheel up zooms in).
  assert.deepEqual(wheelZoom(0, -100), { step: 1, total: 0 });
  assert.deepEqual(wheelZoom(0, 100), { step: -1, total: 0 });
  // A big flick is still one step.
  assert.deepEqual(wheelZoom(0, -1000), { step: 1, total: 0 });
  // Touchpad deltas add up to a step.
  let total = 0;
  const steps: number[] = [];
  for (let i = 0; i < 12; i++) {
    const r = wheelZoom(total, -10);
    total = r.total;
    if (r.step) steps.push(r.step);
  }
  assert.deepEqual(steps, [1, 1]);
  assert.equal(total, -20);
  // Turning back drops what had added up.
  assert.deepEqual(wheelZoom(-(WHEEL_STEP - 1), 10), { step: 0, total: 10 });
});

test("visible range: binary search over 2000 pages, overscan, gaps, ends", () => {
  const l = layoutPages(pages(2000), 1);
  const slot = A4[1] + METRICS.label + METRICS.gap;
  // Viewport starting inside page 1000, as tall as one page.
  const top = l.tops[1000] + 100;
  assert.deepEqual(visibleRange(l, top, A4[1]), { first: 1000, last: 1001 });
  assert.deepEqual(visibleRange(l, top, A4[1], A4[1]), { first: 999, last: 1002 });
  // In the gap after page 10's label: page 10 is out, 11 is in.
  const gapY = l.tops[10] + A4[1] + METRICS.label + 2;
  assert.equal(visibleRange(l, gapY, 100).first, 11);
  // A viewport entirely inside a gap shows nothing.
  const empty = visibleRange(l, gapY, 3);
  assert.ok(empty.first > empty.last);
  // Top and bottom of the column.
  assert.deepEqual(visibleRange(l, 0, 300), { first: 0, last: 0 });
  assert.equal(visibleRange(l, l.total - 300, 300).last, 1999);
  assert.equal(pageAt(l, l.tops[1234] + slot - 1), 1234);
  assert.deepEqual(visibleRange(layoutPages([], 1), 0, 500), { first: 0, last: -1 });
});

test("wanted pages: visible ones top-down first, then nearest overscan", () => {
  const l = layoutPages(pages(50), 0.5);
  const top = l.tops[20];
  const order = wantedPages(l, top, 2 * l.heights[0], 2 * l.heights[0]);
  const seen = visibleRange(l, top, 2 * l.heights[0]);
  assert.deepEqual(order.slice(0, seen.last - seen.first + 1), Array.from({ length: seen.last - seen.first + 1 }, (_, i) => seen.first + i));
  assert.equal(order[seen.last - seen.first + 1], seen.last + 1);
  assert.equal(order[seen.last - seen.first + 2], seen.first - 1);
  assert.equal(new Set(order).size, order.length);
});

test("scroll anchor: kept across zoom, page size changes and fewer pages", () => {
  const before = layoutPages(pages(2000), 1);
  const scrollTop = before.tops[742] + 0.3 * before.heights[742];
  const anchor = anchorAt(before, scrollTop);
  assert.equal(anchor.index, 742);
  assert.ok(Math.abs(anchor.fraction - 0.3) < 1e-9);

  for (const after of [layoutPages(pages(2000), 1.7), layoutPages(pages(2100, LETTER), 1), layoutPages(pages(1990, TRADE), 0.6)]) {
    const y = scrollFor(after, anchor);
    const again = anchorAt(after, y);
    assert.equal(again.index, 742);
    assert.ok(Math.abs(again.fraction - 0.3) < 1e-9);
  }
  // The page is gone: the last page instead, clamped to the column.
  const short = layoutPages(pages(100), 1);
  const viewport = 900;
  const y = scrollFor(short, anchor, viewport);
  assert.ok(y <= short.total - viewport && y >= short.tops[99] - viewport);
  assert.equal(scrollFor(layoutPages([], 1), anchor), 0);
  // Above the first page (in the padding) stays at the top.
  assert.equal(scrollFor(before, anchorAt(before, 5)), 5);
});

test("2000 pages: layout and range lookups are cheap", () => {
  const sizes = pages(2000);
  const start = performance.now();
  for (let i = 0; i < 100; i++) {
    const l = layoutPages(sizes, 1 + i / 100);
    visibleRange(l, i * 1000, 900, 900);
  }
  const ms = (performance.now() - start) / 100;
  assert.ok(ms < 5, `${ms.toFixed(3)} ms per layout + lookup`);
});

// ---- Page cache ----

interface Call {
  version: number;
  index: number;
  resolve: (b: ArrayBuffer) => void;
  reject: (e: unknown) => void;
}

function harness(options: Partial<PageCacheDeps> = {}) {
  const calls: Call[] = [];
  const revoked: string[] = [];
  const loaded: number[] = [];
  const failures: number[] = [];
  let n = 0;
  const cache = new PageCache({
    fetch: (version, index) => new Promise((resolve, reject) => calls.push({ version, index, resolve, reject })),
    toUrl: () => `blob:${++n}`,
    revoke: (url) => revoked.push(url),
    onLoad: (index) => loaded.push(index),
    onFailure: (version) => failures.push(version),
    ...options,
  });
  const take = (version: number, index: number) => {
    const i = calls.findIndex((c) => c.version === version && c.index === index);
    assert.ok(i >= 0, `no request for ${version}:${index} (have ${calls.map((c) => `${c.version}:${c.index}`).join(", ")})`);
    return calls.splice(i, 1)[0];
  };
  return { cache, calls, revoked, loaded, failures, take };
}

const tick = () => new Promise((r) => setTimeout(r, 0));
const bytes = new ArrayBuffer(1);

test("cache: a new version replaces images only when they arrive; old URLs are revoked", async () => {
  const h = harness();
  h.cache.setVersion(1, 3);
  h.cache.want([0, 1]);
  await tick();
  h.take(1, 0).resolve(bytes);
  h.take(1, 1).resolve(bytes);
  await tick();
  assert.equal(h.cache.get(0), "blob:1");
  assert.ok(h.cache.isCurrent(0));
  assert.deepEqual(h.loaded, [0, 1]);

  h.cache.setVersion(2, 3);
  // Still shown, but no longer current.
  assert.equal(h.cache.get(0), "blob:1");
  assert.ok(!h.cache.isCurrent(0));
  h.cache.want([0]);
  await tick();
  h.take(2, 0).resolve(bytes);
  await tick();
  assert.equal(h.cache.get(0), "blob:3");
  assert.ok(h.cache.isCurrent(0));
  assert.deepEqual(h.revoked, ["blob:1"]);
  // Page 1 was still wanted when the version changed: it is on its way, the old image shows meanwhile.
  assert.equal(h.cache.get(1), "blob:2");
  assert.deepEqual(
    h.calls.map((c) => [c.version, c.index]),
    [[2, 1]],
  );
});

test("cache: a few requests at a time, wanted order, re-wishing drops queued pages", async () => {
  const h = harness({ concurrency: 2 });
  h.cache.setVersion(1, 100);
  h.cache.want([5, 6, 7, 8]);
  await tick();
  assert.deepEqual(
    h.calls.map((c) => c.index),
    [5, 6],
  );
  h.cache.want([50, 51]);
  h.take(1, 5).resolve(bytes);
  await tick();
  // The next free slot goes to the new wish, not to 7.
  assert.deepEqual(
    h.calls.map((c) => c.index),
    [6, 50],
  );
});

test("cache: stale versions are refetched at the latest; a failing latest calls compile_now once", async () => {
  const h = harness();
  h.cache.setVersion(1, 3);
  h.cache.want([0]);
  await tick();
  h.cache.setVersion(2, 3);
  h.take(1, 0).reject("Version 1 is not available.");
  await tick();
  assert.deepEqual(h.failures, []);
  h.take(2, 0).resolve(bytes);
  await tick();
  assert.ok(h.cache.isCurrent(0));

  // The latest version fails (e.g. the worker restarted): one compile_now, no retry loop.
  h.cache.setVersion(3, 3);
  h.cache.want([0, 1]);
  await tick();
  h.take(3, 0).reject("Version 3 is not available.");
  h.take(3, 1).reject("Version 3 is not available.");
  await tick();
  assert.deepEqual(h.failures, [3]);
  h.cache.want([0, 1]);
  await tick();
  assert.equal(h.calls.length, 0);
  // compile_now repeats the status: one more try, but no second compile_now.
  h.cache.setVersion(3, 3);
  await tick();
  h.take(3, 0).reject("again");
  h.take(3, 1).reject("again");
  await tick();
  assert.deepEqual(h.failures, [3]);
  // The old image stays on screen meanwhile.
  assert.equal(h.cache.get(0), "blob:1");
});

test("cache: an older version arriving late never replaces a newer image", async () => {
  const h = harness();
  h.cache.setVersion(1, 2);
  h.cache.want([0]);
  await tick();
  h.cache.setVersion(2, 2);
  await tick();
  h.take(2, 0).resolve(bytes);
  await tick();
  h.take(1, 0).resolve(bytes);
  await tick();
  assert.equal(h.cache.get(0), "blob:1");
  assert.ok(h.cache.isCurrent(0));
  assert.deepEqual(h.revoked, []);
});

test("cache: fewer pages, eviction beyond capacity (wanted pages kept), clear", async () => {
  const h = harness({ capacity: 3, concurrency: 10 });
  h.cache.setVersion(1, 10);
  h.cache.want([0, 1, 2, 3, 4]);
  await tick();
  for (const i of [0, 1, 2, 3, 4]) h.take(1, i).resolve(bytes);
  await tick();
  // All five are wanted: none evicted yet.
  assert.equal(h.cache.size, 5);
  h.cache.want([3, 4, 5]);
  await tick();
  h.take(1, 5).resolve(bytes);
  await tick();
  assert.equal(h.cache.size, 3);
  assert.deepEqual(h.revoked.sort(), ["blob:1", "blob:2", "blob:3"]);

  h.cache.setVersion(2, 4);
  assert.equal(h.cache.get(4), undefined);
  assert.equal(h.cache.get(5), undefined);
  assert.ok(h.revoked.includes("blob:5") && h.revoked.includes("blob:6"));

  h.cache.clear();
  assert.equal(h.cache.size, 0);
  assert.ok(h.revoked.includes("blob:4"));
  assert.equal(h.cache.current, null);
});

test("cache: without a version (crash) nothing is fetched and images stay", async () => {
  const h = harness();
  h.cache.setVersion(1, 2);
  h.cache.want([0]);
  await tick();
  h.take(1, 0).resolve(bytes);
  await tick();
  h.cache.setVersion(null, 2);
  h.cache.want([0, 1]);
  await tick();
  assert.equal(h.calls.length, 0);
  assert.equal(h.cache.get(0), "blob:1");
});

test("page point: clicks map to page index and points at any zoom and scroll offset", () => {
  const sizes = pages(3);
  for (const zoom of [0.5, 1, 2]) {
    const scale = zoom * PX_PER_PT;
    const l = layoutPages(sizes, scale);
    // A viewport narrower than the page at 200 % (scrolled sideways), wider below.
    const view = { left: 40, top: 60, width: 900, scrollLeft: 0, scrollTop: 0 };
    const contentWidth = Math.max(view.width, l.width);
    const pageLeft = Math.round((contentWidth - l.widths[1]) / 2);
    for (const [scrollLeft, scrollTop] of [
      [0, 0],
      [Math.max(0, contentWidth - view.width) / 2, l.tops[1] - 30],
      [Math.max(0, contentWidth - view.width), l.tops[1] + 200],
    ]) {
      const v = { ...view, scrollLeft, scrollTop };
      // The point (100pt, 300pt) on page 2, in client coordinates.
      const clientX = v.left - scrollLeft + pageLeft + 100 * scale;
      const clientY = v.top - scrollTop + l.tops[1] + 300 * scale;
      const p = pagePoint(l, v, clientX, clientY)!;
      assert.equal(p.page, 1, `zoom ${zoom}`);
      assert.ok(Math.abs(p.xPt - 100) < 1e-6 && Math.abs(p.yPt - 300) < 1e-6, `zoom ${zoom}: ${p.xPt}, ${p.yPt}`);
    }
    // Between pages (the label and gap) and beside the page: nothing.
    assert.equal(pagePoint(l, view, view.left + pageLeft + 5, view.top + l.tops[1] - 3), null);
    if (pageLeft > 2) assert.equal(pagePoint(l, view, view.left + pageLeft - 2, view.top + l.tops[0] + 10), null);
    // Top left corner of page 1.
    assert.deepEqual(pagePoint(l, view, view.left + pageLeft, view.top + l.tops[0]), { page: 0, xPt: 0, yPt: 0 });
  }
  assert.equal(pagePoint(layoutPages([], 1), { left: 0, top: 0, width: 500, scrollLeft: 0, scrollTop: 0 }, 10, 10), null);
});
