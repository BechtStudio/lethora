//! Lethora core: the document model, undo/redo, checkpoints, and persistence.
//!
//! The document is a Loro `LoroText` container named `"content"` holding plain
//! text in Typst markup, plus the layout layer's settings: three maps of
//! JSON-like scalars (`"layout"`, `"meta"` and `"overrides"`, see [`MapName`];
//! the core does not know their schema), a `LoroText` `"layout_custom"` with
//! expert Typst code and a `LoroText` `"bibliography"` with the sources
//! (BibLaTeX, see "Bibliography" below). All
//! of them share one history: undo/redo, checkpoints and persistence cover
//! settings exactly like the text. The core owns no threads or timers; the app
//! calls [`Document::autosave`] and [`Document::maybe_auto_checkpoint`] on a
//! schedule.
//!
//! # Project folder format
//!
//! Format 2. Each device (a [`DeviceId`]) writes only its own files, so
//! folder-sync tools never see two writers on one file. All I/O goes through
//! [`Storage`]; atomic writes go through a fsynced temp file (`<name>.tmp`)
//! renamed over the target.
//!
//! - `project.json`: `{"format": 2}`, written atomically and last when a
//!   project is created or converted. Its presence marks a project.
//! - `main/<device>.snapshot`: the device's full document history as one
//!   Loro export (all updates; see `export_snapshot`), written atomically.
//! - `main/<device>.log`: the device's append-only records, each
//!   `[u32 LE len][u32 LE crc32][len bytes: Loro update export]`. Every
//!   autosave appends one record with the updates since the last persisted
//!   version. When the log exceeds the compaction threshold, a new snapshot is
//!   written first and the log is truncated afterwards; re-importing updates
//!   already in the snapshot is idempotent, so a crash in between is safe.
//! - `review/<device>.snapshot` and `review/<device>.log`: the same for the
//!   second Loro document, `review` (comments, marks and suggestions, see the
//!   `review` module): same records, torn-tail handling and compaction,
//!   saved by the same autosave. A project without them has an empty
//!   `review`; nothing is written there until `review` changes.
//! - `checkpoints/<device>.json`: the device's automatic checkpoints (a JSON
//!   array of [`Checkpoint`]), written atomically. Named ones live in the
//!   document (see "Checkpoints" below).
//! - `<device>.lock`: empty; an open [`Document`] holds an exclusive OS lock
//!   on it so a device edits (or repairs) its files from one instance only.
//! - `state/<device>.json`: the device's own state, written atomically:
//!   `{"mirror": {"crc32": <u32>, "len": <bytes>, "version": <hex: encoded
//!   Loro frontiers>}, "sync": {"folder": <path>, "project": <32 hex>}}`:
//!   the bibliography text this device last mirrored and the (persisted)
//!   version it came from, and where this device syncs the project
//!   ([`SyncSettings`], kept for the app). Either part may be missing.
//!   Missing or unreadable = nothing mirrored or synced yet.
//! - `references.bib`: a mirror of the bibliography (see "Bibliography").
//! - `assets/<blob>`: asset blobs (64 lowercase hex characters, see
//!   "Assets"), written atomically, never changed or deleted.
//! - `backup/assets/<name>`: loose files found in `assets/`, moved there once
//!   adopted (see "Assets").
//! - `backup/damaged/<main|review>-<device>.log` (`-2`, `-3`, ... if taken):
//!   byte-exact copies of damaged own logs, written atomically before the
//!   log is cut back (see below).
//!
//! Opening loads the own snapshot and replays the own log in order. A torn or
//! corrupt tail (short record or crc mismatch) ends the replay and is
//! truncated away. A record that passes its crc but Loro rejects (damaged)
//! fails the open with [`Error::Damaged`] naming the log, and a snapshot Loro
//! rejects with [`Error::Corrupt`] naming it; both main and review are read
//! before anything is written, so neither changes a file. Only
//! [`Document::open_at_last_good`] recovers a damaged log: after copying it
//! to `backup/damaged/`, it treats the record like a torn tail (it and every
//! record after it are cut away) and rebuilds the document up to it. Then
//! every other device's snapshot and log are imported read-only: a torn tail
//! ends that log, data Loro rejects skips the rest of that device (listed in
//! [`Document::skipped_on_open`]), and changes whose history is missing wait
//! inside Loro.
//! The last persisted version is the one of the own files, so the next
//! autosave also writes what came from other devices into the own log: the
//! own files always hold everything this device has seen, on their own.
//! [`Document::pull_local`] later imports what the other devices add to their
//! files while the project is open (continuing from what opening read).
//! `review/` is opened, saved and pulled the same way, alongside `main/`.
//!
//! Format 1 (`snapshot.loro`, `updates.log`, `checkpoints.json`, `lock` in
//! the root) is converted on open into the opening device's files; the old
//! files are then moved to `format1-backup/`.
//!
//! # Positions
//!
//! [`Document::insert`]/[`Document::delete`] take Unicode scalar indices.
//! [`TextChange`] uses UTF-16 code units and CodeMirror change-set semantics,
//! for the editor bridge: [`Document::apply_changes`] takes edits from the
//! editor; undo, redo and restore return what they changed.
//!
//! # Anchors
//!
//! [`Document::anchor_at`] turns a position into an opaque string (an encoded
//! Loro cursor on the character there) that follows that character through
//! edits, undo/redo and reopening; [`Document::resolve_anchor`] finds it again.
//!
//! # Attribution
//!
//! Loro tags every change with a random peer id per session. The core-owned
//! map `"peers"` (not a [`MapName`]) records who each peer is: key = the peer
//! id in decimal, value = a Loro map value `{"user": <UserId>, "device":
//! <DeviceId>}` (extra keys are ignored). A session writes its entry right
//! before its first local change, in a commit of its own with the origin
//! `"lethora:sys"`, which the undo manager ignores; opening without editing
//! writes nothing. Entries are untrusted: invalid ones count as unknown, and
//! in a server-shared project an entry counts only as far as the stream
//! that brought the peer's ops grounds it (see the `trust` module); in
//! local-only and folder-sync projects it counts as it is.
//! [`Document::author_at`] maps the peer that inserted a character through
//! this map; [`Document::authors_in`] does so for a range, as runs. Text
//! that undo or restore brings back is inserted anew, so it counts as the
//! undoing or restoring user's. Restore leaves `"peers"` alone (see [`Document::restore`]).
//!
//! # Checkpoints
//!
//! Named checkpoints are shared: the core-owned map `"checkpoints"` holds
//! them, key = the [`CheckpointId`] in decimal, value = a Loro map value
//! `{"name": <string>, "created": <i64 Unix seconds>, "version": <binary:
//! encoded Loro frontiers>, "user": <UserId>, "device": <DeviceId>}` (who made
//! it; not read yet, extra keys are ignored). Entries are written like
//! `"peers"` entries (a commit of their own, origin `"lethora:sys"`, so
//! creating one is not undoable) and are untrusted: invalid ones are ignored,
//! and one whose version this document doesn't contain yet (its history has
//! not arrived) is left out until it does. Restore leaves the map alone, so
//! restoring an old checkpoint keeps the newer ones. Automatic checkpoints
//! stay in the own `checkpoints/<device>.json`; other devices' are never
//! read. Named checkpoints found in the own file (written before they were
//! shared) move into the map on open (see [`Document::open_in`]).
//!
//! Ids are JS-safe (below 2^53, never 0): SplitMix64 outputs seeded with the
//! session's random Loro peer id, skipping ids already in use.
//!
//! # Bibliography
//!
//! The sources live in the `"bibliography"` text (content, so undo, restore
//! and checkpoints cover them). Line breaks are stored as `\n` and a leading
//! byte order mark is dropped; at most [`MAX_BIBLIOGRAPHY_BYTES`].
//! [`Document::set_bibliography`] is an undo step of its own.
//!
//! `references.bib` in the project root mirrors it for Zotero, JabRef and
//! hand edits. Its first line is a marker, `% Lethora mirror crc32=<8 hex>;
//! ...`, the crc32 of the bytes after that line; the rest is the text.
//! - [`Document::autosave`] writes the mirror when the text differs from what
//!   this device last mirrored (its state file; nothing mirrored counts as an
//!   empty text), not merely because the file differs: a mirror another
//!   device wrote stays while this device's text is the one it last
//!   mirrored. The write is skipped when the file already holds
//!   exactly that text, and nothing is written for an empty text and no
//!   file. A file without a valid marker (an outside edit) is never
//!   overwritten here: it waits for the merge.
//! - [`Document::merge_bibliography_mirror`] (called by the app on focus, F5
//!   and project load) imports an outside edit. A file with a valid marker
//!   (from any device) is never imported. Otherwise its text (byte order mark,
//!   a stale marker line and `\r` before `\n` dropped) is merged against
//!   the version this device last mirrored (the base): if the bibliography
//!   is unchanged since the base (crc32 and length of the state), or this
//!   device has no base, the file text is applied as an ordinary undo step;
//!   if both changed, a fork at the base takes the file text and its change
//!   is imported, so additions on both sides survive. Such a merge is not
//!   undoable (it arrives like another peer's change); the fork's peer is
//!   registered to this identity first. The mirror is then rewritten with a
//!   marker.
//!
//! # Assets
//!
//! Files the text refers to (figure images) are stored as immutable blobs
//! `assets/<blob>`, where `<blob>` is the lowercase hex SHA-256 of the
//! project key followed by the bytes: identical content is stored once, and
//! once projects are encrypted a server can't tell whether a project holds a
//! known file. The project key is 32 random bytes in the core-owned map
//! `"project"` (key `"asset_key"`, binary), created on the first blob write
//! in a bookkeeping commit (origin `"lethora:sys"`, not undoable). If two
//! devices create one concurrently, Loro's last writer wins; blobs named with
//! the other key stay valid, since the index stores blob names (the same
//! content may then be stored twice).
//!
//! The map `"assets"` (content, so undo, restore and checkpoints cover it) is
//! the index: key = the path used in the text (e.g. `assets/foo.png`), value
//! = the blob name as a string. Folders come from the paths
//! (`assets/figures/ch1/x.png`); a folder is kept explicitly by a marker, key
//! = the folder path ending in `/` (e.g. `assets/figures/ch1/`), value =
//! `true`, which implies its ancestors. Every write that puts an item into a
//! folder writes that folder's marker if it has none, so emptying a folder
//! never removes it; the first component (`assets/`, the root) needs none.
//! Entries are untrusted: values that are neither blob names nor (for keys
//! ending in `/`) `true` are ignored; paths are the caller's to validate. The
//! core knows nothing about images.
//!
//! [`Document::put_asset`] writes the blob and the entry as an undo step of
//! its own; [`Document::repoint_asset`] also points one reference in the
//! text at the new entry, in the same step. [`Document::stage_blob`] writes a
//! blob only; [`Document::insert_with_assets`] then indexes staged blobs and
//! inserts text (e.g. figures for dropped files) as one undo step, so a
//! failure before it leaves at most an unindexed blob.
//! [`Document::move_assets`] renames entries and markers and rewrites every
//! exact string literal `"<old>"` in the content text to `"<new>"`, in one
//! undo step;
//! [`Document::remove_assets`] deletes entries and markers in one undo step,
//! refused while the text uses an item. Usage
//! ([`Document::asset_usage`]) counts those literals in the content text
//! only: a path built from parts, or used only in the expert code, counts as
//! unused. Blobs are never deleted. Restore applies new and changed entries
//! but never deletes one, so a checkpoint from before an image was added,
//! moved or removed (or from before the index existed) still finds it.
//!
//! Loose files (anything else directly in `assets/`, e.g. images from before
//! the index or copied in by hand) are adopted by
//! [`Document::adopt_loose_assets`]: stored as blobs and indexed under
//! `assets/<file name>` (unless that path is indexed already), then moved to
//! `backup/assets/`.
//!
//! # Folder sync
//!
//! An end-to-end encrypted copy of the project (a [`Replica`]) in a separate
//! sync folder, with the same per-device layout: [`Document::sync_setup`],
//! [`Document::sync_join`], [`Document::sync_open`], [`Document::sync_push`]
//! and [`Document::sync_pull`]. The replica format, the envelope and the
//! [`ProjectCode`] are described in the `sync` module
//! (`crates/core/src/sync.rs`). A pull returns what changed as [`Pulled`].
//! The replica carries both documents, `main` and `review`.
//!
//! # Review
//!
//! Comments, marks and suggestions live in the second Loro document,
//! `review`, anchored in `main`'s content text; its format and API are
//! described in the `review` module (`crates/core/src/review.rs`). Write
//! rights follow the role per document (see [`Document::set_role`]).

mod review;
mod server;
mod storage;
mod sync;
mod trust;

/// Keys, envelopes, members and invites, shared with the sync server.
pub use lethora_protocol as protocol;
pub use review::{
    MARK_COLOURS, ReviewAuthor, ReviewId, ReviewItem, ReviewKind, ReviewMessage, Suggestion,
    SuggestionState,
};
pub use server::{
    Action, AuthorInfo, Authors, HttpRequest, HttpResponse, InviteInfo, MemberInfo, Method, OwnPos,
    PALETTE, PresenceInfo, Profile, ServerSettings, ServerState, ServerStatus, ServerSync,
    StreamPos, profile_colour,
};
pub use storage::{FsStorage, LockGuard, LogFile, Storage};
pub use sync::{
    ProjectCode, Pulled, Replica, replica_project, stored_server_settings, stored_sync_settings,
};
pub use trust::PeerOrigin;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lethora_protocol::frame::{Frame, parse_frame};
use lethora_protocol::members::Role;
use loro::cursor::{Cursor, PosType, Side};
use loro::event::{Diff, DiffBatch};
use loro::{
    ContainerID, ContainerTrait, ContainerType, ExportMode, Frontiers, ID, LoroDoc, LoroMap,
    LoroText, LoroValue, PeerID, TextDelta, UndoManager, UpdateOptions, ValueOrContainer,
    VersionVector,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const CONTENT: &str = "content";
const LAYOUT: &str = "layout";
const META: &str = "meta";
const OVERRIDES: &str = "overrides";
const LAYOUT_CUSTOM: &str = "layout_custom";
const BIBLIOGRAPHY: &str = "bibliography";
const PEERS: &str = "peers";
const CHECKPOINTS_MAP: &str = "checkpoints";
const PROJECT_MAP: &str = "project";
/// The asset index map, and the folder of the blobs.
const ASSETS: &str = "assets";
/// Key of the project key in the `project` map (see "Assets").
const ASSET_KEY: &str = "asset_key";
const ASSET_KEY_LEN: usize = 32;
const BACKUP_ASSETS: &str = "backup/assets";
const BACKUP_DAMAGED: &str = "backup/damaged";
/// Root maps the core keeps for itself; restore leaves them alone.
const BOOKKEEPING: [&str; 3] = [PEERS, CHECKPOINTS_MAP, PROJECT_MAP];
/// Commit origin of bookkeeping writes; the undo manager ignores them.
const SYS_ORIGIN: &str = "lethora:sys";
const MAX_KEY_LEN: usize = 256;
const PROJECT: &str = "project.json";
const FORMAT: u64 = 2;
const MAIN: &str = "main";
/// The folder of the `review` document's files.
const REVIEW: &str = "review";
const CHECKPOINTS: &str = "checkpoints";
const SNAPSHOT_EXT: &str = ".snapshot";
const LOG_EXT: &str = ".log";
const FORMAT1_SNAPSHOT: &str = "snapshot.loro";
const FORMAT1_LOG: &str = "updates.log";
const FORMAT1_CHECKPOINTS: &str = "checkpoints.json";
const FORMAT1_LOCK: &str = "lock";
const FORMAT1_BACKUP: &str = "format1-backup";
const STATE: &str = "state";
const REFERENCES: &str = "references.bib";
/// Start of the mirror's marker line (see "Bibliography" in the module docs).
const MARKER: &str = "% Lethora mirror crc32=";
/// A line diff of the bibliography taking longer replaces the whole text.
const BIB_DIFF_TIMEOUT_MS: f64 = 2000.0;
const MAX_UNDO_STEPS: usize = 1000;
/// Checkpoint ids stay below 2^53, so JavaScript numbers hold them exactly.
const MAX_CHECKPOINT_ID: u64 = (1 << 53) - 1;

/// Log size (bytes) above which `autosave` compacts into a fresh snapshot.
pub const DEFAULT_COMPACTION_THRESHOLD: u64 = 4 * 1024 * 1024;
/// Minimum time between automatic checkpoints.
pub const DEFAULT_AUTO_CHECKPOINT_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// Edits closer together than this (wall clock) merge into one undo step.
pub const UNDO_MERGE_INTERVAL_MS: i64 = 1000;
/// Largest bibliography (and `references.bib`), in bytes.
pub const MAX_BIBLIOGRAPHY_BYTES: usize = 16 * 1024 * 1024;

/// Errors returned by the core.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("document model error: {0}")]
    Loro(#[from] loro::LoroError),
    #[error("corrupt project data: {0}")]
    Corrupt(String),
    /// A record of this device's own log (`file`, project-relative, e.g.
    /// `main/<device>.log`) passes its crc but Loro rejects it; see
    /// [`Document::open_at_last_good`].
    #[error("a change in {file} can't be read")]
    Damaged { file: String },
    #[error("not found: {0}")]
    NotFound(String),
    #[error("a project already exists in {0}")]
    AlreadyExists(PathBuf),
    #[error("the project in {0} is already open")]
    Locked(PathBuf),
    #[error("unsupported project format: {0}")]
    UnsupportedFormat(String),
    #[error("invalid change: {0}")]
    InvalidChange(String),
    #[error("invalid project code: {0}")]
    InvalidCode(String),
    #[error("wrong project code for the synced project in {0}")]
    WrongCode(PathBuf),
    /// This device's role on the server doesn't let it change the project,
    /// or its `review` document (the role's name, e.g. "Reader"; see
    /// [`Document::set_role`]).
    #[error("this project is read only for you ({0})")]
    ReadOnly(String),
    /// Only the author of a review item or message may do this (the
    /// action, e.g. "edit this message").
    #[error("only its author can {0}")]
    NotAuthor(String),
    /// A suggestion can't be accepted: the text under it changed since it
    /// was made, or was deleted (why).
    #[error("the suggestion is out of date: {0}")]
    Stale(String),
    /// A members change the project's rules refuse (e.g. only the owner
    /// changes roles).
    #[error("not allowed: {0}")]
    NotAllowed(String),
    #[error("range {pos}+{count} is out of bounds for text of length {len}")]
    OutOfRange {
        pos: usize,
        count: usize,
        len: usize,
    },
}

impl From<loro::LoroEncodeError> for Error {
    fn from(e: loro::LoroEncodeError) -> Self {
        Error::Loro(e.into())
    }
}

/// Result type of the core.
pub type Result<T> = std::result::Result<T, Error>;

/// Identifier of a device (an installation of the app): 32 lowercase hex
/// characters. It names the device's files in a project folder.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct DeviceId(String);

impl DeviceId {
    /// Checks that `s` is 32 lowercase hex characters.
    pub fn parse(s: &str) -> Result<Self> {
        parse_hex_id(s, "device").map(Self)
    }

    /// The id as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Identifier of a user, who may edit from several devices: 32 lowercase hex
/// characters, like a [`DeviceId`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct UserId(String);

impl UserId {
    /// Checks that `s` is 32 lowercase hex characters.
    pub fn parse(s: &str) -> Result<Self> {
        parse_hex_id(s, "user").map(Self)
    }

    /// The id as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn parse_hex_id(s: &str, what: &str) -> Result<String> {
    if s.len() == 32 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        Ok(s.to_owned())
    } else {
        Err(Error::Corrupt(format!(
            "{what} id {s:?} is not 32 lowercase hex characters"
        )))
    }
}

/// Who edits a [`Document`]: this device and its user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub device: DeviceId,
    pub user: UserId,
}

/// Who inserted a character (see [`Document::author_at`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authorship {
    pub user: UserId,
    pub device: DeviceId,
    /// Unix seconds at the start of the Loro change holding the insertion;
    /// Loro merges a peer's consecutive changes within 1000 s into one change.
    /// `None` if the change has no time (format-1 history).
    pub time: Option<i64>,
}

/// Consecutive characters inserted by the same user at the same time (see
/// [`Document::authors_in`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuthorRun {
    /// UTF-16 offsets into the text.
    pub from: usize,
    pub to: usize,
    /// `None` for an unknown peer.
    pub user: Option<UserId>,
    /// As [`Authorship::time`].
    pub time: Option<i64>,
}

/// Identifier of a checkpoint, unique within a project; below 2^53 and never
/// 0 (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CheckpointId(pub u64);

/// A stored document version that can be previewed or restored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: CheckpointId,
    /// User-given name; `None` for automatic checkpoints.
    pub name: Option<String>,
    /// Whether this checkpoint was created automatically.
    pub auto: bool,
    /// Creation time in Unix seconds.
    pub created: u64,
    /// Encoded Loro frontiers of the checkpointed version.
    pub version: Vec<u8>,
}

/// A text change in CodeMirror change-set semantics: replaces `from..to` with
/// `insert`. Offsets are UTF-16 code units into the text before the whole
/// change set; changes in a set are sorted and non-overlapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextChange {
    pub from: usize,
    pub to: usize,
    pub insert: String,
}

/// The settings maps of a document. Their values are JSON scalars (null,
/// bool, number, string); keys and meaning are up to the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MapName {
    /// Layout settings.
    Layout,
    /// Document metadata (title, author, ...).
    Meta,
    /// Anchored per-element exceptions of the layout layer.
    Overrides,
}

impl MapName {
    fn id(self) -> &'static str {
        match self {
            MapName::Layout => LAYOUT,
            MapName::Meta => META,
            MapName::Overrides => OVERRIDES,
        }
    }
}

/// The entries of a settings map.
pub type Entries = BTreeMap<String, Value>;

/// Where an anchor (see [`Document::anchor_at`]) is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorPos {
    /// UTF-16 offset into the text.
    pub pos_utf16: usize,
    /// Byte (UTF-8) offset into the text.
    pub pos_byte: usize,
    /// The anchored character was deleted; `pos_utf16` is where it was.
    pub detached: bool,
}

/// What an edit changed. Switching between them always starts a new undo
/// step, so undoing a setting never takes typing with it (and vice versa).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Content,
    Settings,
    Sources,
    Assets,
}

/// `state/<device>.json` (see the module docs).
#[derive(Debug, Default, Serialize, Deserialize)]
struct DeviceState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mirror: Option<Mirrored>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sync: Option<SyncSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    server: Option<ServerSettings>,
}

/// Where this device syncs the project (see [`Document::sync_settings`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncSettings {
    /// The sync folder holding the replica.
    pub folder: PathBuf,
    /// The replica's project id ([`Replica::project_id`]).
    pub project: String,
    /// The feature level this device raised the project to
    /// ([`Document::sync_require`]); `None`: not raised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<u32>,
}

/// The bibliography text this device last mirrored into `references.bib`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Mirrored {
    crc32: u32,
    /// Length in bytes.
    len: u64,
    /// Hex of the encoded frontiers of the version the text came from.
    version: String,
}

/// An own log cut back to its last good record on open (see
/// [`Document::open_at_last_good`]). Paths are project-relative.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Recovered {
    /// The log, e.g. `main/<device>.log`.
    pub file: String,
    /// Bytes dropped from its end: the damaged record and all after it.
    pub dropped: u64,
    /// The byte-exact copy of the whole log, in `backup/damaged/`.
    pub backup: String,
}

/// An open project: the live document plus its on-disk store.
pub struct Document {
    storage: Box<dyn Storage>,
    identity: Identity,
    doc: LoroDoc,
    text: LoroText,
    custom: LoroText,
    bib: LoroText,
    undo: UndoManager,
    /// What this device last mirrored into `references.bib`.
    mirrored: Option<Mirrored>,
    /// The version the mirror was last checked at (see `update_mirror`).
    mirror_checked: Option<VersionVector>,
    /// This device's files of `main`.
    files: Files,
    /// The `review` document (see the `review` module).
    review: review::Review,
    /// Where `main`'s peers' ops came from, in a server-shared project (see
    /// the `trust` module).
    trust: trust::Trust,
    /// This device's automatic checkpoints, oldest first.
    autos: Vec<Checkpoint>,
    /// Checkpoint id candidates drawn this session (see `new_checkpoint_id`).
    ids_drawn: u64,
    auto_interval: Duration,
    compaction_threshold: u64,
    split_next_edit: bool,
    last_part: Option<Part>,
    /// The Loro peer whose `peers` entry is known to be written.
    registered: Option<PeerID>,
    /// What has been read of the other devices' files in `storage` (see
    /// [`Document::pull_local`]).
    local_reads: sync::Reads,
    /// Where this device syncs the project, from its state file.
    sync: Option<SyncSettings>,
    /// Where this device syncs the project with a server, from its state
    /// file (see the `server` module).
    server: Option<ServerSettings>,
    /// This device's role in the project on a server, which decides what it
    /// may write (see [`Document::set_role`]).
    role: Option<Role>,
    /// The own logs opening cut back (see [`Document::recovered`]).
    recovered: Vec<Recovered>,
    /// The other devices' data skipped on open (see
    /// [`Document::skipped_on_open`]).
    skipped_on_open: Vec<String>,
    /// Held for the document's lifetime; dropping it releases the lock.
    _lock: LockGuard,
}

impl Document {
    /// Creates a new project in folder `dir` (created if missing), see
    /// [`Document::create_in`].
    pub fn create(dir: &Path, identity: &Identity) -> Result<Self> {
        Self::create_in(Box::new(FsStorage::new(dir)), identity)
    }

    /// Opens the project in folder `dir`, see [`Document::open_in`].
    pub fn open(dir: &Path, identity: &Identity) -> Result<Self> {
        Self::open_in(Box::new(FsStorage::new(dir)), identity)
    }

    /// Opens the project in folder `dir`, recovering damaged own logs, see
    /// [`Document::open_in_at_last_good`].
    pub fn open_at_last_good(dir: &Path, identity: &Identity) -> Result<Self> {
        Self::open_in_at_last_good(Box::new(FsStorage::new(dir)), identity)
    }

    /// Creates a new project in `storage`, edited by `identity`.
    /// Fails with [`Error::AlreadyExists`] if `storage` already holds a
    /// project (of any format).
    pub fn create_in(storage: Box<dyn Storage>, identity: &Identity) -> Result<Self> {
        let device = &identity.device;
        storage.create_dir("")?;
        // Lock before the check, so two concurrent creates can't both pass it.
        let lock = storage.lock(&lock_path(device))?;
        if storage.read(PROJECT)?.is_some() || has_format1(&*storage)? {
            return Err(Error::AlreadyExists(storage.location()));
        }
        storage.create_dir(MAIN)?;
        storage.create_dir(CHECKPOINTS)?;
        let doc = new_loro_doc();
        storage.write_atomic(&snapshot_path(device), &export_snapshot(&doc)?)?;
        let mut log = storage.open_log(&log_path(device))?;
        log.truncate(0)?;
        storage.write_atomic(&checkpoints_path(device), b"[]")?;
        // Written last: `project.json` marks the folder as a complete project.
        write_project_file(&*storage)?;
        let saved_vv = doc.oplog_vv();
        Ok(Self::from_parts(
            storage,
            identity,
            doc,
            log,
            saved_vv,
            Vec::new(),
            None,
            lock,
        ))
    }

    /// Opens the project in `storage` as `identity`: converts a format-1
    /// project first, loads this device's files (repairing a torn log tail),
    /// then imports what the other devices wrote (see the module docs).
    /// Named checkpoints in the own checkpoint file (format 1, or written
    /// before they were shared) then move into the `checkpoints` map, the one
    /// case where opening writes.
    /// Fails with [`Error::Locked`] if the device has the project open already,
    /// with [`Error::UnsupportedFormat`] if this version can't read it, with
    /// [`Error::Damaged`] if Loro rejects a record of an own log, and with
    /// [`Error::Corrupt`] naming the file if it rejects an own snapshot;
    /// those two change nothing on disk.
    pub fn open_in(storage: Box<dyn Storage>, identity: &Identity) -> Result<Self> {
        Self::open_with(storage, identity, false)
    }

    /// [`Document::open_in`], but a damaged own log (where `open_in` fails
    /// with [`Error::Damaged`]) is recovered: first copied byte for byte to
    /// `backup/damaged/`, then cut back to its last good record like a torn
    /// tail, dropping the damaged record and all after it. What was cut is in
    /// [`Document::recovered`].
    pub fn open_in_at_last_good(storage: Box<dyn Storage>, identity: &Identity) -> Result<Self> {
        Self::open_with(storage, identity, true)
    }

    /// The own logs opening cut back (see [`Document::open_at_last_good`]).
    pub fn recovered(&self) -> &[Recovered] {
        &self.recovered
    }

    /// The other devices' data opening skipped because it can't be read
    /// (`"<main|review>: device <id>: <why>"`); their files are left alone.
    /// Torn tails, which may still be arriving, are not listed.
    pub fn skipped_on_open(&self) -> &[String] {
        &self.skipped_on_open
    }

    fn open_with(storage: Box<dyn Storage>, identity: &Identity, recover: bool) -> Result<Self> {
        let device = &identity.device;
        // Checked before locking, so a folder without a project gets no lock
        // file.
        match storage.read(PROJECT)? {
            Some(json) => check_format(&json)?,
            None if !has_format1(&*storage)? => return Err(not_found(&*storage)),
            None => {}
        }
        let lock = storage.lock(&lock_path(device))?;
        if storage.read(PROJECT)?.is_none() {
            convert_format1(&*storage, identity)?;
        }
        storage.create_dir(MAIN)?;
        storage.create_dir(CHECKPOINTS)?;

        // The own files; missing if this device never opened the project.
        // Deliberately the version of the own files alone: the next autosave
        // writes everything newer, other devices' changes included, into the
        // own log, so the own files stay self-contained.
        // Both read before anything is written, so a damaged file changes
        // nothing.
        let main = Files::read(&*storage, MAIN, device, recover)?;
        let review = Files::read(&*storage, REVIEW, device, recover)?;
        let (doc, files, main_recovered) = main.into_files(&*storage, device, true)?;
        // `review` likewise; its log isn't created before it changes.
        let (review_doc, review_files, review_recovered) =
            review.into_files(&*storage, device, false)?;
        let mut skipped = Vec::new();
        let local_reads = import_others(&*storage, MAIN, device, &doc, &mut skipped)?;
        let review_reads = import_others(&*storage, REVIEW, device, &review_doc, &mut skipped)?;

        let checkpoints = match storage.read(&checkpoints_path(device))? {
            Some(json) => parse_checkpoints(&json)?,
            None => Vec::new(),
        };
        let (named, autos) = checkpoints.into_iter().partition(|c| !c.auto);
        // Unreadable state only means nothing counts as mirrored (or synced)
        // yet.
        let state = storage
            .read(&state_path(device))?
            .and_then(|json| serde_json::from_slice::<DeviceState>(&json).ok())
            .unwrap_or_default();
        // A project shared on a server is edited as this device's user there
        // (another user's device joined through a device invite).
        let mut identity = identity.clone();
        if let Some(user) = state
            .server
            .as_ref()
            .and_then(|s| UserId::parse(&s.user).ok())
        {
            identity.user = user;
        }
        let Files { log, saved_vv, .. } = files;
        let log = log.expect("loaded with `create`");
        let mut doc = Self::from_parts(
            storage,
            &identity,
            doc,
            log,
            saved_vv,
            autos,
            state.mirror,
            lock,
        );
        doc.local_reads = local_reads;
        doc.recovered = main_recovered.into_iter().chain(review_recovered).collect();
        doc.skipped_on_open = skipped;
        doc.review = review::Review::new(review_doc, review_files, review_reads);
        if let Some(server) = &state.server {
            doc.trust = trust::Trust::from_settings(server.origins.get(MAIN));
            doc.review.trust = trust::Trust::from_settings(server.origins.get(REVIEW));
        }
        doc.sync = state.sync;
        doc.server = state.server;
        doc.move_named(named)?;
        Ok(doc)
    }

    /// Moves `named` (from the own checkpoint file) into the `checkpoints`
    /// map in one bookkeeping commit, keeping each id unless it is taken,
    /// persists that, and only then rewrites the own file without them. A
    /// crash in between leaves them in both places: the next open finds an
    /// entry with the same name, time and version in the map and just drops
    /// it from the file.
    fn move_named(&mut self, named: Vec<Checkpoint>) -> Result<()> {
        if named.is_empty() {
            return Ok(());
        }
        let present = self.named_entries();
        let mut taken = self.taken_ids();
        let mut moved: Vec<Checkpoint> = Vec::new();
        for mut cp in named {
            cp.name.get_or_insert_with(String::new);
            let same = |c: &Checkpoint| {
                (&c.name, c.created, &c.version) == (&cp.name, cp.created, &cp.version)
            };
            if present.iter().chain(&moved).any(same) {
                continue;
            }
            if !is_valid_id(cp.id.0) || taken.contains(&cp.id.0) {
                cp.id = self.new_checkpoint_id(&taken);
            }
            taken.insert(cp.id.0);
            moved.push(cp);
        }
        if !moved.is_empty() {
            self.register_peer()?;
            write_named(&self.doc, &moved, &self.identity)?;
            self.autosave()?;
        }
        self.write_checkpoints()
    }

    #[allow(clippy::too_many_arguments)]
    fn from_parts(
        storage: Box<dyn Storage>,
        identity: &Identity,
        doc: LoroDoc,
        log: Box<dyn LogFile>,
        saved_vv: VersionVector,
        autos: Vec<Checkpoint>,
        mirrored: Option<Mirrored>,
        lock: LockGuard,
    ) -> Self {
        let text = doc.get_text(CONTENT);
        let custom = doc.get_text(LAYOUT_CUSTOM);
        let bib = doc.get_text(BIBLIOGRAPHY);
        let mut undo = UndoManager::new(&doc);
        undo.set_merge_interval(UNDO_MERGE_INTERVAL_MS);
        undo.set_max_undo_steps(MAX_UNDO_STEPS);
        undo.add_exclude_origin_prefix(SYS_ORIGIN);
        Self {
            storage,
            identity: identity.clone(),
            doc,
            text,
            custom,
            bib,
            undo,
            mirrored,
            mirror_checked: None,
            files: Files {
                dir: MAIN,
                log: Some(log),
                saved_vv,
            },
            review: review::Review::empty(),
            trust: trust::Trust::default(),
            autos,
            ids_drawn: 0,
            auto_interval: DEFAULT_AUTO_CHECKPOINT_INTERVAL,
            compaction_threshold: DEFAULT_COMPACTION_THRESHOLD,
            split_next_edit: false,
            last_part: None,
            registered: None,
            local_reads: sync::Reads::new(),
            sync: None,
            server: None,
            role: None,
            recovered: Vec::new(),
            skipped_on_open: Vec::new(),
            _lock: lock,
        }
    }

    /// Follows this device's role in the project on a server (`None`: not
    /// known, or not shared, which writes everything like an owner). Rights
    /// are per document, as the server enforces them: `main` is written by
    /// owners and editors, `review` by owners, editors and reviewers;
    /// readers write nothing. While `main` is read only, every entry that
    /// changes it fails with [`Error::ReadOnly`] (undo and redo included);
    /// imports from other devices, autosaves and automatic checkpoints go
    /// on, and [`Document::merge_bibliography_mirror`] and
    /// [`Document::adopt_loose_assets`] change nothing. While `review` is
    /// read only, every review entry that changes it fails the same way
    /// (see the `review` module).
    pub fn set_role(&mut self, role: Option<Role>) {
        self.role = role;
    }

    /// Whether this device may change `main`, the document (see
    /// [`Document::set_role`]).
    pub fn is_writable(&self) -> bool {
        matches!(self.role, None | Some(Role::Owner | Role::Editor))
    }

    /// Whether this device may change `review`: add, answer and resolve
    /// review items (see [`Document::set_role`]).
    pub fn is_review_writable(&self) -> bool {
        self.role != Some(Role::Reader)
    }

    /// Fails with [`Error::ReadOnly`] while `main` is read only here.
    fn check_writable(&self) -> Result<()> {
        if self.is_writable() {
            Ok(())
        } else {
            Err(self.read_only())
        }
    }

    /// [`Error::ReadOnly`] naming this device's role.
    fn read_only(&self) -> Error {
        Error::ReadOnly(
            match self.role {
                None | Some(Role::Owner) => "Owner",
                Some(Role::Editor) => "Editor",
                Some(Role::Reviewer) => "Reviewer",
                Some(Role::Reader) => "Reader",
            }
            .into(),
        )
    }

    /// Sets the minimum interval between automatic checkpoints.
    pub fn set_auto_checkpoint_interval(&mut self, interval: Duration) {
        self.auto_interval = interval;
    }

    /// Sets the log size (bytes) above which `autosave` compacts.
    pub fn set_compaction_threshold(&mut self, bytes: u64) {
        self.compaction_threshold = bytes;
    }

    /// The full text.
    pub fn text(&self) -> String {
        self.text.to_string()
    }

    /// Length in Unicode scalar values.
    pub fn len(&self) -> usize {
        self.text.len_unicode()
    }

    /// Whether the text is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Length in UTF-16 code units.
    pub fn len_utf16(&self) -> usize {
        self.text.len_utf16()
    }

    /// Inserts `s` at Unicode scalar index `pos`.
    pub fn insert(&mut self, pos: usize, s: &str) -> Result<()> {
        self.check_writable()?;
        let len = self.len();
        if pos > len {
            return Err(Error::OutOfRange { pos, count: 0, len });
        }
        if s.is_empty() {
            return Ok(());
        }
        self.register_peer()?;
        self.text.insert(pos, s)?;
        self.commit_edit(Part::Content);
        Ok(())
    }

    /// Deletes `count` Unicode scalar values starting at `pos`.
    pub fn delete(&mut self, pos: usize, count: usize) -> Result<()> {
        self.check_writable()?;
        let len = self.len();
        if pos.checked_add(count).is_none_or(|end| end > len) {
            return Err(Error::OutOfRange { pos, count, len });
        }
        if count == 0 {
            return Ok(());
        }
        self.register_peer()?;
        self.text.delete(pos, count)?;
        self.commit_edit(Part::Content);
        Ok(())
    }

    /// Applies a change set from the editor (see [`TextChange`]) as one edit.
    /// Fails with [`Error::InvalidChange`], leaving the text untouched, if the
    /// changes are unsorted, overlap, exceed the text or split a surrogate pair.
    pub fn apply_changes(&mut self, changes: &[TextChange]) -> Result<()> {
        self.check_writable()?;
        let len = self.len_utf16();
        let mut end = 0;
        for c in changes {
            if c.from < end || c.to < c.from || c.to > len {
                return Err(Error::InvalidChange(format!(
                    "{}..{} after {end} in text of UTF-16 length {len}",
                    c.from, c.to
                )));
            }
            for pos in [c.from, c.to] {
                if !self.is_utf16_boundary(pos) {
                    return Err(Error::InvalidChange(format!(
                        "{pos} splits a surrogate pair"
                    )));
                }
            }
            end = c.to;
        }
        let changes: Vec<&TextChange> = changes
            .iter()
            .filter(|c| c.from < c.to || !c.insert.is_empty())
            .collect();
        if changes.is_empty() {
            return Ok(());
        }
        self.register_peer()?;
        // Descending, so the offsets of earlier changes stay valid.
        for c in changes.iter().rev() {
            self.text.splice_utf16(c.from, c.to - c.from, &c.insert)?;
        }
        self.commit_edit(Part::Content);
        Ok(())
    }

    /// The entries of the settings map `map`. Values that are not JSON
    /// scalars (possible only in foreign data) are left out.
    pub fn map_entries(&self, map: MapName) -> Entries {
        let LoroValue::Map(values) = self.map(map).get_value() else {
            return Entries::new();
        };
        values
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), from_loro(v)?)))
            .collect()
    }

    /// Sets (`Some`) or deletes (`None`) entries of `map` as one edit: one
    /// undo step, merged like typing with directly preceding settings edits
    /// (see [`Document::end_undo_step`]). Fails with [`Error::InvalidChange`],
    /// changing nothing, if a key is empty or too long or a value is not a
    /// JSON scalar.
    pub fn map_update(
        &mut self,
        map: MapName,
        changes: &BTreeMap<String, Option<Value>>,
    ) -> Result<()> {
        self.check_writable()?;
        let mut converted = Vec::with_capacity(changes.len());
        for (key, value) in changes {
            check_key(key)?;
            converted.push((key, value.as_ref().map(to_loro).transpose()?));
        }
        let m = self.map(map);
        // Only the entries that change.
        converted.retain(|(key, value)| {
            let current = match m.get(key) {
                Some(ValueOrContainer::Value(v)) => Some(v),
                Some(ValueOrContainer::Container(_)) => Some(LoroValue::Null),
                None => None,
            };
            current != *value
        });
        if converted.is_empty() {
            return Ok(());
        }
        self.register_peer()?;
        for (key, value) in converted {
            match value {
                Some(v) => m.insert(key, v)?,
                None => m.delete(key)?,
            }
        }
        self.commit_edit(Part::Settings);
        Ok(())
    }

    /// Replaces all entries of `map` with `entries` as one edit (see
    /// [`Document::map_update`]).
    pub fn map_replace(&mut self, map: MapName, entries: &Entries) -> Result<()> {
        let mut changes = BTreeMap::new();
        self.map(map).for_each(|k, _| {
            changes.insert(k.to_owned(), None);
        });
        for (key, value) in entries {
            changes.insert(key.clone(), Some(value.clone()));
        }
        self.map_update(map, &changes)
    }

    fn map(&self, map: MapName) -> LoroMap {
        self.doc.get_map(map.id())
    }

    /// The expert Typst code of the layout layer.
    pub fn custom_text(&self) -> String {
        self.custom.to_string()
    }

    /// Replaces the expert Typst code with `text` as one edit (see
    /// [`Document::map_update`]). Only the differing middle part is replaced.
    pub fn set_custom_text(&mut self, text: &str) -> Result<()> {
        self.check_writable()?;
        let old = self.custom_text();
        let prefix = old
            .chars()
            .zip(text.chars())
            .take_while(|(a, b)| a == b)
            .count();
        let old_rest = old.chars().count() - prefix;
        let new_rest = text.chars().count() - prefix;
        let suffix = old
            .chars()
            .rev()
            .zip(text.chars().rev())
            .take(old_rest.min(new_rest))
            .take_while(|(a, b)| a == b)
            .count();
        let delete = old_rest - suffix;
        let insert: String = text.chars().skip(prefix).take(new_rest - suffix).collect();
        if delete == 0 && insert.is_empty() {
            return Ok(());
        }
        self.register_peer()?;
        self.custom.splice(prefix, delete, &insert)?;
        self.commit_edit(Part::Settings);
        Ok(())
    }

    /// The sources (BibLaTeX).
    pub fn bibliography(&self) -> String {
        self.bib.to_string()
    }

    /// Replaces the sources with `text` (stored as in the module docs,
    /// "Bibliography") as one edit and one undo step of its own. Only the
    /// differing lines are replaced. Fails with [`Error::InvalidChange`] if
    /// `text` is longer than [`MAX_BIBLIOGRAPHY_BYTES`].
    pub fn set_bibliography(&mut self, text: &str) -> Result<()> {
        self.check_writable()?;
        let text = normalize_bib(text);
        if text.len() > MAX_BIBLIOGRAPHY_BYTES {
            return Err(too_large("the bibliography"));
        }
        if *text == self.bibliography() {
            return Ok(());
        }
        self.register_peer()?;
        replace_text(&self.bib, &text)?;
        self.split_next_edit = true;
        self.commit_edit(Part::Sources);
        self.split_next_edit = true;
        Ok(())
    }

    /// Merges an outside edit of `references.bib` into the bibliography (see
    /// the module docs, "Bibliography") and rewrites the mirror. Returns
    /// whether the bibliography changed. A missing file or one with a valid
    /// marker changes nothing, and so does a read-only project (the outside
    /// edit stays in the file). Fails with [`Error::InvalidChange`] if the
    /// file is larger than [`MAX_BIBLIOGRAPHY_BYTES`] or not UTF-8.
    pub fn merge_bibliography_mirror(&mut self) -> Result<bool> {
        if !self.is_writable() {
            return Ok(false);
        }
        let Some(bytes) = self.storage.read(REFERENCES)? else {
            return Ok(false);
        };
        let Some(text) = outside_edit(&bytes)? else {
            return Ok(false);
        };
        let current = self.bibliography();
        if text != current {
            let base = self.mirror_base();
            let at_base = match &self.mirrored {
                Some(m) => (m.crc32, m.len) == text_sum(&current),
                None => current.is_empty(),
            };
            match base {
                Some(base) if !at_base => self.merge_at(&base, &text)?,
                // Without a base, the file wins (as an undoable step).
                _ => self.set_bibliography(&text)?,
            }
        }
        self.save_log()?;
        self.update_mirror(Some(&bytes))?;
        Ok(self.bibliography() != current)
    }

    /// The version of the last mirrored text, if this document contains it.
    fn mirror_base(&self) -> Option<Frontiers> {
        let m = self.mirrored.as_ref()?;
        let base = Frontiers::decode(&from_hex(&m.version)?).ok()?;
        let vv = self.doc.oplog_vv();
        let contained = base.iter().all(|id| vv.includes_id(id));
        contained.then_some(base)
    }

    /// Three-way merge: a fork at `base` (where the bibliography was last
    /// mirrored) replaces its bibliography with `text`, and that change is
    /// imported under a new peer registered to this identity. Not undoable.
    fn merge_at(&mut self, base: &Frontiers, text: &str) -> Result<()> {
        self.doc.commit();
        let fork = self.doc.fork_at(base)?;
        // The fork's random peer must be new to this document.
        let known = self.doc.oplog_vv();
        let mut peer = fork.peer_id();
        while peer == self.doc.peer_id() || known.get(&peer).is_some() || peer == PeerID::MAX {
            peer = peer.wrapping_mul(0x9e37_79b9_7f4a_7c15).wrapping_add(1);
        }
        if peer != fork.peer_id() {
            fork.set_peer_id(peer)?;
        }
        let from = fork.oplog_vv();
        replace_text(&fork.get_text(BIBLIOGRAPHY), text)?;
        fork.commit();
        if fork.oplog_vv() == from {
            return Ok(());
        }
        let update = fork.export(ExportMode::updates(&from))?;
        self.register_peer()?;
        write_peers(&self.doc, [peer], &self.identity)?;
        import(&self.doc, &update)?;
        self.split_next_edit = true;
        Ok(())
    }

    /// The asset index, path → blob name; invalid entries are left out (see
    /// the module docs, "Assets").
    pub fn assets(&self) -> BTreeMap<String, String> {
        let LoroValue::Map(entries) = self.doc.get_map(ASSETS).get_value() else {
            return BTreeMap::new();
        };
        entries
            .iter()
            .filter_map(|(path, value)| match value {
                LoroValue::String(blob) if is_blob_name(blob) => {
                    Some((path.clone(), blob.to_string()))
                }
                _ => None,
            })
            .collect()
    }

    /// The folder markers of the asset index (keys ending in `/` with the
    /// value `true`, see the module docs, "Assets"); invalid ones are left out.
    pub fn asset_folders(&self) -> BTreeSet<String> {
        let mut folders = BTreeSet::new();
        self.doc.get_map(ASSETS).for_each(|key, value| {
            if is_folder(key) && matches!(value, ValueOrContainer::Value(LoroValue::Bool(true))) {
                folders.insert(key.to_owned());
            }
        });
        folders
    }

    /// The blob indexed under `path`, if its entry is valid.
    pub fn asset_blob(&self, path: &str) -> Option<String> {
        match self.doc.get_map(ASSETS).get(path) {
            Some(ValueOrContainer::Value(LoroValue::String(blob))) if is_blob_name(&blob) => {
                Some(blob.to_string())
            }
            _ => None,
        }
    }

    /// The bytes of the asset at `path`; `None` if it is not indexed or its
    /// blob is missing (e.g. it has not arrived from another device yet).
    pub fn asset_bytes(&self, path: &str) -> Result<Option<Vec<u8>>> {
        match self.asset_blob(path) {
            Some(blob) => self.storage.read(&blob_path(&blob)),
            None => Ok(None),
        }
    }

    /// The byte size of each indexed asset's blob ([`Document::assets`]),
    /// `None` where the blob is missing; the blobs are not read.
    pub fn asset_sizes(&self) -> Result<BTreeMap<String, Option<u64>>> {
        self.assets()
            .into_iter()
            .map(|(path, blob)| Ok((path, self.storage.size(&blob_path(&blob))?)))
            .collect()
    }

    /// How often the text uses each indexed asset ([`Document::assets`]):
    /// the number of exact string literals `"<path>"` in the content text
    /// (not in the expert code).
    pub fn asset_usage(&self) -> BTreeMap<String, usize> {
        let mut usage: BTreeMap<String, usize> =
            self.assets().into_keys().map(|path| (path, 0)).collect();
        let text = self.text();
        for (start, end) in literals(&text, |s| usage.contains_key(s)) {
            if let Some(n) = usage.get_mut(&text[start..end]) {
                *n += 1;
            }
        }
        usage
    }

    /// The blob name `bytes` get in this project; `None` while there is no
    /// project key (then no blob exists yet). Compared with
    /// [`Document::assets`], it finds paths that hold the same content.
    pub fn asset_blob_name(&self, bytes: &[u8]) -> Option<String> {
        self.asset_key().map(|key| blob_name(&key, bytes))
    }

    /// Stores `bytes` as a blob and indexes it under `path` (the path the
    /// text uses), with the marker of its folder if that has none, as one
    /// edit and one undo step of its own. Putting the content `path` already
    /// has changes nothing. Fails with [`Error::InvalidChange`] if `path` is
    /// empty, too long or a folder (ends in `/`).
    pub fn put_asset(&mut self, path: &str, bytes: &[u8]) -> Result<()> {
        self.check_writable()?;
        check_item_path(path)?;
        let blob = self.stage_blob(bytes)?;
        let index = self.asset_blob(path).as_ref() != Some(&blob);
        let folder = self.unmarked_folder(path);
        if !index && folder.is_none() {
            return Ok(());
        }
        self.register_peer()?;
        let map = self.doc.get_map(ASSETS);
        if index {
            map.insert(path, blob.as_str())?;
        }
        if let Some(folder) = folder {
            map.insert(folder, true)?;
        }
        self.split_next_edit = true;
        self.commit_edit(Part::Assets);
        self.split_next_edit = true;
        Ok(())
    }

    /// Points one reference in the text at `new_path`: replaces
    /// `from_utf16..to_utf16`, which must read `expected_old` (the old path),
    /// with `new_path`, and stores `bytes` as a blob indexed under `new_path`
    /// (with its folder's marker, as [`Document::put_asset`]), as one edit
    /// and one undo step of its own (content). The old entry stays (other
    /// references and checkpoints use it). Returns what it changed in the
    /// text. Fails with [`Error::InvalidChange`], changing nothing, if the
    /// range doesn't read `expected_old` or `new_path` is empty, too long or
    /// a folder.
    pub fn repoint_asset(
        &mut self,
        from_utf16: usize,
        to_utf16: usize,
        expected_old: &str,
        new_path: &str,
        bytes: &[u8],
    ) -> Result<Vec<TextChange>> {
        self.check_writable()?;
        check_item_path(new_path)?;
        let valid = from_utf16 <= to_utf16
            && to_utf16 <= self.len_utf16()
            && self.is_utf16_boundary(from_utf16)
            && self.is_utf16_boundary(to_utf16);
        if !valid || self.text.slice_utf16(from_utf16, to_utf16)? != expected_old {
            return Err(Error::InvalidChange(format!(
                "{from_utf16}..{to_utf16} does not read \"{expected_old}\""
            )));
        }
        let blob = self.stage_blob(bytes)?;
        let index = self.asset_blob(new_path).as_ref() != Some(&blob);
        let folder = self.unmarked_folder(new_path);
        let text = expected_old != new_path;
        if !index && folder.is_none() && !text {
            return Ok(Vec::new());
        }
        self.register_peer()?;
        let ((), changes) = self.track(|d| {
            let map = d.doc.get_map(ASSETS);
            if index {
                map.insert(new_path, blob.as_str())?;
            }
            if let Some(folder) = folder {
                map.insert(folder, true)?;
            }
            if text {
                d.text
                    .splice_utf16(from_utf16, to_utf16 - from_utf16, new_path)?;
            }
            d.split_next_edit = true;
            d.commit_edit(Part::Content);
            Ok(())
        })?;
        self.split_next_edit = true;
        Ok(changes)
    }

    /// Inserts `text` at UTF-16 offset `pos_utf16` and indexes each `(path,
    /// blob)` of `entries` (blobs staged with [`Document::stage_blob`]), with
    /// the markers of their folders if they have none, as one edit and one
    /// undo step of its own (content). Entries the index holds already change
    /// nothing. Returns what it changed in the text. Fails with
    /// [`Error::InvalidChange`], changing nothing, if the position is not in
    /// the text or splits a surrogate pair, a path is empty, too long or a
    /// folder, a blob is not stored, or a path is indexed (or named here)
    /// with another blob.
    pub fn insert_with_assets(
        &mut self,
        pos_utf16: usize,
        text: &str,
        entries: &[(String, String)],
    ) -> Result<Vec<TextChange>> {
        self.check_writable()?;
        if pos_utf16 > self.len_utf16() || !self.is_utf16_boundary(pos_utf16) {
            return Err(Error::InvalidChange(format!(
                "{pos_utf16} is not a position in the text"
            )));
        }
        let mut index = BTreeMap::new();
        let mut folders = BTreeSet::new();
        for (path, blob) in entries {
            check_item_path(path)?;
            if !is_blob_name(blob) || self.storage.size(&blob_path(blob))?.is_none() {
                return Err(Error::InvalidChange(format!("blob {blob} is not stored")));
            }
            let other = |b: &String| b != blob;
            if self.asset_blob(path).is_some_and(|b| other(&b))
                || index.get(path.as_str()).is_some_and(other)
            {
                return Err(Error::InvalidChange(format!(
                    "\"{path}\" holds other content"
                )));
            }
            if self.asset_blob(path).is_none() {
                index.insert(path.as_str(), blob.clone());
            }
            folders.extend(self.unmarked_folder(path));
        }
        if text.is_empty() && index.is_empty() && folders.is_empty() {
            return Ok(Vec::new());
        }
        self.register_peer()?;
        let ((), changes) = self.track(|d| {
            let map = d.doc.get_map(ASSETS);
            for (path, blob) in &index {
                map.insert(path, blob.as_str())?;
            }
            for folder in &folders {
                map.insert(folder, true)?;
            }
            if !text.is_empty() {
                d.text.splice_utf16(pos_utf16, 0, text)?;
            }
            d.split_next_edit = true;
            d.commit_edit(Part::Content);
            Ok(())
        })?;
        self.split_next_edit = true;
        Ok(changes)
    }

    /// Writes the marker of the (empty) folder `folder`, a path ending in
    /// `/`, as one edit and one undo step of its own; an existing marker
    /// changes nothing. Fails with [`Error::InvalidChange`] if `folder` is
    /// empty, too long or doesn't end in `/`.
    pub fn add_asset_folder(&mut self, folder: &str) -> Result<()> {
        self.check_writable()?;
        check_folder_path(folder)?;
        if self.asset_entry(folder).is_some() {
            return Ok(());
        }
        self.register_peer()?;
        self.doc.get_map(ASSETS).insert(folder, true)?;
        self.split_next_edit = true;
        self.commit_edit(Part::Assets);
        self.split_next_edit = true;
        Ok(())
    }

    /// Renames index entries, items and folder markers alike: each `(old,
    /// new)` pair moves the entry `old` to `new`, and every exact string
    /// literal `"old"` in the content text becomes `"new"`. Moved items get
    /// their folder's marker if it has none. One edit and one undo step of
    /// its own (content); returns what it changed in the text. Fails with
    /// [`Error::InvalidChange`], changing nothing, if a source is not in the
    /// index or named twice, a target is invalid ([`Document::put_asset`]),
    /// already exists (and doesn't move away itself) or is named twice, an
    /// item would become a folder (or the other way round), or a folder would
    /// move into itself.
    pub fn move_assets(&mut self, moves: &[(String, String)]) -> Result<Vec<TextChange>> {
        self.check_writable()?;
        let moves: Vec<(&str, &str)> = moves
            .iter()
            .map(|(old, new)| (old.as_str(), new.as_str()))
            .filter(|(old, new)| old != new)
            .collect();
        let renames: HashMap<&str, &str> = moves.iter().copied().collect();
        if renames.len() < moves.len() {
            return Err(Error::InvalidChange("an asset is moved twice".into()));
        }
        let map = self.doc.get_map(ASSETS);
        let mut targets = BTreeSet::new();
        let mut values = Vec::with_capacity(moves.len());
        for &(old, new) in &moves {
            check_asset_path(new)?;
            let value = self
                .asset_entry(old)
                .ok_or_else(|| Error::InvalidChange(format!("\"{old}\" is not in the index")))?;
            if is_folder(old) != is_folder(new) {
                return Err(Error::InvalidChange(format!(
                    "\"{old}\" and \"{new}\" are not both folders or both files"
                )));
            }
            if is_folder(old) && new.starts_with(old) {
                return Err(Error::InvalidChange(format!(
                    "\"{old}\" can't move into itself"
                )));
            }
            let taken = map.get(new).is_some() && !renames.contains_key(new);
            if taken || !targets.insert(new) {
                return Err(Error::InvalidChange(format!("\"{new}\" exists already")));
            }
            values.push(value);
        }
        if moves.is_empty() {
            return Ok(Vec::new());
        }
        // The literals to rewrite, in UTF-16 offsets, ascending.
        let text = self.text();
        let mut edits = Vec::new();
        let (mut at, mut at16) = (0, 0);
        for (start, end) in literals(&text, |s| renames.contains_key(s)) {
            let from = at16 + text[at..start].encode_utf16().count();
            at16 = from + text[start..end].encode_utf16().count();
            at = end;
            edits.push((from, at16 - from, renames[&text[start..end]]));
        }
        self.register_peer()?;
        let ((), changes) = self.track(|d| {
            for &(old, _) in &moves {
                map.delete(old)?;
            }
            for (&(_, new), value) in moves.iter().zip(values) {
                map.insert(new, value)?;
            }
            for &(_, new) in &moves {
                if let Some(folder) = d.unmarked_folder(new) {
                    map.insert(folder, true)?;
                }
            }
            for &(from, len, new) in edits.iter().rev() {
                d.text.splice_utf16(from, len, new)?;
            }
            d.split_next_edit = true;
            d.commit_edit(Part::Content);
            Ok(())
        })?;
        self.split_next_edit = true;
        Ok(changes)
    }

    /// Deletes index entries (items and folder markers) as one edit and one
    /// undo step of its own; their blobs stay. Fails with
    /// [`Error::InvalidChange`], changing nothing, if a path is not in the
    /// index or an item is used in the content text (see
    /// [`Document::asset_usage`]).
    pub fn remove_assets(&mut self, paths: &[String]) -> Result<()> {
        self.check_writable()?;
        let paths: BTreeSet<&str> = paths.iter().map(String::as_str).collect();
        if let Some(missing) = paths.iter().find(|p| self.asset_entry(p).is_none()) {
            return Err(Error::InvalidChange(format!(
                "\"{missing}\" is not in the index"
            )));
        }
        let text = self.text();
        let used = literals(&text, |s| !is_folder(s) && paths.contains(s));
        if let Some(&(start, end)) = used.first() {
            return Err(Error::InvalidChange(format!(
                "\"{}\" is used in the text",
                &text[start..end]
            )));
        }
        if paths.is_empty() {
            return Ok(());
        }
        self.register_peer()?;
        let map = self.doc.get_map(ASSETS);
        for path in paths {
            map.delete(path)?;
        }
        self.split_next_edit = true;
        self.commit_edit(Part::Assets);
        self.split_next_edit = true;
        Ok(())
    }

    /// The value of the index entry `path` if it is valid: a blob name for an
    /// item, `true` for a folder marker.
    fn asset_entry(&self, path: &str) -> Option<LoroValue> {
        if is_folder(path) {
            match self.doc.get_map(ASSETS).get(path) {
                Some(ValueOrContainer::Value(LoroValue::Bool(true))) => Some(LoroValue::Bool(true)),
                _ => None,
            }
        } else {
            self.asset_blob(path)
                .map(|blob| LoroValue::from(blob.as_str()))
        }
    }

    /// The folder of item `path` if it needs a marker and has none (see
    /// [`folder_of`]).
    fn unmarked_folder<'p>(&self, path: &'p str) -> Option<&'p str> {
        folder_of(path).filter(|folder| self.asset_entry(folder).is_none())
    }

    /// Adopts the loose files directly in `assets/` (not blobs, not temp
    /// files) whose path `assets/<name>` `accept` approves: each is stored as
    /// a blob and indexed under that path in a bookkeeping commit (not
    /// undoable), unless the path is indexed already (with other content, the
    /// index wins and the file counts as a stale copy). Everything is
    /// persisted, then the files move to `backup/assets/` (`<stem>-2.<ext>`,
    /// ... if the name is taken there). A crash in between only repeats the
    /// move on the next call. Returns the newly indexed paths. A read-only
    /// project changes nothing (the files stay).
    pub fn adopt_loose_assets(&mut self, accept: impl Fn(&str) -> bool) -> Result<Vec<String>> {
        if !self.is_writable() {
            return Ok(Vec::new());
        }
        let mut names: Vec<String> = self
            .storage
            .list(ASSETS)?
            .into_iter()
            .filter(|name| {
                let path = format!("{ASSETS}/{name}");
                !is_blob_name(name)
                    && !name.ends_with(".tmp")
                    && check_asset_path(&path).is_ok()
                    && accept(&path)
            })
            .collect();
        names.sort();
        let mut adopted = BTreeMap::new();
        let mut loose = Vec::new();
        for name in names {
            let path = format!("{ASSETS}/{name}");
            let Some(bytes) = self.storage.read(&path)? else {
                continue;
            };
            let indexed = self.asset_blob(&path);
            if indexed.is_none() {
                adopted.insert(path, self.stage_blob(&bytes)?);
            } else if indexed == self.asset_blob_name(&bytes) {
                // Brings back a blob that went missing.
                self.stage_blob(&bytes)?;
            }
            loose.push(name);
        }
        if loose.is_empty() {
            return Ok(Vec::new());
        }
        if !adopted.is_empty() {
            self.register_peer()?;
            write_bookkeeping(
                &self.doc,
                ASSETS,
                adopted
                    .iter()
                    .map(|(path, blob)| (path.clone(), LoroValue::from(blob.as_str()))),
            )?;
        }
        // Persisted before the files move.
        self.save_log()?;
        self.storage.create_dir(BACKUP_ASSETS)?;
        let mut taken: BTreeSet<String> = self.storage.list(BACKUP_ASSETS)?.into_iter().collect();
        for name in loose {
            let target = (1..)
                .map(|n| numbered(&name, n))
                .find(|t| !taken.contains(t))
                .expect("a free name");
            self.storage.rename(
                &format!("{ASSETS}/{name}"),
                &format!("{BACKUP_ASSETS}/{target}"),
            )?;
            taken.insert(target);
        }
        Ok(adopted.into_keys().collect())
    }

    /// Writes `bytes` as a blob unless it exists, creating the project key
    /// first if there is none; returns the blob name. The index is left
    /// alone: a blob nothing indexes is harmless (blobs are never deleted),
    /// so a caller may stage blobs first and index them later with
    /// [`Document::insert_with_assets`].
    pub fn stage_blob(&mut self, bytes: &[u8]) -> Result<String> {
        self.check_writable()?;
        let key = match self.asset_key() {
            Some(key) => key,
            None => self.create_asset_key()?,
        };
        let blob = blob_name(&key, bytes);
        if !self.storage.list(ASSETS)?.contains(&blob) {
            self.storage.create_dir(ASSETS)?;
            self.storage.write_atomic(&blob_path(&blob), bytes)?;
        }
        Ok(blob)
    }

    /// The project key, if the `project` map holds a valid one.
    fn asset_key(&self) -> Option<[u8; ASSET_KEY_LEN]> {
        match self.doc.get_map(PROJECT_MAP).get(ASSET_KEY) {
            Some(ValueOrContainer::Value(LoroValue::Binary(key))) => key[..].try_into().ok(),
            _ => None,
        }
    }

    /// Creates the project key in a bookkeeping commit (see the module docs).
    fn create_asset_key(&mut self) -> Result<[u8; ASSET_KEY_LEN]> {
        let mut key = [0; ASSET_KEY_LEN];
        getrandom::fill(&mut key).map_err(|e| Error::Io(io::Error::other(e.to_string())))?;
        self.register_peer()?;
        write_bookkeeping(
            &self.doc,
            PROJECT_MAP,
            [(ASSET_KEY.to_owned(), LoroValue::from(key.to_vec()))],
        )?;
        Ok(key)
    }

    /// A stable anchor at UTF-16 offset `pos_utf16`: it stays on the
    /// character there (so an anchor at a block's start stays at that start
    /// when text is inserted before it), through edits, undo/redo, checkpoints
    /// and reopening. An anchor at the end of the text stays at the end.
    /// Returned as a hex string for storage in a settings map.
    pub fn anchor_at(&self, pos_utf16: usize) -> Result<String> {
        let len = self.len_utf16();
        if pos_utf16 > len {
            return Err(Error::OutOfRange {
                pos: pos_utf16,
                count: 0,
                len,
            });
        }
        if !self.is_utf16_boundary(pos_utf16) {
            return Err(Error::InvalidChange(format!(
                "{pos_utf16} splits a surrogate pair"
            )));
        }
        let pos = self
            .text
            .convert_pos(pos_utf16, PosType::Utf16, PosType::Unicode)
            .ok_or_else(|| Error::InvalidChange(format!("bad position {pos_utf16}")))?;
        let cursor = self
            .text
            .get_cursor(pos, Side::Middle)
            .ok_or_else(|| Error::InvalidChange(format!("no anchor at {pos_utf16}")))?;
        Ok(cursor.encode().iter().map(|b| format!("{b:02x}")).collect())
    }

    /// Where `anchor` (from [`Document::anchor_at`]) is now. Fails with
    /// [`Error::InvalidChange`] if it is not an anchor into this text.
    pub fn resolve_anchor(&self, anchor: &str) -> Result<AnchorPos> {
        let bad = || Error::InvalidChange("not an anchor".into());
        if !anchor.len().is_multiple_of(2) || anchor.len() > 1024 {
            return Err(bad());
        }
        let bytes = (0..anchor.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(anchor.get(i..i + 2)?, 16).ok())
            .collect::<Option<Vec<u8>>>()
            .ok_or_else(bad)?;
        let cursor = Cursor::decode(&bytes).map_err(|_| bad())?;
        if cursor.container != self.text.id() {
            return Err(bad());
        }
        let found = self.doc.get_cursor_pos(&cursor).map_err(|_| bad())?;
        // Without the `wasm` feature, Loro's positions count Unicode scalars.
        let pos = found.current.pos.min(self.len());
        let convert = |to| {
            self.text
                .convert_pos(pos, PosType::Unicode, to)
                .ok_or_else(bad)
        };
        Ok(AnchorPos {
            pos_utf16: convert(PosType::Utf16)?,
            pos_byte: convert(PosType::Bytes)?,
            // Loro suggests an updated cursor only when the character is gone.
            detached: cursor.id.is_some() && found.update.is_some(),
        })
    }

    fn is_utf16_boundary(&self, pos: usize) -> bool {
        let t = &self.text;
        t.convert_pos(pos, PosType::Utf16, PosType::Unicode)
            .and_then(|u| t.convert_pos(u, PosType::Unicode, PosType::Utf16))
            == Some(pos)
    }

    /// Ends the current undo step, in `main`'s history and `review`'s: the
    /// next edit (or suggestion) starts a new one even if it falls within the
    /// merge interval.
    pub fn end_undo_step(&mut self) {
        self.split_next_edit = true;
        self.review.split_next_edit = true;
    }

    /// Undoes the last step and returns what it changed, or `None` if there
    /// was nothing to undo.
    pub fn undo(&mut self) -> Result<Option<Vec<TextChange>>> {
        self.check_writable()?;
        if !self.can_undo() {
            return Ok(None);
        }
        self.register_peer()?;
        let (done, changes) = self.track(|d| Ok(d.undo.undo()?))?;
        self.split_next_edit = true;
        Ok(done.then_some(changes))
    }

    /// Redoes the last undone step and returns what it changed, or `None` if
    /// there was nothing to redo.
    pub fn redo(&mut self) -> Result<Option<Vec<TextChange>>> {
        self.check_writable()?;
        if !self.can_redo() {
            return Ok(None);
        }
        self.register_peer()?;
        let (done, changes) = self.track(|d| Ok(d.undo.redo()?))?;
        self.split_next_edit = true;
        Ok(done.then_some(changes))
    }

    /// Whether there is a step to undo.
    pub fn can_undo(&self) -> bool {
        self.undo.can_undo()
    }

    /// Whether there is a step to redo.
    pub fn can_redo(&self) -> bool {
        self.undo.can_redo()
    }

    /// Runs `op` and returns its effect on the text as [`TextChange`]s.
    /// Loro's text events fire synchronously on commit and count Unicode
    /// scalars, so the text before `op` is kept to measure UTF-16 offsets.
    fn track<T>(
        &mut self,
        op: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<(T, Vec<TextChange>)> {
        self.doc.commit();
        let old = self.text();
        let deltas = Arc::new(Mutex::new(Vec::new()));
        let sink = deltas.clone();
        let sub = self.doc.subscribe(
            &self.text.id(),
            Arc::new(move |e| {
                let mut sink = sink.lock().unwrap();
                for c in e.events {
                    if let Diff::Text(d) = c.diff {
                        sink.push(d);
                    }
                }
            }),
        );
        let out = op(self);
        drop(sub);
        let out = out?;
        let changes = match deltas.lock().unwrap().as_slice() {
            [] => Vec::new(),
            [delta] => to_changes(&old, delta),
            // Not observed with loro 1.16.2 (one event per commit); stay correct.
            _ => vec![TextChange {
                from: 0,
                to: old.encode_utf16().count(),
                insert: self.text(),
            }],
        };
        Ok((out, changes))
    }

    /// Commits pending edits to `part`, starting a fresh undo step if one was
    /// requested or the previous edit changed the other part.
    fn commit_edit(&mut self, part: Part) {
        let switched = self.last_part.replace(part) != Some(part);
        if self.split_next_edit || switched {
            self.split_next_edit = false;
            self.commit_as_new_step();
        } else {
            self.doc.commit();
        }
    }

    fn commit_as_new_step(&mut self) {
        self.undo.set_merge_interval(0);
        self.doc.commit();
        self.undo.set_merge_interval(UNDO_MERGE_INTERVAL_MS);
    }

    /// Persists changes made since the last save as one log record (fsynced)
    /// per document (`main`, then `review`), compacting a log into a snapshot
    /// when it grows too large, then writes the bibliography mirror if needed
    /// (see the module docs, "Bibliography"). Does nothing if there are no
    /// unsaved changes.
    pub fn autosave(&mut self) -> Result<()> {
        self.save_log()?;
        self.review.files.save(
            &*self.storage,
            &self.identity.device,
            &self.review.doc,
            self.compaction_threshold,
        )?;
        self.update_mirror(None)
    }

    /// The `main` log part of [`Document::autosave`].
    fn save_log(&mut self) -> Result<()> {
        self.files.save(
            &*self.storage,
            &self.identity.device,
            &self.doc,
            self.compaction_threshold,
        )
    }

    /// Writes `references.bib` if the bibliography differs from what this
    /// device last mirrored, and records what it mirrored (see the module
    /// docs). `merged` is the file content `merge_bibliography_mirror` just
    /// merged: then it runs regardless, and that content may be overwritten
    /// although it has no valid marker. Only runs after `save_log`, so the
    /// recorded version is persisted.
    fn update_mirror(&mut self, merged: Option<&[u8]>) -> Result<()> {
        let vv = self.doc.oplog_vv();
        if merged.is_none() && self.mirror_checked.as_ref() == Some(&vv) {
            return Ok(());
        }
        let text = self.bibliography();
        let (crc32, len) = text_sum(&text);
        let last = self
            .mirrored
            .as_ref()
            .map_or(text_sum(""), |m| (m.crc32, m.len));
        if merged.is_some() || (crc32, len) != last {
            let write = match self.storage.read(REFERENCES)?.as_deref() {
                None => !text.is_empty(),
                Some(file) if Some(file) == merged => true,
                Some(file) => match marked_text(file) {
                    Some(mirrored) => mirrored != text.as_bytes(),
                    // An outside edit: left for `merge_bibliography_mirror`.
                    None => {
                        self.mirror_checked = Some(vv);
                        return Ok(());
                    }
                },
            };
            if write {
                let mut file = format!(
                    "{MARKER}{crc32:08x}; outside edits are merged into the Lethora project\n"
                )
                .into_bytes();
                file.extend_from_slice(text.as_bytes());
                self.storage.write_atomic(REFERENCES, &file)?;
            }
            let mirrored = Mirrored {
                crc32,
                len,
                version: to_hex(&self.doc.oplog_frontiers().encode()),
            };
            self.write_state(Some(&mirrored), self.sync.as_ref(), self.server.as_ref())?;
            self.mirrored = Some(mirrored);
        }
        self.mirror_checked = Some(vv);
        Ok(())
    }

    /// Writes the own state file with `mirror`, `sync` and `server`.
    fn write_state(
        &self,
        mirror: Option<&Mirrored>,
        sync: Option<&SyncSettings>,
        server: Option<&ServerSettings>,
    ) -> Result<()> {
        let state = DeviceState {
            mirror: mirror.cloned(),
            sync: sync.cloned(),
            server: server.cloned(),
        };
        let json = serde_json::to_vec_pretty(&state)
            .map_err(|e| Error::Corrupt(format!("serializing the device state: {e}")))?;
        self.storage.create_dir(STATE)?;
        self.storage
            .write_atomic(&state_path(&self.identity.device), &json)
    }

    /// Who edits the document (the user is the project's user for this
    /// device when the project is shared on a server).
    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    /// Where this device syncs the project, as last set with
    /// [`Document::set_sync_settings`] (kept in the own state file).
    pub fn sync_settings(&self) -> Option<&SyncSettings> {
        self.sync.as_ref()
    }

    /// Sets (`Some`) or clears (`None`) where this device syncs the project
    /// and writes the own state file. The core only keeps it; the app reads
    /// it to reopen the replica.
    pub fn set_sync_settings(&mut self, sync: Option<SyncSettings>) -> Result<()> {
        self.write_state(self.mirrored.as_ref(), sync.as_ref(), self.server.as_ref())?;
        self.sync = sync;
        Ok(())
    }

    /// Creates a named (`Some`) or automatic (`None`) checkpoint of the current
    /// version. A named one goes into the shared `checkpoints` map (not an
    /// undo step) and is autosaved right after; for an automatic one, the
    /// version is autosaved first, then the own checkpoint file is written.
    pub fn checkpoint(&mut self, name: Option<&str>, now: SystemTime) -> Result<CheckpointId> {
        if name.is_some() {
            self.check_writable()?;
        }
        self.doc.commit();
        let id = self.new_checkpoint_id(&self.taken_ids());
        let cp = Checkpoint {
            id,
            name: name.map(str::to_owned),
            auto: name.is_none(),
            created: unix_secs(now),
            version: self.doc.oplog_frontiers().encode(),
        };
        if name.is_some() {
            self.register_peer()?;
            write_named(&self.doc, &[cp], &self.identity)?;
            self.autosave()?;
            return Ok(id);
        }
        self.autosave()?;
        self.autos.push(cp);
        if let Err(e) = self.write_checkpoints() {
            self.autos.pop();
            return Err(e);
        }
        Ok(id)
    }

    /// Creates an automatic checkpoint if the auto interval has elapsed since
    /// the latest checkpoint (named or automatic; ones dated in the future are
    /// skipped) and the document changed since then, bookkeeping aside. With
    /// no checkpoints yet, any non-empty history qualifies.
    pub fn maybe_auto_checkpoint(&mut self, now: SystemTime) -> Result<Option<CheckpointId>> {
        self.doc.commit();
        let now_secs = unix_secs(now);
        let latest = self
            .checkpoints()
            .into_iter()
            .rfind(|c| c.created <= now_secs);
        let due = match latest {
            None => !self.doc.oplog_frontiers().is_empty(),
            Some(last) => {
                now_secs - last.created >= self.auto_interval.as_secs()
                    && self.changed_since(&decode_version(&last.version)?)
            }
        };
        if !due {
            return Ok(None);
        }
        self.checkpoint(None, now).map(Some)
    }

    /// All checkpoints, oldest first: the named ones whose version this
    /// document contains, and this device's automatic ones.
    pub fn checkpoints(&self) -> Vec<Checkpoint> {
        let vv = self.doc.oplog_vv();
        let contained = |c: &Checkpoint| {
            decode_version(&c.version).is_ok_and(|f| f.iter().all(|id| vv.includes_id(id)))
        };
        let mut all: Vec<Checkpoint> = self
            .named_entries()
            .into_iter()
            .filter(contained)
            .chain(self.autos.iter().cloned())
            .collect();
        all.sort_by_key(|c| (c.created, c.id.0));
        all
    }

    /// The valid entries of the `checkpoints` map, contained or not.
    fn named_entries(&self) -> Vec<Checkpoint> {
        let LoroValue::Map(entries) = self.doc.get_map(CHECKPOINTS_MAP).get_value() else {
            return Vec::new();
        };
        entries
            .iter()
            .filter_map(|(key, value)| named_entry(key, value))
            .collect()
    }

    /// Ids in use: the keys of the `checkpoints` map (valid entries or not)
    /// and this device's automatic checkpoints.
    fn taken_ids(&self) -> BTreeSet<u64> {
        let mut taken: BTreeSet<u64> = self.autos.iter().map(|c| c.id.0).collect();
        self.doc.get_map(CHECKPOINTS_MAP).for_each(|key, _| {
            if let Ok(id) = key.parse() {
                taken.insert(id);
            }
        });
        taken
    }

    /// A checkpoint id not in `taken`: the next SplitMix64 output seeded with
    /// this session's random Loro peer id, cut to 53 bits; 0 and ids in use
    /// are skipped. Unique across devices with high probability.
    fn new_checkpoint_id(&mut self, taken: &BTreeSet<u64>) -> CheckpointId {
        loop {
            self.ids_drawn += 1;
            let mut z = self
                .doc
                .peer_id()
                .wrapping_add(self.ids_drawn.wrapping_mul(0x9e37_79b9_7f4a_7c15));
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            let id = (z ^ (z >> 31)) & MAX_CHECKPOINT_ID;
            if is_valid_id(id) && !taken.contains(&id) {
                return CheckpointId(id);
            }
        }
    }

    /// Whether anything outside the bookkeeping maps changed since `version`.
    fn changed_since(&self, version: &Frontiers) -> bool {
        let Some(old) = self.doc.frontiers_to_vv(version) else {
            return true;
        };
        self.doc.oplog_vv().sub_iter(&old).any(|span| {
            let len = (span.counter.end - span.counter.start) as usize;
            self.doc
                .get_changed_containers_in(ID::new(span.peer, span.counter.start), len)
                .iter()
                .any(|cid| !is_bookkeeping(cid))
        })
    }

    /// The text as of checkpoint `id`, without changing the document.
    pub fn text_at(&self, id: CheckpointId) -> Result<String> {
        let version = self.checkpoint_version(id)?;
        let fork = self.doc.fork_at(&version)?;
        Ok(fork.get_text(CONTENT).to_string())
    }

    /// Restores checkpoint `id` non-destructively: the difference is applied
    /// as a new change, undoable in one step. History is kept. The core's
    /// bookkeeping maps (`peers`, `checkpoints`, `project`) keep their
    /// current entries, and the asset index keeps every entry (changed ones
    /// go back). Returns what it changed.
    pub fn restore(&mut self, id: CheckpointId) -> Result<Vec<TextChange>> {
        self.check_writable()?;
        let version = self.checkpoint_version(id)?;
        // What `LoroDoc::revert_to` applies, minus the bookkeeping maps and
        // index deletions. (It differs from `apply_diff` only in not skipping
        // unreachable child containers, which hold nothing visible.)
        self.doc.commit();
        let full = self.doc.diff(&self.doc.state_frontiers(), &version)?;
        let mut diff = DiffBatch::default();
        for (cid, d) in full.iter().filter(|(cid, _)| !is_bookkeeping(cid)) {
            let mut d = d.clone();
            if is_root_map(cid, ASSETS)
                && let Diff::Map(m) = &mut d
            {
                m.updated.retain(|_, value| value.is_some());
                if m.updated.is_empty() {
                    continue;
                }
            }
            let _ = diff.push(cid.clone(), d);
        }
        if diff.iter().next().is_some() {
            self.register_peer()?;
        }
        let ((), changes) = self.track(|d| {
            d.undo.set_merge_interval(0);
            let applied = d.doc.apply_diff(diff);
            d.doc.commit();
            d.undo.set_merge_interval(UNDO_MERGE_INTERVAL_MS);
            Ok(applied?)
        })?;
        self.split_next_edit = true;
        Ok(changes)
    }

    /// Who inserted the character at UTF-16 offset `pos_utf16`, from the
    /// `peers` map (see the module docs); `None` if its peer has no valid
    /// entry. Fails like [`Document::anchor_at`] on a bad position, and with
    /// [`Error::OutOfRange`] at the end of the text (no character there).
    pub fn author_at(&self, pos_utf16: usize) -> Result<Option<Authorship>> {
        let len = self.len_utf16();
        if pos_utf16 >= len {
            return Err(Error::OutOfRange {
                pos: pos_utf16,
                count: 1,
                len,
            });
        }
        if !self.is_utf16_boundary(pos_utf16) {
            return Err(Error::InvalidChange(format!(
                "{pos_utf16} splits a surrogate pair"
            )));
        }
        let pos = self
            .text
            .convert_pos(pos_utf16, PosType::Utf16, PosType::Unicode)
            .ok_or_else(|| Error::InvalidChange(format!("bad position {pos_utf16}")))?;
        // The cursor on a character carries the id of the op that inserted it.
        let Some(id) = self.text.get_cursor(pos, Side::Middle).and_then(|c| c.id) else {
            return Ok(None);
        };
        let Some((user, device)) = self.peer_author(false, id.peer).at(id.counter).cloned() else {
            return Ok(None);
        };
        let time = self
            .doc
            .get_change(id)
            .map(|c| c.timestamp)
            .filter(|&t| t > 0);
        Ok(Some(Authorship { user, device, time }))
    }

    /// Who inserted the characters from UTF-16 offset `from_utf16` to
    /// `to_utf16`, as [`Document::author_at`] per character, merged into runs
    /// of the same user and time. The range is clipped to the text and
    /// widened to whole characters.
    pub fn authors_in(&self, from_utf16: usize, to_utf16: usize) -> Vec<AuthorRun> {
        let t = &self.text;
        let len = self.len_utf16();
        if from_utf16 >= to_utf16.min(len) {
            return Vec::new();
        }
        let unicode = |pos: usize| t.convert_pos(pos.min(len), PosType::Utf16, PosType::Unicode);
        let (Some(start), Some(mut end)) = (unicode(from_utf16), unicode(to_utf16)) else {
            return Vec::new();
        };
        let utf16 = |pos: usize| t.convert_pos(pos, PosType::Unicode, PosType::Utf16);
        if utf16(end).is_some_and(|e| e < to_utf16.min(len)) {
            end += 1;
        }
        let (Some(mut at), Ok(chars)) = (utf16(start), t.slice(start, end.max(start))) else {
            return Vec::new();
        };
        // Per peer: its author, and the counters and time of the change last
        // looked up.
        let mut users: Vec<UserId> = Vec::new();
        let mut known: HashMap<PeerID, trust::PeerAuthor> = HashMap::new();
        let mut changes: HashMap<PeerID, (i32, i32, Option<i64>)> = HashMap::new();
        let mut runs: Vec<(usize, usize, Option<usize>, Option<i64>)> = Vec::new();
        for (i, c) in chars.chars().enumerate() {
            // The cursor on a character carries the id of the op that inserted it.
            let id = t.get_cursor(start + i, Side::Middle).and_then(|c| c.id);
            let (user, time) = id.map_or((None, None), |id| {
                let author = known
                    .entry(id.peer)
                    .or_insert_with(|| self.peer_author(false, id.peer));
                // A user's peers share an index.
                let user = author.at(id.counter).map(|(user, _)| {
                    users.iter().position(|u| u == user).unwrap_or_else(|| {
                        users.push(user.clone());
                        users.len() - 1
                    })
                });
                let time = match changes.get(&id.peer) {
                    Some(&(from, to, time)) if (from..to).contains(&id.counter) => time,
                    _ => {
                        let change = self.doc.get_change(id);
                        let time = change.as_ref().map(|c| c.timestamp).filter(|&t| t > 0);
                        if let Some(c) = change {
                            let to = c.id.counter.saturating_add(c.len as i32);
                            changes.insert(id.peer, (c.id.counter, to, time));
                        }
                        time
                    }
                };
                (user, time)
            });
            let next = at + c.len_utf16();
            match runs.last_mut() {
                Some(run) if run.2 == user && run.3 == time => run.1 = next,
                _ => runs.push((at, next, user, time)),
            }
            at = next;
        }
        runs.into_iter()
            .map(|(from, to, user, time)| AuthorRun {
                from,
                to,
                user: user.map(|u| users[u].clone()),
                time,
            })
            .collect()
    }

    /// Who wrote the ops of `peer` in `main` (with `review`, in `review`):
    /// in a server-shared project as the `trust` module grounds it, else as
    /// the `peers` map says (see the module docs, "Attribution").
    pub(crate) fn peer_author(&self, review: bool, peer: PeerID) -> trust::PeerAuthor {
        let (doc, trust) = if review {
            (&self.review.doc, &self.review.trust)
        } else {
            (&self.doc, &self.trust)
        };
        // Shared: kept in the state file, or synced with a role already.
        if self.server.is_some() || self.role.is_some() {
            trust.author(doc, &self.identity, peer)
        } else {
            trust::PeerAuthor {
                who: peer_entry(&doc.get_map(PEERS), peer),
                until: None,
            }
        }
    }

    /// Writes this session's `peers` entry before its first local change, in
    /// a commit of its own that the undo manager ignores (see the module
    /// docs). Does nothing once written. Every local change passes here, so
    /// it refuses too while the project is read only.
    fn register_peer(&mut self) -> Result<()> {
        self.check_writable()?;
        register(&self.doc, &mut self.registered, &self.identity)
    }

    /// The version of checkpoint `id`; [`Error::NotFound`] if it is not
    /// listed (unknown, or its history has not arrived).
    fn checkpoint_version(&self, id: CheckpointId) -> Result<Frontiers> {
        let cp = self
            .checkpoints()
            .into_iter()
            .find(|c| c.id == id)
            .ok_or_else(|| Error::NotFound(format!("checkpoint {}", id.0)))?;
        decode_version(&cp.version)
    }

    /// Writes the own checkpoint file (automatic checkpoints only).
    fn write_checkpoints(&self) -> Result<()> {
        self.storage.write_atomic(
            &checkpoints_path(&self.identity.device),
            &checkpoints_json(&self.autos)?,
        )
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        let _ = self.autosave();
    }
}

fn snapshot_path(device: &DeviceId) -> String {
    snapshot_in(MAIN, device)
}

fn log_path(device: &DeviceId) -> String {
    log_in(MAIN, device)
}

/// `<dir>/<device>.snapshot`, the snapshot of the document kept in `dir`.
fn snapshot_in(dir: &str, device: &DeviceId) -> String {
    format!("{dir}/{}{SNAPSHOT_EXT}", device.as_str())
}

/// `<dir>/<device>.log`, the log of the document kept in `dir`.
fn log_in(dir: &str, device: &DeviceId) -> String {
    format!("{dir}/{}{LOG_EXT}", device.as_str())
}

/// One Loro document's own files, `<dir>/<device>.snapshot` and
/// `<dir>/<device>.log` (see the module docs), and the version they hold.
struct Files {
    /// `main` or `review`.
    dir: &'static str,
    /// The own log; `None` until the first save if it didn't exist (and
    /// wasn't to be created) on open.
    log: Option<Box<dyn LogFile>>,
    /// The version of the own files.
    saved_vv: VersionVector,
}

/// One Loro document's own files as read on open, before anything is
/// written (see [`Files::read`]).
struct Loaded {
    dir: &'static str,
    doc: LoroDoc,
    /// The own log; `None` if missing.
    log: Option<Vec<u8>>,
    /// Where the replay stopped: the end of the last good record.
    good: usize,
    /// The replay stopped at a record Loro rejects.
    damaged: bool,
}

impl Files {
    /// Reads `device`'s own files in `dir` into a new document: the
    /// snapshot, then the log's records in order. A torn or corrupt tail ends
    /// the replay. A record that passes its crc but Loro rejects fails with
    /// [`Error::Damaged`], or with `recover` ends the replay like a torn tail
    /// (the document is then rebuilt up to it, as the rejected import may
    /// have left changes behind). A snapshot Loro rejects always fails.
    /// Writes nothing; [`Loaded::into_files`] repairs the log.
    fn read(
        storage: &dyn Storage,
        dir: &'static str,
        device: &DeviceId,
        recover: bool,
    ) -> Result<Loaded> {
        let snapshot = storage.read(&snapshot_in(dir, device))?;
        let path = log_in(dir, device);
        let log = storage.read(&path)?;
        let bytes = log.as_deref().unwrap_or_default();
        let replay = |end| replay(dir, device, snapshot.as_deref(), &bytes[..end]);
        let (mut doc, good, damaged) = replay(bytes.len())?;
        if damaged {
            if !recover {
                return Err(Error::Damaged { file: path });
            }
            doc = replay(good)?.0;
        }
        Ok(Loaded {
            dir,
            doc,
            log,
            good,
            damaged,
        })
    }

    /// Appends the changes of `doc` since the last save as one record
    /// (creating the folder and the log if needed); when the log then
    /// exceeds `threshold`, writes a full own snapshot first and empties the
    /// log after. Does nothing if there are no unsaved changes.
    fn save(
        &mut self,
        storage: &dyn Storage,
        device: &DeviceId,
        doc: &LoroDoc,
        threshold: u64,
    ) -> Result<()> {
        doc.commit();
        let vv = doc.oplog_vv();
        if vv == self.saved_vv {
            return Ok(());
        }
        let update = doc.export(ExportMode::updates(&self.saved_vv))?;
        let log = match self.log.take() {
            Some(log) => log,
            None => {
                storage.create_dir(self.dir)?;
                storage.open_log(&log_in(self.dir, device))?
            }
        };
        let log = self.log.insert(log);
        log.append(&frame(&update)?)?;
        self.saved_vv = vv;
        if log.size() > threshold {
            storage.write_atomic(&snapshot_in(self.dir, device), &export_snapshot(doc)?)?;
            log.truncate(0)?;
        }
        Ok(())
    }
}

impl Loaded {
    /// The document and its [`Files`], after repairing the log: a damaged
    /// one is first copied byte for byte to `backup/damaged/` (see
    /// [`Recovered`]), then the log is cut back to its last good record. A
    /// missing log is created only with `create`.
    fn into_files(
        self,
        storage: &dyn Storage,
        device: &DeviceId,
        create: bool,
    ) -> Result<(LoroDoc, Files, Option<Recovered>)> {
        let Loaded {
            dir,
            doc,
            log: bytes,
            good,
            damaged,
        } = self;
        let path = log_in(dir, device);
        let mut recovered = None;
        let mut log = None;
        if create || bytes.is_some() {
            let mut file = storage.open_log(&path)?;
            let len = bytes.as_ref().map_or(0, Vec::len);
            if damaged {
                let bytes = bytes.as_deref().unwrap_or_default();
                let backup = backup_damaged(storage, dir, device, bytes)?;
                recovered = Some(Recovered {
                    file: path,
                    dropped: (len - good) as u64,
                    backup,
                });
            }
            if good < len {
                file.truncate(good as u64)?;
            }
            log = Some(file);
        }
        let files = Files {
            dir,
            log,
            saved_vv: doc.oplog_vv(),
        };
        Ok((doc, files, recovered))
    }
}

/// Writes `bytes` (a damaged own log of `dir`) to
/// `backup/damaged/<dir>-<device>.log`, numbered if taken; returns its path.
fn backup_damaged(
    storage: &dyn Storage,
    dir: &str,
    device: &DeviceId,
    bytes: &[u8],
) -> Result<String> {
    storage.create_dir(BACKUP_DAMAGED)?;
    let taken = storage.list(BACKUP_DAMAGED)?;
    let name = format!("{dir}-{}{LOG_EXT}", device.as_str());
    let target = (1..)
        .map(|n| numbered(&name, n))
        .find(|t| !taken.contains(t))
        .expect("a free name");
    let path = format!("{BACKUP_DAMAGED}/{target}");
    storage.write_atomic(&path, bytes)?;
    Ok(path)
}

/// Writes the `peers` entry of `doc`'s session peer for `identity` before
/// its first local change, in a commit of its own that the undo manager
/// ignores (see the module docs), unless `registered` says it is written.
fn register(doc: &LoroDoc, registered: &mut Option<PeerID>, identity: &Identity) -> Result<()> {
    let peer = doc.peer_id();
    if *registered == Some(peer) {
        return Ok(());
    }
    let me = (identity.user.clone(), identity.device.clone());
    if peer_entry(&doc.get_map(PEERS), peer).as_ref() != Some(&me) {
        // Pending ops (none in practice) must not share this commit.
        doc.commit();
        write_peers(doc, [peer], identity)?;
    }
    *registered = Some(peer);
    Ok(())
}

fn checkpoints_path(device: &DeviceId) -> String {
    format!("{CHECKPOINTS}/{}.json", device.as_str())
}

fn lock_path(device: &DeviceId) -> String {
    format!("{}.lock", device.as_str())
}

fn state_path(device: &DeviceId) -> String {
    format!("{STATE}/{}.json", device.as_str())
}

fn too_large(what: &str) -> Error {
    Error::InvalidChange(format!(
        "{what} is larger than {} MB",
        MAX_BIBLIOGRAPHY_BYTES / (1024 * 1024)
    ))
}

/// Bibliography text as stored: no byte order mark, `\n` line breaks.
fn normalize_bib(text: &str) -> std::borrow::Cow<'_, str> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.contains("\r\n") {
        text.replace("\r\n", "\n").into()
    } else {
        text.into()
    }
}

/// Crc32 and byte length of `text`.
fn text_sum(text: &str) -> (u32, u64) {
    (crc32fast::hash(text.as_bytes()), text.len() as u64)
}

/// Replaces `t` with `text`, changing only the differing lines. A diff that
/// times out (possibly part-way) is finished by replacing everything.
fn replace_text(t: &LoroText, text: &str) -> Result<()> {
    let options = UpdateOptions {
        timeout_ms: Some(BIB_DIFF_TIMEOUT_MS),
        use_refined_diff: true,
    };
    if t.update_by_line(text, options).is_err() {
        t.delete(0, t.len_unicode())?;
        t.insert(0, text)?;
    }
    Ok(())
}

/// The text after a valid marker line (one whose crc32 matches it), i.e.
/// what a Lethora mirror holds; `None` if `file` has no valid marker.
fn marked_text(file: &[u8]) -> Option<&[u8]> {
    let rest = file.strip_prefix(MARKER.as_bytes())?;
    let crc = u32::from_str_radix(std::str::from_utf8(rest.get(..8)?).ok()?, 16).ok()?;
    let text = &rest[rest.iter().position(|&b| b == b'\n')? + 1..];
    (crc32fast::hash(text) == crc).then_some(text)
}

/// The text of `references.bib` if it is an outside edit (no valid marker),
/// stored as in [`normalize_bib`] and without a stale marker line.
fn outside_edit(file: &[u8]) -> Result<Option<String>> {
    if file.len() > MAX_BIBLIOGRAPHY_BYTES {
        return Err(too_large(REFERENCES));
    }
    if marked_text(file).is_some() {
        return Ok(None);
    }
    let text = std::str::from_utf8(file)
        .map_err(|_| Error::InvalidChange(format!("{REFERENCES} is not UTF-8 text")))?;
    let mut text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.starts_with(MARKER) {
        text = text.split_once('\n').map_or("", |(_, rest)| rest);
    }
    Ok(Some(normalize_bib(text).into_owned()))
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

fn not_found(storage: &dyn Storage) -> Error {
    Error::NotFound(format!("no project in {}", storage.location().display()))
}

/// Whether `storage` holds a format-1 project.
fn has_format1(storage: &dyn Storage) -> Result<bool> {
    Ok(storage.list("")?.iter().any(|n| n == FORMAT1_SNAPSHOT))
}

/// Accepts `project.json` of the format this version writes.
fn check_format(json: &[u8]) -> Result<()> {
    #[derive(Deserialize)]
    struct ProjectFile {
        format: u64,
    }
    match serde_json::from_slice::<ProjectFile>(json) {
        Ok(p) if p.format == FORMAT => Ok(()),
        Ok(p) if p.format > FORMAT => Err(Error::UnsupportedFormat(format!(
            "format {} was made by a newer Lethora; update Lethora to open it",
            p.format
        ))),
        _ => Err(Error::UnsupportedFormat(format!(
            "{PROJECT} is unreadable or names an unknown format"
        ))),
    }
}

fn write_project_file(storage: &dyn Storage) -> Result<()> {
    storage.write_atomic(PROJECT, format!("{{\"format\": {FORMAT}}}").as_bytes())
}

fn parse_checkpoints(json: &[u8]) -> Result<Vec<Checkpoint>> {
    serde_json::from_slice(json).map_err(|e| Error::Corrupt(format!("checkpoints: {e}")))
}

fn checkpoints_json(checkpoints: &[Checkpoint]) -> Result<Vec<u8>> {
    serde_json::to_vec_pretty(checkpoints)
        .map_err(|e| Error::Corrupt(format!("serializing checkpoints: {e}")))
}

/// A Loro document that records the time of each change (Loro does not
/// persist this setting, so every document needs it).
fn new_loro_doc() -> LoroDoc {
    let doc = LoroDoc::new();
    doc.set_record_timestamp(true);
    doc
}

/// Imports the other devices' files in `dir` (`main` or `review`) into
/// `doc`, read-only, in name order and
/// each device's snapshot before its log. Their content never fails the open:
/// a torn or corrupt tail ends a log, anything Loro rejects (or a read error)
/// skips the rest of that device, and changes whose history is missing wait
/// inside Loro until it arrives. Files not named `<device>.snapshot|log` are
/// ignored. Returns what was read, for [`Document::pull_local`]; what was
/// skipped goes to `skipped` (see [`Document::skipped_on_open`]).
fn import_others(
    storage: &dyn Storage,
    dir: &str,
    own: &DeviceId,
    doc: &LoroDoc,
    skipped: &mut Vec<String>,
) -> Result<sync::Reads> {
    let (reads, others) = sync::read_others(storage, dir, Some(own), doc, None)?;
    skipped.extend(others.into_iter().map(|s| format!("{dir}: {s}")));
    Ok(reads)
}

/// Converts the format-1 project in `storage` into the files of `identity`'s
/// device. Every peer of the old (single-user) history is registered to
/// `identity`. `project.json` is written last: a crash before it converts
/// again on the next open; a crash after it leaves old files, which are
/// ignored.
fn convert_format1(storage: &dyn Storage, identity: &Identity) -> Result<()> {
    let device = &identity.device;
    let Some(snapshot) = storage.read(FORMAT1_SNAPSHOT)? else {
        // Another device converted it meanwhile, or it was removed.
        return match storage.read(PROJECT)? {
            Some(json) => check_format(&json),
            None => Err(not_found(storage)),
        };
    };
    let doc = new_loro_doc();
    import(&doc, &snapshot)?;
    // A torn tail ends the replay; the old log itself is left as it is.
    if let Some(log) = storage.read(FORMAT1_LOG)? {
        let mut at = 0;
        while let Some((payload, next)) = read_frame(&log, at) {
            import(&doc, payload)?;
            at = next;
        }
    }
    let checkpoints = match storage.read(FORMAT1_CHECKPOINTS)? {
        Some(json) => parse_checkpoints(&json)?,
        None => Vec::new(),
    };
    // This peer too: it writes the entries.
    let peers: Vec<PeerID> = doc.oplog_vv().keys().copied().collect();
    write_peers(&doc, peers.into_iter().chain([doc.peer_id()]), identity)?;
    storage.create_dir(MAIN)?;
    storage.create_dir(CHECKPOINTS)?;
    storage.write_atomic(&snapshot_path(device), &export_snapshot(&doc)?)?;
    storage.open_log(&log_path(device))?.truncate(0)?;
    storage.write_atomic(&checkpoints_path(device), &checkpoints_json(&checkpoints)?)?;
    write_project_file(storage)?;
    // The project is converted; failing to move the old files changes
    // nothing (on Windows, `lock` can't move while an old version holds it).
    let _ = backup_format1(storage);
    Ok(())
}

/// Moves the format-1 files (and leftover temp files) to `format1-backup/`.
fn backup_format1(storage: &dyn Storage) -> Result<()> {
    storage.create_dir(FORMAT1_BACKUP)?;
    let old = [
        FORMAT1_SNAPSHOT,
        FORMAT1_LOG,
        FORMAT1_CHECKPOINTS,
        FORMAT1_LOCK,
    ];
    for name in storage.list("")? {
        if old.contains(&name.as_str()) || name.ends_with(".tmp") {
            let _ = storage.rename(&name, &format!("{FORMAT1_BACKUP}/{name}"));
        }
    }
    Ok(())
}

/// Whether `id` is one of the core's bookkeeping maps (see [`BOOKKEEPING`]).
fn is_bookkeeping(id: &ContainerID) -> bool {
    BOOKKEEPING.iter().any(|map| is_root_map(id, map))
}

/// Whether `id` is the root map `map`.
fn is_root_map(id: &ContainerID, map: &str) -> bool {
    matches!(
        id,
        ContainerID::Root { name, container_type: ContainerType::Map } if name.as_str() == map
    )
}

/// Whether `s` is a blob name: 64 lowercase hex characters.
fn is_blob_name(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The blob name of `bytes` under project key `key` (see "Assets").
fn blob_name(key: &[u8], bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(key);
    hash.update(bytes);
    to_hex(&hash.finalize())
}

fn blob_path(blob: &str) -> String {
    format!("{ASSETS}/{blob}")
}

/// `name`, then `<stem>-2.<ext>`, `<stem>-3.<ext>`, ... for `n` = 1, 2, 3, ...
fn numbered(name: &str, n: u32) -> String {
    if n <= 1 {
        return name.to_owned();
    }
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem}-{n}.{ext}"),
        _ => format!("{name}-{n}"),
    }
}

fn check_asset_path(path: &str) -> Result<()> {
    if path.is_empty() || path.len() > MAX_KEY_LEN {
        return Err(Error::InvalidChange(format!(
            "an asset path must have 1 to {MAX_KEY_LEN} bytes"
        )));
    }
    Ok(())
}

/// [`check_asset_path`] for an item (not a folder).
fn check_item_path(path: &str) -> Result<()> {
    check_asset_path(path)?;
    if is_folder(path) {
        return Err(Error::InvalidChange(format!(
            "\"{path}\" is a folder, not a file"
        )));
    }
    Ok(())
}

/// [`check_asset_path`] for a folder.
fn check_folder_path(path: &str) -> Result<()> {
    check_asset_path(path)?;
    if !is_folder(path) {
        return Err(Error::InvalidChange(format!("\"{path}\" is not a folder")));
    }
    Ok(())
}

/// Whether index key `path` names a folder (ends in `/`, see "Assets").
fn is_folder(path: &str) -> bool {
    path.ends_with('/')
}

/// The folder holding item `path` (up to its last `/`), unless that is the
/// first component (the root), which needs no marker.
fn folder_of(path: &str) -> Option<&str> {
    let end = path.rfind('/')?;
    path[..end].contains('/').then(|| &path[..=end])
}

/// The byte ranges of the contents of the string literals in `text` that
/// `wanted` accepts, ascending and not overlapping. Every pair of
/// consecutive quotes is tried, so a wanted path (which holds no `"`) is
/// found wherever `"<path>"` stands: a scan, not a parser, so a literal in a
/// comment or raw block counts too.
fn literals(text: &str, wanted: impl Fn(&str) -> bool) -> Vec<(usize, usize)> {
    let quotes: Vec<usize> = text.match_indices('"').map(|(i, _)| i).collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i + 1 < quotes.len() {
        let (start, end) = (quotes[i] + 1, quotes[i + 1]);
        if wanted(&text[start..end]) {
            found.push((start, end));
            i += 2;
        } else {
            i += 1;
        }
    }
    found
}

/// The user and device of `peer` in the `peers` map, if its entry is valid.
fn peer_entry(peers: &LoroMap, peer: PeerID) -> Option<(UserId, DeviceId)> {
    let Some(ValueOrContainer::Value(LoroValue::Map(entry))) = peers.get(&peer.to_string()) else {
        return None;
    };
    let field = |key: &str| match entry.get(key) {
        Some(LoroValue::String(s)) => Some(s.to_string()),
        _ => None,
    };
    let user = UserId::parse(&field("user")?).ok()?;
    let device = DeviceId::parse(&field("device")?).ok()?;
    Some((user, device))
}

/// Registers `peers` to `identity` in one bookkeeping commit.
fn write_peers(
    doc: &LoroDoc,
    peers: impl IntoIterator<Item = PeerID>,
    identity: &Identity,
) -> Result<()> {
    let entry = LoroValue::from(HashMap::from([
        ("user", LoroValue::from(identity.user.as_str())),
        ("device", LoroValue::from(identity.device.as_str())),
    ]));
    write_bookkeeping(
        doc,
        PEERS,
        peers
            .into_iter()
            .map(|peer| (peer.to_string(), entry.clone())),
    )
}

/// Writes named `checkpoints`, made by `identity`, into the `checkpoints`
/// map in one bookkeeping commit (see the module docs).
fn write_named(doc: &LoroDoc, checkpoints: &[Checkpoint], identity: &Identity) -> Result<()> {
    write_bookkeeping(
        doc,
        CHECKPOINTS_MAP,
        checkpoints.iter().map(|c| {
            let created = i64::try_from(c.created).unwrap_or(i64::MAX);
            let entry = LoroValue::from(HashMap::from([
                (
                    "name",
                    LoroValue::from(c.name.as_deref().unwrap_or_default()),
                ),
                ("created", LoroValue::from(created)),
                ("version", LoroValue::from(c.version.clone())),
                ("user", LoroValue::from(identity.user.as_str())),
                ("device", LoroValue::from(identity.device.as_str())),
            ]));
            (c.id.0.to_string(), entry)
        }),
    )
}

/// Sets `entries` in the core-owned map `map` in one commit with the origin
/// [`SYS_ORIGIN`], which the undo manager ignores.
fn write_bookkeeping(
    doc: &LoroDoc,
    map: &str,
    entries: impl IntoIterator<Item = (String, LoroValue)>,
) -> Result<()> {
    let map = doc.get_map(map);
    for (key, value) in entries {
        map.insert(&key, value)?;
    }
    doc.set_next_commit_origin(SYS_ORIGIN);
    doc.commit();
    Ok(())
}

/// The named checkpoint in a `checkpoints` map entry, if the entry is valid
/// (see the module docs).
fn named_entry(key: &str, value: &LoroValue) -> Option<Checkpoint> {
    let id = key.parse::<u64>().ok()?;
    if !is_valid_id(id) || id.to_string() != key {
        return None;
    }
    let LoroValue::Map(entry) = value else {
        return None;
    };
    let (
        Some(LoroValue::String(name)),
        Some(&LoroValue::I64(created)),
        Some(LoroValue::Binary(version)),
    ) = (
        entry.get("name"),
        entry.get("created"),
        entry.get("version"),
    )
    else {
        return None;
    };
    decode_version(version).ok()?;
    Some(Checkpoint {
        id: CheckpointId(id),
        name: Some(name.to_string()),
        auto: false,
        created: u64::try_from(created).ok()?,
        version: version.to_vec(),
    })
}

/// Whether `id` is a valid checkpoint id (see [`CheckpointId`]).
fn is_valid_id(id: u64) -> bool {
    (1..=MAX_CHECKPOINT_ID).contains(&id)
}

/// Exports the full document for `snapshot.loro`: all updates rather than
/// `ExportMode::Snapshot`. In loro 1.16.2, the first edit after importing a
/// fast snapshot decodes the text state in O(spans x text length) (11 s for
/// a 6M-char text with 100k scattered edits); replaying updates takes ~130 ms.
fn export_snapshot(doc: &LoroDoc) -> Result<Vec<u8>> {
    Ok(doc.export(ExportMode::all_updates())?)
}

fn import(doc: &LoroDoc, bytes: &[u8]) -> Result<()> {
    let status = doc.import(bytes)?;
    if status.pending.is_some() {
        return Err(Error::Corrupt("update depends on missing history".into()));
    }
    Ok(())
}

/// A new document holding the own `snapshot` of `dir` and then the records
/// of `log`, up to a torn tail or a record Loro rejects. Returns it, where
/// the replay stopped, and whether it stopped at a rejected record. Fails
/// if Loro rejects the snapshot, naming its file.
fn replay(
    dir: &str,
    device: &DeviceId,
    snapshot: Option<&[u8]>,
    log: &[u8],
) -> Result<(LoroDoc, usize, bool)> {
    let doc = new_loro_doc();
    if let Some(snapshot) = snapshot {
        import(&doc, snapshot).map_err(|e| {
            Error::Corrupt(format!("{} can't be read: {e}", snapshot_in(dir, device)))
        })?;
    }
    let mut good = 0;
    while let Some((payload, next)) = read_frame(log, good) {
        if import(&doc, payload).is_err() {
            return Ok((doc, good, true));
        }
        good = next;
    }
    Ok((doc, good, false))
}

/// A log record `[u32 LE len][u32 LE crc32][payload]` holding `payload`
/// (the framing of [`lethora_protocol::frame`]).
fn frame(payload: &[u8]) -> Result<Vec<u8>> {
    lethora_protocol::frame::frame(payload)
        .ok_or_else(|| Error::Corrupt("update too large for one log record".into()))
}

/// Parses the record starting at `at`. Returns its payload and the offset of
/// the next record, or `None` if the record is short or fails its crc.
fn read_frame(buf: &[u8], at: usize) -> Option<(&[u8], usize)> {
    match parse_frame(buf, at) {
        Frame::Record(payload, next) => Some((payload, next)),
        Frame::Short | Frame::Bad => None,
    }
}

/// Converts a text delta (Unicode scalar lengths) into a change set in UTF-16
/// offsets of `old`, the text the delta applies to.
fn to_changes(old: &str, delta: &[TextDelta]) -> Vec<TextChange> {
    let mut chars = old.chars();
    let mut utf16 = |n: usize| chars.by_ref().take(n).map(char::len_utf16).sum::<usize>();
    let mut pos = 0;
    let mut changes: Vec<TextChange> = Vec::new();
    let mut push = |from: usize, to: usize, insert: &str| match changes.last_mut() {
        Some(last) if last.to == from => {
            last.to = to;
            last.insert.push_str(insert);
        }
        _ => changes.push(TextChange {
            from,
            to,
            insert: insert.to_owned(),
        }),
    };
    for d in delta {
        match d {
            TextDelta::Retain { retain, .. } => pos += utf16(*retain),
            TextDelta::Delete { delete } => {
                let end = pos + utf16(*delete);
                push(pos, end, "");
                pos = end;
            }
            TextDelta::Insert { insert, .. } => push(pos, pos, insert),
        }
    }
    changes
}

fn check_key(key: &str) -> Result<()> {
    if key.is_empty() || key.len() > MAX_KEY_LEN {
        return Err(Error::InvalidChange(format!(
            "a settings key must have 1 to {MAX_KEY_LEN} bytes"
        )));
    }
    Ok(())
}

/// A JSON scalar as a Loro value; arrays and objects are rejected.
fn to_loro(value: &Value) -> Result<LoroValue> {
    Ok(match value {
        Value::Null => LoroValue::Null,
        Value::Bool(b) => LoroValue::Bool(*b),
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => LoroValue::I64(i),
            (None, Some(f)) => LoroValue::Double(f),
            (None, None) => return Err(Error::InvalidChange(format!("unsupported number {n}"))),
        },
        Value::String(s) => LoroValue::from(s.as_str()),
        Value::Array(_) | Value::Object(_) => {
            return Err(Error::InvalidChange(
                "settings values must be null, a boolean, a number or a string".into(),
            ));
        }
    })
}

/// A Loro scalar as JSON; `None` for containers, binaries, lists and maps.
fn from_loro(value: &LoroValue) -> Option<Value> {
    Some(match value {
        LoroValue::Null => Value::Null,
        LoroValue::Bool(b) => Value::Bool(*b),
        LoroValue::I64(i) => Value::from(*i),
        LoroValue::Double(f) => serde_json::Number::from_f64(*f).map_or(Value::Null, Value::Number),
        LoroValue::String(s) => Value::String(s.to_string()),
        _ => return None,
    })
}

fn decode_version(bytes: &[u8]) -> Result<Frontiers> {
    Frontiers::decode(bytes).map_err(|e| Error::Corrupt(format!("checkpoint version: {e}")))
}

fn unix_secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}
