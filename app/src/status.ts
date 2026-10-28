// Compile status, diagnostics and undo messages for the publisher view.
// Pure: no DOM, no Tauri (tested under `node --test`).

import type { Reply } from "./sync.ts";

/** Where a diagnostic points (`Location` in crates/typeset, tagged by `in`). */
export type Location =
  | { in: "content"; start: number; end: number; start_utf16: number; end_utf16: number }
  | { in: "custom"; start: number; end: number }
  | { in: "preamble" }
  | { in: "file"; path: string }
  | { in: "override"; id: string }
  | { in: "unknown" };

export interface Diagnostic {
  severity: "error" | "warning";
  message: string;
  hints: string[];
  location: Location;
}

/** The `compile-status` event. */
export interface CompileStatus {
  state: "compiling" | "done" | "failed" | "timeout" | "crashed";
  /** The document version compiled; diagnostics refer to its text. */
  version: number;
  /** The version whose pages `page_svg` serves. */
  pages_version: number | null;
  pages: number;
  page_sizes: [number, number][];
  diagnostics: Diagnostic[];
  /** What became of each override in `version` (empty while compiling). */
  overrides?: OverrideStatus[];
  duration_ms: number;
  message?: string | null;
}

/** One override in a `compile-status` event. */
export interface OverrideStatus {
  id: string;
  /** Injected; `false`: not applicable at its position (see the diagnostics). */
  applied: boolean;
  detached: boolean;
  /**
   * Where it is on the pages, if applied and this compile produced them
   * (`version === pages_version`): page (0-based), then points from its top
   * left corner (a figure's box; a space's top and height).
   */
  page?: number;
  x_pt?: number;
  y_pt?: number;
  width_pt?: number;
  height_pt?: number;
}

export type Tone = "busy" | "ok" | "warn" | "error";

const plural = (n: number, one: string, many = `${one}s`) => `${n.toLocaleString("en-US")} ${n === 1 ? one : many}`;

export function countDiagnostics(diagnostics: readonly Diagnostic[]): { errors: number; warnings: number } {
  let errors = 0;
  for (const d of diagnostics) if (d.severity === "error") errors++;
  return { errors, warnings: diagnostics.length - errors };
}

/** "2 errors · 1 warning" (empty without diagnostics). */
export function diagnosticsSummary(diagnostics: readonly Diagnostic[]): string {
  const { errors, warnings } = countDiagnostics(diagnostics);
  const parts: string[] = [];
  if (errors) parts.push(plural(errors, "error"));
  if (warnings) parts.push(plural(warnings, "warning"));
  return parts.join(" · ");
}

/** The publisher status line for a compile status. */
export function compileStatusText(s: CompileStatus): { text: string; tone: Tone } {
  switch (s.state) {
    case "compiling":
      return { text: "Typesetting…", tone: "busy" };
    case "done": {
      const { warnings } = countDiagnostics(s.diagnostics);
      const base = `${plural(s.pages, "page")} · ${s.duration_ms.toLocaleString("en-US")} ms`;
      return warnings ? { text: `${base} · ${plural(warnings, "warning")}`, tone: "warn" } : { text: base, tone: "ok" };
    }
    case "failed": {
      const summary = diagnosticsSummary(s.diagnostics) || "Typesetting failed";
      const kept = s.pages_version !== null && s.pages > 0 ? " · showing the last good version" : "";
      return { text: `${summary}${kept}`, tone: "error" };
    }
    case "timeout":
      return { text: s.message || "Typesetting took too long and was stopped.", tone: "error" };
    case "crashed":
      return { text: s.message || "The typesetter stopped unexpectedly.", tone: "error" };
  }
}

// ---- Diagnostic locations ----

/** The text range a diagnostic in the content points to, clamped to the current text. */
export function contentRange(loc: { start_utf16: number; end_utf16: number }, docLength: number): { from: number; to: number } {
  const clamp = (n: number) => Math.min(Math.max(0, Number.isFinite(n) ? n : 0), docLength);
  const from = clamp(loc.start_utf16);
  return { from, to: Math.max(from, clamp(loc.end_utf16)) };
}

/** The UTF-16 offset of byte offset `byte` (UTF-8) in `text`, clamped to the text. */
export function utf8ToUtf16(text: string, byte: number): number {
  let bytes = 0;
  for (let i = 0; i < text.length; ) {
    if (bytes >= byte) return i;
    const cp = text.codePointAt(i)!;
    bytes += cp < 0x80 ? 1 : cp < 0x800 ? 2 : cp < 0x10000 ? 3 : 4;
    i += cp > 0xffff ? 2 : 1;
  }
  return text.length;
}

/** A byte range of the expert code as a UTF-16 selection in `text`. */
export function customRange(loc: { start: number; end: number }, text: string): { from: number; to: number } {
  const from = utf8ToUtf16(text, loc.start);
  return { from, to: Math.max(from, utf8ToUtf16(text, loc.end)) };
}

/** What clicking a diagnostic does. */
export type Target =
  | { kind: "content"; from: number; to: number; outdated: boolean }
  | { kind: "custom"; from: number; to: number }
  | { kind: "override"; id: string }
  | { kind: "none" };

/**
 * Where a diagnostic leads: content ranges are clamped to the current text
 * (and flagged when the document changed since the compiled version).
 */
export function diagnosticTarget(d: Diagnostic, ctx: { compiledVersion: number; docVersion: number; docLength: number; customText: string }): Target {
  const loc = d.location;
  if (loc.in === "content") {
    return { kind: "content", ...contentRange(loc, ctx.docLength), outdated: ctx.compiledVersion !== ctx.docVersion };
  }
  if (loc.in === "custom") return { kind: "custom", ...customRange(loc, ctx.customText) };
  if (loc.in === "override") return { kind: "override", id: loc.id };
  return { kind: "none" };
}

/**
 * A short place name for the diagnostics list; `overrideLabel` names an
 * override by id ("Space before 18pt").
 */
export function locationLabel(loc: Location, lineOf?: (utf16: number) => number, overrideLabel?: (id: string) => string | null): string {
  switch (loc.in) {
    case "content":
      return lineOf ? `Line ${lineOf(loc.start_utf16)}` : "Text";
    case "custom":
      return "Expert code";
    case "preamble":
      return "Layout settings";
    case "file":
      return loc.path;
    case "override": {
      const label = overrideLabel?.(loc.id);
      return label ? `Exception: ${label}` : "Exception";
    }
    case "unknown":
      return "";
  }
}

// ---- Undo messages ----

/**
 * The status message after undo/redo, and whether the layout changed. A
 * step that changed the asset index is named as an asset change (and
 * reloads like a layout step); another reply without text changes was a
 * layout step; text steps are named only in the publisher view, where the
 * text is not visible.
 */
export function undoMessage(kind: "undo" | "redo", reply: Reply | null, publisher: boolean): { message: string | null; layout: boolean } {
  const verb = kind === "undo" ? "Undid" : "Redid";
  if (!reply) return { message: publisher ? `Nothing to ${kind}` : null, layout: false };
  if (reply.assets) return { message: `${verb} an asset change`, layout: true };
  if (reply.changes.length === 0) return { message: `${verb} a layout change`, layout: true };
  return { message: publisher ? `${verb} a text change` : null, layout: false };
}
