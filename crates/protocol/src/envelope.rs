//! Sealed envelopes: XChaCha20-Poly1305 with a project key (32 random bytes)
//! and a random nonce, the associated data binding each envelope to its
//! place, so nothing can be moved to another place unnoticed.
//!
//! # Version 1 (folder sync)
//!
//! `[u8 version = 1][u32 LE epoch = 1][24 bytes nonce][ciphertext + 16 bytes
//! tag]`. The associated data is the version byte and the epoch (as in the
//! header), then the ASCII strings `lethora-replica-1`, the project id and the
//! kind (`check`, `snapshot`, `log` or `asset`), each followed by a zero byte,
//! then the name: the device id for `snapshot` and `log`, the blob name for
//! `asset`, nothing for `check`. Unsigned: in a shared folder anyone with the
//! key may write.
//!
//! # Version 3 (folder sync, with a level)
//!
//! Version 1 with the feature level the payload needs
//! ([`LEVEL`](crate::LEVEL)) after the epoch: `[u8 version = 3][u32 LE epoch
//! = 1][u32 LE level][24 bytes nonce][ciphertext + 16 bytes tag]`, the level
//! in the associated data with the rest of the header (version, epoch,
//! level, then the strings as in version 1). What [`Sealer`] writes; it
//! reads versions 1 and 3 (version 1 as level 1) and rejects other versions
//! and epochs ([`Error::Unsupported`]).
//!
//! # Version 2 (sync server)
//!
//! `[u8 version = 2][u32 LE epoch][16 bytes signer device id][24 bytes
//! nonce][ciphertext + 16 bytes tag][64 bytes Ed25519 signature]`, the device
//! id as its raw bytes. The context names the place: project id, document
//! (`main`, `review`, `members`), kind (`snapshot`, `log`, `asset`,
//! `profile`, ...) and name (the device id for `snapshot` and `log`, the blob
//! name for `asset`, empty otherwise); none may hold a zero byte. The
//! associated data is the version byte, the epoch and the signer id (as in
//! the header), then `lethora-server-1` and the four context strings, each
//! followed by a zero byte. The signer device signs `lethora-envelope-2`, a
//! zero byte, every envelope byte before the signature and the four context
//! strings (each followed by a zero byte), so the server checks who wrote an
//! envelope, and where it belongs, without the key. The key is the
//! [`Keyring`]'s for the envelope's epoch; sealing uses the newest epoch.
//! Epoch 1 is the key a project is shared with; removing a member starts the
//! next one (the `epoch` entry of the members log, see
//! [`members`](crate::members)), and older epochs stay readable.
//!
//! # Version 4 (sync server, with a level)
//!
//! Version 2 with the level after the epoch: `[u8 version = 4][u32 LE
//! epoch][u32 LE level][16 bytes signer device id][24 bytes
//! nonce][ciphertext + 16 bytes tag][64 bytes signature]`, the level in the
//! associated data with the rest of the header (version, epoch, level,
//! signer, then the strings as in version 2) and under the signature. What
//! [`seal_v2`] writes; [`header_v2`], [`verify_v2`] and [`open_v2`] read
//! versions 2 and 4 (version 2 as level 1) and reject other versions
//! ([`Error::Unsupported`]).

use std::collections::BTreeMap;
use std::fmt;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};

use crate::keys::{DeviceKeys, DevicePublic, KEY_LEN, SIGNATURE_LEN, Signer};
use crate::{Error, ID_LEN, Result, id_bytes, random, to_hex};

const VERSION_1: u8 = 1;
const VERSION_2: u8 = 2;
const VERSION_3: u8 = 3;
const VERSION_4: u8 = 4;
/// The only epoch of versions 1 and 3.
const EPOCH_1: u32 = 1;
/// The level of envelopes of versions 1 and 2, written before levels.
const LEVEL_1: u32 = 1;
const NONCE_LEN: usize = 24;
/// Bytes before a version 1 envelope's ciphertext.
pub const ENVELOPE_HEADER: usize = 1 + 4 + NONCE_LEN;
/// Bytes before a version 3 envelope's ciphertext.
pub const ENVELOPE_3_HEADER: usize = 1 + 4 + 4 + NONCE_LEN;
/// Bytes before a version 2 envelope's ciphertext.
pub const ENVELOPE_2_HEADER: usize = 1 + 4 + ID_LEN + NONCE_LEN;
/// Bytes before a version 4 envelope's ciphertext.
pub const ENVELOPE_4_HEADER: usize = 1 + 4 + 4 + ID_LEN + NONCE_LEN;
/// Bytes of the Poly1305 tag.
const TAG_LEN: usize = 16;
/// First part of every version 1 envelope's associated data.
const AD_TAG: &str = "lethora-replica-1";
/// First part of the context in a version 2 envelope's associated data.
const AD_TAG_2: &str = "lethora-server-1";
/// Domain tag of version 2 signatures.
const SIGNATURE_TAG: &[u8] = b"lethora-envelope-2\0";

/// What a version 1 envelope holds (part of its associated data).
#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Check,
    Snapshot,
    Log,
    Asset,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Check => "check",
            Kind::Snapshot => "snapshot",
            Kind::Log => "log",
            Kind::Asset => "asset",
        }
    }
}

/// Seals version 3 envelopes of one project and opens versions 1 and 3 (see
/// the module docs).
pub struct Sealer {
    project: String,
    cipher: XChaCha20Poly1305,
}

impl Sealer {
    pub fn new(project: &str, key: &[u8; KEY_LEN]) -> Self {
        Self {
            project: project.to_owned(),
            cipher: XChaCha20Poly1305::new(&Key::from(*key)),
        }
    }

    /// The associated data of an envelope with header `header` (the bytes
    /// before the nonce) of `kind` named `name`.
    fn ad(&self, header: &[u8], kind: Kind, name: &str) -> Vec<u8> {
        let mut ad = header.to_vec();
        for part in [AD_TAG, &self.project, kind.as_str()] {
            ad.extend_from_slice(part.as_bytes());
            ad.push(0);
        }
        ad.extend_from_slice(name.as_bytes());
        ad
    }

    /// An envelope of `kind` named `name` holding `plain`, which needs
    /// feature level `level`.
    pub fn seal(&self, kind: Kind, name: &str, level: u32, plain: &[u8]) -> Result<Vec<u8>> {
        self.seal_with_nonce(kind, name, level, plain, random()?)
    }

    fn seal_with_nonce(
        &self,
        kind: Kind,
        name: &str,
        level: u32,
        plain: &[u8],
        nonce: [u8; NONCE_LEN],
    ) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(ENVELOPE_3_HEADER + plain.len() + TAG_LEN);
        out.push(VERSION_3);
        out.extend_from_slice(&EPOCH_1.to_le_bytes());
        out.extend_from_slice(&level.to_le_bytes());
        let aad = self.ad(&out, kind, name);
        let sealed = self
            .cipher
            .encrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: plain,
                    aad: &aad,
                },
            )
            .map_err(|_| Error::Corrupt(format!("can't encrypt {} {name}", kind.as_str())))?;
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    /// The payload of `envelope`; [`Error::Corrupt`] if it is damaged or
    /// doesn't belong here, [`Error::Unsupported`] for another version or
    /// epoch.
    pub fn open(&self, kind: Kind, name: &str, envelope: &[u8]) -> Result<Vec<u8>> {
        self.open_leveled(kind, name, envelope)
            .map(|(plain, _)| plain)
    }

    /// The payload of `envelope` and the level it needs (1 for version 1),
    /// failing like [`Sealer::open`].
    pub fn open_leveled(&self, kind: Kind, name: &str, envelope: &[u8]) -> Result<(Vec<u8>, u32)> {
        let what = || format!("{} {name}", kind.as_str());
        if envelope.len() < ENVELOPE_HEADER {
            return Err(Error::Corrupt(format!("{} is cut short", what())));
        }
        let epoch = u32::from_le_bytes([envelope[1], envelope[2], envelope[3], envelope[4]]);
        let header = match envelope[0] {
            VERSION_1 => 5,
            VERSION_3 => 9,
            _ => 0,
        };
        if header == 0 || epoch != EPOCH_1 {
            return Err(Error::Unsupported(format!(
                "{} has envelope version {}, epoch {epoch}; update Lethora to read it",
                what(),
                envelope[0]
            )));
        }
        if envelope.len() < header + NONCE_LEN {
            return Err(Error::Corrupt(format!("{} is cut short", what())));
        }
        let level = match header {
            9 => u32::from_le_bytes(envelope[5..9].try_into().expect("4 bytes")),
            _ => LEVEL_1,
        };
        let nonce: [u8; NONCE_LEN] = envelope[header..header + NONCE_LEN]
            .try_into()
            .expect("24 bytes");
        let aad = self.ad(&envelope[..header], kind, name);
        let plain = self
            .cipher
            .decrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: &envelope[header + NONCE_LEN..],
                    aad: &aad,
                },
            )
            .map_err(|_| {
                Error::Corrupt(format!(
                    "{} can't be decrypted (damaged, or not from this place)",
                    what()
                ))
            })?;
        Ok((plain, level))
    }
}

/// The project keys by epoch. `Debug` shows only the epochs.
#[derive(Clone)]
pub struct Keyring {
    keys: BTreeMap<u32, [u8; KEY_LEN]>,
}

impl Keyring {
    /// A keyring holding `key` as epoch 1.
    pub fn new(key: [u8; KEY_LEN]) -> Self {
        Self {
            keys: BTreeMap::from([(EPOCH_1, key)]),
        }
    }

    /// A keyring holding `keys` (epoch, key); `None` if there are none.
    pub fn from_epochs(keys: impl IntoIterator<Item = (u32, [u8; KEY_LEN])>) -> Option<Self> {
        let keys: BTreeMap<u32, [u8; KEY_LEN]> = keys.into_iter().collect();
        (!keys.is_empty()).then_some(Self { keys })
    }

    /// Every epoch with its key, oldest first.
    pub fn epochs(&self) -> impl Iterator<Item = (u32, &[u8; KEY_LEN])> {
        self.keys.iter().map(|(epoch, key)| (*epoch, key))
    }

    /// Adds (or replaces) the key of `epoch`.
    pub fn insert(&mut self, epoch: u32, key: [u8; KEY_LEN]) {
        self.keys.insert(epoch, key);
    }

    /// The key of `epoch`.
    pub fn get(&self, epoch: u32) -> Option<&[u8; KEY_LEN]> {
        self.keys.get(&epoch)
    }

    /// The newest epoch and its key, for sealing.
    pub fn newest(&self) -> (u32, &[u8; KEY_LEN]) {
        let (epoch, key) = self.keys.last_key_value().expect("never empty");
        (*epoch, key)
    }
}

impl fmt::Debug for Keyring {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Keyring")
            .field("epochs", &self.keys.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// Where a version 2 or 4 envelope belongs (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Context<'a> {
    pub project: &'a str,
    pub document: &'a str,
    pub kind: &'a str,
    pub name: &'a str,
}

impl Context<'_> {
    /// The four strings, each followed by a zero byte.
    fn bytes(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        for part in [self.project, self.document, self.kind, self.name] {
            if part.contains('\0') {
                return Err(Error::Invalid(format!(
                    "envelope context {part:?} holds a zero byte"
                )));
            }
            out.extend_from_slice(part.as_bytes());
            out.push(0);
        }
        Ok(out)
    }
}

/// The header of a version 2 or 4 envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub version: u8,
    pub epoch: u32,
    /// The feature level the payload needs (1 for version 2).
    pub level: u32,
    /// The signer's device id (32 lowercase hex).
    pub signer: String,
}

/// The bytes before the nonce of a server envelope of `version`: version,
/// epoch, (level,) signer; `None` for a version that isn't one.
fn prefix_len(version: u8) -> Option<usize> {
    match version {
        VERSION_2 => Some(ENVELOPE_2_HEADER - NONCE_LEN),
        VERSION_4 => Some(ENVELOPE_4_HEADER - NONCE_LEN),
        _ => None,
    }
}

/// The header of version 2 or 4 envelope `envelope`, unchecked (e.g. to
/// look up the signer's key); [`Error::Unsupported`] for another version,
/// [`Error::Corrupt`] if it is too short to be one.
pub fn header_v2(envelope: &[u8]) -> Result<Header> {
    let Some(&version) = envelope.first() else {
        return Err(Error::Corrupt("the envelope is cut short".into()));
    };
    let prefix = prefix_len(version).ok_or_else(|| {
        Error::Unsupported(format!(
            "envelope version {version}; update Lethora to read it"
        ))
    })?;
    if envelope.len() < prefix + NONCE_LEN + TAG_LEN + SIGNATURE_LEN {
        return Err(Error::Corrupt("the envelope is cut short".into()));
    }
    let u32_at = |at: usize| u32::from_le_bytes(envelope[at..at + 4].try_into().expect("4 bytes"));
    Ok(Header {
        version,
        epoch: u32_at(1),
        level: if version == VERSION_4 {
            u32_at(5)
        } else {
            LEVEL_1
        },
        signer: to_hex(&envelope[prefix - ID_LEN..prefix]),
    })
}

/// A version 4 envelope holding `plain`, which needs feature level `level`,
/// sealed with the newest key of `keyring` and signed by device `signer`
/// with `keys`.
pub fn seal_v2(
    keyring: &Keyring,
    signer: &str,
    keys: &DeviceKeys,
    context: &Context,
    level: u32,
    plain: &[u8],
) -> Result<Vec<u8>> {
    let signer = id_bytes(signer, "device")?;
    let context = context.bytes()?;
    let (epoch, key) = keyring.newest();
    let nonce: [u8; NONCE_LEN] = random()?;
    let mut out = Vec::with_capacity(ENVELOPE_4_HEADER + plain.len() + TAG_LEN + SIGNATURE_LEN);
    out.push(VERSION_4);
    out.extend_from_slice(&epoch.to_le_bytes());
    out.extend_from_slice(&level.to_le_bytes());
    out.extend_from_slice(&signer);
    let aad = [&out[..], AD_TAG_2.as_bytes(), b"\0", &context].concat();
    let sealed = XChaCha20Poly1305::new(&Key::from(*key))
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plain,
                aad: &aad,
            },
        )
        .map_err(|_| Error::Corrupt("can't encrypt the envelope".into()))?;
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    let signature = keys.sign(SIGNATURE_TAG, &[&out[..], &context].concat());
    out.extend_from_slice(&signature);
    Ok(out)
}

/// Checks version 2 or 4 envelope `envelope`: that `signer` (the public keys of
/// the device its header names) signed it for `context`. Needs no key: what
/// the server does. Returns the header; [`Error::Signature`] if the signature
/// doesn't match (damaged, another signer or another place).
pub fn verify_v2(envelope: &[u8], signer: &DevicePublic, context: &Context) -> Result<Header> {
    let header = header_v2(envelope)?;
    let context = context.bytes()?;
    let (signed, signature) = envelope.split_at(envelope.len() - SIGNATURE_LEN);
    let signature: &[u8; SIGNATURE_LEN] = signature.try_into().expect("64 bytes");
    signer
        .verify(SIGNATURE_TAG, &[signed, &context].concat(), signature)
        .map_err(|_| Error::Signature("the envelope's signature doesn't match".into()))?;
    Ok(header)
}

/// The payload of version 2 or 4 envelope `envelope`, after checking it like
/// [`verify_v2`] (its header's level is then authentic); [`Error::Unsupported`]
/// for an epoch `keyring` lacks, [`Error::Corrupt`] if it can't be
/// decrypted.
pub fn open_v2(
    keyring: &Keyring,
    signer: &DevicePublic,
    context: &Context,
    envelope: &[u8],
) -> Result<Vec<u8>> {
    let header = verify_v2(envelope, signer, context)?;
    let key = keyring.get(header.epoch).ok_or_else(|| {
        Error::Unsupported(format!(
            "envelope epoch {} is unknown here; the project key of that epoch is missing",
            header.epoch
        ))
    })?;
    let context = context.bytes()?;
    let prefix = prefix_len(header.version).expect("checked");
    let aad = [&envelope[..prefix], AD_TAG_2.as_bytes(), b"\0", &context].concat();
    let nonce: [u8; NONCE_LEN] = envelope[prefix..prefix + NONCE_LEN]
        .try_into()
        .expect("24 bytes");
    XChaCha20Poly1305::new(&Key::from(*key))
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &envelope[prefix + NONCE_LEN..envelope.len() - SIGNATURE_LEN],
                aad: &aad,
            },
        )
        .map_err(|_| Error::Corrupt("the envelope can't be decrypted".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{from_hex, random_id};

    const PROJECT: &str = "0123456789abcdef0123456789abcdef";
    const DEVICE: &str = "fedcba9876543210fedcba9876543210";

    /// Computed with the version 1 code in `lethora-core` before it moved
    /// here: version 1 envelopes, written before levels, open forever (as
    /// level 1).
    #[test]
    fn v1_test_vectors() {
        let key: [u8; KEY_LEN] = std::array::from_fn(|i| i as u8);
        let sealer = Sealer::new(PROJECT, &key);
        let vectors = [
            (
                Kind::Log,
                DEVICE,
                &b"hello, replica"[..],
                "0101000000a0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7\
                 2d896671d87184de041f41e0125128abf463976a77a590e4d6cd87e97d00",
            ),
            (
                Kind::Check,
                "",
                &b""[..],
                "0101000000a0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7\
                 81ea80fe7e063c4a3da7be6c84a665b6",
            ),
        ];
        for (kind, name, plain, hex) in vectors {
            let expected = from_hex(hex).unwrap();
            let opened = sealer.open_leveled(kind, name, &expected).unwrap();
            assert_eq!(opened, (plain.to_vec(), 1), "{}", kind.as_str());
        }
    }

    /// Version 3: the envelope bytes and associated data must never change.
    #[test]
    fn v3_test_vectors() {
        let key: [u8; KEY_LEN] = std::array::from_fn(|i| i as u8);
        let nonce: [u8; NONCE_LEN] = std::array::from_fn(|i| 0xa0 + i as u8);
        let sealer = Sealer::new(PROJECT, &key);
        let vectors = [
            (Kind::Log, DEVICE, 1, &b"hello, replica"[..], V3_LOG),
            (Kind::Check, "", 2, &b""[..], V3_CHECK),
        ];
        for (kind, name, level, plain, hex) in vectors {
            let sealed = sealer
                .seal_with_nonce(kind, name, level, plain, nonce)
                .unwrap();
            let expected = from_hex(hex).unwrap();
            assert_eq!(to_hex(&sealed), hex, "{}", kind.as_str());
            let opened = sealer.open_leveled(kind, name, &expected).unwrap();
            assert_eq!(opened, (plain.to_vec(), level));
        }
    }

    const V3_LOG: &str = "030100000001000000a0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7\
                          2d896671d87184de041f41e01251aa599514e72de802bd7138e919d8f73e";
    const V3_CHECK: &str = "030100000002000000a0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7\
                            02c2be04275e813dc08e87050fcc7eee";

    #[test]
    fn v3_is_bound_to_its_place_and_level() {
        let sealer = Sealer::new(PROJECT, &[7; KEY_LEN]);
        let sealed = sealer.seal(Kind::Log, "dev1", 2, b"hello").unwrap();
        assert_eq!(sealed.len(), ENVELOPE_3_HEADER + 5 + TAG_LEN);
        assert_eq!(sealer.open(Kind::Log, "dev1", &sealed).unwrap(), b"hello");
        assert_eq!(
            sealer.open_leveled(Kind::Log, "dev1", &sealed).unwrap().1,
            2
        );
        assert!(sealer.open(Kind::Log, "dev2", &sealed).is_err());
        assert!(sealer.open(Kind::Asset, "dev1", &sealed).is_err());
        // The level is authenticated: another one doesn't open.
        for level in [1, 3] {
            let mut other = sealed.clone();
            other[5] = level;
            assert!(matches!(
                sealer.open(Kind::Log, "dev1", &other),
                Err(Error::Corrupt(_))
            ));
        }
        let mut epoch = sealed.clone();
        epoch[1] = 2;
        assert!(matches!(
            sealer.open(Kind::Log, "dev1", &epoch),
            Err(Error::Unsupported(_))
        ));
        // Versions 2 (the server's) and 5 (unknown).
        for version in [2, 5] {
            let mut other = sealed.clone();
            other[0] = version;
            assert!(matches!(
                sealer.open(Kind::Log, "dev1", &other),
                Err(Error::Unsupported(_))
            ));
        }
        for len in [ENVELOPE_HEADER - 1, ENVELOPE_3_HEADER - 1] {
            assert!(matches!(
                sealer.open(Kind::Log, "dev1", &sealed[..len]),
                Err(Error::Corrupt(_))
            ));
        }
    }

    fn context<'a>(kind: &'a str, name: &'a str) -> Context<'a> {
        Context {
            project: PROJECT,
            document: "main",
            kind,
            name,
        }
    }

    /// A version 2 envelope, as written before levels (the old `seal_v2`).
    fn seal_v2_old(
        keyring: &Keyring,
        keys: &DeviceKeys,
        context: &Context,
        plain: &[u8],
    ) -> Vec<u8> {
        let context = context.bytes().unwrap();
        let (epoch, key) = keyring.newest();
        let mut out = vec![VERSION_2];
        out.extend_from_slice(&epoch.to_le_bytes());
        out.extend_from_slice(&id_bytes(DEVICE, "device").unwrap());
        let aad = [&out[..], AD_TAG_2.as_bytes(), b"\0", &context].concat();
        let nonce = [3; NONCE_LEN];
        let sealed = XChaCha20Poly1305::new(&Key::from(*key))
            .encrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: plain,
                    aad: &aad,
                },
            )
            .unwrap();
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        let signature = keys.sign(SIGNATURE_TAG, &[&out[..], &context].concat());
        out.extend_from_slice(&signature);
        out
    }

    #[test]
    fn v2_still_opens_as_level_1() {
        let keys = DeviceKeys::generate().unwrap();
        let keyring = Keyring::new([9; KEY_LEN]);
        let ctx = context("log", DEVICE);
        let old = seal_v2_old(&keyring, &keys, &ctx, b"update");
        assert_eq!(old.len(), ENVELOPE_2_HEADER + 6 + TAG_LEN + SIGNATURE_LEN);
        let header = verify_v2(&old, &keys.public(), &ctx).unwrap();
        assert_eq!(
            header,
            Header {
                version: 2,
                epoch: 1,
                level: 1,
                signer: DEVICE.into()
            }
        );
        assert_eq!(header_v2(&old).unwrap(), header);
        assert_eq!(
            open_v2(&keyring, &keys.public(), &ctx, &old).unwrap(),
            b"update"
        );
        let mut bad = old.clone();
        bad[30] ^= 1;
        assert!(open_v2(&keyring, &keys.public(), &ctx, &bad).is_err());
    }

    #[test]
    fn v2_round_trip() {
        let keys = DeviceKeys::generate().unwrap();
        let keyring = Keyring::new([9; KEY_LEN]);
        let ctx = context("log", DEVICE);
        let sealed = seal_v2(&keyring, DEVICE, &keys, &ctx, 3, b"update").unwrap();
        assert_eq!(
            sealed.len(),
            ENVELOPE_4_HEADER + 6 + TAG_LEN + SIGNATURE_LEN
        );
        let header = verify_v2(&sealed, &keys.public(), &ctx).unwrap();
        assert_eq!(
            header,
            Header {
                version: 4,
                epoch: 1,
                level: 3,
                signer: DEVICE.into()
            }
        );
        assert_eq!(header_v2(&sealed).unwrap(), header);
        assert_eq!(
            open_v2(&keyring, &keys.public(), &ctx, &sealed).unwrap(),
            b"update"
        );
        // An empty payload and an empty name.
        let profile = Context {
            document: "members",
            ..context("profile", "")
        };
        let sealed = seal_v2(&keyring, DEVICE, &keys, &profile, 1, b"").unwrap();
        assert_eq!(
            open_v2(&keyring, &keys.public(), &profile, &sealed).unwrap(),
            b""
        );
    }

    #[test]
    fn v2_rejects_tampering_and_other_places() {
        let keys = DeviceKeys::generate().unwrap();
        let public = keys.public();
        let keyring = Keyring::new([9; KEY_LEN]);
        let ctx = context("log", DEVICE);
        let sealed = seal_v2(&keyring, DEVICE, &keys, &ctx, 1, b"update").unwrap();
        // Every byte: header (the level too), ciphertext, tag, signature.
        for i in 0..sealed.len() {
            let mut bad = sealed.clone();
            bad[i] ^= 0x01;
            assert!(verify_v2(&bad, &public, &ctx).is_err(), "byte {i}");
            assert!(open_v2(&keyring, &public, &ctx, &bad).is_err(), "byte {i}");
        }
        assert!(verify_v2(&sealed[..sealed.len() - 1], &public, &ctx).is_err());
        let other = DeviceKeys::generate().unwrap().public();
        assert!(matches!(
            verify_v2(&sealed, &other, &ctx),
            Err(Error::Signature(_))
        ));
        let project = random_id().unwrap();
        let places = [
            Context {
                project: &project,
                ..ctx
            },
            Context {
                document: "review",
                ..ctx
            },
            context("snapshot", DEVICE),
            context("log", "00000000000000000000000000000000"),
        ];
        for place in places {
            assert!(verify_v2(&sealed, &public, &place).is_err(), "{place:?}");
            assert!(open_v2(&keyring, &public, &place, &sealed).is_err());
        }
        // A raised level is caught by the signature.
        let mut raised = sealed.clone();
        raised[5] = 2;
        assert!(matches!(
            verify_v2(&raised, &public, &ctx),
            Err(Error::Signature(_))
        ));
        assert!(matches!(
            seal_v2(&keyring, DEVICE, &keys, &context("lo\0g", ""), 1, b""),
            Err(Error::Invalid(_))
        ));
        assert!(seal_v2(&keyring, "nope", &keys, &ctx, 1, b"").is_err());
    }

    #[test]
    fn v2_versions_and_epochs() {
        let keys = DeviceKeys::generate().unwrap();
        let public = keys.public();
        let ctx = context("asset", "blob");
        let mut keyring = Keyring::new([1; KEY_LEN]);
        keyring.insert(2, [2; KEY_LEN]);
        assert_eq!(keyring.newest().0, 2);
        let sealed = seal_v2(&keyring, DEVICE, &keys, &ctx, 1, b"x").unwrap();
        assert_eq!(header_v2(&sealed).unwrap().epoch, 2);
        assert_eq!(open_v2(&keyring, &public, &ctx, &sealed).unwrap(), b"x");
        // Signed correctly, but this keyring lacks epoch 2.
        let old = Keyring::new([1; KEY_LEN]);
        verify_v2(&sealed, &public, &ctx).unwrap();
        assert!(matches!(
            open_v2(&old, &public, &ctx, &sealed),
            Err(Error::Unsupported(_))
        ));
        // Envelopes of epoch 1 still open with the newer keyring.
        let first = seal_v2(&old, DEVICE, &keys, &ctx, 1, b"y").unwrap();
        assert_eq!(open_v2(&keyring, &public, &ctx, &first).unwrap(), b"y");
        // Other versions: folder sync's 3 and an unknown 5.
        let v3 = Sealer::new(PROJECT, &[1; KEY_LEN])
            .seal(Kind::Asset, "blob", 1, &[0; 100])
            .unwrap();
        assert!(matches!(header_v2(&v3), Err(Error::Unsupported(_))));
        let mut v5 = sealed.clone();
        v5[0] = 5;
        assert!(matches!(
            verify_v2(&v5, &public, &ctx),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            open_v2(&keyring, &public, &ctx, &v5),
            Err(Error::Unsupported(_))
        ));
        assert!(format!("{keyring:?}").contains("[1, 2]"));
    }
}
