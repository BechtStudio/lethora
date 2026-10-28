// The asset explorer (F8, or "Assets" in the bar): a panel docked at the
// bottom of every view, resized by its top edge. Left the folder tree,
// right the open folder (subfolders, then items; the Sources in the root)
// as a grid of preview tiles or a sortable list; search and "Unused" look
// in all folders. Every item shows a status cloud (on this device or not).
// The context menu and the keyboard replace, rename, save, show usages,
// insert and remove; drag and drop or Ctrl+X/Ctrl+V move items and folders.
// Files dropped from the desktop are imported into the folder under the
// pointer (a tree node or a folder tile), else into the open folder.
// Double-click or Enter opens a folder, an image's large preview, or the
// Sources drawer.
//
// The pure parts are in `assets.ts`. No Tauri imports: commands come in
// through `AssetsApi`.

import {
  ASSET_DRAG_TYPE,
  type AssetItem,
  type AssetList,
  type AssetStatus,
  ASSETS_STEP,
  clampHeight,
  type Entry,
  entryKey,
  entryName,
  extensionOf,
  filterEntries,
  findUsages,
  folderEntries,
  type FolderNode,
  folderTree,
  FOLDER_DRAG_TYPE,
  folderUsed,
  formatExtensions,
  formatSize,
  hasFolder,
  isFiltering,
  moveIndex,
  moveTarget,
  nameOf,
  newFolderPath,
  parentOf,
  parseHeight,
  renameTarget,
  ROOT,
  type Sort,
  type SortKey,
  sortEntries,
  STATUSES,
} from "./assets.ts";
import { ASSET_TYPES, type AssetType } from "./assettypes.ts";
import { skippedMessage, supportedFiles } from "./drop.ts";
import { AssetCache, type LoadAsset } from "./figures.ts";
import { type MenuItem, type MenuModel, PageMenu } from "./pagemenu.ts";

export interface AssetsApi {
  list(): Promise<AssetList>;
  /** An asset's bytes (`asset_bytes`). */
  bytes: LoadAsset;
  /** Imports the file `source` into `folder`; resolves to its path. */
  import(source: string, folder: string): Promise<string>;
  /** Imports a file read in the web view (named `name`) into `folder`; resolves to its path. */
  importBytes(name: string, bytes: ArrayBuffer, folder: string): Promise<string>;
  /** Gives `path` the content of the file `source` (same format). */
  update(path: string, source: string): Promise<void>;
  /** Writes `path`'s bytes to the file `target`. */
  save(path: string, target: string): Promise<void>;
  /** Renames or moves an item or folder; the text's literals change (through the edit queue). */
  move(from: string, to: string): Promise<void>;
  remove(path: string): Promise<void>;
  createFolder(path: string): Promise<void>;
  /** Asks for files with `extensions`; [] if cancelled. */
  pickFiles(title: string, extensions: string[], multiple: boolean): Promise<string[]>;
  /** Asks where to save a file named `name`; null if cancelled. */
  pickTarget(title: string, name: string, extensions: string[]): Promise<string | null>;
}

export interface AssetsHost {
  api: AssetsApi;
  /** Sends pending edits (before reading usage, before an asset step). */
  drain(): Promise<void>;
  /** The current text (for Show usages). */
  text(): string;
  /** Selects `from..to` in the text (in a text view). */
  reveal(from: number, to: number): void;
  /** Whether a figure can be inserted at the cursor now (author and science views). */
  canInsert(): boolean;
  insertFigure(path: string): void;
  /** How many sources there are, or null until known. */
  sourceCount(): number | null;
  /** Opens the Sources drawer. */
  openSources(): void;
  /** An asset step changed the index or a file: figure previews reload. */
  changed(): void;
  /** Puts the focus back on the editor (or the pages in the publisher view). */
  focusMain(): void;
  /** The panel opened or closed. */
  shown(open: boolean): void;
  info(message: string): void;
  error(e: unknown): void;
}

/** What a menu item does. */
type AssetAct = { op: "open" | "replace" | "rename" | "save" | "usages" | "insert" | "remove" | "new_folder" | "import"; key: string } | { op: "reveal"; from: number; to: number };

const REFRESH_MS = 300;
const TILE_MIN = 64;
const TILE_MAX = 192;
const TILE_DEFAULT = 104;
const SVG_NS = "http://www.w3.org/2000/svg";

const COLUMNS: ReadonlyArray<{ key: SortKey; label: string }> = [
  { key: "name", label: "Name" },
  { key: "type", label: "Type" },
  { key: "size", label: "Size" },
  { key: "used", label: "Used by" },
  { key: "status", label: "Status" },
];

/** The cloud's outline, and a mark inside it per status (add sync states here). */
const CLOUD = "M6.5 19a4.5 4.5 0 0 1-.4-9A6 6 0 0 1 17.6 8.5 5.25 5.25 0 0 1 17.5 19z";
const CLOUD_MARKS: Record<AssetStatus, string> = {
  available: "M9 14l2 2 4-4.5",
  missing: "M4 4l16 16",
};

function el<K extends keyof HTMLElementTagNameMap>(tag: K, props: Partial<HTMLElementTagNameMap[K]> = {}, ...children: Array<Node | string>): HTMLElementTagNameMap[K] {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...children);
  return e;
}

/** The status cloud (inline SVG), titled for its status. */
function cloud(status: AssetStatus): HTMLElement {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  for (const d of [CLOUD, CLOUD_MARKS[status]]) {
    const path = document.createElementNS(SVG_NS, "path");
    path.setAttribute("d", d);
    svg.append(path);
  }
  const span = el("span", { className: "as-cloud", title: STATUSES[status].title });
  span.dataset.status = status;
  span.append(svg);
  return span;
}

const typeOf = (item: AssetItem): AssetType | undefined => (ASSET_TYPES as Record<string, AssetType>)[item.kind];
const times = (n: number) => (n === 1 ? "once" : `${n.toLocaleString("en-US")} times`);
/** A folder as people read it: `assets/Photos` (the root `assets`). */
const folderLabel = (folder: string) => folder.slice(0, -1);
/** Whether a drag carries files (from the desktop). */
const hasFiles = (e: DragEvent) => e.dataTransfer?.types.includes("Files") ?? false;

function loadPref(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function savePref(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // not persisted
  }
}

export class AssetsPanel {
  readonly el: HTMLElement;
  private host: AssetsHost;
  private open = false;
  private list: AssetList = { folders: [], items: [] };
  private tree: FolderNode = folderTree(this.list);
  /** The open folder (ends in `/`). */
  private folder = loadPref("lethora.assetsFolder") ?? ROOT;
  private expanded = new Set<string>([ROOT]);
  private filter = { query: "", unused: false };
  private mode: "grid" | "list" = loadPref("lethora.assetsMode") === "list" ? "list" : "grid";
  private tile = Math.max(TILE_MIN, Math.min(TILE_MAX, Number(loadPref("lethora.assetsTile")) || TILE_DEFAULT));
  private sort: Sort = { key: "name", descending: false };
  private height = parseHeight(loadPref("lethora.assetsHeight"));
  /** The entries shown, in order, and the selected one's key (and index, for when it disappears). */
  private entries: Entry[] = [];
  private selected: string | null = null;
  private selectedIndex = 0;
  /** The path cut with Ctrl+X, moved by Ctrl+V. */
  private cut: string | null = null;
  /** The path dragged now. */
  private dragging: string | null = null;
  /** The entry renamed inline (rendering waits until it ends). */
  private renaming: string | null = null;
  private refreshTimer: ReturnType<typeof setTimeout> | undefined;
  private generation = 0;
  private cache: AssetCache;
  private observer: IntersectionObserver;
  /** Thumbnails shown (by path) and those scrolled into view. */
  private thumbs = new Map<string, HTMLElement>();
  private visible = new Set<string>();
  private menu = new PageMenu<AssetAct>();
  private menuReturn: HTMLElement | null = null;
  private viewer: { el: HTMLElement; path: string; opener: HTMLElement | null } | null = null;

  private handle: HTMLElement;
  private whereEl: HTMLElement;
  private countEl: HTMLElement;
  private search: HTMLInputElement;
  private unusedChip: HTMLButtonElement;
  private gridButton: HTMLButtonElement;
  private listButton: HTMLButtonElement;
  private sizeInput: HTMLInputElement;
  private treeEl: HTMLUListElement;
  private head: HTMLElement;
  private content: HTMLElement;
  private empty: HTMLElement;

  constructor(parent: HTMLElement, host: AssetsHost) {
    this.host = host;
    this.el = parent;
    this.cache = new AssetCache(host.api.bytes, () => this.showThumbs(), 160);

    this.handle = el("div", { id: "assets-handle", tabIndex: 0 });
    this.handle.setAttribute("role", "separator");
    this.handle.setAttribute("aria-orientation", "horizontal");
    this.handle.setAttribute("aria-label", "Resize the assets panel");
    this.handle.setAttribute("aria-controls", "assets");
    this.handle.setAttribute("aria-valuemin", "0");
    this.handle.setAttribute("aria-valuemax", "100");
    this.wireHandle();

    this.whereEl = el("span", { className: "as-where" });
    this.countEl = el("span", { className: "as-count" });
    this.countEl.setAttribute("aria-live", "polite");
    this.search = el("input", { type: "search", className: "as-search", placeholder: "Search all folders…", autocomplete: "off", spellcheck: false });
    this.search.setAttribute("aria-label", "Search the assets of all folders by name or folder");
    this.search.addEventListener("input", () => {
      this.filter.query = this.search.value;
      this.render();
    });
    this.search.addEventListener("keydown", (e) => {
      if (e.key !== "ArrowDown" || e.altKey || e.ctrlKey || e.metaKey) return;
      e.preventDefault();
      this.focusSelected();
    });
    this.unusedChip = this.button("Unused", "Only the items the text does not use (in all folders)", () => {
      this.filter.unused = !this.filter.unused;
      this.render();
    });
    this.unusedChip.classList.add("as-chip");
    this.gridButton = this.button("Grid", "Preview tiles", () => this.setMode("grid"));
    this.listButton = this.button("List", "A list with name, type, size, usage and status", () => this.setMode("list"));
    const modes = el("span", { className: "as-modes" }, this.gridButton, this.listButton);
    modes.setAttribute("role", "group");
    modes.setAttribute("aria-label", "Show as");
    this.sizeInput = el("input", { type: "range", className: "as-size", min: String(TILE_MIN), max: String(TILE_MAX), step: "8", value: String(this.tile) });
    this.sizeInput.setAttribute("aria-label", "Tile size");
    this.sizeInput.addEventListener("input", () => {
      this.tile = Number(this.sizeInput.value);
      this.content.style.setProperty("--as-tile", `${this.tile}px`);
      savePref("lethora.assetsTile", String(this.tile));
    });
    const importButton = this.button("Import…", "Import files into the open folder", () => void this.importInto(this.folder));
    const folderButton = this.button("New folder", "A new folder in the open folder", () => void this.newFolder(this.folder));
    // Disabled while the project is read only (see readonly.ts).
    importButton.dataset.write = folderButton.dataset.write = "";
    const close = this.button("×", "Close the assets (F8)", () => this.close());
    close.setAttribute("aria-label", "Close the assets");
    close.classList.add("as-close");
    const toolbar = el(
      "div",
      { className: "as-toolbar" },
      el("h2", { className: "as-title", textContent: "Assets" }),
      this.whereEl,
      this.countEl,
      el("span", { className: "as-spacer" }),
      this.search,
      this.unusedChip,
      modes,
      this.sizeInput,
      importButton,
      folderButton,
      close,
    );

    this.treeEl = el("ul", { className: "as-tree" });
    this.treeEl.setAttribute("role", "tree");
    this.treeEl.setAttribute("aria-label", "Folders");
    this.treeEl.addEventListener("keydown", (e) => this.onTreeKey(e));

    this.head = el("div", { className: "as-head" });
    this.head.setAttribute("role", "row");
    for (const c of COLUMNS) {
      const b = el("button", { type: "button", className: `as-col as-c-${c.key}`, textContent: c.label, title: `Sort by ${c.label.toLowerCase()}` });
      b.dataset.key = c.key;
      b.setAttribute("role", "columnheader");
      b.addEventListener("click", () => {
        this.sort = this.sort.key === c.key ? { key: c.key, descending: !this.sort.descending } : { key: c.key, descending: false };
        this.render();
      });
      this.head.append(b);
    }
    this.content = el("div", { className: "as-content", tabIndex: 0 });
    this.content.setAttribute("role", "listbox");
    this.content.style.setProperty("--as-tile", `${this.tile}px`);
    this.content.addEventListener("keydown", (e) => this.onKey(e));
    this.content.addEventListener("click", (e) => this.onClick(e));
    this.content.addEventListener("dblclick", (e) => {
      const key = this.keyAt(e.target);
      if (key !== null) this.activate(key);
    });
    this.content.addEventListener("contextmenu", (e) => this.onContextMenu(e));
    this.content.addEventListener("focus", () => {
      if (document.activeElement === this.content && this.entries.length) this.focusSelected();
    });
    this.empty = el("p", { className: "as-empty", hidden: true });
    const main = el("div", { className: "as-main" }, this.head, this.content, this.empty);
    const body = el("div", { className: "as-body" }, this.treeEl, main);
    this.el.append(this.handle, toolbar, body);
    this.wireFileDrop();

    this.observer = new IntersectionObserver(
      (records) => {
        for (const r of records) {
          const path = (r.target as HTMLElement).dataset.thumb!;
          if (r.isIntersecting) this.visible.add(path);
          else this.visible.delete(path);
        }
        this.cache.want([...this.visible]);
      },
      { root: this.content, rootMargin: "120px" },
    );
    window.addEventListener("resize", () => {
      if (this.open) this.applyHeight();
    });
  }

  get isOpen(): boolean {
    return this.open;
  }

  /** Whether `target` is inside the panel or its large preview. */
  contains(target: EventTarget | null): boolean {
    return target instanceof Node && (this.el.contains(target) || (this.viewer?.el.contains(target) ?? false));
  }

  toggle(): void {
    if (this.open) this.close();
    else this.show();
  }

  /** Opens it with the focus in the open folder's contents; the listing reloads. */
  show(): void {
    if (!this.open) {
      this.open = true;
      this.el.hidden = false;
      document.body.toggleAttribute("data-assets", true);
      this.applyHeight();
      this.host.shown(true);
      this.render();
      void this.refresh();
    }
    this.focusSelected();
  }

  /** Closes it; focus inside goes back to the editor. */
  close(): void {
    if (!this.open) return;
    const focused = this.el.contains(document.activeElement);
    this.menu.close(false);
    this.closeViewer(false);
    this.renaming = null;
    this.cut = null;
    this.open = false;
    this.el.hidden = true;
    document.body.toggleAttribute("data-assets", false);
    this.observer.disconnect();
    this.thumbs.clear();
    this.visible.clear();
    this.host.shown(false);
    if (focused) this.host.focusMain();
  }

  /** Esc: closes the menu, the large preview or an inline rename, or drops the cut mark; true if it did. */
  escape(): boolean {
    if (this.menu.escape()) return true;
    if (this.viewer) {
      this.closeViewer(true);
      return true;
    }
    if (this.renaming !== null) {
      this.endRename();
      return true;
    }
    if (this.cut !== null && this.el.contains(document.activeElement)) {
      this.cut = null;
      this.render();
      return true;
    }
    return false;
  }

  /** Esc with nothing else to close: the focus leaves the panel for the editor; true if it was inside. */
  leave(): boolean {
    if (!this.open || !this.el.contains(document.activeElement)) return false;
    this.host.focusMain();
    return true;
  }

  /** A new version (edits, undo, imports): the listing reloads after a pause while open. */
  changed(): void {
    if (!this.open) return;
    clearTimeout(this.refreshTimer);
    this.refreshTimer = setTimeout(() => void this.refresh(), REFRESH_MS);
  }

  /** Files may differ now (undo, restore, F5): previews load anew and the listing reloads. */
  reload(): void {
    this.cache.clear();
    if (this.open) this.changed();
  }

  /** The sources changed: their count in the root follows. */
  sourcesChanged(): void {
    if (this.open && !isFiltering(this.filter) && this.folder === ROOT) this.render();
  }

  /** Another project: forget the listing, previews, selection and marks (the open folder is checked anew). */
  reset(): void {
    this.generation++;
    this.menu.close(false);
    this.closeViewer(false);
    this.renaming = null;
    this.cut = null;
    this.selected = null;
    this.selectedIndex = 0;
    this.list = { folders: [], items: [] };
    this.tree = folderTree(this.list);
    this.expanded = new Set([ROOT]);
    this.cache.clear();
    if (this.open) {
      this.render();
      void this.refresh();
    }
  }

  // ---- Listing ----

  private async refresh(): Promise<void> {
    clearTimeout(this.refreshTimer);
    const generation = ++this.generation;
    try {
      await this.host.drain();
      const list = await this.host.api.list();
      if (generation !== this.generation || !this.open) return;
      this.list = list;
      this.tree = folderTree(list);
      // A folder that went away (moved, removed, undone): its nearest parent still there.
      if (!this.folderExists(this.folder)) this.openFolder(this.folder, false);
      this.render();
    } catch (e) {
      this.host.error(e);
    }
  }

  private folderExists(folder: string): boolean {
    return hasFolder(this.list, folder);
  }

  /** Where files dropped into the text are imported: the open folder (remembered while the panel is closed) if it still exists, else the root. */
  async importFolder(): Promise<string> {
    if (this.folder === ROOT) return ROOT;
    return hasFolder(await this.host.api.list(), this.folder) ? this.folder : ROOT;
  }

  private item(path: string): AssetItem | undefined {
    return this.list.items.find((i) => i.path === path);
  }

  /** Opens `folder` (or its nearest existing parent), expanding the tree down to it. */
  private openFolder(folder: string, render = true): void {
    let f = folder.startsWith(ROOT) && folder.endsWith("/") ? folder : ROOT;
    while (f !== ROOT && !this.folderExists(f)) f = parentOf(f);
    if (f !== this.folder) {
      this.folder = f;
      this.selected = null;
      this.selectedIndex = 0;
      savePref("lethora.assetsFolder", f);
    }
    for (let p = f; p !== ROOT; p = parentOf(p)) this.expanded.add(p);
    if (render) this.render();
  }

  // ---- Rendering ----

  private button(text: string, title: string, run: () => void): HTMLButtonElement {
    const b = el("button", { type: "button", className: "as-button", textContent: text, title });
    b.addEventListener("click", run);
    return b;
  }

  private setMode(mode: "grid" | "list"): void {
    this.mode = mode;
    savePref("lethora.assetsMode", mode);
    this.render();
    this.focusSelected();
  }

  private render(): void {
    if (!this.open || this.renaming !== null) return;
    const filtering = isFiltering(this.filter);
    const shown = filtering ? filterEntries(this.list, this.filter) : folderEntries(this.list, this.tree, this.folder, true);
    this.entries = sortEntries(shown, this.sort);
    let index = this.selected === null ? -1 : this.entries.findIndex((e) => entryKey(e) === this.selected);
    if (index < 0 && this.entries.length) {
      index = Math.min(this.selectedIndex, this.entries.length - 1);
      this.selected = entryKey(this.entries[index]);
    } else if (index < 0) this.selected = null;
    this.selectedIndex = Math.max(0, index);

    this.whereEl.textContent = filtering ? "All folders" : folderLabel(this.folder);
    const items = this.entries.filter((e) => e.type === "item").length;
    this.countEl.textContent = filtering ? `${items} ${items === 1 ? "match" : "matches"}` : "";
    this.unusedChip.setAttribute("aria-pressed", String(this.filter.unused));
    this.gridButton.setAttribute("aria-pressed", String(this.mode === "grid"));
    this.listButton.setAttribute("aria-pressed", String(this.mode === "list"));
    this.sizeInput.hidden = this.mode !== "grid";
    this.head.hidden = this.mode !== "list";
    for (const b of this.head.querySelectorAll<HTMLElement>(".as-col")) {
      const on = b.dataset.key === this.sort.key;
      b.setAttribute("aria-sort", on ? (this.sort.descending ? "descending" : "ascending") : "none");
      b.dataset.arrow = on ? (this.sort.descending ? "↓" : "↑") : "";
    }
    this.renderTree();

    const hadFocus = this.content.contains(document.activeElement);
    const scrollTop = this.content.scrollTop;
    this.observer.disconnect();
    this.thumbs.clear();
    this.visible.clear();
    this.content.classList.toggle("as-grid", this.mode === "grid");
    this.content.classList.toggle("as-list", this.mode === "list");
    this.content.setAttribute("aria-label", filtering ? "Search results in all folders" : `Contents of ${folderLabel(this.folder)}`);
    this.content.replaceChildren(...this.entries.map((e) => (this.mode === "grid" ? this.tileFor(e, filtering) : this.rowFor(e, filtering))));
    this.content.scrollTop = scrollTop;
    this.empty.hidden = this.entries.length > 0;
    this.empty.textContent = filtering ? "Nothing matches in any folder." : "This folder is empty. Import files or drag items here.";
    this.markSelected();
    this.showThumbs();
    if (hadFocus) this.focusSelected();
  }

  private renderTree(): void {
    const hadFocus = this.treeEl.contains(document.activeElement);
    const focusPath = hadFocus ? ((document.activeElement as HTMLElement).dataset.path ?? null) : null;
    const node = (n: FolderNode, level: number): HTMLLIElement => {
      const open = n.path === ROOT || this.expanded.has(n.path);
      const current = n.path === this.folder && !isFiltering(this.filter);
      const caret = el("span", { className: "as-caret", textContent: n.children.length ? (open ? "▾" : "▸") : "" });
      caret.addEventListener("click", (e) => {
        e.stopPropagation();
        this.toggleExpanded(n.path);
      });
      const row = el("div", { className: "as-node" }, caret, el("span", { className: "asset-icon-folder" }), el("span", { className: "as-node-name", textContent: n.name }));
      const li = el("li", { className: `as-folder${current ? " current" : ""}`, tabIndex: -1 }, row);
      li.dataset.path = n.path;
      li.setAttribute("role", "treeitem");
      li.setAttribute("aria-level", String(level));
      li.setAttribute("aria-selected", String(current));
      li.setAttribute("aria-label", n.name);
      if (n.children.length) li.setAttribute("aria-expanded", String(open));
      row.addEventListener("click", () => {
        this.filter = { query: "", unused: false };
        this.search.value = "";
        this.openFolder(n.path);
      });
      row.addEventListener("contextmenu", (e) => {
        e.preventDefault();
        this.openMenu(e.clientX, e.clientY, this.folderMenu(n.path), li);
      });
      this.dropTarget(row, n.path);
      if (n.path !== ROOT) this.dragSource(row, n.path);
      if (open && n.children.length) {
        const group = el("ul", {}, ...n.children.map((c) => node(c, level + 1)));
        group.setAttribute("role", "group");
        li.append(group);
      }
      return li;
    };
    this.treeEl.replaceChildren(node(this.tree, 1));
    const nodes = this.treeNodes();
    const focusable = nodes.find((n) => n.dataset.path === (focusPath ?? this.folder)) ?? nodes[0];
    focusable.tabIndex = 0;
    if (hadFocus) focusable.focus({ preventScroll: true });
  }

  private tileFor(e: Entry, filtering: boolean): HTMLElement {
    const thumb = el("div", { className: "as-thumb" });
    const tile = el("div", { className: "as-tile" }, thumb, el("span", { className: "as-name", textContent: entryName(e), title: entryName(e) }));
    if (e.type === "item") {
      const type = typeOf(e.item);
      thumb.append(el("span", { className: `as-icon ${type?.icon ?? "asset-icon-file"}` }), cloud(e.item.status));
      if (type?.preview === "image" && e.item.status === "available") {
        thumb.dataset.thumb = e.path;
        this.thumbs.set(e.path, thumb);
        this.observer.observe(thumb);
      }
      if (filtering) tile.append(el("span", { className: "as-sub", textContent: folderLabel(parentOf(e.path)) }));
    } else if (e.type === "folder") thumb.append(el("span", { className: "as-icon asset-icon-folder" }));
    else {
      thumb.append(el("span", { className: "as-icon asset-icon-sources" }));
      const n = this.host.sourceCount();
      if (n !== null) tile.append(el("span", { className: "as-sub", textContent: `${n} ${n === 1 ? "entry" : "entries"}` }));
    }
    return this.option(tile, e);
  }

  private rowFor(e: Entry, filtering: boolean): HTMLElement {
    const cell = (key: string, ...children: Array<Node | string>) => el("span", { className: `as-cell as-c-${key}` }, ...children);
    const icon = e.type === "item" ? (typeOf(e.item)?.icon ?? "asset-icon-file") : e.type === "folder" ? "asset-icon-folder" : "asset-icon-sources";
    const name = cell("name", el("span", { className: `as-icon ${icon}` }), el("span", { className: "as-name", textContent: entryName(e), title: entryName(e) }));
    let cells: HTMLElement[];
    if (e.type === "item") {
      if (filtering) name.append(el("span", { className: "as-sub", textContent: folderLabel(parentOf(e.path)) }));
      const s = e.item.status;
      cells = [
        name,
        cell("type", `${typeOf(e.item)?.label ?? e.item.kind} · ${extensionOf(e.path).toUpperCase()}`),
        cell("size", formatSize(e.item.size)),
        cell("used", e.item.used_by ? String(e.item.used_by) : "–"),
        cell("status", cloud(s), STATUSES[s].label),
      ];
    } else {
      const n = e.type === "sources" ? this.host.sourceCount() : null;
      cells = [name, cell("type", e.type === "folder" ? "Folder" : "Bibliography"), cell("size", n === null ? "" : `${n} ${n === 1 ? "entry" : "entries"}`), cell("used", ""), cell("status", "")];
    }
    return this.option(el("div", { className: "as-row" }, ...cells), e);
  }

  /** Makes `node` the option for `e`: key, label, drag source, drop target for folders. */
  private option(node: HTMLElement, e: Entry): HTMLElement {
    const key = entryKey(e);
    node.dataset.key = key;
    node.tabIndex = -1;
    node.setAttribute("role", "option");
    node.classList.toggle("cut", key === this.cut);
    let label = entryName(e);
    if (e.type === "item") label += `, ${typeOf(e.item)?.label ?? e.item.kind}, ${formatSize(e.item.size)}, ${e.item.used_by ? `used ${times(e.item.used_by)}` : "unused"}, ${STATUSES[e.item.status].label.toLowerCase()}`;
    else label += e.type === "folder" ? ", folder" : ", the bibliography";
    node.setAttribute("aria-label", label);
    if (e.type !== "sources") this.dragSource(node, e.path);
    if (e.type === "folder") this.dropTarget(node, e.path);
    return node;
  }

  private markSelected(): void {
    for (const o of this.content.children) {
      const on = (o as HTMLElement).dataset.key === this.selected;
      o.setAttribute("aria-selected", String(on));
      o.classList.toggle("selected", on);
      (o as HTMLElement).tabIndex = on ? 0 : -1;
    }
    // Tab reaches the selected option, or the list itself while it is empty.
    this.content.tabIndex = this.optionEl(this.selected) ? -1 : 0;
  }

  /** Puts the loaded previews into their tiles (as they arrive). */
  private showThumbs(): void {
    for (const [path, thumb] of this.thumbs) {
      const state = this.cache.get(path);
      if (!state || !("url" in state) || thumb.querySelector("img")?.src === state.url) continue;
      thumb.querySelector("img")?.remove();
      thumb.prepend(el("img", { src: state.url, alt: "", draggable: false }));
      thumb.classList.add("loaded");
    }
    if (this.viewer) this.fillViewer();
  }

  // ---- Selection and focus ----

  private optionEl(key: string | null): HTMLElement | null {
    return key === null ? null : this.content.querySelector<HTMLElement>(`[data-key="${CSS.escape(key)}"]`);
  }

  private focusSelected(): void {
    const o = this.optionEl(this.selected);
    if (o) {
      o.focus({ preventScroll: true });
      o.scrollIntoView({ block: "nearest" });
    } else this.content.focus({ preventScroll: true });
  }

  private select(key: string, focus = true): void {
    this.selected = key;
    this.selectedIndex = Math.max(0, this.entries.findIndex((e) => entryKey(e) === key));
    this.markSelected();
    if (focus) this.focusSelected();
  }

  private entry(key: string | null): Entry | undefined {
    return this.entries.find((e) => entryKey(e) === key);
  }

  private keyAt(target: EventTarget | null): string | null {
    const o = target instanceof Element ? target.closest<HTMLElement>("[role=option]") : null;
    return o && this.content.contains(o) ? (o.dataset.key ?? null) : null;
  }

  /** The tiles per row (1 in the list). */
  private columns(): number {
    if (this.mode === "list") return 1;
    const options = [...this.content.children] as HTMLElement[];
    const top = options[0]?.offsetTop;
    let n = 0;
    while (n < options.length && options[n].offsetTop === top) n++;
    return Math.max(1, n);
  }

  // ---- Mouse and keyboard ----

  private onClick(e: MouseEvent): void {
    if (e.target instanceof HTMLInputElement) return;
    const key = this.keyAt(e.target);
    if (key !== null) this.select(key);
  }

  private onContextMenu(e: MouseEvent): void {
    if (e.target instanceof HTMLInputElement) return;
    e.preventDefault();
    const key = this.keyAt(e.target);
    if (key !== null) this.select(key);
    this.openMenu(e.clientX, e.clientY, key === null ? this.backgroundMenu() : this.entryMenu(key), key === null ? this.content : this.optionEl(key));
  }

  private onKey(e: KeyboardEvent): void {
    if (e.target instanceof HTMLInputElement) return;
    const key = this.selected;
    const mod = e.ctrlKey || e.metaKey;
    const done = () => {
      e.preventDefault();
      e.stopPropagation();
    };
    if ((e.key === "ContextMenu" && !mod) || (e.key === "F10" && e.shiftKey && !mod)) {
      done();
      const o = this.optionEl(key);
      const r = (o ?? this.content).getBoundingClientRect();
      this.openMenu(r.left + 12, o ? r.bottom - 8 : r.top + 12, key === null ? this.backgroundMenu() : this.entryMenu(key), o ?? this.content);
      return;
    }
    if (mod && !e.altKey && !e.shiftKey && (e.key === "x" || e.key === "X")) {
      done();
      const entry = this.entry(key);
      if (!entry || entry.type === "sources") return;
      this.cut = entry.path;
      this.render();
      this.host.info(`Cut ${entryName(entry)}: Ctrl+V in another folder moves it there.`);
      return;
    }
    if (mod && !e.altKey && !e.shiftKey && (e.key === "v" || e.key === "V")) {
      done();
      if (this.cut !== null) void this.moveInto(this.cut, this.folder);
      return;
    }
    if (mod || e.altKey) return;
    const next = moveIndex(this.selectedIndex, this.entries.length, this.columns(), e.key);
    if (next !== null && !e.shiftKey) {
      done();
      this.select(entryKey(this.entries[next]));
      return;
    }
    if (e.shiftKey) return;
    switch (e.key) {
      case "Enter":
        done();
        if (key !== null) this.activate(key);
        break;
      case "F2":
        done();
        if (key !== null) this.startRename(key);
        break;
      case "Delete":
        done();
        if (key !== null) void this.remove(key);
        break;
      case "Backspace":
        done();
        if (!isFiltering(this.filter) && this.folder !== ROOT) {
          const from = this.folder;
          this.openFolder(parentOf(from), false);
          this.selected = from;
          this.render();
          this.focusSelected();
        }
        break;
    }
  }

  /** Double-click / Enter: a folder opens, an image shows large, the Sources drawer opens. */
  private activate(key: string): void {
    const e = this.entry(key);
    if (!e) return;
    if (e.type === "folder") {
      this.openFolder(e.path);
      this.focusSelected();
    } else if (e.type === "sources") this.host.openSources();
    else if (typeOf(e.item)?.preview === "image") this.openViewer(e.path);
  }

  private treeNodes(): HTMLElement[] {
    return [...this.treeEl.querySelectorAll<HTMLElement>("[role=treeitem]")];
  }

  private toggleExpanded(path: string): void {
    if (path === ROOT) return;
    if (!this.expanded.delete(path)) this.expanded.add(path);
    this.renderTree();
  }

  private onTreeKey(e: KeyboardEvent): void {
    const li = (e.target as HTMLElement).closest<HTMLElement>("[role=treeitem]");
    if (!li || e.ctrlKey || e.metaKey || e.altKey) return;
    const path = li.dataset.path!;
    const nodes = this.treeNodes();
    const i = nodes.indexOf(li);
    const go = (n: HTMLElement | undefined) => {
      if (!n) return;
      for (const x of nodes) x.tabIndex = -1;
      n.tabIndex = 0;
      n.focus();
    };
    const expandable = li.hasAttribute("aria-expanded");
    const open = li.getAttribute("aria-expanded") === "true";
    if ((e.key === "ContextMenu" && !e.shiftKey) || (e.key === "F10" && e.shiftKey)) {
      e.preventDefault();
      const r = li.getBoundingClientRect();
      this.openMenu(r.left + 24, r.top + 20, this.folderMenu(path), li);
      return;
    }
    if (e.shiftKey) return;
    switch (e.key) {
      case "ArrowDown":
        go(nodes[i + 1]);
        break;
      case "ArrowUp":
        go(nodes[i - 1]);
        break;
      case "Home":
        go(nodes[0]);
        break;
      case "End":
        go(nodes[nodes.length - 1]);
        break;
      case "ArrowRight":
        if (expandable && !open) this.toggleExpanded(path);
        else if (open) go(nodes[i + 1]);
        break;
      case "ArrowLeft":
        if (expandable && open && path !== ROOT) this.toggleExpanded(path);
        else go(nodes.find((n) => n.dataset.path === parentOf(path)));
        break;
      case "Enter":
      case " ":
        this.filter = { query: "", unused: false };
        this.search.value = "";
        this.openFolder(path);
        if (e.key === "Enter") this.focusSelected();
        break;
      case "F2":
        if (path !== ROOT) this.renameFolderOf(path);
        break;
      case "Delete":
        if (path !== ROOT) void this.remove(path);
        break;
      default:
        return;
    }
    e.preventDefault();
  }

  // ---- Drag and drop (inside the panel, files from the desktop) ----

  private dragSource(node: HTMLElement, path: string): void {
    node.draggable = true;
    node.addEventListener("dragstart", (e) => {
      if (!e.dataTransfer || this.renaming !== null) return;
      e.stopPropagation();
      e.dataTransfer.setData(ASSET_DRAG_TYPE, path);
      if (path.endsWith("/")) e.dataTransfer.setData(FOLDER_DRAG_TYPE, path);
      // Moved inside the panel, copied into the text as a figure.
      e.dataTransfer.effectAllowed = "copyMove";
      this.dragging = path;
    });
    node.addEventListener("dragend", () => {
      this.dragging = null;
      this.clearDrop();
    });
  }

  /** A folder (tree node or tile) takes the panel's items and folders (moved) and desktop files (imported). */
  private dropTarget(node: HTMLElement, folder: string): void {
    const move = (e: DragEvent) => e.dataTransfer?.types.includes(ASSET_DRAG_TYPE) && this.dragging !== null && moveTarget(this.dragging, folder) !== null;
    node.addEventListener("dragover", (e) => {
      const files = hasFiles(e);
      if (!files && !move(e)) return;
      e.preventDefault();
      e.stopPropagation();
      e.dataTransfer!.dropEffect = files ? "copy" : "move";
      this.content.classList.remove("as-drop");
      node.classList.add("as-drop");
    });
    node.addEventListener("dragleave", (e) => {
      if (!node.contains(e.relatedTarget as Node | null)) node.classList.remove("as-drop");
    });
    node.addEventListener("drop", (e) => {
      node.classList.remove("as-drop");
      if (hasFiles(e)) {
        e.preventDefault();
        e.stopPropagation();
        this.clearDrop();
        void this.importFiles([...e.dataTransfer!.files], folder);
        return;
      }
      const path = e.dataTransfer?.getData(ASSET_DRAG_TYPE);
      if (!path) return;
      e.preventDefault();
      e.stopPropagation();
      void this.moveInto(path, folder);
    });
  }

  /** Desktop files dropped anywhere else in the panel go into the open folder (the contents light up). */
  private wireFileDrop(): void {
    this.el.addEventListener("dragover", (e) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      e.dataTransfer!.dropEffect = "copy";
      this.content.classList.add("as-drop");
    });
    this.el.addEventListener("dragleave", (e) => {
      if (!this.el.contains(e.relatedTarget as Node | null)) this.clearDrop();
    });
    this.el.addEventListener("drop", (e) => {
      this.clearDrop();
      if (!hasFiles(e)) return;
      e.preventDefault();
      void this.importFiles([...e.dataTransfer!.files], this.folder);
    });
  }

  private clearDrop(): void {
    for (const t of this.el.querySelectorAll(".as-drop")) t.classList.remove("as-drop");
  }

  // ---- Menus ----

  private openMenu(x: number, y: number, model: MenuModel<AssetAct>, returnTo: HTMLElement | null): void {
    this.menuReturn = returnTo;
    this.menu.open(x, y, model, {
      run: (act) => this.run(act),
      space: () => Promise.resolve(),
      closed: () => {
        const back = this.menuReturn;
        this.menuReturn = null;
        if (back?.isConnected) back.focus({ preventScroll: true });
        else this.focusSelected();
      },
    });
  }

  private entryMenu(key: string): MenuModel<AssetAct> {
    const e = this.entry(key);
    if (!e) return { header: null, items: [] };
    if (e.type === "folder") return this.folderMenu(e.path);
    if (e.type === "sources") return { header: "Sources", items: [{ type: "item", label: "Open the Sources", act: { op: "open", key }, keys: "Enter" }] };
    const it = e.item;
    const used = it.used_by > 0;
    const missing = it.status === "missing";
    const insert = this.host.canInsert();
    const items: MenuItem<AssetAct>[] = [];
    if (typeOf(it)?.preview === "image") items.push({ type: "item", label: "Open preview", act: missing ? null : { op: "open", key }, keys: "Enter", title: missing ? STATUSES.missing.title : undefined }, { type: "separator" });
    items.push(
      { type: "item", label: "Replace…", act: { op: "replace", key }, title: `New content for ${nameOf(it.path)}: every figure using it shows the new file` },
      { type: "item", label: "Rename", act: { op: "rename", key }, keys: "F2" },
      { type: "item", label: "Save as…", act: missing ? null : { op: "save", key }, title: missing ? STATUSES.missing.title : undefined },
      { type: "item", label: used ? `Show usages (${it.used_by})` : "Show usages", act: used ? { op: "usages", key } : null, title: used ? undefined : "The text does not use it" },
      { type: "item", label: "Insert as figure", act: insert ? { op: "insert", key } : null, title: insert ? "At the cursor in the text" : "Switch to the author or science view to insert it" },
      { type: "separator" },
      { type: "item", label: "Remove", act: used ? null : { op: "remove", key }, keys: used ? undefined : "Delete", title: used ? `The text uses it ${times(it.used_by)}; remove those first` : "Undo brings it back" },
    );
    return { header: nameOf(it.path), items };
  }

  private folderMenu(path: string): MenuModel<AssetAct> {
    const root = path === ROOT;
    const used = folderUsed(this.list, path);
    return {
      header: folderLabel(path),
      items: [
        { type: "item", label: "Open", act: { op: "open", key: path }, keys: "Enter" },
        { type: "item", label: "New folder", act: { op: "new_folder", key: path }, title: "A new folder inside this one" },
        { type: "item", label: "Import…", act: { op: "import", key: path }, title: "Import files into this folder" },
        { type: "item", label: "Rename", act: root ? null : { op: "rename", key: path }, keys: root ? undefined : "F2", title: root ? "The assets folder itself keeps its name" : undefined },
        { type: "separator" },
        {
          type: "item",
          label: "Remove",
          act: root || used ? null : { op: "remove", key: path },
          keys: root || used ? undefined : "Delete",
          title: root ? "The assets folder itself stays" : used ? "Something in it is used in the text" : "With everything in it; undo brings it back",
        },
      ],
    };
  }

  private backgroundMenu(): MenuModel<AssetAct> {
    return {
      header: isFiltering(this.filter) ? null : folderLabel(this.folder),
      items: [
        { type: "item", label: "New folder", act: { op: "new_folder", key: this.folder } },
        { type: "item", label: "Import…", act: { op: "import", key: this.folder } },
      ],
    };
  }

  private run(act: AssetAct): void {
    if (act.op === "reveal") {
      this.host.reveal(act.from, act.to);
      return;
    }
    const key = act.key;
    switch (act.op) {
      case "open":
        if (key.endsWith("/")) {
          this.filter = { query: "", unused: false };
          this.search.value = "";
          this.openFolder(key);
          this.focusSelected();
        } else this.activate(key);
        break;
      case "replace":
        void this.replace(key);
        break;
      case "rename":
        if (this.entry(key)) this.startRename(key);
        else this.renameFolderOf(key);
        break;
      case "save":
        void this.saveAs(key);
        break;
      case "usages":
        void this.showUsages(key);
        break;
      case "insert":
        this.host.insertFigure(key);
        break;
      case "remove":
        void this.remove(key);
        break;
      case "new_folder":
        void this.newFolder(key);
        break;
      case "import":
        void this.importInto(key);
        break;
    }
  }

  // ---- Asset steps ----

  /** Runs one asset step (pending edits sent first); the listing and the figure previews follow. */
  private async step(run: () => Promise<void>): Promise<boolean> {
    try {
      await this.host.drain();
      await run();
      this.host.changed();
      await this.refresh();
      return true;
    } catch (e) {
      this.host.error(e);
      void this.refresh();
      return false;
    }
  }

  private async moveInto(path: string, folder: string): Promise<void> {
    const to = moveTarget(path, folder);
    if (to === null) {
      if (this.cut === path) this.host.info(`${nameOf(path)} is in ${folderLabel(folder)} already.`);
      return;
    }
    if (await this.step(() => this.host.api.move(path, to))) {
      if (this.cut === path) this.cut = null;
      if (parentOf(to) === this.folder) this.select(to);
      this.render();
      this.host.info(`Moved ${nameOf(path)} to ${folderLabel(folder)}.`);
    }
  }

  private async remove(key: string): Promise<void> {
    const e = this.entry(key);
    const path = e ? (e.type === "sources" ? null : e.path) : key.endsWith("/") ? key : null;
    if (path === null || path === ROOT) return;
    const item = this.item(path);
    if (item && item.used_by > 0) {
      this.host.info(`${nameOf(path)} is used ${times(item.used_by)} in the text; remove those first.`);
      return;
    }
    if (!item && folderUsed(this.list, path)) {
      this.host.info(`Something in ${nameOf(path)} is used in the text.`);
      return;
    }
    if (await this.step(() => this.host.api.remove(path))) this.host.info(`Removed ${nameOf(path)} (undo brings it back).`);
  }

  private async replace(path: string): Promise<void> {
    const ext = extensionOf(path);
    const [source] = await this.host.api.pickFiles(`Replace ${nameOf(path)} (a .${ext} file)`, formatExtensions(ext), false).catch((e: unknown) => {
      this.host.error(e);
      return [];
    });
    if (!source) return;
    this.cache.forget(path);
    if (await this.step(() => this.host.api.update(path, source))) this.host.info(`Replaced ${nameOf(path)}: every figure using it shows the new file.`);
  }

  private async saveAs(path: string): Promise<void> {
    try {
      const target = await this.host.api.pickTarget("Save as", nameOf(path), formatExtensions(extensionOf(path)));
      if (!target) return;
      await this.host.api.save(path, target);
      this.host.info(`Saved ${nameOf(path)} to ${target}.`);
    } catch (e) {
      this.host.error(e);
    }
  }

  private async importInto(folder: string): Promise<void> {
    let sources: string[];
    try {
      const extensions = Object.values(ASSET_TYPES).flatMap((t: AssetType) => t.extensions);
      sources = await this.host.api.pickFiles(`Import into ${folderLabel(folder)}`, extensions, true);
    } catch (e) {
      this.host.error(e);
      return;
    }
    if (!sources.length) return;
    await this.importEach(
      folder,
      sources.map((source) => () => this.host.api.import(source, folder)),
      [],
    );
  }

  /** Files dropped from the desktop: the ones an asset type takes are imported into `folder`, the others named in one message. */
  private async importFiles(files: File[], folder: string): Promise<void> {
    const { ok, skipped } = supportedFiles(files);
    if (!ok.length) {
      if (skipped.length) this.host.info(skippedMessage(skipped));
      return;
    }
    await this.importEach(
      folder,
      ok.map((file) => async () => this.host.api.importBytes(file.name, await file.arrayBuffer(), folder)),
      skipped,
    );
  }

  /** Runs `imports` in order into `folder` (each its own undo step); the folder opens with the last one selected. */
  private async importEach(folder: string, imports: ReadonlyArray<() => Promise<string>>, skipped: readonly string[]): Promise<void> {
    const imported: string[] = [];
    const ok = await this.step(async () => {
      for (const run of imports) imported.push(await run());
    });
    const last = imported.at(-1);
    if (last === undefined) return;
    this.filter = { query: "", unused: false };
    this.search.value = "";
    this.openFolder(folder, false);
    this.selected = last;
    this.render();
    this.focusSelected();
    const done = imported.length === 1 ? `Imported ${nameOf(last)}.` : `Imported ${imported.length} files into ${folderLabel(folder)}.`;
    if (ok) this.host.info(skipped.length ? `${done} ${skippedMessage(skipped)}` : done);
  }

  private async newFolder(parent: string): Promise<void> {
    const path = newFolderPath(this.tree, parent);
    if (!(await this.step(() => this.host.api.createFolder(path)))) return;
    this.filter = { query: "", unused: false };
    this.search.value = "";
    this.openFolder(parent, false);
    this.selected = path;
    this.render();
    this.startRename(path);
  }

  private async showUsages(path: string): Promise<void> {
    try {
      await this.host.drain();
    } catch (e) {
      this.host.error(e);
      return;
    }
    const usages = findUsages(this.host.text(), path);
    if (!usages.length) {
      this.host.info(`${nameOf(path)} is not named in the text (only literal paths count).`);
      return;
    }
    const o = this.optionEl(path);
    const r = (o ?? this.content).getBoundingClientRect();
    this.openMenu(r.left + 12, r.bottom - 8, {
      header: `${nameOf(path)}: used ${times(usages.length)}`,
      items: usages.map((u): MenuItem<AssetAct> => ({ type: "item", label: `Line ${u.line}: ${u.snippet}`, act: { op: "reveal", from: u.from, to: u.to } })),
    }, o);
  }

  // ---- Rename ----

  /** A folder shown in the tree: renamed inline among its parent's contents. */
  private renameFolderOf(path: string): void {
    this.filter = { query: "", unused: false };
    this.search.value = "";
    this.openFolder(parentOf(path), false);
    this.selected = path;
    this.render();
    this.startRename(path);
  }

  private startRename(key: string): void {
    const e = this.entry(key);
    const o = this.optionEl(key);
    const nameEl = o?.querySelector<HTMLElement>(".as-name");
    if (!e || e.type === "sources" || !o || !nameEl) return;
    this.select(key, false);
    this.renaming = key;
    const name = nameOf(e.path);
    const input = el("input", { type: "text", className: "as-rename", value: name, autocomplete: "off", spellcheck: false });
    input.setAttribute("aria-label", `New name for ${name}`);
    let committing = false;
    const commit = async (keepOpen: boolean) => {
      if (committing || this.renaming !== key) return;
      const target = renameTarget(e.path, input.value);
      if ("error" in target) {
        this.host.error(target.error);
        if (keepOpen) input.focus();
        else this.endRename();
        return;
      }
      if (target.path === e.path) {
        this.endRename();
        return;
      }
      committing = true;
      this.renaming = null;
      if (await this.step(() => this.host.api.move(e.path, target.path))) this.select(target.path);
      else this.render();
      this.focusSelected();
    };
    input.addEventListener("keydown", (ev) => {
      ev.stopPropagation();
      if (ev.key === "Enter") {
        ev.preventDefault();
        void commit(true);
      }
    });
    input.addEventListener("blur", () => void commit(false));
    // No dragging while renaming (it would select text instead).
    o.draggable = false;
    nameEl.replaceWith(input);
    input.focus();
    const dot = e.type === "item" ? name.lastIndexOf(".") : -1;
    input.setSelectionRange(0, dot > 0 ? dot : name.length);
  }

  private endRename(): void {
    if (this.renaming === null) return;
    this.renaming = null;
    this.render();
    this.focusSelected();
  }

  // ---- Large preview ----

  private openViewer(path: string): void {
    const opener = this.optionEl(path);
    this.closeViewer(false);
    const box = el("figure", { className: "as-viewer-box" });
    const viewer = el("div", { className: "as-viewer", tabIndex: -1 }, box);
    viewer.setAttribute("role", "dialog");
    viewer.setAttribute("aria-modal", "true");
    viewer.addEventListener("click", (e) => {
      if (e.target === viewer) this.closeViewer(true);
    });
    viewer.addEventListener("keydown", (e) => {
      if ((e.key !== "ArrowLeft" && e.key !== "ArrowRight") || e.ctrlKey || e.metaKey || e.altKey) return;
      e.preventDefault();
      this.stepViewer(e.key === "ArrowLeft" ? -1 : 1);
    });
    document.body.append(viewer);
    this.viewer = { el: viewer, path, opener };
    this.fillViewer();
    viewer.focus();
  }

  /** Shows the viewer's image and details (the image loads if it isn't yet). */
  private fillViewer(): void {
    const v = this.viewer;
    if (!v) return;
    const it = this.item(v.path);
    const state = this.cache.get(v.path);
    if (!state) this.cache.want([v.path, ...this.visible]);
    const box = v.el.firstElementChild as HTMLElement;
    const name = nameOf(v.path);
    v.el.setAttribute("aria-label", `Preview of ${name}`);
    const img = box.querySelector("img");
    const url = state && "url" in state ? state.url : null;
    const details = it ? `${formatSize(it.size)} · ${it.used_by ? `used ${times(it.used_by)}` : "unused"} · ${STATUSES[it.status].label}` : "";
    const caption = el("figcaption", {}, el("strong", { textContent: name }), el("span", { textContent: `${folderLabel(parentOf(v.path))} · ${details}` }));
    if (img && img.src === url) {
      box.querySelector("figcaption")?.replaceWith(caption);
      return;
    }
    const picture = url
      ? el("img", { src: url, alt: name })
      : el("p", { className: "as-viewer-note", textContent: state && "error" in state ? state.error : it?.status === "missing" ? STATUSES.missing.title : "Loading…" });
    box.replaceChildren(picture, caption);
  }

  /** Left/Right in the viewer: the previous or next image of the contents shown. */
  private stepViewer(by: number): void {
    const v = this.viewer;
    if (!v) return;
    const images = this.entries.filter((e): e is Extract<Entry, { type: "item" }> => e.type === "item" && typeOf(e.item)?.preview === "image" && e.item.status === "available");
    const i = images.findIndex((e) => e.path === v.path);
    const next = images[i + by];
    if (!next) return;
    v.path = next.path;
    this.select(next.path, false);
    v.opener = this.optionEl(next.path);
    this.fillViewer();
  }

  private closeViewer(restore: boolean): void {
    const v = this.viewer;
    if (!v) return;
    this.viewer = null;
    v.el.remove();
    if (restore) (v.opener?.isConnected ? v.opener : this.content).focus({ preventScroll: true });
  }

  // ---- Height ----

  private applyHeight(): void {
    const total = window.innerHeight;
    const h = clampHeight(this.height, total);
    document.body.style.setProperty("--assets-h", `${h}px`);
    this.handle.setAttribute("aria-valuenow", String(total > 0 ? Math.round((h / total) * 100) : 0));
  }

  private setHeight(height: number, persist: boolean): void {
    this.height = clampHeight(height, window.innerHeight);
    this.applyHeight();
    if (persist) savePref("lethora.assetsHeight", String(Math.round(this.height)));
  }

  private wireHandle(): void {
    const h = this.handle;
    h.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      e.preventDefault();
      h.setPointerCapture(e.pointerId);
      document.body.classList.add("assets-dragging");
    });
    h.addEventListener("pointermove", (e) => {
      if (h.hasPointerCapture(e.pointerId)) this.setHeight(window.innerHeight - e.clientY, false);
    });
    const end = (e: PointerEvent) => {
      if (!document.body.classList.contains("assets-dragging")) return;
      document.body.classList.remove("assets-dragging");
      this.setHeight(this.height, true);
      if (h.hasPointerCapture(e.pointerId)) h.releasePointerCapture(e.pointerId);
    };
    h.addEventListener("pointerup", end);
    h.addEventListener("pointercancel", end);
    // Up/Down move the edge (Up makes the panel taller).
    h.addEventListener("keydown", (e) => {
      if ((e.key !== "ArrowUp" && e.key !== "ArrowDown") || e.ctrlKey || e.metaKey || e.altKey || e.shiftKey) return;
      e.preventDefault();
      this.setHeight(this.height + (e.key === "ArrowUp" ? ASSETS_STEP : -ASSETS_STEP), true);
    });
  }
}
