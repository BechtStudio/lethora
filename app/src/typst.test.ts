// Tests for the Typst markup tokenizer (stream parser run over strings, no DOM). Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { tokenize } from "./typst.ts";

/**
 * Tokens as "tags:text" (tags sorted, adjacent equal tokens on one line
 * merged, plain text left out) for readable assertions.
 */
function show(text: string): string[] {
  const out: string[] = [];
  let last = { tags: "", line: -1, end: -1 };
  tokenize(text, (token, value, line, from) => {
    const tags = token.split(" ").sort().join(" ");
    if (tags === last.tags && line === last.line && from === last.end) out[out.length - 1] += value;
    else out.push(`${tags}:${value}`);
    last = { tags, line, end: from + value.length };
  });
  return out;
}

// ---- Ported single-line cases ----

test("headings: level by marker count, marker dimmed", () => {
  assert.deepEqual(show("= Title"), ["h1 mk:="]);
  assert.deepEqual(show("=== Third"), ["h3 mk:==="]);
  assert.deepEqual(show("====== Six"), ["h6 mk:======"]);
  assert.deepEqual(show("======== Deep"), ["h6 mk:========"]);
});

test("headings need a space and the line start", () => {
  assert.deepEqual(show("=Title"), []);
  assert.deepEqual(show("a = b"), []);
  assert.deepEqual(show(" = indented"), []);
});

test("heading content gets inline marks", () => {
  assert.deepEqual(show("== A *bold* word"), ["h2 mk:==", "mk strong:*", "strong:bold", "mk strong:*"]);
});

test("strong and emphasis with dimmed markers", () => {
  assert.deepEqual(show("a *b* c"), ["mk strong:*", "strong:b", "mk strong:*"]);
  assert.deepEqual(show("a _b c_ d"), ["emph mk:_", "emph:b c", "emph mk:_"]);
});

test("nesting: emphasis inside strong", () => {
  assert.deepEqual(show("*a _b_ c*"), [
    "mk strong:*",
    "strong:a ",
    "emph mk strong:_",
    "emph strong:b",
    "emph mk strong:_",
    "strong: c",
    "mk strong:*",
  ]);
});

test("unclosed strong/emphasis produce nothing (line-bounded)", () => {
  assert.deepEqual(show("a *b c"), []);
  assert.deepEqual(show("a _b"), []);
  assert.deepEqual(show("a <b"), []);
  assert.deepEqual(show("*a _b* c"), ["mk strong:*", "strong:a _b", "mk strong:*"]);
  assert.deepEqual(show("a *b\nc* d"), []);
});

test("unclosed raw and math continue (as in Typst)", () => {
  assert.deepEqual(show("a `b"), ["raw:`b"]);
  assert.deepEqual(show("a $b"), ["mathOpen:$", "math:b"]);
});

test("markers inside words are text", () => {
  assert.deepEqual(show("snake_case_name"), []);
  assert.deepEqual(show("2*3*4"), []);
});

test("backslash escapes never start a construct", () => {
  assert.deepEqual(show(String.raw`\*not bold\*`), []);
  assert.deepEqual(show(String.raw`\_no\_`), []);
  assert.deepEqual(show(String.raw`costs \$5 and \$6`), []);
  assert.deepEqual(show(String.raw`mail \@home`), []);
  assert.deepEqual(show(String.raw`not \#code`), []);
  assert.deepEqual(show(String.raw`*a \* b*`), ["mk strong:*", String.raw`strong:a \* b`, "mk strong:*"]);
});

test("a forced line break is markup, an escape is not", () => {
  assert.deepEqual(show("one \\\ntwo"), ["mk:\\"]);
  assert.deepEqual(show(String.raw`a\ b`), ["mk:\\"]);
  assert.deepEqual(show(String.raw`*a \ b*`), ["mk strong:*", "strong:a ", "mk strong:\\", "strong: b", "mk strong:*"]);
  assert.deepEqual(show(String.raw`not \#code`), []);
});

test("raw is monospace and hides markup inside", () => {
  assert.deepEqual(show("use `*x*` here"), ["raw:`*x*`"]);
  assert.deepEqual(show("```rust let a = 1;```"), ["raw:```rust let a = 1;```"]);
  assert.deepEqual(show("empty `` raw"), ["raw:``"]);
});

test("inline math", () => {
  assert.deepEqual(show("so $a^2 + b^2$ holds"), ["mathOpen:$", "math:a^2 + b^2", "mathClose:$"]);
  assert.deepEqual(show(String.raw`$a \$ b$`), ["mathOpen:$", String.raw`math:a \$ b`, "mathClose:$"]);
  assert.deepEqual(show("*$x$*"), ["mk strong:*", "mathOpen strong:$", "math strong:x", "mathClose strong:$", "mk strong:*"]);
  assert.deepEqual(show('$a "$" b$'), ["mathOpen:$", 'math:a "$" b', "mathClose:$"]);
  assert.deepEqual(show("$a$$b$"), ["mathOpen:$", "math:a", "mathClose:$", "mathOpen:$", "math:b", "mathClose:$"]);
});

test("citations", () => {
  assert.deepEqual(show("as @knuth1984 shows"), ["cite:@knuth1984"]);
  assert.deepEqual(show("see @smith:intro."), ["cite:@smith:intro"]);
  assert.deepEqual(show("a@b.com"), []);
  assert.deepEqual(show("@ alone"), []);
});

test("citation supplement is markup", () => {
  assert.deepEqual(show("@knuth[p. *7*] x"), ["cite:@knuth", "mk:[", "mk strong:*", "strong:7", "mk strong:*", "mk:]"]);
  assert.deepEqual(show("@knuth [x]"), ["cite:@knuth"]);
});

test("labels", () => {
  assert.deepEqual(show("= Intro <intro>"), ["h1 mk:=", "label:<intro>"]);
  assert.deepEqual(show("a < b > c"), []);
});

test("comments run to the end of the line, URLs are not comments", () => {
  assert.deepEqual(show("text // note *x*"), ["comment:// note *x*"]);
  assert.deepEqual(show("see https://example.org/x"), []);
  assert.deepEqual(show("*a // b*"), ["comment:// b*"]);
});

test("list, enum and term markers", () => {
  assert.deepEqual(show("- item"), ["mk:-"]);
  assert.deepEqual(show("  + nested _x_"), ["mk:+", "emph mk:_", "emph:x", "emph mk:_"]);
  assert.deepEqual(show("12. twelfth"), ["mk:12."]);
  assert.deepEqual(show("-not a list"), []);
  assert.deepEqual(show("a - b"), []);
  assert.deepEqual(show("1.5 litres"), []);
  assert.deepEqual(show("/ Term: description"), ["mk:/", "strong: Term", "mk::"]);
});

test("offsets are UTF-16 code units", () => {
  const line = "😀 *ab* 👩‍👩‍👧 @x";
  const tokens: Array<[string, number, number]> = [];
  tokenize(line, (token, value, _line, from) => tokens.push([token, from, from + value.length]));
  assert.deepEqual(tokens.filter(([t]) => t !== "strong" && t !== "cite"), [
    ["mk strong", 3, 4],
    ["mk strong", 6, 7],
  ]);
  assert.deepEqual(tokens.find(([t]) => t === "cite"), ["cite", 17, 19]);
  assert.equal(line.slice(17, 19), "@x");
  // Emphasis between an emoji and a letter: the emoji is not a letter.
  assert.deepEqual(show("😀_a_"), ["emph mk:_", "emph:a", "emph mk:_"]);
});

test("escape before an astral character skips the whole pair", () => {
  assert.deepEqual(show("\\😀*a*"), ["mk strong:*", "strong:a", "mk strong:*"]);
});

// ---- Multi-line constructs ----

test("display math across lines", () => {
  assert.deepEqual(show("$ a +\n  b $ after *x*"), [
    "mathOpen:$",
    "math: a +",
    "math:  b ",
    "mathClose:$",
    "mk strong:*",
    "strong:x",
    "mk strong:*",
  ]);
  assert.deepEqual(show("$ a\n\nb $"), ["mathOpen:$", "math: a", "math:b ", "mathClose:$"]);
});

test("escaped dollar and comments inside math", () => {
  assert.deepEqual(show("$a \\$\n// $ not closed\nb$"), [
    "mathOpen:$",
    "math:a \\$",
    "comment math:// $ not closed",
    "math:b",
    "mathClose:$",
  ]);
});

test("raw block with language across lines", () => {
  assert.deepEqual(show("```python\nx = *1* # $\n```\n*b*"), [
    "raw:```python",
    "raw:x = *1* # $",
    "raw:```",
    "mk strong:*",
    "strong:b",
    "mk strong:*",
  ]);
  // A shorter run inside a longer fence is content.
  assert.deepEqual(show("````\na ``` b\n````"), ["raw:````", "raw:a ``` b", "raw:````"]);
});

test("nested block comments", () => {
  assert.deepEqual(show("a /* x /* y */\nz */ *b*"), ["comment:/* x /* y */", "comment:z */", "mk strong:*", "strong:b", "mk strong:*"]);
});

test("code expressions", () => {
  assert.deepEqual(show("see #name here"), ["code:#name"]);
  assert.deepEqual(show("#a.b.c."), ["code:#a.b.c"]);
  assert.deepEqual(show("#f(1, (2), \"a)\") x"), ['code:#f(1, (2), "a)")']);
  assert.deepEqual(show("C# and issue#5 and # alone"), []);
});

test("code with content blocks returns to markup", () => {
  assert.deepEqual(show("#emph[very *bold*] text"), ["code:#emph[", "emph:very ", "emph mk strong:*", "emph strong:bold", "emph mk strong:*", "code:]"]);
  assert.deepEqual(show("#f(x)[a [b] c][d] e"), ["code:#f(x)[", "code:][", "code:]"]);
  assert.deepEqual(show("#box[\n= Inside\n$x$\n]\n_after_"), [
    "code:#box[",
    "h1 mk:=",
    "mathOpen:$",
    "math:x",
    "mathClose:$",
    "code:]",
    "emph mk:_",
    "emph:after",
    "emph mk:_",
  ]);
});

test("code blocks span lines with balanced brackets", () => {
  assert.deepEqual(show("#{\n  let x = (1,\n    2)\n  [*y*]\n}\nz"), [
    "code:#{",
    "code:  let x = (1,",
    "code:    2)",
    "code:  [",
    "mk strong:*",
    "strong:y",
    "mk strong:*",
    "code:]",
    "code:}",
  ]);
});

test("let/set/show statements run to the end of the line", () => {
  assert.deepEqual(show("#set text(size: 12pt) // c\n*a*"), ["code:#set text(size: 12pt) ", "comment:// c", "mk strong:*", "strong:a", "mk strong:*"]);
  assert.deepEqual(show("#let f(x) = {\n  x\n}\nplain"), ["code:#let f(x) = {", "code:  x", "code:}"]);
  assert.deepEqual(show('#show heading: set text(fill: red)\n#letter'), ["code:#show heading: set text(fill: red)", "code:#letter"]);
});

test("math and raw inside code", () => {
  assert.deepEqual(show("#f($x$, `y`)"), ["code:#f(", "mathOpen:$", "math:x", "mathClose:$", "code:, ", "raw:`y`", "code:)"]);
});

test("blank line ends a dangling expression, not math", () => {
  assert.deepEqual(show("#x\n\n*a*"), ["code:#x", "mk strong:*", "strong:a", "mk strong:*"]);
});

test("formatting calls style their content on one line", () => {
  assert.deepEqual(show("a #underline[b] c"), ["code:#underline[", "underline:b", "code:]"]);
  assert.deepEqual(show("#strike[x]#super[2]#sub[i]"), ["code:#strike[", "strike:x", "code:]#super[", "super:2", "code:]#sub[", "sub:i", "code:]"]);
  assert.deepEqual(show("#strong[a [b] c]"), ["code:#strong[", "strong:a [b] c", "code:]"]);
  // Inside strong, nested calls, after arguments.
  assert.deepEqual(show("*a #underline[b] c*"), ["mk strong:*", "strong:a ", "code:#underline[", "strong underline:b", "code:]", "strong: c", "mk strong:*"]);
  assert.deepEqual(show("#underline[#sub[x]]"), ["code:#underline[#sub[", "sub underline:x", "code:]]"]);
  assert.deepEqual(show("#underline(offset: 2pt)[x]"), ["code:#underline(offset: 2pt)[", "underline:x", "code:]"]);
  // Not across lines, not other calls.
  assert.deepEqual(show("#underline[a\nb]"), ["code:#underline[", "code:]"]);
  assert.deepEqual(show("#box[a] #underlined[b]"), ["code:#box[", "code:]", "code:#underlined[", "code:]"]);
  // A character style keeps the styles around it and adds none.
  assert.deepEqual(show('*a #styled("x")[b]*'), ["mk strong:*", "strong:a ", 'code:#styled("x")[', "strong:b", "code:]", "mk strong:*"]);
  assert.deepEqual(show('#styled("x")[#sub[i]]'), ['code:#styled("x")[#sub[', "sub:i", "code:]]"]);
});
