// Writing modes: focus (dim everything but the current paragraph) and
// typewriter (keep the cursor line vertically centred).

import { EditorState, type Range, type Text, type Transaction } from "@codemirror/state";
import { Decoration, type DecorationSet, EditorView, ViewPlugin, type ViewUpdate } from "@codemirror/view";

const isBlank = (s: string) => /^\s*$/.test(s);

/**
 * The line numbers (inclusive) of the paragraph containing `pos`: the run of
 * non-blank lines around it (a blank line is its own paragraph). The scan is
 * bounded to `limit` lines in each direction.
 */
export function paragraphAt(doc: Text, pos: number, limit = 1000): { from: number; to: number } {
  const line = doc.lineAt(pos);
  let from = line.number;
  let to = line.number;
  if (isBlank(line.text)) return { from, to };
  while (from > 1 && line.number - from < limit && !isBlank(doc.line(from - 1).text)) from--;
  while (to < doc.lines && to - line.number < limit && !isBlank(doc.line(to + 1).text)) to++;
  return { from, to };
}

const dim = Decoration.line({ class: "cm-dim" });

function build(view: EditorView): DecorationSet {
  const { doc, selection } = view.state;
  const current = paragraphAt(doc, selection.main.head);
  const ranges: Range<Decoration>[] = [];
  let next = 0;
  for (const { from, to } of view.visibleRanges) {
    for (let pos = Math.max(from, next); pos <= to; ) {
      const line = doc.lineAt(pos);
      if (line.number < current.from || line.number > current.to) ranges.push(dim.range(line.from));
      pos = next = line.to + 1;
    }
  }
  return Decoration.set(ranges);
}

/** Dims every visible line outside the paragraph with the main cursor. */
export const focusMode = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = build(view);
    }
    update(update: ViewUpdate) {
      if (update.docChanged || update.viewportChanged || update.selectionSet) this.decorations = build(update.view);
    }
  },
  { decorations: (v) => v.decorations },
);

/** Typing, deleting and keyboard cursor moves (not mouse selection, which would jump under the pointer). */
function recentres(tr: Transaction): boolean {
  if (!tr.docChanged && !tr.selection) return false;
  if (tr.isUserEvent("input") || tr.isUserEvent("delete") || tr.isUserEvent("undo") || tr.isUserEvent("redo")) return true;
  return tr.isUserEvent("select") && !tr.isUserEvent("select.pointer");
}

/** Keeps the cursor line centred; the editor class adds padding so the first/last lines can be centred too. */
export const typewriterMode = [
  EditorView.editorAttributes.of({ class: "cm-typewriter" }),
  EditorState.transactionExtender.of((tr) =>
    recentres(tr) ? { effects: EditorView.scrollIntoView(tr.newSelection.main.head, { y: "center" }) } : null,
  ),
];
