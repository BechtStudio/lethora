// Tests for labels, `@` completion, citation inserts and unknown references. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { acceptCompletion, CompletionContext, type CompletionResult } from "@codemirror/autocomplete";
import { ensureSyntaxTree } from "@codemirror/language";
import { EditorSelection, EditorState, Text } from "@codemirror/state";
import { typstLanguage } from "./typst.ts";
import type { BibEntry } from "./bib.ts";
import {
  completionKeys,
  completionOptions,
  entriesByKey,
  findRefs,
  insertCitation,
  KIND_NAMES,
  labelsFor,
  refCompletionSource,
  refContext,
  scanLabels,
  unknownRefs,
  VALID_FOR,
} from "./refs.ts";

function stateOf(doc: string, anchor = doc.length, head = anchor): EditorState {
  const state = EditorState.create({ doc, selection: EditorSelection.single(anchor, head), extensions: typstLanguage });
  ensureSyntaxTree(state, state.doc.length, 5000);
  return state;
}

const DOC = `= Introduction <sec:intro>

Some text <note> and more.

$ A = pi r^2 $ <eq:area>

Inline $x$<eq:x> too, but \\$5 <not-eq>.

#figure(
  image("assets/a.png"),
  caption: [A lamp (lit)],
) <fig:lamp>

== Heading <sec:h> with text after

Email me@example.org and \`@raw\` /* @comment */ @woolf1927.
`;

test("labels and what they label", () => {
  const labels = scanLabels(DOC).map((l) => `${l.name}:${l.kind}:${l.text}`);
  assert.deepEqual(labels, [
    "sec:intro:heading:Introduction",
    "note:other:Some text <note> and more.",
    "eq:area:equation:A = pi r^2",
    "eq:x:equation:x",
    "not-eq:other:Inline $x$<eq:x> too, but \\$5 <not-eq>.",
    "fig:lamp:figure:A lamp (lit)",
    "sec:h:other:== Heading <sec:h> with text after",
  ]);
  const eq = scanLabels(DOC).find((l) => l.name === "eq:area")!;
  assert.equal(eq.display, true);
  assert.equal(DOC.slice(eq.pos, eq.pos + 9), "<eq:area>");
  // Comparisons and arrows are no labels.
  assert.deepEqual(scanLabels("a < b > c, x <- y, $a<=b$"), []);
});

test("Tab accepts a completion like Enter", () => {
  assert.deepEqual(completionKeys.map((b) => [b.key, b.run]), [["Tab", acceptCompletion]]);
});

test("a figure of a table is labelled as a table", () => {
  const text = `#figure(
  table(
    columns: 2,
    [a], [b],
  ),
  caption: [Nights],
) <tab:1>

#figure(table[x]) <tab:2>

#figure(image("assets/table.png")) <fig:t>
`;
  assert.deepEqual(
    scanLabels(text).map((l) => `${l.name}:${l.kind}`),
    ["tab:1:table", "tab:2:table", "fig:t:figure"],
  );
  assert.equal(KIND_NAMES.table, "Table");
});

test("labels are cached per document version", () => {
  const doc = Text.of(DOC.split("\n"));
  const a = labelsFor(doc);
  assert.equal(labelsFor(doc), a);
  assert.equal(a.byName.get("fig:lamp")?.kind, "figure");
  const changed = doc.replace(0, 0, Text.of(["<new> "]));
  const b = labelsFor(changed);
  assert.notEqual(b, a);
  assert.ok(b.byName.has("new"));
});

test("label scan of a book-size text stays cheap", () => {
  const chapter = "= Chapter <ch>\n\n" + "The lens turned once every twenty seconds, $r = 22$, see @woolf1927. ".repeat(400) + "\n\n$ x $ <eq:a>\n\n";
  const text = chapter.repeat(Math.ceil(5_000_000 / chapter.length));
  const t = performance.now();
  const labels = scanLabels(text);
  const ms = performance.now() - t;
  assert.ok(labels.length > 300);
  assert.ok(ms < 400, `scan took ${ms.toFixed(0)} ms`);
});

test("@ context: after @ and key characters only, not in emails, raw, comments, code or math", () => {
  const ctx = (doc: string, pos = doc.length) => refContext(stateOf(doc, pos), pos);
  assert.deepEqual(ctx("see @"), { from: 5, text: "" });
  assert.deepEqual(ctx("see @wo"), { from: 5, text: "wo" });
  assert.deepEqual(ctx("(@fig:la"), { from: 2, text: "fig:la" });
  assert.deepEqual(ctx("@"), { from: 1, text: "" });
  assert.equal(ctx("me@exa"), null);
  assert.equal(ctx("see @wo rd"), null);
  assert.equal(ctx("no at sign"), null);
  assert.equal(ctx("`raw @wo"), null);
  assert.equal(ctx("``` @a\n@b"), null);
  assert.equal(ctx("/* @wo"), null);
  assert.equal(ctx("// @wo"), null);
  assert.equal(ctx("$x @wo"), null);
  assert.equal(ctx("#f(@wo"), null);
  assert.deepEqual(ctx("`raw` @wo"), { from: 7, text: "wo" });
});

const ENTRIES: BibEntry[] = [
  { key: "woolf1927", entry_type: "book", title: "To the Lighthouse", authors: ["Woolf, Virginia"], year: "1927" },
  { key: "levitt2013", entry_type: "book", title: null, authors: [], year: null },
];

test("completion options merge sources and labels", () => {
  const options = completionOptions(ENTRIES, scanLabels(DOC));
  const rows = options.map((o) => `${typeof o.section === "string" ? o.section : o.section?.name}|${o.label}|${o.detail}|${o.info ?? ""}`);
  assert.deepEqual(rows.slice(0, 2), ["Sources|woolf1927|Woolf 1927|To the Lighthouse", "Sources|levitt2013||"]);
  assert.ok(rows.includes("Labels|fig:lamp|Figure|A lamp (lit)"));
  assert.ok(rows.includes("Labels|eq:area|Equation|A = pi r^2"));
  assert.ok(rows.includes("Labels|sec:intro|Heading|Introduction"));
  // Duplicate labels appear once.
  assert.equal(completionOptions([], scanLabels("<a> <a>")).length, 1);
  // Typing keeps filtering locally while the word is a key.
  assert.ok(VALID_FOR.test("fig:la-2.x_y"));
  assert.ok(!VALID_FOR.test("fig la"));
});

test("completion source: from after @, null outside references", () => {
  const source = refCompletionSource(() => ENTRIES);
  const run = (doc: string, explicit = false) => source(new CompletionContext(stateOf(doc), doc.length, explicit)) as CompletionResult | null;
  const r = run("Text <fig:a> and @wo")!;
  assert.equal(r.from, "Text <fig:a> and @".length);
  assert.deepEqual(r.options.map((o) => o.label), ["woolf1927", "levitt2013", "fig:a"]);
  assert.equal(run("mail me@wo"), null);
  assert.equal(run("plain words"), null);
  assert.equal(refCompletionSource(() => [])(new CompletionContext(stateOf("@x"), 2, false)), null);
});

test("insert citation adds a space when needed", () => {
  const apply = (doc: string, pos: number) => {
    const state = stateOf(doc, pos);
    const tr = state.update(insertCitation(state, "woolf1927"));
    return `${tr.state.doc.toString()}|${tr.state.selection.main.head}`;
  };
  assert.equal(apply("see", 3), "see @woolf1927|14");
  assert.equal(apply("see ", 4), "see @woolf1927|14");
  assert.equal(apply("(", 1), "(@woolf1927|11");
  assert.equal(apply("", 0), "@woolf1927|10");
  assert.equal(apply("a\nb", 2), "a\n@woolf1927b|12");
});

test("references and unknown ones", () => {
  const doc = "See @woolf1927, @fig:lamp and @nobody. Not me@mail.org or `@raw`.\n<fig:lamp>";
  const state = stateOf(doc);
  const refs = findRefs(state, 0, doc.length);
  assert.deepEqual(refs.map((r) => r.key), ["woolf1927", "fig:lamp", "nobody"]);
  assert.equal(doc.slice(refs[2].from, refs[2].to), "@nobody");
  const keys = entriesByKey(ENTRIES);
  const labels = labelsFor(state.doc).byName;
  const unknown = unknownRefs(refs, (k) => keys.has(k) || labels.has(k));
  assert.deepEqual(unknown.map((r) => r.key), ["nobody"]);
  assert.equal(entriesByKey(ENTRIES), keys);
});
