// Tests for exception markers: chips, geometry and version gating. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import type { OverrideInfo } from "./exceptions.ts";
import { BRACKET_GAP, BRACKET_W, chipText, COLUMN_RULE, MarkerPositions, markerOf, markersByPage, scaleBox } from "./markers.ts";
import type { PageSize } from "./pages.ts";
import type { CompileStatus, OverrideStatus } from "./status.ts";

const info = (id: string, kind: OverrideInfo["kind"], params: OverrideInfo["params"] = {}): OverrideInfo => ({
  id,
  kind,
  params,
  anchor_preview: "",
  pos_utf16: 0,
  detached: false,
  applicable: true,
  error: null,
});

const at = (id: string, over: Partial<OverrideStatus> = {}): OverrideStatus => ({ id, applied: true, detached: false, page: 1, x_pt: 70, y_pt: 100, ...over });

const A4: PageSize = [595, 842];

test("chip texts", () => {
  assert.equal(chipText(info("a", "page_break", { to_odd: false })), "Page break");
  assert.equal(chipText(info("a", "page_break", { to_odd: true })), "Right-hand page");
  assert.equal(chipText(info("a", "column_break")), "Column break");
  assert.equal(chipText(info("a", "space_before", { length: "18pt" })), "18pt");
  assert.equal(chipText(info("a", "figure_placement", { placement: "top" })), "Top");
  assert.equal(chipText(info("a", "figure_placement", { placement: "here" })), "Here");
  assert.equal(chipText(info("a", "figure_placement", { placement: "nope" })), "Figure");
  assert.equal(chipText(info("a", "table_style", { style: "striped" })), "Table: Striped");
  assert.equal(chipText(info("a", "image_quality", { quality: "email" })), "Image: Email");
  assert.equal(chipText(info("a", null)), "");
});

test("marker geometry in points", () => {
  // A page break: a rule across the text area (symmetric margins).
  assert.deepEqual(markerOf(info("p", "page_break"), at("p"), A4), { id: "p", kind: "page_break", page: 1, x: 70, y: 100, w: 455, h: 0, chip: "Page break", detached: false });
  // Never shorter than 40pt, even right of the middle.
  assert.equal(markerOf(info("p", "page_break"), at("p", { x_pt: 320 }), A4)!.w, 40);
  // A column break: a short rule, kept on the page.
  assert.equal(markerOf(info("c", "column_break"), at("c", { x_pt: 310 }), A4)!.w, COLUMN_RULE);
  assert.equal(markerOf(info("c", "column_break"), at("c", { x_pt: 580 }), A4)!.w, 15);
  // A space: a bracket left of the text, as high as the space.
  const space = markerOf(info("s", "space_before", { length: "1em" }), at("s", { height_pt: 22, detached: true }), A4)!;
  assert.deepEqual([space.x, space.y, space.w, space.h, space.chip, space.detached], [70 - BRACKET_GAP - BRACKET_W, 100, BRACKET_W, 22, "1em", true]);
  // A figure: its box (or just its position).
  const fig = markerOf(info("f", "figure_placement", { placement: "bottom" }), at("f", { width_pt: 85, height_pt: 73 }), A4)!;
  assert.deepEqual([fig.x, fig.y, fig.w, fig.h, fig.chip], [70, 100, 85, 73, "Bottom"]);
  assert.deepEqual([markerOf(info("f", "figure_placement"), at("f"), A4)!.w, markerOf(info("f", "figure_placement"), at("f"), A4)!.h], [0, 0]);
  const table = markerOf(info("t", "table_style", { style: "grid" }), at("t", { width_pt: 120, height_pt: 40 }), A4)!;
  assert.deepEqual([table.w, table.h, table.chip], [120, 40, "Table: Grid"]);
  // An image with its own quality: the image's box.
  const img = markerOf(info("i", "image_quality", { quality: "print" }), at("i", { width_pt: 120, height_pt: 80 }), A4)!;
  assert.deepEqual([img.kind, img.x, img.y, img.w, img.h, img.chip], ["image_quality", 70, 100, 120, 80, "Image: Print"]);
  // Not applied, not located, invalid, or no such page: no marker.
  assert.equal(markerOf(info("p", "page_break"), at("p", { applied: false }), A4), null);
  assert.equal(markerOf(info("p", "page_break"), at("p", { page: undefined }), A4), null);
  assert.equal(markerOf(info("p", "page_break"), at("p", { y_pt: undefined }), A4), null);
  assert.equal(markerOf(info("p", null), at("p"), A4), null);
  assert.equal(markerOf(info("p", "page_break"), at("p"), undefined), null);
});

test("markers by page follow the current overrides", () => {
  const overrides = [info("a", "page_break"), info("b", "space_before", { length: "6pt" }), info("c", "column_break")];
  const positions = [at("a", { page: 0 }), at("b", { page: 2, height_pt: 6 }), at("gone", { page: 2 }), at("c", { page: 7 })];
  const map = markersByPage(positions, overrides, [A4, A4, A4]);
  assert.deepEqual([...map.keys()], [0, 2]);
  assert.deepEqual(
    map.get(2)!.map((m) => m.id),
    ["b"],
  );
});

test("boxes scale with the zoom", () => {
  assert.deepEqual(scaleBox({ x: 10, y: 20, w: 30, h: 0 }, 2), { left: 20, top: 40, width: 60, height: 0 });
  assert.deepEqual(scaleBox({ x: 72, y: 72, w: 72, h: 72 }, 96 / 72), { left: 96, top: 96, width: 96, height: 96 });
});

test("positions are used only with the pages they were measured on", () => {
  const status = (over: Partial<CompileStatus>): CompileStatus => ({
    state: "done",
    version: 5,
    pages_version: 5,
    pages: 3,
    page_sizes: [],
    diagnostics: [],
    duration_ms: 1,
    overrides: [at("a"), at("b", { applied: false, page: undefined })],
    ...over,
  });
  const p = new MarkerPositions();
  assert.deepEqual(p.for(5), []);
  assert.equal(p.onStatus(status({ state: "compiling", overrides: [] })), false);
  assert.equal(p.onStatus(status({})), true);
  assert.deepEqual(
    p.for(5).map((s) => s.id),
    ["a"],
  );
  assert.equal(p.find("a", 5)?.y_pt, 100);
  // Other pages shown (older or newer): nothing.
  assert.deepEqual(p.for(4), []);
  assert.deepEqual(p.for(null), []);
  // A failed compile keeps the pages and positions of version 5.
  assert.equal(p.onStatus(status({ state: "failed", version: 6, pages_version: 5, overrides: [at("a", { page: undefined })] })), false);
  assert.equal(p.find("a", 5)?.page, 1);
  // A compile without pages for its version (worker restarted) changes nothing either.
  assert.equal(p.onStatus(status({ version: 7, pages_version: 5 })), false);
  assert.equal(p.onStatus(status({ version: 8, pages_version: 8, overrides: [at("a", { y_pt: 300 })] })), true);
  assert.equal(p.find("a", 8)?.y_pt, 300);
  assert.equal(p.find("a", 5), undefined);
  p.clear();
  assert.deepEqual(p.for(8), []);
});
