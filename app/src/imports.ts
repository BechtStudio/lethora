// File → Import… and Edit → Insert file…, the pure parts: what `import_read`
// answers, where the imported blocks go in the text, and the notice after
// an import. The conversion itself is the Rust side's (`crates/convert`).

import type { Text } from "@codemirror/state";

/** A file dialog filter (`import_formats`). */
export interface ImportFilter {
  name: string;
  extensions: string[];
}

/** What was dropped or approximated, one per kind (`kind` is a stable code). */
export interface ImportWarning {
  kind: string;
  count: number;
  example: string | null;
  message: string;
}

/** `import_read`'s answer. */
export interface ImportReply {
  /** The format's name ("Markdown"). */
  format: string;
  /** Lethora markup, ending in a line break (empty if nothing came out). */
  text: string;
  /** Its images, staged for `figure_insert` (empty without staging). */
  assets: Array<{ path: string; blob: string }>;
  warnings: ImportWarning[];
  title: string | null;
  author: string | null;
}

/** The file name of `path` (either separator). */
export const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;

/** The file name of `path` without its extension: a title when the file has none. */
export const fileStem = (path: string) => fileName(path).replace(/\.[^.]*$/, "") || fileName(path);

/**
 * Where imported blocks go for a cursor at `pos`: there on a blank line or
 * at the start of a block, else at the end of the cursor's paragraph (up to
 * the next blank line), so no word, label or call is split.
 */
export function importPos(doc: Text, pos: number): number {
  const line = doc.lineAt(pos);
  if (!line.text.trim()) return pos;
  if (!line.text.slice(0, pos - line.from).trim() && (line.number === 1 || !doc.line(line.number - 1).text.trim())) return pos;
  let n = line.number;
  while (n < doc.lines && doc.line(n + 1).text.trim()) n++;
  return doc.line(n).to;
}

/**
 * The imported `markup` placed at `pos` in `doc` as blocks of their own:
 * blank lines added towards neighbouring text (as figures are inserted).
 */
export function importText(doc: Text, pos: number, markup: string): string {
  const body = markup.replace(/\n+$/, "");
  if (!body) return "";
  const line = doc.lineAt(pos);
  const blank = (n: number) => n < 1 || n > doc.lines || !doc.line(n).text.trim();
  const before = line.text.slice(0, pos - line.from).trim() ? "\n\n" : blank(line.number - 1) ? "" : "\n";
  const after = line.text.slice(pos - line.from).trim() ? "\n\n" : pos === doc.length ? "\n" : blank(line.number + 1) ? "" : "\n";
  return before + body + after;
}

/** The passing message after importing `name`: what came of it, then the warnings. */
export function importNotice(name: string, reply: ImportReply, suggested = false): string {
  if (!reply.text) return `Nothing to import in ${name}.`;
  const done = suggested
    ? `Suggested the content of ${name}${reply.warnings.some((w) => w.kind === "image-left-out") ? " (images can't be suggested)" : ""}.`
    : `Imported ${name}.`;
  return [done, ...reply.warnings.map((w) => w.message)].join(" ");
}
