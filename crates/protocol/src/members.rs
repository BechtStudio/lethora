//! The members log of a project: a signed, append-only log of who may do
//! what. Only verified here; storing and sending it come with the server.
//!
//! # Format
//!
//! Records in the [`frame`](crate::frame) framing, `[u32 LE len][u32 LE
//! crc32][len bytes: entry]`. An entry is `[u32 LE body len][body: UTF-8
//! JSON][u8 signature count][64 bytes Ed25519 signature, per count]`. Each
//! signature covers `lethora-members-1`, a zero byte and the raw body bytes;
//! bodies are never serialized again to check them.
//!
//! The body: `seq` (0 for the genesis entry, then +1 each), `prev` (lowercase
//! hex SHA-256 of the previous entry's bytes; absent for genesis), `project`
//! (the project id, the same in every entry), `time` (unix seconds, as the
//! writer claims), `by` (the signing device; for `join` the new device) and
//! `op`, an object whose `type` names the operation:
//!
//! - `genesis` (`user`, `device`, `public`): the owner and its first device
//!   with its public keys. Only as entry 0, signed by that device.
//! - `invite` (`id`, `key`, `exchange`, `role` or `user`, `expires`, `uses`
//!   ≥ 1): an invite with its Ed25519 and X25519 public keys (from the link
//!   secret, see [`invite`](crate::invite)). With `role` (editor, reviewer
//!   or reader) it adds a new user and only the owner may create it; with
//!   `user` it adds a device to that user and any current device of that
//!   user may create it.
//! - `revoke` (`id`): ends an invite; by the owner or the invite's creator.
//! - `join` (`invite`, `user`, `device`, `public`, optional `profile`): a new
//!   device through an invite, signed by the invite key and then by the new
//!   device. The invite must exist, not be revoked or used up, its creator
//!   must still be a current device, `time` must not be after `expires`, and
//!   the device must not be a current one. A role invite takes a user who is
//!   not a current member (removed users may come back); a device invite
//!   takes its own user, who must still be one.
//! - `role` (`user`, `role`): the owner changes a member's role (never its
//!   own).
//! - `remove` (`user` or `device`): the owner removes a user with all its
//!   devices (never itself); a device is removed by the owner or by a device
//!   of the same user (itself included), never the owner's last device. A
//!   user whose last device is removed is removed too.
//!   Removed users and devices may come back through a new invite.
//! - `profile` (`profile`): sealed, opaque bytes about the signing device's
//!   user (display name, colour), replacing the previous ones.
//! - `epoch` (`epoch`, `ephemeral`, `devices`, optional `invites`): a new key
//!   epoch, by any current device. `epoch` is the current one plus 1 (epoch 1
//!   is the key the project was shared with, so the first `epoch` entry is
//!   2); `ephemeral` the X25519 public key the new project key is sealed
//!   with; `devices` and `invites` map recipient ids to the sealed key (see
//!   [`keys`](crate::keys)). Every current device must be in `devices` and
//!   no other; `invites` may name invites open at `time` (not revoked, uses
//!   left, not expired, the creator a current device). Written after every
//!   removal ([`Members::needs_rotation`]).
//! - `require` (`level`, `app`): the project's minimum feature level (see
//!   [`LEVEL`](crate::LEVEL)) rises to `level`, which must be above the
//!   current one (1 until the first `require`; never lowered); `app` names
//!   the first app version with that level (1 to [`MAX_REQUIRE_APP`] bytes),
//!   so older apps can say which one to install. By a current device whose
//!   role writes (owner, editor or reviewer): the first one to use a feature
//!   that needs the level.
//!
//! Byte strings (keys, profiles) are lowercase hex. Device and invite
//! exchange keys must not be of low order
//! ([`check_exchange`](crate::keys::check_exchange)). One owner per project,
//! the genesis user; ownership can't be transferred yet.
//!
//! # Newer entries
//!
//! A body this version can't read, an op of an unknown type or with unknown
//! fields, is a newer app's: if its `seq`, `prev` and `project` follow the
//! log and, when `by` is a current device, that device signed it, it is
//! [`Error::Newer`] ("this project needs a newer app"), not corruption; the
//! log stops there for this version. Anything else unreadable stays
//! [`Error::Corrupt`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::frame::{Frame, frame, parse_frame};
use crate::keys::{
    DeviceKeys, DevicePublic, EpochSealer, KEY_LEN, SIGNATURE_LEN, SealedKey, Signer,
    check_exchange, check_public, verify,
};
use crate::{Error, Result, check_id, hex_array, hex_bytes, hex_bytes_opt, to_hex};

/// Domain tag of the entries' signatures.
const SIGNATURE_TAG: &[u8] = b"lethora-members-1\0";
/// Longest `app` of a `require` entry, in bytes.
pub const MAX_REQUIRE_APP: usize = 32;

/// What a member may do (see the docs on collaboration).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Editor,
    Reviewer,
    Reader,
}

impl Role {
    /// Whether this role writes `document`: owners and editors `main` and
    /// `review`, reviewers `review`, readers nothing.
    pub fn writes(self, document: &str) -> bool {
        match self {
            Role::Owner | Role::Editor => matches!(document, "main" | "review"),
            Role::Reviewer => document == "review",
            Role::Reader => false,
        }
    }
}

/// An operation of the log (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum Op {
    Genesis {
        user: String,
        device: String,
        public: DevicePublic,
    },
    Invite {
        id: String,
        #[serde(with = "hex_array")]
        key: [u8; KEY_LEN],
        #[serde(with = "hex_array")]
        exchange: [u8; KEY_LEN],
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<Role>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user: Option<String>,
        expires: u64,
        uses: u32,
    },
    Revoke {
        id: String,
    },
    Join {
        invite: String,
        user: String,
        device: String,
        public: DevicePublic,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "hex_bytes_opt"
        )]
        profile: Option<Vec<u8>>,
    },
    Role {
        user: String,
        role: Role,
    },
    Remove {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device: Option<String>,
    },
    Profile {
        #[serde(with = "hex_bytes")]
        profile: Vec<u8>,
    },
    Epoch {
        epoch: u32,
        #[serde(with = "hex_array")]
        ephemeral: [u8; KEY_LEN],
        devices: BTreeMap<String, SealedKey>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        invites: BTreeMap<String, SealedKey>,
    },
    Require {
        level: u32,
        app: String,
    },
}

/// The body of an entry (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Body {
    pub seq: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prev: String,
    pub project: String,
    pub time: u64,
    pub by: String,
    pub op: Op,
}

/// What this version reads of a newer app's body (see the module docs,
/// "Newer entries").
#[derive(Deserialize)]
struct NewerBody {
    seq: u64,
    #[serde(default)]
    prev: String,
    project: String,
    by: String,
    op: NewerOp,
}

#[derive(Deserialize)]
struct NewerOp {
    #[serde(rename = "type")]
    kind: String,
}

/// A member of the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub role: Role,
    /// The latest `profile` bytes (from `profile` or `join`).
    pub profile: Option<Vec<u8>>,
    pub removed: bool,
}

/// A device of a member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub user: String,
    pub public: DevicePublic,
    pub removed: bool,
}

/// An invite (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    /// The device that created it.
    pub creator: String,
    /// Its Ed25519 public key.
    pub key: [u8; KEY_LEN],
    /// Its X25519 public key.
    pub exchange: [u8; KEY_LEN],
    /// The role of a role invite.
    pub role: Option<Role>,
    /// The user of a device invite.
    pub user: Option<String>,
    pub expires: u64,
    pub uses_left: u32,
    pub revoked: bool,
}

/// The new project key of an `epoch` entry, sealed to its recipients (see
/// the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpochKeys {
    /// The ephemeral X25519 public key.
    pub ephemeral: [u8; KEY_LEN],
    /// By device id.
    pub devices: BTreeMap<String, SealedKey>,
    /// By invite id.
    pub invites: BTreeMap<String, SealedKey>,
}

/// The last entry of the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub seq: u64,
    /// Lowercase hex SHA-256 of the entry's bytes (the next entry's `prev`).
    pub hash: String,
}

/// How reading a whole log ([`Members::from_log`]) ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogEnd {
    /// Every record was valid.
    Complete,
    /// The last record is cut short (a torn tail, e.g. still being synced).
    Incomplete,
    /// This record is invalid; nothing from it on was applied.
    Invalid(Error),
}

/// The result of reading a whole log.
#[derive(Debug, Clone)]
pub struct LogRead {
    /// The state after the valid records; `None` without a valid genesis.
    pub members: Option<Members>,
    /// How many records were valid.
    pub records: usize,
    /// Where the valid records end, in bytes.
    pub bytes: usize,
    pub end: LogEnd,
}

/// The state of a project's members, as its log says (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Members {
    project: String,
    owner: String,
    users: BTreeMap<String, User>,
    devices: BTreeMap<String, Device>,
    invites: BTreeMap<String, Invite>,
    /// The current key epoch.
    epoch: u32,
    /// The `epoch` entries' keys, by epoch.
    epochs: BTreeMap<u32, EpochKeys>,
    /// A member was removed after the latest `epoch` entry.
    needs_rotation: bool,
    /// The minimum feature level and its first app version (`require`).
    required: u32,
    required_app: String,
    head: Head,
}

/// The entry in a single record `record` (nothing after it).
fn unframe(record: &[u8]) -> Result<&[u8]> {
    match parse_frame(record, 0) {
        Frame::Record(entry, end) if end == record.len() => Ok(entry),
        Frame::Record(..) => Err(Error::Corrupt("bytes after the record".into())),
        Frame::Short => Err(Error::Corrupt("the record is cut short".into())),
        Frame::Bad => Err(Error::Corrupt("the record fails its checksum".into())),
    }
}

/// The body, its raw bytes and the signatures of an entry.
type Parsed<'a> = (Body, &'a [u8], &'a [[u8; SIGNATURE_LEN]]);

/// The body, its raw bytes and the signatures of `entry`; [`Error::Newer`]
/// for a body only a newer version reads (unchecked, see the module docs).
fn parse_entry(entry: &[u8]) -> Result<Parsed<'_>> {
    let (raw, signatures) = split_entry(entry)?;
    Ok((parse_body(raw)?, raw, signatures))
}

/// The body in `raw`: [`Error::Newer`] if it is only a newer version's (see
/// the module docs), [`Error::Corrupt`] if it isn't a body.
fn parse_body(raw: &[u8]) -> Result<Body> {
    serde_json::from_slice(raw).map_err(|e| match serde_json::from_slice::<NewerBody>(raw) {
        Ok(newer) => Error::Newer(format!(
            "members entry {} ({}) is of a newer version of Lethora",
            newer.seq, newer.op.kind
        )),
        Err(_) => Error::Corrupt(format!("members entry: {e}")),
    })
}

/// The raw body bytes and the signatures of `entry`.
fn split_entry(entry: &[u8]) -> Result<(&[u8], &[[u8; SIGNATURE_LEN]])> {
    let corrupt = |why: &str| Error::Corrupt(format!("members entry: {why}"));
    let len = entry
        .get(..4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
        .ok_or_else(|| corrupt("cut short"))?;
    let raw = entry
        .get(4..)
        .and_then(|rest| rest.get(..len))
        .ok_or_else(|| corrupt("cut short"))?;
    let rest = &entry[4 + len..];
    let (&count, signatures) = rest.split_first().ok_or_else(|| corrupt("cut short"))?;
    if signatures.len() != usize::from(count) * SIGNATURE_LEN {
        return Err(corrupt("the signatures don't match their count"));
    }
    Ok((raw, signatures.as_chunks::<SIGNATURE_LEN>().0))
}

/// The body of record `record`, unchecked: neither its signatures nor the
/// rules of its op ([`Members::apply`] checks them). E.g. to see what an
/// entry does before applying it. [`Error::Newer`] for a newer version's
/// entry (see the module docs).
pub fn entry_body(record: &[u8]) -> Result<Body> {
    parse_entry(unframe(record)?).map(|(body, ..)| body)
}

/// Checks that `signatures` are exactly those of `keys`, in order, over
/// `raw`.
fn check_signatures(
    raw: &[u8],
    signatures: &[[u8; SIGNATURE_LEN]],
    keys: &[&[u8; KEY_LEN]],
) -> Result<()> {
    if signatures.len() != keys.len() {
        return Err(Error::Signature(format!(
            "the entry needs {} signature(s), it has {}",
            keys.len(),
            signatures.len()
        )));
    }
    for (signature, key) in signatures.iter().zip(keys) {
        verify(key, SIGNATURE_TAG, raw, signature)?;
    }
    Ok(())
}

/// A record holding `body` signed by `signers`, in order.
fn build(body: &Body, signers: &[&dyn Signer]) -> Result<Vec<u8>> {
    let raw = serde_json::to_vec(body).map_err(|e| Error::Invalid(e.to_string()))?;
    let too_large = || Error::Invalid("the members entry is too large".into());
    let len = u32::try_from(raw.len()).map_err(|_| too_large())?;
    let count = u8::try_from(signers.len()).map_err(|_| too_large())?;
    let mut entry = Vec::with_capacity(5 + raw.len() + signers.len() * SIGNATURE_LEN);
    entry.extend_from_slice(&len.to_le_bytes());
    entry.extend_from_slice(&raw);
    entry.push(count);
    for signer in signers {
        entry.extend_from_slice(&signer.sign(SIGNATURE_TAG, &raw));
    }
    frame(&entry).ok_or_else(too_large)
}

fn rejected(why: impl Into<String>) -> Error {
    Error::Rejected(why.into())
}

impl Members {
    /// The genesis record of a new project `project`, owned by `user` with
    /// device `device` (whose keys are `keys`), at `time`.
    pub fn genesis_entry(
        project: &str,
        user: &str,
        device: &str,
        keys: &DeviceKeys,
        time: u64,
    ) -> Result<Vec<u8>> {
        let body = Body {
            seq: 0,
            prev: String::new(),
            project: project.to_owned(),
            time,
            by: device.to_owned(),
            op: Op::Genesis {
                user: user.to_owned(),
                device: device.to_owned(),
                public: keys.public(),
            },
        };
        build(&body, &[keys])
    }

    /// The state after genesis record `record`.
    pub fn genesis(record: &[u8]) -> Result<Self> {
        let entry = unframe(record)?;
        let (body, raw, signatures) = parse_entry(entry)?;
        let Op::Genesis {
            user,
            device,
            public,
        } = body.op
        else {
            return Err(rejected("the first entry must be genesis"));
        };
        if body.seq != 0 || !body.prev.is_empty() {
            return Err(rejected("genesis must be entry 0 without prev"));
        }
        check_id(&body.project, "project")?;
        check_id(&user, "user")?;
        check_id(&device, "device")?;
        if body.by != device {
            return Err(rejected("genesis must be signed by the owner's device"));
        }
        check_signatures(raw, signatures, &[&public.sign])?;
        check_exchange(&public.exchange).map_err(|e| rejected(e.to_string()))?;
        Ok(Self {
            project: body.project,
            owner: user.clone(),
            users: BTreeMap::from([(
                user.clone(),
                User {
                    role: Role::Owner,
                    profile: None,
                    removed: false,
                },
            )]),
            devices: BTreeMap::from([(
                device,
                Device {
                    user,
                    public,
                    removed: false,
                },
            )]),
            invites: BTreeMap::new(),
            epoch: 1,
            epochs: BTreeMap::new(),
            needs_rotation: false,
            required: 1,
            required_app: String::new(),
            head: Head {
                seq: 0,
                hash: to_hex(&Sha256::digest(entry)),
            },
        })
    }

    /// Reads a whole log: the genesis record, then every record that applies,
    /// stopping at the first invalid one (see [`LogRead`]).
    pub fn from_log(log: &[u8]) -> LogRead {
        let mut read = LogRead {
            members: None,
            records: 0,
            bytes: 0,
            end: LogEnd::Complete,
        };
        loop {
            let (record, next) = match parse_frame(log, read.bytes) {
                Frame::Record(_, next) => (&log[read.bytes..next], next),
                Frame::Short if read.bytes == log.len() => {
                    if read.members.is_none() {
                        read.end = LogEnd::Incomplete;
                    }
                    return read;
                }
                Frame::Short => {
                    read.end = LogEnd::Incomplete;
                    return read;
                }
                Frame::Bad => {
                    read.end =
                        LogEnd::Invalid(Error::Corrupt("the record fails its checksum".into()));
                    return read;
                }
            };
            let applied = match &mut read.members {
                Some(members) => members.apply(record),
                None => Self::genesis(record).map(|m| read.members = Some(m)),
            };
            if let Err(e) = applied {
                read.end = LogEnd::Invalid(e);
                return read;
            }
            read.records += 1;
            read.bytes = next;
        }
    }

    /// The next record: `op` by device `by` at `time`, signed by `signers` in
    /// order (for `join` the invite keys, then the new device's keys; else
    /// the keys of `by`). Doesn't check it: [`Members::apply`] does.
    pub fn entry(&self, by: &str, time: u64, op: Op, signers: &[&dyn Signer]) -> Result<Vec<u8>> {
        let body = Body {
            seq: self.head.seq + 1,
            prev: self.head.hash.clone(),
            project: self.project.clone(),
            time,
            by: by.to_owned(),
            op,
        };
        build(&body, signers)
    }

    /// Checks record `record` (framing, chain, project, signatures and the
    /// rules of its op, see the module docs) and applies it; on an error the
    /// state is unchanged. [`Error::Newer`] for a newer version's entry that
    /// follows the log (see the module docs, "Newer entries").
    pub fn apply(&mut self, record: &[u8]) -> Result<()> {
        let entry = unframe(record)?;
        let (raw, signatures) = split_entry(entry)?;
        let body = match parse_body(raw) {
            Ok(body) => body,
            Err(Error::Newer(why)) => {
                let newer: NewerBody = serde_json::from_slice(raw).expect("parsed before");
                self.check_chain(newer.seq, &newer.prev, &newer.project)?;
                check_id(&newer.by, "device")?;
                if let Some(device) = self.current_device(&newer.by)
                    && !signatures
                        .iter()
                        .any(|s| verify(&device.public.sign, SIGNATURE_TAG, raw, s).is_ok())
                {
                    return Err(Error::Signature(
                        "the entry isn't signed by its device".into(),
                    ));
                }
                return Err(Error::Newer(why));
            }
            Err(e) => return Err(e),
        };
        self.check_chain(body.seq, &body.prev, &body.project)?;
        check_id(&body.by, "device")?;
        self.apply_op(body, raw, signatures)?;
        self.head = Head {
            seq: self.head.seq + 1,
            hash: to_hex(&Sha256::digest(entry)),
        };
        Ok(())
    }

    /// Checks that an entry `seq` with `prev` of `project` comes next.
    fn check_chain(&self, seq: u64, prev: &str, project: &str) -> Result<()> {
        if project != self.project {
            return Err(rejected("the entry is of another project"));
        }
        if seq != self.head.seq + 1 {
            return Err(rejected(format!(
                "entry {seq} where {} comes next",
                self.head.seq + 1
            )));
        }
        if prev != self.head.hash {
            return Err(rejected("the entry doesn't follow the previous one"));
        }
        Ok(())
    }

    /// Checks everything about `body`'s op, then applies it.
    fn apply_op(
        &mut self,
        body: Body,
        raw: &[u8],
        signatures: &[[u8; SIGNATURE_LEN]],
    ) -> Result<()> {
        let by = body.by.as_str();
        let time = body.time;
        match body.op {
            Op::Genesis { .. } => Err(rejected("genesis can only be the first entry")),
            Op::Invite {
                id,
                key,
                exchange,
                role,
                user,
                expires,
                uses,
            } => {
                let signer = self.signer(by, raw, signatures)?;
                check_id(&id, "invite")?;
                if self.invites.contains_key(&id) {
                    return Err(rejected(format!("invite {id} exists already")));
                }
                check_public(&key)?;
                check_exchange(&exchange).map_err(|e| rejected(e.to_string()))?;
                if uses == 0 {
                    return Err(rejected("an invite needs at least one use"));
                }
                match (role, &user) {
                    (Some(Role::Owner), None) => {
                        return Err(rejected("an invite can't make an owner"));
                    }
                    (Some(_), None) if signer.user != self.owner => {
                        return Err(rejected("only the owner invites new members"));
                    }
                    (Some(_), None) => {}
                    (None, Some(user)) if *user != signer.user => {
                        return Err(rejected("a device invite is for the own user only"));
                    }
                    (None, Some(_)) => {}
                    _ => return Err(rejected("an invite needs either role or user")),
                }
                self.invites.insert(
                    id,
                    Invite {
                        creator: by.to_owned(),
                        key,
                        exchange,
                        role,
                        user,
                        expires,
                        uses_left: uses,
                        revoked: false,
                    },
                );
                Ok(())
            }
            Op::Revoke { id } => {
                let signer = self.signer(by, raw, signatures)?;
                let invite = self
                    .invites
                    .get(&id)
                    .ok_or_else(|| rejected(format!("no invite {id}")))?;
                if invite.revoked {
                    return Err(rejected(format!("invite {id} is revoked already")));
                }
                if signer.user != self.owner && invite.creator != by {
                    return Err(rejected("only the owner or its creator revokes an invite"));
                }
                self.invites.get_mut(&id).expect("found").revoked = true;
                Ok(())
            }
            Op::Join {
                invite: id,
                user,
                device,
                public,
                profile,
            } => {
                if device != by {
                    return Err(rejected("a join is by the new device"));
                }
                check_id(&user, "user")?;
                let invite = self
                    .invites
                    .get(&id)
                    .ok_or_else(|| rejected(format!("no invite {id}")))?;
                if invite.revoked {
                    return Err(rejected(format!("invite {id} is revoked")));
                }
                if body.time > invite.expires {
                    return Err(rejected(format!("invite {id} has expired")));
                }
                if invite.uses_left == 0 {
                    return Err(rejected(format!("invite {id} is used up")));
                }
                if self.current_device(&invite.creator).is_none() {
                    return Err(rejected(format!("the creator of invite {id} was removed")));
                }
                if self.current_device(&device).is_some() {
                    return Err(rejected(format!("device {device} is a member already")));
                }
                let current = self.users.get(&user).is_some_and(|u| !u.removed);
                match (&invite.role, &invite.user) {
                    (Some(_), _) if current => {
                        return Err(rejected(format!("user {user} is a member already")));
                    }
                    (None, Some(u)) if *u != user || !current => {
                        return Err(rejected(format!("invite {id} is for another user")));
                    }
                    _ => {}
                }
                check_signatures(raw, signatures, &[&invite.key, &public.sign])?;
                check_exchange(&public.exchange).map_err(|e| rejected(e.to_string()))?;
                let invite = self.invites.get_mut(&id).expect("found");
                invite.uses_left -= 1;
                if let Some(role) = invite.role {
                    self.users.insert(
                        user.clone(),
                        User {
                            role,
                            profile,
                            removed: false,
                        },
                    );
                } else if let Some(profile) = profile {
                    self.users.get_mut(&user).expect("current").profile = Some(profile);
                }
                self.devices.insert(
                    device,
                    Device {
                        user,
                        public,
                        removed: false,
                    },
                );
                Ok(())
            }
            Op::Role { user, role } => {
                let signer = self.signer(by, raw, signatures)?;
                if signer.user != self.owner {
                    return Err(rejected("only the owner changes roles"));
                }
                if user == self.owner || role == Role::Owner {
                    return Err(rejected("the owner's role can't change or be given"));
                }
                let member = self
                    .users
                    .get_mut(&user)
                    .filter(|u| !u.removed)
                    .ok_or_else(|| rejected(format!("user {user} is not a member")))?;
                member.role = role;
                Ok(())
            }
            Op::Remove {
                user: Some(user),
                device: None,
            } => {
                let signer = self.signer(by, raw, signatures)?;
                if signer.user != self.owner {
                    return Err(rejected("only the owner removes members"));
                }
                if user == self.owner {
                    return Err(rejected("the owner can't be removed"));
                }
                let member = self
                    .users
                    .get_mut(&user)
                    .filter(|u| !u.removed)
                    .ok_or_else(|| rejected(format!("user {user} is not a member")))?;
                member.removed = true;
                for device in self.devices.values_mut().filter(|d| d.user == user) {
                    device.removed = true;
                }
                self.needs_rotation = true;
                Ok(())
            }
            Op::Remove {
                user: None,
                device: Some(device),
            } => {
                let signer = self.signer(by, raw, signatures)?;
                let target = self
                    .current_device(&device)
                    .ok_or_else(|| rejected(format!("device {device} is not a member")))?;
                if signer.user != self.owner && signer.user != target.user {
                    return Err(rejected("a device is removed by the owner or its own user"));
                }
                if target.user == self.owner && self.device_count(&self.owner) == 1 {
                    return Err(rejected("the owner's last device can't be removed"));
                }
                let user = target.user.clone();
                self.devices.get_mut(&device).expect("found").removed = true;
                // A user without devices is removed too, so a new invite can
                // bring it back.
                if self.device_count(&user) == 0 {
                    self.users.get_mut(&user).expect("current").removed = true;
                }
                self.needs_rotation = true;
                Ok(())
            }
            Op::Remove { .. } => Err(rejected("remove needs either user or device")),
            Op::Profile { profile } => {
                let signer = self.signer(by, raw, signatures)?;
                let user = signer.user.clone();
                self.users.get_mut(&user).expect("current").profile = Some(profile);
                Ok(())
            }
            Op::Epoch {
                epoch,
                ephemeral,
                devices,
                invites,
            } => {
                self.signer(by, raw, signatures)?;
                if epoch != self.epoch + 1 {
                    return Err(rejected(format!(
                        "epoch {epoch} where {} comes next",
                        self.epoch + 1
                    )));
                }
                if let Some(id) = devices.keys().find(|id| self.current_device(id).is_none()) {
                    return Err(rejected(format!("device {id} is not a member")));
                }
                if let Some(id) = self.current_devices().find(|id| !devices.contains_key(*id)) {
                    return Err(rejected(format!("the new key is missing for device {id}")));
                }
                if let Some(id) = invites
                    .keys()
                    .find(|id| !self.invites.get(*id).is_some_and(|i| self.is_open(i, time)))
                {
                    return Err(rejected(format!("invite {id} is not open")));
                }
                self.epoch = epoch;
                self.epochs.insert(
                    epoch,
                    EpochKeys {
                        ephemeral,
                        devices,
                        invites,
                    },
                );
                self.needs_rotation = false;
                Ok(())
            }
            Op::Require { level, app } => {
                self.signer(by, raw, signatures)?;
                if self.role_of_device(by) == Some(Role::Reader) {
                    return Err(rejected("readers don't raise the project's level"));
                }
                if level <= self.required {
                    return Err(rejected(format!(
                        "level {level} is not above the project's level {}",
                        self.required
                    )));
                }
                if app.is_empty() || app.len() > MAX_REQUIRE_APP {
                    return Err(rejected(format!(
                        "the app version is 1 to {MAX_REQUIRE_APP} bytes"
                    )));
                }
                self.required = level;
                self.required_app = app;
                Ok(())
            }
        }
    }

    /// The ids of the current devices.
    fn current_devices(&self) -> impl Iterator<Item = &str> {
        self.devices
            .iter()
            .filter(|(_, d)| !d.removed)
            .map(|(id, _)| id.as_str())
    }

    /// Whether `invite` can be used at `time`: not revoked or used up, not
    /// expired, its creator a current device.
    fn is_open(&self, invite: &Invite, time: u64) -> bool {
        !invite.revoked
            && invite.uses_left > 0
            && time <= invite.expires
            && self.current_device(&invite.creator).is_some()
    }

    /// The `epoch` op starting the next epoch with project key `key`, sealed
    /// to every current device and every invite open at `time` (see the
    /// module docs).
    pub fn epoch_op(&self, key: &[u8; KEY_LEN], time: u64) -> Result<Op> {
        let epoch = self.epoch + 1;
        let sealer = EpochSealer::new()?;
        let seal = |id: &str, exchange: &[u8; KEY_LEN]| {
            sealer
                .seal(&self.project, epoch, id, exchange, key)
                .map(|sealed| (id.to_owned(), sealed))
        };
        let devices = self
            .current_devices()
            .map(|id| seal(id, &self.devices[id].public.exchange))
            .collect::<Result<_>>()?;
        let invites = self
            .invites
            .iter()
            .filter(|(_, i)| self.is_open(i, time))
            .map(|(id, i)| seal(id, &i.exchange))
            .collect::<Result<_>>()?;
        Ok(Op::Epoch {
            epoch,
            ephemeral: sealer.public(),
            devices,
            invites,
        })
    }

    /// Current device `by`, after checking it alone signed `raw`.
    fn signer(&self, by: &str, raw: &[u8], signatures: &[[u8; SIGNATURE_LEN]]) -> Result<&Device> {
        let device = self
            .current_device(by)
            .ok_or_else(|| rejected(format!("device {by} is not a member")))?;
        check_signatures(raw, signatures, &[&device.public.sign])?;
        Ok(device)
    }

    /// Device `id` if it is a current member.
    fn current_device(&self, id: &str) -> Option<&Device> {
        self.devices.get(id).filter(|d| !d.removed)
    }

    /// How many current devices `user` has.
    fn device_count(&self, user: &str) -> usize {
        self.devices
            .values()
            .filter(|d| d.user == user && !d.removed)
            .count()
    }

    /// The project id.
    pub fn project(&self) -> &str {
        &self.project
    }

    /// The owner's user id.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Every user ever added, removed ones included, by id.
    pub fn users(&self) -> &BTreeMap<String, User> {
        &self.users
    }

    /// The devices of `user` (removed ones included), by id.
    pub fn devices_of<'a>(&'a self, user: &'a str) -> impl Iterator<Item = (&'a str, &'a Device)> {
        self.devices
            .iter()
            .filter(move |(_, d)| d.user == user)
            .map(|(id, d)| (id.as_str(), d))
    }

    /// Every invite, by id.
    pub fn invites(&self) -> &BTreeMap<String, Invite> {
        &self.invites
    }

    /// The last entry.
    pub fn head(&self) -> &Head {
        &self.head
    }

    /// The current key epoch (1 until the first `epoch` entry).
    pub fn epoch(&self) -> u32 {
        self.epoch
    }

    /// The keys of every `epoch` entry, by epoch (epoch 1 has none: it is
    /// the key the project was shared with).
    pub fn epochs(&self) -> &BTreeMap<u32, EpochKeys> {
        &self.epochs
    }

    /// Whether a user or device was removed after the latest `epoch` entry
    /// (or genesis): then a current device starts the next epoch.
    pub fn needs_rotation(&self) -> bool {
        self.needs_rotation
    }

    /// The project's minimum feature level (1 until the first `require`).
    pub fn required_level(&self) -> u32 {
        self.required
    }

    /// The first app version with [`Members::required_level`] (empty until
    /// the first `require`).
    pub fn required_app(&self) -> &str {
        &self.required_app
    }

    /// Device `id` (also if removed).
    pub fn device(&self, id: &str) -> Option<&Device> {
        self.devices.get(id)
    }

    /// The role of current device `id`; `None` if it isn't one.
    pub fn role_of_device(&self, id: &str) -> Option<Role> {
        let device = self.current_device(id)?;
        self.users
            .get(&device.user)
            .filter(|u| !u.removed)
            .map(|u| u.role)
    }

    /// Whether device `id` may write `document`: owners and editors `main`
    /// and `review`, reviewers `review`, readers, removed and unknown devices
    /// nothing. The `members` log itself follows its own rules (see the
    /// module docs), not this.
    pub fn may_write(&self, id: &str, document: &str) -> bool {
        self.role_of_device(id).is_some_and(|r| r.writes(document))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::invite::InviteLink;
    use crate::random_id;

    const T: u64 = 1_000;

    struct Dev {
        id: String,
        keys: DeviceKeys,
    }

    impl Dev {
        fn new() -> Self {
            Self {
                id: random_id().unwrap(),
                keys: DeviceKeys::generate().unwrap(),
            }
        }

        /// The next record: `op` by this device.
        fn entry(&self, m: &Members, op: Op) -> Vec<u8> {
            m.entry(&self.id, T, op, &[&self.keys]).unwrap()
        }
    }

    fn invite(link: &InviteLink, role: Option<Role>, user: Option<&str>) -> Op {
        Op::Invite {
            id: link.id.clone(),
            key: link.keys().verifying_key(),
            exchange: link.keys().exchange_key(),
            role,
            user: user.map(str::to_owned),
            expires: T + 100,
            uses: 1,
        }
    }

    fn join_op(link: &InviteLink, user: &str, dev: &Dev) -> Op {
        Op::Join {
            invite: link.id.clone(),
            user: user.to_owned(),
            device: dev.id.clone(),
            public: dev.keys.public(),
            profile: None,
        }
    }

    /// The join record of `dev` for `user` through `link` at `time`.
    fn join(m: &Members, link: &InviteLink, user: &str, dev: &Dev, time: u64) -> Vec<u8> {
        let op = join_op(link, user, dev);
        m.entry(&dev.id, time, op, &[&link.keys(), &dev.keys])
            .unwrap()
    }

    fn new_link() -> InviteLink {
        InviteLink::generate("https://lethora.example", &random_id().unwrap()).unwrap()
    }

    /// A project with its log.
    struct Project {
        m: Members,
        log: Vec<u8>,
        owner: String,
        od: Dev,
    }

    impl Project {
        fn new() -> Self {
            let (owner, od) = (random_id().unwrap(), Dev::new());
            let project = random_id().unwrap();
            let log = Members::genesis_entry(&project, &owner, &od.id, &od.keys, T).unwrap();
            let m = Members::genesis(&log).unwrap();
            Self { m, log, owner, od }
        }

        fn push(&mut self, record: Vec<u8>) {
            self.m.apply(&record).unwrap();
            self.log.extend(record);
        }

        /// Applies `record`, which must fail and change nothing.
        fn reject(&mut self, record: Vec<u8>) -> Error {
            let before = self.m.clone();
            let e = self.m.apply(&record).unwrap_err();
            assert_eq!(self.m, before);
            e
        }

        /// An owner invite for a new member with `role`.
        fn role_invite(&mut self, role: Role) -> InviteLink {
            let link = new_link();
            self.push(self.od.entry(&self.m, invite(&link, Some(role), None)));
            link
        }

        /// A new member with role `role`: its user id and device.
        fn add(&mut self, role: Role) -> (String, Dev) {
            let (user, dev) = (random_id().unwrap(), Dev::new());
            let link = self.role_invite(role);
            self.push(join(&self.m, &link, &user, &dev, T));
            (user, dev)
        }
    }

    /// Whether `device` may write `main`, `review` and `members`.
    fn writes(m: &Members, device: &str) -> (bool, bool, bool) {
        (
            m.may_write(device, "main"),
            m.may_write(device, "review"),
            m.may_write(device, "members"),
        )
    }

    fn rejected(e: Error) -> bool {
        matches!(e, Error::Rejected(_))
    }

    #[test]
    fn full_scenario() {
        let mut p = Project::new();
        let od = p.od.id.clone();
        assert_eq!(p.m.owner(), p.owner);
        assert_eq!(p.m.role_of_device(&od), Some(Role::Owner));
        assert_eq!(writes(&p.m, &od), (true, true, false));
        assert_eq!(p.m.head().seq, 0);
        let genesis = entry_body(&p.log).unwrap();
        assert_eq!((genesis.seq, genesis.by.as_str()), (0, od.as_str()));
        assert!(entry_body(&p.log[1..]).is_err());

        // Role invite, join with a profile.
        let (alice, a1) = (random_id().unwrap(), Dev::new());
        let link = p.role_invite(Role::Editor);
        let mut op = join_op(&link, &alice, &a1);
        if let Op::Join { profile, .. } = &mut op {
            *profile = Some(b"alice".to_vec());
        }
        p.push(p.m.entry(&a1.id, T, op, &[&link.keys(), &a1.keys]).unwrap());
        assert_eq!(p.m.role_of_device(&a1.id), Some(Role::Editor));
        assert_eq!(writes(&p.m, &a1.id), (true, true, false));
        assert_eq!(p.m.users()[&alice].profile.as_deref(), Some(&b"alice"[..]));
        assert_eq!(p.m.invites()[&link.id].uses_left, 0);
        assert_eq!(p.m.invites()[&link.id].creator, od);
        assert_eq!(p.m.device(&a1.id).unwrap().public, a1.keys.public());

        // Role change.
        let op = Op::Role {
            user: alice.clone(),
            role: Role::Reviewer,
        };
        p.push(p.od.entry(&p.m, op));
        assert_eq!(writes(&p.m, &a1.id), (false, true, false));

        // A device invite by a non-owner for its own user, and the join.
        let a2 = Dev::new();
        let link = new_link();
        p.push(a1.entry(&p.m, invite(&link, None, Some(&alice))));
        p.push(join(&p.m, &link, &alice, &a2, T));
        assert_eq!(p.m.role_of_device(&a2.id), Some(Role::Reviewer));
        assert_eq!(p.m.devices_of(&alice).count(), 2);

        // Profile.
        let op = Op::Profile {
            profile: b"alice 2".to_vec(),
        };
        p.push(a2.entry(&p.m, op));
        assert_eq!(
            p.m.users()[&alice].profile.as_deref(),
            Some(&b"alice 2"[..])
        );

        // A device removes its user's other device.
        let op = Op::Remove {
            user: None,
            device: Some(a1.id.clone()),
        };
        p.push(a2.entry(&p.m, op));
        assert!(p.m.device(&a1.id).unwrap().removed);
        assert_eq!(writes(&p.m, &a1.id), (false, false, false));
        assert_eq!(writes(&p.m, &a2.id), (false, true, false));

        // The owner removes the user.
        let op = Op::Remove {
            user: Some(alice.clone()),
            device: None,
        };
        p.push(p.od.entry(&p.m, op));
        assert!(p.m.users()[&alice].removed);
        assert!(p.m.device(&a2.id).unwrap().removed);
        assert_eq!(writes(&p.m, &a2.id), (false, false, false));
        assert_eq!(p.m.role_of_device(&a2.id), None);
        // A removed device can't sign anything.
        let op = Op::Profile {
            profile: b"x".to_vec(),
        };
        assert!(rejected(p.reject(a2.entry(&p.m, op))));

        // Rejoin through a new invite, with a removed device.
        let link = p.role_invite(Role::Reader);
        p.push(join(&p.m, &link, &alice, &a1, T));
        assert!(!p.m.users()[&alice].removed);
        assert_eq!(p.m.users()[&alice].profile, None);
        assert_eq!(p.m.role_of_device(&a1.id), Some(Role::Reader));
        assert_eq!(writes(&p.m, &a1.id), (false, false, false));
        assert_eq!(p.m.role_of_device(&a2.id), None);
        assert_eq!(writes(&p.m, &od), (true, true, false));
        assert_eq!(writes(&p.m, &random_id().unwrap()), (false, false, false));

        // The whole log reads back to the same state.
        let read = Members::from_log(&p.log);
        assert_eq!(read.end, LogEnd::Complete);
        assert_eq!(read.records, 11);
        assert_eq!(read.bytes, p.log.len());
        assert_eq!(read.members.as_ref(), Some(&p.m));
        assert_eq!(p.m.head().seq, 10);
    }

    #[test]
    fn invites_and_joins_are_checked() {
        let mut p = Project::new();
        let (user, dev) = (random_id().unwrap(), Dev::new());

        // Wrong signer, no signature, one too many.
        let link = new_link();
        let op = invite(&link, Some(Role::Editor), None);
        let (other, own) = (DeviceKeys::generate().unwrap(), p.od.keys.clone());
        for signers in [vec![&other], vec![], vec![&own, &own]] {
            let signers: Vec<&dyn Signer> = signers.into_iter().map(|k| k as _).collect();
            let record = p.m.entry(&p.od.id, T, op.clone(), &signers).unwrap();
            assert!(matches!(p.reject(record), Error::Signature(_)));
        }
        // No uses, both or neither of role and user, the owner role.
        for bad in [
            Op::Invite {
                id: link.id.clone(),
                key: link.keys().verifying_key(),
                exchange: link.keys().exchange_key(),
                role: Some(Role::Editor),
                user: None,
                expires: T,
                uses: 0,
            },
            invite(&link, Some(Role::Editor), Some(&p.owner)),
            invite(&link, None, None),
            invite(&link, Some(Role::Owner), None),
        ] {
            assert!(rejected(p.reject(p.od.entry(&p.m, bad))));
        }
        p.push(p.od.entry(&p.m, op.clone()));
        // The same invite id again.
        assert!(rejected(p.reject(p.od.entry(&p.m, op))));

        // Join: only the invite key's signature, the device's alone, or both
        // in the wrong order.
        let op = join_op(&link, &user, &dev);
        let keys = link.keys();
        let orders: [Vec<&dyn Signer>; 3] = [vec![&keys], vec![&dev.keys], vec![&dev.keys, &keys]];
        for signers in orders {
            let record = p.m.entry(&dev.id, T, op.clone(), &signers).unwrap();
            assert!(matches!(p.reject(record), Error::Signature(_)));
        }
        // By another device than the one joining.
        let record =
            p.m.entry(&p.od.id, T, op.clone(), &[&keys, &dev.keys])
                .unwrap();
        assert!(rejected(p.reject(record)));
        // Expired.
        assert!(rejected(p.reject(join(&p.m, &link, &user, &dev, T + 101))));
        // A current device again (the owner's).
        assert!(rejected(p.reject(join(&p.m, &link, &user, &p.od, T))));
        // A current user through a role invite (the owner).
        assert!(rejected(p.reject(join(&p.m, &link, &p.owner, &dev, T))));
        // An unknown invite.
        assert!(rejected(p.reject(join(&p.m, &new_link(), &user, &dev, T))));
        // On time at the last second; then the uses are exhausted.
        p.push(join(&p.m, &link, &user, &dev, T + 100));
        let record = join(&p.m, &link, &random_id().unwrap(), &Dev::new(), T);
        assert!(rejected(p.reject(record)));

        // Revoked: not by a non-owner who didn't create it, then by the
        // owner, not twice; a join then fails.
        let link = p.role_invite(Role::Reader);
        let revoke = Op::Revoke {
            id: link.id.clone(),
        };
        assert!(rejected(p.reject(dev.entry(&p.m, revoke.clone()))));
        p.push(p.od.entry(&p.m, revoke.clone()));
        assert!(p.m.invites()[&link.id].revoked);
        assert!(rejected(p.reject(p.od.entry(&p.m, revoke))));
        let record = join(&p.m, &link, &random_id().unwrap(), &Dev::new(), T);
        assert!(rejected(p.reject(record)));

        // A device invite: another user can't use it; its creator revokes
        // it; nobody invites devices for another user.
        let link = new_link();
        p.push(dev.entry(&p.m, invite(&link, None, Some(&user))));
        let record = join(&p.m, &link, &random_id().unwrap(), &Dev::new(), T);
        assert!(rejected(p.reject(record)));
        let revoke = Op::Revoke {
            id: link.id.clone(),
        };
        p.push(dev.entry(&p.m, revoke));
        let op = invite(&new_link(), None, Some(&p.owner));
        assert!(rejected(p.reject(dev.entry(&p.m, op))));
    }

    #[test]
    fn roles_belong_to_the_user_not_the_device() {
        let mut p = Project::new();
        let (reader, r1) = p.add(Role::Reader);
        // A reader's own device invite carries no role; one with a role is
        // refused, also for its own user.
        for (role, user) in [
            (Some(Role::Editor), None),
            (Some(Role::Editor), Some(&reader)),
        ] {
            assert!(rejected(p.reject(r1.entry(
                &p.m,
                invite(&new_link(), role, user.map(String::as_str))
            ))));
        }
        let r2 = Dev::new();
        let link = new_link();
        p.push(r1.entry(&p.m, invite(&link, None, Some(&reader))));
        p.push(join(&p.m, &link, &reader, &r2, T));
        assert_eq!(p.m.role_of_device(&r2.id), Some(Role::Reader));
        assert_eq!(writes(&p.m, &r2.id), (false, false, false));
        // A role change reaches every device of the user.
        let op = Op::Role {
            user: reader.clone(),
            role: Role::Editor,
        };
        p.push(p.od.entry(&p.m, op));
        for d in [&r1, &r2] {
            assert_eq!(p.m.role_of_device(&d.id), Some(Role::Editor));
        }
    }

    #[test]
    fn invites_die_with_their_creator() {
        let mut p = Project::new();
        let (user, d1) = p.add(Role::Editor);
        let link = new_link();
        p.push(d1.entry(&p.m, invite(&link, None, Some(&user))));
        let op = Op::Remove {
            user: None,
            device: Some(d1.id.clone()),
        };
        p.push(p.od.entry(&p.m, op));
        assert!(rejected(p.reject(join(&p.m, &link, &user, &Dev::new(), T))));
    }

    #[test]
    fn only_the_owner_manages_members() {
        let mut p = Project::new();
        let (editor, e1) = p.add(Role::Editor);
        let (other, o1) = p.add(Role::Reader);
        let owner = p.owner.clone();
        let od = p.od.id.clone();
        let remove_user = |user: &str| Op::Remove {
            user: Some(user.to_owned()),
            device: None,
        };
        let remove_device = |device: &str| Op::Remove {
            user: None,
            device: Some(device.to_owned()),
        };
        let role = |user: &str, role| Op::Role {
            user: user.to_owned(),
            role,
        };

        // Not by an editor: role invites, role changes (its own too),
        // removing users or other users' devices.
        let ops = [
            invite(&new_link(), Some(Role::Reader), None),
            role(&other, Role::Editor),
            role(&editor, Role::Editor),
            remove_user(&other),
            remove_device(&o1.id),
            remove_device(&od),
        ];
        for op in ops {
            assert!(rejected(p.reject(e1.entry(&p.m, op))));
        }

        // The owner: not its own role, no second owner, not itself, not its
        // last device, exactly one target, only members, no second genesis.
        let ops = [
            role(&owner, Role::Editor),
            role(&editor, Role::Owner),
            role(&random_id().unwrap(), Role::Reader),
            remove_user(&owner),
            remove_device(&od),
            Op::Remove {
                user: None,
                device: None,
            },
            Op::Remove {
                user: Some(other.clone()),
                device: Some(o1.id.clone()),
            },
            Op::Genesis {
                user: owner.clone(),
                device: od.clone(),
                public: p.od.keys.public(),
            },
        ];
        for op in ops {
            let e = p.reject(p.od.entry(&p.m, op.clone()));
            assert!(matches!(e, Error::Rejected(_)), "{op:?}: {e}");
        }

        // With a second device the owner may remove another user's device
        // and its own first one, never the last.
        let link = new_link();
        p.push(p.od.entry(&p.m, invite(&link, None, Some(&owner))));
        let od2 = Dev::new();
        p.push(join(&p.m, &link, &owner, &od2, T));
        p.push(od2.entry(&p.m, remove_device(&o1.id)));
        p.push(od2.entry(&p.m, remove_device(&od)));
        assert_eq!(writes(&p.m, &od), (false, false, false));
        assert_eq!(writes(&p.m, &od2.id), (true, true, false));
        assert!(rejected(p.reject(od2.entry(&p.m, remove_device(&od2.id)))));
        // An editor may remove its own (last) device; that removes the user,
        // and a new role invite brings it back.
        p.push(e1.entry(&p.m, remove_device(&e1.id)));
        assert_eq!(p.m.role_of_device(&e1.id), None);
        assert!(p.m.users()[&editor].removed);
        let link = new_link();
        p.push(od2.entry(&p.m, invite(&link, Some(Role::Editor), None)));
        let e2 = Dev::new();
        p.push(join(&p.m, &link, &editor, &e2, T));
        assert_eq!(writes(&p.m, &e2.id), (true, true, false));
    }

    #[test]
    fn the_chain_is_checked() {
        let mut p = Project::new();
        let project = p.m.project().to_owned();
        let hash = p.m.head().hash.clone();
        let by = p.od.id.clone();
        let body = |seq, prev: &str, project: &str| Body {
            seq,
            prev: prev.to_owned(),
            project: project.to_owned(),
            time: T,
            by: by.clone(),
            op: Op::Profile {
                profile: vec![1, 2],
            },
        };
        let bad = [
            body(1, &"0".repeat(64), &project),
            body(1, "", &project),
            body(2, &hash, &project),
            body(0, &hash, &project),
            body(1, &hash, &random_id().unwrap()),
        ];
        for body in bad {
            let record = build(&body, &[&p.od.keys]).unwrap();
            assert!(rejected(p.reject(record)), "{body:?}");
        }
        let good = build(&body(1, &hash, &project), &[&p.od.keys]).unwrap();

        // Damaged framing, a cut record, trailing bytes.
        let mut crc = good.clone();
        crc[10] ^= 1;
        assert!(matches!(p.reject(crc), Error::Corrupt(_)));
        let cut = good[..good.len() - 1].to_vec();
        assert!(matches!(p.reject(cut), Error::Corrupt(_)));
        let longer = [&good[..], &[0]].concat();
        assert!(matches!(p.reject(longer), Error::Corrupt(_)));

        // Body bytes that changed but still parse: the signature fails.
        let mut entry = good[8..].to_vec();
        let at = entry
            .windows(11)
            .position(|w| w == b"\"time\":1000")
            .unwrap();
        entry[at + 10] = b'1';
        let record = frame(&entry).unwrap();
        assert!(matches!(p.reject(record), Error::Signature(_)));

        // Unknown fields, in the body or the op, and unknown ops are a newer
        // app's when they follow the log and are signed by their device;
        // otherwise what is wrong with them.
        let json =
            String::from_utf8(serde_json::to_vec(&body(1, &hash, &project)).unwrap()).unwrap();
        let signed = |raw: &str, keys: &DeviceKeys| {
            let raw = raw.as_bytes();
            let mut entry = (raw.len() as u32).to_le_bytes().to_vec();
            entry.extend_from_slice(raw);
            entry.push(1);
            entry.extend_from_slice(&keys.sign(SIGNATURE_TAG, raw));
            frame(&entry).unwrap()
        };
        let unknown_op = format!(
            "{}\"op\":{{\"type\":\"future\",\"x\":[1]}}}}",
            &json[..json.find("\"op\"").unwrap()]
        );
        for raw in [
            json.replace("\"time\"", "\"extra\":1,\"time\""),
            json.replace("\"type\"", "\"extra\":1,\"type\""),
            unknown_op.clone(),
        ] {
            let record = signed(&raw, &p.od.keys);
            assert!(matches!(entry_body(&record), Err(Error::Newer(_))));
            let e = p.reject(record);
            assert!(matches!(e, Error::Newer(_)), "{raw}: {e}");
            // By another key than its device's: forged.
            let other = DeviceKeys::generate().unwrap();
            assert!(matches!(
                p.reject(signed(&raw, &other)),
                Error::Signature(_)
            ));
        }
        // Not following the log, or not a body at all.
        let stale = unknown_op.replace(&hash, &"0".repeat(64));
        assert!(rejected(p.reject(signed(&stale, &p.od.keys))));
        let not_a_body = signed("{\"seq\":1}", &p.od.keys);
        assert!(matches!(p.reject(not_a_body), Error::Corrupt(_)));
        // A whole log stops at a newer entry.
        let read = Members::from_log(&[&p.log[..], &signed(&unknown_op, &p.od.keys)].concat());
        assert!(matches!(read.end, LogEnd::Invalid(Error::Newer(_))));
        assert_eq!(read.records, 1);

        p.push(good.clone());
        assert_eq!(p.m.head().seq, 1);
        // The same record twice.
        assert!(rejected(p.reject(good)));
    }

    #[test]
    fn reading_a_log_stops_at_the_first_invalid_record() {
        let mut p = Project::new();
        p.add(Role::Editor);
        let valid = p.log.clone();
        let records = 3;
        let m = p.m.clone();
        let next = p.od.entry(&p.m, Op::Profile { profile: vec![7] });

        // A torn tail: incomplete.
        let torn = [&valid[..], &next[..next.len() - 3]].concat();
        let read = Members::from_log(&torn);
        assert_eq!(read.end, LogEnd::Incomplete);
        assert_eq!((read.records, read.bytes), (records, valid.len()));
        assert_eq!(read.members.as_ref(), Some(&m));

        // An invalid record, then a valid one after it: neither applies.
        let wrong = Dev::new().entry(&p.m, Op::Profile { profile: vec![7] });
        let read = Members::from_log(&[&valid[..], &wrong, &next].concat());
        assert!(matches!(read.end, LogEnd::Invalid(Error::Rejected(_))));
        assert_eq!((read.records, read.bytes), (records, valid.len()));
        assert_eq!(read.members.as_ref(), Some(&m));

        // A record failing its checksum.
        let mut bad = [&valid[..], &next].concat();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        let read = Members::from_log(&bad);
        assert!(matches!(read.end, LogEnd::Invalid(Error::Corrupt(_))));
        assert_eq!(read.records, records);

        // Nothing, a torn genesis, a log not starting with genesis.
        let read = Members::from_log(&[]);
        assert_eq!((read.end, read.records), (LogEnd::Incomplete, 0));
        assert!(read.members.is_none());
        let read = Members::from_log(&valid[..10]);
        assert_eq!(read.end, LogEnd::Incomplete);
        assert!(read.members.is_none());
        let read = Members::from_log(&next);
        assert!(matches!(read.end, LogEnd::Invalid(Error::Rejected(_))));
        assert!(read.members.is_none());

        // The complete log with the next record.
        let read = Members::from_log(&[&valid[..], &next].concat());
        assert_eq!((read.end, read.records), (LogEnd::Complete, records + 1));
    }

    #[test]
    fn genesis_is_checked() {
        let (project, user, dev) = (random_id().unwrap(), random_id().unwrap(), Dev::new());
        let good = Members::genesis_entry(&project, &user, &dev.id, &dev.keys, T).unwrap();
        let m = Members::genesis(&good).unwrap();
        assert_eq!((m.project(), m.owner()), (project.as_str(), user.as_str()));
        assert_eq!(m.users()[&user].role, Role::Owner);
        let other = DeviceKeys::generate().unwrap();
        let body = |seq, by: &str, project: &str, op: Op| Body {
            seq,
            prev: String::new(),
            project: project.to_owned(),
            time: T,
            by: by.to_owned(),
            op,
        };
        let genesis = Op::Genesis {
            user: user.clone(),
            device: dev.id.clone(),
            public: dev.keys.public(),
        };
        let revoke = Op::Revoke { id: user.clone() };
        let bad = [
            build(&body(0, &dev.id, &project, genesis.clone()), &[&other]),
            build(&body(0, &dev.id, &project, genesis.clone()), &[]),
            build(&body(1, &dev.id, &project, genesis.clone()), &[&dev.keys]),
            build(&body(0, &user, &project, genesis.clone()), &[&dev.keys]),
            build(&body(0, &dev.id, "nope", genesis), &[&dev.keys]),
            build(&body(0, &dev.id, &project, revoke), &[&dev.keys]),
        ];
        for record in bad {
            assert!(Members::genesis(&record.unwrap()).is_err());
        }
    }

    #[test]
    fn epochs_follow_removals_and_reach_every_current_device() {
        let mut p = Project::new();
        let (user, e1) = p.add(Role::Editor);
        let (_, r1) = p.add(Role::Reader);
        assert_eq!((p.m.epoch(), p.m.needs_rotation()), (1, false));
        let link = p.role_invite(Role::Reader);
        let remove = |device: &Dev| Op::Remove {
            user: None,
            device: Some(device.id.clone()),
        };
        p.push(e1.entry(&p.m, remove(&e1)));
        assert!(p.m.needs_rotation());
        assert!(p.m.users()[&user].removed);

        let key = [7; KEY_LEN];
        let good = p.m.epoch_op(&key, T).unwrap();
        let Op::Epoch {
            epoch,
            devices,
            invites,
            ..
        } = &good
        else {
            panic!("{good:?}");
        };
        assert_eq!(*epoch, 2);
        assert_eq!(devices.len(), 2);
        assert!(devices.contains_key(&p.od.id) && devices.contains_key(&r1.id));
        assert_eq!(invites.keys().collect::<Vec<_>>(), [&link.id]);
        // Expired invites are left out.
        let Op::Epoch { invites, .. } = p.m.epoch_op(&key, T + 101).unwrap() else {
            unreachable!()
        };
        assert!(invites.is_empty());

        // Refused: by the removed device, a wrong number, a current device
        // missing, a removed or unknown device, an unknown, expired or
        // revoked invite.
        assert!(rejected(p.reject(e1.entry(&p.m, good.clone()))));
        type Keys = BTreeMap<String, SealedKey>;
        let changed = |f: &dyn Fn(&mut u32, &mut Keys, &mut Keys)| {
            let mut op = good.clone();
            if let Op::Epoch {
                epoch,
                devices,
                invites,
                ..
            } = &mut op
            {
                f(epoch, devices, invites);
            }
            op
        };
        let sealed = devices[&p.od.id];
        let stranger = random_id().unwrap();
        let bad = [
            changed(&|e, _, _| *e = 3),
            changed(&|e, _, _| *e = 1),
            changed(&|_, d, _| {
                d.remove(&r1.id);
            }),
            changed(&|_, d, _| {
                d.insert(e1.id.clone(), sealed);
            }),
            changed(&|_, d, _| {
                d.insert(stranger.clone(), sealed);
            }),
            changed(&|_, _, i| {
                i.insert(stranger.clone(), sealed);
            }),
        ];
        for op in bad {
            let e = p.reject(p.od.entry(&p.m, op.clone()));
            assert!(matches!(e, Error::Rejected(_)), "{op:?}: {e}");
        }
        let late = p.m.entry(&p.od.id, T + 101, good.clone(), &[&p.od.keys]);
        assert!(rejected(p.reject(late.unwrap())));

        // Any current device may write it, a reader too; each recipient
        // opens the new key.
        p.push(r1.entry(&p.m, good.clone()));
        assert_eq!((p.m.epoch(), p.m.needs_rotation()), (2, false));
        let keys = &p.m.epochs()[&2];
        for dev in [&p.od, &r1] {
            let opened = dev.keys.open_epoch_key(
                p.m.project(),
                2,
                &dev.id,
                &keys.ephemeral,
                &keys.devices[&dev.id],
            );
            assert_eq!(opened.unwrap(), key);
        }
        let opened =
            link.keys()
                .open_epoch_key(p.m.project(), 2, &keys.ephemeral, &keys.invites[&link.id]);
        assert_eq!(opened.unwrap(), key);
        // The same epoch twice; no removal is needed for the next one.
        assert!(rejected(p.reject(p.od.entry(&p.m, good))));
        let next = p.m.epoch_op(&[8; KEY_LEN], T).unwrap();
        p.push(p.od.entry(&p.m, next));
        assert_eq!(p.m.epoch(), 3);

        // A revoked invite is not open.
        p.push(p.od.entry(
            &p.m,
            Op::Revoke {
                id: link.id.clone(),
            },
        ));
        let Op::Epoch { invites, .. } = p.m.epoch_op(&key, T).unwrap() else {
            unreachable!()
        };
        assert!(invites.is_empty());
        let mut op = p.m.epoch_op(&key, T).unwrap();
        if let Op::Epoch { invites, .. } = &mut op {
            invites.insert(link.id.clone(), sealed);
        }
        assert!(rejected(p.reject(p.od.entry(&p.m, op))));

        // Removing a user needs a rotation too; the log reads back.
        let (user, _) = p.add(Role::Editor);
        let op = Op::Remove {
            user: Some(user),
            device: None,
        };
        p.push(p.od.entry(&p.m, op));
        assert!(p.m.needs_rotation());
        let read = Members::from_log(&p.log);
        assert_eq!(read.end, LogEnd::Complete);
        assert_eq!(read.members.as_ref(), Some(&p.m));
    }

    #[test]
    fn the_required_level_only_rises() {
        let mut p = Project::new();
        let (_, reader) = p.add(Role::Reader);
        let (_, reviewer) = p.add(Role::Reviewer);
        assert_eq!((p.m.required_level(), p.m.required_app()), (1, ""));
        let require = |level, app: &str| Op::Require {
            level,
            app: app.to_owned(),
        };
        // Not by a reader, not to the same level, not without an app or
        // with a long one, not by a stranger.
        assert!(rejected(p.reject(reader.entry(&p.m, require(2, "0.2.0")))));
        for op in [
            require(1, "0.2.0"),
            require(0, "0.2.0"),
            require(2, ""),
            require(2, &"9".repeat(MAX_REQUIRE_APP + 1)),
        ] {
            assert!(rejected(p.reject(p.od.entry(&p.m, op))));
        }
        assert!(rejected(
            p.reject(Dev::new().entry(&p.m, require(2, "0.2.0")))
        ));
        // A reviewer may, then never lower; levels may skip.
        p.push(reviewer.entry(&p.m, require(2, "0.2.0")));
        assert_eq!((p.m.required_level(), p.m.required_app()), (2, "0.2.0"));
        assert!(rejected(p.reject(p.od.entry(&p.m, require(2, "0.2.1")))));
        p.push(p.od.entry(&p.m, require(4, "1.0.0")));
        assert_eq!((p.m.required_level(), p.m.required_app()), (4, "1.0.0"));
        let read = Members::from_log(&p.log);
        assert_eq!(read.end, LogEnd::Complete);
        assert_eq!(read.members.as_ref(), Some(&p.m));
    }

    #[test]
    fn low_order_exchange_keys_are_refused() {
        let mut p = Project::new();
        let link = new_link();
        let mut op = invite(&link, Some(Role::Editor), None);
        if let Op::Invite { exchange, .. } = &mut op {
            *exchange = [0; KEY_LEN];
        }
        assert!(rejected(p.reject(p.od.entry(&p.m, op))));
        p.push(p.od.entry(&p.m, invite(&link, Some(Role::Editor), None)));
        let dev = Dev::new();
        let mut op = join_op(&link, &random_id().unwrap(), &dev);
        if let Op::Join { public, .. } = &mut op {
            public.exchange = [0; KEY_LEN];
        }
        let record = p.m.entry(&dev.id, T, op, &[&link.keys(), &dev.keys]);
        assert!(rejected(p.reject(record.unwrap())));
    }

    #[test]
    fn json_shape() {
        let p = Project::new();
        let op = Op::Remove {
            user: None,
            device: Some(p.od.id.clone()),
        };
        let record = p.od.entry(&p.m, op);
        let (body, raw, signatures) = parse_entry(&record[8..]).unwrap();
        assert_eq!((body.seq, signatures.len()), (1, 1));
        assert_eq!(
            std::str::from_utf8(raw).unwrap(),
            format!(
                "{{\"seq\":1,\"prev\":\"{}\",\"project\":\"{}\",\"time\":1000,\"by\":\"{}\",\
                 \"op\":{{\"type\":\"remove\",\"device\":\"{}\"}}}}",
                p.m.head().hash,
                p.m.project(),
                p.od.id,
                p.od.id
            )
        );
    }
}
