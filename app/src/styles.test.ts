// Tests for character styles at the selection and their creation. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { ensureSyntaxTree } from "@codemirror/language";
import { EditorSelection, EditorState } from "@codemirror/state";
import type { LayoutState } from "./layoutpanel.ts";
import {
  type Creations,
  computedName,
  deleteChanges,
  newNamedStyle,
  newStyleId,
  parseSize,
  sameSettings,
  sizeLabel,
  slug,
  styleAt,
  styleChanges,
  styledCss,
  styleEntries,
  styleFor,
  styleLabel,
  styleUses,
  type TextStyles,
  textStylesOf,
  toPt,
  unusedStyles,
} from "./styles.ts";
import { typstLanguage } from "./typst.ts";

const info: TextStyles = {
  body: { font: "Libertinus Serif", size: "11pt" },
  styles: {
    "eb-garamond-14pt": { font: "EB Garamond", size: "14pt" },
    "14pt": { size: "14pt" },
    big: { name: "Big", size: "18pt" },
    mono: { font: "DejaVu Sans Mono" },
  },
  ids: new Set(["eb-garamond-14pt", "14pt", "big", "mono", "gone"]),
};

test("sizes: points, other units, the box's text", () => {
  assert.equal(toPt("14pt"), 14);
  assert.ok(Math.abs(toPt("25.4mm")! - 72) < 1e-9);
  assert.equal(toPt("1in"), 72);
  assert.equal(toPt("1.2em"), null);
  assert.equal(sizeLabel("11pt"), "11");
  assert.equal(sizeLabel("10.5pt"), "10.5");
  assert.equal(sizeLabel("5mm"), "14.2");
  assert.equal(parseSize(" 12 "), "12pt");
  assert.equal(parseSize("10,5"), "10.5pt");
  assert.equal(parseSize("14 PT"), "14pt");
  assert.equal(parseSize("5mm"), "5mm");
  assert.equal(parseSize("big"), null);
  assert.equal(parseSize(""), null);
});

test("ids are readable slugs, unique and at most 48 characters", () => {
  assert.equal(slug("EB Garamond"), "eb-garamond");
  assert.equal(slug("Noto Sans Café"), "noto-sans-cafe");
  assert.equal(newStyleId({ font: "EB Garamond", size: "14pt" }, () => false), "eb-garamond-14pt");
  assert.equal(newStyleId({ size: "10.5pt" }, () => false), "10-5pt");
  assert.equal(newStyleId({ font: "DejaVu Sans Mono" }, () => false), "dejavu-sans-mono");
  assert.equal(newStyleId({ font: "明朝" }, () => false), "style");
  const taken = new Set(["14pt", "14pt-2"]);
  assert.equal(newStyleId({ size: "14pt" }, (id) => taken.has(id)), "14pt-3");
  const long = newStyleId({ font: "A Very Long Font Family Name Indeed For Testing", size: "14pt" }, (id) => !id.endsWith("-2"));
  assert.ok(long.length <= 48 && long.endsWith("-2") && !long.includes("--"), long);
});

test("a target reuses an unnamed style, never a named one, else creates one", () => {
  const creations: Creations = new Map();
  assert.equal(styleFor({ font: "eb garamond", size: "14pt" }, info, creations), "eb-garamond-14pt");
  assert.equal(styleFor({ size: "14pt" }, info, creations), "14pt");
  // "Big" has 18pt but is named.
  assert.equal(styleFor({ size: "18pt" }, info, creations), "18pt");
  // The body's settings (or none): default text.
  assert.equal(styleFor({ font: "Libertinus Serif" }, info, creations), null);
  assert.equal(styleFor({ font: "Libertinus Serif", size: "11pt" }, info, creations), null);
  assert.equal(styleFor({}, info, creations), null);
  // Created once, reused within the same change.
  assert.equal(styleFor({ size: "18pt" }, info, creations), "18pt");
  assert.deepEqual([...creations], [["18pt", { size: "18pt" }]]);
  assert.deepEqual(styleChanges(creations), { "styles.18pt.font": null, "styles.18pt.size": "18pt", "styles.18pt.name": null, "styles.18pt.removed": null });
  // An id the layout knows otherwise is not given again.
  assert.equal(styleFor({ font: "Gone" }, info, new Map()), "gone-2");
});

test("same settings: the same fields, equal values", () => {
  assert.ok(sameSettings({ size: "72pt" }, { size: "1in" }));
  assert.ok(!sameSettings({ size: "14pt" }, { size: "14pt", font: "X" }));
  assert.ok(sameSettings({ font: "X" }, { font: " x" }));
});

test("names and the Style box's entries", () => {
  assert.equal(computedName({ font: "EB Garamond", size: "14pt" }), "EB Garamond 14 pt");
  assert.equal(computedName({ size: "10.5pt" }), "10.5 pt");
  assert.deepEqual(
    styleEntries(info).map((e) => e.label),
    ["Big", "14 pt", "DejaVu Sans Mono", "EB Garamond 14 pt"],
  );
  assert.equal(styleLabel(info, null), "Default text");
  assert.equal(styleLabel(info, undefined), "");
  assert.equal(styleLabel(info, "big"), "Big");
  assert.equal(styleLabel(info, "nope"), "“nope” (missing)");
});

test("the layout's styles, body and known ids", () => {
  const s = {
    preset: "paper",
    layout: { text: { font: "EB Garamond", size: "12pt" }, styles: { a: { size: "14pt" } } },
    overrides: { "styles.a.size": "14pt", "styles.old.name": "Old", "text.size": "12pt" },
    meta: {},
    custom: "",
    text_area: { column_width_pt: 400, height_pt: 600, font_size_pt: 12 },
  } as unknown as LayoutState;
  const t = textStylesOf(s);
  assert.deepEqual(t.body, { font: "EB Garamond", size: "12pt" });
  assert.deepEqual([...t.ids].sort(), ["a", "old"]);
});

test("the layout panel's styles: uses, a new named style, unused ones, deleting", () => {
  // Every reference, also several on a line and in multi-line spans.
  const uses = styleUses('#styled("mono")[a] #styled("mono")[b\nc] #styled("big")[x]\n#styled( "mono")[no] #styled("14pt")');
  assert.deepEqual([...uses].sort(), [["big", 1], ["mono", 2]]);
  // "Style N" (the first free N), its id never one the layout knows.
  assert.deepEqual(newNamedStyle(info), { id: "style-1", name: "Style 1" });
  const taken = { ...info, styles: { ...info.styles, s: { name: "style 1", size: "9pt" } }, ids: new Set([...info.ids, "s", "style-2"]) };
  assert.deepEqual(newNamedStyle(taken), { id: "style-2-2", name: "Style 2" });
  assert.deepEqual(newNamedStyle(info, new Map([["style-1", 2]])), { id: "style-1-2", name: "Style 1" });
  // Unused: unnamed ones only.
  assert.deepEqual(unusedStyles(info, uses).sort(), ["14pt", "eb-garamond-14pt"]);
  // Deleting: every field cleared; a preset's style (a field not stored) is removed.
  const s = {
    layout: { styles: { own: { name: "Own", size: "9pt" }, quote: { font: "EB Garamond", size: "13pt" } } },
    overrides: { "styles.own.name": "Own", "styles.own.size": "9pt", "styles.quote.size": "13pt" },
  } as unknown as LayoutState;
  assert.deepEqual(deleteChanges(s, ["own", "quote"]), {
    "styles.own.font": null,
    "styles.own.size": null,
    "styles.own.name": null,
    "styles.own.removed": null,
    "styles.quote.font": null,
    "styles.quote.size": null,
    "styles.quote.name": null,
    "styles.quote.removed": true,
  });
});

/** A state from `doc` with `|` marking the cursor (or two `|` a selection). */
function stateOf(marked: string): EditorState {
  const a = marked.indexOf("|");
  const b = marked.indexOf("|", a + 1);
  const doc = marked.replace(/\|/g, "");
  const selection = b < 0 ? EditorSelection.single(a) : EditorSelection.single(a, b - 1);
  const state = EditorState.create({ doc, selection, extensions: typstLanguage });
  ensureSyntaxTree(state, state.doc.length, 5000);
  return state.update({}).state;
}

test("the style at the selection", () => {
  const at = (m: string) => styleAt(stateOf(m), info);
  assert.deepEqual(at("a b|c"), { id: null, font: "Libertinus Serif", size: "11pt" });
  assert.deepEqual(at('#styled("mono")[a|b]'), { id: "mono", font: "DejaVu Sans Mono", size: "11pt" });
  // Nested: the inner style's fields over the outer's.
  assert.deepEqual(at('#styled("14pt")[a #styled("mono")[|b|]]'), { id: "mono", font: "DejaVu Sans Mono", size: "14pt" });
  // A missing style is body text.
  assert.deepEqual(at('#styled("nope")[a|]'), { id: "nope", font: "Libertinus Serif", size: "11pt" });
  // Styled text inside the selection: mixed where it differs.
  assert.deepEqual(at('|a #styled("14pt")[b] c|'), { id: undefined, font: "Libertinus Serif", size: undefined });
  assert.deepEqual(at('#styled("mono")[|a]\n#styled("mono")[b|]'), { id: "mono", font: "DejaVu Sans Mono", size: "11pt" });
  assert.deepEqual(at('#styled("mono")[|a]\nb|'), { id: undefined, font: undefined, size: "11pt" });
});

test("styled text's CSS: the font with fallbacks, the size relative to the body", () => {
  assert.equal(styledCss({ font: "EB Garamond", size: "14pt" }, 11), 'font-family: "Lethora:EB Garamond", "EB Garamond", var(--serif); font-size: calc(var(--text-size) * 1.273)');
  assert.equal(styledCss({ size: "22pt" }, 11), "font-size: calc(var(--text-size) * 2)");
  assert.equal(styledCss({ size: "22pt" }, null), "");
});
