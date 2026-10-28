// Spell check (the core's, see crates/spell): a wavy line under the visible
// words the dictionary of the layout's `text.lang` doesn't know. Only text
// counts: math, raw text, code, citations, labels and comments are skipped
// (their tokens, see typst.ts), and so are links, e-mail addresses, words
// with digits, all-caps words (acronyms) and single letters. Words the core
// hasn't answered yet go to it in one batch after a short pause; the answers
// are cached per language, so typing never waits on them.

import { syntaxTree } from "@codemirror/language";
import { type EditorState, type Extension, type Range, StateEffect } from "@codemirror/state";
import { Decoration, type DecorationSet, type EditorView, ViewPlugin, type ViewUpdate } from "@codemirror/view";
import { KIND, nodeKind } from "./typst.ts";

/** A word in the text, `from`..`to` exactly its letters. */
export interface Word {
  from: number;
  to: number;
  word: string;
}

/** Letters (and digits, to skip such words whole) with inner apostrophes or hyphens; the dictionaries split hyphenated words. */
const WORD = /[\p{L}\p{M}\p{N}]+(?:['’-][\p{L}\p{M}\p{N}]+)*/gu;
const DIGIT = /\p{N}/u;
const LINK = /(?:https?:\/\/|www\.)[^\s<>"\]]+|[\w.+-]+@[\w-]+(?:\.[\w-]+)+/gu;

/** Whether a token node is never checked: math, raw, code, citations, labels (`KIND.quiet`) and comments. */
function skipped(type: { id: number; name: string }): boolean {
  return (nodeKind(type).flags & KIND.quiet) !== 0 || type.name.split("_").includes("comment");
}

/** Whether `word` is worth checking: two letters or more, no digits, not all capitals. */
function checkable(word: string): boolean {
  if (DIGIT.test(word)) return false;
  if ([...word].length < 2) return false;
  return word !== word.toUpperCase() || word === word.toLowerCase();
}

/** The checkable words of the lines `from`..`to` touch, as far as the text is parsed. */
export function wordsIn(state: EditorState, from: number, to: number): Word[] {
  const { doc } = state;
  const tree = syntaxTree(state);
  from = doc.lineAt(from).from;
  to = Math.min(doc.lineAt(to).to, tree.length);
  const skips: { from: number; to: number }[] = [];
  tree.iterate({
    from,
    to,
    enter(node) {
      if (skipped(node.type)) skips.push({ from: node.from, to: node.to });
    },
  });
  const words: Word[] = [];
  let s = 0;
  for (let pos = from; pos <= to; ) {
    const line = doc.lineAt(pos);
    const links = [...line.text.matchAll(LINK)].map((m) => ({ from: line.from + m.index, to: line.from + m.index + m[0].length }));
    for (const m of line.text.matchAll(WORD)) {
      const w = { from: line.from + m.index, to: line.from + m.index + m[0].length, word: m[0] };
      if (w.to > to) break;
      while (s < skips.length && skips[s].to <= w.from) s++;
      if (s < skips.length && skips[s].from < w.to) continue;
      if (links.some((l) => l.from < w.to && w.from < l.to) || !checkable(w.word)) continue;
      words.push(w);
    }
    pos = line.to + 1;
  }
  return words;
}

/** The core's verdict: the misspelled ones of `words` in `lang`. */
export type CheckWords = (lang: string, words: string[]) => Promise<string[]>;

const misspelled = Decoration.mark({ class: "cm-misspelled" });
/** Answers arrived, the language or the ignored words changed: the marks are drawn again. */
const refresh = StateEffect.define<null>();
/** The pause after typing or scrolling before unknown words go to the core. */
const DELAY = 200;

/**
 * The spell checker. Its language, ignored words and answers stay when the
 * view loses and regains `extension` (View → Check spelling); `reset` forgets
 * the ignored words and the language (another project).
 */
export function spellChecker(check: CheckWords): {
  extension: Extension;
  /** The `text.lang` checked against (null or "": none). */
  readonly lang: string | null;
  setLang(lang: string | null): void;
  /** Words ignored until `reset` (this project, while it is open). */
  ignore(word: string): void;
  /** A word added to the dictionary: right in every language. */
  added(word: string): void;
  reset(): void;
  /** The misspelled word at `pos` (marked or not), if the checker is on. */
  misspelledAt(state: EditorState, pos: number): Word | null;
} {
  const cache = new Map<string, Map<string, boolean>>();
  /** Words sent and not answered yet, per language. */
  const asking = new Map<string, Set<string>>();
  const ignored = new Set<string>();
  let lang: string | null = null;
  let current: { view: EditorView } | null = null;

  const answers = (l: string) => {
    let m = cache.get(l);
    if (!m) cache.set(l, (m = new Map()));
    return m;
  };
  const redraw = () => current?.view.dispatch({ effects: refresh.of(null) });

  async function send(l: string, words: string[]): Promise<void> {
    let pending = asking.get(l);
    if (!pending) asking.set(l, (pending = new Set()));
    for (const w of words) pending.add(w);
    let wrong: Set<string>;
    try {
      wrong = new Set(await check(l, words));
    } catch (e) {
      // Counted as right, so a failing check isn't asked again and again.
      console.error("Lethora: spell check failed", e);
      wrong = new Set();
    }
    const known = answers(l);
    for (const w of words) {
      known.set(w, !wrong.has(w));
      pending.delete(w);
    }
    if (l === lang) redraw();
  }

  const extension = ViewPlugin.fromClass(
    class {
      view: EditorView;
      decorations: DecorationSet;
      /** The last change was typing: the word at the cursor waits until the cursor leaves it. */
      typing = false;
      unknown: string[] = [];
      timer: ReturnType<typeof setTimeout> | undefined;

      constructor(view: EditorView) {
        this.view = view;
        current = this;
        this.decorations = this.build();
      }

      update(u: ViewUpdate) {
        const wasTyping = this.typing;
        if (u.docChanged) this.typing = u.transactions.some((tr) => tr.isUserEvent("input.type") || tr.isUserEvent("delete"));
        else if (u.selectionSet) this.typing = false;
        if (
          u.docChanged ||
          u.viewportChanged ||
          (u.selectionSet && wasTyping) ||
          syntaxTree(u.state) !== syntaxTree(u.startState) ||
          u.transactions.some((tr) => tr.effects.some((e) => e.is(refresh)))
        ) {
          this.decorations = this.build();
        }
      }

      build(): DecorationSet {
        clearTimeout(this.timer);
        this.unknown = [];
        if (!lang) return Decoration.none;
        const known = answers(lang);
        const pending = asking.get(lang);
        const { state } = this.view;
        const head = state.selection.main.head;
        const marks: Range<Decoration>[] = [];
        const unknown = new Set<string>();
        for (const { from, to } of this.view.visibleRanges) {
          for (const w of wordsIn(state, from, to)) {
            if (ignored.has(w.word)) continue;
            const right = known.get(w.word);
            if (right === undefined) {
              if (!pending?.has(w.word)) unknown.add(w.word);
            } else if (!right && !(this.typing && w.from <= head && head <= w.to)) {
              marks.push(misspelled.range(w.from, w.to));
            }
          }
        }
        if (unknown.size) {
          const l = lang;
          const words = [...unknown];
          this.timer = setTimeout(() => void send(l, words), DELAY);
        }
        return Decoration.set(marks, true);
      }

      destroy() {
        clearTimeout(this.timer);
        if (current === this) current = null;
      }
    },
    { decorations: (v) => v.decorations },
  );

  return {
    extension,
    get lang() {
      return lang;
    },
    setLang(next) {
      if ((next || null) === lang) return;
      lang = next || null;
      redraw();
    },
    ignore(word) {
      ignored.add(word);
      redraw();
    },
    added(word) {
      for (const known of cache.values()) known.set(word, true);
      redraw();
    },
    reset() {
      ignored.clear();
      lang = null;
      redraw();
    },
    misspelledAt(state, pos) {
      if (!current || !lang) return null;
      const line = state.doc.lineAt(pos);
      const w = wordsIn(state, line.from, line.to).find((w) => w.from <= pos && pos <= w.to);
      return w && !ignored.has(w.word) && cache.get(lang)?.get(w.word) === false ? w : null;
    },
  };
}
