// Tests for the asset explorer's tree, contents, filters, sorting, keyboard
// moves, usages and rename targets. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  type AssetItem,
  type AssetList,
  clampHeight,
  entryKey,
  type Entry,
  filterEntries,
  findFolder,
  findUsages,
  folderEntries,
  folderTree,
  folderUsed,
  formatExtensions,
  formatSize,
  isFiltering,
  moveIndex,
  moveTarget,
  nameOf,
  newFolderPath,
  parentOf,
  parseHeight,
  renameTarget,
  sortEntries,
  ASSETS_MIN,
  ABOVE_MIN,
  ASSETS_DEFAULT,
} from "./assets.ts";

const item = (path: string, over: Partial<AssetItem> = {}): AssetItem => ({ path, kind: "image", size: 100, used_by: 0, status: "available", ...over });

const LIST: AssetList = {
  folders: ["assets/Photos/", "assets/Photos/old/", "assets/empty/", "assets/maps/"],
  items: [
    item("assets/b.png", { size: 3000, used_by: 2 }),
    item("assets/a.svg", { size: null, status: "missing" }),
    item("assets/Photos/harbour 10.jpg", { size: 50 }),
    item("assets/Photos/harbour 9.jpg", { size: 70, used_by: 1 }),
    // A folder only implied by an item path.
    item("assets/loose/x.gif"),
  ],
};

const keys = (entries: Entry[]) => entries.map(entryKey);

test("paths: parent folder and name of items and folders", () => {
  assert.equal(parentOf("assets/a.png"), "assets/");
  assert.equal(parentOf("assets/f/sub/a.png"), "assets/f/sub/");
  assert.equal(parentOf("assets/f/sub/"), "assets/f/");
  assert.equal(parentOf("assets/f/"), "assets/");
  assert.equal(parentOf("assets/"), "assets/");
  assert.equal(nameOf("assets/f/Mein Bild.v2.png"), "Mein Bild.v2.png");
  assert.equal(nameOf("assets/f/sub/"), "sub");
  assert.deepEqual(formatExtensions("jpeg"), ["jpg", "jpeg"]);
  assert.deepEqual(formatExtensions("png"), ["png"]);
});

test("tree: folders from markers and item paths, children by name", () => {
  const tree = folderTree(LIST);
  assert.equal(tree.path, "assets/");
  assert.deepEqual(
    tree.children.map((c) => c.name),
    ["empty", "loose", "maps", "Photos"],
  );
  assert.deepEqual(findFolder(tree, "assets/Photos/")!.children.map((c) => c.path), ["assets/Photos/old/"]);
  assert.equal(findFolder(tree, "assets/Photos/old/")!.name, "old");
  assert.equal(findFolder(tree, "assets/nope/"), null);
  // Parents of deep folders without markers are implied.
  const deep = folderTree({ folders: ["assets/a/b/c/"], items: [] });
  assert.equal(findFolder(deep, "assets/a/b/")!.children[0].path, "assets/a/b/c/");
});

test("contents: Sources in the root, then subfolders, then the folder's own items", () => {
  const tree = folderTree(LIST);
  assert.deepEqual(keys(folderEntries(LIST, tree, "assets/", true)), [
    "sources:",
    "assets/empty/",
    "assets/loose/",
    "assets/maps/",
    "assets/Photos/",
    "assets/b.png",
    "assets/a.svg",
  ]);
  assert.deepEqual(keys(folderEntries(LIST, tree, "assets/", false)).slice(0, 1), ["assets/empty/"]);
  assert.deepEqual(keys(folderEntries(LIST, tree, "assets/Photos/", true)), ["assets/Photos/old/", "assets/Photos/harbour 10.jpg", "assets/Photos/harbour 9.jpg"]);
  assert.deepEqual(folderEntries(LIST, tree, "assets/gone/", true), []);
});

test("search and Unused: across all folders, by name or folder, any case", () => {
  assert.equal(isFiltering({ query: "  ", unused: false }), false);
  assert.equal(isFiltering({ query: "", unused: true }), true);
  assert.deepEqual(keys(filterEntries(LIST, { query: "HARBOUR", unused: false })), ["assets/Photos/harbour 10.jpg", "assets/Photos/harbour 9.jpg"]);
  assert.deepEqual(keys(filterEntries(LIST, { query: "photos/", unused: true })), ["assets/Photos/harbour 10.jpg"]);
  assert.deepEqual(keys(filterEntries(LIST, { query: "", unused: true })), ["assets/a.svg", "assets/Photos/harbour 10.jpg", "assets/loose/x.gif"]);
  // The root's name is not searched.
  assert.deepEqual(filterEntries(LIST, { query: "assets", unused: false }), []);
});

test("sorting: folders first by name, items by the key, ties by name", () => {
  const tree = folderTree(LIST);
  const entries = [...folderEntries(LIST, tree, "assets/Photos/", false), ...filterEntries(LIST, { query: "", unused: false })].filter((e, i, all) => all.findIndex((x) => entryKey(x) === entryKey(e)) === i);
  const by = (key: "name" | "type" | "size" | "used" | "status", descending = false) => keys(sortEntries(entries, { key, descending }));
  assert.deepEqual(by("name"), ["assets/Photos/old/", "assets/a.svg", "assets/b.png", "assets/Photos/harbour 9.jpg", "assets/Photos/harbour 10.jpg", "assets/loose/x.gif"]);
  assert.deepEqual(by("name", true).slice(0, 2), ["assets/Photos/old/", "assets/loose/x.gif"]);
  assert.deepEqual(by("size"), ["assets/Photos/old/", "assets/a.svg", "assets/Photos/harbour 10.jpg", "assets/Photos/harbour 9.jpg", "assets/loose/x.gif", "assets/b.png"]);
  assert.deepEqual(by("used", true).slice(1, 3), ["assets/b.png", "assets/Photos/harbour 9.jpg"]);
  assert.deepEqual(by("status", true).slice(0, 2), ["assets/Photos/old/", "assets/a.svg"]);
  assert.deepEqual(by("type"), ["assets/Photos/old/", "assets/loose/x.gif", "assets/Photos/harbour 9.jpg", "assets/Photos/harbour 10.jpg", "assets/b.png", "assets/a.svg"]);
  const sources = sortEntries([{ type: "item", path: "assets/a.png", item: item("assets/a.png") }, { type: "sources" }], { key: "size", descending: true });
  assert.equal(sources[0].type, "sources");
});

test("keyboard: moves in a grid of rows, and in the list", () => {
  // 10 entries in rows of 4: 0-3, 4-7, 8-9.
  assert.equal(moveIndex(5, 10, 4, "ArrowRight"), 6);
  assert.equal(moveIndex(9, 10, 4, "ArrowRight"), 9);
  assert.equal(moveIndex(0, 10, 4, "ArrowLeft"), 0);
  assert.equal(moveIndex(5, 10, 4, "ArrowUp"), 1);
  assert.equal(moveIndex(1, 10, 4, "ArrowUp"), 1);
  assert.equal(moveIndex(1, 10, 4, "ArrowDown"), 5);
  // Down into the shorter last row lands on its end; from the last row it stays.
  assert.equal(moveIndex(7, 10, 4, "ArrowDown"), 9);
  assert.equal(moveIndex(9, 10, 4, "ArrowDown"), 9);
  assert.equal(moveIndex(5, 10, 4, "Home"), 0);
  assert.equal(moveIndex(5, 10, 4, "End"), 9);
  assert.equal(moveIndex(5, 10, 4, "Enter"), null);
  assert.equal(moveIndex(0, 0, 4, "ArrowDown"), null);
  // The list: one column, no Left/Right; an index past the end is clamped.
  assert.equal(moveIndex(2, 5, 1, "ArrowDown"), 3);
  assert.equal(moveIndex(2, 5, 1, "ArrowUp"), 1);
  assert.equal(moveIndex(2, 5, 1, "ArrowLeft"), null);
  assert.equal(moveIndex(9, 5, 1, "ArrowUp"), 3);
});

test("usages: the quoted literals with their lines and snippets", () => {
  const text = 'Intro.\n#figure(image("assets/a.png", width: 80%))\n\nSee "assets/a.png" and "assets/a.png.bak".\n// "assets/a.png"';
  const found = findUsages(text, "assets/a.png");
  assert.deepEqual(
    found.map((u) => [u.line, text.slice(u.from, u.to)]),
    [
      [2, "assets/a.png"],
      [4, "assets/a.png"],
      [5, "assets/a.png"],
    ],
  );
  assert.equal(found[0].snippet, '#figure(image("assets/a.png", width: 80%))');
  assert.equal(found[0].from, text.indexOf('"assets/a.png"') + 1);
  assert.equal(findUsages(text, "assets/a.png", 1).length, 1);
  assert.deepEqual(findUsages(text, "assets/b.png"), []);
  // Quotes pair up in order, as the core counts: the closing quote of one literal never opens the next.
  assert.equal(findUsages('"assets/a.png"assets/a.png"', "assets/a.png").length, 1);
  assert.equal(findUsages('say "hi" "assets/a.png" "', "assets/a.png").length, 1);
  // Long lines are clipped around the literal.
  const long = `${"x".repeat(100)} "assets/a.png" ${"y".repeat(100)}`;
  const [u] = findUsages(long, "assets/a.png");
  assert.ok(u.snippet.startsWith("…") && u.snippet.endsWith("…") && u.snippet.includes("assets/a.png"), u.snippet);
});

test("rename: the folder stays, the format stays, names are checked", () => {
  const ok = (path: string, name: string) => {
    const r = renameTarget(path, name);
    assert.ok("path" in r, `${path} -> ${name}: ${"error" in r ? r.error : ""}`);
    return r.path;
  };
  const error = (path: string, name: string) => {
    const r = renameTarget(path, name);
    assert.ok("error" in r, `${path} -> ${name} should fail`);
    return r.error;
  };
  assert.equal(ok("assets/f/a.png", "Größe 2 (final)"), "assets/f/Größe 2 (final).png");
  assert.equal(ok("assets/f/a.png", " b.png "), "assets/f/b.png");
  assert.equal(ok("assets/f/a.png", "b.PNG"), "assets/f/b.png");
  assert.equal(ok("assets/a.jpg", "b.jpeg"), "assets/b.jpeg");
  assert.equal(ok("assets/a.png", "v1.2"), "assets/v1.2.png");
  assert.equal(ok("assets/f/sub/", "Bilder"), "assets/f/Bilder/");
  assert.match(error("assets/a.png", "b.svg"), /\.png/);
  assert.match(error("assets/a.png", ""), /Enter a name/);
  for (const bad of ["a/b", 'a"b', "a:b", "a\\b", ".hidden", "b.", "a\tb"]) error("assets/a.png", bad);
  error("assets/f/", "x/y");
  error("assets/f/", "..");
});

test("move targets, folder usage and new folder names", () => {
  assert.equal(moveTarget("assets/a.png", "assets/f/"), "assets/f/a.png");
  assert.equal(moveTarget("assets/f/a.png", "assets/"), "assets/a.png");
  assert.equal(moveTarget("assets/f/a.png", "assets/f/"), null);
  assert.equal(moveTarget("assets/f/", "assets/g/"), "assets/g/f/");
  assert.equal(moveTarget("assets/f/", "assets/f/sub/"), null);
  assert.equal(moveTarget("assets/f/", "assets/f/"), null);
  assert.equal(moveTarget("assets/g/f/", "assets/"), "assets/f/");
  assert.equal(folderUsed(LIST, "assets/Photos/"), true);
  assert.equal(folderUsed(LIST, "assets/Photos/old/"), false);
  assert.equal(folderUsed(LIST, "assets/loose/"), false);
  const tree = folderTree({ folders: ["assets/New folder/", "assets/new folder 2/"], items: [] });
  assert.equal(newFolderPath(tree, "assets/"), "assets/New folder 3/");
  assert.equal(newFolderPath(tree, "assets/New folder/"), "assets/New folder/New folder/");
});

test("display: sizes and the panel height", () => {
  assert.equal(formatSize(null), "–");
  assert.equal(formatSize(512), "512 B");
  assert.equal(formatSize(1536), "1.5 KB");
  assert.equal(formatSize(20 * 1024), "20 KB");
  assert.equal(formatSize(3.25 * 1024 * 1024), "3.3 MB");
  assert.equal(parseHeight("300"), 300);
  for (const bad of [null, "", "x", "-4", "0"]) assert.equal(parseHeight(bad), ASSETS_DEFAULT);
  assert.equal(clampHeight(50, 900), ASSETS_MIN);
  assert.equal(clampHeight(300, 900), 300);
  assert.equal(clampHeight(850, 900), 900 - ABOVE_MIN);
  assert.equal(clampHeight(300, 200), ASSETS_MIN);
});
