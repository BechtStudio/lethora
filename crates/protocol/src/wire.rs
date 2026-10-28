//! The sync server's WebSocket messages: one message per binary WebSocket
//! frame, in our own small binary encoding.
//!
//! # Encoding
//!
//! `[u8 protocol version = 2][u8 message type][fields...]`. Integers are
//! little-endian; byte strings and strings are `[u32 LE length][bytes]`
//! (strings UTF-8); ids (project, device, invite) are their 16 raw bytes; a
//! list is `[u32 LE count][items...]`. Decoding never panics: it rejects a
//! protocol version outside [`MIN_VERSION`]`..=`[`VERSION`] and unknown
//! message types ([`Error::Unsupported`]), and input cut short, trailing
//! bytes, unknown codes or roles and invalid UTF-8 ([`Error::Corrupt`]).
//!
//! # Frozen messages
//!
//! [`Message::Challenge`] (type 1: nonce, server time) and
//! [`Message::Refused`] (type 6: code, message) decode whatever the version
//! byte says, so every version can read the other side's challenge and its
//! "update the app" refusal ([`Code::Version`], code 1). Never change these
//! two: not their type bytes, not their fields, not code 1.
//!
//! # Versions
//!
//! The protocol version changes only with the encoding; a server serves a
//! range and refuses a hello outside it with [`Code::Version`]. The hello
//! also carries the client's [`ClientVersion`]: its feature level
//! ([`LEVEL`](crate::LEVEL)) and app version. The server refuses a level
//! below the project's minimum (the members log's `require`, see
//! [`members`](crate::members)) with [`Code::Version`], and remembers each
//! device's last level and app version, listed in the welcome so a client
//! can warn before raising the minimum.
//!
//! # Flow
//!
//! The server opens with [`Message::Challenge`]; the client answers with a
//! [`Message::Hello`], signed over [`Hello::signed_bytes`]: the tag
//! `lethora-hello-1` and a zero byte, the challenge nonce, the protocol
//! version byte, the server's public URL (as a string), the hello's own
//! type byte and fields, then the client's level (`u32`) and app version (a
//! string), which the hello message carries after its fields, before the
//! signature. A device signs with its device key, an invitee with
//! the invite key from the link, a new project's creator with the genesis
//! device's key. The server answers [`Message::Welcome`] or
//! [`Message::Refused`] (and closes).
//!
//! After the welcome, [`Message::Subscribe`] says what the client has; the
//! server sends what it lacks ([`Message::MembersRecords`],
//! [`Message::Records`], [`Message::Compacted`] and
//! [`Message::SnapshotChanged`]), then [`Message::Synced`] once, then the
//! same kinds live. [`Message::Append`] adds records to the device's own
//! stream of a document; [`Message::AppendMembers`] adds one members log
//! record. Batch ids are chosen by the client: per stream they start at 1
//! and must grow, and a batch id not above the stream's last one (which the
//! welcome tells) is taken as a resend: acked with the stream's current
//! length, not stored again. The sender of records gets no copy of them.
//!
//! # Streams
//!
//! Per document (`main`, `review`) each device has a stream: its log
//! (records in the [`frame`](crate::frame) framing, each an envelope v2 or
//! v4, see [`envelope`](crate::envelope)) and
//! optionally a snapshot. Offsets count bytes in the log of the stream's
//! current generation. Compaction starts a new generation: the snapshot then
//! covers the first `covers` bytes of the previous generation's log, which
//! the new log no longer holds. A client at offset `o ≥ covers` in the
//! previous generation continues at `o - covers` ([`Message::Compacted`]);
//! any other client fetches the snapshot over HTTP and reads the log from 0
//! ([`Message::SnapshotChanged`]). Generation 0 has no snapshot.
//!
//! # Project deletion
//!
//! The owner's device deletes a project over HTTP with a body
//! [`deletion_body`]: `[u64 LE unix time][64 bytes signature]`, the signature
//! over the tag `lethora-delete-1`, a zero byte, the project id's 16 bytes
//! and the time.

use crate::keys::{KEY_LEN, SIGNATURE_LEN, Signer, verify};
use crate::members::Role;
use crate::{Error, ID_LEN, Result, id_bytes, to_hex};

/// The protocol version this crate speaks (and writes).
pub const VERSION: u8 = 2;
/// The oldest protocol version this crate decodes (see the module docs).
pub const MIN_VERSION: u8 = 2;
/// Longest app version a hello or welcome carries, in bytes.
pub const MAX_APP_LEN: usize = 64;
/// Bytes of a challenge nonce.
pub const NONCE_LEN: usize = 32;
/// Domain tag of hello signatures.
const HELLO_TAG: &[u8] = b"lethora-hello-1\0";
/// Domain tag of project deletion signatures.
const DELETE_TAG: &[u8] = b"lethora-delete-1\0";
/// Bytes of a project deletion body.
pub const DELETION_LEN: usize = 8 + SIGNATURE_LEN;

/// Why the server refused something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// Another protocol version, or a level below the project's minimum:
    /// one side needs an update (code 1, frozen: see the module docs).
    Version,
    /// A signature or token that doesn't check out.
    Auth,
    UnknownProject,
    NotAMember,
    /// The invite is unknown, revoked, used up or expired.
    InviteClosed,
    /// The project exists already.
    Exists,
    RateLimited,
    TooLarge,
    /// The project's storage quota is used up.
    Quota,
    BadRequest,
    Internal,
    /// The device's role doesn't allow it.
    Forbidden,
    /// A members record that doesn't follow the current head.
    Stale,
}

impl Code {
    fn to_byte(self) -> u8 {
        match self {
            Code::Version => 1,
            Code::Auth => 2,
            Code::UnknownProject => 3,
            Code::NotAMember => 4,
            Code::InviteClosed => 5,
            Code::Exists => 6,
            Code::RateLimited => 7,
            Code::TooLarge => 8,
            Code::Quota => 9,
            Code::BadRequest => 10,
            Code::Internal => 11,
            Code::Forbidden => 12,
            Code::Stale => 13,
        }
    }

    fn from_byte(b: u8) -> Result<Self> {
        Ok(match b {
            1 => Code::Version,
            2 => Code::Auth,
            3 => Code::UnknownProject,
            4 => Code::NotAMember,
            5 => Code::InviteClosed,
            6 => Code::Exists,
            7 => Code::RateLimited,
            8 => Code::TooLarge,
            9 => Code::Quota,
            10 => Code::BadRequest,
            11 => Code::Internal,
            12 => Code::Forbidden,
            13 => Code::Stale,
            _ => return Err(corrupt(format!("unknown code {b}"))),
        })
    }
}

/// How a client introduces itself (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hello {
    /// A current device of `project`.
    Device { project: String, device: String },
    /// Someone with an invite link, to join `project`.
    Invitee { project: String, invite: String },
    /// A new project from its genesis members record.
    Create { genesis: Vec<u8> },
}

impl Hello {
    /// What the hello's signature covers, after the tag: the challenge
    /// `nonce`, the protocol version, the server's public URL `server`, the
    /// hello itself and the client's level and app version `client`.
    pub fn signed_bytes(
        &self,
        nonce: &[u8; NONCE_LEN],
        server: &str,
        client: &ClientVersion,
    ) -> Result<Vec<u8>> {
        let mut w = Writer::new();
        w.bytes_raw(nonce);
        w.u8(VERSION);
        w.str(server)?;
        self.write(&mut w)?;
        client.write(&mut w)?;
        Ok(w.0)
    }

    /// The hello message of a client at `client`, signed by `signer` for
    /// challenge `nonce` of server `server`.
    pub fn sign(
        self,
        client: ClientVersion,
        signer: &dyn Signer,
        nonce: &[u8; NONCE_LEN],
        server: &str,
    ) -> Result<Message> {
        let signed = self.signed_bytes(nonce, server, &client)?;
        Ok(Message::Hello {
            hello: self,
            client,
            signature: signer.sign(HELLO_TAG, &signed),
        })
    }

    /// Checks that `signature` is `public`'s over this hello of a client at
    /// `client` for challenge `nonce` of server `server`;
    /// [`Error::Signature`] if not.
    pub fn verify(
        &self,
        client: &ClientVersion,
        public: &[u8; KEY_LEN],
        nonce: &[u8; NONCE_LEN],
        server: &str,
        signature: &[u8; SIGNATURE_LEN],
    ) -> Result<()> {
        verify(
            public,
            HELLO_TAG,
            &self.signed_bytes(nonce, server, client)?,
            signature,
        )
    }

    fn write(&self, w: &mut Writer) -> Result<()> {
        match self {
            Hello::Device { project, device } => {
                w.u8(T_HELLO_DEVICE);
                w.id(project, "project")?;
                w.id(device, "device")?;
            }
            Hello::Invitee { project, invite } => {
                w.u8(T_HELLO_INVITEE);
                w.id(project, "project")?;
                w.id(invite, "invite")?;
            }
            Hello::Create { genesis } => {
                w.u8(T_CREATE_PROJECT);
                w.bytes(genesis)?;
            }
        }
        Ok(())
    }
}

/// A client's feature level and app version (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientVersion {
    pub level: u32,
    /// The app version, e.g. `0.1.0 (1a2b3c4)`; at most [`MAX_APP_LEN`]
    /// bytes.
    pub app: String,
}

impl ClientVersion {
    fn write(&self, w: &mut Writer) -> Result<()> {
        if self.app.len() > MAX_APP_LEN {
            return Err(Error::Invalid(format!(
                "an app version is at most {MAX_APP_LEN} bytes"
            )));
        }
        w.u32(self.level);
        w.str(&self.app)
    }

    fn read(r: &mut Reader) -> Result<Self> {
        Ok(Self {
            level: r.u32()?,
            app: r.str()?,
        })
    }
}

/// A device's last hello, as the server remembers it (see
/// [`Message::Welcome`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceVersion {
    pub device: String,
    pub version: ClientVersion,
}

/// A device's own stream of a document, as the server has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnStream {
    pub document: String,
    pub generation: u64,
    /// The log's length in bytes.
    pub length: u64,
    /// The last batch id stored.
    pub batch: u64,
}

/// What a client has of a stream (see [`Message::Subscribe`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Position {
    pub document: String,
    pub device: String,
    pub generation: u64,
    /// Bytes of the generation's log read.
    pub offset: u64,
}

/// A WebSocket message (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// Server → client, first: a fresh `nonce` to sign and the server's
    /// clock (unix seconds). Frozen (see the module docs).
    Challenge {
        nonce: [u8; NONCE_LEN],
        server_time: u64,
    },
    /// Client → server: who the client is and its level and app version,
    /// signed (see [`Hello`]).
    Hello {
        hello: Hello,
        client: ClientVersion,
        signature: [u8; SIGNATURE_LEN],
    },
    /// Server → client: the hello is accepted. `token` authorizes HTTP
    /// requests (bearer, valid while this connection is open); `role` is the
    /// device's, `None` for an invitee; `streams` are the device's own
    /// streams; `app` is the server's version; `devices` the last hello of
    /// each current device of the project the server has seen (none for an
    /// invitee).
    Welcome {
        token: String,
        role: Option<Role>,
        streams: Vec<OwnStream>,
        app: String,
        devices: Vec<DeviceVersion>,
    },
    /// Server → client: refused; the server closes the connection. Frozen
    /// (see the module docs).
    Refused {
        code: Code,
        message: String,
    },
    /// Client → server: send what I lack. `members` is how many bytes of the
    /// members log the client has; `streams` what it has of each stream
    /// (streams not listed: nothing). An invitee lists no streams.
    Subscribe {
        members: u64,
        streams: Vec<Position>,
    },
    /// Server → client: members log bytes from `offset` (whole records).
    MembersRecords {
        offset: u64,
        bytes: Vec<u8>,
    },
    /// Server → client: log bytes of a stream from `offset` (whole records).
    Records {
        document: String,
        device: String,
        generation: u64,
        offset: u64,
        bytes: Vec<u8>,
    },
    /// Server → client: the stream has a snapshot the client lacks; fetch it
    /// over HTTP, then the log follows from offset 0 of `generation`.
    SnapshotChanged {
        document: String,
        device: String,
        generation: u64,
        covers: u64,
    },
    /// Server → client: catch-up is sent; live messages follow.
    Synced,
    /// Client → server: framed records for the device's own stream of
    /// `document`.
    Append {
        document: String,
        batch: u64,
        records: Vec<u8>,
    },
    /// Server → client: batch `batch` is stored; the stream is now `length`
    /// bytes long in `generation`.
    Ack {
        batch: u64,
        document: String,
        generation: u64,
        length: u64,
    },
    /// Server → client: batch `batch` was refused (nothing stored).
    Nack {
        batch: u64,
        code: Code,
        message: String,
    },
    /// Client → server: one members log record (framed).
    AppendMembers {
        batch: u64,
        record: Vec<u8>,
    },
    /// Server → client: the members record of `batch` is entry `seq`.
    MembersAck {
        batch: u64,
        seq: u64,
    },
    /// Server → client: the stream was compacted: the client continues at
    /// its offset minus `covers` in `generation`.
    Compacted {
        document: String,
        device: String,
        generation: u64,
        covers: u64,
    },
    /// Client → server: presence bytes for the project's other connections.
    Presence {
        bytes: Vec<u8>,
    },
    /// Server → client: presence bytes from `device`.
    PeerPresence {
        device: String,
        bytes: Vec<u8>,
    },
    /// Either way: answered with a pong of the same value.
    Ping(u64),
    Pong(u64),
}

const T_CHALLENGE: u8 = 1;
const T_HELLO_DEVICE: u8 = 2;
const T_HELLO_INVITEE: u8 = 3;
const T_CREATE_PROJECT: u8 = 4;
const T_WELCOME: u8 = 5;
const T_REFUSED: u8 = 6;
const T_SUBSCRIBE: u8 = 7;
const T_MEMBERS_RECORDS: u8 = 8;
const T_RECORDS: u8 = 9;
const T_SNAPSHOT_CHANGED: u8 = 10;
const T_SYNCED: u8 = 11;
const T_APPEND: u8 = 12;
const T_ACK: u8 = 13;
const T_NACK: u8 = 14;
const T_APPEND_MEMBERS: u8 = 15;
const T_MEMBERS_ACK: u8 = 16;
const T_COMPACTED: u8 = 17;
const T_PRESENCE: u8 = 18;
const T_PEER_PRESENCE: u8 = 19;
const T_PING: u8 = 20;
const T_PONG: u8 = 21;

fn role_byte(role: Option<Role>) -> u8 {
    match role {
        None => 0,
        Some(Role::Owner) => 1,
        Some(Role::Editor) => 2,
        Some(Role::Reviewer) => 3,
        Some(Role::Reader) => 4,
    }
}

fn role_from_byte(b: u8) -> Result<Option<Role>> {
    Ok(match b {
        0 => None,
        1 => Some(Role::Owner),
        2 => Some(Role::Editor),
        3 => Some(Role::Reviewer),
        4 => Some(Role::Reader),
        _ => return Err(corrupt(format!("unknown role {b}"))),
    })
}

impl Message {
    /// The message's bytes; [`Error::Invalid`] if an id isn't one or a field
    /// is too large.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut w = Writer::new();
        w.u8(VERSION);
        match self {
            Message::Challenge { nonce, server_time } => {
                w.u8(T_CHALLENGE);
                w.bytes_raw(nonce);
                w.u64(*server_time);
            }
            Message::Hello {
                hello,
                client,
                signature,
            } => {
                hello.write(&mut w)?;
                client.write(&mut w)?;
                w.bytes_raw(signature);
            }
            Message::Welcome {
                token,
                role,
                streams,
                app,
                devices,
            } => {
                w.u8(T_WELCOME);
                w.str(token)?;
                w.u8(role_byte(*role));
                w.count(streams.len())?;
                for s in streams {
                    w.str(&s.document)?;
                    w.u64(s.generation);
                    w.u64(s.length);
                    w.u64(s.batch);
                }
                w.str(app)?;
                w.count(devices.len())?;
                for d in devices {
                    w.id(&d.device, "device")?;
                    d.version.write(&mut w)?;
                }
            }
            Message::Refused { code, message } => {
                w.u8(T_REFUSED);
                w.u8(code.to_byte());
                w.str(message)?;
            }
            Message::Subscribe { members, streams } => {
                w.u8(T_SUBSCRIBE);
                w.u64(*members);
                w.count(streams.len())?;
                for p in streams {
                    w.str(&p.document)?;
                    w.id(&p.device, "device")?;
                    w.u64(p.generation);
                    w.u64(p.offset);
                }
            }
            Message::MembersRecords { offset, bytes } => {
                w.u8(T_MEMBERS_RECORDS);
                w.u64(*offset);
                w.bytes(bytes)?;
            }
            Message::Records {
                document,
                device,
                generation,
                offset,
                bytes,
            } => {
                w.u8(T_RECORDS);
                w.str(document)?;
                w.id(device, "device")?;
                w.u64(*generation);
                w.u64(*offset);
                w.bytes(bytes)?;
            }
            Message::SnapshotChanged {
                document,
                device,
                generation,
                covers,
            } => {
                w.u8(T_SNAPSHOT_CHANGED);
                w.str(document)?;
                w.id(device, "device")?;
                w.u64(*generation);
                w.u64(*covers);
            }
            Message::Synced => w.u8(T_SYNCED),
            Message::Append {
                document,
                batch,
                records,
            } => {
                w.u8(T_APPEND);
                w.str(document)?;
                w.u64(*batch);
                w.bytes(records)?;
            }
            Message::Ack {
                batch,
                document,
                generation,
                length,
            } => {
                w.u8(T_ACK);
                w.u64(*batch);
                w.str(document)?;
                w.u64(*generation);
                w.u64(*length);
            }
            Message::Nack {
                batch,
                code,
                message,
            } => {
                w.u8(T_NACK);
                w.u64(*batch);
                w.u8(code.to_byte());
                w.str(message)?;
            }
            Message::AppendMembers { batch, record } => {
                w.u8(T_APPEND_MEMBERS);
                w.u64(*batch);
                w.bytes(record)?;
            }
            Message::MembersAck { batch, seq } => {
                w.u8(T_MEMBERS_ACK);
                w.u64(*batch);
                w.u64(*seq);
            }
            Message::Compacted {
                document,
                device,
                generation,
                covers,
            } => {
                w.u8(T_COMPACTED);
                w.str(document)?;
                w.id(device, "device")?;
                w.u64(*generation);
                w.u64(*covers);
            }
            Message::Presence { bytes } => {
                w.u8(T_PRESENCE);
                w.bytes(bytes)?;
            }
            Message::PeerPresence { device, bytes } => {
                w.u8(T_PEER_PRESENCE);
                w.id(device, "device")?;
                w.bytes(bytes)?;
            }
            Message::Ping(value) => {
                w.u8(T_PING);
                w.u64(*value);
            }
            Message::Pong(value) => {
                w.u8(T_PONG);
                w.u64(*value);
            }
        }
        Ok(w.0)
    }

    /// The message in `bytes` (see the module docs for the errors).
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader(bytes);
        let version = r.u8()?;
        let kind = r.u8()?;
        // Challenge and refusal are frozen (see the module docs).
        if !(MIN_VERSION..=VERSION).contains(&version) && !matches!(kind, T_CHALLENGE | T_REFUSED) {
            return Err(Error::Unsupported(format!(
                "protocol version {version}; this side speaks {MIN_VERSION} to {VERSION}"
            )));
        }
        let message = match kind {
            T_CHALLENGE => Message::Challenge {
                nonce: r.array()?,
                server_time: r.u64()?,
            },
            T_HELLO_DEVICE | T_HELLO_INVITEE | T_CREATE_PROJECT => {
                let hello = match kind {
                    T_HELLO_DEVICE => Hello::Device {
                        project: r.id()?,
                        device: r.id()?,
                    },
                    T_HELLO_INVITEE => Hello::Invitee {
                        project: r.id()?,
                        invite: r.id()?,
                    },
                    _ => Hello::Create {
                        genesis: r.bytes()?,
                    },
                };
                Message::Hello {
                    hello,
                    client: ClientVersion::read(&mut r)?,
                    signature: r.array()?,
                }
            }
            T_WELCOME => {
                let token = r.str()?;
                let role = role_from_byte(r.u8()?)?;
                let mut streams = Vec::new();
                for _ in 0..r.u32()? {
                    streams.push(OwnStream {
                        document: r.str()?,
                        generation: r.u64()?,
                        length: r.u64()?,
                        batch: r.u64()?,
                    });
                }
                let app = r.str()?;
                let mut devices = Vec::new();
                for _ in 0..r.u32()? {
                    devices.push(DeviceVersion {
                        device: r.id()?,
                        version: ClientVersion::read(&mut r)?,
                    });
                }
                Message::Welcome {
                    token,
                    role,
                    streams,
                    app,
                    devices,
                }
            }
            T_REFUSED => Message::Refused {
                code: Code::from_byte(r.u8()?)?,
                message: r.str()?,
            },
            T_SUBSCRIBE => {
                let members = r.u64()?;
                let mut streams = Vec::new();
                for _ in 0..r.u32()? {
                    streams.push(Position {
                        document: r.str()?,
                        device: r.id()?,
                        generation: r.u64()?,
                        offset: r.u64()?,
                    });
                }
                Message::Subscribe { members, streams }
            }
            T_MEMBERS_RECORDS => Message::MembersRecords {
                offset: r.u64()?,
                bytes: r.bytes()?,
            },
            T_RECORDS => Message::Records {
                document: r.str()?,
                device: r.id()?,
                generation: r.u64()?,
                offset: r.u64()?,
                bytes: r.bytes()?,
            },
            T_SNAPSHOT_CHANGED => Message::SnapshotChanged {
                document: r.str()?,
                device: r.id()?,
                generation: r.u64()?,
                covers: r.u64()?,
            },
            T_SYNCED => Message::Synced,
            T_APPEND => Message::Append {
                document: r.str()?,
                batch: r.u64()?,
                records: r.bytes()?,
            },
            T_ACK => Message::Ack {
                batch: r.u64()?,
                document: r.str()?,
                generation: r.u64()?,
                length: r.u64()?,
            },
            T_NACK => Message::Nack {
                batch: r.u64()?,
                code: Code::from_byte(r.u8()?)?,
                message: r.str()?,
            },
            T_APPEND_MEMBERS => Message::AppendMembers {
                batch: r.u64()?,
                record: r.bytes()?,
            },
            T_MEMBERS_ACK => Message::MembersAck {
                batch: r.u64()?,
                seq: r.u64()?,
            },
            T_COMPACTED => Message::Compacted {
                document: r.str()?,
                device: r.id()?,
                generation: r.u64()?,
                covers: r.u64()?,
            },
            T_PRESENCE => Message::Presence { bytes: r.bytes()? },
            T_PEER_PRESENCE => Message::PeerPresence {
                device: r.id()?,
                bytes: r.bytes()?,
            },
            T_PING => Message::Ping(r.u64()?),
            T_PONG => Message::Pong(r.u64()?),
            other => {
                return Err(Error::Unsupported(format!("unknown message type {other}")));
            }
        };
        if !r.0.is_empty() {
            return Err(corrupt(format!("{} bytes after the message", r.0.len())));
        }
        Ok(message)
    }
}

/// What the owner's device signs to delete `project` at unix time `time`.
fn deletion_message(project: &str, time: u64) -> Result<Vec<u8>> {
    let mut message = id_bytes(project, "project")?.to_vec();
    message.extend_from_slice(&time.to_le_bytes());
    Ok(message)
}

/// The body of a request deleting `project` at unix time `time`, signed by
/// `signer` (see the module docs).
pub fn deletion_body(signer: &dyn Signer, project: &str, time: u64) -> Result<Vec<u8>> {
    let signature = signer.sign(DELETE_TAG, &deletion_message(project, time)?);
    let mut body = time.to_le_bytes().to_vec();
    body.extend_from_slice(&signature);
    Ok(body)
}

/// The time in deletion body `body` for `project`, after checking that
/// `public` signed it; [`Error::Corrupt`] if it has the wrong length,
/// [`Error::Signature`] if the signature doesn't match.
pub fn verify_deletion(public: &[u8; KEY_LEN], project: &str, body: &[u8]) -> Result<u64> {
    let (time, signature) = body
        .split_first_chunk::<8>()
        .filter(|(_, rest)| rest.len() == SIGNATURE_LEN)
        .ok_or_else(|| corrupt("a deletion body is 72 bytes".into()))?;
    let time = u64::from_le_bytes(*time);
    let signature: &[u8; SIGNATURE_LEN] = signature.try_into().expect("64 bytes");
    verify(
        public,
        DELETE_TAG,
        &deletion_message(project, time)?,
        signature,
    )?;
    Ok(time)
}

fn corrupt(why: String) -> Error {
    Error::Corrupt(format!("wire message: {why}"))
}

struct Writer(Vec<u8>);

impl Writer {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn u8(&mut self, value: u8) {
        self.0.push(value);
    }

    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    fn count(&mut self, n: usize) -> Result<()> {
        let n = u32::try_from(n).map_err(|_| Error::Invalid("too many items".into()))?;
        self.0.extend_from_slice(&n.to_le_bytes());
        Ok(())
    }

    fn bytes_raw(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    fn bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.count(bytes.len())?;
        self.0.extend_from_slice(bytes);
        Ok(())
    }

    fn str(&mut self, s: &str) -> Result<()> {
        self.bytes(s.as_bytes())
    }

    fn id(&mut self, id: &str, what: &str) -> Result<()> {
        self.0.extend_from_slice(&id_bytes(id, what)?);
        Ok(())
    }
}

/// The bytes not read yet.
struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        if self.0.len() < n {
            return Err(corrupt("cut short".into()));
        }
        let (taken, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(taken)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into().expect("N bytes"))
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn bytes(&mut self) -> Result<Vec<u8>> {
        let len = self.u32()? as usize;
        Ok(self.take(len)?.to_vec())
    }

    fn str(&mut self) -> Result<String> {
        String::from_utf8(self.bytes()?).map_err(|_| corrupt("a string isn't UTF-8".into()))
    }

    fn id(&mut self) -> Result<String> {
        Ok(to_hex(&self.array::<ID_LEN>()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::DeviceKeys;
    use crate::random_id;

    fn id() -> String {
        random_id().unwrap()
    }

    fn client(level: u32) -> ClientVersion {
        ClientVersion {
            level,
            app: "0.1.0 (1a2b3c4)".into(),
        }
    }

    /// One of every message.
    fn every_message() -> Vec<Message> {
        let signature = [7; SIGNATURE_LEN];
        vec![
            Message::Challenge {
                nonce: [1; NONCE_LEN],
                server_time: 1_700_000_000,
            },
            Message::Hello {
                hello: Hello::Device {
                    project: id(),
                    device: id(),
                },
                client: client(1),
                signature,
            },
            Message::Hello {
                hello: Hello::Invitee {
                    project: id(),
                    invite: id(),
                },
                client: client(2),
                signature,
            },
            Message::Hello {
                hello: Hello::Create {
                    genesis: b"genesis".to_vec(),
                },
                client: ClientVersion {
                    level: u32::MAX,
                    app: String::new(),
                },
                signature,
            },
            Message::Welcome {
                token: "abc".into(),
                role: Some(Role::Reviewer),
                streams: vec![
                    OwnStream {
                        document: "main".into(),
                        generation: 2,
                        length: 300,
                        batch: 9,
                    },
                    OwnStream {
                        document: "review".into(),
                        generation: 0,
                        length: 0,
                        batch: 0,
                    },
                ],
                app: "0.1.0".into(),
                devices: vec![
                    DeviceVersion {
                        device: id(),
                        version: client(1),
                    },
                    DeviceVersion {
                        device: id(),
                        version: client(3),
                    },
                ],
            },
            Message::Welcome {
                token: String::new(),
                role: None,
                streams: vec![],
                app: String::new(),
                devices: vec![],
            },
            Message::Refused {
                code: Code::InviteClosed,
                message: "the invite has expired".into(),
            },
            Message::Subscribe {
                members: 1234,
                streams: vec![Position {
                    document: "main".into(),
                    device: id(),
                    generation: 1,
                    offset: 77,
                }],
            },
            Message::MembersRecords {
                offset: 5,
                bytes: vec![1, 2, 3],
            },
            Message::Records {
                document: "review".into(),
                device: id(),
                generation: 3,
                offset: u64::MAX,
                bytes: vec![],
            },
            Message::SnapshotChanged {
                document: "main".into(),
                device: id(),
                generation: 4,
                covers: 900,
            },
            Message::Synced,
            Message::Append {
                document: "main".into(),
                batch: 42,
                records: vec![9; 100],
            },
            Message::Ack {
                batch: 42,
                document: "main".into(),
                generation: 1,
                length: 108,
            },
            Message::Nack {
                batch: 43,
                code: Code::Forbidden,
                message: "readers don't write".into(),
            },
            Message::AppendMembers {
                batch: 1,
                record: vec![0; 10],
            },
            Message::MembersAck { batch: 1, seq: 12 },
            Message::Compacted {
                document: "main".into(),
                device: id(),
                generation: 5,
                covers: 64,
            },
            Message::Presence {
                bytes: b"cursor".to_vec(),
            },
            Message::PeerPresence {
                device: id(),
                bytes: b"cursor".to_vec(),
            },
            Message::Ping(7),
            Message::Pong(7),
        ]
    }

    #[test]
    fn every_message_round_trips() {
        for message in every_message() {
            let bytes = message.encode().unwrap();
            assert_eq!(bytes[0], VERSION);
            assert_eq!(Message::decode(&bytes).unwrap(), message, "{message:?}");
        }
        for code in 1..=13 {
            let code = Code::from_byte(code).unwrap();
            assert_eq!(Code::from_byte(code.to_byte()).unwrap(), code);
        }
    }

    #[test]
    fn exact_bytes() {
        let ping = Message::Ping(0x0102).encode().unwrap();
        assert_eq!(ping, [2, T_PING, 2, 1, 0, 0, 0, 0, 0, 0]);
        let presence = Message::Presence {
            bytes: b"ab".to_vec(),
        };
        assert_eq!(
            presence.encode().unwrap(),
            [2, T_PRESENCE, 2, 0, 0, 0, b'a', b'b']
        );
        let device = "000102030405060708090a0b0c0d0e0f";
        let peer = Message::PeerPresence {
            device: device.into(),
            bytes: vec![],
        };
        let mut expected = vec![2, T_PEER_PRESENCE];
        expected.extend(0..16u8);
        expected.extend([0, 0, 0, 0]);
        assert_eq!(peer.encode().unwrap(), expected);
        // The frozen layouts, byte by byte (see the module docs).
        let challenge = Message::Challenge {
            nonce: [9; NONCE_LEN],
            server_time: 3,
        };
        let mut expected = vec![2, 1];
        expected.extend([9; NONCE_LEN]);
        expected.extend([3, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(challenge.encode().unwrap(), expected);
        let refused = Message::Refused {
            code: Code::Version,
            message: "up".into(),
        };
        assert_eq!(refused.encode().unwrap(), [2, 6, 1, 2, 0, 0, 0, b'u', b'p']);
    }

    #[test]
    fn challenge_and_refusal_decode_under_any_version() {
        let frozen = [
            Message::Challenge {
                nonce: [5; NONCE_LEN],
                server_time: 77,
            },
            Message::Refused {
                code: Code::Version,
                message: "This project needs Lethora 0.2.0 or later.".into(),
            },
        ];
        for message in frozen {
            let mut bytes = message.encode().unwrap();
            for version in [0, 1, 3, 200, 255] {
                bytes[0] = version;
                assert_eq!(Message::decode(&bytes).unwrap(), message, "{version}");
            }
        }
        // Everything else needs a version in the range.
        let mut ping = Message::Ping(1).encode().unwrap();
        for version in [MIN_VERSION - 1, VERSION + 1] {
            ping[0] = version;
            assert!(matches!(Message::decode(&ping), Err(Error::Unsupported(_))));
        }
    }

    #[test]
    fn bad_input_never_panics() {
        for message in every_message() {
            let bytes = message.encode().unwrap();
            // Every truncation fails.
            for len in 0..bytes.len() {
                assert!(
                    Message::decode(&bytes[..len]).is_err(),
                    "{message:?} at {len}"
                );
            }
            // Trailing bytes fail.
            let mut longer = bytes.clone();
            longer.push(0);
            assert!(Message::decode(&longer).is_err());
            // Flipped bytes never panic.
            for i in 0..bytes.len() {
                for bit in [0x01, 0x80, 0xff] {
                    let mut bad = bytes.clone();
                    bad[i] ^= bit;
                    let _ = Message::decode(&bad);
                }
            }
        }
        // Pseudo-random garbage, including huge lengths and counts.
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..20_000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let len = (seed % 64) as usize;
            let mut bytes: Vec<u8> = (0..len)
                .map(|i| (seed >> (i % 8 * 8)) as u8 ^ i as u8)
                .collect();
            if let Some(first) = bytes.first_mut() {
                *first = VERSION;
            }
            if bytes.len() > 1 {
                bytes[1] = (seed % 23) as u8;
            }
            let _ = Message::decode(&bytes);
        }
        assert!(matches!(
            Message::decode(&[3, T_PING, 0, 0, 0, 0, 0, 0, 0, 0]),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            Message::decode(&[2, 99]),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            Message::decode(&[2, T_REFUSED, 99, 0, 0, 0, 0]),
            Err(Error::Corrupt(_))
        ));
        assert!(matches!(
            Message::decode(&[2, T_PRESENCE, 0xff, 0xff, 0xff, 0xff]),
            Err(Error::Corrupt(_))
        ));
        assert!(matches!(
            Message::decode(&[1, T_REFUSED, 1, 1, 0, 0, 0, 0xff]),
            Err(Error::Corrupt(_))
        ));
        // A huge count with nothing behind it.
        let mut subscribe = vec![2, T_SUBSCRIBE];
        subscribe.extend([0; 8]);
        subscribe.extend([0xff; 4]);
        assert!(Message::decode(&subscribe).is_err());
        // Ids that aren't ids don't encode.
        let bad = Message::PeerPresence {
            device: "nope".into(),
            bytes: vec![],
        };
        assert!(matches!(bad.encode(), Err(Error::Invalid(_))));
        // Nor do app versions that are too long.
        let long = Message::Hello {
            hello: Hello::Create { genesis: vec![] },
            client: ClientVersion {
                level: 1,
                app: "x".repeat(MAX_APP_LEN + 1),
            },
            signature: [0; SIGNATURE_LEN],
        };
        assert!(matches!(long.encode(), Err(Error::Invalid(_))));
    }

    #[test]
    fn hellos_are_signed_for_their_place() {
        let keys = DeviceKeys::generate().unwrap();
        let public = keys.public().sign;
        let nonce = [3; NONCE_LEN];
        let server = "https://lethora.example";
        let hello = Hello::Device {
            project: id(),
            device: id(),
        };
        let at = client(1);
        let Message::Hello {
            hello: signed,
            client: signed_client,
            signature,
        } = hello
            .clone()
            .sign(at.clone(), &keys, &nonce, server)
            .unwrap()
        else {
            panic!("a hello");
        };
        assert_eq!((&signed, &signed_client), (&hello, &at));
        hello
            .verify(&at, &public, &nonce, server, &signature)
            .unwrap();
        assert!(
            hello
                .verify(&at, &public, &[4; NONCE_LEN], server, &signature)
                .is_err()
        );
        assert!(
            hello
                .verify(&at, &public, &nonce, "https://other.example", &signature)
                .is_err()
        );
        // The level and the app version are signed too.
        for other in [
            client(2),
            ClientVersion {
                level: 1,
                app: "9.9.9".into(),
            },
        ] {
            assert!(
                hello
                    .verify(&other, &public, &nonce, server, &signature)
                    .is_err()
            );
        }
        let other = Hello::Device {
            project: id(),
            device: id(),
        };
        assert!(
            other
                .verify(&at, &public, &nonce, server, &signature)
                .is_err()
        );
        let invitee = Hello::Invitee {
            project: id(),
            invite: id(),
        };
        assert!(
            invitee
                .verify(&at, &public, &nonce, server, &signature)
                .is_err()
        );
        let stranger = DeviceKeys::generate().unwrap().public().sign;
        assert!(
            hello
                .verify(&at, &stranger, &nonce, server, &signature)
                .is_err()
        );
    }

    #[test]
    fn deletion_is_signed() {
        let keys = DeviceKeys::generate().unwrap();
        let public = keys.public().sign;
        let project = id();
        let body = deletion_body(&keys, &project, 1_000).unwrap();
        assert_eq!(body.len(), DELETION_LEN);
        assert_eq!(verify_deletion(&public, &project, &body).unwrap(), 1_000);
        assert!(verify_deletion(&public, &id(), &body).is_err());
        assert!(verify_deletion(&public, &project, &body[1..]).is_err());
        let mut later = body.clone();
        later[0] = 1;
        assert!(verify_deletion(&public, &project, &later).is_err());
        let stranger = DeviceKeys::generate().unwrap().public().sign;
        assert!(verify_deletion(&stranger, &project, &body).is_err());
    }
}
