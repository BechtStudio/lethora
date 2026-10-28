// Tests for the update dialog's wording. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { availableHeading, notesText, progressText, releasedLine, upToDate, type UpdateInfo } from "./updates.ts";

const info: UpdateInfo = { version: "0.2.0", notes: "\n- Faster export.\n- <b>Fixed</b> a crash.\n", date: "2026-11-02" };

test("a found update names its version and date; its notes are plain text", () => {
  assert.equal(availableHeading(info), "Lethora 0.2.0 is available");
  assert.equal(releasedLine(info), "Released 2026-11-02.");
  assert.equal(releasedLine({ ...info, date: null }), "");
  assert.equal(notesText(info), "- Faster export.\n- <b>Fixed</b> a crash.");
  assert.equal(notesText({ ...info, notes: "  \n" }), "No release notes.");
  assert.equal(notesText({ ...info, notes: "### Added\n\n- C# export.\n" }), "Added\n\n- C# export.");
});

test("up to date names the running version", () => {
  assert.equal(upToDate("0.1.0"), "Lethora is up to date (version 0.1.0).");
});

test("the download shows a percentage, else the size so far, then installing", () => {
  assert.equal(progressText({ downloaded: 0, total: 2000 }), "Downloading… 0%");
  assert.equal(progressText({ downloaded: 999, total: 2000 }), "Downloading… 49%");
  assert.equal(progressText({ downloaded: 2000, total: 2000 }), "Installing…");
  assert.equal(progressText({ downloaded: 3 * 1024 * 1024, total: null }), "Downloading… 3.0 MB");
  assert.equal(progressText({ downloaded: 0, total: 0 }), "Downloading… 0 B");
});
