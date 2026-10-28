//! What devices, the sync server and later the browser client share, without
//! Loro: device keys ([`keys`]), sealed envelopes ([`envelope`]), the signed
//! members log of a project ([`members`]), invite links ([`invite`]), the
//! record framing of logs ([`frame`]) and the sync server's messages
//! ([`wire`]).
//!
//! No storage and no network here: the crate only builds and checks bytes,
//! so the server can verify everything without reading any content.
//!
//! Ids (project, user, device, invite) are 32 lowercase hex characters, 16
//! random bytes.
//!
//! # Versions
//!
//! Separately updated copies (apps, the server) meet on the wire, in the
//! members log and in synced records. Three numbers keep them apart: the
//! wire's protocol version ([`wire::VERSION`]; a server serves a range), the
//! feature level ([`LEVEL`]: what synced data a build understands, raised by
//! one per feature that needs it; each record and snapshot names the level it
//! needs, a project its minimum in the members log) and the app version (for
//! people and bug reports only, never for compatibility). A client never
//! moves past data it can't understand and never writes data an older
//! client would get wrong.

pub mod envelope;
pub mod frame;
pub mod invite;
pub mod keys;
pub mod members;
pub mod wire;

/// The feature level this build understands (see "Versions"): 1 is the
/// feature set of the first release (`main` and `review` documents).
/// Envelopes of versions 1 and 2, written before levels existed, count as
/// level 1.
pub const LEVEL: u32 = 1;

/// Errors returned by the protocol crate.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// Damaged data, or data that doesn't belong where it was found.
    #[error("corrupt data: {0}")]
    Corrupt(String),
    /// A version or epoch this version doesn't know.
    #[error("unsupported format: {0}")]
    Unsupported(String),
    /// A signature that doesn't verify.
    #[error("invalid signature: {0}")]
    Signature(String),
    /// A members log entry, correctly chained and signed, that this version
    /// can't read: written by a newer app (see [`members`]).
    #[error("needs a newer app: {0}")]
    Newer(String),
    /// A members log entry that breaks the rules (see [`members`]).
    #[error("rejected: {0}")]
    Rejected(String),
    /// Malformed input, e.g. an invite link or an id.
    #[error("invalid input: {0}")]
    Invalid(String),
    /// The system has no random numbers.
    #[error("{0}")]
    Random(String),
}

/// Result type of the protocol crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Length of an id in bytes (32 hex characters).
pub const ID_LEN: usize = 16;

/// `N` random bytes.
pub fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(|e| Error::Random(e.to_string()))?;
    Ok(bytes)
}

/// A new random id.
pub fn random_id() -> Result<String> {
    random::<ID_LEN>().map(|b| to_hex(&b))
}

/// `bytes` as lowercase hex.
pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The bytes of hex `s` (either case); `None` if it isn't hex.
pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// The `N` bytes of hex `s`; `None` if it isn't hex of that length.
pub fn from_hex_array<const N: usize>(s: &str) -> Option<[u8; N]> {
    from_hex(s)?.try_into().ok()
}

/// Checks that `s` is an id: 32 lowercase hex characters. `what` names it in
/// the error.
pub fn check_id(s: &str, what: &str) -> Result<()> {
    if s.len() == 2 * ID_LEN && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "{what} id {s:?} is not 32 lowercase hex characters"
        )))
    }
}

/// The 16 bytes of id `s` (see [`check_id`]).
pub fn id_bytes(s: &str, what: &str) -> Result<[u8; ID_LEN]> {
    check_id(s, what)?;
    Ok(from_hex_array(s).expect("checked"))
}

/// Serde for byte arrays as lowercase hex strings.
pub(crate) mod hex_array {
    use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

    pub fn serialize<S: Serializer, const N: usize>(
        bytes: &[u8; N],
        s: S,
    ) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::to_hex(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>, const N: usize>(
        d: D,
    ) -> Result<[u8; N], D::Error> {
        let s = String::deserialize(d)?;
        super::from_hex_array(&s)
            .ok_or_else(|| D::Error::custom(format!("expected {} hex characters", 2 * N)))
    }
}

/// Serde for byte strings as lowercase hex strings.
pub(crate) mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::to_hex(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        super::from_hex(&s).ok_or_else(|| D::Error::custom("expected hex"))
    }
}

/// Serde for optional byte strings as lowercase hex strings.
pub(crate) mod hex_bytes_opt {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match bytes {
            Some(bytes) => super::hex_bytes::serialize(bytes, s),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        #[derive(Deserialize)]
        struct Hex(#[serde(with = "super::hex_bytes")] Vec<u8>);
        Ok(Option::<Hex>::deserialize(d)?.map(|h| h.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_and_ids() {
        assert_eq!(to_hex(&[0, 0xab, 0x10]), "00ab10");
        assert_eq!(from_hex("00AB10"), Some(vec![0, 0xab, 0x10]));
        assert_eq!(from_hex("abc"), None);
        assert_eq!(from_hex("zz"), None);
        let id = random_id().unwrap();
        check_id(&id, "test").unwrap();
        assert_eq!(to_hex(&id_bytes(&id, "test").unwrap()), id);
        assert!(check_id(&id.to_uppercase(), "test").is_err());
        assert!(check_id(&id[1..], "test").is_err());
    }
}
