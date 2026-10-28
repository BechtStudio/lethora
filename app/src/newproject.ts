// The "New project" dialog: a name (the project goes into the default
// projects folder, or a place chosen with "Choose location…"), a preset
// (cards) and an optional title and author.

import type { PresetInfo } from "./layoutpanel.ts";

export interface NewProjectChoice {
  preset: string;
  /** Non-empty fields only. */
  meta: Record<string, string>;
  /** The name in the default projects folder (when no `path` was chosen). */
  name: string;
  /** The place chosen with "Choose location…", else null. */
  path: string | null;
}

const byId = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const DEFAULT_NAME = "Untitled";

export class NewProjectDialog {
  private dialog = byId<HTMLDialogElement>("new-project");
  private form = byId<HTMLFormElement>("np-form");
  private list = byId("np-preset-list");
  private pathEl = byId("np-path");
  private name = byId<HTMLInputElement>("np-name");
  private location = byId<HTMLButtonElement>("np-location");
  private title = byId<HTMLInputElement>("np-title");
  private author = byId<HTMLInputElement>("np-author");
  private resolve: ((choice: NewProjectChoice | null) => void) | null = null;
  private path: string | null = null;
  /** The name follows the title until the user edits it. */
  private nameEdited = false;
  private pickLocation: (name: string) => Promise<string | null> = async () => null;

  constructor() {
    this.form.addEventListener("submit", (e) => {
      e.preventDefault();
      const preset = this.form.querySelector<HTMLInputElement>("input[name=np-preset]:checked")?.value;
      if (!preset) return;
      const meta: Record<string, string> = {};
      if (this.title.value.trim()) meta.title = this.title.value.trim();
      if (this.author.value.trim()) meta.author = this.author.value.trim();
      this.finish({ preset, meta, name: this.name.value.trim() || DEFAULT_NAME, path: this.path });
    });
    this.name.addEventListener("input", () => (this.nameEdited = true));
    this.title.addEventListener("input", () => {
      if (!this.nameEdited && this.path === null) this.name.value = this.title.value.trim() || DEFAULT_NAME;
    });
    this.location.addEventListener("click", () => void this.chooseLocation());
    byId("np-cancel").addEventListener("click", () => this.finish(null));
    // Escape closes the dialog natively.
    this.dialog.addEventListener("close", () => this.finish(null));
  }

  /**
   * Asks for the name, preset and metadata of a new project in `folder` (the
   * default projects folder); `pickLocation` asks for another place (a save
   * dialog). `prefill` fills title and author (an import's; the name follows
   * the title). `null` if cancelled.
   */
  ask(folder: string, presets: PresetInfo[], pickLocation: (name: string) => Promise<string | null>, prefill: { title?: string; author?: string } = {}): Promise<NewProjectChoice | null> {
    this.finish(null);
    this.pickLocation = pickLocation;
    this.path = null;
    this.nameEdited = false;
    this.title.value = prefill.title ?? "";
    this.author.value = prefill.author ?? "";
    this.name.value = this.title.value.trim() || DEFAULT_NAME;
    this.name.disabled = false;
    this.showWhere(`Saved in ${folder}`);
    this.list.replaceChildren(
      ...presets.map((p, i) => {
        const input = Object.assign(document.createElement("input"), { type: "radio", name: "np-preset", value: p.id, checked: i === 0 });
        const name = Object.assign(document.createElement("span"), { className: "np-name", textContent: p.name });
        const description = Object.assign(document.createElement("span"), { className: "np-description", textContent: p.description });
        const card = document.createElement("label");
        card.className = "np-card";
        card.append(input, name, description);
        return card;
      }),
    );
    this.dialog.showModal();
    this.name.select();
    return new Promise((resolve) => (this.resolve = resolve));
  }

  private showWhere(text: string): void {
    this.pathEl.textContent = text;
    this.pathEl.title = text;
  }

  /** "Choose location…": the project goes where the save dialog says, named after it. */
  private async chooseLocation(): Promise<void> {
    const path = await this.pickLocation(this.name.value.trim() || DEFAULT_NAME);
    if (!path || !this.dialog.open) return;
    this.path = path;
    this.name.value = path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? path;
    this.name.disabled = true;
    this.showWhere(path);
  }

  private finish(choice: NewProjectChoice | null): void {
    const resolve = this.resolve;
    this.resolve = null;
    if (this.dialog.open) this.dialog.close();
    resolve?.(choice);
  }
}
