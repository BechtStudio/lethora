// Development only: an in-memory stand-in for the Tauri backend, so the UI can
// be opened in a normal browser (`npm run dev`). main.ts imports this only when
// `import.meta.env.DEV` and not running inside Tauri; it is never bundled.

import { ChangeSet, MapMode, Text } from "@codemirror/state";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import { textHash, type Reply, type TextChange } from "./sync.ts";
import { isInviteLink, joinedName } from "./pull.ts";
import { type ReviewItem, suggestInto } from "./review.ts";
import SAMPLE from "./sample.typ?raw";
// Real pages of SAMPLE with the initial layout and overrides below, rendered by
// the typesetter. Regenerate after changing either (or the typesetting) with
// `cargo test -p lethora-typeset --test mock_pages -- --ignored`.
import FIXTURE_JSON from "./fixtures/pages/pages.json?raw";

const FIXTURE_SVGS = import.meta.glob<string>("./fixtures/pages/*.svg", { query: "?raw", import: "default", eager: true });
const FIXTURE = JSON.parse(FIXTURE_JSON) as { page_sizes: [number, number][]; overrides: Array<Record<string, unknown> & { id: string }> };


/** A ~5M-character manuscript for performance checks (`?book=1`). */
function book(): string {
  const sentences = [
    "The lens turned once every twenty seconds, and the beam swept the water like a patient hand.",
    "Mara counted the steps again, though she knew the number by heart.",
    "If the beam travelled $r = 22$ nautical miles, the lit circle was $A = pi r^2$.",
    "Her father wrote in a hand so small it looked like _stitching_, and he never crossed anything out.",
    "The keepers before them had left notes (see @woolf1927, p. 41), most of them *practical*.",
    "A storm came in from the west, and the glass rattled in its frame as if it had something to say.",
    "She worked the sum twice, $sum_(i=1)^n i = frac(n(n+1), 2)$, and got the same answer both times.",
    "Ships passed at night without a word, their lights small as a thought.",
  ];
  const parts: string[] = [];
  let length = 0;
  for (let chapter = 1; length < 5_000_000; chapter++) {
    const heading = `= Chapter ${chapter} <ch-${chapter}>\n`;
    parts.push(heading);
    length += heading.length;
    for (let p = 0; p < 40; p++) {
      let paragraph = "";
      for (let k = 0; k < 7; k++) paragraph += sentences[(chapter * 7 + p * 3 + k) % sentences.length] + " ";
      paragraph = paragraph.trimEnd() + "\n\n";
      // Distinct formulas, so scrolling exercises rendering (not just the cache).
      if (p % 10 === 5) paragraph += `$ integral_0^${chapter} x^${p} dif x\n  = ${chapter}^${p + 1}/${p + 1} $\n\n`;
      if (p === 20) paragraph = `== Part ${p}\n\n` + paragraph;
      parts.push(paragraph);
      length += paragraph.length;
    }
  }
  return parts.join("");
}

const DRAFT = SAMPLE.split("== The Arithmetic of Light")[0].trimEnd() + "\n";

interface StoredCheckpoint {
  id: number;
  name: string | null;
  auto: boolean;
  created: number;
  text: string;
}

const now = () => Math.floor(Date.now() / 1000);
const docOf = (s: string) => Text.of(s.split("\n"));

const BOOK = new URLSearchParams(location.search).has("book");
let doc = docOf(BOOK ? book() : SAMPLE);
let project = { path: "C:/Mock/Sample.lethora", name: BOOK ? "Book-size manuscript" : "Sample manuscript" };
/**
 * Whether compiles serve the fixture pages (the sample) instead of generated
 * placeholder pages. They stay the same after edits and layout changes.
 */
let fixturePages = !BOOK;
/**
 * Each undo step: the changes that revert a text step (applicable to the
 * current text when on top), the layout state before a layout step, or the
 * asset index before an asset step (with the text changes reverting it).
 */
type Step = ChangeSet | LayoutDoc | AssetStep;
const undoStack: Step[] = [];
const redoStack: Step[] = [];
/** Whether the next unsplit edit may join the top undo step. */
let mergeable = false;

const checkpoints: StoredCheckpoint[] = [
  { id: 1, name: "First draft", auto: false, created: now() - 26 * 3600, text: DRAFT },
  { id: 2, name: null, auto: true, created: now() - 3 * 3600, text: SAMPLE.replace("*One hundred and twelve*", "One hundred and ten") },
  { id: 3, name: null, auto: true, created: now() - 12 * 60, text: SAMPLE },
];

function reply(changes: ChangeSet): Reply {
  const out: TextChange[] = [];
  changes.iterChanges((from, to, _fromB, _toB, inserted) => out.push({ from, to, insert: inserted.toString() }));
  return { changes: out, len16: doc.length };
}

function edit(changes: TextChange[], split: boolean): number {
  const set = ChangeSet.of(changes, doc.length);
  if (set.empty) return doc.length;
  const inverse = set.invert(doc);
  doc = set.apply(doc);
  mapOverrides(set);
  const top = undoStack[undoStack.length - 1];
  if (!split && mergeable && top instanceof ChangeSet) undoStack.push(inverse.compose(undoStack.pop() as ChangeSet));
  else undoStack.push(inverse);
  redoStack.length = 0;
  mergeable = true;
  bump();
  return doc.length;
}

function step(from: Step[], to: Step[]): Reply | null {
  const s = from.pop();
  if (!s) return null;
  mergeable = false;
  bump();
  if (s instanceof ChangeSet) {
    to.push(s.invert(doc));
    doc = s.apply(doc);
    mapOverrides(s);
    return reply(s);
  }
  if (s instanceof AssetStep) {
    to.push(new AssetStep(s.text?.invert(doc) ?? null));
    ({ items: assets, folders } = s.index);
    if (!s.text) return { changes: [], len16: doc.length, assets: true };
    doc = s.text.apply(doc);
    mapOverrides(s.text);
    return { ...reply(s.text), assets: true };
  }
  // A layout step: no text changes.
  to.push(cloneLayout(layoutDoc));
  layoutDoc = s;
  return { changes: [], len16: doc.length };
}

/** The change from the current text to `target` (one minimal middle change). */
function changeTo(target: string): ChangeSet {
  const text = doc.toString();
  let start = 0;
  const max = Math.min(text.length, target.length);
  while (start < max && text.charCodeAt(start) === target.charCodeAt(start)) start++;
  let end = 0;
  while (end < max - start && text.charCodeAt(text.length - 1 - end) === target.charCodeAt(target.length - 1 - end)) end++;
  return ChangeSet.of({ from: start, to: text.length - end, insert: target.slice(start, target.length - end) }, doc.length);
}

/** Replaces the text with `target`, as one undo step (minimal middle change). */
function replaceAll(target: string): Reply {
  const set = changeTo(target);
  undoStack.push(set.invert(doc));
  redoStack.length = 0;
  doc = set.apply(doc);
  mapOverrides(set);
  mergeable = false;
  bump();
  return reply(set);
}

function checkpoint(id: number): StoredCheckpoint {
  const c = checkpoints.find((c) => c.id === id);
  if (!c) throw `Not found: checkpoint ${id}.`;
  return c;
}

const escapeXml = (s: string) => s.replace(/[<>&"]/g, (c) => `&#${c.charCodeAt(0)};`);

// ---- Sources (references.bib) ----

interface MockEntry {
  key: string;
  entry_type: string;
  title: string;
  authors: string[];
  year: string;
}

const entry = (key: string, entry_type: string, authors: string[], year: string, title: string): MockEntry => ({ key, entry_type, title, authors, year });

const bib: MockEntry[] = [
  entry("admiralty1898", "book", ["Admiralty, Hydrographic Department"], "1898", "The Admiralty List of Lights: British Isles"),
  entry("elliot2023", "online", ["Elliot, Hannah"], "2023", "Light and Memory on the Northern Coast"),
  entry("fresnel1822", "article", ["Fresnel, Augustin"], "1822", "Mémoire sur un nouveau système d'éclairage des phares"),
  entry("hague1975", "book", ["Hague, Douglas B.", "Christie, Rosemary"], "1975", "Lighthouses: Their Architecture, History and Archaeology"),
  entry("levitt2013", "book", ["Levitt, Theresa"], "2013", "A Short Bright Flash: Augustin Fresnel and the Birth of the Modern Lighthouse"),
  entry("stevenson1959", "book", ["Stevenson, D. Alan"], "1959", "The World's Lighthouses Before 1820"),
  entry("woolf1927", "book", ["Woolf, Virginia"], "1927", "To the Lighthouse"),
];

const bibErrors = [{ message: "keeper1901: expected a comma after the key", line: 58, column: 18 }];

const STYLES = [
  { id: "apa", name: "American Psychological Association 7th edition" },
  { id: "chicago-author-date", name: "Chicago Manual of Style 17th edition (author-date)" },
  { id: "ieee", name: "IEEE" },
  { id: "modern-language-association", name: "Modern Language Association 9th edition" },
];

function formatEntry(a: Args): string {
  const e = bib.find((e) => e.key === a.key);
  if (!e) throw `No readable source with key "${a.key as string}".`;
  const initials = (name: string) => {
    const [last, first = ""] = name.split(",").map((s) => s.trim());
    return `${first.split(/\s+/).filter(Boolean).map((p) => `${p[0]}.`).join(" ")} ${last}`.trim();
  };
  switch (a.style) {
    case "ieee":
      return `${e.authors.map(initials).join(" and ")}, “${e.title},” ${e.year}.`;
    case "modern-language-association":
      return `${e.authors.join(", and ")}. ${e.title}. ${e.year}.`;
    case "chicago-author-date":
      return `${e.authors.join(", and ")}. ${e.year}. ${e.title}.`;
    default:
      return `${e.authors.join(", & ")} (${e.year}). ${e.title}.`;
  }
}

/** A rough stand-in for the core's BibLaTeX merge (keys, title, authors, year). */
function importBib(text: string) {
  let depth = 0;
  let line = 1;
  let openLine = 1;
  for (const ch of text) {
    if (ch === "\n") line++;
    else if (ch === "{") {
      if (depth++ === 0) openLine = line;
    } else if (ch === "}") depth--;
  }
  const heads = [...text.matchAll(/@(\w+)\s*\{\s*([^,\s{}]+)\s*,/g)];
  if (depth !== 0) throw `The imported text is not valid BibLaTeX (line ${openLine}, column 1): unclosed brace.`;
  if (heads.length === 0) throw "The imported text is not valid BibLaTeX (line 1, column 1): expected an entry.";
  const added: string[] = [];
  const skipped_duplicates: string[] = [];
  heads.forEach((m, i) => {
    const key = m[2];
    if (bib.some((e) => e.key === key)) {
      skipped_duplicates.push(key);
      return;
    }
    const body = text.slice(m.index, heads[i + 1]?.index ?? text.length);
    const field = (name: string) =>
      new RegExp(`\\b${name}\\s*=\\s*\\{((?:[^{}]|\\{[^{}]*\\})*)\\}`, "i").exec(body)?.[1].replace(/\\textbackslash\{\}/g, "\\").replace(/\\([{}%#&$_])/g, "$1") ?? "";
    bib.push(entry(key, m[1].toLowerCase(), field("author").split(/\s+and\s+/).filter(Boolean), field("year"), field("title")));
    added.push(key);
  });
  bib.sort((a, b) => a.key.localeCompare(b.key));
  return { added, skipped_duplicates };
}

const ZOTERO_EXPORT = `@book{woolf1927, author = {Woolf, Virginia}, title = {To the Lighthouse}, year = {1927}}
@article{maxwell1865, author = {Maxwell, James Clerk}, title = {A Dynamical Theory of the Electromagnetic Field}, year = {1865}}
@book{nightingale1860, author = {Nightingale, Florence}, title = {Notes on Nursing}, year = {1860}}`;

// ---- Assets ----

/** An indexed asset: its blob's size, or missing (not on this device); the blob's name if it was staged. */
interface MockAsset {
  size: number;
  missing?: boolean;
  blob?: string;
}

/**
 * The asset index: items by path, sorted, and folder markers. sketch.png is
 * referenced by the sample but not indexed; beam.svg is indexed but its blob
 * is missing; harbour.jpg is unused.
 */
let assets = new Map<string, MockAsset>([
  ["assets/diagrams/beam.svg", { size: 2400, missing: true }],
  ["assets/lighthouse.png", { size: 48_213 }],
  ["assets/photos/harbour.jpg", { size: 182_940 }],
  ["assets/photos/keepers.png", { size: 96_400 }],
]);
let folders = new Set(["assets/diagrams/", "assets/diagrams/old/", "assets/photos/"]);

/** An asset step's undo: the index before it, and the changes reverting its text changes. */
class AssetStep {
  index = { items: new Map(assets), folders: new Set(folders) };
  text: ChangeSet | null;
  constructor(text: ChangeSet | null) {
    this.text = text;
  }
}

/**
 * Runs `change` (which checks first, then changes the index and returns the
 * new text, or null to keep it) as one undo step and a new version.
 */
function assetStep(change: () => string | null): Reply {
  const before = new AssetStep(null);
  const target = change();
  const set = target === null ? null : changeTo(target);
  before.text = set?.invert(doc) ?? null;
  undoStack.push(before);
  redoStack.length = 0;
  mergeable = false;
  bump();
  if (!set) return { changes: [], len16: doc.length };
  doc = set.apply(doc);
  mapOverrides(set);
  return reply(set);
}

const ROOT = "assets/";
const sameFormat = (a: string, b: string) => a.replace(/^jpeg$/, "jpg") === b.replace(/^jpeg$/, "jpg");
const extOf = (path: string) => /\.([^./]+)$/.exec(path)?.[1] ?? "";

/** Writes the marker of `path`'s folder (below the root) if it has none. */
function markFolder(path: string): void {
  const folder = path.slice(0, path.lastIndexOf("/") + 1);
  if (folder !== ROOT) folders.add(folder);
}

/** A new path in `folder` for the file `source`: a sanitized stem, numbered while `taken`. */
function freePath(source: string, folder: string, taken: (path: string) => boolean): string {
  const name = source.split(/[\\/]/).pop()!;
  const ext = name.split(".").pop()!.toLowerCase();
  const stem = name.replace(/\.[^.]*$/, "").toLowerCase().replace(/[^a-z0-9_]+/g, "-").replace(/^-+|-+$/g, "") || "image";
  for (let n = 1; ; n++) {
    const path = `${folder}${n === 1 ? stem : `${stem}-${n}`}.${ext}`;
    if (!taken(path)) return path;
  }
}

const sortAssets = () => (assets = new Map([...assets].sort(([a], [b]) => (a < b ? -1 : 1))));

/** Indexes the file `source` under a new path in `folder` (with its folder's marker), stored as `size` bytes. */
function addAsset(source: string, folder = ROOT, size = 20_000 + source.split(/[\\/]/).pop()!.length * 997): string {
  const path = freePath(source, folder, (p) => assets.has(p));
  assets.set(path, { size });
  sortAssets();
  markFolder(path);
  return path;
}

/** The import folder: `folder`, or the root if none. */
function importFolder(folder: unknown): string {
  const f = (folder as string | null | undefined) ?? ROOT;
  if (!f.startsWith(ROOT) || !f.endsWith("/")) throw `Not an asset folder: "${f}".`;
  return f;
}

/** Like `asset_import`: one undo step, a new version. */
function importAsset(a: Args): string {
  const folder = importFolder(a.folder);
  let path = "";
  assetStep(() => {
    path = addAsset(a.path as string, folder);
    return null;
  });
  return path;
}

const IMAGE_EXTENSIONS = ["png", "jpg", "jpeg", "gif", "svg", "webp"];
const MAX_BYTES = 50 * 1024 * 1024;

function checkImage(name: string, size: number): void {
  if (!IMAGE_EXTENSIONS.includes(extOf(name).toLowerCase())) throw `Unsupported file type. Use one of: ${IMAGE_EXTENSIONS.join(", ")}.`;
  if (size > MAX_BYTES) throw "The image is larger than 50 MB.";
}

/** A raw-bytes command's file: the body and the percent-encoded headers (see the end of this file), checked like an import. */
function rawFile(a: Args): { name: string; bytes: Uint8Array; folder: string; taken: string[] } {
  const headers = a.headers as Headers;
  const header = (name: string) => {
    const v = headers.get(name);
    return v === null ? null : decodeURIComponent(v);
  };
  const name = header("x-file-name")?.split(/[\\/]/).pop();
  if (!name) throw "The file name is missing.";
  const bytes = a.body as Uint8Array;
  checkImage(name, bytes.byteLength);
  const taken = (headers.get("x-taken") ?? "").split(",").filter(Boolean).map(decodeURIComponent);
  return { name, bytes, folder: importFolder(header("x-folder")), taken };
}

/** Like `asset_import_bytes`: one undo step, a new version. */
function importBytes(a: Args): string {
  const { name, bytes, folder } = rawFile(a);
  let path = "";
  assetStep(() => {
    path = addAsset(name, folder, bytes.byteLength);
    return null;
  });
  return path;
}

/** Blobs written by staging (name → size); `figure_insert` indexes them. */
const stagedBlobs = new Map<string, number>();

/** A mock blob name (64 hex characters) for `bytes`. */
function blobOf(bytes: Uint8Array): string {
  let hash = 0x811c9dc5;
  for (const b of bytes) hash = Math.imul(hash ^ b, 0x01000193) >>> 0;
  return hash.toString(16).padStart(8, "0").repeat(8);
}

/** Staging: the blob is stored; the same content in the same format in `folder` reuses its path, else a new one in neither the index nor `taken`. */
function stage(source: string, folder: string, taken: string[], blob: string, size: number): { path: string; blob: string } {
  stagedBlobs.set(blob, size);
  const ext = extOf(source).toLowerCase();
  const same = [...assets].find(([p, x]) => x.blob === blob && p.slice(0, p.lastIndexOf("/") + 1) === folder && sameFormat(extOf(p), ext));
  return { path: same ? same[0] : freePath(source, folder, (p) => assets.has(p) || taken.includes(p)), blob };
}

/** Like `asset_stage` (a picked file; its path stands for its content). */
function stageAsset(a: Args): { path: string; blob: string } {
  const source = a.path as string;
  checkImage(source, 0);
  return stage(source, importFolder(a.folder), (a.taken as string[] | undefined) ?? [], blobOf(new TextEncoder().encode(source)), 20_000 + source.length * 997);
}

/** Like `asset_stage_bytes`. */
function stageBytes(a: Args): { path: string; blob: string } {
  const { name, bytes, folder, taken } = rawFile(a);
  return stage(name, folder, taken, blobOf(bytes), bytes.byteLength);
}

/** Like `figure_insert`: the text and the staged entries (with folder markers) as one undo step, a new version. */
function figureInsert(a: Args): Reply {
  const pos = a.pos as number;
  const list = a.assets as Array<{ path: string; blob: string }>;
  if (!(pos >= 0 && pos <= doc.length)) throw `invalid change: ${pos} is not a position in the text`;
  for (const s of list) {
    if (!s.path.startsWith(ROOT) || /["\\:]|\/\.\.?\//.test(s.path) || !IMAGE_EXTENSIONS.includes(extOf(s.path))) throw `Not an asset path: "${s.path}".`;
    if (!stagedBlobs.has(s.blob)) throw `invalid change: blob ${s.blob} is not stored`;
    const held = assets.get(s.path);
    if (held && held.blob !== s.blob) throw `${s.path} was just taken by another file; try again.`;
  }
  return assetStep(() => {
    for (const s of list) {
      if (assets.has(s.path)) continue;
      assets.set(s.path, { size: stagedBlobs.get(s.blob)!, blob: s.blob });
      markFolder(s.path);
    }
    sortAssets();
    return doc.sliceString(0, pos) + (a.text as string) + doc.sliceString(pos);
  });
}

/** Like `import_read`: any file reads as the same short Markdown note with one image (staged if asked) and one warning. */
function importRead(a: Args) {
  const figure = a.stage ? [stage("harbour.svg", "assets/field-notes/", [], blobOf(new TextEncoder().encode("field notes")), 4_000)] : [];
  const image = figure.map((s) => `#figure(\n  image("${s.path}", width: 80%),\n  caption: [The harbour at dawn],\n)\n\n`).join("");
  return {
    format: "Markdown",
    text: `= Field Notes\n\nImported from a _Markdown_ file, with *bold* text and a formula $x^2$.\n\n${image}- first point\n- second point\n`,
    assets: figure,
    warnings: [{ kind: "html", count: 2, example: "<div>", message: "Dropped 2 pieces of HTML (e.g. <div>)." }],
    title: "Field Notes",
    author: "Ada Lovelace",
  };
}

/** How many string literals in the text name `path`. */
const usage = (text: string, path: string) => text.split(`"${path}"`).length - 1;

/** Like `asset_list`: folders from markers and paths, items with size, usage and status. */
function assetList() {
  const text = doc.toString();
  const all = new Set<string>();
  for (const p of [...assets.keys(), ...folders]) {
    for (let i = p.indexOf("/", ROOT.length); i >= 0; i = p.indexOf("/", i + 1)) all.add(p.slice(0, i + 1));
  }
  return {
    folders: [...all].sort(),
    items: [...assets].map(([path, a]) => ({ path, kind: "image", size: a.missing ? null : a.size, used_by: usage(text, path), status: a.missing ? "missing" : "available" })),
  };
}

/** Like `asset_move`: an item (same format) or a folder with everything below it; the text's literals follow. */
function moveAsset(a: Args): Reply & { path: string } {
  const from = a.from as string;
  const to = a.to as string;
  const folder = from.endsWith("/");
  let moves: Array<[string, string]>;
  if (folder) {
    if (from === ROOT || to === ROOT) throw "The assets folder itself can't be moved.";
    if (!to.startsWith(ROOT) || !to.endsWith("/")) throw `Not an asset folder: "${to}".`;
    if (to.startsWith(from)) throw "A folder can't move into itself.";
    moves = [...assets.keys(), ...folders].filter((p) => p.startsWith(from)).map((p) => [p, to + p.slice(from.length)]);
    if (!moves.length) throw `There is no folder ${from}.`;
  } else {
    if (!assets.has(from)) throw `invalid change: "${from}" is not in the index`;
    const ext = extOf(to);
    if (ext !== ext.toLowerCase() || !sameFormat(ext, extOf(from))) throw `A renamed image keeps its format: end the name with .${extOf(from)}.`;
    moves = [[from, to]];
  }
  for (const [, next] of moves) if (assets.has(next) || folders.has(next)) throw `invalid change: "${next}" exists already`;
  const r = assetStep(() => {
    let text = doc.toString();
    for (const [old, next] of moves) {
      const item = assets.get(old);
      if (item) {
        assets.delete(old);
        assets.set(next, item);
        markFolder(next);
        text = text.split(`"${old}"`).join(`"${next}"`);
      } else {
        folders.delete(old);
        folders.add(next);
      }
    }
    sortAssets();
    return text;
  });
  return { ...r, path: to };
}

/** Like `asset_remove`: an unused item, or a folder with nothing used in it. */
function removeAsset(a: Args): null {
  const path = a.path as string;
  const paths = path.endsWith("/") ? [...assets.keys(), ...folders].filter((p) => p.startsWith(path)) : assets.has(path) ? [path] : [];
  if (path === ROOT) throw "The assets folder itself can't be removed.";
  if (!paths.length) throw `invalid change: "${path}" is not in the index`;
  const text = doc.toString();
  const used = paths.find((p) => usage(text, p) > 0);
  if (used) throw `invalid change: "${used}" is used in the text`;
  assetStep(() => {
    for (const p of paths) {
      assets.delete(p);
      folders.delete(p);
    }
    return null;
  });
  return null;
}

/** Like `asset_folder_create`: an existing folder changes nothing. */
function createFolder(a: Args): null {
  const path = a.path as string;
  if (!path.startsWith(ROOT) || !path.endsWith("/") || /["\\:]/.test(path)) throw `Not an asset folder: "${path}".`;
  if (path === ROOT || assetList().folders.includes(path)) return null;
  assetStep(() => {
    folders.add(path);
    return null;
  });
  return null;
}

/** Like `asset_update`: new content of the same format under the same path (shown as another sky). */
function updateAsset(a: Args): null {
  const path = a.path as string;
  const source = a.source as string;
  const item = assets.get(path);
  if (!item) throw `There is no asset ${path}.`;
  if (!sameFormat(extOf(source).toLowerCase(), extOf(path))) throw `Pick a .${extOf(path)} file to replace ${path} (this one is .${extOf(source).toLowerCase()}).`;
  assetStep(() => {
    assets.set(path, { size: item.size + 1234 });
    replaced.set(path, (replaced.get(path) ?? 0) + 1);
    return null;
  });
  return null;
}

/** How often each image's predecessors were replaced: a replacement shows another sky. */
const replaced = new Map<string, number>();
const SKIES = [
  ["#2b2a4a", "#d98a5f"],
  ["#123a52", "#8fd0c4"],
  ["#40213f", "#f0c24e"],
];

/**
 * Like `asset_replace`: the picked file gets a path of its own (as an import)
 * and only the image call at `pos` is changed to it, one undo step.
 */
function replaceAsset(a: Args): Reply & { path: string } {
  const path = a.path as string;
  const pos = a.pos as number;
  const text = doc.toString();
  const call = /^#?image\(\s*"([^"\\\n]*)"/.exec(text.slice(pos));
  if (!call || call[1] !== path) throw `The image ${path} is no longer there; try again.`;
  const next = addAsset(a.source as string);
  replaced.set(next, (replaced.get(path) ?? 0) + 1);
  const from = pos + call[0].length - 1 - path.length;
  return { ...replaceAll(text.slice(0, from) + next + text.slice(from + path.length)), path: next };
}

/** A generated picture: a lighthouse with a beam (PNG via a canvas, or SVG). */
async function assetBytes(a: Args): Promise<ArrayBuffer> {
  const path = a.path as string;
  if (!assets.has(path)) throw `Image not found: ${path}`;
  if (assets.get(path)?.missing) throw `Image not found: ${path}`;
  const [top, bottom] = SKIES[(replaced.get(path) ?? 0) % SKIES.length];
  if (path.endsWith(".svg")) {
    const svg =
      `<svg xmlns="http://www.w3.org/2000/svg" width="480" height="240" viewBox="0 0 480 240">` +
      `<rect width="480" height="240" fill="${top}"/><path d="M240 70 L480 20 L480 120 Z" fill="#f4d58d" opacity="0.6"/>` +
      `<rect x="225" y="70" width="30" height="140" fill="#eee"/><rect x="220" y="55" width="40" height="18" fill="#c33"/>` +
      `<rect y="200" width="480" height="40" fill="#2d5f7a"/></svg>`;
    return new TextEncoder().encode(svg).buffer as ArrayBuffer;
  }
  const canvas = new OffscreenCanvas(640, 360);
  const g = canvas.getContext("2d")!;
  const sky = g.createLinearGradient(0, 0, 0, 360);
  sky.addColorStop(0, top);
  sky.addColorStop(1, bottom);
  g.fillStyle = sky;
  g.fillRect(0, 0, 640, 360);
  g.fillStyle = "rgba(255, 230, 150, 0.45)";
  g.beginPath();
  g.moveTo(330, 110);
  g.lineTo(640, 40);
  g.lineTo(640, 190);
  g.fill();
  g.fillStyle = "#f2efe8";
  g.fillRect(305, 110, 40, 200);
  g.fillStyle = "#b03a2e";
  g.fillRect(298, 88, 54, 24);
  g.fillStyle = "#1d3b4f";
  g.fillRect(0, 300, 640, 60);
  return (await canvas.convertToBlob({ type: /\.jpe?g$/.test(path) ? "image/jpeg" : "image/png" })).arrayBuffer();
}

/** File dialogs: a fixed .bib export or picture, the sync folders (other folder dialogs aren't available). */
function dialogOpen(a: Args) {
  const options = (a.options ?? {}) as { directory?: boolean; title?: string; filters?: Array<{ extensions: string[] }> };
  const exts = options.filters?.flatMap((f) => f.extensions) ?? [];
  // Open shared project… → Choose folder… / Share / Sync → Other folder….
  if (options.directory && /synced project/i.test(options.title ?? "")) return "C:/Mock/USB stick";
  if (options.directory && /sync folder/i.test(options.title ?? "")) return "C:/Mock/USB stick";
  if (!options.directory && exts.includes("bib")) return "C:/Mock/Downloads/zotero-export.bib";
  // File → Import…, Edit → Insert file….
  if (!options.directory && exts.includes("md")) return "C:/Mock/Documents/Field Notes.md";
  const multiple = (a.options as { multiple?: boolean } | undefined)?.multiple === true;
  if (!options.directory && exts.includes("svg")) return multiple ? ["C:/Mock/Pictures/Harbour Lights.svg"] : "C:/Mock/Pictures/Harbour Lights.svg";
  // Replace…: a file of the item's format.
  if (!options.directory && exts.length) return `C:/Mock/Pictures/New version.${exts[0]}`;
  throw "File dialogs are not available in the browser preview.";
}

/** A stand-in formula: the source in a serif face, an error for unbalanced brackets. */
function renderMath(a: Args) {
  const source = a.source as string;
  const size = a.sizePt as number;
  let depth = 0;
  for (const c of source) {
    if ("([{".includes(c)) depth++;
    else if (")]}".includes(c) && --depth < 0) break;
  }
  if (depth !== 0) return { error: "unclosed delimiter (browser preview)" };
  const text = source.trim().replace(/\s+/g, " ");
  const display = a.display as boolean;
  const width = Math.max(size * 0.6, text.length * size * 0.48);
  const height = size * (display ? 1.5 : 1.15);
  const baseline = height * 0.78;
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" width="${width}pt" height="${height}pt" viewBox="0 0 ${width} ${height}">` +
    `<text x="0" y="${baseline}" font-family="Georgia, serif" font-style="italic" font-size="${size}" ` +
    `textLength="${width}" lengthAdjust="spacingAndGlyphs" fill="${a.color as string}">${escapeXml(text)}</text></svg>`;
  return { ok: { svg, width, height, baseline } };
}

// ---- Spell check (a stand-in for crates/spell) ----

/** Misspelled in the preview: these (Mara from the sample, so it shows squiggles), anything with "qq", in English and German only. */
const MOCK_CORRECTIONS: Record<string, string[]> = {
  Mara: ["Mars", "Mary", "Maria", "Mira"],
  teh: ["the", "ten", "tech"],
  recieve: ["receive"],
  seperate: ["separate", "desperate"],
  Fahradschloss: ["Fahrradschloss"],
};
const mockDictionary = new Set<string>();
const mockChecked = (lang: unknown) => /^(en|de)([-_]|$)/i.test(String(lang));
const mockMisspelled = (word: string) => !mockDictionary.has(word) && (word in MOCK_CORRECTIONS || word.includes("qq"));

const spellCommands: Record<string, (a: Args) => unknown> = {
  spell_check: (a) => (mockChecked(a.lang) ? (a.words as string[]).filter(mockMisspelled) : []),
  spell_suggest: (a) => (mockChecked(a.lang) ? (MOCK_CORRECTIONS[a.word as string] ?? []) : []),
  spell_add: (a) => void mockDictionary.add(a.word as string),
};

// ---- Layout layer (a stand-in for crates/typeset/src/layout.rs) ----

type Scalar = string | number | boolean;
type Flat = Record<string, Scalar>;

const PAPER: Flat = {
  "page.size": "a4",
  "page.width": "210mm",
  "page.height": "297mm",
  "page.margin_top": "25mm",
  "page.margin_bottom": "25mm",
  "page.margin_inside": "30mm",
  "page.margin_outside": "30mm",
  "page.two_sided": false,
  "page.columns": 1,
  "page.column_gap": "6mm",
  "text.font": "New Computer Modern",
  "text.size": "11pt",
  "text.leading": "0.6em",
  "text.justify": true,
  "text.hyphenate": true,
  "text.lang": "en",
  "text.first_line_indent": "1.5em",
  "text.paragraph_spacing": "0.6em",
  "headings.numbering": "1.1.",
  "headings.chapter_pagebreak": "none",
  "headings.level1_size": "14pt",
  "headings.level2_size": "12pt",
  "headings.level3_size": "11pt",
  "headings.space_above": "",
  "headings.space_below": "",
  "figures.caption_position": "bottom",
  "figures.numbering": true,
  "figures.default_width": 80,
  "tables.style": "academic",
  "tables.header_bold": true,
  "tables.caption_position": "top",
  "equations.numbering": true,
  "citations.style": "apa",
  "citations.bibliography_title": "",
  "furniture.page_numbers": "bottom-center",
  "furniture.running_head": "none",
  "front.title_page": false,
  "front.toc": false,
  "front.toc_depth": 3,
  "front.toc_title": "",
  "front.toc_dots": true,
  "front.toc_indent": true,
  "front.toc_spacing": "",
  "front.toc_bold_top": false,
  "export.image_quality": "original",
};

const PRESET_VALUES: Record<string, Flat> = {
  paper: PAPER,
  novel: {
    ...PAPER,
    "page.size": "us-trade",
    "page.width": "6in",
    "page.height": "9in",
    "page.margin_top": "0.75in",
    "page.margin_bottom": "0.85in",
    "page.margin_inside": "0.875in",
    "page.margin_outside": "0.625in",
    "page.two_sided": true,
    "text.font": "Libertinus Serif",
    "text.leading": "0.65em",
    "text.first_line_indent": "1.2em",
    "text.paragraph_spacing": "0.65em",
    "headings.numbering": "none",
    "headings.chapter_pagebreak": "right",
    "headings.level1_size": "20pt",
    "headings.level2_size": "14pt",
    "figures.default_width": 100,
    "tables.style": "plain",
    "tables.caption_position": "bottom",
    "equations.numbering": false,
    "citations.style": "chicago-author-date",
    "furniture.page_numbers": "top-outside",
    "furniture.running_head": "chapter",
    "front.title_page": true,
  },
  thesis: {
    ...PAPER,
    "page.margin_top": "30mm",
    "page.margin_bottom": "30mm",
    "page.margin_inside": "35mm",
    "page.margin_outside": "25mm",
    "page.two_sided": true,
    "text.font": "Libertinus Serif",
    "text.leading": "0.8em",
    "text.first_line_indent": "0pt",
    "text.paragraph_spacing": "1.1em",
    "headings.chapter_pagebreak": "right",
    "headings.level1_size": "22pt",
    "headings.level2_size": "15pt",
    "headings.level3_size": "12pt",
    "furniture.page_numbers": "bottom-outside",
    "furniture.running_head": "chapter",
    "front.title_page": true,
    "front.toc": true,
  },
  manuscript: {
    ...PAPER,
    "page.size": "us-letter",
    "page.width": "8.5in",
    "page.height": "11in",
    "page.margin_top": "1in",
    "page.margin_bottom": "1in",
    "page.margin_inside": "1in",
    "page.margin_outside": "1in",
    "text.font": "Libertinus Serif",
    "text.size": "12pt",
    "text.leading": "1.6em",
    "text.justify": false,
    "text.hyphenate": false,
    "text.lang": "en-US",
    "text.first_line_indent": "0.5in",
    "text.paragraph_spacing": "1.6em",
    "headings.numbering": "none",
    "headings.chapter_pagebreak": "page",
    "headings.level1_size": "12pt",
    "headings.level2_size": "12pt",
    "headings.level3_size": "12pt",
    "figures.default_width": 100,
    "equations.numbering": false,
    "citations.style": "chicago-author-date",
    "furniture.page_numbers": "top-outside",
    "furniture.running_head": "author-title",
    "front.title_page": true,
  },
};

const PRESETS = [
  { id: "paper", name: "Scientific paper", description: "A4, compact, numbered sections, equations and figures, bibliography." },
  { id: "novel", name: "Novel / trade book", description: "6 × 9 in, justified serif text, indented paragraphs, chapters on right-hand pages, running heads." },
  { id: "thesis", name: "Thesis / report", description: "A4 two-sided with a binding gutter, title page, contents, numbered chapters on new pages." },
  { id: "manuscript", name: "Manuscript submission", description: "US Letter, 12 pt, double-spaced, ragged right, 1 in margins, title page, Author / Title / page header." },
];

const FONTS = [
  "Arial",
  "Cambria",
  "Candara",
  "Consolas",
  "Constantia",
  "DejaVu Sans Mono",
  "EB Garamond",
  "Georgia",
  "Libertinus Serif",
  "New Computer Modern",
  "Palatino Linotype",
  "Segoe UI",
  "Times New Roman",
  "Verdana",
];

const CHOICES: Record<string, string[]> = {
  "page.size": ["a4", "a5", "us-letter", "us-trade", "custom"],
  "headings.numbering": ["none", "1.", "1.1.", "I.", "A."],
  "headings.chapter_pagebreak": ["none", "page", "right"],
  "figures.caption_position": ["top", "bottom"],
  "tables.style": ["plain", "academic", "grid", "striped"],
  "tables.caption_position": ["top", "bottom"],
  "furniture.page_numbers": ["none", "bottom-center", "bottom-outside", "top-outside"],
  "furniture.running_head": ["none", "title", "chapter", "author-title"],
  "export.image_quality": ["original", "print", "screen", "email"],
};

/** Allowed lengths: points range, and an em range if em is allowed. */
const LENGTHS: Record<string, [number, number, [number, number]?]> = {
  "page.width": [72, 2880],
  "page.height": [72, 2880],
  "page.margin_top": [0, 288],
  "page.margin_bottom": [0, 288],
  "page.margin_inside": [0, 288],
  "page.margin_outside": [0, 288],
  "page.column_gap": [0, 72, [0, 5]],
  "text.size": [4, 72],
  "text.leading": [0, 72, [0, 5]],
  "text.first_line_indent": [0, 144, [0, 10]],
  "text.paragraph_spacing": [0, 144, [0, 10]],
  "headings.level1_size": [4, 96, [0.5, 5]],
  "headings.level2_size": [4, 96, [0.5, 5]],
  "headings.level3_size": [4, 96, [0.5, 5]],
  "headings.space_above": [0, 144, [0, 10]],
  "headings.space_below": [0, 144, [0, 10]],
  "front.toc_spacing": [0, 72, [0, 5]],
};

/** Lengths that may be empty (the default). */
const OPTIONAL_LENGTHS = ["headings.space_above", "headings.space_below", "front.toc_spacing"];

const PT_PER: Record<string, number> = { pt: 1, mm: 72 / 25.4, cm: 72 / 2.54, in: 72 };

function parseLength(s: string): { value: number; unit: string } | null {
  const m = /^(\d+(?:\.\d+)?)(pt|mm|cm|in|em)$/i.exec(s.trim());
  return m ? { value: Number(m[1]), unit: m[2].toLowerCase() } : null;
}

/** Points of a length (em relative to `em` points). */
function toPt(s: Scalar, em = 11): number {
  const l = parseLength(String(s));
  if (!l) return 0;
  return l.unit === "em" ? l.value * em : l.value * PT_PER[l.unit];
}

const shownValue = (value: unknown) => (typeof value === "string" ? `"${value}"` : JSON.stringify(value));

/** A length for `key` within `range`, re-printed; like the core's `check_length`. */
function checkLength(key: string, value: unknown, [lo, hi, ems]: [number, number, [number, number]?]): string {
  const shown = shownValue(value);
  if (typeof value !== "string") throw `${key}: expected text, got ${shown}.`;
  const l = parseLength(value);
  if (!l) throw `${key}: ${shown} is not a length (like 12pt, 2.5cm, 1in or 0.6em).`;
  if (l.unit === "em" && !ems) throw `${key}: use an absolute unit (pt, mm, cm or in).`;
  const ok = l.unit === "em" ? l.value >= ems![0] && l.value <= ems![1] : l.value * PT_PER[l.unit] >= lo && l.value * PT_PER[l.unit] <= hi;
  const text = `${l.value}${l.unit}`;
  if (!ok) throw `${key}: ${text} is out of range (${lo}pt to ${hi}pt${ems ? ` or ${ems[0]}em to ${ems[1]}em` : ""}).`;
  return text;
}

// ---- Character styles (a stand-in for the core's `layout::style_key` and friends) ----

const STYLE_FIELDS = ["name", "font", "size", "removed"];

/** The id and field of `styles.<id>.<field>`; `null` for other keys and bad ids or fields. */
function styleKey(key: string): [id: string, field: string] | null {
  const m = /^styles\.([a-z0-9]+(?:-[a-z0-9]+)*)\.([a-z]+)$/.exec(key);
  return m && m[1].length <= 48 && STYLE_FIELDS.includes(m[2]) ? [m[1], m[2]] : null;
}

function validateStyle(key: string, field: string, value: unknown): Scalar {
  const shown = shownValue(value);
  switch (field) {
    case "font": {
      if (typeof value !== "string") throw `${key}: expected text, got ${shown}.`;
      const name = value.trim();
      const font = FONTS.find((f) => f.toLowerCase() === name.toLowerCase());
      if (font) return font;
      // The core's `is_family_name`: any family, also one not in the list.
      if (/^[\p{L}\p{N} \-_.'&+]{1,100}$/u.test(name)) return name;
      throw `${key}: "${name}" is not a font family name.`;
    }
    case "size":
      return checkLength(key, value, LENGTHS["text.size"]);
    case "name": {
      const name = typeof value === "string" ? value.trim() : "";
      if (name && [...name].length <= 64 && !/\p{Cc}/u.test(name)) return name;
      throw `${key}: expected a name of 1 to 64 characters, without line breaks or control characters.`;
    }
    default:
      if (typeof value === "boolean") return value;
      throw `${key}: expected true or false, got ${shown}.`;
  }
}

/** The character styles of `flat` by id, like the core's `resolve`: removed ones and those with neither font nor size left out. */
function charStyles(flat: Flat): Record<string, Flat> {
  const groups: Record<string, Flat> = {};
  for (const [key, value] of Object.entries(flat)) {
    const style = styleKey(key);
    if (style) (groups[style[0]] ??= {})[style[1]] = value;
  }
  const out: Record<string, Flat> = {};
  for (const id of Object.keys(groups).sort()) {
    const { removed, ...style } = groups[id];
    if (removed !== true && ("font" in style || "size" in style)) out[id] = style;
  }
  return out;
}

function validateLayout(key: string, value: unknown): Scalar {
  const shown = shownValue(value);
  const style = styleKey(key);
  if (style) return validateStyle(key, style[1], value);
  if (key.startsWith("styles.")) throw `"${key}" is not a character style setting (styles.<id>.font, size, name or removed, with an id like eb-garamond-14pt).`;
  if (!(key in PAPER)) throw `Unknown layout setting "${key}".`;
  if (CHOICES[key]) {
    if (typeof value === "string" && CHOICES[key].includes(value)) return value;
    throw `${key}: ${shown} is not one of the choices.`;
  }
  const range = LENGTHS[key];
  if (range) {
    if (OPTIONAL_LENGTHS.includes(key) && typeof value === "string" && value.trim() === "") return "";
    return checkLength(key, value, range);
  }
  if (typeof PAPER[key] === "boolean") {
    if (typeof value === "boolean") return value;
    throw `${key}: expected true or false, got ${shown}.`;
  }
  switch (key) {
    case "page.columns":
      if (value === 1 || value === 2) return value;
      throw `${key}: expected 1 or 2, got ${shown}.`;
    case "front.toc_depth":
      if (typeof value === "number" && Number.isInteger(value) && value >= 1 && value <= 4) return value;
      throw `${key}: expected a whole number from 1 to 4, got ${shown}.`;
    case "figures.default_width":
      if (typeof value === "number" && value >= 10 && value <= 100) return Math.round(value);
      throw `${key}: expected a percentage from 10 to 100, got ${shown}.`;
    case "text.font": {
      const font = FONTS.find((f) => typeof value === "string" && f.toLowerCase() === value.trim().toLowerCase());
      if (font) return font;
      throw `${key}: the font ${shown} is not available.`;
    }
    case "text.lang": {
      const m = typeof value === "string" ? /^([a-z]{2,3})(?:[-_]([a-z]{2}))?$/i.exec(value.trim()) : null;
      if (m) return m[2] ? `${m[1].toLowerCase()}-${m[2].toUpperCase()}` : m[1].toLowerCase();
      throw `${key}: ${shown} is not a language code (like en, de or en-US).`;
    }
    case "citations.style":
      if (STYLES.some((s) => s.id === value)) return value as string;
      throw `${key}: unknown citation style ${shown}.`;
    default:
      if (typeof value !== "string") throw `${key}: expected text, got ${shown}.`;
      if (value.length > 200) throw `${key}: longer than 200 characters.`;
      return value;
  }
}

function validateMeta(key: string, value: unknown): string {
  if (!["title", "subtitle", "author", "date", "contact"].includes(key)) throw `Unknown metadata field "${key}".`;
  if (typeof value !== "string") throw `${key}: expected text, got ${JSON.stringify(value)}.`;
  if (key !== "contact" && /[\n\r]/.test(value)) throw `${key}: contains line breaks or control characters.`;
  return value;
}

/** Validates every change (all or nothing), like the core's `validate_changes`. */
function validateAll<T>(changes: Record<string, unknown>, check: (key: string, value: unknown) => T): Record<string, T | null> {
  const valid: Record<string, T | null> = {};
  const invalid: string[] = [];
  for (const [key, value] of Object.entries(changes)) {
    try {
      valid[key] = value === null ? null : check(key, value);
    } catch (e) {
      invalid.push(String(e));
    }
  }
  if (invalid.length) throw `Invalid settings (nothing was changed): ${invalid.join(" ")}`;
  return valid;
}

interface LayoutDoc {
  preset: string;
  /** Stored settings (may equal the preset's). */
  stored: Flat;
  meta: Record<string, string>;
  custom: string;
  /** Anchored overrides (a stand-in for the core's `overrides` map). */
  overrides: MockOverride[];
}

interface MockOverride {
  id: string;
  /** `null`: invalid stored entries (`error` says why). */
  kind: string | null;
  params: Record<string, unknown>;
  /** The anchor, as a UTF-16 offset (moved by text changes). */
  pos: number;
  detached: boolean;
  error?: string;
}

/** Sample overrides: applied, on a right-hand page, a figure, a table, one detached, one not applicable, one invalid. */
function sampleOverrides(text: string): MockOverride[] {
  const at = (s: string) => Math.max(0, text.indexOf(s));
  return [
    { id: "sp1", kind: "space_before", params: { length: "18pt" }, pos: at("== The Arithmetic of Light"), detached: false },
    { id: "fp1", kind: "figure_placement", params: { placement: "top" }, pos: at("#figure(") + 1, detached: false },
    { id: "ts1", kind: "table_style", params: { style: "grid" }, pos: at("table("), detached: false },
    { id: "cb1", kind: "column_break", params: {}, pos: at("The keepers before them"), detached: true },
    { id: "pb2", kind: "page_break", params: { to_odd: false }, pos: at("  = 1/3 $"), detached: false },
    { id: "pb1", kind: "page_break", params: { to_odd: true }, pos: at("=== A Few Formulas"), detached: false },
    { id: "zz9", kind: null, params: {}, pos: 0, detached: false, error: "figure_placement: placement must be top, bottom, here or auto." },
  ];
}

// The book (`?book=1`) as a two-sided thesis: ~2000 pages.
let layoutDoc: LayoutDoc = BOOK
  ? { preset: "thesis", stored: {}, meta: { title: "The Keeper's Book", author: "M. Ashdown" }, custom: "", overrides: [] }
  : { preset: "novel", stored: { "text.size": "11.5pt", "headings.numbering": "1.1.", "equations.numbering": true }, meta: { title: "The Lighthouse Keeper's Daughter", author: "M. Ashdown" }, custom: "", overrides: sampleOverrides(SAMPLE) };

const cloneLayout = (l: LayoutDoc): LayoutDoc => structuredClone(l);

function resolved(l: LayoutDoc = layoutDoc): Flat {
  return { ...PRESET_VALUES[l.preset], ...l.stored };
}

function layoutState() {
  const flat = resolved();
  const base = PRESET_VALUES[layoutDoc.preset];
  const styles = charStyles(flat);
  const layout: Record<string, unknown> = { schema: 1, styles };
  const overrides: Flat = {};
  for (const [key, value] of Object.entries(flat)) {
    // No preset defines styles: the fields of every resolved style differ.
    const style = styleKey(key);
    if (style) {
      if (style[0] in styles && style[1] !== "removed") overrides[key] = value;
      continue;
    }
    const [section, field] = key.split(".");
    ((layout[section] ??= {}) as Flat)[field] = value;
    if (base[key] !== value) overrides[key] = value;
  }
  const meta = { title: "", subtitle: "", author: "", date: "", contact: "", ...layoutDoc.meta };
  return structuredClone({ preset: layoutDoc.preset, layout, overrides, meta, custom: layoutDoc.custom, text_area: textArea(flat) });
}

/** The core's `layout::text_area`: one column's width, the text height and size, in points. */
function textArea(flat: Flat) {
  const [w, h] = pageSize(flat);
  const size = toPt(flat["text.size"]);
  const columns = flat["page.columns"] === 2 ? 2 : 1;
  const textWidth = w - toPt(flat["page.margin_inside"]) - toPt(flat["page.margin_outside"]);
  return {
    column_width_pt: Math.max(1, (textWidth - (columns - 1) * toPt(flat["page.column_gap"], size)) / columns),
    height_pt: Math.max(1, h - toPt(flat["page.margin_top"]) - toPt(flat["page.margin_bottom"])),
    font_size_pt: size,
  };
}

/** A layout change: one undo step (the state before it). */
function changeLayout(f: (l: LayoutDoc) => void) {
  const before = cloneLayout(layoutDoc);
  f(layoutDoc);
  undoStack.push(before);
  redoStack.length = 0;
  mergeable = false;
  bump();
  return layoutState();
}

const layoutCommands: Record<string, (a: Args) => unknown> = {
  layout_get: () => layoutState(),
  layout_set: (a) => {
    const changes = validateAll(a.changes as Record<string, unknown>, validateLayout);
    return changeLayout((l) => {
      for (const [key, value] of Object.entries(changes)) {
        if (value === null) delete l.stored[key];
        else l.stored[key] = value;
      }
    });
  },
  layout_apply_preset: (a) => {
    const id = a.id as string;
    if (!PRESET_VALUES[id]) throw `Unknown preset "${id}".`;
    return changeLayout((l) => {
      l.preset = id;
      l.stored = {};
    });
  },
  layout_presets: () => PRESETS,
  layout_fonts: () => FONTS,
  // No font files here: previews show the fonts by name (or fall back).
  layout_embedded_fonts: () => [],
  layout_font_file: () => [],
  meta_set: (a) => {
    const changes = validateAll(a.changes as Record<string, unknown>, validateMeta);
    return changeLayout((l) => {
      for (const [key, value] of Object.entries(changes)) {
        if (value) l.meta[key] = value;
        else delete l.meta[key];
      }
    });
  },
  layout_custom_set: (a) => {
    const text = a.text as string;
    if (text.length > 256 * 1024) throw "The custom Typst code is longer than 256 KB.";
    if (text === layoutDoc.custom) return layoutState();
    return changeLayout((l) => {
      l.custom = text;
    });
  },
};

// ---- Anchored overrides (a stand-in for lib.rs / crates/typeset/src/overrides.rs) ----

const BLOCK_KINDS = ["page_break", "column_break", "space_before"];
const KIND_LABELS: Record<string, string> = {
  page_break: "page break",
  column_break: "column break",
  space_before: "space before",
  figure_placement: "figure placement",
  image_quality: "image quality",
  table_style: "table style",
};
/** Last compile: whether each override was applied. */
let lastApplied = new Map<string, boolean>();

/** Text changes move anchors; an anchor whose text was deleted is detached. */
function mapOverrides(set: ChangeSet): void {
  for (const o of layoutDoc.overrides) {
    if (o.kind === null) continue;
    const kept = set.mapPos(Math.min(o.pos, set.length), 1, MapMode.TrackDel);
    if (kept === null) {
      o.detached = true;
      o.pos = set.mapPos(Math.min(o.pos, set.length), 1);
    } else o.pos = kept;
  }
  // The review items' ranges too.
  mapReview(set);
}

const lineStartOf = (text: string, pos: number) => text.lastIndexOf("\n", pos - 1) + 1;

function lineEndOf(text: string, pos: number): number {
  const end = text.indexOf("\n", pos);
  return end < 0 ? text.length : end;
}

/** Roughly the core's `is_block_level`: not indented, not a closing bracket, not inside math. */
function isTopLevel(text: string, lineStart: number): boolean {
  if (/^[ \t)\]]/.test(text.slice(lineStart, lineStart + 1))) return false;
  const dollars = text.slice(0, lineStart).match(/(?<!\\)\$/g)?.length ?? 0;
  return dollars % 2 === 0;
}

/** Offsets of `name` in each `name(` call (`#figure(` for figures), with the call's end. */
function figureCalls(text: string, name: "figure" | "image" | "table" = "figure"): Array<{ at: number; end: number }> {
  const out: Array<{ at: number; end: number }> = [];
  const re = name === "figure" ? /#figure\(/g : name === "image" ? /\bimage\(/g : /\btable\(/g;
  for (const m of text.matchAll(re)) {
    const open = m.index + m[0].length - 1;
    let depth = 0;
    let end = text.length;
    for (let i = open; i < text.length; i++) {
      if (text[i] === "(") depth++;
      else if (text[i] === ")" && --depth === 0) {
        end = i + 1;
        break;
      }
    }
    out.push({ at: open - name.length, end });
  }
  return out;
}

/** The `image(` call containing `pos` (like `Scan::image_at`). */
function imageAt(text: string, pos: number): number | null {
  return figureCalls(text, "image").find((c) => pos >= c.at && pos < c.end)?.at ?? null;
}

/** The first `image(` call inside the figure call at `figure`. */
function imageIn(text: string, figure: number): number | null {
  const call = figureCalls(text).find((c) => c.at === figure);
  return figureCalls(text, "image").find((c) => call && c.at > call.at && c.at < call.end)?.at ?? null;
}

/** The first `table(` call inside the figure call at `figure`. */
function tableIn(text: string, figure: number): number | null {
  const call = figureCalls(text).find((c) => c.at === figure);
  return figureCalls(text, "table").find((c) => call && c.at > call.at && c.at < call.end)?.at ?? null;
}

/** The `table(` call containing `pos`, else the first in the figure there, else one starting on its line (like `Scan::table_at`). */
function tableAt(text: string, pos: number): number | null {
  const calls = figureCalls(text, "table");
  const inside = calls.find((c) => pos >= c.at && pos < c.end);
  if (inside) return inside.at;
  const figure = figureAt(text, pos);
  const inFigure = figure === null ? null : tableIn(text, figure);
  if (inFigure !== null) return inFigure;
  const line = lineStartOf(text, pos);
  return calls.find((c) => c.at >= line && c.at <= lineEndOf(text, line))?.at ?? null;
}

/** The figure call containing `pos`, or starting on its line. */
function figureAt(text: string, pos: number): number | null {
  const calls = figureCalls(text);
  const inside = calls.find((c) => pos >= c.at - 1 && pos <= c.end);
  if (inside) return inside.at;
  const line = lineStartOf(text, pos);
  return calls.find((c) => c.at > line && c.at <= lineEndOf(text, line))?.at ?? null;
}

const MOCK_PLACEMENTS = ["top", "bottom", "here", "auto"];
const MOCK_QUALITIES = ["original", "print", "screen", "email"];
const MOCK_TABLE_STYLES = ["plain", "academic", "grid", "striped"];

/** The call a non-block override of `kind` applies to at `pos`. */
const callAt = (kind: string) => (kind === "image_quality" ? imageAt : kind === "table_style" ? tableAt : figureAt);

/** Like `overrides::validate_new`: normalized parameters, or the core's message. */
function validateOverride(kind: string, params: Record<string, unknown>): Record<string, unknown> {
  const allow = (keys: string[]) => {
    const bad = Object.keys(params).find((k) => !keys.includes(k));
    if (bad) throw `${kind}: unknown parameter "${bad}".`;
  };
  switch (kind) {
    case "page_break":
      allow(["to_odd"]);
      if (params.to_odd !== undefined && typeof params.to_odd !== "boolean") throw `${kind}: to_odd must be true or false.`;
      return { to_odd: params.to_odd === true };
    case "column_break":
      allow([]);
      return {};
    case "space_before": {
      allow(["length"]);
      if (typeof params.length !== "string") throw `${kind}: length must be text like 12pt or 1em.`;
      const l = parseLength(params.length);
      if (!l) throw `${kind}.length: "${params.length.trim()}" is not a length (like 12pt, 2.5cm, 1in or 0.6em).`;
      const ok = l.unit === "em" ? l.value <= 20 : l.value * PT_PER[l.unit] <= 288;
      if (!ok) throw `${kind}.length: ${l.value}${l.unit} is out of range (0pt to 288pt or 0em to 20em).`;
      return { length: `${l.value}${l.unit}` };
    }
    case "figure_placement":
      allow(["placement"]);
      if (!MOCK_PLACEMENTS.includes(params.placement as string)) throw `${kind}: placement must be top, bottom, here or auto.`;
      return { placement: params.placement };
    case "image_quality":
      allow(["quality"]);
      if (!MOCK_QUALITIES.includes(params.quality as string)) throw `${kind}: quality must be original, print, screen or email.`;
      return { quality: params.quality };
    case "table_style":
      allow(["style"]);
      if (!MOCK_TABLE_STYLES.includes(params.style as string)) throw `${kind}: style must be plain, academic, grid or striped.`;
      return { style: params.style };
    default:
      throw `Unknown override kind "${kind}".`;
  }
}

function overrideInfos() {
  const text = doc.toString();
  return layoutDoc.overrides
    .map((o) => {
      const pos = o.kind === null ? null : Math.min(o.pos, text.length);
      return {
        id: o.id,
        kind: o.kind,
        params: structuredClone(o.params),
        anchor_preview: pos === null ? "" : text.slice(pos, lineEndOf(text, pos)).slice(0, 60),
        pos_utf16: pos,
        detached: o.detached,
        applicable: lastApplied.get(o.id) ?? null,
        error: o.error ?? null,
      };
    })
    .sort((a, b) => Number(a.pos_utf16 === null) - Number(b.pos_utf16 === null) || (a.pos_utf16 ?? 0) - (b.pos_utf16 ?? 0) || a.id.localeCompare(b.id));
}

function overrideAdd(a: Args): string {
  const kind = a.kind as string;
  const params = validateOverride(kind, (a.params ?? {}) as Record<string, unknown>);
  const pos = a.posUtf16 as number;
  const text = doc.toString();
  if (pos > text.length) throw `Position ${pos} is beyond the end of the text.`;
  const same = layoutDoc.overrides.filter((o) => o.kind === kind);
  let anchor: number;
  let existing: MockOverride | undefined;
  if (BLOCK_KINDS.includes(kind)) {
    anchor = lineStartOf(text, pos);
    if (!isTopLevel(text, anchor)) throw "This can't go here: the line is inside raw text, a comment, math, code or a function's argument.";
    existing = same.find((o) => lineStartOf(text, Math.min(o.pos, text.length)) === anchor);
  } else {
    const at = callAt(kind);
    const call = at(text, pos);
    if (call === null) throw kind === "image_quality" ? "There is no image here." : kind === "table_style" ? "There is no table here." : "There is no figure here.";
    anchor = call;
    existing = same.find((o) => at(text, Math.min(o.pos, text.length)) === call);
  }
  const id = existing?.id ?? Math.random().toString(36).slice(2, 8);
  changeLayout((l) => {
    l.overrides = l.overrides.filter((o) => o.id !== id);
    l.overrides.push({ id, kind, params, pos: anchor, detached: false });
  });
  return id;
}

function overrideRemove(a: Args): null {
  const id = a.id as string;
  if (!layoutDoc.overrides.some((o) => o.id === id)) throw `There is no override "${id}".`;
  changeLayout((l) => {
    l.overrides = l.overrides.filter((o) => o.id !== id);
  });
  return null;
}

function overrideUpdate(a: Args): null {
  const id = a.id as string;
  const o = layoutDoc.overrides.find((o) => o.id === id);
  if (!o || o.kind === null) throw `There is no override "${id}".`;
  const params = validateOverride(o.kind, (a.params ?? {}) as Record<string, unknown>);
  changeLayout((l) => {
    l.overrides.find((x) => x.id === id)!.params = params;
  });
  return null;
}

/** Several updates and removes as one undo step; all or nothing (like `overrides_batch`). */
function overridesBatch(a: Args): null {
  const ops = a.ops as Array<{ op: "update" | "remove"; id: string; params?: Record<string, unknown> }>;
  if (ops.length === 0) return null;
  const seen = new Set<string>();
  const checked = ops.map((op) => {
    const fail = (e: unknown) => `${String(e)} (exception "${op.id}"; nothing was changed)`;
    if (seen.has(op.id)) throw fail("It is named twice.");
    seen.add(op.id);
    const o = layoutDoc.overrides.find((o) => o.id === op.id);
    if (!o || (op.op === "update" && o.kind === null)) throw fail(`There is no override "${op.id}".`);
    try {
      return { ...op, params: op.op === "update" ? validateOverride(o.kind!, op.params ?? {}) : {} };
    } catch (e) {
      throw fail(e);
    }
  });
  changeLayout((l) => {
    for (const op of checked) {
      if (op.op === "remove") l.overrides = l.overrides.filter((o) => o.id !== op.id);
      else l.overrides.find((x) => x.id === op.id)!.params = op.params;
    }
  });
  return null;
}

/** What each override does in a compile of `text` (statuses and warnings, like `overrides::plan`). */
function overridePlan(text: string) {
  const statuses: Array<{ id: string; applied: boolean; detached: boolean }> = [];
  const diagnostics: MockDiagnostic[] = [];
  /** Block start → override id, for clicks on the injected code. */
  const blocks = new Map<number, string>();
  for (const o of layoutDoc.overrides) {
    if (o.kind === null) continue;
    const pos = Math.min(o.pos, text.length);
    const block = BLOCK_KINDS.includes(o.kind);
    const applied = block ? isTopLevel(text, lineStartOf(text, pos)) : callAt(o.kind)(text, pos) !== null;
    const label = KIND_LABELS[o.kind];
    if (applied && block) blocks.set(lineStartOf(text, pos), o.id);
    if (!applied) {
      const why = block
        ? "its position is inside raw text, a comment, math, code or a function's argument"
        : o.kind === "image_quality"
          ? "there is no image at its position (or another image quality already applies to it)"
          : o.kind === "table_style"
            ? "there is no table at its position (or another table style already applies to it)"
            : "there is no figure at its position (or another placement already applies to it)";
      diagnostics.push({ severity: "warning", message: `This ${label} is not applied: ${why}.`, hints: [], location: { in: "override", id: o.id } });
    }
    if (o.detached) {
      diagnostics.push({ severity: "warning", message: `The text this ${label} was anchored to was deleted; it applies where that text was.`, hints: [], location: { in: "override", id: o.id } });
    }
    statuses.push({ id: o.id, applied, detached: o.detached });
  }
  return { statuses, diagnostics, blocks };
}

const overrideCommands: Record<string, (a: Args) => unknown> = {
  overrides_get: () => overrideInfos(),
  override_add: overrideAdd,
  override_remove: overrideRemove,
  override_update: overrideUpdate,
  overrides_batch: overridesBatch,
};

// ---- Compiling (simulated) ----

type Size = [number, number];

interface MockDiagnostic {
  severity: "error" | "warning";
  message: string;
  hints: string[];
  location: Record<string, unknown>;
}

interface Held {
  version: number;
  sizes: Size[];
  flat: Flat;
  meta: Record<string, string>;
  /** Pages before the text (title page, contents). */
  front: number;
  /** Text pages per chapter, roughly. */
  chapterEvery: number;
  textPages: number;
  /** Line starts of the blocks, in order (clicks map onto them). */
  blocks: number[];
  /** Line starts of chapter headings. */
  headings: number[];
  /** `figure` offsets of the figure calls. */
  figures: number[];
  /** Block start → applied block override (its injected code). */
  overrideBlocks: Map<number, string>;
}

let version = 1;
let watching = false;
let held: Held | null = null;
let attempted = 0;
let lastStatus: unknown = null;
let compileTimer: ReturnType<typeof setTimeout> | undefined;
let compiling = false;

/** A document change: a new version, compiled after a pause while watching. */
function bump(): void {
  version++;
  if (watching) {
    clearTimeout(compileTimer);
    compileTimer = setTimeout(() => void compile(), 400);
  }
}

const PAGE_SIZES: Record<string, Size> = { a4: [595.28, 841.89], a5: [419.53, 595.28], "us-letter": [612, 792], "us-trade": [432, 648] };

function pageSize(flat: Flat): Size {
  const named = PAGE_SIZES[flat["page.size"] as string];
  return named ?? [toPt(flat["page.width"]), toPt(flat["page.height"])];
}

function byteOffset(s: string, utf16: number): number {
  return new TextEncoder().encode(s.slice(0, utf16)).length;
}

function contentDiagnostic(severity: "error" | "warning", text: string, from: number, to: number, message: string, hints: string[] = []): MockDiagnostic {
  return {
    severity,
    message,
    hints,
    location: { in: "content", start: byteOffset(text, from), end: byteOffset(text, to), start_utf16: from, end_utf16: to },
  };
}

/** Plausible problems: unknown references, missing images, `#error` in the text, `#oops` in the expert code. */
function diagnose(text: string): MockDiagnostic[] {
  const out: MockDiagnostic[] = [];
  if (text.length < 1_000_000) {
    const labels = new Set([...text.matchAll(/<([\w:.-]+)>/g)].map((m) => m[1]));
    for (const m of text.matchAll(/(?<![\w\\])@([\w:.-]*\w)/g)) {
      const key = m[1];
      if (!labels.has(key) && !bib.some((e) => e.key === key)) {
        out.push(contentDiagnostic("warning", text, m.index, m.index + m[0].length, `No source or label named "${key}".`, ["Add it in Sources (Ctrl+Shift+B) or check the spelling."]));
      }
    }
    for (const m of text.matchAll(/image\("([^"]+)"/g)) {
      if (!assets.has(m[1])) out.push({ severity: "warning", message: `File not found: ${m[1]} (shown as an empty box).`, hints: [], location: { in: "file", path: m[1] } });
    }
    for (const m of text.matchAll(/#error\b/g)) {
      out.push(contentDiagnostic("error", text, m.index, m.index + m[0].length, "unknown variable: error", ["Did you mean to write a literal #? Escape it as \\#."]));
    }
  }
  const custom = layoutDoc.custom;
  for (const m of custom.matchAll(/#oops\b/g)) {
    out.push({ severity: "error", message: "unknown variable: oops", hints: [], location: { in: "custom", start: byteOffset(custom, m.index), end: byteOffset(custom, m.index + m[0].length) } });
  }
  if (!FONTS.includes(resolved()["text.font"] as string)) {
    out.push({ severity: "warning", message: `unknown font family: ${resolved()["text.font"] as string}`, hints: [], location: { in: "preamble" } });
  }
  return out;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function emitStatus(status: Record<string, unknown>): void {
  void emit("compile-status", status);
}

async function compile(): Promise<void> {
  if (compiling) {
    // The real compile thread is sequential too.
    clearTimeout(compileTimer);
    compileTimer = setTimeout(() => void compile(), 100);
    return;
  }
  compiling = true;
  const v = version;
  attempted = v;
  emitStatus({ state: "compiling", version: v, pages_version: held?.version ?? null, pages: 0, page_sizes: [], diagnostics: [], overrides: [], duration_ms: 0 });
  const started = performance.now();
  const text = doc.toString();
  const flat = resolved();
  const [w, h] = pageSize(flat);
  const size = toPt(flat["text.size"]);
  const textWidth = w - toPt(flat["page.margin_inside"]) - toPt(flat["page.margin_outside"]);
  const textHeight = h - toPt(flat["page.margin_top"]) - toPt(flat["page.margin_bottom"]);
  const charsPerPage = Math.max(200, (textWidth / (0.5 * size)) * (textHeight / (size * (1 + toPt(flat["text.leading"], 1)))));
  const chapters = Math.max(1, (text.match(/^= /gm) ?? []).length);
  const breaks = flat["headings.chapter_pagebreak"] !== "none" ? chapters : 0;
  const textPages = Math.max(1, Math.ceil(text.length / charsPerPage) + breaks);
  const front = (flat["front.title_page"] && (layoutDoc.meta.title || layoutDoc.meta.author) ? 1 : 0) + (flat["front.toc"] ? 1 : 0);
  const pages = front + textPages;
  const plan = overridePlan(text);
  const breaks2 = plan.statuses.filter((s) => s.applied && layoutDoc.overrides.find((o) => o.id === s.id)?.kind === "page_break").length;
  const allPages = pages + breaks2;
  const diagnostics = [...diagnose(text), ...plan.diagnostics];
  await sleep(120 + allPages * 0.35);
  const ok = !diagnostics.some((d) => d.severity === "error");
  lastApplied = new Map(plan.statuses.map((s) => [s.id, s.applied]));
  if (ok) {
    const sizes: Size[] = fixturePages ? FIXTURE.page_sizes : Array.from({ length: allPages }, () => [w, h]);
    const { blocks, headings } = blockStarts(text);
    held = {
      version: v,
      sizes,
      flat,
      meta: { ...layoutDoc.meta },
      front,
      chapterEvery: Math.max(1, Math.round((textPages + breaks2) / chapters)),
      textPages: fixturePages ? sizes.length - front : textPages + breaks2,
      blocks,
      headings,
      figures: figureCalls(text).map((c) => c.at),
      overrideBlocks: plan.blocks,
    };
    locateOverrides(held, text, plan.statuses);
    if (fixturePages) fixturePlaces(plan.statuses);
  }
  const status = {
    state: ok ? "done" : "failed",
    version: v,
    pages_version: held?.version ?? null,
    pages: held?.sizes.length ?? 0,
    page_sizes: held?.sizes ?? [],
    diagnostics,
    overrides: plan.statuses,
    duration_ms: Math.round(performance.now() - started),
  };
  lastStatus = status;
  compiling = false;
  emitStatus(status);
}

function compileWatch(a: Args): null {
  watching = a.active as boolean;
  clearTimeout(compileTimer);
  if (watching && attempted !== version) compileTimer = setTimeout(() => void compile(), 0);
  return null;
}

function compileNow(a: Args): null {
  // Forced (F5): a new version, as in the app.
  if (a.force) version++;
  if (attempted === version && held && lastStatus) emitStatus(lastStatus as Record<string, unknown>);
  else void compile();
  return null;
}

const LINE_WIDTHS = [1, 0.97, 1, 0.99, 0.94, 1, 0.98, 0.62, 1, 0.96, 1, 0.99, 0.85, 1, 0.97, 0.4];

/** Line starts of top-level blocks (a line after a blank line, or a heading/list/figure/equation line). */
function blockStarts(text: string): { blocks: number[]; headings: number[] } {
  const blocks: number[] = [];
  const headings: number[] = [];
  let prevBlank = true;
  // `$` seen so far (isTopLevel per line would be quadratic on the book).
  let dollars = 0;
  for (let start = 0; start <= text.length; ) {
    const end = lineEndOf(text, start);
    const full = text.slice(start, end);
    const line = full.slice(0, 4);
    const blank = /^\s*$/.test(full);
    if (!blank && (prevBlank || /^[=\-+#$]/.test(line)) && !/^[ \t)\]]/.test(line) && dollars % 2 === 0) {
      blocks.push(start);
      if (line.startsWith("= ")) headings.push(start);
    }
    dollars += full.match(/(?<!\\)\$/g)?.length ?? 0;
    prevBlank = blank;
    if (end >= text.length) break;
    start = end + 1;
  }
  return { blocks, headings };
}

/** Where things are on page `index` of the held document (points). */
function pageGeometry(h: Held, index: number) {
  const { flat } = h;
  const [w, ph] = h.sizes[index];
  const size = toPt(flat["text.size"]);
  const left = flat["page.two_sided"] === true && index % 2 === 1;
  const inside = toPt(flat["page.margin_inside"]);
  const outside = toPt(flat["page.margin_outside"]);
  const x0 = left ? outside : inside;
  const x1 = w - (left ? inside : outside);
  const top = toPt(flat["page.margin_top"]);
  const bottom = ph - toPt(flat["page.margin_bottom"]);
  const body = index - h.front;
  const starts = body >= 0 && body % h.chapterEvery === 0;
  const h1 = toPt(flat["headings.level1_size"], size);
  const headingEnd = starts ? top + size + h1 * 2.9 : top;
  // Every third text page shows a figure (if the text has any).
  const figure = body >= 0 && !starts && body % 3 === 1 && h.figures.length > 0 ? h.figures[Math.floor(body / 3) % h.figures.length] : null;
  const fx = x0 + (x1 - x0) * 0.15;
  const fy = top + (bottom - top) * 0.3;
  const box = figure === null ? null : { x: fx, y: fy, w: (x1 - x0) * 0.7, h: (bottom - top) * 0.28, at: figure };
  return { w, h: ph, size, left, x0, x1, top, bottom, body, starts, h1, headingEnd, box };
}

/** Plausible places of the applied overrides on the mock pages (like `overrides::locate`). */
function locateOverrides(h: Held, text: string, statuses: Array<{ id: string; applied: boolean }>): void {
  const size = toPt(h.flat["text.size"]);
  for (const s of statuses) {
    const o = layoutDoc.overrides.find((x) => x.id === s.id);
    if (!s.applied || !o || o.kind === null) continue;
    if (o.kind === "figure_placement") {
      // The page showing the figure's box (with its caption).
      const at = figureAt(text, Math.min(o.pos, text.length));
      for (let i = h.front; i < h.sizes.length; i++) {
        const box = pageGeometry(h, i).box;
        if (box && box.at === at) {
          Object.assign(s, { page: i, x_pt: box.x, y_pt: box.y, width_pt: box.w, height_pt: box.h + size * 2 });
          break;
        }
      }
      continue;
    }
    if (o.kind === "table_style") {
      // The figure box holding the table, if it is in one.
      const table = tableAt(text, Math.min(o.pos, text.length));
      for (let i = h.front; i < h.sizes.length; i++) {
        const box = pageGeometry(h, i).box;
        if (box && table !== null && tableIn(text, box.at) === table) {
          Object.assign(s, { page: i, x_pt: box.x, y_pt: box.y, width_pt: box.w, height_pt: box.h });
          break;
        }
      }
      continue;
    }
    if (o.kind === "image_quality") {
      // The image part of the figure box holding it, if it is in one.
      const image = imageAt(text, Math.min(o.pos, text.length));
      for (let i = h.front; i < h.sizes.length; i++) {
        const box = pageGeometry(h, i).box;
        if (box && image !== null && imageIn(text, box.at) === image) {
          Object.assign(s, { page: i, x_pt: box.x, y_pt: box.y, width_pt: box.w, height_pt: box.h });
          break;
        }
      }
      continue;
    }
    // Where `pageJump` maps the block, inverted.
    const block = lineStartOf(text, Math.min(o.pos, text.length));
    const i = Math.max(0, h.blocks.findIndex((b) => b >= block));
    const t = (i / Math.max(1, h.blocks.length)) * h.textPages;
    const page = Math.min(h.sizes.length - 1, h.front + Math.floor(t));
    const g = pageGeometry(h, page);
    if (o.kind === "page_break") Object.assign(s, { page, x_pt: g.x0, y_pt: g.top });
    else if (o.kind === "column_break") Object.assign(s, { page, x_pt: (g.x0 + g.x1) / 2 + size / 2, y_pt: g.top });
    else Object.assign(s, { page, x_pt: g.x0, y_pt: g.top + (t - Math.floor(t)) * (g.bottom - g.top), height_pt: toPt(String(o.params.length), size) });
  }
}

/** The fixture's places of the sample overrides (by id) instead of the estimated ones. */
function fixturePlaces(statuses: Array<Record<string, unknown> & { id: string }>): void {
  for (const s of statuses) {
    const f = FIXTURE.overrides.find((o) => o.id === s.id);
    if (!f) continue;
    for (const key of ["page", "x_pt", "y_pt", "width_pt", "height_pt"]) {
      if (key in f) s[key] = f[key];
      else delete s[key];
    }
  }
}

/** Like `page_jump`: the mock page's heading, figure box or text lines → offsets in the compiled text. */
function pageJump(a: Args) {
  const v = a.version as number;
  const index = a.page as number;
  if (!held || held.version !== v) throw `Version ${v} is not available.`;
  const none = { target: { in: "none" }, figure_utf16: null, image_utf16: null, table_utf16: null };
  if (index < held.front || index >= held.sizes.length) return none;
  const g = pageGeometry(held, index);
  const x = a.xPt as number;
  const y = a.yPt as number;
  if (x < g.x0 || x > g.x1 || y < g.top || y > g.bottom || held.blocks.length === 0) return none;
  const text = doc.toString();
  const content = (block: number, figure: number | null) => {
    const end = lineEndOf(text, block);
    const word = /^\S*/.exec(text.slice(block, end))![0].length;
    return {
      target: { in: "content", pos_utf16: Math.min(end, block + 2), start_utf16: block, end_utf16: block + word, block_start_utf16: block },
      figure_utf16: figure,
      image_utf16: figure === null ? null : imageIn(text, figure),
      table_utf16: figure === null ? null : tableIn(text, figure),
    };
  };
  if (g.box && x >= g.box.x && x <= g.box.x + g.box.w && y >= g.box.y && y <= g.box.y + g.box.h + g.size * 2) {
    return { ...content(lineStartOf(text, g.box.at), g.box.at), target: { in: "content", pos_utf16: g.box.at, start_utf16: g.box.at, end_utf16: g.box.at + 6, block_start_utf16: lineStartOf(text, g.box.at) } };
  }
  if (g.starts && y <= g.headingEnd) {
    const chapter = Math.floor(g.body / held.chapterEvery);
    const heading = held.headings[chapter] ?? held.blocks[0];
    return content(heading, null);
  }
  const t = ((g.body + (y - g.top) / (g.bottom - g.top)) / held.textPages) * held.blocks.length;
  const i = Math.min(held.blocks.length - 1, Math.max(0, Math.floor(t)));
  const block = held.blocks[i];
  const injected = held.overrideBlocks.get(block);
  if (injected && t - Math.floor(t) < 0.15) return { target: { in: "override", id: injected }, figure_utf16: null, image_utf16: null, table_utf16: null };
  const line = text.slice(block, lineEndOf(text, block));
  return content(block, line.startsWith("#figure") ? block + 1 : null);
}

/** A generated page: running head, a heading where a chapter starts, grey lines for text, the page number. */
function pageSvg(a: Args): ArrayBuffer {
  const v = a.version as number;
  const index = a.index as number;
  if (!held || held.version !== v) throw `Version ${v} is not available.`;
  if (index >= held.sizes.length) throw `There is no page ${index + 1}.`;
  if (fixturePages) return new TextEncoder().encode(FIXTURE_SVGS[`./fixtures/pages/page-${index + 1}.svg`]).buffer as ArrayBuffer;
  const { flat, meta } = held;
  const [w, h] = held.sizes[index];
  const size = toPt(flat["text.size"]);
  const twoSided = flat["page.two_sided"] === true;
  const left = twoSided && index % 2 === 1;
  const inside = toPt(flat["page.margin_inside"]);
  const outside = toPt(flat["page.margin_outside"]);
  const x0 = left ? outside : inside;
  const x1 = w - (left ? inside : outside);
  const top = toPt(flat["page.margin_top"]);
  const bottom = h - toPt(flat["page.margin_bottom"]);
  const font = escapeXml(`${flat["text.font"] as string}, Georgia, serif`);
  const ink = "#222";
  const parts = [`<svg xmlns="http://www.w3.org/2000/svg" width="${w}pt" height="${h}pt" viewBox="0 0 ${w} ${h}">`, `<rect width="${w}" height="${h}" fill="#fff"/>`];
  const text = (x: number, y: number, s: string, px: number, anchor = "start", extra = "") =>
    parts.push(`<text x="${x.toFixed(1)}" y="${y.toFixed(1)}" font-family="${font}" font-size="${px}" text-anchor="${anchor}" fill="${ink}" ${extra}>${escapeXml(s)}</text>`);
  const pageNo = index + 1;
  if (index < held.front) {
    if (index === 0 && flat["front.title_page"]) {
      text(w / 2, h * 0.36, meta.title || "Untitled", size * 2.2, "middle");
      if (meta.subtitle) text(w / 2, h * 0.36 + size * 2.4, meta.subtitle, size * 1.3, "middle", 'font-style="italic"');
      if (meta.author) text(w / 2, h * 0.52, meta.author, size * 1.2, "middle");
    } else {
      text(x0, top + size * 2, (flat["front.toc_title"] as string) || "Contents", toPt(flat["headings.level1_size"]));
      for (let i = 0; i < 12; i++) {
        const y = top + size * (5 + i * 1.8);
        text(x0, y, `Chapter ${i + 1}`, size);
        text(x1, y, String(held.front + 1 + i * held.chapterEvery), size, "end");
      }
    }
    parts.push("</svg>");
    return new TextEncoder().encode(parts.join("")).buffer as ArrayBuffer;
  }
  const body = index - held.front;
  const chapter = Math.floor(body / held.chapterEvery) + 1;
  const starts = body % held.chapterEvery === 0;
  const head = flat["furniture.running_head"] as string;
  if (head !== "none" && !starts) {
    const label = head === "chapter" ? `Chapter ${chapter}` : head === "author-title" ? `${meta.author || "Author"} / ${meta.title || "Title"}` : meta.title || "Untitled";
    text(left ? x0 : x1, top - size * 1.6, label, size * 0.8, left ? "start" : "end", 'font-style="italic"');
  }
  let y = top + size;
  if (starts) {
    const h1 = toPt(flat["headings.level1_size"], size);
    y += h1 * 1.5;
    const numbered = flat["headings.numbering"] !== "none" ? `${chapter}  ` : "";
    text(x0, y, `${numbered}Chapter ${chapter}`, h1, "start", 'font-weight="bold"');
    y += h1 * 1.4;
  }
  const box = pageGeometry(held, index).box;
  if (box) {
    parts.push(`<rect x="${box.x.toFixed(1)}" y="${box.y.toFixed(1)}" width="${box.w.toFixed(1)}" height="${box.h.toFixed(1)}" rx="2" fill="#d8dfe6" stroke="#9aa8b5"/>`);
    text(w / 2, box.y + box.h + size * 1.4, `Figure ${held.figures.indexOf(box.at) + 1}`, size * 0.85, "middle", 'font-style="italic"');
  }
  const columns = flat["page.columns"] === 2 ? 2 : 1;
  const gap = columns === 2 ? toPt(flat["page.column_gap"], size) : 0;
  const colWidth = (x1 - x0 - gap) / columns;
  const pitch = size * (1 + toPt(flat["text.leading"], 1));
  const indent = toPt(flat["text.first_line_indent"], size);
  for (let c = 0; c < columns; c++) {
    const cx = x0 + c * (colWidth + gap);
    for (let line = 0, ly = y; ly + size <= bottom - size; line++, ly += pitch) {
      if (box && ly >= box.y - size * 0.4 && ly - size <= box.y + box.h + size * 2) continue;
      const k = (line + pageNo * 3 + c * 5) % LINE_WIDTHS.length;
      const paragraphStart = k === 8 || k === 0;
      const dx = paragraphStart ? indent : 0;
      const width = (colWidth - dx) * LINE_WIDTHS[k];
      parts.push(`<rect x="${(cx + dx).toFixed(1)}" y="${(ly - size * 0.62).toFixed(1)}" width="${width.toFixed(1)}" height="${(size * 0.5).toFixed(1)}" rx="1" fill="#c9c6bf"/>`);
    }
  }
  const numbers = flat["furniture.page_numbers"] as string;
  const footY = bottom + (h - bottom) / 2;
  if (numbers === "bottom-center") text(w / 2, footY, String(pageNo), size * 0.85, "middle");
  else if (numbers === "bottom-outside") text(left ? x0 : x1, footY, String(pageNo), size * 0.85, left ? "start" : "end");
  else if (numbers === "top-outside" && !starts) text(left ? x0 : x1, top / 2, String(pageNo), size * 0.85, left ? "start" : "end");
  parts.push("</svg>");
  return new TextEncoder().encode(parts.join("")).buffer as ArrayBuffer;
}

async function exportPdf(): Promise<null> {
  if (!held || attempted !== version) await compile();
  if (!held || held.version !== version) throw "The document could not be typeset: unknown variable: error";
  await sleep(300);
  return null;
}

/** File → Export…: like a PDF export; Markdown keeps formulas as Typst (a warning), nothing is written. */
async function exportFile(a: Args) {
  await exportPdf();
  const formulas = (doc.toString().match(/\$/g) ?? []).length >> 1;
  if (a.format !== "markdown" || !formulas) return [];
  const message = formulas === 1 ? "Kept 1 formula as Typst source." : `Kept ${formulas} formulas as Typst source.`;
  return [{ kind: "typst-math", count: formulas, example: null, message }];
}

/** Save dialogs: a fixed folder for new projects, the exports folder for PDFs. */
function dialogSave(a: Args) {
  const options = (a.options ?? {}) as { defaultPath?: string; filters?: Array<{ extensions: string[] }> };
  if (options.filters?.some((f) => f.extensions.includes("pdf"))) return `C:/Mock/Exports/${options.defaultPath ?? "document.pdf"}`;
  if (options.defaultPath) return `C:/Mock/Downloads/${options.defaultPath}`;
  return "C:/Mock/Projects/New Book";
}

/** The default projects folder. */
const MOCK_PROJECTS = "C:/Mock/Documents/Lethora";

function projectNew(a: Args) {
  // Without a chosen place: `<name>.lethora` in the default projects folder (not numbered here).
  const path = typeof a.path === "string" ? a.path : `${MOCK_PROJECTS}/${String(a.name ?? "").trim() || "Untitled"}`;
  const preset = (a.preset as string | null) ?? "paper";
  if (!PRESET_VALUES[preset]) throw `Unknown preset "${preset}".`;
  const meta = validateAll((a.meta ?? {}) as Record<string, unknown>, validateMeta);
  project = { path: `${path}.lethora`, name: path.split(/[\\/]/).pop() || "New project" };
  mockSync.mode = "off";
  doc = docOf("");
  fixturePages = false;
  undoStack.length = 0;
  redoStack.length = 0;
  mergeable = false;
  assets = new Map();
  folders = new Set();
  held = null;
  attempted = 0;
  layoutDoc = { preset, stored: {}, meta: Object.fromEntries(Object.entries(meta).filter(([, v]) => v)) as Record<string, string>, custom: "", overrides: [] };
  version++;
  return opened();
}

// ---- Folder sync: a fake state machine ----
//
// `?sync=encrypted|plain|needsCode|unavailable|needsUpdate` starts in that state (plain:
// in a Dropbox folder, with the one-time notice). Sync apps: Dropbox and
// OneDrive. "Open shared project…" finds the replica "Sample" in Dropbox for
// any valid code; a code starting with `0000` finds nothing (then "Choose
// folder…" finds it on the USB stick) and one starting with `1111` finds it
// on this device already. In the console,
// `__mockRemoteEdit("text", at?)` makes the next pull insert `text` (default:
// at the end of the first paragraph) as another device would, and
// `__mockRemoteSettings()` makes it change the text size.
//
// Server sync: `?sync=server` starts shared on a server as its owner
// (`?sync=server-editor` as an editor, `?sync=server-reviewer` as a
// reviewer: the text read only, the review not; `?sync=server-reader` as a
// reader: read only, `?sync=server-offline` offline, `?sync=server-update`
// stopped for a newer Lethora). Members…: you (two
// devices), an editor and a reader, two open invites; role changes,
// removals, revokes, leaving and deleting (both make the project local)
// work, and `__mockRole("reader")` (or "reviewer", "editor", "owner")
// changes your own role as the owner would (applied live with the next
// pull). "Open shared project…" lists one remembered project without a copy
// here ("Field notes"), which reopens.
// Sharing works for any https address and plain http on local addresses
// (an address containing "down" never answers); invite links are made up;
// "Open shared project…" joins with any invite link (one containing "0000"
// is refused as used up). In server mode `__mockRemoteEdit` also fires
// `remote-changes`, so the edit arrives at once.

type MockSyncState = "ok" | "unavailable" | "needsCode" | "needsUpdate" | "error";
const MOCK_REPLICA = "C:/Mock/Dropbox/Lethora/Sample.lethora-sync";
const MOCK_APPS = [
  { id: "dropbox", label: "Dropbox", path: "C:/Mock/Dropbox" },
  { id: "onedrive", label: "OneDrive", path: "C:/Mock/OneDrive" },
];
const SYNC_START = new URLSearchParams(location.search).get("sync");
const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/** A code in the real format: 56 Crockford base32 characters in groups of 4 (not a real key). */
function fakeCode(): string {
  const chars = Array.from({ length: 56 }, () => CROCKFORD[Math.floor(Math.random() * 32)]);
  return chars.join("").replace(/(.{4})(?!$)/g, "$1-");
}

/** `?presence=1` (or `=N` for N online members): shared on a server, with members whose presence shows (see "Presence" below). */
const PRESENCE = new URLSearchParams(location.search).has("presence");
const SERVER_START = (SYNC_START?.startsWith("server") ?? false) || PRESENCE;

const mockSync = {
  mode: (SERVER_START ? "server" : SYNC_START === "plain" ? "plain" : SYNC_START === "encrypted" || SYNC_START === "needsCode" || SYNC_START === "unavailable" || SYNC_START === "needsUpdate" ? "encrypted" : "off") as "off" | "encrypted" | "plain" | "server",
  state: (SYNC_START === "needsCode" || SYNC_START === "unavailable" || SYNC_START === "needsUpdate" ? SYNC_START : "ok") as MockSyncState,
  server: "https://lethora.example",
  role: SYNC_START === "server-editor" ? "editor" : SYNC_START === "server-reviewer" ? "reviewer" : SYNC_START === "server-reader" ? "reader" : "owner",
  offline: SYNC_START === "server-offline",
  needsUpdate: SYNC_START === "server-update",
  folder: MOCK_REPLICA,
  code: SYNC_START === "encrypted" || SYNC_START === "unavailable" ? fakeCode() : null as string | null,
  lastSync: Date.now(),
  noticeDismissed: false,
};

const SYNC_MESSAGES: Partial<Record<MockSyncState, string>> = {
  unavailable: `The sync folder ${MOCK_REPLICA} is not available. Is the drive connected and the sync app running?`,
  needsCode: "This device has no project code for this project yet. Enter it to connect.",
  needsUpdate: "This project uses features of a newer Lethora; update to keep syncing.",
};

function syncStatus() {
  const s = mockSync;
  if (s.mode === "plain") return { mode: "plain", state: "ok", provider: "Dropbox", ...(s.noticeDismissed ? {} : { notice: { provider: "Dropbox" } }) };
  if (s.mode === "off") return { mode: "off", state: "ok" };
  if (s.mode === "server") {
    if (s.offline) {
      return { mode: "server", state: "offline", server: s.server, role: s.role, message: "Can't reach the server: connection refused.", retryAt: Date.now() + 8000 };
    }
    if (s.needsUpdate) return { mode: "server", state: "needsUpdate", server: s.server, role: s.role, message: "This project now needs Lethora 0.2.0 or later." };
    return { mode: "server", state: "online", server: s.server, role: s.role, lastSync: s.lastSync };
  }
  return { mode: "encrypted", folder: s.folder, state: s.state, ...(s.state === "ok" ? { lastSync: s.lastSync } : { message: SYNC_MESSAGES[s.state] }) };
}

/** Checks a typed code like `ProjectCode::parse` (length and alphabet; "0000…" stands for a wrong code unless `anyKey`). */
function parseCode(code: unknown, anyKey = false): string {
  const digits = String(code).replace(/[\s-]/g, "").toUpperCase().replace(/O/g, "0").replace(/[IL]/g, "1");
  const bad = [...digits].find((c) => !CROCKFORD.includes(c));
  if (bad) throw `This project code is not valid: "${bad}" is not used in codes.`;
  if (digits.length !== 56) throw `This project code is not valid: it has ${digits.length} characters instead of 56.`;
  if (digits.startsWith("0000") && !anyKey) throw "This project code does not unlock the synced project.";
  return digits.replace(/(.{4})(?!$)/g, "$1-");
}

function syncSetup(a: Args) {
  if (mockSync.mode === "encrypted") throw "This project is synced already. Stop syncing first to use another folder.";
  const folder = String(a.folder);
  if (/\.lethora-sync$/i.test(folder)) throw "This folder already holds a synced project; use Open shared project… instead.";
  Object.assign(mockSync, { mode: "encrypted", state: "ok", folder: `${folder}/${project.name}.lethora-sync`, code: fakeCode(), lastSync: Date.now() });
  return { code: mockSync.code, status: syncStatus() };
}

function syncSetupApp(a: Args) {
  const app = MOCK_APPS.find((x) => x.id === a.id);
  if (!app) throw "That sync app's folder was not found on this device any more.";
  return syncSetup({ folder: `${app.path}/Lethora` });
}

/** `sync_find`: "Sample" in Dropbox, or in the folder chosen (see the comment above). */
function syncFind(a: Args) {
  const digits = parseCode(a.code, true).replace(/-/g, "");
  const folder = typeof a.folder === "string" ? a.folder : null;
  const searched = folder === null ? MOCK_APPS.map((x) => x.label) : [];
  if (digits.startsWith("0000") && folder === null) return { searched };
  const replica = folder === null ? MOCK_REPLICA : /\.lethora-sync$/i.test(folder) ? folder : `${folder}/Sample.lethora-sync`;
  if (digits.startsWith("1111")) return { replica, searched, local: `${MOCK_PROJECTS}/Sample.lethora` };
  return { replica, searched };
}

function syncJoin(a: Args) {
  const code = parseCode(a.code, true);
  if (!/\.lethora-sync$/i.test(String(a.replica))) throw `${a.replica} is not a synced Lethora project. Choose the folder whose name ends in .lethora-sync.`;
  // By default in the default projects folder, named after the replica folder.
  const path = typeof a.path === "string" ? a.path : `${MOCK_PROJECTS}/${joinedName(String(a.replica))}`;
  project = { path: /\.lethora$/i.test(path) ? path : `${path}.lethora`, name: path.split(/[\\/]/).pop()!.replace(/\.lethora$/i, "") || "Joined project" };
  // The joined project holds what the other device wrote: the sample.
  undoStack.length = 0;
  redoStack.length = 0;
  mergeable = false;
  version++;
  Object.assign(mockSync, { mode: "encrypted", state: "ok", folder: String(a.replica), code, lastSync: Date.now() });
  return opened();
}

function syncEnterCode(a: Args) {
  if (mockSync.mode !== "encrypted") throw "Encrypted sync is not set up for this project.";
  if (mockSync.state === "unavailable") throw SYNC_MESSAGES.unavailable;
  Object.assign(mockSync, { code: parseCode(a.code), state: "ok", lastSync: Date.now() });
  return syncStatus();
}

function syncStop() {
  Object.assign(mockSync, { mode: "off", state: "ok", code: null });
  return syncStatus();
}

// ---- Server sync ----

const mockServers = ["https://lethora.example"];
let mockName: string | null = null;

/** Checks a server address like the core: https anywhere, http on local addresses only. */
function serverOrigin(url: unknown): string {
  const text = String(url).trim().replace(/\/$/, "");
  const m = /^(https?|wss?):\/\/([^/\s]+)$/i.exec(text);
  if (!m) throw `invalid change: server "${text}": it doesn't start with https://`;
  const scheme = /^(https|wss)$/i.test(m[1]) ? "https" : "http";
  const host = m[2].toLowerCase();
  const name = host.replace(/:\d+$/, "");
  const local = /^(localhost|.*\.local|.*\.localhost|127\.\d+\.\d+\.\d+|10\.\d+\.\d+\.\d+|192\.168\.\d+\.\d+|172\.(1[6-9]|2\d|3[01])\.\d+\.\d+|169\.254\.\d+\.\d+|\[::1\])$/.test(name);
  if (scheme === "http" && !local) throw `invalid change: server "${text}": plain http is only allowed for this machine and the local network`;
  return `${scheme}://${host}`;
}

const hex = (n: number) => Array.from({ length: n }, () => "0123456789abcdef"[Math.floor(Math.random() * 16)]).join("");
const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

async function serverShare(a: Args) {
  if (mockSync.mode === "encrypted") throw "This project syncs through a folder. Stop folder sync first to share it on a server.";
  if (mockSync.mode === "server") throw "This project is shared on a server already.";
  const url = serverOrigin(a.url);
  const name = String(a.name ?? "").trim();
  if (!name) throw "Enter your name, as others will see it.";
  await new Promise((r) => setTimeout(r, 900));
  if (url.includes("down")) throw "Sharing failed: No answer from the server in time. Check the address and try again.";
  Object.assign(mockSync, { mode: "server", server: url, role: "owner", offline: false, lastSync: Date.now() });
  mockServers.splice(0, mockServers.length, url, ...mockServers.filter((s) => s !== url));
  mockName = name;
  return syncStatus();
}

async function serverInvite(a: Args) {
  if (mockSync.mode !== "server") throw "This project is not shared on a server.";
  if (a.role && mockSync.role !== "owner") throw "Creating the invite failed: rejected: only the owner invites new members";
  await new Promise((r) => setTimeout(r, 400));
  const secret = Array.from({ length: 43 }, () => B64[Math.floor(Math.random() * 64)]).join("");
  return `${mockSync.server}/join/${hex(32)}/${hex(32)}#${secret}`;
}

async function serverJoin(a: Args) {
  const link = String(a.link);
  if (!isInviteLink(link)) throw "This is not a Lethora invite link.";
  if (link.includes("0000")) throw "Joining failed: This invite link can't be used any more (expired, used up or revoked). Ask for a new one.";
  await new Promise((r) => setTimeout(r, 900));
  mockName = String(a.name ?? "").trim() || mockName;
  project = { path: `${MOCK_PROJECTS}/Shared project.lethora`, name: "Shared project" };
  undoStack.length = 0;
  redoStack.length = 0;
  mergeable = false;
  version++;
  const server = /^[a-z]+:\/\/[^/]+/i.exec(link)![0];
  Object.assign(mockSync, { mode: "server", server, role: "editor", offline: false, lastSync: Date.now() });
  mockMembers = mockMembersStart();
  return opened();
}

// Members: the ids are made up; "you" are the owner unless the start state
// says otherwise (then Alice owns the project).
const MOCK_DEVICE = "d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0";
const MOCK_YOU = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";
const DAY = 24 * 60 * 60;

interface MockMember {
  user: string;
  name: string;
  colour: string;
  role: string;
  removed: boolean;
  devices: string[];
}

/** The profile colours (the core's `PALETTE`); yours can change. */
const MOCK_PALETTE = ["#0072b2", "#d55e00", "#009e73", "#cc79a7", "#e69f00", "#56b4e9", "#8e6c8a", "#a6761d"];
let mockColour = MOCK_PALETTE[0];

const youMember = (): MockMember => ({ user: MOCK_YOU, name: mockName ?? "Max", colour: mockColour, role: mockSync.role, removed: false, devices: [MOCK_DEVICE, "d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1"] });

function mockMembersStart(): MockMember[] {
  const others: MockMember[] = [
    { user: "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2", name: "Bea", colour: "#009e73", role: "editor", removed: false, devices: ["e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2"] },
    { user: "c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3", name: "Carl", colour: "#cc79a7", role: "reader", removed: false, devices: ["e3e3e3e3e3e3e3e3e3e3e3e3e3e3e3e3"] },
    { user: "d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4", name: "Dana", colour: "#e69f00", role: "editor", removed: true, devices: [] },
  ];
  if (mockSync.role !== "owner") others.unshift({ user: "f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0", name: "Alice", colour: "#d55e00", role: "owner", removed: false, devices: ["e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0"] });
  return others;
}

let mockMembers = mockMembersStart();
let mockInvites = [
  { id: "1111aaaa1111aaaa1111aaaa1111aaaa", creator: MOCK_DEVICE, creator_user: MOCK_YOU, role: "reviewer", expires: now() + 6 * DAY, uses_left: 5 },
  { id: "2222bbbb2222bbbb2222bbbb2222bbbb", creator: MOCK_DEVICE, creator_user: MOCK_YOU, expires: now() + DAY, uses_left: 1 },
];
let mockRemembered = [{ project: "9999cccc9999cccc9999cccc9999cccc", server: "https://lethora.example", title: "Field notes" }];

function onServer(): void {
  if (mockSync.mode !== "server") throw "This project is not shared on a server.";
}

function ownerOnly(what: string): void {
  onServer();
  if (mockSync.role !== "owner") throw `Not allowed: only the owner ${what}.`;
}

const later = () => new Promise((r) => setTimeout(r, 400));

/** Leaving or deletion: the copy turns into a local project. */
function turnLocal() {
  Object.assign(mockSync, { mode: "off", state: "ok", role: "owner" });
  mockMembers = mockMembersStart();
  return syncStatus();
}

const serverCommands: Record<string, (a: Args) => unknown> = {
  server_members: () => {
    onServer();
    return { user: MOCK_YOU, device: MOCK_DEVICE, title: project.name, members: [youMember(), ...mockMembers], invites: mockInvites, palette: MOCK_PALETTE };
  },
  server_set_role: async (a) => {
    ownerOnly("changes roles");
    await later();
    const m = mockMembers.find((x) => x.user === a.user && !x.removed);
    if (!m) throw `Not allowed: user ${a.user} is not a member.`;
    m.role = String(a.role);
  },
  server_set_colour: async (a) => {
    onServer();
    await later();
    if (!MOCK_PALETTE.includes(String(a.colour))) throw `${a.colour} is not one of the profile colours`;
    mockColour = String(a.colour);
  },
  server_remove: async (a) => {
    onServer();
    await later();
    if (typeof a.device === "string") {
      if (a.device === MOCK_DEVICE) throw "Use Leave to remove this device.";
      const m = [youMember(), ...mockMembers].find((x) => x.devices.includes(String(a.device)));
      if (m?.user !== MOCK_YOU && mockSync.role !== "owner") throw "Not allowed: a device is removed by the owner or its own user.";
      for (const x of mockMembers) x.devices = x.devices.filter((d) => d !== a.device);
      return;
    }
    ownerOnly("removes members");
    const m = mockMembers.find((x) => x.user === a.user);
    if (m) Object.assign(m, { removed: true, devices: [] });
  },
  server_revoke: async (a) => {
    onServer();
    await later();
    mockInvites = mockInvites.filter((i) => i.id !== a.invite);
  },
  server_leave: async () => {
    onServer();
    await later();
    return turnLocal();
  },
  server_delete: async () => {
    ownerOnly("deletes the project");
    await new Promise((r) => setTimeout(r, 900));
    return turnLocal();
  },
  server_remembered: () => mockRemembered,
  server_reopen: async (a) => {
    const entry = mockRemembered.find((p) => p.project === a.project);
    if (!entry) throw "This project is not remembered on this device.";
    await new Promise((r) => setTimeout(r, 900));
    mockRemembered = mockRemembered.filter((p) => p !== entry);
    project = { path: `${MOCK_PROJECTS}/${entry.title}.lethora`, name: entry.title };
    undoStack.length = 0;
    redoStack.length = 0;
    mergeable = false;
    version++;
    Object.assign(mockSync, { mode: "server", server: entry.server, role: "editor", offline: false, lastSync: Date.now() });
    mockMembers = mockMembersStart();
    return opening(opened());
  },
};

// ---- Presence ----
//
// With `?presence=1`, Bea's caret walks through the text a few words at a
// time (now and then selecting some) and Carl reads in the publisher view
// (online, no caret). Removed members go.

/** Bea's caret and how much she has selected. */
let beaAt = 0;
let beaSelected = 0;

function presenceGet() {
  if (!PRESENCE || mockSync.mode !== "server" || mockSync.offline) return [];
  const member = (name: string) => mockMembers.find((m) => m.name === name && !m.removed && m.devices.length > 0);
  const peer = (m: MockMember, view: string, anchor: number, head: number) => ({ device: m.devices[0], user: m.user, name: m.name, colour: m.colour, role: m.role, view, anchor, head });
  const bea = member("Bea");
  const carl = member("Carl");
  const at = Math.min(beaAt, doc.length);
  return [...(bea ? [peer(bea, "author", at, Math.min(at + beaSelected, doc.length))] : []), ...(carl ? [peer(carl, "publisher", 0, 0)] : []), ...extraReaders()];
}

/** `?presence=N` with N > 2: N − 2 more readers online (to see the row's "+N"). */
function extraReaders() {
  const n = Number(new URLSearchParams(location.search).get("presence")) - 2;
  const colours = ["#e69f00", "#56b4e9", "#009e73", "#d55e00", "#0072b2", "#cc79a7"];
  return Array.from({ length: Math.max(0, n) }, (_, i) => ({
    device: `mockreader${i}`.padEnd(32, "0"),
    user: `mockreaderuser${i}`.padEnd(32, "0"),
    name: `Reader ${String.fromCharCode(65 + i)}`,
    colour: colours[i % colours.length],
    role: "reader",
    view: "publisher",
    anchor: 0,
    head: 0,
  }));
}

if (PRESENCE) {
  setInterval(() => {
    const text = doc.toString();
    const words = /[\p{L}\p{N}]+/gu;
    words.lastIndex = Math.min(beaAt, text.length);
    // One to four words on; back to the start at the end.
    let hit: RegExpExecArray | null = null;
    for (let n = 1 + Math.floor(Math.random() * 4); n > 0; n--) hit = words.exec(text) ?? hit;
    beaAt = hit && hit.index > beaAt ? hit.index : 0;
    beaSelected = Math.random() < 0.3 ? (hit?.[0].length ?? 0) : 0;
    void emit("presence-changed");
  }, 2500);
}

// ---- Who wrote this ----
//
// With `?presence=1`, the text is by three authors, by line (paragraph): most
// by you or Bea, every fourth with a stretch by Carl, now and then a line by
// an unknown peer.

function authorsGet(a: Args) {
  if (!PRESENCE || mockSync.mode !== "server") return { runs: [], authors: [] };
  const you = youMember();
  const bea = mockMembers.find((m) => m.name === "Bea");
  const carl = mockMembers.find((m) => m.name === "Carl");
  const t = now();
  const times = new Map<string | null, number | null>([[you.user, t - 600], [bea?.user ?? "", t - DAY - 3 * 3600], [carl?.user ?? "", t - 6 * DAY], [null, null]]);
  const from = Math.max(0, Math.min(Number(a.from), doc.length));
  const to = Math.max(from, Math.min(Number(a.to), doc.length));
  const runs: { from: number; to: number; user: string | null; time: number | null }[] = [];
  const push = (start: number, end: number, user: string | null) => {
    [start, end] = [Math.max(start, from), Math.min(end, to)];
    if (start >= end) return;
    const last = runs[runs.length - 1];
    if (last && last.to === start && last.user === user) last.to = end;
    else runs.push({ from: start, to: end, user, time: times.get(user) ?? null });
  };
  for (let n = doc.lineAt(from).number; n <= doc.lines; n++) {
    const line = doc.line(n);
    if (line.from >= to) break;
    const end = Math.min(line.to + 1, doc.length);
    const writer = n % 9 === 4 ? null : (n * 7) % 5 < 3 || !bea ? you.user : bea.user;
    if (carl && n % 4 === 1 && line.length > 90) {
      push(line.from, line.from + 30, writer);
      push(line.from + 30, line.from + 90, carl.user);
      push(line.from + 90, end, writer);
    } else push(line.from, end, writer);
  }
  const used = new Set(runs.map((r) => r.user));
  const authors = [you, ...mockMembers].filter((m) => used.has(m.user)).map((m) => ({ user: m.user, name: m.name, colour: m.colour, you: m.user === you.user }));
  return { runs, authors };
}

// ---- Review: comments and highlights ----
//
// An in-memory review with the core's rules (crates/core/src/review.rs,
// "Permissions"): you are the mock's own user; seeded comments by Bea and by
// you (one with a reply, one resolved, one whose text was deleted, one by an
// unknown author), highlights in two colours and three open suggestions by
// Bea (a replacement, a deletion, and a stale one: its text changed). Suggest
// mode merges like the core (`suggestInto`, consecutive suggestions one undo
// step); Accept changes the mock's text like an undo reply. Ranges follow
// the mock's edits (`mapReview`). `__mockRemoteComment("text"?)` makes the
// next pull bring a comment by Bea, as another device would.

type MockAuthor = { user: string; device: string } | null;
interface MockMessage {
  id: number;
  author: MockAuthor;
  text: string;
  created: number;
  edited: number | null;
  deleted: boolean;
}
interface MockItem {
  id: number;
  from: number;
  to: number;
  detached: boolean;
  colour: string | null;
  resolved: boolean;
  author: MockAuthor;
  created: number;
  messages: MockMessage[];
  suggestion: { insert: string; original: string; state: string; stale: boolean } | null;
  removed: boolean;
}

const MOCK_BEA = "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2";
const youAuthor = (): MockAuthor => ({ user: MOCK_YOU, device: MOCK_DEVICE });
const beaAuthor: MockAuthor = { user: MOCK_BEA, device: "e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2" };
let reviewIds = 100;
const msg = (author: MockAuthor, text: string, ago: number): MockMessage => ({ id: ++reviewIds, author, text, created: now() - ago, edited: null, deleted: false });

/** The range of the first `phrase` in the text (or an empty one at the start). */
function phraseRange(phrase: string): { from: number; to: number } {
  const at = doc.toString().indexOf(phrase);
  return at < 0 ? { from: 0, to: 0 } : { from: at, to: at + phrase.length };
}

function seedItem(phrase: string, author: MockAuthor, ago: number, over: Partial<MockItem> = {}): MockItem {
  return { id: ++reviewIds, ...phraseRange(phrase), detached: false, colour: null, resolved: false, author, created: now() - ago, messages: [], suggestion: null, removed: false, ...over };
}

let reviewItems: MockItem[] = [
  seedItem("counted the steps", beaAuthor, 2 * DAY, {
    messages: [msg(beaAuthor, "Lovely opening. Is the number of steps on purpose?", 2 * DAY), msg(youAuthor(), "Yes: 112 comes back in the last chapter.", DAY + 3600)],
  }),
  seedItem("patient breaths", youAuthor(), 5 * 3600, { colour: "yellow" }),
  seedItem("the price of paraffin", youAuthor(), 40 * 60, { messages: [msg(youAuthor(), "Check: paraffin or kerosene in the 1890s?", 40 * 60)] }),
  seedItem("travelled", beaAuthor, 3 * 3600, { suggestion: { insert: "reached", original: "travelled", state: "open", stale: false }, messages: [msg(beaAuthor, "A beam doesn't travel miles, it reaches them.", 3 * 3600)] }),
  seedItem("single ", beaAuthor, 2 * 3600, { suggestion: { insert: "", original: "single ", state: "open", stale: false } }),
  // Made before "Ships and weather" became "Ships, weather": stale.
  seedItem("Ships, weather", beaAuthor, 30 * 3600, { suggestion: { insert: "Ships, wind and weather", original: "Ships and weather", state: "open", stale: true } }),
  seedItem("The lens turned once every twenty seconds", beaAuthor, 4 * DAY, {
    resolved: true,
    messages: [msg(beaAuthor, "Twenty or thirty seconds?", 4 * DAY), msg(youAuthor(), "Twenty, as in the Admiralty list.", 3 * DAY)],
  }),
  { ...seedItem("The keepers before them", beaAuthor, 6 * DAY, { messages: [msg(beaAuthor, "This sentence felt abrupt; cut it?", 6 * DAY)] }), detached: true },
  seedItem("underlined twice", beaAuthor, 26 * 3600, { colour: "green" }),
  seedItem("The beam swept the water", null, 9 * DAY, { messages: [msg(null, "A good last line.", 9 * DAY)] }),
];
// The detached one: its text is gone, it sits where the text was.
for (const i of reviewItems) if (i.detached) i.to = i.from;

/** Snapshots of the items before each review step (undo) and after an undone one (redo). */
const reviewUndo: MockItem[][] = [];
const reviewRedo: MockItem[][] = [];
/** The next pull's comment by Bea (`__mockRemoteComment`). */
let remoteComment: string | null = null;

const kindOf = (i: MockItem) => (i.suggestion ? "suggestion" : i.messages.length ? "comment" : "mark");

function reviewList() {
  const items = reviewItems
    .filter((i) => !i.removed)
    .map((i) => {
      const { removed: _removed, ...listed } = structuredClone(i);
      // The range's text now differs from the suggested one (or is gone).
      if (listed.suggestion) listed.suggestion.stale = i.detached || doc.sliceString(i.from, i.to) !== listed.suggestion.original;
      return { ...listed, kind: kindOf(i) };
    })
    .sort((a, b) => a.from - b.from || a.to - b.to || a.created - b.created || a.id - b.id);
  return { me: MOCK_YOU, items, undo: reviewUndo.length > 0, redo: reviewRedo.length > 0 };
}

/** The role's rights, as `Document::is_writable` and `is_review_writable` (a project of your own: the owner's). */
const mockWrites = () => mockSync.mode !== "server" || mockSync.role === "owner" || mockSync.role === "editor";
const mockReviews = () => mockSync.mode !== "server" || mockSync.role !== "reader";
const roleName = () => (mockSync.mode !== "server" ? "Owner" : mockSync.role[0].toUpperCase() + mockSync.role.slice(1));

/** Runs a review step (one undo step) after the role check. */
function reviewStep<T>(f: () => T, merge = false): T {
  if (!mockReviews()) throw `This project is read only for you (${roleName()}).`;
  const before = structuredClone(reviewItems);
  const out = f();
  if (!merge) reviewUndo.push(before);
  reviewRedo.length = 0;
  lastSuggestion = 0;
  return out;
}

/** When the last suggestion was made (ms; 0 after any other review step): the next one within a second joins its undo step. */
let lastSuggestion = 0;

function reviewItem(id: unknown): MockItem {
  const i = reviewItems.find((x) => x.id === Number(id) && !x.removed);
  if (!i) throw `Not found: review item ${String(id)}.`;
  return i;
}

function reviewMessage(item: MockItem, id: unknown): MockMessage {
  const m = item.messages.find((x) => x.id === Number(id));
  if (!m) throw `Not found: message ${String(id)}.`;
  return m;
}

/** Its author, or (`moderators`) also an Owner or Editor. */
function checkAuthor(author: MockAuthor, what: string, moderators: boolean): void {
  if (author?.user === MOCK_YOU || (moderators && mockWrites())) return;
  throw `Only its author can ${what}.`;
}

function checkRange(a: Args): { from: number; to: number } {
  const from = Number(a.from);
  const to = Number(a.to);
  if (!(0 <= from && from <= to && to <= doc.length)) throw `invalid change: range ${from}..${to} outside the text`;
  return { from, to };
}

function newItem(range: { from: number; to: number }, over: Partial<MockItem>): number {
  const item: MockItem = { id: ++reviewIds, ...range, detached: false, colour: null, resolved: false, author: youAuthor(), created: now(), messages: [], suggestion: null, removed: false, ...over };
  reviewItems.push(item);
  return item.id;
}

const MOCK_MARK_COLOURS = ["yellow", "green", "blue", "pink"];

const reviewCommands: Record<string, (a: Args) => unknown> = {
  review_list: reviewList,
  review_comment: (a) => reviewStep(() => newItem(checkRange(a), { messages: [msg(youAuthor(), String(a.text), 0)] })),
  review_mark: (a) =>
    reviewStep(() => {
      const range = checkRange(a);
      if (!MOCK_MARK_COLOURS.includes(String(a.colour))) throw `invalid change: unknown colour "${String(a.colour)}"`;
      if (range.from >= range.to) throw "invalid change: a mark needs some text";
      return newItem(range, { colour: String(a.colour) });
    }),
  review_reply: (a) =>
    reviewStep(() => {
      const m = msg(youAuthor(), String(a.text), 0);
      reviewItem(a.item).messages.push(m);
      return m.id;
    }),
  review_edit: (a) =>
    reviewStep(() => {
      const m = reviewMessage(reviewItem(a.item), a.message);
      checkAuthor(m.author, "edit this message", false);
      if (m.deleted) throw "invalid change: the message is deleted";
      Object.assign(m, { text: String(a.text), edited: now() });
      return null;
    }),
  review_delete: (a) =>
    reviewStep(() => {
      const m = reviewMessage(reviewItem(a.item), a.message);
      checkAuthor(m.author, "delete this message", true);
      Object.assign(m, { text: "", deleted: true });
      return null;
    }),
  review_resolve: (a) => reviewStep(() => void (reviewItem(a.item).resolved = a.resolved === true)) ?? null,
  review_colour: (a) =>
    reviewStep(() => {
      const i = reviewItem(a.item);
      checkAuthor(i.author, "recolour this item", false);
      if (!MOCK_MARK_COLOURS.includes(String(a.colour))) throw `invalid change: unknown colour "${String(a.colour)}"`;
      i.colour = String(a.colour);
      return null;
    }),
  review_remove: (a) =>
    reviewStep(() => {
      const i = reviewItem(a.item);
      checkAuthor(i.author, "remove this item", true);
      i.removed = true;
      return null;
    }),
  review_suggest: (a) => {
    const range = checkRange(a);
    if (range.from === range.to && !a.insert) return null;
    const merge = a.split !== true && Date.now() - lastSuggestion < 1000 && reviewUndo.length > 0;
    const made = reviewStep(() => {
      const s = suggestInto(reviewList().items as ReviewItem[], youAuthor()!, { ...range, insert: String(a.insert ?? "") }, (f, t) => doc.sliceString(f, t), () => ({ id: ++reviewIds, created: now() }));
      if (s.own === null && s.item) {
        const { kind: _kind, ...item } = s.item;
        reviewItems.push({ ...item, removed: false });
      } else if (s.own !== null) {
        const i = reviewItem(s.own);
        if (s.item) Object.assign(i, { from: s.item.from, to: s.item.to, suggestion: s.item.suggestion });
        else i.removed = true;
      }
      return s.item?.id ?? null;
    }, merge);
    lastSuggestion = Date.now();
    return made;
  },
  review_accept: (a) => {
    if (!mockWrites()) throw `This project is read only for you (${roleName()}).`;
    const i = openSuggestion(a.item);
    if (i.detached) throw "This suggestion can't be accepted: its text was deleted.";
    if (doc.sliceString(i.from, i.to) !== i.suggestion.original) throw "This suggestion can't be accepted: the text under it changed.";
    const change = { from: i.from, to: i.to, insert: i.suggestion.insert };
    // Bookkeeping, not undone with the review.
    for (const items of [reviewItems, ...reviewUndo, ...reviewRedo]) {
      for (const x of items) if (x.id === i.id && x.suggestion) x.suggestion.state = "accepted";
    }
    // A `main` undo step of its own.
    edit([change], true);
    mergeable = false;
    return { changes: [change], len16: doc.length };
  },
  review_reject: (a) =>
    reviewStep(() => {
      const i = openSuggestion(a.item);
      checkAuthor(i.author, "reject this suggestion", true);
      i.suggestion.state = "rejected";
      return null;
    }),
  review_undo: () => reviewHistory(reviewUndo, reviewRedo),
  review_redo: () => reviewHistory(reviewRedo, reviewUndo),
};

/** Item `id`, an open suggestion. */
function openSuggestion(id: unknown): MockItem & { suggestion: NonNullable<MockItem["suggestion"]> } {
  const i = reviewItem(id);
  if (!i.suggestion) throw `invalid change: review item ${String(id)} is not a suggestion`;
  if (i.suggestion.state !== "open") throw "invalid change: the suggestion was accepted or rejected already";
  return i as MockItem & { suggestion: NonNullable<MockItem["suggestion"]> };
}

function reviewHistory(from: MockItem[][], to: MockItem[][]): boolean {
  if (!mockReviews()) throw `This project is read only for you (${roleName()}).`;
  const s = from.pop();
  if (!s) return false;
  to.push(reviewItems);
  reviewItems = s;
  lastSuggestion = 0;
  return true;
}

/** Text changes move the ranges (also in the undo snapshots); a range whose text is all deleted is detached where it was. */
function mapReview(set: ChangeSet): void {
  for (const items of [reviewItems, ...reviewUndo, ...reviewRedo]) {
    for (const i of items) {
      const from = set.mapPos(Math.min(i.from, set.length), 1);
      const to = Math.max(from, set.mapPos(Math.min(i.to, set.length), -1));
      if (i.from < i.to && from === to) i.detached = true;
      Object.assign(i, { from, to: i.detached ? from : to });
    }
  }
}

/** The next pull's comment by Bea, on the last words of the first paragraph (`__mockRemoteComment`). */
function pullRemoteComment(): boolean {
  if (remoteComment === null) return false;
  const end = firstParagraphEnd(doc.toString());
  const from = Math.max(0, doc.toString().lastIndexOf(" ", Math.max(0, end - 12)) + 1);
  reviewItems.push({ ...seedItem("", beaAuthor, 0, { messages: [msg(beaAuthor, remoteComment, 0)] }), from, to: end });
  remoteComment = null;
  return true;
}

/** The next pull's remote changes (`__mockRemoteEdit`, `__mockRemoteSettings`). */
let remoteEdit: { text: string; at?: number } | null = null;
let remoteSettings = false;

/** The end of the first paragraph that isn't a heading (or the end of the text). */
function firstParagraphEnd(text: string): number {
  for (let start = 0; start < text.length; ) {
    let end = text.indexOf("\n\n", start);
    if (end < 0) end = text.length;
    const block = text.slice(start, end).trim();
    if (block && !/^[=#$]/.test(block)) return end;
    start = end + 2;
  }
  return text.length;
}

/** Maps an undo or redo stack (top last) over a remote change to the current text. */
function mapStack(stack: Step[], remote: ChangeSet): void {
  let r = remote;
  for (let i = stack.length - 1; i >= 0; i--) {
    const s = stack[i];
    const text = s instanceof ChangeSet ? s : s instanceof AssetStep ? s.text : null;
    if (!text) continue;
    const mapped = text.map(r);
    r = r.map(text, true);
    if (s instanceof ChangeSet) stack[i] = mapped;
    else (s as AssetStep).text = mapped;
  }
}

function syncPull() {
  const s = mockSync;
  if ((s.mode === "encrypted" && s.state === "ok") || (s.mode === "server" && !s.offline)) s.lastSync = Date.now();
  const out: Record<string, unknown> = { changes: [], len16: doc.length };
  if (remoteEdit && !(s.mode === "encrypted" && s.state !== "ok")) {
    const at = Math.max(0, Math.min(remoteEdit.at ?? firstParagraphEnd(doc.toString()), doc.length));
    const set = ChangeSet.of({ from: at, insert: remoteEdit.text }, doc.length);
    // Not on the local undo stack: the stacks are mapped over it.
    mapStack(undoStack, set);
    mapStack(redoStack, set);
    doc = set.apply(doc);
    mapOverrides(set);
    mergeable = false;
    Object.assign(out, { changes: [{ from: at, to: at, insert: remoteEdit.text }], len16: doc.length, changed: true });
    remoteEdit = null;
    bump();
  }
  if (remoteSettings) {
    layoutDoc.stored["text.size"] = resolved()["text.size"] === "12.5pt" ? "11pt" : "12.5pt";
    Object.assign(out, { changed: true, settings: true });
    remoteSettings = false;
    bump();
  }
  if (pullRemoteComment()) out.review = true;
  return { ...out, status: syncStatus() };
}

Object.assign(window, {
  __mockRemoteEdit: (text: string, at?: number) => {
    remoteEdit = { text, at };
    if (mockSync.mode === "server") void emit("remote-changes");
  },
  __mockOffline: (offline = true) => void (mockSync.offline = offline),
  __mockRole: (role: string) => {
    mockSync.role = role;
    mockMembers = mockMembersStart();
  },
  __mockRemoteSettings: () => void (remoteSettings = true),
  __mockRemoteComment: (text = "Could you cite a source for this?") => {
    remoteComment = text;
    if (mockSync.mode === "server") void emit("remote-changes");
  },
});

const syncCommands: Record<string, (a: Args) => unknown> = {
  sync_status: syncStatus,
  sync_setup: syncSetup,
  sync_setup_app: syncSetupApp,
  sync_apps: () => MOCK_APPS,
  sync_find: syncFind,
  sync_join: syncJoin,
  projects_folder: () => MOCK_PROJECTS,
  sync_code: () => {
    if (mockSync.mode !== "encrypted" || !mockSync.code) throw "This device has no project code for this project.";
    return mockSync.code;
  },
  sync_enter_code: syncEnterCode,
  sync_stop: syncStop,
  sync_pull: syncPull,
  sync_notice_dismiss: () => void (mockSync.noticeDismissed = true),
  server_recent: () => ({ servers: mockServers, name: mockName }),
  server_share: serverShare,
  server_invite: serverInvite,
  ...serverCommands,
  presence_set: () => null,
  presence_get: presenceGet,
  authors_get: authorsGet,
  ...reviewCommands,
};

const info = ({ text: _text, ...rest }: StoredCheckpoint) => rest;

const opened = () => ({ path: project.path, name: project.name, text: doc.toString(), len16: doc.length });

// ---- Recent projects and Close project ----
//
// A seeded list with one project gone from disk (opening it fails and drops
// it). Opening another one shows the one in-memory document under its name;
// after Close project, a reload shows the start screen.

/** File → Close project was used (the next start shows the start screen). */
let closed = false;
let mockRecent = [
  { path: project.path, exists: true },
  { path: `${MOCK_PROJECTS}/Thesis.lethora`, exists: true },
  { path: "C:/Mock/Dropbox/Papers/Optics paper.lethora", exists: true },
  { path: "E:/Archive/Old novel.lethora", exists: false },
  { path: `${MOCK_PROJECTS}/Short stories & sketches.lethora`, exists: true },
];

const recentName = (path: string) => path.split(/[\\/]/).pop()!.replace(/\.lethora$/i, "");

/** The list without the open project, as `projects_recent` gives it. */
const recentProjects = () => mockRecent.filter((p) => closed || p.path !== project.path).map((p) => ({ name: recentName(p.path), ...p }));

/** A project was opened, created or joined: first in the list. */
function opening<T extends { path: string }>(o: T): T {
  closed = false;
  mockRecent = [{ path: o.path, exists: true }, ...mockRecent.filter((p) => p.path !== o.path)].slice(0, 10);
  return o;
}

// ---- Damaged projects (see damaged.ts) ----
//
// `?damaged`: the sample and Thesis have a damaged own log until opened at
// the last good state (so the start-up reopen asks too). `?skipped`: every
// open skips two other devices' data.

const DAMAGED_LOG = "main/6f1c0a9e5b2d4c7f8e3a1b0d9c8e7f6a.log";
const damagedPaths = new Set(new URLSearchParams(location.search).has("damaged") ? [project.path, `${MOCK_PROJECTS}/Thesis.lethora`] : []);
const MOCK_SKIPPED = new URLSearchParams(location.search).has("skipped")
  ? ["main: device 3b9e2f70c4a1d8e65f0b7a29c3d4e1f8: log at byte 2048: document model error: decode error", "review: device a07c5e1d92b84f36e0d1c7b5a4f3e2d1: snapshot: document model error: decode error"]
  : [];

const damagedReply = (path: string) => ({ damaged: { path, file: DAMAGED_LOG } });

/** `project_open` / `project_last`'s reply for the project just opened at `path`: what was recovered or skipped. */
function openedReply(path: string) {
  const recovered = damagedPaths.delete(path) ? [{ file: DAMAGED_LOG, dropped: 12_800, backup: `backup/damaged/${DAMAGED_LOG.replace("/", "-")}` }] : [];
  return { opened: { ...opening(opened()), recovered, skipped: MOCK_SKIPPED } };
}

function projectOpen(a: Args) {
  const path = String(a.path);
  const entry = mockRecent.find((p) => p.path === path);
  if (entry && !entry.exists) {
    mockRecent = mockRecent.filter((p) => p !== entry);
    throw `Not found: ${path} (moved, renamed or deleted?). It is no longer listed under recent projects.`;
  }
  if (damagedPaths.has(path) && a.recover !== true) return damagedReply(path);
  if (entry && path !== project.path) project = { path, name: recentName(path) };
  return openedReply(path);
}

/** The window buttons: nothing happens in the browser, the maximize button only switches its icon. */
let maximized = false;
const windowCommands: Record<string, (a: Args) => unknown> = {
  "plugin:window|minimize": () => null,
  "plugin:window|toggle_maximize": () => {
    maximized = !maximized;
    void emit("tauri://resize", { width: innerWidth, height: innerHeight });
    return null;
  },
  "plugin:window|is_maximized": () => maximized,
  "plugin:window|close": () => null,
  // No Snap Layouts overlay in the browser.
  snap_rect: () => null,
};

type Args = Record<string, unknown>;

// ---- Updates and app settings ----

/** Help → Check for updates… finds a fake update (`?update=none`: up to date, `?update=fail`: an error); the quiet check after start finds nothing, as in a dev build. */
const UPDATE = new URLSearchParams(location.search).get("update");
/** settings.rs's defaults; `null` restores one. The theme is only stored (the preview follows the system's). */
const SETTINGS_DEFAULTS: Args = {
  reopen_last_project: true,
  theme: "system",
  editor_text_size: 19.5,
  editor_line_spacing: 1.65,
  editor_paragraph_gap: 0.8,
  spell_check: true,
  check_updates_on_start: true,
  shortcuts: {},
};
let mockSettings: Args = { ...SETTINGS_DEFAULTS };

const updateCommands: Record<string, (a: Args) => unknown> = {
  settings_get: () => mockSettings,
  settings_set: (a) => {
    for (const [key, value] of Object.entries(a.changes as Args)) mockSettings[key] = value === null ? SETTINGS_DEFAULTS[key] : value;
    return { ...mockSettings };
  },
  update_check: (a) => {
    if (!a.manual || UPDATE === "none") return null;
    if (UPDATE === "fail") throw "Could not check for updates: Could not fetch a valid release JSON from the remote";
    return {
      version: "0.2.0",
      notes: ["### Added", "", "- Check for updates from the Help menu.", "- <b>Not bold</b>: notes are plain text.", "", "### Fixed", "", ...Array.from({ length: 20 }, (_, i) => `- Fix number ${i + 1}.`)].join("\n"),
      date: "2026-11-02",
    };
  },
  // Downloads for two seconds, then fails: the browser preview can't restart.
  update_install: async () => {
    const total = 12 * 1024 * 1024;
    for (let i = 0; i <= 20; i++) {
      void emit("update-progress", { downloaded: (total * i) / 20, total });
      await new Promise((r) => setTimeout(r, 100));
    }
    throw "Could not install the update: the browser preview can't install updates.";
  },
};

const commands: Record<string, (args: Args) => unknown> = {
  project_last: () => {
    if (closed) return null;
    if (!damagedPaths.has(project.path)) return openedReply(project.path);
    // The start screen asks, with the project in the recent list.
    closed = true;
    return damagedReply(project.path);
  },
  project_open: projectOpen,
  project_new: (a) => opening(projectNew(a)),
  project_close: () => void (closed = true),
  projects_recent: recentProjects,
  projects_recent_remove: (a) => {
    mockRecent = mockRecent.filter((p) => p.path !== a.path);
    return recentProjects();
  },
  projects_recent_clear: () => {
    mockRecent = mockRecent.filter((p) => !closed && p.path === project.path);
    return recentProjects();
  },
  text: () => doc.toString(),
  text_hash: () => textHash(doc),
  edit: (a) => edit(a.changes as TextChange[], a.split as boolean),
  undo: () => step(undoStack, redoStack),
  redo: () => step(redoStack, undoStack),
  save_now: () => null,
  checkpoints: () => checkpoints.map(info),
  checkpoint_create: (a) => {
    const c = { id: checkpoints.length + 1, name: (a.name as string | null) ?? null, auto: false, created: now(), text: doc.toString() };
    checkpoints.push(c);
    return info(c);
  },
  checkpoint_text: (a) => checkpoint(a.id as number).text,
  checkpoint_restore: (a) => replaceAll(checkpoint(a.id as number).text),
  render_math: renderMath,
  bib_entries: () => structuredClone({ entries: bib, errors: bibErrors }),
  bib_styles: () => STYLES,
  bib_format: formatEntry,
  bib_import: (a) => importBib(a.text as string),
  bib_import_file: () => importBib(ZOTERO_EXPORT),
  asset_import: importAsset,
  asset_import_bytes: importBytes,
  asset_stage: stageAsset,
  asset_stage_bytes: stageBytes,
  figure_insert: figureInsert,
  import_formats: () => [
    { name: "All supported files", extensions: ["md", "markdown", "docx", "docm", "dotx", "dotm", "txt", "text"] },
    { name: "Markdown", extensions: ["md", "markdown"] },
    { name: "Word", extensions: ["docx", "docm", "dotx", "dotm"] },
    { name: "Plain text", extensions: ["txt", "text"] },
  ],
  import_read: importRead,
  asset_replace: replaceAsset,
  asset_bytes: assetBytes,
  asset_list: assetList,
  asset_move: moveAsset,
  asset_remove: removeAsset,
  asset_folder_create: createFolder,
  asset_update: updateAsset,
  // Nothing is written in the browser preview.
  asset_save: () => null,
  "plugin:dialog|open": dialogOpen,
  "plugin:dialog|save": dialogSave,
  ...spellCommands,
  ...layoutCommands,
  ...overrideCommands,
  ...syncCommands,
  sync_join: (a) => opening(syncJoin(a)),
  server_join: async (a) => opening(await serverJoin(a)),
  ...windowCommands,
  ...updateCommands,
  page_jump: pageJump,
  compile_watch: compileWatch,
  compile_now: compileNow,
  doc_version: () => version,
  app_info: () => ({ version: "0.1.0", git: "browser", protocol: 2, level: 1, os: "browser preview", arch: "", server: mockSync.mode === "server" ? "0.1.0" : undefined }),
  page_svg: pageSvg,
  export_pdf: exportPdf,
  export_formats: () => [
    { id: "docx", name: "Word", extensions: ["docx"] },
    { id: "epub", name: "EPUB", extensions: ["epub"] },
    { id: "markdown", name: "Markdown", extensions: ["md", "markdown"] },
    { id: "text", name: "Plain text", extensions: ["txt", "text"] },
    { id: "html", name: "HTML", extensions: ["html", "htm"] },
  ],
  export_file: exportFile,
};

mockWindows("main");
mockIPC(
  async (cmd, payload) => {
    await new Promise((r) => setTimeout(r, 8)); // a little latency, like real IPC
    const handler = commands[cmd];
    if (!handler && cmd.startsWith("plugin:dialog|")) throw "File dialogs are not available in the browser preview.";
    if (!handler) throw `Unknown command in the browser preview: ${cmd}`;
    return handler((payload ?? {}) as Args);
  },
  { shouldMockEvents: true },
);

// `mockIPC` drops invoke's options: a raw-bytes command (`asset_import_bytes`,
// `asset_stage_bytes`) gets its body and headers as `{ body, headers }`.
const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke(cmd: string, args?: unknown, options?: { headers?: HeadersInit }): Promise<unknown> } }).__TAURI_INTERNALS__;
const mockedInvoke = internals.invoke;
internals.invoke = (cmd, args, options) =>
  mockedInvoke(cmd, args instanceof Uint8Array || args instanceof ArrayBuffer ? { body: new Uint8Array(args), headers: new Headers(options?.headers) } : args, options);

console.info("Lethora: browser preview with an in-memory mock backend (nothing is saved).");
