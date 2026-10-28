// The sources library (cached `references.bib` summary, citation style,
// formatted references) and the science view's Sources drawer: search,
// insert a citation, import a .bib file or pasted BibTeX, add a source.
// No Tauri imports: commands come in through `SourcesApi`.

import {
  type BibEntry,
  type BibError,
  type BibLibrary,
  CONTAINER,
  entryDetail,
  filterEntries,
  type ImportReport,
  importSummary,
  KEY_RE,
  makeKey,
  searchText,
  type SourceType,
  type StyleInfo,
  toBibLatex,
} from "./bib.ts";

export interface SourcesApi {
  entries(): Promise<BibLibrary>;
  styles(): Promise<StyleInfo[]>;
  format(key: string, style: string): Promise<string>;
  importText(text: string): Promise<ImportReport>;
  /** Asks for a .bib file and imports it; null if cancelled. */
  importFile(): Promise<ImportReport | null>;
}

const STYLE_KEY = "lethora.citeStyle";

function loadStyle(): string {
  try {
    return localStorage.getItem(STYLE_KEY) ?? "";
  } catch {
    return "";
  }
}

/** The project's sources, cached in the web view and refreshed on demand. */
export class Library {
  private api: SourcesApi;
  private listeners = new Set<() => void>();
  private formatted = new Map<string, Promise<string>>();
  private refreshing: Promise<void> | null = null;
  /** Bumped per project, so answers for a previous one are ignored. */
  private generation = 0;
  /** The last library as loaded (to skip unchanged refreshes). */
  private snapshot = "";
  entries: readonly BibEntry[] = [];
  errors: BibError[] = [];
  /** `searchText` per entry. */
  index: string[] = [];
  loaded = false;
  styles: StyleInfo[] = [];
  style = loadStyle();

  constructor(api: SourcesApi) {
    this.api = api;
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /** Forgets the sources (another project is opening). */
  reset(): void {
    this.generation++;
    this.refreshing = null;
    this.entries = [];
    this.errors = [];
    this.index = [];
    this.loaded = false;
    this.snapshot = "";
    this.formatted.clear();
    this.emit();
  }

  /** Reloads `references.bib` (concurrent calls share one request). */
  refresh(): Promise<void> {
    this.refreshing ??= this.load().finally(() => (this.refreshing = null));
    return this.refreshing;
  }

  private async load(): Promise<void> {
    const generation = this.generation;
    const [library, styles] = await Promise.all([this.api.entries(), this.styles.length ? this.styles : this.api.styles()]);
    if (generation !== this.generation) return;
    this.styles = styles;
    if (!styles.some((s) => s.id === this.style)) this.style = styles[0]?.id ?? "";
    const snapshot = JSON.stringify(library);
    const changed = !this.loaded || snapshot !== this.snapshot;
    this.loaded = true;
    if (!changed) return;
    this.snapshot = snapshot;
    this.entries = library.entries;
    this.errors = library.errors;
    this.index = library.entries.map(searchText);
    this.formatted.clear();
    this.emit();
  }

  setStyle(id: string): void {
    this.style = id;
    try {
      localStorage.setItem(STYLE_KEY, id);
    } catch {
      // not persisted
    }
    this.emit();
  }

  /** The source as a reference in the selected style (cached). */
  format(key: string): Promise<string> {
    const id = `${this.style}\u0000${key}`;
    let p = this.formatted.get(id);
    if (!p) {
      p = this.api.format(key, this.style);
      p.catch(() => this.formatted.delete(id));
      this.formatted.set(id, p);
    }
    return p;
  }

  async importText(text: string): Promise<ImportReport> {
    const report = await this.api.importText(text);
    await this.refresh();
    return report;
  }

  async importFile(): Promise<ImportReport | null> {
    const report = await this.api.importFile();
    if (report) await this.refresh();
    return report;
  }

  private emit(): void {
    for (const l of this.listeners) l();
  }
}

// ---- Drawer ----

export interface SourcesHost {
  library: Library;
  /** Inserts `@key` in the editor. */
  cite(key: string): void;
  error(e: unknown): void;
  info(message: string): void;
  focusEditor(): void;
  onOpenChange(open: boolean): void;
}

/** Entries rendered at most (search narrows the rest). */
const MAX_SHOWN = 200;

const byId = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;

const CONTAINER_LABEL = { journal: "Journal", publisher: "Publisher", booktitle: "Book title" };

export class SourcesPanel {
  private host: SourcesHost;
  private panel = byId("sources");
  private search = byId<HTMLInputElement>("src-search");
  private styleSelect = byId<HTMLSelectElement>("src-style");
  private problems = byId("src-problems");
  private problemsSummary = byId("src-problems-summary");
  private problemsList = byId("src-problems-list");
  private report = byId("src-report");
  private list = byId("src-list");
  private count = byId("src-count");
  private preview = byId("src-preview");
  private pasteForm = byId<HTMLFormElement>("src-paste");
  private pasteText = byId<HTMLTextAreaElement>("src-paste-text");
  private addForm = byId<HTMLFormElement>("src-add");
  private keyInput = byId<HTMLInputElement>("src-key");
  /** The key field was edited by hand (stop generating it). */
  private keyEdited = false;
  private hoverTimer: ReturnType<typeof setTimeout> | undefined;
  private previewKey = "";

  constructor(host: SourcesHost) {
    this.host = host;
    const lib = host.library;
    lib.subscribe(() => {
      if (this.isOpen) this.render();
    });
    this.search.addEventListener("input", () => this.renderList());
    this.styleSelect.addEventListener("change", () => {
      lib.setStyle(this.styleSelect.value);
      for (const el of this.list.querySelectorAll<HTMLElement>(".src-ref:not([hidden])")) void this.fillReference(el);
      if (this.previewKey) void this.showPreview(this.previewKey);
    });

    this.list.addEventListener("click", (e) => {
      const target = e.target as HTMLElement;
      const expand = target.closest<HTMLElement>("[data-expand]");
      if (expand) {
        const ref = expand.parentElement!.querySelector<HTMLElement>(".src-ref")!;
        const open = ref.hidden;
        ref.hidden = !open;
        expand.setAttribute("aria-expanded", String(open));
        if (open) void this.fillReference(ref);
        return;
      }
      const item = target.closest<HTMLElement>("[data-cite]");
      if (item) {
        this.host.cite(item.dataset.cite!);
        this.host.info(`Cited @${item.dataset.cite}`);
      }
    });
    const hover = (e: Event) => {
      const item = (e.target as HTMLElement).closest<HTMLElement>("[data-cite]");
      clearTimeout(this.hoverTimer);
      if (item) this.hoverTimer = setTimeout(() => void this.showPreview(item.dataset.cite!), 250);
    };
    this.list.addEventListener("mouseover", hover);
    this.list.addEventListener("focusin", hover);

    byId("src-import").addEventListener("click", () => void this.importFile());
    byId("src-paste-toggle").addEventListener("click", () => this.showForm(this.pasteForm.hidden ? "paste" : null));
    byId("src-add-toggle").addEventListener("click", () => this.showForm(this.addForm.hidden ? "add" : null));
    for (const cancel of this.panel.querySelectorAll<HTMLElement>("[data-cancel]")) {
      cancel.addEventListener("click", () => this.showForm(null));
    }
    this.pasteForm.addEventListener("submit", (e) => {
      e.preventDefault();
      void this.importPasted();
    });
    this.addForm.addEventListener("submit", (e) => {
      e.preventDefault();
      void this.addSource();
    });
    this.addForm.addEventListener("input", (e) => {
      if (e.target === this.keyInput) this.keyEdited = this.keyInput.value.trim() !== "";
      else this.updateKey();
      this.keyInput.setCustomValidity("");
    });
    byId<HTMLSelectElement>("src-type").addEventListener("change", () => this.updateContainer());
    this.updateContainer();
  }

  get isOpen(): boolean {
    return !this.panel.hidden;
  }

  toggle(): void {
    if (this.isOpen) this.close();
    else this.open();
  }

  open(): void {
    if (!this.isOpen) {
      this.panel.hidden = false;
      this.host.onOpenChange(true);
      this.render();
    }
    this.search.focus();
    this.search.select();
    this.host.library.refresh().catch((e) => this.host.error(e));
  }

  close(): void {
    if (!this.isOpen) return;
    this.panel.hidden = true;
    clearTimeout(this.hoverTimer);
    this.host.onOpenChange(false);
    this.host.focusEditor();
  }

  /** Esc: closes an open form, else the drawer. */
  escape(): boolean {
    if (!this.isOpen) return false;
    if (!this.pasteForm.hidden || !this.addForm.hidden) this.showForm(null);
    else this.close();
    return true;
  }

  private render(): void {
    const lib = this.host.library;
    // Styles
    if (this.styleSelect.options.length !== lib.styles.length) {
      this.styleSelect.replaceChildren(
        ...lib.styles.map((s) => {
          const o = document.createElement("option");
          o.value = s.id;
          o.textContent = s.name;
          return o;
        }),
      );
    }
    this.styleSelect.value = lib.style;
    // Problems in references.bib
    const errors = lib.errors;
    this.problems.hidden = errors.length === 0;
    this.problemsSummary.textContent = `${errors.length} ${errors.length === 1 ? "entry" : "entries"} in references.bib could not be read`;
    this.problemsList.replaceChildren(
      ...errors.map((e) => {
        const li = document.createElement("li");
        li.textContent = `Line ${e.line}, column ${e.column}: ${e.message}`;
        return li;
      }),
    );
    this.renderList();
  }

  private renderList(): void {
    const lib = this.host.library;
    const shown = filterEntries(lib.entries as BibEntry[], lib.index, this.search.value);
    const items = shown.slice(0, MAX_SHOWN).map((e) => {
      const li = document.createElement("li");
      li.className = "src-entry";
      const button = document.createElement("button");
      button.type = "button";
      button.className = "src-item";
      button.dataset.cite = e.key;
      button.title = `Insert @${e.key} at the cursor`;
      const meta = document.createElement("span");
      meta.className = "src-meta";
      meta.textContent = entryDetail(e) || "—";
      const title = document.createElement("span");
      title.className = "src-title";
      title.textContent = e.title ?? "(untitled)";
      const key = document.createElement("code");
      key.className = "src-key";
      key.textContent = e.key;
      button.append(meta, title, key);
      const expand = document.createElement("button");
      expand.type = "button";
      expand.className = "src-expand";
      expand.dataset.expand = "";
      expand.setAttribute("aria-expanded", "false");
      expand.setAttribute("aria-label", `Show the reference for ${e.key}`);
      expand.title = "Show the formatted reference";
      expand.textContent = "›";
      const ref = document.createElement("div");
      ref.className = "src-ref";
      ref.dataset.key = e.key;
      ref.hidden = true;
      li.append(button, expand, ref);
      return li;
    });
    this.list.replaceChildren(...items);
    const total = lib.entries.length;
    if (!lib.loaded) this.count.textContent = "Loading sources…";
    else if (total === 0) this.count.textContent = "No sources yet. Import a .bib file, paste BibTeX or add one.";
    else if (shown.length === 0) this.count.textContent = "No source matches.";
    else if (shown.length > MAX_SHOWN) this.count.textContent = `Showing ${MAX_SHOWN} of ${shown.length}. Type to narrow.`;
    else this.count.textContent = shown.length === total ? `${total} ${total === 1 ? "source" : "sources"}` : `${shown.length} of ${total}`;
  }

  private async fillReference(el: HTMLElement): Promise<void> {
    const key = el.dataset.key!;
    el.textContent = "…";
    try {
      el.textContent = await this.host.library.format(key);
      el.classList.remove("src-ref-error");
    } catch (e) {
      el.textContent = e instanceof Error ? e.message : String(e);
      el.classList.add("src-ref-error");
    }
  }

  private async showPreview(key: string): Promise<void> {
    this.previewKey = key;
    this.preview.hidden = false;
    try {
      const text = await this.host.library.format(key);
      if (this.previewKey === key) {
        this.preview.textContent = text;
        this.preview.classList.remove("src-ref-error");
      }
    } catch (e) {
      if (this.previewKey === key) {
        this.preview.textContent = e instanceof Error ? e.message : String(e);
        this.preview.classList.add("src-ref-error");
      }
    }
  }

  private showForm(which: "paste" | "add" | null): void {
    this.pasteForm.hidden = which !== "paste";
    this.addForm.hidden = which !== "add";
    byId("src-paste-toggle").setAttribute("aria-expanded", String(which === "paste"));
    byId("src-add-toggle").setAttribute("aria-expanded", String(which === "add"));
    if (which === "paste") this.pasteText.focus();
    else if (which === "add") {
      this.keyEdited = this.keyInput.value.trim() !== "";
      byId<HTMLTextAreaElement>("src-authors").focus();
    } else this.search.focus();
  }

  private showReport(text: string, error = false, details: string[] = []): void {
    this.report.hidden = false;
    this.report.classList.toggle("src-report-error", error);
    const p = document.createElement("p");
    p.textContent = text;
    const dismiss = document.createElement("button");
    dismiss.type = "button";
    dismiss.className = "bar-dismiss";
    dismiss.textContent = "×";
    dismiss.title = "Dismiss";
    dismiss.setAttribute("aria-label", "Dismiss");
    dismiss.addEventListener("click", () => (this.report.hidden = true));
    p.append(dismiss);
    this.report.replaceChildren(p);
    if (details.length) {
      const ul = document.createElement("ul");
      ul.append(
        ...details.map((d) => {
          const li = document.createElement("li");
          li.textContent = d;
          return li;
        }),
      );
      this.report.append(ul);
    }
  }

  private reportImport(report: ImportReport): void {
    this.showReport(importSummary(report), report.added.length === 0 && report.skipped_duplicates.length === 0);
    const errors = this.host.library.errors;
    if (errors.length) this.problems.querySelector("details")!.open = true;
  }

  private async importFile(): Promise<void> {
    try {
      const report = await this.host.library.importFile();
      if (report) this.reportImport(report);
    } catch (e) {
      this.showReport(e instanceof Error ? e.message : String(e), true);
    }
  }

  private async importPasted(): Promise<void> {
    const text = this.pasteText.value;
    if (!text.trim()) return;
    try {
      const report = await this.host.library.importText(text);
      this.reportImport(report);
      this.pasteText.value = "";
      this.showForm(null);
    } catch (e) {
      // The core names the line and column of the first parse error.
      this.showReport(e instanceof Error ? e.message : String(e), true);
    }
  }

  // ---- Add source ----

  private field(id: string): string {
    return (byId<HTMLInputElement>(id)).value;
  }

  private authors(): string[] {
    return this.field("src-authors")
      .split("\n")
      .map((a) => a.trim())
      .filter(Boolean);
  }

  private type(): SourceType {
    return this.field("src-type") as SourceType;
  }

  private updateContainer(): void {
    const container = CONTAINER[this.type()];
    const row = byId("src-container-row");
    row.hidden = container === null;
    if (container) byId("src-container-label").textContent = CONTAINER_LABEL[container];
  }

  private updateKey(): void {
    if (this.keyEdited) return;
    const title = this.field("src-title");
    const authors = this.authors();
    const year = this.field("src-year");
    this.keyInput.value = authors.length || title.trim() || year.trim() ? makeKey(authors, year, title, this.host.library.entries.map((e) => e.key)) : "";
  }

  private async addSource(): Promise<void> {
    const key = this.keyInput.value.trim();
    if (!KEY_RE.test(key)) {
      this.keyInput.setCustomValidity("Use letters, digits and - _ : . (no spaces).");
      this.keyInput.reportValidity();
      return;
    }
    if (this.host.library.entries.some((e) => e.key === key)) {
      this.keyInput.setCustomValidity(`“${key}” is already in the library.`);
      this.keyInput.reportValidity();
      return;
    }
    const title = this.field("src-title");
    if (!title.trim()) {
      const input = byId<HTMLInputElement>("src-title");
      input.setCustomValidity("A title is needed.");
      input.reportValidity();
      input.addEventListener("input", () => input.setCustomValidity(""), { once: true });
      return;
    }
    try {
      const text = toBibLatex({
        type: this.type(),
        key,
        authors: this.authors(),
        title,
        year: this.field("src-year"),
        container: this.field("src-container"),
        doi: this.field("src-doi"),
        url: this.field("src-url"),
      });
      const report = await this.host.library.importText(text);
      this.reportImport(report);
      if (report.added.length) {
        this.addForm.reset();
        this.keyEdited = false;
        this.updateContainer();
        this.showForm(null);
        this.search.value = key;
        this.renderList();
      }
    } catch (e) {
      this.showReport(e instanceof Error ? e.message : String(e), true);
    }
  }
}
