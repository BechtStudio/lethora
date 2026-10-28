// The command list: every menu item and every keyboard shortcut is a command
// (no shortcut-only features). Plain data and functions, no UI: the menus
// (menus.ts) and the window-wide shortcuts (main.ts) read it, later also the
// macOS menu and a command palette. No DOM imports, so this runs under
// `node --test`.

/** A command. */
export interface Command {
  id: string;
  /** The menu label; `&` marks the mnemonic letter ("&Save", "Save chec&kpoint…"). */
  label: string;
  /** Its shortcuts as shown ("Ctrl+Shift+S", "F5"); menus show the first. */
  keys?: readonly string[];
  /**
   * The keys are handled where they apply (the editor, text fields, the page
   * preview): shown in the menu, never matched window-wide.
   */
  local?: boolean;
  run: () => unknown;
  /** Greyed out in menus and not run by its keys when false. */
  enabled?: () => boolean;
  /** A check mark from live state (a radio dot with `radio`). */
  checked?: () => boolean;
  radio?: boolean;
  /** A submenu: its items and separators (null), from live state (greyed out without items). */
  children?: () => ReadonlyArray<Command | null>;
  /** Dimmed text beside the label, also its tooltip (a recent project's path). */
  detail?: string;
}

/** A parsed shortcut: `key` as `KeyboardEvent.key` gives it, lower case for characters. */
export interface Keys {
  key: string;
  ctrl: boolean;
  shift: boolean;
  alt: boolean;
}

/** The parts of a keydown event shortcuts look at. */
export interface KeyInput {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  /** The physical key ("KeyM"), for Ctrl+Alt letters where the layout types a character (AltGr). */
  code?: string;
}

/** Key names that would clash with the `+` separator or read badly. */
const NAMED: Record<string, string> = { Plus: "+", Minus: "-" };

/** "Ctrl+Shift+S" → { key: "s", ctrl, shift }. Ctrl also stands for Cmd. */
export function parseKeys(text: string): Keys {
  const parts = text.split("+");
  const name = parts.pop() ?? "";
  const key = NAMED[name] ?? (name.length === 1 ? name.toLowerCase() : name);
  return { key, ctrl: parts.includes("Ctrl"), shift: parts.includes("Shift"), alt: parts.includes("Alt") };
}

const isFKey = (key: string) => /^F\d{1,2}$/.test(key);

const hasModifier = (e: KeyInput) => e.ctrlKey || e.metaKey || e.shiftKey || e.altKey;

/** Whether `e` is the shortcut `text` (Cmd counts as Ctrl). */
export function matches(e: KeyInput, text: string): boolean {
  const k = parseKeys(text);
  // Ctrl+Alt is AltGr on Windows: Ctrl+Alt+M types "µ" on a German keyboard.
  const altLetter = k.ctrl && k.alt && /^[a-z]$/.test(k.key) && e.code === `Key${k.key.toUpperCase()}`;
  // Digits by the physical key too: Ctrl+Shift+1 gives "!" (letters never: Ctrl+Z is the key labelled Z).
  const digit = /^[0-9]$/.test(k.key) && e.code === `Digit${k.key}`;
  return (e.key.toLowerCase() === k.key.toLowerCase() || altLetter || digit) && (e.ctrlKey || e.metaKey) === k.ctrl && e.shiftKey === k.shift && e.altKey === k.alt;
}

/** Whether two shortcuts are the same keys ("Ctrl+b" and "Ctrl+B"). */
export function sameKeys(a: string, b: string): boolean {
  const x = parseKeys(a);
  const y = parseKeys(b);
  return x.key.toLowerCase() === y.key.toLowerCase() && x.ctrl === y.ctrl && x.shift === y.shift && x.alt === y.alt;
}

/** Keys that are only modifiers: recording waits for the next. */
const MODIFIERS = ["Control", "Shift", "Alt", "AltGraph", "Meta", "OS", "CapsLock", "Fn"];

/**
 * A keydown as shortcut text ("Ctrl+Shift+S"), null for a modifier alone.
 * Modifiers in the order Ctrl, Shift, Alt (Cmd counts as Ctrl); letters
 * upper case, "+" and "-" as Plus and Minus. With Ctrl+Alt (AltGr typing a
 * character) the letter or digit comes from the physical key, as with Shift
 * and a digit.
 */
export function keysFromEvent(e: KeyInput): string | null {
  if (MODIFIERS.includes(e.key)) return null;
  const ctrl = e.ctrlKey || e.metaKey;
  const code = /^(?:Key([A-Z])|Digit([0-9]))$/.exec(e.code ?? "");
  // Upper case where that stays one character ("ß" would become "SS").
  let name = e.key === " " ? "Space" : e.key.length === 1 && e.key.toUpperCase().length === 1 ? e.key.toUpperCase() : e.key;
  if (code && ((ctrl && e.altKey) || (e.shiftKey && code[2]))) name = code[1] ?? code[2];
  name = Object.keys(NAMED).find((n) => NAMED[n] === name) ?? name;
  return [ctrl && "Ctrl", e.shiftKey && "Shift", e.altKey && "Alt", name].filter(Boolean).join("+");
}

/** Keys for typing and moving the cursor, never a shortcut. */
const TYPING = ["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End", "PageUp", "PageDown", "Backspace", "Delete", "Enter", "Tab", "Escape", "Space", "Insert"];

/** macOS: Cmd+H and Cmd+M are the system's (Hide, Minimize); these commands take Control there. */
export const SYSTEM_KEYS: readonly string[] = ["Ctrl+H", "Ctrl+M"];

/**
 * Why `text` can't be a shortcut, or null if it can. Allowed are F1–F12
 * (alone or with modifiers) and Ctrl (with Shift, Alt) plus a character;
 * a `local` command's keys are reserved. Another command using the keys is
 * no problem here (the settings page offers to move them).
 */
export function keysProblem(text: string, commands: ReadonlyArray<Pick<Command, "label" | "keys" | "local">>, mac = false): string | null {
  const shown = showKeys(text, mac);
  const parts = text.split("+");
  const name = parts.pop() ?? "";
  const k = parseKeys(text);
  const order = ["Ctrl", "Shift", "Alt"].filter((m) => parts.includes(m));
  if (parts.join("+") !== order.join("+")) return `${shown} can't be a shortcut.`;
  if (TYPING.includes(name)) return `Shortcuts can't use ${name}: it is for typing and moving the cursor.`;
  const fKey = /^F([1-9]|1[0-2])$/.test(name);
  if (!fKey && name.length !== 1 && !Object.hasOwn(NAMED, name)) return `${shown} can't be a shortcut.`;
  if (!fKey && !k.ctrl) return `${shown} would type or open a menu: add ${mac ? "⌘" : "Ctrl"}, or use an F-key.`;
  if (sameKeys(text, "Alt+F4")) return `${shown} closes the window.`;
  if (BLOCKED.some((b) => sameKeys(text, b))) return `${shown} would reload the window.`;
  if (sameKeys(text, "Ctrl+Q")) return mac ? `${shown} quits Lethora.` : `${shown} is kept free: it quits Lethora on macOS.`;
  const locked = commands.find((c) => c.local && c.keys?.some((key) => sameKeys(key, text)));
  if (locked) return `${shown} is reserved for ${plainLabel(locked.label)}.`;
  return null;
}

/** The modifiers' Mac symbols, in the Mac order. */
const MAC_MODIFIERS: ReadonlyArray<[string, string]> = [
  ["Alt", "⌥"],
  ["Shift", "⇧"],
  ["Ctrl", "⌘"],
];

/**
 * Shortcut text as shown: as stored, or on macOS the Mac way ("Ctrl+Shift+S"
 * → "⇧⌘S"; `SYSTEM_KEYS` take Control: "⌃H").
 */
export function showKeys(text: string, mac: boolean): string {
  if (!mac) return text;
  const parts = text.split("+");
  const name = parts.pop() ?? "";
  const key = NAMED[name] ?? name;
  if (SYSTEM_KEYS.includes(text)) return `⌃${key}`;
  return MAC_MODIFIERS.filter(([m]) => parts.includes(m)).map(([, symbol]) => symbol).join("") + key;
}

/**
 * Sets the commands' keys: settings.json's `shortcuts` (command id → keys)
 * over the defaults. `local` commands and submenus keep theirs, stored keys
 * that can't be a shortcut are skipped, and a stored key wins over another
 * command's default (a key does one thing). Unknown ids are ignored.
 */
export function applyShortcuts(commands: readonly Command[], defaults: ReadonlyMap<string, readonly string[] | undefined>, stored: Readonly<Record<string, readonly string[]>>): void {
  for (const c of commands) c.keys = defaults.get(c.id);
  const editable = commands.filter((c) => !c.local && !c.children);
  const taken: string[] = [];
  for (const c of editable) {
    if (!Object.hasOwn(stored, c.id)) continue;
    c.keys = stored[c.id].filter((k) => keysProblem(k, commands) === null && !taken.some((t) => sameKeys(t, k)));
    taken.push(...c.keys);
  }
  for (const c of editable) if (!Object.hasOwn(stored, c.id)) c.keys = c.keys?.filter((k) => !taken.some((t) => sameKeys(t, k)));
}

/** Keys only kept from the web view: reloading would lose unsent edits. */
export const BLOCKED: readonly string[] = ["Ctrl+R", "Ctrl+Shift+R"];

/** What a window-wide keydown does: run a command, only be swallowed, or nothing (null). */
export type KeyAction = { run: Command } | { swallow: true } | null;

/**
 * The window-wide meaning of a keydown. An F-key of a command runs it
 * without modifiers and is only swallowed with them (so Ctrl+F5 never
 * reloads either), unless a command has that combination ("Ctrl+F4");
 * `local` commands are skipped.
 */
export function keyAction(commands: readonly Command[], e: KeyInput): KeyAction {
  const global = commands.filter((c) => !c.local);
  if (isFKey(e.key)) {
    const exact = global.find((c) => c.keys?.some((k) => matches(e, k)));
    if (exact) return { run: exact };
    const command = global.find((c) => c.keys?.includes(e.key));
    if (!command) return null;
    return hasModifier(e) ? { swallow: true } : { run: command };
  }
  if (BLOCKED.some((k) => matches(e, k))) return { swallow: true };
  const command = global.find((c) => c.keys?.some((k) => matches(e, k)));
  return command ? { run: command } : null;
}

/** Whether a command can run now. */
export const isEnabled = (c: Command): boolean => c.enabled?.() ?? true;

/** The label without its `&`. */
export const plainLabel = (label: string): string => label.replace("&", "");

/** The mnemonic letter (lower case), or "" if the label marks none. */
export function mnemonic(label: string): string {
  const i = label.indexOf("&");
  return i >= 0 && i + 1 < label.length ? label[i + 1].toLowerCase() : "";
}
