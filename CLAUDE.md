# Lethora Guidelines

## Working rules

- Plan in the main session, together with me. Hand grunt work (broad searches,
  repetitive edits, boilerplate, log digging) to subagents on lesser models:
  Sonnet for searches, triage, and trivial mechanical work, and Opus for
  writing code. Keep decisions, architecture, and final review in the main
  session.
- Always look for the simplest solution first, and prefer it. The smallest
  change that solves the actual problem beats a bigger design. Extend existing
  patterns before inventing new ones. No new dependencies or moving parts
  without a real reason.
- Show me a checklist while you work (use the todo list tool), kept current,
  so I can see what you are working on, what is done, and what is next.
- When you spawn a subagent, tell me at that moment: which model it runs on
  and what it is doing. Report what it came back with when it finishes.
- Never use Haiku.

## Tech Stack

- **Core:** Rust (stable, edition 2024), Cargo workspace (`crates/*`, `app/`).
  `crates/core` is UI-free: document, history, persistence.
- **State/Data:** Loro CRDT (`loro` 1.x) is the single source of truth.
  Content is plain text in Typst markup (subset). Undo/redo via Loro's
  UndoManager; autosave = append-only update log + periodic snapshots;
  named/auto checkpoints = stored version frontiers. Restoring a checkpoint is a
  new change, never destructive. Layout overrides anchor via Loro cursors.
- **UI/Layout:** Tauri 2 shell with a thin web view; CodeMirror 6 as the editing
  surface (its own history disabled; all edits route through the Rust core).
  Typst (as a library) for math, citations, pagination, and PDF export, run
  incrementally off the editing thread.
- **Native Modules:** none (WebView2 on Windows).
- **Backend:** none; local-first project folders. Collaboration/sync later by
  exchanging Loro updates.
- **Layering:** content (author + science constructs, one text) → layout layer
  (publisher template + anchored overrides) → render. Lower layers never depend
  on upper ones; content never contains layout.
  - Content holds *what* and *relative size* (e.g. `width: 80%` of the text
    column, captions, labels, optional placement wishes).
  - The layout layer holds the page and all global style (page, margins,
    columns, typography, heading/figure/caption/citation style, floats) plus
    anchored per-element exceptions. It lives in the Loro document as
    structured settings (presets + key settings + optional expert Typst) and
    is turned into a generated Typst preamble only at compile time.
  - Views render relative sizes against the layout's page and never move
    things; only the paged publisher view places floats. The author view never
    renders layout, with one exception: text with a character style shows in
    its font, at a size relative to the body (Word principle, decided
    2026-10-21).

## Code Style & Naming

-

## Token & Output Rules

- Output minimal explanations. Return updated code directly.
- Modify only explicitly requested files or folders.
- Do not rewrite untouched imports/functions (ask first if widening context is necessary).
- Suggest a 3-bullet plan before executing changes on multi-file edits.

## Primary Commands

-