// Opening a damaged project: a change in this device's own log that can't be
// read (`project_open` reports `damaged`) asks whether to open the project at
// its last good state (the core keeps a copy of the damaged file in
// `backup/damaged/`). After opening, a notice says what was dropped, and
// which other devices' data was skipped. The wording is pure (tested under
// `node --test`); the DOM is used only by the classes.

import { formatSize } from "./assets.ts";

/** An own log cut back on open (the core's `Recovered`); paths are project-relative. */
export interface Recovered {
  file: string;
  /** Bytes dropped from its end. */
  dropped: number;
  /** The copy of the whole damaged log. */
  backup: string;
}

/** `damaged` of `project_open` / `project_last`: the project's folder and the damaged log. */
export interface Damaged {
  path: string;
  file: string;
}

/** The question the dialog asks about `file`. */
export function damagedQuestion(file: string): string {
  return `The last changes in ${file} can't be read. Lethora can open the project at the last good state: those changes are left out, and a copy of the damaged file is kept.`;
}

/** What a recovering open dropped, one sentence pair per log. */
export function recoveredNotice(list: readonly Recovered[]): string {
  return list
    .map((r) => `Opened at the last good state: the last ${formatSize(r.dropped)} of changes in ${r.file} could not be read and were dropped. A copy of the damaged file is in ${r.backup}.`)
    .join(" ");
}

/** The device (first 8 characters of its id, `main/` or `review/` before it) of a `skipped_on_open` entry (`"<dir>: device <id>: <why>"`). */
export function skippedDevice(entry: string): string {
  const m = /^(\w+): device ([0-9a-f]+):/.exec(entry);
  return m ? `${m[1]}/${m[2].slice(0, 8)}…` : entry;
}

/** The other devices' data skipped on open, by device file; empty without any. */
export function skippedNotice(list: readonly string[]): string {
  const files = [...new Set(list.map(skippedDevice))];
  if (!files.length) return "";
  const which = files.length === 1 ? "another device" : `${files.length} other devices`;
  return `Changes from ${which} (${files.join(", ")}) could not be read and were left out. Nothing was changed.`;
}

const byId = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

function button(text: string, className = ""): HTMLButtonElement {
  return Object.assign(document.createElement("button"), { type: "button", textContent: text, className });
}

/** The question for a damaged project. */
export class DamagedDialog {
  private dialog = byId<HTMLDialogElement>("damaged-dialog");
  private body = byId("damaged-body");

  /** Resolves true for "Open at the last good state", false for Cancel (or Escape). */
  ask(file: string): Promise<boolean> {
    const heading = Object.assign(document.createElement("h2"), { id: "damaged-heading", textContent: "Some changes can't be read" });
    const text = Object.assign(document.createElement("p"), { className: "damaged-text", textContent: damagedQuestion(file) });
    const cancel = button("Cancel");
    const go = button("Open at the last good state", "primary");
    const actions = Object.assign(document.createElement("div"), { className: "np-actions" });
    actions.append(cancel, go);
    this.body.replaceChildren(heading, text, actions);
    cancel.addEventListener("click", () => this.dialog.close());
    go.addEventListener("click", () => this.dialog.close("recover"));
    return new Promise((resolve) => {
      this.dialog.addEventListener("close", () => resolve(this.dialog.returnValue === "recover"), { once: true });
      this.dialog.returnValue = "";
      this.dialog.showModal();
      cancel.focus();
    });
  }
}

/** The title bar's notice after an open that recovered or skipped something; dismissable. */
export class OpenNotice {
  private el = byId("open-notice");
  private text = byId("open-notice-text");

  constructor() {
    byId("open-notice-close").addEventListener("click", () => this.hide());
  }

  /** Shows what the open recovered and skipped; hides without either. */
  show(recovered: readonly Recovered[] = [], skipped: readonly string[] = []): void {
    const text = [recoveredNotice(recovered), skippedNotice(skipped)].filter(Boolean).join(" ");
    this.text.textContent = text;
    this.el.title = skipped.join("\n");
    this.el.hidden = !text;
  }

  hide(): void {
    this.el.hidden = true;
  }
}
