// The publisher view's right-click menu on a page: add, change or remove
// exceptions (anchored overrides) at the clicked block, figure, image or
// table, and replace the clicked image.
//
// `menuModel` (pure, tested) decides the items from a `page_jump` result;
// `PageMenu` shows them as an accessible menu (arrow keys, Enter, Esc).

import { showKeys } from "./commands.ts";
import { isAssetPath } from "./figures.ts";
import { type ImageQuality, isBlockKind, type JumpResult, type OverrideInfo, type OverrideKind, overrideLabel, type Params, type Placement, PLACEMENTS, QUALITIES, QUALITY_LABELS, TABLE_STYLE_LABELS, TABLE_STYLES, type TableStyle } from "./exceptions.ts";

export type Act = { op: "add"; kind: OverrideKind; params: Params; pos: number } | { op: "remove"; id: string } | { op: "edit"; id: string } | { op: "replace_image"; path: string; pos: number };

/**
 * A menu entry; `A` is what an item does (the page menu's `Act`, or another
 * owner's). `keys` shows a shortcut, `title` explains a disabled item.
 */
export type MenuItem<A = Act> =
  | { type: "item"; label: string; act: A | null; checked?: boolean; radio?: boolean; hint?: string; keys?: string; title?: string }
  | { type: "space"; label: string; value: string; pos: number; id: string | null }
  | { type: "submenu"; label: string; items: MenuItem<A>[] }
  | { type: "separator" };

export interface MenuModel<A = Act> {
  header: string | null;
  /** The menu's accessible name without a header ("Page" if none). */
  label?: string;
  items: MenuItem<A>[];
}

export interface MenuContext {
  /** The layout has two columns (column breaks make sense). */
  twoColumns: boolean;
  /** The document's export image quality (images without one of their own). */
  imageQuality: ImageQuality;
  /** The layout's default table style (tables without one of their own). */
  tableStyle: TableStyle;
  /** The line holding UTF-16 offset `utf16` in the current text. */
  lineAt(utf16: number): { from: number; text: string };
}

export const DEFAULT_SPACE = "12pt";
const PREVIEW_CHARS = 44;

const PLACEMENT_LABELS: Record<Placement, string> = { top: "Top", bottom: "Bottom", here: "Here", auto: "Auto" };

function clip(s: string, n = PREVIEW_CHARS): string {
  const t = s.replace(/\s+/g, " ").trim();
  if (t.length <= n) return t;
  const cut = t.slice(0, n - 1);
  const space = cut.lastIndexOf(" ");
  return `${(space > n * 0.6 ? cut.slice(0, space) : cut).trimEnd()}…`;
}

/**
 * A short name for the block whose text starts with `text` (its first line,
 * maybe followed by more of a paragraph): "Heading: …", "Figure: …",
 * "Paragraph: “…”" (its first words, forced line breaks `\` dropped).
 */
export function blockPreview(text: string): string {
  const line = text.split("\n", 1)[0];
  const heading = /^(=+)\s+(.*?)(?:\s*<[^<>]*>)?\s*$/.exec(line);
  if (heading) return `Heading: ${clip(heading[2])}`;
  if (/^#figure\b/.test(line)) {
    const caption = /caption:\s*\[([^\]]*)\]?/.exec(line);
    return caption && caption[1].trim() ? `Figure: ${clip(caption[1])}` : "Figure";
  }
  if (/^\$/.test(line)) return "Equation";
  const item = /^\s*[-+]\s+(.*)$/.exec(line);
  if (item) return `List item: “${clip(item[1])}”`;
  if (line.trim() === "") return "Empty line";
  return `Paragraph: “${clip(text.replace(/(^|\s)\\(?=\s|$)/g, "$1"))}”`;
}

/** Lines that start a block of their own, ending a paragraph's preview. */
const BLOCK_LINE = /^\s*(?:$|[=#$]|[-+](?:\s|$)|\d+\.\s|\/\s)/;

/** The text of the block starting at line start `from`: its first line and the paragraph's following lines (a few, enough to preview). */
function blockText(ctx: MenuContext, from: number): string {
  const lines: string[] = [];
  for (let at = from; lines.length < 6; ) {
    const line = ctx.lineAt(at);
    if (line.from !== at || (lines.length > 0 && BLOCK_LINE.test(line.text))) break;
    lines.push(line.text);
    at = line.from + line.text.length + 1;
  }
  return lines.join("\n");
}

/** The literal asset path of the image call at `pos` (`image("assets/…"` on that line), or null. */
export function imagePathAt(ctx: MenuContext, pos: number): string | null {
  const line = ctx.lineAt(pos);
  const call = /^#?image\(\s*"([^"\\\n]*)"/.exec(line.text.slice(pos - line.from));
  return call && isAssetPath(call[1]) ? call[1] : null;
}

const check = (label: string, existing: OverrideInfo | undefined, add: Act): MenuItem =>
  existing ? { type: "item", label, checked: true, hint: "Remove", act: { op: "remove", id: existing.id } } : { type: "item", label, checked: false, act: add };

/** The menu for a click that `page_jump` resolved to `jump`, given the overrides now. */
export function menuModel(jump: JumpResult, overrides: readonly OverrideInfo[], ctx: MenuContext): MenuModel {
  const t = jump.target;
  if (t.in === "override") {
    const o = overrides.find((x) => x.id === t.id);
    const items: MenuItem[] = [];
    if (o && o.kind && o.kind !== "column_break" && o.error === null) items.push({ type: "item", label: "Edit…", act: { op: "edit", id: t.id } });
    items.push({ type: "item", label: "Remove", act: { op: "remove", id: t.id } });
    return { header: `Exception: ${o ? overrideLabel(o) : "unknown"}`, items };
  }
  if (t.in !== "content") return { header: null, items: [{ type: "item", label: "Nothing to anchor here", act: null }] };

  // Clicks inside a figure anchor block exceptions at the figure's line.
  const block = ctx.lineAt(jump.figure_utf16 ?? t.block_start_utf16);
  const pos = block.from;
  const here = overrides.filter((o) => o.error === null && isBlockKind(o.kind) && o.pos_utf16 === pos);
  const of = (kind: OverrideKind) => here.find((o) => o.kind === kind);
  const pageBreak = of("page_break");
  const columnBreak = of("column_break");
  const space = of("space_before");
  const items: MenuItem[] = [
    check("Page break before", pageBreak?.params.to_odd === true ? undefined : pageBreak, { op: "add", kind: "page_break", params: { to_odd: false }, pos }),
    check("Start on a right-hand page", pageBreak?.params.to_odd === true ? pageBreak : undefined, { op: "add", kind: "page_break", params: { to_odd: true }, pos }),
  ];
  if (ctx.twoColumns || columnBreak) items.push(check("Column break before", columnBreak, { op: "add", kind: "column_break", params: {}, pos }));
  items.push({ type: "space", label: "Space before", value: space ? String(space.params.length ?? DEFAULT_SPACE) : DEFAULT_SPACE, pos, id: space?.id ?? null });
  if (jump.figure_utf16 !== null) {
    const fig = jump.figure_utf16;
    const placement = overrides.find((o) => o.error === null && o.kind === "figure_placement" && o.pos_utf16 === fig);
    items.push(
      { type: "separator" },
      {
        type: "submenu",
        label: "Figure placement",
        items: PLACEMENTS.map((p): MenuItem => {
          const current = placement?.params.placement === p;
          return current
            ? { type: "item", label: PLACEMENT_LABELS[p], radio: true, checked: true, hint: "Remove", act: { op: "remove", id: placement!.id } }
            : { type: "item", label: PLACEMENT_LABELS[p], radio: true, checked: false, act: { op: "add", kind: "figure_placement", params: { placement: p }, pos: fig } };
        }),
      },
    );
  }
  if (jump.image_utf16 !== null) {
    const img = jump.image_utf16;
    const own = overrides.find((o) => o.error === null && o.kind === "image_quality" && o.pos_utf16 === img);
    if (jump.figure_utf16 === null) items.push({ type: "separator" });
    items.push({
      type: "submenu",
      label: "Image quality",
      items: [
        { type: "item", label: `Document default (${QUALITY_LABELS[ctx.imageQuality]})`, radio: true, checked: !own, act: own ? { op: "remove", id: own.id } : null },
        ...QUALITIES.map((q): MenuItem => {
          const current = own?.params.quality === q;
          return current
            ? { type: "item", label: QUALITY_LABELS[q], radio: true, checked: true, hint: "Remove", act: { op: "remove", id: own!.id } }
            : { type: "item", label: QUALITY_LABELS[q], radio: true, checked: false, act: { op: "add", kind: "image_quality", params: { quality: q }, pos: img } };
        }),
      ],
    });
    const path = imagePathAt(ctx, img);
    if (path) items.push({ type: "item", label: "Replace image…", act: { op: "replace_image", path, pos: img } });
  }
  if (jump.table_utf16 != null) {
    const table = jump.table_utf16;
    const own = overrides.find((o) => o.error === null && o.kind === "table_style" && o.pos_utf16 === table);
    if (jump.figure_utf16 === null && jump.image_utf16 === null) items.push({ type: "separator" });
    items.push({
      type: "submenu",
      label: "Table style",
      items: [
        { type: "item", label: `Default (${TABLE_STYLE_LABELS[ctx.tableStyle]})`, radio: true, checked: !own, act: own ? { op: "remove", id: own.id } : null },
        ...TABLE_STYLES.map((t): MenuItem => {
          const current = own?.params.style === t;
          return current
            ? { type: "item", label: TABLE_STYLE_LABELS[t], radio: true, checked: true, hint: "Remove", act: { op: "remove", id: own!.id } }
            : { type: "item", label: TABLE_STYLE_LABELS[t], radio: true, checked: false, act: { op: "add", kind: "table_style", params: { style: t }, pos: table } };
        }),
      ],
    });
  }
  return { header: blockPreview(blockText(ctx, pos)), items };
}

// ---- The menu ----

export interface MenuHandlers<A = Act> {
  /** Runs an item's action (the menu is closed first). */
  run(act: A): void;
  /** Applies "Space before" (`id`: the existing one); rejects with a readable message. */
  space(length: string, pos: number): Promise<void>;
  /** The menu closed; focus goes back to where it was. */
  closed(): void;
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, props: Partial<HTMLElementTagNameMap[K]> = {}, ...children: Array<Node | string>): HTMLElementTagNameMap[K] {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...children);
  return e;
}

/** Focusable entries of a menu level (items and the space input). */
const entries = (menu: HTMLElement) => [...menu.querySelectorAll<HTMLElement>(":scope > .pm-item:not([aria-disabled='true']), :scope > .pm-space input")];

export class PageMenu<A = Act> {
  /** Keys are shown the Mac way ("⌘X", see `showKeys`). */
  private mac: boolean;
  private root: HTMLElement | null = null;
  private sub: HTMLElement | null = null;
  private handlers: MenuHandlers<A> | null = null;
  private onOutside = (e: Event) => {
    if (this.root && !this.root.contains(e.target as Node)) this.close();
  };
  private onBlur = () => this.close();

  constructor(mac = false) {
    this.mac = mac;
  }

  get isOpen(): boolean {
    return this.root !== null;
  }

  /** Shows `model` at viewport point (`x`, `y`). */
  open(x: number, y: number, model: MenuModel<A>, handlers: MenuHandlers<A>): void {
    this.close(false);
    this.handlers = handlers;
    const root = el("div", { className: "page-menu", tabIndex: -1 });
    root.setAttribute("role", "menu");
    if (model.header) {
      const header = el("div", { className: "pm-header", textContent: model.header, id: "pm-header" });
      root.append(header);
      root.setAttribute("aria-labelledby", "pm-header");
    } else root.setAttribute("aria-label", model.label ?? "Page");
    this.fill(root, model.items, false);
    root.addEventListener("keydown", (e) => this.key(e, root));
    root.addEventListener("contextmenu", (e) => e.preventDefault());
    // A click on an item leaves the page's selection where it was: the editor gets its caret back
    // (moved into the removed menu, CodeMirror read it as position 0). Fields still take the mouse.
    root.addEventListener("mousedown", (e) => {
      if (!(e.target instanceof HTMLInputElement)) e.preventDefault();
    });
    document.body.append(root);
    this.root = root;
    place(root, x, y);
    window.addEventListener("pointerdown", this.onOutside, true);
    window.addEventListener("wheel", this.onOutside, { capture: true, passive: true });
    window.addEventListener("blur", this.onBlur);
    window.addEventListener("resize", this.onBlur);
    (entries(root)[0] ?? root).focus();
  }

  /** Closes the menu (`restore`: tell the owner, which puts focus back). */
  close(restore = true): void {
    if (!this.root) return;
    this.root.remove();
    this.root = null;
    this.sub = null;
    window.removeEventListener("pointerdown", this.onOutside, true);
    window.removeEventListener("wheel", this.onOutside, true);
    window.removeEventListener("blur", this.onBlur);
    window.removeEventListener("resize", this.onBlur);
    const h = this.handlers;
    this.handlers = null;
    if (restore) h?.closed();
  }

  /** Esc: closes the submenu or the menu; true if something was open. */
  escape(): boolean {
    if (!this.root) return false;
    if (this.sub) this.closeSub(true);
    else this.close();
    return true;
  }

  private fill(menu: HTMLElement, items: MenuItem<A>[], nested: boolean): void {
    for (const item of items) {
      if (item.type === "separator") {
        const sep = el("div", { className: "pm-sep" });
        sep.setAttribute("role", "separator");
        menu.append(sep);
      } else if (item.type === "item") {
        const row = el("div", { className: "pm-item", tabIndex: -1 });
        row.setAttribute("role", item.radio ? "menuitemradio" : item.checked !== undefined ? "menuitemcheckbox" : "menuitem");
        if (item.checked !== undefined) row.setAttribute("aria-checked", String(item.checked));
        const mark = el("span", { className: "pm-check", textContent: item.checked ? (item.radio ? "●" : "✓") : "" });
        mark.setAttribute("aria-hidden", "true");
        row.append(mark, el("span", { className: "pm-label", textContent: item.label }));
        if (item.hint) row.append(el("span", { className: "pm-hint", textContent: item.hint }));
        if (item.keys) {
          row.append(el("span", { className: "pm-hint pm-keys", textContent: showKeys(item.keys, this.mac) }));
          row.setAttribute("aria-keyshortcuts", item.keys);
        }
        if (item.title) row.title = item.title;
        if (item.act === null) {
          row.setAttribute("aria-disabled", "true");
          row.classList.add("disabled");
        } else {
          const act = item.act;
          row.addEventListener("click", () => this.activate(act));
          if (item.hint) row.setAttribute("aria-description", `${item.hint} this exception`);
        }
        if (!nested) row.addEventListener("pointerenter", () => this.closeSub(false));
        menu.append(row);
      } else if (item.type === "space") {
        menu.append(this.spaceRow(item));
      } else {
        const row = el("div", { className: "pm-item pm-parent", tabIndex: -1 });
        row.setAttribute("role", "menuitem");
        row.setAttribute("aria-haspopup", "menu");
        row.setAttribute("aria-expanded", "false");
        const mark = el("span", { className: "pm-check" });
        row.append(mark, el("span", { className: "pm-label", textContent: item.label }), el("span", { className: "pm-hint pm-arrow", textContent: "▸" }));
        row.addEventListener("click", () => this.openSub(row, item.items, true));
        row.addEventListener("pointerenter", () => this.openSub(row, item.items, false));
        menu.append(row);
      }
    }
  }

  private spaceRow(item: Extract<MenuItem<A>, { type: "space" }>): HTMLElement {
    const box = el("div", { className: "pm-space" });
    box.setAttribute("role", "none");
    const mark = el("span", { className: "pm-check", textContent: item.id ? "✓" : "" });
    mark.setAttribute("aria-hidden", "true");
    const input = el("input", { type: "text", value: item.value, autocomplete: "off", spellcheck: false, className: "pm-input" });
    input.setAttribute("role", "menuitem");
    input.setAttribute("aria-label", `${item.label} (a length like 12pt or 1em; Enter applies)`);
    const error = el("p", { className: "pm-error", hidden: true, id: "pm-space-error" });
    error.setAttribute("role", "alert");
    input.setAttribute("aria-describedby", "pm-space-error");
    const apply = () => {
      const h = this.handlers;
      if (!h) return;
      input.disabled = true;
      h.space(input.value.trim(), item.pos).then(
        () => this.close(),
        (e: unknown) => {
          input.disabled = false;
          error.textContent = e instanceof Error ? e.message : String(e);
          error.hidden = false;
          input.setAttribute("aria-invalid", "true");
          input.focus();
          input.select();
        },
      );
    };
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        e.stopPropagation();
        apply();
      }
    });
    input.addEventListener("focus", () => this.closeSub(false));
    const label = el("label", { className: "pm-label", textContent: item.label });
    label.addEventListener("click", () => input.focus());
    const line = el("div", { className: "pm-space-line" }, mark, label, input);
    if (item.id) {
      const id = item.id;
      const remove = el("button", { type: "button", className: "pm-remove", textContent: "Remove", tabIndex: -1 });
      remove.setAttribute("aria-label", "Remove the space before");
      remove.addEventListener("click", () => this.activate({ op: "remove", id } as A));
      line.append(remove);
    }
    box.append(line, error);
    return box;
  }

  private openSub(parent: HTMLElement, items: MenuItem<A>[], focus: boolean): void {
    if (this.sub?.dataset.parent === "1" && parent.getAttribute("aria-expanded") === "true") {
      if (focus) entries(this.sub)[0]?.focus();
      return;
    }
    this.closeSub(false);
    const sub = el("div", { className: "page-menu pm-sub", tabIndex: -1 });
    sub.setAttribute("role", "menu");
    sub.setAttribute("aria-label", parent.textContent?.replace("▸", "").trim() ?? "");
    sub.dataset.parent = "1";
    this.fill(sub, items, true);
    sub.addEventListener("keydown", (e) => this.key(e, sub));
    this.root!.append(sub);
    parent.setAttribute("aria-expanded", "true");
    const r = parent.getBoundingClientRect();
    const right = r.right + sub.offsetWidth + 4 <= window.innerWidth;
    sub.style.left = `${right ? r.right - 2 : r.left - sub.offsetWidth + 2}px`;
    sub.style.top = `${Math.max(4, Math.min(r.top - 4, window.innerHeight - sub.offsetHeight - 4))}px`;
    this.sub = sub;
    if (focus) entries(sub)[0]?.focus();
  }

  private closeSub(focusParent: boolean): void {
    if (!this.sub || !this.root) return;
    this.sub.remove();
    this.sub = null;
    const parent = this.root.querySelector<HTMLElement>(".pm-parent[aria-expanded='true']");
    parent?.setAttribute("aria-expanded", "false");
    if (focusParent) parent?.focus();
  }

  private activate(act: A): void {
    const h = this.handlers;
    this.close();
    h?.run(act);
  }

  private key(e: KeyboardEvent, menu: HTMLElement): void {
    if (e.target instanceof Node && menu !== e.target && !menu.contains(e.target)) return;
    // Events from the submenu bubble through the root: handle them once.
    if (menu === this.root && this.sub?.contains(e.target as Node)) return;
    const list = entries(menu);
    const i = list.indexOf(document.activeElement as HTMLElement);
    const isInput = document.activeElement instanceof HTMLInputElement;
    const move = (to: number) => {
      e.preventDefault();
      list[(to + list.length) % list.length]?.focus();
    };
    switch (e.key) {
      case "ArrowDown":
        move(i + 1);
        break;
      case "ArrowUp":
        move(i < 0 ? list.length - 1 : i - 1);
        break;
      case "Home":
        if (!isInput) move(0);
        break;
      case "End":
        if (!isInput) move(list.length - 1);
        break;
      case "ArrowRight":
        if (!isInput && document.activeElement?.classList.contains("pm-parent")) {
          e.preventDefault();
          (document.activeElement as HTMLElement).click();
        }
        break;
      case "ArrowLeft":
        if (!isInput && menu === this.sub) {
          e.preventDefault();
          this.closeSub(true);
        }
        break;
      case "Enter":
      case " ":
        if (!isInput && document.activeElement instanceof HTMLElement && list.includes(document.activeElement)) {
          e.preventDefault();
          document.activeElement.click();
        }
        break;
      case "Escape":
        e.preventDefault();
        e.stopPropagation();
        this.escape();
        break;
      case "Tab":
        e.preventDefault();
        this.close();
        break;
    }
  }
}

/** Puts the menu at (`x`, `y`), kept inside the window. */
function place(menu: HTMLElement, x: number, y: number): void {
  const w = menu.offsetWidth;
  const h = menu.offsetHeight;
  const left = x + w + 4 > window.innerWidth ? Math.max(4, x - w) : x;
  const top = y + h + 4 > window.innerHeight ? Math.max(4, window.innerHeight - h - 4) : y;
  menu.style.left = `${left}px`;
  menu.style.top = `${top}px`;
}
