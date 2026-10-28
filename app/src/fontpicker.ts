// A font picker: a combobox (a text field with a popup list, typed text
// filters it) listing font families, each shown in its own font. System
// fonts show by name; the fonts embedded in Typst are not installed, so
// their files are loaded into the web view under a private alias
// (`FontFaces`), and a family's CSS names the alias first. `ComboBox` is the
// generic part (the formatting row's size and Style boxes use it too).
//
// No Tauri imports: the fonts come in through hosts. The filtering and the
// CSS names are pure and tested under `node --test`.

/** The alias an embedded font's face is loaded under. */
const ALIAS = "Lethora:";

/** `text` as a quoted CSS string. */
const cssString = (text: string) => `"${text.replace(/["\\]/g, "\\$&")}"`;

/** The CSS `font-family` that shows `family`: its embedded face if loaded, else the system's, else `fallback`. */
export function cssFamily(family: string, fallback = "var(--serif)"): string {
  return `${cssString(ALIAS + family)}, ${cssString(family)}, ${fallback}`;
}

/**
 * The families matching `query` (each of its words, any case): those
 * starting with it first, else in list order. All for null or a blank query.
 */
export function filterFonts(fonts: readonly string[], query: string | null): string[] {
  const q = (query ?? "").trim().toLowerCase();
  if (!q) return [...fonts];
  const words = q.split(/\s+/);
  const hits = fonts.filter((f) => words.every((w) => f.toLowerCase().includes(w)));
  return [...hits.filter((f) => f.toLowerCase().startsWith(q)), ...hits.filter((f) => !f.toLowerCase().startsWith(q))];
}

// ---- Embedded fonts in the web view ----

/** A font file as a command returns it (raw bytes, or numbers from the dev mock). */
export type FontFile = ArrayBuffer | number[];

export interface FontFilesHost {
  /** The embedded families. */
  embedded(): Promise<string[]>;
  /** The file of an embedded family's regular face (empty for others). */
  file(family: string): Promise<FontFile>;
}

/** Loads the embedded fonts' faces into the web view on demand, once each; failures leave the preview to fall back. */
export class FontFaces {
  private host: FontFilesHost;
  private embedded: Promise<Set<string>> | null = null;
  private loaded = new Set<string>();

  constructor(host: FontFilesHost) {
    this.host = host;
  }

  /** Starts loading the embedded ones among `families` (the system's show by name). */
  load(families: Iterable<string>): void {
    this.embedded ??= this.host.embedded().then(
      (list) => new Set(list.map((f) => f.toLowerCase())),
      () => new Set<string>(),
    );
    const wanted = [...families];
    void this.embedded.then((embedded) => {
      for (const family of wanted) {
        const key = family.toLowerCase();
        if (!embedded.has(key) || this.loaded.has(key)) continue;
        this.loaded.add(key);
        void this.add(family);
      }
    });
  }

  private async add(family: string): Promise<void> {
    try {
      const file = await this.host.file(family);
      const bytes = Array.isArray(file) ? new Uint8Array(file) : file;
      if (!bytes.byteLength) return;
      const face = new FontFace(cssString(ALIAS + family), bytes);
      document.fonts.add(await face.load());
    } catch {
      // Shown in the fallback font.
    }
  }
}

// ---- Combobox ----

export interface ComboItem {
  value: string;
  label: string;
  /** The CSS `font-family` the entry shows in (a preview). */
  font?: string;
}

export interface ComboHost {
  /** The entries for the typed text (null: not typed, all of them). */
  items(query: string | null): readonly ComboItem[];
  /** A picked entry's value, or (with `free`) the typed text. */
  commit(value: string): void;
  /** After a pick, Enter or Escape: the focus goes back (to the editor). */
  done(): void;
  /** Before the list opens (to load previews). */
  opening?(): void;
}

export interface ComboOptions {
  /** What the list holds, for the open button ("fonts"). */
  label: string;
  /** Typing filters the list (else it marks the entry matching the text). */
  filter?: boolean;
  /** Typed text that is no entry is applied too (Enter, leaving the field). */
  free?: boolean;
  /** With `free`: an emptied field is applied too (clears the value). */
  clearable?: boolean;
  /** Not typed into: a list to pick from. */
  readOnly?: boolean;
}

let comboCount = 0;

/**
 * A text field with a popup list (ARIA combobox): typing, the arrow keys,
 * Enter, Escape and the mouse. The field shows what `set` gives unless it
 * is being typed in; Escape and leaving it (but with `free`) restore that.
 */
export class ComboBox {
  readonly input: HTMLInputElement;
  private host: ComboHost;
  private options: ComboOptions;
  private button: HTMLButtonElement;
  private list: HTMLUListElement;
  private items: readonly ComboItem[] = [];
  private active = -1;
  /** The field's text and font while it is not typed in. */
  private shown = "";
  private shownFont = "";
  private typed = false;

  /** Wraps `input` (already in the page) with an open button and the list. */
  constructor(input: HTMLInputElement, host: ComboHost, options: ComboOptions) {
    this.input = input;
    this.host = host;
    this.options = options;
    const id = `combo-${++comboCount}`;
    const box = document.createElement("span");
    box.className = "combo";
    input.replaceWith(box);
    this.button = Object.assign(document.createElement("button"), { type: "button", tabIndex: -1, className: "combo-open", textContent: "▾" });
    this.button.setAttribute("aria-label", `Show ${options.label}`);
    this.list = Object.assign(document.createElement("ul"), { id, className: "combo-list", hidden: true });
    this.list.setAttribute("role", "listbox");
    box.append(input, this.button, this.list);
    Object.assign(input, { autocomplete: "off", spellcheck: false, readOnly: options.readOnly ?? false });
    input.setAttribute("role", "combobox");
    input.setAttribute("aria-controls", id);
    input.setAttribute("aria-expanded", "false");
    input.setAttribute("aria-autocomplete", options.filter ? "list" : "none");

    input.addEventListener("focus", () => {
      if (!input.readOnly) input.select();
    });
    input.addEventListener("click", () => {
      if (input.readOnly) this.toggle();
    });
    input.addEventListener("input", () => {
      this.typed = true;
      this.open();
      this.render();
    });
    input.addEventListener("keydown", (e) => this.key(e));
    input.addEventListener("blur", () => {
      if (this.options.free && this.typed && (input.value.trim() || this.options.clearable) && input.value !== this.shown) {
        this.typed = false;
        this.close();
        this.host.commit(input.value);
      } else this.revert();
    });
    // The list and the button never take the focus from the field.
    for (const el of [this.button, this.list]) el.addEventListener("mousedown", (e) => e.preventDefault());
    this.button.addEventListener("click", () => {
      if (input.disabled) return;
      input.focus();
      this.toggle();
    });
    this.list.addEventListener("click", (e) => {
      const option = (e.target as HTMLElement).closest<HTMLElement>("[data-index]");
      if (option) this.pick(Number(option.dataset.index));
    });
  }

  /** Shows `text` (in CSS font `font`) unless the field is being typed in. */
  set(text: string, font = ""): void {
    this.shown = text;
    this.shownFont = font;
    if (document.activeElement === this.input && (this.typed || !this.list.hidden)) return;
    this.input.value = text;
    this.input.style.fontFamily = font;
  }

  set disabled(on: boolean) {
    this.input.disabled = on;
    this.button.disabled = on;
    if (on) this.revert();
  }

  /** Renders the list again if it is open (its entries changed). */
  refresh(): void {
    if (!this.list.hidden) this.render();
  }

  private toggle(): void {
    if (this.list.hidden) {
      this.open();
      this.render();
    } else this.close();
  }

  private open(): void {
    if (!this.list.hidden) return;
    this.host.opening?.();
    // Fixed, so the row's clipping never cuts it.
    const r = this.input.getBoundingClientRect();
    Object.assign(this.list.style, { left: `${r.left}px`, top: `${r.bottom + 2}px`, minWidth: `${r.width + this.button.offsetWidth}px` });
    this.list.hidden = false;
    // Within the window.
    const right = r.left + this.list.offsetWidth;
    if (right > innerWidth - 4) this.list.style.left = `${Math.max(4, innerWidth - 4 - this.list.offsetWidth)}px`;
    this.input.setAttribute("aria-expanded", "true");
  }

  private close(): void {
    this.list.hidden = true;
    this.input.setAttribute("aria-expanded", "false");
    this.input.removeAttribute("aria-activedescendant");
  }

  private revert(): void {
    this.typed = false;
    this.close();
    this.input.value = this.shown;
    this.input.style.fontFamily = this.shownFont;
  }

  private render(): void {
    const text = this.input.value.trim().toLowerCase();
    this.items = this.host.items(this.typed && this.options.filter ? this.input.value : null);
    this.list.replaceChildren(
      ...this.items.map((item, i) => {
        const li = Object.assign(document.createElement("li"), { id: `${this.list.id}-${i}`, textContent: item.label });
        li.dataset.index = String(i);
        li.setAttribute("role", "option");
        if (item.font) li.style.fontFamily = item.font;
        return li;
      }),
    );
    const match = this.items.findIndex((item) => item.label.toLowerCase() === text || item.value.toLowerCase() === text);
    this.setActive(match >= 0 ? match : this.typed && this.options.filter && text && this.items.length ? 0 : -1);
  }

  private setActive(i: number): void {
    this.list.querySelector('[aria-selected="true"]')?.setAttribute("aria-selected", "false");
    this.active = i;
    const li = i >= 0 ? (this.list.children[i] as HTMLElement | undefined) : undefined;
    if (!li) {
      this.input.removeAttribute("aria-activedescendant");
      return;
    }
    li.setAttribute("aria-selected", "true");
    this.input.setAttribute("aria-activedescendant", li.id);
    li.scrollIntoView({ block: "nearest" });
  }

  private pick(i: number): void {
    const item = this.items[i];
    if (!item) return;
    this.typed = false;
    this.close();
    this.input.value = item.label;
    this.input.style.fontFamily = item.font ?? "";
    this.host.commit(item.value);
    this.host.done();
  }

  private key(e: KeyboardEvent): void {
    if (e.altKey || e.ctrlKey || e.metaKey) return;
    switch (e.key) {
      case "ArrowDown":
      case "ArrowUp": {
        e.preventDefault();
        if (this.list.hidden) {
          this.open();
          this.render();
        } else if (this.items.length) {
          const step = e.key === "ArrowDown" ? 1 : -1;
          this.setActive(this.active < 0 ? (step > 0 ? 0 : this.items.length - 1) : Math.min(this.items.length - 1, Math.max(0, this.active + step)));
        }
        return;
      }
      case "Enter": {
        e.preventDefault();
        if (!this.list.hidden && this.active >= 0) this.pick(this.active);
        else if (this.options.free && this.typed && (this.input.value.trim() || this.options.clearable)) {
          this.typed = false;
          this.close();
          this.host.commit(this.input.value);
          this.host.done();
        } else {
          this.revert();
          this.host.done();
        }
        return;
      }
      case "Escape":
        e.preventDefault();
        e.stopPropagation();
        this.revert();
        this.host.done();
        return;
      case "Tab":
        this.close();
    }
  }
}

export interface FontPickerHost {
  /** Every family to offer. */
  fonts(): Promise<string[]>;
  faces: FontFaces;
  /** A family picked. */
  pick(family: string): void;
  /** After a pick or Escape. */
  done(): void;
}

/** A combobox of font families, each previewed in its font (the embedded ones load when the list first opens). */
export class FontPicker {
  readonly box: ComboBox;
  private host: FontPickerHost;
  private fonts: string[] = [];
  private requested = false;

  /** `options`: typed names are applied too (`free`), an emptied field too (`clearable`). */
  constructor(input: HTMLInputElement, host: FontPickerHost, options: Pick<ComboOptions, "free" | "clearable"> = {}) {
    this.host = host;
    this.box = new ComboBox(
      input,
      {
        items: (query) => filterFonts(this.fonts, query).map((f) => ({ value: f, label: f, font: cssFamily(f) })),
        commit: (family) => host.pick(family),
        done: () => host.done(),
        opening: () => this.load(),
      },
      { label: "fonts", filter: true, ...options },
    );
  }

  /** Shows `family` (empty: none, e.g. mixed). */
  set(family: string): void {
    this.box.set(family, family ? cssFamily(family) : "");
  }

  set disabled(on: boolean) {
    this.box.disabled = on;
  }

  private load(): void {
    if (this.requested) return;
    this.requested = true;
    this.host.fonts().then(
      (fonts) => {
        this.fonts = fonts;
        this.host.faces.load(fonts);
        this.box.refresh();
      },
      () => {
        this.requested = false;
      },
    );
  }
}
