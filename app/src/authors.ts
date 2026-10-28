// "Who wrote this" in a server-shared project (View → Show authors): in the
// author and science views, a faint background per run of text in its
// author's colour, a tooltip with name and time, and a small legend. The core
// gives runs for a range of its text (`authors_get`; crates/core/src/server.rs,
// "Who wrote this"); the edit queue maps them to the editor's offsets (see
// `Sync.toEditor`). main.ts asks only for the visible part, again after edits
// and pulls.
//
// No Tauri imports: this runs under `node --test`.

import { type Range, StateEffect, StateField } from "@codemirror/state";
import { Decoration, type DecorationSet, EditorView } from "@codemirror/view";
import { displayName, safeColour } from "./presence.ts";

/** How long after an edit or a scroll the authors are asked for. */
export const AUTHORS_DELAY_MS = 300;

/** `authors_get`: consecutive characters by the same user (offsets into the core's text). */
export interface AuthorRun {
  from: number;
  to: number;
  /** Null for an unknown peer. */
  user: string | null;
  /** Unix seconds of the change; null if unknown. */
  time: number | null;
}

/** `authors_get`: a user of the runs. */
export interface AuthorInfo {
  user: string;
  name?: string;
  colour: string;
  /** This device's user. */
  you: boolean;
}

/** `authors_get`'s reply. */
export interface Authors {
  runs: AuthorRun[];
  authors: AuthorInfo[];
}

/** A run as the editor shows it. */
export interface AuthorMark {
  from: number;
  to: number;
  colour: string;
  /** The tooltip: name · date and time. */
  title: string;
}

/** An author in the legend. */
export interface LegendItem {
  label: string;
  colour: string;
}

const YOU = "You";
const UNKNOWN = "Unknown";
/** For text whose author isn't known. */
export const UNKNOWN_COLOUR = "#8f8a82";

const dateTime = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

/** Unix seconds as the tooltip shows them. */
export const formatTime = (secs: number): string => dateTime.format(new Date(secs * 1000));

const labelOf = (a: AuthorInfo | undefined): string => (!a ? UNKNOWN : a.you ? YOU : displayName(a));

/**
 * The runs to show, mapped by `toEditor` (core → editor offsets) and kept
 * within `length`; `when` formats a time.
 */
export function authorMarks(authors: Authors, toEditor: (pos: number) => number, length: number, when: (secs: number) => string = formatTime): AuthorMark[] {
  const byUser = new Map(authors.authors.map((a) => [a.user, a]));
  const place = (pos: number) => Math.max(0, Math.min(length, toEditor(pos)));
  const marks: AuthorMark[] = [];
  for (const run of authors.runs) {
    const from = place(run.from);
    const to = place(run.to);
    if (from >= to) continue;
    const author = run.user === null ? undefined : byUser.get(run.user);
    const label = labelOf(author);
    const colour = author ? safeColour(author.colour) : UNKNOWN_COLOUR;
    marks.push({ from, to, colour, title: run.time ? `${label} · ${when(run.time)}` : label });
  }
  return marks;
}

/** The legend: you first, the others by name, "Unknown" last if some text has no known author. */
export function legendOf(authors: Authors): LegendItem[] {
  const shown = [...authors.authors].sort((a, b) => Number(b.you) - Number(a.you) || labelOf(a).localeCompare(labelOf(b)) || a.user.localeCompare(b.user));
  const items = shown.map((a) => ({ label: labelOf(a), colour: safeColour(a.colour) }));
  const known = new Set(authors.authors.map((a) => a.user));
  if (authors.runs.some((r) => r.user === null || !known.has(r.user))) items.push({ label: UNKNOWN, colour: UNKNOWN_COLOUR });
  return items;
}

/** Shows `items` in the legend `el` (hidden without any). */
export function showLegend(el: HTMLElement, items: readonly LegendItem[]): void {
  el.replaceChildren(
    ...items.map((item) => {
      const row = document.createElement("span");
      row.textContent = item.label;
      row.style.setProperty("--author", item.colour);
      return row;
    }),
  );
  el.hidden = items.length === 0;
}

// ---- The editor's author backgrounds ----

const setMarks = StateEffect.define<AuthorMark[]>();

/** Replaces the author backgrounds (positions in the editor's current text). */
export const setAuthorMarks = (marks: AuthorMark[]): StateEffect<AuthorMark[]> => setMarks.of(marks);

function decorations(marks: readonly AuthorMark[]): DecorationSet {
  const ranges: Range<Decoration>[] = marks.map((m) => Decoration.mark({ class: "cm-author", attributes: { style: `--author: ${m.colour}`, title: m.title } }).range(m.from, m.to));
  return Decoration.set(ranges, true);
}

/** The author backgrounds: mapped over edits, replaced by `setAuthorMarks`. */
export const authorMarkField = StateField.define<readonly AuthorMark[]>({
  create: () => [],
  update(marks, tr) {
    let next = marks;
    if (tr.docChanged) {
      next = marks.map((m) => ({ ...m, from: tr.changes.mapPos(m.from, 1), to: tr.changes.mapPos(m.to, -1) })).filter((m) => m.from < m.to);
    }
    for (const effect of tr.effects) if (effect.is(setMarks)) next = effect.value;
    return next;
  },
  provide: (field) => EditorView.decorations.from(field, decorations),
});
