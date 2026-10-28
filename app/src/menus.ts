// The menu bar (File, Edit, Format, View, Help): VS Code style dropdowns
// built from the command list (commands.ts). A menu is rendered each time it
// opens, so its enabled and checked states are current.
//
// Mouse: a click opens or closes a menu, hovering switches menus once one is
// open, a click outside closes. Keyboard: Alt alone or F10 focuses the bar
// (mnemonics underlined while Alt is held or the bar has the keyboard),
// Alt+letter opens a menu, arrows move (Left/Right between menus), Home/End,
// Enter/Space run, a mnemonic letter runs its item, Esc and Tab close. On
// closing, the focus goes back to where it was before (so Cut, Copy and
// Paste act there).
//
// Submenus (a command with `children`, e.g. Open recent): hovering opens one
// after a short delay, a click at once; Right, Enter, Space or its mnemonic
// open it with the first item focused, Left and Esc go back to its item.

import { type Command, isEnabled, mnemonic, plainLabel } from "./commands.ts";

/** A menu: its label (`&` marks the mnemonic) and command ids; null is a separator. */
export interface MenuDef {
  label: string;
  items: ReadonlyArray<string | null>;
}

interface MenuHost {
  command: (id: string) => Command;
  run: (c: Command) => void;
  /** Gives the focus back to an element (the editor needs its own call). */
  refocus: (el: HTMLElement) => void;
}

interface Built {
  def: MenuDef;
  top: HTMLButtonElement;
  drop: HTMLElement;
}

/** An open submenu and the item it belongs to. */
interface Sub {
  parent: HTMLElement;
  drop: HTMLElement;
}

/** How long the mouse rests on an item before its submenu opens (or another one closes). */
const SUB_DELAY_MS = 250;

/** A label with its mnemonic letter wrapped (underlined while mnemonics show). */
function labelNodes(label: string): Node[] {
  const i = label.indexOf("&");
  if (i < 0 || i + 1 >= label.length) return [document.createTextNode(label)];
  const letter = document.createElement("span");
  letter.className = "mb-mn";
  letter.textContent = label[i + 1];
  return [document.createTextNode(label.slice(0, i)), letter, document.createTextNode(label.slice(i + 2))];
}

function span(className: string, text = ""): HTMLSpanElement {
  const s = document.createElement("span");
  s.className = className;
  s.textContent = text;
  return s;
}

const hasModifier = (e: KeyboardEvent) => e.ctrlKey || e.metaKey || e.altKey || e.shiftKey;

export class MenuBar {
  private el: HTMLElement;
  private host: MenuHost;
  private menus: Built[] = [];
  /** The open menu, or -1. */
  private openIndex = -1;
  /** The menu the keyboard is on (open or focused in the bar). */
  private current = 0;
  /** Whether the bar has the focus (a menu open or a menu name focused). */
  private engaged = false;
  /** Engaged from the keyboard: mnemonics stay underlined. */
  private viaKeyboard = false;
  /** Where the focus goes back to. */
  private returnTo: HTMLElement | null = null;
  /** Alt is down with no other key or click since: its release focuses the bar. */
  private altAlone = false;
  /** The open submenu, if any. */
  private sub: Sub | null = null;
  private subTimer: ReturnType<typeof setTimeout> | undefined;
  /** The commands of the open menu and its submenu, by id. */
  private shown = new Map<string, Command>();

  constructor(el: HTMLElement, defs: readonly MenuDef[], host: MenuHost) {
    this.el = el;
    this.host = host;
    defs.forEach((def, i) => {
      const wrap = document.createElement("div");
      wrap.className = "mb-menu";
      const top = document.createElement("button");
      top.type = "button";
      top.id = `mb-top-${i}`;
      top.className = "mb-top";
      top.tabIndex = i === 0 ? 0 : -1;
      top.setAttribute("role", "menuitem");
      top.setAttribute("aria-haspopup", "menu");
      top.setAttribute("aria-expanded", "false");
      top.append(...labelNodes(def.label));
      const drop = document.createElement("div");
      drop.className = "mb-drop";
      drop.hidden = true;
      drop.tabIndex = -1;
      drop.setAttribute("role", "menu");
      drop.setAttribute("aria-labelledby", top.id);
      wrap.append(top, drop);
      el.append(wrap);
      this.menus.push({ def, top, drop });
      // The focus stays where it is until the menu takes it (it is remembered first).
      top.addEventListener("mousedown", (e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        if (this.openIndex === i) this.disengage(true);
        else this.open(i, null);
      });
      top.addEventListener("mouseenter", () => {
        if (this.openIndex >= 0 && this.openIndex !== i) this.open(i, null);
      });
    });
    // Tabbing into the bar.
    el.addEventListener("focusin", (e) => {
      if (this.engaged) return;
      this.engaged = true;
      const from = e.relatedTarget;
      this.returnTo = from instanceof HTMLElement && !el.contains(from) ? from : null;
    });
    el.addEventListener("focusout", (e) => {
      const to = e.relatedTarget;
      if (to instanceof Node && !el.contains(to)) this.disengage(false);
    });
    window.addEventListener(
      "pointerdown",
      (e) => {
        this.altAlone = false;
        if (this.engaged && !el.contains(e.target as Node)) this.disengage(true);
      },
      true,
    );
    window.addEventListener("blur", () => {
      this.altAlone = false;
      this.disengage(true);
    });
  }

  /** Whether the bar has the focus (a menu open or a menu name focused). */
  get isOpen(): boolean {
    return this.engaged;
  }

  /** Where the focus goes back to when the bar lets go (null when not engaged). */
  get returnTarget(): HTMLElement | null {
    return this.engaged ? this.returnTo : null;
  }

  /**
   * A window keydown, before the shortcuts and the Escape chain; true if the
   * menus took it (the caller stops it).
   */
  key(e: KeyboardEvent): boolean {
    if (e.key === "Alt") {
      this.altAlone = !e.ctrlKey && !e.metaKey && !e.shiftKey;
      if (this.altAlone) this.showMnemonics(true);
      return false;
    }
    this.altAlone = false;
    if (!this.engaged) {
      if (e.key === "F10" && !hasModifier(e)) {
        this.focusTop(0);
        return true;
      }
      // Not AltGr (Ctrl+Alt).
      if (e.altKey && !e.ctrlKey && !e.metaKey && !e.shiftKey) {
        const i = this.menuFor(e.key);
        if (i < 0) return false;
        this.viaKeyboard = true;
        this.showMnemonics(true);
        this.open(i, "first");
        return true;
      }
      return false;
    }
    const n = this.menus.length;
    const open = this.openIndex;
    const focused = this.items().find((it) => it === document.activeElement);
    switch (e.key) {
      case "Escape":
        if (this.sub) this.closeSub(true);
        else this.disengage(true);
        return true;
      case "Tab":
      case "F10":
        this.disengage(true);
        return true;
      case "ArrowLeft":
      case "ArrowRight": {
        if (e.key === "ArrowLeft" && this.inSub()) {
          this.closeSub(true);
          return true;
        }
        if (e.key === "ArrowRight" && focused && this.shown.get(focused.dataset.command!)?.children) {
          this.choose(focused);
          return true;
        }
        const next = (this.current + (e.key === "ArrowRight" ? 1 : -1) + n) % n;
        if (open >= 0) this.open(next, "first");
        else this.focusTop(next);
        return true;
      }
      case "ArrowDown":
      case "ArrowUp":
        if (open < 0) this.open(this.current, e.key === "ArrowDown" ? "first" : "last");
        else this.moveItem(e.key === "ArrowDown" ? 1 : -1);
        return true;
      case "Home":
      case "End":
        if (open < 0) this.focusTop(e.key === "Home" ? 0 : n - 1);
        else this.focusItem(e.key === "Home" ? "first" : "last");
        return true;
      case "Enter":
      case " ": {
        if (open < 0) {
          this.open(this.current, "first");
          return true;
        }
        if (focused) this.choose(focused);
        return true;
      }
    }
    // Shortcuts still work: the bar lets go first.
    if (e.ctrlKey || e.metaKey) {
      this.disengage(true);
      return false;
    }
    if (e.key.length === 1) {
      const letter = e.key.toLowerCase();
      if (open < 0 || e.altKey) {
        const i = this.menuFor(letter);
        if (i >= 0) this.open(i, "first");
      } else {
        const item = this.items().find((it) => it.dataset.mnemonic === letter);
        if (item) this.choose(item);
      }
    }
    // Nothing else reaches the page while the bar has the keyboard.
    return true;
  }

  /** A window keyup: releasing Alt alone focuses the bar (or lets it go); true if taken. */
  keyUp(e: KeyboardEvent): boolean {
    if (e.key !== "Alt") return false;
    const alone = this.altAlone;
    this.altAlone = false;
    if (!alone) {
      if (!(this.engaged && this.viaKeyboard)) this.showMnemonics(false);
      return false;
    }
    if (this.engaged) this.disengage(true);
    else this.focusTop(0);
    return true;
  }

  /** Opens menu `i`, focusing its first or last item (or the menu itself, for the mouse). */
  private open(i: number, focus: "first" | "last" | null): void {
    this.engage();
    if (this.openIndex >= 0 && this.openIndex !== i) this.hide(this.openIndex);
    const m = this.menus[i];
    this.render(m);
    m.drop.hidden = false;
    m.top.setAttribute("aria-expanded", "true");
    this.openIndex = i;
    this.setCurrent(i);
    if (focus) this.focusItem(focus);
    else m.drop.focus({ preventScroll: true });
  }

  private hide(i: number): void {
    this.closeSub(false);
    const m = this.menus[i];
    m.drop.hidden = true;
    m.drop.replaceChildren();
    m.top.setAttribute("aria-expanded", "false");
    if (this.openIndex === i) this.openIndex = -1;
  }

  /** Focuses menu name `i` in the bar (closing an open menu). */
  private focusTop(i: number): void {
    this.engage();
    this.viaKeyboard = true;
    this.showMnemonics(true);
    if (this.openIndex >= 0) this.hide(this.openIndex);
    this.setCurrent(i);
    this.menus[i].top.focus({ preventScroll: true });
  }

  /** Remembers where the focus was, the first time the bar takes it. */
  private engage(): void {
    if (this.engaged) return;
    const active = document.activeElement;
    this.returnTo = active instanceof HTMLElement && active !== document.body && !this.el.contains(active) ? active : null;
    this.engaged = true;
  }

  /** Closes the menus and lets go of the focus (back to where it was if `restore`). */
  private disengage(restore: boolean): void {
    if (!this.engaged) return;
    if (this.openIndex >= 0) this.hide(this.openIndex);
    this.engaged = false;
    this.viaKeyboard = false;
    this.showMnemonics(false);
    const to = this.returnTo;
    this.returnTo = null;
    if (restore && to?.isConnected) this.host.refocus(to);
    else if (restore && this.el.contains(document.activeElement)) (document.activeElement as HTMLElement).blur();
  }

  /** Runs a command: the focus goes back first. */
  private activate(c: Command): void {
    if (!isEnabled(c)) return;
    this.disengage(true);
    this.host.run(c);
  }

  /** An item chosen from the keyboard: opens its submenu (first item focused) or runs it. */
  private choose(item: HTMLElement): void {
    const c = this.shown.get(item.dataset.command!);
    if (!c) return;
    if (c.children) this.openSub(item, c, true);
    else this.activate(c);
  }

  /** Whether the focus is in the open submenu. */
  private inSub(): boolean {
    return this.sub !== null && this.sub.drop.contains(document.activeElement);
  }

  /** Opens the submenu of item `parent` (command `c`) beside it. */
  private openSub(parent: HTMLElement, c: Command, focusFirst: boolean): void {
    clearTimeout(this.subTimer);
    if (this.sub?.parent !== parent) {
      this.closeSub(false);
      const drop = document.createElement("div");
      drop.className = "mb-drop mb-sub";
      drop.tabIndex = -1;
      drop.setAttribute("role", "menu");
      drop.setAttribute("aria-label", plainLabel(c.label));
      drop.append(...this.entries(c.children?.() ?? []));
      // Level with its item (the menu's padding and border above).
      drop.style.top = `calc(${parent.offsetTop}px - 0.3rem - 1px)`;
      drop.addEventListener("mouseenter", () => clearTimeout(this.subTimer));
      parent.after(drop);
      parent.setAttribute("aria-expanded", "true");
      this.sub = { parent, drop };
    }
    if (focusFirst) this.focusItem("first");
  }

  /** Closes the open submenu (the focus back on its item if `focusParent`). */
  private closeSub(focusParent: boolean): void {
    clearTimeout(this.subTimer);
    const sub = this.sub;
    if (!sub) return;
    this.sub = null;
    if (focusParent) sub.parent.focus({ preventScroll: true });
    sub.drop.remove();
    sub.parent.setAttribute("aria-expanded", "false");
  }

  /** The mouse entered item `el`: its submenu opens, or another one closes, after a rest. */
  private hover(el: HTMLElement, c: Command): void {
    clearTimeout(this.subTimer);
    if (this.sub?.drop.contains(el) || this.sub?.parent === el) return;
    if (c.children && el.getAttribute("aria-disabled") !== "true") this.subTimer = setTimeout(() => this.openSub(el, c, false), SUB_DELAY_MS);
    else if (this.sub) this.subTimer = setTimeout(() => this.closeSub(false), SUB_DELAY_MS);
  }

  /** The bar's tab stop follows the current menu. */
  private setCurrent(i: number): void {
    this.current = i;
    this.menus.forEach((m, j) => (m.top.tabIndex = j === i ? 0 : -1));
  }

  private menuFor(key: string): number {
    const letter = key.toLowerCase();
    return this.menus.findIndex((m) => mnemonic(m.def.label) === letter);
  }

  /** The enabled items of the open menu, or of its submenu while that has the focus. */
  private items(): HTMLElement[] {
    if (this.openIndex < 0) return [];
    const list = this.inSub() ? this.sub!.drop : this.menus[this.openIndex].drop;
    return [...list.querySelectorAll<HTMLElement>(":scope > .mb-item:not([aria-disabled='true'])")];
  }

  private focusItem(which: "first" | "last"): void {
    const items = this.items();
    const item = which === "first" ? items[0] : items[items.length - 1];
    (item ?? this.menus[this.openIndex].drop).focus({ preventScroll: true });
  }

  private moveItem(step: 1 | -1): void {
    const items = this.items();
    if (!items.length) return;
    const at = items.indexOf(document.activeElement as HTMLElement);
    const next = at < 0 ? (step > 0 ? 0 : items.length - 1) : (at + step + items.length) % items.length;
    items[next].focus({ preventScroll: true });
    // Moving on in the menu closes a submenu the mouse opened.
    if (this.sub && !this.inSub() && items[next] !== this.sub.parent) this.closeSub(false);
  }

  private showMnemonics(on: boolean): void {
    this.el.classList.toggle("mnemonics", on);
  }

  private render(m: Built): void {
    this.shown.clear();
    m.drop.replaceChildren(...this.entries(m.def.items.map((id) => (id === null ? null : this.host.command(id)))));
  }

  /** Items and separators (null), remembered by id. */
  private entries(commands: ReadonlyArray<Command | null>): HTMLElement[] {
    return commands.map((c) => {
      if (c === null) {
        const sep = document.createElement("div");
        sep.className = "mb-sep";
        sep.setAttribute("role", "separator");
        return sep;
      }
      this.shown.set(c.id, c);
      return this.item(c);
    });
  }

  private item(c: Command): HTMLElement {
    const el = document.createElement("div");
    el.className = "mb-item";
    el.tabIndex = -1;
    el.dataset.command = c.id;
    el.dataset.mnemonic = mnemonic(c.label);
    const checked = c.checked?.() ?? false;
    el.setAttribute("role", c.checked ? (c.radio ? "menuitemradio" : "menuitemcheckbox") : "menuitem");
    if (c.checked) el.setAttribute("aria-checked", String(checked));
    // A submenu without items is greyed out.
    const enabled = isEnabled(c) && (!c.children || c.children().some((child) => child !== null));
    if (!enabled) el.setAttribute("aria-disabled", "true");
    if (c.keys?.length) el.setAttribute("aria-keyshortcuts", c.keys.map((k) => k.replace("Ctrl", "Control")).join(" "));
    if (c.children) {
      el.setAttribute("aria-haspopup", "menu");
      el.setAttribute("aria-expanded", "false");
    }
    const label = span("mb-label");
    label.append(...labelNodes(c.label));
    const end = c.children ? span("mb-keys mb-arrow", "›") : c.detail ? span("mb-keys mb-detail", c.detail) : span("mb-keys", c.keys?.[0] ?? "");
    if (c.detail) el.title = c.detail;
    el.append(span("mb-check", checked ? (c.radio ? "•" : "✓") : ""), label, end);
    // The focus stays on the item until it runs.
    el.addEventListener("mousedown", (e) => e.preventDefault());
    el.addEventListener("mouseenter", () => this.hover(el, c));
    if (enabled) {
      el.addEventListener("mousemove", () => {
        if (document.activeElement !== el) el.focus({ preventScroll: true });
      });
      el.addEventListener("click", () => (c.children ? this.openSub(el, c, false) : this.activate(c)));
    }
    return el;
  }
}
