//! One project on the server: its files, its members, its connections.
//!
//! # Files
//!
//! Paths are relative to the data directory, under `projects/<project id>/`:
//!
//! - `members.log`: the members log (records exactly as appended).
//! - `<document>/<device>.log`: the device's stream of `main` or `review`,
//!   framed records exactly as appended (each an envelope v2 or v4, kind
//!   `log`).
//! - `<document>/<device>.snapshot`: the stream's latest compaction
//!   snapshot, one envelope v2 or v4 (kind `snapshot`).
//! - `assets/<blob>`: an envelope v2 or v4 (kind `asset`) per blob, named
//!   by the blob (64 lowercase hex); never changed once written.
//! - `invites/<invite id>.key`: the project keys wrapped for an open invite
//!   (see `lethora_protocol::invite`); deleted once the invite is used up,
//!   revoked or found expired.
//! - `meta.json`: `{"bytes": <bytes used>, "streams": {"<document>/<device>":
//!   {"generation", "covers", "length", "batch"}}, "devices": {"<device>":
//!   {"level", "app"}}}`: per stream the generation (0 until the first
//!   compaction), how many bytes of the previous generation's log the
//!   snapshot covers, the log's length and the last batch id stored; per
//!   device the feature level and app version of its last hello (for the
//!   welcome, see `lethora_protocol::wire`). Rewritten (atomically) after
//!   every change.
//!
//! # Loading
//!
//! A project is read when first used: the members log (an invalid tail is
//! logged loudly, kept as `members.log.damaged` and cut off), then every
//! stream's log, cutting off a torn tail. A log shorter than `meta.json`
//! says means a compaction finished without its `meta.json` update; the
//! stream then moves to the next generation. Bytes used are counted from
//! the files.
//!
//! # Writes and connections
//!
//! A mutex serializes everything a project does, so an append, its
//! broadcast and the catch-up of a new connection never interleave. For
//! each subscribed connection the project keeps what it has sent: the
//! members log length and per stream (generation, offset). Live records go
//! to a connection only when it is exactly at their start and its queue has
//! room; otherwise it catches up again from where it is, one message at a
//! time, as its queue drains.
//!
//! # Levels
//!
//! Each connection keeps its hello's feature level. A hello below the
//! project's minimum (the members log's `require`) is refused with
//! `Version`; when a `require` entry is stored, every connection below the
//! new minimum is closed the same way before anything else is sent. Records,
//! snapshots and assets above the minimum are refused (the writer appends
//! the `require` first); a members entry of an op this server doesn't know
//! (a newer app's) is refused, never stored.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use lethora_protocol::check_id;
use lethora_protocol::envelope::{Context, header_v2, verify_v2};
use lethora_protocol::frame::{Frame, parse_frame};
use lethora_protocol::members::{LogEnd, Members, Op, Role, entry_body};
use lethora_protocol::wire::{ClientVersion, Code, DeviceVersion, Message, OwnStream, Position};
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, mpsc, oneshot};
use tracing::{error, warn};

use crate::config::Limits;
use crate::store::Store;

/// The documents a stream may belong to.
pub const DOCUMENTS: [&str; 2] = ["main", "review"];
/// Largest wrapped invite keyring accepted (some 450 epochs).
const MAX_INVITE_KEY: usize = 16 * 1024;

/// Why something was refused: a wire code and a message for the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: Code,
    pub message: String,
}

impl Refusal {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// An internal error: logged, and only "internal error" for the client.
    pub fn internal(what: impl fmt::Display) -> Self {
        error!("internal error: {what}");
        Self::new(Code::Internal, "internal error")
    }
}

/// Who a connection or token belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Who {
    /// A device of the project.
    Device(String),
    /// Someone holding invite link `0`, before joining.
    Invitee(String),
}

/// A stream's state (see the module docs).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stream {
    pub generation: u64,
    pub covers: u64,
    pub length: u64,
    pub batch: u64,
}

/// A device's last hello (see the module docs).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Seen {
    level: u32,
    app: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Meta {
    #[serde(default)]
    bytes: u64,
    #[serde(default)]
    streams: BTreeMap<String, Stream>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    devices: BTreeMap<String, Seen>,
}

/// A stream's key: (document, device).
type Key = (String, String);

/// What a subscribed connection has (see the module docs).
struct Subscription {
    members: u64,
    /// Per stream (generation, offset); missing: nothing.
    cursors: HashMap<Key, (u64, u64)>,
    /// Whether `Synced` was sent.
    synced: bool,
}

/// A connection of the project.
pub struct Peer {
    pub who: Who,
    /// The feature level of its hello.
    pub level: u32,
    /// The connection's queue of messages to send.
    pub tx: mpsc::Sender<Message>,
    /// Woken when the connection has to catch up again.
    pub wake: Arc<Notify>,
    /// Closes the connection with a refusal.
    pub kick: Option<oneshot::Sender<Refusal>>,
    sub: Option<Subscription>,
}

impl Peer {
    pub fn new(
        who: Who,
        level: u32,
        tx: mpsc::Sender<Message>,
        wake: Arc<Notify>,
        kick: oneshot::Sender<Refusal>,
    ) -> Self {
        Self {
            who,
            level,
            tx,
            wake,
            kick: Some(kick),
            sub: None,
        }
    }

    fn device(&self) -> Option<&str> {
        match &self.who {
            Who::Device(device) => Some(device),
            Who::Invitee(_) => None,
        }
    }

    fn close(mut self, refusal: Refusal) {
        if let Some(kick) = self.kick.take() {
            let _ = kick.send(refusal);
        }
    }
}

struct State {
    members: Members,
    members_len: u64,
    streams: BTreeMap<Key, Stream>,
    devices: BTreeMap<String, Seen>,
    bytes: u64,
    peers: HashMap<u64, Peer>,
    deleted: bool,
    last_used: Instant,
}

/// A loaded project (see the module docs).
pub struct Project {
    pub id: String,
    dir: String,
    store: Arc<dyn Store>,
    limits: Limits,
    state: Mutex<State>,
}

/// The length of the valid records at the start of `log`.
fn valid_prefix(log: &[u8]) -> usize {
    let mut at = 0;
    while let Frame::Record(_, next) = parse_frame(log, at) {
        at = next;
    }
    at
}

/// Whether `name` is a blob name: 64 lowercase hex.
pub fn is_blob_name(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Checks that `document` is one a stream may belong to.
fn check_document(document: &str) -> Result<(), Refusal> {
    if DOCUMENTS.contains(&document) {
        Ok(())
    } else {
        Err(Refusal::new(
            Code::BadRequest,
            format!("unknown document {document:?}"),
        ))
    }
}

/// Whether invite `id` is open at unix time `now`: it exists, isn't
/// revoked or used up, hasn't expired and its creator is still a device.
pub fn invite_open(members: &Members, id: &str, now: u64) -> bool {
    members.invites().get(id).is_some_and(|invite| {
        !invite.revoked
            && invite.uses_left > 0
            && now <= invite.expires
            && members.role_of_device(&invite.creator).is_some()
    })
}

fn io(e: std::io::Error) -> Refusal {
    Refusal::internal(format!("storage: {e}"))
}

/// The refusal of a client below the project's minimum level, whose first
/// app version is `app`.
fn needs_update(app: &str) -> Refusal {
    Refusal::new(
        Code::Version,
        format!("This project needs Lethora {app} or later."),
    )
}

/// Checks that an envelope needing `level` fits the project's minimum.
fn check_level(members: &Members, level: u32) -> Result<(), Refusal> {
    if level > members.required_level() {
        return Err(Refusal::new(
            Code::BadRequest,
            format!(
                "this needs level {level}, the project requires {}: append a require entry first",
                members.required_level()
            ),
        ));
    }
    Ok(())
}

impl Project {
    fn path(&self, rest: &str) -> String {
        format!("{}/{rest}", self.dir)
    }

    fn log_path(&self, key: &Key) -> String {
        self.path(&format!("{}/{}.log", key.0, key.1))
    }

    fn snapshot_path(&self, document: &str, device: &str) -> String {
        self.path(&format!("{document}/{device}.snapshot"))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.last_used = Instant::now();
        state
    }

    /// The project's directory relative to the data directory.
    pub fn dir_of(id: &str) -> String {
        format!("projects/{id}")
    }

    /// A new project `id` with genesis record `genesis` (already checked).
    pub fn create(
        store: Arc<dyn Store>,
        limits: Limits,
        id: &str,
        genesis: &[u8],
        members: Members,
    ) -> Result<Self, Refusal> {
        let dir = Self::dir_of(id);
        store
            .write_atomic(&format!("{dir}/members.log"), genesis)
            .map_err(io)?;
        let project = Self {
            id: id.to_owned(),
            dir,
            store,
            limits,
            state: Mutex::new(State {
                members,
                members_len: genesis.len() as u64,
                streams: BTreeMap::new(),
                devices: BTreeMap::new(),
                bytes: genesis.len() as u64,
                peers: HashMap::new(),
                deleted: false,
                last_used: Instant::now(),
            }),
        };
        project.save_meta(&project.lock());
        Ok(project)
    }

    /// Project `id` from the store; `None` if it has no members log (see
    /// the module docs).
    pub fn load(store: Arc<dyn Store>, limits: Limits, id: &str) -> Result<Option<Self>, Refusal> {
        let dir = Self::dir_of(id);
        let members_path = format!("{dir}/members.log");
        let Some(log) = store.read(&members_path).map_err(io)? else {
            return Ok(None);
        };
        let read = Members::from_log(&log);
        let Some(members) = read.members else {
            error!(project = id, "the members log has no valid genesis record");
            return Err(Refusal::new(Code::Internal, "the project is damaged"));
        };
        if read.end != LogEnd::Complete {
            error!(
                project = id,
                records = read.records,
                end = ?read.end,
                "THE MEMBERS LOG HAS AN INVALID TAIL; kept as members.log.damaged, \
                 using the valid records"
            );
            store
                .write_atomic(&format!("{dir}/members.log.damaged"), &log)
                .map_err(io)?;
            store
                .write_atomic(&members_path, &log[..read.bytes])
                .map_err(io)?;
        }
        let meta: Meta = match store.read(&format!("{dir}/meta.json")).map_err(io)? {
            Some(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                warn!(project = id, "meta.json is invalid ({e}); starting over");
                Meta::default()
            }),
            None => Meta::default(),
        };
        let mut streams = BTreeMap::new();
        let mut bytes = read.bytes as u64;
        for document in DOCUMENTS {
            for name in store.list(&format!("{dir}/{document}")).map_err(io)? {
                let path = format!("{dir}/{document}/{name}");
                let Some((device, ext)) = name.rsplit_once('.') else {
                    continue;
                };
                if check_id(device, "device").is_err() || !matches!(ext, "log" | "snapshot") {
                    continue;
                }
                let key = (document.to_owned(), device.to_owned());
                let mut stream = meta
                    .streams
                    .get(&format!("{document}/{device}"))
                    .copied()
                    .unwrap_or_default();
                if ext == "snapshot" {
                    bytes += store.size(&path).map_err(io)?.unwrap_or(0);
                    streams.entry(key).or_insert(stream);
                    continue;
                }
                let log = store.read(&path).map_err(io)?.unwrap_or_default();
                let valid = valid_prefix(&log);
                if valid < log.len() {
                    warn!(
                        project = id,
                        document,
                        device,
                        "cutting a torn or damaged tail of {} bytes",
                        log.len() - valid
                    );
                    store.write_atomic(&path, &log[..valid]).map_err(io)?;
                }
                let valid = valid as u64;
                if valid < stream.length {
                    warn!(
                        project = id,
                        document, device, "finishing an interrupted compaction"
                    );
                    stream.covers = stream.length - valid;
                    stream.generation += 1;
                }
                stream.length = valid;
                bytes += valid;
                streams.insert(key, stream);
            }
        }
        for dir in ["assets", "invites"] {
            for name in store
                .list(&format!("{}/{dir}", Self::dir_of(id)))
                .map_err(io)?
            {
                let path = format!("{}/{dir}/{name}", Self::dir_of(id));
                bytes += store.size(&path).map_err(io)?.unwrap_or(0);
            }
        }
        let project = Self {
            id: id.to_owned(),
            dir,
            store,
            limits,
            state: Mutex::new(State {
                members,
                members_len: read.bytes as u64,
                streams,
                devices: meta.devices,
                bytes,
                peers: HashMap::new(),
                deleted: false,
                last_used: Instant::now(),
            }),
        };
        {
            let mut state = project.lock();
            project.sweep_invites_locked(&mut state, crate::now());
            project.save_meta(&state);
        }
        Ok(Some(project))
    }

    /// Writes `meta.json`; a failure is logged (at worst a resent batch is
    /// stored twice, which readers ignore).
    fn save_meta(&self, state: &State) {
        let meta = Meta {
            bytes: state.bytes,
            streams: state
                .streams
                .iter()
                .map(|((document, device), stream)| (format!("{document}/{device}"), *stream))
                .collect(),
            devices: state.devices.clone(),
        };
        let json = serde_json::to_vec_pretty(&meta).expect("plain data");
        if let Err(e) = self.store.write_atomic(&self.path("meta.json"), &json) {
            error!(project = self.id, "can't write meta.json: {e}");
        }
    }

    /// Appends `bytes` to the file at `path`, which is `length` bytes long;
    /// after a failure, cuts off whatever part was written.
    fn append_file(&self, path: &str, length: u64, bytes: &[u8]) -> Result<(), Refusal> {
        let Err(e) = self.store.append(path, bytes) else {
            return Ok(());
        };
        if let Ok(Some(file)) = self.store.read(path)
            && file.len() as u64 > length
        {
            let _ = self.store.write_atomic(path, &file[..length as usize]);
        }
        Err(io(e))
    }

    /// Whole records of the file at `path` (`length` bytes long) from
    /// `offset`: at most `chunk_bytes`, or one larger record. `None` if
    /// `offset` isn't the start of a record.
    fn read_chunk(&self, path: &str, offset: u64, length: u64) -> Result<Option<Vec<u8>>, Refusal> {
        let max = self.limits.chunk_bytes.min(length - offset);
        let bytes = self
            .store
            .read_range(path, offset, max)
            .map_err(io)?
            .unwrap_or_default();
        let end = valid_prefix(&bytes);
        if end > 0 {
            return Ok(Some(bytes[..end].to_vec()));
        }
        // The first record is larger than a chunk: read it alone.
        let Some(header) = bytes.get(..4) else {
            return Ok(None);
        };
        let len = 8 + u64::from(u32::from_le_bytes(header.try_into().expect("4 bytes")));
        if offset + len > length {
            return Ok(None);
        }
        let bytes = self
            .store
            .read_range(path, offset, len)
            .map_err(io)?
            .unwrap_or_default();
        Ok((valid_prefix(&bytes) as u64 == len).then_some(bytes))
    }

    /// Runs `f` with the members state.
    pub fn with_members<T>(&self, f: impl FnOnce(&Members) -> T) -> T {
        f(&self.lock().members)
    }

    /// Whether the project was deleted (a stale handle).
    pub fn is_deleted(&self) -> bool {
        self.lock().deleted
    }

    /// Whether nobody is connected and nothing happened for `seconds`.
    pub fn is_idle(&self, seconds: u64) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.peers.is_empty() && state.last_used.elapsed().as_secs() >= seconds
    }

    /// The role of current device `device`, or why it has none.
    fn role(state: &State, device: &str) -> Result<Role, Refusal> {
        if state.deleted {
            return Err(Refusal::new(
                Code::UnknownProject,
                "the project was deleted",
            ));
        }
        state
            .members
            .role_of_device(device)
            .ok_or_else(|| Refusal::new(Code::NotAMember, "this device is not a member"))
    }

    /// Checks that `device` is a current member; returns its role.
    pub fn member(&self, device: &str) -> Result<Role, Refusal> {
        Self::role(&self.lock(), device)
    }

    /// Remembers `device`'s last hello: `client`.
    pub fn saw(&self, device: &str, client: &ClientVersion) {
        let mut state = self.lock();
        let seen = Seen {
            level: client.level,
            app: client.app.clone(),
        };
        if state.devices.get(device) != Some(&seen) {
            state.devices.insert(device.to_owned(), seen);
            self.save_meta(&state);
        }
    }

    /// The last hello of each current device, for a welcome.
    pub fn device_versions(&self) -> Vec<DeviceVersion> {
        let state = self.lock();
        state
            .devices
            .iter()
            .filter(|(device, _)| state.members.role_of_device(device).is_some())
            .map(|(device, seen)| DeviceVersion {
                device: device.clone(),
                version: ClientVersion {
                    level: seen.level,
                    app: seen.app.clone(),
                },
            })
            .collect()
    }

    /// The streams of `device`, for its welcome.
    pub fn own_streams(&self, device: &str) -> Vec<OwnStream> {
        self.lock()
            .streams
            .iter()
            .filter(|((_, d), _)| d == device)
            .map(|((document, _), s)| OwnStream {
                document: document.clone(),
                generation: s.generation,
                length: s.length,
                batch: s.batch,
            })
            .collect()
    }

    /// Adds connection `id`; refused below the project's minimum level (see
    /// the module docs).
    pub fn register(&self, id: u64, peer: Peer) -> Result<(), Refusal> {
        let mut state = self.lock();
        if state.deleted {
            return Err(Refusal::new(
                Code::UnknownProject,
                "the project was deleted",
            ));
        }
        if peer.level < state.members.required_level() {
            return Err(needs_update(state.members.required_app()));
        }
        state.peers.insert(id, peer);
        Ok(())
    }

    /// Removes connection `id`.
    pub fn unregister(&self, id: u64) {
        self.lock().peers.remove(&id);
    }

    /// Starts connection `id`'s catch-up from what it has: `members` bytes
    /// of the members log and `positions` of streams.
    pub fn subscribe(&self, id: u64, members: u64, positions: &[Position]) -> Result<(), Refusal> {
        let mut state = self.lock();
        let Some(peer) = state.peers.get_mut(&id) else {
            return Err(Refusal::new(
                Code::UnknownProject,
                "the project was deleted",
            ));
        };
        if peer.sub.is_some() {
            return Err(Refusal::new(Code::BadRequest, "subscribed already"));
        }
        if peer.device().is_none() && !positions.is_empty() {
            return Err(Refusal::new(
                Code::Forbidden,
                "an invitee reads only the members log",
            ));
        }
        let mut cursors = HashMap::new();
        for p in positions {
            check_document(&p.document)?;
            cursors.insert(
                (p.document.clone(), p.device.clone()),
                (p.generation, p.offset),
            );
        }
        peer.sub = Some(Subscription {
            members,
            cursors,
            synced: false,
        });
        Ok(())
    }

    /// Sends connection `id` its next catch-up message through `permit`;
    /// `false` once nothing is left (after `Synced`, the first time).
    pub fn pull(&self, id: u64, permit: mpsc::OwnedPermit<Message>) -> bool {
        let mut guard = self.lock();
        let state = &mut *guard;
        let Some(peer) = state.peers.get_mut(&id) else {
            return false;
        };
        let is_device = peer.device().is_some();
        let Some(sub) = peer.sub.as_mut() else {
            return false;
        };
        match self.next_catch_up(sub, is_device, state.members_len, &state.streams) {
            Ok(Some(message)) => {
                permit.send(message);
                true
            }
            Ok(None) => {
                if !sub.synced {
                    sub.synced = true;
                    permit.send(Message::Synced);
                }
                false
            }
            Err(refusal) => {
                if let Some(peer) = state.peers.remove(&id) {
                    peer.close(refusal);
                }
                false
            }
        }
    }

    /// The next catch-up message for `sub`, updating it; `None` when it has
    /// everything.
    fn next_catch_up(
        &self,
        sub: &mut Subscription,
        is_device: bool,
        members_len: u64,
        streams: &BTreeMap<Key, Stream>,
    ) -> Result<Option<Message>, Refusal> {
        if sub.members != members_len {
            let path = self.path("members.log");
            let mut chunk = None;
            if sub.members < members_len {
                chunk = self.read_chunk(&path, sub.members, members_len)?;
            }
            // Past the end or not at a record: from the start.
            if chunk.is_none() {
                sub.members = 0;
                chunk = self.read_chunk(&path, 0, members_len)?;
            }
            let bytes = chunk.ok_or_else(|| Refusal::internal("the members log can't be read"))?;
            let offset = sub.members;
            sub.members += bytes.len() as u64;
            return Ok(Some(Message::MembersRecords { offset, bytes }));
        }
        if !is_device {
            return Ok(None);
        }
        for (key, stream) in streams {
            let current = (stream.generation, stream.length);
            let cursor = sub.cursors.get(key).copied();
            if cursor == Some(current) {
                continue;
            }
            let (document, device) = (key.0.clone(), key.1.clone());
            let read_from = match cursor {
                Some((g, o)) if g == stream.generation && o < stream.length => Some(o),
                Some((g, o))
                    if g + 1 == stream.generation
                        && o >= stream.covers
                        && o - stream.covers <= stream.length =>
                {
                    sub.cursors
                        .insert(key.clone(), (stream.generation, o - stream.covers));
                    return Ok(Some(Message::Compacted {
                        document,
                        device,
                        generation: stream.generation,
                        covers: stream.covers,
                    }));
                }
                _ => None,
            };
            let chunk = match read_from {
                Some(offset) => self
                    .read_chunk(&self.log_path(key), offset, stream.length)?
                    .map(|bytes| (offset, bytes)),
                None => None,
            };
            let (offset, bytes) = match chunk {
                Some(chunk) => chunk,
                // Nothing usable: start the generation over.
                None if stream.generation > 0 => {
                    sub.cursors.insert(key.clone(), (stream.generation, 0));
                    return Ok(Some(Message::SnapshotChanged {
                        document,
                        device,
                        generation: stream.generation,
                        covers: stream.covers,
                    }));
                }
                None if stream.length == 0 => {
                    sub.cursors.insert(key.clone(), current);
                    continue;
                }
                None => match self.read_chunk(&self.log_path(key), 0, stream.length)? {
                    Some(bytes) => (0, bytes),
                    None => {
                        error!(project = self.id, document, device, "a log can't be read");
                        sub.cursors.insert(key.clone(), current);
                        continue;
                    }
                },
            };
            sub.cursors.insert(
                key.clone(),
                (stream.generation, offset + bytes.len() as u64),
            );
            return Ok(Some(Message::Records {
                document,
                device,
                generation: stream.generation,
                offset,
                bytes,
            }));
        }
        Ok(None)
    }

    /// Appends framed `records` of batch `batch` to the stream of `device`
    /// and `document`, from connection `from` (see the module docs).
    /// Returns the `Ack` or `Nack`.
    pub fn append(
        &self,
        from: u64,
        device: &str,
        document: &str,
        batch: u64,
        records: &[u8],
    ) -> Message {
        match self.try_append(from, device, document, batch, records) {
            Ok(ack) => ack,
            Err(refusal) => Message::Nack {
                batch,
                code: refusal.code,
                message: refusal.message,
            },
        }
    }

    fn try_append(
        &self,
        from: u64,
        device: &str,
        document: &str,
        batch: u64,
        records: &[u8],
    ) -> Result<Message, Refusal> {
        check_document(document)?;
        let mut guard = self.lock();
        let state = &mut *guard;
        Self::role(state, device)?;
        if !state.members.may_write(device, document) {
            return Err(Refusal::new(
                Code::Forbidden,
                format!("this device's role doesn't write {document}"),
            ));
        }
        let public = state.members.device(device).expect("a member").public;
        let context = Context {
            project: &self.id,
            document,
            kind: "log",
            name: device,
        };
        let mut at = 0;
        while at < records.len() {
            let Frame::Record(envelope, next) = parse_frame(records, at) else {
                return Err(Refusal::new(Code::BadRequest, "a record is damaged"));
            };
            let header =
                header_v2(envelope).map_err(|e| Refusal::new(Code::BadRequest, e.to_string()))?;
            if header.signer != device {
                return Err(Refusal::new(Code::Auth, "a record is by another device"));
            }
            verify_v2(envelope, &public, &context).map_err(|_| {
                Refusal::new(Code::Auth, "a record's signature doesn't match this stream")
            })?;
            check_level(&state.members, header.level)?;
            at = next;
        }
        if records.is_empty() {
            return Err(Refusal::new(Code::BadRequest, "no records"));
        }
        let key = (document.to_owned(), device.to_owned());
        let stream = state.streams.get(&key).copied().unwrap_or_default();
        let ack = |s: Stream| Message::Ack {
            batch,
            document: document.to_owned(),
            generation: s.generation,
            length: s.length,
        };
        if batch <= stream.batch {
            return Ok(ack(stream));
        }
        let len = records.len() as u64;
        if state.bytes + len > self.limits.project_bytes {
            return Err(Refusal::new(Code::Quota, "the project's storage is full"));
        }
        self.append_file(&self.log_path(&key), stream.length, records)?;
        let new = Stream {
            length: stream.length + len,
            batch,
            ..stream
        };
        state.streams.insert(key.clone(), new);
        state.bytes += len;
        self.save_meta(state);
        let message = Message::Records {
            document: document.to_owned(),
            device: device.to_owned(),
            generation: stream.generation,
            offset: stream.length,
            bytes: records.to_vec(),
        };
        deliver(
            &mut state.peers,
            from,
            |sub| {
                let cursor = sub.cursors.get(&key).copied();
                let at_start = cursor == Some((stream.generation, stream.length))
                    || (cursor.is_none() && stream == Stream::default());
                if !at_start {
                    return None;
                }
                sub.cursors
                    .insert(key.clone(), (new.generation, new.length));
                Some(())
            },
            &message,
            true,
        );
        Ok(ack(new))
    }

    /// Appends members record `record` of batch `batch` from connection
    /// `from` (`who`), at server time `now`. Returns the `MembersAck` or
    /// `Nack`.
    pub fn append_members(
        &self,
        from: u64,
        who: &Who,
        batch: u64,
        record: &[u8],
        now: u64,
    ) -> Message {
        match self.try_append_members(from, who, record, now) {
            Ok(seq) => Message::MembersAck { batch, seq },
            Err(refusal) => Message::Nack {
                batch,
                code: refusal.code,
                message: refusal.message,
            },
        }
    }

    fn try_append_members(
        &self,
        from: u64,
        who: &Who,
        record: &[u8],
        now: u64,
    ) -> Result<u64, Refusal> {
        let newer = || {
            Refusal::new(
                Code::Version,
                "This server needs an update for this feature (ask its admin).",
            )
        };
        let body = entry_body(record).map_err(|e| match e {
            lethora_protocol::Error::Newer(_) => newer(),
            e => Refusal::new(Code::BadRequest, e.to_string()),
        })?;
        let mut guard = self.lock();
        let state = &mut *guard;
        match who {
            Who::Device(device) => {
                Self::role(state, device)?;
            }
            Who::Invitee(invite) => match &body.op {
                Op::Join { invite: id, .. } if id == invite => {}
                _ => {
                    return Err(Refusal::new(
                        Code::Forbidden,
                        "an invitee only appends its join",
                    ));
                }
            },
        }
        let head = state.members.head();
        if body.seq != head.seq + 1 || body.prev != head.hash {
            return Err(Refusal::new(
                Code::Stale,
                format!("the members log is at entry {}", head.seq),
            ));
        }
        if let Op::Join { invite, .. } = &body.op
            && !invite_open(&state.members, invite, now)
        {
            self.delete_key_unless_open(state, invite, now);
            return Err(Refusal::new(Code::InviteClosed, "the invite is closed"));
        }
        let mut members = state.members.clone();
        members.apply(record).map_err(|e| {
            let code = match e {
                lethora_protocol::Error::Newer(_) => return newer(),
                lethora_protocol::Error::Signature(_) => Code::Auth,
                lethora_protocol::Error::Rejected(_) => Code::Forbidden,
                _ => Code::BadRequest,
            };
            Refusal::new(code, e.to_string())
        })?;
        let len = record.len() as u64;
        if state.bytes + len > self.limits.project_bytes {
            return Err(Refusal::new(Code::Quota, "the project's storage is full"));
        }
        let old_len = state.members_len;
        self.append_file(&self.path("members.log"), old_len, record)?;
        state.members = members;
        state.members_len += len;
        state.bytes += len;
        match &body.op {
            Op::Join { invite, .. } | Op::Revoke { id: invite } => {
                self.delete_key_unless_open(state, invite, now);
            }
            Op::Remove { .. } => {
                let removed: Vec<u64> = state
                    .peers
                    .iter()
                    .filter(|(_, p)| {
                        p.device()
                            .is_some_and(|d| state.members.role_of_device(d).is_none())
                    })
                    .map(|(id, _)| *id)
                    .collect();
                for id in removed {
                    let peer = state.peers.remove(&id).expect("listed");
                    peer.close(Refusal::new(
                        Code::NotAMember,
                        "this device was removed from the project",
                    ));
                }
                // Invites of removed devices are closed now.
                self.sweep_invites_locked(state, now);
            }
            Op::Require { level, app } => {
                // Before anything needing the level can reach them.
                let below: Vec<u64> = state
                    .peers
                    .iter()
                    .filter(|(_, p)| p.level < *level)
                    .map(|(id, _)| *id)
                    .collect();
                for id in below {
                    let peer = state.peers.remove(&id).expect("listed");
                    peer.close(needs_update(app));
                }
            }
            _ => {}
        }
        self.save_meta(state);
        let message = Message::MembersRecords {
            offset: old_len,
            bytes: record.to_vec(),
        };
        deliver(
            &mut state.peers,
            from,
            |sub| {
                (sub.members == old_len).then(|| {
                    sub.members = old_len + len;
                })
            },
            &message,
            false,
        );
        Ok(body.seq)
    }

    /// Forwards presence `bytes` of `device` (connection `from`) to the
    /// project's other subscribed devices; dropped where a queue is full.
    pub fn presence(&self, from: u64, device: &str, bytes: Vec<u8>) {
        let state = self.lock();
        let message = Message::PeerPresence {
            device: device.to_owned(),
            bytes,
        };
        for (id, peer) in &state.peers {
            if *id != from && peer.sub.is_some() && peer.device().is_some() {
                let _ = peer.tx.try_send(message.clone());
            }
        }
    }

    /// Compaction by `device` of its stream of `document`: `snapshot` covers
    /// the log's first `covers` bytes (see the module docs). Returns the new
    /// generation and log length.
    pub fn compact(
        &self,
        device: &str,
        document: &str,
        covers: u64,
        snapshot: &[u8],
    ) -> Result<(u64, u64), Refusal> {
        check_document(document)?;
        let mut guard = self.lock();
        let state = &mut *guard;
        Self::role(state, device)?;
        if !state.members.may_write(device, document) {
            return Err(Refusal::new(
                Code::Forbidden,
                format!("this device's role doesn't write {document}"),
            ));
        }
        let public = state.members.device(device).expect("a member").public;
        let context = Context {
            project: &self.id,
            document,
            kind: "snapshot",
            name: device,
        };
        let header =
            header_v2(snapshot).map_err(|e| Refusal::new(Code::BadRequest, e.to_string()))?;
        if header.signer != device {
            return Err(Refusal::new(
                Code::Auth,
                "the snapshot is by another device",
            ));
        }
        verify_v2(snapshot, &public, &context)
            .map_err(|_| Refusal::new(Code::Auth, "the snapshot's signature doesn't match"))?;
        check_level(&state.members, header.level)?;
        let key = (document.to_owned(), device.to_owned());
        let stream = state.streams.get(&key).copied().unwrap_or_default();
        if covers > stream.length {
            return Err(Refusal::new(
                Code::BadRequest,
                "covers is past the log's end",
            ));
        }
        let log_path = self.log_path(&key);
        let log = self.store.read(&log_path).map_err(io)?.unwrap_or_default();
        if log.len() as u64 != stream.length {
            return Err(Refusal::internal(format!(
                "log {}/{} has {} bytes, expected {}",
                key.0,
                key.1,
                log.len(),
                stream.length
            )));
        }
        let covers_at = covers as usize;
        if valid_prefix(&log[..covers_at]) != covers_at {
            return Err(Refusal::new(
                Code::BadRequest,
                "covers is not a record boundary",
            ));
        }
        let snapshot_path = self.snapshot_path(document, device);
        let old_snapshot = self.store.size(&snapshot_path).map_err(io)?.unwrap_or(0);
        let bytes = state.bytes - old_snapshot - covers + snapshot.len() as u64;
        if bytes > self.limits.project_bytes && bytes > state.bytes {
            return Err(Refusal::new(Code::Quota, "the project's storage is full"));
        }
        // Snapshot first, then the log, then meta.json (see the module docs).
        self.store
            .write_atomic(&snapshot_path, snapshot)
            .map_err(io)?;
        self.store
            .write_atomic(&log_path, &log[covers_at..])
            .map_err(io)?;
        let new = Stream {
            generation: stream.generation + 1,
            covers,
            length: stream.length - covers,
            batch: stream.batch,
        };
        state.streams.insert(key.clone(), new);
        state.bytes = bytes;
        self.save_meta(state);
        for peer in state.peers.values_mut() {
            let Some(sub) = peer.sub.as_mut() else {
                continue;
            };
            let Some((generation, offset)) = sub.cursors.get(&key).copied() else {
                peer.wake.notify_one();
                continue;
            };
            if generation != stream.generation {
                peer.wake.notify_one();
                continue;
            }
            let (message, cursor) = if offset >= covers {
                let message = Message::Compacted {
                    document: document.to_owned(),
                    device: device.to_owned(),
                    generation: new.generation,
                    covers,
                };
                (message, (new.generation, offset - covers))
            } else {
                let message = Message::SnapshotChanged {
                    document: document.to_owned(),
                    device: device.to_owned(),
                    generation: new.generation,
                    covers,
                };
                (message, (new.generation, 0))
            };
            if peer.tx.try_send(message).is_ok() {
                sub.cursors.insert(key.clone(), cursor);
            }
            // Behind or not: catch-up sends what follows.
            peer.wake.notify_one();
        }
        Ok((new.generation, new.length))
    }

    /// The snapshot of a stream and its generation, for member `device`.
    pub fn snapshot(
        &self,
        device: &str,
        document: &str,
        of: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, Refusal> {
        check_document(document)?;
        let generation = {
            let state = self.lock();
            Self::role(&state, device)?;
            state
                .streams
                .get(&(document.to_owned(), of.to_owned()))
                .map(|s| s.generation)
        };
        let Some(generation) = generation else {
            return Ok(None);
        };
        let bytes = self
            .store
            .read(&self.snapshot_path(document, of))
            .map_err(io)?;
        Ok(bytes.map(|bytes| (generation, bytes)))
    }

    /// Stores asset `blob` uploaded by `device`; `false` if it existed
    /// already (then nothing is written).
    pub fn put_asset(&self, device: &str, blob: &str, envelope: &[u8]) -> Result<bool, Refusal> {
        let mut guard = self.lock();
        let state = &mut *guard;
        Self::role(state, device)?;
        if !state.members.may_write(device, "main") {
            return Err(Refusal::new(
                Code::Forbidden,
                "this device's role doesn't write main",
            ));
        }
        let path = self.path(&format!("assets/{blob}"));
        if self.store.size(&path).map_err(io)?.is_some() {
            return Ok(false);
        }
        let header =
            header_v2(envelope).map_err(|e| Refusal::new(Code::BadRequest, e.to_string()))?;
        if !state.members.may_write(&header.signer, "main") {
            return Err(Refusal::new(
                Code::Auth,
                "the asset is by a device that doesn't write main",
            ));
        }
        let public = state
            .members
            .device(&header.signer)
            .expect("a member")
            .public;
        let context = Context {
            project: &self.id,
            document: "main",
            kind: "asset",
            name: blob,
        };
        verify_v2(envelope, &public, &context)
            .map_err(|_| Refusal::new(Code::Auth, "the asset's signature doesn't match"))?;
        check_level(&state.members, header.level)?;
        let len = envelope.len() as u64;
        if state.bytes + len > self.limits.project_bytes {
            return Err(Refusal::new(Code::Quota, "the project's storage is full"));
        }
        self.store.write_atomic(&path, envelope).map_err(io)?;
        state.bytes += len;
        self.save_meta(state);
        Ok(true)
    }

    /// Asset `blob`, for member `device`.
    pub fn asset(&self, device: &str, blob: &str) -> Result<Option<Vec<u8>>, Refusal> {
        self.member(device)?;
        self.store
            .read(&self.path(&format!("assets/{blob}")))
            .map_err(io)
    }

    /// Stores the wrapped project key of invite `invite`, by its creator
    /// `device`, at server time `now`.
    pub fn put_invite_key(
        &self,
        device: &str,
        invite: &str,
        wrapped: &[u8],
        now: u64,
    ) -> Result<(), Refusal> {
        let mut guard = self.lock();
        let state = &mut *guard;
        Self::role(state, device)?;
        let creator = state
            .members
            .invites()
            .get(invite)
            .map(|i| i.creator.clone());
        if creator.as_deref() != Some(device) {
            return Err(Refusal::new(
                Code::Forbidden,
                "only the invite's creator stores its key",
            ));
        }
        if !invite_open(&state.members, invite, now) {
            self.delete_key_unless_open(state, invite, now);
            return Err(Refusal::new(Code::InviteClosed, "the invite is closed"));
        }
        if wrapped.is_empty() || wrapped.len() > MAX_INVITE_KEY {
            return Err(Refusal::new(
                Code::TooLarge,
                "wrapped keys are at most 16 KiB",
            ));
        }
        let path = self.path(&format!("invites/{invite}.key"));
        let old = self.store.size(&path).map_err(io)?.unwrap_or(0);
        self.store.write_atomic(&path, wrapped).map_err(io)?;
        state.bytes = state.bytes - old + wrapped.len() as u64;
        self.save_meta(state);
        Ok(())
    }

    /// The wrapped project key of open invite `invite` at server time `now`.
    pub fn invite_key(&self, invite: &str, now: u64) -> Result<Option<Vec<u8>>, Refusal> {
        let mut guard = self.lock();
        let state = &mut *guard;
        if state.deleted {
            return Err(Refusal::new(
                Code::UnknownProject,
                "the project was deleted",
            ));
        }
        if !invite_open(&state.members, invite, now) {
            self.delete_key_unless_open(state, invite, now);
            return Err(Refusal::new(Code::InviteClosed, "the invite is closed"));
        }
        self.store
            .read(&self.path(&format!("invites/{invite}.key")))
            .map_err(io)
    }

    /// Checks hello signature data for invite `invite` at server time
    /// `now`: returns the invite's public key if it is open.
    pub fn open_invite_key(&self, invite: &str, now: u64) -> Result<[u8; 32], Refusal> {
        let mut guard = self.lock();
        let state = &mut *guard;
        if !invite_open(&state.members, invite, now) {
            self.delete_key_unless_open(state, invite, now);
            return Err(Refusal::new(Code::InviteClosed, "the invite is closed"));
        }
        Ok(state.members.invites()[invite].key)
    }

    /// Deletes the wrapped key of `invite` if the invite isn't open.
    fn delete_key_unless_open(&self, state: &mut State, invite: &str, now: u64) {
        if invite_open(&state.members, invite, now) || check_id(invite, "invite").is_err() {
            return;
        }
        let path = self.path(&format!("invites/{invite}.key"));
        match self.store.size(&path) {
            Ok(Some(size)) => match self.store.delete(&path) {
                Ok(()) => {
                    state.bytes = state.bytes.saturating_sub(size);
                    self.save_meta(state);
                }
                Err(e) => error!(project = self.id, "can't delete an invite key: {e}"),
            },
            Ok(None) => {}
            Err(e) => error!(project = self.id, "can't read an invite key: {e}"),
        }
    }

    /// Deletes the wrapped keys of invites that aren't open at `now`.
    pub fn sweep_invites(&self, now: u64) {
        self.sweep_invites_locked(&mut self.lock(), now);
    }

    fn sweep_invites_locked(&self, state: &mut State, now: u64) {
        let names = match self.store.list(&self.path("invites")) {
            Ok(names) => names,
            Err(e) => {
                error!(project = self.id, "can't list invite keys: {e}");
                return;
            }
        };
        for name in names {
            if let Some(invite) = name.strip_suffix(".key") {
                self.delete_key_unless_open(state, invite, now);
            }
        }
    }

    /// Marks the project deleted and closes its connections; the caller
    /// deletes the files.
    pub fn close_for_deletion(&self) {
        let mut state = self.lock();
        state.deleted = true;
        for (_, peer) in state.peers.drain() {
            peer.close(Refusal::new(
                Code::UnknownProject,
                "the project was deleted",
            ));
        }
    }
}

/// Sends `message` to the subscribed device connections `advance` accepts:
/// it returns `Some` (after moving the connection's cursor past the
/// message) when the connection is exactly at the message's start. The
/// sender `from` gets no copy, only the cursor move. A connection whose
/// queue is full, or that is behind, is woken to catch up instead. With
/// `devices_only`, invitees are skipped.
fn deliver(
    peers: &mut HashMap<u64, Peer>,
    from: u64,
    mut advance: impl FnMut(&mut Subscription) -> Option<()>,
    message: &Message,
    devices_only: bool,
) {
    for (id, peer) in peers.iter_mut() {
        if devices_only && peer.device().is_none() {
            continue;
        }
        let Some(sub) = peer.sub.as_mut() else {
            continue;
        };
        if *id == from {
            advance(sub);
            continue;
        }
        // Check for room first, so the cursor only moves when it is sent.
        match peer.tx.clone().try_reserve_owned() {
            Ok(permit) => {
                if advance(sub).is_some() {
                    permit.send(message.clone());
                } else {
                    peer.wake.notify_one();
                }
            }
            Err(_) => peer.wake.notify_one(),
        }
    }
}
