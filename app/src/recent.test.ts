// Tests for File → Open recent's items. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { mnemonic, plainLabel } from "./commands.ts";
import { recentItems, recentLabel, type RecentProject } from "./recent.ts";

const project = (i: number): RecentProject => ({ name: `Book ${i}`, path: `C:/Books/Book ${i}.lethora`, exists: i !== 2 });

test("labels number the first ten entries 1–9, then 0; a & in the name stays text", () => {
  assert.equal(recentLabel(0, "Book"), "&1 Book");
  assert.equal(mnemonic(recentLabel(8, "Book")), "9");
  assert.equal(mnemonic(recentLabel(9, "Book")), "0");
  assert.equal(mnemonic(recentLabel(0, "Smith & Jones")), "1");
  assert.equal(plainLabel(recentLabel(0, "Smith & Jones")), "1 Smith & Jones");
});

test("the submenu lists the projects with their paths, then Clear recent", () => {
  const opened: string[] = [];
  let cleared = 0;
  const items = recentItems([project(1), project(2)], (path) => opened.push(path), () => cleared++);
  assert.deepEqual(
    items.map((c) => c && [c.id, c.label, c.detail]),
    [["recent-0", "&1 Book 1", "C:/Books/Book 1.lethora"], ["recent-1", "&2 Book 2", "C:/Books/Book 2.lethora"], null, ["recent-clear", "&Clear recent", undefined]],
  );
  // A missing project is listed too: opening it reports the error and drops it.
  items[1]!.run();
  items[3]!.run();
  assert.deepEqual(opened, ["C:/Books/Book 2.lethora"]);
  assert.equal(cleared, 1);
});

test("an empty list gives an empty submenu (Open recent greyed out); at most ten entries", () => {
  assert.deepEqual(recentItems([], () => {}, () => {}), []);
  const many = recentItems(Array.from({ length: 12 }, (_, i) => project(i)), () => {}, () => {});
  assert.equal(many.filter((c) => c?.id.startsWith("recent-") && c.id !== "recent-clear").length, 10);
});
