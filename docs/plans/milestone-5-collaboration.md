# Milestone 5: Collaboration and sync

Step plans, decisions, hand-test lists and known limitations for Milestone 5 (design record: `../dev/collaboration.md`).

Design: `docs/dev/collaboration.md` (roles Owner/Editor/Reviewer/Reader;
documents `main`, `review`, `members`; end-to-end encryption once shared).
Open questions in its section 12 are decided when the step needing them is
planned.

- [ ] 1. Foundation, local only (planned 2026-10-15; one build + review +
      commit per part; no keys, encryption, `review` document, `members`
      log, live import of other devices or attribution UI yet):
      - [x] 1a. Storage interface in `crates/core` (read, list, atomic
            write, log append, lock), file code moved behind it. Layout:
            `project.json` (format number, marks a project),
            `main/<device>.snapshot|log`, `checkpoints/<device>.json`,
            `<device>.lock`. Open: own files (torn tail repaired), then other
            devices' files read-only (torn tail skipped, pending changes
            tolerated). The own log records everything new since the last
            save, imports included, so own files are self-contained. Format-1
            projects converted on open, old files moved to
            `format1-backup/`. Random device id (`device.json`) and user id
            (`profile.json`) in the app config dir. Loro timestamps on.
            Done, awaiting the hand test. Known: if the app config dir can't
            be written, every start gets a new device id (a new file set per
            project); Loro merges a device's changes within 1000 s into one
            timestamp (see 1b); importing another device's files is silent when it skips them.
      - [x] 1b. `peers` map in `main` (peer → user, device), written in its
            own commit with an origin the UndoManager excludes; core query
            for the author at a position. No UI. Done, awaiting the hand
            test (with 1a). Restore's bookkeeping filter (`BOOKKEEPING` in
            core) is ready for 1c's `checkpoints` map. Attribution times
            stay at Loro's 1000 s change merging; revisit with the UI in
            step 3 if finer times are wanted.
      - [x] 1c. Named checkpoints in a `checkpoints` map in `main`, automatic
            ones in `checkpoints/<device>.json`; ids stay JS-safe numbers;
            commands and drawer unchanged. Restore leaves the bookkeeping
            maps (`checkpoints`, `peers`) alone (plain `revert_to` would drop
            newer checkpoints); creating one is not undoable. Done,
            awaiting the hand test (with 1a, 1b). Known: typing right after
            a named checkpoint starts a new undo step (Loro behaviour after
            an excluded commit); the drawer doesn't show who made a
            checkpoint yet (stored as `user`/`device`).
      - [x] 1d. `bibliography` text in `main`; `bib_*` commands use it; the
            compile request carries it and the worker serves a virtual
            `references.bib`; imports recompile and are undo steps.
            `references.bib` stays as a mirror for Zotero/JabRef: written
            after changes; outside edits (on focus/F5) merged through a fork
            at the last written version, so concurrent edits survive. Done,
            awaiting the hand test. Known: undoing an import says "Undid a
            layout change"; two devices open at once can both import the
            same outside edit (duplicates; folder-sync territory, step 2); a
            device without state whose history is older than a newer marked
            mirror rewrites it with its older text; sources drawer labels
            still say "references.bib"; three-way merge cost at book-size
            history not measured.
      - [x] 1e. Images as blobs `assets/<hash>` (SHA-256 keyed with a
            random project key in `main`), `assets` map name → hash in
            `main`; the text keeps `image("assets/foo.png")`, the worker and
            `asset_bytes` resolve through the map; same content stored once;
            blobs never deleted (checkpoints keep their images).
            "Replace image…" on a figure's right-click menu replaces editing
            files in place. Done, awaiting the hand test. Known: the science
            view's figure preview has no right-click menu (Replace only from
            the pages); (fixed in A1/A3: a figure insert is one undo step,
            named "an asset change");
            the menu finds the path only on the
            `image(` line; adoption errors on open are only logged; a blob
            not yet arrived from another device shows "Image not found"; the
            dev mock's page clicks don't hit figure boxes on the real
            pre-rendered pages, so the menu item was checked by unit tests.
      - [x] Then: `docs/dev/architecture.md`, tests, dev mock, CHANGELOG
- [x] Assets explorer (planned 2026-10-15, built before step 2; Unity-like;
      A1–A3 hand-tested 2026-10-16):
      - [x] A1. Core/app: asset types by extension (one table in Rust and
            the web view: icon, preview, inserted snippet, import check;
            images now, data for graphs etc. later); import from bytes
            (`asset_import_bytes`, for drag & drop and the browser build);
            folders from paths (`assets/figures/ch1/…`, empty folders kept
            explicitly); rename/move of items and folders updating every
            literal path in the text in one undo step; remove only unused
            items; a listing with kind, size, used-by count (text scan) and
            status (available / missing, sync states later). Done (no UI;
            nothing to hand-test until A2). Known: usage is a literal scan
            of the content text (expert Typst and paths built from parts
            don't count; literals in comments do); restore brings moved or
            removed entries back (it never deletes index entries); "in use"
            refusals reach the UI as raw core messages; no dev-mock
            versions of the new commands yet (A2); undoing a bibliography
            import still says "layout change"
      - [x] A2. Bottom panel (resizable, F8 + bar button, all three
            views): folder tree, grid of preview squares with a size slider
            and a list view (name, type, size, used by, status; sortable),
            search over all folders, "Unused" filter, a status cloud per
            item, context menu (Open preview, Replace… = new content under
            the same path, same format; Rename; Save as…; Show usages;
            Insert as figure; Remove if unused), double-click/Enter = large
            preview, drag onto a tree folder or folder tile and Ctrl+X/V to
            move, keyboard. Sources appear as a special item opening the
            Sources drawer (they stay a document text: blobs are immutable,
            the bibliography must merge). Tauri's native file-drop handler
            is off (WebView2 swallows HTML5 drag events otherwise). Done,
            checked in the dev mock (drag via synthetic events); awaiting
            the hand test. Known: single selection only; list view shows
            icons, not thumbnails; the large preview has no full focus
            trap; Esc in the search box leaves the panel; zoom keys don't
            reach the pages while the panel has focus; figure previews
            reload after every panel step; paths used in the text but not
            in the index (e.g. a missing `sketch.png`) are not listed;
            `asset_import_bytes` isn't in the dev mock
      - [x] A3. Drag & drop: files from the desktop into the panel (into
            the folder under the pointer, else the open folder); from the
            desktop or the panel into the author and science editors → a
            figure as its own block at the nearest paragraph boundary, with
            a drop marker line; desktop files go to the panel's open folder.
            Import + figure text are one undo step (`asset_stage*` then
            `figure_insert`), the Figure button too. Stray file drops are
            blocked. Done, checked in the dev mock with synthetic drag
            events; hand-tested. Known: the Figure button now
            inserts at the cursor and keeps a selection (it replaced it
            before) and imports into `assets/`, not the open folder;
            boundaries are blank lines and the text's ends only (a heading
            followed directly by text is not one); lines far beyond the
            parsed range count as top level; typing above the drop point
            during the drop shifts it to the old boundary; two identical
            files in one drop get two paths. Later: drop onto pages, other
            file kinds
      - [x] A4. Dev mock (seeded folders, all asset commands), docs
            (`docs/dev/architecture.md`, README), screenshots `assets-panel`
            and `assets-drop`
- [ ] 2. Folder sync (planned 2026-10-16; one build + review + commit per
      part; no device keys, signatures or `members` log yet, those come with
      step 3):
      - [x] 2a. Core (`crates/core/src/sync.rs`, no UI): an encrypted copy
            of the project in a sync folder the user picks, separate from the
            plain local project folder. Same layout as the own files:
            `replica.json` (format, project id, key check),
            `main/<device>.snapshot|log`, `assets/<hash>`; every record,
            snapshot and blob sealed with XChaCha20-Poly1305 and the project
            key (versioned envelope with epoch and nonce; associated data =
            project id, document, device). Push after autosave (what is new
            since the last push; own copy self-contained, compacted like the
            local log); pull reads other devices' new records from their last
            offset (torn tails retried), imports them and returns the content
            `TextChange`s plus flags for the other parts that changed; blobs
            both ways (downloads checked by the envelope, whose associated data
            names the blob); setup and
            join (a new local project built from the copy). The same pull
            without decryption imports other devices' plain files live, for
            projects kept inside Dropbox/iCloud/etc. Done (no UI; nothing
            to hand-test until 2b). Known: older records of a device can be
            replayed (harmless, nothing changes); a device skipped for a bad
            record is re-read from scratch whenever its files grow, until it
            compacts; a garbage length field looks like a torn tail (the
            device waits instead of being skipped); the `checkpoints` flag
            misses a named checkpoint that becomes listed when its history
            arrives with another change; two devices pushing the same blob
            at once write it twice (same content); push/pull themselves
            don't check the replica's project id (2b checks it whenever the
            replica is opened)
      - [x] 2b. App: `sync_setup`/`sync_join`/`sync_stop`/`sync_status`/
            `sync_pull`; the project key in the app config dir (never in a
            synced folder), sync settings in `state/<device>.json`; polling
            every 2 s in the autosave thread → `remote-changes` event →
            `sync.ts` queues a pull through its FIFO (like restore) → open
            panels refresh; a sync status cloud in the bar and a dialog
            (setup shows the project code, join takes it, stop); guards
            against nesting sync folder and project; a one-time notice for a
            project inside a known sync-client folder (plain text there);
            dev mock. Done; checked in the dev mock (setup, code, remote
            edit + local undo, plain notice, code entry); awaiting the hand
            test. Change from the plan: the web view polls `sync_pull`
            through the FIFO every 2 s (no event), so in-flight edits are
            transformed; push runs on every 1 s autosave tick (writing
            nothing when nothing is new). Without an open replica every
            project runs `pull_local` (cheap), whatever folder it is in;
            sync-app detection only sets the plain status and the notice. Also
            `sync_code`, `sync_enter_code` (stored key missing or wrong),
            `sync_notice_dismiss`. Key in `<config>/keys/<project id>.key`,
            dismissals in `<config>/notices.json`. Known: setup (full
            snapshot + all blobs), push, pull and the 10 s reopen run under
            the document lock (a slow drive can stall edits briefly); Stop
            deletes the key by project id (a second local copy of the same
            synced project on this device loses it too); the warning cloud
            doesn't pin the bar; the dialog's "last sync" doesn't tick
            while open; the push-failure path with the folder still present
            is untested against a flaky drive; (2d: "Open shared project…"
            is on the start screen and in Share / Sync; a File menu comes
            with the title bar)
      - [x] 2d. Simpler flow, like the coming server share (decided
            2026-10-16 after the user's two-PC test of 2b): default projects
            folder `<Documents>/Lethora` (`<home>/Lethora` when a sync app
            syncs Documents), so New project asks only for a name ("Choose
            location…" stays) and Join asks nothing; Share / Sync offers the
            detected sync apps (Dropbox, OneDrive, iCloud Drive, Google
            Drive) as one-click buttons (copy in `<app>/Lethora/<name>.lethora-sync`)
            plus "Other folder…"; "Open shared project…" takes only the code
            and finds the copy in the sync apps' `Lethora/` folders and top
            folders (read-only check `replica_project`), else "Choose
            folder…"; refuses a second local copy of a project already on
            this device. Done, checked in the dev mock; hand-tested by the
            user on two Windows PCs 2026-10-23. Known: several copies of one project → the newest is used
            silently; the "already here" check covers the open, recent (T2)
            and default-folder projects only; Google Drive on Windows probes
            G: then D:–Z: (a dead network drive may slow the dialog); macOS
            and Linux detection only unit-tested; iCloud's macOS Documents
            sync is not detected (Documents keeps its path)
      - [x] 2c. Docs (`docs/dev/architecture.md` section 3 "Folder sync",
            flow (i), commands, invariants, tests, limitations;
            `docs/dev/collaboration.md` decisions; README), screenshots
            `share-sync`, `open-shared`
- [ ] Title bar and menus (decided 2026-10-16, built after 2c, before
      step 3): one slim custom title-bar row replaces the native Windows
      title bar and today's top bar: icon, File / Edit / View (Help later),
      project name + sync cloud, the Author / Science / Publisher switcher
      kept as it is, fixed in the row's centre (the dominant control), tools,
      own window buttons via Tauri's window API (Windows/Linux; drag
      region; Snap Layouts flyout via a native overlay on maximize,
      decorum rejected 2026-10-16);
      on macOS the native global menu bar and an overlay title bar with
      the native buttons instead (later, on a Mac). One command list (id, label, shortcut, action) feeds
      the in-window menus, the macOS menu, the shortcuts and a later command
      palette. File: New, Open, Open shared project, Open recent, Share /
      Sync, Save checkpoint, Export PDF, Close project (back to a start
      screen with open/new/open shared and recent projects), Exit. Edit:
      undo/redo, clipboard, find. View: the three views too (Author /
      Science / Publisher, in addition to the centre switcher), split pane, panels, Focus/
      Typewriter, zoom, Compile. Most bar toggles move into View (fixes the
      clipping below ~1080 px); the row keeps Science: Formulas, Figure,
      Sources; Publisher: zoom, Compile, Export PDF; Pages in author/science;
      Assets everywhere. The
      row no longer auto-hides (Focus hides its tools). Alt mnemonics and
      keyboard navigation, menu roles. Parts (one build + review + commit
      each):
      - [x] T1. Web: command list (`commands.ts`) driving menus and
            shortcuts; File / Edit / View menus; new row layout; no
            auto-hide. Checked in the dev mock (menus by mouse and
            keyboard, mnemonics, focus return, Find/Replace, row at 800 px
            in all views); awaiting the hand test. Known: Edit → Paste uses
            `navigator.clipboard.readText()` (WebView2 may ask for
            clipboard permission once; check in Tauri); CodeMirror's other
            search keys (F3, Ctrl+G, Ctrl+D…) are unbound (no menu items);
            Ctrl+Shift+F/T do nothing without a project now; the editor's
            top padding was sized for the overlaid bar; below ~700 px the
            right group clips; shortcuts shown as "Ctrl+…" only; the
            `publisher` screenshot's callout 2 now points at the View menu
            (docs text in T3)
      - [x] T2. Native: frameless window, own window buttons, drag region;
            Close project (Ctrl+W, Ctrl+F4) → start screen with recent
            projects (up to 10, Open recent submenu); Exit. Checked in the
            dev mock (submenu, close, start screen list, reopen, light/dark,
            buttons); awaiting the hand test: drag + double-click, edge
            resize, shadow/rounded corners, snapping (Win+Arrow, Win+Z),
            min/max/restore icon, maximized vs taskbar, close button and
            Exit save and reopen, Ctrl+W → start screen also after restart,
            Open recent of a deleted folder, upgrade from
            `last_project.json`, Alt+F4, glyphs at 100–200 % scaling,
            Linux if possible. Snap Layouts flyout on maximize (native
            overlay, `snap.rs`). Known: the maximize button shows no
            tooltip on Windows (the overlay takes the mouse); Windows
            10 shows a 1 px white border on undecorated windows; tao's top
            resize strip overlaps the row's top pixels; macOS may flash
            frameless at start and gets a draggable row without own
            buttons; Linux has no shadow and untested edge resizing; the
            recent list's "exists" flag refreshes only on open, close and
            start; window buttons aren't keyboard focusable (like native)
      - [x] T3. Docs (`docs/dev/architecture.md` flows (j) commands, menus,
            title bar and (k) start screen, recent projects, Close project;
            README), screenshots regenerated, new `menus` and
            `start-screen`
      - [x] Snap Layouts hover flyout on our Maximize button: a native
            overlay (`snap.rs`) answers `HTMAXBUTTON`; hand-tested
            2026-10-16. Details and options not taken in
            `docs/dev/snap-layouts.md`
      - [ ] macOS (built 2026-10-25, awaiting the hand test on a Mac):
            overlay title bar (`tauri.macos.conf.json`: hidden title,
            traffic lights at macOS's default place, as most apps; a custom
            position was lost after full screen, user's call 2026-10-25; the
            row's start leaves 80 px for them, none in native full screen,
            detected as the window filling its screen;
            no row icon or in-row menus there). System menu bar from the
            same menu defs and commands (`macmenu.ts`, Tauri's JS menu API,
            own capability `capabilities/macos.json`): Lethora menu (About,
            Services, Hide/Hide Others/Show All, Quit ⌘Q = our Exit, edits
            sent first), File / Edit / Format / View, Window (Minimize,
            Zoom, Full Screen), Help (macOS search field). Cut/Copy/Paste are
            the system's items; Undo/Redo/Select All stay ours. States
            refresh ~100 ms after input (no "menu opens" hook), only what
            changed. ⌘H/⌘M stay macOS's: Replace and Insert equation take
            ⌃H/⌃M; Control+Cmd keys go to the system (⌃⌘F full screen); zoom
            keys have no menu shortcut. Dock icon: `icons/icon-macos.svg` on
            Apple's 1024 grid (824 px continuous-corner square, soft shadow)
            → `icon.icns` only (`npx tauri icon`, other icons unchanged).
            Checked: build, tests, `cargo check`, row layout in the dev mock
            (Windows unchanged, simulated Mac inset). Hand test on the Mac:
            the name starts clear of the traffic lights; view switch
            centred, inset gone in full screen and back after (hand test
            found it stuck after full screen with the first build, fixed); drag and double-click zoom once, buttons in the
            row's top 28 px clickable; menu bar Lethora / File / Edit /
            Format / View / Window / Help; ticks and greying follow the app
            (views, Pages, Focus, no project), Open recent after open/close;
            each shortcut runs once (⌘S, ⌘B, ⌘1, ⌘., F5–F8, ⌘Z in editor and
            a field, ⇧⌘Z); ⌘H hides, ⌘M minimizes, ⌃⌘F full screen, ⌃M
            equation, ⌃H Replace; Cut/Copy/Paste from menu and keys in the
            editor and dialog fields, paste in suggest mode suggests; ⌘Q
            quits and keeps the last edits; About; Dock icon sized like the
            others with rounded corners, under `tauri dev` and bundled; no
            error toasts (permissions). Known: without a project ⌘Z and ⌘A
            do nothing in start-screen fields (our Undo/Select All are
            greyed there); Quit from the Dock's menu goes through macOS, not
            our Exit
- [ ] 3. Sync server (planned 2026-10-17; one build + review + commit per
      part). Loro's protocol (`loro-dev/protocol`, v1; its encrypted `%ELO`
      experimental, crates.io server/client pre-v1, server depends on `loro`,
      `%ELO` in memory only, no handshake keys, signatures, compaction,
      blobs or encrypted presence; records are one peer's counter span)
      checked and not used: our own small protocol shaped like the folder
      replica (per-device sealed streams read from an offset), borrowing
      only its batch id + Ack idea. Decided: anyone may create a project on
      a server (creator = owner; quotas and rate limits in the server
      config) until accounts; one owner per project (transfer later); folder
      sync stays "project code = Editor" (no roles, members log or
      signatures there). On the server a device's stream holds only its own
      changes (not everything since the last push, as in the replica);
      compaction snapshots still hold everything.
      - [x] 3a. Identity and members (no network, no UI; detailed
            2026-10-17): a new Loro-free crate `crates/protocol`
            (`lethora-protocol`, shared by core, server and the later
            browser client) with `keys` (Ed25519 signing + X25519 exchange
            device keys), `envelope` (v1 moved from `sync.rs` byte for byte;
            v2 for the server: signer device id in the header, Ed25519
            signature over tag + envelope + context (project, document,
            kind, name) so the server verifies without the key; keyring
            epoch → key), `members` (signed append-only log, `[len][crc]`
            framing, JSON body bytes signed raw, `seq` + `prev` hash chain;
            ops `genesis`, `invite` (role invites by the owner, device
            invites by any device of a user for that user), `revoke`,
            `join` (signed by the invite key and the new device), `role`,
            `remove` (owner; a user its own devices), `profile` (sealed,
            opaque); incremental verifier with roles, devices, invites,
            head, `may_write(device, document)`; removed users may rejoin
            through a new invite) and `invite` (link
            `https://<host>/join/<invite-id>#<secret>`, HKDF → invite
            signing key + wrap key; the wrapped project key lives on the
            server by invite id and is deleted on expiry, revoke or last use
            (3b), never in the log). App: `device.json` gains the secret
            keys (generated if missing). Unit tests. Done (nothing to
            hand-test). Also: a join needs the invite's creator to still be
            a current device (a lost device's invites die with it);
            removing a non-owner's last device removes the user too (so a
            new role invite brings it back).
            Known: a join's expiry check uses the writer's claimed time
            (3b's server must check its own clock); wrap, keyring and
            exported secret arrays aren't zeroized (only the dalek keys
            are); the wasm32 build is unverified (target not installed;
            getrandom will need `wasm_js`); keys sit in plain text in
            `device.json` (OS keystore later)
      - [x] 3b. Protocol messages and server (detailed 2026-10-17):
            `crates/protocol` gains `wire` (own binary encoding, versioned,
            length-prefixed; no new dependency); `crates/server`
            (`lethora-server`: tokio, axum, tracing; depends only on
            `lethora-protocol`). Store interface with a filesystem backend:
            per project `members.log`, `<doc>/<device>.log|snapshot`,
            `assets/<blob>`, `invites/<id>.key` (wrapped project key,
            deleted on expiry, revoke or last use), `meta.json` (snapshot
            generations, last batch id per stream, bytes used); projects
            loaded on demand, writes serialized per project. WebSocket:
            server challenge → hello as device (current member), as invitee
            (invite key; may only read the members log and wrapped key and
            append its `join`, then reconnects) or create (genesis record,
            per-IP rate); signed over challenge, protocol version and public
            URL; welcome with an HTTP token and the own streams' positions.
            Subscribe with members length + per document/device (generation,
            offset) → missing members records, log bytes in ≤ 1 MiB chunks,
            "snapshot changed" notes, then live. Append (document, batch id,
            records): `may_write` + every record an envelope v2 by this
            device for its stream → append, broadcast, ack with offset;
            duplicate batch ids not stored twice. Members appends checked by
            `Members::apply` + the server clock for joins (stale head
            refused). Presence forwarded, never stored, rate-limited.
            Compaction: snapshot PUT names the own log length it covers →
            atomic write, generation + 1, the log keeps only later records,
            "compacted" broadcast (clients past it shift their offset, no
            download). HTTP with the bearer token: snapshots, assets
            (immutable, v2-signed, PUT needs `main` write), wrapped invite
            keys (PUT by the creator), owner-signed project deletion (local
            copies stay), `/healthz`. Limits in the config: 4 MiB messages,
            100 MiB assets, 2 GiB per project, 10 new projects per IP and
            day, 20 connections per IP, per-connection token bucket.
            `lethora-server.toml` + `LETHORA_*` env (listen address, data
            dir, public URL, limits). TLS from a reverse proxy only
            (built-in later). In-process integration tests (real server on
            `127.0.0.1:0`, test client with tokio-tungstenite as a dev
            dependency). Done (nothing to hand-test until 3c). Changes
            from the plan: WebSocket at `/v1/ws`; members appends carry a
            batch id; presence goes out as `PeerPresence { device }`; codes
            `Forbidden` and `Stale` added; batch ids ≤ the stream's last
            count as resends (acked, not stored); over-rate messages are
            delayed, over-rate presence dropped; any `main` writer may
            upload a blob another device sealed; clients must `Ping` (idle
            120 s); `meta.json` also keeps stream lengths so load finishes
            a crashed compaction; a damaged members tail is kept as
            `members.log.damaged` and cut. Known: a malicious server can
            still withhold, delay or reorder records and members entries
            (e.g. hide a removal from some clients), replay older records,
            delete data and see metadata (ids, roles, sizes, times, IPs,
            who is online); hellos, HTTP requests and invitee connections
            aren't rate-limited (only connections per IP); per-IP counters
            and tokens live in memory (reset on restart; reconnect for a new
            token); one lock per project and one global registry lock
            (projects load one at a time after a restart); logs read whole
            on load and compaction (relies on clients compacting); two
            fsyncs per append; snapshots and assets held in memory and
            capped by the asset limit (100 MiB, book-size snapshots not
            measured); quota per project only (no per-owner or global
            limit); a crash between append and `meta.json` can store a
            resent batch twice, a lost `meta.json` resets generations; no
            server WebSocket pings; expired invite keys swept only in loaded
            projects; no version negotiation; `X-Forwarded-For` untested;
            the members log is never compacted
      - [x] 3c1. Sync engine, share, invite, join (detailed 2026-10-17):
            sans-IO `ServerSync` in `crates/core/src/server.rs` (wire
            messages and HTTP results in, actions out; no tokio): own stream
            = own peers' changes only (export from the pushed version with
            other devices' peers at their current counters; own peers from
            the `peers` map); one batch in flight, pushed on ack, resend
            resolved by the welcome's last batch id; compaction = full
            snapshot PUT with `covers`; incoming records verified (v2, known
            member device) and queued, imported only by the web view's FIFO
            pull (never on the undo stack); members log kept in
            `server/members.log`, verified, a non-continuing server log is
            an error; positions, pushed version and own stream position in
            `state/<device>.json` (written by autosave after the data they
            cover); key in `<config>/keys/<project>.key`. Share = genesis +
            snapshot (generation 1) + all blobs; join = invitee hello →
            members log → wrapped key → `join` → device hello → local
            project from the catch-up; a device invite joins as the
            inviter's user, stored per project (app-wide user id
            unchanged). One sync mode at a time (server or folder). App: an
            async task per open project (`tokio-tungstenite` + `reqwest`,
            OS native-tls, no HTTP/2): hello, subscribe, `Ping` every 30 s,
            reconnect with backoff; push ~200 ms after an edit;
            `remote-changes` event → `sync.ts` queues a pull at once; cloud
            states online / connecting / offline / removed / error. Plain
            http/ws allowed for loopback and private LAN addresses only
            (warning shown). UI: Share / Sync offers "Lethora server" first
            (URL remembered), name asked once (colour from the user id;
            sealed profile), Invite people (role, expiry 1/7/30 days, uses
            1/5/25, Copy), Add my other device, "Open shared project…" takes
            a code or a link; dev mock. Tests: sans-IO units; two
            `Document`s through an in-process `lethora-server` (dev
            dependency): share, invite, join, concurrent edits converge,
            compaction, a blob, reconnect after a lost ack. Done, checked in
            the dev mock (share with the LAN warning, invite link, remote
            edit, offline cloud, join by link); hand-tested on two PCs
            2026-10-18 (user and device joins, edits both ways with figures
            and equations, rejoin of a member device). Hand test: server PC `cargo build --release -p lethora-server`,
            `lethora-server.toml` with `listen = "0.0.0.0:8080"`,
            `data_dir = "lethora-data"`, `public_url = "http://<LAN IP>:8080"`,
            run it, open the port, `/healthz` says ok; PC A: Share / Sync →
            server address → name → Share → Online; Invite people → link;
            PC B: Open shared project… → link → name → Join; type on both
            (≤ ~1 s), an image arrives; stop and restart the server
            (offline → reconnects, catches up); Add my other device; close
            and reopen. Changes from the plan: invite links carry the project
            id (`/join/<project>/<invite>#<secret>`: the invitee's hello
            needs it and the server has no invite → project lookup);
            `server_origin` normalizes server URLs (also `ws`/`wss`; LAN
            http allowed for `public_url` too); a joined project's folder
            is named after its title; the 2 s poll stays on in server mode
            (refreshes the status; `remote-changes` pulls at once); blob
            presence checked with `HEAD`; share/invite/join commands wait on
            the sync's state (120/30/300 s). Known: a share failing partway
            leaves an orphan project on the server (a retry makes a new
            one); a device whose local copy was deleted rejoins with any
            open invite of the project (fixed 2026-10-18 after the hand
            test: it stays the member it was, reads its own stream back and
            uploads nothing again), but there is no "reopen from server"
            without an invite yet; without
            `keys/<project>.key` a shared project shows needsCode with no
            way to enter a key; a crash before an ack is saved resends
            landed batches as duplicates (harmless); a server stream that
            shrank makes the client push everything again; truncation of the
            server's members log to a valid prefix isn't detected; records
            failing verification are skipped and only logged; Readers and
            Reviewers have no read-only UI yet (client role check + server
            Nack, 3c2); blobs over the server limit stay local with a status
            message; one batch at a time, a change over 3 MiB uploads a full
            snapshot instead; share, push and pull run under the document
            lock, and the push runs on a tokio worker (large snapshots can
            stall edits briefly); names and colours stored but not shown
            yet (3e); `servers.json` keeps 5 servers; `pull_local` skipped
            in server mode
      - [x] 3c2. Members and roles (detailed 2026-10-18). Core
            (`ServerSync`): members ops role change, remove user, remove
            device (leave = remove the own device), revoke invite (signed,
            appended, acked like the invite op); queries: members (name and
            colour from profiles, role, devices, removed) and open invites
            (creator, role, expiry, uses left); owner deletes the project
            (HTTP `DELETE` with the protocol's signed `deletion_body`; a
            `Delete` method beside Get/Head/Put). Read-only: a writable flag
            on `Document` from the role (Owner/Editor write; Reviewer and
            Reader don't until step 4), every mutating core entry refuses
            with a clear "read only" error; UI: editors `readOnly`, a "Read
            only · <role>" badge in the title bar, write controls disabled
            (layout panel, figure/asset import, sources import, exceptions,
            named checkpoints, restore); a role change applies live.
            Members screen: a "Members…" button in the shared-state Share /
            Sync dialog (and File → Members…): list, roles; owner: change
            role, remove, revoke invites, delete from server (confirm by
            typing the project title; local copies stay); anyone: remove own
            devices, leave. After leaving, being removed or the project
            being deleted, the local copy stays a normal local project
            (server settings cleared, key forgotten; can be shared again as
            a new project). Reopen from server: at share/join the app
            remembers server, project id, title and user per project in its
            config dir (key already in `keys/`); "Open shared project…"
            lists remembered projects without a local copy here and
            rebuilds one like a join without an invite (device hello,
            catch-up incl. the own stream). Tests: core units for every op
            and refusal; end-to-end (role → Reader: edits refused, back;
            remove member; revoke; leave; reopen from server; owner
            deletes); dev mock; screenshots (moved to 3g). Done, checked
            in the dev mock (members screen, role change, delete with the
            title, leave, live Reader badge and disabled controls, reopen
            from server); awaits the two-PC hand test: as in 3c1 (LAN
            server, share on PC A, join on PC B as Editor); A: File →
            Members… → B listed with name and colour; set B to Reader → B
            shows "Read only · Reader" within ~1 s, typing, Figure,
            Formulas, layout panel, checkpoint form and Restore blocked, A's
            edits still arrive; back to Editor → B types again; A: invite
            link → Revoke → B-side join with that link refused; B: Leave
            project… → B's copy is local (cloud off, editable), A sees B
            removed; invite B again, join; on B delete the project folder,
            Open shared project… → "From your servers" → Open rebuilds it;
            A: Remove B → B turns local; A: Delete from server… (type the
            title) → A and B both local, the server's project folder gone.
            Changes from the plan: one `MembersOp::Change { id, op }` for
            role/remove/revoke, checked locally against a clone of the
            members state (refusals carry the protocol's reason,
            `Error::NotAllowed`), results via `change_result(id)`; turning
            local is automatic when the sync stops as `removed` (left,
            removed, deleted) with a `server-left` message; the server's
            `UnknownProject` now also means removed; `merge_bibliography_mirror`
            and `adopt_loose_assets` do nothing (no error) while read only;
            auto checkpoints, autosave and imports go on; remembered
            projects are written whenever a server project opens (covers
            projects shared before 3c2); leave removes only this device (a
            user with other devices stays a member); no server changes.
            Known: a change sent offline times out in the UI after 30 s but
            stays queued and lands on reconnect; local checks use this
            device's copy of the members log (a just-joined member can be
            changed once it synced here); a server that lost the project or
            a wrong server looks like a deletion (the copy turns local, the
            key is forgotten; the content stays); deleting needs the device
            clock within 5 min of the server's; the publisher page menu and
            edit popover, the assets context menu, rename, move and drag
            aren't disabled (the core refuses them with an error); edits
            typed just before a role change arrives are refused and the
            editor resyncs; a demoted member's unpushed edits stay local
            until it is an editor again; outside `references.bib` edits
            aren't merged while read only; a project that turned local keeps
            its project user as identity until reopened; reopen lists only
            remembered projects whose key is still here; the dev mock's
            invites are always "by" the own user
      - [ ] 3d. Removing a member starts a new epoch: new key wrapped to the
            remaining devices' X25519 keys in the members log; older epochs
            stay readable from the keyring. Detailed 2026-10-22. Protocol:
            a members op `epoch` (`epoch` = previous + 1, starting at 2;
            `ephemeral` X25519 public key; `keys`: per current device and
            per open invite the new 32-byte key, sealed with
            XChaCha20-Poly1305 under HKDF-SHA256 of the X25519 shared secret,
            associated data binding project, epoch and recipient). Any
            current device may write it; the verifier requires every current
            device as a recipient, no removed or unknown device, invites only
            if open. Invites gain an X25519 exchange key (HKDF from the link
            secret, info `lethora-invite-1 exchange`), required in the
            `invite` op (no pre-3d migration: the app isn't live; shared
            test projects are shared again). The wrapped invite blob holds
            the whole keyring at invite time; later epochs reach the joiner
            through the log entries wrapped for its invite, then for its
            device. Trigger: any current device that sees a removal (user or
            device) newer than the latest epoch writes the next epoch (the
            remover at once; after a leave, the first remaining device
            online); a stale head means another device was faster: catch
            up and check again. Sealing always uses the newest epoch
            (records, snapshots, blobs, profiles); older epochs still open.
            Keys: `keys/<project>.key` holds `epoch:hex` lines (a bare
            64-hex line is epoch 1, so folder-sync key files stay as they
            are); keys found in the log are added on load and when they
            arrive. Folder sync and blob names (keyed by the in-document
            asset key) unchanged. Server: no code beyond the shared verifier
            and a larger invite blob limit. No UI (logged only). Tests:
            protocol units (wrap/unwrap, verifier rules); end-to-end with
            three devices (remove → epoch 2 by the remover, new records
            epoch 2, the removed device's keyring can't open them; leave →
            a remaining device rotates; join through an invite made before
            the rotation reads all epochs; concurrent rotations → one
            epoch). Doc afterwards: `docs/dev/key-epochs.md`, an informal
            graphic of the flow (server, clients, what travels) with a
            short explanation.
            Built 2026-10-22 (tests, clippy, fmt pass; no UI, so no dev mock
            check); doc `docs/dev/key-epochs.md` with two graphics
            (`docs/images/key-epochs-*.svg`). Changes from the plan: the
            op's sealed keys are two maps, `devices` and `invites`; low-order
            X25519 keys are refused at genesis, invite and join (else a
            member could make every rotation fail); `ServerSync::key()` is
            now `keyring()` and open/reopen take a `Keyring` (small call-site
            edits in `app/src-tauri/src/lib.rs`); core has no logger, so the
            sync collects notes (`take_notes()`) that the app prints; the
            invite blob is format 2 (`[u32 epoch][key]` per epoch), the
            server accepts up to 16 KiB. Hand test (with 3c2's two-PC
            setup; projects shared before 3d must be shared again): A
            shares, invites B (keep a second, unused invite link), B joins;
            A removes B → A's log/console shows epoch 2 written, A keeps
            typing, B turns local and gets nothing new; join on B (or a
            third device) with the kept link → it reads old and new text;
            that member leaves on its own → the remaining device writes the
            next epoch; close and reopen A → it still reads everything.
            Known: until a current device is online after a removal, new
            content keeps the old key; a rotation refused for another reason
            than Stale is only logged (retried on the next members change or
            reconnect); every device seeing a removal at once tries, the
            losers spend one Stale round trip; whoever fetched an open
            invite's blob keeps those keys and that invite's epoch copies,
            revoking doesn't rotate; profiles sealed in older epochs aren't
            re-sealed; a current member can write an epoch whose copies
            don't open for others; an epoch entry grows ~180 bytes per
            device or open invite
      - [x] 3e. Presence: profile (display name, colour); Loro
            `EphemeralStore` encrypted with the project key, forwarded by the
            server; remote cursors and selections in the author and science
            editors. Detailed 2026-10-22. Each device publishes its state
            (view: author/science/publisher, cursor and selection as Loro
            cursors so they survive concurrent edits) in an
            `EphemeralStore`; its updates are sealed as envelopes v2
            (newest epoch, kind `presence`) and sent as `Presence`, the
            server forwards them as `PeerPresence` (never stored, already
            rate-limited); clients verify (current member device), open,
            apply; entries time out (Loro's timeout, ~30 s, refreshed while
            online); a removed or offline device's entries go. Sent
            throttled (≤ ~10/s, on cursor move, edit, view switch, and a
            keep-alive). UI: remote carets and selections in the author and
            science editors in the member's profile colour, a name flag
            shown on hover and briefly after the caret moves; the title bar
            gets a row of small coloured initials of who's online (tooltip:
            name, role, view; own device not shown; other devices of the
            same user shown once). Publisher view shows nothing. Read-only
            members publish presence too. Folder sync: none. Dev mock: a
            fake remote member with a moving caret (`?presence=1`).
            Tests: sealing/opening, timeouts, mapping Loro cursors to
            editor offsets, two `Document`s through the in-process server
            see each other's cursor.
            Built 2026-10-22, checked in the dev mock (`?presence=1`: Bea's
            moving caret and flag in the author and science views, light and
            dark; online row "B", "C" with name · role · view tooltips).
            Changes from the plan: an incoming entry is decoded in a scratch
            store and only the sender's own entry is copied, stamped with
            the local arrival time (timeouts don't depend on device clocks,
            a member can't write another device's entry); selections travel
            as the existing text anchors (`anchor_at`/`resolve_anchor`); a
            device answers a newcomer within 100 ms instead of at the next
            keep-alive (10 s); the name flag is a CSS animation restarted per
            move; no server changes. Added after review: the online row
            shows five users, then a "+N" (tooltip lists the rest; dev
            mock `?presence=N`). Hand test (two PCs, as 3c2): both open
            the shared project → each sees the other's initials in the title
            bar (tooltip name · role · view); click/select in the author view
            on A → B sees A's coloured caret/selection with a name flag
            (≈ ≤ 0.2 s); same in the science view; A switches to Publisher →
            A's caret disappears on B, the tooltip says Publisher view; type
            on both near each other → carets stay on the right characters;
            close A → A disappears on B within ~30 s; a Reader shows too.
            Known: a disconnected device stays visible up to 30 s (nothing
            sent on leave), timed-out entries go up to 2 s late; others'
            presence is dropped on this device's own disconnect and returns
            at their next keep-alive (≤ 10 s); a caret in text not yet pulled
            shows after the pull; a remote insert exactly at a caret pushes
            it right; a reply racing an in-flight edit can be off until the
            next refresh; this user's other devices show like members; the
            flag replays when a line scrolls back in; the publisher view
            keeps the online row; the dev mock's caret isn't mapped over
            local edits; while typing, a remote caret can lag one
            character behind and stay there when the typist stops, until
            their next edit (seen in the hand test 2026-10-23).
      - [x] 3f. "Who wrote this" UI. Detailed 2026-10-22: View → Show
            authors (off by default; only in shared projects, hidden in
            local-only ones): text in the author and science editors gets a
            faint background in each author's profile colour (character →
            Loro peer → `peers` map → device → user → profile), a small
            legend of authors (with "you"); hovering text shows name and
            when (Loro change timestamp; coarse, known). Computed by the
            core per text range (runs of the same user), only for the
            visible part, updated after edits and pulls without blocking
            typing. Unknown peers (no `peers` entry) show as "unknown".
            Publisher view: nothing. Tests: core runs for text written by
            two users (merged, interleaved, deleted, undone); UI decoration
            mapping; dev mock with two authors.
            Built 2026-10-22, checked in the dev mock (`?presence=1`, View →
            Show authors: You/Bea/Carl/Unknown backgrounds, legend bottom
            right, hover "Name · date", author and science views, light and
            dark). Changes from the plan: runs split by user and change
            time (accurate tooltip times); unknown authors grey, an
            unreadable profile shows "Someone"; the tooltip is the native
            `title`; the menu item is disabled (not hidden) outside
            server-shared projects; per-character lookup was fast enough
            (5.2 ms per 20k UTF-16 units of the book fixture, release).
            Known: text brought back by undo or a checkpoint restore counts
            as written by whoever undid/restored it (Loro re-inserts it);
            times are coarse (a change's start; Loro merges a peer's changes
            within ~1000 s); fresh typing is uncoloured for ~300 ms; the
            legend sits under open drawers; new names show on the next
            edit, scroll or pull.
      - Combined hand test of step 3 (3c2–3f, 2026-10-22; share projects
        made before 3d again). Setup as 3c1: LAN server, PC A shares, PC B
        joins as Editor (keep a second, unused invite link from A).
        1. Presence (3e): each PC shows the other's initials in the title
           bar (tooltip name · role · view); carets and selections appear on
           the other PC in the author and science views with a name flag;
           Publisher hides the caret; typing near each other keeps carets
           on the right characters.
        2. Authors (3f): both type a sentence; View → Show authors on both →
           each sentence has its author's colour, the legend says You and
           the other name, hover shows name and time; in a local project
           the menu item is disabled.
        3. Roles (3c2): A sets B to Reader → B shows "Read only · Reader",
           its presence stays; back to Editor.
        4. Key epochs (3d): A removes B → A's `tauri dev` terminal shows
           epoch 2 written; B turns local and gets nothing new; A keeps
           typing. Join on B with the kept link → B reads old and new text;
           B leaves on its own → A writes the next epoch; close and reopen A
           → everything still reads.
        5. The rest of 3c2's list: revoke an invite, reopen from server,
           delete from server.
      - [x] 3f+. Fixes from the combined hand test (planned 2026-10-22).
            Found: after ~10 min online, creating an invite or an
            add-device link fails with "HTTP 401: a valid bearer token is
            needed" (the invite still lands in the members log and shows as
            open, but its link is never shown); reopening helps. Cause: the
            welcome's HTTP token expires after `token_seconds` (600) and
            the app only gets a new one on reconnect, so snapshot and asset
            uploads fail the same way. Removed members stay listed forever.
            Profile colours are hashed from the user id, so two members
            often get the same or neighbouring colours.
            Decided: (1) a token is valid while its WebSocket connection is
            open and is revoked when it closes (no fixed expiry; a removed
            device loses HTTP access at once). (2) Removed members fold
            under a collapsed "Removed (N)" at the bottom of the members
            list (profiles stay; Show authors still names their text).
            (3) A new profile takes the first `PALETTE` colour no other
            current member uses (the user-id hash when all are taken); each
            member can change their own colour per project from the 8
            swatches on their row (a new profile op; nobody sets others').
            Also noted as expected: rejoining after a removal makes a new
            local copy ("Title 2"), since the old copy stays local;
            invites made before a removal still work after it.
            Built 2026-10-22 (Opus), reviewed, checked in the dev mock
            (`?sync=server`: fold with a removed "Dana", swatches on the
            own row, pick → dot updates). Also: joining auto-picks too (a
            joiner's first profile is sealed in the join); a device joining
            its user keeps the user's colour; readers may change theirs
            (the members log allows profile ops from any current device).
            Hand test (two PCs, as 3c2): stay online > 10 min, then create
            an invite and an add-device link → both show the link; B joins
            → B's dot differs from A's; B picks a colour → A's list,
            carets and Show authors show it; A removes B → B under
            "Removed (1)", collapsed.
            Known: a request in flight across a reconnect fails at once
            with 401 (an invite made then shows an error, its key PUT is
            not retried); two first profiles sealed at once can share a
            colour; picking a colour someone has is allowed.
      - [x] 3g. Docs (`docs/dev/architecture.md`, `docs/dev/collaboration.md`,
            README), deployment (Dockerfile, systemd example, config
            reference), screenshots. Detailed 2026-10-23: new
            `docs/dev/server.md` (run it, config reference of every key and
            `LETHORA_*` variable with defaults, reverse proxy, backups,
            updating, limits of the current server); new `deploy/`
            (Dockerfile + `Dockerfile.dockerignore`, docker-compose with
            Caddy, Caddyfile, nginx snippet, systemd unit, commented
            `lethora-server.example.toml`, checked by a config test so it
            can't drift); README "Run a sync server" section; architecture
            sections for `crates/protocol`, `crates/server`, the core's
            `ServerSync` and the app's `serversync.rs`; collaboration.md
            brought in line with what is built (TLS only via a proxy,
            filesystem storage only, presence, who wrote this). Screenshots
            (dev mock, `scripts/shots.mjs`): members screen, invite link,
            presence (caret + online row), Show authors.
            Built 2026-10-23 (Opus), reviewed: `docs/dev/server.md`, `deploy/`
            (image 84 MB on debian:trixie-slim; build, run, `/healthz` and
            the compose stack through Caddy with HTTPS and WebSocket checked
            locally), config test `example_config`, the four screenshots.
            Docs fixed where they disagreed with the code (invite link
            form, who writes the members log, TLS/storage backends).
            Known: the systemd unit is untested on Linux; restoring an
            older backup is untested; the server address in the app must
            equal `public_url`.
- [x] 4. Review tools: comments, marks, suggestions (planned 2026-10-24;
      one build + review + commit per part). Decisions: a second Loro
      document `review` (design section 4) holds all review items, anchored
      in `main`'s content text with Loro cursors (start and end, like the
      exceptions); write permission per document: `main` Owner/Editor,
      `review` Owner/Editor/Reviewer, Reader nothing (the server already
      enforces this with `may_write`). Suggestions through a suggest mode
      (Word-like track changes): a Reviewer's typing and deleting in the
      author and science views becomes suggestions (insertions underlined,
      deletions struck through), Editors can switch it on too; accepting is
      a normal `main` edit by an Owner/Editor (one undo step), rejecting
      resolves it in `review`. Marks are a highlighter (a few fixed colours,
      no text; adding a note makes it a comment). Readers see all review
      items read only. A comment's author comes from Loro attribution
      (peer → device → user), not a stored field; only the author edits or
      deletes their comment, Owner and Editors may also delete any, anyone
      with `review` write resolves and reopens (client rule: inside one
      document the server can't tell). Checkpoints and restores don't touch
      `review`; text deleted under an item leaves it "detached" (shown in
      the panel, not lost). The publisher view shows no review items in
      step 4.
      - [x] 4a. Core, local: `review` document (`review/<device>.snapshot`
            and `.log`, own undo, compaction, loaded with the project);
            per-document writability replacing the single read-only flag;
            API for comment threads (add, reply, edit, delete, resolve,
            reopen), marks (add, recolour, remove), suggestions (from a
            replace/insert/delete in suggest mode; accept → `main` edit,
            reject) and listing items with resolved anchors and authors.
            Tests: every entry against every role. Built 2026-10-24 (Opus),
            reviewed: `crates/core/src/review.rs` (format, anchors, authors,
            permissions, suggest-mode rules in its module docs),
            `crates/core/tests/review.rs` (13 tests incl. the role matrix
            over None/Owner/Editor/Reviewer/Reader, own and others' items).
            `main` and `review` share one `Files` (load, save, compaction);
            `pull_local` reads `review/` too (`Pulled::review`). Items are
            one map each (kind from content), never deleted (`removed`
            flag, so undo brings back the same item and author); author =
            the peer in the item's or message's container id; anchors
            `from` = first character (`Side::Middle`), `to` = last character
            (`Side::Right`). Accept writes `accepted` as bookkeeping (not
            undoable); a stale or detached suggestion can't be accepted.
            Nothing to hand-test until 4c. Known: undoing a deletion brings
            text back as new characters, so a range loses its restored
            first/last character and a fully deleted one stays detached (as
            for exception anchors); undoing an accepted change doesn't
            reopen the suggestion; typing inside an own suggestion's range
            appends at the end of its insert; `review` isn't synced yet (4b)
      - [x] 4b. Sync and roles: the `review` stream in server sync (core
            `DOC` becomes per document: positions, acks, compaction,
            snapshots) and in the folder-sync replica (`review/`); a
            role-matrix suite, core and end-to-end with the real server:
            every role × `main`/`review`, a Reviewer's forged `main` update
            refused by the server (Nack) and ignored by other clients, live
            role changes both ways (Reviewer ↔ Editor ↔ Reader), Reader
            sees review items arrive. Identity grounding (found while
            briefing 4a): `peers` entries are untrusted today, so a device
            could register its peer under another user and pose as that
            author in comments; trust a `peers` entry only when it arrived
            in the stream of the device it names (server and folder sync,
            both documents). Built 2026-10-24 (Opus), reviewed:
            per-document own streams in `ServerSync` (pushed version,
            position, batch in flight, compaction each; one batch-id counter
            so a `Nack` holds up only its stream); a device pushes a
            document only while its role writes it (`Role::writes`),
            unpushed changes stay until allowed again; incoming data of
            document X from device D is imported only if D may write X now,
            or could and this device hasn't caught up since D lost the right
            (`revoked` streams, persisted), so joiners still get a demoted
            editor's earlier records. Trust (`crates/core/src/trust.rs`,
            server-shared projects only): an op counts as D's only if D's
            own signed stream brought it (per-peer origin and counter range,
            from Loro's import-blob metadata) and the `peers` entry and the
            members log agree; else the author is unknown (Show authors and
            review authors, so forged authors grant no author-only rights);
            a device whose current peer gets forged ops switches to a fresh
            peer. Design change (accepted in review): compaction snapshots
            hold only the device's own changes, so every change arrives only
            in its own stream (the sharer's first snapshot holds everything
            then). Folder-sync replica: `review/<device>.*`, envelope name
            `review/<device>` (files can't be swapped between documents);
            no roles there. State: `review`, `origins`, `revoked` (all
            optional; old state files load). Tests: 5 new end-to-end
            (every role × both documents incl. crafted signed appends, role
            changes with unpushed changes, readers/joiners/rejoiners,
            forged authors, review records in a new key epoch), 3 folder
            sync. Nothing to hand-test until 4c. Known: unknown authors (never
            someone else) for other devices' changes from before sharing,
            for other users' text in projects shared before 4b (rejoining
            recomputes), after a crash between an import and the next state
            write, and for a peer whose `peers` entry another device
            overwrote; the origins in the state file grow by one entry per
            session peer; a demotion seen live is enforced on incoming
            records from the next catch-up on (the server at once);
            plain-folder co-devices' ops in a server project no longer
            reach the server through compaction either
      - [ ] 4c. App, in two parts (split 2026-10-24, one build + review +
            commit each):
            - [x] 4c1. Review panel on the right (View → Review), cards
                  aligned with their text, threads, resolve, filters
                  (open/resolved); comments and marks with highlights in
                  the author and science views; role-aware controls (title
                  bar "Reviewer" badge, `main` read only but review actions
                  on, all writes off for Readers); dev mock. Built
                  2026-10-24 (Opus), reviewed, checked in the dev mock:
                  Tauri `review_*` commands (`review_list` gives `me`, the
                  items, undo/redo flags; writes push without a recompile);
                  `app/src/review.ts` (pure: rights, filter, history
                  routing, `reviewField` decorations, `stackCards`) and
                  `reviewpanel.ts` (cards placed with `requestMeasure`, the
                  active card at its line, others pushed apart; inactive
                  cards show the first message, the active one the whole
                  thread); `Sync.call` queues review requests behind
                  pending edits so offsets map exactly. Two rights states
                  in main.ts (`readOnly` for `main`, `reviewReadOnly`) with
                  a second WriteGuard. Edit → New comment (Ctrl+Alt+M, also
                  by physical key for AltGr layouts) and Highlight ▸, also
                  on right-click; comments need a selection; highlights
                  show only while the panel shows (View → Review switches
                  review markup as a whole; adding opens the panel).
                  Undo: Ctrl+Z in the panel, and for a Reviewer everywhere,
                  is review undo. Names from `server_members` ("You",
                  member, "Someone" for a non-member, "Unknown"). Review
                  fixes: names were "Someone" until the first edit (listed
                  before the sync status arrived); after posting a comment,
                  and after undoing the focused card, focus fell to the
                  page so Ctrl+Z did nothing (now the new card, else the
                  panel, keeps it). Dev mock: `?sync=server-reviewer`,
                  `__mockRole("reviewer")`, `__mockRemoteComment()`.
                  Hand-test (with 4c2, against a server): Reviewer and
                  Reader on a second PC; comment, reply, edit, delete,
                  resolve, highlight and recolour arrive both ways; a
                  Reviewer can't type in the text; Reader controls greyed;
                  role changes live; card placement while scrolling,
                  typing and resizing; Ctrl+Z in the panel vs the text.
                  Known: see known-issues "Review tools" (4c1).
            - [x] 4c2. Suggest mode toggle and rendering (insertions
                  underlined, deletions struck through), Accept/Reject on
                  cards and per suggestion, Accept disabled for Reviewers;
                  dev mock. Built 2026-10-24 (Opus), reviewed, checked in
                  the dev mock: Edit → Suggest changes (Ctrl+Shift+X;
                  Ctrl+Shift+E is Markers), always on for a Reviewer (the
                  editor takes input again, the "Reviewer" badge says it;
                  "Suggesting" badge for Owners/Editors only, the bar has
                  no room for both). A transaction filter (`suggestMode`
                  in review.ts) drops every user change and sends
                  `review_suggest` calls in order through `Sync.call`
                  (multi-range edits last to first); a TS port of the
                  core's three merge rules shows keystrokes and places the
                  caret at once (after typing it stays, Backspace → the
                  deletion's start, Delete → its end), the list refreshes
                  as soon as all calls are answered. A caret move, paste,
                  cut or switching the mode on starts a new review undo
                  step (`split` → `end_undo_step`). Rendering: insert
                  widget underlined and deletion struck in the author's
                  colour, open suggestions only; accepted/rejected tagged
                  on cards. `review_accept` replies like undo (applied via
                  `sync.apply`, new compile version). Reviewers' format
                  buttons, headings, tables, Replace and equations make
                  suggestions; style, font and size stay off (layout).
                  Review fixes: the editor's right-click menu put the caret
                  at 0 on a mouse click (PageMenu now keeps the page
                  selection on mousedown; pre-existing); the two badges
                  squeezed the project name to nothing. Hand-test (with
                  4c1, against a server): a Reviewer types, deletes and
                  pastes suggestions that the Owner/Editor sees live, and
                  accepts (text changes on both PCs, publisher view
                  recompiles, Ctrl+Z with the mode off undoes it) or
                  rejects; an Editor toggles the mode; stale suggestions
                  can't be accepted; fast typing and Backspace in suggest
                  mode; the right-click menu keeps the caret. Known: see
                  known-issues "Review tools" (4c2).
      - [x] 4d. Docs (architecture, collaboration, README), screenshots,
            hand-test list: two PCs against a server (Owner, Reviewer,
            Reader; later Docker on Linux and macOS). Built 2026-10-24
            (Opus), reviewed: collaboration.md section 10 "Review tools"
            (panel, rights table, suggest mode, Accept/Reject, detached,
            undo; old sections 10/11 now 11/12), architecture.md section
            3 "Review" and section 4 (l) (app side, commands, `Sync.call`,
            suggest flow; read only rewritten as per-document rights),
            README feature bullet; shots `review-panel`, `suggestions`,
            `review-menu`.

            **Hand test, all of step 4 (passed on two Windows PCs 2026-10-25;
            the cross-platform test on Linux, Windows and macOS is left).** Setup: one server,
            Owner on PC 1 shares with PC 2 as Reviewer, later as Reader;
            again later against Docker on Linux and macOS.

            Owner (PC 1) and Reviewer (PC 2):
            - Reviewer: "Reviewer" badge, suggest mode on (no "Suggesting"
              badge) → typing, deleting, pasting make suggestions; the text
              changes on neither PC.
            - Reviewer: comment, reply, edit/delete own message, resolve,
              reopen, highlight, recolour → arrive live on PC 1; Owner
              actions arrive on PC 2.
            - Reviewer: fast typing and Backspace in suggest mode → caret
              stays, typed text underlined, Backspace first takes back
              typed text, then widens the struck range; one suggestion per
              run.
            - Reviewer: the Owner's messages and highlights → no edit,
              delete or recolour offered.
            - Reviewer: Accept (card, right-click) → disabled with the
              reason; Reject only on own suggestions.
            - Owner: Accept → the text changes on both PCs, the publisher
              view recompiles, Ctrl+Z in the text (suggest mode off) undoes
              it.
            - Owner: Reject → text unchanged, card "Rejected" under
              Resolved/All on both PCs.
            - Owner: change or delete the text under an open suggestion →
              "The text under it changed since." / "Its text was
              deleted."; Accept disabled, Reject works.
            - Owner: Edit → Suggest changes (Ctrl+Shift+X) → "Suggesting"
              badge, edits become suggestions; off → normal edits.
            - Either: delete all of a comment's text → the card stays,
              "Its text was deleted.", where the text was.
            - Ctrl+Z in the panel, in suggest mode and anywhere as Reviewer
              undoes review steps; otherwise the text.
            - Right-click menu in the text keeps the caret; New comment and
              Highlight act on the selection.
            - Cards stay beside their lines while scrolling, typing,
              resizing; the active card at its line.
            - Make or restore a checkpoint → review items untouched.
            - Publisher view → no review items, no panel.

            Owner (PC 1) and Reader (PC 2):
            - Reader: "Read only · Reader" → text and review visible, all
              review controls greyed, typing does nothing, no suggest mode.
            - Owner adds/changes comments and suggestions → arrive live on
              PC 2.

            Live role changes (PC 2 open):
            - Reviewer → Editor: badge goes, text editable (Suggest changes
              starts off). Editor → Reviewer: badge, suggest mode forced on.
            - Reviewer → Reader: review controls grey out, a half-written
              comment is dropped. Reader → Reviewer: controls come back.
            - Changes made just before a demotion stay unpushed (no data
              loss) and aren't applied by others.

            Sync and older projects:
            - A project shared before 4b → other users' authors may show
              "Unknown" (never someone else) until they rejoin.
            - Folder sync (project code) between two devices → comments and
              suggestions arrive (`review/` in the replica; no roles).
            - Offline review changes → pushed and arrive after reconnecting.
- [ ] Protocol and client versioning (user's request 2026-10-25, before the
      first release). Found in the step 4 hand test: a client from before
      review sync silently skipped the `review` streams and kept their read
      positions, so after upgrading the other member's review changes never
      arrived (fixed in a46291e: no position for documents not synced
      here). Today the wire has only a version byte (`VERSION = 1`, any
      other refused as a decode error). Two more gaps found while planning:
      the members log rejects unknown ops (`deny_unknown_fields`, closed
      `Op` enum), so any new op stops old clients with "members log
      doesn't match" and old servers refuse its append; and a client skips
      a record it can't open but still moves its read position past it
      (`records` in core `server.rs`), so after updating it never reads it.
      Scope (decided 2026-10-25): version wherever separately updated
      copies meet (wire, members log, synced documents and records, files
      on disk); not the web view ↔ core or Tauri commands (one binary).
      Rule: a client never moves past data it can't understand, and never
      writes data an older client would get wrong. Plan:
      - Three numbers. `protocol`: the wire encoding, rarely changed; the
        server serves a range. `level`: what synced data a client
        understands, one integer raised per feature that needs it. App
        version (SemVer + git hash): for people and bug reports only,
        never for compatibility. The hello carries all three, the welcome
        the server's. Challenge and refusal keep a frozen layout, so any
        version can read "update the app".
      - Minimum level per project: members op `Require { level, app }`
        (`app` = the first app version with that level, so old clients can
        name it; by an owner, editor or reviewer device). Raised by the
        client that first uses the feature (`ServerSync::require`); never
        lowered. An unknown members op that follows the chain and is signed
        by its device counts as "this project needs a newer app"
        (`Error::Newer`), not as corruption; the server refuses ops it
        doesn't know ("This server needs an update for this feature (ask
        its admin)") and stores nothing, so the server is updated first.
        Folder sync has no members log: the level comes from the records
        read plus the device's own raised level (`Document::sync_require`,
        in its own state file; no shared file changes, `replica.json`
        stays format 1).
      - Numbers built: protocol 2 (`MIN_VERSION..=VERSION`, both 2), the
        hello carries `ClientVersion { level, app }` (signed), the welcome
        the server's version and each device's last level; `LEVEL = 1`;
        envelopes v3 (folder) and v4 (server) carry the level in the
        header, authenticated; v1/v2 read as level 1.
      - Cut-over while old clients are online:
        1. The new client appends `Require` and waits for its ack before
           writing any data that uses the feature.
        2. The server stores it, then closes every open connection whose
           hello level is lower (`Code::Version`), before any new-feature
           data exists; hellos below the level are refused from then on.
           A batch from an old client that arrives just after the cut is
           still accepted (old data stays valid for new clients).
        3. The old client stops syncing with "This project now needs
           Lethora <app> or later". Editing stays possible, as offline; its
           changes stay in its own log and are pushed after the update
           (Loro merges them, nothing is lost).
        4. Per-record level as safety net: each record and snapshot says
           the level it needs; a client meeting one above its own stops
           that stream without moving its read position (also for a
           record it can't open for being of a newer format; damaged or
           forged records are still skipped). Covers races and folder
           sync, where sync apps may deliver blobs before `replica.json`.
        5. Heads-up for the new client: the server keeps each device's
           last hello level; before raising, the client warns "N members
           use an older Lethora; they need to update to keep syncing". It
           warns, doesn't block. The dialog itself (the query is built:
           `ServerSync::devices_below`) comes with the first feature that
           raises the level.
      - Diagnostics: one version source (workspace `Cargo.toml`; version
        dropped from `tauri.conf.json`), git hash from a build script.
        Help → About shows app version, protocol, level and the server's
        version, with "Copy diagnostic info". The server prints its
        version at startup and in `/healthz`.
      - Must be in the first release (can't be added to shipped clients):
        stop-don't-skip (4) and unknown members ops as "update needed".
      - Trade-off (accepted): the first use of a feature locks out members
        who haven't updated until they do, instead of silent drift.
      - Built by Opus, reviewed, checked in the dev mock (`?sync=server-update`,
        `?sync=needsUpdate`, Help → About), awaiting the hand test. Protocol 2
        refuses the protocol 1 builds: update the server and both PCs together.
        Hand test (two PCs + server, and folder sync):
        - Server startup log and `/healthz` show version, protocol 2-2,
          level 1.
        - Existing projects (server-shared and folder-synced) still open
          and sync both ways; share, join, edit, comment both ways;
          `meta.json` on the server gains `devices`.
        - Help → About: version (git hash), protocol 2, level 1, system,
          server version once online; "Copy diagnostic info" pastes plain
          text.
        - An old build (protocol 1) against the new server → a clear
          "update Lethora" message in the Share / Sync dialog, no retry loop.
        - Folder sync between two devices still works both ways.
        - `npm --prefix app run tauri build` once: the bundle picks up
          the version without it in `tauri.conf.json`.
        - The needs-update path (level 2) is covered by tests only: no
          build has level 2 yet.
        Known: the `-dirty` mark in About only refreshes when the git index
        or HEAD changes (an unstaged edit after the last build isn't seen).
- [ ] 5. Accounts and OIDC. User's idea (2026-10-22), instead of direct
      re-invites of a removed member: contacts (friends/colleagues) for
      quick invites, later a global user search; accounts public, private
      or contacts-only, and owned projects visible by the same settings per
      project.
- [ ] 6. Browser version (WebAssembly), for trying the app instantly
- [ ] Later: opt-in encryption for local-only projects (project setting);
      external audit of the encryption before offering it to institutions

## Progress log (moved from TODO.md)

State (2026-10-13): Milestones 1–3 done; Milestone 4 steps 1–5, the
exception markers and upgrade batches A and B hand-tested and pushed (plus a
layout panel fix: a setting changed back to its previous value wasn't sent).
(The bar clipping below ~1080 px is fixed by the title bar, T1.) Batches
C–G, the tech doc (`docs/dev/architecture.md`), README and the screenshot
tooling are pushed. The user manual is deferred (see "Later").
2026-10-15: collaboration planned, decision record in
`docs/dev/collaboration.md`; Milestone 5 step 1 planned in five parts (below).
Step 1 (parts 1a–1e) built and committed, awaiting the user's hand test
(open an existing project → converted, `format1-backup/` appears; text,
checkpoints and sources intact, `references.bib` gets a `% Lethora mirror`
first line, images still show and their files move to `backup/assets/`
while `assets/` holds 64-hex blobs; create, edit, undo, named checkpoint,
restore an older checkpoint → newer ones stay; import a source, undo it;
edit `references.bib` outside (or let Zotero export) → focus/F5 merges it;
insert a figure, right-click its image on a page → "Replace image…", undo;
reopen). "Replace image…" now repoints only the clicked figure to the new
file's own path (one undo step); the user tested it. Assets explorer A1
(core and commands), A2 (the bottom panel, F8) and A3 (drag & drop) are
built and hand-tested (both Replace features too); A4 (screenshots, README)
done and pushed. 2026-10-16: Milestone 5 step 2 (folder sync) planned in
three parts (below); 2a–2d built, documented and committed (2b hand-tested on two PCs; 2d
hand-tested on two Windows PCs 2026-10-23).
Title bar T1–T3 (menus, command list, row; frameless window, own buttons,
recent projects, Close project, Exit; docs, screenshots) built, committed
and hand-tested, pushed; the Snap Layouts flyout on our Maximize button
(native overlay, `snap.rs`, `docs/dev/snap-layouts.md`) hand-tested and merged.
The splash screen (own small window, light and dark) is built and
hand-tested. 2026-10-17: Milestone 5 step 3 (sync server) planned in parts
3a–3g (below); 3a (protocol crate: keys, envelopes, members log, invite
links), 3b (wire messages + `lethora-server`) and 3c1 (sync engine,
share, invite links, join) built and committed; 3c1 hand-tested on two
PCs 2026-10-18 (after two fixes: a ready join stays ready, a member device
rejoins with its own stream) and pushed. 3c2 (members screen, read-only
roles, leave/remove/delete turn local, reopen from server) built and
committed 2026-10-18, awaiting the two-PC hand test (steps in 3c2 below).
3d (key epochs) built 2026-10-22; 3e (presence) and 3f (who wrote this) planned the same day, built before the hand test of all of step 3. 3e and 3f built the same day. Next: the user's combined hand test of step 3 (under 3f), then 3g. The macOS title bar
waits for a Mac.
