// The macOS menu bar: the system's own, built from the same menu definitions
// and command list as the in-row menus (menus.ts), which macOS doesn't show.
// Only in the Tauri app on macOS, never in the browser preview.
//
// As on every Mac, the Lethora menu holds About, Settings… (File →
// Settings…) and Quit (File → Exit, so edits are sent first), and there is a
// Window menu. Cut, Copy and Paste are
// the system's own items: they act in every field, as Ctrl+X/C/V do on
// Windows (our commands paste by script only because a page's menu can't
// trigger the native clipboard).
//
// Shortcuts: the web view gets Cmd keys first, and the keydown handler
// (main.ts) stops the ones it runs, so the menu's never runs a second time;
// keys it leaves alone (Cmd+Z and Cmd+A in a field) reach the menu. Cmd+H and
// Cmd+M stay macOS's (Hide, Minimize), those commands take Control instead.
// Changed shortcuts (settings.json) reach the items with the next `refresh`.
//
// There is no "menu will open" hook: enabled, checked and submenu items are
// brought up to date shortly after input (`refresh`), each only when changed.

import { CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu } from "@tauri-apps/api/menu";
import { type Command, isEnabled, matches, plainLabel, SYSTEM_KEYS } from "./commands.ts";
import type { MenuDef } from "./menus.ts";

/** The Tauri app on macOS (read before the browser preview's mock sets up Tauri's internals). */
export const nativeMac = navigator.userAgent.includes("Macintosh") && "__TAURI_INTERNALS__" in window;

/** A keydown the page leaves to macOS's menus: Cmd+H, Cmd+M and Control+Cmd keys (full screen). */
export const systemKey = (e: KeyboardEvent): boolean => e.metaKey && (e.ctrlKey || SYSTEM_KEYS.some((k) => matches(e, k)));

/**
 * Menu shortcuts where macOS differs (null: none). Redo is Shift+Cmd+Z; the
 * zoom keys only act while the page preview has the focus (main.ts), from
 * the menu they would zoom from anywhere. All are `local` commands, whose
 * keys never change, so a changed shortcut never meets these.
 */
const MAC_KEYS: Record<string, string | null> = { redo: "Ctrl+Shift+Z", "zoom-in": null, "zoom-out": null, "zoom-fit": null };

/** Moved into the Lethora menu. */
const APP_ITEMS = ["about", "settings", "exit"];

const NATIVE = { cut: "Cut", copy: "Copy", paste: "Paste" } as const;

/** How long after input the states are brought up to date. */
const REFRESH_MS = 100;

interface MacMenuHost {
  command: (id: string) => Command;
  run: (c: Command) => void;
  error: (e: unknown) => void;
}

/** A command's item and the state it shows (`checked` null: a plain item). */
interface Entry {
  command: Command;
  item: MenuItem | CheckMenuItem;
  enabled: boolean;
  checked: boolean | null;
  accelerator: string | undefined;
}

/** A command with a submenu: its items, rebuilt when their ids, labels or details change. */
interface SubEntry {
  command: Command;
  menu: Submenu;
  enabled: boolean;
  shape: string;
  entries: Entry[];
  items: Array<MenuItem | CheckMenuItem | PredefinedMenuItem>;
}

/** "Ctrl+Shift+S" → "CmdOrCtrl+Shift+S" (the commands' Ctrl stands for Cmd), or none. */
function accelerator(c: Command): string | undefined {
  const key = c.id in MAC_KEYS ? MAC_KEYS[c.id] : c.keys?.[0];
  if (!key) return undefined;
  return SYSTEM_KEYS.includes(key) ? key : key.replace(/^Ctrl\+/, "CmdOrCtrl+");
}

/** The label without its mnemonic; Open recent's entries without their number (see `recentLabel`). */
function label(c: Command): string {
  const text = /^recent-\d/.test(c.id) ? c.label.replace(/^&\d /, "") : plainLabel(c.label);
  // The menu strips mnemonics itself: `&&` keeps a `&` in a project's name.
  return text.replaceAll("&", "&&");
}

/** Without separators (null) at either end or twice in a row. */
function tidy(ids: ReadonlyArray<string | null>): Array<string | null> {
  const out: Array<string | null> = [];
  for (const id of ids) if (id !== null || (out.length > 0 && out[out.length - 1] !== null)) out.push(id);
  if (out[out.length - 1] === null) out.pop();
  return out;
}

const separator = () => PredefinedMenuItem.new({ item: "Separator" });

export class MacMenu {
  private host: MacMenuHost;
  private entries: Entry[] = [];
  private subs: SubEntry[] = [];
  private timer: ReturnType<typeof setTimeout> | undefined;
  /** Building and updates, one after the other. */
  private queue: Promise<void>;

  constructor(defs: readonly MenuDef[], host: MacMenuHost) {
    this.host = host;
    this.queue = this.build(defs).catch(host.error);
    for (const type of ["pointerup", "keyup", "focusin"]) window.addEventListener(type, () => this.refresh(), true);
    // The pointer leaving the window may be on its way to the menu bar.
    document.documentElement.addEventListener("mouseleave", () => this.refresh());
  }

  /** Brings the items' states up to date soon. */
  refresh(): void {
    clearTimeout(this.timer);
    this.timer = setTimeout(() => {
      this.queue = this.queue.then(() => this.update()).catch(this.host.error);
    }, REFRESH_MS);
  }

  private async build(defs: readonly MenuDef[]): Promise<void> {
    const command = this.host.command;
    const app = await Submenu.new({
      text: "Lethora",
      items: [
        await this.track(command("about")),
        await separator(),
        await this.track(command("settings")),
        await separator(),
        await PredefinedMenuItem.new({ item: "Services" }),
        await separator(),
        await PredefinedMenuItem.new({ item: "Hide" }),
        await PredefinedMenuItem.new({ item: "HideOthers" }),
        await PredefinedMenuItem.new({ item: "ShowAll" }),
        await separator(),
        await this.track(command("exit"), "Quit Lethora", "CmdOrCtrl+Q"),
      ],
    });
    const menus = await Promise.all(
      defs.map(async (def) => {
        const ids = tidy(def.items.filter((id) => id === null || !APP_ITEMS.includes(id)));
        return Submenu.new({ text: plainLabel(def.label), items: await Promise.all(ids.map((id) => this.item(id))) });
      }),
    );
    const windowMenu = await Submenu.new({
      text: "Window",
      items: [await PredefinedMenuItem.new({ item: "Minimize" }), await PredefinedMenuItem.new({ item: "Maximize" }), await separator(), await PredefinedMenuItem.new({ item: "Fullscreen" })],
    });
    // Window goes before Help.
    const helpAt = defs.findIndex((def) => plainLabel(def.label) === "Help");
    const help = helpAt >= 0 ? menus[helpAt] : null;
    if (help) menus.splice(helpAt, 0, windowMenu);
    else menus.push(windowMenu);
    const menu = await Menu.new({ items: [app, ...menus] });
    await menu.setAsAppMenu();
    await windowMenu.setAsWindowsMenuForNSApp();
    await help?.setAsHelpMenuForNSApp();
  }

  /** The item for a menu definition's id (null: a separator). */
  private async item(id: string | null): Promise<MenuItem | CheckMenuItem | PredefinedMenuItem | Submenu> {
    if (id === null) return separator();
    if (id in NATIVE) return PredefinedMenuItem.new({ item: NATIVE[id as keyof typeof NATIVE] });
    const c = this.host.command(id);
    if (!c.children) return this.track(c);
    const sub: SubEntry = { command: c, menu: await Submenu.new({ text: label(c), enabled: false }), enabled: false, shape: "", entries: [], items: [] };
    await this.updateSub(sub);
    this.subs.push(sub);
    return sub.menu;
  }

  /** A command's item, kept up to date by `refresh`. */
  private async track(c: Command, text?: string, keys?: string): Promise<MenuItem | CheckMenuItem> {
    const entry = await this.entry(c, text, keys);
    this.entries.push(entry);
    return entry.item;
  }

  private async entry(c: Command, text = label(c), keys = accelerator(c)): Promise<Entry> {
    const enabled = isEnabled(c);
    const checked = c.checked ? c.checked() : null;
    const entry: Entry = { command: c, item: null!, enabled, checked, accelerator: keys };
    const options = { text, enabled, accelerator: keys, action: () => this.activate(entry) };
    entry.item = checked === null ? await MenuItem.new(options) : await CheckMenuItem.new({ ...options, checked });
    return entry;
  }

  private activate(entry: Entry): void {
    // A check item flips its own mark when chosen; the update sets the command's.
    if (entry.checked !== null) entry.checked = !entry.checked;
    this.host.run(entry.command);
    this.refresh();
  }

  private async update(): Promise<void> {
    const calls = this.entries.flatMap((e) => this.sync(e));
    for (const sub of this.subs) await this.updateSub(sub);
    await Promise.all(calls);
  }

  /** Sets what changed of an item's enabled and checked state and its shortcut. */
  private sync(entry: Entry): Array<Promise<void>> {
    const calls: Array<Promise<void>> = [];
    const enabled = isEnabled(entry.command);
    if (enabled !== entry.enabled) {
      entry.enabled = enabled;
      calls.push(entry.item.setEnabled(enabled));
    }
    // `local` keys never change (and Quit has its own).
    const keys = entry.command.local ? entry.accelerator : accelerator(entry.command);
    if (keys !== entry.accelerator) {
      entry.accelerator = keys;
      calls.push(entry.item.setAccelerator(keys ?? null));
    }
    if (entry.checked !== null) {
      const checked = entry.command.checked?.() ?? false;
      if (checked !== entry.checked) {
        entry.checked = checked;
        calls.push((entry.item as CheckMenuItem).setChecked(checked));
      }
    }
    return calls;
  }

  /** A submenu's items from live state: rebuilt when the list changed, else only their states. */
  private async updateSub(sub: SubEntry): Promise<void> {
    const children = sub.command.children!();
    // Greyed out without items, as in the in-row menus.
    const enabled = isEnabled(sub.command) && children.some((c) => c !== null);
    if (enabled !== sub.enabled) {
      sub.enabled = enabled;
      await sub.menu.setEnabled(enabled);
    }
    const shape = JSON.stringify(children.map((c) => c && [c.id, c.label, c.detail]));
    if (shape === sub.shape) {
      // The same items; their commands are new objects (live closures).
      const commands = children.filter((c): c is Command => c !== null);
      sub.entries.forEach((e, i) => (e.command = commands[i]));
      await Promise.all(sub.entries.flatMap((e) => this.sync(e)));
      return;
    }
    sub.shape = shape;
    for (const item of sub.items) await sub.menu.remove(item);
    await Promise.all(sub.items.map((item) => item.close()));
    const entries = await Promise.all(children.map((c) => (c ? this.entry(c) : null)));
    sub.items = await Promise.all(entries.map((e) => e?.item ?? separator()));
    sub.entries = entries.filter((e): e is Entry => e !== null);
    if (sub.items.length) await sub.menu.append(sub.items);
  }
}
