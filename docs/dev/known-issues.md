# Known issues

Accepted or low-priority limitations of finished work, kept on purpose, one
line each. Work we intend to do lives in the
[issue tracker](https://github.com/BechtStudio/lethora/issues). Fixing one:
remove its line and note the fix in `CHANGELOG.md`. Turning one into planned
work: open an issue and remove the line. The full notes are under "Known:" in
the archived step plans in `docs/plans/` (step in parentheses).

Limitations a writer may notice are in [the user manual](https://github.com/BechtStudio/lethora/blob/main/docs/manual/known-limitations.md).

## Sync and sharing
- Unwritable app config dir: every start gets a new device id, a new file set per project (M5 1a)
- Importing another device's files is silent when it skips them (M5 1a)
- Folder sync: a device skipped for a bad record is re-read from scratch whenever its files grow (M5 2a)
- Folder sync: a garbage length field looks like a torn tail, the device waits instead of being skipped (M5 2a)
- Folder sync: the `checkpoints` flag misses a named checkpoint listed only when its history arrives (M5 2a)
- Folder sync: two devices pushing the same blob at once write it twice (M5 2a)
- Folder sync: push/pull don't check the replica's project id (2b checks on open) (M5 2a)
- Folder sync push-failure path with the folder present is untested against a flaky drive (M5 2b)
- A share failing partway leaves an orphan project on the server (a retry makes a new one) (M5 3c1)
- A server stream that shrank makes the client push everything again (M5 3c1)
- Truncation of the server's members log to a valid prefix isn't detected (M5 3c1)
- Records failing verification are skipped and only logged (M5 3c1)
- One batch at a time; a change over 3 MiB uploads a full snapshot instead (M5 3c1)
- Local member checks use this device's copy of the members log (just-joined member) (M5 3c2)
- Dev mock: invites are always "by" the own user (M5 3c2)
- Key epochs: until a current device is online after a removal, new content keeps the old key (M5 3d)
- Key epochs: a rotation refused for a reason other than Stale is only logged, retried on the next change or reconnect (M5 3d)
- Key epochs: whoever fetched an open invite's keys keeps them and that invite's epoch copies; revoking doesn't rotate (M5 3d)
- Key epochs: a current member can write an epoch whose copies don't open for others (not checkable) (M5 3d)
- Key epochs: every epoch entry grows ~180 bytes per device or open invite (M5 3d)
- A join's expiry check uses the writer's claimed time (the server must check its own clock) (M5 3a)
- Wrap, keyring and exported secret arrays aren't zeroized (only the dalek keys are) (M5 3a)
- The wasm32 build of the protocol crate is unverified (M5 3a)

## Server
- A malicious server can withhold, delay, reorder or replay records, delete data, see metadata (M5 3b)
- Hellos, HTTP requests and invitee connections aren't rate-limited (only connections per IP) (M5 3b)
- Per-IP counters and tokens live in memory (reset on restart) (M5 3b)
- One lock per project and one global registry lock (projects load one at a time after restart) (M5 3b)
- Logs are read whole on load and compaction (relies on clients compacting) (M5 3b)
- Two fsyncs per append (M5 3b)
- Snapshots and assets held in memory, capped by the 100 MiB asset limit; book-size not measured (M5 3b)
- Quota per project only (no per-owner or global limit) (M5 3b)
- A crash between append and `meta.json` can store a resent batch twice; a lost `meta.json` resets generations (M5 3b)
- No server WebSocket pings (M5 3b)
- Expired invite keys are swept only in loaded projects (M5 3b)
- No protocol version negotiation; `X-Forwarded-For` untested (M5 3b)
- The members log is never compacted (M5 3b)

## Review tools
- Unknown authors for others' changes from before sharing, in projects shared before 4b (rejoin recomputes), after a crash before the state write, and for a peer whose `peers` entry was overwritten (M5 4b)
- The trust origins in the server state grow by one entry per session peer (M5 4b)
- A demotion seen live is enforced on incoming records from the next catch-up on (the server enforces it at once) (M5 4b)

## Bibliography, checkpoints and history
- A device without state, older than a newer marked mirror, rewrites it with its older text (M5 1d)
- Three-way merge cost at book-size history not measured (M5 1d)

## Assets and images
- Dev mock page clicks don't hit figure boxes on real pre-rendered pages (checked by unit tests) (M5 1e)
- Asset usage is a literal text scan (expert Typst, built paths don't count; comments do) (M5 A1)
- Lines far beyond the parsed range count as top level for a drop (M5 A3)
- Typing above the drop point during a drop shifts it to the old boundary (M5 A3)

## Title bar and window
- The editor's top padding was sized for the overlaid bar (M5 T1)
- tao's top resize strip overlaps the row's top pixels (M5 T2)

## Layout and exceptions
- Exceptions made before paragraph-follows-Typst on a mid-paragraph line show no checkmark (M4 batch 0)
- Exceptions drawer not tried with ~1000 exceptions (the mock's book mode has none) (M4 G)

## Editor and formatting
- Format scan reads lines from their start as markup (multi-line math/raw can be misread) (M6 1)
- A `]` in a code string inside styled content ends the styling early (M6 1)
- Ctrl+I replaces CodeMirror's select-parent-syntax (M6 1)

## Character styles
- A lone `styles.<id>.name` is stored but unused and stays after the last font/size is removed (M6 2a)
- Invalid style groups aren't reported (M6 2a)
- `#styled(5)[x]` (not a string) is a Typst error (M6 2a)
- Embedded-font previews work only in the real app (the mock has no font files) (M6 2b)
- Style use counts rescan the whole text (debounced 400 ms) while the publisher shows (M6 2c)

## Tables
- Academic rules only in `table(...)` calls without trailing content blocks; a `..` header is missed; code-built tables get only the top rule (M6 3a)
- Bold header also reaches tables nested in a cell; header rowspans aren't counted (M6 3a)
- Table exceptions are matched by position, as figure placement (M6 3a)
- With an academic default, every compile of a text containing `table(` parses the content once more (M6 3a)
- Table edits: named arguments after the cells move above them; a changed merged cell's arguments are reordered (M6 3b)
- Delete table on a table nested in another call leaves that call empty (M6 3b)
- EPUB: not yet validated with epubcheck or a real reader (R4b)
- Recovery: with server sync, changes already pushed and then dropped by "Open at the last good state" stay on the server; other devices' later changes built on them wait on this device (R5b)
