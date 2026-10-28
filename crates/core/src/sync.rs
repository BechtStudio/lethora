//! Folder sync: an end-to-end encrypted copy (a "replica") of a project in a
//! folder that a sync tool shares (Dropbox, OneDrive, Syncthing, a USB
//! stick), and the live import of other devices' files.
//!
//! The project stays a plain local folder. [`Document::sync_setup`] creates a
//! replica in another [`Storage`] (the sync folder) and returns the
//! [`ProjectCode`]; another device joins with [`Document::sync_join`], which
//! builds a new local project from the replica; [`Document::sync_open`]
//! reopens a replica for an open project. [`Document::sync_push`] copies what
//! is new into the own replica files, [`Document::sync_pull`] imports what the
//! other devices added to theirs. [`Document::pull_local`] does the same as a
//! pull, without decryption, for the other devices' plain files in the
//! project folder itself (a project kept inside a synced folder).
//! [`replica_project`] tells, writing nothing, whether a code unlocks a
//! replica (to find a synced project by its code), [`stored_sync_settings`]
//! where a project not open syncs.
//!
//! # Replica format
//!
//! Format 1. As in the project folder, each device writes only its own files,
//! and there are no lock files. Paths are relative to the sync folder:
//!
//! - `replica.json`: `{"format": 1, "project": "<project id: 32 lowercase
//!   hex>", "check": "<hex: envelope of an empty payload, kind check>"}`, the
//!   project id random. `check` tells a wrong project code from damaged data.
//!   Written atomically and last at setup: its presence marks a complete
//!   replica.
//! - `main/<device>.snapshot`: one envelope (kind `snapshot`) holding all
//!   updates the device knew (as the project's own snapshot), written
//!   atomically at setup and at compaction.
//! - `main/<device>.log`: append-only records in the project log's framing,
//!   `[u32 LE len][u32 LE crc32][len bytes: envelope]`, each envelope (kind
//!   `log`) holding a Loro update export: what the device's document gained
//!   since its previous push, other devices' changes included, so the
//!   device's replica files hold everything it knows on their own. When the
//!   log passes the document's compaction threshold, a new snapshot is
//!   written first and the log truncated after.
//! - `review/<device>.snapshot` and `review/<device>.log`: the same for the
//!   `review` document (comments, marks, suggestions). A replica made before
//!   them gets them on the next open; until a device writes them it simply
//!   has none.
//! - `assets/<blob>`: one envelope (kind `asset`) per blob, holding its
//!   bytes, under the blob's local name (64 lowercase hex), written
//!   atomically, never changed or deleted.
//!
//! # Envelope
//!
//! Version 3 of [`lethora_protocol::envelope`] (built there): `[u8 version =
//! 3][u32 LE epoch = 1][u32 LE level][24 bytes nonce][ciphertext + 16 bytes
//! tag]`: XChaCha20-Poly1305 with the project key (32 random bytes) and a
//! random nonce; `level` is the feature level the payload needs (see
//! "Levels"). The associated data binds the envelope to its place: the
//! version byte, the epoch and the level (as in the header), then the ASCII
//! strings `lethora-replica-1`, the project id and the kind (`check`,
//! `snapshot`, `log` or `asset`), each followed by a zero byte, then the
//! name: the device id for `main`'s `snapshot` and `log`, `review/` and the
//! device id for `review`'s, the blob name for `asset`, nothing for `check`.
//! A record copied into another device's file, another document or another
//! project, or a blob stored under another name, fails to open. Version 1,
//! written before levels (the same without the level), is read as level 1;
//! other versions and epochs are a newer version's
//! ([`Error::UnsupportedFormat`]); the epoch is there for key rotation.
//! Versions 2 and 4, signed by the writing device, are the sync server's.
//! There is no protection against replaying a device's older records
//! (importing them again changes nothing).
//!
//! # Levels
//!
//! Records, snapshots and blobs are sealed at the project's level: the
//! highest level found in the records read so far, or the one this device
//! raised it to ([`Document::sync_require`], kept in its own state file; no
//! shared file changes), whichever is higher, and never above what this
//! version knows ([`lethora_protocol::LEVEL`]). So an older member keeps
//! reading a project that uses no newer feature.
//!
//! # Project code
//!
//! The project key for humans, and the recovery key: the 32 key bytes plus 3
//! check bytes (the start of SHA-256 of `lethora-project-code-1` and the key)
//! in Crockford base32, 56 characters, shown in 14 groups of 4 joined by `-`.
//! Parsing ignores case, spaces and dashes and reads `O` as `0`, `I` and `L`
//! as `1`; the check bytes catch typos before the key is tried.
//!
//! # Reading other devices
//!
//! For each other device the reader keeps what it has read: the snapshot's
//! size and the log's size and offset. A pull first lists `main/` (and
//! `review/` the same way) and stats each device's two files; a device whose
//! sizes are unchanged is not read.
//! Otherwise a changed snapshot size or a log shorter than the offset (the
//! device compacted) re-reads that device from scratch (snapshot, then the
//! log from 0); else the log is read from the offset. A record cut short (a
//! torn tail, e.g. still being synced) ends the read and is retried from the
//! same offset; an invalid record (crc, decryption or data Loro rejects)
//! re-reads the device from scratch once, then skips the rest of it until
//! its files change. Importing known updates again changes nothing, and
//! changes whose history is missing wait inside Loro.
//!
//! A record or snapshot of a newer version, or of a level above this
//! version's, is never skipped: reading that device stops before it (the
//! records before it are imported, its offset stays at its start) until
//! its files change, and [`Replica::needs_update`] says the app needs an
//! update. Pushing goes on: data of the older level stays valid for newer
//! versions. Plain project files ([`Document::pull_local`]) carry no level.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io;
use std::path::PathBuf;

use lethora_protocol::envelope::{self, Kind};
use loro::event::Diff;
use loro::{ContainerID, ContainerType, ExportMode, LoroDoc, VersionVector};

use crate::review::Review;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Files;
use crate::Result;
use crate::{
    ASSETS, BIBLIOGRAPHY, CHECKPOINTS, CHECKPOINTS_MAP, CONTENT, DeviceId, DeviceState, Document,
    Error, Frame, Identity, LAYOUT, LAYOUT_CUSTOM, LOG_EXT, LogFile, MAIN, META, OVERRIDES,
    PROJECT, REVIEW, SNAPSHOT_EXT, ServerSettings, Storage, SyncSettings, TextChange, blob_path,
    checkpoints_path, export_snapshot, frame, from_hex, has_format1, is_blob_name, lock_path,
    log_in, log_path, new_loro_doc, parse_frame, parse_hex_id, snapshot_in, snapshot_path,
    state_path, to_changes, to_hex, write_project_file,
};

const REPLICA: &str = "replica.json";
const REPLICA_FORMAT: u64 = 1;
const KEY_LEN: usize = 32;
/// Check bytes at the end of a project code.
const CODE_CHECK_LEN: usize = 3;
/// Characters of a project code: (32 + 3) bytes = 280 bits = 56 x 5 bits.
const CODE_LEN: usize = (KEY_LEN + CODE_CHECK_LEN) * 8 / 5;
const CODE_GROUP: usize = 4;
const CODE_TAG: &[u8] = b"lethora-project-code-1";
/// Crockford's base32 alphabet.
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The project key as a code people can copy and type (see the module
/// docs). [`Display`](fmt::Display) gives the grouped code; `Debug` never
/// shows the key.
#[derive(Clone, PartialEq, Eq)]
pub struct ProjectCode([u8; KEY_LEN]);

impl ProjectCode {
    /// The code of project key `key` (e.g. as stored by the app).
    pub fn from_key(key: [u8; KEY_LEN]) -> Self {
        Self(key)
    }

    /// The project key.
    pub fn key(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    /// Reads a code as typed: case, spaces and dashes don't matter, `O` reads
    /// as `0`, `I` and `L` as `1`. Fails with [`Error::InvalidCode`] if it has
    /// the wrong length, a character Crockford base32 doesn't use, or a typo
    /// (its check bytes don't match).
    pub fn parse(code: &str) -> Result<Self> {
        let mut digits = Vec::with_capacity(CODE_LEN);
        for c in code.chars() {
            if c == '-' || c.is_whitespace() {
                continue;
            }
            let c = match c.to_ascii_uppercase() {
                'O' => '0',
                'I' | 'L' => '1',
                c => c,
            };
            let digit = CROCKFORD
                .iter()
                .position(|&d| char::from(d) == c)
                .ok_or_else(|| Error::InvalidCode(format!("\"{c}\" is not used in codes")))?;
            digits.push(digit as u8);
        }
        if digits.len() != CODE_LEN {
            return Err(Error::InvalidCode(format!(
                "it has {} characters instead of {CODE_LEN}",
                digits.len()
            )));
        }
        let bytes = from_base32(&digits);
        let (key, check) = bytes.split_at(KEY_LEN);
        if check != code_check(key) {
            return Err(Error::InvalidCode("it has a typo".into()));
        }
        Ok(Self(key.try_into().expect("32 bytes")))
    }

    /// A new random project key.
    fn generate() -> Result<Self> {
        random().map(Self)
    }
}

impl fmt::Display for ProjectCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut bytes = self.0.to_vec();
        bytes.extend_from_slice(&code_check(&self.0));
        let code = to_base32(&bytes);
        for (i, group) in code.as_bytes().chunks(CODE_GROUP).enumerate() {
            if i > 0 {
                f.write_str("-")?;
            }
            f.write_str(std::str::from_utf8(group).expect("ASCII"))?;
        }
        Ok(())
    }
}

impl fmt::Debug for ProjectCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProjectCode(..)")
    }
}

/// The check bytes of a project code for `key`.
fn code_check(key: &[u8]) -> [u8; CODE_CHECK_LEN] {
    let hash = Sha256::new()
        .chain_update(CODE_TAG)
        .chain_update(key)
        .finalize();
    [hash[0], hash[1], hash[2]]
}

/// `bytes` in Crockford base32, most significant bits first (the length in
/// bits must be a multiple of 5, as for a project code).
fn to_base32(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 8 / 5);
    let (mut acc, mut bits) = (0u32, 0);
    for &b in bytes {
        acc = (acc << 8 | u32::from(b)) & 0xffff;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(CROCKFORD[(acc >> bits & 31) as usize]));
        }
    }
    out
}

/// The bytes of base32 `digits` (values 0..32), the inverse of [`to_base32`].
fn from_base32(digits: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(digits.len() * 5 / 8);
    let (mut acc, mut bits) = (0u32, 0);
    for &d in digits {
        acc = (acc << 5 | u32::from(d)) & 0xffff;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// `N` random bytes.
fn random<const N: usize>() -> Result<[u8; N]> {
    lethora_protocol::random().map_err(from_protocol)
}

/// A protocol error as a core error.
pub(crate) fn from_protocol(e: lethora_protocol::Error) -> Error {
    match e {
        lethora_protocol::Error::Unsupported(why) => Error::UnsupportedFormat(why),
        lethora_protocol::Error::Random(why) => Error::Io(io::Error::other(why)),
        lethora_protocol::Error::Corrupt(why) => Error::Corrupt(why),
        e => Error::Corrupt(e.to_string()),
    }
}

/// Seals and opens envelopes of one project (see the module docs), with the
/// core's errors, for a reader of a feature level (see "Levels").
pub(crate) struct Sealer {
    inner: envelope::Sealer,
    /// The highest level opened: [`lethora_protocol::LEVEL`] but in tests.
    reader: u32,
}

impl Sealer {
    fn new(project: &str, key: &[u8; KEY_LEN]) -> Self {
        Self {
            inner: envelope::Sealer::new(project, key),
            reader: lethora_protocol::LEVEL,
        }
    }

    fn seal(&self, kind: Kind, name: &str, level: u32, plain: &[u8]) -> Result<Vec<u8>> {
        self.inner
            .seal(kind, name, level, plain)
            .map_err(from_protocol)
    }

    /// The payload of `envelope` and the level it needs; [`Error::Corrupt`]
    /// if it is damaged or doesn't belong here, [`Error::UnsupportedFormat`]
    /// for another version or epoch, or a level above the reader's.
    fn open(&self, kind: Kind, name: &str, envelope: &[u8]) -> Result<(Vec<u8>, u32)> {
        let (plain, level) = self
            .inner
            .open_leveled(kind, name, envelope)
            .map_err(from_protocol)?;
        if level > self.reader {
            return Err(Error::UnsupportedFormat(format!(
                "{} {name} needs feature level {level}; update Lethora to read it",
                kind.as_str()
            )));
        }
        Ok((plain, level))
    }
}

/// `replica.json` (see the module docs).
#[derive(Debug, Serialize, Deserialize)]
struct ReplicaFile {
    format: u64,
    project: String,
    check: String,
}

/// Reads and checks `replica.json`.
fn read_replica_file(storage: &dyn Storage) -> Result<ReplicaFile> {
    #[derive(Deserialize)]
    struct Format {
        format: u64,
    }
    let Some(json) = storage.read(REPLICA)? else {
        return Err(Error::NotFound(format!(
            "no synced project in {}",
            storage.location().display()
        )));
    };
    match serde_json::from_slice::<Format>(&json) {
        Ok(f) if f.format == REPLICA_FORMAT => {}
        Ok(f) if f.format > REPLICA_FORMAT => {
            return Err(Error::UnsupportedFormat(format!(
                "synced project format {} was made by a newer Lethora; update Lethora to open it",
                f.format
            )));
        }
        _ => {
            return Err(Error::UnsupportedFormat(format!(
                "{REPLICA} is unreadable or names an unknown format"
            )));
        }
    }
    let file: ReplicaFile =
        serde_json::from_slice(&json).map_err(|e| Error::Corrupt(format!("{REPLICA}: {e}")))?;
    parse_hex_id(&file.project, "project")?;
    Ok(file)
}

/// Whether `code` unlocks the replica of `file` (opens its check);
/// [`Error::UnsupportedFormat`] for a check of another version or epoch.
fn unlocks(file: &ReplicaFile, code: &ProjectCode) -> Result<bool> {
    let check = from_hex(&file.check)
        .ok_or_else(|| Error::Corrupt(format!("{REPLICA}: the check is not hex")))?;
    match Sealer::new(&file.project, code.key()).open(Kind::Check, "", &check) {
        Ok((payload, _)) => Ok(payload.is_empty()),
        Err(e @ Error::UnsupportedFormat(_)) => Err(e),
        Err(_) => Ok(false),
    }
}

/// The project id of the replica in `storage` if `code` unlocks it, `None`
/// if it doesn't, to find a synced project by its code. Reads only
/// `replica.json` and writes nothing. Fails with [`Error::NotFound`] without
/// `replica.json` and [`Error::UnsupportedFormat`] for a format this version
/// can't read.
pub fn replica_project(storage: &dyn Storage, code: &ProjectCode) -> Result<Option<String>> {
    let file = read_replica_file(storage)?;
    Ok(unlocks(&file, code)?.then_some(file.project))
}

/// The sync settings `device` stored in the project in `storage` (see
/// [`Document::sync_settings`]), read without opening the project: `None`
/// if there are none or the state file can't be read. Writes nothing.
pub fn stored_sync_settings(storage: &dyn Storage, device: &DeviceId) -> Option<SyncSettings> {
    let json = storage.read(&state_path(device)).ok()??;
    serde_json::from_slice::<DeviceState>(&json).ok()?.sync
}

/// The server settings `device` stored in the project in `storage` (see
/// [`Document::server_settings`]), read without opening the project, like
/// [`stored_sync_settings`].
pub fn stored_server_settings(storage: &dyn Storage, device: &DeviceId) -> Option<ServerSettings> {
    let json = storage.read(&state_path(device)).ok()??;
    serde_json::from_slice::<DeviceState>(&json).ok()?.server
}

/// What has been read of one other device's files (see the module docs,
/// "Reading other devices").
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct DeviceRead {
    /// Size of the snapshot when last read (`None`: there was none).
    snapshot: Option<u64>,
    /// Size of the log when last looked at.
    log: u64,
    /// Where the next log record starts.
    offset: u64,
    /// The highest level of what was read (see the module docs, "Levels").
    level: u32,
    /// Reading stopped at a newer version's record or snapshot.
    newer: bool,
}

/// What has been read of each other device.
pub(crate) type Reads = BTreeMap<DeviceId, DeviceRead>;

/// The current sizes of a device's snapshot and log.
type Sizes = (Option<u64>, u64);

/// The devices with files in `dir` of `storage` (other than `own`, if
/// given) whose sizes differ from `reads`, with their sizes. Devices that
/// can't be stat'ed are added to `skipped`.
fn changed_devices(
    storage: &dyn Storage,
    dir: &str,
    own: Option<&DeviceId>,
    reads: &Reads,
    skipped: &mut Vec<String>,
) -> Result<Vec<(DeviceId, Sizes)>> {
    let devices: BTreeSet<DeviceId> = storage
        .list(dir)?
        .iter()
        .filter_map(|n| n.strip_suffix(SNAPSHOT_EXT).or(n.strip_suffix(LOG_EXT)))
        .filter_map(|d| DeviceId::parse(d).ok())
        .filter(|d| Some(d) != own)
        .collect();
    let mut changed = Vec::new();
    for device in devices {
        let sizes = storage
            .size(&snapshot_in(dir, &device))
            .and_then(|s| Ok((s, storage.size(&log_in(dir, &device))?.unwrap_or(0))));
        match sizes {
            Ok(sizes) => {
                let read = reads.get(&device);
                if read.is_none_or(|r| (r.snapshot, r.log) != sizes) {
                    changed.push((device, sizes));
                }
            }
            Err(e) => skipped.push(format!("device {}: {e}", device.as_str())),
        }
    }
    Ok(changed)
}

/// Reads the other devices' files in `dir` of `storage` (other than `own`,
/// if given) into `doc` from scratch, opening envelopes with `sealer` (plain
/// files without). Returns what was read and the devices skipped, with the
/// reason.
pub(crate) fn read_others(
    storage: &dyn Storage,
    dir: &str,
    own: Option<&DeviceId>,
    doc: &LoroDoc,
    sealer: Option<&Sealer>,
) -> Result<(Reads, Vec<String>)> {
    let mut reads = Reads::new();
    let mut skipped = Vec::new();
    for (device, sizes) in changed_devices(storage, dir, own, &reads, &mut skipped)? {
        let read = reads.entry(device.clone()).or_default();
        if let Err(e) = read_device(storage, dir, &device, sizes, read, sealer, doc) {
            skipped.push(format!("device {}: {e}", device.as_str()));
        }
    }
    Ok((reads, skipped))
}

/// Reads what is new in `device`'s files in `dir` (now `sizes`) into `doc` and
/// updates `read` (see the module docs, "Reading other devices"). Fails with
/// the reason when part of them can't be read; `read` then still records the
/// sizes, so the device is tried again only once its files change.
fn read_device(
    storage: &dyn Storage,
    dir: &str,
    device: &DeviceId,
    sizes: Sizes,
    read: &mut DeviceRead,
    sealer: Option<&Sealer>,
    doc: &LoroDoc,
) -> std::result::Result<(), String> {
    let (snapshot, log) = sizes;
    let fresh = snapshot != read.snapshot || log < read.offset;
    (read.snapshot, read.log) = sizes;
    read.newer = false;
    // On an invalid record, everything again from scratch.
    if !fresh {
        match read_log(
            storage,
            dir,
            device,
            read.offset,
            sealer,
            doc,
            &mut read.level,
        ) {
            Ok(offset) => {
                read.offset = offset;
                return Ok(());
            }
            Err((offset, Stop::Newer)) => {
                (read.offset, read.newer) = (offset, true);
                return Ok(());
            }
            Err((_, Stop::Bad(_))) => {}
        }
    }
    read.offset = 0;
    if snapshot.is_some() {
        let imported = storage
            .read(&snapshot_in(dir, device))
            .and_then(|bytes| match bytes {
                Some(bytes) => {
                    let (update, level) = open_record(sealer, Kind::Snapshot, dir, device, &bytes)?;
                    doc.import(&update)?;
                    read.level = read.level.max(level);
                    Ok(())
                }
                None => Ok(()),
            });
        match imported {
            // A newer version's: nothing of this device until its files
            // change (see the module docs).
            Err(Error::UnsupportedFormat(_)) => {
                read.newer = true;
                return Ok(());
            }
            imported => imported.map_err(|e| format!("snapshot: {e}"))?,
        }
    }
    match read_log(storage, dir, device, 0, sealer, doc, &mut read.level) {
        Ok(offset) => {
            read.offset = offset;
            Ok(())
        }
        Err((offset, Stop::Newer)) => {
            (read.offset, read.newer) = (offset, true);
            Ok(())
        }
        Err((offset, Stop::Bad(e))) => {
            read.offset = offset;
            Err(format!("log at byte {offset}: {e}"))
        }
    }
}

/// Why reading a log stopped before its end.
enum Stop {
    /// At a newer version's record (see the module docs).
    Newer,
    /// At an invalid record, with the reason.
    Bad(String),
}

/// Imports the records of `device`'s log in `dir` from byte `from` into
/// `doc`, raising `level` to theirs, and returns where the next record
/// starts: at the end, or at a record cut short. Stops at a newer version's
/// record or an invalid one, with its offset.
fn read_log(
    storage: &dyn Storage,
    dir: &str,
    device: &DeviceId,
    from: u64,
    sealer: Option<&Sealer>,
    doc: &LoroDoc,
    level: &mut u32,
) -> std::result::Result<u64, (u64, Stop)> {
    let bytes = storage
        .read_from(&log_in(dir, device), from)
        .map_err(|e| (from, Stop::Bad(e.to_string())))?
        .unwrap_or_default();
    let mut at = 0;
    loop {
        let offset = from + at as u64;
        match parse_frame(&bytes, at) {
            Frame::Record(payload, next) => {
                let imported = open_record(sealer, Kind::Log, dir, device, payload)
                    .and_then(|(update, level)| Ok(doc.import(&update).map(|_| level)?));
                match imported {
                    Ok(read) => *level = (*level).max(read),
                    Err(Error::UnsupportedFormat(_)) => return Err((offset, Stop::Newer)),
                    Err(e) => return Err((offset, Stop::Bad(e.to_string()))),
                }
                at = next;
            }
            Frame::Short => return Ok(offset),
            Frame::Bad => {
                return Err((offset, Stop::Bad("the record fails its checksum".into())));
            }
        }
    }
}

/// The payload of a record of `device` in `dir` and the level it needs:
/// opened with `sealer`, or as it is without one (plain project files,
/// level 1).
fn open_record<'a>(
    sealer: Option<&Sealer>,
    kind: Kind,
    dir: &str,
    device: &DeviceId,
    bytes: &'a [u8],
) -> Result<(Cow<'a, [u8]>, u32)> {
    match sealer {
        Some(sealer) => sealer
            .open(kind, &record_name(dir, device), bytes)
            .map(|(plain, level)| (Cow::Owned(plain), level)),
        None => Ok((Cow::Borrowed(bytes), 1)),
    }
}

/// The envelope name of `device`'s snapshot and log records in `dir` (see
/// the module docs, "Envelope").
fn record_name(dir: &str, device: &DeviceId) -> String {
    if dir == MAIN {
        device.as_str().to_owned()
    } else {
        format!("{dir}/{}", device.as_str())
    }
}

/// What a pull ([`Document::sync_pull`], [`Document::pull_local`]) changed,
/// so the app can update without reloading.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Pulled {
    /// Whether the document (`main`) changed at all (bookkeeping such as
    /// `peers` included); `false` means nothing else below is set but
    /// `review`, `blobs` and `skipped`.
    pub changed: bool,
    /// What changed in the content text, in CodeMirror change-set semantics
    /// (UTF-16 offsets into the text before the pull, see [`TextChange`]).
    pub changes: Vec<TextChange>,
    /// Whether the settings changed: the `layout`, `meta` or `overrides` map
    /// or the expert code.
    pub settings: bool,
    /// Whether the bibliography changed.
    pub bibliography: bool,
    /// Whether the asset index changed.
    pub assets: bool,
    /// Whether the named checkpoints changed. A named checkpoint whose
    /// history was missing can also become listed with any other change.
    pub checkpoints: bool,
    /// Whether the `review` document changed (review items, or its
    /// bookkeeping); the app lists the items again.
    pub review: bool,
    /// How many blobs arrived in the local `assets/` folder.
    pub blobs: usize,
    /// Devices and blobs that could not be read, with the reason, for
    /// logging. Each is reported once and tried again when its files change.
    pub skipped: Vec<String>,
}

/// Imports what is new in the other devices' files in `dir` of `storage`
/// into `doc` (see the module docs) and reports what changed.
fn import_new(
    doc: &LoroDoc,
    storage: &dyn Storage,
    dir: &str,
    own: &DeviceId,
    reads: &mut Reads,
    sealer: Option<&Sealer>,
) -> Result<Pulled> {
    let mut pulled = Pulled::default();
    let changed = changed_devices(storage, dir, Some(own), reads, &mut pulled.skipped)?;
    if changed.is_empty() {
        return Ok(pulled);
    }
    // Local edits must not share a commit with, or follow, the imports.
    doc.commit();
    let before = doc.oplog_frontiers();
    let old = doc.get_text(CONTENT).to_string();
    for (device, sizes) in changed {
        let read = reads.entry(device.clone()).or_default();
        if let Err(e) = read_device(storage, dir, &device, sizes, read, sealer, doc) {
            pulled
                .skipped
                .push(format!("device {}: {e}", device.as_str()));
        }
    }
    let after = doc.oplog_frontiers();
    if after == before {
        return Ok(pulled);
    }
    pulled.changed = true;
    report(&doc.diff(&before, &after)?, &old, &mut pulled);
    Ok(pulled)
}

/// Fills `pulled` from `diff`, the difference a pull made; `old` is the
/// content text before it.
pub(crate) fn report(diff: &loro::event::DiffBatch, old: &str, pulled: &mut Pulled) {
    for (cid, d) in diff.iter() {
        let ContainerID::Root {
            name,
            container_type,
        } = cid
        else {
            continue;
        };
        match (name.as_str(), container_type) {
            (CONTENT, ContainerType::Text) => {
                if let Diff::Text(delta) = d {
                    pulled.changes = to_changes(old, delta);
                }
            }
            (LAYOUT | META | OVERRIDES, ContainerType::Map)
            | (LAYOUT_CUSTOM, ContainerType::Text) => pulled.settings = true,
            (BIBLIOGRAPHY, ContainerType::Text) => pulled.bibliography = true,
            (ASSETS, ContainerType::Map) => pulled.assets = true,
            (CHECKPOINTS_MAP, ContainerType::Map) => pulled.checkpoints = true,
            _ => {}
        }
    }
}

/// An open replica of a project in a sync folder, for one device (see the
/// module docs). Made by [`Document::sync_setup`], [`Document::sync_open`]
/// or [`Document::sync_join`]; used with [`Document::sync_push`] and
/// [`Document::sync_pull`] of that device's document.
pub struct Replica {
    storage: Box<dyn Storage>,
    project: String,
    code: ProjectCode,
    sealer: Sealer,
    device: DeviceId,
    /// `main`'s replica files.
    main: ReplicaFiles,
    /// `review`'s replica files.
    review: ReplicaFiles,
    /// Blobs known to be in the replica.
    blobs: BTreeSet<String>,
    /// Blobs known to be in the local project.
    local_blobs: BTreeSet<String>,
    /// Replica blobs that failed to open, with their size then.
    bad_blobs: BTreeMap<String, Option<u64>>,
    /// The document version at which every indexed blob was pushed.
    blobs_pushed: Option<VersionVector>,
    /// The document version at which every indexed blob was pulled.
    blobs_pulled: Option<VersionVector>,
    /// The level this device raised the project to (see the module docs,
    /// "Levels").
    raised: u32,
}

/// One document's files in a replica: the own ones and what was read of
/// the other devices'.
struct ReplicaFiles {
    /// `main` or `review`.
    dir: &'static str,
    /// The own replica log.
    log: Box<dyn LogFile>,
    /// The version of the own replica files.
    pushed_vv: VersionVector,
    /// The own snapshot is unreadable: the next push rewrites it.
    compact: bool,
    /// What has been read of the other devices' replica files.
    reads: Reads,
}

impl ReplicaFiles {
    /// Reads `device`'s own files of `dir` in `storage` (repairing a torn
    /// log tail) for the version they hold, creating the folder and the log
    /// if missing.
    fn load(
        storage: &dyn Storage,
        sealer: &Sealer,
        dir: &'static str,
        device: &DeviceId,
    ) -> Result<Self> {
        storage.create_dir(dir)?;
        let own = new_loro_doc();
        let name = record_name(dir, device);
        let mut compact = false;
        // Own files of a newer version (this device ran one): never
        // rewritten by this one.
        let newer = |e: Error| match e {
            e @ Error::UnsupportedFormat(_) => Err(e),
            _ => Ok(()),
        };
        if let Some(bytes) = storage.read(&snapshot_in(dir, device))? {
            let imported = sealer
                .open(Kind::Snapshot, &name, &bytes)
                .and_then(|(s, _)| Ok(own.import(&s).map(|_| ())?));
            if let Err(e) = imported {
                newer(e)?;
                compact = true;
            }
        }
        let mut log = storage.open_log(&log_in(dir, device))?;
        let bytes = log.read_all()?;
        let mut good = 0;
        while let Frame::Record(payload, next) = parse_frame(&bytes, good) {
            let imported = sealer
                .open(Kind::Log, &name, payload)
                .and_then(|(u, _)| Ok(own.import(&u).map(|_| ())?));
            if let Err(e) = imported {
                newer(e)?;
                break;
            }
            good = next;
        }
        if good < bytes.len() {
            log.truncate(good as u64)?;
        }
        Ok(Self {
            dir,
            log,
            pushed_vv: own.oplog_vv(),
            compact,
            reads: Reads::new(),
        })
    }

    /// Writes a fresh own snapshot of `doc` and an empty log (setup).
    fn create(
        storage: &dyn Storage,
        sealer: &Sealer,
        dir: &'static str,
        device: &DeviceId,
        doc: &LoroDoc,
    ) -> Result<Self> {
        storage.create_dir(dir)?;
        let name = record_name(dir, device);
        // A new project's level.
        let snapshot = sealer.seal(Kind::Snapshot, &name, 1, &export_snapshot(doc)?)?;
        storage.write_atomic(&snapshot_in(dir, device), &snapshot)?;
        let mut log = storage.open_log(&log_in(dir, device))?;
        log.truncate(0)?;
        Ok(Self {
            dir,
            log,
            pushed_vv: doc.oplog_vv(),
            compact: false,
            reads: Reads::new(),
        })
    }

    /// Appends what `doc` gained since the last push as one record at
    /// `level`, then compacts like the local log (see
    /// [`Document::sync_push`]).
    fn push(
        &mut self,
        storage: &dyn Storage,
        sealer: &Sealer,
        device: &DeviceId,
        doc: &LoroDoc,
        threshold: u64,
        level: u32,
    ) -> Result<()> {
        let name = record_name(self.dir, device);
        let vv = doc.oplog_vv();
        if vv != self.pushed_vv {
            let update = doc.export(ExportMode::updates(&self.pushed_vv))?;
            let sealed = sealer.seal(Kind::Log, &name, level, &update)?;
            self.log.append(&frame(&sealed)?)?;
            self.pushed_vv = vv;
        }
        if self.compact || self.log.size() > threshold {
            let sealed = sealer.seal(Kind::Snapshot, &name, level, &export_snapshot(doc)?)?;
            storage.write_atomic(&snapshot_in(self.dir, device), &sealed)?;
            self.log.truncate(0)?;
            self.compact = false;
        }
        Ok(())
    }
}

impl Replica {
    /// The project id (from `replica.json`), for the app to remember.
    pub fn project_id(&self) -> &str {
        &self.project
    }

    /// The project code (holding the key), for the app to keep and show.
    pub fn code(&self) -> &ProjectCode {
        &self.code
    }

    /// The sync folder.
    pub fn location(&self) -> PathBuf {
        self.storage.location()
    }

    /// The project's level, at which this device writes (see the module
    /// docs, "Levels").
    pub fn level(&self) -> u32 {
        let read = self.main.reads.values().chain(self.review.reads.values());
        read.map(|r| r.level)
            .fold(self.raised.max(1), u32::max)
            .min(self.sealer.reader)
    }

    /// Why syncing needs a newer Lethora, if reading stopped at a newer
    /// version's record (see the module docs, "Reading other devices").
    pub fn needs_update(&self) -> Option<&'static str> {
        let mut read = self.main.reads.values().chain(self.review.reads.values());
        read.any(|r| r.newer)
            .then_some("This project uses features of a newer Lethora; update to keep syncing.")
    }

    /// Reads as a version of feature level `level` would (for tests: an
    /// older client).
    pub fn set_reader_level(&mut self, level: u32) {
        self.sealer.reader = level;
    }

    fn new(
        storage: Box<dyn Storage>,
        project: String,
        code: &ProjectCode,
        device: &DeviceId,
        main: ReplicaFiles,
        review: ReplicaFiles,
    ) -> Self {
        Self {
            sealer: Sealer::new(&project, code.key()),
            storage,
            project,
            code: code.clone(),
            device: device.clone(),
            main,
            review,
            blobs: BTreeSet::new(),
            local_blobs: BTreeSet::new(),
            bad_blobs: BTreeMap::new(),
            blobs_pushed: None,
            blobs_pulled: None,
            raised: 1,
        }
    }

    /// Opens the replica in `storage` for `device`: checks `code` against
    /// `replica.json`, then reads the device's own replica files of both
    /// documents (repairing a torn log tail) for the versions they hold.
    fn load(storage: Box<dyn Storage>, code: &ProjectCode, device: &DeviceId) -> Result<Self> {
        let file = read_replica_file(&*storage)?;
        if !unlocks(&file, code)? {
            return Err(Error::WrongCode(storage.location()));
        }
        let sealer = Sealer::new(&file.project, code.key());
        let main = ReplicaFiles::load(&*storage, &sealer, MAIN, device)?;
        let review = ReplicaFiles::load(&*storage, &sealer, REVIEW, device)?;
        Ok(Self::new(storage, file.project, code, device, main, review))
    }
}

impl Document {
    /// Starts folder sync: creates a replica of this project in `storage`
    /// (the sync folder) with a new project id and key, holding a snapshot of
    /// everything and every indexed blob, `replica.json` written last.
    /// Returns the open replica and the project code other devices join
    /// with. Fails with [`Error::AlreadyExists`] if `storage` holds a replica
    /// already (join it instead).
    pub fn sync_setup(&mut self, storage: Box<dyn Storage>) -> Result<(Replica, ProjectCode)> {
        if self.server.is_some() {
            return Err(Error::InvalidChange(
                "this project is shared on a server; it can't sync through a folder too".into(),
            ));
        }
        storage.create_dir("")?;
        if storage.read(REPLICA)?.is_some() {
            return Err(Error::AlreadyExists(storage.location()));
        }
        self.autosave()?;
        let code = ProjectCode::generate()?;
        let project = to_hex(&random::<16>()?);
        let device = &self.identity.device;
        let sealer = Sealer::new(&project, code.key());
        let main = ReplicaFiles::create(&*storage, &sealer, MAIN, device, &self.doc)?;
        let review = ReplicaFiles::create(&*storage, &sealer, REVIEW, device, &self.review.doc)?;
        let mut replica = Replica::new(storage, project, &code, device, main, review);
        self.push_blobs(&mut replica)?;
        let file = ReplicaFile {
            format: REPLICA_FORMAT,
            project: replica.project.clone(),
            check: to_hex(&replica.sealer.seal(Kind::Check, "", 1, &[])?),
        };
        let json = serde_json::to_vec_pretty(&file)
            .map_err(|e| Error::Corrupt(format!("serializing {REPLICA}: {e}")))?;
        // Written last: `replica.json` marks a complete replica.
        replica.storage.write_atomic(REPLICA, &json)?;
        Ok((replica, code))
    }

    /// Opens the replica in `storage` (set up earlier for this project) with
    /// `code`. The first pull then reads every other device once. Fails with
    /// [`Error::NotFound`] without `replica.json`, [`Error::UnsupportedFormat`]
    /// for a format this version can't read and [`Error::WrongCode`] if
    /// `code` doesn't unlock it.
    pub fn sync_open(&self, storage: Box<dyn Storage>, code: &ProjectCode) -> Result<Replica> {
        let mut replica = Replica::load(storage, code, &self.identity.device)?;
        replica.raised = self.sync.as_ref().and_then(|s| s.level).unwrap_or(1);
        Ok(replica)
    }

    /// Raises the project's level to `level` for this device's writes (see
    /// the module docs, "Levels"), kept in its state file when folder sync
    /// settings are set; never lowered. Refused for a level this version
    /// doesn't know.
    pub fn sync_require(&mut self, replica: &mut Replica, level: u32) -> Result<()> {
        self.check_replica(replica)?;
        if level > replica.sealer.reader {
            return Err(Error::InvalidChange(format!(
                "this version of Lethora doesn't know feature level {level}"
            )));
        }
        if let Some(mut settings) = self.sync.clone()
            && settings.level.unwrap_or(1) < level
        {
            settings.level = Some(level);
            self.set_sync_settings(Some(settings))?;
        }
        replica.raised = replica.raised.max(level);
        Ok(())
    }

    /// Joins a synced project: opens the replica in `replica` with `code`
    /// (failing like [`Document::sync_open`], before anything is written),
    /// then creates a new local project in `local` from every device's
    /// replica files and blobs (files that can't be read are skipped, as on
    /// open), edited by `identity`. Adds no change of its own to the
    /// document. Fails with [`Error::AlreadyExists`] if `local` holds a
    /// project.
    pub fn sync_join(
        local: Box<dyn Storage>,
        replica: Box<dyn Storage>,
        code: &ProjectCode,
        identity: &Identity,
    ) -> Result<(Document, Replica)> {
        let device = &identity.device;
        let mut replica = Replica::load(replica, code, device)?;
        local.create_dir("")?;
        let lock = local.lock(&lock_path(device))?;
        if local.read(PROJECT)?.is_some() || has_format1(&*local)? {
            return Err(Error::AlreadyExists(local.location()));
        }
        let doc = new_loro_doc();
        let review_doc = new_loro_doc();
        // The own replica files too, if this device was in the project before.
        for (files, doc) in [
            (&mut replica.main, &doc),
            (&mut replica.review, &review_doc),
        ] {
            let (mut reads, _skipped) = read_others(
                &*replica.storage,
                files.dir,
                None,
                doc,
                Some(&replica.sealer),
            )?;
            reads.remove(device);
            files.reads = reads;
        }
        local.create_dir(MAIN)?;
        local.create_dir(CHECKPOINTS)?;
        local.write_atomic(&snapshot_path(device), &export_snapshot(&doc)?)?;
        let mut log = local.open_log(&log_path(device))?;
        log.truncate(0)?;
        local.write_atomic(&checkpoints_path(device), b"[]")?;
        let saved_vv = doc.oplog_vv();
        let mut document =
            Document::from_parts(local, identity, doc, log, saved_vv, Vec::new(), None, lock);
        let review_files = Files {
            dir: REVIEW,
            log: None,
            saved_vv: VersionVector::new(),
        };
        document.review = Review::new(review_doc, review_files, Reads::new());
        // `review`'s own files: everything the replica holds.
        document.review.files.save(
            &*document.storage,
            device,
            &document.review.doc,
            document.compaction_threshold,
        )?;
        document.pull_blobs(&mut replica, &mut Pulled::default())?;
        // Written last: `project.json` marks the folder as a complete project.
        write_project_file(&*document.storage)?;
        Ok((document, replica))
    }

    /// Autosaves, then copies what is new into the own replica files: blobs
    /// the asset index names that the replica lacks (before the records
    /// naming them), then per document (`main`, `review`) one log record
    /// with everything it gained since the last push; compacts each replica
    /// log like the local one. Writes nothing when there is nothing new.
    pub fn sync_push(&mut self, replica: &mut Replica) -> Result<()> {
        self.check_replica(replica)?;
        self.autosave()?;
        self.push_blobs(replica)?;
        let (threshold, level) = (self.compaction_threshold, replica.level());
        let (storage, sealer, device) = (&*replica.storage, &replica.sealer, &self.identity.device);
        replica
            .main
            .push(storage, sealer, device, &self.doc, threshold, level)?;
        replica
            .review
            .push(storage, sealer, device, &self.review.doc, threshold, level)
    }

    /// Imports what the other devices added to their replica files since the
    /// last pull (see the module docs, "Reading other devices"), then the
    /// blobs the asset index names that are missing locally. Cheap when
    /// nothing changed: a listing and two sizes per device. Remote changes
    /// never land on the undo stack; the next autosave writes them into the
    /// own log. A device or blob that can't be read is skipped (and
    /// reported), never failing the pull.
    pub fn sync_pull(&mut self, replica: &mut Replica) -> Result<Pulled> {
        self.check_replica(replica)?;
        let mut pulled = import_new(
            &self.doc,
            &*replica.storage,
            MAIN,
            &self.identity.device,
            &mut replica.main.reads,
            Some(&replica.sealer),
        )?;
        let mut review = import_new(
            &self.review.doc,
            &*replica.storage,
            REVIEW,
            &self.identity.device,
            &mut replica.review.reads,
            Some(&replica.sealer),
        )?;
        pulled.review = review.changed;
        pulled.skipped.append(&mut review.skipped);
        self.after_pull(&pulled);
        self.pull_blobs(replica, &mut pulled)?;
        Ok(pulled)
    }

    /// [`Document::sync_pull`] for the other devices' plain files in the
    /// project folder itself (no decryption; their blobs are in the same
    /// `assets/` already), `main/` and `review/`. Continues from what
    /// opening read, so the first call reads only what was added since.
    pub fn pull_local(&mut self) -> Result<Pulled> {
        let mut pulled = import_new(
            &self.doc,
            &*self.storage,
            MAIN,
            &self.identity.device,
            &mut self.local_reads,
            None,
        )?;
        let mut review = import_new(
            &self.review.doc,
            &*self.storage,
            REVIEW,
            &self.identity.device,
            &mut self.review.reads,
            None,
        )?;
        pulled.review = review.changed;
        pulled.skipped.append(&mut review.skipped);
        self.after_pull(&pulled);
        Ok(pulled)
    }

    pub(crate) fn after_pull(&mut self, pulled: &Pulled) {
        // The next local edit starts an undo step of its own.
        if pulled.changed {
            self.split_next_edit = true;
        }
        if pulled.review {
            self.review.split_next_edit = true;
        }
    }

    fn check_replica(&self, replica: &Replica) -> Result<()> {
        if replica.device != self.identity.device {
            return Err(Error::InvalidChange(
                "the replica was opened for another device".into(),
            ));
        }
        Ok(())
    }

    /// The blob names the asset index holds.
    pub(crate) fn indexed_blobs(&self) -> BTreeSet<String> {
        self.assets().into_values().collect()
    }

    /// Writes the indexed blobs the replica lacks and this project has.
    /// Blobs not here yet are tried again on the next push.
    fn push_blobs(&mut self, replica: &mut Replica) -> Result<()> {
        let vv = self.doc.oplog_vv();
        if replica.blobs_pushed.as_ref() == Some(&vv) {
            return Ok(());
        }
        let mut todo: Vec<String> = self
            .indexed_blobs()
            .into_iter()
            .filter(|b| !replica.blobs.contains(b))
            .collect();
        if !todo.is_empty() {
            // Another device may have written them meanwhile.
            let present = replica.storage.list(ASSETS)?;
            replica
                .blobs
                .extend(present.into_iter().filter(|n| is_blob_name(n)));
            todo.retain(|b| !replica.blobs.contains(b));
        }
        let mut complete = true;
        for blob in todo {
            let Some(bytes) = self.storage.read(&blob_path(&blob))? else {
                complete = false;
                continue;
            };
            let sealed = replica
                .sealer
                .seal(Kind::Asset, &blob, replica.level(), &bytes)?;
            replica.storage.create_dir(ASSETS)?;
            replica.storage.write_atomic(&blob_path(&blob), &sealed)?;
            replica.blobs.insert(blob);
        }
        replica.blobs_pushed = complete.then_some(vv);
        Ok(())
    }

    /// Copies the indexed blobs missing here from the replica into the local
    /// `assets/`, counting them in `pulled`. The envelope (its associated
    /// data names the blob) is the integrity check; blobs that fail are
    /// reported once in `pulled` and tried again when their size changes.
    fn pull_blobs(&mut self, replica: &mut Replica, pulled: &mut Pulled) -> Result<()> {
        let vv = self.doc.oplog_vv();
        if replica.blobs_pulled.as_ref() == Some(&vv) {
            return Ok(());
        }
        let mut todo: Vec<String> = self
            .indexed_blobs()
            .into_iter()
            .filter(|b| !replica.local_blobs.contains(b))
            .collect();
        if !todo.is_empty() {
            let present = self.storage.list(ASSETS)?;
            replica
                .local_blobs
                .extend(present.into_iter().filter(|n| is_blob_name(n)));
            todo.retain(|b| !replica.local_blobs.contains(b));
        }
        if !todo.is_empty() {
            let present = replica.storage.list(ASSETS)?;
            replica
                .blobs
                .extend(present.into_iter().filter(|n| is_blob_name(n)));
        }
        let mut complete = true;
        for blob in todo {
            if !replica.blobs.contains(&blob) {
                // Not arrived in the sync folder yet.
                complete = false;
                continue;
            }
            let path = blob_path(&blob);
            let size = replica.storage.size(&path)?;
            if replica.bad_blobs.get(&blob) == Some(&size) {
                complete = false;
                continue;
            }
            let opened = replica.storage.read(&path).and_then(|bytes| match bytes {
                Some(bytes) => replica
                    .sealer
                    .open(Kind::Asset, &blob, &bytes)
                    .map(|(bytes, _)| Some(bytes)),
                None => Ok(None),
            });
            match opened {
                Ok(Some(bytes)) => {
                    self.storage.create_dir(ASSETS)?;
                    self.storage.write_atomic(&path, &bytes)?;
                    replica.local_blobs.insert(blob.clone());
                    replica.bad_blobs.remove(&blob);
                    pulled.blobs += 1;
                }
                Ok(None) => complete = false,
                Err(e) => {
                    complete = false;
                    pulled.skipped.push(format!("blob {blob}: {e}"));
                    if !matches!(e, Error::Io(_)) {
                        replica.bad_blobs.insert(blob, size);
                    }
                }
            }
        }
        replica.blobs_pulled = complete.then_some(vv);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lethora_protocol::envelope::ENVELOPE_3_HEADER;

    #[test]
    fn replica_can_move_between_threads() {
        fn send<T: Send>() {}
        send::<Replica>();
    }

    #[test]
    fn base32_round_trip() {
        let bytes: Vec<u8> = (0..35).map(|i| (i * 37 + 11) as u8).collect();
        let code = to_base32(&bytes);
        assert_eq!(code.len(), CODE_LEN);
        let digits: Vec<u8> = code
            .bytes()
            .map(|c| CROCKFORD.iter().position(|&d| d == c).unwrap() as u8)
            .collect();
        assert_eq!(from_base32(&digits), bytes);
    }

    #[test]
    fn envelope_is_bound_to_its_place() {
        let mut sealer = Sealer::new(&"a".repeat(32), &[7; KEY_LEN]);
        let sealed = sealer.seal(Kind::Log, "dev1", 1, b"hello").unwrap();
        assert_eq!(sealed.len(), ENVELOPE_3_HEADER + 5 + 16);
        assert_eq!(
            sealer.open(Kind::Log, "dev1", &sealed).unwrap(),
            (b"hello".to_vec(), 1)
        );
        assert!(sealer.open(Kind::Log, "dev2", &sealed).is_err());
        assert!(sealer.open(Kind::Snapshot, "dev1", &sealed).is_err());
        let other = Sealer::new(&"b".repeat(32), &[7; KEY_LEN]);
        assert!(other.open(Kind::Log, "dev1", &sealed).is_err());
        let mut newer = sealed.clone();
        newer[0] = 2;
        assert!(matches!(
            sealer.open(Kind::Log, "dev1", &newer),
            Err(Error::UnsupportedFormat(_))
        ));
        let mut epoch = sealed;
        epoch[1] = 2;
        assert!(matches!(
            sealer.open(Kind::Log, "dev1", &epoch),
            Err(Error::UnsupportedFormat(_))
        ));
        // A level above the reader's is a newer version's.
        let level2 = sealer.seal(Kind::Log, "dev1", 2, b"new").unwrap();
        sealer.reader = 2;
        assert_eq!(sealer.open(Kind::Log, "dev1", &level2).unwrap().1, 2);
        sealer.reader = 1;
        assert!(matches!(
            sealer.open(Kind::Log, "dev1", &level2),
            Err(Error::UnsupportedFormat(_))
        ));
    }
}
