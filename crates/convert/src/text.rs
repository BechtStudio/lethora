//! Plain text: blank lines separate paragraphs, the lines of a paragraph
//! join with a space. Also the byte decoding every text format shares.

use crate::model::{Block, Doc, FigureBody, Image, Inline, Table};
use crate::{
    Error, Imported, MAX_DEPTH, MAX_INPUT_BYTES, ReadOptions, Reader, Result, WarningKind,
    Warnings, WriteOptions, Writer, Written,
};

/// The plain-text reader.
pub struct PlainText;

impl Reader for PlainText {
    fn read(&self, bytes: &[u8], _: &ReadOptions) -> Result<Imported> {
        let mut warnings = Warnings::default();
        let text = decode(bytes, &mut warnings)?;
        let mut blocks = Vec::new();
        let mut para: Vec<&str> = Vec::new();
        for line in text.lines().chain([""]) {
            let line = line.trim();
            if !line.is_empty() {
                para.push(line);
            } else if !para.is_empty() {
                blocks.push(Block::Para(vec![Inline::Text(para.join(" "))]));
                para.clear();
            }
        }
        Ok(Imported {
            doc: Doc {
                blocks,
                ..Doc::default()
            },
            warnings: warnings.into_vec(),
        })
    }
}

/// Plain text (UTF-8, LF): the title block, then blocks separated by blank
/// lines; headings as their text, lists with `- ` and `1. `, quotes with
/// `> `, table cells tab-separated, footnotes as `[1]` with the notes at
/// the end, images as `[Image: alt]`, formulas as their Typst source and
/// links as `text (url)` where the two differ.
impl Writer for PlainText {
    fn write(&self, doc: &Doc, _: &WriteOptions) -> Result<Written> {
        let mut w = TextWriter {
            notes: Vec::new(),
            depth: 0,
        };
        let m = &doc.meta;
        let mut head: Vec<String> = [&m.title, &m.subtitle]
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        head.extend(m.authors.iter().cloned());
        head.extend(m.date.clone());
        let mut out = match head.is_empty() {
            true => String::new(),
            false => format!("{}\n\n", head.join("\n")),
        };
        out.push_str(&w.blocks(&doc.blocks)?);
        // Notes may hold notes: numbered on as they turn up.
        let mut done = 0;
        let mut notes = String::new();
        while done < w.notes.len() {
            let blocks = w.notes[done];
            done += 1;
            let body = w.blocks(blocks)?;
            let lines: Vec<String> = body
                .trim_end()
                .lines()
                .enumerate()
                .map(|(i, l)| match (i, l.is_empty()) {
                    (0, _) => format!("[{done}] {l}"),
                    (_, true) => String::new(),
                    _ => format!("    {l}"),
                })
                .collect();
            notes.push_str(&lines.join("\n"));
            notes.push('\n');
        }
        if !notes.is_empty() {
            out.push_str(&format!("---\n\n{notes}"));
        }
        let text = format!("{}\n", out.trim_end());
        Ok(Written {
            bytes: text.into_bytes(),
            files: Vec::new(),
            warnings: Vec::new(),
        })
    }
}

struct TextWriter<'d> {
    /// The footnotes so far, in order (number = index + 1).
    notes: Vec<&'d [Block]>,
    depth: usize,
}

impl<'d> TextWriter<'d> {
    fn enter(&mut self) -> Result<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Error::TooDeep);
        }
        Ok(())
    }

    /// Blocks separated by blank lines, ending in one.
    fn blocks(&mut self, blocks: &'d [Block]) -> Result<String> {
        self.enter()?;
        let mut out = String::new();
        for b in blocks {
            out.push_str(&self.block(b)?);
            out.push_str("\n\n");
        }
        self.depth -= 1;
        Ok(out)
    }

    fn block(&mut self, block: &'d Block) -> Result<String> {
        Ok(match block {
            Block::Para(c) | Block::Heading { content: c, .. } => {
                self.inlines(c)?.trim().to_owned()
            }
            Block::List(l) => {
                let mut items = Vec::new();
                for (i, item) in l.items.iter().enumerate() {
                    let marker = match l.ordered {
                        true => format!("{}. ", l.start + i as u64),
                        false => "- ".to_owned(),
                    };
                    let body = self.blocks(item)?;
                    let pad = " ".repeat(marker.chars().count());
                    let body = body
                        .trim_end()
                        .replace("\n\n", "\n")
                        .replace('\n', &format!("\n{pad}"));
                    items.push(format!("{marker}{body}"));
                }
                items.join("\n")
            }
            Block::Quote(b) => {
                let inner = self.blocks(b)?;
                let lines: Vec<String> = inner
                    .trim_end()
                    .lines()
                    .map(|l| format!("> {l}").trim_end().to_owned())
                    .collect();
                lines.join("\n")
            }
            Block::Code { text, .. } => text.trim_end_matches('\n').to_owned(),
            Block::Math { math, number, .. } => match number {
                Some(n) => format!("{}    {n}", math.typst),
                None => math.typst.clone(),
            },
            Block::Figure(f) => {
                let body = match &f.body {
                    FigureBody::Image(img) => image(img),
                    FigureBody::Table(t) => self.table(t)?,
                };
                let mut caption = f.number.clone().unwrap_or_default();
                let text = self.inlines(&f.caption)?;
                if !text.is_empty() {
                    if !caption.is_empty() {
                        caption.push_str(": ");
                    }
                    caption.push_str(&text);
                }
                match caption.is_empty() {
                    true => body,
                    false => format!("{body}\n{caption}"),
                }
            }
            Block::Table(t) => self.table(t)?,
            Block::Rule => "* * *".to_owned(),
        })
    }

    /// One row per line, cells tab-separated (a cell's lines joined).
    fn table(&mut self, t: &'d Table) -> Result<String> {
        let mut rows = Vec::new();
        for row in &t.rows {
            let mut cells = Vec::new();
            for cell in row {
                let text = self.blocks(&cell.content)?;
                cells.push(text.split_whitespace().collect::<Vec<_>>().join(" "));
            }
            rows.push(cells.join("\t"));
        }
        Ok(rows.join("\n"))
    }

    fn inlines(&mut self, inlines: &'d [Inline]) -> Result<String> {
        self.enter()?;
        let mut out = String::new();
        for i in inlines {
            match i {
                Inline::Text(t) => out.push_str(&t.replace('\n', " ")),
                Inline::Emph(c)
                | Inline::Strong(c)
                | Inline::Underline(c)
                | Inline::Strike(c)
                | Inline::Super(c)
                | Inline::Sub(c) => out.push_str(&self.inlines(c)?),
                Inline::Code(c) => out.push_str(c),
                Inline::LineBreak => out.push('\n'),
                Inline::Link { url, content } => {
                    let text = self.inlines(content)?;
                    let bare = url.strip_prefix("mailto:").unwrap_or(url);
                    match text.is_empty() || text == *url || text == bare {
                        true => out.push_str(if text.is_empty() { url } else { &text }),
                        false => out.push_str(&format!("{text} ({url})")),
                    }
                }
                Inline::Math(m) => out.push_str(&m.typst),
                Inline::Image(img) => out.push_str(&image(img)),
                Inline::Footnote(blocks) => {
                    self.notes.push(blocks);
                    out.push_str(&format!("[{}]", self.notes.len()));
                }
                Inline::Cite(cite) => match &cite.text {
                    Some(c) => out.push_str(&self.inlines(c)?),
                    None => {
                        let keys: Vec<String> =
                            cite.items.iter().map(|i| format!("@{}", i.key)).collect();
                        out.push_str(&format!("[{}]", keys.join("; ")));
                    }
                },
                Inline::Ref { label, text } => match text {
                    Some(c) => out.push_str(&self.inlines(c)?),
                    None => out.push_str(label),
                },
            }
        }
        self.depth -= 1;
        Ok(out)
    }
}

fn image(img: &Image) -> String {
    match &img.alt {
        Some(alt) => format!("[Image: {alt}]"),
        None => "[Image]".to_owned(),
    }
}

/// Text from bytes: UTF-8 (a byte order mark dropped), UTF-16 with a byte
/// order mark, else Windows-1252 (with a warning). Line ends become `\n`.
pub(crate) fn decode(bytes: &[u8], warnings: &mut Warnings) -> Result<String> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(Error::TooLarge);
    }
    let text = if let Some(rest) = bytes.strip_prefix(b"\xef\xbb\xbf") {
        String::from_utf8_lossy(rest).into_owned()
    } else if let Some(rest) = bytes.strip_prefix(b"\xff\xfe") {
        utf16(rest, u16::from_le_bytes)
    } else if let Some(rest) = bytes.strip_prefix(b"\xfe\xff") {
        utf16(rest, u16::from_be_bytes)
    } else {
        match std::str::from_utf8(bytes) {
            Ok(s) => s.to_owned(),
            Err(_) => {
                warnings.add(WarningKind::Encoding);
                bytes.iter().map(|&b| windows_1252(b)).collect()
            }
        }
    };
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
}

pub(crate) fn utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> String {
    let units = bytes.as_chunks::<2>().0.iter().map(|c| unit(*c));
    char::decode_utf16(units)
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// Windows-1252's 0x80–0x9F; the five undefined bytes are U+FFFD.
const CP1252: [char; 32] = [
    '€', '\u{fffd}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{fffd}', 'Ž',
    '\u{fffd}', '\u{fffd}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{fffd}',
    'ž', 'Ÿ',
];

fn windows_1252(b: u8) -> char {
    match b {
        0x80..=0x9f => CP1252[usize::from(b - 0x80)],
        _ => char::from(b),
    }
}

/// Whether `bytes` look like text: a UTF-16 byte order mark, or no NUL
/// byte in the first 8 KiB.
pub(crate) fn looks_like_text(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\xff\xfe")
        || bytes.starts_with(b"\xfe\xff")
        || !bytes[..bytes.len().min(8192)].contains(&0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(bytes: &[u8]) -> Imported {
        PlainText.read(bytes, &ReadOptions::new("a.txt")).unwrap()
    }

    fn paras(doc: &Doc) -> Vec<String> {
        doc.blocks
            .iter()
            .map(|b| match b {
                Block::Para(i) => match &i[..] {
                    [Inline::Text(t)] => t.clone(),
                    _ => panic!("{i:?}"),
                },
                b => panic!("{b:?}"),
            })
            .collect()
    }

    #[test]
    fn paragraphs_and_line_joins() {
        let doc = read(b"One\r\ntwo\r\n\r\n\r\n  Three  \n").doc;
        assert_eq!(paras(&doc), ["One two", "Three"]);
    }

    #[test]
    fn encodings() {
        let bom = read(b"\xef\xbb\xbfGr\xc3\xbc\xc3\x9fe");
        assert_eq!(paras(&bom.doc), ["Grüße"]);
        assert!(bom.warnings.is_empty());
        let le: Vec<u8> = [0xff, 0xfe]
            .into_iter()
            .chain("Ä€\n\nb".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(paras(&read(&le).doc), ["Ä€", "b"]);
        let be: Vec<u8> = [0xfe, 0xff]
            .into_iter()
            .chain("x".encode_utf16().flat_map(u16::to_be_bytes))
            .collect();
        assert_eq!(paras(&read(&be).doc), ["x"]);
        let cp = read(b"\x93Caf\xe9\x94 \x80");
        assert_eq!(paras(&cp.doc), ["“Café” €"]);
        assert_eq!(cp.warnings[0].kind, WarningKind::Encoding);
    }

    #[test]
    fn sniffing() {
        assert!(looks_like_text(b"plain"));
        assert!(!looks_like_text(b"PK\x03\x04\x00\x00"));
        assert!(looks_like_text(b"\xff\xfea\x00"));
    }
}
