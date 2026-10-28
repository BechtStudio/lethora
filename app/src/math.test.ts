// Tests for math span finding, the render cache and palette inserts. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { ensureSyntaxTree } from "@codemirror/language";
import { EditorSelection, EditorState } from "@codemirror/state";
import { typstLanguage } from "./typst.ts";
import { findMathSpans, isDisplay, MathCache, mathKey, toHex, type MathReply, type MathRequest } from "./math.ts";
import { labelsFor } from "./refs.ts";
import { equationLabel, findPlaceholder, insertMath, insertSnippet, PALETTE, type Snippet } from "./palette.ts";

function stateOf(doc: string, anchor = 0, head = anchor): EditorState {
  const state = EditorState.create({ doc, selection: EditorSelection.single(anchor, head), extensions: typstLanguage });
  ensureSyntaxTree(state, state.doc.length, 5000);
  return state;
}

const spans = (doc: string) =>
  findMathSpans(stateOf(doc), 0, doc.length).map((s) => `${s.display ? "D" : "I"}:${doc.slice(s.from, s.to)}`);

// ---- Span finding ----

test("inline vs display (whitespace at both ends)", () => {
  assert.deepEqual(spans("a $x^2$ b $ y $ c $ z$ d $w $"), ["I:$x^2$", "D:$ y $", "I:$ z$", "I:$w $"]);
  assert.equal(isDisplay(" "), false);
  assert.equal(isDisplay("\n a\n"), true);
});

test("escapes, strings and adjacent formulas", () => {
  assert.deepEqual(spans(String.raw`\$5 and $a \$ b$ and \$6`), [String.raw`I:$a \$ b$`]);
  assert.deepEqual(spans('$"$" x$'), ['I:$"$" x$']);
  assert.deepEqual(spans("$a$$b$"), ["I:$a$", "I:$b$"]);
});

test("formulas across lines, in markup and code", () => {
  const doc = "text\n$ sum_(i=1)^n i\n  = n $ more\n#f($x$)\n`$no$` /* $no$ */";
  assert.deepEqual(spans(doc), ["D:$ sum_(i=1)^n i\n  = n $", "I:$x$"]);
  const found = findMathSpans(stateOf(doc), 0, doc.length)[0];
  assert.equal(found.source, " sum_(i=1)^n i\n  = n ");
});

test("unclosed formula is not a span; range limits", () => {
  assert.deepEqual(spans("a $x$ b $y"), ["I:$x$"]);
  const doc = "$a$ " + "x".repeat(100) + " $b$";
  const state = stateOf(doc);
  assert.deepEqual(findMathSpans(state, 50, 60), []);
  assert.deepEqual(findMathSpans(state, 1, 2).map((s) => s.source), ["a"]);
  assert.deepEqual(findMathSpans(state, doc.length, doc.length).map((s) => s.source), ["b"]);
});

// ---- Render cache ----

const req = (source: string): MathRequest => ({ source, display: false, sizePt: 12, color: "#000000" });

/** A renderer whose calls resolve when the test says so. */
function fakeRenderer() {
  const calls: Array<{ request: MathRequest; resolve: (r: MathReply) => void }> = [];
  const render = (request: MathRequest) => new Promise<MathReply>((resolve) => calls.push({ request, resolve }));
  const ok = (i: number) => calls[i].resolve({ ok: { svg: `<svg>${calls[i].request.source}</svg>`, width: 10, height: 8, baseline: 6 } });
  return { calls, render, ok };
}

const tick = () => new Promise((r) => setTimeout(r, 0));

test("cache dedupes requests and stores results as data URLs", async () => {
  const fake = fakeRenderer();
  let results = 0;
  const cache = new MathCache(fake.render, () => results++);
  cache.want([req("a"), req("a")]);
  cache.want([req("a")]);
  assert.equal(fake.calls.length, 1);
  assert.equal(cache.get(req("a")), undefined);
  fake.ok(0);
  await tick();
  assert.equal(results, 1);
  const r = cache.get(req("a"));
  assert.ok(r && "url" in r);
  assert.equal(r.url, "data:image/svg+xml;charset=utf-8,%3Csvg%3Ea%3C%2Fsvg%3E");
  assert.equal(r.baseline, 6);
  cache.want([req("a")]);
  assert.equal(fake.calls.length, 1);
});

test("cache keys include display, size and colour", () => {
  assert.notEqual(mathKey(req("a")), mathKey({ ...req("a"), display: true }));
  assert.notEqual(mathKey(req("a")), mathKey({ ...req("a"), color: "#ffffff" }));
  assert.notEqual(mathKey(req("a")), mathKey({ ...req("a"), sizePt: 13 }));
});

test("cache caps concurrency and drops queued requests no longer wanted", async () => {
  const fake = fakeRenderer();
  const cache = new MathCache(fake.render, () => {}, { concurrency: 4 });
  cache.want(Array.from({ length: 10 }, (_, i) => req(`f${i}`)));
  assert.equal(fake.calls.length, 4);
  assert.deepEqual(cache.pending, { running: 4, queued: 6 });
  // Scrolled on: only two new formulas are visible.
  cache.want([req("g1"), req("g2")]);
  assert.deepEqual(cache.pending, { running: 4, queued: 2 });
  fake.ok(0);
  await tick();
  assert.equal(fake.calls.length, 5);
  assert.equal(fake.calls[4].request.source, "g1");
  for (let i = 1; i < 5; i++) fake.ok(i);
  await tick();
  assert.deepEqual(
    fake.calls.map((c) => c.request.source),
    ["f0", "f1", "f2", "f3", "g1", "g2"],
  );
});

test("cache is bounded (least recently used dropped)", async () => {
  const fake = fakeRenderer();
  const cache = new MathCache(fake.render, () => {}, { capacity: 2 });
  for (const s of ["a", "b"]) {
    cache.want([req(s)]);
    fake.ok(fake.calls.length - 1);
    await tick();
  }
  cache.get(req("a")); // a is now the most recent
  cache.want([req("c")]);
  fake.ok(2);
  await tick();
  assert.equal(cache.size, 2);
  assert.ok(cache.get(req("a")));
  assert.equal(cache.get(req("b")), undefined);
  assert.ok(cache.get(req("c")));
});

test("cache stores render errors and failed calls as errors", async () => {
  const cache = new MathCache(
    async (r) => {
      if (r.source === "boom") throw "IPC down";
      return { error: "unclosed delimiter" };
    },
    () => {},
  );
  cache.want([req("x"), req("boom")]);
  await tick();
  await tick();
  assert.deepEqual(cache.get(req("x")), { error: "unclosed delimiter" });
  assert.deepEqual(cache.get(req("boom")), { error: "IPC down" });
});

test("colours to #rrggbb", () => {
  assert.equal(toHex(" #2C2A26 "), "#2c2a26");
  assert.equal(toHex("#abc"), "#aabbcc");
  assert.equal(toHex("rgb(220, 214, 203)"), "#dcd6cb");
  assert.equal(toHex("rgba(1, 2, 3, 0.5)"), "#010203");
  assert.equal(toHex("red"), null);
});

// ---- Palette inserts ----

const apply = (state: EditorState, spec: ReturnType<typeof insertSnippet>) => {
  const s = state.update(spec).state;
  const { from, to } = s.selection.main;
  return `${s.doc.sliceString(0, from)}[${s.doc.sliceString(from, to)}]${s.doc.sliceString(to)}`;
};

const put = (state: EditorState, snippet: Snippet) => apply(state, insertSnippet(state, snippet));

const frac: Snippet = { label: "a⁄b", insert: "frac(a, b)", select: "a" };
const alpha: Snippet = { label: "α", insert: "alpha" };

test("snippet outside math is wrapped in $…$, placeholder selected", () => {
  assert.equal(put(stateOf("so  here", 3), frac), "so $frac([a], b)$ here");
  assert.equal(put(stateOf("x", 1), alpha), "x$alpha[]$");
});

test("snippet inside math is inserted as is, spaced from identifiers", () => {
  assert.equal(put(stateOf("$x + $", 5), frac), "$x + frac([a], b)$");
  assert.equal(put(stateOf("$x$", 2), alpha), "$x alpha[]$");
  assert.equal(put(stateOf("$xy$", 2), alpha), "$x alpha[] y$");
  assert.equal(put(stateOf("$ a\n  b $", 6), alpha), "$ a\n  alpha[] b $");
  // Right after the opening `$` is inside; after the closing one is not.
  assert.equal(put(stateOf("$x$", 1), alpha), "$alpha[] x$");
  assert.equal(put(stateOf("$x$", 3), alpha), "$x$$alpha[]$");
});

test("placeholders are whole words", () => {
  assert.equal(findPlaceholder("frac(a, b)", "a"), 5);
  assert.equal(findPlaceholder("x^n", "x"), 0);
  assert.equal(findPlaceholder("max", "a"), -1);
});

test("a selected text takes the placeholder's place", () => {
  assert.equal(put(stateOf("$1 + 2$", 1, 6), frac), "$frac(1 + 2[], b)$");
});

test("Ctrl+M / Ctrl+Shift+M insert inline and display math", () => {
  assert.equal(apply(stateOf("ab", 1), insertMath(stateOf("ab", 1), false)), "a$[]$b");
  const s = stateOf("ab", 1);
  assert.equal(apply(s, insertMath(s, true)), "a$ [] $b");
  const sel = stateOf("say x^2 now", 4, 7);
  assert.equal(apply(sel, insertMath(sel, false)), "say $[x^2]$ now");
  const inside = stateOf("a $x^2$ b", 4);
  assert.equal(apply(inside, insertMath(inside, false)), "a $x[]^2$ b");
});

test("display formulas get the next free equation label", () => {
  const taken = new Set(["eq:1", "eq:3", "fig:eq"]);
  assert.equal(equationLabel(() => false), "eq:1");
  assert.equal(equationLabel((n) => taken.has(n)), "eq:2");
  taken.add("eq:2");
  assert.equal(equationLabel((n) => taken.has(n)), "eq:4");
  const labels = labelsFor(stateOf("$ a $ <eq:1>\n\n#figure[x] <eq:2>\n\n$ b $ <eq:4>").doc).byName;
  assert.equal(equationLabel((n) => labels.has(n)), "eq:3");
  const s = stateOf("ab", 1);
  assert.equal(apply(s, insertMath(s, true, "eq:2")), "a$ [] $ <eq:2>b");
  const sel = stateOf("say x^2 now", 4, 7);
  assert.equal(apply(sel, insertMath(sel, true, "eq:1")), "say $ [x^2] $ <eq:1> now");
  // Inline formulas never, and nothing inside a formula.
  assert.equal(apply(sel, insertMath(sel, false, "eq:1")), "say $[x^2]$ now");
  const inside = stateOf("a $ x^2 $ b", 5);
  assert.deepEqual(insertMath(inside, true, "eq:1"), {});
});

test("palette groups and snippets are well formed", () => {
  assert.deepEqual(
    PALETTE.map((g) => g.name),
    ["Greek", "Operators", "Relations", "Arrows", "Structures"],
  );
  for (const g of PALETTE) {
    for (const s of g.items) {
      assert.ok(s.label && s.insert, `${g.name}: ${s.label}`);
      if (s.select) assert.ok(findPlaceholder(s.insert, s.select) >= 0, `${s.insert} has ${s.select}`);
    }
  }
});
