//! Invite links: `https://<host>/join/<project id>/<invite id>#<secret>`,
//! the secret 32 random bytes in base64url without padding. The part after
//! `#` never reaches the server; the project id tells the joining device
//! which project to say hello to. The host may include a port.
//!
//! Server URLs (in links, and the public URL signed in hellos, see
//! [`server_origin`]) are origins: `https://host[:port]`. Plain `http` (or
//! `ws`, read as `http`) is allowed only for hosts on this machine or the
//! local network: `localhost`, `*.localhost` and `*.local` names, IPv4
//! loopback (127/8), private (10/8, 172.16/12, 192.168/16) and link-local
//! (169.254/16) addresses, IPv6 loopback (`[::1]`), unique local (fc00::/7)
//! and link-local (fe80::/10) addresses. `wss` reads as `https`.
//!
//! From the secret, HKDF-SHA256 (salt: the invite id as ASCII) derives the
//! invite's Ed25519 signing key (info `lethora-invite-1 sign`; its public key
//! goes into the `invite` entry of the members log and the joining device
//! signs its `join` entry with it), an X25519 exchange key (info
//! `lethora-invite-1 exchange`; its public key goes into the `invite` entry
//! too, so key epochs started while the invite is open reach it, see
//! [`keys`](crate::keys)) and a wrap key (info `lethora-invite-1 wrap`). The
//! wrap key seals the project's keyring at invite time for the server to
//! keep by invite id (never in the members log): `[u8 version = 2][24 bytes
//! nonce][ciphertext + 16 bytes tag]`, the plaintext per epoch, oldest
//! first, `[u32 LE epoch][32 bytes key]`; XChaCha20-Poly1305 with associated
//! data the version byte, then `lethora-invite-key-1`, the project id and
//! the invite id, each followed by a zero byte.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use ed25519_dalek::SigningKey;
use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

use crate::envelope::Keyring;
use crate::keys::{KEY_LEN, SIGNATURE_LEN, SealedKey, Signer, open_epoch_key, sign_with};
use crate::{Error, Result, check_id, random, random_id};

/// Bytes of an invite secret.
pub const SECRET_LEN: usize = 32;
/// Characters of an invite secret in base64url without padding.
const SECRET_CHARS: usize = 43;
const SIGN_INFO: &[u8] = b"lethora-invite-1 sign";
const EXCHANGE_INFO: &[u8] = b"lethora-invite-1 exchange";
const WRAP_INFO: &[u8] = b"lethora-invite-1 wrap";
const WRAP_VERSION: u8 = 2;
const WRAP_TAG: &str = "lethora-invite-key-1";
const NONCE_LEN: usize = 24;
/// Bytes of one epoch in a wrapped keyring.
const EPOCH_LEN: usize = 4 + KEY_LEN;
/// Bytes of a wrapped keyring besides its epochs.
const WRAPPED_OVERHEAD: usize = 1 + NONCE_LEN + 16;
/// The base64url alphabet.
const BASE64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// An invite link (see the module docs). [`Display`](fmt::Display) gives the
/// link; `Debug` never shows the secret.
#[derive(Clone, PartialEq, Eq)]
pub struct InviteLink {
    /// The server's origin, e.g. `https://lethora.example:8443`.
    pub server: String,
    /// The project id (32 lowercase hex).
    pub project: String,
    /// The invite id (32 lowercase hex).
    pub id: String,
    secret: [u8; SECRET_LEN],
}

impl InviteLink {
    /// A new invite with a random id and secret for `project` on `server`
    /// (an origin like `https://lethora.example`, see [`server_origin`]).
    pub fn generate(server: &str, project: &str) -> Result<Self> {
        check_id(project, "project")?;
        Ok(Self {
            server: server_origin(server)?,
            project: project.to_owned(),
            id: random_id()?,
            secret: random()?,
        })
    }

    /// Reads a link as pasted (surrounding whitespace doesn't matter);
    /// [`Error::Invalid`] if it isn't an invite link.
    pub fn parse(link: &str) -> Result<Self> {
        let invalid = |why: &str| Error::Invalid(format!("not an invite link: {why}"));
        let link = link.trim();
        let (server, rest) = match link.find("://") {
            Some(i) => match link[i + 3..].find('/') {
                Some(j) => link.split_at(i + 3 + j),
                None => return Err(invalid("it has no path")),
            },
            None => return Err(invalid("it doesn't start with https://")),
        };
        let server = server_origin(server)?;
        let (project, id, secret) = rest
            .strip_prefix("/join/")
            .and_then(|rest| rest.split_once('/'))
            .and_then(|(project, rest)| {
                let (id, secret) = rest.split_once('#')?;
                Some((project, id, secret))
            })
            .ok_or_else(|| invalid("it doesn't end in /join/<project>/<id>#<secret>"))?;
        check_id(project, "project")?;
        check_id(id, "invite")?;
        let secret =
            secret_from_base64url(secret).ok_or_else(|| invalid("the part after # is damaged"))?;
        Ok(Self {
            server,
            project: project.to_owned(),
            id: id.to_owned(),
            secret,
        })
    }

    /// The secret (the part after `#`).
    pub fn secret(&self) -> &[u8; SECRET_LEN] {
        &self.secret
    }

    /// The keys derived from the secret.
    pub fn keys(&self) -> InviteKeys {
        InviteKeys::derive(&self.id, &self.secret).expect("the id was checked")
    }
}

impl fmt::Display for InviteLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}/join/{}/{}#{}",
            self.server,
            self.project,
            self.id,
            to_base64url(&self.secret)
        )
    }
}

impl fmt::Debug for InviteLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InviteLink")
            .field("server", &self.server)
            .field("project", &self.project)
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// Checks server URL `s` (surrounding whitespace and a trailing `/` don't
/// matter) and returns it as an origin with the scheme and host in
/// lowercase: `https://host[:port]`, or `http://` for a host on this machine
/// or the local network (see the module docs); `ws` and `wss` read as `http`
/// and `https`. The same rule for links and the URL signed in hellos.
pub fn server_origin(s: &str) -> Result<String> {
    let s = s.trim();
    let s = s.strip_suffix('/').unwrap_or(s);
    let invalid = |why: &str| Error::Invalid(format!("server {s:?}: {why}"));
    let (scheme, host) = s
        .split_once("://")
        .ok_or_else(|| invalid("it doesn't start with https://"))?;
    let scheme = match scheme.to_ascii_lowercase().as_str() {
        "https" | "wss" => "https",
        "http" | "ws" => "http",
        _ => return Err(invalid("it doesn't start with https://")),
    };
    let host = host.to_ascii_lowercase();
    let (name, port) = match host.strip_prefix('[') {
        Some(rest) => {
            let end = rest.find(']').ok_or_else(|| invalid("unclosed ["))?;
            let ip = &rest[..end];
            if ip.is_empty() || !ip.bytes().all(|b| b.is_ascii_hexdigit() || b == b':') {
                return Err(invalid("bad IPv6 address"));
            }
            (&host[..end + 2], &host[end + 2..])
        }
        None => {
            let end = host.find(':').unwrap_or(host.len());
            let name = &host[..end];
            let ok = |b: u8| b.is_ascii_alphanumeric() || b == b'-' || b == b'.';
            if name.is_empty() || !name.bytes().all(ok) {
                return Err(invalid("bad host name"));
            }
            (name, &host[end..])
        }
    };
    if !port.is_empty() {
        let digits = port.strip_prefix(':').unwrap_or("x");
        if !digits.bytes().all(|b| b.is_ascii_digit()) || digits.parse::<u16>().is_err() {
            return Err(invalid("bad port"));
        }
    }
    if scheme == "http" && !is_local_host(name) {
        return Err(invalid(
            "plain http is only allowed for this machine and the local network",
        ));
    }
    Ok(format!("{scheme}://{host}"))
}

/// Whether host `name` (lowercase; an IPv6 address in brackets) is on this
/// machine or the local network (see the module docs).
pub fn is_local_host(name: &str) -> bool {
    if let Some(ip) = name.strip_prefix('[').and_then(|n| n.strip_suffix(']')) {
        return ip.parse::<Ipv6Addr>().is_ok_and(|ip| {
            let first = ip.segments()[0];
            ip.is_loopback() || first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80
        });
    }
    if let Ok(ip) = name.parse::<Ipv4Addr>() {
        return ip.is_loopback() || ip.is_private() || ip.is_link_local();
    }
    name == "localhost" || name.ends_with(".localhost") || name.ends_with(".local")
}

/// `bytes` in base64url without padding.
fn to_base64url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let (mut acc, mut bits) = (0u32, 0);
    for &b in bytes {
        acc = (acc << 8 | u32::from(b)) & 0xffff;
        bits += 8;
        while bits >= 6 {
            bits -= 6;
            out.push(char::from(BASE64URL[(acc >> bits & 63) as usize]));
        }
    }
    if bits > 0 {
        out.push(char::from(BASE64URL[(acc << (6 - bits) & 63) as usize]));
    }
    out
}

/// The secret in base64url `s` (43 characters, canonical, no padding).
fn secret_from_base64url(s: &str) -> Option<[u8; SECRET_LEN]> {
    if s.len() != SECRET_CHARS {
        return None;
    }
    let mut out = Vec::with_capacity(SECRET_LEN + 1);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let digit = BASE64URL.iter().position(|&d| d == c)?;
        acc = (acc << 6 | digit as u32) & 0xffff;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    let secret: [u8; SECRET_LEN] = out.try_into().ok()?;
    // Unused low bits must be zero: one secret, one spelling.
    (to_base64url(&secret) == s).then_some(secret)
}

/// The keys of an invite, derived from its secret (see the module docs).
/// `Debug` shows only the public key.
#[derive(Clone)]
pub struct InviteKeys {
    id: String,
    sign: SigningKey,
    exchange: StaticSecret,
    wrap: [u8; KEY_LEN],
}

impl InviteKeys {
    /// The keys of invite `id` with `secret`.
    pub fn derive(id: &str, secret: &[u8; SECRET_LEN]) -> Result<Self> {
        check_id(id, "invite")?;
        let hkdf = Hkdf::<Sha256>::new(Some(id.as_bytes()), secret);
        let mut sign = [0; KEY_LEN];
        let mut exchange = [0; KEY_LEN];
        let mut wrap = [0; KEY_LEN];
        hkdf.expand(SIGN_INFO, &mut sign).expect("32 bytes");
        hkdf.expand(EXCHANGE_INFO, &mut exchange).expect("32 bytes");
        hkdf.expand(WRAP_INFO, &mut wrap).expect("32 bytes");
        Ok(Self {
            id: id.to_owned(),
            sign: SigningKey::from_bytes(&sign),
            exchange: StaticSecret::from(exchange),
            wrap,
        })
    }

    /// The invite id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The X25519 public key (the `exchange` of the `invite` entry).
    pub fn exchange_key(&self) -> [u8; KEY_LEN] {
        PublicKey::from(&self.exchange).to_bytes()
    }

    /// The project key of `epoch` in `project` sealed to this invite in an
    /// epoch entry with ephemeral key `ephemeral` (see
    /// [`keys`](crate::keys)).
    pub fn open_epoch_key(
        &self,
        project: &str,
        epoch: u32,
        ephemeral: &[u8; KEY_LEN],
        sealed: &SealedKey,
    ) -> Result<[u8; KEY_LEN]> {
        open_epoch_key(&self.exchange, project, epoch, &self.id, ephemeral, sealed)
    }

    /// The associated data of a wrapped key of `project`.
    fn ad(&self, project: &str) -> Vec<u8> {
        let mut ad = vec![WRAP_VERSION];
        for part in [WRAP_TAG, project, &self.id] {
            ad.extend_from_slice(part.as_bytes());
            ad.push(0);
        }
        ad
    }

    /// `keyring`, the project keys of `project`, sealed with the wrap key.
    pub fn wrap(&self, project: &str, keyring: &Keyring) -> Result<Vec<u8>> {
        check_id(project, "project")?;
        let plain: Vec<u8> = keyring
            .epochs()
            .flat_map(|(epoch, key)| [&epoch.to_le_bytes()[..], key].concat())
            .collect();
        let nonce: [u8; NONCE_LEN] = random()?;
        let sealed = XChaCha20Poly1305::new(&Key::from(self.wrap))
            .encrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: &plain,
                    aad: &self.ad(project),
                },
            )
            .map_err(|_| Error::Corrupt("can't wrap the project keys".into()))?;
        Ok([&[WRAP_VERSION][..], &nonce, &sealed].concat())
    }

    /// The project keys of `project` in `wrapped`; [`Error::Corrupt`] if
    /// they don't open (another secret, project or invite, or damaged).
    pub fn unwrap(&self, project: &str, wrapped: &[u8]) -> Result<Keyring> {
        let damaged = || Error::Corrupt("the wrapped project keys are damaged".into());
        match wrapped.first() {
            Some(&WRAP_VERSION)
                if wrapped.len() > WRAPPED_OVERHEAD
                    && (wrapped.len() - WRAPPED_OVERHEAD).is_multiple_of(EPOCH_LEN) => {}
            Some(&WRAP_VERSION) | None => return Err(damaged()),
            Some(version) => {
                return Err(Error::Unsupported(format!(
                    "wrapped key version {version}; update Lethora to read it"
                )));
            }
        }
        let nonce: [u8; NONCE_LEN] = wrapped[1..1 + NONCE_LEN].try_into().expect("24 bytes");
        let plain = XChaCha20Poly1305::new(&Key::from(self.wrap))
            .decrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: &wrapped[1 + NONCE_LEN..],
                    aad: &self.ad(project),
                },
            )
            .map_err(|_| {
                Error::Corrupt("the wrapped project keys don't open with this invite".into())
            })?;
        let epochs: Vec<(u32, [u8; KEY_LEN])> = plain
            .as_chunks::<EPOCH_LEN>()
            .0
            .iter()
            .map(|e| {
                let epoch = u32::from_le_bytes([e[0], e[1], e[2], e[3]]);
                (epoch, e[4..].try_into().expect("32 bytes"))
            })
            .collect();
        if epochs.windows(2).any(|w| w[0].0 >= w[1].0) || epochs[0].0 == 0 {
            return Err(damaged());
        }
        Keyring::from_epochs(epochs).ok_or_else(damaged)
    }
}

impl Signer for InviteKeys {
    fn sign(&self, tag: &[u8], message: &[u8]) -> [u8; SIGNATURE_LEN] {
        sign_with(&self.sign, tag, message)
    }

    fn verifying_key(&self) -> [u8; KEY_LEN] {
        self.sign.verifying_key().to_bytes()
    }
}

impl fmt::Debug for InviteKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InviteKeys")
            .field("id", &self.id)
            .field("public", &crate::to_hex(&self.verifying_key()))
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::verify;

    const PROJECT: &str = "00112233445566778899aabbccddeeff";

    #[test]
    fn plain_http_only_on_this_machine_and_the_local_network() {
        for local in [
            "http://localhost",
            "http://lethora.localhost:8080",
            "http://nas.local",
            "http://127.0.0.1:1420",
            "http://127.8.9.10",
            "http://10.0.0.5",
            "http://172.16.0.1",
            "http://172.31.255.254",
            "http://192.168.178.20:8080",
            "http://169.254.10.10",
            "http://[::1]",
            "http://[fd12:3456::1]:80",
            "http://[fc00::1]",
            "http://[fe80::1]",
        ] {
            assert_eq!(server_origin(local).unwrap(), local, "{local}");
        }
        for public in [
            "http://lethora.example",
            "http://172.32.0.1",
            "http://172.15.255.255",
            "http://192.169.0.1",
            "http://8.8.8.8",
            "http://[2001:db8::1]",
            "http://[ff02::1]",
            "http://local",
            "http://example.localdomain",
        ] {
            assert!(server_origin(public).is_err(), "{public}");
            let https = public.replacen("http://", "https://", 1);
            assert_eq!(server_origin(&https).unwrap(), https);
        }
        assert_eq!(
            server_origin(" WS://192.168.1.2:9000/ ").unwrap(),
            "http://192.168.1.2:9000"
        );
        assert_eq!(
            server_origin("wss://Lethora.Example").unwrap(),
            "https://lethora.example"
        );
        assert!(server_origin("ftp://localhost").is_err());
        assert!(server_origin("https://lethora.example/path").is_err());
    }

    #[test]
    fn link_round_trip() {
        for server in [
            "https://lethora.example",
            "https://lethora.example:8443",
            "http://localhost:1420",
            "http://127.0.0.1",
            "http://[::1]:80",
            "https://[2001:db8::1]",
            "http://192.168.1.20:8080",
        ] {
            let link = InviteLink::generate(server, PROJECT).unwrap();
            let text = link.to_string();
            assert!(text.starts_with(&format!("{server}/join/{PROJECT}/{}#", link.id)));
            assert_eq!(text.len(), server.len() + 6 + 33 + 32 + 1 + SECRET_CHARS);
            assert_eq!(InviteLink::parse(&text).unwrap(), link);
            assert_eq!(InviteLink::parse(&format!("  {text}\n")).unwrap(), link);
            assert!(!format!("{link:?}").contains(&text[text.len() - SECRET_CHARS..]));
        }
        let link = InviteLink::parse(
            "HTTPS://Lethora.Example/join/00112233445566778899aabbccddeeff/\
             0123456789abcdef0123456789abcdef\
             #AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8",
        )
        .unwrap();
        assert_eq!(link.server, "https://lethora.example");
        assert_eq!(link.project, PROJECT);
        assert_eq!(link.secret(), &std::array::from_fn(|i| i as u8));
    }

    #[test]
    fn bad_links() {
        let good = InviteLink::generate("https://lethora.example", PROJECT)
            .unwrap()
            .to_string();
        let (base, secret) = good.split_once('#').unwrap();
        let id = &base[base.len() - 32..];
        let bad = [
            String::new(),
            "lethora.example/join/x#y".into(),
            good.replace("https://", "http://"),
            good.replace("https://", "ftp://"),
            good.replace("/join/", "/joint/"),
            good.replace("lethora.example", "lethora.example:99999"),
            good.replace("lethora.example", "user@lethora.example"),
            good.replace("lethora.example", ""),
            good.replace(id, &id.to_uppercase()),
            good.replace(id, &id[1..]),
            good.replace(&format!("{PROJECT}/"), ""),
            good.replace(PROJECT, &PROJECT[1..]),
            format!("{base}/#{secret}"),
            format!("{base}?x#{secret}"),
            base.to_owned(),
            format!("{base}#{}", &secret[1..]),
            format!("{base}#{secret}A"),
            format!("{base}#{}=", &secret[..42]),
            format!("{base}#{}+", &secret[..42]),
            // The last character's unused low bits set.
            format!("{base}#AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh9"),
        ];
        for link in bad {
            assert!(InviteLink::parse(&link).is_err(), "{link}");
        }
    }

    #[test]
    fn derivation_and_wrapping() {
        let id = "0123456789abcdef0123456789abcdef";
        let other_id = "fedcba9876543210fedcba9876543210";
        let project = "00112233445566778899aabbccddeeff";
        let secret = [5; SECRET_LEN];
        let keys = InviteKeys::derive(id, &secret).unwrap();
        let again = InviteKeys::derive(id, &secret).unwrap();
        assert_eq!(keys.verifying_key(), again.verifying_key());
        assert_eq!(keys.wrap, again.wrap);
        assert_ne!(keys.wrap, keys.sign.to_bytes());
        let other_secret = InviteKeys::derive(id, &[6; SECRET_LEN]).unwrap();
        let other_invite = InviteKeys::derive(other_id, &secret).unwrap();
        assert_ne!(other_secret.verifying_key(), keys.verifying_key());
        assert_ne!(other_invite.verifying_key(), keys.verifying_key());
        assert!(InviteKeys::derive("nope", &secret).is_err());

        let signature = keys.sign(b"tag\0", b"join");
        verify(&keys.verifying_key(), b"tag\0", b"join", &signature).unwrap();

        assert_eq!(keys.exchange_key(), again.exchange_key());
        assert_ne!(keys.exchange_key(), other_secret.exchange_key());
        crate::keys::check_exchange(&keys.exchange_key()).unwrap();

        // The whole keyring travels.
        let mut keyring = Keyring::new([42; KEY_LEN]);
        keyring.insert(2, [43; KEY_LEN]);
        keyring.insert(5, [44; KEY_LEN]);
        let wrapped = keys.wrap(project, &keyring).unwrap();
        assert_eq!(wrapped.len(), WRAPPED_OVERHEAD + 3 * EPOCH_LEN);
        let opened = again.unwrap(project, &wrapped).unwrap();
        assert_eq!(
            opened.epochs().collect::<Vec<_>>(),
            keyring.epochs().collect::<Vec<_>>()
        );
        assert!(other_secret.unwrap(project, &wrapped).is_err());
        assert!(other_invite.unwrap(project, &wrapped).is_err());
        assert!(keys.unwrap(other_id, &wrapped).is_err());
        assert!(keys.unwrap(project, &wrapped[..wrapped.len() - 1]).is_err());
        assert!(keys.unwrap(project, &wrapped[..WRAPPED_OVERHEAD]).is_err());
        let mut damaged = wrapped.clone();
        damaged[30] ^= 1;
        assert!(keys.unwrap(project, &damaged).is_err());
        let mut newer = wrapped;
        newer[0] = 3;
        assert!(matches!(
            keys.unwrap(project, &newer),
            Err(Error::Unsupported(_))
        ));
        let one = keys.wrap(project, &Keyring::new([1; KEY_LEN])).unwrap();
        assert_eq!(
            keys.unwrap(project, &one).unwrap().newest(),
            (1, &[1; KEY_LEN])
        );

        // An epoch key sealed to the invite opens with its keys only.
        let sealer = crate::keys::EpochSealer::new().unwrap();
        let sealed = sealer
            .seal(project, 2, id, &keys.exchange_key(), &[9; KEY_LEN])
            .unwrap();
        let ephemeral = sealer.public();
        assert_eq!(
            again
                .open_epoch_key(project, 2, &ephemeral, &sealed)
                .unwrap(),
            [9; KEY_LEN]
        );
        assert!(
            other_invite
                .open_epoch_key(project, 2, &ephemeral, &sealed)
                .is_err()
        );

        let link = InviteLink::generate("https://lethora.example", PROJECT).unwrap();
        assert_eq!(
            link.keys().verifying_key(),
            InviteKeys::derive(&link.id, link.secret())
                .unwrap()
                .verifying_key()
        );
    }
}
