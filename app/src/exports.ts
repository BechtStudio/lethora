// File → Export…, the pure parts: the save dialog's filters (the formats
// the Rust side writes, PDF after EPUB), the format a chosen path asks for, the
// default file name and the notice after an export. The conversion itself
// is the Rust side's (`crates/typeset/src/export.rs`, `crates/convert`).

import { fileName, type ImportWarning } from "./imports.ts";

/** A save dialog filter with its format's id (`export_formats`). */
export interface ExportFilter {
  id: string;
  name: string;
  extensions: string[];
}

/** PDF: the existing Export PDF, offered in the same dialog. */
export const PDF_FILTER: ExportFilter = { id: "pdf", name: "PDF", extensions: ["pdf"] };

/** The dialog's filters: the writers (Word and EPUB first) with PDF right after EPUB, else last. */
export function exportFilters(writers: readonly ExportFilter[]): ExportFilter[] {
  const at = writers.findIndex((w) => w.id === "epub") + 1 || writers.length;
  return [...writers.slice(0, at), PDF_FILTER, ...writers.slice(at)];
}

/** A file name (without extension) from `title`, without what file systems refuse; "document" if nothing is left. */
export function baseName(title: string): string {
  return (
    title
      .replace(/[\\/:*?"<>|\u0000-\u001f]+/g, " ")
      .replace(/\s+/g, " ")
      .trim()
      .slice(0, 120)
      .trim() || "document"
  );
}

/**
 * The format the chosen `path` asks for by its extension (any case), and
 * the path to write; a path without an extension gets `fallback`'s. Null
 * for an extension no filter has.
 */
export function exportTarget(path: string, filters: readonly ExportFilter[], fallback: ExportFilter = filters[0]): { path: string; format: string } | null {
  const ext = /\.([^.\\/]+)$/.exec(fileName(path))?.[1]?.toLowerCase();
  if (!ext) return { path: `${path}.${fallback.extensions[0]}`, format: fallback.id };
  const filter = filters.find((f) => f.extensions.includes(ext));
  return filter ? { path, format: filter.id } : null;
}

/** The passing message after exporting to `path`: the file, then what was dropped or approximated. */
export function exportNotice(path: string, warnings: readonly ImportWarning[]): string {
  return [`Exported ${fileName(path)}.`, ...warnings.map((w) => w.message)].join(" ");
}
