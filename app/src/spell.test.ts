// Tests for the words the spell checker sends (no DOM). Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { ensureSyntaxTree } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { wordsIn } from "./spell.ts";
import { typstLanguage } from "./typst.ts";

function words(doc: string): string[] {
  const state = EditorState.create({ doc, extensions: typstLanguage });
  ensureSyntaxTree(state, state.doc.length, 5000);
  return wordsIn(state, 0, state.doc.length).map((w) => w.word);
}

test("plain words, apostrophes and hyphenated words", () => {
  assert.deepEqual(words("The keeper's daughter don’t well-known E-Mail-Adresse"), ["The", "keeper's", "daughter", "don’t", "well-known", "E-Mail-Adresse"]);
  assert.deepEqual(words("'quoted' words -- and --- dashes"), ["quoted", "words", "and", "dashes"]);
});

test("positions are exactly the letters, also inside markup", () => {
  const doc = "= Head\n*bold* and _emph_ \\*esc";
  const state = EditorState.create({ doc, extensions: typstLanguage });
  ensureSyntaxTree(state, state.doc.length, 5000);
  const found = wordsIn(state, 0, doc.length).map((w) => [w.word, doc.slice(w.from, w.to)]);
  assert.deepEqual(found, [
    ["Head", "Head"],
    ["bold", "bold"],
    ["and", "and"],
    ["emph", "emph"],
    ["esc", "esc"],
  ]);
});

test("math, raw text, code, citations, labels and comments are skipped", () => {
  assert.deepEqual(words("One $alpha beta$ two `rawword` three"), ["One", "two", "three"]);
  assert.deepEqual(words("See @smith2020 and <sec:intro> here // a commnt\n/* blok */ end"), ["See", "and", "here", "end"]);
  assert.deepEqual(words('#set text(lang: "de")\n#emph[Inside] #figure(image("a.png"), caption: [Captioned])'), ["Inside", "Captioned"]);
});

test("links, digits, acronyms and single letters are skipped", () => {
  assert.deepEqual(words("Visit https://example.com/pathword or www.lethora.app now"), ["Visit", "or", "now"]);
  assert.deepEqual(words("Mail max.mustermann@example.org today"), ["Mail", "today"]);
  assert.deepEqual(words("A NASA probe in 2024 saw B12 and x"), ["probe", "in", "saw", "and"]);
});

test("only the lines of the range, as far as parsed", () => {
  const doc = "first line\nsecond line\nthird line";
  const state = EditorState.create({ doc, extensions: typstLanguage });
  ensureSyntaxTree(state, state.doc.length, 5000);
  // A range inside a line takes the whole line.
  assert.deepEqual(
    wordsIn(state, 14, 16).map((w) => w.word),
    ["second", "line"],
  );
});
