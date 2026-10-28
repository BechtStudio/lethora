//! Device keys: each device (an installation of the app) has an Ed25519 key
//! to sign and an X25519 key for key exchange (key epochs). The app keeps the
//! two 32-byte secrets in its config directory.
//!
//! Every signature covers a domain tag followed by the message, so a
//! signature made for one purpose (an envelope, a members log entry) can't be
//! passed off as another. Tags end in a zero byte. Verification is strict
//! (Ed25519 as in RFC 8032, rejecting weak public keys and non-canonical
//! signatures).
//!
//! # Sealed epoch keys
//!
//! A new key epoch's project key reaches each recipient (a device, or an
//! invite, see [`invite`](crate::invite)) sealed to its X25519 exchange key:
//! one random ephemeral X25519 key per epoch entry, and per recipient
//! HKDF-SHA256 over the shared secret (ephemeral × recipient; salt: the
//! ephemeral, then the recipient public key; info `lethora-epoch-1`) as the
//! XChaCha20-Poly1305 key. A sealed key is `[24 bytes nonce][32 bytes key +
//! 16 bytes tag]`, its associated data `lethora-epoch-1`, a zero byte, the
//! project id, a zero byte, the epoch (u32 LE), the recipient id and a zero
//! byte. Exchange keys of low order (the shared secret would be zero) are
//! refused ([`check_exchange`]).

use std::fmt;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

use crate::{Error, Result, from_hex_array, hex_array, random, to_hex};

/// Length of a secret or public key.
pub const KEY_LEN: usize = 32;
/// Length of an Ed25519 signature.
pub const SIGNATURE_LEN: usize = 64;
const NONCE_LEN: usize = 24;
/// Length of a sealed epoch key: nonce, key and tag.
pub const SEALED_KEY_LEN: usize = NONCE_LEN + KEY_LEN + 16;
/// HKDF info and first part of the associated data of sealed epoch keys.
const EPOCH_TAG: &[u8] = b"lethora-epoch-1";

/// Something that signs: a device or an invite.
pub trait Signer {
    /// The signature of `tag` followed by `message`.
    fn sign(&self, tag: &[u8], message: &[u8]) -> [u8; SIGNATURE_LEN];
    /// The Ed25519 public key that verifies its signatures.
    fn verifying_key(&self) -> [u8; KEY_LEN];
}

/// The signature of `tag` followed by `message` with `key`.
pub(crate) fn sign_with(key: &SigningKey, tag: &[u8], message: &[u8]) -> [u8; SIGNATURE_LEN] {
    key.sign(&[tag, message].concat()).to_bytes()
}

/// Checks that `signature` signs `tag` followed by `message` for Ed25519
/// public key `public`; [`Error::Signature`] if not.
pub fn verify(
    public: &[u8; KEY_LEN],
    tag: &[u8],
    message: &[u8],
    signature: &[u8; SIGNATURE_LEN],
) -> Result<()> {
    let key = VerifyingKey::from_bytes(public)
        .map_err(|_| Error::Signature("not a valid public key".into()))?;
    key.verify_strict(&[tag, message].concat(), &Signature::from_bytes(signature))
        .map_err(|_| Error::Signature("the signature doesn't match".into()))
}

/// Checks that `public` is a usable Ed25519 public key (a valid point, not
/// a weak one); [`Error::Signature`] if not.
pub fn check_public(public: &[u8; KEY_LEN]) -> Result<()> {
    match VerifyingKey::from_bytes(public) {
        Ok(key) if !key.is_weak() => Ok(()),
        _ => Err(Error::Signature("not a valid public key".into())),
    }
}

/// Checks that `public` is a usable X25519 public key (not of low order, so
/// a shared secret with it isn't zero); [`Error::Invalid`] if not.
pub fn check_exchange(public: &[u8; KEY_LEN]) -> Result<()> {
    // A low-order point gives zero with every (clamped) secret.
    let probe = StaticSecret::from([1; KEY_LEN]);
    if probe
        .diffie_hellman(&PublicKey::from(*public))
        .was_contributory()
    {
        Ok(())
    } else {
        Err(Error::Invalid("not a valid exchange key".into()))
    }
}

/// A project key sealed to one recipient (see the module docs); in JSON as
/// lowercase hex.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SealedKey(#[serde(with = "hex_array")] pub [u8; SEALED_KEY_LEN]);

/// The cipher of a sealed epoch key: `secret` and `other` are the two sides
/// of the exchange, `ephemeral` and `recipient` the two public keys.
fn epoch_cipher(
    secret: &StaticSecret,
    other: &[u8; KEY_LEN],
    ephemeral: &[u8; KEY_LEN],
    recipient: &[u8; KEY_LEN],
) -> Result<XChaCha20Poly1305> {
    let shared = secret.diffie_hellman(&PublicKey::from(*other));
    if !shared.was_contributory() {
        return Err(Error::Invalid("not a valid exchange key".into()));
    }
    let salt = [&ephemeral[..], &recipient[..]].concat();
    let mut key = [0; KEY_LEN];
    Hkdf::<Sha256>::new(Some(&salt), shared.as_bytes())
        .expand(EPOCH_TAG, &mut key)
        .expect("32 bytes");
    Ok(XChaCha20Poly1305::new(&Key::from(key)))
}

/// The associated data of the key of `epoch` sealed to `recipient`.
fn epoch_ad(project: &str, epoch: u32, recipient: &str) -> Vec<u8> {
    [
        EPOCH_TAG,
        b"\0",
        project.as_bytes(),
        b"\0",
        &epoch.to_le_bytes(),
        recipient.as_bytes(),
        b"\0",
    ]
    .concat()
}

/// Seals the project key of a new epoch to its recipients with one new
/// ephemeral X25519 key (see the module docs).
pub struct EpochSealer {
    secret: StaticSecret,
    public: [u8; KEY_LEN],
}

impl EpochSealer {
    /// A sealer with a new random ephemeral key.
    pub fn new() -> Result<Self> {
        let secret = StaticSecret::from(random::<KEY_LEN>()?);
        let public = PublicKey::from(&secret).to_bytes();
        Ok(Self { secret, public })
    }

    /// The ephemeral public key (the `ephemeral` of the epoch entry).
    pub fn public(&self) -> [u8; KEY_LEN] {
        self.public
    }

    /// `key`, the project key of `epoch` in `project`, sealed to `recipient`
    /// (a device or invite id) with X25519 public key `exchange`.
    pub fn seal(
        &self,
        project: &str,
        epoch: u32,
        recipient: &str,
        exchange: &[u8; KEY_LEN],
        key: &[u8; KEY_LEN],
    ) -> Result<SealedKey> {
        let cipher = epoch_cipher(&self.secret, exchange, &self.public, exchange)?;
        let nonce: [u8; NONCE_LEN] = random()?;
        let sealed = cipher
            .encrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: key,
                    aad: &epoch_ad(project, epoch, recipient),
                },
            )
            .map_err(|_| Error::Corrupt("can't seal the epoch key".into()))?;
        let bytes = [&nonce[..], &sealed].concat();
        Ok(SealedKey(bytes.try_into().expect("72 bytes")))
    }
}

/// The project key of `epoch` in `project` in `sealed`, sealed to
/// `recipient`, whose exchange secret is `secret`, with ephemeral key
/// `ephemeral`; [`Error::Corrupt`] if it doesn't open.
pub(crate) fn open_epoch_key(
    secret: &StaticSecret,
    project: &str,
    epoch: u32,
    recipient: &str,
    ephemeral: &[u8; KEY_LEN],
    sealed: &SealedKey,
) -> Result<[u8; KEY_LEN]> {
    let own = PublicKey::from(secret).to_bytes();
    let cipher = epoch_cipher(secret, ephemeral, ephemeral, &own)?;
    let (nonce, sealed) = sealed.0.split_at(NONCE_LEN);
    let nonce: [u8; NONCE_LEN] = nonce.try_into().expect("24 bytes");
    let key = cipher
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: sealed,
                aad: &epoch_ad(project, epoch, recipient),
            },
        )
        .map_err(|_| Error::Corrupt(format!("the key of epoch {epoch} doesn't open here")))?;
    Ok(key.try_into().expect("32 bytes"))
}

/// The secret keys of this device. `Debug` shows only the public keys.
#[derive(Clone)]
pub struct DeviceKeys {
    sign: SigningKey,
    exchange: StaticSecret,
}

impl DeviceKeys {
    /// New random keys.
    pub fn generate() -> Result<Self> {
        Ok(Self::from_secrets(random()?, random()?))
    }

    /// The keys of the two secrets (as returned by [`DeviceKeys::secrets`]).
    pub fn from_secrets(sign: [u8; KEY_LEN], exchange: [u8; KEY_LEN]) -> Self {
        Self {
            sign: SigningKey::from_bytes(&sign),
            exchange: StaticSecret::from(exchange),
        }
    }

    /// The signing and the exchange secret, to store.
    pub fn secrets(&self) -> ([u8; KEY_LEN], [u8; KEY_LEN]) {
        (self.sign.to_bytes(), self.exchange.to_bytes())
    }

    /// The keys of the two secrets as hex (as in `device.json`);
    /// [`Error::Invalid`] if either isn't 64 hex characters.
    pub fn from_hex(sign: &str, exchange: &str) -> Result<Self> {
        let key = |s: &str| {
            from_hex_array(s)
                .ok_or_else(|| Error::Invalid("a device key is not 64 hex characters".into()))
        };
        Ok(Self::from_secrets(key(sign)?, key(exchange)?))
    }

    /// The signing and the exchange secret as lowercase hex.
    pub fn to_hex(&self) -> (String, String) {
        let (sign, exchange) = self.secrets();
        (to_hex(&sign), to_hex(&exchange))
    }

    /// The public keys, for others to verify and to wrap keys to.
    pub fn public(&self) -> DevicePublic {
        DevicePublic {
            sign: self.sign.verifying_key().to_bytes(),
            exchange: PublicKey::from(&self.exchange).to_bytes(),
        }
    }

    /// The project key of `epoch` in `project` sealed to this device (id
    /// `device`) in an epoch entry with ephemeral key `ephemeral` (see the
    /// module docs).
    pub fn open_epoch_key(
        &self,
        project: &str,
        epoch: u32,
        device: &str,
        ephemeral: &[u8; KEY_LEN],
        sealed: &SealedKey,
    ) -> Result<[u8; KEY_LEN]> {
        open_epoch_key(&self.exchange, project, epoch, device, ephemeral, sealed)
    }
}

impl Signer for DeviceKeys {
    fn sign(&self, tag: &[u8], message: &[u8]) -> [u8; SIGNATURE_LEN] {
        sign_with(&self.sign, tag, message)
    }

    fn verifying_key(&self) -> [u8; KEY_LEN] {
        self.sign.verifying_key().to_bytes()
    }
}

impl fmt::Debug for DeviceKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceKeys")
            .field("public", &self.public())
            .finish_non_exhaustive()
    }
}

/// The public keys of a device; in JSON as lowercase hex strings.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicePublic {
    /// Ed25519: verifies the device's signatures.
    #[serde(with = "hex_array")]
    pub sign: [u8; KEY_LEN],
    /// X25519: key exchange.
    #[serde(with = "hex_array")]
    pub exchange: [u8; KEY_LEN],
}

impl DevicePublic {
    /// Checks a signature of this device (see [`verify`]).
    pub fn verify(
        &self,
        tag: &[u8],
        message: &[u8],
        signature: &[u8; SIGNATURE_LEN],
    ) -> Result<()> {
        verify(&self.sign, tag, message, signature)
    }
}

impl fmt::Debug for DevicePublic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DevicePublic")
            .field("sign", &to_hex(&self.sign))
            .field("exchange", &to_hex(&self.exchange))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip_and_sign() {
        let keys = DeviceKeys::generate().unwrap();
        let (sign, exchange) = keys.to_hex();
        assert_eq!((sign.len(), exchange.len()), (64, 64));
        let again = DeviceKeys::from_hex(&sign, &exchange).unwrap();
        assert_eq!(again.public(), keys.public());
        assert_eq!(
            DeviceKeys::from_secrets(keys.secrets().0, keys.secrets().1).public(),
            keys.public()
        );
        assert!(DeviceKeys::from_hex(&sign[2..], &exchange).is_err());

        let public = keys.public();
        let signature = keys.sign(b"tag\0", b"message");
        public.verify(b"tag\0", b"message", &signature).unwrap();
        assert!(public.verify(b"tag2\0", b"message", &signature).is_err());
        assert!(public.verify(b"tag\0", b"massage", &signature).is_err());
        let other = DeviceKeys::generate().unwrap().public();
        assert!(other.verify(b"tag\0", b"message", &signature).is_err());

        let json = serde_json::to_string(&public).unwrap();
        assert_eq!(serde_json::from_str::<DevicePublic>(&json).unwrap(), public);
        let debug = format!("{keys:?}");
        assert!(!debug.contains(&sign) && !debug.contains(&exchange));
    }

    #[test]
    fn epoch_keys_open_only_for_their_recipient_project_and_epoch() {
        const PROJECT: &str = "00112233445566778899aabbccddeeff";
        const DEVICE: &str = "0123456789abcdef0123456789abcdef";
        let device = DeviceKeys::generate().unwrap();
        let other = DeviceKeys::generate().unwrap();
        let key = [42; KEY_LEN];
        let sealer = EpochSealer::new().unwrap();
        let ephemeral = sealer.public();
        let sealed = sealer
            .seal(PROJECT, 2, DEVICE, &device.public().exchange, &key)
            .unwrap();
        let open = |keys: &DeviceKeys, project: &str, epoch, id: &str, sealed: &SealedKey| {
            keys.open_epoch_key(project, epoch, id, &ephemeral, sealed)
        };
        assert_eq!(open(&device, PROJECT, 2, DEVICE, &sealed).unwrap(), key);
        assert!(open(&other, PROJECT, 2, DEVICE, &sealed).is_err());
        assert!(open(&device, PROJECT, 3, DEVICE, &sealed).is_err());
        assert!(open(&device, DEVICE, 2, DEVICE, &sealed).is_err());
        assert!(open(&device, PROJECT, 2, PROJECT, &sealed).is_err());
        let mut damaged = sealed;
        damaged.0[30] ^= 1;
        assert!(open(&device, PROJECT, 2, DEVICE, &damaged).is_err());
        // Another ephemeral key: another shared secret.
        let again = EpochSealer::new().unwrap().public();
        assert!(
            device
                .open_epoch_key(PROJECT, 2, DEVICE, &again, &sealed)
                .is_err()
        );
        let json = serde_json::to_string(&sealed).unwrap();
        assert_eq!(json.len(), 2 + 2 * SEALED_KEY_LEN);
        assert_eq!(serde_json::from_str::<SealedKey>(&json).unwrap(), sealed);

        // Low-order exchange keys are refused.
        check_exchange(&device.public().exchange).unwrap();
        assert!(check_exchange(&[0; KEY_LEN]).is_err());
        let mut one = [0; KEY_LEN];
        one[0] = 1;
        assert!(check_exchange(&one).is_err());
        assert!(
            sealer
                .seal(PROJECT, 2, DEVICE, &[0; KEY_LEN], &key)
                .is_err()
        );
    }
}
