// Formula palette for the science view, and the math insert commands.
// Inserts are ordinary editor transactions (they sync to the core like typing).

import { EditorSelection, type EditorState, type TransactionSpec } from "@codemirror/state";
import { ensureSyntaxTree } from "@codemirror/language";
import type { EditorView } from "@codemirror/view";
import { mathAt } from "./math.ts";

export interface Snippet {
  /** Button label (Unicode text). */
  label: string;
  /** Typst math to insert. */
  insert: string;
  /** Placeholder in `insert` selected after inserting (first whole-word occurrence). */
  select?: string;
  /** Tooltip; defaults to `insert`. */
  title?: string;
}

export interface SnippetGroup {
  name: string;
  items: Snippet[];
}

const sym = (label: string, insert: string): Snippet => ({ label, insert });

export const PALETTE: SnippetGroup[] = [
  {
    name: "Greek",
    items: [
      ...["alpha α", "beta β", "gamma γ", "delta δ", "epsilon ε", "zeta ζ", "eta η", "theta θ", "kappa κ", "lambda λ", "mu μ", "nu ν"],
      ...["xi ξ", "pi π", "rho ρ", "sigma σ", "tau τ", "phi φ", "chi χ", "psi ψ", "omega ω"],
      ...["Gamma Γ", "Delta Δ", "Theta Θ", "Lambda Λ", "Pi Π", "Sigma Σ", "Phi Φ", "Psi Ψ", "Omega Ω"],
    ].map((s) => {
      const [insert, label] = s.split(" ");
      return sym(label, insert);
    }),
  },
  {
    name: "Operators",
    items: [
      { label: "∑", insert: "sum_(i=1)^n", select: "i=1", title: "Sum" },
      { label: "∏", insert: "product_(i=1)^n", select: "i=1", title: "Product" },
      { label: "∫", insert: "integral_a^b", select: "a", title: "Integral" },
      { label: "lim", insert: "lim_(x -> 0)", select: "x -> 0", title: "Limit" },
      sym("±", "plus.minus"),
      sym("×", "times"),
      sym("·", "dot"),
      sym("÷", "div"),
      sym("∂", "partial"),
      sym("∇", "nabla"),
      sym("∞", "infinity"),
    ],
  },
  {
    name: "Relations",
    items: [
      sym("≠", "!="),
      sym("≤", "<="),
      sym("≥", ">="),
      sym("≈", "approx"),
      sym("≡", "equiv"),
      sym("∼", "tilde.op"),
      sym("∝", "prop"),
      sym("∈", "in"),
      sym("∉", "in.not"),
      sym("⊂", "subset"),
      sym("⊆", "subset.eq"),
      sym("∪", "union"),
      sym("∩", "inter"),
      sym("∀", "forall"),
      sym("∃", "exists"),
    ],
  },
  {
    name: "Arrows",
    items: [
      sym("→", "->"),
      sym("←", "<-"),
      sym("↔", "<->"),
      sym("⇒", "=>"),
      sym("⇐", "arrow.l.double"),
      sym("⇔", "<=>"),
      sym("↦", "|->"),
      sym("↑", "arrow.t"),
      sym("↓", "arrow.b"),
    ],
  },
  {
    name: "Structures",
    items: [
      { label: "a⁄b", insert: "frac(a, b)", select: "a", title: "Fraction" },
      { label: "√x", insert: "sqrt(x)", select: "x", title: "Square root" },
      { label: "ⁿ√x", insert: "root(n, x)", select: "n", title: "Root" },
      { label: "xⁿ", insert: "x^n", select: "x", title: "Power" },
      { label: "xᵢ", insert: "x_i", select: "x", title: "Subscript" },
      { label: "(⋮)", insert: "vec(a, b, c)", select: "a", title: "Vector" },
      { label: "[⋯]", insert: "mat(a, b; c, d)", select: "a", title: "Matrix" },
      { label: "{⋯", insert: 'cases(a "if" x > 0, b "otherwise")', select: "a", title: "Cases" },
      { label: "(ⁿₖ)", insert: "binom(n, k)", select: "n", title: "Binomial" },
      { label: "|x|", insert: "abs(x)", select: "x", title: "Absolute value" },
      { label: "‖x‖", insert: "norm(x)", select: "x", title: "Norm" },
    ],
  },
];

/** Letters and digits: math names (`_` and `^` attach sub/superscripts). */
const WORD_CHAR = /[\p{L}\p{N}]/u;

/** First occurrence of `word` in `text` not inside a longer identifier (`a` in `frac(a, b)`). */
export function findPlaceholder(text: string, word: string): number {
  for (let at = text.indexOf(word); at >= 0; at = text.indexOf(word, at + 1)) {
    if (!WORD_CHAR.test(text.charAt(at - 1)) && !WORD_CHAR.test(text.charAt(at + word.length))) return at;
  }
  return -1;
}

/** Whether `pos` is inside an equation (between its `$` delimiters). */
export function insideMath(state: EditorState, pos: number): boolean {
  ensureSyntaxTree(state, Math.min(state.doc.length, pos + 4000), 100);
  return mathAt(state, pos) !== null;
}

/**
 * Inserts `snippet` at the main selection: as is inside an equation (spaced
 * from neighbouring identifiers), else wrapped in `$…$`. A selected text takes
 * the placeholder's place; otherwise the placeholder is selected.
 */
export function insertSnippet(state: EditorState, snippet: Snippet): TransactionSpec {
  const { from, to } = state.selection.main;
  const selected = state.sliceDoc(from, to);
  let body = snippet.insert;
  let selStart = -1;
  let selEnd = -1;
  if (snippet.select) {
    const at = findPlaceholder(body, snippet.select);
    if (at >= 0) {
      if (selected && !selected.includes("\n")) {
        body = body.slice(0, at) + selected + body.slice(at + snippet.select.length);
        selStart = selEnd = at + selected.length;
      } else {
        selStart = at;
        selEnd = at + snippet.select.length;
      }
    }
  }
  if (selStart < 0) selStart = selEnd = body.length;

  let before: string;
  let after: string;
  if (insideMath(state, from)) {
    before = WORD_CHAR.test(state.sliceDoc(from - 1, from)) ? " " : "";
    after = WORD_CHAR.test(state.sliceDoc(to, to + 1)) ? " " : "";
  } else {
    before = "$";
    after = "$";
  }
  const insert = before + body + after;
  const base = from + before.length;
  return {
    changes: { from, to, insert },
    selection: EditorSelection.single(base + selStart, base + selEnd),
    scrollIntoView: true,
    userEvent: "input",
  };
}

/** `eq:N` with the smallest N ≥ 1 not taken. */
export function equationLabel(taken: (name: string) => boolean): string {
  for (let n = 1; ; n++) if (!taken(`eq:${n}`)) return `eq:${n}`;
}

/**
 * Inline `$|$` or display `$ | $` math at the main selection (wrapping a
 * selected text); a display formula gets `label` (`$ | $ <eq:1>`) if given.
 */
export function insertMath(state: EditorState, display: boolean, label?: string): TransactionSpec {
  const { from, to } = state.selection.main;
  // Already in a formula: nesting `$` would break it.
  if (insideMath(state, from)) return {};
  const selected = state.sliceDoc(from, to);
  const pad = display ? " " : "";
  const insert = `$${pad}${selected}${pad}$${display && label ? ` <${label}>` : ""}`;
  const start = from + 1 + pad.length;
  return {
    changes: { from, to, insert },
    selection: EditorSelection.single(start, start + selected.length),
    scrollIntoView: true,
    userEvent: "input",
  };
}

// ---- Panel ----

/** The palette panel (element `#palette`); buttons keep the editor's focus and selection. */
export class Palette {
  private el: HTMLElement;
  private view: () => EditorView;
  private onOpenChange: (open: boolean) => void;

  constructor(el: HTMLElement, view: () => EditorView, onOpenChange: (open: boolean) => void) {
    this.el = el;
    this.view = view;
    this.onOpenChange = onOpenChange;
    const snippets: Snippet[] = [];
    for (const group of PALETTE) {
      const section = document.createElement("section");
      const heading = document.createElement("h2");
      heading.textContent = group.name;
      const grid = document.createElement("div");
      grid.className = "palette-grid";
      for (const item of group.items) {
        const button = document.createElement("button");
        button.type = "button";
        button.textContent = item.label;
        button.title = item.title ? `${item.title}: ${item.insert}` : item.insert;
        button.dataset.index = String(snippets.length);
        snippets.push(item);
        grid.append(button);
      }
      section.append(heading, grid);
      el.append(section);
    }
    // Keep the editor focused (and its selection) while clicking.
    el.addEventListener("mousedown", (e) => {
      if ((e.target as HTMLElement).closest("button")) e.preventDefault();
    });
    el.addEventListener("click", (e) => {
      const button = (e.target as HTMLElement).closest<HTMLElement>("button[data-index]");
      if (!button) return;
      const v = this.view();
      v.dispatch(insertSnippet(v.state, snippets[Number(button.dataset.index)]));
      v.focus();
    });
  }

  get isOpen(): boolean {
    return !this.el.hidden;
  }

  toggle(): void {
    this.set(!this.isOpen);
  }

  /** Esc: closes the palette. Returns whether it did anything. */
  escape(): boolean {
    if (!this.isOpen) return false;
    this.set(false);
    this.view().focus();
    return true;
  }

  set(open: boolean): void {
    if (open === this.isOpen) return;
    this.el.hidden = !open;
    this.onOpenChange(open);
  }
}
