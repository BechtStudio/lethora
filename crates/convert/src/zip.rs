//! A small zip reader and writer on `flate2`, for the zip-based formats
//! (DOCX, EPUB): stored and deflated entries, no zip64, no encryption.
//!
//! The reader takes the central directory as the truth (local headers only
//! for where the data starts), reads entries lazily by name and checks each
//! one's CRC-32. Against zip bombs it caps the number of entries, every
//! entry's size and their total, and the compression ratio; an entry that
//! inflates to more or less than it says is an error. The writer writes
//! entries in the order added (EPUB's `mimetype` first and stored) with a
//! fixed date, so the same input gives the same bytes.

use std::io::{Read, Write};

use flate2::Crc;

use crate::{Error, MAX_INPUT_BYTES, Result};

/// The most entries an archive may have.
pub const MAX_ENTRIES: usize = 20_000;
/// The largest an entry may inflate to.
pub const MAX_ENTRY_BYTES: u64 = 256 * 1024 * 1024;
/// The largest all entries together may inflate to.
pub const MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
/// The highest compression ratio an entry may have (beyond its first MiB;
/// deflate reaches about 1000:1, XML about 10:1 to 50:1).
pub const MAX_RATIO: u64 = 200;

const LOCAL: u32 = 0x0403_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const END: u32 = 0x0605_4b50;
const END64_LOCATOR: u32 = 0x0706_4b50;
const STORED: u16 = 0;
const DEFLATE: u16 = 8;
/// General purpose flags: encrypted, names in UTF-8.
const ENCRYPTED: u16 = 1;
const UTF8: u16 = 1 << 11;

/// Whether `bytes` start like a zip archive.
pub fn is_zip(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
}

fn invalid(what: &str) -> Error {
    Error::Invalid(format!("Not a readable zip file: {what}."))
}

/// Little-endian fields at an offset, failing past the end.
fn u16_at(b: &[u8], at: usize) -> Result<u16> {
    b.get(at..at + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| invalid("truncated"))
}

fn u32_at(b: &[u8], at: usize) -> Result<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| invalid("truncated"))
}

/// One entry as the central directory has it.
#[derive(Debug, Clone)]
struct Entry {
    name: String,
    method: u16,
    crc: u32,
    compressed: u64,
    size: u64,
    /// Where its local header starts.
    offset: usize,
}

/// An archive being read.
#[derive(Debug)]
pub struct Archive<'a> {
    bytes: &'a [u8],
    entries: Vec<Entry>,
}

impl<'a> Archive<'a> {
    /// Reads the central directory of `bytes`.
    pub fn new(bytes: &'a [u8]) -> Result<Self> {
        if bytes.len() > MAX_INPUT_BYTES {
            return Err(Error::TooLarge);
        }
        // The end record: 22 bytes plus a comment of at most 64 KiB.
        let min = bytes.len().saturating_sub(22 + 0xffff);
        let end = (min..=bytes.len().saturating_sub(22))
            .rev()
            .find(|&i| u32_at(bytes, i).is_ok_and(|s| s == END))
            .ok_or_else(|| invalid("no central directory"))?;
        if end >= 20 && u32_at(bytes, end - 20)? == END64_LOCATOR {
            return Err(invalid("ZIP64 is not supported"));
        }
        let disk = u16_at(bytes, end + 4)?;
        let cd_disk = u16_at(bytes, end + 6)?;
        let count = usize::from(u16_at(bytes, end + 10)?);
        let cd_size = u32_at(bytes, end + 12)?;
        let cd_offset = u32_at(bytes, end + 16)?;
        if disk != 0 || cd_disk != 0 {
            return Err(invalid("split archives are not supported"));
        }
        if count == 0xffff || cd_size == u32::MAX || cd_offset == u32::MAX {
            return Err(invalid("ZIP64 is not supported"));
        }
        if count > MAX_ENTRIES {
            return Err(invalid("too many entries"));
        }
        let cd_offset = cd_offset as usize;
        if cd_offset
            .checked_add(cd_size as usize)
            .is_none_or(|e| e > end)
        {
            return Err(invalid("central directory out of bounds"));
        }
        let mut entries: Vec<Entry> = Vec::with_capacity(count);
        let mut total = 0u64;
        let mut at = cd_offset;
        for _ in 0..count {
            if u32_at(bytes, at)? != CENTRAL {
                return Err(invalid("broken central directory"));
            }
            let flags = u16_at(bytes, at + 8)?;
            let method = u16_at(bytes, at + 10)?;
            let crc = u32_at(bytes, at + 16)?;
            let compressed = u32_at(bytes, at + 20)?;
            let size = u32_at(bytes, at + 24)?;
            let name_len = usize::from(u16_at(bytes, at + 28)?);
            let extra_len = usize::from(u16_at(bytes, at + 30)?);
            let comment_len = usize::from(u16_at(bytes, at + 32)?);
            let offset = u32_at(bytes, at + 42)?;
            let raw = bytes
                .get(at + 46..at + 46 + name_len)
                .ok_or_else(|| invalid("truncated"))?;
            at += 46 + name_len + extra_len + comment_len;
            if compressed == u32::MAX || size == u32::MAX || offset == u32::MAX {
                return Err(invalid("ZIP64 is not supported"));
            }
            if flags & ENCRYPTED != 0 {
                return Err(Error::Invalid(
                    "The file is encrypted (password protected).".into(),
                ));
            }
            let (compressed, size) = (u64::from(compressed), u64::from(size));
            if size > MAX_ENTRY_BYTES {
                return Err(Error::TooLarge);
            }
            if size > 1024 * 1024 && size / compressed.max(1) > MAX_RATIO {
                return Err(invalid("an entry is compressed suspiciously well"));
            }
            total += size;
            if total > MAX_TOTAL_BYTES {
                return Err(Error::TooLarge);
            }
            let name = name(raw, flags);
            // The first of entries with the same name counts.
            if entries.iter().all(|e| e.name != name) {
                entries.push(Entry {
                    name,
                    method,
                    crc,
                    compressed,
                    size,
                    offset: offset as usize,
                });
            }
        }
        Ok(Archive { bytes, entries })
    }

    /// The entries' names, in the archive's order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.name.as_str())
    }

    /// Whether there is an entry named `name`.
    pub fn contains(&self, name: &str) -> bool {
        self.entry(name).is_some()
    }

    fn entry(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// The bytes of the entry named `name`, `None` if there is none.
    pub fn read(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let Some(e) = self.entry(name) else {
            return Ok(None);
        };
        let b = self.bytes;
        if u32_at(b, e.offset)? != LOCAL {
            return Err(invalid("broken entry"));
        }
        let start = e.offset
            + 30
            + usize::from(u16_at(b, e.offset + 26)?)
            + usize::from(u16_at(b, e.offset + 28)?);
        let data = start
            .checked_add(e.compressed as usize)
            .and_then(|end| b.get(start..end))
            .ok_or_else(|| invalid("truncated"))?;
        let out = match e.method {
            STORED if e.compressed == e.size => data.to_vec(),
            STORED => return Err(invalid("an entry's sizes disagree")),
            DEFLATE => {
                let mut out = Vec::with_capacity(e.size.min(16 * 1024 * 1024) as usize);
                flate2::read::DeflateDecoder::new(data)
                    .take(e.size + 1)
                    .read_to_end(&mut out)
                    .map_err(|_| invalid("broken compressed data"))?;
                if out.len() as u64 != e.size {
                    return Err(invalid("an entry's sizes disagree"));
                }
                out
            }
            _ => return Err(invalid("unsupported compression")),
        };
        let mut crc = Crc::new();
        crc.update(&out);
        if crc.sum() != e.crc {
            return Err(invalid("checksum mismatch"));
        }
        Ok(Some(out))
    }
}

/// An entry's name: UTF-8 if flagged or valid, else code page 437.
fn name(raw: &[u8], flags: u16) -> String {
    match std::str::from_utf8(raw) {
        Ok(s) => s.to_owned(),
        Err(_) if flags & UTF8 != 0 => String::from_utf8_lossy(raw).into_owned(),
        Err(_) => raw
            .iter()
            .map(|&b| match b {
                0x80.. => CP437[usize::from(b - 0x80)],
                _ => char::from(b),
            })
            .collect(),
    }
}

/// Code page 437's upper half.
const CP437: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', 'É', 'æ', 'Æ',
    'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ', 'á', 'í', 'ó', 'ú', 'ñ', 'Ñ',
    'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»', '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕',
    '╣', '║', '╗', '╝', '╜', '╛', '┐', '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦',
    '╠', '═', '╬', '╧', '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐',
    '▀', 'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩', '≡', '±',
    '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{a0}',
];

/// An archive being written.
#[derive(Debug, Default)]
pub struct ZipWriter {
    out: Vec<u8>,
    central: Vec<u8>,
    count: usize,
}

impl ZipWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an entry named `name`, deflated if `deflate` (else stored).
    pub fn add(&mut self, name: &str, data: &[u8], deflate: bool) -> Result<()> {
        let too_large = || Error::Invalid("Too large for a zip file without ZIP64.".into());
        if self.count >= 0xffff || name.len() > 0xffff {
            return Err(too_large());
        }
        let mut crc = Crc::new();
        crc.update(data);
        let packed = match deflate {
            true => {
                let mut enc =
                    flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                enc.write_all(data)
                    .and_then(|_| enc.finish())
                    .map_err(|e| Error::Invalid(e.to_string()))?
            }
            false => Vec::new(),
        };
        let (method, body) = match deflate {
            true => (DEFLATE, packed.as_slice()),
            false => (STORED, data),
        };
        let (size, compressed, offset) = (
            u32::try_from(data.len()).map_err(|_| too_large())?,
            u32::try_from(body.len()).map_err(|_| too_large())?,
            u32::try_from(self.out.len()).map_err(|_| too_large())?,
        );
        // Version 2.0, UTF-8 names, 1980-01-01 00:00.
        let common = |v: &mut Vec<u8>| {
            v.extend_from_slice(&20u16.to_le_bytes());
            v.extend_from_slice(&UTF8.to_le_bytes());
            v.extend_from_slice(&method.to_le_bytes());
            v.extend_from_slice(&0u16.to_le_bytes());
            v.extend_from_slice(&0x21u16.to_le_bytes());
            v.extend_from_slice(&crc.sum().to_le_bytes());
            v.extend_from_slice(&compressed.to_le_bytes());
            v.extend_from_slice(&size.to_le_bytes());
            v.extend_from_slice(&(name.len() as u16).to_le_bytes());
            v.extend_from_slice(&0u16.to_le_bytes());
        };
        self.out.extend_from_slice(&LOCAL.to_le_bytes());
        common(&mut self.out);
        self.out.extend_from_slice(name.as_bytes());
        self.out.extend_from_slice(body);
        self.central.extend_from_slice(&CENTRAL.to_le_bytes());
        self.central.extend_from_slice(&20u16.to_le_bytes());
        common(&mut self.central);
        // Comment length, disk, internal and external attributes.
        self.central.extend_from_slice(&[0; 10]);
        self.central.extend_from_slice(&offset.to_le_bytes());
        self.central.extend_from_slice(name.as_bytes());
        self.count += 1;
        Ok(())
    }

    /// The archive's bytes.
    pub fn finish(mut self) -> Result<Vec<u8>> {
        let too_large = || Error::Invalid("Too large for a zip file without ZIP64.".into());
        let offset = u32::try_from(self.out.len()).map_err(|_| too_large())?;
        let size = u32::try_from(self.central.len()).map_err(|_| too_large())?;
        self.out.append(&mut self.central);
        self.out.extend_from_slice(&END.to_le_bytes());
        self.out.extend_from_slice(&[0; 4]);
        for _ in 0..2 {
            self.out
                .extend_from_slice(&(self.count as u16).to_le_bytes());
        }
        self.out.extend_from_slice(&size.to_le_bytes());
        self.out.extend_from_slice(&offset.to_le_bytes());
        self.out.extend_from_slice(&0u16.to_le_bytes());
        Ok(self.out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zip(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut w = ZipWriter::new();
        for (name, data, deflate) in entries {
            w.add(name, data, *deflate).unwrap();
        }
        w.finish().unwrap()
    }

    #[test]
    fn round_trip() {
        let text = "Grüße ".repeat(1000);
        let bytes = zip(&[
            ("mimetype", b"application/epub+zip", false),
            ("a/ü.txt", text.as_bytes(), true),
            ("empty", b"", true),
        ]);
        assert!(is_zip(&bytes));
        assert_eq!(
            &bytes[30..38],
            b"mimetype",
            "first entry, name right after its header"
        );
        let a = Archive::new(&bytes).unwrap();
        assert_eq!(
            a.names().collect::<Vec<_>>(),
            ["mimetype", "a/ü.txt", "empty"]
        );
        assert_eq!(a.read("a/ü.txt").unwrap().unwrap(), text.as_bytes());
        assert_eq!(a.read("empty").unwrap().unwrap(), b"");
        assert!(a.read("missing").unwrap().is_none());
        assert!(a.contains("mimetype"));
    }

    #[test]
    fn cp437_names() {
        assert_eq!(name(b"\x81ber", 0), "über");
        assert_eq!(name("über".as_bytes(), 0), "über");
    }

    /// The offset of the central directory's first entry.
    fn central(bytes: &[u8]) -> usize {
        let end = bytes.len() - 22;
        u32_at(bytes, end + 16).unwrap() as usize
    }

    #[test]
    fn hostile_archives_fail_cleanly() {
        let good = zip(&[("a", &[7; 5000], true), ("b", b"hello", false)]);
        // Truncated anywhere: an error, never a panic.
        for len in 0..good.len() {
            if let Ok(a) = Archive::new(&good[..len]) {
                let _ = a.read("a");
                let _ = a.read("b");
            }
        }
        // A wrong checksum.
        let mut bad = good.clone();
        let at = central(&bad) + 16;
        bad[at] ^= 1;
        let a = Archive::new(&bad).unwrap();
        assert!(a.read("a").is_err());
        // Sizes that lie: larger or smaller than the data inflates to.
        for size in [4999u32, 5001, 1 << 20] {
            let mut bad = good.clone();
            let at = central(&bad) + 24;
            bad[at..at + 4].copy_from_slice(&size.to_le_bytes());
            let a = Archive::new(&bad).unwrap();
            assert!(a.read("a").is_err(), "{size}");
        }
        // A bomb: tiny data claiming a huge size.
        let mut bomb = zip(&[("a", &vec![0; 2 * 1024 * 1024], true)]);
        let at = central(&bomb);
        assert!(
            Archive::new(&bomb).is_err(),
            "2 MiB of zeros deflate 1000:1"
        );
        bomb[at + 24..at + 28].copy_from_slice(&(MAX_ENTRY_BYTES as u32 + 1).to_le_bytes());
        assert!(matches!(Archive::new(&bomb), Err(Error::TooLarge)));
        // Encrypted and ZIP64 entries.
        let mut enc = good.clone();
        let at = central(&enc) + 8;
        enc[at] |= 1;
        assert!(Archive::new(&enc).is_err());
        let mut z64 = good.clone();
        let at = central(&z64) + 20;
        z64[at..at + 4].copy_from_slice(&[0xff; 4]);
        assert!(Archive::new(&z64).is_err());
        // Flipped bytes in the data.
        for i in (0..good.len()).step_by(7) {
            let mut bad = good.clone();
            bad[i] = bad[i].wrapping_add(0x55);
            if let Ok(a) = Archive::new(&bad) {
                let _ = a.read("a");
                let _ = a.read("b");
            }
        }
        assert!(Archive::new(b"not a zip").is_err());
    }
}
