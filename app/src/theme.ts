// The writing column and markup styles; colors come from CSS custom
// properties in style.css (light and dark).

import { EditorView } from "@codemirror/view";

const chip = {
  fontFamily: "var(--mono)",
  fontSize: "0.84em",
  backgroundColor: "var(--chip)",
  borderRadius: "4px",
  padding: "0.05em 0.25em",
  boxDecorationBreak: "clone",
  WebkitBoxDecorationBreak: "clone",
};

export const writingTheme = EditorView.theme({
  "&": { height: "100%", color: "var(--ink)", backgroundColor: "var(--paper)" },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": {
    fontFamily: "var(--serif)",
    fontSize: "var(--text-size)",
    lineHeight: "var(--line-spacing)",
    scrollbarWidth: "thin",
    scrollbarColor: "var(--scrollbar) transparent",
  },
  ".cm-content": {
    boxSizing: "border-box",
    width: "100%",
    maxWidth: "calc(68ch + 4rem)",
    margin: "0 auto",
    padding: "max(16vh, 4.5rem) 2rem 40vh",
    caretColor: "var(--accent)",
  },
  "&.cm-typewriter .cm-content": { paddingTop: "50vh", paddingBottom: "50vh" },
  ".cm-content ::selection, .cm-content::selection": { backgroundColor: "var(--selection)" },
  ".cm-line": { transition: "opacity 0.25s ease" },
  ".cm-dim": { opacity: "0.28" },
  // A blank line is a paragraph break: drawn as paragraph spacing, not a full line.
  ".cm-blank": { lineHeight: "var(--paragraph-gap)" },

  ".cm-h1, .cm-h2, .cm-h3, .cm-h4, .cm-h5, .cm-h6": { fontWeight: "600", lineHeight: "1.3" },
  ".cm-h1": { fontSize: "1.65em", paddingTop: "0.5em", paddingBottom: "0.2em" },
  ".cm-h2": { fontSize: "1.35em", paddingTop: "0.45em", paddingBottom: "0.15em" },
  ".cm-h3": { fontSize: "1.15em", paddingTop: "0.35em" },
  ".cm-h4": { fontSize: "1.05em", paddingTop: "0.3em" },
  ".cm-h5, .cm-h6": { fontSize: "1em", paddingTop: "0.25em" },

  ".cm-mk": { color: "var(--ink-faint)", fontWeight: "normal", fontStyle: "normal" },
  ".cm-strong": { fontWeight: "700" },
  ".cm-emph": { fontStyle: "italic" },
  ".cm-underline": { textDecoration: "underline", textUnderlineOffset: "0.15em" },
  ".cm-strike": { textDecoration: "line-through" },
  ".cm-underline.cm-strike": { textDecorationLine: "underline line-through" },
  // Raised or lowered without growing the line (relative, not vertical-align).
  ".cm-super, .cm-sub": { position: "relative", fontSize: "0.8em" },
  ".cm-super": { top: "-0.4em" },
  ".cm-sub": { top: "0.25em" },
  ".cm-raw": { fontFamily: "var(--mono)", fontSize: "0.84em" },
  // A formula is several tokens (delimiters, content per line): one chip, rounded at its ends.
  ".cm-math": { ...chip, borderRadius: "0", paddingLeft: "0", paddingRight: "0" },
  ".cm-math-open": { borderRadius: "4px 0 0 4px", paddingLeft: "0.25em" },
  ".cm-math-close": { borderRadius: "0 4px 4px 0", paddingRight: "0.25em" },
  ".cm-cite": { ...chip, color: "var(--accent-ink)", whiteSpace: "nowrap" },
  ".cm-label": { color: "var(--ink-faint)", fontSize: "0.88em", whiteSpace: "nowrap" },
  ".cm-comment": { color: "var(--ink-faint)", fontStyle: "italic" },
  ".cm-code": { fontFamily: "var(--mono)", fontSize: "0.84em", color: "var(--ink-soft)" },
  // Character styles (styles.ts): font and size inline; a missing style's text is marked like a detached figure.
  ".cm-styled-missing": { textDecoration: "underline dotted var(--warning)", textUnderlineOffset: "0.2em", textDecorationSkipInk: "none" },

  // Science view: rendered formulas (images), errors and the live preview.
  ".cm-math-render img": { display: "inline", cursor: "text" },
  ".cm-math-display": { display: "inline-block", width: "100%", textAlign: "center", padding: "0.35em 0" },
  ".cm-math-display img": { verticalAlign: "middle" },
  ".cm-math-error": {
    textDecoration: "underline wavy var(--error)",
    textDecorationSkipInk: "none",
    textUnderlineOffset: "0.2em",
  },
  ".cm-tooltip.cm-math-preview": {
    backgroundColor: "var(--paper-raised)",
    border: "1px solid var(--rule)",
    borderRadius: "6px",
    boxShadow: "var(--shadow)",
    padding: "0.4rem 0.6rem",
    maxWidth: "min(40rem, 90vw)",
    overflowX: "auto",
  },
  ".cm-math-preview img": { display: "block" },
  ".cm-math-preview-error": { fontFamily: "var(--ui)", fontSize: "12.5px", color: "var(--error)", whiteSpace: "pre-wrap" },

  // Science view: figure previews (image, caption, label).
  ".cm-figure": { display: "block", textAlign: "center", padding: "0.6em 0 0.4em", cursor: "text" },
  ".cm-figure-inline": { display: "inline-block", maxWidth: "100%", verticalAlign: "top" },
  ".cm-figure img": { display: "block", margin: "0 auto", maxWidth: "100%", height: "auto", borderRadius: "2px" },
  ".cm-figure-caption": { display: "block", marginTop: "0.45em", fontSize: "0.88em", color: "var(--ink-soft)", lineHeight: "1.45" },
  ".cm-figure-label": { fontFamily: "var(--mono)", fontSize: "0.8em", color: "var(--ink-faint)", whiteSpace: "nowrap" },
  // Placement: the figure's own (plain), an exception's (accent, dashed), a detached one's (warning, dotted).
  ".cm-figure-badge": {
    display: "inline-block",
    padding: "0 0.45em",
    border: "1px solid transparent",
    borderRadius: "1em",
    fontFamily: "var(--ui)",
    fontSize: "10.5px",
    lineHeight: "1.5",
    color: "var(--ink-soft)",
    background: "var(--chip)",
    whiteSpace: "nowrap",
    verticalAlign: "0.15em",
  },
  ".cm-figure-badge.exception": { color: "var(--accent-ink)", borderColor: "var(--accent)", borderStyle: "dashed", background: "transparent" },
  ".cm-figure-badge.detached": { color: "var(--warning)", borderColor: "var(--warning)", borderStyle: "dotted", background: "transparent", fontStyle: "italic" },
  ".cm-figure-missing": {
    display: "inline-block",
    padding: "1.5em 2em",
    border: "1px dashed var(--rule)",
    borderRadius: "6px",
    fontFamily: "var(--ui)",
    fontSize: "12.5px",
    color: "var(--ink-faint)",
    background: "var(--chip)",
  },
  ".cm-figure-loading": { minHeight: "6em", minWidth: "12em", boxSizing: "border-box" },
});
