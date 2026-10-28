// Tests for the asset type table. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { ASSET_TYPES, assetKind } from "./assettypes.ts";
import { IMAGE_EXTENSIONS } from "./figures.ts";

test("asset kinds by lowercase extension", () => {
  assert.equal(assetKind("assets/a.png"), "image");
  assert.equal(assetKind("assets/Ch 1/Bild.v2.jpeg"), "image");
  assert.equal(assetKind("assets/a.PNG"), null, "Typst matches extensions case-sensitively");
  assert.equal(assetKind("assets/data.csv"), null);
  assert.equal(assetKind("assets/png"), null);
});

test("image type: the preview's formats and the figure snippet", () => {
  // The preview (MIME types in figures.ts) knows every image extension.
  assert.deepEqual([...ASSET_TYPES.image.extensions].sort(), [...IMAGE_EXTENSIONS].sort());
  assert.equal(ASSET_TYPES.image.snippet("assets/Mein Bild.png"), 'image("assets/Mein Bild.png", width: 80%)');
});
