# Release 1.0

Scope, decisions, step plans, hand-test lists and known limitations for the
first release.

Planned with the user 2026-10-25. Each step follows the usual workflow
(TODO.md): detail the step with the user, build, review, hand test.

## Scope

- 1.0 is the local app plus folder sync (Milestones 1–4, 6, M5 steps 1–2,
  title bar and menus, versioning).
- The Lethora server (M5 steps 3–4) ships as "self-host, beta": documented,
  usable, no hosted service. A public or paid server needs accounts, quotas,
  billing and tested restores first.
- After 1.0: M5 step 5 (accounts and OIDC), step 6 (browser version),
  billing, several servers, the table grid editor, the chapter-window
  preview, pruning old checkpoints.

## Decisions (2026-10-25)

- License: open source, copyleft. GPL-3.0 for the app and core crates,
  AGPL-3.0 for `crates/server`. The repository is public by the release.
- Signing: macOS (Developer ID + notarization) and Windows (cloud signing
  or an OV certificate; eligibility to check). Linux unsigned.
  Changed 2026-10-27: 1.0 ships unsigned on all three systems, with
  SHA-256 checksums and build provenance from day one (the source is
  open, so anyone can check what was built); signing comes later (see
  R6, "Signing later").
- Spell check: own checker in Rust (Hunspell dictionaries via the
  pure-Rust `spellbook` crate), not the web view's. It skips Typst markup,
  math, code and labels, follows the layout's `text.lang`, and puts its
  suggestions in our right-click menu. Same on all three platforms. English
  and German bundled.
- Formats in 1.0: DOCX import, Markdown/text import, DOCX export, EPUB
  export. No pandoc (it would roughly double the app size).
- Keys in the OS keystore move to should-have (documented for 1.0): on a
  desktop it mainly protects config-folder backups, the Linux Secret
  Service is often missing, Keychain prompts annoy.

## Steps

- [ ] R1. License and notices: `LICENSE` (GPL-3.0), AGPL-3.0 for
      `crates/server` (crate `license` fields, file in the crate),
      generated third-party notices (`cargo-about` for Rust, an npm license
      list for the web side, the bundled fonts' licenses), shown from
      About. Small. Done last (user, 2026-10-26): the notices must list
      every third-party part, and R2–R6 still add some (the repository
      stays private until then).
- [x] R2. Spell check (built 2026-10-26, reviewed, dev-mock checked;
      awaiting the hand test below): `spellbook` with English and German dictionaries
      (license check per dictionary), a tokenizer that skips markup, math,
      code, labels and references, language from `text.lang`, squiggles
      in the author and science views, suggestions plus "Add to
      dictionary" and "Ignore" in the editor's right-click menu (fixes the
      lost suggestions, known issue M6 3b). Medium.

      Detailed with the user 2026-10-26:
      - New crate `crates/spell` (UI-free, `spellbook` 0.4, MPL-2.0, no_std
        so the browser version can reuse it). Five bundled dictionaries
        from wooorm/dictionaries, licenses checked
        ([README](../../crates/spell/dictionaries/README.md)): en-US, en-GB
        (SCOWL, MIT AND BSD), de-DE, de-AT (igerman98), de-CH (LibreOffice's
        ß-free `de_CH_frami`, igerman98-based; wooorm's de-CH still has ß),
        all German GPL-2.0 OR GPL-3.0. `text.lang` picks one (`en` → en-US, `de` → de-DE, region
        when given); other languages: no checking. Each dictionary loads
        on first use, off the main thread.
      - Book size: the web side tokenizes only the visible ranges with the
        existing Typst tokenizer (skips math, raw, code, citations, labels,
        comments), sends only words it hasn't seen in batches to an async
        command and caches the answer per word and language. Typing never
        waits on the checker.
      - User dictionary: one personal word list in the app config folder
        (all projects, this computer). "Ignore": every occurrence, this
        project, until it is closed; nothing saved.
      - The web view's own spellcheck goes off. View → Check spelling
        toggles it (remembered).

      Hand test (real app, `npm --prefix app run tauri dev`):
      1. A new project (`text.lang` en): type "teh recieve colour " — the
         first two get a wavy line once the cursor has left them, "colour"
         too (en-US). Layout → Language `en-GB`: "colour" is fine.
      2. Language `de`: "Donaudampfschiff Haustür E-Mail-Adresse geht's"
         are fine, "Fahradschloss" is marked. `de-CH`: "Strasse" is fine.
         `fr`: no lines at all.
      3. Right-click a marked word: corrections on top (pick one: it
         replaces only that word, Ctrl+Z undoes it), Add to dictionary
         (gone in every project, still gone after a restart;
         `dictionary.txt` in the app config folder), Ignore (gone in this
         project until it is closed and reopened).
      4. No lines in math, `raw`, `#code`, `@citations`, `<labels>`,
         comments and links; text inside `*bold*` and headings is checked.
      5. View → Check spelling off and on (remembered after a restart);
         science view shows the lines too; light and dark theme.
      6. A book-size project: typing and scrolling stay smooth; the first
         German check after opening takes a moment (dictionary load).
      7. Reviewer/reader: corrections greyed out, Add and Ignore work; in
         suggest mode a correction becomes a suggestion.
- [x] R3. Import (built 2026-10-26 as R3a–R3b, reviewed; awaiting the
      hand tests below): Markdown and plain text first (`pulldown-cmark` →
      Typst; text as paragraphs), then DOCX (own zip + XML reader:
      headings by style, emphasis, footnotes, lists, images into
      `assets/`, simple tables). Medium to large.
- [x] R4. Export (built 2026-10-26 as R4a–R4c, reviewed; awaiting the
      hand tests below): EPUB first, built on Typst 0.15's HTML export
      (experimental: if footnotes or figures don't come out cleanly, that
      part is our own); then DOCX (own writer, manuscript level: headings,
      paragraphs, emphasis, footnotes, images, tables, resolved citations
      and references; math as images). Large.

  R3 and R4 detailed with the user 2026-10-26:
  - Import goes both ways: File → Import… makes a new project from a file
    (the New project dialog, preset chosen there, title from the file),
    Edit → Insert file… inserts at the cursor (one undo step, images as
    assets). DOCX import keeps content only (headings, paragraphs,
    emphasis, lists, footnotes, tables, images, links); fonts, sizes and
    page setup are dropped, the layout comes from the preset. Word
    equations (OMML) become Typst math; on DOCX export math goes in as
    images. Extra exports this round: Markdown, HTML, plain text.
  - Architecture (hub and spoke, like pandoc but small): a new UI-free
    crate `crates/convert` (no Typst, no Loro) with one document model
    (`model.rs`: blocks and inlines, serde) between all formats. Each
    format is one module implementing `Reader` (bytes → model + warnings)
    or `Writer` (model → bytes + warnings); `registry.rs` lists them (id,
    name, extensions, detection), so the menus and dialogs list whatever
    it holds. A new format is one module plus one registry line; a bug in
    one format stays in its module. One shared model → Lethora markup
    writer (`typst.rs`) serves every import. Every import and export
    returns warnings (what was dropped or approximated), shown after the
    import or export and giving bug reports something to quote.
  - Export reads the compiled document, not the markup: the worker
    compiles the same source for Typst's HTML target and walks the HTML
    DOM into the model (`crates/typeset/src/export.rs`), so citations,
    the bibliography, numbering, cross-references and footnotes come out
    resolved. Math comes as MathML (HTML, EPUB) or rendered SVG/PNG
    (DOCX), its Typst source always kept. The writer runs in the worker
    too (`Request::Export`, like `ExportPdf`), so a writer crash can't
    take the app down. File → Export… offers every format in its save
    dialog; Ctrl+E Export PDF stays.
  - Tests: golden fixtures per format (`crates/convert/tests/fixtures/
    <format>/`: input, expected markup or model), DOCX inputs kept as
    unzipped XML folders zipped at test time, so a bug report becomes a
    fixture; round trips (DOCX and Markdown write → read). Limits on zip
    entries, sizes and nesting against hostile files.
  - Own zip reader and writer on `flate2` (already in the tree), XML via
    `roxmltree` (already in the tree), raster math via `resvg` (already
    in the tree). One new dependency: `pulldown-cmark` (MIT).

  Parts (one commit each):
  - [x] R3a. `crates/convert`: model, Reader/Writer traits, registry,
        model → Lethora markup, Markdown and plain-text readers, golden
        tests; app: File → Import…, Edit → Insert file…, warnings notice,
        dev mock. Built by Opus 2026-10-26, reviewed, dev-mock checked
        (fix: an insert no longer splits the word or label at the cursor).

        Hand test (real app):
        1. File → Import… a Markdown file with headings, emphasis, a list,
           a table, a footnote, `$x^2$` and `$$\frac{a}{b}$$`, an image
           next to it and one `[@key]`: New project opens titled after the
           file (or its front matter title); the project has it all, the
           image under `assets/<file name>/`, the citation as text plus a
           warning in the title bar.
        2. Edit → Insert file… with the cursor inside a word: the content
           goes after that paragraph; Ctrl+Z removes it and its images in
           one step. A `[@key]` of a source in this project becomes `@key`.
        3. A `.txt` file in Windows-1252 (umlauts) and one in UTF-16:
           umlauts right, the first with an encoding warning.
        4. Suggest mode: Insert file becomes one suggestion, images left out
           (warning). Reader: Insert file greyed out.

        Known: two Markdown lists separated only by a blank line merge;
        footnotes referenced twice are duplicated; Markdown column
        alignment, definition lists, super/subscript dropped; a lone image
        becomes a figure captioned by its alt text.
  - [x] R3b. DOCX reader: own zip reader, styles (headings by style and
        outline level, title), emphasis, lists (numbering.xml), footnotes,
        links, simple tables, images into `assets/`, OMML → Typst math.
        Built by Opus 2026-10-26, reviewed; every formula in the import
        fixtures is also compiled (`crates/typeset/tests/convert_math.rs`).

        Hand test (real app; files saved by Word, and one by LibreOffice):
        1. File → Import… a thesis-like Word file: title and author in New
           project, headings at the right levels (also a German Word with
           "Überschrift 1"), bold/italic, bullet and numbered lists (nested,
           one restarting at 1), footnotes, a link, a table with merged
           cells and a header row, an image with a caption (becomes a
           figure, width about as in Word), an image inline in text.
        2. Equations: a fraction, a sum with limits, a root, a matrix, a
           cases brace, aligned lines: they render in the science view and
           look like Word's.
        3. A file with tracked changes and comments: changes accepted, the
           title bar names what was dropped (comments, headers/footers).
        4. A Zotero-cited file: the citations stay as text, warning shown.
        5. Edit → Insert file… a .docx: images land in `assets/<file name>/`,
           Ctrl+Z removes everything in one step.

        Known: formatting from paragraph styles (an italic Quote style) is
        dropped; an unnumbered paragraph inside a list item ends the list;
        nested tables are flattened; hand-typed caption labels ("Figure 1:")
        without Word's Caption style stay text; `w:sym` characters from
        fonts other than Symbol (Wingdings) show as boxes; the language
        comes from the styles' default, not the text.
  - [x] R4a. Export pipeline: HTML-target compile in the worker, DOM →
        model, `Request::Export`, File → Export…; HTML, Markdown and
        plain-text writers.
        Built by Opus 2026-10-26, reviewed, HTML output checked in the
        browser pane. File → Export… has no shortcut: Ctrl+Shift+E is
        View → Markers.

        Hand test (real app):
        1. A project with headings, lists, a figure, a table with merged
           cells, citations and a bibliography, footnotes, `@fig`/`@sec`
           references, inline and display math: File → Export… → HTML.
           Open it in a browser: everything there, citations and
           references resolved, math rendered, footnotes linked both ways.
        2. The same as Markdown: images in `<name>_images/`, the title bar
           names what was simplified (math kept as Typst, merged cells).
           Import that Markdown again into a new project: it reads back.
        3. Plain text: readable, notes at the end.
        4. PDF from the Export… dialog still works; Ctrl+E unchanged.
        5. A project with a compile error: Export… names the error.

        Known: heading numbers are part of the heading text; the
        bibliography is a bulleted list; small caps and highlights keep
        only their text; links to labels become text; a single equation
        with `numbering: none` still gets a number in exports when the
        layout numbers equations.
  - [x] R4b. EPUB writer (EPUB 3: chapters at level-1 headings, nav,
        metadata from the project, MathML, footnotes as EPUB notes,
        images); own zip writer.
        Built by Opus 2026-10-26, reviewed; every export fixture project's
        EPUB passes our structural check (`epub::check`: zip layout,
        manifest, spine, XHTML well-formed, links resolve). Not yet
        checked with epubcheck or a real reader.

        Hand test:
        1. Export a book-like project (chapters, footnotes, figures, a
           table, math, citations) as EPUB; open it in Apple Books, Calibre
           or Thorium: title page, contents, chapters in order, footnotes
           pop up (Apple Books) or link back, math rendered, images shown,
           `@fig` references jump across chapters.
        2. Dark mode in the reader: text and table lines readable.
        3. If available: `epubcheck book.epub` reports no errors.
        4. Export again: the reader treats it as the same book (same
           identifier for the same title and author).

        Known: the identifier comes from title and authors (two books
        with the same both share it); "Contents", "Title page" are in
        English; no cover image; no MathML alttext; citations don't link
        to the bibliography.
  - [x] R4c. DOCX writer (styles from the layout's body font and size,
        headings, lists, footnotes, tables, images, math as images, page
        size and margins); round trip with the R3b reader.
        Built by Opus 2026-10-26, reviewed; `docx::check` runs over every
        writer fixture and export fixture project, a round trip through
        the R3b reader is compared exactly, and all ten test documents
        opened in Word 365 without repair and printed correctly to PDF
        (LibreOffice isn't installed here).

        Hand test:
        1. Export the thesis-like project as Word; open it in Word: the
           navigation pane shows the headings, styles are Word's own
           (change "Heading 1" and all headings follow), footnotes are real
           footnotes, lists continue and restart right, the table repeats
           its header row on a new page, captions renumber after
           Ctrl+A, F9.
        2. Formulas: crisp when zoomed, inline ones on the baseline,
           display ones centred with the number on the right.
        3. A click on a "Figure 2" reference jumps to the figure.
        4. Open the same file in LibreOffice and in Google Docs: no repair,
           images and formulas visible.
        5. Page size and margins match the layout (A4/Letter, two-sided →
           mirrored margins); the body font is the layout's (or Word's
           substitute when it isn't installed).

        Known: the layout's fonts aren't embedded (Word substitutes them);
        no headers, footers, page numbers, table of contents or columns;
        every table is Table Grid (the layout's table style isn't mapped);
        references are links, not REF fields (their text doesn't update
        on renumbering); a multi-line display formula's number sits on
        its first line.
- [ ] R5. Hardening: a strict Content Security Policy instead of
      `csp: null` (exceptions only for CodeMirror's inline styles,
      blob/data images and Tauri IPC; full click-through test); a recovery
      UI for a log record that passes CRC but Loro rejects ("Open at the
      last good state", keeping a copy of the damaged file) instead of
      `open()` failing. Medium.

      Detailed with the user 2026-10-26 (survey of the web side: no inline
      scripts, eval, workers, wasm or network calls in the web view; sync
      and server traffic run in Rust):
      - CSP (`tauri.conf.json`): `default-src 'self'; script-src 'self';
        style-src 'self'; style-src-elem 'self' 'unsafe-inline'; img-src
        'self' blob: data:; connect-src ipc: http://ipc.localhost`.
        `'unsafe-inline'` only for style elements: CodeMirror (style-mod)
        puts its themes into a runtime `<style>` (a nonce would be a fixed
        string, so no gain); every other style goes through CSSOM, which
        CSP allows. `blob:` for figure, asset and page images, `data:` for
        math SVGs and the CSS mask icons. Check that Tauri's own nonce or
        hash injection into style-src does not cancel `'unsafe-inline'`
        (else `dangerousDisableAssetCspModification` for style-src).
        `devCsp` adds `ws://localhost:1420` (Vite HMR) and inline styles.
        Click-through: the dev mock build with the same policy as a
        `<meta>` tag in the browser pane (no CSP violations in the console
        across all views, dialogs and panels), then the real app.
      - Recovery, own files: a record of this device's own log (main or
        review) that passes its crc but Loro rejects ends opening with a
        distinct "damaged" error naming the file. The app asks: "Open at the
        last good state" (stop at that record, like a torn tail: the bad
        record and everything after it are dropped, after a byte-exact copy
        of the log went to `backup/damaged/`) or Cancel. After recovery a
        notice says what was dropped and where the copy is. The same
        question comes when the project reopened at start-up is the damaged
        one (the start screen asks instead of skipping it silently).
      - A rejected own snapshot: no recovery, a clear error naming the file;
        nothing is changed.
      - Other devices' files that are skipped on open (already the case: a
        torn tail or data Loro rejects) now get a notice in the title bar
        (which device's file), nothing changed.

      Parts (one commit each): R5a CSP; R5b recovery (core error and
      recovering open with tests, app dialog and notices, dev mock hooks).
      - [x] R5a. CSP in `tauri.conf.json` as a directive object. Built by
        Opus 2026-10-26, reviewed; the dev mock build with the same policy
        as a `<meta>` tag showed no violations across the three views,
        Pages, Review, Assets, layout panel, menus and dialogs (a test
        inline script was blocked). Findings from Tauri 2.12's source:
        Tauri adds hashes and nonces to `script-src` and `style-src` only,
        never to `style-src-elem`, so `'unsafe-inline'` there holds; on
        desktop the policy is a header of the `tauri://` asset handler, so
        `tauri dev` (loads from Vite) gets none and `devCsp` would do
        nothing. No `devCsp` set.

        Hand test (a built app, not `tauri dev`:
        `npm --prefix app run tauri build -- --debug --no-bundle`, then
        `target/debug/lethora-app.exe`; F12 opens the devtools):
        1. Devtools console: no "Content Security Policy" errors after
           opening a project and going through Author, Science, Publisher,
           Pages, Review, Assets, the layout panel, every dialog in the
           menus, Import…, Export…, and a right-click on a misspelled word.
        2. The editor looks as usual (themes, syntax colours, squiggles);
           math, figure and asset thumbnails, page previews and the splash
           show.
        3. The font picker lists fonts in their own face; a character
           style's font shows in the text.
        4. Server sync and folder sync still work (they run in Rust).

        Known: the policy applies to built apps only.
      - [x] R5b. Recovery. Built by Opus 2026-10-27, reviewed, dev mock
        clicked through (`?damaged`, `?skipped`, combinable): the start-up
        question over the start screen, Cancel, the question from the
        recent list, the recovery and skipped notice, dismiss. Core:
        `Error::Damaged { file }`, `Document::open_at_last_good`,
        `recovered()`, `skipped_on_open()`; main and review are both read
        before anything is written. App: `project_open` (with `recover`)
        and `project_last` return `{opened}` or `{damaged: {path, file}}`.
        Decided with the user: the title-bar notice is only for other
        devices' data that was rejected or unreadable, not torn tails
        (those are retried on the next pull).

        Hand test (`tauri dev`; close the app before changing files):
        1. In a test project, append a crc-valid record that Loro rejects
           to `main/<device>.log` (PowerShell, in the project folder:
           `$r = [byte[]](20,0,0,0,214,23,133,136,108,101,116,104,111,114,97,45,100,97,109,97,103,101,100,45,116,101,115,116); $f = [IO.File]::Open("$PWD\main\<device>.log", 'Append'); $f.Write($r, 0, $r.Length); $f.Close()`),
           then make a small edit to another project and open the test
           project via File → Open: the dialog names the file; Cancel
           keeps the current project.
        2. Open again → "Open at the last good state": the text is as
           before the bad record, the notice says what was dropped, and
           `backup/damaged/main-<device>.log` is byte-identical to the
           damaged log. Edit, close, reopen: no question.
        3. Damage it again and restart the app with it as the last
           project: the start screen shows and asks; Cancel stays there.
        4. Damage `main/<device>.snapshot`: a clear error naming it; the
           files are unchanged.
        5. With folder sync, damage another device's log record: the
           project opens with a title-bar notice naming that device.

        Known: with server sync, changes this device already pushed and
        then dropped stay on the server, and other devices' later changes
        built on them wait on this device (see known-issues).
- [ ] R6. Release: GitHub Actions matrix (Windows, macOS, Linux) that
      builds, signs and publishes to GitHub Releases;
      `tauri-plugin-updater` with its own signing key and a manifest next
      to the release files (checks on start, asks before installing);
      version 1.0.0 and a CHANGELOG release section. Then the open
      cross-platform hand test (Linux, Windows, macOS). Medium.

      Detailed with the user 2026-10-27 (no `.github/` yet; the
      typesetter worker is the app's own exe, so no sidecar to bundle or
      sign; versions come from `[workspace.package]` and
      `app/package.json`):
      - CI checks (`.github/workflows/ci.yml`): on push and pull request,
        `cargo fmt --check`, clippy (`-D warnings`), `cargo test
        --workspace`, `npm --prefix app test` and the web build, on
        Ubuntu 22.04, Windows and macOS. Cargo and npm caches.
      - Release (`.github/workflows/release.yml`): a `v*` tag builds with
        `tauri-action` into a draft GitHub Release; the user publishes it.
        Formats: Windows NSIS only (per-user, no admin; `targets` instead
        of `all`, so no MSI), macOS two DMGs (aarch64 and x86_64, not
        universal), Linux AppImage and `.deb` on Ubuntu 22.04 (oldest
        glibc). The tag must match the app version (the job fails
        otherwise).
      - Unsigned first (decided 2026-10-27). Instead, from the first
        release: a last job hashes every release file into
        `SHA256SUMS.txt` on the release, and
        `actions/attest-build-provenance` records which commit and
        workflow run built each file (checked with `gh attestation verify
        <file> --repo BechtStudio/lethora`). The README shows both checks
        (`sha256sum -c`, `Get-FileHash`, `shasum -a 256`). Checksums catch
        broken or altered downloads; the provenance ties a file to this
        open source. Updates are checked anyway by the updater's own
        signature.
      - Signing hooks stay in the workflow, a no-op while their secrets
        are missing: macOS Developer ID and notarization (App Store
        Connect API key), Windows a `signCommand`. Linux stays unsigned.
      - Signing later (checked 2026-10-27):
        - Windows, Azure Artifact Signing (was Trusted Signing): public
          certificates for organizations in the EU (also US, CA, UK, CH,
          NO, AU, NZ, JP, KR, SG, IL); individuals only in the US and
          Canada. Basic SKU about €8–9 a month (5,000 signatures), billed
          in full per month, no free or trial subscriptions. Needs a legal
          entity (name, business identifier, website, email on its
          domain); whether a sole trader passes is unclear. Validation
          takes 1–20 business days. The certificate shows the entity's
          legal name. SmartScreen reputation still builds with downloads,
          so warnings fade rather than vanish at once. Signs from GitHub
          Actions (`azure/artifact-signing-action` or `signtool` with its
          dlib).
        - Windows, SignPath Foundation (free for open source): OSI license
          for all code (GPL/AGPL fine, no proprietary parts, no commercial
          dual licensing), actively maintained, already released in the
          form to be signed, and for executables "a certain verifiable
          reputation" (no fixed threshold; checked once at the start), so
          after 1.0 has some users. The certificate names SignPath
          Foundation as publisher, not Lethora. Builds must come from CI
          from the repository, every release is approved by hand, MFA for
          everyone on the team, and the home and download pages need a
          "Code signing policy" section (roles, attribution, privacy). Data
          sent to systems the user didn't choose must be disclosed and
          switchable: the update check (its Help toggle covers that). Their
          pages name Windows formats only; macOS still needs Apple.
        - macOS: Apple Developer Program (99 USD a year, individuals
          accepted). Until then the README explains the first start of an
          unsigned app (System Settings → Privacy & Security → Open Anyway).
      - Updater: `tauri-plugin-updater` and `tauri-plugin-process`
        (relaunch), `createUpdaterArtifacts`, its own minisign key (private
        key and password as GitHub secrets, public key in
        `tauri.conf.json`; the user keeps a backup: losing it strands every
        installed copy). Endpoint
        `https://github.com/BechtStudio/lethora/releases/latest/download/latest.json`
        (written by `tauri-action`; needs the public repository). The check
        runs in Rust (the CSP stays), quietly a few seconds after start;
        a found update asks first: "Lethora X is available" with its notes,
        Install and restart / Later. Help → Check for updates… (says "up to
        date" or the error), and Help → Check for updates on start (a
        checked item, stored next to `recent_projects.json`, on by
        default). Check while building whether the plugin updates a
        `.deb`; if not, `.deb` installs get a notice linking to the release
        instead.
      - Version 1.0.0 in all manifests, a CHANGELOG release section, a
        README install section (first start on macOS and Windows while
        unsigned or new: Gatekeeper, SmartScreen; Linux AppImage and
        `.deb`).
      - The server gets no release files yet: docs/dev/server.md keeps the
        Docker build from source.

      Parts (one commit each): R6a CI checks; R6b release workflow and
      signing hooks; R6c updater; R6d version 1.0.0 and docs.
      Order: see "New order" below R10 (changed 2026-10-27: test
      releases 0.1.0 and 0.1.1 instead of 0.9.x).
      - [x] R6a. CI checks. Built by Opus 2026-10-27, reviewed; every
        command passes locally on Windows. One job per system: checkout,
        Rust stable, rust-cache, Node 24, the apt packages of
        `scripts/setup-linux.sh` (Linux), fmt (Linux only), `npm ci`, web
        tests and build, clippy and `cargo test` with `--locked`. No xvfb:
        no test needs a display (the worker test runs the exe with
        `--compile-worker`, before any Tauri code). The web build doesn't
        need to come before Cargo (debug builds use `devUrl`). Awaiting
        the first push, which is its hand test: first Linux build and test
        run ever, including the 256 MB `RLIMIT_AS` worker test and
        `/proc` memory reads; the font tests need a system font on each
        runner.
      - [x] R6b. Release workflow. Built by Opus 2026-10-27, reviewed.
        Jobs: `draft` (tag = version of `lethora-app` from `cargo
        metadata` and `app/package.json`, notes from the CHANGELOG section
        or a link, one draft reused on re-runs) → `build` matrix with
        `tauri-action@v1` (`releaseId`, no updater JSON yet; Intel Mac
        cross-compiled on the arm64 runner; no Rust cache: CI caches debug
        builds) → `checksums` (`gh release download`, sorted `sha256sum`,
        upload; `attest-build-provenance@v4` over `SHA256SUMS.txt` only
        when the repository is public: attestations in private repos need
        GitHub Enterprise Cloud). `bundle.targets` is one list (`nsis`,
        `dmg`, `appimage`, `deb`; the bundler drops what doesn't fit the
        system); `bundle.macOS.signingIdentity: "-"` for the ad-hoc
        signature Apple Silicon requires. Tauri treats an empty variable as
        set, so the macOS step passes on only the secrets that exist;
        with a real certificate, `APPLE_SIGNING_IDENTITY` must be set too.
        The Windows `signCommand` comes in as a `--config` file only with
        the `WINDOWS_SIGN_COMMAND` secret (tested locally with a dummy
        command: it ran on the exe, the NSIS DLLs and the installer).
        Locally: `tauri build` makes only the NSIS installer (20.7 MiB).
        Tested 2026-10-27 with the tag `v0.1.0` (private repository, draft
        left in place): all four builds and the checksums job passed; the
        draft has the NSIS installer (21 MB), both DMGs (31 and 33 MB),
        AppImage (107 MB) and `.deb` (34 MB) plus `SHA256SUMS.txt`; the
        Windows installer matches its sum. A `v0.0.1` tag failed in the
        draft job as it should (hand test 3). Left: installing and
        starting each file (hand test 2), the sums on macOS and Linux and
        the attestation (hand test 6, needs the public repository).
      - CI test 2026-10-27 (hand test 1): green on all three systems after
        three fixes: no debug info in CI (`CARGO_PROFILE_DEV_DEBUG=0`; the
        Linux linker died with a bus error on a full disk), the Windows
        sync-apps test only on Windows (backslash paths), and the worker
        timeout test at 3 s (the limit includes the cold worker start).
        Tag pushes no longer run CI (`branches: ["**"]`).
      - [x] R6c. Updater. Built by Opus 2026-10-27, reviewed, dev-mock
        checked (menu items, dialog, progress, errors). `tauri-plugin-updater`
        2.14 (native-tls, no zip), no `tauri-plugin-process`: Rust
        commands `update_check(manual)` and `update_install` (download
        with `update-progress` events, save, install,
        `AppHandle::restart()`); the plugin's before-exit hook runs
        `close_project` (Windows exits at once for the installer).
        App-wide `settings.json` (user decision 2026-10-27, instead of its
        own file; R7 grows it): `settings_get`, `settings_set(changes)`,
        unknown fields kept. The release workflow's "Signing hooks" step
        writes one `--config` file (Windows `signCommand`,
        `createUpdaterArtifacts`) and sets `uploadUpdaterJson` only with the
        `TAURI_SIGNING_PRIVATE_KEY` secret. `.deb` (from the plugin source):
        supported, `dpkg -i` through pkexec/sudo, `latest.json` key
        `linux-x86_64-deb`, falling back to `linux-x86_64`. Release notes
        show as text with Markdown `#`s stripped.
        The user's updater key (minisign key id 9355EBC9FCC29EA9, with a
        password; the user keeps the backup) is in `plugins.updater.pubkey`
        since 2026-10-27; its private half and password are the repository
        secrets `TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)`. Untested
        until the 0.1.0 → 0.1.1 hand test (4, 5): a real check, download, install
        and restart; reopening the project after restart (read from the
        code: `project_last`); whether the bundler signs the `.deb` and
        tauri-action writes `linux-x86_64-deb`; whether `latest.json` lists
        all four platforms (the matrix jobs merge into it); a key without
        a password on CI (give the key a password).
        First `v0.1.0` with the key (2026-10-27): Windows and Linux
        updater files and `latest.json` (keys `windows-x86_64(-nsis)`,
        `linux-x86_64(-appimage|-deb)`) came out, macOS none (the bundler
        makes its `.app.tar.gz` only with the `app` target: added), and
        `notes` was empty (tauri-action fills it from `releaseBody`, not
        from the release: now the CHANGELOG section from the draft job).
        Asset URLs are API URLs (work once the repository is public).

      Hand test:
        1. A push runs CI green on all three systems.
        2. A test tag builds a draft release with the NSIS installer, two
           DMGs, AppImage, `.deb`, `latest.json` and `.sig` files; each
           installs and starts (unsigned: the warnings the README names).
        3. A tag that doesn't match the version fails the job.
        4. With 0.1.0 installed on each system and 0.1.1 published: the
           start-up check asks, Later leaves it, Install updates and
           restarts into 0.1.1 with the open project; Help → Check for
           updates then says up to date; with the start-up check off,
           nothing asks.
        5. Offline: the start-up check stays silent; the Help item shows
           the error.
        6. `SHA256SUMS.txt` matches each downloaded file on all three
           systems, and `gh attestation verify` passes for one file per
           system.
        7. Later, with certificates: no Gatekeeper warning (`spctl -a
           -vv`), the Windows file's Digital Signatures tab names the
           publisher.

Added by the user 2026-10-27, before the 1.0 release and going public
(each detailed with the user when it is next; until then only the
outline below):

- [ ] R7. App settings: a preferences dialog for global app settings
      (this computer, all projects; stored in the app config folder next
      to `recent_projects.json`), gathering the scattered ones (spell
      check, update check on start, editor display settings: line
      spacing, paragraph gap, text size of the author and science views).
      Split (2026-10-27): R7a the settings view, R7b editable shortcuts.
      - [x] R7a. Settings view. Built by Opus 2026-10-27, reviewed,
        hand-tested by the user (2026-10-27, no findings), dev-mock checked (pages, search, live text size, update check,
        light and dark). User decisions: full-window view like the Claude
        desktop app (File → Settings…, Ctrl+,; macOS Lethora → Settings…,
        Cmd+,), search above a category list, sections of rows; View →
        Check spelling and Help → Check for updates on start stay as
        shortcuts to the same settings; one text size for the author and
        science views. Pages: General (reopen the last project), Appearance
        (theme: system, light, dark), Editor (text size, line spacing,
        paragraph gap, spell check), Keyboard shortcuts (read only, from
        the command list), Updates (check on start; Check now, Install and
        restart… through the update dialog), About (facts, Copy diagnostic
        info). Data-driven (`settings.ts`: categories → sections → rows; a
        new setting is one row plus one field in `settings.rs`, where all
        defaults live). Changes apply and save at once; "Restore defaults"
        per page sends `null`, which removes the field so its default
        applies. Spell check moved from localStorage (`lethora.spell`,
        carried over once) into `settings.json`. The theme is
        `AppHandle::set_theme` (WebView2 `PreferredColorScheme`, macOS app
        appearance), applied in setup before any window paints, so the CSS
        keeps following `prefers-color-scheme`. Reopen is gated in Rust
        (`project_last`), so a web-view reload keeps the open project.
        Known: Linux theme untested (GTK prefer-dark, may not reach
        WebKitGTK); with reopen off, the restart after an update starts on
        the start screen; window-wide shortcuts still act behind the view
        (as behind the other dialogs); shortcuts show as "Ctrl+…" on macOS.

        Hand test (`npm --prefix app run tauri dev`):
          1. First start with no `settings.json`: today's look, the last
             project reopens, spelling on.
          2. Ctrl+, opens Settings with the focus in the search field;
             Esc clears a search, then closes.
          3. Theme Dark / Light / Match system switches the whole window at
             once (title bar, editor, panels); after a restart the chosen
             theme shows from the first frame (splash too, no flash).
          4. Text size, line spacing and paragraph gap change the author and
             science views live (formulas follow the text size); the
             publisher view doesn't change; Restore defaults puts them back.
          5. Check spelling in Settings and View → Check spelling stay in
             step; a previous "off" from View carries over once.
          6. Reopen the last project off: the next start shows the start
             screen; on again: it reopens.
          7. Updates → Check now: up to date, or offline shows the error.
          8. About shows the facts; Copy pastes them.
          9. macOS: Lethora → Settings… (Cmd+,) opens it; the theme switch
             works.
      - [x] R7b. Editable shortcuts. Built by Opus 2026-10-27, reviewed,
        dev-mock checked (record, conflict and Replace, refused keys, Esc,
        menu label, the key running its new command, Restore defaults,
        light and dark). User decisions: every top-level menu command is
        listed (also those without a default key); several keys per
        command (chips with ×, Add, Reset); a key another command uses
        asks ("used by", Replace / Cancel) and then moves; macOS shows
        ⌘⇧⌥. Locked: the `local` commands (Undo, Redo, Cut, Copy, Paste,
        Select all, zoom, Exit), their keys reserved. Rules in one place
        (`keysProblem`, commands.ts): F1–F12, or Ctrl (+Shift/+Alt) with a
        character; refused: typing and cursor keys, no Ctrl, Alt+F4,
        Ctrl+R, Ctrl+Shift+R, Ctrl+Q. Stored as `shortcuts` (command id →
        keys, only changed commands) in `settings.json`; `null` restores.
        Known: toolbar tooltips and help texts keep the default keys;
        Escape on the "used by" line closes Settings; macOS shows Exit as
        ⌥F4.

        Hand test (`npm --prefix app run tauri dev`):
          1. Settings → Keyboard shortcuts: Add on Open shared project,
             press Ctrl+B: "Ctrl+B is used by Bold"; Replace. File menu
             shows Ctrl+B there, Ctrl+B in the text opens the dialog, Bold
             has no key.
          2. Add, then B alone, Ctrl+Z or Ctrl+R: refused with the reason;
             Esc while it waits cancels and leaves Settings open.
          3. × removes a key (it no longer works); Reset brings the
             defaults back; Restore defaults resets every command.
          4. Restart: changed keys stay (settings.json `shortcuts`).
          5. Ctrl+Shift+1 (or another digit) on a command works, also on
             a German keyboard; Ctrl+Alt+letter works without AltGr's
             character being typed.
          6. macOS: the page shows ⌘⇧⌥; a changed key appears in the
             menu bar and runs the command.
- [ ] R8. App language: a translatable UI (string catalog, English
      first, German second), the language picked in R7 settings
      (default: the system language).
- [ ] R9. Docs and user manual: hosted docs from what `docs/` and the
      README already note (getting started, task walkthroughs, reference;
      see TODO.md "Later", User manual); decide the form with the user
      (website, in-app help or both) and a novel-like sample.
- [ ] R10. Contributing: `CONTRIBUTING.md` (build and test setup, the
      layering rules from CLAUDE.md, commit and pull-request rules, code
      style, how to report bugs), a code of conduct, issue and
      pull-request templates.

New order (user, 2026-10-27): test release 0.1.0 now (R6a–c, with the
updater key; installed from the draft) → R7 settings → repository public
(before that: a secret scan over the whole git history and the LICENSE
files, the quick half of R1) → test release 0.1.1, published as a normal
release (the updater reads GitHub's latest release, which needs the public
repository) → the update hand test (4, 5; the user's plan: start 0.1.0
offline so the quiet check fails, turn the check on start off, then online
Help → Check for updates…, install, restart into 0.1.1) → R8–R10 → R1's
third-party notices → R6d and the 1.0.0 tag.

## Outside the code (the user, can start now)

- Not needed for 1.0 (ships unsigned, see R6 "Signing later"): the Apple
  Developer Program; Windows signing through Azure Artifact Signing (as an
  organization) or, once Lethora has users, SignPath Foundation.
- [x] Create the updater key and keep a backup of it and its password
  outside the repository (done 2026-10-27, secrets set).
- Make the repository public before the release.

## Should-have (polish, not blocking)

(The user manual, preferences dialog and German UI moved up to R7–R9.)
macOS shortcuts shown as ⌘ instead of "Ctrl+…"; AltGr+M (µ) opening a
comment on German keyboards; the title bar clipping below ~700 px;
read-only gaps in the publisher, assets and exceptions menus; keys in the
OS keystore; a triage pass
over [known-issues.md](known-issues.md) for what a normal user hits.
