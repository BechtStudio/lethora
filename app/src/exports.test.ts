import { test } from "node:test";
import assert from "node:assert/strict";
import { baseName, exportFilters, exportNotice, exportTarget, PDF_FILTER } from "./exports.ts";

const writers = [
  { id: "docx", name: "Word", extensions: ["docx"] },
  { id: "epub", name: "EPUB", extensions: ["epub"] },
  { id: "markdown", name: "Markdown", extensions: ["md", "markdown"] },
  { id: "text", name: "Plain text", extensions: ["txt", "text"] },
  { id: "html", name: "HTML", extensions: ["html", "htm"] },
];
const filters = exportFilters(writers);

test("the dialog lists the writers, PDF after EPUB", () => {
  assert.deepEqual(
    filters.map((f) => f.id),
    ["docx", "epub", "pdf", "markdown", "text", "html"],
  );
  assert.equal(filters[2], PDF_FILTER);
  // Without EPUB, PDF goes last.
  assert.deepEqual(
    exportFilters(writers.slice(2)).map((f) => f.id),
    ["markdown", "text", "html", "pdf"],
  );
});

test("the extension picks the format", () => {
  assert.deepEqual(exportTarget("C:\\Out\\Thesis.MD", filters), { path: "C:\\Out\\Thesis.MD", format: "markdown" });
  assert.deepEqual(exportTarget("/home/a/thesis.htm", filters), { path: "/home/a/thesis.htm", format: "html" });
  assert.deepEqual(exportTarget("/home/a/thesis.pdf", filters), { path: "/home/a/thesis.pdf", format: "pdf" });
  assert.deepEqual(exportTarget("/home/a/thesis.docx", filters), { path: "/home/a/thesis.docx", format: "docx" });
  // No extension: the first format's (Word).
  assert.deepEqual(exportTarget("/home/a.b/thesis", filters), { path: "/home/a.b/thesis.docx", format: "docx" });
  assert.equal(exportTarget("/home/a/thesis.docm", filters), null);
});

test("names and the notice", () => {
  assert.equal(baseName('A "Study": of/Pages?'), "A Study of Pages");
  assert.equal(baseName("  \u0001 "), "document");
  assert.equal(exportNotice("C:\\Out\\a.md", []), "Exported a.md.");
  const warning = { kind: "typst-math", count: 2, example: null, message: "Kept 2 formulas as Typst source." };
  assert.equal(exportNotice("/x/a.md", [warning]), "Exported a.md. Kept 2 formulas as Typst source.");
});
