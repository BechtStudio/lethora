// Checkpoint drawer: create named checkpoints, list them, preview one in a
// read-only editor overlaying the live one, and restore it.
//
// No Tauri imports: commands come in through `CheckpointApi`.

import { EditorState, type Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { Sync } from "./sync.ts";

export interface CheckpointInfo {
  id: number;
  name: string | null;
  auto: boolean;
  /** Unix seconds. */
  created: number;
}

export interface CheckpointApi {
  list(): Promise<CheckpointInfo[]>;
  create(name: string | null): Promise<CheckpointInfo>;
  text(id: number): Promise<string>;
}

export interface PanelHost {
  api: CheckpointApi;
  /** The live project's edit queue (null without a project). */
  sync(): Sync | null;
  /** The live editor's element, hidden and inert while a preview shows. */
  editorEl: HTMLElement;
  /** Theme and decorations for the read-only preview. */
  previewExtensions: Extension;
  info(message: string): void;
  error(e: unknown): void;
  /** Number of errors reported so far (to tell whether a restore failed). */
  errorCount(): number;
  focusEditor(): void;
  onOpenChange(open: boolean): void;
}

/** "5 min ago" style, from unix seconds. */
export function relativeTime(created: number, nowMs = Date.now()): string {
  const s = Math.max(0, Math.round(nowMs / 1000 - created));
  if (s < 45) return "just now";
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min ago`;
  const h = Math.round(s / 3600);
  if (h < 24) return h === 1 ? "1 hour ago" : `${h} hours ago`;
  const d = Math.round(s / 86400);
  if (d === 1) return "yesterday";
  if (d < 30) return `${d} days ago`;
  return new Date(created * 1000).toLocaleDateString(undefined, { dateStyle: "medium" });
}

const absoluteTime = (created: number) =>
  new Date(created * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });

const label = (c: CheckpointInfo) => c.name ?? (c.auto ? "Automatic" : "Checkpoint");

const byId = (id: string) => document.getElementById(id)!;

export class CheckpointPanel {
  private host: PanelHost;
  private panel = byId("panel");
  private form = byId("cp-form") as HTMLFormElement;
  private nameInput = byId("cp-name") as HTMLInputElement;
  private list = byId("cp-list");
  private previewEl = byId("preview");
  private previewTitle = byId("preview-title");
  private restoreButton = byId("preview-restore") as HTMLButtonElement;
  private previewView: EditorView | null = null;
  private shown: CheckpointInfo | null = null;
  /** Ignores answers to superseded preview requests. */
  private previewToken = 0;

  constructor(host: PanelHost) {
    this.host = host;
    this.form.addEventListener("submit", (e) => {
      e.preventDefault();
      void this.create();
    });
    this.list.addEventListener("click", (e) => {
      const item = (e.target as HTMLElement).closest<HTMLElement>("[data-id]");
      const c = item && this.items.find((c) => c.id === Number(item.dataset.id));
      if (c) void this.preview(c);
    });
    this.restoreButton.addEventListener("click", () => void this.restore());
    byId("preview-close").addEventListener("click", () => this.closePreview());
  }

  private items: CheckpointInfo[] = [];

  get isOpen(): boolean {
    return !this.panel.hidden;
  }

  toggle(): void {
    if (this.isOpen) this.close();
    else this.open();
  }

  open(): void {
    if (!this.host.sync()) return;
    if (!this.isOpen) {
      this.panel.hidden = false;
      this.host.onOpenChange(true);
    }
    void this.refresh();
  }

  /** Opens the drawer with the name field focused. */
  openForCreate(): void {
    this.open();
    if (!this.isOpen) return;
    this.nameInput.focus();
    this.nameInput.select();
  }

  close(): void {
    this.closePreview(false);
    if (!this.isOpen) return;
    this.panel.hidden = true;
    this.host.onOpenChange(false);
    this.host.focusEditor();
  }

  /** The list may have changed (e.g. another device's named checkpoint): reloads it if shown. */
  reload(): void {
    if (this.isOpen) void this.refresh();
  }

  /** Esc: closes the preview, else the drawer. Returns whether it did anything. */
  escape(): boolean {
    if (!this.previewEl.hidden) this.closePreview();
    else if (this.isOpen) this.close();
    else return false;
    return true;
  }

  private async refresh(): Promise<void> {
    try {
      const items = await this.host.api.list();
      this.items = items.sort((a, b) => b.created - a.created || b.id - a.id);
      this.render();
    } catch (e) {
      this.host.error(e);
    }
  }

  private render(): void {
    const now = Date.now();
    this.list.replaceChildren(
      ...this.items.map((c) => {
        const li = document.createElement("li");
        const button = document.createElement("button");
        button.type = "button";
        button.dataset.id = String(c.id);
        button.className = c.auto ? "cp-item cp-auto" : "cp-item cp-named";
        if (this.shown?.id === c.id) button.setAttribute("aria-current", "true");
        const name = document.createElement("span");
        name.className = "cp-name";
        name.textContent = label(c);
        const time = document.createElement("time");
        time.dateTime = new Date(c.created * 1000).toISOString();
        time.title = absoluteTime(c.created);
        time.textContent = relativeTime(c.created, now);
        button.title = `Preview ${label(c)} (${absoluteTime(c.created)})`;
        button.append(name, time);
        li.append(button);
        return li;
      }),
    );
    if (this.items.length === 0) {
      const li = document.createElement("li");
      li.className = "cp-empty";
      li.textContent = "No checkpoints yet.";
      this.list.append(li);
    }
  }

  private async create(): Promise<void> {
    const sync = this.host.sync();
    if (!sync) return;
    const name = this.nameInput.value.trim() || null;
    try {
      // The checkpoint must include every edit typed so far.
      await sync.drain();
      const c = await this.host.api.create(name);
      this.nameInput.value = "";
      this.host.info(`Checkpoint “${label(c)}” created`);
      await this.refresh();
    } catch (e) {
      this.host.error(e);
    }
  }

  private async preview(c: CheckpointInfo): Promise<void> {
    const token = ++this.previewToken;
    try {
      const text = await this.host.api.text(c.id);
      if (token !== this.previewToken || !this.isOpen) return;
      const state = EditorState.create({
        doc: text,
        extensions: [this.host.previewExtensions, EditorState.readOnly.of(true), EditorView.editable.of(false)],
      });
      if (this.previewView) this.previewView.setState(state);
      else this.previewView = new EditorView({ parent: byId("preview-doc"), state });
      this.shown = c;
      this.previewTitle.textContent = `Checkpoint ‘${label(c)}’ · ${relativeTime(c.created)}`;
      this.previewTitle.title = absoluteTime(c.created);
      // Keep the live editor out of reach so nothing is typed into it unseen.
      this.host.editorEl.inert = true;
      this.host.editorEl.classList.add("behind-preview");
      this.previewEl.hidden = false;
      this.render();
      this.restoreButton.focus();
    } catch (e) {
      this.host.error(e);
    }
  }

  closePreview(focusEditor = true): void {
    this.previewToken++;
    if (this.previewEl.hidden) return;
    this.previewEl.hidden = true;
    this.shown = null;
    this.host.editorEl.inert = false;
    this.host.editorEl.classList.remove("behind-preview");
    this.render();
    if (focusEditor) this.host.focusEditor();
  }

  private async restore(): Promise<void> {
    const sync = this.host.sync();
    const c = this.shown;
    if (!sync || !c) return;
    this.restoreButton.disabled = true;
    const errors = this.host.errorCount();
    try {
      await sync.restore(c.id); // drains pending edits first
      this.closePreview();
      if (this.host.errorCount() === errors) this.host.info("Restored — Ctrl+Z to undo");
      await this.refresh();
    } catch (e) {
      this.host.error(e);
    } finally {
      this.restoreButton.disabled = false;
    }
  }
}
