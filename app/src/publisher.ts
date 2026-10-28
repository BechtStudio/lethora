// The publisher view: a virtualized page preview of the compiled document,
// its status and diagnostics, next to the layout panel.
//
// Only pages in or near the viewport exist in the DOM and fetch their image
// (`page_svg` → Blob URL in an <img>, see PageCache). The scroll position is
// kept as "page + fraction" across recompiles and zoom changes.
//
// The same view shows in the Publisher tab or, without the layout panel, in
// the split pane beside the editor (its own zoom; a click on a page selects
// that spot in the text).
//
// Right-clicking a page opens a menu of exceptions (anchored overrides) for
// the clicked block or figure; the Exceptions drawer lists them
// (exceptionsdrawer.ts; the layout panel shows a summary). Markers on the
// pages show where each one is (see markers.ts); they can be hidden. A click
// on a marker (or the menu's "Edit…") edits it in a popover (exceptionedit.ts).
//
// No Tauri imports: commands come in through `PublisherHost`.

import { type Box, ExceptionEdit } from "./exceptionedit.ts";
import { ExceptionsDrawer } from "./exceptionsdrawer.ts";
import { ExceptionsSummary, type ImageQuality, type JumpResult, type OverrideApi, OverrideStore, QUALITIES, TABLE_STYLES, type TableStyle } from "./exceptions.ts";
import { LayoutPanel, type LayoutHost, valueOf } from "./layoutpanel.ts";
import { type Marker, MarkerPositions, markersByPage, scaleBox } from "./markers.ts";
import { type Act, type MenuContext, type MenuHandlers, menuModel, type MenuModel, PageMenu } from "./pagemenu.ts";
import {
  anchorAt,
  layoutPages,
  PageCache,
  type PageBytes,
  type PageLayout,
  pagePoint,
  type PageSize,
  scrollFor,
  stepZoom,
  visibleRange,
  wantedPages,
  WHEEL_STEP,
  wheelZoom,
  type Zoom,
  zoomScale,
  PX_PER_PT,
} from "./pages.ts";
import { type CompileStatus, compileStatusText, type OverrideStatus, type Diagnostic, diagnosticTarget, diagnosticsSummary, locationLabel, type Tone } from "./status.ts";

export interface PublisherHost {
  watch(active: boolean): Promise<void>;
  compileNow(): Promise<void>;
  pageSvg(version: number, index: number): Promise<PageBytes>;
  /** The core's document version now (after `drain`). */
  docVersion(): Promise<number>;
  drain(): Promise<void>;
  docLength(): number;
  /** 1-based line of a UTF-16 offset in the current text. */
  lineOf(utf16: number): number;
  /** The line holding a UTF-16 offset in the current text. */
  lineAt(utf16: number): { from: number; text: string };
  /** Where a click at (`xPt`, `yPt`) on page `page` (0-based) of compiled `version` leads. */
  pageJump(version: number, page: number, xPt: number, yPt: number): Promise<JumpResult>;
  /** The override commands (`drain` sends pending edits first). */
  overrides: OverrideApi;
  /** Asks for an image file and replaces the image of the image call at `pos` (reading asset `path`) with it. */
  replaceImage(path: string, pos: number): Promise<void>;
  /** Shows a text range in the editor (the Publisher tab switches to the science view). */
  revealText(from: number, to: number, outdated: boolean): void;
  setStatus(text: string, tone: Tone | ""): void;
  /** The zoom label of the Publisher tab's or the split pane's zoom. */
  setZoomLabel(text: string, place: PubPlace): void;
  /** The markers toggle shows `on`. */
  setMarkersShown(on: boolean): void;
  /** The Exceptions drawer's buttons show it open or closed. */
  setDrawerShown(open: boolean): void;
  /** The current text (a new object per change). */
  doc(): { toString(): string };
  error(e: unknown): void;
}

/** Where the pages show: the Publisher tab or the split pane beside the editor. */
export type PubPlace = "tab" | "split";

/** Each place's zoom is remembered on its own. */
const ZOOM_KEYS: Record<PubPlace, string> = { tab: "lethora.zoom", split: "lethora.splitZoom" };

/** A point in the preview, in CSS pixels from its top left. */
type Point = { x: number; y: number };

const MAX_DIAGNOSTICS = 200;
/** How long a marker stays highlighted after its row was selected. */
const FLASH_MS = 1500;

const byId = (id: string) => document.getElementById(id)!;

interface PageEl {
  el: HTMLElement;
  img: HTMLImageElement;
  label: HTMLElement;
  src: string;
  /** The exception markers' overlay, drawn for `marksKey`. */
  marks: HTMLElement;
  marksKey: string;
}

export class Publisher {
  readonly panel: LayoutPanel;
  readonly overrides: OverrideStore;
  private drawer: ExceptionsDrawer;
  private menu = new PageMenu();
  private edit: ExceptionEdit;
  /** Where the page menu last opened (the edit popover's spot if the marker is not shown). */
  private menuAt: Point = { x: 0, y: 0 };
  /** The override edited when the last pointerdown came (which closed the popover). */
  private editWas: string | null = null;
  /** Bumped per right-click: a slower answer for an earlier click is dropped. */
  private menuToken = 0;
  private host: PublisherHost;
  private section = byId("publisher");
  private scroller = byId("pub-scroll");
  private sizer = byId("pub-sizer");
  private placeholder = byId("pub-placeholder");
  private diags = byId("pub-diags") as HTMLDetailsElement;
  private diagsSummary = byId("pub-diags-summary");
  private diagsList = byId("pub-diags-list");
  private cache: PageCache;
  private active = false;
  /** Where the pages show (or last showed). */
  private where: PubPlace = "tab";
  private sizes: PageSize[] = [];
  private layout: PageLayout = layoutPages([], 1);
  private zooms: Record<PubPlace, Zoom> = { tab: loadZoom(ZOOM_KEYS.tab), split: loadZoom(ZOOM_KEYS.split) };
  /** Ctrl+wheel distance not yet turned into a zoom step. */
  private wheelTotal = 0;
  private els = new Map<number, PageEl>();
  private frame = 0;
  private status: CompileStatus | null = null;
  /** Diagnostics shown, with the version they refer to. */
  private shownDiagnostics: { version: number; list: Diagnostic[] } | null = null;
  private resize: ResizeObserver;
  private lastWidth = 0;
  private positions = new MarkerPositions();
  /** Markers of the pages shown, by page. */
  private markers = new Map<number, Marker[]>();
  private markersShown = loadMarkersShown();
  /** Bumped when the markers change: page overlays are redrawn. */
  private markerStamp = 0;
  /** The highlighted marker (its row was just selected). */
  private flash: string | null = null;
  private flashTimer = 0;
  /** `pageNumbers` for these positions. */
  private pagesOf: { list: readonly OverrideStatus[]; map: Map<string, number> } = { list: [], map: new Map() };

  constructor(host: PublisherHost, layoutHost: LayoutHost) {
    this.host = host;
    this.panel = new LayoutPanel(byId("layout-panel"), layoutHost);
    this.overrides = new OverrideStore(host.overrides);
    // Selects the anchored line, so the spot is visible.
    const openInText = (pos: number) => {
      const line = host.lineAt(pos);
      host.revealText(pos, Math.max(pos, line.from + line.text.length), false);
    };
    const toPages = () => this.scroller.focus({ preventScroll: true });
    this.drawer = new ExceptionsDrawer(
      this.overrides,
      {
        doc: () => host.doc(),
        pages: () => this.pageNumbers(),
        showOnPage: (id) => this.showMarker(id),
        openInText,
        shown: (open) => host.setDrawerShown(open),
        closed: toPages,
        error: host.error,
      },
      this.section,
    );
    this.edit = new ExceptionEdit(this.overrides, { openInText, closed: toPages, error: host.error }, this.section);
    this.panel.addSection(new ExceptionsSummary(this.overrides, () => this.drawer.show()).el);
    // Diagnostics name overrides by their label.
    this.overrides.subscribe(() => {
      if (this.shownDiagnostics?.list.some((d) => d.location.in === "override")) this.renderDiagnostics();
      this.updateMarkers();
    });
    this.scroller.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      void this.openMenu(e).catch(host.error);
    });
    this.cache = new PageCache({
      fetch: (version, index) => host.pageSvg(version, index),
      toUrl: (bytes) => {
        const data = bytes instanceof ArrayBuffer || ArrayBuffer.isView(bytes) ? bytes : new Uint8Array(bytes);
        return URL.createObjectURL(new Blob([data as BlobPart], { type: "image/svg+xml" }));
      },
      revoke: (url) => URL.revokeObjectURL(url),
      onLoad: (index) => this.showImage(index),
      onFailure: () => void host.compileNow().catch(host.error),
    });
    // In the split pane a click on a page selects that spot in the text,
    // unless it only closed the page menu or the edit popover (they close on pointerdown).
    let menuWasOpen = false;
    window.addEventListener(
      "pointerdown",
      () => {
        menuWasOpen = this.menu.isOpen || this.edit.isOpen;
        this.editWas = this.edit.current;
      },
      true,
    );
    this.scroller.addEventListener("click", (e) => {
      if (menuWasOpen || this.where !== "split" || e.button !== 0 || e.ctrlKey || e.metaKey || e.shiftKey || e.altKey) return;
      void this.openText(e.clientX, e.clientY).catch(host.error);
    });
    this.scroller.addEventListener("scroll", () => this.schedule(), { passive: true });
    // Ctrl+wheel (or a touchpad pinch) zooms by steps around the pointer.
    this.scroller.addEventListener(
      "wheel",
      (e) => {
        if (!(e.ctrlKey || e.metaKey)) return;
        e.preventDefault();
        const r = wheelZoom(this.wheelTotal, e.deltaMode === 0 ? e.deltaY : Math.sign(e.deltaY) * WHEEL_STEP);
        this.wheelTotal = r.total;
        if (r.step === 0) return;
        const rect = this.scroller.getBoundingClientRect();
        this.setZoom(stepZoom(this.zoomFactor, r.step), { x: e.clientX - rect.left, y: e.clientY - rect.top });
      },
      { passive: false },
    );
    this.resize = new ResizeObserver(() => {
      const width = this.scroller.clientWidth;
      if (!this.active || width === this.lastWidth) return;
      this.lastWidth = width;
      this.relayout();
    });
    this.resize.observe(this.scroller);
    this.updateZoomLabel("tab");
    this.updateZoomLabel("split");
    host.setMarkersShown(this.markersShown);
  }

  get isActive(): boolean {
    return this.active;
  }

  /**
   * Shows the view in the Publisher tab or the split pane, or hides it
   * (null); compiles while shown, also while it moves between the two.
   */
  setActive(place: PubPlace | null): void {
    const on = place !== null;
    if (on === this.active && (!on || place === this.where)) return;
    const shown = this.active;
    this.menu.close(false);
    this.edit.close(false);
    this.drawer.close(false);
    this.active = on;
    this.section.hidden = !on;
    if (on !== shown) void this.host.watch(on).catch(this.host.error);
    if (place) {
      this.where = place;
      this.lastWidth = this.scroller.clientWidth;
      if (!shown) {
        void this.panel.refresh();
        void this.overrides.refresh().catch(this.host.error);
      }
      // Another width and zoom.
      this.relayout();
      this.showStatus();
      // The split pane leaves the focus in the editor.
      if (place === "tab") this.scroller.focus({ preventScroll: true });
    } else {
      cancelAnimationFrame(this.frame);
      this.frame = 0;
    }
  }

  /** Reloads the layout panel and the exceptions (after undo/redo). */
  refreshLayout(): void {
    void this.panel.refresh();
    void this.overrides.refresh().catch(this.host.error);
  }

  /** Esc: closes the page menu, the edit popover or the Exceptions drawer (or ends its edit); true if one was open. */
  escape(): boolean {
    return this.menu.escape() || this.edit.escape() || this.drawer.escape();
  }

  /** F7 / Exceptions: opens or closes the Exceptions drawer while the pages show. */
  toggleDrawer(): void {
    if (this.active) this.drawer.toggle();
  }

  /** Another project: forget pages, status and diagnostics. */
  reset(): void {
    this.cache.clear();
    for (const p of this.els.values()) p.el.remove();
    this.els.clear();
    this.sizes = [];
    this.status = null;
    this.positions.clear();
    this.markers.clear();
    this.flash = null;
    this.shownDiagnostics = null;
    this.renderDiagnostics();
    this.menu.close(false);
    this.edit.close(false);
    this.drawer.reset();
    this.menuToken++;
    this.overrides.reset();
    this.panel.reset();
    this.relayout();
    this.showStatus();
  }

  /** A `compile-status` event. */
  onStatus(s: CompileStatus): void {
    this.status = s;
    if ((s.state === "done" || s.state === "failed") && s.pages_version !== null) {
      this.setPages(s.pages_version, s.page_sizes);
    } else if (s.state !== "compiling") {
      // No pages to fetch (e.g. the worker restarted): keep showing the old images.
      this.cache.setVersion(null, this.sizes.length);
    }
    if (s.state === "done" || s.state === "failed") {
      this.shownDiagnostics = { version: s.version, list: s.diagnostics };
      this.renderDiagnostics();
    }
    if (this.positions.onStatus(s)) this.updateMarkers();
    this.overrides.onStatus(s);
    this.showStatus();
  }

  // ---- Zoom ----

  get zoomFactor(): number {
    return this.layout.scale / PX_PER_PT;
  }

  zoomIn(): void {
    this.setZoom(stepZoom(this.zoomFactor, 1));
  }

  zoomOut(): void {
    this.setZoom(stepZoom(this.zoomFactor, -1));
  }

  zoomFit(): void {
    this.setZoom("fit");
  }

  /** Sets the current place's zoom, keeping the spot at `focus` (a point in the preview) in place if given. */
  private setZoom(zoom: Zoom, focus?: Point): void {
    // The markers move.
    this.edit.close();
    this.zooms[this.where] = zoom;
    try {
      localStorage.setItem(ZOOM_KEYS[this.where], String(zoom));
    } catch {
      // not persisted
    }
    this.relayout(undefined, focus);
  }

  private updateZoomLabel(place = this.where): void {
    const zoom = this.zooms[place];
    this.host.setZoomLabel(zoom === "fit" ? "Fit width" : `${Math.round(zoom * 100)} %`, place);
  }

  // ---- Exception markers ----

  /** Shows or hides the exception markers (remembered). */
  toggleMarkers(): void {
    this.markersShown = !this.markersShown;
    try {
      localStorage.setItem("lethora.markers", this.markersShown ? "1" : "0");
    } catch {
      // not persisted
    }
    this.host.setMarkersShown(this.markersShown);
    this.markerStamp++;
    this.render();
  }

  /** Recomputes the markers (new positions, pages or overrides) and redraws them. */
  private updateMarkers(): void {
    this.markers = markersByPage(this.positions.for(this.cache.current), this.overrides.list, this.sizes);
    this.markerStamp++;
    this.render();
  }

  /** The page (0-based) of each override on the pages shown, from the last compile. */
  private pageNumbers(): ReadonlyMap<string, number> {
    const list = this.positions.for(this.cache.current);
    if (this.pagesOf.list !== list) this.pagesOf = { list, map: new Map(list.map((s) => [s.id, s.page!])) };
    return this.pagesOf.map;
  }

  /** Scrolls to override `id`'s marker and highlights it briefly. */
  private showMarker(id: string): void {
    const s = this.positions.find(id, this.cache.current);
    if (!this.active || s?.page == null || s.y_pt == null || s.page >= this.sizes.length) return;
    const mid = this.layout.tops[s.page] + (s.y_pt + (s.height_pt ?? 0) / 2) * this.layout.scale;
    this.scroller.scrollTop = Math.max(0, mid - this.scroller.clientHeight / 2);
    this.flash = id;
    clearTimeout(this.flashTimer);
    this.flashTimer = window.setTimeout(() => {
      this.flash = null;
      this.markerStamp++;
      this.render();
    }, FLASH_MS);
    this.markerStamp++;
    this.render();
  }

  /**
   * Edits override `id` in the popover, at its marker if that shows, else at
   * viewport point `at`; an open Exceptions drawer makes its row current.
   */
  private openEdit(id: string, at: Point): void {
    this.drawer.reveal(id);
    const chip = this.sizer.querySelector<HTMLElement>(`.pub-mark[data-id="${CSS.escape(id)}"] .pub-chip`);
    const r = chip?.getBoundingClientRect();
    const view = this.scroller.getBoundingClientRect();
    const shown = r && r.bottom > view.top && r.top < view.bottom && r.right > view.left && r.left < view.right;
    const box: Box = shown ? r : { left: at.x, top: at.y, right: at.x, bottom: at.y };
    this.edit.open(id, box);
  }

  /** Draws the markers of page `index` on `p`, if they changed. */
  private drawMarks(p: PageEl, index: number): void {
    const key = `${this.markerStamp}:${this.layout.scale}`;
    if (p.marksKey === key) return;
    p.marksKey = key;
    const list = this.markersShown ? (this.markers.get(index) ?? []) : [];
    p.marks.replaceChildren(...list.map((m) => this.markEl(m)));
  }

  private markEl(m: Marker): HTMLElement {
    const box = scaleBox(m, this.layout.scale);
    const el = document.createElement("div");
    el.className = `pub-mark ${m.kind.replace("_", "-")}${m.detached ? " detached" : ""}${this.flash === m.id ? " flash" : ""}`;
    el.dataset.id = m.id;
    el.style.left = `${box.left}px`;
    el.style.top = `${box.top}px`;
    el.style.width = `${box.width}px`;
    el.style.height = `${box.height}px`;
    const chip = document.createElement("button");
    chip.type = "button";
    chip.className = "pub-chip";
    chip.textContent = m.chip;
    const label = this.overrides.label(m.id) ?? "Exception";
    chip.title = `${label}${m.detached ? " (its text was deleted)" : ""}. Click: edit; right-click: more.`;
    el.append(chip);
    // Clicks on the chip (or a space's bracket) are the marker's, not the page's.
    el.addEventListener("click", (e) => {
      e.stopPropagation();
      // A second click on the marker only closes its popover.
      if (e.detail > 0 && this.editWas === m.id) return;
      this.openEdit(m.id, { x: e.clientX, y: e.clientY });
    });
    el.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      e.stopPropagation();
      const r = chip.getBoundingClientRect();
      const keyboard = e.button !== 2;
      const model = menuModel({ target: { in: "override", id: m.id }, figure_utf16: null, image_utf16: null, table_utf16: null }, this.overrides.list, this.menuContext());
      this.menuAt = keyboard ? { x: r.left, y: r.bottom } : { x: e.clientX, y: e.clientY };
      this.menu.open(this.menuAt.x, this.menuAt.y, model, this.menuHandlers());
    });
    return el;
  }

  // ---- Pages ----

  private setPages(version: number, sizes: PageSize[]): void {
    const same = sizes.length === this.sizes.length && sizes.every((s, i) => s[0] === this.sizes[i][0] && s[1] === this.sizes[i][1]);
    if (!same) {
      const anchor = anchorAt(this.layout, this.scroller.scrollTop);
      this.sizes = sizes;
      this.relayout(anchor);
    }
    this.cache.setVersion(version, sizes.length);
    this.render();
  }

  /**
   * Lays the pages out again (zoom, size or page changes), keeping the page
   * at the top in place, or with `focus` (a point in the preview) the spot
   * under it.
   */
  private relayout(anchor = anchorAt(this.layout, this.scroller.scrollTop), focus?: Point): void {
    this.updateZoomLabel();
    if (!this.active) {
      // Laid out when shown (the scroller has no size while hidden).
      this.layout = layoutPages(this.sizes, this.layout.scale || 1);
      return;
    }
    const old = this.layout;
    const { scrollLeft, scrollTop } = this.scroller;
    const width = this.scroller.clientWidth;
    const scale = zoomScale(this.zooms[this.where], this.sizes, width);
    this.layout = layoutPages(this.sizes, scale);
    const contentWidth = Math.max(width, this.layout.width);
    this.sizer.style.height = `${this.layout.total}px`;
    this.sizer.style.width = `${contentWidth}px`;
    if (focus) {
      // Pages are centred: the spot's distance from the centre scales with the zoom.
      this.scroller.scrollTop = scrollFor(this.layout, anchorAt(old, scrollTop + focus.y)) - focus.y;
      this.scroller.scrollLeft = contentWidth / 2 + ((scrollLeft + focus.x - Math.max(width, old.width) / 2) * scale) / old.scale - focus.x;
    } else {
      this.scroller.scrollTop = scrollFor(this.layout, anchor, this.scroller.clientHeight);
      this.scroller.scrollLeft = (contentWidth - width) / 2;
    }
    this.placeholder.hidden = this.sizes.length > 0;
    this.render();
  }

  private schedule(): void {
    if (this.frame || !this.active) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      this.render();
    });
  }

  /** Creates the page elements in and near the viewport, drops the others, asks for images. */
  private render(): void {
    if (!this.active) return;
    const top = this.scroller.scrollTop;
    const height = this.scroller.clientHeight;
    const overscan = height;
    const { first, last } = visibleRange(this.layout, top, height, overscan);
    for (const [index, p] of this.els) {
      if (index < first || index > last) {
        p.el.remove();
        this.els.delete(index);
      }
    }
    const contentWidth = Math.max(this.scroller.clientWidth, this.layout.width);
    for (let i = first; i <= last; i++) {
      let p = this.els.get(i);
      if (!p) {
        p = this.pageEl(i);
        this.els.set(i, p);
        this.sizer.append(p.el);
      }
      const w = this.layout.widths[i];
      const h = this.layout.heights[i];
      const s = p.el.style;
      s.transform = `translate(${Math.round((contentWidth - w) / 2)}px, ${this.layout.tops[i]}px)`;
      s.width = `${w}px`;
      s.height = `${h}px`;
      this.setSrc(p, i);
      this.drawMarks(p, i);
    }
    this.cache.want(wantedPages(this.layout, top, height, overscan));
  }

  private pageEl(index: number): PageEl {
    const el = document.createElement("div");
    el.className = "pub-page";
    const img = document.createElement("img");
    img.alt = `Page ${index + 1}`;
    img.draggable = false;
    img.decoding = "async";
    const label = document.createElement("span");
    label.className = "pub-label";
    label.textContent = String(index + 1);
    const marks = document.createElement("div");
    marks.className = "pub-marks";
    el.append(img, marks, label);
    return { el, img, label, src: "", marks, marksKey: "" };
  }

  private setSrc(p: PageEl, index: number): void {
    const url = this.cache.get(index);
    if (url && url !== p.src) {
      p.src = url;
      p.img.src = url;
      p.el.classList.add("loaded");
    }
    p.el.classList.toggle("stale", !this.cache.isCurrent(index));
  }

  private showImage(index: number): void {
    const p = this.els.get(index);
    if (p) this.setSrc(p, index);
  }

  // ---- Status and diagnostics ----

  private showStatus(): void {
    const s = this.status;
    if (!s) {
      this.host.setStatus(this.active ? "Typesetting…" : "", this.active ? "busy" : "");
      this.placeholder.textContent = "Typesetting…";
      return;
    }
    const { text, tone } = compileStatusText(s);
    this.host.setStatus(text, tone);
    if (this.sizes.length === 0) this.placeholder.textContent = s.state === "compiling" ? "Typesetting…" : s.state === "done" ? "No pages." : text;
  }

  private renderDiagnostics(): void {
    const d = this.shownDiagnostics;
    const list = d?.list ?? [];
    this.diags.hidden = list.length === 0;
    if (list.length === 0) {
      this.diagsList.replaceChildren();
      return;
    }
    this.diagsSummary.textContent = diagnosticsSummary(list);
    if (list.some((x) => x.severity === "error")) this.diags.open = true;
    const items = list.slice(0, MAX_DIAGNOSTICS).map((diag) => this.diagnosticItem(diag, d!.version));
    if (list.length > MAX_DIAGNOSTICS) {
      const more = document.createElement("li");
      more.className = "pub-diag-more";
      more.textContent = `and ${(list.length - MAX_DIAGNOSTICS).toLocaleString("en-US")} more`;
      items.push(more);
    }
    this.diagsList.replaceChildren(...items);
  }

  private diagnosticItem(d: Diagnostic, version: number): HTMLElement {
    const li = document.createElement("li");
    li.className = `pub-diag ${d.severity}`;
    const clickable = d.location.in === "content" || d.location.in === "custom" || d.location.in === "override";
    const body = document.createElement(clickable ? "button" : "div");
    body.className = "pub-diag-body";
    if (body instanceof HTMLButtonElement) {
      body.type = "button";
      body.title = d.location.in === "content" ? "Show in the text" : d.location.in === "override" ? "Edit the exception" : "Show in the expert code";
      body.addEventListener("click", () => void this.follow(d, version).catch(this.host.error));
    }
    const icon = document.createElement("span");
    icon.className = "pub-diag-icon";
    icon.textContent = d.severity === "error" ? "✕" : "!";
    icon.setAttribute("aria-label", d.severity);
    const message = document.createElement("span");
    message.className = "pub-diag-message";
    message.textContent = d.message;
    const where = document.createElement("span");
    where.className = "pub-diag-where";
    const docLength = this.host.docLength();
    where.textContent = locationLabel(
      d.location,
      (u) => this.host.lineOf(Math.min(u, docLength)),
      (id) => this.overrides.label(id),
    );
    body.append(icon, message, where);
    li.append(body);
    for (const hint of d.hints) {
      const h = document.createElement("p");
      h.className = "pub-diag-hint";
      h.textContent = `Hint: ${hint}`;
      li.append(h);
    }
    return li;
  }

  private async follow(d: Diagnostic, version: number): Promise<void> {
    await this.host.drain();
    const docVersion = d.location.in === "content" ? await this.host.docVersion() : version;
    const target = diagnosticTarget(d, { compiledVersion: version, docVersion, docLength: this.host.docLength(), customText: this.panel.customText });
    if (target.kind === "content") this.host.revealText(target.from, target.to, target.outdated);
    else if (target.kind === "custom") this.panel.revealCustom(target.from, target.to);
    else if (target.kind === "override") {
      // Scrolls to its marker, then edits it there (or at the pane's centre if it has none).
      this.showMarker(target.id);
      const r = this.scroller.getBoundingClientRect();
      this.openEdit(target.id, { x: r.left + r.width / 2, y: r.top + r.height / 3 });
    }
  }

  // ---- Page menu ----

  /** The page spot at viewport point (`x`, `y`), if any. */
  private pointAt(x: number, y: number): { page: number; xPt: number; yPt: number } | null {
    const rect = this.scroller.getBoundingClientRect();
    const view = { left: rect.left, top: rect.top, width: this.scroller.clientWidth, scrollLeft: this.scroller.scrollLeft, scrollTop: this.scroller.scrollTop };
    return pagePoint(this.layout, view, x, y);
  }

  /** A click in the split pane: selects the clicked word in the text (offsets of the compiled text, clamped). */
  private async openText(x: number, y: number): Promise<void> {
    const point = this.pointAt(x, y);
    const version = this.cache.current;
    if (!point || version === null) return;
    const { target } = await this.jump(version, point.page, point.xPt, point.yPt);
    if (!this.active || this.where !== "split" || target.in !== "content") return;
    const length = this.host.docLength();
    this.host.revealText(Math.min(target.start_utf16, length), Math.min(target.end_utf16, length), false);
  }

  /** Right-click (or the menu key) on the preview: the exceptions menu for the clicked spot. */
  private async openMenu(e: MouseEvent): Promise<void> {
    const token = ++this.menuToken;
    const rect = this.scroller.getBoundingClientRect();
    // The menu key and Shift+F10 come without a pointer position: use the view's centre.
    const keyboard = e.button !== 2;
    const x = keyboard ? rect.left + rect.width / 2 : e.clientX;
    const y = keyboard ? rect.top + rect.height / 3 : e.clientY;
    const point = this.pointAt(x, y);
    const version = this.cache.current;
    let model: MenuModel;
    if (!point || version === null) {
      model = menuModel({ target: { in: "none" }, figure_utf16: null, image_utf16: null, table_utf16: null }, [], this.menuContext());
    } else {
      const [jump] = await Promise.all([this.jump(version, point.page, point.xPt, point.yPt), this.overrides.refresh().catch(() => {})]);
      if (token !== this.menuToken || !this.active) return;
      model = menuModel(jump, this.overrides.list, this.menuContext());
    }
    this.menuAt = { x, y };
    this.menu.open(x, y, model, this.menuHandlers());
  }

  private menuHandlers(): MenuHandlers {
    return {
      run: (act) => this.runAct(act),
      space: async (length, pos) => {
        await this.overrides.add("space_before", { length }, pos);
      },
      closed: () => this.scroller.focus({ preventScroll: true }),
    };
  }

  private menuContext(): MenuContext {
    const state = this.panel.state;
    const quality = state === null ? undefined : valueOf(state, "export.image_quality");
    const table = state === null ? undefined : valueOf(state, "tables.style");
    return {
      twoColumns: state !== null && valueOf(state, "page.columns") === 2,
      imageQuality: QUALITIES.includes(quality as ImageQuality) ? (quality as ImageQuality) : "original",
      tableStyle: TABLE_STYLES.includes(table as TableStyle) ? (table as TableStyle) : "plain",
      lineAt: (u: number) => this.host.lineAt(u),
    };
  }

  /** `page_jump` on `version`; if that version is gone, once more on the latest. */
  private async jump(version: number, page: number, xPt: number, yPt: number): Promise<JumpResult> {
    try {
      return await this.host.pageJump(version, page, xPt, yPt);
    } catch (e) {
      const latest = this.status?.pages_version ?? null;
      if (latest === null || latest === version) throw e;
      return this.host.pageJump(latest, page, xPt, yPt);
    }
  }

  private runAct(act: Act): void {
    const done = (p: Promise<unknown>) => void p.catch(this.host.error);
    if (act.op === "add") done(this.overrides.add(act.kind, act.params, act.pos));
    else if (act.op === "remove") done(this.overrides.remove(act.id));
    else if (act.op === "replace_image") done(this.host.replaceImage(act.path, act.pos));
    else this.openEdit(act.id, this.menuAt);
  }
}

function loadMarkersShown(): boolean {
  try {
    return localStorage.getItem("lethora.markers") !== "0";
  } catch {
    return true;
  }
}

function loadZoom(key: string): Zoom {
  try {
    const z = Number(localStorage.getItem(key));
    return Number.isFinite(z) && z >= 0.5 && z <= 2 ? z : "fit";
  } catch {
    return "fit";
  }
}
