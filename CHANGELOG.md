# Changelog

History of what was done. Format based on [Keep a Changelog](https://keepachangelog.com).

## [Unreleased]

### Fixed

- macOS: the page preview failed with "Could not start the typesetter:
  Invalid argument (os error 22)". macOS rejects the worker's address-space
  cap (`RLIMIT_AS`), so the cap now applies on Linux only; on macOS the
  worker runs uncapped (the timeout still applies).
- Sync server: a client skipping the streams of a document it doesn't sync
  (one added by a later version) no longer keeps a read position for them.
  It did, so after upgrading it never read what it had skipped, and Loro
  held back every later change built on it (seen with `review` after
  upgrading from a version before review sync: the other member's comments,
  highlights and replies never arrived).
- Clicking an item of the editor's right-click menu put the cursor at the
  start of the document (Copy, table commands); it stays where it was.
- Read only (reviewers and readers): Edit → Cut and Paste are greyed out
  while they would act on the document, like the editor's right-click menu
  (Paste failed with "Document is not focused"); Copy and Select all stay,
  and fields such as Find keep all four.
- Read only: a click in the text places the cursor again, so the keyboard
  works there (select, Ctrl+C, Ctrl+A, Ctrl+F); edits are still refused.
  Edit → Replace… (Ctrl+H) is greyed out; Find shows no replace fields.
- Sync server: creating an invite or an add-device link failed with "HTTP 401:
  a valid bearer token is needed" after ~10 minutes online (the invite still
  showed as open, without a link), and snapshot and asset uploads failed the
  same way. The welcome's HTTP token now lives as long as its connection and
  is revoked when it closes (a removed device loses HTTP access at once);
  `limits.token_seconds` is gone (old config files still load).
- Members: removed members fold under a collapsed "Removed (N)"; a new
  member gets the first profile colour no current member uses (was hashed
  from the user id, so colours often clashed); each member can pick their
  own colour per project from the 8 swatches (click your dot).
- `@` completion and the reference hover card call a table figure's label
  "Table" instead of "Figure" (a figure whose body is a `table` call).
- Restoring a checkpoint (which also reverts layout and exceptions) now
  reloads the layout panel, the exceptions and the science view's figures,
  as undo does; the open page view showed stale settings and markers.
- Layout panel: changing a select, number or text setting back to its
  previous value (e.g. Contents depth 3 → 4 → 3) was not sent to the core
  while the control kept focus, and the old value came back on the next
  change. Changes are now compared with the core's value; the applied row
  shows the core's normalized value (` 4PT` → `4pt`).
- Book-size memory and speed (novel/thesis presets): the running head asked
  every page for every chapter's page to stay off chapter openers; it now
  checks only the next chapter (identical output, regression test). Novel at
  ~800k words: memory while editing 3.0–3.1 → 1.9–2.1 GB, cold compile 3.2 →
  2.2 s, edit 1.1 → 0.65–0.7 s, font-size change 4.3 → 1.5–1.7 s.
- The worker trims Typst's cache when idle (250 ms after a compile, or before
  the next one) instead of during the compile, so page renders right after a
  compile stay at ~3 ms.

- Checkpoint preview, restore, and undo of a big paste took minutes on
  book-size texts (Loro 1.16.2 was quadratic when travelling back across one
  long insert span). Now ~0.15–0.3 s at ~700k words. Fix: patched
  `loro-internal` vendored in `vendor/loro-internal` via `[patch.crates-io]`
  (diff in `vendor/loro-internal-fix.patch`), `loro` pinned to `=1.16.2`.
  Regression test `back_across_big_paste_is_fast`.

### Added

- License files (#4): `LICENSE` (GPL-3.0) for the app and its crates,
  `crates/server/LICENSE` (AGPL-3.0) for the sync server, the matching
  `license` fields in the Cargo manifests and `app/package.json`, and a
  License section in the README. `vendor/loro-internal/LICENSE` keeps
  Loro's MIT notice with the vendored copy.
- Docs site (#19): Docusaurus in `website/`, with the user manual at
  `/manual` (from `docs/manual/`) and the developer docs at `/dev` (from
  `docs/dev/`), read in place; follows the system's light or dark theme.
  `.github/workflows/docs.yml` builds it on every change and deploys it to
  GitHub Pages (https://bechtstudio.github.io/lethora/) once the repository
  is public; a pull request only builds it, so a broken link fails there.
  Commands in CONTRIBUTING.md.

- Editable keyboard shortcuts (Release 1.0 step R7b): Settings → Keyboard
  shortcuts lists every menu command; each key has × to remove it, Add
  records a new one, Reset brings back the defaults (per command, or all
  with Restore defaults). A key another command uses asks first ("Ctrl+B
  is used by Bold": Replace / Cancel), so a key does one thing. Keys that
  would type, move the cursor or reload are refused with the reason; the
  text fields' own keys (Undo, Copy, Paste, Select all, zoom, Exit) stay.
  Menus, the macOS menu bar and the right-click menu follow the change at
  once; only changed commands are stored (`shortcuts` in `settings.json`).
  On macOS keys show as ⌘⇧⌥ (Control ⌃ for Replace and Insert equation).
- Settings (Release 1.0 step R7a): File → Settings… (Ctrl+,; macOS
  Lethora → Settings…) opens a full-window view with a search field and
  categories: General (reopen the last project on start), Appearance
  (theme: match system, light, dark), Editor (text size, line spacing and
  space between paragraphs of the author and science views; check
  spelling), Keyboard shortcuts (a searchable list), Updates (check on
  start; check now and install) and About (version facts, copy diagnostic
  info). Changes apply and save at once in the app-wide `settings.json`;
  each page has Restore defaults. Spell check moved there from the web
  view's local storage (an "off" is carried over once).
- Updates (Release 1.0 step R6c, `tauri-plugin-updater`): Help → Check
  for updates… (shows "up to date" or the error) and Help → Check for
  updates on start (on by default; quietly 5 s after start, silent when
  up to date or offline, never in a debug build). A found update asks
  first: its version, date and release notes (the CHANGELOG section, as
  plain text), Install and restart / Later. Installing sends every edit,
  saves the project, downloads with progress, installs (Windows: NSIS
  passive) and restarts into the last project. The check runs in Rust
  (the CSP stays closed); the download must be signed by the release's
  own updater key (public key in `tauri.conf.json`). AppImage and `.deb`
  installs both update. App-wide settings now live in `settings.json` in
  the config folder (unknown fields kept; R7 grows it into preferences).
  The release workflow signs the updater files and adds `latest.json`
  only when the `TAURI_SIGNING_PRIVATE_KEY` secret exists, so a local
  `tauri build` needs no key. CI no longer runs on tag pushes.
- Release workflow (Release 1.0 step R6b): a `v*` tag builds one draft
  GitHub Release (published by hand) with the Windows NSIS installer, two
  macOS DMGs (Apple Silicon, Intel), a Linux AppImage and `.deb`, plus
  `SHA256SUMS.txt` over every file and, once the repository is public,
  build provenance (`gh attestation verify`). The run stops if the tag
  doesn't match the app version (Cargo and `app/package.json`); the
  release text is the version's CHANGELOG section. Signing hooks for macOS
  (Developer ID, notarization) and Windows (`signCommand`) stay inactive
  until their secrets exist; until then the macOS app is ad-hoc signed.
  `tauri build` now makes only these formats (no MSI, no `.rpm`).
- CI checks (Release 1.0 step R6a): `.github/workflows/ci.yml` runs on
  every push and pull request on Ubuntu 22.04, Windows and macOS:
  `cargo fmt --check` (Linux), clippy with `-D warnings`, `cargo test
  --workspace`, the web tests and the web build (`--locked` for Cargo).
  The known `unused_imports` warning in `vendor/loro-internal` doesn't
  fail clippy (`-D warnings` applies to workspace members only).
- Recovery of damaged changes (Release 1.0 step R5b): when a change in
  this device's own log (main or review) passes its checksum but can't be
  read, opening no longer just fails. The app names the file and offers
  "Open at the last good state": a byte-exact copy of the log goes to
  `backup/damaged/`, then the damaged change and everything after it are
  dropped, and a notice says how much and where the copy is. The start-up
  reopen asks the same instead of silently showing the start screen. A
  damaged own snapshot gives a clear error naming the file and changes
  nothing. Other devices' data skipped on open (rejected or unreadable)
  now shows a title-bar notice naming the device; torn tails, which may
  still be arriving, stay silent.
- Content Security Policy (Release 1.0 step R5a): built apps only load
  scripts, styles and images from the app itself (images also as `blob:`
  and `data:`), and the web view talks only to Tauri's IPC. The one
  exception is inline style elements, which CodeMirror needs for its
  themes; scripts stay strict. Applies to built apps only: `tauri dev`
  loads from Vite, where Tauri sends no policy.
- Word export (Release 1.0 step R4c): File → Export… → Word (now the
  first and default choice): a `.docx` with Word's own styles (Title,
  Heading 1–6, Caption, Quote, Bibliography, …) set from the layout's
  fonts, sizes, page size and margins; real Word footnotes and lists,
  tables with merged cells and repeating header rows, images (SVG with a
  PNG fallback), captions Word can renumber, references as links to
  bookmarks, and formulas as crisp images sitting on the text baseline
  (their Typst source as alt text). Checked by opening every test document
  in Word 365.
- EPUB export (Release 1.0 step R4b): File → Export… → EPUB 3: a title
  page, one file per chapter (level-1 heading), table of contents (nav and
  NCX for older readers), pop-up footnotes, MathML formulas, images,
  references linked across chapters, accessibility metadata, a plain
  stylesheet that leaves fonts and colours to the reader. HTML export now
  shares its serializer (polyglot HTML/XHTML).
- Export (Release 1.0 step R4a): File → Export… saves the project as HTML
  (one self-contained file: images inside, MathML, footnotes as endnotes),
  Markdown (images in a `<name>_images` folder next to it, math kept as
  Typst) or plain text, and lists PDF too. Exports read the compiled
  document (Typst's HTML target), so citations, the bibliography, numbering,
  cross-references and footnotes come out resolved; the export runs in the
  typesetter process. What a format can't hold is named afterwards in the
  title bar.
- Word import (Release 1.0 step R3b): `.docx` (also `.docm`, `.dotx`)
  through File → Import… and Edit → Insert file…: headings by style name
  (any language) or outline level, Title/Subtitle and document properties
  as the project's title and author, bold/italic/underline/strike/super/
  subscript, bulleted and numbered lists (nesting, restarts), footnotes and
  endnotes, links, tables (merged cells, header rows), images (width
  relative to the text column, captions from Word's Caption style), and
  Word equations converted to Typst math. Tracked changes are accepted;
  comments, headers and footers, charts and EMF/WMF pictures are dropped
  with a warning; citation fields (Zotero, Mendeley, EndNote, Citavi,
  Word) are kept as text. Own zip reader with limits against zip bombs.
- Import (Release 1.0 step R3a): File → Import… makes a new project from a
  Markdown or plain-text file (the New project dialog, titled after the
  file), Edit → Insert file… inserts one after the cursor's paragraph as one
  undo step (images next to a Markdown file become assets; in suggest mode
  a suggestion without images). Built on the new UI-free crate
  `crates/convert`: one document model between all formats, a reader or
  writer per format, a registry the dialogs read, warnings for what was
  dropped (shown after the import), golden fixtures per format. Markdown:
  CommonMark plus tables, strikethrough, footnotes, math (common LaTeX
  converted to Typst math), pandoc citations (only known sources), front
  matter title and author. Plain text: UTF-8, UTF-16 or Windows-1252.
- Spell check (Release 1.0 step R2): our own, in Rust (`crates/spell`,
  `spellbook` with bundled Hunspell dictionaries en-US, en-GB, de-DE,
  de-AT, de-CH), in the language of the layout's `text.lang`. A wavy line
  under misspelled words in the author and science views; math, raw text,
  code, citations, labels, comments, links, words with digits and all-caps
  words are skipped. Only the visible text is checked, in batches off the
  editing thread, cached per word. The editor's right-click menu offers up
  to five corrections, Add to dictionary (a personal list, `dictionary.txt`
  in the app config folder, all projects) and Ignore (this project, until
  it closes). View → Check spelling turns it off. Replaces the web view's
  spellcheck, whose suggestions our own right-click menu had hidden.
- macOS: the title bar is our row with the traffic lights at its start
  (overlay title bar, no title text; none in full screen), and the menus
  live in the system menu bar, built from the same command list: a Lethora
  menu (About, Hide, Quit ⌘Q, which saves first), File, Edit, Format, View,
  Window and Help. Cut, Copy and Paste are the system's items. ⌘H and ⌘M
  stay Hide and Minimize, so Replace and Insert equation use ⌃H and ⌃M
  there. The Dock icon follows Apple's grid (rounded square with margin and
  shadow) instead of a full-bleed square.
- Protocol and client versioning (Milestone 5, before the first release):
  wire protocol 2 (the hello carries the client's feature level and app
  version, the welcome the server's version and each device's last level;
  challenge and refusal keep a frozen layout); feature `LEVEL` 1; envelopes
  v3 (folder sync) and v4 (server) carry the level they need; members op
  `require` raises a project's minimum level, and the server then cuts off
  connections below it and refuses hellos below it. A client never moves
  past a record of a newer version or level: it stops there and shows
  "needs a newer Lethora" (editing stays possible; nothing is lost). Help →
  About Lethora with version, git hash, protocol, level, system and server
  version, and "Copy diagnostic info"; the server logs its version at
  startup and in `/healthz`. One version source: the workspace
  `Cargo.toml`.
- Review tools, core (Milestone 5 step 4a): a second Loro document,
  `review` (`review/<device>.snapshot` and `.log`), with comment threads,
  highlighter marks and suggestions anchored in the text, its own undo,
  and authors taken from Loro's attribution. Write rights per document:
  `main` for owners and editors, `review` also for reviewers, nothing for
  readers; accepting a suggestion is an edit of `main`. No UI yet.
- Review tools, sync and roles (Milestone 5 step 4b): `review` syncs as a
  stream of its own on the server and as `review/` in the folder-sync
  replica. Devices push and import each document by role (a reviewer's or
  reader's `main` changes are refused by the server and ignored by other
  devices). In server-shared projects authors rest on signatures: a change
  counts as a device's only if that device's own stream brought it, so a
  member can no longer pose as someone else (Show authors, review items).
  Compaction snapshots now hold only the device's own changes.
- Review tools, app (Milestone 5 step 4c1): View → Review (or the bar's
  Review button) shows a panel of cards beside their lines in the author
  and science views, with threads, replies, edit and delete, Resolve/Reopen
  and an Open/Resolved/All filter. Edit → New comment (Ctrl+Alt+M) and
  Edit → Highlight (yellow, green, blue, pink), also on right-click; the
  text shows highlights and commented ranges while the panel is open.
  Ctrl+Z in the panel (for reviewers everywhere) undoes review changes.
  Reviewers see a "Reviewer" badge and can comment and highlight but not
  edit the text; readers see the review read only.
- Review tools, suggestions (Milestone 5 step 4c2): Edit → Suggest changes
  (Ctrl+Shift+X; always on for reviewers) turns typing, deleting, pasting
  and formatting in the text into suggestions: insertions underlined and
  deletions struck through in the author's colour, the text itself
  unchanged. Accept (owners and editors; one undo step, typeset again) and
  Reject on the card and on right-click; a suggestion whose text changed
  can't be accepted. Ctrl+Z in suggest mode undoes suggestions.
- Review tools, docs (Milestone 5 step 4d): "Review tools" in
  [docs/dev/collaboration.md](docs/dev/collaboration.md) (section 10) with three
  screenshots, the app side in [docs/dev/architecture.md](docs/dev/architecture.md),
  README.
- Sync server deployment: [docs/dev/server.md](docs/dev/server.md) (config
  reference, reverse proxy, Docker, systemd, backups, limits) and
  `deploy/` (Dockerfile, docker-compose with Caddy, nginx snippet, systemd
  unit, example config checked by a test); architecture and collaboration
  docs cover the server; screenshots of members, invite link, presence and
  Show authors.
- "Who wrote this" (Milestone 5 step 3f): View → Show authors in
  server-shared projects gives text in the author and science views a faint
  background in each author's colour, with a small legend; hovering shows
  the author and when. Computed by the core for the visible text only.

- Live presence (Milestone 5 step 3e): in a server-shared project, other
  members' carets and selections show in the author and science views in
  their colour with a name flag (on hover and briefly after a move), and the
  title bar shows coloured initials of who's online (name, role, view).
  Presence is sealed with the project key, forwarded by the server, never
  stored, and times out after 30 s. The row shows up to five people, then
  a "+N" whose tooltip lists the rest. Dev mock: `?presence=1` (`=N` for N
  online members).

- Key epochs (Milestone 5 step 3d): removing a member or device of a
  server-synced project starts a new project key, sealed in the members log
  to each remaining device and each open invite link with its X25519 key;
  new content uses the newest key, older epochs stay readable. Any current
  device that sees a removal starts the epoch (the remover at once, after a
  leave the first remaining device online). Invite links carry an exchange
  key and the whole keyring; `keys/<project>.key` holds `epoch:hex` lines.
  Explained with graphics in `docs/dev/key-epochs.md`. Invites and logs from
  before 3d aren't readable any more (no migration; share test projects
  again).

- Table editing (Milestone 6 step 3b): in a table, Tab/Shift+Tab move
  between cells (Tab in the last cell adds a row); insert rows and columns,
  delete rows, columns or the table, header row on/off, merge selected
  cells and split them, each as one edit of the Typst text. From a
  "Table ▾" button in the formatting row (only in a table) and a new
  right-click menu in the editor (Cut, Copy, Paste, plus the table items).
- Tab accepts the selected `@` completion (source key or label), like
  Enter; with no list open Tab does what it did before.
- Tables (Milestone 6 step 3a): a Table button in the formatting row and
  Edit → Insert table… open a Word-like grid picker (up to 10 × 8, "Header
  row"); the table goes in as its own block, `#figure(table(…), caption:
  []) <tab:N>` with the next free N, the cursor in the first cell. Layout
  panel "Tables": default style (Plain, Academic, Grid, Striped), bold
  header, caption above/below; one table's own style is an exception
  (right-click it on a page → Table style; Exceptions drawer, markers).

- Layout panel "Character styles" section (Milestone 6 step 2c): every
  style with its name in its own font, name, font, size and use count; add
  a named style, rename, change font/size for all uses, delete (confirmed
  when used; the text falls back to body text), remove unused unnamed
  styles. The Typography font row uses the new font picker with previews;
  the row's Style box links here ("Edit styles…").
- Style, font and size boxes in the formatting row (Milestone 6 step 2b).
  A cursor in plain text: the font and size set the body's (the whole
  document). A selection or styled text: it gets a character style
  (`#styled("<id>")[…]`), reusing an unnamed style with the same settings
  or creating one named after them ("EB Garamond 16 pt"); styled text
  changes in place, nests, splits or unwraps ("Default text"). The font
  list previews each font (embedded Typst fonts loaded into the web view);
  the editor shows styled text in its font at its size relative to the
  body's and marks references to missing styles.
- Character styles in the layout layer (Milestone 6 step 2a, no UI yet):
  `styles.<id>.font|size|name|removed` keys (readable slug ids, absolute
  sizes, any font family name), validated by `layout_set`, resolved per
  field over a preset's styles (presets ship none yet). The preamble
  defines `#styled("<id>")[…]` from them; an unknown id leaves body text.
- Formatting row (Milestone 6 step 1) under the title bar in the author and
  science views (hidden in the publisher view, focus mode and the checkpoint
  preview; disabled while read only), plus a Format menu. Toggles for bold
  `*…*`, italic `_…_`, underline `#underline[…]`, strikethrough
  `#strike[…]`, superscript `#super[…]`, subscript `#sub[…]` (Ctrl+B/I/U)
  with pressed state; a heading box (Paragraph, Heading 1–6) and bulleted
  `- ` / numbered `+ ` list toggles on the selected lines. Word behaviour:
  a cursor in a word formats the word, between words inserts an empty pair,
  inside a format removes it; partial-word selections use `#strong[…]` /
  `#emph[…]`; inline formats start after a heading or list marker; nothing
  happens in math, raw text, code, citations or labels, or across another
  format's span. The editor styles the content of these calls (single
  line). Logic in `app/src/format.ts`.

- Linux build setup: `scripts/setup-linux.sh` (Debian/Ubuntu 22.04+)
  installs the Tauri system packages (WebKitGTK 4.1, OpenSSL for server
  sync), Rust via rustup if missing, checks Node.js 24+ and installs the web
  dependencies; README section on dev and release builds (`.deb`, `.rpm`,
  `.AppImage`) and common fixes. Not yet built on Linux.
- Sync server groundwork (Milestone 5 step 3a, no network or UI yet): new
  Loro-free crate `crates/protocol` (`lethora-protocol`) with device keys
  (Ed25519 signing, X25519 exchange), sealed envelopes (version 1 of folder
  sync moved from core byte for byte, pinned by test vectors; version 2 for
  the server, signed by the device so the server checks the writer and the
  place without the key; a keyring per epoch), the signed `members` log
  (hash-chained entries `genesis`, `invite`, `revoke`, `join`, `role`,
  `remove`, `profile`; an incremental verifier with roles, devices, invites
  and `may_write`) and invite links (`https://<host>/join/<id>#<secret>`,
  HKDF-derived invite signing key and project-key wrap). `device.json` gains
  the device's secret keys, generated for existing installs.
- Server sync in the app (Milestone 5 step 3c1): Share / Sync offers
  "Lethora server" first (address remembered, your name asked once, a
  warning for plain http on a LAN); Invite people (role, expiry, uses) and
  Add my other device give a link; "Open shared project…" takes a project
  code or an invite link. Changes arrive live (`remote-changes` → an
  immediate pull through the edit queue) and go out ~200 ms after an edit;
  the cloud shows online, connecting, offline (with the next try) and
  removed. Core: a sans-IO `ServerSync` (own changes only in the own
  stream, acked batches, compaction by snapshot, verified records and
  members log, positions saved after the data they cover); app: one task
  per project (`tokio-tungstenite`, `reqwest`, OS TLS, reconnect with
  backoff). A project syncs to a folder or a server, not both. Invite
  links now carry the project id.
- Members and roles (Milestone 5 step 3c2): a Members screen ("Members…"
  in the shared Share / Sync dialog, File → Members…) lists members with
  name, colour, role and devices, and the open invites. The owner changes
  roles, removes members, revokes invites and deletes the project from the
  server (confirmed by typing its title); anyone removes their own devices
  or leaves. Reviewers and Readers are read only until the review tools:
  the core refuses every change (`Error::ReadOnly`), the editor is read
  only, a "Read only · <role>" badge shows in the title bar, and the write
  controls are disabled; a role change applies live. After leaving, being
  removed or the project's deletion the copy stays as a normal local
  project (server settings cleared, key forgotten). Projects shared or
  joined here are remembered (`server_projects.json`), and "Open shared
  project…" reopens one without a local copy from the server, no invite
  needed.
- Sync server (Milestone 5 step 3b, no app side yet): `lethora-server`
  (`crates/server`; tokio, axum, no Loro) and the wire messages
  (`lethora_protocol::wire`, own versioned binary encoding). WebSocket at
  `/v1/ws`: a signed hello as a device, an invitee or creating a project;
  catch-up from per-device stream offsets, then live records, members
  entries and forwarded presence; appends checked for role and signature,
  resent batches stored once. HTTP: snapshots (compaction keeps later
  records; clients past it just shift their offset), immutable assets,
  wrapped invite keys (deleted when the invite closes), owner-signed project
  deletion, `/healthz`. Filesystem storage per project, crash repair on
  load, limits and rate throttling from `lethora-server.toml` /
  `LETHORA_*`. In-process integration tests.
- Splash screen: a small startup window (`splash.html`, own capability)
  with the app icon, tagline, progress bar and status line, in a light or
  dark background following the system theme. The main window starts hidden
  and reports progress (`startup.ts`); when ready it shows itself and
  closes the splash, which stays at least 1.5 s. Shows the app version and
  copyright; 720×480, artwork 1200×625 (same pieces in both themes).

- Collaboration foundation, part 1a (Milestone 5): project folder format 2
  with per-device files (`project.json`, `main/<device>.snapshot|log`,
  `checkpoints/<device>.json`, `<device>.lock`), so folder-sync tools never
  see two writers on one file. Opening reads the own files, then every other
  device's files read-only (never failing on them); the own files stay
  self-contained. Format-1 projects are converted on open (old files in
  `format1-backup/`). All core I/O goes through a `Storage` trait
  (`FsStorage` for folders). Device and user ids in `device.json` /
  `profile.json` in the app config directory. Loro records change
  timestamps. Book benchmark: open 78–81 ms, autosave ~2 ms (unchanged).
- Collaboration foundation, part 1b: "who wrote this" data. A `peers` map
  records each Loro peer's user and device, written lazily before a
  session's first change in a commit the undo manager ignores;
  `Document::author_at` returns user, device and time of a character.
  `Document::create/open` take an `Identity` (device + user). Restore now
  applies a filtered diff that leaves the bookkeeping maps alone (plain
  `revert_to` would have dropped newer `peers` entries). Converted format-1
  history is attributed to the converting user. No UI yet. Book benchmark
  unchanged (restore 41 ms, undo 36 ms).
- Collaboration foundation, part 1c: named checkpoints are shared through a
  `checkpoints` map in the document (not undoable, kept by restore, listed
  once their history has arrived); automatic ones stay per device. JS-safe
  ids unique across devices. Named checkpoints from the own file move into
  the document on open. Commands and drawer unchanged.
- Collaboration foundation, part 1d: the sources live in the document's
  `bibliography` text (undo, restore and checkpoints cover them; imports are
  an undo step and recompile). The compiler gets the text with the request
  and serves it as a virtual `references.bib`. `references.bib` stays as a
  mirror with a marker line (`% Lethora mirror crc32=…`): written on
  autosave, outside edits (Zotero auto-export, hand edits) merged in on
  project load, focus and F5 (three-way when both sides changed). Existing
  projects' files are taken over on first load. The sources list refreshes
  after undo/redo of non-text steps and restore. Book benchmark unchanged;
  1 MB bibliography: mirror write ~10 ms, merge 20–26 ms.
- Collaboration foundation, part 1e: images are immutable blobs
  `assets/<keyed SHA-256>` (project key in the document); a content map
  `assets` maps the text's path (`assets/foo.png`, unchanged) to its blob,
  so identical content is stored once and two different `figure1.png` can't
  overwrite each other. Restore brings back replaced images but never drops
  index entries. "Replace image…" in the page right-click menu (same format;
  one undo step). Loose files in `assets/` (older projects, hand copies) are
  taken in on open and F5 and moved to `backup/assets/`. The compile request
  carries the index. Book benchmark unchanged.
- Folder sync, part 2a (core, no UI yet): `crates/core/src/sync.rs`. An
  end-to-end encrypted replica of the project in a separate sync folder
  (`replica.json`, `main/<device>.snapshot|log`, `assets/<blob>`), every
  record, snapshot and blob in an XChaCha20-Poly1305 envelope bound to its
  project, kind and device or blob name. `sync_setup` returns the project
  code (56 Crockford base32 characters with check bytes; also the recovery
  key), `sync_join` builds a new local project from the replica without
  adding a change of its own, `sync_open`/`sync_push`/`sync_pull`;
  `pull_local` imports other devices' plain files live (projects kept
  inside a synced folder). A pull reads only what grew since the last one
  (a listing and two sizes per device when nothing changed), retries torn
  tails, detects compaction, skips damaged devices and blobs without
  failing, and returns the content changes (UTF-16) plus flags for
  settings, bibliography, assets and checkpoints. Sync settings live in
  `state/<device>.json`. New dependency `chacha20poly1305` 0.10.
- Folder sync, part 2b (app): a sync cloud next to the project name opens
  the sync dialog: set up encrypted sync (the copy goes into
  `<project>.lethora-sync` inside the folder you pick, the project code is
  shown with Copy), show the code, enter a missing code, stop syncing (the
  copy stays). "Join synced project…" on the start screen (renamed "Open
  shared project…" in 2d) builds a new local project from a sync folder and
  its code. Other devices' changes
  arrive every 2 s while you type (not on your undo stack), and the layout,
  sources, assets and checkpoints refresh when they change. Without
  encrypted sync, other devices' plain files in the project folder arrive
  live too; projects inside Dropbox, OneDrive, iCloud, Google Drive, Box or
  Syncthing folders get a one-time notice recommending encrypted sync. The sync
  folder and the project can't be nested. Project keys stay in the app's
  settings folder.
- Folder sync, part 2d: simpler sharing. New projects go to
  `Documents/Lethora` (or `~/Lethora` when Documents itself is synced), so
  New project asks only for a name. Share / Sync sets up the encrypted copy
  with one click in Dropbox, OneDrive, iCloud Drive or Google Drive (found
  automatically), or in another folder. "Open shared project…" needs only
  the project code: Lethora finds the copy in your sync apps and opens it,
  and offers to open the project instead if it is already on this device.
- Folder sync, part 2c: `docs/dev/architecture.md` (data model "Folder sync",
  flow (i), commands, invariants, tests, limitations),
  `docs/dev/collaboration.md` (decisions as built; pairing by project code
  answers the open question), README, screenshots `share-sync` and
  `open-shared`.
- Title bar, part T1 (web): the top bar is now a fixed title-bar row (no
  auto-hide): icon, File / Edit / View menus, project name with sync cloud,
  word count and status, the view switcher in the exact centre, and
  per-view tools (Science: Formulas, Figure, Sources; Publisher: zoom,
  Compile, Export PDF; Pages in author/science; Assets everywhere). Focus, Typewriter,
  Checkpoints, Markers and Exceptions moved into the View menu; Focus mode
  hides the row's tools. One command list (`app/src/commands.ts`) drives
  the menus and every shortcut (`menus.ts`: ARIA menubar, Alt / F10,
  Alt+letter, arrows, mnemonics, Esc returns the focus). New: Edit menu
  (undo/redo through the core, clipboard, select all), Find / Replace
  (Ctrl+F / Ctrl+H, `@codemirror/search`), Export PDF from every view.
  The row fits down to 800 px (word count and labels shrink first).
- Title bar, part T2 (native): no native title bar on Windows and Linux
  (`decorations: false`; macOS turns them back on): own minimize /
  maximize-restore / close buttons (Windows 11 look) and the row drags the
  window (`data-tauri-drag-region="deep"`, double-click maximizes). Recent
  projects (up to 10, `recent_projects.json` in the config dir, read from
  the old `last_project.json` on upgrade) in File → Open recent (a
  submenu, Clear recent) and on the start screen (click/Enter opens, ×
  or Delete removes from the list). File → Close project (Ctrl+W,
  Ctrl+F4) saves, closes and shows the start screen; the next start shows
  it too. File → Exit closes through the same path as the close button.
  The "already here" check for shared projects now covers all recent
  projects.
- Title bar: Windows 11 Snap Layouts on the own maximize button. A
  transparent native child window (`snap.rs`, Windows only) lies over the
  button and answers `WM_NCHITTEST` with `HTMAXBUTTON`, which WebView2's
  own child window never could; it sends hover, press and click to the web
  view (the button's hover/pressed look, the click maximizes or restores).
  The web view reports the button's rectangle in physical pixels on
  resize, zoom and DPI changes (`snap_rect`).
- Title bar, part T3: `docs/dev/architecture.md` flows (j) "Commands, menus
  and the title bar" and (k) "Start screen, recent projects, Close
  project", module/command/test tables and limitations updated; README
  feature bullet and main shortcuts; all screenshots regenerated with the
  new row, new `menus` and `start-screen` shots.
- "Replace image…" stores the new file under its own path (named like an
  import, any format) and repoints only the clicked image call to it, index
  and text in one undo step; the old entry stays for other figures and
  checkpoints. Previously the old path kept showing the new picture and
  every figure sharing the path changed.
- Assets explorer, part A1 (core and commands, no UI yet): an asset type
  table by extension (Rust `assets::TYPES` with the import check, web view
  `assettypes.ts` with label, icon, preview and inserted snippet; images
  only for now). Folders from paths (`assets/figures/ch1/x.png`), kept by
  marker entries (`assets/figures/ch1/` → `true`) in the `assets` map, so
  undo, restore and checkpoints cover them and emptied folders stay.
  Commands `asset_list` (folders; items with kind, size, used-by count,
  available/missing), `asset_import_bytes` (raw body, name and folder in
  headers, for drag & drop), `asset_move` (item or folder; index and every
  `"path"` literal in the text in one undo step), `asset_remove` (item or
  folder, refused while the text uses an item; blobs stay),
  `asset_folder_create`; `asset_import` takes a target folder. Undo/redo of
  an asset step now says "Undid an asset change".
- Assets explorer, part A2: a bottom Assets panel (F8 or "Assets", in all
  three views, resizable): folder tree, grid of previews with a size slider
  or a sortable list (name, type, size, used by, status), search over all
  folders, "Unused" filter, a status cloud per item (available / missing).
  Context menu: Open preview, Replace… (new content under the same path,
  same format; new command `asset_update`), Rename (F2), Save as… (new
  command `asset_save`), Show usages (jumps to the text), Insert as figure,
  Remove (only when unused); folder menu: New folder, Import…, Rename,
  Remove. Double-click/Enter opens a large preview; drag onto a folder or
  Ctrl+X/Ctrl+V moves. Sources show as a special item opening the Sources
  drawer. Tauri's native file-drop handler is off so HTML5 drag & drop
  reaches the web view. The dev mock implements the asset commands with
  seeded folders.
- Assets explorer, part A3: drag & drop. Files from the desktop dropped on
  the Assets panel are imported into the folder under the pointer (else the
  open folder); files from the desktop or items from the panel dropped into
  the author or science editor become figures, each its own block at the
  nearest paragraph boundary (a marker line shows where), desktop files
  imported into the panel's open folder. Unsupported files are named in one
  message. Importing and inserting the figure text is one undo step
  (`asset_stage`, `asset_stage_bytes`, then `figure_insert`; core
  `stage_blob` and `insert_with_assets`); the Figure button uses it too.
  File drops elsewhere are blocked so the web view never opens the file.
- Assets explorer, part A4: screenshots `assets-panel` and `assets-drop`
  (`scripts/shots.mjs`), the Assets panel in the README; the README's link
  to a not yet written manual is now a note.
- Docs: `docs/dev/architecture.md` (processes, data model, key flows, module
  map, command/event reference, invariants, testing) and an updated README.
  Screenshot tooling for the manual: `scripts/screenshots.mjs` drives
  headless Edge (Chrome DevTools Protocol, no dependencies) over the dev
  mock with numbered callouts; shots declared in `scripts/shots.mjs`. The
  dev mock now shows real pages pre-rendered by the Typst worker from
  `app/src/sample.typ` (`crates/typeset/tests/mock_pages.rs`, 9 pages,
  622 KB); the sample gained heading and equation numbering so it compiles
  for real.
- Upgrades, batch G: Exceptions drawer for long works (F7, or "Exceptions"
  in the Publisher tab's bar and the split pane's header; covers the layout
  panel in the tab, the editor side in split). Rows grouped by chapter
  (level-1 headings, collapsible) in document order with page numbers;
  filters by kind and "Needs attention" (detached, not applied, invalid),
  search over label and anchored text ("N of M"). Keyboard tree: Up/Down
  (the pages follow and flash the marker), Shift+arrows/Ctrl+A select,
  Enter edits inline, Delete removes. Multi-select: Remove or "Set value…"
  for one kind, each one undo step via the new core command
  `overrides_batch` (all or nothing). The layout panel's list became a
  summary (count, needs attention, Open).
- Upgrades, batch F: exceptions are edited in place. A click on a marker,
  right-click → "Edit…", or a problem that names an exception opens a
  popover at the marker (in the Publisher tab and the split pane; at the
  click spot if no marker shows): label, state, anchored text, the same
  fields as the Exceptions list (shared code), Show in text, Remove. Stays
  open after a change; closes on Esc, ×, an outside click, wheel, zoom or
  when the exception goes away (e.g. undo). Fixes "Edit…" and marker clicks
  doing nothing in the split pane (they selected a row in the hidden panel).
- Upgrades, batch E: split page pane. F6 or the "Pages" button (author
  and science views) shows the pages to the right of the editor; the same
  page view as the Publisher tab, without the layout panel. Resizable by a
  drag handle (or Left/Right on it), each side at least 320 px; on/off and
  width remembered. A slim header holds zoom (its own, default Fit width),
  Markers (shared with the tab), the compile status and close. Right-click
  exceptions work as in the tab; a click on a page selects the clicked word
  in the current editor (no view switch; a click that only closes the page
  menu does nothing else); diagnostics also stay in the current view.
  Ctrl +/-/0 and Ctrl+Z/Y act on the pages while the pane has focus.
- Upgrades, batch D: PDF image quality. Layout panel "PDF export → Image
  quality": Original (default, unchanged) / Print 300 dpi / Screen 150 dpi
  / Email 96 dpi, applied at export only: raster images larger than their
  largest placement needs are swapped for downscaled copies in the laid-out
  pages (pages and layout identical; JPEG stays JPEG at quality 90/85/75,
  everything else PNG with alpha; kept if not smaller; ICC and EXIF
  orientation respected). Per-image exception `image_quality` (right-click
  an image or figure → Image quality, marker "Image: …"). 20 images of
  4000×3000: Original 362 MB / 0.9 s, Print 71 MB / 4.0 s, Screen 17 MB /
  2.6 s, Email 6.8 MB / 2.2 s. Limitations: images are processed one after
  another; a CMYK JPEG is left unchanged.
- Paragraphs follow Typst in the author and science views: Enter starts a
  new paragraph (inserts an empty line, drawn compact; in a list it adds the
  next item, on an empty item it ends the list; in math, code and raw text a
  plain line break), Shift+Enter is a forced line break (`\`, dimmed; math's
  line break in math). Block exceptions and the publisher's right-click menu
  now use the start of the Typst paragraph instead of the source line, so a
  paragraph written over several source lines is one block (it previously
  rendered as one paragraph but was offered as several); the menu names it
  by its first words. Only the empty line right after a paragraph is drawn
  compact, so the line being typed on never changes height. Backspace at
  the start of a line (or right after a `\`) and Delete at a line's end
  remove a forced line break (the lines join with a space) or a paragraph
  break (the paragraphs run together) whole, as one undo step.
- Upgrades, batch C (publisher right-click targets): a click that misses
  text leads to the figure whose box it is in (any figure, also floats),
  else to the nearest text line within about one line height (x clamped
  into the line). A click nearest to page furniture (running head, page
  number) never snaps into the body. Book-size (1316 pages): ≤ 3.2 ms per
  jump. Limitations: a chapter running head repeats the heading, so it
  leads to that heading; a snapped point on a link (citation, TOC entry)
  gives nothing; tall inline math can win a snap from the gap above it.
- Upgrades, batch B (layout panel):
  - Heading sizes: Level 1, Level 2, "Level 3 and below" (levels 3+ share
    one size, so a deeper heading is never larger).
  - Space above / below headings (one pair for all levels; empty = Typst's
    per-level default, which no single pair reproduces). Paragraph spacing
    doesn't affect headings (Typst's own heading spacing wins).
  - Table of contents: depth 1–4, title, dot leaders, indent levels, entry
    spacing (empty = line spacing), bold level-1 entries; shown when the
    contents are on. Defaults generate the same Typst as before.
- Upgrades, batch A:
  - View switcher fixed in the centre of the top bar (same place in every
    view).
  - Ctrl+wheel zooms the publisher pages one step per notch around the
    pointer (touchpad deltas add up); the web view itself never zooms.
  - "Compile" button and F5: typeset again, re-reading `references.bib` and
    images (sources list and science-view figures refreshed too); counts a
    new document version so pages are fetched anew. Ctrl+R / Ctrl+Shift+R
    no longer reload the app.
  - Display formulas inserted with Ctrl+Shift+M or the palette get a label
    `<eq:N>` (smallest free number); inline ones none.
  - 151 web tests.
- Milestone 4, step 5 (figure sizing in the science view):
  - Images draw at their `width:`/`height:` against the layout's text
    column (the editor column stands for it): `%`, cm, mm, in, pt, em;
    capped at the column. No width = the layout's default figure width (as
    the generated `#set image(width: ..)` does); `width: auto` = natural size
    (pixels at the image's DPI as Typst reads it, SVG at 96 dpi); width and
    height = a box filled per `fit` (default cover).
  - `layout_get` reports the text area (`column_width_pt`, `height_pt`,
    `font_size_pt`); figures redraw after layout changes.
  - Placement badge in the caption line: the figure's own `placement:`, or
    an exception's ("Top · exception", detached in warning style).
  - 149 web tests.
- Milestone 4, exception markers (publisher view):
  - Typeset: an invisible `#metadata(...) <lethora-override>` is injected
    with each block override (after a break, below a space with its resolved
    height); after compiling, positions are read via the introspector.
    Placed figures are matched to the overridden call by span (no label
    added) and reported with their floated bounding box. `OverrideStatus`
    gains `page`, `x_pt`, `y_pt`, `width_pt`, `height_pt`. Tests check that
    markers change no layout (19 cases, all positions compared). Locating
    200 overrides at book size takes ~20 ms.
  - Publisher view: dashed rule + chip for page/column breaks ("Page break",
    "Right-hand page", "Column break"), margin bracket with the length for
    space before, outline + "Top/Bottom/Here/Auto" chip for figures; detached
    ones dotted in the warning colour. Click a marker to select its row;
    right-click for Edit/Remove; selecting a row scrolls to its marker and
    highlights it. "Markers" toggle and Ctrl+Shift+E, remembered, on by
    default. Markers come from the compile whose pages are shown.
  - 143 web tests.
- Milestone 4, step 4b (anchored overrides, UI):
  - Right-click on a page (or Shift+F10): menu for the clicked block with
    page break before, right-hand page, column break (2 columns), space
    before (inline length), figure placement; existing ones shown checked.
  - "Exceptions" group in the layout panel: labels, anchor text, detached /
    not applied (with reason) / invalid badges, inline edit, remove, "Open in
    Science view"; diagnostics in an override select its row.
  - 138 web tests.
- Milestone 4, step 4a (anchored overrides, backend):
  - Core: anchors as encoded Loro cursors (`anchor_at`, `resolve_anchor`,
    detached when the anchored character is deleted); `overrides` map in the
    document (shared undo, checkpoints, autosave).
  - Typeset: override kinds page break (optionally to a right-hand page),
    column break, space before, figure placement; injected only into the
    compile source at the anchored block (top-level markup only) or into the
    `figure(...)` call (existing `placement:` replaced); source map keeps
    diagnostics on the right content offsets, `Location::Override`.
  - Click-to-source via `typst-ide` (`page_jump`, ~1.6 ms).
  - Commands `overrides_get`, `override_add`, `override_remove`,
    `override_update`; `compile-status` reports per-override status.
  - Book size with 200 overrides: one-word edit ~0.9–1.0 s.
- Milestone 4, step 3 (publisher view):
  - Ctrl+3 Publisher view: virtualized page preview (3–5 page elements at
    ~2000 pages, scroll position kept across recompiles), zoom (Ctrl+±/0),
    status (typesetting, pages, time, warnings/errors), diagnostics footer
    linking to the text (Science view) or the expert box.
  - Layout panel: preset with description (confirm reset of overrides),
    grouped settings with override markers and per-setting reset, inline
    backend errors, expert Typst box; edits wait for the sync queue first.
  - PDF export (Ctrl+E, save dialog).
  - New project: preset, title and author chosen up front
    (`project_new(path, preset, meta)`, nothing to undo back to).
  - "Undid/Redid a layout change" messages; panel refreshes after undo/redo.
  - 123 web tests.
- Milestone 4, step 2 (compile worker):
  - The app runs itself with `--compile-worker` as an isolated typesetting
    process (framed JSON over stdin/stdout, no console window); kept alive for
    incremental compiles; holds the last good document for page SVGs and PDF.
  - Supervisor: timeout (60 s + 20 s per MB of text), memory cap (Windows job
    object, kill-on-close; Unix setrlimit), automatic restart after timeout,
    crash or out-of-memory.
  - App: document version counter, `compile_watch`, `compile_now`,
    `page_svg`, `export_pdf`, `compile-status` events; compiles 400 ms after
    the last change (max 2 s while typing), latest version wins.
  - System fonts via `typst-kit` (188 families here, ~50 ms scan).
  - Book size (~5M chars): one-word recompile 1.1–1.6 s, page SVG 3–4 ms, PDF
    under 1 s; worker memory 3–6 GB (open item).
- Milestone 4, step 1 (layout layer foundation):
  - Core: `layout` and `meta` maps plus `layout_custom` text in the Loro
    document; covered by undo, checkpoints, autosave, crash recovery. Settings
    and text edits never share an undo step.
  - Typeset: layout schema (33 keys), presets novel / paper / thesis /
    manuscript (only overrides stored), strict validation and escaping,
    preamble generator, whole-document compiler (files confined to
    `references.bib` and `assets/`), diagnostics mapped to content offsets,
    SVG pages and PDF export. `assets` helpers moved into the typeset crate.
  - Tauri: `layout_get`, `layout_set`, `layout_apply_preset`,
    `layout_presets`, `layout_fonts`, `meta_set`, `layout_custom_set`.
  - ~100–135-page document: 0.24–0.88 s cold compile, 57–121 ms after an edit.
- Milestone 3, step 3 (sources, references, figures):
  - Sources drawer (Ctrl+Shift+B): search, insert `@key`, formatted reference
    in a chosen CSL style, import a `.bib` file, paste BibTeX, add a source by
    form (generated key, BibLaTeX escaping); refreshes on window focus.
  - `@` autocomplete for source keys and document labels (figures, equations,
    headings); hover cards; unknown references underlined (science view).
  - Figures (Ctrl+Shift+G): image copied into `assets/` under a unique safe
    name, `#figure` markup with label inserted, image + caption preview.
  - Rust: `bib_import_file`, `asset_import`, `asset_bytes` (raw bytes; path
    strictly confined to `assets/`, 50 MB cap) with unit tests.
  - 100 web tests; label scan ~3.5 ms at 5M chars, never per keystroke.
- Milestone 3, step 2 (science view, web side):
  - Author/Science view switch (Ctrl+1/2, top bar), remembered.
  - Typst markup via a CodeMirror `StreamLanguage` tokenizer (replaces the
    line-by-line decorator): multi-line math, raw blocks, nested comments,
    code expressions with content blocks; spellcheck off in math/raw/code/
    citations/labels.
  - Rendered formulas in the science view as `<img>` (never inline SVG),
    aligned on the text baseline, display math centred, theme-coloured; source
    and a live preview tooltip while the cursor is inside; errors underlined.
    Only visible formulas render, max 4 requests in flight.
  - Formula palette (Ctrl+Shift+P); Ctrl+M / Ctrl+Shift+M insert inline/display
    math (no-op inside a formula).
  - Book size (5M chars): typing ~2 ms per keystroke, jump to end ~18 ms.
  - 79 web tests.
- Milestone 3, step 1 (`crates/typeset`):
  - Typst 0.15.1 formula rendering to SVG with embedded fonts, width/height/
    baseline for inline alignment, readable error messages, 2000-entry cache
    (cold 6.6 ms, warm ~0.35 ms, hit ~1 µs). Formulas are untrusted: no file or
    package access, a syntax guard rejecting loops/let/risky functions, 8 KiB
    cap, large-stack thread with panic catching.
  - Sources via hayagriva 0.10.1: parse `references.bib` (per-entry fallback on
    errors), format a reference in 10 curated CSL styles, import with duplicate
    keys skipped and original formatting kept.
  - Tauri commands: `render_math`, `bib_entries`, `bib_import`, `bib_format`,
    `bib_styles` (own lock; never blocks editing).
- Milestone 2, step 3 (author view):
  - Calm writing column (~68ch, offline serif), light/dark themes, fading top
    bar with project name, word count, toggles and sticky errors.
  - Softened Typst markup (single-line constructs, visible lines only):
    headings, strong, emphasis, raw, inline math and citation chips (no
    spellcheck), labels, comments, list markers; markers dimmed, never hidden.
  - Focus mode (Ctrl+Shift+F) and typewriter mode (Ctrl+Shift+T), off by
    default, remembered.
  - Checkpoint drawer (Ctrl+., Ctrl+Shift+S to name one): list, read-only
    preview, restore (undoable).
  - Undo steps also end on cursor moves, newline, cut, selection delete, drop;
    the WebView's native undo/redo is routed through the core.
  - Dev-only browser mode with a mock backend (`app/src/devmock.ts`), never in
    production builds.
  - 51 web tests (sync, markup, focus paragraphs, word count, times).
- Milestone 2, step 2 (Tauri app, `app/`):
  - Tauri 2.12 shell (`lethora-app`) with thin async commands over the core:
    project new/open/reopen-last, edit, undo/redo, save, checkpoints
    (list/create/preview/restore). Background thread autosaves every 1 s and
    tries an automatic checkpoint every 60 s; failures show in the UI. Explicit
    save on exit.
  - Edit queue (`app/src/sync.ts`): optimistic local edits batched per frame,
    one request in flight, one FIFO for edits/undo/redo/restore; core changes
    are transformed against edits typed meanwhile; length check per reply and
    a dev-build hash check, with automatic resync. Close waits for the queue
    (max 5 s). Project switching suspends the queue.
  - Bare CodeMirror 6 editor (no CM history), spellcheck, Ctrl+Z/Y/Shift+Z,
    Ctrl+S/O/N, empty state with New/Open.
  - 20 queue tests on Node's built-in runner (incl. 8 randomized seeds).
- Milestone 2, step 1 (core, for the CodeMirror bridge):
  - `TextChange {from, to, insert}` in UTF-16 offsets with CodeMirror change-set
    semantics; `apply_changes` validates the whole batch (order, overlap,
    bounds, surrogate pairs) before touching the text, then commits once.
  - `undo`/`redo` return `Option<Vec<TextChange>>`, `restore` returns
    `Vec<TextChange>`, derived from Loro text events (book size: undo of
    1000 scattered edits 38 ms, restore 45 ms).
  - Project lock: exclusive OS lock on `<project>/lock` (`Error::Locked`),
    taken before any read or log repair; released automatically on crash.
- Milestone 1: `lethora-core` crate (Cargo workspace, Loro 1.16).
  - `Document` with create/open, insert/delete (Unicode scalar positions).
  - Undo/redo via Loro's UndoManager; typing merges per ~1 s, explicit
    `end_undo_step()`; restore and undo/redo start new steps.
  - Autosave: CRC-framed append-only `updates.log`, fsynced per save;
    compaction into `snapshot.loro` above 4 MiB (snapshot written atomically first).
  - Checkpoints: named and automatic (10 min default), `checkpoints.json`,
    preview via `text_at`, non-destructive `restore` (Loro `revert_to`) as one
    undoable step.
  - Crash recovery: torn/corrupt log tail is cut off and the log repaired.
  - 16 tests plus an ignored book-size benchmark (6.7M chars: 6 µs/edit,
    autosave ~2 ms, open ~76 ms in release).
- Project guidelines with tech stack and layering decisions (`CLAUDE.md`).
- `TODO.md` (future) and `CHANGELOG.md` (history).

### Decided

- Going public (#1, #2, 2026-10-28, with the user): the project moved to the
  organization BechtStudio. The public repository `BechtStudio/lethora`
  starts with one fresh commit; the full private history is kept in
  `BechtStudio/lethora-archive`. The issues moved with their numbers. The
  updater endpoint, docs site (https://bechtstudio.github.io/lethora/) and
  links follow the new name. `docs/business.md` stays in the archive; the
  old `TODO.md`, which issues link to, is kept as `docs/plans/TODO.md`.

- License (#4, 2026-10-28, with the user): "or later" versions,
  `GPL-3.0-or-later` and `AGPL-3.0-or-later` (server), so the code can move
  to a later GPL version. The bundled dictionaries (GPL 2 or 3, SCOWL) and
  Loro (MIT) fit with GPL-3.0.

- Docs site (#19, 2026-10-28, with the user): the developer docs moved from
  `docs/` to `docs/dev/` (Docusaurus can't nest the manual's instance inside
  theirs); `docs/plans/`, `docs/business.md` and `docs/images/` stay. Links
  that leave the site are full GitHub URLs on `main`. Versions start at the
  1.0.0 tag; until then the site shows the current docs.

- Tracking (2026-10-27, with the user): open work moved to GitHub issues
  with milestones v0.1.1 (going public) and v1.0.0; no milestone means
  backlog; no Projects board. TODO.md is gone, `docs/plans/` is a frozen
  archive (new plans go into issues), workflow and commands are in
  CONTRIBUTING.md. Accepted limitations split into `docs/dev/known-issues.md`
  (technical) and `docs/manual/known-limitations.md` (for writers); each
  limitation lives in one place, an issue or one of these files. Docs site
  to come: Docusaurus on GitHub Pages, `/dev` from `docs/` and `/manual`
  from `docs/manual/` (#19); no GitHub Wiki.
- Editable shortcuts (2026-10-27, with the user): every top-level menu
  command can have keys (submenu entries can't); several per command;
  a taken key asks, then moves (never two commands on one key); `local`
  commands are locked and their keys reserved. Allowed: F1–F12, or Ctrl
  (+Shift/+Alt) with a character; Alt+letter stays the menu bar's. The
  defaults stay in the command list (main.ts); settings.json holds only
  changes, applied by setting each command's `keys`. Digits also match by
  the physical key (Ctrl+Shift+1 types "!"), letters never (QWERTZ).
- App settings (2026-10-27): a full-window view inside the main window
  (not a second window), modelled on current desktop settings: search and
  categories on the left, sections of rows on the right, changes apply at
  once (no OK/Cancel). The pages are data in `settings.ts`; every default
  lives in `settings.rs` (`null` restores one). The theme is the windows'
  theme from Rust, so the CSS keeps following `prefers-color-scheme`.
  Editable shortcuts are a separate step (R7b).
- Spell check (2026-10-26): checked in Rust, tokenized in the web view
  (the editor's Typst tokenizer already knows what is markup), visible
  ranges only, so book size costs nothing. Personal words in the app
  config (per user, all projects), not in the project; Ignore is not
  saved. All five regional dictionaries bundled (~8 MB). de-CH is
  LibreOffice's ß-free `de_CH_frami` (wooorm's de-CH still spells with ß).
- Versioning (2026-10-25): version wherever separately updated copies meet
  (wire, members log, synced records, files), not inside one binary. Three
  numbers: protocol (wire), feature level (what synced data a client
  understands; projects require a minimum), app version (people and bug
  reports only). The first client to use a feature raises the level;
  members who haven't updated are locked out until they do, instead of
  drifting silently. Servers refuse members ops they don't know. Folder
  sync derives the level from records (no shared file). The "older
  members" warning dialog comes with the first feature that raises the
  level.
- Key epochs (2026-10-22): any current device writes the next epoch after a
  removal; invite links keep working across epochs via an X25519 key from
  the link secret; no UI; no migration for pre-3d projects (not live yet).

- Rust core, Tauri 2 + CodeMirror 6 UI, Typst for layout/math/citations.
- Content is plain text in a Typst markup subset.
- Loro CRDT as the document model, chosen because collaboration is a later
  goal; it also provides undo/redo, version checkpoints, and stable anchors.
- `snapshot.loro` stores all updates instead of Loro's fast snapshot: in Loro
  1.16.2 the first edit after loading a fast snapshot of a large, heavily
  edited text stalls (11 s vs 8 ms). Trade-off: ~1.5x larger file.
- Undo history is per session; checkpoints are the persistent history.
- Science view: sources live in `references.bib` in the project folder
  (Zotero/JabRef interop); math input is Typst math only; graph tooling is
  decided later. Full-document compile moves to Milestone 4.
- Layout layer: stored in the document as structured settings (presets + key
  settings + expert Typst), turned into a Typst preamble only at compile time;
  content keeps only what and relative size (layering rule in `CLAUDE.md`).
  Built before figure sizing, which renders against the layout's page.
- One shared undo history for text and layout; preset chosen on "New
  project"; system fonts come with the compile worker.
- Book-size investigation: Typst's floor is ~0.8 MB/page; citations and our
  running-head check were the big extra costs. Chapter-window preview
  deferred; the paper preset stays without chapter breaks (real papers are
  short; long documents fit the thesis preset).
- Exception markers are located by invisible Typst metadata injected with
  each override at compile time (tags only, so no layout change), not by
  estimating positions in the UI; figures are matched by span so a user's
  label stays untouched.
- Collaboration and sync (design in `docs/dev/collaboration.md`, nothing built
  yet):
  - For one person on several devices and small groups (2–10); live editing
    and offline merging both; live cursors and "who wrote this" from day 1.
  - Roles Owner, Editor, Reviewer, Reader (separate Author/Layout roles
    dropped as impractical).
  - End-to-end encrypted once shared (folder or server sync), so each write
    permission is its own document: `main` (content and layout, as today),
    `review` (comments, marks, suggestions) and a signed `members` log. The
    server checks signatures and roles without reading content and does not
    depend on Loro.
  - Identity is a device key pair; guests join through links, accounts and
    OIDC are added later on top of the keys.
  - Per-device snapshot and log files, so any folder-sync tool works without
    conflicts; images become immutable hashed blobs.
  - The bibliography moves from `references.bib` into the document (both of
    two concurrent additions survive); this replaces the earlier
    "sources live in `references.bib`" decision. Zotero/JabRef interop:
    `references.bib` stays as a marked mirror whose outside edits are merged
    back in (decided 2026-10-15).
  - Self-hosted `lethora-server`: one Rust binary, WebSocket, storage on the
    filesystem, SQLite or S3; no vendor-specific services.
  - `crates/core` gets a storage interface and the sync client is written in
    Rust, keeping a later browser (WebAssembly) version open.
- Assets (2026-10-15): blob files keep their keyed-hash names (no
  overwrites, dedup, checkpoints keep old images, no file names leak once
  synced); readable names live only in the index and the text. A generic,
  Unity-like Assets explorer comes before folder sync: bottom panel,
  folders from day one, grid and list views, a status cloud per item,
  remove only unused items, drag & drop into the panel and into the editors
  (figure at the nearest paragraph boundary). Sources stay a document text
  and show as a special item.
- Assets explorer A1 (2026-10-15): renamed assets and folders get free
  names (spaces, Unicode, case; only `"`, `\`, `:`, control characters and
  leading/trailing spaces or dots refused; an item keeps its format);
  imports keep sanitized names. A folder can be removed when nothing in it
  is used. Usage counts and renames cover exact `"path"` literals in the
  content text only, not the expert Typst code. Identical content is reused
  on import only within the target folder.
- Assets explorer A2 (2026-10-15): the panel opens in all three views;
  Replace… in the panel keeps the path and swaps the content (same format),
  so every figure using it changes (the page menu's "Replace image…" still
  repoints one figure); items move by dragging onto a folder (plus
  Ctrl+X/Ctrl+V); double-click/Enter opens a large preview, inserting is a
  menu item.
- Assets explorer A3 (2026-10-16): desktop files dropped into an editor go
  to the panel's open folder; a figure drop (and the Figure button) is one
  undo step: blobs are staged first, then index entries and text go in one
  commit, so a failure leaves at most an unindexed blob.
- Folder sync (2026-10-16): the project stays a plain local folder; sync
  keeps an end-to-end encrypted copy in a separate folder the user picks
  (Dropbox, OneDrive, Syncthing, a USB stick), same per-device layout, so
  the `references.bib` mirror stays local and the server in step 3 is the
  same copy kept remotely. XChaCha20-Poly1305 with the project key only;
  device keys and signatures come with the `members` log in step 3 (without
  it a signature proves nothing). A second device joins with the project
  code (shown at setup, pasted on the other device; also the recovery key).
  Changes are found by polling every 2 s (no file-watcher dependency).
  Projects kept inside a sync-client folder stay plain text there but get
  the same live import, plus a one-time notice recommending encrypted sync.
  Sharing follows the coming server flow (2026-10-16, after a two-PC
  test): a default projects folder, sync apps detected and offered as
  one-click targets, and joining by pasting the code alone (the code finds
  its copy); a second local copy of the same synced project is refused (it
  would make two writers on one replica log).
- Title bar and menus (2026-10-16): a custom title bar with File / Edit /
  View (VS Code style) on Windows and Linux, the native global menu on
  macOS, all built from one command list that also drives the shortcuts
  (no shortcut-only features). Own window buttons through Tauri's window
  API (decided 2026-10-16 instead of `tauri-plugin-decorum`: unmaintained
  since 2024-09 and it fakes Win+Z through `enigo` for the Snap Layouts
  flyout; only that hover flyout is lost, Win+Z and edge snapping stay).
  The row keeps per-view essentials (Science: Formulas, Figure, Sources;
  Publisher: zoom, Compile, Export PDF; Pages in author/science; Assets
  everywhere); the rest
  lives in the menus. macOS (native menu, overlay title bar) waits for a
  Mac to test on. The Author / Science /
  Publisher switcher keeps its design and its fixed place in the centre;
  View lists the three views as well.
  Built after folder sync's docs.
- Tables (Milestone 6 step 3, 2026-10-21): content is `#figure(table(…),
  caption: […]) <tab:N>`; how tables look is the layout layer's
  (`tables.style`, `tables.header_bold`, `tables.caption_position`), one
  table's own style an anchored exception. Each style is a set of `table`
  arguments: the default a set rule, an exception the same arguments
  injected into its call at compile time, so no style-dependent show rule
  leaks into an exception; academic rules that need the table's rows are
  injected `table.hline`s.
