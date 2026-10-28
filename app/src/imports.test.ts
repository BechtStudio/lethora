import { test } from "node:test";
import assert from "node:assert/strict";
import { Text } from "@codemirror/state";
import { fileName, fileStem, importNotice, importPos, importText, type ImportReply } from "./imports.ts";

const doc = (s: string) => Text.of(s.split("\n"));

test("imported blocks stand apart from the text around them", () => {
  assert.equal(importText(doc(""), 0, "= A\n"), "= A\n");
  assert.equal(importText(doc("Before."), 7, "B\n"), "\n\nB\n");
  assert.equal(importText(doc("Before after"), 6, "B\n"), "\n\nB\n\n");
  assert.equal(importText(doc("One\n\nTwo"), 4, "B\n"), "\nB\n");
  assert.equal(importText(doc("One\n\n\n\nTwo"), 5, "B\n"), "B");
  assert.equal(importText(doc("One\n\nTwo"), 0, "B\n\n"), "B\n\n");
  assert.equal(importText(doc("x"), 0, "\n"), "");
});

test("imports go after the cursor's paragraph, never inside a word or label", () => {
  const d = doc("= Title <ch-one>\n\nOne line\nsame para.\n\nNext");
  assert.equal(importPos(d, 12), 16, "inside the label: end of the heading");
  assert.equal(importPos(d, 21), 37, "mid paragraph: its last line's end");
  assert.equal(importPos(d, 18), 18, "start of a paragraph: there");
  assert.equal(importPos(d, 17), 17, "a blank line: there");
  assert.equal(importPos(d, 0), 0);
  assert.equal(importPos(d, d.length), d.length);
});

test("file names and the notice", () => {
  assert.equal(fileName("C:\\Docs\\Field Notes.md"), "Field Notes.md");
  assert.equal(fileStem("/home/a/notes.tar.md"), "notes.tar");
  assert.equal(fileStem("README"), "README");
  const reply: ImportReply = {
    format: "Markdown",
    text: "x\n",
    assets: [],
    warnings: [{ kind: "html", count: 2, example: "<div>", message: "Dropped 2 pieces of HTML (e.g. <div>)." }],
    title: null,
    author: null,
  };
  assert.equal(importNotice("a.md", reply), "Imported a.md. Dropped 2 pieces of HTML (e.g. <div>).");
  assert.equal(importNotice("a.md", { ...reply, warnings: [] }, true), "Suggested the content of a.md.");
  const image = { kind: "image-left-out", count: 1, example: "x.png", message: "Left out 1 image (e.g. x.png): a type or size that can't be used here." };
  assert.equal(
    importNotice("a.md", { ...reply, warnings: [image] }, true),
    "Suggested the content of a.md (images can't be suggested). Left out 1 image (e.g. x.png): a type or size that can't be used here.",
  );
  assert.equal(importNotice("a.md", { ...reply, text: "" }), "Nothing to import in a.md.");
});
