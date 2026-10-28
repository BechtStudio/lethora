// Tests for the sources library helpers: search, key generation, BibLaTeX. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  asciiWord,
  type BibEntry,
  cleanDoi,
  entryDetail,
  escapeBib,
  escapeVerbatim,
  filterEntries,
  importSummary,
  makeKey,
  searchText,
  shortAuthors,
  toBibLatex,
} from "./bib.ts";

const e = (key: string, authors: string[], year: string | null, title: string | null): BibEntry => ({ key, entry_type: "book", authors, year, title });

test("short authors and detail", () => {
  assert.equal(shortAuthors([]), "");
  assert.equal(shortAuthors(["Woolf, Virginia"]), "Woolf");
  assert.equal(shortAuthors(["Hague, Douglas", "Christie, Rosemary"]), "Hague & Christie");
  assert.equal(shortAuthors(["A, x", "B, y", "C, z"]), "A et al.");
  assert.equal(shortAuthors(["Virginia Woolf"]), "Woolf");
  assert.equal(entryDetail(e("k", ["Woolf, Virginia"], "1927", "T")), "Woolf 1927");
  assert.equal(entryDetail(e("k", [], null, "T")), "");
});

test("search filters by key, title, authors and year (all terms)", () => {
  const entries = [
    e("woolf1927", ["Woolf, Virginia"], "1927", "To the Lighthouse"),
    e("levitt2013", ["Levitt, Theresa"], "2013", "A Short Bright Flash"),
    e("hague1975", ["Hague, Douglas"], "1975", "Lighthouses"),
  ];
  const index = entries.map(searchText);
  const keys = (q: string) => filterEntries(entries, index, q).map((x) => x.key);
  assert.deepEqual(keys(""), ["woolf1927", "levitt2013", "hague1975"]);
  assert.deepEqual(keys("LIGHTHOUSE"), ["woolf1927", "hague1975"]);
  assert.deepEqual(keys("light 1975"), ["hague1975"]);
  assert.deepEqual(keys("theresa"), ["levitt2013"]);
  assert.deepEqual(keys("levitt20"), ["levitt2013"]);
  assert.deepEqual(keys("nothing"), []);
});

test("search over 2000 entries is fast", () => {
  const entries = Array.from({ length: 2000 }, (_, i) => e(`key${i}`, [`Author${i}, First`], String(1900 + (i % 120)), `Title number ${i} about lighthouses`));
  const index = entries.map(searchText);
  const t = performance.now();
  for (let i = 0; i < 50; i++) filterEntries(entries, index, `author1 light ${i}`);
  assert.ok((performance.now() - t) / 50 < 5, "under 5 ms per search");
});

test("import summary", () => {
  assert.equal(importSummary({ added: ["a", "b"], skipped_duplicates: [] }), "2 added");
  assert.equal(importSummary({ added: ["a"], skipped_duplicates: ["w"] }), "1 added, 1 duplicate skipped (w)");
  assert.equal(importSummary({ added: [], skipped_duplicates: ["w", "x"] }), "Nothing new added, 2 duplicates skipped (w, x)");
});

test("keys: lastname + year + first title word, ASCII, de-duplicated", () => {
  assert.equal(asciiWord("Brontë-Smith"), "brontesmith");
  assert.equal(asciiWord("Straße"), "strasse");
  assert.equal(makeKey(["Woolf, Virginia"], "1927", "To the Lighthouse", []), "woolf1927to");
  assert.equal(makeKey(["Levitt, Theresa"], "2013", "A Short Bright Flash", []), "levitt2013short");
  assert.equal(makeKey(["Émile Zola"], "1885", "Germinal", []), "zola1885germinal");
  assert.equal(makeKey(["Zola, Émile"], "1885", "Germinal", ["zola1885germinal", "Zola1885Germinal2"]), "zola1885germinal3");
  assert.equal(makeKey([], "", "", []), "source");
  assert.equal(makeKey([], "", "", ["source"]), "source2");
  assert.equal(makeKey(["  ", "Bell, Q."], "1972", "Virginia Woolf: A Biography", []), "bell1972virginia");
});

test("BibLaTeX escaping keeps braces balanced and specials literal", () => {
  assert.equal(escapeBib("Fish & Chips: 100% {fresh} #1 a_b $5 ~x^2 C:\\path"), String.raw`Fish \& Chips: 100\% \{fresh\} \#1 a\_b \$5 \textasciitilde{}x\textasciicircum{}2 C:\textbackslash{}path`);
  assert.equal(escapeBib("  two\n lines  "), "two lines");
  assert.equal(escapeVerbatim(" https://x.org/a%20b?q={1} "), "https://x.org/a%20b?q=%7B1%7D");
  assert.equal(cleanDoi("https://doi.org/10.1000/xyz{1}"), "10.1000/xyz%7B1%7D");
  assert.equal(cleanDoi("doi: 10.1/abc"), "10.1/abc");
  // Every generated value has balanced braces.
  const nasty = "}{ \\{ {{ }";
  const v = escapeBib(nasty);
  let depth = 0;
  for (let i = 0; i < v.length; i++) {
    if (v[i] === "\\") {
      i++;
      if (/[a-z]/.test(v[i])) while (/[a-z]/.test(v[i + 1] ?? "")) i++;
      continue;
    }
    if (v[i] === "{") depth++;
    if (v[i] === "}") depth--;
    assert.ok(depth >= 0);
  }
  assert.equal(depth, 0);
});

test("BibLaTeX entries per type", () => {
  const text = toBibLatex({
    type: "article",
    key: "smith2020light",
    authors: ["Smith, Jane", "", "Ford and Sons, Ltd."],
    title: "Light & Shade",
    year: "2020",
    container: "Journal of {Optics}",
    doi: "https://doi.org/10.1/x",
    url: "https://example.org/a b",
  });
  assert.equal(
    text,
    [
      "@article{smith2020light,",
      "  author = {Smith, Jane and {Ford and Sons, Ltd.}},",
      String.raw`  title = {Light \& Shade},`,
      String.raw`  journal = {Journal of \{Optics\}},`,
      "  year = {2020},",
      "  doi = {10.1/x},",
      "  url = {https://example.org/ab},",
      "}",
      "",
    ].join("\n"),
  );
  const book = toBibLatex({ type: "book", key: "k", authors: [], title: "T", year: "", container: "Hogarth" });
  assert.match(book, /^@book\{k,\n {2}title = \{T\},\n {2}publisher = \{Hogarth\},\n\}\n$/);
  const proc = toBibLatex({ type: "inproceedings", key: "k", authors: [], title: "T", year: "", container: "Proc. X" });
  assert.match(proc, /booktitle = \{Proc\. X\}/);
  // online/misc have no container field, even if one was typed earlier.
  assert.doesNotMatch(toBibLatex({ type: "online", key: "k", authors: [], title: "T", year: "", container: "J" }), /journal|publisher|booktitle/);
  assert.throws(() => toBibLatex({ type: "misc", key: "bad key", authors: [], title: "T", year: "" }));
  assert.throws(() => toBibLatex({ type: "misc", key: "a,b", authors: [], title: "T", year: "" }));
});
