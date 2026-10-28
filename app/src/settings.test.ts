// Tests for the Settings view's data and search. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import type { Command } from "./commands.ts";
import { CATEGORIES, choicesFor, defaultsOf, type Row, searchSettings, shortcutSections, storedKeys, versionLine, withKeys, withShortcuts } from "./settings.ts";

const commands: Command[] = [
  { id: "new", label: "&New project…", keys: ["Ctrl+N"], run: () => undefined },
  { id: "open-recent", label: "Open &recent", run: () => undefined, children: () => [] },
  { id: "share", label: "Share / S&ync…", run: () => undefined },
  { id: "redo", label: "&Redo", keys: ["Ctrl+Y", "Ctrl+Shift+Z"], local: true, run: () => undefined },
  { id: "bold", label: "&Bold", keys: ["Ctrl+Shift+B"], run: () => undefined },
  { id: "about", label: "&About Lethora", run: () => undefined },
];
const command = (id: string) => commands.find((c) => c.id === id);
/** Bold's default is Ctrl+B (here changed). */
const defaults = (id: string) => (id === "bold" ? ["Ctrl+B"] : (command(id)?.keys ?? []));
const menus = [
  { label: "&File", items: ["new", "open-recent", null, "share"] },
  { label: "&Edit", items: ["redo"] },
  { label: "F&ormat", items: ["bold"] },
  { label: "&Help", items: ["about"] },
];
const categories = withShortcuts(CATEGORIES, shortcutSections(menus, command, defaults));
const category = (id: string) => categories.find((c) => c.id === id)!;
/** "Category › Section › Row" for each found row. */
const found = (query: string) => searchSettings(categories, query).flatMap((f) => f.sections.flatMap((s) => s.rows.map((r) => `${f.category.label} › ${s.title} › ${r.label}`)));

test("every stored row's key is unique and every select has options", () => {
  const keys = categories.flatMap(storedKeys);
  assert.equal(new Set(keys).size, keys.length);
  assert.deepEqual(keys.toSorted(), ["check_updates_on_start", "editor_line_spacing", "editor_paragraph_gap", "editor_text_size", "reopen_last_project", "shortcuts", "spell_check", "theme"]);
  for (const c of categories) for (const s of c.sections) for (const r of s.rows) if (r.kind === "select") assert.ok(r.options && r.options.length > 1, r.key);
});

test("the shortcuts page lists every command but submenus, by menu, without mnemonics", () => {
  assert.deepEqual(
    category("shortcuts").sections.map((s) => [s.title, s.rows.map((r) => [r.label, r.keys])]),
    [
      [
        "File",
        [
          ["New project…", ["Ctrl+N"]],
          ["Share / Sync…", []],
        ],
      ],
      ["Edit", [["Redo", ["Ctrl+Y", "Ctrl+Shift+Z"]]]],
      ["Format", [["Bold", ["Ctrl+Shift+B"]]]],
      ["Help", [["About Lethora", []]]],
    ],
  );
});

test("locked commands are shown with a note; the others with their defaults", () => {
  const rows = category("shortcuts").sections.flatMap((s) => s.rows);
  const redo = rows.find((r) => r.key === "redo")!;
  assert.equal(redo.local, true);
  assert.match(redo.description, /Text fields handle these keys/);
  const bold = rows.find((r) => r.key === "bold")!;
  assert.ok(!bold.local);
  assert.equal(bold.description, "");
  assert.deepEqual(bold.defaults, ["Ctrl+B"]);
});

test("new keys are stored only for changed commands and taken from others", () => {
  const rows = category("shortcuts").sections.flatMap((s) => s.rows);
  const row = (id: string) => rows.find((r) => r.key === id)!;
  const stored = { bold: ["Ctrl+Shift+B"], gone: ["Ctrl+J"] };
  // Bold back at its default: left out (unknown ids stay).
  assert.deepEqual(withKeys(stored, rows, row("bold"), ["Ctrl+B"]), { gone: ["Ctrl+J"] });
  // Share takes Ctrl+N from New project, both are stored.
  assert.deepEqual(withKeys({}, rows, row("share"), ["Ctrl+N"]), { share: ["Ctrl+N"], new: [] });
  // Removing the last key stores an empty list.
  assert.deepEqual(withKeys({}, rows, row("new"), []), { new: [] });
  // Resetting takes the default back from whoever has it (Share is back at none).
  const taken: Row[] = rows.map((r) => (r.key === "share" ? { ...r, keys: ["Ctrl+B"] } : r));
  assert.deepEqual(withKeys({ bold: ["Ctrl+Shift+B"], share: ["Ctrl+B"] }, taken, row("bold"), ["Ctrl+B"]), {});
});

test("restore defaults sends null for the page's stored settings only", () => {
  assert.deepEqual(defaultsOf(category("editor")), { editor_text_size: null, editor_line_spacing: null, editor_paragraph_gap: null, spell_check: null });
  assert.deepEqual(defaultsOf(category("updates")), { check_updates_on_start: null });
  assert.deepEqual(defaultsOf(category("about")), {});
  // The shortcuts page resets every command's keys.
  assert.deepEqual(defaultsOf(category("shortcuts")), { shortcuts: null });
});

test("a value outside the choices is added so the select shows it", () => {
  const size = category("editor").sections[0].rows[0];
  assert.equal(choicesFor(size, 19.5).length, 6);
  assert.deepEqual(choicesFor(size, 20).at(-1), { value: 20, label: "20" });
  assert.equal(choicesFor(size, undefined).length, 6);
});

test("search matches labels, descriptions, keywords, sections and categories, case-insensitive", () => {
  assert.deepEqual(found("TEXT SIZE"), ["Editor › Text › Text size"]);
  assert.deepEqual(found("dark mode"), ["Appearance › Theme › Theme"]);
  assert.deepEqual(found("dictionary"), ["Editor › Spelling › Check spelling"]);
  // A section's name finds all its rows; words may come from different places.
  assert.deepEqual(found("startup"), ["General › Startup › Reopen the last project"]);
  assert.deepEqual(found("editor spacing"), ["Editor › Text › Line spacing"]);
  assert.ok(found("update").includes("Updates › Manual › Check for updates now"));
});

test("search finds commands on the shortcuts page by name and keys", () => {
  assert.deepEqual(found("redo"), ["Keyboard shortcuts › Edit › Redo"]);
  assert.deepEqual(found("ctrl+n"), ["Keyboard shortcuts › File › New project…"]);
  assert.deepEqual(found("ctrl+shift+z"), ["Keyboard shortcuts › Edit › Redo"]);
  // On macOS also by the keys as shown there.
  const mac = withShortcuts(CATEGORIES, shortcutSections(menus, command, defaults, true));
  const rows = (query: string) => searchSettings(mac, query).flatMap((f) => f.sections.flatMap((s) => s.rows.map((r) => r.label)));
  assert.deepEqual(rows("⇧⌘B"), ["Bold"]);
  assert.deepEqual(rows("ctrl+shift+b"), ["Bold"]);
});

test("results are grouped by category and section; nothing for no match or an empty query", () => {
  const result = searchSettings(categories, "check");
  assert.deepEqual(
    result.map((f) => [f.category.id, f.sections.map((s) => s.title)]),
    [
      ["editor", ["Spelling"]],
      ["updates", ["Automatic", "Manual"]],
    ],
  );
  assert.deepEqual(searchSettings(categories, "zebra"), []);
  assert.deepEqual(searchSettings(categories, "   "), []);
});

test("the manual check names the installed version", () => {
  assert.equal(versionLine("0.1.0"), "You have version 0.1.0.");
});
