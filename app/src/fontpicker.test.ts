// Tests for the font picker's filtering and CSS names. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { cssFamily, filterFonts } from "./fontpicker.ts";

const FONTS = ["Arial", "DejaVu Sans Mono", "EB Garamond", "Libertinus Sans", "Libertinus Serif", "New Computer Modern", "Noto Sans"];

test("filtering: every word, any case, those starting with the text first", () => {
  assert.deepEqual(filterFonts(FONTS, null), FONTS);
  assert.deepEqual(filterFonts(FONTS, "  "), FONTS);
  assert.deepEqual(filterFonts(FONTS, "sans"), ["DejaVu Sans Mono", "Libertinus Sans", "Noto Sans"]);
  assert.deepEqual(filterFonts(FONTS, "lib ser"), ["Libertinus Serif"]);
  assert.deepEqual(filterFonts(FONTS, "n"), ["New Computer Modern", "Noto Sans", "DejaVu Sans Mono", "EB Garamond", "Libertinus Sans", "Libertinus Serif"]);
  assert.deepEqual(filterFonts(FONTS, "xyz"), []);
});

test("a family's CSS: the embedded alias, the name, the fallback", () => {
  assert.equal(cssFamily("EB Garamond"), '"Lethora:EB Garamond", "EB Garamond", var(--serif)');
  assert.equal(cssFamily('Odd"Name', "serif"), '"Lethora:Odd\\"Name", "Odd\\"Name", serif');
});
