//! The Markdown writer: CommonMark with GFM tables, strikethrough and
//! footnotes (`[^1]`), YAML front matter for the metadata, heading labels
//! as `{#label}`, everything the markdown reader reads back.
//!
//! - Images go to a folder next to the file (`<stem>_images/`), written as
//!   [`Written::files`] and referenced relatively; a figure is its image
//!   with the caption (and number) as alt text, as the reader takes it.
//! - Formulas keep their Typst source in `$…$` / `$$…$$` (there is no
//!   Typst → LaTeX converter), with a [`WarningKind::TypstMath`] warning.
//! - Tables with merged cells or several blocks in a cell are simplified
//!   (the content in the first slot, blocks joined by `<br>`); underline
//!   is dropped; superscript and subscript are `<sup>`/`<sub>`.
//! - Citations with a typeset text (exports) are that text; others are
//!   written in Pandoc's form (`[@key, p. 4]`).

use crate::markdown::{Markdown, plain};
use crate::model::{Block, Cite, Doc, Figure, FigureBody, Image, Inline, List, Table};
use crate::{Error, MAX_DEPTH, Result, WarningKind, Warnings, WriteOptions, Writer, Written};

impl Writer for Markdown {
    fn write(&self, doc: &Doc, options: &WriteOptions) -> Result<Written> {
        let stem = match options.stem.trim() {
            "" => "images".to_owned(),
            s => format!("{s}_images"),
        };
        let mut w = MdWriter {
            doc,
            folder: stem,
            files: Vec::new(),
            notes: Vec::new(),
            warnings: Warnings::default(),
            depth: 0,
            bol: true,
        };
        let mut out = front_matter(doc);
        out.push_str(&w.blocks(&doc.blocks)?);
        // Notes may hold notes: numbered on as they turn up.
        let mut done = 0;
        while done < w.notes.len() {
            let blocks = w.notes[done];
            done += 1;
            let body = w.blocks(blocks)?;
            let mut lines = body.trim_end().lines();
            out.push_str(&format!("[^{done}]: {}\n", lines.next().unwrap_or("")));
            for line in lines {
                out.push_str(&indent(line, "    "));
            }
            out.push('\n');
        }
        let text = format!("{}\n", out.trim_end());
        Ok(Written {
            bytes: text.into_bytes(),
            files: w.files,
            warnings: w.warnings.into_vec(),
        })
    }
}

/// YAML front matter with what the reader takes back; empty without
/// metadata.
fn front_matter(doc: &Doc) -> String {
    let m = &doc.meta;
    let quote = |s: &str| format!("\"{}\"", s.replace(['\n', '\r'], " "));
    let mut lines = Vec::new();
    for (key, value) in [
        ("title", &m.title),
        ("subtitle", &m.subtitle),
        ("date", &m.date),
        ("lang", &m.lang),
    ] {
        if let Some(v) = value {
            lines.push(format!("{key}: {}", quote(v)));
        }
    }
    match &m.authors[..] {
        [] => {}
        [one] => lines.push(format!("author: {}", quote(one))),
        many => {
            lines.push("author:".to_owned());
            lines.extend(many.iter().map(|a| format!("  - {}", quote(a))));
        }
    }
    match lines.is_empty() {
        true => String::new(),
        false => format!("---\n{}\n---\n\n", lines.join("\n")),
    }
}

struct MdWriter<'d> {
    doc: &'d Doc,
    /// The images' folder (`thesis_images`).
    folder: String,
    files: Vec<(String, Vec<u8>)>,
    /// The footnotes so far, in order (number = index + 1).
    notes: Vec<&'d [Block]>,
    warnings: Warnings,
    depth: usize,
    /// Whether the inline text written next starts a line.
    bol: bool,
}

impl<'d> MdWriter<'d> {
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
        for (i, b) in blocks.iter().enumerate() {
            // Two lists in a row would merge into one.
            if i > 0 && matches!((&blocks[i - 1], b), (Block::List(_), Block::List(_))) {
                out.push_str("<!-- -->\n\n");
            }
            out.push_str(&self.block(b)?);
            out.push_str("\n\n");
        }
        self.depth -= 1;
        Ok(out)
    }

    fn block(&mut self, block: &'d Block) -> Result<String> {
        Ok(match block {
            Block::Para(c) => self.line(c)?,
            Block::Heading {
                level,
                content,
                label,
            } => {
                // After the marks: nothing starts a block here.
                self.bol = false;
                let text = self.inlines(content)?.replace("\\\n", " ");
                let label = label
                    .as_ref()
                    .map(|l| format!(" {{#{l}}}"))
                    .unwrap_or_default();
                format!(
                    "{} {text}{label}",
                    "#".repeat(usize::from((*level).clamp(1, 6)))
                )
            }
            Block::List(l) => self.list(l)?,
            Block::Quote(b) => {
                let inner = self.blocks(b)?;
                inner
                    .trim_end()
                    .lines()
                    .map(|l| {
                        if l.is_empty() {
                            ">".to_owned()
                        } else {
                            format!("> {l}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            Block::Code { lang, text } => {
                let fence = "`".repeat(longest_run(text, '`').max(2) + 1);
                let lang = lang.as_deref().unwrap_or("");
                format!("{fence}{lang}\n{}\n{fence}", text.trim_end_matches('\n'))
            }
            Block::Math { math, .. } => {
                self.warnings.add(WarningKind::TypstMath);
                format!("$${}$$", one_line(&math.typst))
            }
            Block::Figure(f) => self.figure(f)?,
            Block::Table(t) => self.table(t)?,
            Block::Rule => "---".to_owned(),
        })
    }

    /// Inlines starting a line of their own.
    fn line(&mut self, inlines: &'d [Inline]) -> Result<String> {
        self.bol = true;
        self.inlines(inlines)
    }

    fn list(&mut self, l: &'d List) -> Result<String> {
        // Tight: each item a paragraph, maybe with a nested list under it.
        let tight = l.items.iter().all(|item| {
            matches!(
                &item[..],
                [Block::Para(_)] | [Block::Para(_), Block::List(_)]
            )
        });
        let mut items = Vec::new();
        for (i, item) in l.items.iter().enumerate() {
            let marker = match l.ordered {
                true => format!("{}. ", l.start + i as u64),
                false => "- ".to_owned(),
            };
            let pad = " ".repeat(marker.len());
            let body = match tight {
                true => {
                    let mut parts = Vec::new();
                    for b in item {
                        parts.push(self.block(b)?);
                    }
                    parts.join("\n")
                }
                false => self.blocks(item)?.trim_end().to_owned(),
            };
            let mut lines = body.lines();
            let mut text = format!("{marker}{}", lines.next().unwrap_or(""));
            for line in lines {
                text.push('\n');
                if !line.is_empty() {
                    text.push_str(&pad);
                    text.push_str(line);
                }
            }
            items.push(text);
        }
        Ok(items.join(if tight { "\n" } else { "\n\n" }))
    }

    fn figure(&mut self, f: &'d Figure) -> Result<String> {
        let mut caption = plain(&f.caption);
        if let Some(n) = &f.number {
            caption = match caption.is_empty() {
                true => n.clone(),
                false => format!("{n}: {caption}"),
            };
        }
        Ok(match &f.body {
            FigureBody::Image(img) => {
                let alt = match caption.is_empty() {
                    true => img.alt.clone().unwrap_or_default(),
                    false => caption,
                };
                self.image(img, &alt)
            }
            FigureBody::Table(t) => {
                let table = self.table(t)?;
                match caption.is_empty() {
                    true => table,
                    false => {
                        self.bol = true;
                        let mut text = String::new();
                        self.text(&caption, &mut text);
                        format!("{text}\n\n{table}")
                    }
                }
            }
        })
    }

    /// A GFM table; merged cells and blocks in cells simplified (warned).
    fn table(&mut self, t: &'d Table) -> Result<String> {
        let columns = t.columns.max(1);
        let mut simplified = t.header_rows > 1;
        // Cells by slot: a merged cell's content in its first slot.
        let mut grid: Vec<Vec<String>> = vec![vec![String::new(); columns]; t.rows.len()];
        let mut covered = vec![vec![false; columns]; t.rows.len()];
        for (y, row) in t.rows.iter().enumerate() {
            let mut x = 0;
            for cell in row {
                while x < columns && covered[y][x] {
                    x += 1;
                }
                if x >= columns {
                    break;
                }
                simplified |= cell.colspan > 1 || cell.rowspan > 1;
                for dy in 0..cell.rowspan as usize {
                    for dx in 0..cell.colspan as usize {
                        if let Some(slot) = covered.get_mut(y + dy).and_then(|r| r.get_mut(x + dx))
                        {
                            *slot = true;
                        }
                    }
                }
                let mut parts = Vec::new();
                for b in &cell.content {
                    match b {
                        Block::Para(c) => parts.push(self.line(c)?),
                        other => {
                            simplified = true;
                            parts.push(self.block(other)?);
                        }
                    }
                }
                simplified |= parts.len() > 1;
                grid[y][x] = parts
                    .join("<br>")
                    .replace("\\\n", "<br>")
                    .replace('\n', " ")
                    .replace('|', "\\|");
                x += cell.colspan as usize;
            }
        }
        if simplified {
            self.warnings.add(WarningKind::TableSimplified);
        }
        let row = |cells: &[String]| format!("| {} |", cells.join(" | "));
        let mut rows = grid.iter();
        let header = match t.header_rows {
            0 => row(&vec![String::new(); columns]),
            _ => row(rows.next().map_or(&[][..], |r| &r[..])),
        };
        let mut out = vec![header, row(&vec!["---".to_owned(); columns])];
        out.extend(rows.map(|r| row(r)));
        Ok(out.join("\n"))
    }

    /// `![alt](folder/name)`, the image's file added to the side files.
    fn image(&mut self, img: &Image, alt: &str) -> String {
        let path = match self.doc.resource(&img.resource) {
            Some(r) => {
                let path = format!("{}/{}", self.folder, r.name);
                if !self.files.iter().any(|(p, _)| *p == path) {
                    self.files.push((path.clone(), r.bytes.clone()));
                }
                path
            }
            None => img.resource.clone(),
        };
        let mut text = String::new();
        self.bol = false;
        self.text(alt, &mut text);
        format!("![{text}]({})", destination(&path))
    }

    fn inlines(&mut self, inlines: &'d [Inline]) -> Result<String> {
        self.enter()?;
        let mut out = String::new();
        for (i, item) in inlines.iter().enumerate() {
            match item {
                Inline::Text(t) => {
                    self.text(t, &mut out);
                    // `!` before a link would make it an image.
                    let bracket = matches!(
                        inlines.get(i + 1),
                        Some(Inline::Link { .. } | Inline::Footnote(_) | Inline::Cite(_))
                    );
                    if bracket && out.ends_with('!') && !out.ends_with("\\!") {
                        out.insert(out.len() - 1, '\\');
                    }
                }
                Inline::Emph(c) => self.delimited(c, "*", &mut out)?,
                Inline::Strong(c) => self.delimited(c, "**", &mut out)?,
                Inline::Strike(c) => self.delimited(c, "~~", &mut out)?,
                Inline::Underline(c) => {
                    self.warnings
                        .add_example(WarningKind::Formatting, "underline");
                    out.push_str(&self.inlines(c)?);
                }
                Inline::Super(c) => {
                    let inner = self.inlines(c)?;
                    out.push_str(&format!("<sup>{inner}</sup>"));
                }
                Inline::Sub(c) => {
                    let inner = self.inlines(c)?;
                    out.push_str(&format!("<sub>{inner}</sub>"));
                }
                Inline::Code(c) => {
                    out.push_str(&code_span(c));
                    self.bol = false;
                }
                Inline::LineBreak => {
                    out.push_str("\\\n");
                    self.bol = true;
                }
                Inline::Link { url, content } => {
                    let shown = plain(content);
                    if content.is_empty() || shown == *url || format!("mailto:{shown}") == *url {
                        let target = if content.is_empty() { url } else { &shown };
                        out.push_str(&format!("<{}>", target.replace(['<', '>', ' '], "")));
                    } else {
                        let inner = self.inlines(content)?;
                        out.push_str(&format!("[{inner}]({})", destination(url)));
                    }
                    self.bol = false;
                }
                Inline::Math(m) => {
                    self.warnings.add(WarningKind::TypstMath);
                    out.push_str(&format!("${}$", one_line(&m.typst)));
                    self.bol = false;
                }
                Inline::Image(img) => {
                    let alt = img.alt.clone().unwrap_or_default();
                    out.push_str(&self.image(img, &alt));
                }
                Inline::Footnote(blocks) => {
                    self.notes.push(blocks);
                    out.push_str(&format!("[^{}]", self.notes.len()));
                    self.bol = false;
                }
                Inline::Cite(cite) => self.cite(cite, &mut out)?,
                Inline::Ref { label, text } => match text {
                    Some(c) => out.push_str(&self.inlines(c)?),
                    None => self.text(label, &mut out),
                },
            }
        }
        self.depth -= 1;
        Ok(out)
    }

    /// `content` between `mark`s, the whitespace at its ends outside them
    /// (CommonMark wants the marks next to text).
    fn delimited(&mut self, content: &'d [Inline], mark: &str, out: &mut String) -> Result<()> {
        let inner = self.inlines(content)?;
        let trimmed = inner.trim();
        if trimmed.is_empty() {
            out.push_str(&inner);
            return Ok(());
        }
        let lead = &inner[..inner.len() - inner.trim_start().len()];
        let tail = &inner[inner.trim_end().len()..];
        out.push_str(&format!("{lead}{mark}{trimmed}{mark}{tail}"));
        Ok(())
    }

    fn cite(&mut self, cite: &'d Cite, out: &mut String) -> Result<()> {
        let source = |cite: &Cite| {
            let items: Vec<String> = cite
                .items
                .iter()
                .map(|i| match &i.locator {
                    Some(l) => format!("@{}, {l}", i.key),
                    None => format!("@{}", i.key),
                })
                .collect();
            format!("[{}]", items.join("; "))
        };
        match &cite.text {
            // As written in Markdown (an import): the same again.
            Some(c) if plain(c).starts_with('@') => out.push_str(&plain(c)),
            Some(c) if plain(c).starts_with("[@") => out.push_str(&source(cite)),
            Some(c) => out.push_str(&self.inlines(c)?),
            None => out.push_str(&source(cite)),
        }
        self.bol = false;
        Ok(())
    }

    /// Text with everything Markdown would read as markup escaped.
    fn text(&mut self, text: &str, out: &mut String) {
        let chars: Vec<char> = text
            .chars()
            .map(|c| if c == '\n' { ' ' } else { c })
            .collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if self.bol {
                // Leading blanks would make code; markers make blocks.
                if matches!(c, ' ' | '\t') {
                    i += 1;
                    continue;
                }
                self.bol = false;
                if matches!(c, '-' | '+' | '=' | '#' | '>') {
                    out.push('\\');
                } else if c.is_ascii_digit() {
                    let digits = chars[i..].iter().take_while(|c| c.is_ascii_digit()).count();
                    if matches!(chars.get(i + digits), Some('.' | ')')) {
                        out.extend(&chars[i..i + digits]);
                        out.push('\\');
                        i += digits;
                        continue;
                    }
                }
            }
            let prev = i.checked_sub(1).map(|p| chars[p]);
            let next = chars.get(i + 1).copied();
            let alnum = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
            let escape = match c {
                '\\' | '`' | '*' | '[' | ']' | '<' | '$' | '~' | '{' | '}' => true,
                // Inside a word an underscore is text.
                '_' => !(alnum(prev) && alnum(next)),
                '!' => next == Some('['),
                // Only an entity (`&amp;`) needs it.
                '&' => {
                    let rest: String = chars[i + 1..].iter().take(32).collect();
                    rest.split_once(';').is_some_and(|(name, _)| {
                        !name.is_empty()
                            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '#')
                    })
                }
                _ => false,
            };
            if escape {
                out.push('\\');
            }
            out.push(c);
            i += 1;
        }
    }
}

/// Lines indented by `pad`, each ending in a line break.
fn indent(line: &str, pad: &str) -> String {
    match line.is_empty() {
        true => "\n".to_owned(),
        false => format!("{pad}{line}\n"),
    }
}

/// A link destination: in `<…>` if it has blanks or brackets.
fn destination(url: &str) -> String {
    if url.contains([' ', '(', ')', '<', '>']) || url.is_empty() {
        format!("<{}>", url.replace('<', "%3C").replace('>', "%3E"))
    } else {
        url.to_owned()
    }
}

/// Inline code in enough backticks, padded where it starts or ends with
/// one or with blanks on both sides.
fn code_span(code: &str) -> String {
    let code = code.replace('\n', " ");
    let ticks = "`".repeat(longest_run(&code, '`') + 1);
    let pad = code.starts_with('`')
        || code.ends_with('`')
        || (code.starts_with(' ') && code.ends_with(' ') && !code.trim().is_empty());
    match pad {
        true => format!("{ticks} {code} {ticks}"),
        false => format!("{ticks}{code}{ticks}"),
    }
}

fn longest_run(s: &str, c: char) -> usize {
    s.split(|x| x != c).map(str::len).max().unwrap_or(0)
}

/// Math on one line, without blanks at its ends.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> String {
        let doc = Doc::default();
        let mut w = MdWriter {
            doc: &doc,
            folder: String::new(),
            files: Vec::new(),
            notes: Vec::new(),
            warnings: Warnings::default(),
            depth: 0,
            bol: true,
        };
        let mut out = String::new();
        w.text(s, &mut out);
        out
    }

    #[test]
    fn escaping() {
        assert_eq!(text("1. Not a list"), "1\\. Not a list");
        assert_eq!(text("- no"), "\\- no");
        assert_eq!(text("# no"), "\\# no");
        assert_eq!(
            text("a *b* [c] $d$ `e` <f>"),
            "a \\*b\\* \\[c\\] \\$d\\$ \\`e\\` \\<f>"
        );
        assert_eq!(text("snake_case _x_"), "snake_case \\_x\\_");
        assert_eq!(text("A & B &amp; Hi!"), "A & B \\&amp; Hi!");
        assert_eq!(text("    indented"), "indented");
        assert_eq!(code_span("a`b"), "``a`b``");
        assert_eq!(code_span("`x"), "`` `x ``");
        assert_eq!(destination("my pic.png"), "<my pic.png>");
    }
}
