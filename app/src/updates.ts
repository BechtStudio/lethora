// Updates (see updater.rs): Help → Check for updates… shows "up to date" or
// the error; a found update (also from the quiet check after start) asks
// first: its version and release notes, Install and restart / Later. While
// installing, the download's progress; errors stay in the dialog. The
// wording is pure (tested under `node --test`); the DOM is used only by the
// class.

import { formatSize } from "./assets.ts";

/** `update_check`: a newer version. */
export interface UpdateInfo {
  version: string;
  /** The release notes, plain text (shown as text, never as HTML). */
  notes: string;
  /** `YYYY-MM-DD`. */
  date: string | null;
}

/** `update-progress`. */
export interface UpdateProgress {
  downloaded: number;
  total: number | null;
}

/** How long after start the quiet check waits. */
export const UPDATE_CHECK_DELAY_MS = 5000;

export const upToDate = (version: string): string => `Lethora is up to date (version ${version}).`;

export const availableHeading = (info: UpdateInfo): string => `Lethora ${info.version} is available`;

/** The line under the heading: the release date, if known. */
export const releasedLine = (info: UpdateInfo): string => (info.date ? `Released ${info.date}.` : "");

/** The notes (the CHANGELOG section, Markdown) as text: headings lose their `#`s. */
export const notesText = (info: UpdateInfo): string => info.notes.replace(/^#+[ \t]*/gm, "").trim() || "No release notes.";

/** The download's state: a percentage, the size so far without a total, then installing. */
export function progressText(p: UpdateProgress): string {
  if (p.total && p.downloaded >= p.total) return "Installing…";
  if (p.total) return `Downloading… ${Math.floor((p.downloaded * 100) / p.total)}%`;
  return `Downloading… ${formatSize(p.downloaded)}`;
}

const byId = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

function el<K extends keyof HTMLElementTagNameMap>(tag: K, props: Partial<HTMLElementTagNameMap[K]> = {}, ...children: Array<Node | string>): HTMLElementTagNameMap[K] {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...children);
  return e;
}

export class UpdateDialog {
  private dialog = byId<HTMLDialogElement>("update-dialog");
  private body = byId("update-body");
  /** The note under the notes while installing (null: not installing). */
  private progress: HTMLElement | null = null;
  private install: () => Promise<void>;

  /** `install`: sends every edit, then downloads, installs and restarts; settles only on errors. */
  constructor(install: () => Promise<void>) {
    this.install = install;
    // No Escape while installing (the app is about to restart).
    this.dialog.addEventListener("cancel", (e) => {
      if (this.progress) e.preventDefault();
    });
  }

  /** Help → Check for updates…, while it checks. */
  checking(): void {
    this.message([el("p", { className: "update-note", role: "status", textContent: "Checking for updates…" })]);
  }

  upToDate(version: string): void {
    this.message([el("p", { role: "status", textContent: upToDate(version) })]);
  }

  failed(e: unknown): void {
    this.message([el("p", { className: "update-error", role: "alert", textContent: String(e) })]);
  }

  available(info: UpdateInfo): void {
    if (this.progress) return;
    const notes = el("div", { className: "update-notes", tabIndex: 0, textContent: notesText(info) });
    const note = el("p", { className: "update-note", role: "status" });
    const error = el("p", { className: "update-error", role: "alert", hidden: true });
    const later = el("button", { type: "button", textContent: "Later" });
    const install = el("button", { type: "button", className: "primary", textContent: "Install and restart" });
    later.addEventListener("click", () => this.dialog.close());
    install.addEventListener("click", () => {
      later.disabled = install.disabled = true;
      error.hidden = true;
      note.textContent = "Saving your project…";
      this.progress = note;
      this.install().catch((e) => {
        this.progress = null;
        note.textContent = "";
        error.textContent = String(e);
        error.hidden = false;
        later.disabled = install.disabled = false;
      });
    });
    const released = releasedLine(info);
    this.show(availableHeading(info), [...(released ? [el("p", { className: "update-note", textContent: released })] : []), notes, note, error], [later, install]);
    install.focus();
  }

  /** An `update-progress` event. */
  onProgress(p: UpdateProgress): void {
    if (this.progress) this.progress.textContent = progressText(p);
  }

  /** A message with a Close button (not over an update being installed). */
  private message(content: Node[]): void {
    if (this.progress) return;
    const close = el("button", { type: "button", className: "primary", textContent: "Close" });
    close.addEventListener("click", () => this.dialog.close());
    this.show("Check for updates", content, [close]);
    close.focus();
  }

  private show(heading: string, content: Node[], actions: HTMLButtonElement[]): void {
    this.body.replaceChildren(el("h2", { id: "update-heading", textContent: heading }), ...content, el("div", { className: "np-actions" }, ...actions));
    if (!this.dialog.open) this.dialog.showModal();
  }
}
