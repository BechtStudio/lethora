// Tests for compile status texts, diagnostic locations and undo messages. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  type CompileStatus,
  compileStatusText,
  contentRange,
  customRange,
  type Diagnostic,
  diagnosticsSummary,
  diagnosticTarget,
  locationLabel,
  undoMessage,
  utf8ToUtf16,
} from "./status.ts";

const status = (over: Partial<CompileStatus>): CompileStatus => ({
  state: "done",
  version: 7,
  pages_version: 7,
  pages: 12,
  page_sizes: [],
  diagnostics: [],
  duration_ms: 240,
  ...over,
});

const warning: Diagnostic = { severity: "warning", message: "w", hints: [], location: { in: "unknown" } };
const error: Diagnostic = { severity: "error", message: "e", hints: [], location: { in: "preamble" } };

test("status line: typesetting, pages and time, warnings, errors, timeout, crash", () => {
  assert.deepEqual(compileStatusText(status({ state: "compiling" })), { text: "Typesetting…", tone: "busy" });
  assert.deepEqual(compileStatusText(status({})), { text: "12 pages · 240 ms", tone: "ok" });
  assert.deepEqual(compileStatusText(status({ pages: 1, duration_ms: 1520 })), { text: "1 page · 1,520 ms", tone: "ok" });
  assert.deepEqual(compileStatusText(status({ diagnostics: [warning, warning] })), { text: "12 pages · 240 ms · 2 warnings", tone: "warn" });
  assert.deepEqual(compileStatusText(status({ state: "failed", pages_version: 5, diagnostics: [error, warning] })), {
    text: "1 error · 1 warning · showing the last good version",
    tone: "error",
  });
  assert.deepEqual(compileStatusText(status({ state: "failed", pages_version: null, pages: 0, diagnostics: [error, error] })), { text: "2 errors", tone: "error" });
  assert.equal(compileStatusText(status({ state: "timeout", message: null })).text, "Typesetting took too long and was stopped.");
  const oom = "Typesetting ran out of memory (the limit is 4096 MB).";
  assert.deepEqual(compileStatusText(status({ state: "crashed", message: oom })), { text: oom, tone: "error" });
  assert.equal(diagnosticsSummary([]), "");
});

test("content ranges: UTF-16 offsets clamped to the current text", () => {
  assert.deepEqual(contentRange({ start_utf16: 3, end_utf16: 9 }, 100), { from: 3, to: 9 });
  assert.deepEqual(contentRange({ start_utf16: 90, end_utf16: 120 }, 100), { from: 90, to: 100 });
  assert.deepEqual(contentRange({ start_utf16: 150, end_utf16: 160 }, 100), { from: 100, to: 100 });
  assert.deepEqual(contentRange({ start_utf16: 9, end_utf16: 3 }, 100), { from: 9, to: 9 });
});

test("expert code: UTF-8 byte offsets to UTF-16", () => {
  const text = "é😀#oops x";
  // é = 2 bytes, 😀 = 4 bytes (2 UTF-16 units).
  assert.equal(utf8ToUtf16(text, 0), 0);
  assert.equal(utf8ToUtf16(text, 2), 1);
  assert.equal(utf8ToUtf16(text, 6), 3);
  assert.equal(utf8ToUtf16(text, 4), 3); // inside the emoji: the next boundary
  assert.equal(utf8ToUtf16(text, 99), text.length);
  assert.deepEqual(customRange({ start: 6, end: 11 }, text), { from: 3, to: 8 });
  assert.equal(text.slice(3, 8), "#oops");
});

test("diagnostic targets: content (maybe outdated), custom, others plain", () => {
  const ctx = { compiledVersion: 4, docVersion: 4, docLength: 50, customText: "#oops" };
  const content: Diagnostic = { ...error, location: { in: "content", start: 10, end: 14, start_utf16: 8, end_utf16: 12 } };
  assert.deepEqual(diagnosticTarget(content, ctx), { kind: "content", from: 8, to: 12, outdated: false });
  assert.deepEqual(diagnosticTarget(content, { ...ctx, docVersion: 6, docLength: 10 }), { kind: "content", from: 8, to: 10, outdated: true });
  const custom: Diagnostic = { ...error, location: { in: "custom", start: 1, end: 5 } };
  assert.deepEqual(diagnosticTarget(custom, ctx), { kind: "custom", from: 1, to: 5 });
  for (const location of [{ in: "preamble" }, { in: "file", path: "references.bib" }, { in: "unknown" }] as const) {
    assert.deepEqual(diagnosticTarget({ ...error, location }, ctx), { kind: "none" });
  }
  assert.equal(locationLabel(content.location, (u) => u + 1), "Line 9");
  assert.equal(locationLabel({ in: "file", path: "assets/a.png" }), "assets/a.png");
  assert.equal(locationLabel({ in: "preamble" }), "Layout settings");
});

test("undo messages: layout steps are named everywhere, text steps in the publisher view", () => {
  const layout = { changes: [], len16: 10 };
  const text = { changes: [{ from: 0, to: 1, insert: "" }], len16: 9 };
  assert.deepEqual(undoMessage("undo", layout, false), { message: "Undid a layout change", layout: true });
  assert.deepEqual(undoMessage("redo", layout, true), { message: "Redid a layout change", layout: true });
  assert.deepEqual(undoMessage("undo", text, false), { message: null, layout: false });
  assert.deepEqual(undoMessage("undo", text, true), { message: "Undid a text change", layout: false });
  assert.deepEqual(undoMessage("redo", null, false), { message: null, layout: false });
  assert.deepEqual(undoMessage("redo", null, true), { message: "Nothing to redo", layout: false });
  // Asset steps (an import, or a rename that also changed the text) are named everywhere.
  assert.deepEqual(undoMessage("undo", { ...layout, assets: true }, false), { message: "Undid an asset change", layout: true });
  assert.deepEqual(undoMessage("redo", { ...text, assets: true }, false), { message: "Redid an asset change", layout: true });
});

test("override locations: labelled by the exception, lead to its row", () => {
  const loc = { in: "override", id: "s1" } as const;
  assert.equal(locationLabel(loc, undefined, (id) => (id === "s1" ? "Space before 18pt" : null)), "Exception: Space before 18pt");
  assert.equal(locationLabel(loc), "Exception");
  assert.equal(locationLabel({ in: "override", id: "gone" }, undefined, () => null), "Exception");
  const ctx = { compiledVersion: 4, docVersion: 4, docLength: 50, customText: "" };
  assert.deepEqual(diagnosticTarget({ ...warning, location: loc }, ctx), { kind: "override", id: "s1" });
});
