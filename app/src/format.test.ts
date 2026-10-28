// Tests for the formatting row's logic: inline formats, headings and lists. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { ensureSyntaxTree } from "@codemirror/language";
import { EditorSelection, EditorState, type TransactionSpec } from "@codemirror/state";
import { activeFormats, type InlineFormat, lineSpans, paragraphKind, restyle, setHeading, toggleInline, toggleList } from "./format.ts";
import { typstLanguage } from "./typst.ts";

/** A state from `doc` with `|` marking the cursor (or two `|` a selection, anchor first). */
function stateOf(marked: string): EditorState {
  const a = marked.indexOf("|");
  const b = marked.indexOf("|", a + 1);
  const doc = marked.replace(/\|/g, "");
  const selection = b < 0 ? EditorSelection.single(a) : EditorSelection.single(a, b - 1);
  const state = EditorState.create({ doc, selection, extensions: typstLanguage });
  ensureSyntaxTree(state, state.doc.length, 5000);
  return state.update({}).state;
}

/** The text after `spec` with `|` around the selection (one `|` for a cursor); null if nothing changed. */
function apply(f: (s: EditorState) => TransactionSpec, marked: string): string | null {
  const state = stateOf(marked);
  const spec = f(state);
  if (!spec.changes) return null;
  const next = state.update(spec).state;
  const { from, to } = next.selection.main;
  const text = next.doc.toString();
  return from === to ? text.slice(0, from) + "|" + text.slice(from) : text.slice(0, from) + "|" + text.slice(from, to) + "|" + text.slice(to);
}

const fmt = (format: InlineFormat, marked: string) => apply((s) => toggleInline(s, format), marked);

test("wrap a selection in each format", () => {
  assert.equal(fmt("bold", "a |word| b"), "a *|word|* b");
  assert.equal(fmt("italic", "a |word| b"), "a _|word|_ b");
  assert.equal(fmt("underline", "a |word| b"), "a #underline[|word|] b");
  assert.equal(fmt("strike", "a |word| b"), "a #strike[|word|] b");
  assert.equal(fmt("super", "x|2|"), "x#super[|2|]");
  assert.equal(fmt("sub", "H|2|O"), "H#sub[|2|]O");
});

test("unwrap a selection inside or exactly over a span", () => {
  assert.equal(fmt("bold", "a *|word|* b"), "a |word| b");
  assert.equal(fmt("bold", "a |*word*| b"), "a |word| b");
  assert.equal(fmt("bold", "a *one |two| three* b"), "a one |two| three b");
  assert.equal(fmt("italic", "_|x|_"), "|x|");
  assert.equal(fmt("underline", "a #underline[|word|] b"), "a |word| b");
  assert.equal(fmt("underline", "a |#underline[word]| b"), "a |word| b");
  assert.equal(fmt("sub", "H#sub[|2|]O"), "H|2|O");
});

test("a partial word uses the function form; both forms toggle off", () => {
  assert.equal(fmt("bold", "wo|rd|s"), "wo#strong[|rd|]s");
  assert.equal(fmt("italic", "|wor|d"), "#emph[|wor|]d");
  assert.equal(fmt("bold", "wo#strong[|rd|]s"), "wo|rd|s");
  assert.equal(fmt("italic", "#emph[x|y|z]"), "x|y|z");
  // The content holding the delimiter.
  assert.equal(fmt("italic", "|snake_case|"), "#emph[|snake_case|]");
});

test("whitespace stays outside the wrapped part", () => {
  assert.equal(fmt("bold", "a| word |b"), "a| *word* |b");
});

test("cursor in a word formats the word", () => {
  assert.equal(fmt("bold", "a wo|rd b"), "a *wo|rd* b");
  assert.equal(fmt("underline", "a wo|rd b"), "a #underline[wo|rd] b");
  assert.equal(fmt("italic", "a Grö|ße b"), "a _Grö|ße_ b");
});

test("cursor between words inserts an empty pair", () => {
  assert.equal(fmt("bold", "a | b"), "a *|* b");
  assert.equal(fmt("italic", "|"), "_|_");
  assert.equal(fmt("strike", "a | b"), "a #strike[|] b");
  // Next to a letter: the function form (`**` would be text there).
  assert.equal(fmt("bold", "word|"), "word#strong[|]");
  assert.equal(fmt("bold", "|word"), "#strong[|]word");
});

test("cursor inside a span unwraps it", () => {
  assert.equal(fmt("bold", "a *bo|ld* b"), "a bo|ld b");
  assert.equal(fmt("bold", "a *bold|* b"), "a bold| b");
  assert.equal(fmt("underline", "#underline[a |b]"), "a |b");
  assert.equal(fmt("bold", "#strong[a|]"), "a|");
  // Nested: the innermost span of the format.
  assert.equal(fmt("italic", "_a *b|* c_"), "a *b|* c");
});

test("overlapping spans merge into one", () => {
  assert.equal(fmt("bold", "|ab *cd* ef|"), "*|ab cd ef|*");
  assert.equal(fmt("bold", "|ab *c|d* ef"), "*|ab c|d* ef");
});

test("multi-line selections format each line", () => {
  assert.equal(fmt("bold", "|one\n\n two |"), "*|one*\n\n *two* |");
  assert.equal(fmt("bold", "*|one*\n\n*two|*"), "|one\n\ntwo|");
  // Not every line bold: all become bold.
  assert.equal(fmt("bold", "*|one*\ntwo|"), "*|one*\n*two|*");
  // Blank lines are skipped.
  assert.equal(fmt("italic", "|a\n   \nb|"), "_|a_\n   \n_b|_");
});

test("nothing in math, raw text or code", () => {
  assert.equal(fmt("bold", "$x |+| y$"), null);
  assert.equal(fmt("bold", "a `ra|w` b"), null);
  assert.equal(fmt("bold", "#f(|x|)"), null);
  assert.equal(fmt("bold", "|a $x$ b|"), "*|a $x$ b|*");
  // Not into a call's name or brackets, a citation or a label.
  assert.equal(fmt("bold", "a #under|line[b] c"), null);
  assert.equal(fmt("sub", "#super|[|x]"), null);
  assert.equal(fmt("bold", "see @wo|olf, p. 4"), null);
  assert.equal(fmt("italic", "= Title <c|h-1>"), null);
  assert.equal(fmt("bold", "|see @woolf|, p. 4"), "*|see @woolf|*, p. 4");
  assert.equal(fmt("bold", "|#underline[b]|"), "*|#underline[b]|*");
  // Not across another format's span, nor a call around unbalanced brackets.
  assert.equal(fmt("bold", "|a _b| c_"), null);
  assert.equal(fmt("underline", "|a [b|"), null);
  assert.equal(fmt("bold", "|a [b|"), "*|a [b|*");
  // A multi-line formula in between is left alone.
  assert.equal(fmt("bold", "|a\n$ x\ny $\nb|"), "*|a*\n$ x\ny $\n*b|*");
});

test("active formats at the cursor and over a selection", () => {
  const active = (m: string) => [...activeFormats(stateOf(m))].sort();
  assert.deepEqual(active("a *b|c* d"), ["bold"]);
  assert.deepEqual(active("a *bc*| d"), []);
  assert.deepEqual(active("*_a #underline[b|]_*"), ["bold", "italic", "underline"]);
  assert.deepEqual(active("x#super[|2]"), ["super"]);
  assert.deepEqual(active("#strong[|ab|]"), ["bold"]);
  assert.deepEqual(active("*|a* b|"), []);
  assert.deepEqual(active("*|a*\n\n*b|*"), ["bold"]);
  // The selection takes in an italic delimiter outside the bold span.
  assert.deepEqual(active("*|a*\n\n_*b|*_"), []);
});

const heading = (level: number, marked: string) => apply((s) => setHeading(s, level), marked);
const list = (kind: "bullet" | "numbered", marked: string) => apply((s) => toggleList(s, kind), marked);

test("headings 0–6 switch the line's prefix", () => {
  assert.equal(heading(1, "Ti|tle"), "= Ti|tle");
  assert.equal(heading(3, "= Ti|tle"), "=== Ti|tle");
  assert.equal(heading(6, "|Title"), "====== |Title");
  assert.equal(heading(0, "== Ti|tle"), "Ti|tle");
  assert.equal(heading(2, "== Ti|tle"), null);
  assert.equal(heading(0, "plain|"), null);
  // Every touched line, blank ones skipped; not the line the selection only ends at.
  assert.equal(heading(2, "|a\n\n= b\n|c"), "== |a\n\n== b\n|c");
});

test("a heading replaces a list marker", () => {
  assert.equal(heading(1, "- it|em"), "= it|em");
  assert.equal(heading(2, "  + it|em"), "== it|em");
  assert.equal(heading(1, "1. it|em"), "= it|em");
});

test("list toggles", () => {
  assert.equal(list("bullet", "it|em"), "- it|em");
  assert.equal(list("bullet", "- it|em"), "it|em");
  assert.equal(list("numbered", "it|em"), "+ it|em");
  assert.equal(list("numbered", "+ it|em"), "it|em");
  assert.equal(list("numbered", "3. it|em"), "it|em");
  // Indentation stays.
  assert.equal(list("bullet", "  it|em"), "  - it|em");
  assert.equal(list("bullet", "  - it|em"), "  it|em");
  // Mixed lines: all get it.
  assert.equal(list("bullet", "|- a\nb|"), "|- a\n- b|");
  assert.equal(list("bullet", "|- a\n- b|"), "|a\nb|");
  // A cursor at the line start ends up after the marker.
  assert.equal(list("bullet", "|item"), "- |item");
});

test("lists replace headings and each other", () => {
  assert.equal(list("bullet", "== He|ad"), "- He|ad");
  assert.equal(list("numbered", "- it|em"), "+ it|em");
  assert.equal(list("bullet", "  + it|em"), "  - it|em");
  assert.equal(list("bullet", "|+ a\n- b|"), "|- a\n- b|");
});

test("paragraph kind of the touched lines", () => {
  const kind = (m: string) => paragraphKind(stateOf(m));
  assert.deepEqual(kind("== a|b"), { heading: 2, bullet: false, numbered: false });
  assert.deepEqual(kind("a|b"), { heading: 0, bullet: false, numbered: false });
  assert.deepEqual(kind("|- a\n\n- b|"), { heading: 0, bullet: true, numbered: false });
  assert.deepEqual(kind("|+ a\n2. b|"), { heading: 0, bullet: false, numbered: true });
  assert.deepEqual(kind("|= a\n== b|"), { heading: null, bullet: false, numbered: false });
  assert.deepEqual(kind("|"), { heading: 0, bullet: false, numbered: false });
});

test("styled spans are read with their style id", () => {
  const state = stateOf('a #styled("big")[b *c*] #styled("x-1")[] #styled(x)[d] #styled("y", z)[e]|');
  const spans = lineSpans(state.doc.line(1)).map((s) => [s.format, s.style ?? "", state.sliceDoc(s.contentFrom, s.contentTo)]);
  assert.deepEqual(spans, [
    ["bold", "", "c"],
    ["styled", "big", "b *c*"],
    ["styled", "x-1", ""],
  ]);
});

test("inline formats respect styled spans", () => {
  assert.equal(fmt("bold", '#styled("a")[x |y| z]'), '#styled("a")[x *|y|* z]');
  assert.equal(fmt("bold", '|#styled("a")[x]|'), '*|#styled("a")[x]|*');
  // Not across one.
  assert.equal(fmt("bold", '|a #styled("x")[b| c]'), null);
  assert.deepEqual([...activeFormats(stateOf('#styled("a")[*b|*]'))], ["bold"]);
});

/** Restyles to `to` whatever the text has. */
const style = (to: string | null, marked: string) => apply((s) => restyle(s, () => to), marked);

test("restyle wraps a selection per line, trimmed", () => {
  assert.equal(style("big", "a |word| b"), 'a #styled("big")[|word|] b');
  assert.equal(style("big", "a| two words |b"), 'a| #styled("big")[two words] |b');
  assert.equal(style("big", "|one\n\n= two|"), '#styled("big")[|one]\n\n= #styled("big")[two|]');
  // Spans inside stay (the inner one applies).
  assert.equal(style("big", '|a #styled("x")[b] c|'), '#styled("big")[|a #styled("x")[b] c|]');
  // Default text outside any span: nothing.
  assert.equal(style(null, "a |word| b"), null);
});

test("restyle re-points or unwraps a span whose content is selected", () => {
  assert.equal(style("b", 'x #styled("a")[|word|] y'), 'x #styled("b")[|word|] y');
  assert.equal(style("b", 'x |#styled("a")[word]| y'), 'x |#styled("b")[word]| y');
  assert.equal(style("b", '#styled("a")[ |word| ]'), '#styled("b")[ |word| ]');
  assert.equal(style(null, 'x #styled("a")[|word|] y'), "x |word| y");
  // The same style: nothing.
  assert.equal(style("a", '#styled("a")[|word|]'), null);
});

test("restyle nests a part strictly inside a span, or splits it for default text", () => {
  assert.equal(style("b", '#styled("a")[one |two| three]'), '#styled("a")[one #styled("b")[|two|] three]');
  assert.equal(style(null, '#styled("a")[one |two| three]'), '#styled("a")[one ]|two|#styled("a")[ three]');
  // At an edge the span just ends earlier or starts later.
  assert.equal(style(null, '#styled("a")[|one| two]'), '|one|#styled("a")[ two]');
  assert.equal(style(null, '#styled("a")[one |two|]'), '#styled("a")[one ]|two|');
});

test("restyle at a cursor: the span, the word, or an empty span", () => {
  assert.equal(style("b", '#styled("a")[wo|rd]'), '#styled("b")[wo|rd]');
  assert.equal(style(null, '#styled("a")[wo|rd]'), "wo|rd");
  assert.equal(style("b", "a wo|rd c"), 'a #styled("b")[wo|rd] c');
  assert.equal(style("b", "a | c"), 'a #styled("b")[|] c');
  assert.equal(style("b", "|= Title"), '= #styled("b")[|]Title');
  assert.equal(style(null, "a wo|rd c"), null);
});

test("restyle keeps out of math, code and other spans", () => {
  assert.equal(style("b", "$x |+| y$"), null);
  assert.equal(style("b", "#f(|x|)"), null);
  assert.equal(style("b", "|a _b| c_"), null);
  assert.equal(style("b", "|a [b|"), null);
  // The current style is what the innermost span has.
  const seen: (string | null)[] = [];
  apply((s) => restyle(s, (current) => (seen.push(current), current)), '#styled("a")[x #styled("b")[|y|]]');
  assert.deepEqual(seen, ["b"]);
});

test("inline formats start after a heading or list marker", () => {
  assert.equal(fmt("underline", "|= Title"), "= #underline[|]Title");
  assert.equal(fmt("bold", "|= Title|"), "|= *Title|*");
  assert.equal(fmt("italic", "|- one\n- two|"), "|- _one_\n- _two|_");
  assert.equal(fmt("bold", "  |+ item|"), "  |+ *item|*");
});
