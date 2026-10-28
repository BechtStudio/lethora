// App shell: CodeMirror setup, keys, dialogs, and wiring the edit queue to Tauri.

import "./style.css";
import { Compartment, EditorSelection, EditorState, type Extension, type TransactionSpec } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { defaultKeymap, selectAll } from "@codemirror/commands";
import { openSearchPanel, search, searchKeymap } from "@codemirror/search";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open, save } from "@tauri-apps/plugin-dialog";
import { fromCore, Sync, type Backend, type Reply, type TextChange } from "./sync.ts";
import { snapLayouts, TopBar, type ViewMode, WindowButtons } from "./chrome.ts";
import { CheckpointPanel, type CheckpointInfo } from "./checkpoints.ts";
import { focusMode, typewriterMode } from "./focus.ts";
import { mathRendering, svgDataUrl, toHex, type MathReply } from "./math.ts";
import { equationLabel, insertMath, Palette } from "./palette.ts";
import { entriesByKey, insertCitation, labelsFor, refCompletion, refScience, type RefHost } from "./refs.ts";
import { type AssetBytes, type FigureLayout, figureLabels, figurePreview, figureText, IMAGE_EXTENSIONS, insertFigure, labelStem, placementExceptions, uniqueLabel } from "./figures.ts";
import type { BibLibrary, ImportReport, StyleInfo } from "./bib.ts";
import { Library, SourcesPanel } from "./sources.ts";
import { typstMarkup } from "./typst.ts";
import { breakKeys } from "./breaks.ts";
import { activeFormats, type InlineFormat, paragraphKind, type ParagraphKind, restyle, setHeading, toggleInline, toggleList } from "./format.ts";
import { ComboBox, cssFamily, FontFaces, type FontFile, FontPicker } from "./fontpicker.ts";
import { type Creations, parseSize, type Settings, settingsOf, SIZES, sizeLabel, styleAt, styleChanges, styledText, styleEntries, styleFor, styleLabel, styleUses, type TextStyles, textStylesOf } from "./styles.ts";
import { insertTable, nextTableLabel, type TableAction, tableAt, tableCommand, tableKeys, tableMenu, TablePicker } from "./tables.ts";
import { type MenuHandlers, type MenuItem, PageMenu } from "./pagemenu.ts";
import { writingTheme } from "./theme.ts";
import { countWords } from "./words.ts";
import { spellChecker, type Word } from "./spell.ts";
import { Publisher } from "./publisher.ts";
import { type LayoutState, type PresetInfo, valueOf } from "./layoutpanel.ts";
import type { PageBytes } from "./pages.ts";
import type { JumpResult, OverrideInfo } from "./exceptions.ts";
import { type CompileStatus, undoMessage } from "./status.ts";
import { NewProjectDialog } from "./newproject.ts";
import { fileName, fileStem, type ImportFilter, importNotice, importPos, type ImportReply, importText, type ImportWarning } from "./imports.ts";
import { baseName, type ExportFilter, exportFilters, exportNotice, exportTarget, PDF_FILTER } from "./exports.ts";
import { AboutDialog, type AppInfo } from "./about.ts";
import { UPDATE_CHECK_DELAY_MS, UpdateDialog, type UpdateInfo, type UpdateProgress } from "./updates.ts";
import { type AppSettings, SettingsView, shortcutSections } from "./settings.ts";
import { clampFraction, fractionAt, paneWidth, parseFraction, SPLIT_STEP } from "./split.ts";
import type { AssetList } from "./assets.ts";
import { AssetsPanel } from "./assetspanel.ts";
import { boundaryAt, type Dropped, figureDrops, skippedMessage, supportedFiles } from "./drop.ts";
import { Poller, readOnlyRole, reviewReadOnly as reviewReadOnlyOf, type FindReply, type MembersReply, type PullReply, type RememberedProject, type Role, type SyncApp, type SyncStatus } from "./pull.ts";
import { SyncCloud, SyncDialog, SyncNotice } from "./syncdialog.ts";
import { type Damaged, DamagedDialog, OpenNotice, type Recovered } from "./damaged.ts";
import { WriteGuard } from "./readonly.ts";
import { applyShortcuts, type Command, isEnabled, keyAction } from "./commands.ts";
import { MenuBar, type MenuDef } from "./menus.ts";
import { MacMenu, nativeMac, systemKey } from "./macmenu.ts";
import { RecentList, type RecentProject, recentItems } from "./recent.ts";
import { startupDone, startupProgress } from "./startup.ts";
import { caretsOf, onlineUsers, type PeerPresence, PresenceSender, remoteCarets, setRemoteCarets } from "./presence.ts";
import { AUTHORS_DELAY_MS, authorMarkField, authorMarks, type Authors, legendOf, setAuthorMarks, showLegend } from "./authors.ts";
import { COLOUR_LABELS, historyTarget, MARK_COLOURS, type MarkColour, type ReviewAccess, reviewField, type ReviewList, type SuggestCall, type Suggesting, suggestionsOf, suggestMode } from "./review.ts";
import { type ReviewApi, ReviewPanel } from "./reviewpanel.ts";

// Browser preview for UI work: a mock backend, never part of a production build.
if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) await import("./devmock.ts");

interface Opened {
  path: string;
  name: string;
  text: string;
  len16: number;
  /** Own logs this open cut back (see damaged.ts). */
  recovered?: Recovered[];
  /** Other devices' data this open skipped. */
  skipped?: string[];
}

/** `project_open` / `project_last`: opened, or an own log is damaged (see damaged.ts). */
type Opening = { opened: Opened } | { damaged: Damaged };

const backend: Backend = {
  edit: (changes: TextChange[], split: boolean) => invoke<number>("edit", { changes, split }),
  // Nothing to undo while read only.
  undo: () => (readOnly ? Promise.resolve(null) : invoke<Reply | null>("undo").then((r) => afterUndo("undo", r))),
  redo: () => (readOnly ? Promise.resolve(null) : invoke<Reply | null>("redo").then((r) => afterUndo("redo", r))),
  restore: (id: number) => invoke<Reply>("checkpoint_restore", { id }).then(afterRestore),
  text: () => invoke<string>("text"),
  hash: () => invoke<number>("text_hash"),
};

const editorEl = document.getElementById("editor")!;
const emptyEl = document.getElementById("empty")!;
const publisherEl = document.getElementById("publisher")!;

const bar = new TopBar();
const showError = (e: unknown) => bar.error(e);

// Own window buttons on Windows and Linux (no native title bar there); macOS keeps its native ones.
if (!navigator.userAgent.includes("Macintosh")) {
  const buttons = new WindowButtons(document.getElementById("window-buttons")!, getCurrentWindow(), showError);
  // Windows 11 shows Snap Layouts on the maximize button (a native overlay over it, see snap.rs).
  if (navigator.userAgent.includes("Windows")) {
    snapLayouts(buttons.max, (r) => invoke("snap_rect", r), (h) => listen<string>("snap-maximize", (e) => h(e.payload)), showError);
  }
}

// macOS draws its traffic lights over the row's start (tauri.macos.conf.json); in native full screen there are none.
// Full screen is the window filling its screen (asking the window lags: its state flips after the last resize on exit).
if (nativeMac) {
  const root = document.documentElement;
  root.classList.add("mac");
  const fullscreen = () => root.classList.toggle("fullscreen", innerWidth >= screen.width && innerHeight >= screen.height);
  window.addEventListener("resize", fullscreen);
  fullscreen();
}

let sync: Sync | null = null;
let projectName = "";
/** The role that makes the open project's text and settings read only here (see `readOnlyRole`), or null. */
let readOnly: Role | null = null;
/** The review is read only here too (readers, see `reviewReadOnly`). */
let reviewReadOnly = false;
/** Edit → Suggest changes, an Owner's or Editor's choice (see `suggesting`). */
let suggestOn = false;
/** Suggest mode: edits in the text become suggestions. Always for a reviewer, never for a reader. */
const suggesting = () => readOnly === "reviewer" || (readOnly === null && suggestOn);
/** The last sync status (for File → Members…, and whether presence is on). */
let lastStatus: SyncStatus | null = null;
/** Sends this device's view and selection to the core (see `publishPresence`). */
const presence = new PresenceSender((here) => invoke<void>("presence_set", { ...here }));
/** Who wrote the visible text (View → Show authors, see `refreshAuthors`): its legend and the pending request. */
const authorsLegend = document.getElementById("authors-legend")!;
let authorsTimer: ReturnType<typeof setTimeout> | undefined;

// ---- Preferences (optional: the app works without localStorage) ----

function loadFlag(key: string, fallback = false): boolean {
  try {
    const v = localStorage.getItem(key);
    return v === null ? fallback : v === "1";
  } catch {
    return fallback;
  }
}

function saveFlag(key: string, on: boolean): void {
  try {
    localStorage.setItem(key, on ? "1" : "0");
  } catch {
    // not persisted
  }
}

function loadView(): ViewMode {
  try {
    const v = localStorage.getItem("lethora.view");
    return v === "science" || v === "publisher" ? v : "author";
  } catch {
    return "author";
  }
}

let focusOn = loadFlag("lethora.focus");
let typewriterOn = loadFlag("lethora.typewriter");
/** View → Check spelling, settings.json's `spell_check` (see `applySettings`). */
let spellOn = true;
/** View → Show authors (server-shared projects only, see authors.ts). */
let authorsOn = loadFlag("lethora.authors");
/** View → Review: the Review panel and the highlights (author and science views, see reviewpanel.ts). */
let reviewOn = loadFlag("lethora.review");
let viewMode = loadView();
/** The author or science view last shown (where the publisher view goes back to for a text position). */
let textView: ViewMode = viewMode === "science" ? "science" : "author";
/** The page preview beside the editor (author and science views). */
let splitOn = loadFlag("lethora.split");
const focusSlot = new Compartment();
const typewriterSlot = new Compartment();
const spellSlot = new Compartment();
/** Extensions of the current view (author: none beyond the shared ones). */
const viewSlot = new Compartment();
/** Read only (reviewers and readers): nothing typed or inserted reaches the text; the core's changes (other devices') still do. */
const readOnlySlot = new Compartment();
// Still focusable (cursor, selection, keys such as Ctrl+F); edits are refused.
const readOnlyEditor: Extension = [EditorState.readOnly.of(true), EditorState.changeFilter.of((tr) => tr.annotation(fromCore) === true)];
/** Suggest mode (see `suggesting`): edits become suggest calls (`sendSuggestions`), the text stays. */
const suggestSlot = new Compartment();
const suggestEditor = suggestMode(() => review.current);

// ---- Editor ----

const undo = () => {
  historyStep("undo");
  return true;
};
const redo = () => {
  historyStep("redo");
  return true;
};

let wordTimer: ReturnType<typeof setTimeout> | undefined;
function scheduleWordCount(delay = 500): void {
  clearTimeout(wordTimer);
  wordTimer = setTimeout(() => bar.setWords(countWords(view.state.doc)), delay);
}

const writing: Extension = [EditorView.lineWrapping, writingTheme, typstMarkup];

/** The core's spell check (spell.ts), in the layout's `text.lang`. */
const spell = spellChecker((lang, words) => invoke<string[]>("spell_check", { lang, words }));

// ---- Formatting row (the commands are in the Format menu) ----

const formatBar = document.getElementById("format-bar")!;
/** "Table ▾": the table commands, shown while the cursor is in a table. */
const tableMenuButton = formatBar.querySelector<HTMLButtonElement>('button[data-command="table-menu"]')!;
const headingBox = document.getElementById("fmt-heading") as HTMLSelectElement;
/** The inline formats and paragraph kind at the selection (pressed buttons, menu checks). */
let formats = new Set<InlineFormat>();
let paragraph: ParagraphKind = { heading: 0, bullet: false, numbered: false };

/** The character styles and the body's font and size (styles.ts), null until the project's layout is read. */
let textStyles: TextStyles | null = null;
/** The editor's styled text (follows `textStyles`). */
const styledSlot = new Compartment();
/** The embedded fonts' faces, loaded for previews and styled text. */
const fontFaces = new FontFaces({
  embedded: () => invoke<string[]>("layout_embedded_fonts"),
  file: (family) => invoke<FontFile>("layout_font_file", { family }),
});

/** Runs a row action while formatting is possible (else the row shows the state again); errors go to the bar. */
function rowAction(action: () => Promise<void>): void {
  // Styles, fonts and sizes change the layout too (through the core): never suggestions.
  if (formatting() && writable()) action().catch(showError);
  else refreshFormatBar();
}

const fontPicker = new FontPicker(document.getElementById("fmt-font") as HTMLInputElement, {
  fonts: () => invoke<string[]>("layout_fonts"),
  faces: fontFaces,
  pick: (family) => rowAction(() => changeText({ font: family })),
  done: () => view.focus(),
});
const sizeBox = new ComboBox(
  document.getElementById("fmt-size") as HTMLInputElement,
  {
    items: () => SIZES.map((n) => ({ value: String(n), label: String(n) })),
    commit: (text) => {
      const size = parseSize(text);
      if (size) rowAction(() => changeText({ size }));
      else {
        bar.error(`“${text.trim()}” is not a font size (a number of points, like 11 or 10.5).`);
        refreshFormatBar();
      }
    },
    done: () => view.focus(),
  },
  { label: "font sizes", free: true },
);
/** The Style box's last entry: opens the layout panel's Character styles (no style id has a space). */
const EDIT_STYLES = "edit styles";
const styleBox = new ComboBox(
  document.getElementById("fmt-style") as HTMLInputElement,
  {
    items: () => [
      { value: "", label: "Default text" },
      ...(textStyles ? styleEntries(textStyles) : []).map((e) => ({ value: e.id, label: e.label, font: e.font && cssFamily(e.font) })),
      { value: EDIT_STYLES, label: "Edit styles…" },
    ],
    commit: (id) => {
      if (id !== EDIT_STYLES) rowAction(() => restyleText(() => () => id || null));
      else {
        refreshFormatBar();
        void setView("publisher").then(() => publisher.panel.revealStyles(), showError);
      }
    },
    done: () => view.focus(),
  },
  { label: "character styles", readOnly: true },
);

/** Shows the formats at the selection; every control is disabled while read only or without a project (a reviewer's buttons and headings make suggestions). */
function refreshFormatBar(): void {
  formats = activeFormats(view.state);
  paragraph = paragraphKind(view.state);
  const off = sync === null || readOnly !== null;
  const textOff = off && !(sync !== null && suggesting());
  for (const b of formatBar.querySelectorAll<HTMLButtonElement>("button[data-command]")) {
    const id = b.dataset.command!;
    // Toggles only (not Table).
    if (b.hasAttribute("aria-pressed")) b.setAttribute("aria-pressed", String(id === "bullets" ? paragraph.bullet : id === "numbering" ? paragraph.numbered : formats.has(id as InlineFormat)));
    b.disabled = textOff;
  }
  tableMenuButton.hidden = sync === null || tableAt(view.state) === null;
  // Mixed headings: no entry selected.
  headingBox.value = paragraph.heading === null ? "" : String(paragraph.heading);
  headingBox.disabled = textOff;
  // Mixed style, font or size: empty.
  const at = textStyles && sync ? styleAt(view.state, textStyles) : null;
  const styleFont = at?.id ? textStyles!.styles[at.id]?.font : undefined;
  styleBox.set(at ? styleLabel(textStyles!, at.id) : "", styleFont ? cssFamily(styleFont) : "");
  fontPicker.set(at?.font ?? "");
  sizeBox.set(at?.size ? sizeLabel(at.size) : "");
  styleBox.disabled = fontPicker.disabled = sizeBox.disabled = off;
}

/** Takes the styles and the body's font and size from layout state `s`: the editor's styled text and the row follow. */
function setStyles(s: LayoutState): void {
  textStyles = textStylesOf(s);
  spell.setLang(String(valueOf(s, "text.lang") ?? ""));
  fontFaces.load(Object.values(textStyles.styles).flatMap((style) => (style.font ? [style.font] : [])));
  view.dispatch({ effects: styledSlot.reconfigure(styledText(textStyles)) });
  refreshFormatBar();
}

/** Reads the styles anew (a project loaded, undo/redo, a restore, other devices' changes) while the layout panel isn't there to report them. */
function refreshStyles(): void {
  if (sync) invoke<LayoutState>("layout_get").then(setStyles, showError);
}

/** A layout state after the row's own change: the styles and the science view's figures follow, and the layout panel while it shows. */
function layoutChanged(s: LayoutState): void {
  figures.setLayout(figureLayout(s));
  setStyles(s);
  if (publisher.isActive) void publisher.panel.refresh();
}

/**
 * A font or size from the row: a cursor outside styled text sets the
 * body's (the whole document); else the selection, or the styled text the
 * cursor is in, gets a character style with it (see styles.ts).
 */
async function changeText(change: Settings): Promise<void> {
  const info = textStyles;
  if (!info) return;
  if (view.state.selection.main.empty && styleAt(view.state, info).id === null) {
    const [key, value] = change.font ? ["text.font", change.font] : ["text.size", change.size];
    if (value === (key === "text.font" ? info.body.font : info.body.size)) return;
    await drain();
    layoutChanged(await invoke<LayoutState>("layout_set", { changes: { [key]: value } }));
    return;
  }
  await restyleText((styles, creations) => (current) => styleFor({ ...settingsOf(styles, current), ...change }, styles, creations));
}

/**
 * Restyles the selection (`restyle`) with the style `pick` chooses for the
 * text's current one: the styles it needs are created first (an undo step
 * of their own), then the text is edited (another).
 */
async function restyleText(pick: (styles: TextStyles, creations: Creations) => (current: string | null) => string | null): Promise<void> {
  if (!textStyles) return;
  const creations: Creations = new Map();
  restyle(view.state, pick(textStyles, creations));
  if (creations.size) {
    await drain();
    layoutChanged(await invoke<LayoutState>("layout_set", { changes: styleChanges(creations) }));
  }
  const missing: Creations = new Map();
  const spec = restyle(view.state, pick(textStyles, missing));
  // The styles changed meanwhile (another device): the text stays.
  if (missing.size) return;
  view.dispatch(spec);
}

// ---- Sources, references and figures ----

const library = new Library({
  entries: () => invoke<BibLibrary>("bib_entries"),
  styles: () => invoke<StyleInfo[]>("bib_styles"),
  format: (key, style) => invoke<string>("bib_format", { key, style }),
  importText: (text) => invoke<ImportReport>("bib_import", { text }),
  importFile: async () => {
    const path = await open({ title: "Import a .bib file", multiple: false, directory: false, filters: [{ name: "BibTeX", extensions: ["bib"] }] });
    return typeof path === "string" ? invoke<ImportReport>("bib_import_file", { path }) : null;
  },
});

/** An equation for a hover card, rendered like the science view's formulas (the core caches renders). */
async function renderEquation(source: string, display: boolean) {
  const px = parseFloat(getComputedStyle(view.contentDOM).fontSize) || 19.5;
  const color = toHex(getComputedStyle(document.documentElement).getPropertyValue("--ink")) ?? "#000000";
  const r = await invoke<MathReply>("render_math", { source, display, sizePt: Math.round(px * 75) / 100, color });
  return "ok" in r ? { url: svgDataUrl(r.ok.svg), width: (r.ok.width * 4) / 3, height: (r.ok.height * 4) / 3 } : null;
}

const refHost: RefHost = {
  entries: () => library.entries,
  loaded: () => library.loaded,
  format: (key) => library.format(key),
  renderEquation,
  subscribe: (listener) => library.subscribe(listener),
};

const figures = figurePreview((path) => invoke<AssetBytes>("asset_bytes", { path }));

/** What the science view sizes figures against. */
const figureLayout = (s: LayoutState): FigureLayout => ({ ...s.text_area, default_width: Number(valueOf(s, "figures.default_width")) || 100 });

/** Reloads the layout and the exceptions the science view's figures show (the publisher view keeps them current while shown). */
function refreshFigures(): void {
  if (!sync || viewMode !== "science") return;
  invoke<LayoutState>("layout_get").then((s) => figures.setLayout(figureLayout(s)), showError);
  drain().then(() => publisher.overrides.refresh()).catch(showError);
}

const science: Extension = [
  mathRendering((r) => invoke<MathReply>("render_math", { source: r.source, display: r.display, sizePt: r.sizePt, color: r.color })),
  refScience(refHost),
  figures.extension,
];
const viewExtensions = (mode: ViewMode): Extension => (mode === "science" ? science : []);

function extensions(): Extension[] {
  return [
    writing,
    refCompletion(() => library.entries),
    // Tab / Shift+Tab between table cells (below the completion's Tab).
    tableKeys,
    // Our own spell check (`spellSlot`), not the web view's.
    EditorView.contentAttributes.of({ spellcheck: "false", "aria-label": "Manuscript" }),
    focusSlot.of(focusOn ? focusMode : []),
    typewriterSlot.of(typewriterOn ? typewriterMode : []),
    spellSlot.of(spellOn ? spell.extension : []),
    viewSlot.of(viewExtensions(viewMode)),
    styledSlot.of(textStyles ? styledText(textStyles) : []),
    readOnlySlot.of(readOnly && !suggesting() ? readOnlyEditor : []),
    suggestSlot.of(suggesting() ? suggestEditor : []),
    breakKeys,
    // Find and replace: its replacements are ordinary transactions (sent to the core like typing).
    search({ top: true }),
    keymap.of(searchKeymap.filter((b) => b.key === "Escape")),
    figureDrops((dropped, boundary) => void dropFigures(dropped, boundary).catch(showError)),
    remoteCarets,
    authorMarkField,
    reviewField,
    keymap.of([{ key: "Mod-z", run: undo }, { key: "Mod-y", run: redo }, { key: "Mod-Shift-z", run: redo }, ...defaultKeymap]),
    // The WebView's own undo (e.g. context menu) goes through the core too.
    EditorView.domEventHandlers({
      beforeinput(event) {
        if (event.inputType === "historyUndo") undo();
        else if (event.inputType === "historyRedo") redo();
        else return false;
        event.preventDefault();
        return true;
      },
    }),
    EditorView.updateListener.of((update) => {
      for (const tr of update.transactions) {
        sync?.observe(tr);
        const suggested = suggestionsOf(tr);
        if (suggested) sendSuggestions(suggested);
        // A caret move ends the review undo step, as typing's.
        else if (tr.selection && !tr.docChanged && tr.isUserEvent("select")) splitSuggestion = true;
      }
      if (update.docChanged) {
        for (const c of unsent) {
          c.from = update.changes.mapPos(c.from, 1);
          c.to = Math.max(c.from, update.changes.mapPos(c.to, -1));
        }
      }
      if (update.docChanged) {
        scheduleWordCount();
        assets.changed();
        if (publisher.isActive) publisher.panel.textChanged();
      }
      if (update.docChanged || update.selectionSet || update.startState.readOnly !== update.state.readOnly) refreshFormatBar();
      if (update.docChanged || update.selectionSet) publishPresence();
      if (update.docChanged || update.viewportChanged) requestAuthors();
      review.update(update);
    }),
  ];
}

const view = new EditorView({ parent: editorEl, state: EditorState.create({ extensions: extensions() }) });
// Our own right-click menu in the text (see `openEditorMenu`).
view.dom.addEventListener("contextmenu", (e) => openEditorMenu(e));

// ---- Review: comments and highlights (see reviewpanel.ts) ----

/** What this device's role lets it change (see review.ts). */
const access = (): ReviewAccess => ({ main: readOnly === null, review: !reviewReadOnly });

/** A review request through the edit queue: pending edits are sent first, so offsets map exactly (see `Sync.call`). */
function reviewCall<T>(request: (current: Sync) => Promise<T>): Promise<T | null> {
  const current = sync;
  return current ? current.call(() => request(current)) : Promise.resolve(null);
}

const reviewApi: ReviewApi = {
  list: () =>
    reviewCall(async (current) => {
      const list = await invoke<ReviewList>("review_list");
      return { ...list, items: list.items.map((i) => ({ ...i, from: current.toEditor(i.from), to: current.toEditor(i.to) })) };
    }),
  people: async () => {
    if (lastStatus?.mode !== "server") return new Map();
    const reply = await invoke<MembersReply>("server_members").catch(() => null);
    return new Map((reply?.members ?? []).map((m) => [m.user, { name: m.name, colour: m.colour }]));
  },
  comment: (range, text) =>
    reviewCall((current) => {
      const r = range();
      return r ? invoke<number>("review_comment", { from: current.toCore(r.from), to: current.toCore(r.to), text }) : Promise.resolve(null);
    }),
  mark: (range, colour) =>
    reviewCall((current) => {
      const r = range();
      return r && r.from < r.to ? invoke<number>("review_mark", { from: current.toCore(r.from), to: current.toCore(r.to), colour }) : Promise.resolve(null);
    }),
  reply: (item, text) => reviewCall(() => invoke<number>("review_reply", { item, text })),
  edit: (item, message, text) => reviewCall(() => invoke<void>("review_edit", { item, message, text })),
  deleteMessage: (item, message) => reviewCall(() => invoke<void>("review_delete", { item, message })),
  resolve: (item, resolved) => reviewCall(() => invoke<void>("review_resolve", { item, resolved })),
  colour: (item, colour) => reviewCall(() => invoke<void>("review_colour", { item, colour })),
  remove: (item) => reviewCall(() => invoke<void>("review_remove", { item })),
  // A `main` edit: applied to the editor like undo's reply (and typeset again).
  accept: (item) => sync?.apply(() => invoke<Reply>("review_accept", { item })) ?? Promise.resolve(null),
  reject: (item) => reviewCall(() => invoke<void>("review_reject", { item })),
};

const reviewEl = document.getElementById("review")!;
const review = new ReviewPanel(reviewEl, { view, api: reviewApi, close: () => setReview(false), error: (e) => showError(e) });

/** Shows the Review panel while switched on, in the author and science views of an open project. */
function showReview(): void {
  const shown = reviewOn && sync !== null && viewMode !== "publisher";
  document.body.toggleAttribute("data-review", shown);
  review.setShown(shown);
}

function setReview(on: boolean): void {
  reviewOn = on;
  saveFlag("lethora.review", on);
  bar.setToggle("review", on);
  if (!on && review.contains(document.activeElement)) view.focus();
  showReview();
}

/** Whether review items can be added here: the author or science view, a role that writes the review, no checkpoint preview over it. */
const canReview = () => inText() && !reviewReadOnly && !editorEl.classList.contains("behind-preview");
const canAddReview = () => canReview() && !view.state.selection.main.empty;

/** Edit → New comment (Ctrl+Alt+M): a new card on the selection (the panel opens). */
function newComment(): void {
  if (!canAddReview()) return;
  if (!reviewOn) setReview(true);
  review.newComment();
}

/** Edit → Highlight: the selection in `colour` (the panel opens: highlights show only with it). */
function addMark(colour: MarkColour): void {
  if (!canAddReview()) return;
  if (!reviewOn) setReview(true);
  review.addMark(colour);
}

/** Review undo or redo (through the edit queue); the panel lists the items again. */
async function reviewHistory(kind: "undo" | "redo"): Promise<void> {
  if (!sync || reviewReadOnly) return;
  const done = await reviewCall(() => invoke<boolean>(kind === "undo" ? "review_undo" : "review_redo"));
  if (!done) bar.info(kind === "undo" ? "Nothing to undo in the review." : "Nothing to redo in the review.");
  else if (!review.isShown) bar.info(kind === "undo" ? "Undid a review change." : "Redid a review change.");
  await review.refresh();
}

/** Ctrl+Z / Ctrl+Y outside text fields, from `target`: the review's history or the document's (see `historyTarget`). */
function historyStep(kind: "undo" | "redo", target: EventTarget | null = document.activeElement): void {
  if (historyTarget({ field: false, panel: review.contains(target), suggest: suggesting() }, access()) === "review") void reviewHistory(kind).catch(showError);
  else void (kind === "undo" ? sync?.undo() : sync?.redo());
}

// ---- Suggest mode (Edit → Suggest changes; see `suggestMode`) ----

const suggestBadge = document.getElementById("suggesting")!;
/** Suggest calls not sent yet, in the editor's current text (the core's changes meanwhile move them). */
const unsent = new Set<SuggestCall>();
/** The next suggestion starts a review undo step of its own (the caret moved, or after a paste or cut). */
let splitSuggestion = true;

/** Sends an edit's suggest calls in order through the edit queue; the panel shows them at once and lists the items again once all are answered. */
function sendSuggestions(s: Suggesting): void {
  const current = sync;
  if (!current) return;
  // Suggestions show only with the panel.
  if (!reviewOn) setReview(true);
  review.suggested(s);
  let split = splitSuggestion || s.split;
  splitSuggestion = s.split;
  for (const c of s.calls) {
    const call = { ...c };
    const first = split;
    split = false;
    unsent.add(call);
    current
      .call(() => {
        unsent.delete(call);
        return invoke<number | null>("review_suggest", { from: current.toCore(call.from), to: current.toCore(call.to), insert: call.insert, split: first });
      })
      .catch(showError)
      .finally(() => {
        unsent.delete(call);
        review.suggestSettled();
      });
  }
}

/** The editor follows the role and suggest mode: read only (readers), edits as suggestions, or plain edits; the title bar shows "Suggesting". */
function applyEditing(): void {
  const on = suggesting();
  view.dispatch({ effects: [readOnlySlot.reconfigure(readOnly && !on ? readOnlyEditor : []), suggestSlot.reconfigure(on ? suggestEditor : [])] });
  // A reviewer's badge says it already (and the bar has no room for both).
  suggestBadge.hidden = !on || readOnly === "reviewer";
  splitSuggestion = true;
  refreshFormatBar();
}

/** Edit → Suggest changes (Owners and Editors; a reviewer always suggests, a reader never): turning it on opens the Review panel. */
function setSuggest(on: boolean): void {
  if (readOnly !== null) return;
  suggestOn = on;
  applyEditing();
  if (on && !reviewOn) setReview(true);
}
startupProgress(30, "Preparing the editor…");

function setFocusMode(on: boolean): void {
  focusOn = on;
  saveFlag("lethora.focus", on);
  view.dispatch({ effects: focusSlot.reconfigure(on ? focusMode : []) });
  // The row hides its tools.
  document.body.toggleAttribute("data-focus", on);
}

function setTypewriter(on: boolean): void {
  typewriterOn = on;
  saveFlag("lethora.typewriter", on);
  view.dispatch({
    effects: [
      typewriterSlot.reconfigure(on ? typewriterMode : []),
      ...(on ? [EditorView.scrollIntoView(view.state.selection.main.head, { y: "center" })] : []),
    ],
  });
}

/**
 * Switches views; only the view's extensions change (never the text or the
 * edit queue). The publisher view is entered with every edit sent.
 */
async function setView(mode: ViewMode): Promise<void> {
  if (mode === "publisher" && viewMode !== "publisher" && sync) await sync.drain();
  if (mode !== "publisher") textView = mode;
  if (mode !== viewMode) {
    viewMode = mode;
    try {
      localStorage.setItem("lethora.view", mode);
    } catch {
      // not persisted
    }
    view.dispatch({ effects: viewSlot.reconfigure(viewExtensions(mode)) });
    refreshFigures();
  }
  if (mode !== "science") {
    palette.set(false);
    sources.close();
  }
  document.body.dataset.mode = mode;
  bar.setView(mode);
  showView();
  publishPresence();
  requestAuthors(0);
}

/** Shows the editor or the publisher view for the current project and view, and the split pane. */
function showView(): void {
  const pub = sync !== null && viewMode === "publisher";
  const split = sync !== null && !pub && splitOn;
  editorEl.hidden = sync === null || pub;
  // First the layout, so that the pages are laid out for the pane's width.
  document.body.toggleAttribute("data-split", split);
  publisher.setActive(pub ? "tab" : split ? "split" : null);
  showReview();
}

/** F6 / Pages: the page preview beside the editor; opened with every edit sent. */
async function setSplit(on: boolean): Promise<void> {
  if (on && sync && !publisher.isActive) await sync.drain();
  splitOn = on;
  saveFlag("lethora.split", on);
  bar.setToggle("pages", on);
  if (!on && publisherEl.contains(document.activeElement)) view.focus();
  showView();
}

// ---- Split pane width (a fraction of the window, see split.ts) ----

const splitHandle = document.getElementById("split-handle")!;

function loadSplitFraction(): number {
  try {
    return parseFraction(localStorage.getItem("lethora.splitWidth"));
  } catch {
    return 0.5;
  }
}

let splitFraction = loadSplitFraction();

/** Sizes the pane (and the editor beside it) for the window's width. */
function applySplitWidth(): void {
  const total = document.documentElement.clientWidth;
  const width = paneWidth(splitFraction, total);
  document.body.style.setProperty("--split-w", `${width}px`);
  splitHandle.setAttribute("aria-valuenow", String(total > 0 ? Math.round(100 - (width / total) * 100) : 50));
}

function setSplitFraction(fraction: number, persist: boolean): void {
  splitFraction = clampFraction(fraction, document.documentElement.clientWidth);
  applySplitWidth();
  if (!persist) return;
  try {
    localStorage.setItem("lethora.splitWidth", String(splitFraction));
  } catch {
    // not persisted
  }
}

applySplitWidth();
window.addEventListener("resize", applySplitWidth);
splitHandle.addEventListener("pointerdown", (e) => {
  if (e.button !== 0) return;
  e.preventDefault();
  splitHandle.setPointerCapture(e.pointerId);
  document.body.classList.add("split-dragging");
});
splitHandle.addEventListener("pointermove", (e) => {
  if (splitHandle.hasPointerCapture(e.pointerId)) setSplitFraction(fractionAt(e.clientX, document.documentElement.clientWidth), false);
});
const endDrag = (e: PointerEvent) => {
  if (!document.body.classList.contains("split-dragging")) return;
  document.body.classList.remove("split-dragging");
  setSplitFraction(splitFraction, true);
  if (splitHandle.hasPointerCapture(e.pointerId)) splitHandle.releasePointerCapture(e.pointerId);
};
splitHandle.addEventListener("pointerup", endDrag);
splitHandle.addEventListener("pointercancel", endDrag);
// Left/Right move the handle (Left widens the pane).
splitHandle.addEventListener("keydown", (e) => {
  if ((e.key !== "ArrowLeft" && e.key !== "ArrowRight") || e.ctrlKey || e.metaKey || e.altKey || e.shiftKey) return;
  e.preventDefault();
  setSplitFraction(splitFraction + (e.key === "ArrowLeft" ? SPLIT_STEP : -SPLIT_STEP), true);
});

/** The status after undo/redo; the layout panel and sources follow non-text steps. */
function afterUndo(kind: "undo" | "redo", reply: Reply | null): Reply | null {
  const { message, layout } = undoMessage(kind, reply, viewMode === "publisher");
  if (message) bar.info(message);
  if (layout && publisher.isActive) publisher.refreshLayout();
  else if (layout) {
    refreshFigures();
    refreshStyles();
  }
  if (layout) library.refresh().catch(showError);
  // The step may have imported or replaced an image.
  if (layout) figures.reload();
  if (layout) assets.reload();
  return reply;
}

/** A restore may change the layout, the exceptions and the sources too: reload them. */
function afterRestore(reply: Reply): Reply {
  if (publisher.isActive) publisher.refreshLayout();
  else {
    refreshFigures();
    refreshStyles();
  }
  library.refresh().catch(showError);
  figures.reload();
  assets.reload();
  return reply;
}

/**
 * Selects a range of the text (from a diagnostic or a page); the Publisher
 * tab switches to the science view, the split pane keeps the current view.
 */
async function revealText(from: number, to: number, outdated: boolean): Promise<void> {
  if (viewMode === "publisher") await setView("science");
  view.dispatch({ selection: { anchor: from, head: to }, effects: EditorView.scrollIntoView(from, { y: "center" }), userEvent: "select" });
  view.focus();
  if (outdated) bar.info("This problem may be outdated: the text changed since it was typeset.");
}

const zoomFitButton = document.getElementById("zoom-fit")!;
const paneZoomFitButton = document.getElementById("pane-zoom-fit")!;
const paneStatus = document.getElementById("pane-status")!;

/** The exception markers and the Exceptions drawer, as the publisher reports them (for the View menu). */
let markersShown = true;
let drawerOpen = false;

const drain = async () => {
  await sync?.drain();
};

const publisher = new Publisher(
  {
    watch: (active) => invoke<void>("compile_watch", { active }),
    compileNow: () => invoke<void>("compile_now", { force: false }),
    pageSvg: (version, index) => invoke<PageBytes>("page_svg", { version, index }),
    docVersion: () => invoke<number>("doc_version"),
    drain,
    docLength: () => view.state.doc.length,
    lineOf: (utf16) => view.state.doc.lineAt(Math.min(utf16, view.state.doc.length)).number,
    lineAt: (utf16) => {
      const line = view.state.doc.lineAt(Math.max(0, Math.min(utf16, view.state.doc.length)));
      return { from: line.from, text: line.text };
    },
    pageJump: (version, page, xPt, yPt) => invoke<JumpResult>("page_jump", { version, page, xPt, yPt }),
    overrides: {
      list: () => invoke<OverrideInfo[]>("overrides_get"),
      add: (kind, params, posUtf16) => invoke<string>("override_add", { kind, params, posUtf16 }),
      remove: (id) => invoke<void>("override_remove", { id }),
      update: (id, params) => invoke<void>("override_update", { id, params }),
      batch: (ops) => invoke<void>("overrides_batch", { ops }),
      drain,
    },
    replaceImage: (path, pos) => replaceImage(path, pos),
    revealText: (from, to, outdated) => void revealText(from, to, outdated).catch(showError),
    // The bar shows it in the Publisher tab, the pane's head in the split pane.
    setStatus: (text, tone) => {
      bar.setPublisherStatus(text, tone);
      paneStatus.textContent = text;
      paneStatus.title = text;
      paneStatus.dataset.tone = tone;
    },
    setZoomLabel: (text, place) => ((place === "split" ? paneZoomFitButton : zoomFitButton).textContent = text),
    setMarkersShown: (on) => {
      markersShown = on;
      document.getElementById("pane-markers")!.setAttribute("aria-pressed", String(on));
    },
    setDrawerShown: (open) => {
      drawerOpen = open;
      document.getElementById("pane-exceptions")!.setAttribute("aria-pressed", String(open));
    },
    doc: () => view.state.doc,
    error: showError,
  },
  {
    get: () => invoke<LayoutState>("layout_get"),
    set: (changes) => invoke<LayoutState>("layout_set", { changes }),
    setMeta: (changes) => invoke<LayoutState>("meta_set", { changes }),
    applyPreset: (id) => invoke<LayoutState>("layout_apply_preset", { id }),
    setCustom: (text) => invoke<LayoutState>("layout_custom_set", { text }),
    presets: () => invoke<PresetInfo[]>("layout_presets"),
    fonts: () => invoke<string[]>("layout_fonts"),
    faces: fontFaces,
    styles: () => invoke<StyleInfo[]>("bib_styles"),
    styleUses: () => styleUses(view.state.doc.toString()),
    drain,
    error: showError,
    onState: (s) => {
      figures.setLayout(figureLayout(s));
      setStyles(s);
    },
  },
);
publisher.overrides.subscribe(() => figures.setExceptions(placementExceptions(publisher.overrides.list)));
startupProgress(55, "Preparing the page preview…");

/** The file name exports default to: the title (or the project's name). */
async function exportBase(): Promise<string> {
  // The layout panel's state is current only while the pages show.
  const state = (publisher.isActive ? publisher.panel.state : null) ?? (await invoke<LayoutState>("layout_get"));
  return baseName(state.meta.title?.trim() || projectName);
}

/** Exports the compiled document as PDF (from any view), named after the title (or the project). */
async function exportPdf(): Promise<void> {
  if (!sync) return;
  const base = await exportBase();
  const path = await save({ title: "Export PDF", defaultPath: `${base}.pdf`, filters: [{ name: "PDF", extensions: ["pdf"] }] });
  if (!path) return;
  const target = /\.pdf$/i.test(path) ? path : `${path}.pdf`;
  await sync.drain();
  bar.info("Exporting…");
  await invoke("export_pdf", { path: target });
  bar.info(`Exported to ${target}`);
}

/**
 * File → Export…: the compiled document in a format the Rust side writes
 * (or PDF), picked by the file's extension in the save dialog. The
 * warnings go to the title bar (all of them to the console).
 */
async function exportFile(): Promise<void> {
  if (!sync) return;
  const [base, writers] = await Promise.all([exportBase(), invoke<ExportFilter[]>("export_formats")]);
  const filters = exportFilters(writers);
  const dialogFilters = filters.map(({ name, extensions }) => ({ name, extensions }));
  const chosen = await save({ title: "Export", defaultPath: `${base}.${filters[0].extensions[0]}`, filters: dialogFilters });
  if (!chosen) return;
  const target = exportTarget(chosen, filters);
  if (!target) throw new Error(`Lethora can't export ${fileName(chosen)}: pick one of the listed formats.`);
  await sync.drain();
  bar.info("Exporting…");
  if (target.format === PDF_FILTER.id) {
    await invoke("export_pdf", { path: target.path });
    bar.info(exportNotice(target.path, []));
    return;
  }
  const warnings = await invoke<ImportWarning[]>("export_file", { path: target.path, format: target.format });
  if (warnings.length) console.info(`Lethora: exported ${target.path} (${target.format})`, warnings);
  bar.info(exportNotice(target.path, warnings));
}

const palette = new Palette(
  document.getElementById("palette")!,
  () => view,
  (isOpen) => bar.setToggle("palette", isOpen),
);

/** Science-view math commands. */
const scienceOnly = (f: () => void) => async () => {
  if (sync && viewMode === "science") f();
};

/** Inline or display math at the cursor; a display formula is labelled `eq:N`. */
function insertEquation(display: boolean): void {
  const labels = labelsFor(view.state.doc).byName;
  const keys = entriesByKey(library.entries);
  const label = display ? equationLabel((name) => labels.has(name) || keys.has(name)) : undefined;
  view.dispatch(insertMath(view.state, display, label));
  view.focus();
}

const tablePicker = new TablePicker();

/**
 * Format row → Table, Edit → Insert table…: the grid picker below the row's
 * button (at the cursor while the row is hidden); a pick inserts a labelled
 * table figure as a block of its own, the cursor in its first cell (one
 * transaction, sent to the core like typing).
 */
function openTablePicker(): void {
  if (!formatting()) return;
  const button = formatBar.querySelector<HTMLElement>('button[data-command="table"]');
  const shown = button && button.offsetParent !== null ? button.getBoundingClientRect() : null;
  const cursor = view.coordsAtPos(view.state.selection.main.head);
  const anchor = shown ?? (cursor ? { left: cursor.left, top: cursor.top, bottom: cursor.bottom } : { left: 40, top: 80, bottom: 80 });
  tablePicker.open(anchor, {
    insert: (columns, rows, header) => {
      if (formatting()) {
        const label = nextTableLabel(labelsFor(view.state.doc).byName.keys());
        view.dispatch(insertTable(view.state, columns, rows, header, label));
      }
      view.focus();
    },
    closed: () => view.focus(),
  });
}

/**
 * What the editor's menus do: a spelling correction (`spell-<index>` of
 * `menuSpelling`), the clipboard, a review item, the suggestion at the
 * click (`menuSuggestion`), or a table command.
 */
type EditorAct = `spell-${number}` | "spell-add" | "spell-ignore" | "cut" | "copy" | "paste" | "comment" | `highlight-${MarkColour}` | "accept" | "reject" | TableAction;

/** The suggestion the right-click menu opened on. */
let menuSuggestion = 0;
/** The misspelled word the right-click menu opened on, and its corrections. */
let menuSpelling: { word: Word; corrections: string[] } | null = null;

/** Replaces the menu's misspelled word with correction `i` (an edit like typing), if the word is still there. */
function correctSpelling(i: number): void {
  const typo = menuSpelling;
  const insert = typo?.corrections[i];
  if (!typo || insert === undefined || !formatting()) return;
  const { from, to, word } = typo.word;
  if (view.state.sliceDoc(from, to) !== word) return;
  view.dispatch({ changes: { from, to, insert }, selection: EditorSelection.cursor(from + insert.length), userEvent: "input.complete" });
}

/** Adds the menu's misspelled word to the personal dictionary (all projects). */
async function addToDictionary(): Promise<void> {
  const word = menuSpelling?.word.word;
  if (!word) return;
  await invoke("spell_add", { word });
  spell.added(word);
}

/** Ignores the menu's misspelled word everywhere in this project until it closes. */
function ignoreSpelling(): void {
  if (menuSpelling) spell.ignore(menuSpelling.word.word);
}

/** The editor's right-click menu and the row's "Table ▾" menu (one at a time). */
const editorMenu = new PageMenu<EditorAct>(nativeMac);

const editorMenuHandlers: MenuHandlers<EditorAct> = {
  run: (act) => {
    if (act === "spell-add") addToDictionary().catch(showError);
    else if (act === "spell-ignore") ignoreSpelling();
    else if (act.startsWith("spell-")) correctSpelling(Number(act.slice("spell-".length)));
    else if (act === "cut" || act === "copy" || act === "paste") runCommand(commandById.get(act)!);
    else if (act === "comment") newComment();
    else if (act.startsWith("highlight-")) addMark(act.slice("highlight-".length) as MarkColour);
    else if (act === "accept") review.accept(menuSuggestion);
    else if (act === "reject") review.reject(menuSuggestion);
    else if (formatting()) format((s) => tableCommand(s, act as TableAction) ?? {});
  },
  // No space item in these menus.
  space: async () => undefined,
  closed: () => view.focus(),
};

/** The table commands at the selection while formatting is possible (greyed out while read only). */
const tableItems = (): MenuItem<EditorAct>[] => tableMenu(view.state).map((i) => (i.type === "item" && !formatting() ? { ...i, act: null } : i));

/** Formatting row → Table ▾: the table commands below the button. */
function openTableMenu(): void {
  const items = tableItems();
  if (!items.length) return;
  const r = tableMenuButton.getBoundingClientRect();
  editorMenu.open(r.left, r.bottom + 4, { header: null, label: "Table", items }, editorMenuHandlers);
}

/**
 * Right-click in the editor (both views): on a misspelled word its
 * corrections, Add to dictionary and Ignore first; Cut, Copy, Paste and, in
 * a table, the table commands. A click outside the selection puts the
 * cursor there first (one inside keeps it, so selected cells can be
 * merged); from the keyboard the menu opens at the cursor.
 */
function openEditorMenu(e: MouseEvent): void {
  const target = e.target as Element;
  // The search panel's fields keep the web view's own menu.
  if (!sync || !inText() || target.closest(".cm-panels")) return;
  e.preventDefault();
  let x = e.clientX;
  let y = e.clientY;
  let clicked = view.state.selection.main.head;
  if (e.button !== 2) {
    const at = view.coordsAtPos(view.state.selection.main.head);
    if (at) [x, y] = [at.left, at.bottom];
  } else {
    const pos = view.posAtCoords({ x, y });
    const { from, to } = view.state.selection.main;
    if (pos !== null) clicked = pos;
    if (pos !== null && !(from < to && from <= pos && pos <= to)) view.dispatch({ selection: EditorSelection.cursor(pos), userEvent: "select.pointer" });
  }
  // The suggestion under the click (or the caret), as shown with the panel.
  const suggestion = review.isShown ? review.suggestionAt(clicked) : null;
  menuSuggestion = suggestion?.id ?? 0;
  const selected = !view.state.selection.main.empty;
  const items: MenuItem<EditorAct>[] = [
    { type: "item", label: "Cut", keys: "Ctrl+X", act: selected && formatting() ? "cut" : null },
    { type: "item", label: "Copy", keys: "Ctrl+C", act: selected ? "copy" : null },
    { type: "item", label: "Paste", keys: "Ctrl+V", act: formatting() ? "paste" : null },
    { type: "separator" },
    { type: "item", label: "New comment", keys: commandById.get("comment")!.keys?.[0], act: canAddReview() ? "comment" : null },
    { type: "submenu", label: "Highlight", items: MARK_COLOURS.map((c) => ({ type: "item", label: COLOUR_LABELS[c], act: canAddReview() ? (`highlight-${c}` as const) : null })) },
  ];
  if (suggestion) {
    items.push({ type: "separator" }, { type: "item", label: "Accept suggestion", act: suggestion.accept ? "accept" : null, title: suggestion.why || undefined }, { type: "item", label: "Reject suggestion", act: suggestion.reject ? "reject" : null });
  }
  const table = tableItems();
  if (table.length) items.push({ type: "separator" }, ...table);
  const typo = spellOn ? spell.misspelledAt(view.state, clicked) : null;
  menuSpelling = null;
  if (!typo) {
    editorMenu.open(x, y, { header: null, label: "Edit", items }, editorMenuHandlers);
    return;
  }
  // The corrections first (the menu opens when they are there, quickly).
  invoke<string[]>("spell_suggest", { lang: spell.lang, word: typo.word })
    .catch((e: unknown) => {
      showError(e);
      return [];
    })
    .then((suggested) => {
      const corrections = suggested.slice(0, 5);
      menuSpelling = { word: typo, corrections };
      const spelling: MenuItem<EditorAct>[] = corrections.length
        ? corrections.map((c, i) => ({ type: "item", label: c, act: formatting() ? (`spell-${i}` as const) : null }))
        : [{ type: "item", label: "No suggestions", act: null }];
      spelling.push({ type: "item", label: "Add to dictionary", act: "spell-add" }, { type: "item", label: "Ignore", act: "spell-ignore" }, { type: "separator" });
      editorMenu.open(x, y, { header: null, label: "Edit", items: [...spelling, ...items] }, editorMenuHandlers);
    });
}

/** Whether a figure label is taken (by a label in the text or a source key). */
function labelTaken(): (name: string) => boolean {
  const labels = labelsFor(view.state.doc).byName;
  const keys = entriesByKey(library.entries);
  return (name) => labels.has(name) || keys.has(name);
}

/** Asks for an image, copies it into `assets/` and inserts a labelled figure at the cursor, one undo step. */
async function addFigure(): Promise<void> {
  if (!sync || viewMode !== "science") return;
  const path = await open({ title: "Insert a figure", multiple: false, directory: false, filters: [{ name: "Images", extensions: IMAGE_EXTENSIONS }] });
  if (typeof path !== "string") return;
  await insertStaged([(taken) => invoke<Staged>("asset_stage", { path, folder: null, taken })], () => view.state.selection.main.head);
}

/** Inserts a labelled figure showing asset `path` at the cursor. */
function insertFigureFor(path: string): void {
  view.dispatch(insertFigure(view.state, path, uniqueLabel(labelStem(path), labelTaken())));
  view.focus();
}

/** A staged file (`asset_stage`, `asset_stage_bytes`): its blob is stored, its path proposed, nothing indexed yet. */
interface Staged {
  path: string;
  blob: string;
}

/** The percent-encoded headers of a raw-bytes asset command (`asset_import_bytes`, `asset_stage_bytes`). */
function bytesHeaders(name: string, folder: string, taken: readonly string[] = []): Record<string, string> {
  const headers: Record<string, string> = { "x-file-name": encodeURIComponent(name), "x-folder": encodeURIComponent(folder) };
  if (taken.length) headers["x-taken"] = taken.map(encodeURIComponent).join(",");
  return headers;
}

/**
 * Stages files in order (each `stage` gets the paths proposed before it),
 * then inserts a labelled figure for each, as blocks of their own at `at()`
 * (asked once pending edits are sent): the files' index entries and the
 * figures are one undo step, and nothing changes if anything fails. The
 * first caption is selected.
 */
async function insertStaged(stages: ReadonlyArray<(taken: string[]) => Promise<Staged>>, at: () => number): Promise<void> {
  if (!sync) return;
  const staged: Staged[] = [];
  for (const stage of stages) staged.push(await stage(staged.map((s) => s.path)));
  let caption = -1;
  const reply = await sync.apply(() => {
    const pos = at();
    const figure = figureText(view.state.doc, pos, pos, figureLabels(staged.map((s) => s.path), labelTaken()));
    caption = pos + figure.caption;
    return invoke<Reply>("figure_insert", { pos, text: figure.insert, assets: staged });
  });
  // Typing while the request was in flight may have moved it.
  if (!reply || view.state.sliceDoc(caption, caption + "Caption".length) !== "Caption") return;
  view.dispatch({ selection: EditorSelection.single(caption, caption + "Caption".length), effects: EditorView.scrollIntoView(caption, { y: "center" }), userEvent: "select" });
  view.focus();
}

/**
 * Files or a panel item dropped into the text at paragraph boundary
 * `boundary` (found again once pending edits are sent). Files are imported
 * into the assets panel's open folder with their figures as one undo step
 * (unsupported ones are named); a panel item's figure uses its path.
 */
async function dropFigures(dropped: Dropped, boundary: number): Promise<void> {
  if (!sync || viewMode === "publisher") return;
  if ("path" in dropped) {
    const pos = boundaryAt(view.state, boundary);
    const { insert, caption } = figureText(view.state.doc, pos, pos, figureLabels([dropped.path], labelTaken()));
    const from = pos + caption;
    view.dispatch({ changes: { from: pos, insert }, selection: EditorSelection.single(from, from + "Caption".length), scrollIntoView: true, userEvent: "input.drop" });
    view.focus();
    return;
  }
  const { ok, skipped } = supportedFiles(dropped.files);
  const note = skipped.length ? skippedMessage(skipped) : "";
  if (!ok.length) {
    bar.info(note);
    return;
  }
  const folder = await assets.importFolder();
  const stages = ok.map((file) => async (taken: string[]) => invoke<Staged>("asset_stage_bytes", new Uint8Array(await file.arrayBuffer()), { headers: bytesHeaders(file.name, folder, taken) }));
  await insertStaged(stages, () => boundaryAt(view.state, boundary));
  bar.info(`${ok.length === 1 ? "Inserted a figure" : `Inserted ${ok.length} figures`} (files in ${folder.slice(0, -1)}). ${note}`.trim());
}

/**
 * Asks for an image and stores it under a path of its own; only the image
 * call at `pos` (reading asset `path`) is changed to it, one undo step. The
 * reply is applied like a restore's.
 */
async function replaceImage(path: string, pos: number): Promise<void> {
  if (!sync) return;
  const source = await open({ title: "Replace image", multiple: false, directory: false, filters: [{ name: "Images", extensions: IMAGE_EXTENSIONS }] });
  if (typeof source !== "string") return;
  // Pending edits are sent first.
  const reply = await sync.apply(() => invoke<Reply & { path: string }>("asset_replace", { path, source, pos }));
  if (!reply) return;
  figures.invalidate(reply.path);
  bar.info(`Replaced the image: ${reply.path}.`);
}

const sources = new SourcesPanel({
  library,
  cite: (key) => {
    view.dispatch(insertCitation(view.state, key));
    view.focus();
  },
  error: showError,
  info: (message) => bar.info(message),
  focusEditor: () => view.focus(),
  onOpenChange: (isOpen) => {
    bar.setToggle("sources", isOpen);
    if (isOpen) panel.close();
  },
});

// ---- Assets (bottom panel, every view) ----

/** Selects an asset's usage in the text; the publisher view goes back to the last text view. */
async function revealUsage(from: number, to: number): Promise<void> {
  if (viewMode === "publisher") await setView(textView);
  view.dispatch({ selection: { anchor: from, head: to }, effects: EditorView.scrollIntoView(from, { y: "center" }), userEvent: "select" });
  view.focus();
}

const assets = new AssetsPanel(document.getElementById("assets")!, {
  api: {
    list: () => invoke<AssetList>("asset_list"),
    bytes: (path) => invoke<AssetBytes>("asset_bytes", { path }),
    import: (source, folder) => invoke<string>("asset_import", { path: source, folder }),
    importBytes: (name, bytes, folder) => invoke<string>("asset_import_bytes", new Uint8Array(bytes), { headers: bytesHeaders(name, folder) }),
    update: (path, source) => invoke<void>("asset_update", { path, source }),
    save: (path, target) => invoke<void>("asset_save", { path, target }),
    // The text's literals change: applied like `asset_replace`'s reply.
    move: async (from, to) => {
      await sync?.apply(() => invoke<Reply & { path: string }>("asset_move", { from, to }));
    },
    remove: (path) => invoke<void>("asset_remove", { path }),
    createFolder: (path) => invoke<void>("asset_folder_create", { path }),
    pickFiles: async (title, extensions, multiple) => {
      const picked = await open({ title, multiple, directory: false, filters: [{ name: "Assets", extensions }] });
      return picked === null ? [] : Array.isArray(picked) ? picked : [picked];
    },
    pickTarget: (title, name, extensions) => save({ title, defaultPath: name, filters: [{ name: extensions[0].toUpperCase(), extensions }] }),
  },
  drain,
  text: () => view.state.doc.toString(),
  reveal: (from, to) => void revealUsage(from, to).catch(showError),
  canInsert: () => sync !== null && viewMode !== "publisher",
  insertFigure: (path) => {
    if (sync && viewMode !== "publisher") insertFigureFor(path);
  },
  sourceCount: () => (library.loaded ? library.entries.length : null),
  // The drawer belongs to the science view.
  openSources: () => void (viewMode === "science" ? Promise.resolve() : setView("science")).then(() => sources.open(), showError),
  changed: () => figures.reload(),
  focusMain: () => (viewMode === "publisher" ? document.getElementById("pub-scroll")!.focus() : view.focus()),
  shown: (isOpen) => bar.setToggle("assets", isOpen),
  info: (message) => bar.info(message),
  error: showError,
});
library.subscribe(() => assets.sourcesChanged());

document.body.toggleAttribute("data-focus", focusOn);
bar.setToggle("pages", splitOn);
bar.setToggle("review", reviewOn);
void setView(viewMode);
startupProgress(75, "Preparing the interface…");

// ---- Checkpoints ----

const panel = new CheckpointPanel({
  api: {
    list: () => invoke<CheckpointInfo[]>("checkpoints"),
    create: (name) => invoke<CheckpointInfo>("checkpoint_create", { name }),
    text: (id) => invoke<string>("checkpoint_text", { id }),
  },
  sync: () => sync,
  editorEl,
  previewExtensions: writing,
  info: (message) => bar.info(message),
  error: showError,
  errorCount: () => bar.errors,
  focusEditor: () => view.focus(),
  onOpenChange: (isOpen) => {
    document.body.classList.toggle("panel-open", isOpen);
    if (isOpen) sources.close();
  },
});

// ---- Sync (folder and server) ----

const syncDialog = new SyncDialog({
  api: {
    status: () => invoke<SyncStatus>("sync_status"),
    setup: (folder) => invoke<{ code: string; status: SyncStatus }>("sync_setup", { folder }),
    setupApp: (id) => invoke<{ code: string; status: SyncStatus }>("sync_setup_app", { id }),
    apps: () => invoke<SyncApp[]>("sync_apps"),
    code: () => invoke<string>("sync_code"),
    enterCode: (code) => invoke<SyncStatus>("sync_enter_code", { code }),
    stop: () => invoke<SyncStatus>("sync_stop"),
    dismissNotice: () => invoke<void>("sync_notice_dismiss"),
    pickFolder: async () => {
      const folder = await open({ directory: true, title: "Choose a sync folder (e.g. in Syncthing or on a USB stick)" });
      return typeof folder === "string" ? folder : null;
    },
    find: (code, folder) => invoke<FindReply>("sync_find", { code, folder }),
    pickSharedFolder: async () => {
      const folder = await open({ directory: true, title: "Choose the synced project's folder (name ends in .lethora-sync) or the folder holding it" });
      return typeof folder === "string" ? folder : null;
    },
    serverRecent: () => invoke<{ servers: string[]; name: string | null }>("server_recent"),
    serverShare: (url, name) => invoke<SyncStatus>("server_share", { url, name }),
    serverInvite: (role: Exclude<Role, "owner"> | null, days, uses) => invoke<string>("server_invite", { role, days, uses }),
    serverMembers: () => invoke<MembersReply>("server_members"),
    serverSetRole: (user, role) => invoke<void>("server_set_role", { user, role }),
    serverSetColour: (colour) => invoke<void>("server_set_colour", { colour }),
    serverRemove: (target) => invoke<void>("server_remove", target),
    serverRevoke: (invite) => invoke<void>("server_revoke", { invite }),
    serverLeave: () => invoke<SyncStatus>("server_leave"),
    serverDelete: () => invoke<SyncStatus>("server_delete"),
    serverRemembered: () => invoke<RememberedProject[]>("server_remembered"),
  },
  onStatus: (status) => applyStatus(status),
  join: (replica, code) => switchOrFail(() => invoke<Opened>("sync_join", { replica, code })),
  joinServer: (link, name) => switchOrFail(() => invoke<Opened>("server_join", { link, name })),
  reopenServer: (project) => switchOrFail(() => invoke<Opened>("server_reopen", { project })),
  openProject: (path) => switchOrFail(() => openPath(path)),
  info: (message) => bar.info(message),
  error: showError,
});

/** Opens the sync dialog for the current status. */
async function openSyncDialog(setup = false): Promise<void> {
  if (!sync) return;
  const status = await invoke<SyncStatus>("sync_status");
  applyStatus(status);
  if (setup) syncDialog.startSetup(status);
  else syncDialog.open(status);
}

/** File → Members…: the members screen of the Share / Sync dialog. */
async function openMembers(): Promise<void> {
  if (!sync) return;
  const status = await invoke<SyncStatus>("sync_status");
  applyStatus(status);
  syncDialog.members(status);
}

const readOnlyBadge = document.getElementById("read-only")!;
const ROLE_LABELS: Record<Role, string> = { owner: "Owner", editor: "Editor", reviewer: "Reviewer", reader: "Reader" };

/** The write controls disabled while read only (the editor itself follows `readOnlySlot`). */
const writeGuard = new WriteGuard([
  { root: document.getElementById("bar")!, selector: "#insert-figure, #toggle-palette" },
  { root: document.getElementById("layout-panel")!, selector: "input, select, textarea, button" },
  { root: document.getElementById("sources")!, selector: "#src-import, #src-paste-toggle, #src-add-toggle" },
  { root: document.getElementById("cp-form")!, selector: "input, button" },
  { root: document.getElementById("preview")!, selector: "#preview-restore" },
  { root: document.getElementById("assets")!, selector: "[data-write]" },
  { root: publisherEl, selector: '.exd [data-action="edit"], .exd [data-action="remove"], .exd-bulk-bar .ex-action:not(:last-child)' },
]);
/** The review's write controls, disabled for readers. */
const reviewGuard = new WriteGuard([{ root: reviewEl, selector: "[data-write]" }]);

/** Shows a new sync status: the cloud, and read only (badge, editor, write controls) as the role says, live. */
function applyStatus(status: SyncStatus | null): void {
  const wasServer = lastStatus?.mode === "server";
  lastStatus = status;
  syncCloud.set(status);
  if ((status?.mode === "server") !== wasServer) {
    presence.reset();
    publishPresence();
    refreshPresence();
    requestAuthors(0);
    // The members' names for the cards.
    void review.refresh();
  }
  const role = readOnlyRole(status);
  const reviewLocked = reviewReadOnlyOf(status);
  if (role === readOnly && reviewLocked === reviewReadOnly) return;
  const mainChanged = role !== readOnly;
  readOnly = role;
  reviewReadOnly = reviewLocked;
  reviewGuard.set(reviewLocked);
  review.setAccess(access());
  if (!mainChanged) return;
  applyEditing();
  readOnlyBadge.hidden = role === null;
  // A reviewer writes the review only: comments, highlights and suggestions.
  readOnlyBadge.textContent = role === "reviewer" ? "Reviewer" : role ? `Read only · ${ROLE_LABELS[role]}` : "";
  readOnlyBadge.title =
    role === "reviewer"
      ? "Your role in this shared project (Reviewer) lets you comment, highlight and suggest changes, not edit the text directly."
      : role
        ? `Your role in this shared project (${ROLE_LABELS[role]}) lets you read, not change it.`
        : "";
  document.body.toggleAttribute("data-read-only", role !== null);
  writeGuard.set(role !== null);
  if (role) {
    palette.set(false);
    publisher.escape();
  }
}

const syncCloud = new SyncCloud(() => void openSyncDialog().catch(showError));
// Left, removed or deleted from the server: the project turned local (see serversync.rs).
void listen<string>("server-left", (e) => {
  bar.info(`${e.payload} This copy is a local project now.`);
  invoke<SyncStatus>("sync_status").then(applyStatus, showError);
});
const syncNotice = new SyncNotice({
  setup: () => void openSyncDialog(true).catch(showError),
  dismiss: () => invoke<void>("sync_notice_dismiss"),
  error: showError,
});
const openNotice = new OpenNotice();
const damagedDialog = new DamagedDialog();

/** Pulls other devices' changes every 2 s while a project is open, and at once on `remote-changes` (see pull.ts). */
let poller: Poller | null = null;

// Server sync: queued remote changes are pulled through the edit queue at once.
void listen("remote-changes", () => poller?.request());
let pullError = "";

// ---- Presence (server sync only, see presence.ts) ----

/** Publishes this device's view and selection (core offsets) in a server-shared project. */
function publishPresence(): void {
  if (!sync || lastStatus?.mode !== "server") return;
  const { anchor, head } = view.state.selection.main;
  presence.update({ view: viewMode, anchor: sync.toCore(anchor), head: sync.toCore(head) });
}

/** Shows the others' presence: carets in the editor, initials in the title bar. */
function refreshPresence(): void {
  const current = sync;
  const peers = current && lastStatus?.mode === "server" ? invoke<PeerPresence[]>("presence_get") : Promise.resolve([]);
  peers.then((list) => {
    if (current !== sync) return;
    bar.setOnline(onlineUsers(list));
    const carets = current ? caretsOf(list, (pos) => current.toEditor(pos), view.state.doc.length) : [];
    view.dispatch({ effects: setRemoteCarets(carets) });
  }, (e) => console.warn("Lethora: presence failed:", e));
}

void listen("presence-changed", () => refreshPresence());

// ---- Who wrote this (server sync only, see authors.ts) ----

/** Whether the author backgrounds show: switched on, in a server-shared project, in the author or science view. */
function authorsShown(): boolean {
  return authorsOn && sync !== null && lastStatus?.mode === "server" && viewMode !== "publisher";
}

function setAuthors(on: boolean): void {
  authorsOn = on;
  saveFlag("lethora.authors", on);
  requestAuthors(0);
}

/** Asks for the authors of the visible text after `delay` ms (the latest request wins). */
function requestAuthors(delay = AUTHORS_DELAY_MS): void {
  clearTimeout(authorsTimer);
  authorsTimer = setTimeout(refreshAuthors, delay);
}

/** Shows who wrote the visible text (or nothing, if the authors don't show). */
function refreshAuthors(): void {
  const current = sync;
  if (!current || !authorsShown()) {
    if (view.state.field(authorMarkField).length) view.dispatch({ effects: setAuthorMarks([]) });
    showLegend(authorsLegend, []);
    return;
  }
  const { from, to } = view.viewport;
  invoke<Authors>("authors_get", { from: current.toCore(from), to: current.toCore(to) }).then(
    (authors) => {
      if (current !== sync || !authorsShown()) return;
      view.dispatch({ effects: setAuthorMarks(authorMarks(authors, (pos) => current.toEditor(pos), view.state.doc.length)) });
      showLegend(authorsLegend, legendOf(authors));
    },
    (e) => console.warn("Lethora: authors failed:", e),
  );
}

/**
 * Imports other devices' changes: queued like a restore, so local edits go
 * first and typing meanwhile is transformed over it; then reloads only what
 * changed. Remote changes are not on the local undo stack.
 */
async function pull(): Promise<void> {
  const current = sync;
  if (!current) return;
  const reply = await current.apply(() => invoke<PullReply>("sync_pull"));
  if (!reply || current !== sync) return;
  pullError = "";
  applyStatus(reply.status);
  // Others' selections may lie in the text just pulled; who wrote it changed.
  if (reply.changed) {
    refreshPresence();
    requestAuthors();
  }
  if (reply.settings) {
    if (publisher.isActive) publisher.refreshLayout();
    else {
      refreshFigures();
      refreshStyles();
    }
  }
  if (reply.bibliography) library.refresh().catch(showError);
  if (reply.assets) {
    figures.reload();
    assets.reload();
  }
  if (reply.checkpoints) panel.reload();
  if (reply.review) void review.refresh();
}

/** Starts pulling for the project just loaded and shows its sync status (and the plain-sync notice). */
function startSync(): void {
  poller?.stop();
  syncNotice.hide();
  poller = new Poller(pull, {
    onError: (e) => {
      const text = e instanceof Error ? e.message : String(e);
      if (text !== pullError) console.warn("Lethora: pulling changes failed:", text);
      pullError = text;
    },
  });
  poller.start();
  invoke<SyncStatus>("sync_status").then((status) => {
    applyStatus(status);
    if (status.notice) syncNotice.show(status.notice.provider);
  }, showError);
}

/**
 * "Open shared project…": the project code is pasted, the synced project is
 * found in the sync apps' folders (or a chosen one) and joined into the
 * default projects folder (see `SyncDialog.openShared`).
 */
function openSharedProject(): void {
  syncDialog.openShared();
}

/** `switchProject` as a promise that rejects if switching failed. */
function switchOrFail(request: () => Promise<Opened | null>): Promise<void> {
  return new Promise((resolve, reject) => void switchProject(request, reject).then(resolve));
}

// ---- Projects ----

function load(opened: Opened): void {
  sync?.dispose();
  publisher.reset();
  panel.close();
  sources.close();
  library.reset();
  figures.clear();
  textStyles = null;
  suggestOn = false;
  unsent.clear();
  spell.reset();
  view.setState(EditorState.create({ doc: opened.text, extensions: extensions() }));
  suggestBadge.hidden = !suggesting() || readOnly === "reviewer";
  if (view.state.doc.length !== opened.len16) {
    console.error(`Lethora: loaded length ${view.state.doc.length}, server has ${opened.len16}`);
  }
  sync = new Sync(backend, view, { checkHash: import.meta.env.DEV, onError: showError });
  // Presence and authors start with this project's status (see `applyStatus`).
  lastStatus = null;
  bar.setOnline([]);
  showLegend(authorsLegend, []);
  review.reset();
  refreshFormatBar();
  projectName = opened.name;
  bar.setProject(opened.name);
  scheduleWordCount(0);
  emptyEl.hidden = true;
  document.body.dataset.view = "editor";
  showView();
  if (viewMode !== "publisher") view.focus();
  void review.refresh();
  library.refresh().catch(showError);
  refreshFigures();
  refreshStyles();
  assets.reset();
  startSync();
  openNotice.show(opened.recovered, opened.skipped);
  refreshRecent();
}

function showEmpty(): void {
  editorEl.hidden = true;
  document.body.toggleAttribute("data-split", false);
  publisher.setActive(null);
  showReview();
  emptyEl.hidden = false;
  document.body.dataset.view = "empty";
  bar.setProject(null);
  bar.setWords(null);
  refreshFormatBar();
  poller?.stop();
  applyStatus(null);
  syncNotice.hide();
  openNotice.hide();
  refreshRecent();
}

/**
 * File → Close project: sends every edit, the core saves and closes the
 * project (the next start shows the start screen), then every panel forgets
 * it and the start screen shows. If saving fails, the project stays open.
 */
async function closeProject(): Promise<void> {
  const current = sync;
  if (!current) return;
  await current.suspend();
  publisher.setActive(null);
  try {
    await invoke("project_close");
  } catch (e) {
    current.resume();
    showView();
    throw e;
  }
  current.dispose();
  sync = null;
  projectName = "";
  publisher.reset();
  panel.close();
  sources.close();
  palette.set(false);
  assets.close();
  assets.reset();
  library.reset();
  figures.clear();
  syncDialog.close();
  review.reset();
  textStyles = null;
  suggestOn = false;
  unsent.clear();
  spell.reset();
  view.setState(EditorState.create({ extensions: extensions() }));
  showEmpty();
  document.getElementById("new")!.focus();
}

// ---- Recent projects (the core keeps the list, see recent.ts) ----

/** The recent projects without the open one, newest first (for Open recent and the start screen). */
let recent: readonly RecentProject[] = [];

const recentList = new RecentList({
  open: (path) => void openRecent(path),
  remove: (path) => invoke<RecentProject[]>("projects_recent_remove", { path }).then(setRecent),
  error: showError,
});

function setRecent(list: readonly RecentProject[]): readonly RecentProject[] {
  recent = list;
  recentList.set(list);
  // After every project open and close too (both list the recent projects again).
  macMenu?.refresh();
  return list;
}

function refreshRecent(): void {
  invoke<RecentProject[]>("projects_recent").then(setRecent, showError);
}

/** Opens a recent project; one gone from disk is reported and taken off the list (by the core). */
function openRecent(path: string): Promise<void> {
  return switchProject(
    () => openPath(path),
    (e) => {
      showError(e);
      refreshRecent();
    },
  );
}

async function clearRecent(): Promise<void> {
  setRecent(await invoke<RecentProject[]>("projects_recent_clear"));
}

/** Sends all edits to the current project, then switches; on failure or `null` (cancelled) the current one stays (`fail` gets the error). */
async function switchProject(request: () => Promise<Opened | null>, fail: (e: unknown) => void = showError): Promise<void> {
  const current = sync;
  await current?.suspend();
  publisher.setActive(null);
  try {
    const opened = await request();
    if (opened) load(opened);
    else {
      current?.resume();
      showView();
    }
  } catch (e) {
    current?.resume();
    showView();
    fail(e);
  }
}

async function openProject(): Promise<void> {
  const path = await open({ directory: true, title: "Open project" });
  if (typeof path === "string") await switchProject(() => openPath(path));
}

/** Opens the project at `path` (see `opening`). */
async function openPath(path: string): Promise<Opened | null> {
  return opening(await invoke<Opening>("project_open", { path }));
}

/** The opened project; a damaged one first asks whether to open it at the last good state (`null`: cancelled). */
async function opening(reply: Opening): Promise<Opened | null> {
  if ("opened" in reply) return reply.opened;
  if (!(await damagedDialog.ask(reply.damaged.file))) return null;
  return opening(await invoke<Opening>("project_open", { path: reply.damaged.path, recover: true }));
}

const newProjectDialog = new NewProjectDialog();
const aboutDialog = new AboutDialog();

/** Help → About Lethora. */
async function openAbout(): Promise<void> {
  aboutDialog.open(await invoke<AppInfo>("app_info"));
}

// ---- App-wide settings (File → Settings…, see settings.ts) and updates (see updates.ts) ----

/** settings.json in the app config folder (null until read). */
let appSettings: AppSettings | null = null;

/** Applies the settings (the theme is applied by the core): the editor's text, spelling and the shortcuts. */
function applySettings(s: AppSettings): void {
  appSettings = s;
  applyShortcuts(commands, defaultKeys, s.shortcuts);
  const root = document.documentElement.style;
  root.setProperty("--text-size", `${s.editor_text_size}px`);
  root.setProperty("--line-spacing", String(s.editor_line_spacing));
  root.setProperty("--paragraph-gap", String(s.editor_paragraph_gap));
  view.requestMeasure();
  if (s.spell_check !== spellOn) {
    spellOn = s.spell_check;
    view.dispatch({ effects: spellSlot.reconfigure(spellOn ? spell.extension : []) });
  }
  macMenu?.refresh();
  settingsView.refresh();
}

/** Saves changes (`null`: the default) and applies them. */
async function changeSettings(changes: Partial<Record<keyof AppSettings, unknown>>): Promise<void> {
  applySettings(await invoke<AppSettings>("settings_set", { changes }));
}

const settingsView = new SettingsView({
  settings: () => appSettings,
  change: changeSettings,
  shortcuts: () => shortcutSections(menus, (id) => commandById.get(id), (id) => defaultKeys.get(id) ?? [], nativeMac),
  mac: nativeMac,
  appInfo: () => invoke<AppInfo>("app_info"),
  checkUpdate: () => invoke<UpdateInfo | null>("update_check", { manual: true }),
  install: (info) => updateDialog.available(info),
});

const settingsRead = invoke<AppSettings>("settings_get")
  .then(async (s) => {
    applySettings(s);
    // Once: View → Check spelling was a local preference before settings.json.
    if (!loadFlag("lethora.spell", true) && s.spell_check) await changeSettings({ spell_check: false });
    try {
      localStorage.removeItem("lethora.spell");
    } catch {
      // nothing to carry over
    }
  })
  .catch(showError);

/** Install and restart: every edit is sent first, as when the window closes. */
async function installUpdate(): Promise<void> {
  await sendEdits();
  await invoke("update_install");
}

const updateDialog = new UpdateDialog(installUpdate);
void listen<UpdateProgress>("update-progress", (e) => updateDialog.onProgress(e.payload));

/** Help → Check for updates… */
async function checkForUpdates(): Promise<void> {
  updateDialog.checking();
  try {
    const found = await invoke<UpdateInfo | null>("update_check", { manual: true });
    if (found) updateDialog.available(found);
    else updateDialog.upToDate((await invoke<AppInfo>("app_info")).version);
  } catch (e) {
    updateDialog.failed(e);
  }
}

/** The quiet check after start (the core skips it when turned off): only a found update shows. */
function checkUpdatesSoon(): void {
  setTimeout(() => {
    invoke<UpdateInfo | null>("update_check", { manual: false }).then(
      (found) => found && updateDialog.available(found),
      (e) => console.info("Lethora: update check failed:", e),
    );
  }, UPDATE_CHECK_DELAY_MS);
}

/** Asks for a name (or a place), a preset and optional title/author, and creates the project (by default in Documents/Lethora). */
async function newProject(): Promise<void> {
  const [folder, presets] = await Promise.all([invoke<string>("projects_folder"), invoke<PresetInfo[]>("layout_presets")]);
  const choice = await newProjectDialog.ask(folder, presets, (name) => save({ title: "New project", defaultPath: name }));
  if (!choice) return;
  const meta = Object.keys(choice.meta).length > 0 ? choice.meta : null;
  await switchProject(() => invoke<Opened>("project_new", { path: choice.path, name: choice.name, preset: choice.preset, meta }));
}

// ---- Import (File → Import…, Edit → Insert file…; see imports.ts) ----

/** Asks for a file in a format the registry reads. */
async function pickImport(title: string): Promise<string | null> {
  const filters = await invoke<ImportFilter[]>("import_formats");
  const path = await open({ title, multiple: false, directory: false, filters });
  return typeof path === "string" ? path : null;
}

/** File → Import…: a new project (the New project dialog, titled after the file) holding the file's content. */
async function importProject(): Promise<void> {
  const path = await pickImport("Import");
  if (!path) return;
  // Read without a project first: the title, and a broken file fails here.
  const probe = await invoke<ImportReply>("import_read", { path, stage: false });
  const [folder, presets] = await Promise.all([invoke<string>("projects_folder"), invoke<PresetInfo[]>("layout_presets")]);
  const prefill = { title: probe.title ?? fileStem(path), author: probe.author ?? "" };
  const choice = await newProjectDialog.ask(folder, presets, (name) => save({ title: "New project", defaultPath: name }), prefill);
  if (!choice) return;
  const meta = Object.keys(choice.meta).length > 0 ? choice.meta : null;
  const before = sync;
  await switchProject(() => invoke<Opened>("project_new", { path: choice.path, name: choice.name, preset: choice.preset, meta }));
  // Not created (the error is shown).
  if (!sync || sync === before) return;
  await insertImport(path, () => 0);
}

/** Edit → Insert file…: the file's content at the cursor as blocks of their own, one undo step; in suggest mode a suggestion, without images. */
async function insertFile(): Promise<void> {
  if (!sync) return;
  const path = await pickImport("Insert file");
  if (!path) return;
  if (!suggesting()) {
    await insertImport(path, () => importPos(view.state.doc, view.state.selection.main.head));
    return;
  }
  const reply = await invoke<ImportReply>("import_read", { path, stage: false });
  const pos = importPos(view.state.doc, view.state.selection.main.head);
  const insert = importText(view.state.doc, pos, reply.text);
  // Like a paste: suggest mode makes it a suggestion.
  if (insert) view.dispatch({ changes: { from: pos, insert }, scrollIntoView: true, userEvent: "input.paste" });
  view.focus();
  bar.info(importNotice(fileName(path), reply, true));
}

/**
 * Reads `path` with its images staged and inserts it at `at()` (asked once
 * pending edits are sent): the text and the images' index entries are one
 * undo step. The warnings go to the title bar (all of them to the console).
 */
async function insertImport(path: string, at: () => number): Promise<void> {
  if (!sync) return;
  const reply = await invoke<ImportReply>("import_read", { path, stage: true });
  if (reply.text) {
    await sync.apply(() => {
      const pos = at();
      return invoke<Reply>("figure_insert", { pos, text: importText(view.state.doc, pos, reply.text), assets: reply.assets });
    });
    view.focus();
  }
  if (reply.warnings.length) console.info(`Lethora: imported ${path} (${reply.format})`, reply.warnings);
  bar.info(importNotice(fileName(path), reply));
}

/**
 * F5 / Compile: typesets the document again even if the text is unchanged,
 * merging outside edits of `references.bib` first and reading the images
 * from disk anew (the sources list and the science view's figures too).
 */
async function recompile(): Promise<void> {
  if (!sync) return;
  await sync.drain();
  await library.refresh().catch(showError);
  // Adopts image files copied into `assets/` before the figures reload.
  await invoke("compile_now", { force: true });
  figures.reload();
  assets.reload();
  // The page preview (tab or split pane) shows the compile status itself.
  if (!publisher.isActive) bar.info("Sources and images reloaded; typesetting in the background.");
}

async function saveNow(): Promise<void> {
  if (!sync) return;
  await sync.drain();
  await invoke("save_now");
  bar.info("Saved");
}

// ---- Commands, menus and shortcuts ----

const editorOnly = (f: () => unknown) => async () => {
  if (sync) await f();
};

/** Where Ctrl+Z belongs to the field (its own text), not to the document. */
const isTextField = (t: EventTarget | null) =>
  t instanceof HTMLTextAreaElement || (t instanceof HTMLInputElement && ["text", "number", "search"].includes(t.type)) || (t instanceof HTMLElement && t.isContentEditable);

const hasProject = () => sync !== null;
/** A project this device may change (not read only). */
const writable = () => sync !== null && readOnly === null;
const inText = () => sync !== null && viewMode !== "publisher";
const inScience = () => sync !== null && viewMode === "science";
const writeInScience = () => inScience() && readOnly === null;
const inPublisher = () => sync !== null && viewMode === "publisher";
const pagesShown = () => sync !== null && publisher.isActive;

const inEditor = (t: HTMLElement) => view.contentDOM.contains(t);

/** The editor takes edits: into the text, or as suggestions (suggest mode, also a reviewer's). */
const editable = () => writable() || (sync !== null && suggesting());

/** The formatting row acts: an editable project's author or science view, no checkpoint preview over it (styles, fonts and sizes: writable only). */
const formatting = () => editable() && inText() && !editorEl.classList.contains("behind-preview");

/** Where the menus' clipboard and undo items act: the focused field, else the editor while it shows. */
function editTarget(): HTMLElement | null {
  const t = menuBar?.returnTarget ?? document.activeElement;
  if (isTextField(t)) return t as HTMLElement;
  return inText() ? view.contentDOM : null;
}

/** Whether Cut/Paste may change the edit target: a field, or the editor of a writable project. */
function editsTarget(): boolean {
  const t = editTarget();
  return t !== null && (!inEditor(t) || editable());
}

/** Edit → Undo/Redo: a field's own history, else the review's or the document's (through the core, as Ctrl+Z). */
function undoRedo(kind: "undo" | "redo"): void {
  const t = editTarget();
  if (t && !inEditor(t)) document.execCommand(kind);
  else historyStep(kind, menuBar?.returnTarget ?? document.activeElement);
}

/** Whether Edit → Undo/Redo can act: in a field, on the review's history (as last listed) or on a writable document. */
function canUndoRedo(kind: "undo" | "redo"): boolean {
  if (!sync) return false;
  const t = editTarget();
  if (t && !inEditor(t)) return true;
  if (historyTarget({ field: false, panel: review.contains(menuBar?.returnTarget ?? document.activeElement), suggest: suggesting() }, access()) === "main") return writable();
  return !reviewReadOnly && (!review.isShown || (kind === "undo" ? review.canUndo : review.canRedo));
}

/** Edit → Cut/Copy: the web view's own, so CodeMirror sends a cut to the core like Ctrl+X. */
function clip(kind: "cut" | "copy"): void {
  const t = editTarget();
  if (!t) return;
  if (document.activeElement !== t) {
    if (inEditor(t)) view.focus();
    else t.focus();
  }
  document.execCommand(kind);
}

/** Edit → Paste: the clipboard's text, typed into the editor as one transaction (or into the field). */
async function paste(): Promise<void> {
  const t = editTarget();
  if (!t) return;
  const text = await navigator.clipboard.readText();
  if (!text) return;
  if (inEditor(t)) {
    view.dispatch({ ...view.state.replaceSelection(text), scrollIntoView: true, userEvent: "input.paste" });
    view.focus();
  } else {
    t.focus();
    document.execCommand("insertText", false, text);
  }
}

function selectEverything(): void {
  const t = editTarget();
  if (!t) return;
  if (inEditor(t)) {
    selectAll(view);
    view.focus();
  } else {
    t.focus();
    document.execCommand("selectAll");
  }
}

/** Formatting commands: as the row, while the editor (not a field) has the focus. */
const canFormat = () => formatting() && editTarget() === view.contentDOM;

/** Applies a formatting edit at the editor's selection. */
function format(edit: (state: EditorState) => TransactionSpec): void {
  view.dispatch(edit(view.state));
  view.focus();
}

const inline = (id: InlineFormat, label: string, keys?: string[]): Command => ({
  id,
  label,
  keys,
  run: () => format((s) => toggleInline(s, id)),
  enabled: canFormat,
  checked: () => formats.has(id),
});

const HEADINGS = ["&Paragraph", "Heading &1", "Heading &2", "Heading &3", "Heading &4", "Heading &5", "Heading &6"];

/** Format → Heading: Paragraph and Heading 1–6, the selection's checked. */
const headingItems = (): Command[] =>
  HEADINGS.map((label, level) => ({ id: `heading-${level}`, label, run: () => format((s) => setHeading(s, level)), enabled: canFormat, checked: () => paragraph.heading === level, radio: true }));

/** Edit → Find/Replace: CodeMirror's search panel (Replace puts the cursor in its replace field). */
function find(replace: boolean): void {
  if (!inText()) return;
  openSearchPanel(view);
  if (replace) view.dom.querySelector<HTMLInputElement>(".cm-search [name=replace]")?.focus();
}

/**
 * Every menu item and shortcut (see commands.ts). `local` keys are handled
 * where they apply: the editor's keymap, text fields, and the page preview
 * routing in the keydown handler below.
 */
const commands: Command[] = [
  // File
  { id: "new", label: "&New project…", keys: ["Ctrl+N"], run: newProject },
  { id: "open", label: "&Open project…", keys: ["Ctrl+O"], run: openProject },
  { id: "open-shared", label: "Open s&hared project…", run: openSharedProject },
  { id: "import", label: "&Import…", run: importProject },
  // A submenu (greyed out while the list is empty).
  { id: "open-recent", label: "Open &recent", run: () => undefined, children: () => recentItems(recent, openRecent, clearRecent) },
  { id: "share", label: "Share / S&ync…", run: () => openSyncDialog(), enabled: hasProject },
  { id: "members", label: "&Members…", run: openMembers, enabled: () => hasProject() && lastStatus?.mode === "server" },
  { id: "save", label: "&Save", keys: ["Ctrl+S"], run: saveNow, enabled: hasProject },
  { id: "checkpoint-new", label: "Save chec&kpoint…", keys: ["Ctrl+Shift+S"], run: () => panel.openForCreate(), enabled: writable },
  { id: "export-pdf", label: "&Export PDF…", keys: ["Ctrl+E"], run: exportPdf, enabled: hasProject },
  { id: "export", label: "Ex&port…", run: exportFile, enabled: hasProject },
  { id: "settings", label: "Se&ttings…", keys: ["Ctrl+,"], run: () => settingsView.open() },
  { id: "close", label: "&Close project", keys: ["Ctrl+W", "Ctrl+F4"], run: closeProject, enabled: hasProject },
  // Alt+F4 is only shown: the OS closes the window, through the same close request.
  { id: "exit", label: "E&xit", keys: ["Alt+F4"], local: true, run: () => getCurrentWindow().close() },
  // Edit
  { id: "undo", label: "&Undo", keys: ["Ctrl+Z"], local: true, run: () => undoRedo("undo"), enabled: () => canUndoRedo("undo") },
  { id: "redo", label: "&Redo", keys: ["Ctrl+Y", "Ctrl+Shift+Z"], local: true, run: () => undoRedo("redo"), enabled: () => canUndoRedo("redo") },
  { id: "cut", label: "Cu&t", keys: ["Ctrl+X"], local: true, run: () => clip("cut"), enabled: () => hasProject() && editsTarget() },
  { id: "copy", label: "&Copy", keys: ["Ctrl+C"], local: true, run: () => clip("copy"), enabled: () => hasProject() && editTarget() !== null },
  { id: "paste", label: "&Paste", keys: ["Ctrl+V"], local: true, run: paste, enabled: () => hasProject() && editsTarget() },
  { id: "select-all", label: "Select &all", keys: ["Ctrl+A"], local: true, run: selectEverything, enabled: () => hasProject() && editTarget() !== null },
  { id: "find", label: "&Find…", keys: ["Ctrl+F"], run: () => find(false), enabled: inText },
  { id: "replace", label: "R&eplace…", keys: ["Ctrl+H"], run: () => find(true), enabled: () => inText() && editable() },
  { id: "equation", label: "Insert e&quation", keys: ["Ctrl+M"], run: () => insertEquation(false), enabled: () => inScience() && editable() },
  { id: "display-equation", label: "Insert &display equation", keys: ["Ctrl+Shift+M"], run: () => insertEquation(true), enabled: () => inScience() && editable() },
  { id: "figure", label: "Insert fi&gure…", keys: ["Ctrl+Shift+G"], run: addFigure, enabled: writeInScience },
  { id: "table", label: "Insert ta&ble…", run: openTablePicker, enabled: formatting },
  { id: "insert-file", label: "Insert f&ile…", run: insertFile, enabled: formatting },
  // The row's "Table ▾" (not in a menu: the right-click menu has the same items).
  { id: "table-menu", label: "Table", run: openTableMenu, enabled: formatting },
  { id: "comment", label: "New co&mment", keys: ["Ctrl+Alt+M"], run: newComment, enabled: canAddReview },
  {
    id: "highlight",
    label: "&Highlight",
    run: () => undefined,
    enabled: canAddReview,
    children: () => MARK_COLOURS.map((c) => ({ id: `highlight-${c}`, label: `&${COLOUR_LABELS[c]}`, run: () => addMark(c), enabled: canAddReview })),
  },
  // Owners and Editors switch it; a reviewer's is always on (checked, greyed out).
  { id: "suggest", label: "&Suggest changes", keys: ["Ctrl+Shift+X"], run: () => setSuggest(!suggestOn), enabled: () => inText() && readOnly === null, checked: suggesting },
  // Format
  inline("bold", "&Bold", ["Ctrl+B"]),
  inline("italic", "&Italic", ["Ctrl+I"]),
  inline("underline", "&Underline", ["Ctrl+U"]),
  inline("strike", "&Strikethrough"),
  inline("super", "Su&perscript"),
  inline("sub", "Subs&cript"),
  { id: "heading", label: "&Heading", run: () => undefined, enabled: canFormat, children: headingItems },
  { id: "bullets", label: "Bu&lleted list", run: () => format((s) => toggleList(s, "bullet")), enabled: canFormat, checked: () => paragraph.bullet },
  { id: "numbering", label: "&Numbered list", run: () => format((s) => toggleList(s, "numbered")), enabled: canFormat, checked: () => paragraph.numbered },
  // View
  { id: "view-author", label: "&Author", keys: ["Ctrl+1"], run: () => setView("author"), enabled: hasProject, checked: () => viewMode === "author", radio: true },
  { id: "view-science", label: "&Science", keys: ["Ctrl+2"], run: () => setView("science"), enabled: hasProject, checked: () => viewMode === "science", radio: true },
  { id: "view-publisher", label: "&Publisher", keys: ["Ctrl+3"], run: () => setView("publisher"), enabled: hasProject, checked: () => viewMode === "publisher", radio: true },
  { id: "pages", label: "Pa&ges beside the text", keys: ["F6"], run: () => setSplit(!splitOn), enabled: inText, checked: () => splitOn },
  { id: "palette", label: "&Formula palette", keys: ["Ctrl+Shift+P"], run: () => palette.toggle(), enabled: writeInScience, checked: () => palette.isOpen },
  { id: "sources", label: "S&ources", keys: ["Ctrl+Shift+B"], run: () => sources.toggle(), enabled: inScience, checked: () => sources.isOpen },
  { id: "checkpoints", label: "&Checkpoints", keys: ["Ctrl+."], run: () => panel.toggle(), enabled: hasProject, checked: () => panel.isOpen },
  { id: "assets", label: "Asse&ts", keys: ["F8"], run: () => assets.toggle(), enabled: hasProject, checked: () => assets.isOpen },
  { id: "review", label: "Re&view", run: () => setReview(!reviewOn), enabled: inText, checked: () => reviewOn },
  { id: "exceptions", label: "E&xceptions", keys: ["F7"], run: () => publisher.toggleDrawer(), enabled: pagesShown, checked: () => drawerOpen },
  { id: "markers", label: "&Markers", keys: ["Ctrl+Shift+E"], run: () => publisher.toggleMarkers(), enabled: inPublisher, checked: () => markersShown },
  { id: "focus", label: "Focus mo&de", keys: ["Ctrl+Shift+F"], run: () => setFocusMode(!focusOn), enabled: hasProject, checked: () => focusOn },
  { id: "typewriter", label: "T&ypewriter mode", keys: ["Ctrl+Shift+T"], run: () => setTypewriter(!typewriterOn), enabled: hasProject, checked: () => typewriterOn },
  { id: "spell", label: "Chec&k spelling", run: () => changeSettings({ spell_check: !spellOn }), enabled: hasProject, checked: () => spellOn },
  { id: "authors", label: "Show aut&hors", run: () => setAuthors(!authorsOn), enabled: () => hasProject() && lastStatus?.mode === "server", checked: () => authorsOn && lastStatus?.mode === "server" },
  { id: "zoom-in", label: "Zoom &in", keys: ["Ctrl+Plus"], local: true, run: () => publisher.zoomIn(), enabled: pagesShown },
  { id: "zoom-out", label: "Zoom o&ut", keys: ["Ctrl+Minus"], local: true, run: () => publisher.zoomOut(), enabled: pagesShown },
  { id: "zoom-fit", label: "Fit &width", keys: ["Ctrl+0"], local: true, run: () => publisher.zoomFit(), enabled: pagesShown },
  { id: "compile", label: "Compi&le", keys: ["F5"], run: recompile, enabled: hasProject },
  // Help
  { id: "update-check", label: "Check for &updates…", run: checkForUpdates },
  { id: "update-on-start", label: "Check for updates on &start", run: () => changeSettings({ check_updates_on_start: !appSettings?.check_updates_on_start }), checked: () => appSettings?.check_updates_on_start === true },
  { id: "about", label: "&About Lethora", run: openAbout },
];

const commandById = new Map(commands.map((c) => [c.id, c]));
/** The command list's own keys; settings.json's `shortcuts` go over them (see `applySettings`). */
const defaultKeys = new Map(commands.map((c) => [c.id, c.keys]));

/** Runs a command if it is enabled; errors go to the bar. */
function runCommand(c: Command): void {
  if (!isEnabled(c)) return;
  Promise.resolve()
    .then(() => c.run())
    .catch(showError);
}

const menus: MenuDef[] = [
  { label: "&File", items: ["new", "open", "open-shared", "open-recent", "import", null, "share", "members", null, "save", "checkpoint-new", null, "export-pdf", "export", null, "settings", null, "close", "exit"] },
  { label: "&Edit", items: ["undo", "redo", null, "cut", "copy", "paste", "select-all", null, "find", "replace", null, "equation", "display-equation", "figure", "table", "insert-file", null, "comment", "highlight", "suggest"] },
  { label: "F&ormat", items: ["bold", "italic", "underline", "strike", null, "super", "sub", null, "heading", "bullets", "numbering"] },
  {
    label: "&View",
    items: ["view-author", "view-science", "view-publisher", null, "pages", "palette", "sources", "checkpoints", "assets", "review", "exceptions", "markers", null, "focus", "typewriter", "spell", "authors", null, "zoom-in", "zoom-out", "zoom-fit", null, "compile"],
  },
  // On macOS About moves to the Lethora menu (macmenu.ts).
  { label: "&Help", items: ["update-check", "update-on-start", null, "about"] },
];

// macOS shows the menus in its own menu bar (macmenu.ts), the row has none there.
const menuBar = nativeMac
  ? null
  : new MenuBar(document.getElementById("menubar")!, menus, {
      command: (id) => commandById.get(id)!,
      run: runCommand,
      refocus: (el) => (inEditor(el) ? view.focus() : el.focus({ preventScroll: true })),
    });
const macMenu = nativeMac ? new MacMenu(menus, { command: (id) => commandById.get(id)!, run: runCommand, error: showError }) : null;

// Window-wide and in the capture phase, so they work in the empty state and
// before CodeMirror's own keymap sees them.
window.addEventListener(
  "keydown",
  (e) => {
    // Settings → Keyboard shortcuts is recording keys.
    if (settingsView.recording) return;
    // Cmd+H, Cmd+M and Control+Cmd keys go on to macOS's menus.
    if (macMenu && systemKey(e)) return;
    // The menu bar first (an open menu takes Escape before the chain below).
    if (menuBar?.key(e)) {
      e.preventDefault();
      e.stopPropagation();
      return;
    }
    if (e.key === "Escape" && !e.ctrlKey && !e.metaKey && !e.altKey) {
      // The settings view (a modal dialog) closes itself.
      if (settingsView.isOpen) return;
      // The menus first: they sit over everything (also over the palette beside the split pane);
      // then the edit popover and the Exceptions drawer; last the focus leaves the assets panel.
      if (editorMenu.escape() || tablePicker.escape() || review.escape() || assets.escape() || publisher.escape() || panel.escape() || sources.escape() || palette.escape() || assets.leave()) {
        e.preventDefault();
        e.stopPropagation();
      }
      return;
    }
    const inAssets = assets.contains(e.target);
    const inReview = review.contains(e.target);
    if ((e.ctrlKey || e.metaKey) && !e.altKey && sync && (viewMode === "publisher" || inAssets || inReview || (publisher.isActive && publisherEl.contains(e.target as Node)))) {
      // Zoom and undo/redo in the publisher view (the editor is hidden) and in the split pane while it has the focus;
      // undo/redo in the assets panel and in the Review panel (the review's, see `historyStep`).
      const key = e.key.toLowerCase();
      const zoom = inAssets || inReview ? null : key === "+" || key === "=" ? () => publisher.zoomIn() : key === "-" || key === "_" ? () => publisher.zoomOut() : key === "0" ? () => publisher.zoomFit() : null;
      const history = isTextField(e.target) ? null : key === "z" ? (e.shiftKey ? "redo" : "undo") : key === "y" && !e.shiftKey ? "redo" : null;
      if (zoom || history) {
        e.preventDefault();
        e.stopPropagation();
        if (zoom) zoom();
        else historyStep(history!, e.target);
        return;
      }
    }
    // The command list's keys: F5–F8 run without modifiers and are only swallowed with them
    // (F5 never reloads); Ctrl+R and Ctrl+Shift+R are blocked.
    const action = keyAction(commands, e);
    if (!action) return;
    e.preventDefault();
    e.stopPropagation();
    if ("run" in action) runCommand(action.run);
  },
  true,
);

// Releasing Alt alone focuses the menu bar.
window.addEventListener(
  "keyup",
  (e) => {
    if (!settingsView.recording && menuBar?.keyUp(e)) e.preventDefault();
  },
  true,
);

const onClick = (id: string, action: () => Promise<void> | void) =>
  document.getElementById(id)!.addEventListener("click", () => {
    Promise.resolve(action()).catch(showError);
  });

onClick("new", newProject);
onClick("open", openProject);
onClick("join", openSharedProject);
onClick("toggle-assets", editorOnly(() => assets.toggle()));
onClick("toggle-pages", async () => {
  if (!sync || viewMode === "publisher") return;
  await setSplit(!splitOn);
  view.focus();
});
onClick("toggle-sources", scienceOnly(() => sources.toggle()));
onClick("toggle-review", () => {
  if (inText()) setReview(!reviewOn);
});
onClick("insert-figure", addFigure);
onClick("toggle-palette", () => {
  palette.toggle();
  view.focus();
});
onClick("view-author", async () => {
  await setView("author");
  view.focus();
});
onClick("view-science", async () => {
  await setView("science");
  view.focus();
});
onClick("view-publisher", () => setView("publisher"));
onClick("zoom-in", () => publisher.zoomIn());
onClick("zoom-out", () => publisher.zoomOut());
onClick("zoom-fit", () => publisher.zoomFit());
onClick("pane-zoom-in", () => publisher.zoomIn());
onClick("pane-zoom-out", () => publisher.zoomOut());
onClick("pane-zoom-fit", () => publisher.zoomFit());
onClick("pane-markers", () => publisher.toggleMarkers());
onClick("pane-exceptions", () => publisher.toggleDrawer());
onClick("pane-close", () => setSplit(false));
onClick("compile", recompile);
onClick("export-pdf", exportPdf);

// The formatting row: its buttons keep the editor's focus and selection.
// A row wider than the window scrolls sideways with the wheel.
formatBar.addEventListener(
  "wheel",
  (e) => {
    if (formatBar.scrollWidth <= formatBar.clientWidth || e.deltaX) return;
    formatBar.scrollLeft += e.deltaY;
    e.preventDefault();
  },
  { passive: false },
);
formatBar.addEventListener("mousedown", (e) => {
  if ((e.target as HTMLElement).closest("button")) e.preventDefault();
});
formatBar.addEventListener("click", (e) => {
  const button = (e.target as HTMLElement).closest<HTMLButtonElement>("button[data-command]");
  if (button && formatting()) commandById.get(button.dataset.command!)!.run();
});
headingBox.addEventListener("change", () => {
  if (formatting()) format((s) => setHeading(s, Number(headingBox.value)));
  else refreshFormatBar();
});

// Files dropped where nothing takes them never open in the web view (it would leave the app).
const strayFiles = (e: DragEvent) => !e.defaultPrevented && (e.dataTransfer?.types.includes("Files") ?? false);
window.addEventListener("dragover", (e) => {
  if (!strayFiles(e)) return;
  e.preventDefault();
  e.dataTransfer!.dropEffect = "none";
});
window.addEventListener("drop", (e) => {
  if (strayFiles(e)) e.preventDefault();
});

// Ctrl+wheel never zooms the whole web view (the publisher view zooms its pages).
window.addEventListener(
  "wheel",
  (e) => {
    if (e.ctrlKey || e.metaKey) e.preventDefault();
  },
  { passive: false },
);

// Zotero's auto-export (or another editor) may have changed references.bib.
let focusRefreshed = 0;
window.addEventListener("focus", () => {
  if (!sync || Date.now() - focusRefreshed < 1000) return;
  focusRefreshed = Date.now();
  library.refresh().catch(showError);
});

void listen<string>("save-error", (e) => bar.error(e.payload));
void listen<CompileStatus>("compile-status", (e) => {
  if (sync) publisher.onStatus(e.payload);
});
void listen("save-ok", () => {
  bar.clearError();
  bar.info("Saving works again.");
});

/** Sends every edit before the app goes away (bounded, so closing never hangs). */
async function sendEdits(): Promise<void> {
  if (sync) await Promise.race([sync.drain(), new Promise((r) => setTimeout(r, 5000))]);
}

// The close button and File → Exit come here too.
void getCurrentWindow().onCloseRequested(async () => {
  publisher.setActive(null);
  await sendEdits();
});

/** The last project (or the start screen), then the window shows and the splash closes. */
async function finishStartup(): Promise<void> {
  startupProgress(90, "Opening your last project…");
  await settingsRead;
  let damaged: Opening | null = null;
  try {
    const last = await invoke<Opening | null>("project_last");
    if (last && "opened" in last) load(last.opened);
    else {
      showEmpty();
      damaged = last;
    }
  } catch (e) {
    showEmpty();
    showError(e);
  }
  await startupDone();
  checkUpdatesSoon();
  // Asked over the start screen, once the window shows.
  if (damaged) await switchProject(() => opening(damaged));
}

void finishStartup().catch((e) => {
  // Never leave the window hidden behind the splash.
  void getCurrentWindow().show();
  showError(e);
});
