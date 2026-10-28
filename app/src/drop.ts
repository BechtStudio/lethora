// Drag & drop into the author and science editors: files from the desktop
// and items from the assets panel become figures, each its own block at the
// nearest paragraph boundary (the start or end of the text, or a blank line
// between blocks in the document's own markup: never inside a block, a
// call's arguments, math, raw text or a comment), shown by a marker line
// while dragging. The pure parts (boundaries, what a drag carries, which
// files an asset type takes) are tested under `node --test`. No Tauri
// imports: the drop is handed to the host.

import type { EditorState, Extension, Text } from "@codemirror/state";
import { EditorView, ViewPlugin } from "@codemirror/view";
import { ASSET_DRAG_TYPE, FOLDER_DRAG_TYPE } from "./assets.ts";
import { ASSET_TYPES, type AssetType, assetKind } from "./assettypes.ts";
import { isAssetPath } from "./figures.ts";
import { topLevelAt } from "./typst.ts";

// ---- Boundaries ----

/** How many lines a boundary is looked for on either side of the pointer. */
const MAX_SCAN = 2000;
const BLANK = /^\s*$/;

/**
 * The paragraph boundaries around `pos`: the last at or before its line and
 * the first after it (null if none within `MAX_SCAN` lines). A boundary is
 * the start of the text, the start of the first blank line after a block
 * where `topLevel` (the line starts in the document's own markup, see
 * `topLevelAt`) holds, or the end of the text.
 */
export function boundariesAround(doc: Text, pos: number, topLevel: (lineFrom: number) => boolean): { before: number | null; after: number | null } {
  const at = doc.lineAt(Math.max(0, Math.min(pos, doc.length))).number;
  const gap = (n: number) => {
    const line = doc.line(n);
    return BLANK.test(line.text) && !BLANK.test(doc.line(n - 1).text) && topLevel(line.from);
  };
  let before: number | null = null;
  for (let n = at; n >= Math.max(1, at - MAX_SCAN); n--) {
    if (n === 1 || gap(n)) {
      before = doc.line(n).from;
      break;
    }
  }
  let after: number | null = null;
  const last = Math.min(doc.lines, at + MAX_SCAN);
  for (let n = at + 1; n <= last && after === null; n++) if (gap(n)) after = doc.line(n).from;
  if (after === null && last === doc.lines) after = doc.length;
  return { before, after };
}

/** The boundary around `pos` (see `boundariesAround`) that `distance` puts nearest, or null if none. */
export function nearestBoundary(doc: Text, pos: number, topLevel: (lineFrom: number) => boolean, distance: (boundary: number) => number): number | null {
  const { before, after } = boundariesAround(doc, pos, topLevel);
  if (before === null || after === null) return before ?? after;
  return distance(after) < distance(before) ? after : before;
}

/** The boundary nearest to `pos` in the text (by characters): where a drop at `pos` goes in after the text changed. */
export function boundaryAt(state: EditorState, pos: number): number {
  const at = Math.max(0, Math.min(pos, state.doc.length));
  return nearestBoundary(state.doc, at, (from) => topLevelAt(state, from), (b) => Math.abs(b - at)) ?? at;
}

// ---- What a drag carries ----

/** Files from the desktop, or the path of an item dragged from the assets panel. */
export type Dropped = { files: File[] } | { path: string };

/** What a drag can drop into the text, from its types (all a dragover sees): files, a panel item (not a folder or the Sources), or nothing. */
export function dragContent(types: readonly string[]): "files" | "asset" | null {
  if (types.includes("Files")) return "files";
  if (types.includes(ASSET_DRAG_TYPE) && !types.includes(FOLDER_DRAG_TYPE)) return "asset";
  return null;
}

/** What a drop brings into the text: the files, or a panel item's asset path; null for anything else (folders, text). */
export function droppedContent(data: { readonly types: readonly string[]; readonly files: ArrayLike<File>; getData(type: string): string }): Dropped | null {
  const kind = dragContent(data.types);
  if (kind === "files") return data.files.length ? { files: Array.from(data.files) } : null;
  const path = kind === "asset" ? data.getData(ASSET_DRAG_TYPE) : "";
  return isAssetPath(path) ? { path } : null;
}

// ---- Supported files ----

/** The dropped files an asset type takes (by extension in any case, up to its size), in order, and the names of the others. */
export function supportedFiles<F extends { name: string; size: number }>(files: readonly F[]): { ok: F[]; skipped: string[] } {
  const ok: F[] = [];
  const skipped: string[] = [];
  for (const file of files) {
    const kind = assetKind(file.name.toLowerCase());
    const type = kind ? (ASSET_TYPES as Record<string, AssetType>)[kind] : undefined;
    if (type && file.size <= type.maxBytes) ok.push(file);
    else skipped.push(file.name);
  }
  return { ok, skipped };
}

/** The one message naming skipped files (the first three, then how many more). */
export function skippedMessage(names: readonly string[]): string {
  const shown = names.slice(0, 3);
  const more = names.length - shown.length;
  const list = more > 0 ? `${shown.join(", ")} and ${more} more` : shown.length > 1 ? `${shown.slice(0, -1).join(", ")} and ${shown.at(-1)}` : shown[0];
  const types = Object.values(ASSET_TYPES) as AssetType[];
  const extensions = types.flatMap((t) => t.extensions);
  const mb = Math.max(...types.map((t) => t.maxBytes)) / (1024 * 1024);
  return `Skipped ${list}: only ${extensions.slice(0, -1).join(", ")} and ${extensions.at(-1)} files up to ${mb} MB can be added.`;
}

// ---- Editor extension ----

/**
 * Drops into the editor: while files or a panel item are dragged over it, a
 * marker line shows the nearest paragraph boundary (by height on screen);
 * on the drop, `onDrop` gets what was dropped and that boundary. Other drags
 * (text) are left to CodeMirror.
 */
export function figureDrops(onDrop: (dropped: Dropped, boundary: number) => void): Extension {
  return ViewPlugin.fromClass(
    class {
      view: EditorView;
      marker: HTMLElement | null = null;
      listeners: Array<[string, (e: DragEvent) => void]>;

      constructor(view: EditorView) {
        this.view = view;
        // On the scroller (margins included), before CodeMirror's own drop handling.
        this.listeners = [
          ["dragover", (e) => this.over(e)],
          ["dragleave", (e) => {
            if (!view.scrollDOM.contains(e.relatedTarget as Node | null)) this.hide();
          }],
          ["drop", (e) => this.drop(e)],
        ];
        for (const [type, f] of this.listeners) view.scrollDOM.addEventListener(type, f as EventListener, true);
      }

      /** The boundary nearest to the pointer, or null if this drag can't drop here. */
      boundary(e: DragEvent): number | null {
        if (!e.dataTransfer || !dragContent(e.dataTransfer.types)) return null;
        const { view } = this;
        const pos = view.posAtCoords({ x: e.clientX, y: e.clientY }, false);
        const y = e.clientY - view.documentTop;
        return nearestBoundary(view.state.doc, pos, (from) => topLevelAt(view.state, from), (b) => Math.abs(this.markerY(b) - y));
      }

      /** The marker's height in the document for `boundary`: a blank line's middle, the top of the text or its bottom. */
      markerY(boundary: number): number {
        const line = this.view.state.doc.lineAt(boundary);
        const block = this.view.lineBlockAt(boundary);
        if (BLANK.test(line.text)) return block.top + block.height / 2;
        return boundary === line.from ? block.top : block.bottom;
      }

      over(e: DragEvent): void {
        const b = this.boundary(e);
        if (b === null) {
          this.hide();
          return;
        }
        e.preventDefault();
        e.stopPropagation();
        e.dataTransfer!.dropEffect = "copy";
        this.show(b);
      }

      drop(e: DragEvent): void {
        const b = this.boundary(e);
        this.hide();
        if (b === null || !e.dataTransfer) return;
        e.preventDefault();
        e.stopPropagation();
        const dropped = droppedContent(e.dataTransfer);
        if (dropped) onDrop(dropped, b);
      }

      show(boundary: number): void {
        const { view } = this;
        if (!this.marker) {
          this.marker = view.scrollDOM.appendChild(document.createElement("div"));
          this.marker.className = "cm-drop-marker";
        }
        const outer = view.scrollDOM.getBoundingClientRect();
        const content = view.contentDOM.getBoundingClientRect();
        const style = this.marker.style;
        style.left = `${content.left - outer.left + view.scrollDOM.scrollLeft}px`;
        style.width = `${content.width}px`;
        style.top = `${view.documentTop + this.markerY(boundary) - outer.top + view.scrollDOM.scrollTop}px`;
      }

      hide(): void {
        this.marker?.remove();
        this.marker = null;
      }

      destroy(): void {
        this.hide();
        for (const [type, f] of this.listeners) this.view.scrollDOM.removeEventListener(type, f as EventListener, true);
      }
    },
  );
}
