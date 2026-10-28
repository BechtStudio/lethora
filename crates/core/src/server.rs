//! Server sync: a project shared through `lethora-server`, end-to-end
//! encrypted. [`ServerSync`] is the client's protocol state, sans I/O: the
//! app feeds it the server's messages ([`ServerSync::handle`]), HTTP results
//! ([`ServerSync::http_result`]), connection changes and a clock, and carries
//! out what it asks for ([`ServerSync::take_actions`]): messages to send,
//! HTTP requests, "remote changes are queued" and reconnects. The document is
//! touched only in explicit calls taking it, so the app can make them under
//! its document lock: [`Document::server_push`], [`Document::server_pull`],
//! [`Document::server_save`], [`Document::server_share`] and
//! [`Document::server_join`].
//!
//! The wire messages, the server's streams and compaction are described in
//! `lethora_protocol::wire`; envelopes (version 4, signed; version 2 still
//! read) in `lethora_protocol::envelope`; the members log in
//! `lethora_protocol::members`.
//!
//! # Streams
//!
//! Both Loro documents, `main` and `review`, are synced the same way: per
//! document each device appends to its own stream on the server, with its
//! own pushed version, stream position, batch in flight, compaction and read
//! positions of the others' streams (keyed `<document>/<device>`). A
//! device's stream holds only its own changes: each push exports the ops of
//! this device's own peers since the pushed version (which peers are its
//! own, and how far, says the `trust` module), so another device's changes
//! are never uploaded again. Each record (one per push) is an envelope v4 of
//! the stream's document, kind `log`, named by the device, sealed with the
//! project key and signed by the device key, so a record of one document
//! never opens as the other's. One batch is in flight per document; batch
//! ids come from one counter for both documents and the members entries
//! (above every own stream's last batch id), so a `Nack`, which names only
//! the batch, holds up just the stream it refused. The pushed version
//! advances on its `Ack`. After a reconnect the welcome's last batch id for
//! the stream says whether the batch in flight landed: if not, the next push
//! sends what is new again. When the stream's log passes the document's
//! compaction threshold (or a push is too large for a message), the device
//! uploads a snapshot instead (kind `snapshot`), covering the stream's log as
//! it is then: a Loro update export of everything the stream holds, the
//! device's own changes (and, for the device that shared the project,
//! everything the project held then, its first snapshot), never other
//! devices' later changes. So every device's changes arrive in its own
//! stream only, which the `trust` module relies on.
//!
//! Other devices' records are checked (envelope v2 or v4 of the stream's
//! document, signed by the stream's device, a device of the members log
//! allowed to write the document as below, decrypted with the project key)
//! and queued; [`Document::server_pull`] imports the queue (never on the
//! undo stacks), so the web view can run it through its edit queue like a
//! restore, and records for the `trust` module where the new ops came from.
//! A stream compacted past what this device read is read again from its
//! snapshot (`SnapshotChanged`, fetched over HTTP). Streams of a document
//! this version doesn't sync (a later version's) are skipped without keeping
//! a read position, so a later version reads them from the start.
//!
//! # Levels
//!
//! Every envelope written is sealed at the project's level: the members
//! log's minimum (`require`) or the highest level of the records and
//! snapshots read, whichever is higher, never above what this version knows
//! ([`lethora_protocol::LEVEL`]; the reader level, lower in tests to play an
//! older client). So an older member keeps reading a project that uses no
//! newer feature. A record or snapshot this version can't read (a newer
//! envelope version, or a level above its own) is never skipped: its
//! stream's read position stays at its start (the records before it in the
//! same message are imported), nothing after it is queued, and syncing
//! stops with [`ServerState::NeedsUpdate`]: "This project now needs Lethora
//! <app> or later" (the app from the members log's `require`). The hello
//! says this version's level; a refusal for the version (`Code::Version`)
//! and a members entry only a newer version reads end the same way. Nothing
//! is retried in that state; editing goes on as offline, and the changes
//! are pushed after the update. [`ServerSync::require`] raises the minimum
//! (appending `require`; writing at the level follows once it is in the
//! log); [`ServerSync::devices_below`] lists the devices whose last hello
//! (the welcome's) was of a lower level, for a warning before.
//!
//! Blobs: before records naming them are appended, the indexed blobs this
//! device has and the server lacks (`HEAD` says) are uploaded (an envelope
//! v2, kind `asset`, document `main`, named by the blob); indexed blobs
//! missing locally are downloaded, checked and stored by the pull.
//!
//! # Roles
//!
//! A device pushes a document only while its role writes it (owners and
//! editors `main` and `review`, reviewers `review`, readers nothing; the
//! server refuses anything else with `Forbidden`). Changes made while the
//! role allowed them and not pushed when it changes stay here, unpushed
//! (the pushed version doesn't move, nothing is retried meanwhile) and go
//! out once the role allows it again; nothing is lost.
//!
//! Incoming records and snapshots of document `X` from device `D` are
//! imported only if `D` may write `X` now, or could at some point of the
//! members log and this device has not caught up (`Synced`) since `D` lost
//! that right. The server stores records only while their device may write,
//! but this device can't tell when a record was stored: a device catching
//! up after a role change (a joiner reads everything) must still take the
//! records of a demoted editor from before, or later changes building on
//! them would never apply. Once caught up, everything stored before the
//! change has arrived (a stream waiting for its snapshot is left until it
//! does), so from then on nothing more is taken from that stream until the
//! members log grants the right again (the stream names are kept in the
//! state, `revoked`). A demotion seen while online is thus enforced here
//! from the next catch-up on (the server enforces it at once). Refused data
//! is skipped and reported like data that fails its checks, its read
//! position moves on, and it never reaches the document.
//!
//! # Members
//!
//! The members log is kept in `server/members.log` in the project folder,
//! exactly as the server has it, each record checked by `Members::apply`
//! before it is stored. A server log that doesn't continue the stored one
//! (other bytes at known offsets, a gap or an invalid record) stops syncing
//! with an error. Profiles (display name and colour) are envelopes v2
//! (document `members`, kind `profile`, named by the user id) holding
//! `{"name": ..., "colour": "#rrggbb"}`, the colour one of [`PALETTE`]. A
//! user's first profile (sharing, joining) takes the first one no other
//! current member uses (a member without a profile by its
//! [`profile_colour`]), or its own [`profile_colour`] when all are taken; a
//! device joining a user keeps the user's colour. Each user can change its
//! own colour per project ([`ServerSync::set_colour`]).
//!
//! # State
//!
//! `state/<device>.json` gains `"server"` ([`ServerSettings`]): `{"url":
//! <server origin>, "project": <32 hex>, "user": <32 hex: this device's user
//! in the project>, "streams": {"<document>/<device>": {"generation",
//! "offset", "needs_snapshot"}}, "pushed": <hex: encoded Loro version
//! vector>, "own": {"generation", "length", "batch"}, "review": {"pushed",
//! "own"}, "members": <bytes of the members log>, "origins": {"<document>":
//! {"<peer>": {"device", "user", "end", "open"}}}, "revoked":
//! ["<document>/<device>"]}`: `pushed` and `own` are `main`'s own stream,
//! `review` the same for `review`; `origins` are the `trust` module's,
//! `revoked` the streams nothing more is taken from (see "Roles"). A state
//! file without `review`, `origins` or `revoked` (written before `review`
//! was synced) starts `review`'s own stream fresh. Read positions and
//! origins are those of what was imported, and [`Document::server_save`]
//! writes them only after autosaving, so they never get ahead of the saved
//! document. The project keys are the app's to keep.
//!
//! # Share and join
//!
//! Sharing creates the project on the server (hello `create` with the
//! genesis record; this device's user becomes the owner), uploads a full
//! snapshot of `main` as generation 1 (the compaction PUT with `covers` 0),
//! then one of `review` the same way if it holds anything, and the blobs,
//! and publishes the profile. Joining through an invite link says
//! hello as the invitee, reads the members log, fetches and unwraps the
//! project keys, appends the `join` (a role invite joins as the app's user, a
//! device invite as the invite's user, kept in the state above and used as
//! the identity's user whenever the project opens), then connects again as
//! the new device and builds the local project, both documents, from the
//! catch-up. Reopening ([`ServerSync::reopen`]) is a join without an invite:
//! a device that is still a member (its local copy lost) says hello as
//! itself with the key and user it kept and builds the project the same
//! way, its own earlier changes of both documents read back from its own
//! streams.
//!
//! # Members and roles
//!
//! Role changes, removals (a user, or a device: leaving is removing the own
//! device), revoking invites and changing the own colour (a new profile) are
//! members entries like the invite: each is checked against the current
//! state first (so refusals come with the protocol's reason), queued, built
//! against the head when sent and found in the log after a lost ack;
//! [`ServerSync::change_result`] says how it went.
//! The owner deletes the project over HTTP (`DELETE`, signed as
//! `lethora_protocol::wire::deletion_body` says). A removed device and a
//! deleted project stop the sync with [`ServerState::Removed`].
//!
//! # Key epochs
//!
//! After a removal (`Members::needs_rotation`), the first current device
//! that is caught up appends an `epoch` entry: a new random project key
//! sealed to every current device and open invite (the remover right after
//! its removal lands; after a leave, whoever is online first). One is in
//! flight at a time, built against the head when sent; when another device
//! was faster (`Stale`, or found in the log after a lost ack) it is dropped
//! once the log shows no removal after the latest epoch, so a removal never
//! starts two epochs. A removed device never rotates (it stops). The keyring
//! holds every epoch this device can open: the key it shared or joined with
//! (a join gets the inviter's whole keyring, see
//! `lethora_protocol::invite`), the log's epochs sealed to its invite while
//! joining and to the device afterwards, added whenever the log grows and
//! when it opens. Sealing always uses the newest epoch; envelopes open with
//! the key of their own epoch. Rotations are noted in
//! [`ServerSync::take_notes`] for the app's log.
//!
//! # Presence
//!
//! Each device publishes where its user is: the view (`author`, `science`
//! or `publisher`) and the selection as two anchors (see
//! [`Document::anchor_at`]: Loro cursors, so they follow concurrent edits),
//! set with [`Document::server_set_presence`]. It is one entry, keyed by the
//! device id, of a Loro `EphemeralStore`; the store's encoding of it is
//! sealed as an envelope v4 (document `main`, kind `presence`, named by the
//! device) and sent as `Presence`, which the server forwards to the
//! project's other connections as `PeerPresence` and never stores. Sent when
//! it changes (at most every [`PRESENCE_GAP_MS`]), again every
//! [`PRESENCE_KEEPALIVE_MS`] while online, once after catching up and soon
//! after a device appears that wasn't here. Incoming presence is taken from
//! current member devices only (checked and opened like records), and only
//! the sender's own entry of it, stamped with the time it arrived here (so
//! device clocks don't matter); entries not refreshed for
//! [`PRESENCE_TIMEOUT_MS`] go, as do all of them when the connection ends,
//! and removed devices aren't listed. [`Action::Presence`] says the list
//! changed; [`Document::server_presence`] lists the others with their
//! profile, role, view and selection in the current text. Read-only members
//! publish too.
//!
//! # Who wrote this
//!
//! [`Document::server_authors`] gives a range of the text as runs of the
//! same user ([`Document::authors_in`]: character → Loro op → its author)
//! with those users' profiles from the members log, this device's user
//! marked. A user without a profile that opens gets [`profile_colour`]. In
//! a server-shared project an op's author, in `main` and in `review` (where
//! it also decides who may edit a comment), is grounded in the stream that
//! brought it, not in the `peers` map alone (the `trust` module); one that
//! isn't has no user (unknown). If an import brings ops under this device's
//! current Loro peer that aren't its own, the document switches to a fresh
//! peer.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use lethora_protocol::envelope::{Context, Keyring, header_v2, open_v2, seal_v2};
use lethora_protocol::frame::{Frame, parse_frame};
use lethora_protocol::invite::{InviteLink, server_origin};
use lethora_protocol::keys::{DeviceKeys, KEY_LEN, Signer};
use lethora_protocol::members::{LogEnd, Members, Op, Role, entry_body};
use lethora_protocol::wire::{
    ClientVersion, Code, DeviceVersion, Hello, Message, NONCE_LEN, OwnStream, Position,
    deletion_body,
};
use loro::awareness::EphemeralStore;
use loro::{ExportMode, IdSpan, LoroDoc, LoroValue, PeerID, VersionVector};
use serde::{Deserialize, Serialize};

use crate::sync::{Reads, from_protocol, report};
use crate::trust::{PeerOrigin, Trust};
use crate::{
    AuthorRun, CHECKPOINTS, DeviceId, Document, Error, Files, Identity, LogFile, MAIN, PROJECT,
    Pulled, REVIEW, Result, Storage, UserId, blob_path, checkpoints_path, export_snapshot, frame,
    from_hex, has_format1, is_blob_name, lock_path, log_path, new_loro_doc, review, snapshot_path,
    to_hex, write_project_file,
};

/// The local copy of the members log.
const MEMBERS_LOG: &str = "server/members.log";
const SERVER_DIR: &str = "server";
/// The documents synced as streams (see the module docs, "Streams").
const DOCUMENTS: [&str; 2] = [MAIN, REVIEW];
/// How long a failed download or upload waits before it is tried again.
const RETRY_SECS: u64 = 30;
/// Largest record pushed in one message; larger pushes upload a snapshot.
const MAX_RECORD: usize = 3 * 1024 * 1024;
/// The envelope kind of presence.
const PRESENCE: &str = "presence";
/// The views a device's presence names.
const VIEWS: [&str; 3] = ["author", "science", "publisher"];
/// How long another device's presence lasts without a refresh (ms).
pub const PRESENCE_TIMEOUT_MS: u64 = 30_000;
/// The least time between two presence messages (ms).
pub const PRESENCE_GAP_MS: u64 = 100;
/// How often unchanged presence is sent again while online (ms).
pub const PRESENCE_KEEPALIVE_MS: u64 = 10_000;
/// How often others' presence is checked for timeouts (ms).
const PRESENCE_CHECK_MS: u64 = 2_000;
/// This app's version, in the hello (SemVer: the same for every crate).
const APP: &str = env!("CARGO_PKG_VERSION");
/// The level of a new project.
const NEW_LEVEL: u32 = 1;

/// Profile colours (see the module docs, "Members"): distinguishable,
/// readable on light and dark backgrounds.
pub const PALETTE: [&str; 8] = [
    "#0072b2", "#d55e00", "#009e73", "#cc79a7", "#e69f00", "#56b4e9", "#8e6c8a", "#a6761d",
];

/// The profile colour of `user` (32 hex): from [`PALETTE`] by its first byte.
pub fn profile_colour(user: &str) -> &'static str {
    let byte = user
        .get(..2)
        .and_then(|b| u8::from_str_radix(b, 16).ok())
        .unwrap_or(0);
    PALETTE[usize::from(byte) % PALETTE.len()]
}

/// A member's profile (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub colour: String,
}

/// What a stream has been read up to (see the module docs, "State").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamPos {
    pub generation: u64,
    /// Bytes of the generation's log read.
    pub offset: u64,
    /// The generation's snapshot is still needed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub needs_snapshot: bool,
}

/// This device's own stream of a document on the server.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnPos {
    pub generation: u64,
    pub length: u64,
    /// The last batch id stored.
    pub batch: u64,
}

/// This device's own stream of `review` in the state file (`main`'s are
/// [`ServerSettings::pushed`] and [`ServerSettings::own`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnState {
    /// Hex of the encoded Loro version vector the stream holds.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pushed: String,
    #[serde(default)]
    pub own: OwnPos,
}

/// Where and how this device syncs a project with a server (kept in its
/// state file, see the module docs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerSettings {
    /// The server's origin, e.g. `https://lethora.example`.
    pub url: String,
    pub project: String,
    /// This device's user in the project.
    pub user: String,
    #[serde(default)]
    pub streams: BTreeMap<String, StreamPos>,
    /// `main`'s own stream: the version it holds and where it is.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pushed: String,
    #[serde(default)]
    pub own: OwnPos,
    /// `review`'s own stream (missing in older state files: started fresh).
    #[serde(default)]
    pub review: OwnState,
    #[serde(default)]
    pub members: u64,
    /// Per document, where its Loro peers' ops came from (see the `trust`
    /// module).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub origins: BTreeMap<String, BTreeMap<String, PeerOrigin>>,
    /// Streams (`<document>/<device>`) nothing more is imported from: the
    /// device lost the right to write the document before this device last
    /// caught up (see the module docs, "Roles").
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub revoked: BTreeSet<String>,
}

/// How server sync is doing (the app adds `offline` while it can't connect).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ServerState {
    /// Saying hello or catching up.
    Connecting,
    Online,
    /// This device was removed from the project, or the project is not on
    /// the server any more (deleted).
    Removed,
    /// The project or the server needs a newer Lethora (see the module docs,
    /// "Levels"); see the message.
    NeedsUpdate,
    /// Stopped or failing; see the message.
    Error,
}

/// The status of a [`ServerSync`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerStatus {
    pub state: ServerState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
}

/// An HTTP method the client uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
    Put,
    Delete,
}

/// An HTTP request for the app to make: `path` is relative to the server's
/// origin, `token` goes into `Authorization: Bearer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub id: u64,
    pub method: Method,
    pub path: String,
    pub token: String,
    pub body: Vec<u8>,
}

/// The answer to an [`HttpRequest`]: status, body and the
/// `x-lethora-generation` header if present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub generation: Option<u64>,
}

/// What [`ServerSync`] asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Send this message.
    Send(Message),
    /// Make this request and report the result with its id.
    Http(HttpRequest),
    /// Remote changes are queued: pull them ([`Document::server_pull`]).
    RemoteChanges,
    /// Push now ([`Document::server_push`]): an ack or an upload made room.
    Push,
    /// Close the connection and connect again.
    Reconnect,
    /// Other devices' presence changed ([`Document::server_presence`]).
    Presence,
}

/// A stream: (document, device).
type Key = (String, String);

/// Why an envelope isn't imported.
#[derive(Debug)]
enum Unread {
    /// A newer version's (see the module docs, "Levels"): its stream stops.
    Newer,
    /// Damaged, forged or not openable here: skipped.
    Bad(String),
}

impl From<&str> for Unread {
    fn from(why: &str) -> Self {
        Unread::Bad(why.into())
    }
}

impl std::fmt::Display for Unread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unread::Newer => f.write_str("of a newer version of Lethora"),
            Unread::Bad(why) => f.write_str(why),
        }
    }
}

/// What an HTTP request was for.
#[derive(Debug)]
enum Purpose {
    Snapshot {
        key: Key,
        generation: u64,
    },
    BlobHead(String),
    BlobPut(String),
    BlobGet(String),
    /// A snapshot upload of `document` covering `covers` bytes of the own
    /// log; `vv` the own peers' version it holds.
    Compact {
        document: &'static str,
        covers: u64,
        vv: VersionVector,
    },
    InviteKeyPut(String),
    InviteKeyGet,
    /// Deleting the project: the id of its result.
    Delete(u64),
}

/// Something queued for the next pull from stream `key`: an update to
/// import (with the user of the stream's device), then the stream's read
/// position after it.
#[derive(Debug)]
struct Queued {
    key: Key,
    update: Option<(Vec<u8>, String)>,
    pos: Option<StreamPos>,
}

/// This device's own stream of one document (see the module docs,
/// "Streams").
#[derive(Debug, Default)]
struct Own {
    pos: OwnPos,
    /// The own peers' version the stream holds.
    pushed: VersionVector,
    /// The batch in flight and the version it brings the stream to.
    pending: Option<(u64, VersionVector)>,
    /// A snapshot upload is in flight.
    compacting: bool,
    /// The next push uploads a snapshot (sharing a `review` with content).
    snapshot_due: bool,
    /// Unix seconds before which no push goes out (after a refusal).
    push_after: u64,
}

/// A members log entry waiting to be appended; built against the head when
/// it is sent.
#[derive(Debug, Clone)]
enum MembersOp {
    Profile(Vec<u8>),
    Invite {
        link: InviteLink,
        role: Option<Role>,
        user: Option<String>,
        expires: u64,
        uses: u32,
    },
    Join {
        link: InviteLink,
        profile: Vec<u8>,
    },
    /// A role change, removal or revoke (see the module docs); `id` names
    /// its result.
    Change {
        id: u64,
        op: Op,
    },
    /// The next key epoch (see the module docs, "Key epochs"); its key is
    /// made when it is built.
    Epoch,
}

/// A member of the project, for the app (see [`ServerSync::member_list`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemberInfo {
    pub user: String,
    /// The display name, if its profile opens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub colour: String,
    pub role: Role,
    pub removed: bool,
    /// The user's current devices.
    pub devices: Vec<String>,
}

/// An invite that can still be used (see [`ServerSync::open_invites`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InviteInfo {
    pub id: String,
    /// The device that created it, and its user.
    pub creator: String,
    pub creator_user: String,
    /// The role of a role invite; `None` for a device invite.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    /// Unix seconds.
    pub expires: u64,
    pub uses_left: u32,
}

/// Another device's presence, for the app (see
/// [`Document::server_presence`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PresenceInfo {
    pub device: String,
    pub user: String,
    /// The display name, if its profile opens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub colour: String,
    pub role: Role,
    /// `author`, `science` or `publisher`.
    pub view: String,
    /// The selection as UTF-16 offsets into the current text; `None` where
    /// an anchor doesn't resolve here (e.g. in text not pulled yet).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<usize>,
}

/// Who wrote a part of the text, for the app (see
/// [`Document::server_authors`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Authors {
    pub runs: Vec<AuthorRun>,
    /// The users of the runs (unknown ones aside), by user id.
    pub authors: Vec<AuthorInfo>,
}

/// An author in [`Authors`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuthorInfo {
    pub user: String,
    /// The display name, if its profile opens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub colour: String,
    /// This device's user.
    pub you: bool,
}

/// Where an invite made here stands.
#[derive(Debug, Clone, PartialEq, Eq)]
enum InviteState {
    Pending,
    Ready(String),
    Failed(String),
}

/// What the client is doing (see the module docs).
#[derive(Debug)]
enum Phase {
    /// Sharing: hello `create` with the genesis record (until a welcome),
    /// then the first snapshot upload.
    Share {
        genesis: Vec<u8>,
        snapshot: Vec<u8>,
        vv: VersionVector,
        created: bool,
        uploading: bool,
    },
    /// Joining as the invitee of `link`: members log, key, `join`.
    Invitee { link: InviteLink, name: String },
    /// Joining: connected as the new device, catching up to build the
    /// project.
    Catchup,
    /// The project exists here: syncing.
    Device,
}

/// The client side of server sync for one project and device (see the
/// module docs).
pub struct ServerSync {
    url: String,
    project: String,
    device: String,
    user: String,
    keys: DeviceKeys,
    keyring: Option<Keyring>,
    /// The display name to publish if the members log has no profile yet.
    name: Option<String>,
    phase: Phase,
    /// The project exists on the server and here.
    established: bool,
    stopped: Option<(ServerState, String)>,
    message: Option<String>,
    // The connection.
    token: Option<String>,
    synced: bool,
    role: Option<Role>,
    /// Unix seconds, from the app.
    clock: u64,
    /// The server's clock from the last challenge (for members entries).
    server_time: u64,
    /// The highest level read ([`LEVEL`](lethora_protocol::LEVEL) but in
    /// tests), and the highest of what was read (see the module docs,
    /// "Levels").
    reader_level: u32,
    level: u32,
    /// From the last welcome: the server's version and the devices' last
    /// hellos.
    server_app: Option<String>,
    versions: Vec<DeviceVersion>,
    // The members log.
    members: Option<Members>,
    members_bytes: Vec<u8>,
    members_log: Option<Box<dyn LogFile>>,
    members_ops: VecDeque<MembersOp>,
    members_inflight: Option<(u64, MembersOp, Vec<u8>)>,
    members_stale: bool,
    /// The next batch id, for streams and members entries alike, so a
    /// `Nack` names one batch (see the module docs, "Streams").
    next_batch: u64,
    /// Streams whose device could write the document at some point of the
    /// members log, and those of them nothing more is taken from (see the
    /// module docs, "Roles").
    writers: BTreeSet<Key>,
    revoked: BTreeSet<Key>,
    invites: BTreeMap<String, InviteState>,
    /// The links of invites still pending.
    invite_links: BTreeMap<String, InviteLink>,
    /// The results of changes and the deletion by id; `None` while pending.
    changes: BTreeMap<u64, Option<std::result::Result<(), String>>>,
    next_change: u64,
    // The own streams, by document.
    own: BTreeMap<&'static str, Own>,
    // Other streams.
    received: BTreeMap<Key, StreamPos>,
    imported: BTreeMap<Key, StreamPos>,
    queue: VecDeque<Queued>,
    snapshots: BTreeSet<Key>,
    snapshot_retry: BTreeMap<Key, (u64, u64)>,
    // Blobs.
    server_blobs: BTreeSet<String>,
    blobs_inflight: BTreeSet<String>,
    blobs_to_put: BTreeSet<String>,
    failed_blobs: BTreeSet<String>,
    blob_retry: BTreeMap<String, u64>,
    missing_checked: Option<VersionVector>,
    arrived: Vec<(String, Vec<u8>)>,
    // Presence (see the module docs).
    /// Other devices' entries, stamped when they arrived.
    presence: EphemeralStore,
    /// This device's entry, and whether it changed since it was sent.
    own_presence: Option<LoroValue>,
    presence_dirty: bool,
    /// Unix ms of the last presence sent and of the last timeout check.
    presence_sent: u64,
    presence_checked: u64,
    // Requests and results.
    http: BTreeMap<u64, Purpose>,
    next_http: u64,
    actions: Vec<Action>,
    skipped: Vec<String>,
    notes: Vec<String>,
}

fn key_name(key: &Key) -> String {
    format!("{}/{}", key.0, key.1)
}

/// The streams whose device may write their document in `members` now.
fn writers_of(members: &Members) -> Vec<Key> {
    let mut keys = Vec::new();
    for user in members.users().keys() {
        for (device, _) in members.devices_of(user) {
            for document in DOCUMENTS {
                if members.may_write(device, document) {
                    keys.push((document.to_owned(), device.to_owned()));
                }
            }
        }
    }
    keys
}

fn invalid(e: lethora_protocol::Error) -> Error {
    Error::InvalidChange(e.to_string())
}

/// The bytes of a version vector, as hex.
fn vv_hex(vv: &VersionVector) -> String {
    to_hex(&vv.encode())
}

/// The version vector in hex `s`; empty if it is empty or unreadable.
fn vv_from_hex(s: &str) -> VersionVector {
    from_hex(s)
        .and_then(|b| VersionVector::decode(&b).ok())
        .unwrap_or_default()
}

/// The first [`PALETTE`] colour not in `used`; `user`'s [`profile_colour`]
/// when all are.
fn free_colour(user: &str, used: &[String]) -> &'static str {
    PALETTE
        .into_iter()
        .find(|c| !used.iter().any(|u| u == c))
        .unwrap_or_else(|| profile_colour(user))
}

/// The sealed profile of `user` named `name` in `colour`, at `level`.
#[allow(clippy::too_many_arguments)]
fn seal_profile(
    keyring: &Keyring,
    device: &str,
    keys: &DeviceKeys,
    project: &str,
    user: &str,
    name: &str,
    colour: &str,
    level: u32,
) -> Result<Vec<u8>> {
    let profile = Profile {
        name: name.to_owned(),
        colour: colour.to_owned(),
    };
    let json = serde_json::to_vec(&profile).map_err(|e| Error::Corrupt(e.to_string()))?;
    let context = Context {
        project,
        document: "members",
        kind: "profile",
        name: user,
    };
    seal_v2(keyring, device, keys, &context, level, &json).map_err(from_protocol)
}

impl ServerSync {
    #[allow(clippy::too_many_arguments)]
    fn new(
        url: String,
        project: String,
        device: &DeviceId,
        user: String,
        keys: DeviceKeys,
        keyring: Option<Keyring>,
        phase: Phase,
    ) -> Self {
        Self {
            url,
            project,
            device: device.as_str().to_owned(),
            user,
            keys,
            keyring,
            name: None,
            phase,
            established: false,
            stopped: None,
            message: None,
            token: None,
            synced: false,
            role: None,
            clock: 0,
            server_time: 0,
            reader_level: lethora_protocol::LEVEL,
            level: NEW_LEVEL,
            server_app: None,
            versions: Vec::new(),
            members: None,
            members_bytes: Vec::new(),
            members_log: None,
            members_ops: VecDeque::new(),
            members_inflight: None,
            members_stale: false,
            next_batch: 1,
            writers: BTreeSet::new(),
            revoked: BTreeSet::new(),
            invites: BTreeMap::new(),
            invite_links: BTreeMap::new(),
            changes: BTreeMap::new(),
            next_change: 1,
            own: DOCUMENTS.map(|d| (d, Own::default())).into(),
            received: BTreeMap::new(),
            imported: BTreeMap::new(),
            queue: VecDeque::new(),
            snapshots: BTreeSet::new(),
            snapshot_retry: BTreeMap::new(),
            server_blobs: BTreeSet::new(),
            blobs_inflight: BTreeSet::new(),
            blobs_to_put: BTreeSet::new(),
            failed_blobs: BTreeSet::new(),
            blob_retry: BTreeMap::new(),
            missing_checked: None,
            arrived: Vec::new(),
            presence: EphemeralStore::new(PRESENCE_TIMEOUT_MS as i64),
            own_presence: None,
            presence_dirty: false,
            presence_sent: 0,
            presence_checked: 0,
            http: BTreeMap::new(),
            next_http: 1,
            actions: Vec::new(),
            skipped: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Starts joining the project of invite link `link` (as pasted) on this
    /// device, whose user is `user` (used for a role invite) and whose
    /// display name is `name`. Drive it like any other until
    /// [`ServerSync::join_ready`], then build the project with
    /// [`Document::server_join`].
    pub fn join(
        link: &str,
        device: &DeviceId,
        keys: DeviceKeys,
        user: &UserId,
        name: &str,
    ) -> Result<Self> {
        let link = InviteLink::parse(link).map_err(invalid)?;
        let mut sync = Self::new(
            link.server.clone(),
            link.project.clone(),
            device,
            user.as_str().to_owned(),
            keys,
            None,
            Phase::Invitee {
                link,
                name: name.to_owned(),
            },
        );
        sync.name = Some(name.to_owned());
        Ok(sync)
    }

    /// Starts reopening project `project` on the server at `url` (an origin)
    /// on this device, which is still a member as `user` (both remembered
    /// from when it shared or joined) and has the project keys `keyring`: a
    /// join without an invite (see the module docs); `name` is published as
    /// the profile if the members log has none for this user. Drive it like a
    /// join until [`ServerSync::join_ready`], then build the project with
    /// [`Document::server_join`]. A device removed meanwhile stops with
    /// [`ServerState::Removed`].
    pub fn reopen(
        url: &str,
        project: &str,
        device: &DeviceId,
        keys: DeviceKeys,
        user: &UserId,
        keyring: Keyring,
        name: Option<&str>,
    ) -> Result<Self> {
        let url = server_origin(url).map_err(invalid)?;
        lethora_protocol::check_id(project, "project").map_err(invalid)?;
        let mut sync = Self::new(
            url,
            project.to_owned(),
            device,
            user.as_str().to_owned(),
            keys,
            Some(keyring),
            Phase::Catchup,
        );
        sync.name = name.map(str::to_owned);
        Ok(sync)
    }

    /// The server's origin.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The project id.
    pub fn project(&self) -> &str {
        &self.project
    }

    /// This device's user in the project.
    pub fn user(&self) -> &str {
        &self.user
    }

    /// This device's role, once known.
    pub fn role(&self) -> Option<Role> {
        self.role
    }

    /// The project keys, once known: every epoch this device can open.
    pub fn keyring(&self) -> Option<&Keyring> {
        self.keyring.as_ref()
    }

    /// Lines for the app's log since the last call (key epochs started and
    /// keys added, see the module docs).
    pub fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }

    /// Whether the project exists on the server and here.
    pub fn is_established(&self) -> bool {
        self.established
    }

    /// Whether syncing stopped for good (removed, or an error the server
    /// can't fix by reconnecting): don't connect again.
    pub fn is_stopped(&self) -> bool {
        self.stopped.is_some()
    }

    /// Whether this device's own stream is read like the others': while
    /// joining, since a device that was in the project before (its local copy
    /// lost) finds its earlier changes only there.
    fn reads_own(&self) -> bool {
        matches!(self.phase, Phase::Invitee { .. } | Phase::Catchup)
    }

    /// Whether a join has everything to build the project.
    pub fn join_ready(&self) -> bool {
        matches!(self.phase, Phase::Catchup)
            && self.synced
            && self.keyring.is_some()
            && self.snapshots.is_empty()
            && !self.received.values().any(|p| p.needs_snapshot)
    }

    /// Whether nothing is on its way: no batch, upload, download or members
    /// entry in flight, nothing queued for a pull.
    pub fn is_idle(&self) -> bool {
        self.own
            .values()
            .all(|o| o.pending.is_none() && !o.compacting)
            && self.http.is_empty()
            && self.queue.is_empty()
            && self.arrived.is_empty()
            && self.members_inflight.is_none()
            && self.members_ops.is_empty()
    }

    /// The `meta` map of what a join has queued (e.g. a title to name the new
    /// project's folder by), read from a scratch copy.
    pub fn queued_meta(&self) -> crate::Entries {
        let doc = new_loro_doc();
        for queued in self.queue.iter().filter(|q| q.key.0 == MAIN) {
            if let Some((update, _)) = &queued.update {
                let _ = doc.import(update);
            }
        }
        let loro::LoroValue::Map(values) = doc.get_map(crate::META).get_value() else {
            return crate::Entries::new();
        };
        values
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), crate::from_loro(v)?)))
            .collect()
    }

    /// The members state, once known.
    pub fn members(&self) -> Option<&Members> {
        self.members.as_ref()
    }

    /// Reads (and says hello) as a version of feature level `level` would:
    /// for tests, an older client (see the module docs, "Levels").
    pub fn set_reader_level(&mut self, level: u32) {
        self.reader_level = level;
    }

    /// The project's level, at which this device writes (see the module
    /// docs, "Levels").
    pub fn project_level(&self) -> u32 {
        let required = self
            .members
            .as_ref()
            .map_or(NEW_LEVEL, |m| m.required_level());
        required.max(self.level).min(self.reader_level)
    }

    /// The server's version, from the last welcome.
    pub fn server_version(&self) -> Option<&str> {
        self.server_app.as_deref()
    }

    /// The other current devices whose last hello (as the last welcome
    /// lists them) was below `level`, with their app version: who must
    /// update before the project needs `level`.
    pub fn devices_below(&self, level: u32) -> Vec<(String, String)> {
        let current = |device: &str| {
            self.members
                .as_ref()
                .is_some_and(|m| m.role_of_device(device).is_some())
        };
        self.versions
            .iter()
            .filter(|d| d.device != self.device && d.version.level < level && current(&d.device))
            .map(|d| (d.device.clone(), d.version.app.clone()))
            .collect()
    }

    /// Raises the project's minimum level to `level` (see the module docs,
    /// "Levels"): appends `require` with `app`, the first app version that
    /// has the level. This device writes at the level once the entry is in
    /// the log. Refused for a level this version doesn't know, or not above
    /// the current one; returns the id of its result
    /// ([`ServerSync::change_result`]).
    pub fn require(&mut self, level: u32, app: &str) -> Result<u64> {
        if level > self.reader_level {
            return Err(Error::InvalidChange(format!(
                "this version of Lethora doesn't know feature level {level}"
            )));
        }
        self.change(Op::Require {
            level,
            app: app.to_owned(),
        })
    }

    /// Stops syncing: a newer version's data or the server says so (see the
    /// module docs, "Levels").
    fn needs_update(&mut self) {
        let app = self
            .members
            .as_ref()
            .filter(|m| m.required_level() > self.reader_level)
            .map(|m| m.required_app().to_owned());
        let message = match app {
            Some(app) => format!("This project now needs Lethora {app} or later."),
            None => "This project now needs a newer Lethora.".into(),
        };
        self.stop(ServerState::NeedsUpdate, message);
    }

    /// The profile of `user`, if it has one this device can open.
    pub fn profile_of(&self, user: &str) -> Option<Profile> {
        let members = self.members.as_ref()?;
        let bytes = members.users().get(user)?.profile.as_ref()?;
        let signer = header_v2(bytes).ok()?.signer;
        let public = members.device(&signer)?.public;
        let context = Context {
            project: &self.project,
            document: "members",
            kind: "profile",
            name: user,
        };
        let json = open_v2(self.keyring.as_ref()?, &public, &context, bytes).ok()?;
        serde_json::from_slice(&json).ok()
    }

    /// The status (see [`ServerState`]).
    pub fn status(&self) -> ServerStatus {
        let (state, message) = match &self.stopped {
            Some((state, message)) => (*state, Some(message.clone())),
            None if self.established && self.synced && matches!(self.phase, Phase::Device) => {
                (ServerState::Online, self.message.clone())
            }
            None => (ServerState::Connecting, self.message.clone()),
        };
        ServerStatus {
            state,
            message,
            role: self.role,
        }
    }

    /// What to do now (see [`Action`]), oldest first.
    pub fn take_actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }

    /// Sets the clock (unix seconds) and retries what is due.
    pub fn tick(&mut self, now: u64) {
        self.clock = now;
        if self.token.is_none() {
            return;
        }
        let due: Vec<(Key, u64)> = self
            .snapshot_retry
            .iter()
            .filter(|(_, (_, at))| *at <= now)
            .map(|(key, (generation, _))| (key.clone(), *generation))
            .collect();
        for (key, generation) in due {
            self.snapshot_retry.remove(&key);
            self.get_snapshot(key, generation);
        }
    }

    /// The settings to keep in the state file: the imported positions.
    pub fn settings(&self) -> ServerSettings {
        ServerSettings {
            url: self.url.clone(),
            project: self.project.clone(),
            user: self.user.clone(),
            streams: self
                .imported
                .iter()
                .map(|(key, pos)| (key_name(key), *pos))
                .collect(),
            pushed: vv_hex(&self.own[MAIN].pushed),
            own: self.own[MAIN].pos,
            review: OwnState {
                pushed: vv_hex(&self.own[REVIEW].pushed),
                own: self.own[REVIEW].pos,
            },
            members: self.members_bytes.len() as u64,
            origins: BTreeMap::new(),
            revoked: self.revoked.iter().map(key_name).collect(),
        }
    }

    /// The transport is connected; a challenge comes next.
    pub fn connected(&mut self) {
        self.token = None;
        self.synced = false;
    }

    /// The connection ended. What was in flight on it is resolved after the
    /// next welcome. A join that was ready stays ready: its catch-up is
    /// queued, and the app closes the joining connection before building
    /// the project.
    pub fn disconnected(&mut self) {
        self.token = None;
        if !self.join_ready() {
            self.synced = false;
        }
        if let Some((_, op, _)) = self.members_inflight.take() {
            self.members_ops.push_front(op);
        }
        // Others' presence can't be refreshed now; this device's is sent
        // again once caught up.
        self.presence_sent = 0;
        if !self.presence.keys().is_empty() {
            self.presence = EphemeralStore::new(PRESENCE_TIMEOUT_MS as i64);
            self.act(Action::Presence);
        }
    }

    fn act(&mut self, action: Action) {
        self.actions.push(action);
    }

    fn send(&mut self, message: Message) {
        self.act(Action::Send(message));
    }

    fn request(&mut self, method: Method, path: String, body: Vec<u8>, purpose: Purpose) {
        let Some(token) = self.token.clone() else {
            return;
        };
        let id = self.next_http;
        self.next_http += 1;
        self.http.insert(id, purpose);
        self.act(Action::Http(HttpRequest {
            id,
            method,
            path,
            token,
            body,
        }));
    }

    /// Stops syncing for good with `state` and `message`.
    fn stop(&mut self, state: ServerState, message: impl Into<String>) {
        if self.stopped.is_none() {
            self.stopped = Some((state, message.into()));
        }
    }

    fn fail(&mut self, message: impl Into<String>) {
        self.stop(ServerState::Error, message);
    }

    /// Handles a message from the server.
    pub fn handle(&mut self, message: Message) {
        if self.stopped.is_some() {
            return;
        }
        match message {
            Message::Challenge { nonce, server_time } => self.hello(nonce, server_time),
            Message::Welcome {
                token,
                role,
                streams,
                app,
                devices,
            } => {
                (self.server_app, self.versions) = (Some(app), devices);
                self.welcome(token, role, &streams);
            }
            Message::Refused { code, message } => self.refused(code, message),
            Message::MembersRecords { offset, bytes } => self.members_records(offset, &bytes),
            Message::Records {
                document,
                device,
                generation,
                offset,
                bytes,
            } => self.records((document, device), generation, offset, &bytes),
            Message::SnapshotChanged {
                document,
                device,
                generation,
                ..
            } => self.snapshot_changed((document, device), generation),
            Message::Compacted {
                document,
                device,
                generation,
                covers,
            } => self.compacted((document, device), generation, covers),
            Message::Synced => self.caught_up(),
            Message::Ack {
                batch,
                document,
                generation,
                length,
            } => self.ack(batch, &document, generation, length),
            Message::Nack {
                batch,
                code,
                message,
            } => self.nack(batch, code, message),
            Message::MembersAck { batch, .. } => self.members_ack(batch),
            Message::Ping(value) => self.send(Message::Pong(value)),
            Message::PeerPresence { device, bytes } => self.peer_presence(&device, &bytes),
            Message::Pong(_) => {}
            Message::Hello { .. }
            | Message::Subscribe { .. }
            | Message::Append { .. }
            | Message::AppendMembers { .. }
            | Message::Presence { .. } => {
                self.message = Some("The server sent a message meant for servers.".into());
            }
        }
    }

    fn hello(&mut self, nonce: [u8; NONCE_LEN], server_time: u64) {
        self.server_time = server_time;
        let device_hello = || Hello::Device {
            project: self.project.clone(),
            device: self.device.clone(),
        };
        let client = ClientVersion {
            level: self.reader_level,
            app: APP.into(),
        };
        let signed = match &self.phase {
            Phase::Share {
                genesis,
                created: false,
                ..
            } => Hello::Create {
                genesis: genesis.clone(),
            }
            .sign(client, &self.keys, &nonce, &self.url),
            Phase::Invitee { link, .. } => Hello::Invitee {
                project: self.project.clone(),
                invite: link.id.clone(),
            }
            .sign(client, &link.keys(), &nonce, &self.url),
            _ => device_hello().sign(client, &self.keys, &nonce, &self.url),
        };
        match signed {
            Ok(message) => self.send(message),
            Err(e) => self.fail(format!("Can't say hello to the server: {e}")),
        }
    }

    fn welcome(&mut self, token: String, role: Option<Role>, streams: &[OwnStream]) {
        self.token = Some(token);
        self.role = role;
        self.message = None;
        if let Phase::Share {
            genesis, created, ..
        } = &mut self.phase
            && !*created
        {
            *created = true;
            let genesis = genesis.clone();
            if let Err(e) = self.add_members(0, &genesis) {
                self.fail(format!("The new project's members log is invalid: {e}"));
                return;
            }
        }
        if let Phase::Invitee { .. } = self.phase {
            self.send(Message::Subscribe {
                members: self.members_bytes.len() as u64,
                streams: Vec::new(),
            });
            return;
        }
        self.reconcile_own(streams);
        let reads_own = self.reads_own();
        let mut positions: Vec<Position> = self
            .received
            .iter()
            .filter(|((_, device), pos)| {
                (reads_own || *device != self.device) && !pos.needs_snapshot
            })
            .map(|((document, device), pos)| Position {
                document: document.clone(),
                device: device.clone(),
                generation: pos.generation,
                offset: pos.offset,
            })
            .collect();
        for (document, own) in &self.own {
            if !reads_own && streams.iter().any(|s| s.document == *document) {
                positions.push(Position {
                    document: (*document).into(),
                    device: self.device.clone(),
                    generation: own.pos.generation,
                    offset: own.pos.length,
                });
            }
        }
        self.send(Message::Subscribe {
            members: self.members_bytes.len() as u64,
            streams: positions,
        });
        if let Phase::Share {
            snapshot,
            vv,
            uploading,
            ..
        } = &mut self.phase
            && !*uploading
        {
            *uploading = true;
            let (snapshot, vv) = (snapshot.clone(), vv.clone());
            self.own_mut(MAIN).compacting = true;
            self.request(
                Method::Put,
                self.snapshot_path(MAIN, &self.device, Some(0)),
                snapshot,
                Purpose::Compact {
                    document: MAIN,
                    covers: 0,
                    vv,
                },
            );
        }
    }

    /// Takes the own streams' state from the welcome, resolving each batch
    /// in flight: it landed if its stream's last batch reached it.
    fn reconcile_own(&mut self, streams: &[OwnStream]) {
        for (document, own) in &mut self.own {
            let (generation, length, batch) = streams
                .iter()
                .find(|s| s.document == *document)
                .map_or((0, 0, 0), |s| (s.generation, s.length, s.batch));
            if let Some((sent, vv)) = own.pending.take()
                && batch >= sent
            {
                own.pushed.merge(&vv);
            }
            if generation < own.pos.generation
                || (generation == own.pos.generation && length < own.pos.length)
            {
                // The server lost part of this stream: push everything again.
                own.pushed = VersionVector::new();
            }
            own.pos = OwnPos {
                generation,
                length,
                batch: batch.max(own.pos.batch),
            };
        }
        self.raise_next_batch();
    }

    /// Keeps the next batch id above every own stream's last one.
    fn raise_next_batch(&mut self) {
        let last = self.own.values().map(|o| o.pos.batch).max().unwrap_or(0);
        self.next_batch = self.next_batch.max(last + 1);
    }

    /// The own stream of `document` (one of [`DOCUMENTS`]).
    fn own_mut(&mut self, document: &str) -> &mut Own {
        self.own.get_mut(document).expect("a synced document")
    }

    fn refused(&mut self, code: Code, message: String) {
        match code {
            Code::NotAMember => self.stop(
                ServerState::Removed,
                "This device is no longer a member of the project.",
            ),
            Code::InviteClosed => self.fail(
                "This invite link can't be used any more (expired, used up or revoked). Ask for a new one.",
            ),
            Code::Exists => match &mut self.phase {
                Phase::Share { created, .. } if !*created => {
                    // The project was created, but its welcome was lost.
                    *created = true;
                    self.act(Action::Reconnect);
                }
                _ => self.fail(format!("The server refused: {message}")),
            },
            Code::UnknownProject => self.stop(
                ServerState::Removed,
                "The project is not on this server any more (deleted).",
            ),
            // The server's own words: this app or the server is too old.
            Code::Version => self.stop(ServerState::NeedsUpdate, message),
            Code::Auth => self.fail(format!("The server didn't accept this device: {message}")),
            _ => self.message = Some(format!("The server refused: {message}")),
        }
    }

    /// Checks and stores members log bytes from `offset` (see the module
    /// docs, "Members").
    fn members_records(&mut self, offset: u64, bytes: &[u8]) {
        match self.add_members(offset, bytes) {
            Ok(()) => {}
            Err(lethora_protocol::Error::Newer(_)) => return self.needs_update(),
            Err(e) => {
                return self.fail(format!(
                    "The server's members log doesn't match the one on this device; syncing stopped. ({e})"
                ));
            }
        }
        if !self.members_changed() {
            return;
        }
        if self.members_stale && self.members_inflight.is_none() {
            self.members_stale = false;
            self.next_members_op();
        }
        self.maybe_rotate();
    }

    /// Queues the next key epoch if a member was removed after the latest
    /// one and this device may start it: a current device, caught up (see
    /// the module docs, "Key epochs"). Never twice at a time.
    fn maybe_rotate(&mut self) {
        let due = self.established
            && self.synced
            && matches!(self.phase, Phase::Device)
            && self
                .members
                .as_ref()
                .is_some_and(|m| m.needs_rotation() && m.role_of_device(&self.device).is_some());
        let queued = self
            .members_ops
            .iter()
            .chain(self.members_inflight.as_ref().map(|(_, op, _)| op))
            .any(|op| matches!(op, MembersOp::Epoch));
        if due && !queued {
            self.members_ops.push_back(MembersOp::Epoch);
            self.next_members_op();
        }
    }

    /// Adds the keys of the members log's epochs this device can open (sealed
    /// to it, or while joining to its invite) to the keyring.
    fn learn_keys(&mut self) {
        let (Some(members), Some(keyring)) = (&self.members, &mut self.keyring) else {
            return;
        };
        let invite = match &self.phase {
            Phase::Invitee { link, .. } => Some(link.keys()),
            _ => None,
        };
        for (&epoch, keys) in members.epochs() {
            if keyring.get(epoch).is_some() {
                continue;
            }
            let opened = if let Some(sealed) = keys.devices.get(&self.device) {
                self.keys.open_epoch_key(
                    &self.project,
                    epoch,
                    &self.device,
                    &keys.ephemeral,
                    sealed,
                )
            } else if let Some((invite, sealed)) = invite
                .as_ref()
                .and_then(|i| Some((i, keys.invites.get(i.id())?)))
            {
                invite.open_epoch_key(&self.project, epoch, &keys.ephemeral, sealed)
            } else {
                continue;
            };
            match opened {
                Ok(key) => {
                    keyring.insert(epoch, key);
                    self.notes
                        .push(format!("Got the project key of epoch {epoch}."));
                }
                Err(e) => self.notes.push(format!(
                    "The project key of epoch {epoch} doesn't open: {e}"
                )),
            }
        }
    }

    /// The members log grew: takes this device's role from it, or stops if
    /// this device was removed or the project needs a newer version (then
    /// `false`).
    fn members_changed(&mut self) -> bool {
        if self
            .members
            .as_ref()
            .is_some_and(|m| m.required_level() > self.reader_level)
        {
            self.needs_update();
            return false;
        }
        let removed = self.members.as_ref().is_some_and(|m| {
            m.device(&self.device).is_some() && m.role_of_device(&self.device).is_none()
        });
        if removed && self.established {
            self.stop(
                ServerState::Removed,
                "This device was removed from the project.",
            );
            return false;
        }
        if let Some(role) = self
            .members
            .as_ref()
            .and_then(|m| m.role_of_device(&self.device))
        {
            self.role = Some(role);
        }
        // Names and roles shown with presence may have changed, devices gone.
        if !self.presence.keys().is_empty() {
            self.act(Action::Presence);
        }
        true
    }

    /// Applies the records of `bytes` (members log bytes from `offset`) that
    /// are new here, then stores them; on an error (`Newer` for a newer
    /// version's entry) nothing.
    fn add_members(
        &mut self,
        offset: u64,
        bytes: &[u8],
    ) -> std::result::Result<(), lethora_protocol::Error> {
        let corrupt = |why: &str| lethora_protocol::Error::Corrupt(why.into());
        let have = self.members_bytes.len() as u64;
        if offset > have {
            return Err(corrupt(&format!("a gap at byte {have}")));
        }
        let overlap = usize::try_from(have - offset).unwrap_or(usize::MAX);
        let known = &self.members_bytes[offset as usize..];
        let same = bytes.len().min(overlap);
        if bytes[..same] != known[..same] {
            return Err(corrupt("other bytes where this device has some"));
        }
        if bytes.len() <= overlap {
            return Ok(());
        }
        let new = &bytes[overlap..];
        let mut at = 0;
        let mut members = self.members.clone();
        let (mut writers, mut revoked) = (self.writers.clone(), self.revoked.clone());
        while at < new.len() {
            let Frame::Record(_, next) = parse_frame(new, at) else {
                return Err(corrupt("a damaged record"));
            };
            let record = &new[at..next];
            let m = match &mut members {
                Some(m) => {
                    m.apply(record)?;
                    m
                }
                None => {
                    let m = Members::genesis(record)?;
                    if m.project() != self.project {
                        return Err(corrupt("the log is of another project"));
                    }
                    members.insert(m)
                }
            };
            for key in writers_of(m) {
                revoked.remove(&key);
                writers.insert(key);
            }
            at = next;
        }
        if let Some(log) = &mut self.members_log {
            log.append(new)
                .map_err(|e| corrupt(&format!("the members log file: {e}")))?;
        }
        self.members_bytes.extend_from_slice(new);
        self.members = members;
        (self.writers, self.revoked) = (writers, revoked);
        self.learn_keys();
        Ok(())
    }

    /// Whether records and snapshots of stream `key` are imported (see the
    /// module docs, "Roles"); the user of its device if so.
    fn importable(&self, key: &Key) -> std::result::Result<String, String> {
        let members = self.members.as_ref().ok_or("no members log yet")?;
        let device = members
            .device(&key.1)
            .ok_or("by a device the members log doesn't know")?;
        let allowed = members.may_write(&key.1, &key.0)
            || (self.writers.contains(key) && !self.revoked.contains(key));
        if !allowed {
            return Err(format!("its device may not write {}", key.0));
        }
        Ok(device.user.clone())
    }

    /// Stops taking anything more from the streams whose device can't write
    /// their document now, once caught up (see the module docs, "Roles").
    /// A stream still waiting for its snapshot is left until it arrives.
    fn revoke_lost(&mut self) {
        let Some(members) = &self.members else {
            return;
        };
        let lost: Vec<Key> = self
            .writers
            .iter()
            .filter(|key| {
                !members.may_write(&key.1, &key.0)
                    && !self.snapshots.contains(*key)
                    && !self.received.get(*key).is_some_and(|p| p.needs_snapshot)
            })
            .cloned()
            .collect();
        self.revoked.extend(lost);
    }

    fn records(&mut self, key: Key, generation: u64, offset: u64, bytes: &[u8]) {
        if key.1 == self.device && !self.reads_own() {
            return;
        }
        // A document this version doesn't sync: skipped without keeping a
        // read position, so a later version that syncs it reads its streams
        // from the start.
        if !DOCUMENTS.contains(&key.0.as_str()) {
            return;
        }
        let needs_snapshot = match self.received.get(&key) {
            Some(p) if p.generation == generation => p.needs_snapshot,
            _ => generation > 0,
        };
        let mut updates = Vec::new();
        // Where reading stopped at a newer version's record.
        let mut newer = None;
        match self.importable(&key) {
            Ok(user) => {
                let mut at = 0;
                while at < bytes.len() {
                    let Frame::Record(envelope, next) = parse_frame(bytes, at) else {
                        self.skipped
                            .push(format!("stream {}: a damaged record", key_name(&key)));
                        break;
                    };
                    match self.open(&key.0, &key.1, "log", envelope) {
                        Ok((update, level)) => {
                            self.level = self.level.max(level);
                            updates.push((update, user.clone()));
                        }
                        Err(Unread::Newer) => {
                            newer = Some(at);
                            break;
                        }
                        Err(e) => self.skipped.push(format!(
                            "stream {} at byte {}: {e}",
                            key_name(&key),
                            offset + at as u64
                        )),
                    }
                    at = next;
                }
            }
            Err(e) => self
                .skipped
                .push(format!("stream {} at byte {offset}: {e}", key_name(&key))),
        }
        let pos = StreamPos {
            generation,
            offset: offset + newer.unwrap_or(bytes.len()) as u64,
            needs_snapshot,
        };
        self.received.insert(key.clone(), pos);
        let count = updates.len();
        for (i, update) in updates.into_iter().enumerate() {
            self.queue.push_back(Queued {
                key: key.clone(),
                update: Some(update),
                pos: (i + 1 == count).then_some(pos),
            });
        }
        if count == 0 {
            self.queue_position(key, pos);
        } else {
            self.act(Action::RemoteChanges);
        }
        if newer.is_some() {
            self.needs_update();
        }
    }

    /// The payload of `envelope`, a record (`kind` `log`) or snapshot of
    /// `device`'s stream of `document` (or its presence, document `main`),
    /// after checking it (see the module docs), and the level it needs.
    fn open(
        &self,
        document: &str,
        device: &str,
        kind: &str,
        envelope: &[u8],
    ) -> std::result::Result<(Vec<u8>, u32), Unread> {
        let header = match header_v2(envelope) {
            Ok(header) => header,
            Err(lethora_protocol::Error::Unsupported(_)) => return Err(Unread::Newer),
            Err(e) => return Err(Unread::Bad(e.to_string())),
        };
        if header.signer != device {
            return Err("signed by another device".into());
        }
        let public = self
            .members
            .as_ref()
            .and_then(|m| m.device(device))
            .ok_or("by a device the members log doesn't know")?
            .public;
        let keyring = self.keyring.as_ref().ok_or("no project key")?;
        let context = Context {
            project: &self.project,
            document,
            kind,
            name: device,
        };
        let plain = open_v2(keyring, &public, &context, envelope)
            .map_err(|e| Unread::Bad(e.to_string()))?;
        // Authentic now: the level is under the signature.
        if header.level > self.reader_level {
            return Err(Unread::Newer);
        }
        Ok((plain, header.level))
    }

    /// Queues the new position `pos` of stream `key`; with nothing queued
    /// before it, it counts as imported right away.
    fn queue_position(&mut self, key: Key, pos: StreamPos) {
        if self.queue.is_empty() {
            self.imported.insert(key, pos);
        } else {
            self.queue.push_back(Queued {
                key,
                update: None,
                pos: Some(pos),
            });
        }
    }

    fn snapshot_changed(&mut self, key: Key, generation: u64) {
        if key.1 == self.device && !self.reads_own() {
            return;
        }
        // Not synced by this version: no read position (see `records`).
        if !DOCUMENTS.contains(&key.0.as_str()) {
            return;
        }
        let pos = StreamPos {
            generation,
            offset: 0,
            needs_snapshot: true,
        };
        self.received.insert(key.clone(), pos);
        self.queue_position(key.clone(), pos);
        self.get_snapshot(key, generation);
    }

    fn snapshot_path(&self, document: &str, device: &str, covers: Option<u64>) -> String {
        let path = format!("/v1/projects/{}/{document}/{device}/snapshot", self.project);
        match covers {
            Some(covers) => format!("{path}?covers={covers}"),
            None => path,
        }
    }

    fn get_snapshot(&mut self, key: Key, generation: u64) {
        if self.snapshots.contains(&key) {
            return;
        }
        self.snapshots.insert(key.clone());
        let path = self.snapshot_path(&key.0, &key.1, None);
        self.request(
            Method::Get,
            path,
            Vec::new(),
            Purpose::Snapshot { key, generation },
        );
    }

    fn compacted(&mut self, key: Key, generation: u64, covers: u64) {
        if key.1 == self.device {
            if let Some(own) = self.own.get_mut(key.0.as_str())
                && generation > own.pos.generation
            {
                own.pos.length = own.pos.length.saturating_sub(covers);
                own.pos.generation = generation;
            }
            if !self.reads_own() {
                return;
            }
        }
        let Some(mut pos) = self.received.get(&key).copied() else {
            return;
        };
        if pos.generation + 1 != generation {
            return;
        }
        pos.generation = generation;
        pos.offset = pos.offset.saturating_sub(covers);
        self.received.insert(key.clone(), pos);
        self.queue_position(key, pos);
    }

    /// `Synced`: the catch-up is complete.
    fn caught_up(&mut self) {
        self.synced = true;
        // An invitee reads no streams yet.
        if !matches!(self.phase, Phase::Invitee { .. }) {
            self.revoke_lost();
        }
        match &self.phase {
            Phase::Invitee { link, .. } => {
                let path = format!("/v1/projects/{}/invites/{}", self.project, link.id);
                self.request(Method::Get, path, Vec::new(), Purpose::InviteKeyGet);
            }
            Phase::Device if self.established => {
                self.ensure_profile();
                self.next_members_op();
                self.maybe_rotate();
                self.act(Action::Push);
                self.presence_dirty = self.own_presence.is_some();
            }
            _ => {}
        }
    }

    /// Queues the profile if the members log has none for this user yet.
    fn ensure_profile(&mut self) {
        let Some(name) = self.name.clone() else {
            return;
        };
        let has = self
            .members
            .as_ref()
            .and_then(|m| m.users().get(&self.user))
            .is_some_and(|u| u.profile.is_some());
        let queued = self
            .members_ops
            .iter()
            .chain(self.members_inflight.as_ref().map(|(_, op, _)| op))
            .any(|op| matches!(op, MembersOp::Profile(_)));
        if has || queued {
            return;
        }
        let Some(keyring) = &self.keyring else {
            return;
        };
        match seal_profile(
            keyring,
            &self.device,
            &self.keys,
            &self.project,
            &self.user,
            &name,
            &self.new_profile_colour(),
            self.project_level(),
        ) {
            Ok(profile) => self.members_ops.push_back(MembersOp::Profile(profile)),
            Err(e) => self.message = Some(format!("Can't seal the profile: {e}")),
        }
    }

    /// The colour of a profile this user publishes on its own: the one it
    /// has as a current member, else the first [`PALETTE`] colour no other
    /// current member uses (see the module docs, "Members").
    fn new_profile_colour(&self) -> String {
        let current = self
            .members
            .as_ref()
            .and_then(|m| m.users().get(&self.user))
            .is_some_and(|u| !u.removed);
        if let Some(profile) = self.profile_of(&self.user).filter(|_| current) {
            return profile.colour;
        }
        let used: Vec<String> = self
            .member_list()
            .into_iter()
            .filter(|m| !m.removed && m.user != self.user)
            .map(|m| m.colour)
            .collect();
        free_colour(&self.user, &used).to_owned()
    }

    /// Whether `op` is in the members log already (it landed, but its ack was
    /// lost).
    fn applied(&self, op: &MembersOp) -> bool {
        let Some(members) = &self.members else {
            return false;
        };
        match op {
            MembersOp::Profile(bytes) => members
                .users()
                .get(&self.user)
                .is_some_and(|u| u.profile.as_ref() == Some(bytes)),
            MembersOp::Invite { link, .. } => members.invites().contains_key(&link.id),
            MembersOp::Join { .. } => members.role_of_device(&self.device).is_some(),
            MembersOp::Change { op, .. } => match op {
                Op::Role { user, role } => members
                    .users()
                    .get(user)
                    .is_some_and(|u| !u.removed && u.role == *role),
                Op::Remove {
                    user: Some(user), ..
                } => members.users().get(user).is_some_and(|u| u.removed),
                Op::Remove {
                    device: Some(device),
                    ..
                } => members.device(device).is_some_and(|d| d.removed),
                Op::Revoke { id } => members.invites().get(id).is_some_and(|i| i.revoked),
                Op::Profile { profile } => members
                    .users()
                    .get(&self.user)
                    .is_some_and(|u| u.profile.as_ref() == Some(profile)),
                Op::Require { level, .. } => members.required_level() >= *level,
                _ => false,
            },
            // This device's, or another device was faster.
            MembersOp::Epoch => !members.needs_rotation(),
        }
    }

    /// Sends the next members op, if none is in flight and the connection is
    /// caught up.
    fn next_members_op(&mut self) {
        while self.members_inflight.is_none() && self.synced && self.token.is_some() {
            let Some(op) = self.members_ops.pop_front() else {
                return;
            };
            if self.applied(&op) {
                self.op_done(op);
                continue;
            }
            match self.build_op(&op) {
                Ok(record) => {
                    if let MembersOp::Epoch = op
                        && let Some(m) = &self.members
                    {
                        self.notes.push(format!(
                            "Starting key epoch {} after a member was removed.",
                            m.epoch() + 1
                        ));
                    }
                    let batch = self.next_batch;
                    self.next_batch += 1;
                    self.send(Message::AppendMembers {
                        batch,
                        record: record.clone(),
                    });
                    self.members_inflight = Some((batch, op, record));
                }
                Err(e) => self.op_failed(op, e.to_string()),
            }
        }
    }

    /// The record of `op` on top of the current head.
    fn build_op(&self, op: &MembersOp) -> std::result::Result<Vec<u8>, lethora_protocol::Error> {
        let members = self
            .members
            .as_ref()
            .ok_or_else(|| lethora_protocol::Error::Invalid("no members log yet".into()))?;
        let time = self.server_time.max(self.clock);
        match op {
            MembersOp::Profile(profile) => members.entry(
                &self.device,
                time,
                Op::Profile {
                    profile: profile.clone(),
                },
                &[&self.keys],
            ),
            MembersOp::Invite {
                link,
                role,
                user,
                expires,
                uses,
            } => members.entry(
                &self.device,
                time,
                Op::Invite {
                    id: link.id.clone(),
                    key: link.keys().verifying_key(),
                    exchange: link.keys().exchange_key(),
                    role: *role,
                    user: user.clone(),
                    expires: *expires,
                    uses: *uses,
                },
                &[&self.keys],
            ),
            MembersOp::Join { link, profile } => members.entry(
                &self.device,
                self.server_time,
                Op::Join {
                    invite: link.id.clone(),
                    user: self.user.clone(),
                    device: self.device.clone(),
                    public: self.keys.public(),
                    profile: Some(profile.clone()),
                },
                &[&link.keys(), &self.keys],
            ),
            MembersOp::Change { op, .. } => {
                members.entry(&self.device, time, op.clone(), &[&self.keys])
            }
            MembersOp::Epoch => {
                let key: [u8; KEY_LEN] = lethora_protocol::random()?;
                let op = members.epoch_op(&key, time)?;
                members.entry(&self.device, time, op, &[&self.keys])
            }
        }
    }

    fn members_ack(&mut self, batch: u64) {
        let Some((sent, op, record)) = self.members_inflight.take() else {
            return;
        };
        if sent != batch {
            self.members_inflight = Some((sent, op, record));
            return;
        }
        let have = self.members_bytes.len() as u64;
        if let Err(e) = self.add_members(have, &record) {
            return self.fail(format!(
                "The members log on this device can't take its own entry: {e}"
            ));
        }
        self.op_done(op);
        if self.members_changed() {
            self.next_members_op();
            self.maybe_rotate();
        }
    }

    /// `op` is in the members log.
    fn op_done(&mut self, op: MembersOp) {
        match op {
            MembersOp::Profile(_) => {}
            MembersOp::Invite { link, .. } => {
                let Some(keyring) = &self.keyring else {
                    return;
                };
                match link.keys().wrap(&self.project, keyring) {
                    Ok(wrapped) => {
                        let path = format!("/v1/projects/{}/invites/{}", self.project, link.id);
                        self.request(Method::Put, path, wrapped, Purpose::InviteKeyPut(link.id));
                        // Without a token the PUT waits for the next welcome.
                    }
                    Err(e) => {
                        self.invites
                            .insert(link.id.clone(), InviteState::Failed(e.to_string()));
                    }
                }
            }
            MembersOp::Join { .. } => {
                // Joined: connect again as the new device.
                self.phase = Phase::Catchup;
                self.synced = false;
                self.act(Action::Reconnect);
            }
            MembersOp::Change { id, .. } => {
                self.changes.insert(id, Some(Ok(())));
            }
            MembersOp::Epoch => {}
        }
    }

    fn op_failed(&mut self, op: MembersOp, why: String) {
        match op {
            MembersOp::Profile(_) => self.message = Some(format!("The profile was refused: {why}")),
            MembersOp::Invite { link, .. } => {
                self.invites.insert(link.id, InviteState::Failed(why));
            }
            MembersOp::Join { .. } => self.fail(format!("Joining was refused: {why}")),
            MembersOp::Change { id, .. } => {
                self.changes.insert(id, Some(Err(why)));
            }
            // Tried again with the next members change or connection.
            MembersOp::Epoch => self
                .notes
                .push(format!("Starting a key epoch was refused: {why}")),
        }
    }

    fn ack(&mut self, batch: u64, document: &str, generation: u64, length: u64) {
        let Some(own) = self.own.get_mut(document) else {
            return;
        };
        if let Some((pending, vv)) = own.pending.take() {
            if pending == batch {
                own.pushed.merge(&vv);
            } else {
                own.pending = Some((pending, vv));
            }
        }
        own.pos.batch = own.pos.batch.max(batch);
        if generation == own.pos.generation {
            own.pos.length = own.pos.length.max(length);
        } else if generation > own.pos.generation {
            own.pos.generation = generation;
            own.pos.length = length;
        }
        self.raise_next_batch();
        self.act(Action::Push);
    }

    fn nack(&mut self, batch: u64, code: Code, message: String) {
        if let Some((sent, op, record)) = self.members_inflight.take() {
            if sent == batch {
                if code == Code::Stale {
                    // Built on an older head: again now if the log has moved
                    // on here since, else once it has caught up.
                    self.members_ops.push_front(op);
                    let built = entry_body(&record).map(|b| b.seq).ok();
                    let next = self.members.as_ref().map(|m| m.head().seq + 1);
                    if built != next {
                        self.next_members_op();
                    } else {
                        self.members_stale = true;
                    }
                } else {
                    self.op_failed(op, message);
                    self.next_members_op();
                }
                return;
            }
            self.members_inflight = Some((sent, op, record));
        }
        // Batch ids are unique across documents: only that stream waits.
        let clock = self.clock;
        let refused = self
            .own
            .iter_mut()
            .find(|(_, o)| o.pending.as_ref().is_some_and(|(p, _)| *p == batch));
        if let Some((document, own)) = refused {
            own.pending = None;
            own.push_after = clock + RETRY_SECS;
            self.message = Some(match code {
                Code::Forbidden => format!(
                    "This device's role doesn't allow changing {document}; its changes stay on this device."
                ),
                Code::Quota => "The project's storage on the server is full.".into(),
                _ => format!("The server refused changes: {message}"),
            });
        }
    }

    /// Handles the result of HTTP request `id`.
    pub fn http_result(&mut self, id: u64, result: std::result::Result<HttpResponse, String>) {
        let Some(purpose) = self.http.remove(&id) else {
            return;
        };
        // A deletion closes the connections: its answer may come after that.
        if self.stopped.is_some() && !matches!(purpose, Purpose::Delete(_)) {
            return;
        }
        let ok = |r: &std::result::Result<HttpResponse, String>| matches!(r, Ok(r) if (200..300).contains(&r.status));
        let why = |r: &std::result::Result<HttpResponse, String>| match r {
            Ok(r) => format!(
                "HTTP {}: {}",
                r.status,
                String::from_utf8_lossy(&r.body[..r.body.len().min(200)])
            ),
            Err(e) => e.clone(),
        };
        match purpose {
            Purpose::Snapshot { key, generation } => {
                self.snapshots.remove(&key);
                match &result {
                    Ok(r) if r.status == 200 => self.snapshot_arrived(key, generation, &r.body),
                    _ => {
                        self.skipped.push(format!(
                            "snapshot of {}: {}",
                            key_name(&key),
                            why(&result)
                        ));
                        self.snapshot_retry
                            .insert(key, (generation, self.clock + RETRY_SECS));
                    }
                }
            }
            Purpose::BlobHead(blob) => {
                self.blobs_inflight.remove(&blob);
                match &result {
                    Ok(r) if r.status == 200 => {
                        self.server_blobs.insert(blob);
                    }
                    Ok(r) if r.status == 404 => {
                        self.blobs_to_put.insert(blob);
                    }
                    _ => {
                        self.blob_retry.insert(blob, self.clock + RETRY_SECS);
                    }
                }
                self.act(Action::Push);
            }
            Purpose::BlobPut(blob) => {
                self.blobs_inflight.remove(&blob);
                match &result {
                    _ if ok(&result) => {
                        self.blobs_to_put.remove(&blob);
                        self.server_blobs.insert(blob);
                    }
                    Ok(r) if r.status == 413 => {
                        self.message = Some(format!(
                            "An image is too large for the server; it stays on this device. ({})",
                            why(&result)
                        ));
                        self.failed_blobs.insert(blob);
                    }
                    _ => {
                        self.blob_retry.insert(blob, self.clock + RETRY_SECS);
                    }
                }
                self.act(Action::Push);
            }
            Purpose::BlobGet(blob) => {
                self.blobs_inflight.remove(&blob);
                let opened = match &result {
                    Ok(r) if r.status == 200 => self.open_blob(&blob, &r.body),
                    _ => Err(why(&result)),
                };
                match opened {
                    Ok(bytes) => {
                        self.server_blobs.insert(blob.clone());
                        self.arrived.push((blob, bytes));
                        self.act(Action::RemoteChanges);
                    }
                    Err(e) => {
                        if !matches!(&result, Ok(r) if r.status == 404) {
                            self.skipped.push(format!("blob {blob}: {e}"));
                        }
                        self.blob_retry.insert(blob, self.clock + RETRY_SECS);
                    }
                }
            }
            Purpose::Compact {
                document,
                covers,
                vv,
            } => {
                self.own_mut(document).compacting = false;
                let answer = match &result {
                    Ok(r) if ok(&result) => serde_json::from_slice::<serde_json::Value>(&r.body)
                        .ok()
                        .and_then(|v| Some((v["generation"].as_u64()?, v["length"].as_u64()?))),
                    _ => None,
                };
                match answer {
                    Some((generation, length)) => {
                        let own = self.own_mut(document);
                        if generation > own.pos.generation {
                            own.pos.generation = generation;
                            own.pos.length = length;
                        }
                        own.pushed.merge(&vv);
                        own.snapshot_due = false;
                        if let Phase::Share { .. } = self.phase {
                            self.phase = Phase::Device;
                            self.established = true;
                            if self.synced {
                                self.caught_up();
                            }
                        }
                        self.act(Action::Push);
                    }
                    None if covers == 0 && matches!(self.phase, Phase::Share { .. }) => {
                        self.fail(format!("Uploading the project failed: {}", why(&result)));
                    }
                    None => {
                        self.message = Some(format!("Compacting failed: {}", why(&result)));
                        let at = self.clock + RETRY_SECS;
                        self.own_mut(document).push_after = at;
                    }
                }
            }
            Purpose::InviteKeyPut(invite) => {
                let state = if ok(&result) {
                    let link = self
                        .invite_links
                        .remove(&invite)
                        .map(|l| l.to_string())
                        .unwrap_or_default();
                    InviteState::Ready(link)
                } else {
                    InviteState::Failed(why(&result))
                };
                self.invites.insert(invite, state);
            }
            Purpose::InviteKeyGet => match &result {
                Ok(r) if r.status == 200 => self.invite_key(&r.body),
                Ok(r) if r.status == 410 => self.fail(
                    "This invite link can't be used any more (expired, used up or revoked). Ask for a new one.",
                ),
                _ => self.fail(format!("Fetching the invite failed: {}", why(&result))),
            },
            Purpose::Delete(id) => {
                if ok(&result) {
                    self.changes.insert(id, Some(Ok(())));
                    self.stop(
                        ServerState::Removed,
                        "The project was deleted from the server.",
                    );
                } else {
                    self.changes.insert(id, Some(Err(why(&result))));
                }
            }
        }
    }

    /// The snapshot of stream `key`'s `generation` arrived: queue it. One
    /// whose device may not write the document is skipped like a record.
    fn snapshot_arrived(&mut self, key: Key, generation: u64, envelope: &[u8]) {
        let opened = match self.importable(&key) {
            Ok(user) => match self.open(&key.0, &key.1, "snapshot", envelope) {
                Ok((update, level)) => {
                    self.level = self.level.max(level);
                    Ok((update, user))
                }
                // Still needed: read after the update (see the module docs,
                // "Levels").
                Err(Unread::Newer) => return self.needs_update(),
                Err(e) => Err((e.to_string(), true)),
            },
            Err(e) => Err((e, false)),
        };
        let done = |sync: &mut Self| {
            sync.received.get_mut(&key).map(|pos| {
                if pos.generation <= generation {
                    pos.needs_snapshot = false;
                }
                *pos
            })
        };
        match opened {
            Ok(update) => {
                let pos = done(self);
                self.queue.push_back(Queued {
                    key,
                    update: Some(update),
                    pos,
                });
                self.act(Action::RemoteChanges);
                if self.synced {
                    self.revoke_lost();
                }
            }
            Err((e, false)) => {
                self.skipped
                    .push(format!("snapshot of {}: {e}", key_name(&key)));
                if let Some(pos) = done(self) {
                    self.queue_position(key, pos);
                }
            }
            Err((e, _)) => {
                self.skipped
                    .push(format!("snapshot of {}: {e}", key_name(&key)));
                self.snapshot_retry
                    .insert(key, (generation, self.clock + RETRY_SECS));
            }
        }
    }

    /// The bytes of blob `blob` from its envelope, after checking it.
    fn open_blob(&self, blob: &str, envelope: &[u8]) -> std::result::Result<Vec<u8>, String> {
        let header = header_v2(envelope).map_err(|e| e.to_string())?;
        let members = self.members.as_ref().ok_or("no members log yet")?;
        let public = members
            .device(&header.signer)
            .ok_or("by a device the members log doesn't know")?
            .public;
        let keyring = self.keyring.as_ref().ok_or("no project key")?;
        let context = Context {
            project: &self.project,
            document: MAIN,
            kind: "asset",
            name: blob,
        };
        open_v2(keyring, &public, &context, envelope).map_err(|e| e.to_string())
    }

    /// The wrapped project keys of the invite arrived (joining): the
    /// inviter's keyring, plus the epochs sealed to the invite since.
    fn invite_key(&mut self, wrapped: &[u8]) {
        let Phase::Invitee { link, name } = &self.phase else {
            return;
        };
        let (link, name) = (link.clone(), name.clone());
        let keyring = match link.keys().unwrap(&self.project, wrapped) {
            Ok(keyring) => keyring,
            Err(e) => {
                self.fail(format!("The invite's key doesn't open: {e}"));
                return;
            }
        };
        let invite = self
            .members
            .as_ref()
            .and_then(|m| m.invites().get(&link.id))
            .cloned();
        let Some(invite) = invite else {
            self.fail("The invite is not in the project's members log.");
            return;
        };
        let current = self.members.as_ref().and_then(|m| {
            m.role_of_device(&self.device)?;
            m.device(&self.device).map(|d| d.user.clone())
        });
        if let Some(user) = current {
            // This device is a member already (its local copy was lost): it
            // stays the user it is, whatever the invite says; no `join`.
            self.user = user;
        } else if let Some(user) = invite.user {
            // A device invite: this device joins the inviter's user.
            self.user = user;
        }
        self.keyring = Some(keyring);
        self.learn_keys();
        let profile = match seal_profile(
            self.keyring.as_ref().expect("set"),
            &self.device,
            &self.keys,
            &self.project,
            &self.user,
            &name,
            &self.new_profile_colour(),
            self.project_level(),
        ) {
            Ok(profile) => profile,
            Err(e) => {
                self.fail(format!("Can't seal the profile: {e}"));
                return;
            }
        };
        self.members_ops
            .push_back(MembersOp::Join { link, profile });
        self.next_members_op();
    }

    /// Creates an invite: for a new member with `role`, or (`None`) for
    /// another device of this device's user; it expires after `days` and
    /// can be used `uses` times. Returns its id; [`ServerSync::invite_result`]
    /// says when the link is ready.
    pub fn invite(&mut self, role: Option<Role>, days: u64, uses: u32) -> Result<String> {
        if !self.established {
            return Err(Error::InvalidChange("the project is not shared yet".into()));
        }
        let link = InviteLink::generate(&self.url, &self.project).map_err(invalid)?;
        let id = link.id.clone();
        let expires = self.server_time.max(self.clock) + days * 24 * 60 * 60;
        let user = role.is_none().then(|| self.user.clone());
        self.invite_links.insert(id.clone(), link.clone());
        self.invites.insert(id.clone(), InviteState::Pending);
        self.members_ops.push_back(MembersOp::Invite {
            link,
            role,
            user,
            expires,
            uses: uses.max(1),
        });
        self.next_members_op();
        Ok(id)
    }

    /// The link of invite `id` once it is ready (`Ok`), or why it failed;
    /// `None` while pending.
    pub fn invite_result(&self, id: &str) -> Option<std::result::Result<String, String>> {
        match self.invites.get(id)? {
            InviteState::Pending => None,
            InviteState::Ready(link) => Some(Ok(link.clone())),
            InviteState::Failed(why) => Some(Err(why.clone())),
        }
    }

    /// Queues members op `op` after checking it against the current state
    /// (the protocol's rules, see `lethora_protocol::members`); returns the
    /// id of its result ([`ServerSync::change_result`]).
    fn change(&mut self, op: Op) -> Result<u64> {
        if !self.established {
            return Err(Error::InvalidChange("the project is not shared yet".into()));
        }
        let members = self
            .members
            .as_ref()
            .ok_or_else(|| Error::InvalidChange("the members log isn't here yet".into()))?;
        let time = self.server_time.max(self.clock);
        let record = members
            .entry(&self.device, time, op.clone(), &[&self.keys])
            .map_err(invalid)?;
        members.clone().apply(&record).map_err(|e| match e {
            lethora_protocol::Error::Rejected(why) => Error::NotAllowed(why),
            e => invalid(e),
        })?;
        let id = self.next_change;
        self.next_change += 1;
        self.changes.insert(id, None);
        self.members_ops.push_back(MembersOp::Change { id, op });
        self.next_members_op();
        Ok(id)
    }

    /// Gives member `user` the role `role` (the owner only, never to or of
    /// the owner). Returns the id of its result.
    pub fn set_role(&mut self, user: &str, role: Role) -> Result<u64> {
        self.change(Op::Role {
            user: user.to_owned(),
            role,
        })
    }

    /// Removes member `user` with all its devices (the owner only, never
    /// itself). Returns the id of its result.
    pub fn remove_user(&mut self, user: &str) -> Result<u64> {
        self.change(Op::Remove {
            user: Some(user.to_owned()),
            device: None,
        })
    }

    /// Removes device `device` (by the owner or a device of its user, never
    /// the owner's last device); this device's own id leaves the project.
    /// Returns the id of its result.
    pub fn remove_device(&mut self, device: &str) -> Result<u64> {
        self.change(Op::Remove {
            user: None,
            device: Some(device.to_owned()),
        })
    }

    /// Revokes invite `invite` (by the owner or its creator). Returns the id
    /// of its result.
    pub fn revoke(&mut self, invite: &str) -> Result<u64> {
        self.change(Op::Revoke {
            id: invite.to_owned(),
        })
    }

    /// Gives this device's user `colour` (one of [`PALETTE`]) in this
    /// project: a new profile, keeping the name. Returns the id of its
    /// result.
    pub fn set_colour(&mut self, colour: &str) -> Result<u64> {
        if !PALETTE.contains(&colour) {
            return Err(Error::InvalidChange(format!(
                "{colour} is not one of the profile colours"
            )));
        }
        let keyring = self
            .keyring
            .as_ref()
            .ok_or_else(|| Error::InvalidChange("the project is not shared yet".into()))?;
        let name = self
            .profile_of(&self.user)
            .map(|p| p.name)
            .or_else(|| self.name.clone())
            .unwrap_or_default();
        let profile = seal_profile(
            keyring,
            &self.device,
            &self.keys,
            &self.project,
            &self.user,
            &name,
            colour,
            self.project_level(),
        )?;
        self.change(Op::Profile { profile })
    }

    /// Deletes the project from the server (the owner only; local copies
    /// stay). Needs a connection. Returns the id of its result; once it is
    /// done, the sync stops with [`ServerState::Removed`].
    pub fn delete_project(&mut self) -> Result<u64> {
        if !self.established {
            return Err(Error::InvalidChange("the project is not shared yet".into()));
        }
        if self.role != Some(Role::Owner) {
            return Err(Error::NotAllowed(
                "only the owner deletes the project".into(),
            ));
        }
        if self.token.is_none() || self.stopped.is_some() {
            return Err(Error::InvalidChange(
                "not connected to the server; try again when online".into(),
            ));
        }
        let time = self.server_time.max(self.clock);
        let body = deletion_body(&self.keys, &self.project, time).map_err(invalid)?;
        let id = self.next_change;
        self.next_change += 1;
        self.changes.insert(id, None);
        let path = format!("/v1/projects/{}", self.project);
        self.request(Method::Delete, path, body, Purpose::Delete(id));
        Ok(id)
    }

    /// How change or deletion `id` went: `Ok` once it is done, or why it was
    /// refused; `None` while pending (or for an unknown id).
    pub fn change_result(&self, id: u64) -> Option<std::result::Result<(), String>> {
        self.changes.get(&id).cloned().flatten()
    }

    /// The project's members, current ones first, then removed ones (each by
    /// user id): name and colour from their profiles (the colour from the
    /// user id without one), role and current devices.
    pub fn member_list(&self) -> Vec<MemberInfo> {
        let Some(members) = &self.members else {
            return Vec::new();
        };
        let mut list: Vec<MemberInfo> = members
            .users()
            .iter()
            .map(|(user, u)| {
                let profile = self.profile_of(user);
                MemberInfo {
                    user: user.clone(),
                    colour: profile
                        .as_ref()
                        .map_or_else(|| profile_colour(user).to_owned(), |p| p.colour.clone()),
                    name: profile.map(|p| p.name),
                    role: u.role,
                    removed: u.removed,
                    devices: members
                        .devices_of(user)
                        .filter(|(_, d)| !d.removed)
                        .map(|(id, _)| id.to_owned())
                        .collect(),
                }
            })
            .collect();
        list.sort_by_key(|m| m.removed);
        list
    }

    /// The invites that can still be used: not revoked, used up or expired,
    /// and their creator still a member.
    pub fn open_invites(&self) -> Vec<InviteInfo> {
        let Some(members) = &self.members else {
            return Vec::new();
        };
        let now = self.server_time.max(self.clock);
        members
            .invites()
            .iter()
            .filter(|(_, i)| !i.revoked && i.uses_left > 0 && i.expires >= now)
            .filter_map(|(id, i)| {
                members.role_of_device(&i.creator)?;
                Some(InviteInfo {
                    id: id.clone(),
                    creator: i.creator.clone(),
                    creator_user: members.device(&i.creator)?.user.clone(),
                    role: i.role,
                    expires: i.expires,
                    uses_left: i.uses_left,
                })
            })
            .collect()
    }

    /// Whether presence goes out and comes in: online as a device.
    fn presence_online(&self) -> bool {
        self.established
            && matches!(self.phase, Phase::Device)
            && self.token.is_some()
            && self.synced
            && self.stopped.is_none()
    }

    /// Sets this device's presence entry, sent now or as soon as the gap
    /// allows ([`ServerSync::presence_tick`]); unix ms `now`.
    fn set_presence(&mut self, value: LoroValue, now: u64) {
        if self.own_presence.as_ref() == Some(&value) {
            return;
        }
        self.own_presence = Some(value);
        self.presence_dirty = true;
        self.presence_tick(now);
    }

    /// Unix ms when [`ServerSync::presence_tick`] has something to do next:
    /// a send or a timeout check (see the module docs, "Presence").
    pub fn presence_due(&self) -> Option<u64> {
        if !self.presence_online() {
            return None;
        }
        let send = self.own_presence.as_ref().map(|_| {
            self.presence_sent
                + if self.presence_dirty {
                    PRESENCE_GAP_MS
                } else {
                    PRESENCE_KEEPALIVE_MS
                }
        });
        let check =
            (!self.presence.keys().is_empty()).then_some(self.presence_checked + PRESENCE_CHECK_MS);
        send.into_iter().chain(check).min()
    }

    /// Sends this device's presence if due and drops others' timed-out
    /// entries; unix ms `now`.
    pub fn presence_tick(&mut self, now: u64) {
        if !self.presence_online() {
            return;
        }
        if now >= self.presence_checked + PRESENCE_CHECK_MS {
            self.presence_checked = now;
            let before = self.presence.keys().len();
            self.presence.remove_outdated();
            if self.presence.keys().len() != before {
                self.act(Action::Presence);
            }
        }
        let Some(value) = self.own_presence.clone() else {
            return;
        };
        let gap = if self.presence_dirty {
            PRESENCE_GAP_MS
        } else {
            PRESENCE_KEEPALIVE_MS
        };
        if now < self.presence_sent + gap {
            return;
        }
        self.presence_sent = now;
        self.presence_dirty = false;
        match self.seal_presence(value) {
            Ok(bytes) => self.send(Message::Presence { bytes }),
            Err(e) => self.notes.push(format!("Presence wasn't sent: {e}")),
        }
    }

    /// This device's presence entry `value`, encoded by an `EphemeralStore`
    /// and sealed (see the module docs, "Presence").
    fn seal_presence(&self, value: LoroValue) -> Result<Vec<u8>> {
        let keyring = self
            .keyring
            .as_ref()
            .ok_or_else(|| Error::InvalidChange("no project key".into()))?;
        let store = EphemeralStore::new(PRESENCE_TIMEOUT_MS as i64);
        store.set(&self.device, value);
        let context = Context {
            project: &self.project,
            document: MAIN,
            kind: PRESENCE,
            name: &self.device,
        };
        seal_v2(
            keyring,
            &self.device,
            &self.keys,
            &context,
            self.project_level(),
            &store.encode(&self.device),
        )
        .map_err(from_protocol)
    }

    /// Presence `bytes` from `device`: taken if a current member device
    /// sealed it, only its own entry (see the module docs, "Presence").
    fn peer_presence(&mut self, device: &str, bytes: &[u8]) {
        if device == self.device || !self.presence_online() {
            return;
        }
        let current = self
            .members
            .as_ref()
            .is_some_and(|m| m.role_of_device(device).is_some());
        if !current {
            return;
        }
        let plain = match self.open(MAIN, device, PRESENCE, bytes) {
            Ok((plain, _)) => plain,
            Err(e) => {
                self.skipped.push(format!("presence of {device}: {e}"));
                return;
            }
        };
        // Decoded without a timeout: the sender's clock doesn't count here.
        let scratch = EphemeralStore::new(i64::MAX / 4);
        if let Err(e) = scratch.apply(&plain) {
            self.skipped.push(format!("presence of {device}: {e}"));
            return;
        }
        let known = self.presence.get(device).is_some();
        match scratch.get(device) {
            Some(value) => {
                self.presence.set(device, value);
                // A device that wasn't here learns of this one soon.
                if !known && self.own_presence.is_some() {
                    self.presence_dirty = true;
                }
            }
            None if known => self.presence.delete(device),
            None => return,
        }
        self.act(Action::Presence);
    }

    /// Other current member devices' presence entries: the info without
    /// positions, and the anchors of its selection.
    fn presence_entries(&self) -> Vec<(PresenceInfo, String, String)> {
        let Some(members) = &self.members else {
            return Vec::new();
        };
        let text = |map: &loro::LoroMapValue, key: &str| match map.get(key) {
            Some(LoroValue::String(s)) => Some(s.to_string()),
            _ => None,
        };
        let mut list: Vec<_> = self
            .presence
            .get_all_states()
            .into_iter()
            .filter(|(device, _)| *device != self.device)
            .filter_map(|(device, value)| {
                let LoroValue::Map(map) = value else {
                    return None;
                };
                let role = members.role_of_device(&device)?;
                let user = members.device(&device)?.user.clone();
                let view = text(&map, "view").filter(|v| VIEWS.contains(&v.as_str()))?;
                let profile = self.profile_of(&user);
                let info = PresenceInfo {
                    colour: profile
                        .as_ref()
                        .map_or_else(|| profile_colour(&user).to_owned(), |p| p.colour.clone()),
                    name: profile.map(|p| p.name),
                    device,
                    user,
                    role,
                    view,
                    anchor: None,
                    head: None,
                };
                Some((info, text(&map, "anchor")?, text(&map, "head")?))
            })
            .collect();
        list.sort_by(|a, b| a.0.device.cmp(&b.0.device));
        list
    }

    /// Whether a push of `document` may go out now: one batch in flight
    /// per document, and only while the role writes it (see the module
    /// docs, "Roles").
    fn may_push(&self, document: &str) -> bool {
        let own = &self.own[document];
        self.established
            && matches!(self.phase, Phase::Device)
            && self.token.is_some()
            && self.stopped.is_none()
            && own.pending.is_none()
            && !own.compacting
            && self.clock >= own.push_after
            && self.role.is_none_or(|r| r.writes(document))
    }

    /// Requests the indexed blobs in `missing` (not here) that aren't on
    /// their way or waiting for a retry.
    fn want_blobs(&mut self, missing: &BTreeSet<String>) {
        if self.token.is_none() {
            return;
        }
        for blob in missing {
            if self.blobs_inflight.contains(blob)
                || self.blob_retry.get(blob).is_some_and(|at| *at > self.clock)
            {
                continue;
            }
            self.blob_retry.remove(blob);
            self.blobs_inflight.insert(blob.clone());
            let path = format!("/v1/projects/{}/assets/{blob}", self.project);
            self.request(
                Method::Get,
                path,
                Vec::new(),
                Purpose::BlobGet(blob.clone()),
            );
        }
    }
}

impl Document {
    /// `document`'s Loro document and the origins of its peers.
    fn synced_doc(&self, document: &str) -> (&LoroDoc, &Trust) {
        if document == REVIEW {
            (&self.review.doc, &self.review.trust)
        } else {
            (&self.doc, &self.trust)
        }
    }

    /// The settings to keep: `sync`'s, with the origins of both documents.
    fn server_settings_of(&self, sync: &ServerSync) -> ServerSettings {
        let mut settings = sync.settings();
        settings.origins = BTreeMap::from([
            (MAIN.to_owned(), self.trust.settings()),
            (REVIEW.to_owned(), self.review.trust.settings()),
        ]);
        settings.origins.retain(|_, o| !o.is_empty());
        settings
    }

    /// Gives `main` and `review` a fresh Loro peer when an import brought
    /// ops under their current one that aren't this device's (forged by
    /// another device, see the `trust` module): later edits must not
    /// continue such a peer, whose ops this device neither pushes nor
    /// counts as its own.
    fn renew_contested_peers(&mut self) -> Result<()> {
        for document in DOCUMENTS {
            let (doc, trust) = self.synced_doc(document);
            if !trust.contested(&self.identity, doc.peer_id()) {
                continue;
            }
            let known = doc.oplog_vv();
            let mut peer = PeerID::MAX;
            while peer == PeerID::MAX || known.get(&peer).is_some() {
                peer = u64::from_le_bytes(lethora_protocol::random().map_err(from_protocol)?);
            }
            doc.commit();
            doc.set_peer_id(peer)?;
            if document == REVIEW {
                self.review.registered = None;
            } else {
                self.registered = None;
            }
        }
        Ok(())
    }

    /// Where this device syncs the project with a server, from its state
    /// file (see [`Document::server_save`]).
    pub fn server_settings(&self) -> Option<&ServerSettings> {
        self.server.as_ref()
    }

    /// Starts sharing this project on the server at `url` (an origin, see
    /// `lethora_protocol::invite::server_origin`) as its owner: a new random
    /// project id and key, the genesis record, a sealed snapshot of
    /// everything. `name` is the user's display name for the profile, `now`
    /// the unix time. Returns the [`ServerSync`] to drive (the project is
    /// shared once it [`is_established`](ServerSync::is_established); then
    /// [`Document::server_save`] keeps the settings) and the project key for
    /// the app to keep. Refused while folder sync is on or the project is
    /// shared already.
    pub fn server_share(
        &mut self,
        url: &str,
        name: &str,
        keys: DeviceKeys,
        now: u64,
    ) -> Result<(ServerSync, [u8; KEY_LEN])> {
        if self.sync.is_some() {
            return Err(Error::InvalidChange(
                "this project syncs through a folder; stop folder sync to share it on a server"
                    .into(),
            ));
        }
        if self.server.is_some() {
            return Err(Error::InvalidChange(
                "this project is shared on a server already".into(),
            ));
        }
        let url = server_origin(url).map_err(invalid)?;
        self.autosave()?;
        let project = lethora_protocol::random_id().map_err(from_protocol)?;
        let key: [u8; KEY_LEN] = lethora_protocol::random().map_err(from_protocol)?;
        let device = self.identity.device.clone();
        let user = self.identity.user.as_str().to_owned();
        let genesis = Members::genesis_entry(&project, &user, device.as_str(), &keys, now)
            .map_err(from_protocol)?;
        let keyring = Keyring::new(key);
        let context = Context {
            project: &project,
            document: MAIN,
            kind: "snapshot",
            name: device.as_str(),
        };
        let snapshot = seal_v2(
            &keyring,
            device.as_str(),
            &keys,
            &context,
            NEW_LEVEL,
            &export_snapshot(&self.doc)?,
        )
        .map_err(from_protocol)?;
        self.storage.create_dir(SERVER_DIR)?;
        let mut log = self.storage.open_log(MEMBERS_LOG)?;
        log.truncate(0)?;
        // The stream holds everything the project held when shared.
        let vv = self.doc.oplog_vv();
        let mut sync = ServerSync::new(
            url,
            project,
            &device,
            user,
            keys,
            Some(keyring),
            Phase::Share {
                genesis,
                snapshot,
                vv,
                created: false,
                uploading: false,
            },
        );
        sync.members_log = Some(log);
        sync.name = Some(name.to_owned());
        sync.clock = now;
        // `review` goes up as a snapshot too, once `main` is (see the module
        // docs, "Share and join").
        sync.own_mut(REVIEW).snapshot_due = !self.review.doc.oplog_vv().is_empty();
        Ok((sync, key))
    }

    /// The [`ServerSync`] of this shared project (see
    /// [`Document::server_settings`]), with the project keys `keyring` (the
    /// epochs found in the members log are added) and this device's `keys`;
    /// `name` is published as the profile if the members log has none for
    /// this user. Reads the members log kept here (a torn tail is cut); one
    /// that is damaged or of another project gives a stopped sync with an
    /// error. Fails with [`Error::NotFound`] if the project isn't shared.
    pub fn server_open(
        &self,
        keys: DeviceKeys,
        keyring: Keyring,
        name: Option<&str>,
    ) -> Result<ServerSync> {
        let settings = self
            .server
            .clone()
            .ok_or_else(|| Error::NotFound("server settings of this project".into()))?;
        let mut sync = ServerSync::new(
            settings.url.clone(),
            settings.project.clone(),
            &self.identity.device,
            settings.user.clone(),
            keys,
            Some(keyring),
            Phase::Device,
        );
        sync.established = true;
        sync.name = name.map(str::to_owned);
        for (document, state) in [
            (MAIN, (&settings.own, &settings.pushed)),
            (REVIEW, (&settings.review.own, &settings.review.pushed)),
        ] {
            let own = sync.own_mut(document);
            own.pos = *state.0;
            own.pushed = vv_from_hex(state.1);
        }
        sync.raise_next_batch();
        sync.revoked = settings
            .revoked
            .iter()
            .filter_map(|name| {
                let (document, device) = name.split_once('/')?;
                Some((document.to_owned(), device.to_owned()))
            })
            .collect();
        for (name, pos) in &settings.streams {
            if let Some((document, device)) = name.split_once('/') {
                let key = (document.to_owned(), device.to_owned());
                sync.received.insert(key.clone(), *pos);
                sync.imported.insert(key, *pos);
            }
        }
        self.storage.create_dir(SERVER_DIR)?;
        let mut log = self.storage.open_log(MEMBERS_LOG)?;
        let bytes = log.read_all()?;
        let read = Members::from_log(&bytes);
        match (&read.end, &read.members) {
            (LogEnd::Invalid(e), _) => {
                sync.fail(format!(
                    "The members log on this device is damaged ({e}); syncing stopped."
                ));
            }
            (_, Some(m)) if m.project() != settings.project => {
                sync.fail("The members log on this device is of another project; syncing stopped.");
            }
            _ => {
                if read.bytes < bytes.len() {
                    log.truncate(read.bytes as u64)?;
                }
                // Record by record, for the devices that could ever write.
                if let Err(e) = sync.add_members(0, &bytes[..read.bytes]) {
                    sync.fail(format!(
                        "The members log on this device is damaged ({e}); syncing stopped."
                    ));
                }
                sync.role = sync
                    .members
                    .as_ref()
                    .and_then(|m| m.role_of_device(self.identity.device.as_str()));
            }
        }
        sync.members_log = Some(log);
        Ok(sync)
    }

    /// Pushes what is new to the server (see the module docs, "Streams"):
    /// autosaves, then per document this device's role writes, appends one
    /// record with this device's changes since the pushed version, or
    /// uploads a snapshot when the own log grew past the compaction
    /// threshold; `main`'s records wait for the indexed blobs the server
    /// lacks, uploaded first. Does nothing for a document with a batch in
    /// flight, or while the connection isn't ready; [`Action::Push`] says
    /// when to try again. Changes a role doesn't allow pushing stay here,
    /// unpushed, until it does.
    pub fn server_push(&mut self, sync: &mut ServerSync) -> Result<()> {
        if !DOCUMENTS.iter().any(|d| sync.may_push(d)) {
            return Ok(());
        }
        self.autosave()?;
        for document in DOCUMENTS {
            if !sync.may_push(document) || (document == MAIN && self.upload_blobs(sync)?) {
                continue;
            }
            self.push_document(sync, document)?;
        }
        Ok(())
    }

    /// One push of `document` (see [`Document::server_push`]).
    fn push_document(&mut self, sync: &mut ServerSync, document: &'static str) -> Result<()> {
        let device = self.identity.device.as_str().to_owned();
        let (doc, trust) = self.synced_doc(document);
        let own = &sync.own[document];
        let (spans, target) = trust.unpushed(doc, &self.identity, &own.pushed);
        let (length, last_batch, whole) = (own.pos.length, own.pos.batch, own.snapshot_due);
        let level = sync.project_level();
        let keyring = sync
            .keyring
            .clone()
            .ok_or_else(|| Error::InvalidChange("the project key is missing".into()))?;
        let compact = length > self.compaction_threshold || whole;
        let mut record = None;
        if !spans.is_empty() && !compact {
            let update = doc.export(ExportMode::updates_in_range(spans))?;
            let context = Context {
                project: &sync.project,
                document,
                kind: "log",
                name: &device,
            };
            let sealed = seal_v2(&keyring, &device, &sync.keys, &context, level, &update)
                .map_err(from_protocol)?;
            record = Some(frame(&sealed)?);
        }
        let too_large = record.as_ref().is_some_and(|r| r.len() > MAX_RECORD);
        if compact || too_large {
            // What the stream holds and the own ops beyond (everything when
            // sharing; see the module docs, "Streams").
            let (update, vv) = if whole {
                (export_snapshot(doc)?, doc.oplog_vv())
            } else {
                let mut vv = sync.own[document].pushed.clone();
                vv.merge(&target);
                let spans: Vec<IdSpan> =
                    vv.iter().map(|(&p, &end)| IdSpan::new(p, 0, end)).collect();
                (doc.export(ExportMode::updates_in_range(spans))?, vv)
            };
            let context = Context {
                project: &sync.project,
                document,
                kind: "snapshot",
                name: &device,
            };
            let snapshot = seal_v2(&keyring, &device, &sync.keys, &context, level, &update)
                .map_err(from_protocol)?;
            sync.own_mut(document).compacting = true;
            let path = sync.snapshot_path(document, &device, Some(length));
            sync.request(
                Method::Put,
                path,
                snapshot,
                Purpose::Compact {
                    document,
                    covers: length,
                    vv,
                },
            );
            return Ok(());
        }
        if let Some(record) = record {
            let batch = sync.next_batch.max(last_batch + 1);
            sync.next_batch = batch + 1;
            sync.own_mut(document).pending = Some((batch, target));
            sync.send(Message::Append {
                document: document.into(),
                batch,
                records: record,
            });
        }
        Ok(())
    }

    /// Uploads the indexed blobs this device has and the server lacks
    /// (checked with `HEAD` first). Returns whether some are on their way,
    /// so records naming them wait.
    fn upload_blobs(&mut self, sync: &mut ServerSync) -> Result<bool> {
        let mut waiting = false;
        for blob in self.indexed_blobs() {
            if sync.server_blobs.contains(&blob) || sync.failed_blobs.contains(&blob) {
                continue;
            }
            if sync.blobs_inflight.contains(&blob) {
                waiting = true;
                continue;
            }
            if sync
                .blob_retry
                .get(&blob)
                .is_some_and(|at| *at > sync.clock)
            {
                continue;
            }
            let path = format!("/v1/projects/{}/assets/{blob}", sync.project);
            if sync.blobs_to_put.contains(&blob) {
                let Some(bytes) = self.storage.read(&blob_path(&blob))? else {
                    sync.blobs_to_put.remove(&blob);
                    continue;
                };
                let keyring = sync
                    .keyring
                    .clone()
                    .ok_or_else(|| Error::InvalidChange("the project key is missing".into()))?;
                let context = Context {
                    project: &sync.project,
                    document: MAIN,
                    kind: "asset",
                    name: &blob,
                };
                let device = self.identity.device.as_str();
                let level = sync.project_level();
                let sealed = seal_v2(&keyring, device, &sync.keys, &context, level, &bytes)
                    .map_err(from_protocol)?;
                sync.blobs_inflight.insert(blob.clone());
                sync.request(Method::Put, path, sealed, Purpose::BlobPut(blob));
                waiting = true;
            } else if self.storage.size(&blob_path(&blob))?.is_some() {
                sync.blobs_inflight.insert(blob.clone());
                sync.request(Method::Head, path, Vec::new(), Purpose::BlobHead(blob));
                waiting = true;
            }
        }
        Ok(waiting)
    }

    /// Imports what [`ServerSync`] has queued into `main` and `review` (see
    /// the module docs, like [`Document::sync_pull`]: never on the undo
    /// stacks, the next local edit of each starts an undo step of its own),
    /// stores the blobs that arrived and requests the indexed ones still
    /// missing here. The read positions advance with the imports.
    pub fn server_pull(&mut self, sync: &mut ServerSync) -> Result<Pulled> {
        self.set_role(sync.role());
        let mut pulled = Pulled::default();
        // Local edits must not share a commit with, or follow, the imports.
        self.doc.commit();
        self.review.doc.commit();
        let before = self.doc.oplog_frontiers();
        let review_before = self.review.doc.oplog_frontiers();
        let main_queued = sync
            .queue
            .iter()
            .any(|q| q.key.0 == MAIN && q.update.is_some());
        let old = if main_queued {
            self.text()
        } else {
            String::new()
        };
        import_queue(
            sync,
            (&self.doc, &mut self.trust),
            (&self.review.doc, &mut self.review.trust),
            &self.identity,
            &mut pulled.skipped,
        );
        let after = self.doc.oplog_frontiers();
        if after != before {
            pulled.changed = true;
            report(&self.doc.diff(&before, &after)?, &old, &mut pulled);
        }
        pulled.review = self.review.doc.oplog_frontiers() != review_before;
        self.after_pull(&pulled);
        self.renew_contested_peers()?;
        for (blob, bytes) in std::mem::take(&mut sync.arrived) {
            self.storage.create_dir(crate::ASSETS)?;
            self.storage.write_atomic(&blob_path(&blob), &bytes)?;
            pulled.blobs += 1;
        }
        let vv = self.doc.oplog_vv();
        let retry_due = sync.blob_retry.values().any(|at| *at <= sync.clock);
        let check = sync.missing_checked.as_ref() != Some(&vv) || pulled.blobs > 0 || retry_due;
        if check && sync.token.is_some() {
            let present: BTreeSet<String> = self
                .storage
                .list(crate::ASSETS)?
                .into_iter()
                .filter(|n| is_blob_name(n))
                .collect();
            let missing: BTreeSet<String> = self
                .indexed_blobs()
                .into_iter()
                .filter(|b| !present.contains(b))
                .collect();
            sync.want_blobs(&missing);
            sync.missing_checked = Some(vv);
        }
        pulled.skipped.append(&mut sync.skipped);
        Ok(pulled)
    }

    /// Autosaves, then keeps `sync`'s settings (read positions, pushed
    /// version, own stream) in the state file if they changed, once the
    /// project is shared. Positions only cover imported data, so they never
    /// get ahead of the saved document. Also follows `sync`'s role (see
    /// [`Document::set_role`]).
    pub fn server_save(&mut self, sync: &ServerSync) -> Result<()> {
        self.set_role(sync.role());
        self.autosave()?;
        if !sync.established {
            return Ok(());
        }
        let settings = self.server_settings_of(sync);
        if self.server.as_ref() != Some(&settings) {
            self.write_state(self.mirrored.as_ref(), self.sync.as_ref(), Some(&settings))?;
            self.server = Some(settings);
        }
        Ok(())
    }

    /// Builds a new local project in `local` from a join that is ready
    /// ([`ServerSync::join_ready`]): the catch-up becomes the document, the
    /// members log and the settings are kept, `project.json` is written
    /// last. Edited by `identity`'s device as the project user the join
    /// chose. Fails with [`Error::AlreadyExists`] if `local` holds a project.
    pub fn server_join(
        local: Box<dyn Storage>,
        sync: &mut ServerSync,
        identity: &Identity,
    ) -> Result<Document> {
        if !sync.join_ready() {
            return Err(Error::InvalidChange("the join isn't complete yet".into()));
        }
        let identity = Identity {
            device: identity.device.clone(),
            user: UserId::parse(&sync.user)?,
        };
        let device = &identity.device;
        local.create_dir("")?;
        let lock = local.lock(&lock_path(device))?;
        if local.read(PROJECT)?.is_some() || has_format1(&*local)? {
            return Err(Error::AlreadyExists(local.location()));
        }
        let (doc, review_doc) = (new_loro_doc(), new_loro_doc());
        let (mut trust, mut review_trust) = (Trust::default(), Trust::default());
        let mut skipped = Vec::new();
        import_queue(
            sync,
            (&doc, &mut trust),
            (&review_doc, &mut review_trust),
            &identity,
            &mut skipped,
        );
        sync.skipped.append(&mut skipped);
        local.create_dir(MAIN)?;
        local.create_dir(CHECKPOINTS)?;
        local.write_atomic(&snapshot_path(device), &export_snapshot(&doc)?)?;
        let mut log = local.open_log(&log_path(device))?;
        log.truncate(0)?;
        local.write_atomic(&checkpoints_path(device), b"[]")?;
        local.create_dir(SERVER_DIR)?;
        local.write_atomic(MEMBERS_LOG, &sync.members_bytes)?;
        let mut members_log = local.open_log(MEMBERS_LOG)?;
        members_log.read_all()?;
        let saved_vv = doc.oplog_vv();
        let mut document =
            Document::from_parts(local, &identity, doc, log, saved_vv, Vec::new(), None, lock);
        document.trust = trust;
        let review_files = Files {
            dir: REVIEW,
            log: None,
            saved_vv: VersionVector::new(),
        };
        document.review = review::Review::new(review_doc, review_files, Reads::new());
        document.review.trust = review_trust;
        // `review`'s own files: everything the catch-up brought.
        document.review.files.save(
            &*document.storage,
            device,
            &document.review.doc,
            document.compaction_threshold,
        )?;
        for (blob, bytes) in std::mem::take(&mut sync.arrived) {
            document.storage.create_dir(crate::ASSETS)?;
            document.storage.write_atomic(&blob_path(&blob), &bytes)?;
        }
        sync.members_log = Some(members_log);
        sync.phase = Phase::Device;
        sync.established = true;
        // A device that was in the project before read its own earlier
        // changes from its streams: they are on the server already.
        for name in DOCUMENTS {
            let (doc, trust) = document.synced_doc(name);
            sync.own_mut(name).pushed = trust.own_version(doc, &identity);
        }
        let settings = document.server_settings_of(sync);
        document.write_state(None, None, Some(&settings))?;
        document.server = Some(settings);
        document.set_role(sync.role());
        // Written last: `project.json` marks the folder as a complete project.
        write_project_file(&*document.storage)?;
        Ok(document)
    }

    /// Makes this a local project again (after leaving, being removed or the
    /// project's deletion from the server; the app stops the sync first):
    /// the server settings leave the state file, the members log kept here
    /// is emptied, and the project is writable. It can be shared again, as a
    /// new project.
    pub fn server_forget(&mut self) -> Result<()> {
        self.write_state(self.mirrored.as_ref(), self.sync.as_ref(), None)?;
        self.server = None;
        self.trust = Trust::default();
        self.review.trust = Trust::default();
        self.set_role(None);
        if self.storage.size(MEMBERS_LOG)?.is_some() {
            self.storage.open_log(MEMBERS_LOG)?.truncate(0)?;
        }
        Ok(())
    }

    /// Publishes where this device's user is (see the module docs,
    /// "Presence"): view `view` (`author`, `science` or `publisher`) and the
    /// selection from UTF-16 offset `anchor_utf16` to `head_utf16` (equal for
    /// a caret; past the end means the end), at unix ms `now_ms`. Allowed
    /// while read only.
    pub fn server_set_presence(
        &self,
        sync: &mut ServerSync,
        view: &str,
        anchor_utf16: usize,
        head_utf16: usize,
        now_ms: u64,
    ) -> Result<()> {
        if !VIEWS.contains(&view) {
            return Err(Error::InvalidChange(format!("no view {view:?}")));
        }
        let len = self.len_utf16();
        let anchor = self.anchor_at(anchor_utf16.min(len))?;
        let head = if head_utf16 == anchor_utf16 {
            anchor.clone()
        } else {
            self.anchor_at(head_utf16.min(len))?
        };
        let value = LoroValue::from(HashMap::from([
            ("view", LoroValue::from(view)),
            ("anchor", anchor.into()),
            ("head", head.into()),
        ]));
        sync.set_presence(value, now_ms);
        Ok(())
    }

    /// The other devices' presence (see the module docs, "Presence"), their
    /// selections resolved in the current text.
    pub fn server_presence(&self, sync: &ServerSync) -> Vec<PresenceInfo> {
        let pos = |anchor: &str| self.resolve_anchor(anchor).ok().map(|p| p.pos_utf16);
        sync.presence_entries()
            .into_iter()
            .map(|(info, anchor, head)| PresenceInfo {
                anchor: pos(&anchor),
                head: pos(&head),
                ..info
            })
            .collect()
    }

    /// Who wrote the text from UTF-16 offset `from_utf16` to `to_utf16` (see
    /// [`Document::authors_in`]), with the authors' profiles (see the module
    /// docs, "Who wrote this").
    pub fn server_authors(&self, sync: &ServerSync, from_utf16: usize, to_utf16: usize) -> Authors {
        let runs = self.authors_in(from_utf16, to_utf16);
        let users: BTreeSet<&str> = runs
            .iter()
            .filter_map(|r| Some(r.user.as_ref()?.as_str()))
            .collect();
        let authors = users
            .into_iter()
            .map(|user| {
                let profile = sync.profile_of(user);
                AuthorInfo {
                    user: user.to_owned(),
                    colour: profile
                        .as_ref()
                        .map_or_else(|| profile_colour(user).to_owned(), |p| p.colour.clone()),
                    name: profile.map(|p| p.name),
                    you: user == sync.user,
                }
            })
            .collect();
        Authors { runs, authors }
    }
}

/// Imports `sync`'s queue into `main` and `review`, each with the origins
/// of its peers (see the `trust` module), moving the read positions along;
/// `me` is this device. Updates Loro rejects are reported in `skipped`.
fn import_queue(
    sync: &mut ServerSync,
    main: (&LoroDoc, &mut Trust),
    review: (&LoroDoc, &mut Trust),
    me: &Identity,
    skipped: &mut Vec<String>,
) {
    let ((main_doc, main_trust), (review_doc, review_trust)) = (main, review);
    while let Some(item) = sync.queue.pop_front() {
        if let Some((update, user)) = &item.update {
            let (doc, trust) = if item.key.0 == REVIEW {
                (review_doc, &mut *review_trust)
            } else {
                (main_doc, &mut *main_trust)
            };
            if let Err(e) = trust.import(doc, update, &item.key.1, user, me) {
                skipped.push(format!("an update from the server: {e}"));
            }
        }
        if let Some(pos) = item.pos {
            sync.imported.insert(item.key, pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lethora_protocol::frame::frame as frame_record;
    use lethora_protocol::random_id;
    use loro::ID;

    fn ids() -> (DeviceId, UserId) {
        (
            DeviceId::parse(&random_id().unwrap()).unwrap(),
            UserId::parse(&random_id().unwrap()).unwrap(),
        )
    }

    #[test]
    fn colours_come_from_the_palette_by_user() {
        assert_eq!(profile_colour("00aa"), PALETTE[0]);
        assert_eq!(profile_colour("09aa"), PALETTE[1]);
        assert_eq!(profile_colour("ff00"), PALETTE[7]);
        assert_eq!(profile_colour("zz"), PALETTE[0]);
    }

    #[test]
    fn settings_round_trip_as_json() {
        let mut settings = ServerSettings {
            url: "https://lethora.example".into(),
            project: random_id().unwrap(),
            user: random_id().unwrap(),
            streams: BTreeMap::new(),
            pushed: String::new(),
            own: OwnPos {
                generation: 1,
                length: 99,
                batch: 4,
            },
            review: OwnState::default(),
            members: 300,
            origins: BTreeMap::new(),
            revoked: BTreeSet::new(),
        };
        // State written before `review` was synced reads with it fresh.
        let old = serde_json::to_string(&settings).unwrap();
        assert!(!old.contains("origins") && !old.contains("revoked"));
        let old = old.replace(
            ",\"review\":{\"own\":{\"generation\":0,\"length\":0,\"batch\":0}}",
            "",
        );
        assert!(!old.contains("review"), "{old}");
        assert_eq!(
            serde_json::from_str::<ServerSettings>(&old).unwrap(),
            settings
        );
        settings.review.own.batch = 7;
        settings.revoked.insert("main/0123".into());
        settings.origins.insert(
            MAIN.into(),
            BTreeMap::from([(
                "42".to_owned(),
                PeerOrigin {
                    device: "0123".into(),
                    user: "4567".into(),
                    end: 9,
                    open: true,
                },
            )]),
        );
        settings.streams.insert(
            "main/0123".into(),
            StreamPos {
                generation: 2,
                offset: 10,
                needs_snapshot: true,
            },
        );
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("\"needs_snapshot\":true"));
        assert_eq!(
            serde_json::from_str::<ServerSettings>(&json).unwrap(),
            settings
        );
    }

    /// A sync in the device phase for a fresh members log with `device` as
    /// the owner's device.
    fn device_sync() -> (ServerSync, DeviceKeys, String) {
        let (device, user) = ids();
        let keys = DeviceKeys::generate().unwrap();
        let project = random_id().unwrap();
        let genesis =
            Members::genesis_entry(&project, user.as_str(), device.as_str(), &keys, 1).unwrap();
        let mut sync = ServerSync::new(
            "https://lethora.example".into(),
            project,
            &device,
            user.as_str().into(),
            keys.clone(),
            Some(Keyring::new([7; 32])),
            Phase::Device,
        );
        sync.established = true;
        sync.add_members(0, &genesis).unwrap();
        (sync, keys, device.as_str().to_owned())
    }

    fn welcome(sync: &mut ServerSync, generation: u64, length: u64, batch: u64) {
        sync.connected();
        sync.handle(Message::Welcome {
            token: "t".into(),
            role: Some(Role::Owner),
            streams: vec![OwnStream {
                document: MAIN.into(),
                generation,
                length,
                batch,
            }],
            app: "9.9.9".into(),
            devices: Vec::new(),
        });
    }

    #[test]
    fn welcome_subscribes_from_the_positions_and_the_own_stream() {
        let (mut sync, ..) = device_sync();
        let other = random_id().unwrap();
        let pos = StreamPos {
            generation: 1,
            offset: 40,
            needs_snapshot: false,
        };
        sync.received.insert((MAIN.into(), other.clone()), pos);
        let pending = random_id().unwrap();
        sync.received.insert(
            (MAIN.into(), pending.clone()),
            StreamPos {
                needs_snapshot: true,
                ..pos
            },
        );
        welcome(&mut sync, 2, 77, 5);
        let actions = sync.take_actions();
        let Some(Action::Send(Message::Subscribe { members, streams })) = actions.last() else {
            panic!("{actions:?}");
        };
        assert_eq!(*members, sync.members_bytes.len() as u64);
        // The stream still needing its snapshot is read from scratch.
        assert!(!streams.iter().any(|p| p.device == pending));
        assert!(streams.iter().any(|p| p.device == other && p.offset == 40));
        assert!(
            streams
                .iter()
                .any(|p| p.device == sync.device && p.generation == 2 && p.offset == 77)
        );
        assert_eq!(
            sync.own[MAIN].pos,
            OwnPos {
                generation: 2,
                length: 77,
                batch: 5
            }
        );
    }

    #[test]
    fn a_refused_batch_holds_up_only_its_document() {
        let (mut sync, ..) = device_sync();
        welcome(&mut sync, 0, 0, 4);
        // Batch ids continue above every stream's and never repeat.
        assert_eq!(sync.next_batch, 5);
        let mut vv = VersionVector::new();
        vv.set_end(ID::new(1, 10));
        sync.own_mut(MAIN).pending = Some((5, vv.clone()));
        sync.own_mut(REVIEW).pending = Some((6, vv.clone()));
        sync.handle(Message::Nack {
            batch: 6,
            code: Code::Forbidden,
            message: "no".into(),
        });
        assert!(sync.own[REVIEW].pending.is_none());
        assert!(!sync.may_push(REVIEW), "waits before trying again");
        assert!(sync.status().message.unwrap().contains("review"));
        assert_eq!(sync.own[MAIN].pending.as_ref().map(|p| p.0), Some(5));
        sync.handle(Message::Ack {
            batch: 5,
            document: MAIN.into(),
            generation: 0,
            length: 50,
        });
        assert_eq!(sync.own[MAIN].pushed.get(&1), Some(&10));
        assert!(sync.own[REVIEW].pushed.is_empty());
        assert!(sync.may_push(MAIN));
        // A reviewer pushes `review` only, a reader nothing.
        sync.role = Some(Role::Reviewer);
        assert!(!sync.may_push(MAIN));
        sync.clock += RETRY_SECS;
        assert!(sync.may_push(REVIEW));
        sync.role = Some(Role::Reader);
        assert!(!sync.may_push(MAIN) && !sync.may_push(REVIEW));
    }

    #[test]
    fn a_batch_in_flight_is_resolved_by_ack_or_by_the_next_welcome() {
        let (mut sync, ..) = device_sync();
        welcome(&mut sync, 0, 0, 0);
        let mut vv = VersionVector::new();
        vv.set_end(ID::new(1, 10));
        sync.own_mut(MAIN).pending = Some((1, vv.clone()));
        sync.handle(Message::Ack {
            batch: 1,
            document: MAIN.into(),
            generation: 0,
            length: 120,
        });
        assert_eq!(sync.own[MAIN].pushed.get(&1), Some(&10));
        assert_eq!(sync.own[MAIN].pos.length, 120);
        assert!(sync.own_mut(MAIN).pending.is_none());

        // Lost ack, landed: the welcome's last batch reached it.
        vv.set_end(ID::new(1, 20));
        sync.own_mut(MAIN).pending = Some((2, vv.clone()));
        sync.disconnected();
        welcome(&mut sync, 0, 250, 2);
        assert_eq!(sync.own[MAIN].pushed.get(&1), Some(&20));
        assert_eq!(sync.own[MAIN].pos.batch, 2);
        assert!(sync.own_mut(MAIN).pending.is_none());

        // Lost, not landed: the next push sends it again with batch 3.
        vv.set_end(ID::new(1, 30));
        sync.own_mut(MAIN).pending = Some((3, vv));
        sync.disconnected();
        welcome(&mut sync, 0, 250, 2);
        assert_eq!(sync.own[MAIN].pushed.get(&1), Some(&20));
        assert!(sync.own_mut(MAIN).pending.is_none());
        assert_eq!(sync.own[MAIN].pos.batch + 1, 3);
    }

    #[test]
    fn compaction_shifts_positions() {
        let (mut sync, ..) = device_sync();
        welcome(&mut sync, 0, 500, 3);
        let other = (MAIN.to_owned(), random_id().unwrap());
        sync.received.insert(
            other.clone(),
            StreamPos {
                generation: 0,
                offset: 300,
                needs_snapshot: false,
            },
        );
        sync.handle(Message::Compacted {
            document: MAIN.into(),
            device: other.1.clone(),
            generation: 1,
            covers: 200,
        });
        assert_eq!(sync.received[&other].offset, 100);
        assert_eq!(sync.received[&other].generation, 1);
        // Nothing queued: imported right away.
        assert_eq!(sync.imported[&other].offset, 100);
        // A second notice of the same compaction changes nothing.
        sync.handle(Message::Compacted {
            document: MAIN.into(),
            device: other.1.clone(),
            generation: 1,
            covers: 200,
        });
        assert_eq!(sync.received[&other].offset, 100);
        // The own stream.
        let own = sync.device.clone();
        sync.handle(Message::Compacted {
            document: MAIN.into(),
            device: own,
            generation: 1,
            covers: 500,
        });
        assert_eq!(sync.own[MAIN].pos.generation, 1);
        assert_eq!(sync.own[MAIN].pos.length, 0);
        // A stream read from its snapshot needs it before it counts.
        sync.handle(Message::SnapshotChanged {
            document: MAIN.into(),
            device: other.1.clone(),
            generation: 2,
            covers: 100,
        });
        assert!(sync.received[&other].needs_snapshot);
        let actions = sync.take_actions();
        assert!(actions.iter().any(|a| matches!(
            a,
            Action::Http(HttpRequest { method: Method::Get, path, .. }) if path.ends_with("/snapshot")
        )));
    }

    #[test]
    fn members_log_must_continue_the_stored_one() {
        let (mut sync, keys, device) = device_sync();
        let members = sync.members.clone().unwrap();
        let link = InviteLink::generate("https://lethora.example", &sync.project).unwrap();
        let record = members
            .entry(
                &device,
                2,
                Op::Invite {
                    id: link.id.clone(),
                    key: link.keys().verifying_key(),
                    exchange: link.keys().exchange_key(),
                    role: Some(Role::Editor),
                    user: None,
                    expires: 100,
                    uses: 1,
                },
                &[&keys],
            )
            .unwrap();
        let len = sync.members_bytes.len() as u64;
        // The whole log again, then the new record: fine.
        let mut all = sync.members_bytes.clone();
        all.extend_from_slice(&record);
        sync.handle(Message::MembersRecords {
            offset: 0,
            bytes: all,
        });
        assert!(!sync.is_stopped());
        assert!(
            sync.members
                .as_ref()
                .unwrap()
                .invites()
                .contains_key(&link.id)
        );
        assert_eq!(sync.members_bytes.len() as u64, len + record.len() as u64);

        // Other bytes at a known offset: stopped.
        let mut other = record.clone();
        let last = other.len() - 1;
        other[last] ^= 1;
        sync.handle(Message::MembersRecords {
            offset: len,
            bytes: other,
        });
        assert!(sync.is_stopped());
        assert_eq!(sync.status().state, ServerState::Error);

        // An invalid record: stopped.
        let (mut sync, ..) = device_sync();
        let len = sync.members_bytes.len() as u64;
        sync.handle(Message::MembersRecords {
            offset: len,
            bytes: frame_record(b"nonsense").unwrap(),
        });
        assert!(sync.is_stopped());
    }

    #[test]
    fn a_push_holds_only_this_devices_changes() {
        let dir = tempfile::tempdir().unwrap();
        let identity = |device: &DeviceId| Identity {
            device: device.clone(),
            user: UserId::parse(&random_id().unwrap()).unwrap(),
        };
        let (mut sync, ..) = device_sync();
        let b_device = DeviceId::parse(&sync.device).unwrap();
        let mut a = Document::create(&dir.path().join("a"), &identity(&ids().0)).unwrap();
        a.insert(0, "from A").unwrap();
        a.autosave().unwrap();
        let mut b = Document::create(&dir.path().join("b"), &identity(&b_device)).unwrap();
        b.doc
            .import(&a.doc.export(ExportMode::all_updates()).unwrap())
            .unwrap();
        b.insert(6, ", and B").unwrap();
        let peers = |d: &Document| -> Vec<PeerID> {
            let vv = d.trust.own_version(&d.doc, &d.identity);
            vv.keys().copied().collect()
        };
        let (a_peers, b_peers) = (peers(&a), peers(&b));
        assert!(!a_peers.is_empty() && !b_peers.is_empty());

        welcome(&mut sync, 0, 0, 0);
        sync.take_actions();
        b.server_push(&mut sync).unwrap();
        let actions = sync.take_actions();
        let Some(Action::Send(Message::Append { batch, records, .. })) = actions.last() else {
            panic!("{actions:?}");
        };
        assert_eq!(*batch, 1);
        let Frame::Record(envelope, _) = parse_frame(records, 0) else {
            panic!("a record");
        };
        let (update, level) = sync
            .open(MAIN, &sync.device.clone(), "log", envelope)
            .unwrap();
        assert_eq!(level, 1);
        let meta = loro::LoroDoc::decode_import_blob_meta(&update, true).unwrap();
        let peers: Vec<PeerID> = meta.partial_end_vv.keys().copied().collect();
        assert!(!peers.is_empty());
        assert!(
            peers
                .iter()
                .all(|p| b_peers.contains(p) && !a_peers.contains(p))
        );

        // In flight: nothing more goes out; after the ack, nothing is new.
        b.server_push(&mut sync).unwrap();
        assert!(sync.take_actions().is_empty());
        sync.handle(Message::Ack {
            batch: 1,
            document: MAIN.into(),
            generation: 0,
            length: records.len() as u64,
        });
        sync.take_actions();
        b.server_push(&mut sync).unwrap();
        assert!(sync.take_actions().is_empty());

        // At a raised project level, records are sealed at it.
        sync.set_reader_level(2);
        sync.level = 2;
        b.insert(0, "2").unwrap();
        b.server_push(&mut sync).unwrap();
        let actions = sync.take_actions();
        let Some(Action::Send(Message::Append { records, .. })) = actions.last() else {
            panic!("{actions:?}");
        };
        let Frame::Record(envelope, _) = parse_frame(records, 0) else {
            panic!("a record");
        };
        assert_eq!(header_v2(envelope).unwrap().level, 2);
    }

    #[test]
    fn records_are_checked_and_queued() {
        let (mut sync, ..) = device_sync();
        welcome(&mut sync, 0, 0, 0);
        sync.take_actions();
        let keyring = sync.keyring.clone().unwrap();
        // A stranger's record: skipped, the position still moves.
        let stranger = random_id().unwrap();
        let stranger_keys = DeviceKeys::generate().unwrap();
        let context = Context {
            project: &sync.project.clone(),
            document: MAIN,
            kind: "log",
            name: &stranger,
        };
        let sealed = seal_v2(&keyring, &stranger, &stranger_keys, &context, 1, b"x").unwrap();
        let bytes = frame_record(&sealed).unwrap();
        sync.handle(Message::Records {
            document: MAIN.into(),
            device: stranger.clone(),
            generation: 0,
            offset: 0,
            bytes: bytes.clone(),
        });
        assert!(sync.queue.is_empty());
        assert_eq!(sync.skipped.len(), 1);
        let key = (MAIN.to_owned(), stranger);
        assert_eq!(sync.imported[&key].offset, bytes.len() as u64);

        // The own device's record (signed by it): queued, with the position.
        let own = sync.device.clone();
        let keys = sync.keys.clone();
        let context = Context {
            project: &sync.project.clone(),
            document: MAIN,
            kind: "log",
            name: &own,
        };
        let sealed = seal_v2(&keyring, &own, &keys, &context, 1, b"update").unwrap();
        // Records of the own stream are never read back.
        sync.handle(Message::Records {
            document: MAIN.into(),
            device: own,
            generation: 0,
            offset: 0,
            bytes: frame_record(&sealed).unwrap(),
        });
        assert!(sync.queue.is_empty());
    }

    #[test]
    fn documents_not_synced_here_keep_no_read_position() {
        let (mut sync, ..) = device_sync();
        welcome(&mut sync, 0, 0, 0);
        sync.take_actions();
        // A later version's document: nothing is kept, so once a version
        // syncs it, its streams are read from the start.
        let key = ("later".to_owned(), random_id().unwrap());
        sync.handle(Message::Records {
            document: key.0.clone(),
            device: key.1.clone(),
            generation: 0,
            offset: 0,
            bytes: b"anything".to_vec(),
        });
        sync.handle(Message::SnapshotChanged {
            document: key.0.clone(),
            device: key.1.clone(),
            generation: 1,
            covers: 100,
        });
        assert!(sync.queue.is_empty());
        assert!(sync.skipped.is_empty());
        assert!(!sync.received.contains_key(&key));
        assert!(!sync.imported.contains_key(&key));
        assert!(sync.take_actions().is_empty());
    }

    /// A framed record of `member`'s `main` stream in `sync`'s project,
    /// needing `level`.
    fn member_record(sync: &ServerSync, member: &Member, level: u32, update: &[u8]) -> Vec<u8> {
        let context = Context {
            project: &sync.project,
            document: MAIN,
            kind: "log",
            name: &member.device,
        };
        let keyring = sync.keyring.as_ref().unwrap();
        let sealed = seal_v2(
            keyring,
            &member.device,
            &member.keys,
            &context,
            level,
            update,
        );
        frame_record(&sealed.unwrap()).unwrap()
    }

    /// `sync` opened again (as after an update of the app) from what it
    /// imported, reading at `level`.
    fn reopened(sync: &ServerSync, level: u32) -> ServerSync {
        let mut again = ServerSync::new(
            sync.url.clone(),
            sync.project.clone(),
            &DeviceId::parse(&sync.device).unwrap(),
            sync.user.clone(),
            sync.keys.clone(),
            sync.keyring.clone(),
            Phase::Device,
        );
        again.established = true;
        again.set_reader_level(level);
        again.add_members(0, &sync.members_bytes).unwrap();
        again.received = sync.imported.clone();
        again.imported = sync.imported.clone();
        welcome(&mut again, 0, 0, 0);
        again.take_actions();
        again
    }

    #[test]
    fn a_newer_record_stops_its_stream_before_it() {
        let (mut sync, member) = with_member();
        assert_eq!(sync.project_level(), 1);
        let key = (MAIN.to_owned(), member.device.clone());
        let first = member_record(&sync, &member, 1, b"a");
        let newer = member_record(&sync, &member, 2, b"b");
        let after = member_record(&sync, &member, 1, b"c");
        sync.handle(Message::Records {
            document: MAIN.into(),
            device: member.device.clone(),
            generation: 0,
            offset: 0,
            bytes: [&first[..], &newer, &after].concat(),
        });
        // The record before it is queued, the position stays at its start.
        let queued: Vec<_> = sync.queue.iter().filter_map(|q| q.update.clone()).collect();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].0, b"a");
        assert_eq!(sync.received[&key].offset, first.len() as u64);
        assert!(sync.skipped.is_empty());
        let status = sync.status();
        assert_eq!(status.state, ServerState::NeedsUpdate);
        assert_eq!(
            status.message.as_deref(),
            Some("This project now needs a newer Lethora.")
        );
        assert!(sync.is_stopped());
        // Nothing more is taken, nothing pushed.
        sync.handle(Message::Records {
            document: MAIN.into(),
            device: member.device.clone(),
            generation: 0,
            offset: (first.len() + newer.len()) as u64,
            bytes: after.clone(),
        });
        assert_eq!(sync.queue.len(), 1);
        assert!(!sync.may_push(MAIN));
        let mut skipped = Vec::new();
        let (doc, review) = (new_loro_doc(), new_loro_doc());
        let me = Identity {
            device: DeviceId::parse(&sync.device).unwrap(),
            user: UserId::parse(&sync.user).unwrap(),
        };
        import_queue(
            &mut sync,
            (&doc, &mut Trust::default()),
            (&review, &mut Trust::default()),
            &me,
            &mut skipped,
        );
        assert_eq!(sync.imported[&key].offset, first.len() as u64);

        // After the update the stream goes on from that record, and writes
        // follow the level read.
        let mut updated = reopened(&sync, 2);
        updated.handle(Message::Records {
            document: MAIN.into(),
            device: member.device.clone(),
            generation: 0,
            offset: first.len() as u64,
            bytes: [&newer[..], &after].concat(),
        });
        assert!(!updated.is_stopped());
        let queued: Vec<_> = updated
            .queue
            .iter()
            .filter_map(|q| Some(q.update.clone()?.0))
            .collect();
        assert_eq!(queued, [b"b".to_vec(), b"c".to_vec()]);
        assert_eq!(updated.project_level(), 2);
    }

    #[test]
    fn a_newer_snapshot_or_envelope_version_stops_too() {
        let (mut sync, member) = with_member();
        let key = (MAIN.to_owned(), member.device.clone());
        // A newer envelope version: not opened, not skipped.
        let mut unknown = member_record(&sync, &member, 1, b"a");
        unknown[8] = 9;
        let unknown = frame_record(&unknown[8..]).unwrap();
        sync.handle(Message::Records {
            document: MAIN.into(),
            device: member.device.clone(),
            generation: 0,
            offset: 0,
            bytes: unknown,
        });
        assert_eq!(sync.status().state, ServerState::NeedsUpdate);
        assert_eq!(sync.received[&key].offset, 0);

        // A snapshot of a higher level: still needed after the update.
        let (mut sync, member) = with_member();
        let key = (MAIN.to_owned(), member.device.clone());
        sync.handle(Message::SnapshotChanged {
            document: MAIN.into(),
            device: member.device.clone(),
            generation: 1,
            covers: 10,
        });
        let id = *sync.http.keys().next().unwrap();
        let context = Context {
            project: &sync.project,
            document: MAIN,
            kind: "snapshot",
            name: &member.device,
        };
        let keyring = sync.keyring.clone().unwrap();
        let snapshot = seal_v2(&keyring, &member.device, &member.keys, &context, 2, b"s");
        sync.http_result(
            id,
            Ok(HttpResponse {
                status: 200,
                body: snapshot.unwrap(),
                generation: Some(1),
            }),
        );
        assert_eq!(sync.status().state, ServerState::NeedsUpdate);
        assert!(sync.queue.iter().all(|q| q.update.is_none()));
        assert!(sync.imported[&key].needs_snapshot);
        assert!(sync.snapshot_retry.is_empty());
    }

    #[test]
    fn newer_members_entries_and_version_refusals_need_an_update() {
        // An entry of an op this version doesn't know, signed by its device.
        let (mut sync, keys, device) = device_sync();
        let head = sync.members.as_ref().unwrap().head().clone();
        let raw = format!(
            "{{\"seq\":1,\"prev\":\"{}\",\"project\":\"{}\",\"time\":2,\"by\":\"{device}\",\
             \"op\":{{\"type\":\"future\"}}}}",
            head.hash, sync.project
        );
        let mut entry = (raw.len() as u32).to_le_bytes().to_vec();
        entry.extend_from_slice(raw.as_bytes());
        entry.push(1);
        entry.extend_from_slice(&keys.sign(b"lethora-members-1\0", raw.as_bytes()));
        let len = sync.members_bytes.len() as u64;
        sync.handle(Message::MembersRecords {
            offset: len,
            bytes: frame_record(&entry).unwrap(),
        });
        assert_eq!(sync.status().state, ServerState::NeedsUpdate);
        assert_eq!(sync.members_bytes.len() as u64, len);

        // A `require` above this version's level names the app.
        let (mut sync, keys, device) = device_sync();
        let op = Op::Require {
            level: 2,
            app: "0.2.0".into(),
        };
        let record = sync
            .members
            .as_ref()
            .unwrap()
            .entry(&device, 2, op, &[&keys])
            .unwrap();
        let len = sync.members_bytes.len() as u64;
        sync.handle(Message::MembersRecords {
            offset: len,
            bytes: record,
        });
        let status = sync.status();
        assert_eq!(status.state, ServerState::NeedsUpdate);
        assert_eq!(
            status.message.as_deref(),
            Some("This project now needs Lethora 0.2.0 or later.")
        );

        // The server's refusal, in its words.
        let (mut sync, ..) = device_sync();
        sync.handle(Message::Refused {
            code: Code::Version,
            message: "This project needs Lethora 0.2.0 or later.".into(),
        });
        let status = sync.status();
        assert_eq!(status.state, ServerState::NeedsUpdate);
        assert_eq!(
            status.message.as_deref(),
            Some("This project needs Lethora 0.2.0 or later.")
        );
    }

    #[test]
    fn requiring_a_level_raises_the_writing_level_once_it_lands() {
        let (mut sync, member) = with_member();
        assert!(matches!(
            sync.require(2, "0.2.0"),
            Err(Error::InvalidChange(_))
        ));
        sync.set_reader_level(2);
        // Not to the level the project has.
        assert!(matches!(
            sync.require(1, "0.1.0"),
            Err(Error::NotAllowed(_))
        ));
        let id = sync.require(2, "0.2.0").unwrap();
        let (batch, record) = sent_entry(&mut sync);
        assert!(matches!(
            entry_body(&record).unwrap().op,
            Op::Require { level: 2, .. }
        ));
        assert_eq!(sync.project_level(), 1);
        ack(&mut sync, batch);
        assert_eq!(sync.change_result(id), Some(Ok(())));
        assert_eq!(sync.project_level(), 2);
        assert!(!sync.is_stopped());

        // The welcome's last hellos say who is below a level.
        sync.connected();
        sync.handle(Message::Welcome {
            token: "t".into(),
            role: Some(Role::Owner),
            streams: Vec::new(),
            app: "0.2.0".into(),
            devices: vec![
                DeviceVersion {
                    device: sync.device.clone(),
                    version: ClientVersion {
                        level: 2,
                        app: "0.2.0".into(),
                    },
                },
                DeviceVersion {
                    device: member.device.clone(),
                    version: ClientVersion {
                        level: 1,
                        app: "0.1.0".into(),
                    },
                },
            ],
        });
        assert_eq!(sync.server_version(), Some("0.2.0"));
        assert_eq!(
            sync.devices_below(2),
            [(member.device.clone(), "0.1.0".to_owned())]
        );
        assert!(sync.devices_below(1).is_empty());
    }

    /// A second member: its device and keys, its user and the open invite
    /// (role reader) the owner made besides.
    struct Member {
        device: String,
        keys: DeviceKeys,
        user: String,
        invite: String,
    }

    /// The owner's sync (as [`device_sync`], connected and caught up) with
    /// an editor `Member` that joined, and an open reader invite.
    fn with_member() -> (ServerSync, Member) {
        let (mut sync, keys, device) = device_sync();
        let (b_device, b_user) = ids();
        let b_keys = DeviceKeys::generate().unwrap();
        let add = |sync: &mut ServerSync, record: Vec<u8>| {
            let have = sync.members_bytes.len() as u64;
            sync.add_members(have, &record).unwrap();
        };
        let link = InviteLink::generate("https://lethora.example", &sync.project).unwrap();
        let invite = |link: &InviteLink, role| Op::Invite {
            id: link.id.clone(),
            key: link.keys().verifying_key(),
            exchange: link.keys().exchange_key(),
            role: Some(role),
            user: None,
            expires: 1_000,
            uses: 1,
        };
        let members = sync.members.clone().unwrap();
        add(
            &mut sync,
            members
                .entry(&device, 2, invite(&link, Role::Editor), &[&keys])
                .unwrap(),
        );
        let members = sync.members.clone().unwrap();
        let join = Op::Join {
            invite: link.id.clone(),
            user: b_user.as_str().into(),
            device: b_device.as_str().into(),
            public: b_keys.public(),
            profile: None,
        };
        add(
            &mut sync,
            members
                .entry(b_device.as_str(), 3, join, &[&link.keys(), &b_keys])
                .unwrap(),
        );
        let other = InviteLink::generate("https://lethora.example", &sync.project).unwrap();
        let members = sync.members.clone().unwrap();
        add(
            &mut sync,
            members
                .entry(&device, 4, invite(&other, Role::Reader), &[&keys])
                .unwrap(),
        );
        sync.tick(10);
        welcome(&mut sync, 0, 0, 0);
        sync.handle(Message::Synced);
        sync.take_actions();
        let member = Member {
            device: b_device.as_str().into(),
            keys: b_keys,
            user: b_user.as_str().into(),
            invite: other.id,
        };
        (sync, member)
    }

    /// The member's own sync, on the same members log.
    fn member_sync(owner: &ServerSync, member: &Member) -> ServerSync {
        let mut sync = ServerSync::new(
            owner.url.clone(),
            owner.project.clone(),
            &DeviceId::parse(&member.device).unwrap(),
            member.user.clone(),
            member.keys.clone(),
            owner.keyring.clone(),
            Phase::Device,
        );
        sync.established = true;
        sync.add_members(0, &owner.members_bytes).unwrap();
        sync.tick(10);
        welcome(&mut sync, 0, 0, 0);
        sync.role = Some(Role::Editor);
        sync.handle(Message::Synced);
        sync.take_actions();
        sync
    }

    /// The members entry `sync` sent last, with its batch.
    fn sent_entry(sync: &mut ServerSync) -> (u64, Vec<u8>) {
        let actions = sync.take_actions();
        actions
            .into_iter()
            .find_map(|a| match a {
                Action::Send(Message::AppendMembers { batch, record }) => Some((batch, record)),
                _ => None,
            })
            .expect("a members entry was sent")
    }

    fn ack(sync: &mut ServerSync, batch: u64) {
        sync.handle(Message::MembersAck { batch, seq: 0 });
    }

    #[test]
    fn role_changes_removals_and_revokes_are_sent_and_acked() {
        let (mut sync, member) = with_member();
        let role = |s: &ServerSync| s.members.as_ref().unwrap().users()[&member.user].role;

        let id = sync.set_role(&member.user, Role::Reader).unwrap();
        assert_eq!(sync.change_result(id), None);
        let (batch, record) = sent_entry(&mut sync);
        assert!(matches!(
            entry_body(&record).unwrap().op,
            Op::Role {
                role: Role::Reader,
                ..
            }
        ));
        ack(&mut sync, batch);
        assert_eq!(sync.change_result(id), Some(Ok(())));
        assert_eq!(role(&sync), Role::Reader);

        let id = sync.revoke(&member.invite).unwrap();
        let (batch, _) = sent_entry(&mut sync);
        ack(&mut sync, batch);
        assert_eq!(sync.change_result(id), Some(Ok(())));
        assert!(sync.open_invites().is_empty());

        let id = sync.remove_user(&member.user).unwrap();
        let (batch, _) = sent_entry(&mut sync);
        ack(&mut sync, batch);
        assert_eq!(sync.change_result(id), Some(Ok(())));
        let list = sync.member_list();
        assert!(
            list.iter()
                .any(|m| m.user == member.user && m.removed && m.devices.is_empty())
        );
        assert!(!sync.is_stopped());
    }

    #[test]
    fn changes_the_rules_refuse_are_refused_here_first() {
        let (mut sync, member) = with_member();
        let owner = sync.user.clone();
        let own = sync.device.clone();
        let not_allowed = |r: Result<u64>| match r {
            Err(Error::NotAllowed(why)) => why,
            other => panic!("{other:?}"),
        };
        assert!(not_allowed(sync.set_role(&owner, Role::Editor)).contains("owner's role"));
        assert!(not_allowed(sync.set_role(&member.user, Role::Owner)).contains("owner"));
        assert!(not_allowed(sync.remove_user(&owner)).contains("owner can't be removed"));
        assert!(not_allowed(sync.remove_device(&own)).contains("last device"));
        assert!(not_allowed(sync.revoke(&random_id().unwrap())).contains("no invite"));
        assert!(
            not_allowed(sync.set_role(&random_id().unwrap(), Role::Editor))
                .contains("not a member")
        );
        // Nothing was queued or sent.
        assert!(sync.members_ops.is_empty() && sync.members_inflight.is_none());
        assert!(sync.take_actions().is_empty());

        // The member may not change roles, remove others or delete; it may
        // revoke nothing it didn't create.
        let mut theirs = member_sync(&sync, &member);
        assert!(
            not_allowed(theirs.set_role(&member.user, Role::Reader)).contains("only the owner")
        );
        assert!(not_allowed(theirs.remove_user(&owner)).contains("only the owner"));
        assert!(not_allowed(theirs.remove_device(&own)).contains("owner or its own user"));
        assert!(not_allowed(theirs.revoke(&member.invite)).contains("owner or its creator"));
        assert!(matches!(theirs.delete_project(), Err(Error::NotAllowed(_))));
    }

    #[test]
    fn a_refused_change_fails_and_a_landed_one_is_found_after_a_lost_ack() {
        let (mut sync, member) = with_member();
        let id = sync.set_role(&member.user, Role::Reviewer).unwrap();
        let (batch, _) = sent_entry(&mut sync);
        sync.handle(Message::Nack {
            batch,
            code: Code::Forbidden,
            message: "no".into(),
        });
        assert_eq!(sync.change_result(id), Some(Err("no".into())));

        // Lost ack: the entry is in the server's log when the device is back.
        let id = sync.set_role(&member.user, Role::Reviewer).unwrap();
        let (_, record) = sent_entry(&mut sync);
        sync.disconnected();
        let offset = sync.members_bytes.len() as u64;
        welcome(&mut sync, 0, 0, 0);
        sync.handle(Message::MembersRecords {
            offset,
            bytes: record,
        });
        sync.handle(Message::Synced);
        assert_eq!(sync.change_result(id), Some(Ok(())));
        let sent = sync
            .take_actions()
            .into_iter()
            .any(|a| matches!(a, Action::Send(Message::AppendMembers { .. })));
        assert!(!sent, "not sent again");
    }

    #[test]
    fn leaving_removes_the_own_device_and_stops() {
        let (owner, member) = with_member();
        let mut sync = member_sync(&owner, &member);
        let own = sync.device.clone();
        let id = sync.remove_device(&own).unwrap();
        let (batch, _) = sent_entry(&mut sync);
        ack(&mut sync, batch);
        assert_eq!(sync.change_result(id), Some(Ok(())));
        assert_eq!(sync.status().state, ServerState::Removed);
        // A member that lost its last device is removed with it.
        assert!(sync.members.as_ref().unwrap().users()[&member.user].removed);
    }

    #[test]
    fn a_free_colour_is_the_first_unused_one() {
        let used = |c: &[&str]| c.iter().map(|&c| c.to_owned()).collect::<Vec<_>>();
        assert_eq!(free_colour("ff00", &[]), PALETTE[0]);
        assert_eq!(
            free_colour("ff00", &used(&[PALETTE[0], PALETTE[2]])),
            PALETTE[1]
        );
        // All taken: the user id's.
        assert_eq!(free_colour("ff00", &used(&PALETTE)), profile_colour("ff00"));
    }

    /// Publishes `sync`'s first profile, named `name`; returns its colour.
    fn publish_profile(sync: &mut ServerSync, name: &str) -> String {
        sync.name = Some(name.to_owned());
        sync.ensure_profile();
        sync.next_members_op();
        let (batch, _) = sent_entry(sync);
        ack(sync, batch);
        let user = sync.user.clone();
        sync.profile_of(&user).unwrap().colour
    }

    #[test]
    fn a_first_profile_takes_a_colour_no_current_member_uses() {
        // The member has no profile: it counts with its user id's colour.
        let (mut sync, member) = with_member();
        let theirs = profile_colour(&member.user);
        let expected = if theirs == PALETTE[0] {
            PALETTE[1]
        } else {
            PALETTE[0]
        };
        assert_eq!(publish_profile(&mut sync, "Ann"), expected);

        // Removed members don't count.
        let (mut sync, member) = with_member();
        let id = sync.remove_user(&member.user).unwrap();
        let (batch, _) = sent_entry(&mut sync);
        ack(&mut sync, batch);
        assert_eq!(sync.change_result(id), Some(Ok(())));
        // The key epoch after the removal.
        let (batch, record) = sent_entry(&mut sync);
        assert!(matches!(entry_body(&record).unwrap().op, Op::Epoch { .. }));
        ack(&mut sync, batch);
        assert_eq!(publish_profile(&mut sync, "Ann"), PALETTE[0]);
    }

    #[test]
    fn the_own_colour_changes_to_a_palette_colour_keeping_the_name() {
        let (mut sync, _) = with_member();
        publish_profile(&mut sync, "Ann");
        assert!(matches!(
            sync.set_colour("#123456"),
            Err(Error::InvalidChange(_))
        ));
        assert!(sync.members_ops.is_empty() && sync.take_actions().is_empty());

        let id = sync.set_colour(PALETTE[5]).unwrap();
        let (batch, record) = sent_entry(&mut sync);
        assert!(matches!(
            entry_body(&record).unwrap().op,
            Op::Profile { .. }
        ));
        ack(&mut sync, batch);
        assert_eq!(sync.change_result(id), Some(Ok(())));
        let user = sync.user.clone();
        assert_eq!(
            sync.profile_of(&user),
            Some(Profile {
                name: "Ann".into(),
                colour: PALETTE[5].into(),
            })
        );
        // A later profile of this user (a device joining it) keeps it.
        assert_eq!(sync.new_profile_colour(), PALETTE[5]);
    }

    #[test]
    fn the_owner_deletes_the_project_with_a_signed_delete() {
        let (mut sync, _) = with_member();
        let id = sync.delete_project().unwrap();
        let actions = sync.take_actions();
        let Some(Action::Http(request)) = actions.last() else {
            panic!("{actions:?}");
        };
        assert_eq!(request.method, Method::Delete);
        assert_eq!(request.path, format!("/v1/projects/{}", sync.project));
        let public = sync.keys.public().sign;
        let time =
            lethora_protocol::wire::verify_deletion(&public, &sync.project, &request.body).unwrap();
        assert_eq!(time, 10);
        // The server closes the connections first; the answer still counts.
        sync.handle(Message::Refused {
            code: Code::UnknownProject,
            message: "the project was deleted".into(),
        });
        assert_eq!(sync.status().state, ServerState::Removed);
        sync.http_result(
            request.id,
            Ok(HttpResponse {
                status: 204,
                body: Vec::new(),
                generation: None,
            }),
        );
        assert_eq!(sync.change_result(id), Some(Ok(())));

        // A refusal is reported.
        let (mut sync, _) = with_member();
        let id = sync.delete_project().unwrap();
        let Some(Action::Http(request)) = sync.take_actions().pop() else {
            panic!("a request");
        };
        sync.http_result(
            request.id,
            Ok(HttpResponse {
                status: 401,
                body: b"too far".to_vec(),
                generation: None,
            }),
        );
        assert!(sync.change_result(id).unwrap().unwrap_err().contains("401"));
        assert!(!sync.is_stopped());
    }

    #[test]
    fn members_and_open_invites_are_listed() {
        let (sync, member) = with_member();
        let list = sync.member_list();
        assert_eq!(list.len(), 2);
        let owner = list.iter().find(|m| m.role == Role::Owner).unwrap();
        assert_eq!(owner.devices, std::slice::from_ref(&sync.device));
        let editor = list.iter().find(|m| m.user == member.user).unwrap();
        assert_eq!(editor.role, Role::Editor);
        assert_eq!(editor.colour, profile_colour(&member.user));
        assert_eq!(editor.name, None);
        // The editor invite is used up; the reader one is open.
        let invites = sync.open_invites();
        assert_eq!(invites.len(), 1);
        assert_eq!(invites[0].id, member.invite);
        assert_eq!(invites[0].role, Some(Role::Reader));
        assert_eq!(invites[0].creator_user, sync.user);
        assert_eq!(invites[0].uses_left, 1);
    }

    #[test]
    fn a_role_change_makes_the_document_read_only_and_back() {
        let dir = tempfile::tempdir().unwrap();
        let (owner, member) = with_member();
        let mut sync = member_sync(&owner, &member);
        let identity = Identity {
            device: DeviceId::parse(&member.device).unwrap(),
            user: UserId::parse(&member.user).unwrap(),
        };
        let mut doc = Document::create(dir.path(), &identity).unwrap();
        doc.insert(0, "Text").unwrap();
        doc.checkpoint(Some("named"), std::time::SystemTime::now())
            .unwrap();
        let checkpoint = doc.checkpoints()[0].id;
        // The owner makes the member a reader; the record arrives.
        let mut owner = owner;
        owner.set_role(&member.user, Role::Reader).unwrap();
        let (batch, record) = sent_entry(&mut owner);
        ack(&mut owner, batch);
        let offset = sync.members_bytes.len() as u64;
        sync.handle(Message::MembersRecords {
            offset,
            bytes: record,
        });
        assert_eq!(sync.role(), Some(Role::Reader));
        doc.server_save(&sync).unwrap();
        assert!(!doc.is_writable());
        let read_only = |r: Result<()>| {
            assert!(
                matches!(r, Err(Error::ReadOnly(ref role)) if role == "Reader"),
                "{r:?}"
            )
        };
        read_only(doc.insert(0, "x"));
        read_only(doc.delete(0, 1));
        read_only(doc.apply_changes(&[]));
        read_only(doc.map_replace(crate::MapName::Layout, &crate::Entries::new()));
        read_only(doc.set_custom_text("x"));
        read_only(doc.set_bibliography("x"));
        read_only(doc.put_asset("assets/a.png", b"x"));
        read_only(doc.stage_blob(b"x").map(drop));
        read_only(doc.add_asset_folder("assets/f/"));
        read_only(doc.move_assets(&[]).map(drop));
        read_only(doc.remove_assets(&[]));
        read_only(doc.insert_with_assets(0, "x", &[]).map(drop));
        read_only(doc.repoint_asset(0, 0, "", "assets/b.png", b"x").map(drop));
        read_only(doc.undo().map(drop));
        read_only(doc.redo().map(drop));
        read_only(doc.restore(checkpoint).map(drop));
        read_only(
            doc.checkpoint(Some("n"), std::time::SystemTime::now())
                .map(drop),
        );
        assert!(!doc.merge_bibliography_mirror().unwrap());
        assert!(doc.adopt_loose_assets(|_| true).unwrap().is_empty());
        // Local bookkeeping goes on.
        doc.checkpoint(None, std::time::SystemTime::now()).unwrap();
        doc.autosave().unwrap();
        assert_eq!(doc.text(), "Text");

        // Back to editor.
        owner.set_role(&member.user, Role::Editor).unwrap();
        let (batch, record) = sent_entry(&mut owner);
        ack(&mut owner, batch);
        let offset = sync.members_bytes.len() as u64;
        sync.handle(Message::MembersRecords {
            offset,
            bytes: record,
        });
        doc.server_pull(&mut sync).unwrap();
        assert!(doc.is_writable());
        doc.insert(0, "More ").unwrap();
        // Forgetting the server: local and writable.
        doc.set_role(Some(Role::Reader));
        doc.server_forget().unwrap();
        assert!(doc.is_writable() && doc.server_settings().is_none());
    }

    #[test]
    fn reopening_says_hello_as_the_device_and_reads_every_stream() {
        let (owner, member) = with_member();
        let mut sync = ServerSync::reopen(
            "https://lethora.example",
            &owner.project,
            &DeviceId::parse(&member.device).unwrap(),
            member.keys.clone(),
            &UserId::parse(&member.user).unwrap(),
            Keyring::new([7; 32]),
            Some("Bob"),
        )
        .unwrap();
        assert!(!sync.is_established());
        sync.connected();
        sync.handle(Message::Challenge {
            nonce: [1; NONCE_LEN],
            server_time: 5,
        });
        let Some(Action::Send(message)) = sync.take_actions().pop() else {
            panic!("a hello");
        };
        assert!(matches!(
            message,
            Message::Hello {
                hello: Hello::Device { ref device, .. },
                ..
            } if *device == member.device
        ));
        sync.handle(Message::Welcome {
            token: "t".into(),
            role: Some(Role::Editor),
            streams: Vec::new(),
            app: String::new(),
            devices: Vec::new(),
        });
        let Some(Action::Send(Message::Subscribe { members, streams })) = sync.take_actions().pop()
        else {
            panic!("a subscribe");
        };
        assert_eq!((members, streams.len()), (0, 0));
        sync.handle(Message::MembersRecords {
            offset: 0,
            bytes: owner.members_bytes.clone(),
        });
        sync.handle(Message::Synced);
        assert!(sync.join_ready());
        assert_eq!(sync.user(), member.user);

        // A removed device is told so.
        let mut sync = ServerSync::reopen(
            "https://lethora.example",
            &owner.project,
            &DeviceId::parse(&member.device).unwrap(),
            member.keys.clone(),
            &UserId::parse(&member.user).unwrap(),
            Keyring::new([7; 32]),
            None,
        )
        .unwrap();
        sync.handle(Message::Refused {
            code: Code::NotAMember,
            message: "this device is not a member".into(),
        });
        assert_eq!(sync.status().state, ServerState::Removed);
    }

    /// A presence entry as [`Document::server_set_presence`] makes it.
    fn presence_value(view: &str, anchor: &str, head: &str) -> LoroValue {
        LoroValue::from(HashMap::from([
            ("view", LoroValue::from(view)),
            ("anchor", LoroValue::from(anchor)),
            ("head", LoroValue::from(head)),
        ]))
    }

    /// The presence messages `sync` sent.
    fn sent_presence(sync: &mut ServerSync) -> Vec<Vec<u8>> {
        sync.take_actions()
            .into_iter()
            .filter_map(|a| match a {
                Action::Send(Message::Presence { bytes }) => Some(bytes),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn presence_is_throttled_kept_alive_and_sent_once_caught_up() {
        let (mut sync, _) = with_member();
        assert_eq!(sync.presence_due(), None);
        sync.set_presence(presence_value("author", "a", "a"), 1_000);
        assert_eq!(sent_presence(&mut sync).len(), 1);
        // Unchanged: nothing; changed within the gap: later.
        sync.set_presence(presence_value("author", "a", "a"), 1_020);
        sync.set_presence(presence_value("author", "b", "b"), 1_050);
        assert!(sent_presence(&mut sync).is_empty());
        assert_eq!(sync.presence_due(), Some(1_000 + PRESENCE_GAP_MS));
        sync.presence_tick(1_100);
        assert_eq!(sent_presence(&mut sync).len(), 1);
        sync.presence_tick(1_200);
        assert!(sent_presence(&mut sync).is_empty());
        // The keep-alive.
        assert_eq!(sync.presence_due(), Some(1_100 + PRESENCE_KEEPALIVE_MS));
        sync.presence_tick(1_100 + PRESENCE_KEEPALIVE_MS);
        assert_eq!(sent_presence(&mut sync).len(), 1);
        // Offline: nothing; caught up again: sent at once.
        sync.disconnected();
        assert_eq!(sync.presence_due(), None);
        sync.presence_tick(20_000);
        assert!(sent_presence(&mut sync).is_empty());
        welcome(&mut sync, 0, 0, 0);
        sync.handle(Message::Synced);
        sync.take_actions();
        assert_eq!(sync.presence_due(), Some(PRESENCE_GAP_MS));
        sync.presence_tick(20_000);
        assert_eq!(sent_presence(&mut sync).len(), 1);
    }

    #[test]
    fn presence_is_opened_from_current_members_only() {
        let (mut owner, member) = with_member();
        let mut sync = member_sync(&owner, &member);
        owner.set_presence(presence_value("science", "aa", "bb"), 1_000);
        let bytes = sent_presence(&mut owner).pop().unwrap();
        // Signed by another device than the one named: skipped.
        sync.handle(Message::PeerPresence {
            device: member.device.clone(),
            bytes: bytes.clone(),
        });
        let stranger = random_id().unwrap();
        sync.handle(Message::PeerPresence {
            device: stranger.clone(),
            bytes: bytes.clone(),
        });
        assert!(sync.presence_entries().is_empty());
        // The owner's own: listed with its user and role.
        sync.handle(Message::PeerPresence {
            device: owner.device.clone(),
            bytes: bytes.clone(),
        });
        assert!(sync.take_actions().contains(&Action::Presence));
        let entries = sync.presence_entries();
        assert_eq!(entries.len(), 1);
        let (info, anchor, head) = &entries[0];
        assert_eq!(
            (info.device.as_str(), info.user.as_str(), info.role),
            (owner.device.as_str(), owner.user.as_str(), Role::Owner)
        );
        assert_eq!(
            (info.view.as_str(), anchor.as_str(), head.as_str()),
            ("science", "aa", "bb")
        );
        assert_eq!(info.colour, profile_colour(&owner.user));
        // Entries for other devices inside the owner's presence are ignored.
        let store = EphemeralStore::new(1_000);
        store.set(&member.device, presence_value("author", "x", "x"));
        store.set(&stranger, presence_value("author", "x", "x"));
        let context = Context {
            project: &owner.project,
            document: MAIN,
            kind: PRESENCE,
            name: &owner.device,
        };
        let forged = seal_v2(
            owner.keyring.as_ref().unwrap(),
            &owner.device,
            &owner.keys,
            &context,
            1,
            &store.encode_all(),
        )
        .unwrap();
        sync.handle(Message::PeerPresence {
            device: owner.device.clone(),
            bytes: forged,
        });
        // (Without an entry of its own, the owner has none.)
        assert!(sync.presence.keys().is_empty());
        sync.handle(Message::PeerPresence {
            device: owner.device.clone(),
            bytes: bytes.clone(),
        });
        assert_eq!(sync.presence_entries().len(), 1);
        // A stranger's own presence, sealed with the project key: ignored.
        let stranger_keys = DeviceKeys::generate().unwrap();
        let context = Context {
            name: &stranger,
            ..context
        };
        let store = EphemeralStore::new(1_000);
        store.set(&stranger, presence_value("author", "x", "x"));
        let sealed = seal_v2(
            owner.keyring.as_ref().unwrap(),
            &stranger,
            &stranger_keys,
            &context,
            1,
            &store.encode(&stranger),
        )
        .unwrap();
        sync.handle(Message::PeerPresence {
            device: stranger,
            bytes: sealed,
        });
        assert_eq!(sync.presence_entries().len(), 1);
        // A device that wasn't here before is told of this one soon (the
        // member has an entry of its own).
        let mut sync = member_sync(&owner, &member);
        sync.set_presence(presence_value("author", "m", "m"), 1_000);
        sent_presence(&mut sync);
        sync.handle(Message::PeerPresence {
            device: owner.device.clone(),
            bytes,
        });
        assert_eq!(sync.presence_due(), Some(1_000 + PRESENCE_GAP_MS));
    }

    #[test]
    fn presence_goes_with_timeouts_disconnects_and_removals() {
        let (mut owner, member) = with_member();
        let mut sync = member_sync(&owner, &member);
        sync.set_presence(presence_value("author", "m", "m"), 1_000);
        let bytes = sent_presence(&mut sync).pop().unwrap();
        let from_member = |owner: &mut ServerSync| {
            owner.handle(Message::PeerPresence {
                device: member.device.clone(),
                bytes: bytes.clone(),
            });
            owner.take_actions();
        };

        // Timed out (a short timeout here).
        owner.presence = EphemeralStore::new(50);
        from_member(&mut owner);
        assert_eq!(owner.presence_entries().len(), 1);
        std::thread::sleep(std::time::Duration::from_millis(120));
        owner.presence_tick(10_000);
        assert!(owner.take_actions().contains(&Action::Presence));
        assert!(owner.presence_entries().is_empty());

        // The connection ended.
        owner.presence = EphemeralStore::new(PRESENCE_TIMEOUT_MS as i64);
        from_member(&mut owner);
        owner.disconnected();
        assert!(owner.take_actions().contains(&Action::Presence));
        assert!(owner.presence_entries().is_empty());
        welcome(&mut owner, 0, 0, 0);
        owner.handle(Message::Synced);
        owner.take_actions();

        // Removed: not listed, and nothing new is taken.
        from_member(&mut owner);
        assert_eq!(owner.presence_entries().len(), 1);
        owner.remove_user(&member.user).unwrap();
        let (batch, _) = sent_entry(&mut owner);
        ack(&mut owner, batch);
        assert!(owner.take_actions().contains(&Action::Presence));
        assert!(owner.presence_entries().is_empty());
        owner.presence = EphemeralStore::new(PRESENCE_TIMEOUT_MS as i64);
        from_member(&mut owner);
        assert!(owner.presence.keys().is_empty());
    }

    #[test]
    fn presence_selections_follow_concurrent_edits() {
        let (mut owner, member) = with_member();
        let mut sync = member_sync(&owner, &member);
        let identity = |device: &str, user: &str| Identity {
            device: DeviceId::parse(device).unwrap(),
            user: UserId::parse(user).unwrap(),
        };
        let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let mut doc_a =
            Document::create(dir_a.path(), &identity(&owner.device, &owner.user)).unwrap();
        let mut doc_b =
            Document::create(dir_b.path(), &identity(&member.device, &member.user)).unwrap();
        doc_a.insert(0, "Hello w\u{f6}rld \u{1f600} end").unwrap();
        doc_b
            .doc
            .import(&doc_a.doc.export(ExportMode::all_updates()).unwrap())
            .unwrap();
        // "w\u{f6}rld \u{1f600}" selected (UTF-16 6..14), then text A
        // hasn't sent yet.
        doc_a
            .server_set_presence(&mut owner, "science", 6, 14, 1_000)
            .unwrap();
        let first = sent_presence(&mut owner).pop().unwrap();
        doc_a.insert(0, "New ").unwrap();
        doc_a
            .server_set_presence(&mut owner, "author", 2, 2, 2_000)
            .unwrap();
        let second = sent_presence(&mut owner).pop().unwrap();
        assert!(matches!(
            doc_a.server_set_presence(&mut owner, "elsewhere", 0, 0, 3_000),
            Err(Error::InvalidChange(_))
        ));

        // B types before the selection meanwhile: it moves along.
        doc_b.insert(0, "Oh, ").unwrap();
        sync.handle(Message::PeerPresence {
            device: owner.device.clone(),
            bytes: first,
        });
        let list = doc_b.server_presence(&sync);
        assert_eq!(list.len(), 1);
        assert_eq!(
            (list[0].view.as_str(), list[0].anchor, list[0].head),
            ("science", Some(10), Some(18))
        );
        // In text B hasn't got yet: no position, until it arrives.
        sync.handle(Message::PeerPresence {
            device: owner.device.clone(),
            bytes: second,
        });
        assert_eq!(doc_b.server_presence(&sync)[0].head, None);
        doc_b
            .doc
            .import(&doc_a.doc.export(ExportMode::all_updates()).unwrap())
            .unwrap();
        let list = doc_b.server_presence(&sync);
        assert_eq!(doc_b.text().find("New").map(|at| at + 2), list[0].head);
        assert_eq!(list[0].view, "author");
    }
}
