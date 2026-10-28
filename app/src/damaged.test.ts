// Tests for the wording around damaged projects. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { damagedQuestion, recoveredNotice, skippedDevice, skippedNotice } from "./damaged.ts";

const LOG = "main/6f1c0a9e5b2d4c7f8e3a1b0d9c8e7f6a.log";

test("the question names the file", () => {
  assert.ok(damagedQuestion(LOG).startsWith(`The last changes in ${LOG} can't be read.`));
});

test("the recovery notice says what was dropped and where the copy is", () => {
  assert.equal(
    recoveredNotice([{ file: LOG, dropped: 12_800, backup: "backup/damaged/main-6f1c.log" }]),
    `Opened at the last good state: the last 13 KB of changes in ${LOG} could not be read and were dropped. A copy of the damaged file is in backup/damaged/main-6f1c.log.`,
  );
  assert.ok(recoveredNotice([{ file: LOG, dropped: 40, backup: "b" }]).includes("the last 40 B of changes"));
  assert.equal(recoveredNotice([]), "");
});

test("skipped data is named by device file, once per file", () => {
  const main = "main: device 3b9e2f70c4a1d8e65f0b7a29c3d4e1f8: log at byte 2048: document model error: decode error";
  const review = "review: device a07c5e1d92b84f36e0d1c7b5a4f3e2d1: snapshot: document model error";
  assert.equal(skippedDevice(main), "main/3b9e2f70…");
  assert.equal(skippedDevice("something else"), "something else");
  assert.equal(skippedNotice([]), "");
  assert.equal(skippedNotice([main, main]), "Changes from another device (main/3b9e2f70…) could not be read and were left out. Nothing was changed.");
  assert.equal(skippedNotice([main, review]), "Changes from 2 other devices (main/3b9e2f70…, review/a07c5e1d…) could not be read and were left out. Nothing was changed.");
});
