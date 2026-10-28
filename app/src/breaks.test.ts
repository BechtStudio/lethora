// Tests for Enter (paragraph) and Shift+Enter (forced line break) and the mode at the cursor. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { ensureSyntaxTree } from "@codemirror/language";
import { EditorSelection, EditorState, type TransactionSpec } from "@codemirror/state";
import { joinBackward, joinForward, lineBreak, paragraphBreak } from "./breaks.ts";
import { modeAt, typstLanguage } from "./typst.ts";

/** A state from `doc` with `|` marking the cursor (or two `|` a selection). */
function stateOf(marked: string): EditorState {
  const a = marked.indexOf("|");
  const b = marked.indexOf("|", a + 1);
  const doc = marked.replace(/\|/g, "");
  const selection = b < 0 ? EditorSelection.single(a) : EditorSelection.single(a, b - 1);
  const state = EditorState.create({ doc, selection, extensions: typstLanguage });
  ensureSyntaxTree(state, state.doc.length, 5000);
  return state.update({}).state; // takes the parsed tree into the state
}

/** The text after the key with `|` at the cursor, or null where the plain line break applies. */
function press(key: (s: EditorState) => TransactionSpec | null, marked: string): string | null {
  const state = stateOf(marked);
  const spec = key(state);
  if (!spec) return null;
  const next = state.update(spec).state;
  const head = next.selection.main.head;
  const text = next.doc.toString();
  return text.slice(0, head) + "|" + text.slice(head);
}

const enter = (m: string) => press(paragraphBreak, m);
const shiftEnter = (m: string) => press(lineBreak, m);

test("mode at the cursor: markup, math, code, raw", () => {
  const at = (m: string) => {
    const s = stateOf(m);
    return modeAt(s, s.selection.main.head);
  };
  assert.equal(at("plain |text"), "markup");
  assert.equal(at("a $x + |y$ b"), "math");
  assert.equal(at("$ a\n  + b|\n$"), "math");
  assert.equal(at("a $x$|"), "markup");
  assert.equal(at("```\ncode|\n```"), "raw");
  assert.equal(at("use `ra|w` here"), "raw");
  assert.equal(at("#figure(\n  image(\"a.png\"),|\n)"), "code");
  assert.equal(at("#{\n  let x = 1|\n}"), "code");
  assert.equal(at("/* a\nb| */"), "code");
  // An expression a line break ends belongs to the markup around it.
  assert.equal(at("Text #lorem(5)|"), "markup");
  assert.equal(at("#let x = 1|"), "markup");
  assert.equal(at("#emph[in |content]"), "markup");
  // Far into a long document (resumes from a cached tokenizer state).
  const long = "A paragraph of some words.\n\n".repeat(2000);
  assert.equal(at(`${long}$ x\n  = |y $`), "math");
  assert.equal(at(`${long}$ x $ |`), "markup");
});

test("Enter in markup: a blank line (a line break on a blank line)", () => {
  assert.equal(enter("Hello|"), "Hello\n\n|");
  assert.equal(enter("Hello |world"), "Hello\n\n|world");
  assert.equal(enter("= Title|"), "= Title\n\n|");
  assert.equal(enter("a\n\n|"), "a\n\n\n|");
  assert.equal(enter("a\n  |\nb"), "a\n  \n|\nb");
  assert.equal(enter("one |two| three"), "one\n\n|three");
  assert.equal(enter("Text #lorem(5)|"), "Text #lorem(5)\n\n|");
  assert.equal(enter("  indented|"), "  indented\n\n  |");
});

test("Enter in math, code and raw: a plain line break", () => {
  assert.equal(enter("$ a + b|"), null);
  assert.equal(enter("```\ncode|\n```"), null);
  assert.equal(enter("#figure(\n  image(\"a.png\"),|\n)"), null);
});

test("Enter continues lists, enums and term lists", () => {
  assert.equal(enter("- one|"), "- one\n- |");
  assert.equal(enter("+ one|"), "+ one\n+ |");
  assert.equal(enter("3. three|"), "3. three\n4. |");
  assert.equal(enter("/ Term: what it is|"), "/ Term: what it is\n/ |");
  assert.equal(enter("  - nested|"), "  - nested\n  - |");
  assert.equal(enter("- one| two"), "- one\n- |two");
  // Before the marker: a paragraph break.
  assert.equal(enter("|- one"), "\n\n|- one");
  // Not items.
  assert.equal(enter("-5 degrees|"), "-5 degrees\n\n|");
  assert.equal(enter("/path|"), "/path\n\n|");
});

test("Enter on an empty item ends the list", () => {
  assert.equal(enter("- one\n- |"), "- one\n\n|");
  assert.equal(enter("1. one\n2.|"), "1. one\n\n|");
  assert.equal(enter("/ A: b\n/ |"), "/ A: b\n\n|");
});

test("Shift+Enter: a forced line break in markup and math", () => {
  assert.equal(shiftEnter("word|"), "word \\\n|");
  assert.equal(shiftEnter("word |next"), "word \\\n|next");
  assert.equal(shiftEnter("word| next"), "word \\\n|next");
  assert.equal(shiftEnter("  indented|"), "  indented \\\n  |");
  // In an item the next line stays in the item.
  assert.equal(shiftEnter("- one|"), "- one \\\n  |");
  assert.equal(shiftEnter("$ a = b|\n$"), "$ a = b \\\n|\n$");
});

test("Shift+Enter in code and raw: a plain line break", () => {
  assert.equal(shiftEnter("```\ncode|\n```"), null);
  assert.equal(shiftEnter("#{\n  let x = 1|\n}"), null);
});

const backspace = (m: string) => press(joinBackward, m);
const del = (m: string) => press(joinForward, m);

test("Backspace removes a forced line break or a paragraph break whole", () => {
  // Forced line break: from the next line's start, or right after the `\`.
  assert.equal(backspace("Test \\\n|Test2"), "Test |Test2");
  assert.equal(backspace("Test \\|\nTest2"), "Test |Test2");
  assert.equal(backspace("Test \\\n|"), "Test|");
  // Paragraph break: the paragraphs run together, as in Word.
  assert.equal(backspace("One\n\n|Two"), "One|Two");
  assert.equal(backspace("One\n\n\n|Two"), "One|Two");
  assert.equal(backspace("One\n\n|"), "One|");
  assert.equal(backspace("One \\\n\n|Two"), "One|Two");
  // Elsewhere the default applies.
  assert.equal(backspace("One\n|Two"), null);
  assert.equal(backspace("O|ne"), null);
  assert.equal(backspace("|One"), null);
  assert.equal(backspace("Esc\\\\|\nTwo"), null);
  assert.equal(backspace("$ a \\\n|b $"), null);
  assert.equal(backspace("```\na\n\n|b\n```"), null);
});

test("Delete removes a forced line break or a paragraph break whole", () => {
  assert.equal(del("Test \\|\nTest2"), "Test |Test2");
  assert.equal(del("Test|\n\nTest2"), "Test|Test2");
  assert.equal(del("Test \\|\n\nTest2"), "Test|Test2");
  assert.equal(del("One|\nTwo"), null);
  assert.equal(del("O|ne\n\nTwo"), null);
  assert.equal(del("One|\n\n"), null);
  assert.equal(del("|\n\nTwo"), null);
});
