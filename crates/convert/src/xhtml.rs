//! The markup HTML and EPUB share: blocks and inlines as polyglot markup,
//! well-formed XML and valid HTML5 at once (void elements closed with
//! ` />`, only the five XML entities, MathML with its namespace and checked
//! to be XML), so the HTML file and EPUB's XHTML content documents come
//! from one serializer. What differs is the [`Target`]: heading levels,
//! where images come from, how footnotes look.
//!
//! Labels become ids (each once); references link to the file and id of
//! what they name, or stay text. Formulas without (well-formed) MathML
//! show their Typst source as code, with a warning.

use std::collections::{HashMap, HashSet};

use crate::model::{Block, Cite, Doc, Figure, FigureBody, Image, Inline, List, Math, Table};
use crate::{Error, MAX_DEPTH, Result, WarningKind, Warnings};

const MATHML: &str = "http://www.w3.org/1998/Math/MathML";

/// What the markup is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    /// One HTML file: headings from `<h2>` (the title is the `<h1>`),
    /// images as `data:` URLs, footnotes as endnotes.
    Html,
    /// EPUB content documents: headings from `<h1>`, images as files
    /// ([`Markup::images`]), footnotes as pop-up notes at the end of their
    /// file, a figure's image without alt text described by its caption,
    /// links only to URLs with a scheme.
    Epub,
}

/// A heading for the table of contents.
pub(crate) struct Heading {
    pub level: u8,
    pub text: String,
    /// Its id in its file.
    pub id: String,
}

/// Writes blocks and inlines of one document, file after file.
pub(crate) struct Markup<'d> {
    doc: &'d Doc,
    target: Target,
    /// The file each label's element is in (all `""` for HTML).
    targets: HashMap<&'d str, String>,
    /// The file being written, so links within it are `#id` alone.
    pub file: String,
    /// EPUB: image sources by resource name; resources not in it are left
    /// out.
    pub images: HashMap<&'d str, String>,
    /// The resources shown so far.
    pub shown: HashSet<&'d str>,
    /// Images shown without alt text (the accessibility metadata says so).
    pub without_alt: usize,
    /// EPUB: the bibliography's list, shown without bullets.
    pub bibliography: Option<&'d List>,
    /// EPUB: top-level headings of levels 1–3 written so far (each gets an
    /// id); `None` collects none.
    pub outline: Option<Vec<Heading>>,
    pub warnings: Warnings,
    /// Ids written so far: a label shown twice gets its id once.
    ids: HashSet<String>,
    /// Footnotes waiting for the end of their file, with their numbers.
    notes: Vec<(usize, &'d [Block])>,
    /// Footnotes numbered so far (per book).
    numbered: usize,
    /// Ids made up for headings so far.
    made_up: usize,
    depth: usize,
}

impl<'d> Markup<'d> {
    /// Markup for `doc`; `targets` gives each label's file.
    pub fn new(doc: &'d Doc, target: Target, targets: HashMap<&'d str, String>) -> Self {
        Markup {
            doc,
            target,
            targets,
            file: String::new(),
            images: HashMap::new(),
            shown: HashSet::new(),
            without_alt: 0,
            bibliography: None,
            outline: None,
            warnings: Warnings::default(),
            ids: HashSet::new(),
            notes: Vec::new(),
            numbered: 0,
            made_up: 0,
            depth: 0,
        }
    }

    fn enter(&mut self) -> Result<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Error::TooDeep);
        }
        Ok(())
    }

    pub fn blocks(&mut self, blocks: &'d [Block]) -> Result<String> {
        self.enter()?;
        let mut out = String::new();
        for b in blocks {
            out.push_str(&self.block(b)?);
            out.push('\n');
        }
        self.depth -= 1;
        Ok(out)
    }

    /// The footnotes referenced since the last call, numbered on as they
    /// turn up (notes may hold notes): HTML's endnotes section, EPUB's
    /// `<aside>`s.
    pub fn notes(&mut self) -> Result<String> {
        let outline = self.outline.take();
        let mut out = String::new();
        let mut done = 0;
        while done < self.notes.len() {
            let (n, blocks) = self.notes[done];
            let mut inner = self.blocks(blocks)?;
            let (back, open) = match self.target {
                Target::Html => (format!("#fnref{n}"), format!("<li id=\"fn{n}\">")),
                Target::Epub => (
                    format!("#fnref-{n}"),
                    format!(
                        "<aside epub:type=\"footnote\" role=\"doc-footnote\" class=\"footnote\" id=\"fn-{n}\">"
                    ),
                ),
            };
            let back = format!(" <a href=\"{back}\" role=\"doc-backlink\">↩</a>");
            match inner.rfind("</p>") {
                Some(at) if inner[at..].trim_end() == "</p>" => inner.insert_str(at, &back),
                _ => inner.push_str(&back),
            }
            let close = match self.target {
                Target::Html => "</li>",
                Target::Epub => "</aside>",
            };
            out.push_str(&format!("{open}\n{inner}{close}\n"));
            done += 1;
        }
        self.notes.clear();
        self.outline = outline;
        if self.target == Target::Html && !out.is_empty() {
            out = format!(
                "<section class=\"footnotes\" role=\"doc-endnotes\">\n<ol>\n{out}</ol>\n</section>\n"
            );
        }
        Ok(out)
    }

    /// One paragraph's inlines bare, else the blocks (list items, cells).
    fn tight(&mut self, blocks: &'d [Block]) -> Result<String> {
        match blocks {
            [] => Ok(String::new()),
            [Block::Para(c)] => self.inlines(c),
            _ => Ok(format!("\n{}", self.blocks(blocks)?)),
        }
    }

    /// The id for `label` if it is the first element with it.
    fn id(&mut self, label: Option<&str>) -> Option<String> {
        let label = label?;
        self.ids.insert(label.to_owned()).then(|| label.to_owned())
    }

    /// A new id for a heading without a label.
    fn fresh_id(&mut self) -> String {
        loop {
            self.made_up += 1;
            let id = format!("toc-{}", self.made_up);
            if !self.targets.contains_key(id.as_str()) && self.ids.insert(id.clone()) {
                return id;
            }
        }
    }

    fn block(&mut self, block: &'d Block) -> Result<String> {
        Ok(match block {
            Block::Para(c) => format!("<p>{}</p>", self.inlines(c)?),
            Block::Heading {
                level,
                content,
                label,
            } => {
                let inner = self.inlines(content)?;
                let mut id = self.id(label.as_deref());
                let h = match self.target {
                    Target::Html => level.saturating_add(1).clamp(2, 6),
                    Target::Epub => (*level).clamp(1, 6),
                };
                if self.depth == 1 && *level <= 3 && self.outline.is_some() {
                    let id = id.get_or_insert_with(|| self.fresh_id()).clone();
                    let text = plain(content);
                    if let Some(outline) = &mut self.outline {
                        outline.push(Heading {
                            level: *level,
                            text,
                            id,
                        });
                    }
                }
                format!("<h{h}{}>{inner}</h{h}>", id_attr(id))
            }
            Block::List(l) => self.list(l)?,
            Block::Quote(b) => format!("<blockquote>\n{}</blockquote>", self.blocks(b)?),
            Block::Code { lang, text: code } => {
                let class = lang
                    .as_ref()
                    .map(|l| format!(" class=\"language-{}\"", attr(l)))
                    .unwrap_or_default();
                format!("<pre><code{class}>{}</code></pre>", text(code))
            }
            Block::Math {
                math,
                label,
                number,
            } => {
                let number = number
                    .as_ref()
                    .map(|n| format!("<span class=\"number\">{}</span>", text(n)))
                    .unwrap_or_default();
                let id = self.id(label.as_deref());
                format!(
                    "<div class=\"equation\"{}>{}{number}</div>",
                    id_attr(id),
                    self.math(math, true)
                )
            }
            Block::Figure(f) => self.figure(f)?,
            Block::Table(t) => self.table(t)?,
            Block::Rule => "<hr />".to_owned(),
        })
    }

    fn list(&mut self, l: &'d List) -> Result<String> {
        let (tag, start) = match (l.ordered, l.start) {
            (false, _) => ("ul", String::new()),
            (true, 1) => ("ol", String::new()),
            (true, n) => ("ol", format!(" start=\"{n}\"")),
        };
        let class = match self.bibliography {
            Some(b) if std::ptr::eq(b, l) => " class=\"bibliography\"",
            _ => "",
        };
        let mut out = format!("<{tag}{class}{start}>\n");
        for item in &l.items {
            out.push_str(&format!("<li>{}</li>\n", self.tight(item)?));
        }
        out.push_str(&format!("</{tag}>"));
        Ok(out)
    }

    fn figure(&mut self, f: &'d Figure) -> Result<String> {
        let mut caption = String::new();
        if let Some(n) = &f.number {
            caption.push_str(&format!("<span class=\"number\">{}</span>", text(n)));
            if !f.caption.is_empty() {
                caption.push_str(": ");
            }
        }
        caption.push_str(&self.inlines(&f.caption)?);
        let caption = match caption.is_empty() {
            true => String::new(),
            false => format!("<figcaption>{caption}</figcaption>"),
        };
        let id = self.id(f.label.as_deref());
        // Tables are captioned above, images below.
        let (before, body, after) = match &f.body {
            FigureBody::Image(img) => (String::new(), self.image(img, &f.caption), caption),
            FigureBody::Table(t) => (caption, self.table(t)?, String::new()),
        };
        let parts: Vec<String> = [before, body, after]
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect();
        Ok(format!(
            "<figure{}>\n{}\n</figure>",
            id_attr(id),
            parts.join("\n")
        ))
    }

    fn table(&mut self, t: &'d Table) -> Result<String> {
        let mut out = String::from("<table>\n");
        for (i, row) in t.rows.iter().enumerate() {
            let head = i < t.header_rows;
            if i == 0 && head {
                out.push_str("<thead>\n");
            }
            if i == t.header_rows {
                out.push_str("<tbody>\n");
            }
            out.push_str("<tr>");
            for cell in row {
                let tag = if head { "th" } else { "td" };
                let mut spans = String::new();
                if cell.colspan > 1 {
                    spans.push_str(&format!(" colspan=\"{}\"", cell.colspan));
                }
                if cell.rowspan > 1 {
                    spans.push_str(&format!(" rowspan=\"{}\"", cell.rowspan));
                }
                out.push_str(&format!(
                    "<{tag}{spans}>{}</{tag}>",
                    self.tight(&cell.content)?
                ));
            }
            out.push_str("</tr>\n");
            if i + 1 == t.header_rows {
                out.push_str("</thead>\n");
            }
        }
        if t.rows.len() > t.header_rows {
            out.push_str("</tbody>\n");
        }
        out.push_str("</table>");
        Ok(out)
    }

    /// An image; `caption` describes a figure's image without alt text
    /// (EPUB).
    fn image(&mut self, img: &'d Image, caption: &[Inline]) -> String {
        let mut alt = img.alt.clone().filter(|a| !a.trim().is_empty());
        if alt.is_none() && self.target == Target::Epub {
            alt = Some(plain(caption)).filter(|a| !a.is_empty());
        }
        let src = match self.target {
            Target::Html => match self.doc.resource(&img.resource) {
                Some(r) => format!("data:{};base64,{}", r.mime, base64(&r.bytes)),
                None => img.resource.clone(),
            },
            Target::Epub => match self.images.get(img.resource.as_str()) {
                Some(src) => src.clone(),
                None => {
                    let kind = match self.doc.resource(&img.resource) {
                        Some(_) => WarningKind::ImageLeftOut,
                        None => WarningKind::MissingImage,
                    };
                    self.warnings.add_example(kind, &img.resource);
                    return alt.map(|a| text(&format!("[{a}]"))).unwrap_or_default();
                }
            },
        };
        self.shown.insert(img.resource.as_str());
        if alt.is_none() {
            self.without_alt += 1;
        }
        let style = img
            .width
            .map(|w| format!(" style=\"width: {}%\"", (w * 100.0).round()))
            .unwrap_or_default();
        format!(
            "<img src=\"{}\" alt=\"{}\"{style} />",
            attr(&src),
            attr(alt.as_deref().unwrap_or(""))
        )
    }

    fn math(&mut self, math: &Math, display: bool) -> String {
        match math.mathml.as_deref().and_then(|m| mathml(m, display)) {
            Some(m) => m,
            None => {
                self.warnings.add(WarningKind::TypstMath);
                format!("<code class=\"math\">{}</code>", text(&math.typst))
            }
        }
    }

    fn inlines(&mut self, inlines: &'d [Inline]) -> Result<String> {
        self.enter()?;
        let mut out = String::new();
        for i in inlines {
            let span = |w: &mut Self, tag: &str, c: &'d [Inline]| -> Result<String> {
                Ok(format!("<{tag}>{}</{tag}>", w.inlines(c)?))
            };
            out.push_str(&match i {
                Inline::Text(t) => text(t),
                Inline::Emph(c) => span(self, "em", c)?,
                Inline::Strong(c) => span(self, "strong", c)?,
                Inline::Underline(c) => span(self, "u", c)?,
                Inline::Strike(c) => span(self, "s", c)?,
                Inline::Super(c) => span(self, "sup", c)?,
                Inline::Sub(c) => span(self, "sub", c)?,
                Inline::Code(c) => format!("<code>{}</code>", text(c)),
                Inline::LineBreak => "<br />\n".to_owned(),
                Inline::Link { url, content } => {
                    let shown = match content.is_empty() {
                        true => text(url),
                        false => self.inlines(content)?,
                    };
                    // An EPUB has no files but its own: a relative link
                    // goes nowhere.
                    match self.target == Target::Html || has_scheme(url) {
                        true => format!("<a href=\"{}\">{shown}</a>", attr(url)),
                        false => shown,
                    }
                }
                Inline::Math(m) => self.math(m, false),
                Inline::Image(img) => self.image(img, &[]),
                Inline::Footnote(blocks) => {
                    self.numbered += 1;
                    let n = self.numbered;
                    self.notes.push((n, blocks));
                    match self.target {
                        Target::Html => format!(
                            "<sup id=\"fnref{n}\"><a href=\"#fn{n}\" role=\"doc-noteref\">{n}</a></sup>"
                        ),
                        Target::Epub => format!(
                            "<sup><a epub:type=\"noteref\" role=\"doc-noteref\" href=\"#fn-{n}\" id=\"fnref-{n}\">{n}</a></sup>"
                        ),
                    }
                }
                Inline::Cite(cite) => self.cite(cite)?,
                Inline::Ref { label, text: shown } => {
                    let shown = match shown {
                        Some(c) => self.inlines(c)?,
                        None => text(label),
                    };
                    // A link only to what is there.
                    match self.targets.get(label.as_str()) {
                        Some(file) => {
                            let href = match *file == self.file {
                                true => format!("#{label}"),
                                false => format!("{file}#{label}"),
                            };
                            format!("<a href=\"{}\">{shown}</a>", attr(&href))
                        }
                        None => {
                            self.warnings
                                .add_example(WarningKind::UnknownCitation, label);
                            shown
                        }
                    }
                }
            });
        }
        self.depth -= 1;
        Ok(out)
    }

    fn cite(&mut self, cite: &'d Cite) -> Result<String> {
        let keys: Vec<&str> = cite.items.iter().map(|i| i.key.as_str()).collect();
        let shown = match &cite.text {
            Some(c) => self.inlines(c)?,
            None => {
                let items: Vec<String> = cite
                    .items
                    .iter()
                    .map(|i| match &i.locator {
                        Some(l) => format!("@{}, {l}", i.key),
                        None => format!("@{}", i.key),
                    })
                    .collect();
                text(&format!("[{}]", items.join("; ")))
            }
        };
        Ok(format!(
            "<span class=\"citation\" data-cites=\"{}\">{shown}</span>",
            attr(&keys.join(" "))
        ))
    }
}

/// MathML that is one well-formed `<math>` element, with its namespace
/// (and `display="block"` for a display formula); `None` otherwise.
fn mathml(source: &str, display: bool) -> Option<String> {
    let source = source.trim();
    let doc = roxmltree::Document::parse(source).ok()?;
    let root = doc.root_element();
    if root.tag_name().name() != "math" || !source.starts_with("<math") {
        return None;
    }
    let mut out = source.to_owned();
    if display && root.attribute("display").is_none() {
        out.insert_str(5, " display=\"block\"");
    }
    match root.tag_name().namespace() {
        None => out.insert_str(5, &format!(" xmlns=\"{MATHML}\"")),
        Some(MATHML) => {}
        Some(_) => return None,
    }
    Some(out)
}

/// Whether `url` starts with a scheme (`https:`, `mailto:`).
pub(crate) fn has_scheme(url: &str) -> bool {
    url.split_once(':').is_some_and(|(scheme, _)| {
        scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

/// The labels of headings, formulas and figures in `blocks` (also in
/// lists, quotes and table cells), in order.
pub(crate) fn labels<'d>(blocks: &'d [Block], out: &mut Vec<&'d str>) {
    fn walk<'d>(blocks: &'d [Block], out: &mut Vec<&'d str>, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let table = |t: &'d Table, out: &mut Vec<&'d str>| {
            for cell in t.rows.iter().flatten() {
                walk(&cell.content, out, depth + 1);
            }
        };
        for b in blocks {
            match b {
                Block::Heading { label, .. } | Block::Math { label, .. } => {
                    out.extend(label.as_deref())
                }
                Block::Figure(f) => {
                    out.extend(f.label.as_deref());
                    if let FigureBody::Table(t) = &f.body {
                        table(t, out);
                    }
                }
                Block::List(l) => l.items.iter().for_each(|i| walk(i, out, depth + 1)),
                Block::Quote(b) => walk(b, out, depth + 1),
                Block::Table(t) => table(t, out),
                Block::Para(_) | Block::Code { .. } | Block::Rule => {}
            }
        }
    }
    walk(blocks, out, 0);
}

/// The bibliography's list: a bulleted list right after a heading at the
/// end of a document that cites (as exports have it).
pub(crate) fn bibliography(doc: &Doc) -> Option<&List> {
    match doc.blocks.as_slice() {
        [.., Block::Heading { .. }, Block::List(l)] if !l.ordered && cites(&doc.blocks, 0) => {
            Some(l)
        }
        _ => None,
    }
}

/// Whether `blocks` hold a citation.
fn cites(blocks: &[Block], depth: usize) -> bool {
    fn inline(inlines: &[Inline], depth: usize) -> bool {
        depth <= MAX_DEPTH
            && inlines.iter().any(|i| match i {
                Inline::Cite(_) => true,
                Inline::Emph(c)
                | Inline::Strong(c)
                | Inline::Underline(c)
                | Inline::Strike(c)
                | Inline::Super(c)
                | Inline::Sub(c)
                | Inline::Link { content: c, .. } => inline(c, depth + 1),
                Inline::Footnote(b) => cites(b, depth + 1),
                _ => false,
            })
    }
    let table = |t: &Table| {
        t.rows
            .iter()
            .flatten()
            .any(|c| cites(&c.content, depth + 1))
    };
    depth <= MAX_DEPTH
        && blocks.iter().any(|b| match b {
            Block::Para(c) | Block::Heading { content: c, .. } => inline(c, depth + 1),
            Block::List(l) => l.items.iter().any(|i| cites(i, depth + 1)),
            Block::Quote(b) => cites(b, depth + 1),
            Block::Figure(f) => {
                inline(&f.caption, depth + 1) || matches!(&f.body, FigureBody::Table(t) if table(t))
            }
            Block::Table(t) => table(t),
            Block::Code { .. } | Block::Math { .. } | Block::Rule => false,
        })
}

/// Inlines as plain text (a table of contents entry, alt text): spans'
/// text, formulas' source, without footnotes.
pub(crate) fn plain(inlines: &[Inline]) -> String {
    fn walk(inlines: &[Inline], out: &mut String, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        for i in inlines {
            match i {
                Inline::Text(t) | Inline::Code(t) => out.push_str(t),
                Inline::Emph(c)
                | Inline::Strong(c)
                | Inline::Underline(c)
                | Inline::Strike(c)
                | Inline::Super(c)
                | Inline::Sub(c)
                | Inline::Cite(Cite { text: Some(c), .. })
                | Inline::Ref { text: Some(c), .. } => walk(c, out, depth + 1),
                Inline::LineBreak => out.push(' '),
                Inline::Link { url, content } if content.is_empty() => out.push_str(url),
                Inline::Link { content, .. } => walk(content, out, depth + 1),
                Inline::Math(m) => out.push_str(&m.typst),
                Inline::Image(img) => out.push_str(img.alt.as_deref().unwrap_or("")),
                Inline::Cite(Cite { items, .. }) => {
                    let keys: Vec<&str> = items.iter().map(|i| i.key.as_str()).collect();
                    out.push_str(&format!("[{}]", keys.join("; ")));
                }
                Inline::Ref { label, .. } => out.push_str(label),
                Inline::Footnote(_) => {}
            }
        }
    }
    let mut out = String::new();
    walk(inlines, &mut out, 0);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// ` id="…"`, or nothing.
fn id_attr(id: Option<String>) -> String {
    id.map(|i| format!(" id=\"{}\"", attr(&i)))
        .unwrap_or_default()
}

/// Text escaped for an element's content or a double-quoted attribute
/// value: the XML entities only.
pub(crate) fn text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // Not allowed in XML (or HTML) text.
            c if c.is_control() && !matches!(c, '\n' | '\t') => {}
            '\u{fffe}' | '\u{ffff}' => {}
            c => out.push(c),
        }
    }
    out
}

/// Text escaped for a double-quoted attribute value, on one line.
pub(crate) fn attr(s: &str) -> String {
    text(s).replace(['\n', '\t'], " ")
}

/// `bytes` in base64 (with padding).
pub(crate) fn base64(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(DIGITS[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping_and_base64() {
        assert_eq!(text("a<b>&c\u{1}\u{ffff}"), "a&lt;b&gt;&amp;c");
        assert_eq!(attr("say \"hi\"\nnow"), "say &quot;hi&quot; now");
        assert_eq!(base64(b"hi"), "aGk=");
        assert_eq!(base64(b"Hello, World!"), "SGVsbG8sIFdvcmxkIQ==");
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"abc"), "YWJj");
    }

    #[test]
    fn mathml_is_checked_and_namespaced() {
        assert_eq!(
            mathml("<math><mi>x</mi></math>", true).unwrap(),
            format!("<math xmlns=\"{MATHML}\" display=\"block\"><mi>x</mi></math>")
        );
        let given = format!("<math xmlns=\"{MATHML}\" display=\"inline\"><mi>x</mi></math>");
        assert_eq!(mathml(&given, true).unwrap(), given);
        assert!(mathml("<math><mi>x</math>", false).is_none(), "not XML");
        assert!(mathml("<mi>x</mi>", false).is_none(), "not a formula");
        assert!(
            mathml("<math>&nbsp;</math>", false).is_none(),
            "HTML entity"
        );
    }

    #[test]
    fn schemes_and_plain_text() {
        assert!(has_scheme("https://x") && has_scheme("mailto:a@b"));
        assert!(!has_scheme("notes.html") && !has_scheme("#x") && !has_scheme("1:2"));
        let inlines = vec![
            Inline::Text("A ".into()),
            Inline::Emph(vec![Inline::Text("b".into())]),
            Inline::Footnote(vec![Block::Para(vec![Inline::Text("note".into())])]),
            Inline::LineBreak,
            Inline::Math(Math::typst("x^2")),
        ];
        assert_eq!(plain(&inlines), "A b x^2");
    }
}
