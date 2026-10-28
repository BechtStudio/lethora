// Tests for the layout panel's fields, value mapping and override markers. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { ALL_FIELDS, changeFor, type Field, fieldError, fromControl, GROUPS, isOverridden, type LayoutState, overrideCount, toControl, valueOf } from "./layoutpanel.ts";

/** The 45 layout keys of crates/typeset/src/layout.rs. */
const LAYOUT_KEYS = [
  "page.size",
  "page.width",
  "page.height",
  "page.margin_top",
  "page.margin_bottom",
  "page.margin_inside",
  "page.margin_outside",
  "page.two_sided",
  "page.columns",
  "page.column_gap",
  "text.font",
  "text.size",
  "text.leading",
  "text.justify",
  "text.hyphenate",
  "text.lang",
  "text.first_line_indent",
  "text.paragraph_spacing",
  "headings.numbering",
  "headings.chapter_pagebreak",
  "headings.level1_size",
  "headings.level2_size",
  "headings.level3_size",
  "headings.space_above",
  "headings.space_below",
  "figures.caption_position",
  "figures.numbering",
  "figures.default_width",
  "tables.style",
  "tables.header_bold",
  "tables.caption_position",
  "equations.numbering",
  "citations.style",
  "citations.bibliography_title",
  "furniture.page_numbers",
  "furniture.running_head",
  "front.title_page",
  "front.toc",
  "front.toc_depth",
  "front.toc_title",
  "front.toc_dots",
  "front.toc_indent",
  "front.toc_spacing",
  "front.toc_bold_top",
  "export.image_quality",
];

const state: LayoutState = {
  preset: "novel",
  layout: {
    schema: 1,
    page: { size: "us-trade", width: "6in", height: "9in", columns: 1, two_sided: true, margin_top: "20mm" },
    text: { size: "11.5pt", justify: true },
    figures: { default_width: 100 },
  },
  overrides: { "text.size": "11.5pt", "page.margin_top": "20mm" },
  meta: { title: "The Keeper", subtitle: "", author: "", date: "", contact: "" },
  custom: "",
};

const field = (key: string): Field => ALL_FIELDS.find((f) => f.key === key)!;

test("fields: every layout key and every metadata field, once", () => {
  const keys = ALL_FIELDS.map((f) => f.key);
  assert.equal(new Set(keys).size, keys.length);
  assert.deepEqual(keys.filter((k) => !k.startsWith("meta.")).sort(), [...LAYOUT_KEYS].sort());
  assert.deepEqual(
    keys.filter((k) => k.startsWith("meta.")),
    ["meta.title", "meta.subtitle", "meta.author", "meta.date", "meta.contact"],
  );
  assert.deepEqual(
    GROUPS.map((g) => g.title),
    ["Document", "Page", "Typography", "Headings", "Figures & equations", "Tables", "Citations", "Page furniture", "Front matter", "PDF export"],
  );
});

test("values: nested layout sections and metadata; custom size and gap rows only when they apply", () => {
  assert.equal(valueOf(state, "page.size"), "us-trade");
  assert.equal(valueOf(state, "text.size"), "11.5pt");
  assert.equal(valueOf(state, "meta.title"), "The Keeper");
  assert.equal(valueOf(state, "headings.numbering"), undefined);
  assert.equal(field("page.width").when!(state), false);
  const custom = { ...state, layout: { ...state.layout, page: { ...(state.layout.page as object), size: "custom", columns: 2 } } };
  assert.equal(field("page.width").when!(custom), true);
  assert.equal(field("page.column_gap").when!(custom), true);
  // Contents settings only with a table of contents.
  const contents = ["front.toc_depth", "front.toc_title", "front.toc_dots", "front.toc_indent", "front.toc_spacing", "front.toc_bold_top"];
  const toc = { ...state, layout: { ...state.layout, front: { toc: true } } };
  for (const key of contents) {
    assert.equal(field(key).when!(state), false, key);
    assert.equal(field(key).when!(toc), true, key);
  }
});

test("override markers: per key and counted per group; metadata never", () => {
  assert.ok(isOverridden(state, "text.size"));
  assert.ok(!isOverridden(state, "text.font"));
  assert.ok(!isOverridden(state, "meta.title"));
  const count = (id: string) => overrideCount(state, GROUPS.find((g) => g.id === id)!);
  assert.equal(count("typography"), 1);
  assert.equal(count("page"), 1);
  assert.equal(count("document"), 0);
});

test("controls: values to controls and back", () => {
  // Selects keep the value's type (columns are numbers).
  assert.equal(toControl(field("page.columns"), 2), "2");
  assert.deepEqual(fromControl(field("page.columns"), "2"), { value: 2 });
  assert.deepEqual(fromControl(field("headings.numbering"), "1.1."), { value: "1.1." });
  assert.ok("error" in fromControl(field("page.size"), "b5"));
  // Toggles.
  assert.equal(toControl(field("text.justify"), true), true);
  assert.equal(toControl(field("text.justify"), undefined), false);
  assert.deepEqual(fromControl(field("text.justify"), false), { value: false });
  // Numbers: parsed here, range checked by the core.
  assert.deepEqual(fromControl(field("figures.default_width"), "75"), { value: 75 });
  assert.deepEqual(fromControl(field("figures.default_width"), "150"), { value: 150 });
  assert.ok("error" in fromControl(field("figures.default_width"), ""));
  assert.ok("error" in fromControl(field("figures.default_width"), "abc"));
  // Lengths and fonts are trimmed and validated by the core.
  assert.equal(toControl(field("page.margin_top"), "20mm"), "20mm");
  assert.deepEqual(fromControl(field("page.margin_top"), " 25mm "), { value: "25mm" });
  assert.deepEqual(fromControl(field("page.margin_top"), "lots"), { value: "lots" });
  // Optional lengths: empty is the default.
  assert.deepEqual(fromControl(field("headings.space_above"), " "), { value: "" });
  assert.equal(toControl(field("front.toc_spacing"), ""), "");
  // The export image quality is a choice of four.
  assert.deepEqual(fromControl(field("export.image_quality"), "screen"), { value: "screen" });
  assert.ok("error" in fromControl(field("export.image_quality"), "72dpi"));
  const quality = field("export.image_quality");
  assert.ok(quality.kind === "select");
  assert.deepEqual(
    quality.options.map(([, label]) => label),
    ["Original (no change)", "Print (300 dpi)", "Screen (150 dpi)", "Email (96 dpi)"],
  );
  // The contents depth is a number.
  assert.deepEqual(fromControl(field("front.toc_depth"), "4"), { value: 4 });
  assert.deepEqual(fromControl(field("text.font"), "Libertinus Serif "), { value: "Libertinus Serif" });
  // Text: layout text kept as typed; empty metadata is cleared.
  assert.deepEqual(fromControl(field("citations.bibliography_title"), ""), { value: "" });
  assert.deepEqual(fromControl(field("meta.title"), "  "), { value: null });
  assert.deepEqual(fromControl(field("meta.contact"), "a\nb"), { value: "a\nb" });
  assert.equal(toControl(field("meta.title"), undefined), "");
});

test("changes: layout keys to layout_set, metadata to meta_set without the prefix", () => {
  assert.deepEqual(changeFor(field("text.size"), "12pt"), { command: "layout_set", changes: { "text.size": "12pt" } });
  assert.deepEqual(changeFor(field("text.size"), null), { command: "layout_set", changes: { "text.size": null } });
  assert.deepEqual(changeFor(field("meta.author"), "Ann"), { command: "meta_set", changes: { author: "Ann" } });
});

test("errors: the core's message for one field, without prefix and key", () => {
  const message = 'Invalid settings (nothing was changed): page.margin_top: "lots" is not a length (like 12pt, 2.5cm, 1in or 0.6em).';
  assert.equal(fieldError(message, "page.margin_top"), '"lots" is not a length (like 12pt, 2.5cm, 1in or 0.6em).');
  assert.equal(fieldError("Invalid settings (nothing was changed): title: contains line breaks or control characters.", "meta.title"), "Contains line breaks or control characters.");
  assert.equal(fieldError("No project is open.", "text.size"), "No project is open.");
});
