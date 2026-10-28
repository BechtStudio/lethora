# Lethora

A simple, distraction-free writing editor for literature authors, scientists,
and publishers. Fast at book size, with undo/redo and autosave checkpoints built
into the core.

> Early development: all three views work; see the
> [issues](https://github.com/BechtStudio/lethora/issues) and
> [milestones](https://github.com/BechtStudio/lethora/milestones) for what is
> still missing.

## Views

- **Author view**: plain text with softened Typst markup, Typst-style
  paragraphs, focus and typewriter modes, checkpoints you can preview and
  restore.
- **Science view**: rendered math with a formula palette, a sources library
  (`references.bib`, CSL styles, `@` citations and cross-references), figures
  previewed at their size in the layout's text column.
- **Publisher view**: typeset pages (Typst) with layout presets (paper,
  novel, thesis, manuscript) and a layout panel; per-element exceptions (page
  and column breaks, extra space, figure placement, image quality) with
  markers on the pages, an edit popover and an Exceptions drawer (F7); PDF
  export with image quality presets. The pages can also show in a split pane
  beside the author or science view (F6).
- **Assets panel** (F8, in every view): the project's images in folders,
  as previews or a list with size, usage and status; rename and move
  (references in the text follow), replace, save, remove unused ones; drag
  files from the desktop into the panel or into the text to add figures.
- **Share / Sync** (the cloud next to the project name): an end-to-end
  encrypted copy of the project in Dropbox, OneDrive, iCloud Drive, Google
  Drive (one click) or any other synced folder; your other devices choose
  "Open shared project…" and paste the project code. Their changes arrive
  while you type. Or share through a Lethora server (below): invite people
  by link with a role, see who is online and their cursors, and View →
  Show authors colours the text by who wrote it.
- **Review** (View → Review, in the author and science views): comments
  (Ctrl+Alt+M), highlights and suggestions on cards beside their lines,
  with replies, resolve and an Open / Resolved / All filter. Edit →
  Suggest changes (Ctrl+Shift+X) turns edits into suggestions (insertions
  underlined, deletions struck through) that an Owner or Editor accepts or
  rejects.
  Reviewers comment and suggest without editing the text; Readers see
  everything read only.
- **Title bar and menus**: one slim row with File / Edit / View, the
  project name, the view switch in the centre and each view's tools; on
  Windows and Linux it replaces the native title bar (own window buttons,
  drag to move). Every shortcut is also a menu item, and the menus work
  from the keyboard (Alt or F10, Alt+letter). The start screen and File →
  Open recent list your recent projects; File → Close project (Ctrl+W)
  goes back to the start screen.

Main shortcuts: Ctrl+1 / 2 / 3 switch views, Ctrl+F / Ctrl+H find and
replace in the text, Ctrl+S save, Ctrl+Shift+S checkpoint, Ctrl+E export PDF, F5
compile, F6 pages beside the text, F7 exceptions, F8 assets, Ctrl+W close
project. The menus show the rest.

The text is the single source of truth; layout is a separate layer applied on
top and never changes the text.

## Stack

- **Core** (`crates/core`): Rust, [Loro](https://loro.dev) CRDT document,
  append-only autosave log, snapshots, checkpoints, anchors, crash recovery.
- **Typesetting** (`crates/typeset`): [Typst](https://typst.app) as a library
  for math, citations, the layout layer, pages and PDF, run in a separate
  worker process with a timeout and a memory cap.
- **App** (`app/`): [Tauri 2](https://tauri.app) with a
  [CodeMirror 6](https://codemirror.net) editor (TypeScript, no framework).
  Every edit goes through the Rust core.
- **Vendored** (`vendor/loro-internal`): `loro-internal` 1.16.2 with a fix for
  slow checkpoint restore/undo across large pastes
  ([patch](vendor/loro-internal-fix.patch)). Remove once upstream ships a fix.

## Development

Requires Rust (stable) and Node.js 24+. On Windows, the WebView2 runtime
(included in Windows 11); on Linux see [Linux](#linux).

```bash
npm --prefix app install
npm --prefix app run tauri dev     # run the app
cargo test --workspace             # Rust tests
npm --prefix app test              # web tests
```

To run Lethora on another computer, build a release (after `npm --prefix
app install`):

```bash
npm --prefix app run tauri build
```

The installer lands in `target/release/bundle/nsis/Lethora_<version>_x64-setup.exe`
(per-user, no admin rights). `target/release/lethora-app.exe`
also runs on its own (the compile worker is the same executable). The builds
are unsigned, so Windows SmartScreen warns on first start ("More info" →
"Run anyway"). Typst uses the computer's installed fonts.

### Linux

Needs Ubuntu 22.04 / Debian 12 or newer (WebKitGTK 4.1) and Node.js 24+
(e.g. via [nvm](https://github.com/nvm-sh/nvm); the distribution's `nodejs`
is too old). On Debian/Ubuntu one script installs the system packages, Rust
(rustup, if missing) and the web dependencies; other distributions: the
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/#linux).

```bash
scripts/setup-linux.sh
npm --prefix app run tauri dev     # run the app
npm --prefix app run tauri build   # release
```

A folder copied from Windows needs a fresh `npm --prefix app install`
(`node_modules` holds Windows builds). The release lands in
`target/release/bundle/`: `deb/Lethora_<version>_amd64.deb` and
`appimage/Lethora_<version>_amd64.AppImage` (runs on its own after
`chmod +x`); `target/release/lethora-app` also runs directly.

If something fails:

- `GLib-GIO-ERROR ... does not contain a key named 'antialiasing'` when
  the window opens: the terminal is VS Code's from the snap, whose GTK/GIO
  paths leak into the app. Run from a normal terminal, or unset them:
  `env -u GTK_PATH -u GIO_MODULE_DIR -u GTK_EXE_PREFIX -u GTK_IM_MODULE_FILE
  -u GSETTINGS_SCHEMA_DIR -u LOCPATH npm --prefix app run tauri dev`
  (VS Code from Microsoft's `.deb` avoids it).
- Blank or white window (NVIDIA, some Wayland setups): start with
  `WEBKIT_DISABLE_DMABUF_RENDERER=1`.
- AppImage bundling fails while stripping: build with `NO_STRIP=true`.

Book-size benchmarks (release):
`cargo test --release -p lethora-core -- --ignored --nocapture` and
`cargo test --release -p lethora-typeset --test worker -- --ignored --nocapture`

## Run a sync server

Shared projects go through `lethora-server` (`crates/server`), one binary
that stores and forwards end-to-end encrypted records. It speaks plain
HTTP/WebSocket; HTTPS comes from a reverse proxy such as Caddy.

```bash
cargo build --release -p lethora-server
target/release/lethora-server --config lethora-server.toml
# or with Docker and Caddy (automatic HTTPS):
LETHORA_DOMAIN=sync.example.org docker compose -f deploy/docker-compose.yml up -d
```

In the app, Share / Sync → "Lethora server" takes the server's address
(`public_url`, e.g. `https://sync.example.org`). Configuration, reverse
proxies, systemd, backups and limits: [docs/dev/server.md](docs/dev/server.md);
example files in [deploy/](deploy).

## Documentation

Once the repository is public, the docs are also published as a site at
https://bechtstudio.github.io/lethora/.

- [docs/dev/architecture.md](docs/dev/architecture.md): architecture, data model, key
  flows, commands, conventions, testing.
- [docs/dev/collaboration.md](docs/dev/collaboration.md): decisions on
  collaboration and sync.
- [docs/dev/server.md](docs/dev/server.md): running the sync server (config,
  reverse proxy, Docker, systemd, backups).
- [docs/dev/key-epochs.md](docs/dev/key-epochs.md): a new project key after a
  member is removed.
- [docs/dev/snap-layouts.md](docs/dev/snap-layouts.md): Windows 11 Snap Layouts on
  the own maximize button.
- User manual: planned. Screenshots in [docs/images](docs/images)
  (`node scripts/screenshots.mjs`), e.g. [the menus](docs/images/menus.png),
  [the start screen](docs/images/start-screen.png) and
  [the publisher view](docs/images/publisher.png) (1 zoom, 2 the View menu
  with markers and exceptions, 3 Export PDF), and for shared projects
  [members](docs/images/members.png), [live presence](docs/images/presence.png),
  [Show authors](docs/images/show-authors.png), [the Review
  panel](docs/images/review-panel.png),
  [suggestions](docs/images/suggestions.png) and [accepting
  one](docs/images/review-menu.png).
- [CHANGELOG.md](CHANGELOG.md): history and decisions.

## License

Lethora is free software under the [GNU General Public License v3.0 or
later](LICENSE). The sync server (`crates/server`) is under the [GNU Affero
General Public License v3.0 or later](crates/server/LICENSE).
