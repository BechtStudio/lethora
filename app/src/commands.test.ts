// Tests for the command list's shortcut matching. Run: npm test.

import { test } from "node:test";
import assert from "node:assert/strict";
import { applyShortcuts, type Command, type KeyInput, isEnabled, keyAction, keysFromEvent, keysProblem, matches, mnemonic, parseKeys, plainLabel, sameKeys, showKeys } from "./commands.ts";

const key = (k: string, mods: Partial<Omit<KeyInput, "key">> = {}): KeyInput => ({ key: k, ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, ...mods });

const noop = () => {};
const commands: Command[] = [
  { id: "save", label: "&Save", keys: ["Ctrl+S"], run: noop },
  { id: "checkpoint", label: "Save chec&kpoint…", keys: ["Ctrl+Shift+S"], run: noop },
  { id: "checkpoints", label: "&Checkpoints", keys: ["Ctrl+."], run: noop },
  { id: "author", label: "&Author", keys: ["Ctrl+1"], run: noop, radio: true },
  { id: "compile", label: "Compi&le", keys: ["F5"], run: noop },
  { id: "pages", label: "Pa&ges", keys: ["F6"], run: noop, enabled: () => false },
  { id: "undo", label: "&Undo", keys: ["Ctrl+Z"], local: true, run: noop },
  { id: "redo", label: "&Redo", keys: ["Ctrl+Y", "Ctrl+Shift+Z"], local: true, run: noop },
  { id: "find", label: "&Find…", keys: ["Ctrl+F"], run: noop },
  { id: "focus", label: "Focus mo&de", keys: ["Ctrl+Shift+F"], run: noop },
  { id: "zoom-in", label: "Zoom &in", keys: ["Ctrl+Plus"], local: true, run: noop },
  { id: "close", label: "&Close project", keys: ["Ctrl+W", "Ctrl+F4"], run: noop },
  // Shown only: the OS closes the window.
  { id: "exit", label: "E&xit", keys: ["Alt+F4"], local: true, run: noop },
];

const ran = (e: KeyInput) => {
  const a = keyAction(commands, e);
  return a && "run" in a ? a.run.id : a;
};

test("shortcut text is parsed into a key and modifiers", () => {
  assert.deepEqual(parseKeys("Ctrl+Shift+S"), { key: "s", ctrl: true, shift: true, alt: false });
  assert.deepEqual(parseKeys("Ctrl+."), { key: ".", ctrl: true, shift: false, alt: false });
  assert.deepEqual(parseKeys("F5"), { key: "F5", ctrl: false, shift: false, alt: false });
  assert.deepEqual(parseKeys("Ctrl+Plus"), { key: "+", ctrl: true, shift: false, alt: false });
  assert.deepEqual(parseKeys("Ctrl+Minus"), { key: "-", ctrl: true, shift: false, alt: false });
});

test("matching: Cmd counts as Ctrl, every modifier must agree, letters ignore case", () => {
  assert.ok(matches(key("s", { ctrlKey: true }), "Ctrl+S"));
  assert.ok(matches(key("s", { metaKey: true }), "Ctrl+S"));
  assert.ok(matches(key("S", { ctrlKey: true, shiftKey: true }), "Ctrl+Shift+S"));
  assert.ok(!matches(key("S", { ctrlKey: true, shiftKey: true }), "Ctrl+S"));
  assert.ok(!matches(key("s", { ctrlKey: true, altKey: true }), "Ctrl+S"));
  assert.ok(!matches(key("s"), "Ctrl+S"));
  // Ctrl+Alt letters by the physical key (AltGr layouts type a character), only with Ctrl+Alt.
  assert.ok(matches(key("µ", { ctrlKey: true, altKey: true, code: "KeyM" }), "Ctrl+Alt+M"));
  assert.ok(matches(key("m", { ctrlKey: true, altKey: true, code: "KeyM" }), "Ctrl+Alt+M"));
  assert.ok(!matches(key("µ", { ctrlKey: true, code: "KeyM" }), "Ctrl+M"));
  assert.ok(!matches(key("j", { ctrlKey: true, altKey: true, code: "KeyJ" }), "Ctrl+Alt+M"));
});

test("window-wide keys run their command", () => {
  assert.equal(ran(key("s", { ctrlKey: true })), "save");
  assert.equal(ran(key("S", { ctrlKey: true, shiftKey: true })), "checkpoint");
  assert.equal(ran(key(".", { ctrlKey: true })), "checkpoints");
  assert.equal(ran(key("1", { ctrlKey: true })), "author");
  assert.equal(ran(key("f", { ctrlKey: true })), "find");
  assert.equal(ran(key("F", { ctrlKey: true, shiftKey: true })), "focus");
  // Ctrl+Shift+1 is Ctrl+! on most layouts: nothing.
  assert.equal(ran(key("!", { ctrlKey: true, shiftKey: true })), null);
  // AltGr (Ctrl+Alt) never triggers a Ctrl shortcut.
  assert.equal(ran(key("s", { ctrlKey: true, altKey: true })), null);
  assert.equal(ran(key("s")), null);
});

test("local keys are left to the editor, text fields and the page preview", () => {
  assert.equal(ran(key("z", { ctrlKey: true })), null);
  assert.equal(ran(key("Z", { ctrlKey: true, shiftKey: true })), null);
  assert.equal(ran(key("y", { ctrlKey: true })), null);
  assert.equal(ran(key("+", { ctrlKey: true })), null);
});

test("F-keys run without modifiers and are only swallowed with them", () => {
  assert.equal(ran(key("F5")), "compile");
  // Disabled commands still own their key (the caller checks `isEnabled`).
  assert.equal(ran(key("F6")), "pages");
  assert.deepEqual(keyAction(commands, key("F5", { ctrlKey: true })), { swallow: true });
  assert.deepEqual(keyAction(commands, key("F5", { shiftKey: true })), { swallow: true });
  assert.deepEqual(keyAction(commands, key("F6", { altKey: true })), { swallow: true });
  // F-keys no command uses are left alone.
  assert.equal(keyAction(commands, key("F1")), null);
  assert.equal(keyAction(commands, key("F3", { ctrlKey: true })), null);
});

test("an F-key with modifiers runs the command that has that combination", () => {
  assert.equal(ran(key("F4", { ctrlKey: true })), "close");
  assert.equal(ran(key("w", { ctrlKey: true })), "close");
  // Alt+F4 stays the OS's (Exit only shows it), plain F4 is no one's.
  assert.equal(keyAction(commands, key("F4", { altKey: true })), null);
  assert.equal(keyAction(commands, key("F4")), null);
});

test("reloading is blocked", () => {
  assert.deepEqual(keyAction(commands, key("r", { ctrlKey: true })), { swallow: true });
  assert.deepEqual(keyAction(commands, key("R", { ctrlKey: true, shiftKey: true })), { swallow: true });
  assert.deepEqual(keyAction(commands, key("r", { metaKey: true })), { swallow: true });
  assert.equal(keyAction(commands, key("r")), null);
});

test("enabled defaults to true", () => {
  assert.ok(isEnabled(commands[0]));
  assert.ok(!isEnabled(commands[5]));
});

test("labels: the mnemonic letter and the plain text", () => {
  assert.equal(mnemonic("Save chec&kpoint…"), "k");
  assert.equal(mnemonic("&File"), "f");
  assert.equal(mnemonic("Plain"), "");
  assert.equal(plainLabel("Save chec&kpoint…"), "Save checkpoint…");
  assert.equal(plainLabel("Plain"), "Plain");
});

test("digits match by the physical key too, letters never", () => {
  assert.ok(matches(key("!", { ctrlKey: true, shiftKey: true, code: "Digit1" }), "Ctrl+Shift+1"));
  assert.ok(matches(key("&", { ctrlKey: true, code: "Digit1" }), "Ctrl+1"));
  assert.ok(!matches(key("!", { ctrlKey: true, code: "Digit1" }), "Ctrl+Shift+1"));
  // QWERTZ: the key labelled Z (code KeyY) is Ctrl+Z, not Ctrl+Y.
  assert.ok(!matches(key("z", { ctrlKey: true, code: "KeyY" }), "Ctrl+Y"));
  assert.ok(matches(key("z", { ctrlKey: true, code: "KeyY" }), "Ctrl+Z"));
});

test("a keydown becomes shortcut text", () => {
  assert.equal(keysFromEvent(key("b", { ctrlKey: true, code: "KeyB" })), "Ctrl+B");
  assert.equal(keysFromEvent(key("S", { metaKey: true, shiftKey: true, code: "KeyS" })), "Ctrl+Shift+S");
  assert.equal(keysFromEvent(key("µ", { ctrlKey: true, altKey: true, code: "KeyM" })), "Ctrl+Alt+M");
  assert.equal(keysFromEvent(key("!", { ctrlKey: true, shiftKey: true, code: "Digit1" })), "Ctrl+Shift+1");
  assert.equal(keysFromEvent(key("+", { ctrlKey: true, code: "BracketRight" })), "Ctrl+Plus");
  assert.equal(keysFromEvent(key("-", { ctrlKey: true, code: "Slash" })), "Ctrl+Minus");
  assert.equal(keysFromEvent(key(".", { ctrlKey: true, code: "Period" })), "Ctrl+.");
  assert.equal(keysFromEvent(key("F5", { shiftKey: true, code: "F5" })), "Shift+F5");
  assert.equal(keysFromEvent(key(" ", { ctrlKey: true, code: "Space" })), "Ctrl+Space");
  assert.equal(keysFromEvent(key("ß", { ctrlKey: true, code: "Minus" })), "Ctrl+ß");
  assert.equal(keysFromEvent(key("b", { altKey: true, code: "KeyB" })), "Alt+B");
  // A modifier alone: recording waits.
  assert.equal(keysFromEvent(key("Control", { ctrlKey: true })), null);
  assert.equal(keysFromEvent(key("Shift", { shiftKey: true })), null);
});

test("which keys can be a shortcut", () => {
  const ok = (text: string) => assert.equal(keysProblem(text, commands), null, text);
  ok("Ctrl+B");
  ok("Ctrl+Shift+1");
  ok("Ctrl+Alt+M");
  ok("Ctrl+Shift+Plus");
  ok("Ctrl+.");
  ok("F9");
  ok("Shift+F9");
  ok("Alt+F3");
  const refused = (text: string, why: RegExp) => assert.match(keysProblem(text, commands) ?? "", why, text);
  refused("B", /add Ctrl/);
  refused("Shift+B", /add Ctrl/);
  refused("Alt+B", /open a menu/);
  refused("Ctrl+ArrowUp", /typing and moving/);
  refused("Ctrl+Enter", /typing and moving/);
  refused("Space", /typing and moving/);
  refused("Ctrl+R", /reload/);
  refused("Ctrl+Shift+R", /reload/);
  refused("Alt+F4", /closes the window/);
  refused("Ctrl+Q", /quits/);
  refused("Ctrl+Z", /reserved for Undo/);
  refused("Ctrl+Shift+Z", /reserved for Redo/);
  refused("Ctrl+Plus", /reserved for Zoom in/);
  refused("F13", /can't be a shortcut/);
  refused("Ctrl+PrintScreen", /can't be a shortcut/);
  refused("Shift+Ctrl+B", /can't be a shortcut/);
  refused("Ctrl+", /can't be a shortcut/);
  // macOS words it with its keys.
  assert.equal(keysProblem("B", commands, true), "B would type or open a menu: add ⌘, or use an F-key.");
  assert.equal(keysProblem("Ctrl+Z", commands, true), "⌘Z is reserved for Undo.");
});

test("the same keys whatever the letter case", () => {
  assert.ok(sameKeys("Ctrl+b", "Ctrl+B"));
  assert.ok(!sameKeys("Ctrl+B", "Ctrl+Shift+B"));
});

test("macOS shows keys its own way", () => {
  assert.equal(showKeys("Ctrl+Shift+S", false), "Ctrl+Shift+S");
  assert.equal(showKeys("Ctrl+Shift+S", true), "⇧⌘S");
  assert.equal(showKeys("Ctrl+Alt+M", true), "⌥⌘M");
  assert.equal(showKeys("Ctrl+Shift+Alt+B", true), "⌥⇧⌘B");
  assert.equal(showKeys("Ctrl+Plus", true), "⌘+");
  assert.equal(showKeys("Ctrl+Minus", true), "⌘-");
  assert.equal(showKeys("F5", true), "F5");
  assert.equal(showKeys("Shift+F5", true), "⇧F5");
  // Cmd+H and Cmd+M are the system's: Control.
  assert.equal(showKeys("Ctrl+H", true), "⌃H");
  assert.equal(showKeys("Ctrl+M", true), "⌃M");
  assert.equal(showKeys("Ctrl+Shift+M", true), "⇧⌘M");
});

test("stored shortcuts replace the defaults and the keys follow", () => {
  const list: Command[] = commands.map((c) => ({ ...c }));
  const defaults = new Map(list.map((c) => [c.id, c.keys]));
  const keysOf = (id: string) => list.find((c) => c.id === id)!.keys;
  const run = (e: KeyInput) => {
    const a = keyAction(list, e);
    return a && "run" in a ? a.run.id : a;
  };
  applyShortcuts(list, defaults, { compile: ["F9"], save: ["Ctrl+Shift+1", "Ctrl+Z", "B"], find: [], undo: ["Ctrl+U"], gone: ["Ctrl+G"] });
  assert.deepEqual(keysOf("compile"), ["F9"]);
  // Reserved and refused keys are skipped; local commands keep theirs.
  assert.deepEqual(keysOf("save"), ["Ctrl+Shift+1"]);
  assert.deepEqual(keysOf("find"), []);
  assert.deepEqual(keysOf("undo"), ["Ctrl+Z"]);
  assert.equal(run(key("F9")), "compile");
  assert.deepEqual(keyAction(list, key("F9", { ctrlKey: true })), { swallow: true });
  assert.equal(keyAction(list, key("F5")), null);
  assert.equal(run(key("!", { ctrlKey: true, shiftKey: true, code: "Digit1" })), "save");
  assert.equal(run(key("s", { ctrlKey: true })), null);
  assert.equal(run(key("f", { ctrlKey: true })), null);
  // A stored key wins over another command's default.
  applyShortcuts(list, defaults, { focus: ["Ctrl+F"] });
  assert.deepEqual(keysOf("find"), []);
  assert.equal(run(key("f", { ctrlKey: true })), "focus");
  // Nothing stored: the defaults again.
  applyShortcuts(list, defaults, {});
  assert.deepEqual(keysOf("compile"), ["F5"]);
  assert.deepEqual(keysOf("find"), ["Ctrl+F"]);
  assert.equal(run(key("F5")), "compile");
});
