// Tests for the page menu's items (from page_jump results and overrides). Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import type { JumpResult, OverrideInfo } from "./exceptions.ts";
import { blockPreview, imagePathAt, type MenuContext, type MenuItem, menuModel } from "./pagemenu.ts";

const TEXT = [
  "= The Keeper", // 0
  "", // 13
  "Mara had counted the steps so often that she no longer needed to.", // 14
  "", // 81
  "#figure(", // 82
  '  image("assets/lighthouse.png"),', // 91
  "  caption: [The lamp room.],", // 125
  ") <fig:lamp>", // 154
].join("\n");

const ctx = (twoColumns = false): MenuContext => ({
  twoColumns,
  imageQuality: "screen",
  tableStyle: "academic",
  lineAt: (u) => {
    const from = TEXT.lastIndexOf("\n", u - 1) + 1;
    const end = TEXT.indexOf("\n", u);
    return { from, text: TEXT.slice(from, end < 0 ? TEXT.length : end) };
  },
});

const PARA = TEXT.indexOf("Mara");
const FIG = TEXT.indexOf("figure(");
const IMG = TEXT.indexOf("image(");
const content = (pos: number, block: number, figure: number | null = null, image: number | null = null, table: number | null = null): JumpResult => ({
  target: { in: "content", pos_utf16: pos, start_utf16: pos, end_utf16: pos + 4, block_start_utf16: block },
  figure_utf16: figure,
  image_utf16: image,
  table_utf16: table,
});

const ov = (over: Partial<OverrideInfo>): OverrideInfo => ({
  id: "x",
  kind: "page_break",
  params: { to_odd: false },
  anchor_preview: "",
  pos_utf16: PARA,
  detached: false,
  applicable: true,
  error: null,
  ...over,
});

/** Items as short strings: "[x] Label (Remove)", "space 12pt", "sub: …", "---". */
function show(items: MenuItem[]): string[] {
  return items.map((i) => {
    switch (i.type) {
      case "separator":
        return "---";
      case "space":
        return `space ${i.value}${i.id ? ` #${i.id}` : ""} @${i.pos}`;
      case "submenu":
        return `${i.label} ▸ ${show(i.items).join(" | ")}`;
      case "item": {
        const mark = i.checked === undefined ? "" : i.checked ? "[x] " : "[ ] ";
        return `${mark}${i.label}${i.hint ? ` (${i.hint})` : ""}${i.act === null ? " (disabled)" : ""}`;
      }
    }
  });
}

test("menu: a paragraph offers breaks and space before; actions anchor at the block start", () => {
  const m = menuModel(content(PARA + 10, PARA), [], ctx());
  assert.equal(m.header, "Paragraph: “Mara had counted the steps so often that…”");
  assert.deepEqual(show(m.items), ["[ ] Page break before", "[ ] Start on a right-hand page", `space 12pt @${PARA}`]);
  const first = m.items[0];
  assert.ok(first.type === "item");
  assert.deepEqual(first.act, { op: "add", kind: "page_break", params: { to_odd: false }, pos: PARA });
});

test("menu: column break only with two columns (or when one exists)", () => {
  assert.deepEqual(show(menuModel(content(2, 0), [], ctx(true)).items), ["[ ] Page break before", "[ ] Start on a right-hand page", "[ ] Column break before", "space 12pt @0"]);
  const existing = ov({ id: "c1", kind: "column_break", params: {}, pos_utf16: 0 });
  assert.ok(show(menuModel(content(2, 0), [existing], ctx(false)).items).includes("[x] Column break before (Remove)"));
});

test("menu: existing overrides on the block are checked and removable", () => {
  const right = ov({ id: "p1", params: { to_odd: true } });
  const space = ov({ id: "s1", kind: "space_before", params: { length: "18pt" } });
  const elsewhere = ov({ id: "z", pos_utf16: 0 });
  const m = menuModel(content(PARA, PARA), [right, space, elsewhere], ctx());
  assert.deepEqual(show(m.items), ["[ ] Page break before", "[x] Start on a right-hand page (Remove)", `space 18pt #s1 @${PARA}`]);
  const [plain, odd] = m.items;
  assert.ok(plain.type === "item" && odd.type === "item");
  // A plain page break replaces the right-hand one (same kind, same block).
  assert.deepEqual(plain.act, { op: "add", kind: "page_break", params: { to_odd: false }, pos: PARA });
  assert.deepEqual(odd.act, { op: "remove", id: "p1" });
  // Invalid overrides are not matched.
  assert.deepEqual(show(menuModel(content(PARA, PARA), [ov({ error: "bad" })], ctx()).items).slice(0, 1), ["[ ] Page break before"]);
});

test("menu: in a figure, block items anchor at the figure's line and placement is a submenu", () => {
  const caption = TEXT.indexOf("The lamp");
  const m = menuModel(content(caption, TEXT.indexOf("  caption"), FIG), [ov({ id: "f1", kind: "figure_placement", params: { placement: "top" }, pos_utf16: FIG })], ctx());
  assert.equal(m.header, "Figure");
  const line = TEXT.indexOf("#figure");
  assert.deepEqual(show(m.items), [
    "[ ] Page break before",
    "[ ] Start on a right-hand page",
    `space 12pt @${line}`,
    "---",
    "Figure placement ▸ [x] Top (Remove) | [ ] Bottom | [ ] Here | [ ] Auto",
  ]);
  const sub = m.items[4];
  assert.ok(sub.type === "submenu" && sub.items[2].type === "item");
  assert.deepEqual(sub.items[2].act, { op: "add", kind: "figure_placement", params: { placement: "here" }, pos: FIG });
});

test("menu: on an image, its quality is a submenu; the document's default is named and checked without an override", () => {
  const items = (overrides: OverrideInfo[], figure: number | null = FIG) => menuModel(content(IMG + 2, TEXT.indexOf("  image"), figure, IMG), overrides, ctx()).items;
  const plain = items([]);
  assert.deepEqual(show(plain).slice(3), [
    "---",
    "Figure placement ▸ [ ] Top | [ ] Bottom | [ ] Here | [ ] Auto",
    "Image quality ▸ [x] Document default (Screen) (disabled) | [ ] Original | [ ] Print | [ ] Screen | [ ] Email",
    "Replace image…",
  ]);
  const sub = plain[5];
  assert.ok(sub.type === "submenu" && sub.items[4].type === "item");
  assert.deepEqual(sub.items[4].act, { op: "add", kind: "image_quality", params: { quality: "email" }, pos: IMG });

  // An override of its own is checked; the default (or the checked one) removes it.
  const own = ov({ id: "q1", kind: "image_quality", params: { quality: "original" }, pos_utf16: IMG });
  const elsewhere = ov({ id: "q2", kind: "image_quality", params: { quality: "print" }, pos_utf16: 0 });
  const withOwn = items([own, elsewhere]);
  assert.equal(show(withOwn)[5], "Image quality ▸ [ ] Document default (Screen) | [x] Original (Remove) | [ ] Print | [ ] Screen | [ ] Email");
  const sub2 = withOwn[5];
  assert.ok(sub2.type === "submenu" && sub2.items[0].type === "item" && sub2.items[1].type === "item" && sub2.items[2].type === "item");
  assert.deepEqual(sub2.items[0].act, { op: "remove", id: "q1" });
  assert.deepEqual(sub2.items[1].act, { op: "remove", id: "q1" });
  // Another quality replaces it (same kind, same image: the core replaces).
  assert.deepEqual(sub2.items[2].act, { op: "add", kind: "image_quality", params: { quality: "print" }, pos: IMG });

  // An image outside a figure: its own separator, no placement.
  assert.deepEqual(show(items([], null)).slice(3), ["---", "Image quality ▸ [x] Document default (Screen) (disabled) | [ ] Original | [ ] Print | [ ] Screen | [ ] Email", "Replace image…"]);
});

test("menu: on a table, its style is a submenu; the default is named and checked without an override", () => {
  // A table outside a figure (TEXT has none: offsets are what matters).
  const table = PARA;
  const items = (overrides: OverrideInfo[], figure: number | null = null) => menuModel(content(PARA + 2, PARA, figure, null, table), overrides, ctx()).items;
  assert.deepEqual(show(items([])).slice(3), ["---", "Table style ▸ [x] Default (Academic) (disabled) | [ ] Plain | [ ] Academic | [ ] Grid | [ ] Striped"]);
  const sub = items([])[4];
  assert.ok(sub.type === "submenu" && sub.items[3].type === "item");
  assert.deepEqual(sub.items[3].act, { op: "add", kind: "table_style", params: { style: "grid" }, pos: table });
  // Its own style: checked; it and the default remove it.
  const own = ov({ id: "t1", kind: "table_style", params: { style: "striped" }, pos_utf16: table });
  const withOwn = items([own]);
  assert.equal(show(withOwn)[4], "Table style ▸ [ ] Default (Academic) | [ ] Plain | [ ] Academic | [ ] Grid | [x] Striped (Remove)");
  const sub2 = withOwn[4];
  assert.ok(sub2.type === "submenu" && sub2.items[0].type === "item" && sub2.items[4].type === "item");
  assert.deepEqual(sub2.items[0].act, { op: "remove", id: "t1" });
  assert.deepEqual(sub2.items[4].act, { op: "remove", id: "t1" });
  // In a figure: after the placement, one separator.
  assert.deepEqual(
    show(items([], FIG)).slice(3).map((s) => s.split(" ▸")[0]),
    ["---", "Figure placement", "Table style"],
  );
});

test("menu: Replace image names the image's literal asset path; none without one", () => {
  const replace = menuModel(content(IMG + 2, TEXT.indexOf("  image"), FIG, IMG), [], ctx()).items.at(-1)!;
  assert.ok(replace.type === "item");
  assert.deepEqual(replace.act, { op: "replace_image", path: "assets/lighthouse.png", pos: IMG });
  const line = (text: string): MenuContext => ({ ...ctx(), lineAt: () => ({ from: 10, text }) });
  assert.equal(imagePathAt(line('#image("assets/a b.jpg", width: 5cm)'), 10), "assets/a b.jpg");
  assert.equal(imagePathAt(line('x image( "assets/sub/c.svg")'), 12), "assets/sub/c.svg");
  for (const bad of ["image(path)", 'image("assets/../x.png")', 'image("other/x.png")', 'image("assets/x.PNG")', "image("]) {
    assert.equal(imagePathAt(line(bad), 10), null, bad);
  }
  const noPath = menuModel(content(IMG + 2, TEXT.indexOf("  image"), FIG, 0), [], ctx()).items;
  assert.ok(!show(noPath).includes("Replace image…"));
});

test("menu: an override marker offers Edit and Remove; nothing else is disabled", () => {
  const space = ov({ id: "s1", kind: "space_before", params: { length: "18pt" } });
  const m = menuModel({ target: { in: "override", id: "s1" }, figure_utf16: null, image_utf16: null }, [space], ctx());
  assert.equal(m.header, "Exception: Space before 18pt");
  assert.deepEqual(show(m.items), ["Edit…", "Remove"]);
  const column = ov({ id: "c1", kind: "column_break", params: {} });
  assert.deepEqual(show(menuModel({ target: { in: "override", id: "c1" }, figure_utf16: null, image_utf16: null }, [column], ctx()).items), ["Remove"]);
  const none = menuModel({ target: { in: "none" }, figure_utf16: null, image_utf16: null }, [], ctx());
  assert.equal(none.header, null);
  assert.deepEqual(show(none.items), ["Nothing to anchor here (disabled)"]);
});

test("block previews: headings, figures, equations, list items, paragraphs", () => {
  assert.equal(blockPreview("== The Arithmetic of Light <sec:light>"), "Heading: The Arithmetic of Light");
  assert.equal(blockPreview('#figure(image("a.png"), caption: [A sketch that went missing.]) <fig:sketch>'), "Figure: A sketch that went missing.");
  assert.equal(blockPreview("#figure("), "Figure");
  assert.equal(blockPreview("$ A = pi r^2 $ <eq:light>"), "Equation");
  assert.equal(blockPreview("- trim the wick before dusk"), "List item: “trim the wick before dusk”");
  assert.equal(blockPreview("Short."), "Paragraph: “Short.”");
  assert.equal(blockPreview("Test \\\nTest2"), "Paragraph: “Test Test2”");
  assert.equal(blockPreview("A first line \\\nthat goes on over a second line and then a third"), "Paragraph: “A first line that goes on over a second…”");
  assert.equal(blockPreview("An escape \\* stays"), "Paragraph: “An escape \\* stays”");
  assert.equal(blockPreview("== Title\nText"), "Heading: Title");
  assert.equal(blockPreview(""), "Empty line");
});
