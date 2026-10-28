// The page view's edit popover: one exception's label, anchored text, state
// and parameters, at its marker on the page (or where the click was). Used
// in the Publisher tab and in the split pane, where the layout panel is
// hidden. The fields are the Exceptions list's (`paramEditor`).
//
// `popoverPlace` is pure and tested under `node --test`.

import { type OverrideStore, paramEditor } from "./exceptions.ts";

/** A box in viewport pixels (a point has no size). */
export interface Box {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

const MARGIN = 4;
const GAP = 4;

/**
 * Where a popover of `size` goes for `anchor` in a `view` of that size: below
 * the anchor, else above it, else as low as fits; its left edge at the
 * anchor's, kept inside the view.
 */
export function popoverPlace(anchor: Box, size: { width: number; height: number }, view: { width: number; height: number }): { left: number; top: number } {
  const below = anchor.bottom + GAP;
  const above = anchor.top - GAP - size.height;
  const top = below + size.height + MARGIN <= view.height ? below : above >= MARGIN ? above : Math.max(MARGIN, view.height - size.height - MARGIN);
  const left = Math.max(MARGIN, Math.min(anchor.left, view.width - size.width - MARGIN));
  return { left, top };
}

export interface EditHost {
  /** Shows UTF-16 offset `pos` in the text (as the Exceptions list's "Open in Science view"). */
  openInText(pos: number): void;
  /** The popover closed with the focus in it: the focus goes back to the pages. */
  closed(): void;
  error(e: unknown): void;
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, props: Partial<HTMLElementTagNameMap[K]> = {}, ...children: Array<Node | string>): HTMLElementTagNameMap[K] {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...children);
  return e;
}

/**
 * The edit popover (one at a time). Changes apply like in the Exceptions list
 * and it stays open with the new values; it closes on Esc, ×, Remove, a
 * pointerdown or wheel outside, a resize, or when its exception is gone.
 */
export class ExceptionEdit {
  private store: OverrideStore;
  private host: EditHost;
  private parent: HTMLElement;
  private root: HTMLElement | null = null;
  private id: string | null = null;
  private anchor: Box = { left: 0, top: 0, right: 0, bottom: 0 };
  /** What the shown content was built from (redrawn only when it changes). */
  private key = "";
  private draft: string | null = null;
  private error: string | null = null;
  private onOutside = (e: Event) => {
    if (this.root && !this.root.contains(e.target as Node)) this.close();
  };
  private onResize = () => this.close();

  constructor(store: OverrideStore, host: EditHost, parent: HTMLElement) {
    this.store = store;
    this.host = host;
    this.parent = parent;
    store.subscribe(() => this.render());
  }

  get isOpen(): boolean {
    return this.root !== null;
  }

  /** The override shown, if open. */
  get current(): string | null {
    return this.id;
  }

  /** Shows override `id` at `anchor` (viewport pixels). */
  open(id: string, anchor: Box): void {
    this.close(false);
    if (!this.store.find(id)) return;
    this.id = id;
    this.anchor = anchor;
    this.draft = null;
    this.error = null;
    const root = el("div", { className: "page-menu ex-pop", tabIndex: -1 });
    root.setAttribute("role", "dialog");
    // Leaving it with Tab closes it.
    root.addEventListener("focusout", (e) => {
      if (e.relatedTarget instanceof Node && !root.contains(e.relatedTarget)) this.close(false);
    });
    this.root = root;
    this.parent.append(root);
    this.render(true);
    window.addEventListener("pointerdown", this.onOutside, true);
    window.addEventListener("wheel", this.onOutside, { capture: true, passive: true });
    window.addEventListener("resize", this.onResize);
    (root.querySelector<HTMLElement>(".ex-edit :is(input, select)") ?? root.querySelector<HTMLElement>(".ex-pop-close"))?.focus({ preventScroll: true });
  }

  /** Closes it (`restore`: if the focus was in it, back to the pages). */
  close(restore = true): void {
    const root = this.root;
    if (!root) return;
    const focused = root.contains(document.activeElement);
    root.remove();
    this.root = null;
    this.id = null;
    this.key = "";
    window.removeEventListener("pointerdown", this.onOutside, true);
    window.removeEventListener("wheel", this.onOutside, true);
    window.removeEventListener("resize", this.onResize);
    if (restore && focused) this.host.closed();
  }

  /** Esc: closes it; true if it was open. */
  escape(): boolean {
    if (!this.root) return false;
    this.close();
    return true;
  }

  private render(force = false): void {
    const root = this.root;
    if (!root || this.id === null) return;
    const o = this.store.find(this.id);
    if (!o) {
      this.close();
      return;
    }
    const state = this.store.state(o);
    const key = JSON.stringify([o, state]);
    if (!force && key === this.key) return;
    this.key = key;
    const active = document.activeElement;
    const refocus = active instanceof HTMLElement && root.contains(active) ? (active.dataset.focusKey ?? null) : null;

    const close = el("button", { type: "button", className: "ex-action ex-pop-close", textContent: "×" });
    close.dataset.focusKey = "close";
    close.setAttribute("aria-label", "Close");
    close.addEventListener("click", () => this.close());
    const badges = el("span", { className: "ex-badges" });
    for (const b of state.badges) badges.append(el("span", { className: `ex-badge ${b.kind}`, textContent: b.text, title: b.title }));
    const head = el("div", { className: "ex-pop-head" }, el("span", { className: "ex-pop-title", textContent: state.label }), badges, close);
    const anchor = el("p", { className: "ex-pop-anchor", textContent: o.anchor_preview.trim() || (o.pos_utf16 === null ? "" : "(empty line)") });
    anchor.title = o.anchor_preview;
    const parts: HTMLElement[] = [head, anchor];
    if (state.reason) parts.push(el("p", { className: `ex-pop-reason${state.badges.some((b) => b.kind === "invalid") ? " invalid" : ""}`, textContent: state.reason }));
    if (state.editable) {
      parts.push(
        paramEditor(o, {
          idPrefix: "ex-pop",
          draft: this.draft,
          error: this.error,
          onDraft: (text) => (this.draft = text),
          apply: (params) => {
            this.store.update(o.id, params).then(
              () => {
                this.draft = null;
                this.error = null;
                this.render(true);
              },
              (e: unknown) => {
                this.error = e instanceof Error ? e.message : String(e);
                this.render(true);
              },
            );
          },
          cancel: () => this.close(),
        }),
      );
    }
    const actions = el("div", { className: "ex-pop-actions" });
    if (o.pos_utf16 !== null) {
      const pos = o.pos_utf16;
      const show = el("button", { type: "button", className: "ex-action", textContent: "Show in text" });
      show.dataset.focusKey = "show";
      show.title = "Show where it is anchored in the text";
      show.addEventListener("click", () => {
        this.close(false);
        this.host.openInText(pos);
      });
      actions.append(show);
    }
    const remove = el("button", { type: "button", className: "ex-action", textContent: "Remove" });
    remove.dataset.focusKey = "remove";
    remove.setAttribute("aria-label", `Remove ${state.label}`);
    remove.addEventListener("click", () => {
      this.close();
      this.store.remove(o.id).catch(this.host.error);
    });
    actions.append(remove);
    parts.push(actions);

    root.setAttribute("aria-label", `Exception: ${state.label}`);
    root.replaceChildren(...parts);
    const at = popoverPlace(this.anchor, { width: root.offsetWidth, height: root.offsetHeight }, { width: window.innerWidth, height: window.innerHeight });
    root.style.left = `${at.left}px`;
    root.style.top = `${at.top}px`;
    if (refocus) root.querySelector<HTMLElement>(`[data-focus-key="${CSS.escape(refocus)}"]`)?.focus({ preventScroll: true });
  }
}
