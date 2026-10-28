//! Markdown (CommonMark with tables, footnotes, strikethrough, task lists
//! and math) via `pulldown-cmark`.
//!
//! - Pandoc citations (`[@key]`, `[@a; @b, p. 4]`, `@key`) become
//!   citations; LaTeX math becomes Typst math ([`crate::latex`]).
//! - Images with a relative path are read through
//!   [`ReadOptions::resolve`] into resources; one standing alone in a
//!   paragraph is a figure captioned by its alt text. Remote images become
//!   links.
//! - Raw HTML is dropped (inline tags' text stays, `<br>` is a line break),
//!   task list checkboxes too; YAML front matter gives the title, subtitle,
//!   author(s), date and lang.

use std::collections::{HashMap, HashSet};

use pulldown_cmark::{CodeBlockKind, Event, LinkType, Options, Parser, Tag};

use crate::model::{
    Block, Cell, Cite, CiteItem, Doc, Figure, FigureBody, Image, Inline, List, Math, Meta,
    Resource, Table,
};
use crate::{
    Error, Imported, MAX_DEPTH, ReadOptions, Reader, Result, WarningKind, Warnings, latex, text,
};

/// The Markdown reader.
pub struct Markdown;

const OPTIONS: Options = Options::ENABLE_TABLES
    .union(Options::ENABLE_FOOTNOTES)
    .union(Options::ENABLE_STRIKETHROUGH)
    .union(Options::ENABLE_TASKLISTS)
    .union(Options::ENABLE_MATH)
    .union(Options::ENABLE_HEADING_ATTRIBUTES)
    .union(Options::ENABLE_YAML_STYLE_METADATA_BLOCKS);

impl Reader for Markdown {
    fn read(&self, bytes: &[u8], options: &ReadOptions) -> Result<Imported> {
        let mut warnings = Warnings::default();
        let source = text::decode(bytes, &mut warnings)?;
        let events: Vec<Event> = Parser::new_ext(&source, OPTIONS).collect();
        let mut c = Converter {
            notes: notes(&events),
            events,
            pos: 0,
            depth: 0,
            used: HashSet::new(),
            active: HashSet::new(),
            meta: Meta::default(),
            resources: Vec::new(),
            warnings,
            options,
        };
        let blocks = c.blocks()?;
        let mut unused: Vec<&String> = c.notes.keys().filter(|k| !c.used.contains(*k)).collect();
        unused.sort();
        for label in unused {
            c.warnings
                .add_example(WarningKind::UnusedFootnote, format!("[^{label}]"));
        }
        Ok(Imported {
            doc: Doc {
                meta: c.meta,
                blocks,
                resources: c.resources,
            },
            warnings: c.warnings.into_vec(),
        })
    }
}

/// Footnote definitions by label: the events between their start and end.
fn notes(events: &[Event]) -> HashMap<String, (usize, usize)> {
    let mut notes = HashMap::new();
    let mut i = 0;
    while i < events.len() {
        if let Event::Start(Tag::FootnoteDefinition(label)) = &events[i] {
            let mut depth = 0;
            let mut end = i + 1;
            while end < events.len() {
                match &events[end] {
                    Event::Start(_) => depth += 1,
                    Event::End(_) if depth == 0 => break,
                    Event::End(_) => depth -= 1,
                    _ => {}
                }
                end += 1;
            }
            notes.entry(label.to_string()).or_insert((i + 1, end));
            i = end;
        }
        i += 1;
    }
    notes
}

/// Where an inline run ends.
#[derive(Clone, Copy, PartialEq)]
enum Until {
    /// At the next end tag (consumed): a paragraph, heading, cell or span.
    End,
    /// Before the next block or end tag: the text of a tight list item.
    Block,
}

struct Converter<'e, 'o> {
    events: Vec<Event<'e>>,
    pos: usize,
    depth: usize,
    notes: HashMap<String, (usize, usize)>,
    /// Footnotes referenced, and those being read (a note citing itself).
    used: HashSet<String>,
    active: HashSet<String>,
    meta: Meta,
    resources: Vec<Resource>,
    warnings: Warnings,
    options: &'o ReadOptions<'o>,
}

impl<'e> Converter<'e, '_> {
    fn enter(&mut self) -> Result<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Error::TooDeep);
        }
        Ok(())
    }

    fn peek(&self) -> Option<Event<'e>> {
        self.events.get(self.pos).cloned()
    }

    /// Blocks up to the end tag closing the container (consumed) or the end.
    fn blocks(&mut self) -> Result<Vec<Block>> {
        self.enter()?;
        let mut out = Vec::new();
        while let Some(event) = self.peek() {
            match event {
                Event::End(_) => {
                    self.pos += 1;
                    break;
                }
                Event::Rule => {
                    self.pos += 1;
                    out.push(Block::Rule);
                }
                Event::Start(tag) if is_block(&tag) => {
                    self.pos += 1;
                    self.block(tag, &mut out)?;
                }
                _ => self.para(Until::Block, &mut out)?,
            }
        }
        self.depth -= 1;
        Ok(out)
    }

    /// The block `tag` starts (its start consumed).
    fn block(&mut self, tag: Tag<'e>, out: &mut Vec<Block>) -> Result<()> {
        match tag {
            Tag::Paragraph => self.para(Until::End, out)?,
            Tag::Heading { level, id, .. } => {
                let (content, _) = self.inlines(Until::End, false)?;
                out.push(Block::Heading {
                    level: level as u8,
                    content: trim(content),
                    label: id.map(|id| id.to_string()),
                });
            }
            Tag::BlockQuote(_) => out.push(Block::Quote(self.blocks()?)),
            Tag::CodeBlock(kind) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().map(str::to_owned)
                    }
                    CodeBlockKind::Indented => None,
                };
                out.push(Block::Code {
                    lang,
                    text: self.raw_text(),
                });
            }
            Tag::HtmlBlock => {
                let html = self.raw_text();
                if !is_comment(&html) {
                    self.warnings
                        .add_example(WarningKind::Html, first_tag(&html));
                }
            }
            Tag::List(start) => out.push(self.list(start)?),
            Tag::FootnoteDefinition(label) => {
                // Read where it is referenced.
                self.pos = self.notes.get(label.as_ref()).map_or(self.pos, |n| n.1 + 1);
            }
            Tag::Table(aligns) => out.push(self.table(aligns.len())?),
            Tag::MetadataBlock(_) => {
                let yaml = self.raw_text();
                self.front_matter(&yaml);
            }
            // Items, rows and cells outside their container, definition
            // lists (not enabled): their content as blocks.
            _ => out.extend(self.blocks()?),
        }
        Ok(())
    }

    /// The text events up to the end tag (consumed).
    fn raw_text(&mut self) -> String {
        let mut s = String::new();
        while let Some(event) = self.peek() {
            self.pos += 1;
            match event {
                Event::End(_) => break,
                Event::Text(t) | Event::Html(t) | Event::Code(t) => s.push_str(&t),
                _ => {}
            }
        }
        s
    }

    /// A paragraph, split where display math stands in it.
    fn para(&mut self, until: Until, out: &mut Vec<Block>) -> Result<()> {
        loop {
            let (content, display) = self.inlines(until, true)?;
            push_para(content, out);
            match display {
                Some(math) => out.push(Block::Math {
                    math,
                    label: None,
                    number: None,
                }),
                None => return Ok(()),
            }
        }
    }

    /// Inlines up to `until`. At the top of a paragraph (`top`) display math
    /// ends the run and is returned.
    fn inlines(&mut self, until: Until, top: bool) -> Result<(Vec<Inline>, Option<Math>)> {
        self.enter()?;
        let mut out = Vec::new();
        let mut display = None;
        while let Some(event) = self.peek() {
            match event {
                Event::End(_) => {
                    if until == Until::End {
                        self.pos += 1;
                    }
                    break;
                }
                Event::Start(ref tag) if is_block(tag) => break,
                Event::Rule => break,
                Event::DisplayMath(m) if top => {
                    self.pos += 1;
                    display = Some(self.math(&m));
                    break;
                }
                _ => {
                    self.pos += 1;
                    if let Some(i) = self.inline(event)? {
                        out.push(i);
                    }
                }
            }
        }
        self.depth -= 1;
        Ok((finish(out), display))
    }

    fn inline(&mut self, event: Event<'e>) -> Result<Option<Inline>> {
        let span = |c: &mut Self| c.inlines(Until::End, false).map(|(i, _)| i);
        Ok(Some(match event {
            Event::Start(Tag::Emphasis) => Inline::Emph(span(self)?),
            Event::Start(Tag::Strong) => Inline::Strong(span(self)?),
            Event::Start(Tag::Strikethrough) => Inline::Strike(span(self)?),
            Event::Start(Tag::Superscript) => Inline::Super(span(self)?),
            Event::Start(Tag::Subscript) => Inline::Sub(span(self)?),
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                ..
            }) => {
                let mut url = dest_url.to_string();
                if link_type == LinkType::Email && !url.starts_with("mailto:") {
                    url.insert_str(0, "mailto:");
                }
                Inline::Link {
                    url,
                    content: span(self)?,
                }
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                let alt = plain(&span(self)?);
                return Ok(self.image(&dest_url, alt));
            }
            Event::Start(_) => return Ok(None),
            Event::Text(t) => Inline::Text(t.to_string()),
            Event::Code(c) => Inline::Code(c.to_string()),
            Event::InlineMath(m) | Event::DisplayMath(m) => Inline::Math(self.math(&m)),
            Event::Html(h) | Event::InlineHtml(h) => {
                let tag = h.trim().to_ascii_lowercase();
                if matches!(tag.as_str(), "<br>" | "<br/>" | "<br />") {
                    Inline::LineBreak
                } else {
                    if !is_comment(&h) {
                        self.warnings.add_example(WarningKind::Html, first_tag(&h));
                    }
                    return Ok(None);
                }
            }
            Event::FootnoteReference(label) => self.footnote(&label)?,
            Event::SoftBreak => Inline::Text(" ".into()),
            Event::HardBreak => Inline::LineBreak,
            Event::TaskListMarker(_) => {
                self.warnings.add(WarningKind::TaskList);
                return Ok(None);
            }
            Event::End(_) | Event::Rule => return Ok(None),
        }))
    }

    fn math(&mut self, latex: &str) -> Math {
        Math::typst(latex::to_typst(latex, &mut self.warnings))
    }

    fn footnote(&mut self, label: &str) -> Result<Inline> {
        let Some(&(start, _)) = self
            .notes
            .get(label)
            .filter(|_| !self.active.contains(label))
        else {
            let written = format!("[^{label}]");
            self.warnings
                .add_example(WarningKind::MissingFootnote, written.clone());
            return Ok(Inline::Text(written));
        };
        let back = self.pos;
        self.pos = start;
        self.active.insert(label.to_owned());
        let blocks = self.blocks();
        self.active.remove(label);
        self.pos = back;
        self.used.insert(label.to_owned());
        Ok(Inline::Footnote(blocks?))
    }

    /// The image at `dest`: a resource if it can be read, a link if remote.
    fn image(&mut self, dest: &str, alt: String) -> Option<Inline> {
        let dest = percent_decode(dest);
        let alt = Some(alt.trim().to_owned()).filter(|a| !a.is_empty());
        if dest.starts_with("http://") || dest.starts_with("https://") || dest.starts_with("//") {
            self.warnings
                .add_example(WarningKind::RemoteImage, dest.clone());
            let shown = alt.unwrap_or_else(|| dest.clone());
            return Some(Inline::Link {
                url: dest,
                content: vec![Inline::Text(shown)],
            });
        }
        let image = |resource: String| {
            Some(Inline::Image(Image {
                resource,
                alt: alt.clone(),
                width: None,
            }))
        };
        if self.resources.iter().any(|r| r.name == dest) {
            return image(dest);
        }
        let scheme = dest
            .split_once(':')
            .is_some_and(|(s, _)| s.len() > 1 && s.chars().all(|c| c.is_ascii_alphanumeric()));
        let bytes = if scheme || dest.is_empty() {
            None
        } else {
            (self.options.resolve)(&dest)
        };
        let Some(bytes) = bytes else {
            let shown: String = dest.chars().take(40).collect();
            self.warnings.add_example(WarningKind::MissingImage, shown);
            return None;
        };
        self.resources.push(Resource {
            mime: mime(&dest).to_owned(),
            name: dest.clone(),
            bytes,
            fallback: None,
        });
        image(dest)
    }

    fn list(&mut self, start: Option<u64>) -> Result<Block> {
        let mut items = Vec::new();
        while let Some(event) = self.peek() {
            self.pos += 1;
            match event {
                Event::Start(Tag::Item) => items.push(self.blocks()?),
                Event::End(_) => break,
                _ => {}
            }
        }
        Ok(Block::List(List {
            ordered: start.is_some(),
            start: start.unwrap_or(1),
            items,
        }))
    }

    fn table(&mut self, columns: usize) -> Result<Block> {
        let mut rows = Vec::new();
        let mut header_rows = 0;
        while let Some(event) = self.peek() {
            self.pos += 1;
            match event {
                Event::Start(Tag::TableHead) => {
                    rows.push(self.row()?);
                    header_rows = 1;
                }
                Event::Start(Tag::TableRow) => rows.push(self.row()?),
                Event::End(_) => break,
                _ => {}
            }
        }
        for row in &mut rows {
            row.resize_with(columns, || Cell::new(Vec::new()));
        }
        Ok(Block::Table(Table {
            columns,
            header_rows,
            rows,
        }))
    }

    /// The cells of a row (or of the header) up to its end.
    fn row(&mut self) -> Result<Vec<Cell>> {
        let mut cells = Vec::new();
        while let Some(event) = self.peek() {
            self.pos += 1;
            match event {
                Event::Start(Tag::TableCell) => {
                    let content = trim(self.inlines(Until::End, false)?.0);
                    cells.push(Cell::new(match content.is_empty() {
                        true => Vec::new(),
                        false => vec![Block::Para(content)],
                    }));
                }
                Event::End(_) => break,
                _ => {}
            }
        }
        Ok(cells)
    }

    /// YAML front matter, the trivial part: `title`, `subtitle`, `author`
    /// (one, a `[a, b]` list or `- ` items), `authors`, `date`, `lang`.
    fn front_matter(&mut self, yaml: &str) {
        let mut key = String::new();
        for line in yaml.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed == "---" {
                continue;
            }
            if line.starts_with([' ', '\t', '-']) {
                match trimmed.strip_prefix("- ") {
                    Some(item) if key == "author" || key == "authors" => {
                        self.meta.authors.push(unquote(item));
                    }
                    _ => {}
                }
                continue;
            }
            let Some((k, value)) = line.split_once(':') else {
                continue;
            };
            key = k.trim().to_owned();
            let value = value.trim();
            match key.as_str() {
                _ if value.is_empty() => {}
                "title" => self.meta.title = Some(unquote(value)),
                "subtitle" => self.meta.subtitle = Some(unquote(value)),
                "date" => self.meta.date = Some(unquote(value)),
                "lang" => self.meta.lang = Some(unquote(value)),
                "author" | "authors" => {
                    match value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
                        Some(list) => self
                            .meta
                            .authors
                            .extend(list.split(',').map(unquote).filter(|a| !a.is_empty())),
                        None => self.meta.authors.push(unquote(value)),
                    }
                }
                _ => self
                    .warnings
                    .add_example(WarningKind::FrontMatter, key.clone()),
            }
            if value.is_empty() && !matches!(key.as_str(), "author" | "authors") {
                self.warnings
                    .add_example(WarningKind::FrontMatter, key.clone());
            }
        }
    }
}

fn is_block(tag: &Tag) -> bool {
    !matches!(
        tag,
        Tag::Emphasis
            | Tag::Strong
            | Tag::Strikethrough
            | Tag::Superscript
            | Tag::Subscript
            | Tag::Link { .. }
            | Tag::Image { .. }
    )
}

/// Adds a paragraph unless it is empty; an image standing alone becomes a
/// figure captioned by its alt text.
fn push_para(content: Vec<Inline>, out: &mut Vec<Block>) {
    let content = trim(content);
    match &content[..] {
        [] => {}
        [Inline::Image(img)] => out.push(Block::Figure(Figure {
            caption: img.alt.iter().map(|a| Inline::Text(a.clone())).collect(),
            body: FigureBody::Image(img.clone()),
            label: None,
            number: None,
        })),
        _ => out.push(Block::Para(content)),
    }
}

/// Without whitespace and line breaks at either end.
pub(crate) fn trim(mut content: Vec<Inline>) -> Vec<Inline> {
    while let Some(Inline::Text(t)) = content.first_mut() {
        *t = t.trim_start().to_owned();
        if !t.is_empty() {
            break;
        }
        content.remove(0);
    }
    while let Some(last) = content.last_mut() {
        match last {
            Inline::Text(t) => {
                *t = t.trim_end().to_owned();
                if !t.is_empty() {
                    break;
                }
            }
            Inline::LineBreak => {}
            _ => break,
        }
        content.pop();
    }
    content
}

/// Adjacent text merged, then citations taken out of it.
fn finish(items: Vec<Inline>) -> Vec<Inline> {
    let mut merged: Vec<Inline> = Vec::with_capacity(items.len());
    for item in items {
        match (merged.last_mut(), item) {
            (Some(Inline::Text(prev)), Inline::Text(t)) => prev.push_str(&t),
            (_, item) => merged.push(item),
        }
    }
    merged
        .into_iter()
        .flat_map(|i| match i {
            Inline::Text(t) if t.contains('@') => citations(&t),
            i => vec![i],
        })
        .collect()
}

/// Text with Pandoc citations (`[@a, p. 4; @b]`, `@key`) as citations.
fn citations(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut plain = String::new();
    let mut i = 0;
    while let Some(c) = text[i..].chars().next() {
        let rest = &text[i..];
        let found = match c {
            '[' => bracketed(rest),
            '@' if !plain.chars().last().is_some_and(char::is_alphanumeric) => {
                key(&rest[1..]).map(|(k, len)| {
                    let cite = Cite {
                        items: vec![CiteItem {
                            key: k.to_owned(),
                            locator: None,
                        }],
                        text: Some(vec![Inline::Text(rest[..=len].to_owned())]),
                    };
                    (cite, len + 1)
                })
            }
            _ => None,
        };
        match found {
            Some((cite, len)) => {
                if !plain.is_empty() {
                    out.push(Inline::Text(std::mem::take(&mut plain)));
                }
                out.push(Inline::Cite(cite));
                i += len;
            }
            None => {
                plain.push(c);
                i += c.len_utf8();
            }
        }
    }
    if !plain.is_empty() {
        out.push(Inline::Text(plain));
    }
    out
}

/// `[@a, p. 4; -@b]` at the start of `s`: the citation and its length.
fn bracketed(s: &str) -> Option<(Cite, usize)> {
    let end = s.find(']')?;
    let inner = &s[1..end];
    if inner.contains('[') {
        return None;
    }
    let mut items = Vec::new();
    for part in inner.split(';') {
        let part = part.trim();
        let part = part.strip_prefix('-').unwrap_or(part).strip_prefix('@')?;
        let (k, len) = key(part)?;
        let rest = part[len..].trim();
        let locator = match rest.strip_prefix(',') {
            Some(l) if !l.trim().is_empty() => Some(l.trim().to_owned()),
            None if rest.is_empty() => None,
            _ => return None,
        };
        items.push(CiteItem {
            key: k.to_owned(),
            locator,
        });
    }
    let cite = Cite {
        items,
        text: Some(vec![Inline::Text(s[..=end].to_owned())]),
    };
    Some((cite, end + 1))
}

/// A citation key at the start of `s` (letters, digits, `_`, and `-:./`
/// inside): the key and its length in bytes.
fn key(s: &str) -> Option<(&str, usize)> {
    if !s
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '_')
    {
        return None;
    }
    let len = s
        .find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '-' | ':' | '.' | '/')))
        .unwrap_or(s.len());
    let k = s[..len].trim_end_matches(|c: char| !(c.is_alphanumeric() || c == '_'));
    Some((k, k.len()))
}

/// The plain text of inlines (an image's alt text).
pub(crate) fn plain(items: &[Inline]) -> String {
    let mut s = String::new();
    for i in items {
        match i {
            Inline::Text(t) | Inline::Code(t) => s.push_str(t),
            Inline::Emph(c)
            | Inline::Strong(c)
            | Inline::Underline(c)
            | Inline::Strike(c)
            | Inline::Super(c)
            | Inline::Sub(c)
            | Inline::Link { content: c, .. } => s.push_str(&plain(c)),
            Inline::Cite(Cite { text: Some(c), .. }) => s.push_str(&plain(c)),
            Inline::LineBreak => s.push(' '),
            _ => {}
        }
    }
    s
}

fn is_comment(html: &str) -> bool {
    html.trim_start().starts_with("<!--")
}

/// The first tag of an HTML snippet, for a warning (`<div>`).
fn first_tag(html: &str) -> String {
    let html = html.trim();
    match html.find('>') {
        Some(end) if html.starts_with('<') => html[..=end].to_owned(),
        _ => html.chars().take(20).collect(),
    }
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    for q in ['"', '\''] {
        if let Some(inner) = s.strip_prefix(q).and_then(|s| s.strip_suffix(q)) {
            return inner.to_owned();
        }
    }
    s.to_owned()
}

/// `%XX` escapes decoded; the input as is if that isn't UTF-8.
pub(crate) fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| char::from(b).to_digit(16);
        match (
            bytes[i],
            bytes.get(i + 1).and_then(|&b| hex(b)),
            bytes.get(i + 2).and_then(|&b| hex(b)),
        ) {
            (b'%', Some(hi), Some(lo)) => {
                out.push((hi * 16 + lo) as u8);
                i += 3;
            }
            (b, _, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

/// The media type by extension.
pub(crate) fn mime(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn citation_forms() {
        let cite = |s: &str| citations(s);
        assert!(
            matches!(&cite("see [@doe, p. 4; -@roe]")[..], [Inline::Text(t), Inline::Cite(c)]
            if t == "see " && c.items.len() == 2 && c.items[0].locator.as_deref() == Some("p. 4") && c.items[1].key == "roe")
        );
        assert!(
            matches!(&cite("@doe2020.")[..], [Inline::Cite(c), Inline::Text(t)] if c.items[0].key == "doe2020" && t == ".")
        );
        assert!(matches!(&cite("mail a@b.org")[..], [Inline::Text(_)]));
        // A prefix isn't supported: the brackets stay text around a bare citation.
        assert!(
            matches!(&cite("[see @doe]")[..], [Inline::Text(a), Inline::Cite(_), Inline::Text(b)] if a == "[see " && b == "]")
        );
        assert!(matches!(&cite("[@]")[..], [Inline::Text(_)]));
    }

    #[test]
    fn decoding_helpers() {
        assert_eq!(percent_decode("my%20pic.png"), "my pic.png");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(first_tag("<div class=\"x\">a</div>"), "<div class=\"x\">");
        assert_eq!(unquote("'A'"), "A");
    }
}
