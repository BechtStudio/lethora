// Tests for "who wrote this": marks, tooltips, the legend and mapping over edits. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { EditorState } from "@codemirror/state";
import { authorMarkField, authorMarks, type Authors, legendOf, setAuthorMarks, UNKNOWN_COLOUR } from "./authors.ts";

const authors: Authors = {
  runs: [
    { from: 0, to: 5, user: "u1", time: 100 },
    { from: 5, to: 9, user: "u2", time: 200 },
    { from: 9, to: 12, user: null, time: null },
    { from: 12, to: 30, user: "u3", time: 300 },
  ],
  authors: [
    { user: "u3", colour: "nope", you: false },
    { user: "u2", name: "Bea Lind", colour: "#009e73", you: false },
    { user: "u1", name: "Max", colour: "#0072b2", you: true },
  ],
};

test("marks: names, you, unknown, colours, tooltips; mapped and clamped", () => {
  const marks = authorMarks(authors, (pos) => pos + 1, 20, (secs) => `t${secs}`);
  assert.deepEqual(marks, [
    { from: 1, to: 6, colour: "#0072b2", title: "You · t100" },
    { from: 6, to: 10, colour: "#009e73", title: "Bea Lind · t200" },
    { from: 10, to: 13, colour: UNKNOWN_COLOUR, title: "Unknown" },
    { from: 13, to: 20, colour: "#8e6c8a", title: "Someone · t300" },
  ]);
  // Runs that map to nothing (e.g. beyond the editor's text) are left out.
  assert.deepEqual(authorMarks(authors, () => 4, 20), []);
  // A real date and time by default.
  assert.match(authorMarks({ runs: [{ from: 0, to: 1, user: "u1", time: 1_761_000_000 }], authors: authors.authors }, (p) => p, 5)[0].title, /^You · .*\d/);
});

test("the legend: you first, then by name, unknown last", () => {
  assert.deepEqual(legendOf(authors), [
    { label: "You", colour: "#0072b2" },
    { label: "Bea Lind", colour: "#009e73" },
    { label: "Someone", colour: "#8e6c8a" },
    { label: "Unknown", colour: UNKNOWN_COLOUR },
  ]);
  assert.deepEqual(legendOf({ runs: [], authors: [] }), []);
  assert.deepEqual(legendOf({ runs: [{ from: 0, to: 1, user: "u1", time: 1 }], authors: authors.authors.slice(2) }), [{ label: "You", colour: "#0072b2" }]);
});

test("the editor's marks follow edits and are replaced", () => {
  let state = EditorState.create({ doc: "Hello brave new world", extensions: [authorMarkField] });
  const marks = [
    { from: 0, to: 5, colour: "#0072b2", title: "You" },
    { from: 6, to: 11, colour: "#009e73", title: "Bea" },
  ];
  state = state.update({ effects: setAuthorMarks(marks) }).state;
  assert.deepEqual(state.field(authorMarkField), marks);
  // Typed before Bea's word: it moves; at its edges: not taken in.
  state = state.update({ changes: [{ from: 0, insert: ">> " }, { from: 6, insert: "_" }, { from: 11, insert: "_" }] }).state;
  assert.equal(state.doc.toString(), ">> Hello _brave_ new world");
  assert.deepEqual(
    state.field(authorMarkField).map((m) => [m.from, m.to]),
    [
      [3, 8],
      [10, 15],
    ],
  );
  // Deleted: gone.
  state = state.update({ changes: { from: 9, to: 16 } }).state;
  assert.deepEqual(
    state.field(authorMarkField).map((m) => m.title),
    ["You"],
  );
  state = state.update({ effects: setAuthorMarks([]) }).state;
  assert.deepEqual(state.field(authorMarkField), []);
});
