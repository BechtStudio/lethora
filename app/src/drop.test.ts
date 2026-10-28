// Tests for drops into the text: paragraph boundaries, what a drag carries,
// supported files. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { ensureSyntaxTree } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { ASSET_DRAG_TYPE, FOLDER_DRAG_TYPE } from "./assets.ts";
import { boundariesAround, boundaryAt, dragContent, droppedContent, nearestBoundary, skippedMessage, supportedFiles } from "./drop.ts";
import { topLevelAt, typstLanguage } from "./typst.ts";

function stateOf(doc: string): EditorState {
  const state = EditorState.create({ doc, extensions: typstLanguage });
  ensureSyntaxTree(state, state.doc.length, 5000);
  return state;
}

const LINES = [
  "Intro line one", // 1
  "still intro.",
  "", // 3: a boundary
  "#figure(",
  '  image("assets/a.png"),',
  "", // 6: inside a call
  "  caption: [Cap],",
  ") <fig:a>",
  "", // 9: a boundary
  "$ x",
  "", // 11: inside math
  "y $",
  "", // 13: a boundary
  "```",
  "", // 15: inside raw text
  "```",
  "", // 17: a boundary
  "#quote[",
  "", // 19: inside a content block
  "]",
  "/* a",
  "", // 22: inside a comment
  "*/",
  "", // 24: a boundary
  "", // 25: the same gap
  "End.",
];

test("paragraph boundaries: blank lines between blocks, never inside a call, math, raw, a content block or a comment", () => {
  const state = stateOf(LINES.join("\n"));
  const doc = state.doc;
  const top = (from: number) => topLevelAt(state, from);
  const at = (n: number) => doc.line(n).from;
  const found = new Set<number>();
  for (let n = 1; n <= doc.lines; n++) {
    const { before, after } = boundariesAround(doc, at(n), top);
    found.add(before!).add(after!);
  }
  assert.deepEqual([...found].sort((a, b) => a - b), [0, at(3), at(9), at(13), at(17), at(24), doc.length]);
  // Inside the figure: the gaps around it.
  assert.deepEqual(boundariesAround(doc, at(6), top), { before: at(3), after: at(9) });
  assert.deepEqual(boundariesAround(doc, at(9), top), { before: at(9), after: at(13) });
  assert.deepEqual(boundariesAround(doc, at(25), top), { before: at(24), after: doc.length });
  // The nearer one by the given distance; by characters for a position.
  assert.equal(nearestBoundary(doc, at(5), top, (b) => Math.abs(b - at(5))), at(3));
  assert.equal(nearestBoundary(doc, at(7), top, (b) => Math.abs(b - at(7))), at(9));
  assert.equal(boundaryAt(state, at(26) + 2), doc.length);
  assert.equal(boundaryAt(state, at(2) + 3), at(3));
  // A boundary stays where it is.
  for (const b of found) assert.equal(boundaryAt(state, b), b);
});

test("paragraph boundaries: short texts", () => {
  for (const [text, before, after] of [
    ["", 0, 0],
    ["A", 0, 1],
    ["A\n", 0, 2],
  ] as const) {
    const state = stateOf(text);
    assert.deepEqual(boundariesAround(state.doc, 0, (f) => topLevelAt(state, f)), { before, after }, JSON.stringify(text));
  }
});

test("drag content: files, panel items, but not folders or other drags", () => {
  assert.equal(dragContent(["Files"]), "files");
  assert.equal(dragContent([ASSET_DRAG_TYPE]), "asset");
  assert.equal(dragContent([ASSET_DRAG_TYPE, FOLDER_DRAG_TYPE]), null);
  assert.equal(dragContent(["text/plain"]), null);
  const data = (types: string[], files: File[], path = "") => ({ types, files, getData: (t: string) => (t === ASSET_DRAG_TYPE ? path : "") });
  const file = new File(["x"], "a.png");
  assert.deepEqual(droppedContent(data(["Files"], [file])), { files: [file] });
  assert.equal(droppedContent(data(["Files"], [])), null);
  assert.deepEqual(droppedContent(data([ASSET_DRAG_TYPE], [], "assets/f/a.png")), { path: "assets/f/a.png" });
  // A folder or the Sources (even without the folder type), or no path.
  for (const path of ["assets/f/", "sources:", ""]) assert.equal(droppedContent(data([ASSET_DRAG_TYPE], [], path)), null, path);
  assert.equal(droppedContent(data([ASSET_DRAG_TYPE, FOLDER_DRAG_TYPE], [], "assets/f/")), null);
});

test("supported files: by extension in any case and size, in order; one message for the rest", () => {
  const f = (name: string, size = 10) => ({ name, size });
  const { ok, skipped } = supportedFiles([f("A.PNG"), f("notes.txt"), f("big.jpg", 50 * 1024 * 1024 + 1), f("b.svg"), f("noext")]);
  assert.deepEqual(ok.map((x) => x.name), ["A.PNG", "b.svg"]);
  assert.deepEqual(skipped, ["notes.txt", "big.jpg", "noext"]);
  assert.equal(skippedMessage(["a.txt"]), "Skipped a.txt: only png, jpg, jpeg, gif, svg and webp files up to 50 MB can be added.");
  assert.match(skippedMessage(["a", "b"]), /^Skipped a and b:/);
  assert.match(skippedMessage(["a", "b", "c", "d", "e"]), /^Skipped a, b, c and 2 more:/);
});
