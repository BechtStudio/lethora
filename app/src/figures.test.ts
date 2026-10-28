// Tests for figure scanning, inserting, asset paths, sizing and placement badges. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { ensureSyntaxTree } from "@codemirror/language";
import { EditorSelection, EditorState } from "@codemirror/state";
import { typstLanguage } from "./typst.ts";
import {
  AssetCache,
  closeBracket,
  type FigureLayout,
  figureLabels,
  figureText,
  findFigures,
  imageBox,
  imageDpi,
  insertFigure,
  isAssetPath,
  labelStem,
  matchExceptions,
  mimeFor,
  namedArgs,
  placementBadge,
  placementExceptions,
  scanFigure,
  uniqueLabel,
} from "./figures.ts";

function stateOf(doc: string, anchor = 0, head = anchor): EditorState {
  const state = EditorState.create({ doc, selection: EditorSelection.single(anchor, head), extensions: typstLanguage });
  ensureSyntaxTree(state, state.doc.length, 5000);
  return state;
}

const FIG = `#figure(
  image("assets/lamp.png", width: 80%),
  caption: [The lamp (at dusk) #emph[really]],
) <fig:lamp>`;

test("balanced brackets: strings, nested calls, content blocks, comments, raw, math", () => {
  const end = (s: string) => closeBracket(s, s.indexOf("("));
  assert.equal(end("f(a, (b), [c)] ) tail"), 16);
  assert.equal(end('f(")", "\\")") x'), 13);
  assert.equal(end("f([a ( b], c) x"), 13);
  assert.equal(end("f([`)`], $)$) x"), 13);
  assert.equal(end("f(a // )\n) x"), 10);
  assert.equal(end("f(a /* ) */) x"), 12);
  assert.equal(end("f([#g(a, \")\")]) x"), 15);
  assert.equal(end("f([a \\] b]) x"), 11);
  assert.equal(end("f(a"), -1);
  assert.equal(end("f(a]"), -1);
  assert.equal(end('f("unclosed)'), -1);
});

test("scanFigure: path, caption, label", () => {
  const f = scanFigure(FIG + " more", 0)!;
  assert.equal(f.from, 0);
  assert.equal(f.to, FIG.length);
  assert.equal(FIG.slice(f.callEnd - 1, f.callEnd), ")");
  assert.equal(f.label, "fig:lamp");
  assert.equal(f.path, "assets/lamp.png");
  assert.equal(f.caption, "The lamp (at dusk) #emph[really]");
  assert.equal(FIG.slice(f.captionFrom!, f.captionFrom! + 8), "The lamp");

  const bare = scanFigure('#figure(image("assets/a.svg"))\n', 0)!;
  assert.equal(bare.label, null);
  assert.equal(bare.caption, null);
  assert.equal(bare.path, "assets/a.svg");
  // Not previewed: computed paths, other folders, escapes, tables.
  assert.equal(scanFigure('#figure(image("../x.png"))', 0)!.path, null);
  assert.equal(scanFigure('#figure(image("assets/../x.png"))', 0)!.path, null);
  assert.equal(scanFigure('#figure(image(path))', 0)!.path, null);
  assert.equal(scanFigure('#figure(image("assets/x.PNG"))', 0)!.path, null);
  assert.equal(scanFigure("#figure(table(columns: 2)[a][b], caption: [T])", 0)!.path, null);
  assert.equal(scanFigure("#figure(image(", 0), null);
});

test("findFigures: in range, skips raw and comments", () => {
  const doc = `Intro.\n\n${FIG}\n\n\`#figure(image("assets/no.png"))\` // #figure(image("assets/no.png"))\n\n#figure(image("assets/b.png")) <fig:b>`;
  const state = stateOf(doc);
  const all = findFigures(state, 0, doc.length);
  assert.deepEqual(all.map((f) => f.label), ["fig:lamp", "fig:b"]);
  const second = all[1];
  assert.deepEqual(findFigures(state, second.from + 3, second.from + 4).map((f) => f.label), ["fig:b"]);
  assert.deepEqual(findFigures(state, 0, 3), []);
  // A range ending inside the first figure still finds it.
  assert.deepEqual(findFigures(state, 0, doc.indexOf("caption")).map((f) => f.label), ["fig:lamp"]);
});

test("asset paths and MIME types", () => {
  assert.equal(mimeFor("assets/a.png"), "image/png");
  assert.equal(mimeFor("assets/a.jpg"), "image/jpeg");
  assert.equal(mimeFor("assets/a.jpeg"), "image/jpeg");
  assert.equal(mimeFor("assets/a.gif"), "image/gif");
  assert.equal(mimeFor("assets/a.svg"), "image/svg+xml");
  assert.equal(mimeFor("assets/a.webp"), "image/webp");
  assert.equal(mimeFor("assets/a.pdf"), null);
  assert.equal(mimeFor("assets/a.PNG"), null);
  assert.equal(mimeFor("assets/png"), null);
  assert.ok(isAssetPath("assets/sub/a.png"));
  for (const bad of ["a.png", "assets/", "assets//a.png", "assets/./a.png", "assets/../a.png", "/assets/a.png", "assets\\a.png", "C:assets/a.png", "assets/a.exe"]) {
    assert.ok(!isAssetPath(bad), bad);
  }
});

test("label stems and unique labels", () => {
  assert.equal(labelStem("assets/Harbour Lights-2.svg"), "harbour-lights-2");
  assert.equal(labelStem("assets/Café.png"), "cafe");
  assert.equal(labelStem("assets/___.png"), "figure");
  const taken = new Set(["fig:lamp", "fig:lamp-2"]);
  assert.equal(uniqueLabel("lamp", (n) => taken.has(n)), "fig:lamp-3");
  assert.equal(uniqueLabel("map", (n) => taken.has(n)), "fig:map");
});

test("insertFigure: own lines, caption selected", () => {
  const apply = (doc: string, at: number) => {
    const state = stateOf(doc, at);
    const tr = state.update(insertFigure(state, "assets/lamp.png", "fig:lamp"));
    const sel = tr.state.selection.main;
    return { text: tr.state.doc.toString(), selected: tr.state.sliceDoc(sel.from, sel.to) };
  };
  const body = '#figure(\n  image("assets/lamp.png", width: 80%),\n  caption: [Caption],\n) <fig:lamp>';
  assert.deepEqual(apply("", 0), { text: body + "\n", selected: "Caption" });
  assert.deepEqual(apply("Before.\n\nAfter.", 8), { text: `Before.\n\n${body}\n\nAfter.`, selected: "Caption" });
  assert.deepEqual(apply("A\n\n\nB", 3), { text: `A\n\n${body}\n\nB`, selected: "Caption" });
  assert.deepEqual(apply("Mid text", 3), { text: `Mid\n\n${body}\n\n text`, selected: "Caption" });
});

test("figureText: several figures, each its own block; labels never taken twice", () => {
  const doc = (s: string) => EditorState.create({ doc: s }).doc;
  const figure = (path: string, label: string) => `#figure(\n  image("${path}", width: 80%),\n  caption: [Caption],\n) <${label}>`;
  const two = [
    { path: "assets/a.png", label: "fig:a" },
    { path: "assets/f/b.svg", label: "fig:b" },
  ];
  const between = figureText(doc("A\n\nB"), 2, 2, two);
  assert.equal(between.insert, `\n${figure("assets/a.png", "fig:a")}\n\n${figure("assets/f/b.svg", "fig:b")}\n`);
  assert.equal(between.insert.slice(between.caption, between.caption + 7), "Caption");
  assert.equal(`A\n${between.insert}\nB`, `A\n\n${figure("assets/a.png", "fig:a")}\n\n${figure("assets/f/b.svg", "fig:b")}\n\nB`);
  // At the ends of the text.
  assert.equal(figureText(doc("A"), 0, 0, two.slice(0, 1)).insert, `${figure("assets/a.png", "fig:a")}\n\n`);
  assert.equal(figureText(doc("A"), 1, 1, two.slice(0, 1)).insert, `\n\n${figure("assets/a.png", "fig:a")}\n`);
  const taken = new Set(["fig:lamp"]);
  assert.deepEqual(
    figureLabels(["assets/Lamp.png", "assets/x/lamp.jpg", "assets/Über Bild.png"], (n) => taken.has(n)).map((f) => f.label),
    ["fig:lamp-2", "fig:lamp-3", "fig:uber-bild"],
  );
});

test("scanFigure: image size, fit and placement arguments", () => {
  const f = scanFigure('#figure(\n  image("assets/a.png", width: 80%, height: 4.5cm, fit: "contain"),\n  placement: top,\n  caption: [Width: 10%, placement: bottom],\n)', 0)!;
  assert.deepEqual(f.image, { width: { value: 80, unit: "%" }, height: { value: 4.5, unit: "cm" }, fit: "contain" });
  assert.equal(f.placement, "top");
  // Absent, auto, not literal, unsupported units and fits.
  const g = scanFigure('#figure(image("assets/a.png", width: auto, height: 50% - 1cm, fit: "fill"), placement: none)', 0)!;
  assert.deepEqual(g.image, { width: "auto", height: null, fit: null });
  assert.equal(g.placement, "none");
  const widths = [
    ["2in", { value: 2, unit: "in" }],
    ["12.5mm", { value: 12.5, unit: "mm" }],
    ["100pt", { value: 100, unit: "pt" }],
    [".5em", { value: 0.5, unit: "em" }],
    ["1fr", null],
    ["w", null],
    ["calc.min(1cm, 2cm)", null],
  ] as const;
  for (const [arg, width] of widths) {
    assert.deepEqual(scanFigure(`#figure(image("assets/a.png", width: ${arg}))`, 0)!.image.width, width, arg);
  }
  // Only the figure's own literal placement counts.
  const h = scanFigure('#figure(image("assets/a.png"), caption: [x], kind: "placement: top")', 0)!;
  assert.deepEqual(h.image, { width: null, height: null, fit: null });
  assert.equal(h.placement, null);
  assert.equal(scanFigure("#figure(placement: here, [x])", 0)!.placement, null);
});

test("namedArgs: top-level only, through strings, brackets and comments", () => {
  const src = 'f(a: 1, b: g(c: 2, d), e: "x, y: z", /* h: 1, */ [i: j], k: (1, 2), l: 3 / 2)';
  const args = namedArgs(src, 1, src.length);
  assert.deepEqual([...args.keys()], ["a", "b", "e", "k", "l"]);
  assert.equal(args.get("b"), "g(c: 2, d)");
  assert.equal(args.get("e"), '"x, y: z"');
  assert.equal(args.get("l"), "3 / 2");
});

const LAYOUT: FigureLayout = { column_width_pt: 400, height_pt: 600, font_size_pt: 10, default_width: 80 };

test("imageBox: units as fractions of the column, capped", () => {
  const width = (value: number, unit: "%" | "pt" | "mm" | "cm" | "in" | "em") => imageBox({ width: { value, unit }, height: null, fit: null }, LAYOUT, null)!.width;
  const close = (a: number, b: number) => assert.ok(Math.abs(a - b) < 1e-9, `${a} vs ${b}`);
  close(width(50, "%"), 0.5);
  close(width(200, "pt"), 0.5);
  close(width(2, "in"), 144 / 400);
  close(width(5, "cm"), (5 * 72) / 2.54 / 400);
  close(width(50, "mm"), (50 * 72) / 25.4 / 400);
  close(width(10, "em"), 100 / 400);
  close(width(150, "%"), 1);
  close(width(1000, "pt"), 1);
  // No width: the layout's default width (Typst's `#set image(width: ..)`), keeping the aspect ratio.
  assert.deepEqual(imageBox({ width: null, height: null, fit: null }, LAYOUT, null), { width: 0.8, ratio: null, fit: "cover" });
  assert.equal(imageBox({ width: { value: 0, unit: "pt" }, height: null, fit: null }, LAYOUT, null), null);
});

test("imageBox: heights, fit and natural sizes", () => {
  // Width and height: a box (cover unless a fit is given); % height of the text height.
  assert.deepEqual(imageBox({ width: { value: 50, unit: "%" }, height: { value: 100, unit: "pt" }, fit: null }, LAYOUT, null), { width: 0.5, ratio: 2, fit: "cover" });
  assert.deepEqual(imageBox({ width: { value: 200, unit: "pt" }, height: { value: 50, unit: "%" }, fit: "stretch" }, LAYOUT, null), { width: 0.5, ratio: 200 / 300, fit: "stretch" });
  // A height alone still gets the default width.
  assert.deepEqual(imageBox({ width: null, height: { value: 10, unit: "em" }, fit: "contain" }, LAYOUT, null), { width: 0.8, ratio: 320 / 100, fit: "contain" });
  // `width: auto`: natural size (capped), or scaled from the height.
  const natural = { width: 300, height: 150 };
  assert.equal(imageBox({ width: "auto", height: null, fit: null }, LAYOUT, null), null);
  assert.deepEqual(imageBox({ width: "auto", height: null, fit: null }, LAYOUT, natural), { width: 0.75, ratio: null, fit: "cover" });
  assert.deepEqual(imageBox({ width: "auto", height: "auto", fit: null }, LAYOUT, { width: 800, height: 400 }), { width: 1, ratio: null, fit: "cover" });
  assert.deepEqual(imageBox({ width: "auto", height: { value: 100, unit: "pt" }, fit: null }, LAYOUT, natural), { width: 0.5, ratio: null, fit: "cover" });
  // Wider than the column at that height: a column-wide box.
  assert.deepEqual(imageBox({ width: "auto", height: { value: 400, unit: "pt" }, fit: null }, LAYOUT, natural), { width: 1, ratio: 1, fit: "cover" });
});

test("imageDpi: EXIF, JFIF and PNG pHYs like Typst", () => {
  const bytes = (...parts: number[][]) => new Uint8Array(parts.flat());
  const u32 = (n: number) => [(n >>> 24) & 255, (n >>> 16) & 255, (n >>> 8) & 255, n & 255];
  const u16 = (n: number) => [(n >>> 8) & 255, n & 255];
  const ascii = (s: string) => [...s].map((c) => c.charCodeAt(0));
  const chunk = (type: string, data: number[]) => [...u32(data.length), ...ascii(type), ...data, ...u32(0)];
  const png = (...chunks: number[][]) => bytes([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a], ...chunks, chunk("IDAT", []));
  const ihdr = chunk("IHDR", [...u32(10), ...u32(10), 8, 2, 0, 0, 0]);
  // 11811 px/m: 300 dpi.
  assert.equal(Math.round(imageDpi(png(ihdr, chunk("pHYs", [...u32(11811), ...u32(11811), 1])))!), 300);
  assert.equal(imageDpi(png(ihdr, chunk("pHYs", [...u32(11811), ...u32(11811), 0]))), null);
  assert.equal(imageDpi(png(ihdr)), null);
  // JFIF: dots per inch, per cm, or no unit.
  const jfif = (unit: number, x: number) => bytes([0xff, 0xd8, 0xff, 0xe0], u16(16), ascii("JFIF"), [0, 1, 2, unit], u16(x), u16(x), [0, 0], [0xff, 0xda]);
  assert.equal(imageDpi(jfif(1, 150)), 150);
  assert.equal(imageDpi(jfif(2, 100)), 254);
  assert.equal(imageDpi(jfif(0, 1)), null);
  // EXIF (big-endian TIFF, XResolution 350/1) wins over JFIF.
  const tiff = [...ascii("MM"), ...u16(42), ...u32(8), ...u16(1), ...u16(0x011a), ...u16(5), ...u32(1), ...u32(26), ...u32(0), ...u32(350), ...u32(1)];
  const exif = [0xff, 0xe1, ...u16(2 + 6 + tiff.length), ...ascii("Exif"), 0, 0, ...tiff];
  assert.equal(imageDpi(bytes([0xff, 0xd8, 0xff, 0xe0], u16(16), ascii("JFIF"), [0, 1, 2, 1], u16(72), u16(72), [0, 0], exif, [0xff, 0xda])), 350);
  assert.equal(imageDpi(bytes([1, 2, 3])), null);
  assert.equal(imageDpi(bytes([0xff, 0xd8, 0xff])), null);
});

test("placement badges: own argument, exceptions matched like the core", () => {
  const doc = 'Text\n\n#figure(image("assets/a.png"), placement: bottom) <a>\n\n#figure(image("assets/b.png"))\n#figure(image("assets/c.png"), placement: none)\n';
  const state = stateOf(doc);
  const spans = findFigures(state, 0, doc.length);
  assert.equal(spans.length, 3);
  const [a, b, c] = spans;
  assert.deepEqual(placementBadge(a, undefined), { text: "Bottom", kind: "own" });
  assert.equal(placementBadge(b, undefined), null);
  assert.deepEqual(placementBadge(c, undefined), { text: "Here", kind: "own" });

  const exceptions = placementExceptions([
    // Anchored at the call (after the `#`), as the core does.
    { kind: "figure_placement", params: { placement: "top" }, pos_utf16: b.from + 1, detached: false },
    // On the label: not in the call, so the first figure starting on the line.
    { kind: "figure_placement", params: { placement: "here" }, pos_utf16: doc.indexOf("<a>"), detached: true },
    { kind: "page_break", params: {}, pos_utf16: c.from, detached: false },
    { kind: "figure_placement", params: { placement: "auto" }, pos_utf16: null, detached: false },
    // Neither in a figure nor on a figure's line.
    { kind: "figure_placement", params: { placement: "auto" }, pos_utf16: 0, detached: false },
  ]);
  assert.equal(exceptions.length, 3);
  const matched = matchExceptions(state.doc, spans, exceptions);
  assert.deepEqual(placementBadge(a, matched.get(a)), { text: "Here · exception", kind: "detached" });
  assert.deepEqual(placementBadge(b, matched.get(b)), { text: "Top · exception", kind: "exception" });
  assert.equal(matched.get(c), undefined);
  // Anywhere inside the call counts too.
  const inside = matchExceptions(state.doc, spans, [{ pos: doc.indexOf("assets/c.png"), placement: "bottom", detached: false }]);
  assert.deepEqual(placementBadge(c, inside.get(c)), { text: "Bottom · exception", kind: "exception" });
});

test("asset cache: a replaced image is forgotten and loads again", async () => {
  const loads: string[] = [];
  let results = 0;
  const cache = new AssetCache(
    async (path) => {
      loads.push(path);
      return new Uint8Array([loads.length]);
    },
    () => results++,
  );
  const settled = async (n: number) => {
    while (results < n) await new Promise((r) => setTimeout(r, 1));
  };
  cache.want(["assets/a.png", "assets/b.png"]);
  await settled(2);
  const before = cache.get("assets/a.png");
  assert.ok(before && "url" in before);
  cache.forget("assets/a.png");
  assert.equal(cache.get("assets/a.png"), undefined);
  assert.ok(cache.get("assets/b.png"));
  cache.want(["assets/a.png", "assets/b.png"]);
  await settled(3);
  assert.deepEqual(loads, ["assets/a.png", "assets/b.png", "assets/a.png"]);
  const after = cache.get("assets/a.png");
  assert.ok(after && "url" in after && after.url !== before.url);
  cache.clear();
});
