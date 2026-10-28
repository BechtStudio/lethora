// The asset explorer's pure parts: the folder tree and a folder's contents
// from `asset_list`, search and the "Unused" filter, sorting, the grid's
// keyboard moves, where the text uses an asset, and rename and move
// targets (names are free, an item keeps its format). The panel itself is
// in `assetspanel.ts`. No DOM, no Tauri (tested under `node --test`).

import { assetKind } from "./assettypes.ts";

/** Whether an item's blob is on this device; sync states can be added here. */
export type AssetStatus = "available" | "missing";

/** One `asset_list` item. */
export interface AssetItem {
  path: string;
  /** A kind of `ASSET_TYPES` (`image`). */
  kind: string;
  /** Bytes of the blob; null while it is missing. */
  size: number | null;
  /** How many string literals in the text name it. */
  used_by: number;
  status: AssetStatus;
}

/** `asset_list`: every folder below `assets/` (ending in `/`) and the items, sorted by path. */
export interface AssetList {
  folders: string[];
  items: AssetItem[];
}

export const ROOT = "assets/";

/** What the panel's items and folders carry when dragged (their path; moves inside the panel, figures in the text). */
export const ASSET_DRAG_TYPE = "application/x-lethora-asset";
/** Also carried by a dragged folder (a dragover sees only the types), so the text refuses it. */
export const FOLDER_DRAG_TYPE = "application/x-lethora-folder";

/** How the explorer shows each status (list column, the tile's cloud). */
export const STATUSES: Record<AssetStatus, { label: string; title: string }> = {
  available: { label: "Available", title: "Available on this device" },
  missing: { label: "Missing", title: "Not on this device yet (it has not arrived from another device)" },
};

const STATUS_ORDER: AssetStatus[] = ["available", "missing"];

/** The folder holding `path` (an item, or a folder ending in `/`); the root for the root. */
export function parentOf(path: string): string {
  const i = path.lastIndexOf("/", path.length - 2);
  return i < 0 ? ROOT : path.slice(0, i + 1);
}

/** The last component of `path` (a folder's without its `/`). */
export function nameOf(path: string): string {
  const inner = path.endsWith("/") ? path.slice(0, -1) : path;
  return inner.slice(inner.lastIndexOf("/") + 1);
}

/** The lowercase extension of `path`, or "". */
export function extensionOf(path: string): string {
  const m = /\.([^./]+)$/.exec(path);
  return m ? m[1].toLowerCase() : "";
}

/** Whether extensions `a` and `b` name one format (Typst decodes by extension; jpg and jpeg are one). */
export function sameFormat(a: string, b: string): boolean {
  const canonical = (e: string) => (e.toLowerCase() === "jpeg" ? "jpg" : e.toLowerCase());
  return canonical(a) === canonical(b);
}

/** The extensions of `ext`'s format (for a file dialog's filter). */
export function formatExtensions(ext: string): string[] {
  return sameFormat(ext, "jpg") ? ["jpg", "jpeg"] : [ext];
}

const byName = (a: string, b: string) => a.localeCompare(b, undefined, { numeric: true, sensitivity: "base" }) || a.localeCompare(b);

// ---- Tree and contents ----

export interface FolderNode {
  /** Ends in `/` (`assets/` for the root). */
  path: string;
  name: string;
  children: FolderNode[];
}

/** The folders of `list` (and those above any item) as a tree under the root, children by name. */
export function folderTree(list: AssetList): FolderNode {
  const nodes = new Map<string, FolderNode>([[ROOT, { path: ROOT, name: "assets", children: [] }]]);
  const add = (folder: string): FolderNode => {
    const known = nodes.get(folder);
    if (known) return known;
    const node: FolderNode = { path: folder, name: nameOf(folder), children: [] };
    nodes.set(folder, node);
    add(parentOf(folder)).children.push(node);
    return node;
  };
  for (const f of list.folders) if (f.startsWith(ROOT) && f.endsWith("/")) add(f);
  for (const item of list.items) add(parentOf(item.path));
  for (const node of nodes.values()) node.children.sort((a, b) => byName(a.name, b.name));
  return nodes.get(ROOT)!;
}

/** Whether `folder` is in `list`: the root, a listed folder, or one holding an item. */
export function hasFolder(list: AssetList, folder: string): boolean {
  return folder === ROOT || list.folders.includes(folder) || list.items.some((i) => i.path.startsWith(folder));
}

/** The node of `folder` in `tree`, or null. */
export function findFolder(tree: FolderNode, folder: string): FolderNode | null {
  if (tree.path === folder) return tree;
  for (const child of tree.children) if (folder.startsWith(child.path)) return findFolder(child, folder);
  return null;
}

/** What the content area shows: a folder, an item, or the Sources (the bibliography, in the root). */
export type Entry = { type: "folder"; path: string } | { type: "item"; path: string; item: AssetItem } | { type: "sources" };

export const SOURCES_KEY = "sources:";

/** An entry's key (its path; the Sources' own key). */
export const entryKey = (e: Entry): string => (e.type === "sources" ? SOURCES_KEY : e.path);

/** An entry's shown name. */
export const entryName = (e: Entry): string => (e.type === "sources" ? "Sources" : nameOf(e.path));

/** The entries directly in `folder`: the Sources (root only, if `sources`), subfolders, then items. */
export function folderEntries(list: AssetList, tree: FolderNode, folder: string, sources: boolean): Entry[] {
  const node = findFolder(tree, folder);
  const out: Entry[] = sources && folder === ROOT ? [{ type: "sources" }] : [];
  for (const child of node?.children ?? []) out.push({ type: "folder", path: child.path });
  for (const item of list.items) if (parentOf(item.path) === folder) out.push({ type: "item", path: item.path, item });
  return out;
}

export interface Filter {
  query: string;
  /** Only items the text does not use. */
  unused: boolean;
}

/** Whether `f` searches all folders instead of showing one. */
export const isFiltering = (f: Filter): boolean => f.query.trim() !== "" || f.unused;

/** The items of every folder that pass `f`: the query in the name or folder path (any case), unused ones only if asked. */
export function filterEntries(list: AssetList, f: Filter): Entry[] {
  const q = f.query.trim().toLowerCase();
  return list.items.filter((item) => (!f.unused || item.used_by === 0) && (q === "" || item.path.slice(ROOT.length).toLowerCase().includes(q))).map((item) => ({ type: "item", path: item.path, item }));
}

// ---- Sorting ----

export type SortKey = "name" | "type" | "size" | "used" | "status";

export interface Sort {
  key: SortKey;
  descending: boolean;
}

/**
 * `entries` sorted by `sort`: the Sources and folders always first, by name;
 * items by the key, then by name and path.
 */
export function sortEntries(entries: readonly Entry[], sort: Sort): Entry[] {
  const rank = (e: Entry) => (e.type === "sources" ? 0 : e.type === "folder" ? 1 : 2);
  const value = (item: AssetItem): number | string => {
    switch (sort.key) {
      case "type":
        return `${item.kind}.${extensionOf(item.path)}`;
      case "size":
        return item.size ?? -1;
      case "used":
        return item.used_by;
      case "status":
        return STATUS_ORDER.indexOf(item.status);
      default:
        return "";
    }
  };
  const sign = sort.descending ? -1 : 1;
  return [...entries].sort((a, b) => {
    const r = rank(a) - rank(b);
    if (r !== 0) return r;
    const names = byName(entryName(a), entryName(b)) || byName(entryKey(a), entryKey(b));
    if (a.type !== "item" || b.type !== "item") return names;
    const va = value(a.item);
    const vb = value(b.item);
    const by = typeof va === "number" && typeof vb === "number" ? va - vb : byName(String(va), String(vb));
    return sign * (by || names);
  });
}

// ---- Keyboard ----

/**
 * The index after `key` from `index` among `count` entries laid out in rows
 * of `columns` (1 in the list): arrows move by one or a row (Down to a
 * shorter last row lands on its end), Home and End to the ends; null for
 * other keys and Left/Right in the list.
 */
export function moveIndex(index: number, count: number, columns: number, key: string): number | null {
  if (count === 0) return null;
  const at = Math.max(0, Math.min(index, count - 1));
  const lastRow = Math.floor((count - 1) / columns);
  switch (key) {
    case "ArrowLeft":
      return columns > 1 ? Math.max(0, at - 1) : null;
    case "ArrowRight":
      return columns > 1 ? Math.min(count - 1, at + 1) : null;
    case "ArrowUp":
      return at - columns >= 0 ? at - columns : at;
    case "ArrowDown":
      return Math.floor(at / columns) < lastRow ? Math.min(count - 1, at + columns) : at;
    case "Home":
      return 0;
    case "End":
      return count - 1;
    default:
      return null;
  }
}

// ---- Usages ----

/** Where the text names an asset: the path inside its quotes (UTF-16), its line (1-based), a snippet of that line. */
export interface Usage {
  from: number;
  to: number;
  line: number;
  snippet: string;
}

const SNIPPET = 56;

/** The line around column `col`, whitespace collapsed, clipped to about `SNIPPET` characters. */
function snippetAt(line: string, col: number): string {
  if (line.length <= SNIPPET) return line.replace(/\s+/g, " ").trim();
  const start = Math.max(0, Math.min(col - 16, line.length - SNIPPET));
  const part = line.slice(start, start + SNIPPET).replace(/\s+/g, " ").trim();
  return `${start > 0 ? "…" : ""}${part}${start + SNIPPET < line.length ? "…" : ""}`;
}

/**
 * Every string literal reading `path` in `text`, at most `max`. Quotes pair
 * up as the core's usage count (`literals` in crates/core) pairs them.
 */
export function findUsages(text: string, path: string, max = 100): Usage[] {
  const out: Usage[] = [];
  let line = 1;
  let lineStart = 0;
  let scanned = 0;
  for (let open = text.indexOf('"'); open >= 0 && out.length < max; ) {
    const close = text.indexOf('"', open + 1);
    if (close < 0) break;
    if (close - open - 1 !== path.length || !text.startsWith(path, open + 1)) {
      open = close;
      continue;
    }
    for (let nl = text.indexOf("\n", scanned); nl >= 0 && nl < open; nl = text.indexOf("\n", nl + 1)) {
      line++;
      lineStart = nl + 1;
    }
    scanned = open;
    const end = text.indexOf("\n", open);
    out.push({ from: open + 1, to: close, line, snippet: snippetAt(text.slice(lineStart, end < 0 ? text.length : end), open - lineStart) });
    open = text.indexOf('"', close + 1);
  }
  return out;
}

// ---- Rename and move ----

/** Why `name` can't be a path component (as `assets::validate_relative` checks), or null. */
export function nameProblem(name: string): string | null {
  if (name === "") return "Enter a name.";
  if (name.includes("/")) return "A name can't contain /.";
  if (/["\\:\u0000-\u001f\u007f]/.test(name)) return 'A name can\'t contain ", \\ or :.';
  if (/^[. ]|[. ]$/.test(name)) return "A name can't start or end with a dot.";
  return null;
}

/**
 * The path `path` (an item, or a folder ending in `/`) gets when renamed to
 * `input` in its folder. An item keeps its format: a name without a known
 * extension gets the old one, a known extension must be of the same format
 * (written lowercase).
 */
export function renameTarget(path: string, input: string): { path: string } | { error: string } {
  const name = input.trim();
  const parent = parentOf(path);
  if (path.endsWith("/")) {
    const problem = nameProblem(name);
    return problem ? { error: problem } : { path: `${parent}${name}/` };
  }
  const ext = extensionOf(path);
  const typed = extensionOf(name);
  let full = `${name}.${ext}`;
  if (typed && assetKind(`x.${typed}`)) {
    if (!sameFormat(typed, ext)) return { error: `The name keeps the file's format: end it with .${ext}.` };
    full = `${name.slice(0, -typed.length)}${typed}`;
  }
  const problem = nameProblem(name) ?? nameProblem(full);
  return problem ? { error: problem } : { path: `${parent}${full}` };
}

/** Where `path` (item or folder) goes when moved into `folder` with its name; null if it is there already or would go into itself. */
export function moveTarget(path: string, folder: string): string | null {
  if (parentOf(path) === folder || (path.endsWith("/") && folder.startsWith(path))) return null;
  return `${folder}${nameOf(path)}${path.endsWith("/") ? "/" : ""}`;
}

/** Whether the text uses an item at or below `folder`. */
export const folderUsed = (list: AssetList, folder: string): boolean => list.items.some((i) => i.path.startsWith(folder) && i.used_by > 0);

/** A free folder name in `parent`: "New folder", "New folder 2", … */
export function newFolderPath(tree: FolderNode, parent: string): string {
  const taken = new Set((findFolder(tree, parent)?.children ?? []).map((c) => c.name.toLowerCase()));
  for (let n = 1; ; n++) {
    const name = n === 1 ? "New folder" : `New folder ${n}`;
    if (!taken.has(name.toLowerCase())) return `${parent}${name}/`;
  }
}

// ---- Display ----

/** A size for people: bytes, KB or MB (one decimal under 10); a dash while missing. */
export function formatSize(bytes: number | null): string {
  if (bytes === null) return "–";
  if (bytes < 1024) return `${bytes} B`;
  const [value, unit] = bytes < 1024 * 1024 ? [bytes / 1024, "KB"] : [bytes / (1024 * 1024), "MB"];
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${unit}`;
}

/** The panel's height range: at least `ASSETS_MIN`, leaving `ABOVE_MIN` for the editor or pages. */
export const ASSETS_MIN = 120;
export const ABOVE_MIN = 160;
export const ASSETS_DEFAULT = 260;
/** How far Up/Down on the handle resize it, in CSS pixels. */
export const ASSETS_STEP = 24;

/** A stored height, or the default if it is missing or not a positive number. */
export function parseHeight(stored: string | null): number {
  const h = stored === null ? NaN : Number(stored);
  return Number.isFinite(h) && h > 0 ? h : ASSETS_DEFAULT;
}

/** `height` clamped for a window `total` pixels high (the minimum wins when both don't fit). */
export function clampHeight(height: number, total: number): number {
  return Math.max(ASSETS_MIN, Math.min(height, total - ABOVE_MIN));
}
