// The title-bar row: project name, sync cloud, who else is online, word
// count, status and errors (the menus are menus.ts, the view switch and tools are wired in main.ts),
// and the window buttons on Windows and Linux (the window has no native
// title bar there; the row's empty parts drag it, see index.html). The row
// always shows; Focus mode hides its tools (CSS).

import { onlineRow, type OnlineUser } from "./presence.ts";

const INFO_FADE_MS = 3500;
const INFO_CLEAR_MS = 4200;

const byId = (id: string) => document.getElementById(id)!;

/** The row's toggle buttons (the other toggles live in the View menu). */
export type Toggle = "palette" | "sources" | "pages" | "assets" | "review";

/** The views: writing, science (rendered math, sources), publisher (pages, layout). */
export type ViewMode = "author" | "science" | "publisher";

export class TopBar {
  private bar = byId("bar");
  private nameEl = byId("project-name");
  private onlineEl = byId("online");
  private wordsEl = byId("words");
  private statusEl = byId("status");
  private pubStatusEl = byId("pub-status");
  private errorEl = byId("error");
  private errorText = byId("error-text");
  private infoTimers: Array<ReturnType<typeof setTimeout>> = [];
  private numberFormat = new Intl.NumberFormat();
  /** Errors reported so far. */
  errors = 0;

  constructor() {
    byId("error-dismiss").addEventListener("click", () => this.clearError());
  }

  setProject(name: string | null): void {
    this.nameEl.textContent = name ?? "";
    this.nameEl.title = name ?? "";
  }

  /** The other members online in a shared project: their initials in their colours. */
  setOnline(users: readonly OnlineUser[]): void {
    const { shown, rest } = onlineRow(users);
    const more = document.createElement("span");
    more.className = "bar-online-more";
    more.textContent = `+${rest.length}`;
    more.title = rest.map((u) => u.title).join("\n");
    this.onlineEl.replaceChildren(
      ...shown.map((u) => {
        const el = document.createElement("span");
        el.textContent = u.initials;
        el.title = u.title;
        el.style.setProperty("--peer", u.colour);
        el.style.setProperty("--peer-ink", u.ink);
        return el;
      }),
      ...(rest.length ? [more] : []),
    );
    this.onlineEl.hidden = users.length === 0;
    this.onlineEl.setAttribute("aria-label", users.length ? `Online: ${users.map((u) => u.title).join("; ")}` : "");
  }

  setWords(n: number | null): void {
    this.wordsEl.textContent = n === null ? "" : `${this.numberFormat.format(n)} ${n === 1 ? "word" : "words"}`;
  }

  setToggle(which: Toggle, on: boolean): void {
    byId(`toggle-${which}`).setAttribute("aria-pressed", String(on));
  }

  /** Marks the active view in the Author/Science/Publisher switch. */
  setView(mode: ViewMode): void {
    for (const b of this.bar.querySelectorAll<HTMLElement>(".bar-views button")) {
      b.setAttribute("aria-pressed", String(b.id === `view-${mode}`));
    }
  }

  /** The publisher view's compile status (stays until replaced). */
  setPublisherStatus(text: string, tone: string): void {
    this.pubStatusEl.textContent = text;
    this.pubStatusEl.title = text;
    this.pubStatusEl.dataset.tone = tone;
  }

  /** A passing message ("Saved") that fades. */
  info(message: string): void {
    for (const t of this.infoTimers) clearTimeout(t);
    this.statusEl.classList.remove("fading");
    this.statusEl.textContent = message;
    if (!message) return;
    this.infoTimers = [
      setTimeout(() => this.statusEl.classList.add("fading"), INFO_FADE_MS),
      setTimeout(() => {
        this.statusEl.textContent = "";
        this.statusEl.classList.remove("fading");
      }, INFO_CLEAR_MS),
    ];
  }

  /** A sticky error: stays until dismissed or cleared. */
  error(e: unknown): void {
    this.errors++;
    this.errorText.textContent = e instanceof Error ? e.message : String(e);
    this.errorEl.hidden = false;
  }

  clearError(): void {
    this.errorEl.hidden = true;
    this.errorText.textContent = "";
  }
}

/** What the window buttons need of the window (Tauri's `getCurrentWindow()`). */
export interface WindowControls {
  minimize(): Promise<void>;
  toggleMaximize(): Promise<void>;
  /** Closes through the close request (edits are sent first, see main.ts). */
  close(): Promise<void>;
  isMaximized(): Promise<boolean>;
  onResized(handler: () => void): unknown;
}

/** Windows 11 caption glyphs, 10 × 10 px (straight lines snapped to pixels). */
const GLYPHS = {
  minimize: '<path d="M0 5.5h10" shape-rendering="crispEdges"/>',
  maximize: '<rect x="0.5" y="0.5" width="9" height="9" shape-rendering="crispEdges"/>',
  restore: '<g shape-rendering="crispEdges"><rect x="0.5" y="2.5" width="7" height="7"/><path d="M2.5 2.5v-2h7v7h-2"/></g>',
  close: '<path d="M0.5 0.5l9 9m0-9l-9 9"/>',
};

const glyph = (name: keyof typeof GLYPHS) =>
  `<svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" stroke-width="1" aria-hidden="true">${GLYPHS[name]}</svg>`;

/** Minimize, maximize/restore and close, at the row's right end. */
export class WindowButtons {
  /** The maximize/restore button (Snap Layouts sit on it on Windows, see snapLayouts). */
  readonly max: HTMLButtonElement;

  constructor(el: HTMLElement, win: WindowControls, error: (e: unknown) => void) {
    const button = (name: keyof typeof GLYPHS, label: string, action: () => Promise<void>) => {
      const b = document.createElement("button");
      b.type = "button";
      // Not a tab stop, like native caption buttons (Alt+Space and the OS keys still work).
      b.tabIndex = -1;
      b.className = `win-${name}`;
      b.title = label;
      b.setAttribute("aria-label", label);
      b.innerHTML = glyph(name);
      b.addEventListener("click", () => void action().catch(error));
      return b;
    };
    this.max = button("maximize", "Maximize", () => win.toggleMaximize());
    el.append(button("minimize", "Minimize", () => win.minimize()), this.max, button("close", "Close", () => win.close()));
    const update = () => void win.isMaximized().then((on) => this.setMaximized(on), () => {});
    win.onResized(update);
    update();
  }

  /** The middle button restores while the window is maximized. */
  private setMaximized(on: boolean): void {
    const label = on ? "Restore down" : "Maximize";
    if (this.max.title === label) return;
    this.max.title = label;
    this.max.setAttribute("aria-label", label);
    this.max.innerHTML = glyph(on ? "restore" : "maximize");
  }
}

/** A rectangle in physical pixels from the web view's top left. */
export type SnapRect = { x: number; y: number; width: number; height: number };

/**
 * Windows 11 Snap Layouts on the maximize `button`: a native overlay (snap.rs)
 * takes the mouse there. `report` keeps it over the button (on resize, layout
 * and DPI or zoom changes); `onState` delivers the mouse's state on it
 * ("hover", "leave", "press", "click"), shown as the button's own hover and
 * pressed looks; a click clicks the button.
 */
export function snapLayouts(
  button: HTMLElement,
  report: (r: SnapRect) => Promise<unknown>,
  onState: (handler: (state: string) => void) => unknown,
  error: (e: unknown) => void,
): void {
  let last = "";
  const update = () => {
    const r = button.getBoundingClientRect();
    const dpr = devicePixelRatio;
    // Edges rounded on their own, so they stay pixel-exact.
    const x = Math.round(r.left * dpr);
    const y = Math.round(r.top * dpr);
    const rect = { x, y, width: Math.round(r.right * dpr) - x, height: Math.round(r.bottom * dpr) - y };
    const key = JSON.stringify(rect);
    if (key === last) return;
    last = key;
    void report(rect).catch(error);
  };
  // Fires once when the ratio changes, so it is set up again for the new one.
  const watchRatio = () =>
    matchMedia(`(resolution: ${devicePixelRatio}dppx)`).addEventListener(
      "change",
      () => {
        update();
        watchRatio();
      },
      { once: true },
    );
  addEventListener("resize", update);
  new ResizeObserver(update).observe(button);
  watchRatio();
  update();
  onState((state) => {
    if (state === "hover") button.classList.add("hover");
    else if (state === "leave") button.classList.remove("hover", "pressed");
    else if (state === "press") button.classList.add("pressed");
    else if (state === "click") {
      button.classList.remove("pressed");
      button.click();
    }
  });
}
