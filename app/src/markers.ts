// Exception markers: where each applied override sits on the compiled pages
// of the publisher view (a dashed rule for a break, a bracket for a space, an
// outline for a placed figure, an image with its own quality or a table
// with its own style), each with a chip naming it.
//
// Pure (no DOM): positions come from the `compile-status` event, the view
// scales the boxes and draws them (tested under `node --test`).

import { type OverrideInfo, type OverrideKind, type Placement, qualityLabel, tableStyleLabel } from "./exceptions.ts";
import type { PageSize } from "./pages.ts";
import type { CompileStatus, OverrideStatus } from "./status.ts";

/** A marker on page `page`: a box in points from the page's top left corner. */
export interface Marker {
  id: string;
  kind: OverrideKind;
  page: number;
  x: number;
  y: number;
  w: number;
  h: number;
  chip: string;
  detached: boolean;
}

/** Length of a column break's rule, in points. */
export const COLUMN_RULE = 48;
/** How far left of the text a space's bracket is, and its width, in points. */
export const BRACKET_GAP = 10;
export const BRACKET_W = 5;
/** Shortest page-break rule, in points. */
const MIN_RULE = 40;

const PLACEMENT_CHIPS: Record<Placement, string> = { top: "Top", bottom: "Bottom", here: "Here", auto: "Auto" };

/** The chip of a marker: "Page break", "Right-hand page", "Column break", the space's length, the placement, "Image: Screen", "Table: Grid". */
export function chipText(o: Pick<OverrideInfo, "kind" | "params">): string {
  switch (o.kind) {
    case "page_break":
      return o.params.to_odd === true ? "Right-hand page" : "Page break";
    case "column_break":
      return "Column break";
    case "space_before":
      return String(o.params.length ?? "");
    case "figure_placement":
      return PLACEMENT_CHIPS[o.params.placement as Placement] ?? "Figure";
    case "image_quality":
      return `Image: ${qualityLabel(o.params.quality)}`;
    case "table_style":
      return `Table: ${tableStyleLabel(o.params.style)}`;
    default:
      return "";
  }
}

/**
 * The marker of override `o` placed at `s` on a page of `size`, or `null`
 * (not applied, not located). A page break's rule spans the text area,
 * taken as symmetric around the block's left edge.
 */
export function markerOf(o: OverrideInfo, s: OverrideStatus, size: PageSize | undefined): Marker | null {
  if (!o.kind || !s.applied || s.page == null || s.x_pt == null || s.y_pt == null || !size) return null;
  const base = { id: o.id, kind: o.kind, page: s.page, x: s.x_pt, y: s.y_pt, w: 0, h: 0, chip: chipText(o), detached: s.detached };
  switch (o.kind) {
    case "page_break":
      return { ...base, w: Math.max(MIN_RULE, size[0] - 2 * s.x_pt) };
    case "column_break":
      return { ...base, w: Math.max(0, Math.min(COLUMN_RULE, size[0] - s.x_pt)) };
    case "space_before":
      return { ...base, x: Math.max(0, s.x_pt - BRACKET_GAP - BRACKET_W), w: BRACKET_W, h: s.height_pt ?? 0 };
    case "figure_placement":
    case "image_quality":
    case "table_style":
      return { ...base, w: s.width_pt ?? 0, h: s.height_pt ?? 0 };
  }
}

/** The markers of `positions` for the current `overrides` (removed ones drop out), by page. */
export function markersByPage(positions: readonly OverrideStatus[], overrides: readonly OverrideInfo[], sizes: readonly PageSize[]): Map<number, Marker[]> {
  const byId = new Map(overrides.map((o) => [o.id, o]));
  const out = new Map<number, Marker[]>();
  for (const s of positions) {
    const o = byId.get(s.id);
    const m = o && s.page != null ? markerOf(o, s, sizes[s.page]) : null;
    if (!m) continue;
    const list = out.get(m.page) ?? [];
    list.push(m);
    out.set(m.page, list);
  }
  return out;
}

/** A marker's box in CSS pixels at `scale` pixels per point. */
export function scaleBox(m: Pick<Marker, "x" | "y" | "w" | "h">, scale: number): { left: number; top: number; width: number; height: number } {
  return { left: m.x * scale, top: m.y * scale, width: m.w * scale, height: m.h * scale };
}

/**
 * The positions of the last compile that produced pages, kept with that
 * version: a failed compile keeps them, as the view keeps its pages.
 */
export class MarkerPositions {
  private version: number | null = null;
  private list: OverrideStatus[] = [];

  /** A `compile-status` event; true if the positions changed. */
  onStatus(s: CompileStatus): boolean {
    if (s.state !== "done" || s.pages_version === null || s.pages_version !== s.version) return false;
    this.version = s.version;
    this.list = (s.overrides ?? []).filter((o) => o.applied && o.page != null);
    return true;
  }

  /** The positions on the pages of `pagesVersion` (none if they are from another compile). */
  for(pagesVersion: number | null): readonly OverrideStatus[] {
    return pagesVersion !== null && pagesVersion === this.version ? this.list : [];
  }

  /** The position of override `id` on the pages of `pagesVersion`. */
  find(id: string, pagesVersion: number | null): OverrideStatus | undefined {
    return this.for(pagesVersion).find((s) => s.id === id);
  }

  clear(): void {
    this.version = null;
    this.list = [];
  }
}
