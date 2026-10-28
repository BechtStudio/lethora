// The layout panel of the publisher view: preset, grouped settings, metadata
// and expert Typst code. Values come from `layout_get`; every change goes to
// the core (validated there) and the panel re-renders from its answer.
//
// No Tauri imports: commands come in through `LayoutHost`. The field table
// and value mapping are pure and tested under `node --test`.

import { ComboBox, cssFamily, type FontFaces, FontPicker } from "./fontpicker.ts";
import { deleteChanges, newNamedStyle, parseSize, sameSettings, SIZES, sizeLabel, styleChanges, type StyleEntry, styleEntries, textStylesOf, unusedStyles } from "./styles.ts";

/**
 * A character style (stored as `styles.<id>.<field>`): at least a font or a
 * size (absolute, like `12pt`); `name` only on named styles.
 */
export interface CharStyle {
  name?: string;
  font?: string;
  size?: string;
}

/** What `layout_get` (and every layout command) returns. */
export interface LayoutState {
  preset: string;
  /** Resolved settings by section (`layout.page.size`), plus `schema` and the character styles by id. */
  layout: Record<string, unknown> & { styles: Record<string, CharStyle> };
  /** Settings that differ from the preset, by dotted key. */
  overrides: Record<string, unknown>;
  meta: Record<string, string>;
  custom: string;
  /** The text area in points (one column's width); the science view sizes figures against it. */
  text_area: { column_width_pt: number; height_pt: number; font_size_pt: number };
}

export interface PresetInfo {
  id: string;
  name: string;
  description: string;
}

export interface StyleInfo {
  id: string;
  name: string;
}

type Choice = readonly [value: string | number, label: string];

interface Base {
  /** Dotted layout key, or `meta.<field>`. */
  key: string;
  label: string;
  /** Shown only when this holds (e.g. custom page size). */
  when?: (s: LayoutState) => boolean;
  title?: string;
}

export type Field =
  | (Base & { kind: "select"; options: readonly Choice[] })
  | (Base & { kind: "toggle" })
  | (Base & { kind: "length"; placeholder?: string })
  | (Base & { kind: "number"; min: number; max: number; step: number; unit?: string })
  | (Base & { kind: "font" })
  | (Base & { kind: "style" })
  | (Base & { kind: "text"; placeholder?: string })
  | (Base & { kind: "textarea"; placeholder?: string });

export interface Group {
  id: string;
  title: string;
  fields: Field[];
}

const isCustomSize = (s: LayoutState) => valueOf(s, "page.size") === "custom";
const twoColumns = (s: LayoutState) => valueOf(s, "page.columns") === 2;
const hasToc = (s: LayoutState) => valueOf(s, "front.toc") === true;

export const GROUPS: Group[] = [
  {
    id: "document",
    title: "Document",
    fields: [
      { key: "meta.title", label: "Title", kind: "text" },
      { key: "meta.subtitle", label: "Subtitle", kind: "text" },
      { key: "meta.author", label: "Author", kind: "text" },
      { key: "meta.date", label: "Date", kind: "text", placeholder: "e.g. Spring 2026" },
      { key: "meta.contact", label: "Contact", kind: "textarea", placeholder: "Address, email (for title pages)" },
    ],
  },
  {
    id: "page",
    title: "Page",
    fields: [
      {
        key: "page.size",
        label: "Size",
        kind: "select",
        options: [
          ["a4", "A4"],
          ["a5", "A5"],
          ["us-letter", "US Letter"],
          ["us-trade", "US Trade (6 × 9 in)"],
          ["custom", "Custom"],
        ],
      },
      { key: "page.width", label: "Width", kind: "length", when: isCustomSize },
      { key: "page.height", label: "Height", kind: "length", when: isCustomSize },
      { key: "page.margin_top", label: "Top margin", kind: "length" },
      { key: "page.margin_bottom", label: "Bottom margin", kind: "length" },
      { key: "page.margin_inside", label: "Inside margin", kind: "length", title: "The binding side (left on one-sided pages)" },
      { key: "page.margin_outside", label: "Outside margin", kind: "length" },
      { key: "page.two_sided", label: "Two-sided", kind: "toggle", title: "Margins and page furniture alternate between left and right pages" },
      {
        key: "page.columns",
        label: "Columns",
        kind: "select",
        options: [
          [1, "One"],
          [2, "Two"],
        ],
      },
      { key: "page.column_gap", label: "Column gap", kind: "length", when: twoColumns },
    ],
  },
  {
    id: "typography",
    title: "Typography",
    fields: [
      { key: "text.font", label: "Font", kind: "font" },
      { key: "text.size", label: "Size", kind: "length" },
      { key: "text.leading", label: "Line spacing", kind: "length", title: "Space between lines (e.g. 0.65em)" },
      { key: "text.justify", label: "Justify", kind: "toggle" },
      { key: "text.hyphenate", label: "Hyphenate", kind: "toggle" },
      { key: "text.lang", label: "Language", kind: "text", placeholder: "en, de, en-US…", title: "Language code: hyphenation, quotes and terms" },
      { key: "text.first_line_indent", label: "First-line indent", kind: "length" },
      { key: "text.paragraph_spacing", label: "Paragraph spacing", kind: "length" },
    ],
  },
  {
    id: "headings",
    title: "Headings",
    fields: [
      {
        key: "headings.numbering",
        label: "Numbering",
        kind: "select",
        options: [
          ["none", "None"],
          ["1.", "1."],
          ["1.1.", "1.1."],
          ["I.", "I."],
          ["A.", "A."],
        ],
      },
      {
        key: "headings.chapter_pagebreak",
        label: "Chapters start",
        kind: "select",
        options: [
          ["none", "Where they fall"],
          ["page", "On a new page"],
          ["right", "On a right-hand page"],
        ],
      },
      { key: "headings.level1_size", label: "Level 1 size", kind: "length" },
      { key: "headings.level2_size", label: "Level 2 size", kind: "length" },
      { key: "headings.level3_size", label: "Level 3 and below", kind: "length", title: "Size of level 3 and all deeper headings" },
      { key: "headings.space_above", label: "Space above", kind: "length", placeholder: "Default", title: "Above headings of every level; empty: Typst's default by level. em is relative to the heading's size" },
      { key: "headings.space_below", label: "Space below", kind: "length", placeholder: "Default", title: "Below headings of every level; empty: Typst's default by level. em is relative to the heading's size" },
    ],
  },
  {
    id: "figures",
    title: "Figures & equations",
    fields: [
      {
        key: "figures.caption_position",
        label: "Captions",
        kind: "select",
        options: [
          ["bottom", "Below"],
          ["top", "Above"],
        ],
        title: "Of figures other than tables (tables: see Tables)",
      },
      { key: "figures.numbering", label: "Number figures", kind: "toggle" },
      { key: "figures.default_width", label: "Default width", kind: "number", min: 10, max: 100, step: 5, unit: "%", title: "Image width in percent of the column, unless the figure says otherwise" },
      { key: "equations.numbering", label: "Number equations", kind: "toggle" },
    ],
  },
  {
    id: "tables",
    title: "Tables",
    fields: [
      {
        key: "tables.style",
        label: "Default style",
        kind: "select",
        options: [
          ["plain", "Plain"],
          ["academic", "Academic"],
          ["grid", "Grid"],
          ["striped", "Striped"],
        ],
        title: "How tables look, unless a table has its own style (right-click it on a page). Academic: rules above and below the table and under its header",
      },
      { key: "tables.header_bold", label: "Bold header", kind: "toggle", title: "The header row (table.header) in bold" },
      {
        key: "tables.caption_position",
        label: "Caption",
        kind: "select",
        options: [
          ["top", "Above"],
          ["bottom", "Below"],
        ],
      },
    ],
  },
  {
    id: "citations",
    title: "Citations",
    fields: [
      { key: "citations.style", label: "Style", kind: "style" },
      { key: "citations.bibliography_title", label: "Bibliography title", kind: "text", placeholder: "Default for the language" },
    ],
  },
  {
    id: "furniture",
    title: "Page furniture",
    fields: [
      {
        key: "furniture.page_numbers",
        label: "Page numbers",
        kind: "select",
        options: [
          ["none", "None"],
          ["bottom-center", "Bottom centre"],
          ["bottom-outside", "Bottom outside"],
          ["top-outside", "Top outside"],
        ],
      },
      {
        key: "furniture.running_head",
        label: "Running head",
        kind: "select",
        options: [
          ["none", "None"],
          ["title", "Title"],
          ["chapter", "Chapter"],
          ["author-title", "Author / Title"],
        ],
      },
    ],
  },
  {
    id: "front",
    title: "Front matter",
    fields: [
      { key: "front.title_page", label: "Title page", kind: "toggle", title: "Needs a title, subtitle or author" },
      { key: "front.toc", label: "Table of contents", kind: "toggle" },
      {
        key: "front.toc_depth",
        label: "Contents depth",
        kind: "select",
        options: [
          [1, "Level 1"],
          [2, "Levels 1–2"],
          [3, "Levels 1–3"],
          [4, "Levels 1–4"],
        ],
        when: hasToc,
      },
      { key: "front.toc_title", label: "Contents title", kind: "text", placeholder: "Default for the language", when: hasToc },
      { key: "front.toc_dots", label: "Dot leaders", kind: "toggle", when: hasToc },
      { key: "front.toc_indent", label: "Indent levels", kind: "toggle", when: hasToc },
      { key: "front.toc_spacing", label: "Entry spacing", kind: "length", placeholder: "Line spacing", when: hasToc },
      { key: "front.toc_bold_top", label: "Bold level 1", kind: "toggle", when: hasToc },
    ],
  },
  {
    id: "export",
    title: "PDF export",
    fields: [
      {
        key: "export.image_quality",
        label: "Image quality",
        kind: "select",
        options: [
          ["original", "Original (no change)"],
          ["print", "Print (300 dpi)"],
          ["screen", "Screen (150 dpi)"],
          ["email", "Email (96 dpi)"],
        ],
        title: "Resolution of the images in the exported PDF; the pages look the same. Right-click an image to give it its own.",
      },
    ],
  },
];

export const ALL_FIELDS: Field[] = GROUPS.flatMap((g) => g.fields);

const isMeta = (key: string) => key.startsWith("meta.");

/** The current value of `key` (layout `section.field`, or `meta.field`). */
export function valueOf(s: LayoutState, key: string): unknown {
  const dot = key.indexOf(".");
  const section = key.slice(0, dot);
  const field = key.slice(dot + 1);
  if (section === "meta") return s.meta[field] ?? "";
  const part = s.layout[section];
  return part && typeof part === "object" ? (part as Record<string, unknown>)[field] : undefined;
}

/** Whether `key` differs from the preset (metadata never does). */
export function isOverridden(s: LayoutState, key: string): boolean {
  return !isMeta(key) && Object.prototype.hasOwnProperty.call(s.overrides, key);
}

/** How many settings of `group` differ from the preset. */
export function overrideCount(s: LayoutState, group: Group): number {
  return group.fields.filter((f) => isOverridden(s, f.key)).length;
}

/** A value as its control shows it. */
export function toControl(field: Field, value: unknown): string | boolean {
  if (field.kind === "toggle") return value === true;
  return value === undefined || value === null ? "" : String(value);
}

/** A control's input as the value to send, or why it can't be sent. */
export function fromControl(field: Field, raw: string | boolean): { value: unknown } | { error: string } {
  switch (field.kind) {
    case "toggle":
      return { value: raw === true };
    case "select": {
      const choice = field.options.find(([v]) => String(v) === raw);
      return choice ? { value: choice[0] } : { error: "Not one of the choices." };
    }
    case "number": {
      const text = String(raw).trim();
      const n = Number(text);
      if (text === "" || !Number.isFinite(n)) return { error: `Enter a number from ${field.min} to ${field.max}.` };
      return { value: n };
    }
    case "length":
    case "font":
    case "style":
      return { value: String(raw).trim() };
    case "text":
    case "textarea": {
      const text = String(raw);
      // Empty metadata is cleared rather than stored.
      return { value: isMeta(field.key) && text.trim() === "" ? null : text };
    }
  }
}

/** The command and changes that set `field` to `value` (`null`: back to the preset / cleared). */
export function changeFor(field: Field, value: unknown): { command: "layout_set" | "meta_set"; changes: Record<string, unknown> } {
  return isMeta(field.key)
    ? { command: "meta_set", changes: { [field.key.slice(5)]: value } }
    : { command: "layout_set", changes: { [field.key]: value } };
}

/** The backend's error for one field, without the batch prefix and the key. */
export function fieldError(message: string, key: string): string {
  let m = message.replace(/^Invalid settings \(nothing was changed\):\s*/, "");
  const bare = isMeta(key) ? key.slice(5) : key;
  if (m.startsWith(`${bare}: `)) m = m.slice(bare.length + 2);
  return m.charAt(0).toUpperCase() + m.slice(1);
}

// ---- The panel ----

export interface LayoutHost {
  get(): Promise<LayoutState>;
  set(changes: Record<string, unknown>): Promise<LayoutState>;
  setMeta(changes: Record<string, unknown>): Promise<LayoutState>;
  applyPreset(id: string): Promise<LayoutState>;
  setCustom(text: string): Promise<LayoutState>;
  presets(): Promise<PresetInfo[]>;
  fonts(): Promise<string[]>;
  /** The embedded fonts' faces, for the font pickers' previews. */
  faces: FontFaces;
  styles(): Promise<StyleInfo[]>;
  /** How often the text uses each character style (`styleUses` of the current text). */
  styleUses(): Map<string, number>;
  /** Sends pending text edits first, so undo order matches what the user did. */
  drain(): Promise<void>;
  error(e: unknown): void;
  /** After every state the panel shows. */
  onState(s: LayoutState): void;
}

type Control = HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement;

interface Row {
  field: Field;
  el: HTMLElement;
  control: Control;
  error: HTMLElement;
  /** The value last rendered (to tell whether the user is editing). */
  rendered: string | boolean;
  /** The user's text failed validation: keep it until fixed or reset. */
  invalid: boolean;
  /** The font field's picker. */
  picker?: FontPicker;
}

/** A character style's entry in the Character styles section. */
interface StyleRow {
  el: HTMLLIElement;
  sample: HTMLElement;
  uses: HTMLElement;
  name: HTMLInputElement;
  font: FontPicker;
  size: ComboBox;
  remove: HTMLButtonElement;
  error: HTMLElement;
  confirm: HTMLElement | null;
  /** The name last rendered (to tell whether the user is editing). */
  shownName: string;
  invalid: boolean;
}

const CUSTOM_IDLE_MS = 800;
const NUMBER_IDLE_MS = 300;
const USES_IDLE_MS = 400;

function el<K extends keyof HTMLElementTagNameMap>(tag: K, props: Partial<HTMLElementTagNameMap[K]> = {}, ...children: Array<Node | string>): HTMLElementTagNameMap[K] {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...children);
  return e;
}

export class LayoutPanel {
  private host: LayoutHost;
  private rows = new Map<string, Row>();
  private groups = new Map<string, { details: HTMLDetailsElement; count: HTMLElement; group: Group }>();
  private presetSelect: HTMLSelectElement;
  private presetDescription: HTMLElement;
  private confirmBox: HTMLElement;
  private confirmText: HTMLElement;
  private confirmButton: HTMLButtonElement;
  private pendingPreset: string | null = null;
  private presets: PresetInfo[] = [];
  /** The font families (asked once, for every picker). */
  private fontList: Promise<string[]> | null = null;
  private stylesLoaded = false;
  private styleSection: HTMLDetailsElement;
  private styleSummary: HTMLElement;
  private styleCount: HTMLElement;
  private styleEmpty: HTMLElement;
  private styleList: HTMLUListElement;
  private removeUnused: HTMLButtonElement;
  private styleRows = new Map<string, StyleRow>();
  /** Uses of each style in the text, counted on refresh and after text changes. */
  private uses = new Map<string, number>();
  private usesTimer: ReturnType<typeof setTimeout> | undefined;
  private expert: HTMLDetailsElement;
  private customInput: HTMLTextAreaElement;
  private customError: HTMLElement;
  private customRendered = "";
  private customTimer: ReturnType<typeof setTimeout> | undefined;
  private numberTimers = new Map<string, ReturnType<typeof setTimeout>>();
  /** One request at a time, in the order the user made them. */
  private chain: Promise<void> = Promise.resolve();
  private current: LayoutState | null = null;

  constructor(root: HTMLElement, host: LayoutHost) {
    this.host = host;

    this.presetSelect = el("select", { id: "lp-preset", className: "lp-preset-select" });
    this.presetSelect.setAttribute("aria-describedby", "lp-preset-description");
    this.presetSelect.addEventListener("change", () => this.choosePreset(this.presetSelect.value));
    this.presetDescription = el("p", { id: "lp-preset-description", className: "lp-note" });
    this.confirmText = el("span");
    this.confirmButton = el("button", { type: "button", className: "primary" });
    const cancel = el("button", { type: "button", textContent: "Cancel" });
    this.confirmBox = el("div", { className: "lp-confirm", hidden: true }, this.confirmText, el("span", { className: "lp-confirm-actions" }, this.confirmButton, cancel));
    this.confirmBox.setAttribute("role", "alertdialog");
    this.confirmButton.addEventListener("click", () => {
      const id = this.pendingPreset;
      this.closeConfirm();
      if (id) this.applyPreset(id);
    });
    cancel.addEventListener("click", () => {
      this.closeConfirm();
      if (this.current) this.presetSelect.value = this.current.preset;
      this.presetSelect.focus();
    });
    const presetLabel = el("label", { htmlFor: "lp-preset", className: "lp-preset-label", textContent: "Preset" });
    root.append(el("div", { className: "lp-preset" }, presetLabel, this.presetSelect, this.presetDescription, this.confirmBox));

    this.styleCount = el("span", { className: "lp-count" });
    this.styleSummary = el("summary", {}, "Character styles", this.styleCount);
    this.styleEmpty = el("p", { className: "lp-note lp-styles-empty", textContent: "No character styles yet. Select text and pick a font or size in the formatting row, or add a named style here." });
    this.styleList = el("ul", { className: "lp-styles-list" });
    const addStyle = el("button", { type: "button", textContent: "Add style", title: "A named style with the body's font and size" });
    addStyle.addEventListener("click", () => this.addStyle());
    this.removeUnused = el("button", { type: "button", textContent: "Remove unused", title: "Deletes the unnamed styles the text doesn't use (named ones stay)" });
    this.removeUnused.addEventListener("click", () => this.removeUnusedStyles());
    this.styleSection = el("details", { className: "lp-group lp-styles", open: true }, this.styleSummary, this.styleEmpty, this.styleList, el("div", { className: "lp-styles-actions" }, addStyle, this.removeUnused));

    for (const group of GROUPS) {
      const count = el("span", { className: "lp-count" });
      const details = el("details", { className: "lp-group", open: true }, el("summary", {}, group.title, count));
      for (const field of group.fields) details.append(this.buildRow(field));
      this.groups.set(group.id, { details, count, group });
      root.append(details);
      // The styles after the body's font and size.
      if (group.id === "typography") root.append(this.styleSection);
    }

    this.customInput = el("textarea", { id: "lp-custom", className: "lp-custom", spellcheck: false, rows: 8, placeholder: "#set par(leading: 0.7em)" });
    this.customInput.setAttribute("aria-describedby", "lp-custom-note");
    this.customInput.addEventListener("input", () => {
      clearTimeout(this.customTimer);
      this.customTimer = setTimeout(() => this.applyCustom(), CUSTOM_IDLE_MS);
    });
    this.customInput.addEventListener("blur", () => this.applyCustom());
    this.customError = el("p", { className: "lp-error", hidden: true });
    this.customError.setAttribute("role", "alert");
    this.expert = el(
      "details",
      { className: "lp-group lp-expert" },
      el("summary", {}, "Expert"),
      el("p", { id: "lp-custom-note", className: "lp-note", textContent: "Typst code, applied after the layout rules above (it can override them)." }),
      this.customInput,
      this.customError,
    );
    root.append(this.expert);
  }

  get state(): LayoutState | null {
    return this.current;
  }

  /** Reloads everything from the core (after undo/redo, entering the view, another project). */
  refresh(): Promise<void> {
    return this.run(async () => {
      const state = await this.host.get();
      this.uses = this.host.styleUses();
      this.render(state);
      void this.loadLists();
    });
  }

  /** The text changed: the use counts follow (debounced). */
  textChanged(): void {
    clearTimeout(this.usesTimer);
    this.usesTimer = setTimeout(() => {
      this.uses = this.host.styleUses();
      this.renderUses();
    }, USES_IDLE_MS);
  }

  /** Opens the Character styles section and scrolls it into view (once the panel shows its state). */
  revealStyles(): void {
    void this.chain.then(() => {
      this.styleSection.open = true;
      this.styleSection.scrollIntoView({ block: "start" });
      this.styleSummary.focus({ preventScroll: true });
    });
  }

  /** Forgets the shown project (another one is loading). */
  reset(): void {
    this.current = null;
    for (const row of this.rows.values()) {
      row.invalid = false;
      row.error.hidden = true;
    }
    this.customError.hidden = true;
    this.closeConfirm();
    for (const row of this.styleRows.values()) row.el.remove();
    this.styleRows.clear();
    this.uses = new Map();
  }

  /** Opens the expert box and selects `from..to` in it. */
  revealCustom(from: number, to: number): void {
    this.expert.open = true;
    this.customInput.focus();
    this.customInput.setSelectionRange(from, to);
    this.customInput.scrollIntoView({ block: "nearest" });
  }

  get customText(): string {
    return this.customInput.value;
  }

  /** Adds a section after the settings groups, before the expert box. */
  addSection(section: HTMLElement): void {
    this.expert.before(section);
  }

  private run(op: () => Promise<void>): Promise<void> {
    const next = this.chain.then(op).catch((e) => this.host.error(e));
    this.chain = next;
    return next;
  }

  private async loadLists(): Promise<void> {
    if (this.presets.length === 0) {
      this.presets = await this.host.presets().catch(() => []);
      this.presetSelect.replaceChildren(...this.presets.map((p) => el("option", { value: p.id, textContent: p.name })));
      if (this.current) this.renderPreset(this.current);
    }
    if (!this.stylesLoaded) {
      const styles = await this.host.styles().catch(() => null);
      if (styles) {
        this.stylesLoaded = true;
        const select = this.rows.get("citations.style")!.control as HTMLSelectElement;
        select.replaceChildren(...styles.map((s) => el("option", { value: s.id, textContent: s.name })));
        if (this.current) this.render(this.current);
      }
    }
  }

  /** The font families for a picker (asked again after a failure). */
  private fonts = (): Promise<string[]> =>
    (this.fontList ??= this.host.fonts().catch((e) => {
      this.fontList = null;
      throw e;
    }));

  private buildRow(field: Field): HTMLElement {
    const id = `lp-${field.key.replace(/\./g, "-")}`;
    let control: Control;
    switch (field.kind) {
      case "select":
        control = el("select", { id });
        control.append(...field.options.map(([v, label]) => el("option", { value: String(v), textContent: label })));
        break;
      case "style":
        control = el("select", { id });
        break;
      case "toggle":
        control = el("input", { id, type: "checkbox", className: "lp-switch" });
        control.setAttribute("role", "switch");
        break;
      case "number":
        control = el("input", { id, type: "number", min: String(field.min), max: String(field.max), step: String(field.step) });
        break;
      case "textarea":
        control = el("textarea", { id, rows: 3, placeholder: field.placeholder ?? "" });
        break;
      case "font":
        control = el("input", { id, type: "text", placeholder: "Search fonts…" });
        break;
      default:
        control = el("input", { id, type: "text", autocomplete: "off", spellcheck: false, placeholder: "placeholder" in field ? (field.placeholder ?? "") : "" });
        if (field.kind === "length") control.classList.add("lp-length");
    }
    const errorId = `${id}-error`;
    const error = el("p", { id: errorId, className: "lp-error", hidden: true });
    error.setAttribute("role", "alert");
    control.setAttribute("aria-describedby", errorId);
    const dot = el("span", { className: "lp-dot", title: "Changed from the preset" });
    dot.setAttribute("aria-hidden", "true");
    const label = el("label", { htmlFor: id }, dot, field.label);
    if (field.title) label.title = field.title;
    const reset = el("button", { type: "button", className: "lp-reset", textContent: "↺", title: "Reset to the preset's value" });
    reset.setAttribute("aria-label", `Reset ${field.label} to the preset's value`);
    reset.addEventListener("click", () => this.resetField(field));
    const controlBox = el("div", { className: "lp-control" }, control);
    if (field.kind === "number" && field.unit) controlBox.append(el("span", { className: "lp-unit", textContent: field.unit }));
    if (!isMeta(field.key)) controlBox.append(reset);
    const rowEl = el("div", { className: `lp-row lp-${field.kind}` }, label, controlBox, error);
    const row: Row = { field, el: rowEl, control, error, rendered: "", invalid: false };
    this.rows.set(field.key, row);

    if (field.kind === "font") {
      // A picked or typed family is applied (the core checks it); Escape gives up a refused one.
      row.picker = new FontPicker(
        control as HTMLInputElement,
        {
          fonts: this.fonts,
          faces: this.host.faces,
          pick: (family) => this.apply(row, family),
          done: () => {
            if (!row.invalid || control.value.trim() !== row.rendered) return;
            this.clearError(row);
            if (this.current) this.renderRow(row, this.current);
          },
        },
        { free: true },
      );
      return rowEl;
    }

    const read = () => (field.kind === "toggle" ? (control as HTMLInputElement).checked : control.value);
    control.addEventListener("change", () => {
      clearTimeout(this.numberTimers.get(field.key));
      this.apply(row, read());
    });
    if (field.kind === "number") {
      control.addEventListener("input", () => {
        clearTimeout(this.numberTimers.get(field.key));
        this.numberTimers.set(
          field.key,
          setTimeout(() => this.apply(row, read()), NUMBER_IDLE_MS),
        );
      });
    }
    if (control instanceof HTMLInputElement && control.type === "text") {
      control.addEventListener("keydown", (e) => {
        if (e.key === "Enter") control.dispatchEvent(new Event("change"));
        else if (e.key === "Escape" && row.invalid) {
          e.preventDefault();
          e.stopPropagation();
          this.clearError(row);
          if (this.current) this.renderRow(row, this.current);
        }
      });
    }
    return rowEl;
  }

  private apply(row: Row, raw: string | boolean): void {
    const parsed = fromControl(row.field, raw);
    if ("error" in parsed) {
      this.showError(row, parsed.error);
      return;
    }
    const { command, changes } = changeFor(row.field, parsed.value);
    void this.run(async () => {
      // Compare with the core's value, not `row.rendered`: a focused control
      // is not re-rendered, so `rendered` can be stale.
      if (!row.invalid && this.current && raw === toControl(row.field, valueOf(this.current, row.field.key))) return;
      await this.host.drain();
      try {
        const state = command === "meta_set" ? await this.host.setMeta(changes) : await this.host.set(changes);
        this.clearError(row);
        // The input is applied: show the core's (normalized) value unless the user typed on.
        if (row.field.kind !== "toggle" && row.control.value === raw) row.rendered = raw;
        this.render(state);
      } catch (e) {
        this.showError(row, fieldError(e instanceof Error ? e.message : String(e), row.field.key));
      }
    });
  }

  private resetField(field: Field): void {
    const row = this.rows.get(field.key)!;
    void this.run(async () => {
      await this.host.drain();
      const state = await this.host.set({ [field.key]: null });
      this.clearError(row);
      this.render(state);
    });
  }

  private applyCustom(): void {
    clearTimeout(this.customTimer);
    const text = this.customInput.value;
    if (text === this.customRendered) return;
    void this.run(async () => {
      if (text === this.customRendered) return;
      await this.host.drain();
      try {
        const state = await this.host.setCustom(text);
        this.customError.hidden = true;
        this.customRendered = text;
        this.render(state);
      } catch (e) {
        this.customError.textContent = e instanceof Error ? e.message : String(e);
        this.customError.hidden = false;
      }
    });
  }

  private choosePreset(id: string): void {
    const s = this.current;
    if (!s || id === s.preset) {
      this.closeConfirm();
      return;
    }
    if (Object.keys(s.overrides).length === 0) {
      this.applyPreset(id);
      return;
    }
    const name = this.presets.find((p) => p.id === id)?.name ?? id;
    this.pendingPreset = id;
    this.confirmText.textContent = "Reset your changes to the new preset?";
    this.confirmButton.textContent = `Reset to ${name}`;
    this.confirmBox.hidden = false;
    this.presetDescription.textContent = this.presets.find((p) => p.id === id)?.description ?? "";
  }

  private closeConfirm(): void {
    this.pendingPreset = null;
    this.confirmBox.hidden = true;
  }

  private applyPreset(id: string): void {
    void this.run(async () => {
      await this.host.drain();
      const state = await this.host.applyPreset(id);
      for (const row of this.rows.values()) this.clearError(row);
      this.render(state);
    });
  }

  private showError(row: Row, message: string): void {
    row.invalid = true;
    row.error.textContent = message;
    row.error.hidden = false;
    row.el.classList.add("invalid");
    row.control.setAttribute("aria-invalid", "true");
  }

  private clearError(row: Row): void {
    row.invalid = false;
    row.error.hidden = true;
    row.el.classList.remove("invalid");
    row.control.removeAttribute("aria-invalid");
  }

  private renderPreset(s: LayoutState): void {
    if (this.pendingPreset === null) {
      if (![...this.presetSelect.options].some((o) => o.value === s.preset)) {
        this.presetSelect.append(el("option", { value: s.preset, textContent: s.preset }));
      }
      this.presetSelect.value = s.preset;
      this.presetDescription.textContent = this.presets.find((p) => p.id === s.preset)?.description ?? "";
    }
  }

  private renderRow(row: Row, s: LayoutState): void {
    const { field, control } = row;
    row.el.hidden = field.when ? !field.when(s) : false;
    row.el.classList.toggle("overridden", isOverridden(s, field.key));
    const value = toControl(field, valueOf(s, field.key));
    // Keep text the core refused, and what the user is typing (the picker keeps that itself).
    if (row.invalid) return;
    if (row.picker) {
      if (value) this.host.faces.load([String(value)]);
      row.picker.set(String(value));
      row.rendered = value;
      return;
    }
    const editing = document.activeElement === control && control.value !== row.rendered && field.kind !== "toggle";
    if (editing) return;
    if (control instanceof HTMLInputElement && control.type === "checkbox") control.checked = value === true;
    else {
      if (control instanceof HTMLSelectElement && ![...control.options].some((o) => o.value === value)) {
        control.append(el("option", { value: String(value), textContent: String(value) }));
      }
      control.value = String(value);
    }
    row.rendered = value;
  }

  private render(s: LayoutState): void {
    this.current = s;
    this.renderPreset(s);
    for (const row of this.rows.values()) this.renderRow(row, s);
    for (const { count, group } of this.groups.values()) {
      const n = overrideCount(s, group);
      count.textContent = n ? ` · ${n} changed` : "";
    }
    const editingCustom = document.activeElement === this.customInput && this.customInput.value !== this.customRendered;
    if (!editingCustom && this.customError.hidden) {
      this.customInput.value = s.custom;
    }
    this.customRendered = s.custom;
    this.renderStyles(s);
    this.host.onState(s);
  }

  // ---- Character styles ----

  /** The styles' entries in `styleEntries` order (named first); an entry stays while its style does. */
  private renderStyles(s: LayoutState): void {
    const info = textStylesOf(s);
    const entries = styleEntries(info);
    for (const [id, row] of this.styleRows) {
      if (id in info.styles) continue;
      row.el.remove();
      this.styleRows.delete(id);
    }
    const focused = document.activeElement;
    entries.forEach((entry, i) => {
      const row = this.styleRows.get(entry.id) ?? this.buildStyle(entry.id);
      this.renderStyle(row, entry, s);
      const at = this.styleList.children[i] ?? null;
      if (at !== row.el) this.styleList.insertBefore(row.el, at);
    });
    // An entry moved: the focus stays where it was.
    if (focused instanceof HTMLElement && focused !== document.activeElement && this.styleList.contains(focused)) focused.focus({ preventScroll: true });
    this.styleEmpty.hidden = entries.length > 0;
    this.styleCount.textContent = entries.length ? ` · ${entries.length}` : "";
    this.renderUses();
  }

  private renderStyle(row: StyleRow, entry: StyleEntry, s: LayoutState): void {
    const style = s.layout.styles[entry.id];
    row.sample.textContent = entry.label;
    row.sample.style.fontFamily = entry.font ? cssFamily(entry.font) : "";
    row.remove.setAttribute("aria-label", `Delete style ${entry.label}`);
    const name = style.name ?? "";
    const editing = document.activeElement === row.name && row.name.value.trim() !== row.shownName;
    if (!row.invalid && !editing) row.name.value = name;
    row.shownName = name;
    row.font.set(style.font ?? "");
    row.size.set(style.size ? sizeLabel(style.size) : "");
  }

  /** The use counts, and what "Remove unused" would delete. */
  private renderUses(): void {
    for (const [id, row] of this.styleRows) {
      const n = this.uses.get(id) ?? 0;
      row.uses.textContent = n ? `${n.toLocaleString("en-US")} ${n === 1 ? "use" : "uses"}` : "Unused";
    }
    const unused = this.current ? unusedStyles(textStylesOf(this.current), this.uses).length : 0;
    this.removeUnused.textContent = unused ? `Remove unused (${unused})` : "Remove unused";
    this.removeUnused.disabled = unused === 0;
  }

  private buildStyle(id: string): StyleRow {
    const sample = el("span", { className: "lp-style-sample" });
    const uses = el("span", { className: "lp-style-uses" });
    const remove = el("button", { type: "button", className: "lp-style-delete", textContent: "Delete", title: "Delete this style (its text shows as default text)" });
    const name = el("input", { type: "text", autocomplete: "off", spellcheck: false, placeholder: "Unnamed", title: "The style's name; empty: named after its font and size" });
    const fontInput = el("input", { type: "text", placeholder: "Body font" });
    const sizeInput = el("input", { type: "text", inputMode: "decimal", placeholder: "Body", className: "lp-length" });
    name.setAttribute("aria-label", "Style name");
    fontInput.setAttribute("aria-label", "Style font");
    sizeInput.setAttribute("aria-label", "Style size in points");
    const error = el("p", { className: "lp-error", hidden: true });
    error.setAttribute("role", "alert");
    const li = el(
      "li",
      { className: "lp-style" },
      el("div", { className: "lp-style-head" }, sample, uses, remove),
      el("div", { className: "lp-control" }, name),
      el("div", { className: "lp-control" }, fontInput, sizeInput, el("span", { className: "lp-unit", textContent: "pt" })),
      error,
    );
    const field = (f: "font" | "size") => (text: string) => this.setStyleField(id, f, text);
    const font = new FontPicker(fontInput, { fonts: this.fonts, faces: this.host.faces, pick: field("font"), done: () => {} }, { free: true, clearable: true });
    const size = new ComboBox(sizeInput, { items: () => SIZES.map((n) => ({ value: String(n), label: String(n) })), commit: field("size"), done: () => {} }, { label: "font sizes", free: true, clearable: true });
    const row: StyleRow = { el: li, sample, uses, name, font, size, remove, error, confirm: null, shownName: "", invalid: false };
    this.styleRows.set(id, row);

    name.addEventListener("keydown", (e) => {
      if (e.key === "Enter") this.commitName(id);
      else if (e.key === "Escape" && (row.invalid || name.value.trim() !== row.shownName)) {
        e.preventDefault();
        e.stopPropagation();
        this.clearStyleError(row);
        name.value = row.shownName;
      }
    });
    name.addEventListener("blur", () => this.commitName(id));
    remove.addEventListener("click", () => this.askDelete(id));
    return row;
  }

  private showStyleError(row: StyleRow, message: string): void {
    row.invalid = true;
    row.error.textContent = message;
    row.error.hidden = false;
    row.el.classList.add("invalid");
  }

  private clearStyleError(row: StyleRow): void {
    row.invalid = false;
    row.error.hidden = true;
    row.el.classList.remove("invalid");
  }

  /** One `layout_set` for a style; a refusal shows at its entry (`key`: the field it is about). */
  private setStyle(row: StyleRow, changes: Record<string, unknown>, key: string): void {
    void this.run(async () => {
      await this.host.drain();
      try {
        const state = await this.host.set(changes);
        this.clearStyleError(row);
        this.render(state);
      } catch (e) {
        this.showStyleError(row, fieldError(e instanceof Error ? e.message : String(e), key));
      }
    });
  }

  /** The typed name (Enter, leaving the field): stored, or cleared (the style is unnamed again). */
  private commitName(id: string): void {
    const row = this.styleRows.get(id);
    const style = this.current?.layout.styles[id];
    if (!row || !style) return;
    const name = row.name.value.trim();
    if (name === (style.name ?? "")) {
      if (row.invalid) this.clearStyleError(row);
      return;
    }
    const key = `styles.${id}.name`;
    this.setStyle(row, { [key]: name || null }, key);
  }

  /** A font or size for the style (all its uses); empty clears it while the style keeps the other. */
  private setStyleField(id: string, field: "font" | "size", raw: string): void {
    const row = this.styleRows.get(id);
    const style = this.current?.layout.styles[id];
    if (!row || !style) return;
    const fail = (message: string) => {
      this.showStyleError(row, message);
      if (field === "font") row.font.set(style.font ?? "");
      else row.size.set(style.size ? sizeLabel(style.size) : "");
    };
    const text = raw.trim();
    const value = field === "font" || !text ? text || null : parseSize(text);
    if (value === null && text) return fail(`“${text}” is not a font size (a number of points, like 11 or 10.5).`);
    if (value === null && !(field === "font" ? style.size : style.font)) return fail("A style needs a font or a size (or delete it).");
    if (sameSettings({ [field]: value ?? undefined }, { [field]: style[field] })) {
      if (row.invalid) this.clearStyleError(row);
      return;
    }
    const key = `styles.${id}.${field}`;
    this.setStyle(row, { [key]: value }, key);
  }

  /** A named style "Style N" with the body's font and size; its name field takes the focus. */
  private addStyle(): void {
    void this.run(async () => {
      if (!this.current) return;
      const info = textStylesOf(this.current);
      this.uses = this.host.styleUses();
      const { id, name } = newNamedStyle(info, this.uses);
      const changes = { ...styleChanges(new Map([[id, { font: info.body.font || undefined, size: info.body.size || undefined }]])), [`styles.${id}.name`]: name };
      await this.host.drain();
      this.render(await this.host.set(changes));
      const row = this.styleRows.get(id);
      if (!row) return;
      row.name.focus();
      row.name.select();
    });
  }

  /** Deletes style `id`, after asking when the text uses it. */
  private askDelete(id: string): void {
    const row = this.styleRows.get(id);
    if (!row || row.confirm) return;
    this.uses = this.host.styleUses();
    this.renderUses();
    const n = this.uses.get(id) ?? 0;
    if (!n) {
      this.deleteStyles([id]);
      return;
    }
    const cancel = el("button", { type: "button", textContent: "Cancel" });
    const go = el("button", { type: "button", className: "primary", textContent: "Delete" });
    const text = `Delete “${row.sample.textContent}”? Its ${n.toLocaleString("en-US")} ${n === 1 ? "use" : "uses"} in the text will show as default text. Undo brings it back.`;
    const confirm = el("div", { className: "lp-confirm" }, el("span", { textContent: text }), el("span", { className: "lp-confirm-actions" }, go, cancel));
    confirm.setAttribute("role", "alertdialog");
    const close = () => {
      confirm.remove();
      row.confirm = null;
    };
    cancel.addEventListener("click", () => {
      close();
      row.remove.focus();
    });
    go.addEventListener("click", () => {
      close();
      this.deleteStyles([id]);
    });
    confirm.addEventListener("keydown", (e) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      cancel.click();
    });
    row.confirm = confirm;
    row.el.append(confirm);
    cancel.focus();
  }

  /** "Remove unused": every unnamed style the text doesn't use, in one change. */
  private removeUnusedStyles(): void {
    if (!this.current) return;
    this.uses = this.host.styleUses();
    const ids = unusedStyles(textStylesOf(this.current), this.uses);
    if (ids.length) this.deleteStyles(ids);
    else this.renderUses();
  }

  private deleteStyles(ids: string[]): void {
    void this.run(async () => {
      if (!this.current) return;
      await this.host.drain();
      this.render(await this.host.set(deleteChanges(this.current, ids)));
    });
  }
}
