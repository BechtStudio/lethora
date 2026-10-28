// File → Settings… (macOS: Lethora → Settings…): app-wide settings
// (settings.json, see settings.rs) in a view over the whole window, a
// category list with a search field on the left, the category's page on the
// right. The pages are data: categories → sections → rows. Adding a setting
// is one row here plus one field in settings.rs, where its default lives.
// Changes apply at once and are saved; "Restore defaults" sends `null`s.
// The Keyboard shortcuts page is built from the command list; changed keys
// are stored in `shortcuts` (only the changed commands, see `applyShortcuts`).
// The data and the search are pure (tested under `node --test`); the DOM is
// used only by the class.

import { type AppInfo, aboutFacts, diagnosticText } from "./about.ts";
import { type Command, keysFromEvent, keysProblem, plainLabel, sameKeys, showKeys } from "./commands.ts";
import type { MenuDef } from "./menus.ts";
import { availableHeading, type UpdateInfo, upToDate } from "./updates.ts";

/** `settings_get` / `settings_set` (settings.json, see settings.rs). */
export interface AppSettings {
  reopen_last_project: boolean;
  theme: "system" | "light" | "dark";
  /** CSS px. */
  editor_text_size: number;
  /** A multiple of the text size. */
  editor_line_spacing: number;
  /** A blank line's height, em. */
  editor_paragraph_gap: number;
  spell_check: boolean;
  check_updates_on_start: boolean;
  /** Command id → keys, only the commands changed from their defaults (an empty list: none). */
  shortcuts: Record<string, string[]>;
}

export type SettingKey = keyof AppSettings;

/** A select's choice. */
export interface Choice {
  value: string | number;
  label: string;
}

/**
 * A row: a stored setting (`toggle`, `select`: `key` is its field), a part
 * the view draws itself (`custom`: `key` names it), or a command's keys
 * (`shortcut`).
 */
export interface Row {
  kind: "toggle" | "select" | "custom" | "shortcut";
  key: string;
  label: string;
  description: string;
  options?: readonly Choice[];
  /** More words the search finds it by. */
  keywords?: readonly string[];
  /** A shortcut row's keys as stored ("Ctrl+Shift+S"), and its command's defaults. */
  keys?: readonly string[];
  defaults?: readonly string[];
  /** A shortcut row whose keys text fields, the page preview or the system handle: shown, never changed. */
  local?: boolean;
}

export interface Section {
  title: string;
  rows: readonly Row[];
}

export interface Category {
  id: string;
  label: string;
  /** An SVG path (16 × 16, stroked) for the category list. */
  icon: string;
  /** A line under the page title. */
  note?: string;
  sections: readonly Section[];
}

export const CATEGORIES: readonly Category[] = [
  {
    id: "general",
    label: "General",
    icon: "M2.5 4h11M2.5 8h11M2.5 12h11M5.5 2.5v3M10.5 6.5v3M7 10.5v3",
    sections: [
      {
        title: "Startup",
        rows: [{ kind: "toggle", key: "reopen_last_project", label: "Reopen the last project", description: "Open the project you worked on last when Lethora starts.", keywords: ["start", "launch", "recent"] }],
      },
    ],
  },
  {
    id: "appearance",
    label: "Appearance",
    icon: "M13.5 9.5A5.5 5.5 0 1 1 6.5 2.5a4.5 4.5 0 0 0 7 7z",
    sections: [
      {
        title: "Theme",
        rows: [
          {
            kind: "select",
            key: "theme",
            label: "Theme",
            description: "Light or dark, or the same as the system.",
            options: [
              { value: "system", label: "Match system" },
              { value: "light", label: "Light" },
              { value: "dark", label: "Dark" },
            ],
            keywords: ["dark mode", "light mode", "colour", "color"],
          },
        ],
      },
    ],
  },
  {
    id: "editor",
    label: "Editor",
    icon: "M3 13l.6-2.6L11 3l2 2-7.4 7.4L3 13zM9.5 4.5l2 2",
    note: "For the author and science views; the publisher view shows the pages as the layout sets them.",
    sections: [
      {
        title: "Text",
        rows: [
          {
            kind: "select",
            key: "editor_text_size",
            label: "Text size",
            description: "The size of the text you write in.",
            options: [16, 18, 19.5, 21, 24, 28].map((px) => ({ value: px, label: px === 19.5 ? "19.5 px (default)" : `${px} px` })),
            keywords: ["font", "zoom", "bigger", "smaller"],
          },
          {
            kind: "select",
            key: "editor_line_spacing",
            label: "Line spacing",
            description: "The space between the lines of a paragraph.",
            options: [1.3, 1.5, 1.65, 1.8, 2].map((n) => ({ value: n, label: n === 1.65 ? "1.65 (default)" : String(n) })),
            keywords: ["line height", "leading"],
          },
          {
            kind: "select",
            key: "editor_paragraph_gap",
            label: "Space between paragraphs",
            description: "How tall the blank line between two paragraphs is.",
            options: [
              { value: 0.4, label: "Small" },
              { value: 0.8, label: "Default" },
              { value: 1.2, label: "Large" },
              { value: 1.65, label: "A full line" },
            ],
            keywords: ["paragraph gap", "blank line"],
          },
        ],
      },
      {
        title: "Spelling",
        rows: [{ kind: "toggle", key: "spell_check", label: "Check spelling", description: "Underline words the dictionary doesn't know. Also in View → Check spelling.", keywords: ["spell check", "dictionary", "typo"] }],
      },
    ],
  },
  {
    id: "shortcuts",
    label: "Keyboard shortcuts",
    icon: "M1.5 4.5h13v7h-13zM4 7h.5M6.5 7h.5M9 7h.5M11.5 7h.5M5 9.5h6",
    note: "Click Add and press the keys. Text-field keys such as Undo and Copy are fixed.",
    // Filled from the command list (see `shortcutSections`).
    sections: [],
  },
  {
    id: "updates",
    label: "Updates",
    icon: "M13 8a5 5 0 1 1-1.6-3.7M13 2.5v3h-3",
    sections: [
      {
        title: "Automatic",
        rows: [{ kind: "toggle", key: "check_updates_on_start", label: "Check for updates on start", description: "Look for a new version shortly after Lethora starts; it asks before installing. Also in the Help menu.", keywords: ["update", "version", "automatic"] }],
      },
      {
        title: "Manual",
        rows: [{ kind: "custom", key: "update-check", label: "Check for updates now", description: "See whether a newer version is available.", keywords: ["update", "version", "install", "upgrade"] }],
      },
    ],
  },
  {
    id: "about",
    label: "About",
    icon: "M8 2a6 6 0 1 0 0 12A6 6 0 0 0 8 2zM8 7.2v4.3M8 4.8v.2",
    sections: [
      {
        title: "Lethora",
        rows: [
          { kind: "custom", key: "about-facts", label: "Version and system", description: "This copy of Lethora, the system it runs on and the server it is connected to.", keywords: ["version", "build", "protocol", "server", "system", "about"] },
          { kind: "custom", key: "diagnostic", label: "Copy diagnostic info", description: "The lines above as plain text, for a bug report.", keywords: ["bug", "report", "support", "clipboard"] },
        ],
      },
    ],
  },
];

/** Why a `local` command's keys stay. */
const fixedNote = (id: string): string =>
  id.startsWith("zoom-") ? "The page preview handles these keys; they can't change." : id === "exit" ? "The system handles this key; it can't change." : "Text fields handle these keys; they can't change.";

/**
 * The Keyboard shortcuts page's sections: per menu, every command but the
 * submenus, with or without keys. On macOS the rows are also found by their
 * keys as shown there ("⇧⌘S").
 */
export function shortcutSections(menus: readonly MenuDef[], command: (id: string) => Command | undefined, defaults: (id: string) => readonly string[] = () => [], mac = false): Section[] {
  return menus
    .map((menu) => ({
      title: plainLabel(menu.label),
      rows: menu.items
        .map((id) => (id === null ? undefined : command(id)))
        .filter((c): c is Command => !!c && !c.children)
        .map(
          (c): Row => ({
            kind: "shortcut",
            key: c.id,
            label: plainLabel(c.label),
            description: c.local ? fixedNote(c.id) : "",
            keys: c.keys ?? [],
            defaults: defaults(c.id),
            local: c.local,
            keywords: mac ? (c.keys ?? []).map((k) => showKeys(k, true)) : undefined,
          }),
        ),
    }))
    .filter((s) => s.rows.length > 0);
}

/** Whether two key lists are the same keys in the same order. */
const sameList = (a: readonly string[], b: readonly string[]) => a.length === b.length && a.every((k, i) => sameKeys(k, b[i]));

/**
 * The `shortcuts` setting with `row`'s keys set to `keys`, which any other
 * row gives up (a key does one thing). A row back at its defaults is left out.
 */
export function withKeys(stored: Readonly<Record<string, readonly string[]>>, rows: readonly Row[], row: Row, keys: readonly string[]): Record<string, string[]> {
  const out: Record<string, string[]> = Object.fromEntries(Object.entries(stored).map(([id, k]) => [id, [...k]]));
  const set = (r: Row, k: readonly string[]) => {
    if (sameList(k, r.defaults ?? [])) delete out[r.key];
    else out[r.key] = [...k];
  };
  const taken = (k: string) => keys.some((x) => sameKeys(k, x));
  for (const r of rows) if (r.key !== row.key && !r.local && r.keys?.some(taken)) set(r, r.keys.filter((k) => !taken(k)));
  set(row, keys);
  return out;
}

/** The categories with the Keyboard shortcuts page filled in. */
export const withShortcuts = (categories: readonly Category[], shortcuts: readonly Section[]): Category[] => categories.map((c) => (c.id === "shortcuts" ? { ...c, sections: shortcuts } : c));

/** The settings a page stores (what its "Restore defaults" resets); changeable shortcut rows store `shortcuts`. */
export function storedKeys(category: Category): SettingKey[] {
  const rows = category.sections.flatMap((s) => s.rows);
  const keys = rows.filter((r) => r.kind === "toggle" || r.kind === "select").map((r) => r.key as SettingKey);
  if (rows.some((r) => r.kind === "shortcut" && !r.local)) keys.push("shortcuts");
  return keys;
}

/** `settings_set`'s changes that restore a page's defaults. */
export const defaultsOf = (category: Category): Record<string, null> => Object.fromEntries(storedKeys(category).map((key) => [key, null]));

/** A select's choices; a value set elsewhere (e.g. by hand in settings.json) is added so it shows. */
export function choicesFor(row: Row, value: unknown): Choice[] {
  const options = [...(row.options ?? [])];
  if ((typeof value === "string" || typeof value === "number") && !options.some((o) => o.value === value)) options.push({ value, label: String(value) });
  return options;
}

/** A search result: a category's matching rows, by section. */
export interface Found {
  category: Category;
  sections: Section[];
}

/** Whether a row matches every word of `query` (in its label, description, keywords or keys, its section's or category's name). */
export function rowMatches(category: Category, section: Section, row: Row, query: string): boolean {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const text = [category.label, section.title, row.label, row.description, ...(row.keywords ?? []), ...(row.keys ?? [])].join("\n").toLowerCase();
  return words.length > 0 && words.every((w) => text.includes(w));
}

/** The rows matching `query`, grouped by category and section (empty for an empty query). */
export function searchSettings(categories: readonly Category[], query: string): Found[] {
  return categories
    .map((category) => ({
      category,
      sections: category.sections.map((section) => ({ title: section.title, rows: section.rows.filter((row) => rowMatches(category, section, row, query)) })).filter((s) => s.rows.length > 0),
    }))
    .filter((f) => f.sections.length > 0);
}

/** The Manual updates row's line: the installed version. */
export const versionLine = (version: string): string => `You have version ${version}.`;

const byId = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

function el<K extends keyof HTMLElementTagNameMap>(tag: K, props: Partial<HTMLElementTagNameMap[K]> = {}, ...children: Array<Node | string>): HTMLElementTagNameMap[K] {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...children);
  return e;
}

const SVG = "http://www.w3.org/2000/svg";

function icon(path: string): SVGSVGElement {
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("viewBox", "0 0 16 16");
  svg.setAttribute("aria-hidden", "true");
  const p = document.createElementNS(SVG, "path");
  p.setAttribute("d", path);
  svg.append(p);
  return svg;
}

export interface SettingsHost {
  /** The settings as last read or saved (null: not read yet). */
  settings: () => AppSettings | null;
  /** Saves changes (`null`: the default) and applies them. */
  change: (changes: Record<string, unknown>) => Promise<void>;
  /** The Keyboard shortcuts page (the commands' keys as they are now). */
  shortcuts: () => Section[];
  /** Keys are shown the Mac way. */
  mac: boolean;
  appInfo: () => Promise<AppInfo>;
  /** `update_check` (manual). */
  checkUpdate: () => Promise<UpdateInfo | null>;
  /** Hands a found update to the update dialog (notes, Install and restart). */
  install: (info: UpdateInfo) => void;
}

/** What a custom row adds: its control on the right, something under it. */
interface Custom {
  control?: Node;
  below?: Node;
  /** Replaces the row's description. */
  description?: Promise<string>;
}

export class SettingsView {
  private dialog = byId<HTMLDialogElement>("settings-dialog");
  private host: SettingsHost;
  private search: HTMLInputElement;
  private tabs: HTMLElement;
  private page: HTMLElement;
  private categories: Category[] = [...CATEGORIES];
  private current = "general";
  /** The shown stored settings' controls, brought up to date by `refresh`. */
  private controls = new Map<string, HTMLInputElement | HTMLSelectElement>();
  private error = el("p", { className: "st-error", role: "alert", hidden: true });
  /** `app_info` for this opening (the server may have changed since the last). */
  private info: Promise<AppInfo> | null = null;
  /** Ends recording a shortcut (null: not recording). */
  private stopRecording: (() => void) | null = null;

  constructor(host: SettingsHost) {
    this.host = host;
    const close = el("button", { type: "button", className: "st-close", title: "Close (Esc)", textContent: "×" });
    close.setAttribute("aria-label", "Close settings");
    close.addEventListener("click", () => this.dialog.close());
    const title = el("h1", { id: "settings-heading", textContent: "Settings" });
    const head = el("div", { className: "st-head" }, title, close);
    head.setAttribute("data-tauri-drag-region", "");
    title.setAttribute("data-tauri-drag-region", "");

    this.search = el("input", { type: "search", className: "st-search", placeholder: "Search settings", autocomplete: "off", spellcheck: false });
    this.search.setAttribute("aria-label", "Search settings");
    this.search.addEventListener("input", () => this.render());
    // Escape clears a search first, then closes.
    this.search.addEventListener("keydown", (e) => {
      if (e.key === "Escape" && this.search.value) {
        e.preventDefault();
        this.search.value = "";
        this.render();
      } else if (e.key === "ArrowDown") {
        e.preventDefault();
        this.tabs.querySelector<HTMLElement>("[tabindex='0']")?.focus();
      }
    });
    this.tabs = el("div", { className: "st-tabs" });
    this.tabs.setAttribute("role", "tablist");
    this.tabs.setAttribute("aria-orientation", "vertical");
    this.tabs.setAttribute("aria-label", "Categories");
    this.tabs.addEventListener("keydown", (e) => this.tabKey(e));
    this.page = el("div", { id: "st-page", className: "st-page", tabIndex: -1 });
    this.page.setAttribute("role", "tabpanel");
    const body = el("div", { className: "st-body" }, el("nav", { className: "st-nav" }, this.search, this.tabs), this.page);
    this.dialog.replaceChildren(head, body);
  }

  get isOpen(): boolean {
    return this.dialog.open;
  }

  /** Recording a shortcut: the window-wide keys are left alone (main.ts). */
  get recording(): boolean {
    return this.stopRecording !== null;
  }

  /** Opens on a category (else the last shown), with the focus in the search field. */
  open(category?: string): void {
    this.categories = withShortcuts(CATEGORIES, this.host.shortcuts());
    if (category) this.current = category;
    this.info = null;
    this.search.value = "";
    this.error.hidden = true;
    this.render();
    if (!this.dialog.open) this.dialog.showModal();
    this.search.focus();
  }

  /** Brings the shown controls up to date (after a change from anywhere). */
  refresh(): void {
    const s = this.host.settings();
    if (!s) return;
    for (const [key, control] of this.controls) {
      const value = s[key as SettingKey];
      if (control instanceof HTMLInputElement) control.checked = value === true;
      else control.value = String(choicesFor(this.rowOf(key)!, value).findIndex((c) => c.value === value));
    }
  }

  private rowOf(key: string): Row | undefined {
    for (const c of this.categories) for (const s of c.sections) for (const r of s.rows) if (r.key === key) return r;
    return undefined;
  }

  private get appInfo(): Promise<AppInfo> {
    this.info ??= this.host.appInfo();
    return this.info;
  }

  /** Arrow keys, Home and End move between the categories (and show them). */
  private tabKey(e: KeyboardEvent): void {
    const tabs = [...this.tabs.querySelectorAll<HTMLButtonElement>("[role=tab]")];
    const at = tabs.indexOf(document.activeElement as HTMLButtonElement);
    if (at < 0) return;
    const next = e.key === "ArrowDown" || e.key === "ArrowRight" ? (at + 1) % tabs.length : e.key === "ArrowUp" || e.key === "ArrowLeft" ? (at - 1 + tabs.length) % tabs.length : e.key === "Home" ? 0 : e.key === "End" ? tabs.length - 1 : -1;
    if (next < 0) return;
    e.preventDefault();
    this.show(tabs[next].dataset.category!);
    this.tabs.querySelector<HTMLElement>("[tabindex='0']")?.focus();
  }

  /** Shows a category's page (and ends a search). */
  private show(id: string): void {
    this.current = id;
    this.search.value = "";
    this.render();
  }

  private render(): void {
    this.stopRecording?.();
    const query = this.search.value.trim();
    this.tabs.replaceChildren(
      ...this.categories.map((c) => {
        // The current category stays the one Tab reaches, also while searching.
        const selected = !query && c.id === this.current;
        const tab = el("button", { type: "button", id: `st-tab-${c.id}`, className: "st-tab", tabIndex: c.id === this.current ? 0 : -1 }, icon(c.icon), el("span", { textContent: c.label }));
        tab.dataset.category = c.id;
        tab.setAttribute("role", "tab");
        tab.setAttribute("aria-selected", String(selected));
        tab.setAttribute("aria-controls", "st-page");
        tab.addEventListener("click", () => this.show(c.id));
        return tab;
      }),
    );
    this.controls.clear();
    const column = el("div", { className: "st-column" });
    if (query) {
      this.page.removeAttribute("aria-labelledby");
      const found = searchSettings(this.categories, query);
      column.append(el("h2", { className: "st-title", textContent: "Search results" }), this.error);
      if (found.length === 0) column.append(el("p", { className: "st-empty", role: "status", textContent: "No settings match." }));
      for (const f of found) {
        const go = el("button", { type: "button", className: "st-found", textContent: f.category.label, title: `Show ${f.category.label}` });
        go.addEventListener("click", () => this.show(f.category.id));
        column.append(el("h3", { className: "st-found-head" }, go), ...f.sections.map((s) => this.section(s)));
      }
    } else {
      const category = this.categories.find((c) => c.id === this.current) ?? this.categories[0];
      this.page.setAttribute("aria-labelledby", `st-tab-${category.id}`);
      const head = el("div", { className: "st-page-head" }, el("h2", { className: "st-title", textContent: category.label }));
      if (storedKeys(category).length > 0) {
        const reset = el("button", { type: "button", id: "st-restore", className: "st-button", textContent: "Restore defaults" });
        reset.addEventListener("click", () => this.change(defaultsOf(category), reset.id));
        head.append(reset);
      }
      column.append(head);
      if (category.note) column.append(el("p", { className: "st-note", textContent: category.note }));
      column.append(this.error, ...category.sections.map((s) => this.section(s)));
    }
    this.page.replaceChildren(column);
    this.refresh();
  }

  private section(section: Section): HTMLElement {
    return el("section", { className: "st-section" }, el("h3", { className: "st-section-title", textContent: section.title }), el("div", { className: "st-rows" }, ...section.rows.map((r) => this.row(r))));
  }

  private row(row: Row): HTMLElement {
    // Command ids may equal a setting's (Help's "update-check").
    const id = row.kind === "shortcut" ? `st-keys-${row.key}` : `st-${row.key}`;
    // Only a stored setting's label belongs to a control.
    const stored = row.kind === "toggle" || row.kind === "select";
    const label = stored ? el("label", { className: "st-label", htmlFor: id, textContent: row.label }) : el("span", { className: "st-label", textContent: row.label });
    const description = el("p", { className: "st-description", id: `${id}-description`, textContent: row.description });
    const text = el("div", { className: "st-text" }, label);
    if (row.description) text.append(description);
    const control = el("div", { className: "st-control" });
    const line = el("div", { className: "st-row" }, text, control);
    if (row.kind === "toggle") {
      const input = el("input", { type: "checkbox", id, className: "lp-switch" });
      input.setAttribute("role", "switch");
      input.setAttribute("aria-describedby", description.id);
      input.addEventListener("change", () => this.change({ [row.key]: input.checked }));
      this.controls.set(row.key, input);
      control.append(input);
    } else if (row.kind === "select") {
      const choices = choicesFor(row, this.host.settings()?.[row.key as SettingKey]);
      const select = el("select", { id, className: "st-select" }, ...choices.map((c, i) => el("option", { value: String(i), textContent: c.label })));
      select.setAttribute("aria-describedby", description.id);
      select.addEventListener("change", () => this.change({ [row.key]: choices[Number(select.value)].value }));
      this.controls.set(row.key, select);
      control.append(select);
    } else if (row.kind === "shortcut") {
      line.classList.add("st-shortcut");
      return this.shortcut(row, line, control);
    } else {
      const custom = this.custom(row.key);
      if (custom.control) control.append(custom.control);
      custom.description?.then((text) => (description.textContent = text)).catch(() => undefined);
      if (custom.below) return el("div", { className: "st-block" }, line, custom.below);
    }
    return line;
  }

  /** Every shortcut row (the Keyboard shortcuts page's). */
  private shortcutRows(): Row[] {
    return this.categories.flatMap((c) => c.sections.flatMap((s) => s.rows.filter((r) => r.kind === "shortcut")));
  }

  /** A command's keys, each with × to remove it, Add, and Reset while they differ from the defaults; `local` ones are only shown. */
  private shortcut(row: Row, line: HTMLElement, control: HTMLElement): HTMLElement {
    const mac = this.host.mac;
    const keys = row.keys ?? [];
    if (row.local) {
      control.append(...keys.map((k) => el("kbd", { textContent: showKeys(k, mac) })));
      return line;
    }
    const message = el("div", { className: "st-status", role: "status" });
    const add = el("button", { type: "button", id: `st-add-${row.key}`, className: "st-button st-add", textContent: "Add" });
    const change = (next: readonly string[]) => this.change({ shortcuts: withKeys(this.host.settings()?.shortcuts ?? {}, this.shortcutRows(), row, next) }, add.id);
    for (const k of keys) {
      const remove = el("button", { type: "button", className: "st-chip-remove", textContent: "×", title: "Remove" });
      remove.setAttribute("aria-label", `Remove ${showKeys(k, mac)} from ${row.label}`);
      remove.addEventListener("click", () => change(keys.filter((x) => x !== k)));
      control.append(el("span", { className: "st-chip" }, el("kbd", { textContent: showKeys(k, mac) }), remove));
    }
    add.setAttribute("aria-label", `Add a shortcut to ${row.label}`);
    add.addEventListener("click", () => this.record(row, add, message, (k) => change([...keys, k])));
    control.append(add);
    if (!sameList(keys, row.defaults ?? [])) {
      const back = row.defaults?.length ? row.defaults.map((k) => showKeys(k, mac)).join(", ") : "no shortcut";
      const reset = el("button", { type: "button", className: "st-button", textContent: "Reset", title: `Back to ${back}` });
      reset.setAttribute("aria-label", `Reset ${row.label}`);
      reset.addEventListener("click", () => change(row.defaults ?? []));
      control.append(reset);
    }
    return el("div", { className: "st-block" }, line, message);
  }

  /**
   * Add: the next keys pressed (not a modifier alone) go to `row` through
   * `take`, after a check; another command's keys ask first (Replace takes
   * them). Esc, Tab and leaving the button cancel.
   */
  private record(row: Row, add: HTMLButtonElement, message: HTMLElement, take: (keys: string) => void): void {
    this.stopRecording?.();
    const mac = this.host.mac;
    const label = add.getAttribute("aria-label") ?? "";
    message.className = "st-status";
    message.replaceChildren();
    add.textContent = "Press keys… (Esc cancels)";
    add.setAttribute("aria-label", `Press the keys for ${row.label}, Escape cancels`);
    add.classList.add("st-recording");
    add.focus();
    const stop = () => {
      add.removeEventListener("keydown", key);
      add.removeEventListener("blur", stop);
      add.textContent = "Add";
      add.setAttribute("aria-label", label);
      add.classList.remove("st-recording");
      this.stopRecording = null;
    };
    const key = (e: KeyboardEvent) => {
      // Tab moves on.
      if (e.key === "Tab" && !e.ctrlKey && !e.metaKey && !e.altKey) return stop();
      // Also keeps Escape from closing the dialog.
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape" && !e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey) return stop();
      const keys = keysFromEvent(e);
      if (!keys) return;
      stop();
      if (row.keys?.some((k) => sameKeys(k, keys))) return;
      const rows = this.shortcutRows();
      const problem = keysProblem(keys, rows, mac);
      if (problem) {
        message.className = "st-status st-status-error";
        message.textContent = problem;
        return;
      }
      const owner = rows.find((r) => r.key !== row.key && r.keys?.some((k) => sameKeys(k, keys)));
      if (!owner) return take(keys);
      const replace = el("button", { type: "button", className: "st-button", textContent: "Replace" });
      replace.setAttribute("aria-label", `Use ${showKeys(keys, mac)} for ${row.label} instead of ${owner.label}`);
      replace.addEventListener("click", () => take(keys));
      const cancel = el("button", { type: "button", className: "st-button", textContent: "Cancel" });
      cancel.addEventListener("click", () => {
        message.replaceChildren();
        add.focus();
      });
      message.replaceChildren(el("span", { textContent: `${showKeys(keys, mac)} is used by ${owner.label}.` }), replace, cancel);
      replace.focus();
    };
    add.addEventListener("keydown", key);
    add.addEventListener("blur", stop);
    this.stopRecording = stop;
  }

  /** The rows the view draws itself (Updates and About). */
  private custom(key: string): Custom {
    if (key === "update-check") {
      const status = el("div", { className: "st-status", role: "status" });
      const check = el("button", { type: "button", className: "st-button", textContent: "Check now" });
      check.addEventListener("click", async () => {
        check.disabled = true;
        status.className = "st-status";
        status.replaceChildren("Checking for updates…");
        try {
          const found = await this.host.checkUpdate();
          if (found) {
            const install = el("button", { type: "button", className: "st-button primary", textContent: "Install and restart…" });
            install.addEventListener("click", () => this.host.install(found));
            status.replaceChildren(el("span", { textContent: `${availableHeading(found)}.` }), install);
          } else status.replaceChildren(upToDate((await this.appInfo).version));
        } catch (e) {
          status.className = "st-status st-status-error";
          status.replaceChildren(String(e));
        } finally {
          check.disabled = false;
        }
      });
      return { control: check, below: status, description: this.appInfo.then((i) => versionLine(i.version)) };
    }
    if (key === "about-facts") {
      const facts = el("dl", { className: "sd-facts st-facts" });
      this.appInfo.then(
        (info) => facts.replaceChildren(...aboutFacts(info).flatMap(([label, value]) => [el("dt", { textContent: label }), el("dd", { textContent: value, title: value })])),
        (e) => facts.replaceChildren(el("dd", { className: "st-status-error", textContent: String(e) })),
      );
      return { below: facts };
    }
    if (key === "diagnostic") {
      const note = el("p", { className: "st-status", role: "status" });
      const copy = el("button", { type: "button", className: "st-button", textContent: "Copy" });
      copy.setAttribute("aria-label", "Copy diagnostic info");
      copy.addEventListener("click", async () => {
        try {
          await navigator.clipboard.writeText(diagnosticText(await this.appInfo));
          note.textContent = "Copied. Paste it into your bug report.";
        } catch {
          note.textContent = "Could not copy. Select the lines above and copy them by hand.";
        }
      });
      return { control: copy, below: note };
    }
    return {};
  }

  /** Saves changes; new shortcuts draw the page again, with the focus on `focus` (an id). */
  private change(changes: Record<string, unknown>, focus?: string): void {
    this.error.hidden = true;
    this.host.change(changes).then(
      () => {
        if (!("shortcuts" in changes)) return this.refresh();
        this.categories = withShortcuts(CATEGORIES, this.host.shortcuts());
        this.render();
        if (focus) document.getElementById(focus)?.focus();
      },
      (e) => {
        this.error.textContent = String(e);
        this.error.hidden = false;
        this.refresh();
      },
    );
  }
}
