# TODO

Future work, in milestone order; this file is the index. Detailed step
plans, decisions and hand-test lists live in `docs/plans/` (one file per
milestone), open limitations of finished steps in
[docs/plans/known-issues.md](docs/plans/known-issues.md). Move items to
`CHANGELOG.md` when done.

## Now

State (2026-10-25): Milestones 1–4 done (batches and docs: [milestone-4-upgrades](docs/plans/milestone-4-upgrades.md)); Milestone 6
(editing tools, [plan](docs/plans/milestone-6-editing.md)) done and hand-tested.
- Milestone 5 ([plan](docs/plans/milestone-5-collaboration.md)): steps 1–3g built; step 3 (sync server: roles, key
  epochs [docs/key-epochs.md](docs/key-epochs.md), presence, who wrote this, server docs) hand-tested on two PCs and pushed.
- Milestone 5 step 4 (review tools, 4a–4d) built, hand-tested on two Windows PCs against a server and pushed.
- Protocol and client versioning built and reviewed, awaiting the hand test.
- macOS title bar, menu bar and Dock icon built and reviewed, awaiting the hand test on a Mac.
- Release 1.0 ([plan](docs/plans/release-1.md)): local app + folder sync, server as self-host beta; steps R1–R6 (license, spell check, import, export, hardening, release pipeline) planned; R1 (license and notices) moves to the end.
- R2 (spell check, `crates/spell`) built and reviewed, awaiting the hand test.
- R3a (import: `crates/convert`, Markdown and text) and R3b (Word import), R4a (export: HTML, Markdown, text), R4b (EPUB), R4c (Word export) built and reviewed, awaiting the hand test.
- R5 (hardening): R5a strict CSP (built and reviewed, awaiting the hand test in a built app), R5b recovery of damaged own log records (built and reviewed, awaiting the hand test).
- R6 (release pipeline) detailed 2026-10-27: R6a CI checks, R6b release workflow and signing hooks, R6c updater, R6d version 1.0.0 and docs.
- R6a (CI checks, `.github/workflows/ci.yml`) green on all three systems (2026-10-27, after three fixes).
- R6b (release workflow, `.github/workflows/release.yml`: draft release, checksums, provenance, signing hooks): test tag `v0.1.0` built all five files into a draft.
- R6c (updater: Help → Check for updates…, check on start, app-wide `settings.json`) built and reviewed; the user's updater key and secrets in place; test release 0.1.0 (draft) built with the updater files; update hand test with 0.1.1.
- Added 2026-10-27, before 1.0: R7 app settings, R8 app language, R9 docs and user manual, R10 contributing guide.
- Next: the user's hand test of R2–R5, then R7; secret scan + LICENSE, repository public; release 0.1.1 and the update hand test; R8–R10; R1 notices; R6d (M5 steps 5–6 come after 1.0).

## Open hand tests

Each links to the step in its plan file (older steps whose text still says
"awaiting" were hand-tested and pushed).

- Cross-platform: three real devices (Linux, Windows, macOS) against a
  server, all of Milestone 5 (sync, roles, presence, review tools); the
  step 4 list under 4d in [milestone-5-collaboration](docs/plans/milestone-5-collaboration.md)
  is a good start.
- Protocol and client versioning: list under its entry in
  [milestone-5-collaboration](docs/plans/milestone-5-collaboration.md)
  (server and both PCs updated together: protocol 2).
- macOS title bar, menu bar and Dock icon: list under "Title bar and menus"
  in [milestone-5-collaboration](docs/plans/milestone-5-collaboration.md).
- R2 spell check: list under R2 in [release-1](docs/plans/release-1.md).
- R3 import and R4 export: lists under R3a, R3b, R4a, R4b and R4c in [release-1](docs/plans/release-1.md).
- R5a CSP (in a built app): list under R5a in [release-1](docs/plans/release-1.md).
- R5b recovery: list under R5b in [release-1](docs/plans/release-1.md).
- R6b release: the `v0.1.0` draft built (CI and hand test 3 passed 2026-10-27); left: install and start each file (hand test 2), sums on macOS/Linux and attestation (hand test 6) under R6 in [release-1](docs/plans/release-1.md).

## Workflow

Workflow per step: short plan + decisions with the user → Opus subagent
builds (no commits) → main session reviews, re-runs all checks, visual check
in the browser pane (dev mock build) → small fixes → update the step in
its `docs/plans/` file (status, hand-test list, "Known:"), the one-line
index here, `docs/plans/known-issues.md` and CHANGELOG.md → commit locally
→ push only when the user says so.

Useful commands:
- Run the app: `npm --prefix app run tauri dev`
- Tests: `cargo test --workspace`, `npm --prefix app test`
- Lint: `cargo clippy --workspace --all-targets -- -D warnings` (one known
  warning from vendor/loro-internal), `cargo fmt --check`
- Book benchmarks (release): `cargo test --release -p lethora-core -- --ignored --nocapture`,
  `cargo test --release -p lethora-typeset --test worker -- --ignored --nocapture`
- Dev UI in a browser: `NODE_ENV=development npx vite build --mode development
  --outDir <dir>` in `app/`, serve with `vite preview` (mock backend; `?book=1`
  for a book-size document)

## Milestone 3: Science view (leftovers)

- [ ] Palette: dock it so it doesn't cover text in narrow windows
- [ ] Math in headings renders at body size (use the line's font size)
- [ ] Tokenizer gaps: code inside math, multi-line strings in code, `else` on
      the next line, smart quotes/`~`/URLs/`\u{…}` unstyled
- [ ] Label scan doesn't skip labels inside raw/comments/math; figure labels
      must follow `#figure(...)` on the same line
- [ ] Graph tooling: decide which kind (function plots, data charts, diagrams)

## Milestone 4: Layout layer + publisher view

- [ ] Later: chapter-window preview for very large documents (compile the
      cursor's chapter ± 1 with page/heading/equation/figure counters seeded
      from the last full compile, out-of-window references as text, no TOC or
      bibliography in the window; full compile when idle and for export).
      Measured: 20–100 ms per edit, < 0.5 GB. Needed for long papers
      (paper preset at 800k words: 1.5 s edits, up to 5.7 GB; one unbroken
      page run) and documents with thousands of citations (cost inside Typst)
- [ ] Worker: crash/timeout during page render or font listing emits no
      status event (the page view's `PageCache.onFailure` calls
      `compile_now` once per version as a workaround); no memory cap on
      macOS (RLIMIT_AS is rejected; a watchdog on resident size would do);
      Linux code paths (setrlimit, memory reporting) not tested yet
- [ ] Publisher view: checkpoint drawer covers the layout panel; panel at
      narrow widths; scroll position across view switches unverified; font
      picker is a plain datalist
- [ ] Overrides: undoing the deletion of an anchored character leaves the
      anchor detached (Loro restores deleted text as new characters); fix via
      Loro's undo cursor hooks
- [ ] Exception markers, limitations: the page-break rule assumes symmetric
      margins (inside/outside margins shift it); a page break and a space on
      the same line put the break's marker below the space; chapter-opener
      breaks use a heuristic (next non-`context` element); figures searched
      within 8 pages of their text position; on a new version, markers
      switch before all page images have refreshed
- [ ] Later overrides needing a start and end anchor: keep together, tighten
      or loosen a paragraph
- [ ] Figure sizing, limitations: expert code overriding `#set image(...)`
      is not seen; `%` heights use the full text height (Typst uses the
      remaining space); figures whose `image(...)` is not the first argument
      get no preview; EXIF in WebP not read
- Done upgrade batches 0–2 (paragraphs, batches A–G, docs): [plan](docs/plans/milestone-4-upgrades.md)

## Milestone 5: Collaboration and sync

Plan, decisions, hand tests and limitations: [milestone-5-collaboration](docs/plans/milestone-5-collaboration.md).

- [ ] 1. Foundation, local only (parts 1a–1e built, hand-tested, pushed)
- [x] 2. Folder sync (parts 2a–2d built, hand-tested on two PCs)
- [ ] Title bar and menus (T1–T3 built, hand-tested, pushed; macOS below)
- [ ] macOS: overlay title bar, system menu bar from the command list, Dock icon on Apple's grid (built; awaiting the hand test on a Mac)
- [x] 3. Sync server (3a–3c2 built, hand-tested)
- [x] 3d. Removing a member starts a new epoch (hand-tested)
- [x] 3e. Presence: remote cursors and selections, who's online (hand-tested)
- [x] 3f. "Who wrote this": View → Show authors, colour + hover (hand-tested)
- [x] 3f+. Hand-test fixes: HTTP token lives with its connection (401 after 10 min), removed members folded, free/own member colours (hand-tested)
- [x] 3g. Docs, deployment (docs/server.md, deploy/: Docker + Caddy/nginx, systemd, example config), screenshots
- [x] 4. Review tools: comments, marks, suggestions (4a–4d, hand-tested on Windows; cross-platform left)
- [ ] Protocol and client versioning: protocol/level/app version in the hello, `Require` level per project with server cut-off, stop-don't-skip records, About + diagnostics (built; awaiting the hand test)
- [ ] 5. Accounts and OIDC
- [ ] 6. Browser version (WebAssembly), for trying the app instantly
- [ ] Later: opt-in encryption for local-only projects; external audit of the encryption

## Later

- [ ] A visual grid editor for tables in the science view (from Milestone
      6 step 3; tables are edited on the code for now)
- [ ] User manual, when it is time to ship one. First decide with the user
      how it reaches users (in-app help, website, PDF, ...), then write it:
      getting started, task walkthroughs (novel, paper, preparing for
      print), then reference (settings, keyboard, limitations). Screenshots
      via `node scripts/screenshots.mjs` (shots in `scripts/shots.mjs`).
      Open question: the sample (`app/src/sample.typ`) numbers headings and
      equations only because it cites `@sec:light`/`@eq:light`; drop those
      references so the sample novel looks like a novel
- [ ] Editor display settings (app preferences, not in the project): line
      spacing, paragraph-gap height and text size of the author and science
      views; theme values only, independent of the layout layer
- [ ] Undo granularity: settings changes (`layout_set`, `meta_set`,
      `layout_custom_set`) within 1 s merge into one undo step; an empty
      undo/redo still bumps the version (a needless recompile)
- [ ] Checkpoint preview shows only the text, while restoring also reverts
      layout, metadata, exceptions and expert code
- [ ] Persist the undo stack across sessions (Loro's UndoManager is in-memory)
- [ ] Report the Loro 1.16.2 fast-snapshot slowdown upstream; switch
      `export_snapshot` back to `ExportMode::Snapshot` once fixed
- [ ] Report the quadratic checkout/undo bug upstream (root cause + patch in
      `vendor/loro-internal-fix.patch`); drop `vendor/loro-internal`, the
      `[patch.crates-io]` entry and the `=1.16.2` pin once a fixed Loro ships
- [ ] Pruning/thinning of old automatic checkpoints
- [ ] Normalize `\r\n`/`\r` to `\n` when importing outside text (CodeMirror
      normalizes; a `\r` in the core would cause repeated resyncs)
- [ ] Before the first release: review app size (59 MB release binary, mostly
      Typst; accepted for now)
- [ ] Edge: keystrokes typed within one round-trip after Ctrl+Z (while an edit
      is in flight) are sent before the undo and undone with it
- [ ] Recovery UI for a log record that passes CRC but Loro rejects
      (currently `open()` fails with `Error::Loro` instead of dropping data)
