# Milestone 4: upgrade batches and docs

The finished Milestone 4 upgrade batches A–G, the docs work and their known limitations, moved out of the TODO.

0. Done: paragraphs follow Typst (Enter/Shift+Enter/Backspace/Delete,
   paragraph-aware block starts), hand-tested and pushed (`8d5bde7`).
   Known: exceptions made before this on a mid-paragraph line apply at the
   paragraph start but show no checkmark in the menu (re-adding fixes it);
   Enter inside a content argument (`#quote[..]`, captions) makes a
   paragraph break there.
1. The user's upgrade list, in batches (decisions made 2026-10-13):
   - [x] A: view switcher fixed in the bar's centre; Ctrl+wheel zoom in the
         publisher; Compile button next to Export PDF + F5 (forces a
         recompile, re-reads images and references.bib; Ctrl+R stays
         blocked); display formulas inserted via Ctrl+Shift+M/palette get
         `<eq:N>` (next free number), inline ones no label
   - [x] B: heading sizes L1, L2, "L3 and below" (levels 3–6); space above
         and below headings (one pair); TOC settings: depth 1–4, title, dot
         leaders, indent levels, entry spacing, bold top-level entries
   - [x] C: right-click targets: inside a figure's box → the figure;
         else the nearest text line within ~1 line height, x clamped into
         the line; never snapping from page furniture into body text
   - [x] D: PDF image quality presets (Original / Print 300 dpi / Screen
         150 dpi / Email 96 dpi) in the layout panel, applied at export
         only; per-image exception. Hand-tested and pushed.
   - [x] E: split page pane beside the author/science editor (F6 or
         "Pages", resizable, own zoom, shared markers, right-click
         exceptions, click on a page selects the word in the editor);
         layout panel stays in the Publisher tab. Known: after a click on a
         page the editor has focus, so the pane's Ctrl zoom keys need a
         click on the pane's header or background first (Ctrl+wheel always
         works); under 640 px each side gets half
   - [x] F: edit an exception in a popover at its marker (marker click,
         right-click → Edit…, a problem naming it), tab and split pane.
         Known: a problem in the expert code still opens it in the layout
         panel, which the split pane hides
   - [x] G: Exceptions drawer (F7 / "Exceptions" button): chapter groups
         with page numbers, kind and "Needs attention" filters, search,
         keyboard with the pages following, inline edit, bulk remove/set
         value as one undo step (`overrides_batch`). Known: not tried with
         ~1000 exceptions (the mock's book mode has none); the drawer
         closes when the pages move between tab and split pane
2. Docs:
   - [x] Tech doc `docs/dev/architecture.md` (contributors + future sessions),
         README updated
   - [x] Screenshots: `node scripts/screenshots.mjs [--only a,b] [--no-build]`
         (headless Edge over the dev mock, shots in `scripts/shots.mjs`,
         PNGs in `docs/images/`); the mock serves real pages pre-rendered
         from `app/src/sample.typ` (regenerate:
         `cargo test -p lethora-typeset --test mock_pages -- --ignored`)
   - [ ] User manual: deferred, see "Later"
