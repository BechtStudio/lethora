//! Record framing of append-only logs: `[u32 LE len][u32 LE crc32][len bytes:
//! payload]`. The same for a project's update logs, a replica's logs and the
//! members log. A record cut short at the end is a torn tail (still being
//! written or synced), not damage.

/// Bytes before a record's payload.
pub const FRAME_HEADER: usize = 8;

/// A record holding `payload`; `None` if it is too large for one record.
pub fn frame(payload: &[u8]) -> Option<Vec<u8>> {
    let len = u32::try_from(payload.len()).ok()?;
    let mut frame = Vec::with_capacity(FRAME_HEADER + payload.len());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(&crc32fast::hash(payload).to_le_bytes());
    frame.extend_from_slice(payload);
    Some(frame)
}

/// What [`parse_frame`] found at an offset of a log.
#[derive(Debug, PartialEq, Eq)]
pub enum Frame<'a> {
    /// A record: its payload and the offset of the next record.
    Record(&'a [u8], usize),
    /// The end of the log, or a record cut short (a torn tail).
    Short,
    /// A complete record whose payload fails its crc.
    Bad,
}

/// Parses the record starting at `at`.
pub fn parse_frame(buf: &[u8], at: usize) -> Frame<'_> {
    let Some(header) = at
        .checked_add(FRAME_HEADER)
        .and_then(|end| buf.get(at..end))
    else {
        return Frame::Short;
    };
    let len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
    let crc = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    let start = at + FRAME_HEADER;
    let Some(payload) = start.checked_add(len).and_then(|end| buf.get(start..end)) else {
        return Frame::Short;
    };
    if crc32fast::hash(payload) == crc {
        Frame::Record(payload, start + len)
    } else {
        Frame::Bad
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let mut log = frame(b"one").unwrap();
        log.extend(frame(b"").unwrap());
        let Frame::Record(one, next) = parse_frame(&log, 0) else {
            panic!("first record");
        };
        assert_eq!(one, b"one");
        assert_eq!(parse_frame(&log, next), Frame::Record(b"", log.len()));
        assert_eq!(parse_frame(&log, log.len()), Frame::Short);
        assert_eq!(parse_frame(&log[..log.len() - 1], next), Frame::Short);
        log[FRAME_HEADER] ^= 1;
        assert_eq!(parse_frame(&log, 0), Frame::Bad);
    }
}
