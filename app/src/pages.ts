// Page preview math and the page image cache for the publisher view.
//
// Pure (no DOM, no Tauri): the view feeds in page sizes, zoom and scroll
// position; the cache gets its fetching and Blob URL handling injected, so
// both run under `node --test`.

/** CSS pixels per typographic point (100 % zoom shows pages at true size). */
export const PX_PER_PT = 96 / 72;
/** Zoom steps for Ctrl+Plus/Minus (1 = 100 %). */
export const ZOOM_STEPS = [0.5, 0.67, 0.75, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2];
export const MIN_ZOOM = ZOOM_STEPS[0];
export const MAX_ZOOM = ZOOM_STEPS[ZOOM_STEPS.length - 1];

/** "fit" (page width fills the preview) or a factor from 0.5 to 2. */
export type Zoom = "fit" | number;

/** Page width and height in points. */
export type PageSize = [number, number];

/** Spacing of the page column, in CSS pixels. */
export interface Metrics {
  /** Space above the first page and below the last. */
  padY: number;
  /** Space left and right of the widest page. */
  padX: number;
  /** Height of the page number label under each page. */
  label: number;
  /** Space between a label and the next page. */
  gap: number;
}

export const METRICS: Metrics = { padY: 24, padX: 24, label: 22, gap: 14 };

/** Where every page sits in the scrolled column, in CSS pixels. */
export interface PageLayout {
  scale: number;
  tops: Float64Array;
  widths: Float64Array;
  heights: Float64Array;
  /** Height of the whole column. */
  total: number;
  /** Width of the widest page plus padding. */
  width: number;
}

const A4: PageSize = [595.28, 841.89];

/** Pixels per point for `zoom`, given the preview's inner width. */
export function zoomScale(zoom: Zoom, sizes: readonly PageSize[], viewportWidth: number, m: Metrics = METRICS): number {
  if (zoom !== "fit") return clampZoom(zoom) * PX_PER_PT;
  let widest = 0;
  for (const [w] of sizes) widest = Math.max(widest, w);
  if (widest <= 0) widest = A4[0];
  const fit = (viewportWidth - 2 * m.padX) / widest;
  return Math.min(4 * PX_PER_PT, Math.max(0.1 * PX_PER_PT, fit));
}

export function clampZoom(z: number): number {
  return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, z));
}

/** The next zoom step above (`dir` 1) or below (-1) the current factor. */
export function stepZoom(current: number, dir: 1 | -1): number {
  const eps = 0.005;
  if (dir > 0) return ZOOM_STEPS.find((z) => z > current + eps) ?? MAX_ZOOM;
  for (let i = ZOOM_STEPS.length - 1; i >= 0; i--) if (ZOOM_STEPS[i] < current - eps) return ZOOM_STEPS[i];
  return MIN_ZOOM;
}

/** Wheel distance (CSS pixels) of one zoom step: a mouse notch, or a touchpad pinch adding up to it. */
export const WHEEL_STEP = 50;

/**
 * Ctrl+wheel zoom: adds `deltaY` (pixels) to the running `total` and says
 * which step to take (1: in, -1: out, 0: none yet). At most one step per
 * event, so a fast flick doesn't race through the zooms; turning the wheel
 * the other way starts over.
 */
export function wheelZoom(total: number, deltaY: number): { step: 1 | -1 | 0; total: number } {
  const sum = Math.sign(total) === Math.sign(deltaY) ? total + deltaY : deltaY;
  if (Math.abs(sum) < WHEEL_STEP) return { step: 0, total: sum };
  return { step: sum < 0 ? 1 : -1, total: 0 };
}

/** Lays out pages top to bottom at `scale` pixels per point. O(pages). */
export function layoutPages(sizes: readonly PageSize[], scale: number, m: Metrics = METRICS): PageLayout {
  const n = sizes.length;
  const tops = new Float64Array(n);
  const widths = new Float64Array(n);
  const heights = new Float64Array(n);
  let y = m.padY;
  let widest = 0;
  for (let i = 0; i < n; i++) {
    const [w, h] = sizes[i];
    tops[i] = y;
    widths[i] = Math.max(1, w * scale);
    heights[i] = Math.max(1, h * scale);
    widest = Math.max(widest, widths[i]);
    y += heights[i] + m.label + (i < n - 1 ? m.gap : 0);
  }
  return { scale, tops, widths, heights, total: y + m.padY, width: widest + 2 * m.padX };
}

/** The first page whose top is above `y` (0 if none). Binary search. */
export function pageAt(layout: PageLayout, y: number): number {
  const { tops } = layout;
  let lo = 0;
  let hi = tops.length - 1;
  if (hi < 0) return 0;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (tops[mid] <= y) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

/**
 * Pages (with their labels) overlapping the viewport extended by `overscan`
 * pixels above and below; inclusive, empty when `first > last`.
 */
export function visibleRange(layout: PageLayout, scrollTop: number, viewportHeight: number, overscan = 0, m: Metrics = METRICS): { first: number; last: number } {
  const n = layout.tops.length;
  if (n === 0) return { first: 0, last: -1 };
  const top = scrollTop - overscan;
  const bottom = scrollTop + viewportHeight + overscan;
  let first = pageAt(layout, top);
  if (layout.tops[first] + layout.heights[first] + m.label <= top && first < n - 1) first++;
  const last = pageAt(layout, bottom);
  if (layout.tops[first] >= bottom && first > 0) return { first, last: first - 1 };
  return { first, last: Math.max(first, last) };
}

/** Pages to fetch, most wanted first: visible ones top-down, then the overscan nearest-first. */
export function wantedPages(layout: PageLayout, scrollTop: number, viewportHeight: number, overscan: number, m: Metrics = METRICS): number[] {
  const seen = visibleRange(layout, scrollTop, viewportHeight, 0, m);
  const near = visibleRange(layout, scrollTop, viewportHeight, overscan, m);
  const out: number[] = [];
  for (let i = seen.first; i <= seen.last; i++) out.push(i);
  for (let d = 1; ; d++) {
    const below = seen.last + d;
    const above = seen.first - d;
    const more = below <= near.last || above >= near.first;
    if (!more) break;
    if (below <= near.last) out.push(below);
    if (above >= near.first) out.push(above);
  }
  return out;
}

/** The preview's scroll box: its client rectangle and scroll offsets. */
export interface Viewport {
  left: number;
  top: number;
  /** Inner width (pages are centred in `max(width, layout.width)`). */
  width: number;
  scrollLeft: number;
  scrollTop: number;
}

/**
 * The page under a pointer at (`clientX`, `clientY`) and the point on it in
 * typographic points from the page's top left; `null` between pages.
 */
export function pagePoint(layout: PageLayout, view: Viewport, clientX: number, clientY: number): { page: number; xPt: number; yPt: number } | null {
  if (layout.tops.length === 0) return null;
  const x = clientX - view.left + view.scrollLeft;
  const y = clientY - view.top + view.scrollTop;
  const page = pageAt(layout, y);
  const w = layout.widths[page];
  const h = layout.heights[page];
  // Same rounding as the page elements' placement.
  const left = Math.round((Math.max(view.width, layout.width) - w) / 2);
  const dx = x - left;
  const dy = y - layout.tops[page];
  if (dx < 0 || dx > w || dy < 0 || dy > h) return null;
  return { page, xPt: dx / layout.scale, yPt: dy / layout.scale };
}

/** A scroll position as "page `index`, `fraction` of its height down". */
export interface Anchor {
  index: number;
  fraction: number;
}

/** The page at the viewport's top edge and how far into it. */
export function anchorAt(layout: PageLayout, scrollTop: number): Anchor {
  if (layout.tops.length === 0) return { index: 0, fraction: 0 };
  const index = pageAt(layout, scrollTop);
  return { index, fraction: (scrollTop - layout.tops[index]) / layout.heights[index] };
}

/** The scroll position showing `anchor` in `layout` (pages may have changed size or number). */
export function scrollFor(layout: PageLayout, anchor: Anchor, viewportHeight = 0): number {
  const n = layout.tops.length;
  if (n === 0) return 0;
  const index = Math.min(Math.max(0, anchor.index), n - 1);
  const y = layout.tops[index] + anchor.fraction * layout.heights[index];
  return Math.max(0, Math.min(y, layout.total - viewportHeight));
}

// ---- Page image cache ----

export type PageBytes = ArrayBuffer | Uint8Array | number[];

export interface PageCacheDeps {
  /** One page of one compiled version (`page_svg`). */
  fetch(version: number, index: number): Promise<PageBytes>;
  /** Makes a Blob URL for SVG bytes. */
  toUrl(bytes: PageBytes): string;
  revoke(url: string): void;
  /** A page image arrived (show it). */
  onLoad(index: number): void;
  /** Fetching from the latest version failed; called once per version (call `compile_now`). */
  onFailure(version: number, error: unknown): void;
  concurrency?: number;
  /** Most page images kept (wanted pages are never evicted). */
  capacity?: number;
}

interface Shown {
  version: number;
  url: string;
}

/**
 * Page images by index as Blob URLs. A page keeps showing its previous
 * version's image until the latest version's arrives; replaced and evicted
 * URLs are revoked. Fetches run a few at a time, wanted pages first.
 */
export class PageCache {
  private deps: PageCacheDeps;
  private concurrency: number;
  private capacity: number;
  private version: number | null = null;
  private pages = 0;
  /** Insertion order is least recently used first. */
  private shown = new Map<number, Shown>();
  /** `version:index` of requests in flight. */
  private inFlight = new Set<string>();
  private wanted: number[] = [];
  /** Pages of the latest version that failed (not retried until the next status). */
  private failed = new Set<number>();
  private failureReported: number | null = null;

  constructor(deps: PageCacheDeps) {
    this.deps = deps;
    this.concurrency = deps.concurrency ?? 3;
    this.capacity = deps.capacity ?? 48;
  }

  /** The version whose pages are fetched now (`null`: none, e.g. after a crash). */
  get current(): number | null {
    return this.version;
  }

  get size(): number {
    return this.shown.size;
  }

  get loading(): number {
    return this.inFlight.size;
  }

  /** A compile result: pages now come from `version` (`null`: keep what is shown, fetch nothing). */
  setVersion(version: number | null, pages: number): void {
    this.version = version;
    this.failed.clear();
    if (version === null) return;
    this.pages = pages;
    for (const [index, s] of this.shown) {
      if (index >= pages) {
        this.deps.revoke(s.url);
        this.shown.delete(index);
      }
    }
    this.pump();
  }

  /** The URL to show for page `index` (any version), if loaded. */
  get(index: number): string | undefined {
    return this.shown.get(index)?.url;
  }

  /** Whether page `index` shows the latest version. */
  isCurrent(index: number): boolean {
    return this.version !== null && this.shown.get(index)?.version === this.version;
  }

  /** Fetches these pages (in this order) if they aren't current; replaces earlier wishes. */
  want(indices: number[]): void {
    this.wanted = indices;
    for (const i of indices) {
      const s = this.shown.get(i);
      if (s) {
        this.shown.delete(i);
        this.shown.set(i, s);
      }
    }
    this.pump();
  }

  /** Drops everything (e.g. another project), revoking every URL. */
  clear(): void {
    for (const s of this.shown.values()) this.deps.revoke(s.url);
    this.shown.clear();
    this.wanted = [];
    this.failed.clear();
    this.version = null;
    this.pages = 0;
    this.failureReported = null;
  }

  private needs(index: number): boolean {
    const v = this.version;
    return v !== null && index < this.pages && this.shown.get(index)?.version !== v && !this.failed.has(index) && !this.inFlight.has(`${v}:${index}`);
  }

  private pump(): void {
    for (const index of this.wanted) {
      if (this.inFlight.size >= this.concurrency) return;
      if (this.needs(index)) this.start(this.version!, index);
    }
  }

  private start(version: number, index: number): void {
    const key = `${version}:${index}`;
    this.inFlight.add(key);
    Promise.resolve()
      .then(() => this.deps.fetch(version, index))
      .then(
        (bytes) => {
          this.inFlight.delete(key);
          const shown = this.shown.get(index);
          // Pages of an older version may still beat an older image (or none).
          if (index >= this.pages || (shown && shown.version >= version)) return;
          const url = this.deps.toUrl(bytes);
          if (shown) this.deps.revoke(shown.url);
          this.shown.delete(index);
          this.shown.set(index, { version, url });
          this.evict();
          this.deps.onLoad(index);
        },
        (error: unknown) => {
          this.inFlight.delete(key);
          // An older version is gone: the latest one is fetched instead (pump).
          if (version !== this.version) return;
          this.failed.add(index);
          if (this.failureReported !== version) {
            this.failureReported = version;
            this.deps.onFailure(version, error);
          }
        },
      )
      .finally(() => this.pump());
  }

  private evict(): void {
    if (this.shown.size <= this.capacity) return;
    const keep = new Set(this.wanted);
    for (const [index, s] of this.shown) {
      if (this.shown.size <= this.capacity) break;
      if (keep.has(index)) continue;
      this.deps.revoke(s.url);
      this.shown.delete(index);
    }
  }
}
