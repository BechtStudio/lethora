//! The DOCX writer, manuscript level: a clean Word document people go on
//! editing, built on Word's own styles so they can restyle it. A zip
//! through [`crate::zip`] with `[Content_Types].xml`, the package's and the
//! parts' relationships, `docProps/core.xml` (title, creator, language,
//! modified) and `app.xml`, and under `word/` the document, its styles,
//! numbering, footnotes (with the separators) and settings, and `media/`.
//!
//! - Styles from [`PageHints`] (the layout's body font and size, heading
//!   sizes, paragraph spacing, first-line indent, justification, page size
//!   and margins, mirrored if two-sided): Normal, Body Text and First
//!   Paragraph (the first-line indent only after a paragraph, as Typst
//!   does), Title, Subtitle, Author, Date, Heading 1–6 (with outline levels,
//!   so the navigation pane and a table of contents work), Caption,
//!   footnote text and reference, Quote, Source Code and Verbatim Char,
//!   Hyperlink, Table Grid (its first row bold), Bibliography (hanging
//!   indent), List Paragraph, Compact (table cells), Figure and Equation.
//! - A title block from the metadata (with a title). Headings keep their
//!   numbers as text; runs are bold, italic, underlined, struck, super- or
//!   subscript, code in Verbatim Char. Links are hyperlinks, references
//!   internal hyperlinks to a bookmark on the labelled heading, figure
//!   caption or equation (labels made into valid bookmark names), citations
//!   their text. Footnotes are Word's (a note in a note goes in parentheses
//!   where it was). Lists use `numbering.xml`: one abstract numbering for
//!   bullets, one for numbers, a `w:num` per list with its start. Tables
//!   get a grid of equal columns over the text width, `gridSpan`, `vMerge`
//!   and repeated header rows. A figure is its image (or table) and a
//!   Caption paragraph "Figure 1: …", the number a `SEQ` field Word can
//!   renumber, above or below as the layout puts it; the bibliography's
//!   entries are Bibliography paragraphs.
//! - Images (PNG, JPEG, GIF, SVG) are inline pictures, as wide as their
//!   share of the text width (the layout's default if none), as tall as
//!   their aspect ratio says (read from the file's header), with their alt
//!   text. An SVG goes in as Word 2016's `asvg:svgBlip` with its PNG twin
//!   ([`crate::model::Resource::fallback`]) as the picture older readers
//!   show. Formulas are their rendered images ([`crate::model::Math`]) at
//!   their rendered size: inline ones lowered onto the baseline
//!   (`w:position`), display ones centred with their number at the right
//!   margin; the Typst source is the alt text. Without an image a formula
//!   is its Typst source in code.
//!
//! [`check`] tests the structure, for the tests.

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::docx::{Docx, resolve};
use crate::epub::utc;
use crate::model::{Block, Doc, Figure, FigureBody, Image, Inline, List, Math, Resource, Table};
use crate::xhtml::{attr, bibliography, labels, plain, text};
use crate::zip::{Archive, ZipWriter};
use crate::{
    Error, MAX_DEPTH, PageHints, Result, WarningKind, Warnings, WriteOptions, Writer, Written,
};

const NS_W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const NS_WP: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const NS_PIC: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";
const NS_ASVG: &str = "http://schemas.microsoft.com/office/drawing/2016/SVG/main";
const NS_RELS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const NS_TYPES: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
/// Relationship types are this plus their name (`styles`).
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/";
/// The `a:ext` holding an SVG picture.
const SVG_EXT: &str = "{96DAC541-7B7A-43D3-8B79-37D633B846F1}";
const HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";
/// The monospace font of code.
const MONO: &str = "Courier New";
const MAIN_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml";

/// Twentieths of a point (twips), Word's page and paragraph unit.
fn twips(pt: f64) -> i64 {
    (pt * 20.0).round() as i64
}

/// English Metric Units, DrawingML's (12,700 to the point).
fn emu(pt: f64) -> i64 {
    (pt * 12_700.0).round() as i64
}

/// Half-points, Word's font sizes and run positions.
fn half_points(pt: f64) -> i64 {
    (pt * 2.0).round() as i64
}

/// Hints within what Word takes: sizes positive, the page at most 22 in,
/// margins leaving a text area.
fn sane(mut h: PageHints) -> PageHints {
    let d = PageHints::default();
    let fix = |v: f64, lo: f64, hi: f64, default: f64| match v.is_finite() {
        true => v.clamp(lo, hi),
        false => default,
    };
    h.size_pt = fix(h.size_pt, 4.0, 72.0, d.size_pt);
    if h.heading_sizes_pt.is_empty() {
        h.heading_sizes_pt = d.heading_sizes_pt.clone();
    }
    for s in &mut h.heading_sizes_pt {
        *s = fix(*s, 4.0, 96.0, d.size_pt);
    }
    h.width_pt = fix(h.width_pt, 144.0, 1584.0, d.width_pt);
    h.height_pt = fix(h.height_pt, 144.0, 1584.0, d.height_pt);
    let (w, ht) = (h.width_pt * 0.4, h.height_pt * 0.4);
    h.margin_top_pt = fix(h.margin_top_pt, 0.0, ht, d.margin_top_pt.min(ht));
    h.margin_bottom_pt = fix(h.margin_bottom_pt, 0.0, ht, d.margin_bottom_pt.min(ht));
    h.margin_inside_pt = fix(h.margin_inside_pt, 0.0, w, d.margin_inside_pt.min(w));
    h.margin_outside_pt = fix(h.margin_outside_pt, 0.0, w, d.margin_outside_pt.min(w));
    h.first_line_indent_pt = fix(h.first_line_indent_pt, 0.0, 144.0, 0.0);
    h.paragraph_spacing_pt = fix(h.paragraph_spacing_pt, 0.0, 72.0, d.paragraph_spacing_pt);
    h.line_spacing = fix(h.line_spacing, 0.5, 5.0, d.line_spacing);
    h.image_width = fix(h.image_width, 0.05, 1.0, d.image_width);
    h
}

/// Where blocks are written: what a plain paragraph's style is.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Place {
    Body,
    Note,
    Quote,
    Cell,
}

#[derive(Debug, Clone, Copy)]
struct Ctx {
    place: Place,
    /// The list level of the item being written (its further paragraphs
    /// are indented to it), if any.
    level: Option<u8>,
}

impl Ctx {
    const BODY: Ctx = Ctx {
        place: Place::Body,
        level: None,
    };

    fn at(place: Place) -> Ctx {
        Ctx { place, level: None }
    }
}

/// A run's formatting.
#[derive(Debug, Clone, Copy, Default)]
struct Fmt {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    sup: bool,
    sub: bool,
    code: bool,
    /// Inside a hyperlink (no link in it).
    link: bool,
}

impl Fmt {
    /// `w:rPr`, with the run lowered by `lower` half-points; empty if
    /// nothing is set.
    fn rpr(self, lower: i64) -> String {
        let mut out = String::new();
        if self.link {
            out.push_str("<w:rStyle w:val=\"Hyperlink\"/>");
        } else if self.code {
            out.push_str("<w:rStyle w:val=\"VerbatimChar\"/>");
        }
        if self.bold {
            out.push_str("<w:b/>");
        }
        if self.italic {
            out.push_str("<w:i/>");
        }
        if self.strike {
            out.push_str("<w:strike/>");
        }
        if lower != 0 {
            out.push_str(&format!("<w:position w:val=\"{}\"/>", -lower));
        }
        if self.underline && !self.link {
            out.push_str("<w:u w:val=\"single\"/>");
        }
        if self.sup {
            out.push_str("<w:vertAlign w:val=\"superscript\"/>");
        } else if self.sub {
            out.push_str("<w:vertAlign w:val=\"subscript\"/>");
        }
        match out.is_empty() {
            true => out,
            false => format!("<w:rPr>{out}</w:rPr>"),
        }
    }
}

/// Text as runs of `f` (tabs as `w:tab`); nothing for no text.
fn run(f: Fmt, s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = s
        .split('\t')
        .map(|p| match p.is_empty() {
            true => String::new(),
            false => format!("<w:t xml:space=\"preserve\">{}</w:t>", text(p)),
        })
        .collect();
    format!("<w:r>{}{}</w:r>", f.rpr(0), parts.join("<w:tab/>"))
}

/// A paragraph of style `style` with further properties `ppr` (in schema
/// order, after `w:pStyle`).
fn para(style: &str, ppr: &str, content: &str) -> String {
    format!("<w:p><w:pPr><w:pStyle w:val=\"{style}\"/>{ppr}</w:pPr>{content}</w:p>")
}

/// A part's relationships.
#[derive(Debug, Default)]
struct Rels {
    /// Type name (`image`), target, external.
    items: Vec<(&'static str, String, bool)>,
}

impl Rels {
    /// The id of a relationship to `target` (added if new).
    fn add(&mut self, kind: &'static str, target: &str, external: bool) -> String {
        let at = self
            .items
            .iter()
            .position(|(k, t, e)| *k == kind && t == target && *e == external)
            .unwrap_or_else(|| {
                self.items.push((kind, target.to_owned(), external));
                self.items.len() - 1
            });
        format!("rId{}", at + 1)
    }

    fn xml(&self) -> String {
        let mut out = format!("{HEAD}<Relationships xmlns=\"{NS_RELS}\">");
        for (n, (kind, target, external)) in self.items.iter().enumerate() {
            let mode = match external {
                true => " TargetMode=\"External\"",
                false => "",
            };
            out.push_str(&format!(
                "<Relationship Id=\"rId{}\" Type=\"{REL}{kind}\" Target=\"{}\"{mode}/>",
                n + 1,
                attr(target)
            ));
        }
        out.push_str("</Relationships>");
        out
    }
}

/// A list instance: a `w:num` of the bullet or number abstract numbering.
struct Num {
    ordered: bool,
    start: u64,
    level: u8,
}

struct DocxWriter<'d> {
    doc: &'d Doc,
    h: PageHints,
    /// The text area in points.
    text_width: f64,
    text_height: f64,
    /// The space around quotes and code and below tables, in twips.
    gap: i64,
    /// The document's and the footnotes' relationships, and which the part
    /// being written uses.
    rels: [Rels; 2],
    part: usize,
    /// Media files by resource name, in the order added.
    media: Vec<(&'d str, String)>,
    /// Footnotes' XML, id 1 first.
    notes: Vec<String>,
    /// Inside a note: its first paragraph still needs the note's mark.
    mark: bool,
    in_note: bool,
    /// Bookmark names by label, and the labels placed so far.
    bookmarks: HashMap<&'d str, String>,
    placed: HashSet<&'d str>,
    nums: Vec<Num>,
    /// Pictures so far (their `wp:docPr` ids).
    pictures: u32,
    bibliography: Option<&'d List>,
    warnings: Warnings,
    depth: usize,
}

impl Writer for Docx {
    fn write(&self, doc: &Doc, options: &WriteOptions) -> Result<Written> {
        let h = sane(options.page.clone().unwrap_or_default());
        let text_width = (h.width_pt - h.margin_inside_pt - h.margin_outside_pt).max(72.0);
        let text_height = (h.height_pt - h.margin_top_pt - h.margin_bottom_pt).max(72.0);
        let mut found = Vec::new();
        labels(&doc.blocks, &mut found);
        let gap = block_gap(&h);
        let mut w = DocxWriter {
            doc,
            h,
            text_width,
            text_height,
            gap,
            rels: [Rels::default(), Rels::default()],
            part: 0,
            media: Vec::new(),
            notes: Vec::new(),
            mark: false,
            in_note: false,
            bookmarks: bookmark_names(&found),
            placed: HashSet::new(),
            nums: Vec::new(),
            pictures: 0,
            bibliography: bibliography(doc),
            warnings: Warnings::default(),
            depth: 0,
        };
        for kind in ["styles", "numbering", "footnotes", "settings"] {
            w.rels[0].add(kind, &format!("{kind}.xml"), false);
        }
        let mut body = w.title_block();
        body.push_str(&w.blocks(&doc.blocks, Ctx::BODY)?);
        if body.ends_with("</w:tbl>") {
            body.push_str("<w:p/>");
        }
        body.push_str(&w.section());
        let document = format!(
            "{HEAD}<w:document {}><w:body>{body}</w:body></w:document>",
            namespaces()
        );
        let footnotes = w.footnotes();
        let lang = word_lang(doc.meta.lang.as_deref().unwrap_or(""));
        let modified = match options.modified.is_empty() {
            true => utc(SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs())),
            false => options.modified.clone(),
        };

        let mut types = format!(
            "{HEAD}<Types xmlns=\"{NS_TYPES}\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>"
        );
        let mut extensions: Vec<(&str, &str)> = Vec::new();
        for (name, path) in &w.media {
            let ext = path.rsplit('.').next().unwrap_or("");
            let mime = doc.resource(name).map_or("", |r| r.mime.as_str());
            if !extensions.iter().any(|(e, _)| *e == ext) {
                extensions.push((ext, mime));
            }
        }
        for (ext, mime) in extensions {
            types.push_str(&format!(
                "<Default Extension=\"{ext}\" ContentType=\"{}\"/>",
                attr(mime)
            ));
        }
        let office = "application/vnd.openxmlformats-officedocument";
        for (part, mime) in [
            ("/word/document.xml", MAIN_TYPE.to_owned()),
            (
                "/word/styles.xml",
                format!("{office}.wordprocessingml.styles+xml"),
            ),
            (
                "/word/numbering.xml",
                format!("{office}.wordprocessingml.numbering+xml"),
            ),
            (
                "/word/footnotes.xml",
                format!("{office}.wordprocessingml.footnotes+xml"),
            ),
            (
                "/word/settings.xml",
                format!("{office}.wordprocessingml.settings+xml"),
            ),
            (
                "/docProps/core.xml",
                "application/vnd.openxmlformats-package.core-properties+xml".to_owned(),
            ),
            (
                "/docProps/app.xml",
                format!("{office}.extended-properties+xml"),
            ),
        ] {
            types.push_str(&format!(
                "<Override PartName=\"{part}\" ContentType=\"{mime}\"/>"
            ));
        }
        types.push_str("</Types>");
        let package = format!(
            "{HEAD}<Relationships xmlns=\"{NS_RELS}\">\
<Relationship Id=\"rId1\" Type=\"{REL}officeDocument\" Target=\"word/document.xml\"/>\
<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/>\
<Relationship Id=\"rId3\" Type=\"{REL}extended-properties\" Target=\"docProps/app.xml\"/>\
</Relationships>"
        );
        let m = &doc.meta;
        let mut core = format!(
            "{HEAD}<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" \
xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" \
xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">"
        );
        if let Some(t) = some(&m.title) {
            core.push_str(&format!("<dc:title>{}</dc:title>", text(t)));
        }
        let authors: Vec<&str> = m.authors.iter().map(|a| a.trim()).collect();
        if authors.iter().any(|a| !a.is_empty()) {
            core.push_str(&format!(
                "<dc:creator>{}</dc:creator>",
                text(&authors.join("; "))
            ));
        }
        core.push_str(&format!("<dc:language>{}</dc:language>", text(&lang)));
        core.push_str(&format!(
            "<dcterms:modified xsi:type=\"dcterms:W3CDTF\">{}</dcterms:modified></cp:coreProperties>",
            text(&modified)
        ));
        let app = format!(
            "{HEAD}<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\">\
<Application>Lethora</Application></Properties>"
        );

        let mut zip = ZipWriter::new();
        zip.add("[Content_Types].xml", types.as_bytes(), true)?;
        zip.add("_rels/.rels", package.as_bytes(), true)?;
        zip.add("docProps/core.xml", core.as_bytes(), true)?;
        zip.add("docProps/app.xml", app.as_bytes(), true)?;
        zip.add("word/document.xml", document.as_bytes(), true)?;
        zip.add(
            "word/styles.xml",
            styles(&w.h, &lang, text_width).as_bytes(),
            true,
        )?;
        zip.add("word/numbering.xml", w.numbering().as_bytes(), true)?;
        zip.add("word/footnotes.xml", footnotes.as_bytes(), true)?;
        zip.add("word/settings.xml", settings(&w.h).as_bytes(), true)?;
        zip.add(
            "word/_rels/document.xml.rels",
            w.rels[0].xml().as_bytes(),
            true,
        )?;
        if !w.rels[1].items.is_empty() {
            zip.add(
                "word/_rels/footnotes.xml.rels",
                w.rels[1].xml().as_bytes(),
                true,
            )?;
        }
        for (name, path) in &w.media {
            if let Some(r) = doc.resource(name) {
                zip.add(&format!("word/{path}"), &r.bytes, true)?;
            }
        }
        Ok(Written {
            bytes: zip.finish()?,
            files: Vec::new(),
            warnings: w.warnings.into_vec(),
        })
    }
}

/// The namespaces of the document and footnotes parts.
fn namespaces() -> String {
    format!(
        "xmlns:w=\"{NS_W}\" xmlns:r=\"{NS_R}\" xmlns:wp=\"{NS_WP}\" xmlns:a=\"{NS_A}\" \
xmlns:pic=\"{NS_PIC}\" xmlns:asvg=\"{NS_ASVG}\""
    )
}

/// A metadata field trimmed, `None` if empty.
fn some(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// A language tag as Word has them (`en` → `en-US`); `en-US` if none.
fn word_lang(lang: &str) -> String {
    let lang = lang.trim();
    if lang.contains('-') {
        return lang.to_owned();
    }
    let region = match lang.to_ascii_lowercase().as_str() {
        "" | "en" => "en-US",
        "de" => "de-DE",
        "fr" => "fr-FR",
        "es" => "es-ES",
        "it" => "it-IT",
        "nl" => "nl-NL",
        "pt" => "pt-PT",
        "sv" => "sv-SE",
        "da" => "da-DK",
        "nb" | "no" => "nb-NO",
        "fi" => "fi-FI",
        "pl" => "pl-PL",
        "cs" => "cs-CZ",
        "ru" => "ru-RU",
        "ja" => "ja-JP",
        "zh" => "zh-CN",
        "ko" => "ko-KR",
        _ => return lang.to_owned(),
    };
    region.to_owned()
}

/// Bookmark names for `labels` (first occurrence each): ASCII letters,
/// digits and `_`, starting with a letter, at most 40 characters, unique
/// regardless of case (as Word compares them).
fn bookmark_names<'d>(labels: &[&'d str]) -> HashMap<&'d str, String> {
    let mut out = HashMap::new();
    let mut taken = HashSet::new();
    for label in labels {
        if out.contains_key(label) {
            continue;
        }
        let mut base: String = label
            .chars()
            .map(|c| match c.is_ascii_alphanumeric() {
                true => c,
                false => '_',
            })
            .collect();
        if !base.starts_with(|c: char| c.is_ascii_alphabetic()) {
            base.insert_str(0, "ref_");
        }
        base.truncate(40);
        let mut name = base.clone();
        let mut n = 2;
        while !taken.insert(name.to_ascii_lowercase()) {
            let suffix = format!("_{n}");
            name = format!("{}{suffix}", &base[..base.len().min(40 - suffix.len())]);
            n += 1;
        }
        out.insert(*label, name);
    }
    out
}

/// A caption's number ("Figure 2") as runs: the label, then the number in
/// a `SEQ` field Word can renumber (`SEQ Figure`); as text if it isn't a
/// label word and a number.
fn number_runs(number: &str) -> String {
    let number = number.trim();
    let field = number.rsplit_once(' ').filter(|(label, n)| {
        !n.is_empty()
            && n.bytes().all(|b| b.is_ascii_digit())
            && label.chars().count() <= 40
            && label.starts_with(char::is_alphabetic)
            && label.chars().all(|c| c.is_alphanumeric() || c == '_')
    });
    match field {
        Some((label, n)) => format!(
            "{}<w:fldSimple w:instr=\" SEQ {label} \\* ARABIC \"><w:r><w:rPr><w:noProof/></w:rPr><w:t>{n}</w:t></w:r></w:fldSimple>",
            run(Fmt::default(), &format!("{label} "))
        ),
        None => run(Fmt::default(), number),
    }
}

impl<'d> DocxWriter<'d> {
    fn enter(&mut self) -> Result<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Error::TooDeep);
        }
        Ok(())
    }

    /// A paragraph; the first one of a note gets the note's mark.
    fn p(&mut self, style: &str, ppr: &str, content: &str) -> String {
        if std::mem::take(&mut self.mark) {
            let mark = "<w:r><w:rPr><w:rStyle w:val=\"FootnoteReference\"/></w:rPr><w:footnoteRef/></w:r>\
<w:r><w:t xml:space=\"preserve\"> </w:t></w:r>";
            return para(style, ppr, &format!("{mark}{content}"));
        }
        para(style, ppr, content)
    }

    /// The bookmark's start and end for `label` where it is first placed;
    /// empty strings elsewhere.
    fn bookmark(&mut self, label: Option<&'d str>) -> (String, String) {
        let Some((label, name)) = label.and_then(|l| self.bookmarks.get(l).map(|n| (l, n))) else {
            return Default::default();
        };
        if self.in_note || !self.placed.insert(label) {
            return Default::default();
        }
        let id = self.placed.len();
        (
            format!("<w:bookmarkStart w:id=\"{id}\" w:name=\"{name}\"/>"),
            format!("<w:bookmarkEnd w:id=\"{id}\"/>"),
        )
    }

    /// Title, subtitle, authors and date, if there is a title.
    fn title_block(&mut self) -> String {
        let m = &self.doc.meta;
        let Some(title) = some(&m.title) else {
            return String::new();
        };
        let mut lines = vec![("Title", title)];
        lines.extend(some(&m.subtitle).map(|s| ("Subtitle", s)));
        lines.extend(
            m.authors
                .iter()
                .map(|a| ("Author", a.trim()))
                .filter(|(_, a)| !a.is_empty()),
        );
        lines.extend(some(&m.date).map(|d| ("Date", d)));
        // Space below the block, whatever ends it.
        let last = lines.len() - 1;
        lines
            .into_iter()
            .enumerate()
            .map(|(i, (style, s))| {
                let ppr = match i == last {
                    true => "<w:spacing w:after=\"480\"/>",
                    false => "",
                };
                para(style, ppr, &run(Fmt::default(), s))
            })
            .collect()
    }

    fn blocks(&mut self, blocks: &'d [Block], ctx: Ctx) -> Result<String> {
        self.enter()?;
        let mut out = String::new();
        let mut after_para = false;
        for b in blocks {
            // A table has no space below: the paragraph after it does.
            let gap = match out.ends_with("</w:tbl>") {
                true => format!("<w:spacing w:before=\"{}\"/>", self.gap),
                false => String::new(),
            };
            out.push_str(&self.block(b, ctx, after_para, &gap)?);
            after_para = matches!(b, Block::Para(_));
        }
        self.depth -= 1;
        Ok(out)
    }

    /// A plain paragraph's style and further properties in `ctx`.
    fn para_style(&self, ctx: Ctx, after_para: bool) -> (&'static str, String) {
        let indent = |l: u8| format!("<w:ind w:left=\"{}\"/>", 720 * (u32::from(l) + 1));
        match (ctx.place, ctx.level) {
            (Place::Note, Some(l)) => ("FootnoteText", indent(l)),
            (_, Some(l)) => ("ListParagraph", indent(l)),
            (Place::Body, None) if after_para => ("BodyText", String::new()),
            (Place::Body, None) => ("FirstParagraph", String::new()),
            (Place::Note, None) => ("FootnoteText", String::new()),
            (Place::Quote, None) => ("Quote", String::new()),
            (Place::Cell, None) => ("Compact", String::new()),
        }
    }

    /// A block; `gap` is the spacing a paragraph takes (in `w:pPr`).
    fn block(&mut self, block: &'d Block, ctx: Ctx, after_para: bool, gap: &str) -> Result<String> {
        Ok(match block {
            Block::Para(c) => {
                let (style, ppr) = self.para_style(ctx, after_para);
                let runs = self.inlines(c, Fmt::default())?;
                self.p(style, &format!("{gap}{ppr}"), &runs)
            }
            Block::Heading {
                level,
                content,
                label,
            } => {
                let runs = self.inlines(content, Fmt::default())?;
                let (start, end) = self.bookmark(label.as_deref());
                let style = format!("Heading{}", (*level).clamp(1, 6));
                self.p(&style, "", &format!("{start}{runs}{end}"))
            }
            Block::List(l) => self.list(l, ctx)?,
            Block::Quote(b) => self.blocks(b, Ctx::at(Place::Quote))?,
            Block::Code { text: code, .. } => {
                let code = code.strip_suffix('\n').unwrap_or(code);
                let lines: Vec<String> = code
                    .split('\n')
                    .map(|l| run(Fmt::default(), l.trim_end_matches('\r')))
                    .collect();
                self.p("SourceCode", "", &lines.join("<w:r><w:br/></w:r>"))
            }
            Block::Math {
                math,
                label,
                number,
            } => {
                let (start, end) = self.bookmark(label.as_deref());
                let formula = self.math(math);
                let number = number
                    .as_deref()
                    .map(|n| format!("<w:r><w:tab/></w:r>{}", run(Fmt::default(), n)))
                    .unwrap_or_default();
                self.p(
                    "Equation",
                    "",
                    &format!("{start}<w:r><w:tab/></w:r>{formula}{number}{end}"),
                )
            }
            Block::Figure(f) => self.figure(f)?,
            Block::Table(t) => self.table(t)?,
            Block::Rule => self.p(
                "Normal",
                "<w:pBdr><w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"1\" w:color=\"auto\"/></w:pBdr>",
                "",
            ),
        })
    }

    fn list(&mut self, l: &'d List, ctx: Ctx) -> Result<String> {
        let mut out = String::new();
        if self.bibliography.is_some_and(|b| std::ptr::eq(b, l)) {
            for item in &l.items {
                for b in item {
                    out.push_str(&match b {
                        Block::Para(c) => {
                            let runs = self.inlines(c, Fmt::default())?;
                            self.p("Bibliography", "", &runs)
                        }
                        b => self.block(b, ctx, false, "")?,
                    });
                }
            }
            return Ok(out);
        }
        // Word has nine levels; deeper lists stay on the ninth.
        let level = ctx.level.map_or(0, |l| l.saturating_add(1).min(8));
        self.nums.push(Num {
            ordered: l.ordered,
            start: l.start,
            level,
        });
        let num = self.nums.len();
        let style = match ctx.place {
            Place::Note => "FootnoteText",
            _ => "ListParagraph",
        };
        let numbered =
            format!("<w:numPr><w:ilvl w:val=\"{level}\"/><w:numId w:val=\"{num}\"/></w:numPr>");
        let inner = Ctx {
            level: Some(level),
            ..ctx
        };
        for item in &l.items {
            let rest = match item.split_first() {
                Some((Block::Para(c), rest)) => {
                    let runs = self.inlines(c, Fmt::default())?;
                    out.push_str(&self.p(style, &numbered, &runs));
                    rest
                }
                _ => {
                    out.push_str(&self.p(style, &numbered, ""));
                    item.as_slice()
                }
            };
            out.push_str(&self.blocks(rest, inner)?);
        }
        Ok(out)
    }

    fn figure(&mut self, f: &'d Figure) -> Result<String> {
        let mut caption = String::new();
        if let Some(n) = &f.number {
            caption.push_str(&number_runs(n));
            if !f.caption.is_empty() {
                caption.push_str(&run(Fmt::default(), ": "));
            }
        }
        caption.push_str(&self.inlines(&f.caption, Fmt::default())?);
        let (image, top) = match &f.body {
            FigureBody::Image(_) => (true, self.h.figure_caption_top),
            FigureBody::Table(_) => (false, self.h.table_caption_top),
        };
        // The label's bookmark: on the caption, else on the image; a table
        // without a caption gets an empty one to hold it.
        let captioned = !caption.is_empty() || (!image && f.label.is_some());
        let (start, end) = match captioned {
            true => self.bookmark(f.label.as_deref()),
            false => Default::default(),
        };
        let keep = "<w:keepNext/>";
        let caption = captioned.then(|| {
            let ppr = if top { keep } else { "" };
            self.p("Caption", ppr, &format!("{start}{caption}{end}"))
        });
        let body = match &f.body {
            FigureBody::Image(img) => {
                let (start, end) = match captioned {
                    true => Default::default(),
                    false => self.bookmark(f.label.as_deref()),
                };
                let alt = img
                    .alt
                    .clone()
                    .filter(|a| !a.trim().is_empty())
                    .unwrap_or_else(|| plain(&f.caption));
                let picture = self.image(img, &alt, false);
                let ppr = if captioned && !top { keep } else { "" };
                self.p("Figure", ppr, &format!("{start}{picture}{end}"))
            }
            FigureBody::Table(t) => self.table(t)?,
        };
        Ok(match (caption, top) {
            (Some(c), true) => format!("{c}{body}"),
            (Some(c), false) => format!("{body}{c}"),
            (None, _) => body,
        })
    }

    fn table(&mut self, t: &'d Table) -> Result<String> {
        self.enter()?;
        /// A slot of a row: a cell starting there (restarting a vertical
        /// merge if it spans down), the continuation of one from above
        /// (its width), or padding.
        enum Slot<'d> {
            Cell(&'d crate::model::Cell),
            Continue(u32),
            Pad,
        }
        let mut grid: Vec<Vec<Slot>> = Vec::new();
        // Per column: rows below still covered by a cell from above, and
        // the width of the cell starting there.
        let mut left: Vec<u32> = Vec::new();
        let mut spans: Vec<u32> = Vec::new();
        let mut columns = t.columns.min(1000);
        for row in &t.rows {
            let mut slots = Vec::new();
            let mut started = Vec::new();
            let mut col = 0;
            let mut cells = row.iter();
            loop {
                if left.get(col).is_some_and(|&n| n > 0) {
                    let span = spans[col].max(1);
                    slots.push(Slot::Continue(span));
                    col += span as usize;
                } else if let Some(cell) = cells.next() {
                    let span = cell.colspan.clamp(1, 1000);
                    if cell.rowspan > 1 {
                        started.push((col, span as usize, cell.rowspan - 1));
                    }
                    slots.push(Slot::Cell(cell));
                    col += span as usize;
                } else if col < left.len() {
                    slots.push(Slot::Pad);
                    col += 1;
                } else {
                    break;
                }
            }
            for n in &mut left {
                *n = n.saturating_sub(1);
            }
            for (c, span, rows) in started {
                if left.len() < c + span {
                    left.resize(c + span, 0);
                    spans.resize(c + span, 0);
                }
                left[c..c + span].fill(rows);
                spans[c..c + span].fill(0);
                spans[c] = span as u32;
            }
            columns = columns.max(col);
            grid.push(slots);
        }
        let columns = columns.max(1);
        if grid.is_empty() {
            self.depth -= 1;
            return Ok(String::new());
        }
        let width = twips(self.text_width) / columns as i64;
        let look = match t.header_rows {
            0 => {
                "<w:tblLook w:val=\"0400\" w:firstRow=\"0\" w:lastRow=\"0\" w:firstColumn=\"0\" w:lastColumn=\"0\" w:noHBand=\"0\" w:noVBand=\"1\"/>"
            }
            _ => {
                "<w:tblLook w:val=\"0420\" w:firstRow=\"1\" w:lastRow=\"0\" w:firstColumn=\"0\" w:lastColumn=\"0\" w:noHBand=\"0\" w:noVBand=\"1\"/>"
            }
        };
        let mut out = format!(
            "<w:tbl><w:tblPr><w:tblStyle w:val=\"TableGrid\"/><w:tblW w:w=\"{}\" w:type=\"dxa\"/>\
<w:tblLayout w:type=\"fixed\"/>{look}</w:tblPr><w:tblGrid>",
            width * columns as i64
        );
        for _ in 0..columns {
            out.push_str(&format!("<w:gridCol w:w=\"{width}\"/>"));
        }
        out.push_str("</w:tblGrid>");
        for (i, slots) in grid.iter().enumerate() {
            out.push_str("<w:tr>");
            if i < t.header_rows {
                out.push_str("<w:trPr><w:tblHeader/></w:trPr>");
            }
            let mut used = 0;
            let cell = |span: u32, merge: &str, content: &str| {
                let grid_span = match span {
                    1 => String::new(),
                    n => format!("<w:gridSpan w:val=\"{n}\"/>"),
                };
                format!(
                    "<w:tc><w:tcPr><w:tcW w:w=\"{}\" w:type=\"dxa\"/>{grid_span}{merge}</w:tcPr>{content}</w:tc>",
                    width * i64::from(span)
                )
            };
            for slot in slots {
                out.push_str(&match slot {
                    Slot::Cell(c) => {
                        let span = c.colspan.clamp(1, 1000);
                        used += span as usize;
                        let mut content = self.blocks(&c.content, Ctx::at(Place::Cell))?;
                        if !content.ends_with("</w:p>") {
                            content.push_str("<w:p/>");
                        }
                        let merge = match c.rowspan > 1 {
                            true => "<w:vMerge w:val=\"restart\"/>",
                            false => "",
                        };
                        cell(span, merge, &content)
                    }
                    Slot::Continue(span) => {
                        used += *span as usize;
                        cell(*span, "<w:vMerge/>", "<w:p/>")
                    }
                    Slot::Pad => {
                        used += 1;
                        cell(1, "", "<w:p/>")
                    }
                });
            }
            for _ in used..columns {
                out.push_str(&cell(1, "", "<w:p/>"));
            }
            out.push_str("</w:tr>");
        }
        out.push_str("</w:tbl>");
        self.depth -= 1;
        Ok(out)
    }

    fn inlines(&mut self, inlines: &'d [Inline], f: Fmt) -> Result<String> {
        self.enter()?;
        let mut out = String::new();
        for i in inlines {
            let with = |g: fn(&mut Fmt)| {
                let mut f = f;
                g(&mut f);
                f
            };
            out.push_str(&match i {
                Inline::Text(t) => run(f, &t.replace(['\n', '\r'], " ")),
                Inline::Emph(c) => self.inlines(c, with(|f| f.italic = true))?,
                Inline::Strong(c) => self.inlines(c, with(|f| f.bold = true))?,
                Inline::Underline(c) => self.inlines(c, with(|f| f.underline = true))?,
                Inline::Strike(c) => self.inlines(c, with(|f| f.strike = true))?,
                Inline::Super(c) => self.inlines(c, with(|f| f.sup = true))?,
                Inline::Sub(c) => self.inlines(c, with(|f| f.sub = true))?,
                Inline::Code(c) => run(with(|f| f.code = true), &c.replace(['\n', '\r'], " ")),
                Inline::LineBreak => format!("<w:r>{}<w:br/></w:r>", f.rpr(0)),
                Inline::Link { url, content } => {
                    // No link in a link, nor to nowhere.
                    let external = !url.is_empty() && !url.starts_with('#') && !f.link;
                    let g = Fmt {
                        link: f.link || external,
                        ..f
                    };
                    let shown = match content.is_empty() {
                        true => run(g, url),
                        false => self.inlines(content, g)?,
                    };
                    match external {
                        false => shown,
                        true => {
                            let id = self.rels[self.part].add("hyperlink", &uri(url), true);
                            format!("<w:hyperlink r:id=\"{id}\" w:history=\"1\">{shown}</w:hyperlink>")
                        }
                    }
                }
                Inline::Math(m) => self.math(m),
                Inline::Image(img) => {
                    let alt = img.alt.clone().unwrap_or_default();
                    self.image(img, &alt, true)
                }
                Inline::Footnote(blocks) => self.footnote(blocks, f)?,
                Inline::Cite(cite) => match &cite.text {
                    Some(c) => self.inlines(c, f)?,
                    None => {
                        let items: Vec<String> = cite
                            .items
                            .iter()
                            .map(|i| match &i.locator {
                                Some(l) => format!("@{}, {l}", i.key),
                                None => format!("@{}", i.key),
                            })
                            .collect();
                        run(f, &format!("[{}]", items.join("; ")))
                    }
                },
                Inline::Ref { label, text: shown } => {
                    let shown = match shown {
                        Some(c) => self.inlines(c, f)?,
                        None => run(f, label),
                    };
                    match self.bookmarks.get(label.as_str()) {
                        Some(name) if !f.link => {
                            format!("<w:hyperlink w:anchor=\"{name}\" w:history=\"1\">{shown}</w:hyperlink>")
                        }
                        Some(_) => shown,
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

    /// A footnote's reference, its note written; in a note, the note's
    /// paragraphs in parentheses (Word has no notes in notes).
    fn footnote(&mut self, blocks: &'d [Block], f: Fmt) -> Result<String> {
        if self.in_note {
            self.warnings.add(WarningKind::NestedNote);
            let mut out = run(f, " (");
            let mut first = true;
            for b in blocks {
                if !first {
                    out.push_str(&run(f, " "));
                }
                first = false;
                out.push_str(&match b {
                    Block::Para(c) | Block::Heading { content: c, .. } => self.inlines(c, f)?,
                    b => run(f, &block_text(b)),
                });
            }
            out.push_str(&run(f, ")"));
            return Ok(out);
        }
        self.notes.push(String::new());
        let id = self.notes.len();
        let part = std::mem::replace(&mut self.part, 1);
        self.in_note = true;
        self.mark = true;
        let written = self.blocks(blocks, Ctx::at(Place::Note));
        let mut xml = match written {
            Ok(x) => x,
            Err(e) => {
                (self.part, self.in_note, self.mark) = (part, false, false);
                return Err(e);
            }
        };
        if self.mark {
            xml.insert_str(0, &self.p("FootnoteText", "", ""));
        }
        if !xml.ends_with("</w:p>") {
            xml.push_str("<w:p/>");
        }
        (self.part, self.in_note, self.mark) = (part, false, false);
        self.notes[id - 1] = format!("<w:footnote w:id=\"{id}\">{xml}</w:footnote>");
        Ok(format!(
            "<w:r><w:rPr><w:rStyle w:val=\"FootnoteReference\"/></w:rPr><w:footnoteReference w:id=\"{id}\"/></w:r>"
        ))
    }

    /// The resource `name` if Word shows its type; a warning if not, or if
    /// there is none.
    fn shown(&mut self, name: &str) -> Option<&'d Resource> {
        let doc = self.doc;
        let Some(r) = doc.resource(name) else {
            self.warnings.add_example(WarningKind::MissingImage, name);
            return None;
        };
        match media_ext(&r.mime) {
            Some(_) => Some(r),
            None => {
                self.warnings.add_example(WarningKind::ImageLeftOut, name);
                None
            }
        }
    }

    /// The relationship id of `r`'s media file in the part being written.
    fn media(&mut self, r: &'d Resource) -> String {
        let path = match self.media.iter().find(|(n, _)| *n == r.name) {
            Some((_, p)) => p.clone(),
            None => {
                let ext = media_ext(&r.mime).unwrap_or("bin");
                let path = format!("media/image{}.{ext}", self.media.len() + 1);
                self.media.push((&r.name, path.clone()));
                path
            }
        };
        self.rels[self.part].add("image", &path, false)
    }

    /// `a:blip` for `r`: an SVG as `asvg:svgBlip`, its PNG twin (or else
    /// itself) the main picture.
    fn blip(&mut self, r: &'d Resource) -> String {
        if r.mime != "image/svg+xml" {
            return format!("<a:blip r:embed=\"{}\"/>", self.media(r));
        }
        let doc = self.doc;
        let main = r
            .fallback
            .as_deref()
            .and_then(|f| doc.resource(f))
            .filter(|f| matches!(f.mime.as_str(), "image/png" | "image/jpeg" | "image/gif"))
            .unwrap_or(r);
        let main = self.media(main);
        let svg = self.media(r);
        format!(
            "<a:blip r:embed=\"{main}\"><a:extLst><a:ext uri=\"{SVG_EXT}\"><asvg:svgBlip r:embed=\"{svg}\"/></a:ext></a:extLst></a:blip>"
        )
    }

    /// A picture of `r`, `width` × `height` points, as a run (lowered by
    /// `lower` points).
    fn picture(
        &mut self,
        r: &'d Resource,
        width: f64,
        height: f64,
        alt: &str,
        lower: f64,
    ) -> String {
        self.pictures += 1;
        let id = self.pictures;
        let blip = self.blip(r);
        let (cx, cy) = (emu(width).max(1), emu(height).max(1));
        let descr = attr(alt);
        let rpr = Fmt::default().rpr(half_points(lower.max(0.0)));
        format!(
            "<w:r>{rpr}<w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">\
<wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:effectExtent l=\"0\" t=\"0\" r=\"0\" b=\"0\"/>\
<wp:docPr id=\"{id}\" name=\"Picture {id}\" descr=\"{descr}\"/>\
<wp:cNvGraphicFramePr><a:graphicFrameLocks noChangeAspect=\"1\"/></wp:cNvGraphicFramePr>\
<a:graphic><a:graphicData uri=\"{NS_PIC}\"><pic:pic>\
<pic:nvPicPr><pic:cNvPr id=\"{id}\" name=\"Picture {id}\" descr=\"{descr}\"/><pic:cNvPicPr><a:picLocks noChangeAspect=\"1\"/></pic:cNvPicPr></pic:nvPicPr>\
<pic:blipFill>{blip}<a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
<pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr>\
</pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"
        )
    }

    /// An image as a run: its share of the text width (the layout's
    /// default if none), no taller than the text area; one in the `line`
    /// (an icon) as tall as the text unless its width is known. Left out
    /// (its alt text in brackets) if Word can't show it.
    fn image(&mut self, img: &'d Image, alt: &str, line: bool) -> String {
        let size = self.shown(&img.resource).and_then(|r| {
            let size = dimensions(r);
            if size.is_none() {
                self.warnings
                    .add_example(WarningKind::ImageLeftOut, &img.resource);
            }
            size.map(|s| (r, s))
        });
        let Some((r, (w, h))) = size else {
            let alt = alt.trim();
            return match alt.is_empty() {
                true => String::new(),
                false => run(Fmt::default(), &format!("[{alt}]")),
            };
        };
        let share = img.width.filter(|w| w.is_finite() && *w > 0.0);
        let mut width = match (share, line) {
            (None, true) => (self.h.size_pt * w / h).min(self.text_width),
            (share, _) => self.text_width * share.unwrap_or(self.h.image_width).min(1.0),
        };
        let mut height = width * h / w;
        if height > self.text_height {
            width *= self.text_height / height;
            height = self.text_height;
        }
        self.picture(r, width, height, alt, 0.0)
    }

    /// A formula as its rendered image at its size, lowered onto the
    /// baseline (a display formula's number sits on it too), else its Typst
    /// source as code.
    fn math(&mut self, math: &'d Math) -> String {
        let image = math.image.as_deref().and_then(|name| self.shown(name));
        let sized = image.and_then(|r| {
            let size = match math.size {
                Some(s) if s.width > 0.0 && s.height > 0.0 => Some((s.width, s.height, s.baseline)),
                _ => dimensions(r).map(|(w, h)| (w, h, h)),
            };
            size.map(|s| (r, s))
        });
        match sized {
            Some((r, (w, h, baseline))) => {
                self.picture(r, w, h, &math.typst, (h - baseline).clamp(0.0, h))
            }
            None => {
                self.warnings.add(WarningKind::TypstMath);
                let f = Fmt {
                    code: true,
                    ..Fmt::default()
                };
                run(f, &math.typst.replace(['\n', '\r'], " "))
            }
        }
    }

    /// `w:sectPr`: the page size and margins.
    fn section(&self) -> String {
        let h = &self.h;
        let header = twips(h.margin_top_pt / 2.0).min(708);
        let footer = twips(h.margin_bottom_pt / 2.0).min(708);
        format!(
            "<w:sectPr><w:pgSz w:w=\"{}\" w:h=\"{}\"/><w:pgMar w:top=\"{}\" w:right=\"{}\" w:bottom=\"{}\" w:left=\"{}\" w:header=\"{header}\" w:footer=\"{footer}\" w:gutter=\"0\"/><w:cols w:space=\"708\"/></w:sectPr>",
            twips(h.width_pt),
            twips(h.height_pt),
            twips(h.margin_top_pt),
            twips(h.margin_outside_pt),
            twips(h.margin_bottom_pt),
            twips(h.margin_inside_pt),
        )
    }

    fn footnotes(&self) -> String {
        let sep = |kind: &str, id: i32, mark: &str| {
            format!(
                "<w:footnote w:type=\"{kind}\" w:id=\"{id}\"><w:p><w:pPr><w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr><w:r><w:{mark}/></w:r></w:p></w:footnote>"
            )
        };
        format!(
            "{HEAD}<w:footnotes {}>{}{}{}</w:footnotes>",
            namespaces(),
            sep("separator", -1, "separator"),
            sep("continuationSeparator", 0, "continuationSeparator"),
            self.notes.concat()
        )
    }

    /// Bullets (abstract numbering 0) and numbers (1) on nine levels, and
    /// a `w:num` per list restarting its level at its start.
    fn numbering(&self) -> String {
        let mut out = format!("{HEAD}<w:numbering xmlns:w=\"{NS_W}\">");
        for (id, ordered) in [(0, false), (1, true)] {
            out.push_str(&format!(
                "<w:abstractNum w:abstractNumId=\"{id}\"><w:multiLevelType w:val=\"hybridMultilevel\"/>"
            ));
            for l in 0..9u32 {
                let (fmt, shown) = match ordered {
                    true => ("decimal", format!("%{}.", l + 1)),
                    false => ("bullet", ["•", "◦", "▪"][l as usize % 3].to_owned()),
                };
                out.push_str(&format!(
                    "<w:lvl w:ilvl=\"{l}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"{fmt}\"/><w:lvlText w:val=\"{shown}\"/>\
<w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"{}\" w:hanging=\"360\"/></w:pPr></w:lvl>",
                    720 * (l + 1)
                ));
            }
            out.push_str("</w:abstractNum>");
        }
        for (n, num) in self.nums.iter().enumerate() {
            let restart = match num.ordered {
                true => format!(
                    "<w:lvlOverride w:ilvl=\"{}\"><w:startOverride w:val=\"{}\"/></w:lvlOverride>",
                    num.level,
                    num.start.min(32_767)
                ),
                false => String::new(),
            };
            out.push_str(&format!(
                "<w:num w:numId=\"{}\"><w:abstractNumId w:val=\"{}\"/>{restart}</w:num>",
                n + 1,
                u8::from(num.ordered)
            ));
        }
        out.push_str("</w:numbering>");
        out
    }
}

/// A block's text, for a note put in parentheses.
fn block_text(b: &Block) -> String {
    match b {
        Block::Para(c) | Block::Heading { content: c, .. } => plain(c),
        Block::List(l) => l
            .items
            .iter()
            .flatten()
            .map(block_text)
            .collect::<Vec<_>>()
            .join(" "),
        Block::Quote(b) => b.iter().map(block_text).collect::<Vec<_>>().join(" "),
        Block::Code { text, .. } => text.split_whitespace().collect::<Vec<_>>().join(" "),
        Block::Math { math, .. } => math.typst.clone(),
        Block::Figure(f) => plain(&f.caption),
        Block::Table(_) | Block::Rule => String::new(),
    }
}

/// A URL as a relationship target: spaces, characters URIs don't have and
/// non-ASCII percent-encoded (Word refuses some raw ones).
fn uri(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for c in url.chars() {
        match c {
            c if c.is_ascii_graphic() && !"\"<>\\^`{|}".contains(c) => out.push(c),
            c => {
                let mut buf = [0; 4];
                for b in c.encode_utf8(&mut buf).bytes() {
                    out.push_str(&format!("%{b:02X}"));
                }
            }
        }
    }
    out
}

/// The media file extension for an image type Word shows.
fn media_ext(mime: &str) -> Option<&'static str> {
    Some(match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        _ => return None,
    })
}

/// An image's width and height (pixels, or points for SVG): from a PNG's,
/// GIF's or JPEG's header, or an SVG's `width`/`height`/`viewBox`; only
/// their ratio matters for pictures.
fn dimensions(r: &Resource) -> Option<(f64, f64)> {
    let b = r.bytes.as_slice();
    let be16 = |i: usize| {
        b.get(i..i + 2)
            .map(|s| f64::from(u16::from_be_bytes([s[0], s[1]])))
    };
    let le16 = |i: usize| {
        b.get(i..i + 2)
            .map(|s| f64::from(u16::from_le_bytes([s[0], s[1]])))
    };
    let be32 = |i: usize| {
        b.get(i..i + 4)
            .map(|s| f64::from(u32::from_be_bytes([s[0], s[1], s[2], s[3]])))
    };
    let size = if b.starts_with(b"\x89PNG\r\n\x1a\n") && b.get(12..16) == Some(b"IHDR") {
        be32(16).zip(be32(20))
    } else if b.starts_with(b"GIF8") {
        le16(6).zip(le16(8))
    } else if b.starts_with(&[0xff, 0xd8]) {
        // The first start-of-frame marker (not DHT, JPG or DAC).
        let mut i = 2;
        let mut found = None;
        while i + 4 <= b.len() && b[i] == 0xff {
            let marker = b[i + 1];
            match marker {
                0xff => i += 1,
                0x01 | 0xd0..=0xd8 => i += 2,
                0xc0..=0xcf if !matches!(marker, 0xc4 | 0xc8 | 0xcc) => {
                    found = be16(i + 7).zip(be16(i + 5));
                    break;
                }
                _ => i += 2 + be16(i + 2)? as usize,
            }
        }
        found
    } else if r.mime == "image/svg+xml" {
        svg_size(b)
    } else {
        None
    };
    size.filter(|(w, h)| w.is_finite() && h.is_finite() && *w > 0.0 && *h > 0.0)
}

/// An SVG's size in points: `width` and `height`, one of them and the
/// `viewBox`'s ratio, or the `viewBox` (in pixels).
fn svg_size(bytes: &[u8]) -> Option<(f64, f64)> {
    let text = std::str::from_utf8(bytes).ok()?;
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        nodes_limit: 1_000_000,
        ..roxmltree::ParsingOptions::default()
    };
    let doc = roxmltree::Document::parse_with_options(text, options).ok()?;
    let root = doc.root_element();
    if root.tag_name().name() != "svg" {
        return None;
    }
    let length = |name: &str| root.attribute(name).and_then(svg_length);
    let view = root.attribute("viewBox").and_then(|v| {
        let n: Vec<f64> = v
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|s| !s.is_empty())
            .map(|s| s.parse().ok())
            .collect::<Option<_>>()?;
        (n.len() == 4 && n[2] > 0.0 && n[3] > 0.0).then(|| (n[2] * 0.75, n[3] * 0.75))
    });
    match (length("width"), length("height"), view) {
        (Some(w), Some(h), _) => Some((w, h)),
        (Some(w), None, Some((vw, vh))) => Some((w, w * vh / vw)),
        (None, Some(h), Some((vw, vh))) => Some((h * vw / vh, h)),
        (_, _, view) => view,
    }
}

/// An SVG length in points (`12pt`, `16px`, `16`, `5mm`); `None` for
/// percentages and the unknown.
fn svg_length(s: &str) -> Option<f64> {
    let s = s.trim();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E')))
        .unwrap_or(s.len());
    let (number, unit) = s.split_at(split);
    let factor = match unit.trim() {
        "" | "px" => 0.75,
        "pt" => 1.0,
        "pc" => 12.0,
        "in" => 72.0,
        "cm" => 72.0 / 2.54,
        "mm" => 72.0 / 25.4,
        _ => return None,
    };
    number
        .parse::<f64>()
        .ok()
        .map(|n| n * factor)
        .filter(|n| n.is_finite() && *n > 0.0)
}

/// The space around quotes and code and below tables, in twips: the
/// paragraph spacing, at least half a line.
fn block_gap(h: &PageHints) -> i64 {
    twips(h.paragraph_spacing_pt.max(h.size_pt / 2.0))
}

/// `word/settings.xml`: mirrored margins when two-sided, hyphenation, the
/// footnote separators, justified lines not stretched before a line break,
/// Word 2013+ compatibility.
fn settings(h: &PageHints) -> String {
    format!(
        "{HEAD}<w:settings xmlns:w=\"{NS_W}\"><w:zoom w:percent=\"100\"/>{}<w:defaultTabStop w:val=\"720\"/>{}\
<w:characterSpacingControl w:val=\"doNotCompress\"/>\
<w:footnotePr><w:footnote w:id=\"-1\"/><w:footnote w:id=\"0\"/></w:footnotePr>\
<w:compat><w:doNotExpandShiftReturn/><w:compatSetting w:name=\"compatibilityMode\" w:uri=\"http://schemas.microsoft.com/office/word\" w:val=\"15\"/></w:compat>\
</w:settings>",
        if h.two_sided {
            "<w:mirrorMargins/>"
        } else {
            ""
        },
        if h.hyphenate {
            "<w:autoHyphenation/>"
        } else {
            ""
        },
    )
}

/// `word/styles.xml`: the document defaults and the styles from the hints.
fn styles(h: &PageHints, lang: &str, text_width: f64) -> String {
    let font = attr(&h.font);
    let sz = |pt: f64| {
        let n = half_points(pt);
        format!("<w:sz w:val=\"{n}\"/><w:szCs w:val=\"{n}\"/>")
    };
    let heading = |level: usize| {
        let sizes = &h.heading_sizes_pt;
        sizes
            .get(level - 1)
            .or(sizes.last())
            .copied()
            .unwrap_or(h.size_pt)
    };
    let spacing = twips(h.paragraph_spacing_pt);
    let gap = block_gap(h);
    let block = format!("<w:spacing w:before=\"{gap}\" w:after=\"{gap}\"/>");
    let line = (240.0 * h.line_spacing).round() as i64;
    let mut out = format!(
        "{HEAD}<w:styles xmlns:w=\"{NS_W}\"><w:docDefaults><w:rPrDefault><w:rPr>\
<w:rFonts w:ascii=\"{font}\" w:hAnsi=\"{font}\" w:eastAsia=\"{font}\" w:cs=\"{font}\"/>{}<w:lang w:val=\"{}\"/>\
</w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after=\"{spacing}\" w:line=\"{line}\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault></w:docDefaults>",
        sz(h.size_pt),
        attr(lang)
    );
    // Id, name, type, the attributes after the name (basedOn, next, …),
    // paragraph and run properties.
    let mut style = |kind: &str, id: &str, name: &str, after: &str, ppr: &str, rpr: &str| {
        let ppr = match ppr.is_empty() {
            true => String::new(),
            false => format!("<w:pPr>{ppr}</w:pPr>"),
        };
        let rpr = match rpr.is_empty() {
            true => String::new(),
            false => format!("<w:rPr>{rpr}</w:rPr>"),
        };
        out.push_str(&format!(
            "<w:style w:type=\"{kind}\" w:styleId=\"{id}\"><w:name w:val=\"{name}\"/>{after}{ppr}{rpr}</w:style>"
        ));
    };
    let based = |on: &str| format!("<w:basedOn w:val=\"{on}\"/>");
    let next = |id: &str| format!("<w:next w:val=\"{id}\"/>");
    let q = "<w:qFormat/>";
    let center = "<w:jc w:val=\"center\"/>";
    let justify = if h.justify {
        "<w:jc w:val=\"both\"/>"
    } else {
        ""
    };
    style("paragraph", "Normal", "Normal", q, "<w:widowControl/>", "");
    style(
        "paragraph",
        "BodyText",
        "Body Text",
        &format!("{}{q}", based("Normal")),
        &format!(
            "<w:ind w:firstLine=\"{}\"/>{justify}",
            twips(h.first_line_indent_pt)
        ),
        "",
    );
    style(
        "paragraph",
        "FirstParagraph",
        "First Paragraph",
        &format!("{}{}{q}", based("BodyText"), next("BodyText")),
        "<w:ind w:firstLine=\"0\"/>",
        "",
    );
    let title = heading(1) * 1.4;
    style(
        "paragraph",
        "Title",
        "Title",
        &format!("{}{}{q}", based("Normal"), next("Subtitle")),
        &format!("<w:keepNext/><w:keepLines/><w:spacing w:before=\"0\" w:after=\"240\"/>{center}"),
        &format!("<w:b/><w:bCs/>{}", sz(title)),
    );
    style(
        "paragraph",
        "Subtitle",
        "Subtitle",
        &format!("{}{}{q}", based("Normal"), next("Author")),
        &format!("<w:keepNext/><w:keepLines/><w:spacing w:after=\"240\"/>{center}"),
        &sz(heading(2)),
    );
    style(
        "paragraph",
        "Author",
        "Author",
        &format!("{}{}{q}", based("Normal"), next("Author")),
        &format!("<w:keepNext/><w:keepLines/>{center}"),
        "",
    );
    style(
        "paragraph",
        "Date",
        "Date",
        &format!("{}{}{q}", based("Normal"), next("FirstParagraph")),
        &format!("<w:keepNext/><w:keepLines/><w:spacing w:after=\"480\"/>{center}"),
        "",
    );
    for level in 1..=6 {
        let size = heading(level);
        let page = match level == 1 && h.chapter_page_break {
            true => "<w:pageBreakBefore/>",
            false => "",
        };
        style(
            "paragraph",
            &format!("Heading{level}"),
            &format!("heading {level}"),
            &format!(
                "{}{}<w:uiPriority w:val=\"9\"/>{q}",
                based("Normal"),
                next("FirstParagraph")
            ),
            &format!(
                "<w:keepNext/><w:keepLines/>{page}<w:spacing w:before=\"{}\" w:after=\"{}\"/><w:outlineLvl w:val=\"{}\"/>",
                twips(size * 1.2),
                twips(size * 0.5),
                level - 1
            ),
            &format!("<w:b/><w:bCs/>{}", sz(size)),
        );
    }
    style(
        "paragraph",
        "Caption",
        "caption",
        &format!(
            "{}{}<w:uiPriority w:val=\"35\"/>{q}",
            based("Normal"),
            next("FirstParagraph")
        ),
        &format!("<w:spacing w:before=\"120\" w:after=\"240\"/>{center}"),
        "",
    );
    style(
        "paragraph",
        "Figure",
        "Figure",
        &format!("{}{}{q}", based("Normal"), next("Caption")),
        &format!("<w:spacing w:before=\"120\" w:after=\"120\"/>{center}"),
        "",
    );
    style(
        "paragraph",
        "Equation",
        "Equation",
        &format!("{}{q}", based("Normal")),
        &format!(
            "<w:tabs><w:tab w:val=\"center\" w:pos=\"{}\"/><w:tab w:val=\"right\" w:pos=\"{}\"/></w:tabs><w:spacing w:before=\"120\" w:after=\"120\"/>",
            twips(text_width / 2.0),
            twips(text_width)
        ),
        "",
    );
    style(
        "paragraph",
        "FootnoteText",
        "footnote text",
        &format!("{}<w:uiPriority w:val=\"99\"/>", based("Normal")),
        "<w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/>",
        &sz(h.size_pt * 0.82),
    );
    style(
        "paragraph",
        "Quote",
        "Quote",
        &format!(
            "{}{}<w:uiPriority w:val=\"29\"/>{q}",
            based("Normal"),
            next("BodyText")
        ),
        &format!("{block}<w:ind w:left=\"720\" w:right=\"720\"/>"),
        "",
    );
    style(
        "paragraph",
        "SourceCode",
        "Source Code",
        &based("Normal"),
        &format!(
            "<w:spacing w:before=\"{gap}\" w:after=\"{gap}\" w:line=\"240\" w:lineRule=\"auto\"/>"
        ),
        &format!(
            "<w:rFonts w:ascii=\"{MONO}\" w:hAnsi=\"{MONO}\" w:cs=\"{MONO}\"/>{}",
            sz(h.size_pt * 0.9)
        ),
    );
    style(
        "paragraph",
        "Bibliography",
        "Bibliography",
        &format!(
            "{}{}<w:uiPriority w:val=\"37\"/>",
            based("Normal"),
            next("Bibliography")
        ),
        "<w:ind w:left=\"567\" w:hanging=\"567\"/>",
        "",
    );
    style(
        "paragraph",
        "ListParagraph",
        "List Paragraph",
        &format!("{}<w:uiPriority w:val=\"34\"/>{q}", based("Normal")),
        "<w:contextualSpacing/>",
        "",
    );
    style(
        "paragraph",
        "Compact",
        "Compact",
        &format!("{}{q}", based("Normal")),
        "<w:spacing w:before=\"36\" w:after=\"36\"/>",
        "",
    );
    out.push_str(
        "<w:style w:type=\"character\" w:default=\"1\" w:styleId=\"DefaultParagraphFont\">\
<w:name w:val=\"Default Paragraph Font\"/><w:uiPriority w:val=\"1\"/><w:semiHidden/><w:unhideWhenUsed/></w:style>",
    );
    let mut style = |id: &str, name: &str, after: &str, rpr: &str| {
        out.push_str(&format!(
            "<w:style w:type=\"character\" w:styleId=\"{id}\"><w:name w:val=\"{name}\"/>\
<w:basedOn w:val=\"DefaultParagraphFont\"/>{after}<w:rPr>{rpr}</w:rPr></w:style>"
        ));
    };
    style(
        "Hyperlink",
        "Hyperlink",
        "<w:uiPriority w:val=\"99\"/><w:unhideWhenUsed/>",
        "<w:color w:val=\"0563C1\"/><w:u w:val=\"single\"/>",
    );
    style(
        "FootnoteReference",
        "footnote reference",
        "<w:uiPriority w:val=\"99\"/><w:unhideWhenUsed/>",
        "<w:vertAlign w:val=\"superscript\"/>",
    );
    style(
        "VerbatimChar",
        "Verbatim Char",
        "",
        &format!(
            "<w:rFonts w:ascii=\"{MONO}\" w:hAnsi=\"{MONO}\" w:cs=\"{MONO}\"/>{}",
            sz(h.size_pt * 0.9)
        ),
    );
    let border = |side: &str| {
        format!("<w:{side} w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>")
    };
    let borders: String = ["top", "left", "bottom", "right", "insideH", "insideV"]
        .iter()
        .map(|s| border(s))
        .collect();
    out.push_str(&format!(
        "<w:style w:type=\"table\" w:default=\"1\" w:styleId=\"TableNormal\"><w:name w:val=\"Normal Table\"/>\
<w:uiPriority w:val=\"99\"/><w:semiHidden/><w:unhideWhenUsed/><w:tblPr><w:tblInd w:w=\"0\" w:type=\"dxa\"/>\
<w:tblCellMar><w:top w:w=\"0\" w:type=\"dxa\"/><w:left w:w=\"108\" w:type=\"dxa\"/><w:bottom w:w=\"0\" w:type=\"dxa\"/><w:right w:w=\"108\" w:type=\"dxa\"/></w:tblCellMar></w:tblPr></w:style>\
<w:style w:type=\"table\" w:styleId=\"TableGrid\"><w:name w:val=\"Table Grid\"/><w:basedOn w:val=\"TableNormal\"/><w:uiPriority w:val=\"39\"/>\
<w:pPr><w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr><w:tblPr><w:tblBorders>{borders}</w:tblBorders></w:tblPr>\
<w:tblStylePr w:type=\"firstRow\"><w:rPr><w:b/><w:bCs/></w:rPr></w:tblStylePr></w:style>\
<w:style w:type=\"numbering\" w:default=\"1\" w:styleId=\"NoList\"><w:name w:val=\"No List\"/><w:uiPriority w:val=\"99\"/><w:semiHidden/><w:unhideWhenUsed/></w:style>"
    ));
    out.push_str("</w:styles>");
    out
}

/// What is wrong with a DOCX's structure, empty if nothing, for the tests:
/// every XML part parses; every part has a content type and every
/// override a part; every internal relationship's target exists; every
/// `r:id`/`r:embed` of the document and footnotes is one of their
/// relationships (an `r:embed` an image); every footnote reference has a
/// note; every `w:numId` used is a `w:num` with an existing abstract
/// numbering; every `w:anchor` is a bookmark (names unique, each started
/// bookmark ended); the styles used exist; no `wp:docPr` id twice.
#[doc(hidden)]
pub fn check(bytes: &[u8]) -> Vec<String> {
    let mut problems = Vec::new();
    if let Err(e) = check_into(bytes, &mut problems) {
        problems.push(e.to_string());
    }
    problems
}

fn check_into(bytes: &[u8], problems: &mut Vec<String>) -> Result<()> {
    let zip = Archive::new(bytes)?;
    let names: Vec<&str> = zip.names().collect();
    let mut texts: HashMap<&str, String> = HashMap::new();
    for name in &names {
        if name.ends_with(".xml") || name.ends_with(".rels") {
            let bytes = zip.read(name)?.unwrap_or_default();
            match String::from_utf8(bytes) {
                Ok(t) => {
                    texts.insert(*name, t);
                }
                Err(_) => problems.push(format!("{name}: not UTF-8")),
            }
        }
    }
    let mut docs: HashMap<&str, roxmltree::Document> = HashMap::new();
    for (name, t) in &texts {
        match roxmltree::Document::parse(t) {
            Ok(d) => {
                docs.insert(*name, d);
            }
            Err(e) => problems.push(format!("{name}: {e}")),
        }
    }
    let Some(types) = docs.get("[Content_Types].xml") else {
        return Err(Error::Invalid("[Content_Types].xml is missing".into()));
    };
    let mut defaults = HashSet::new();
    let mut overrides = HashSet::new();
    for n in types.root_element().children().filter(|n| n.is_element()) {
        match n.tag_name().name() {
            "Default" => {
                defaults.insert(n.attribute("Extension").unwrap_or("").to_ascii_lowercase());
            }
            "Override" => {
                let part = n
                    .attribute("PartName")
                    .unwrap_or("")
                    .trim_start_matches('/');
                if !names.contains(&part) {
                    problems.push(format!("{part}: a content type, no part"));
                }
                overrides.insert(part.to_owned());
            }
            _ => {}
        }
    }
    for name in names.iter().filter(|n| **n != "[Content_Types].xml") {
        let ext = name
            .rsplit_once('.')
            .map_or("", |(_, e)| e)
            .to_ascii_lowercase();
        if !overrides.contains(*name) && !defaults.contains(&ext) {
            problems.push(format!("{name}: no content type"));
        }
    }
    // Relationships by the part they belong to: id → (type, target).
    let mut rels: HashMap<String, HashMap<String, (String, String)>> = HashMap::new();
    for name in names.iter().filter(|n| n.ends_with(".rels")) {
        let Some(doc) = docs.get(name) else { continue };
        let (dir, file) = name.rsplit_once('/').unwrap_or(("", name));
        let dir = dir
            .strip_suffix("_rels")
            .unwrap_or(dir)
            .trim_end_matches('/');
        let part = match dir {
            "" => file.trim_end_matches(".rels").to_owned(),
            d => format!("{d}/{}", file.trim_end_matches(".rels")),
        };
        let mut own = HashMap::new();
        for r in doc.root_element().children().filter(|n| n.is_element()) {
            let (id, kind, target) = (
                r.attribute("Id").unwrap_or(""),
                r.attribute("Type").unwrap_or(""),
                r.attribute("Target").unwrap_or(""),
            );
            if r.attribute("TargetMode") != Some("External") {
                let path = resolve(dir, target);
                if !names.contains(&path.as_str()) {
                    problems.push(format!("{name}: {id} targets {path}, which is missing"));
                }
            }
            if own
                .insert(id.to_owned(), (kind.to_owned(), target.to_owned()))
                .is_some()
            {
                problems.push(format!("{name}: {id} twice"));
            }
        }
        rels.insert(part, own);
    }
    let main_type = format!("{REL}officeDocument");
    if !rels.get("").is_some_and(|r| {
        r.values()
            .any(|(k, t)| *k == main_type && t == "word/document.xml")
    }) {
        problems.push("_rels/.rels: no officeDocument word/document.xml".into());
    }
    let w = |n: &roxmltree::Node, name: &str| n.is_element() && n.has_tag_name((NS_W, name));
    let val = |n: roxmltree::Node| n.attribute((NS_W, "val")).unwrap_or("").to_owned();
    let parts = ["word/document.xml", "word/footnotes.xml"];
    let get = |name: &str| docs.get(name);
    // Footnotes, numbering, styles and bookmarks.
    let notes: HashSet<String> = get("word/footnotes.xml")
        .map(|d| {
            d.descendants()
                .filter(|n| w(n, "footnote"))
                .filter_map(|n| n.attribute((NS_W, "id")).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let (mut nums, mut abstracts) = (HashSet::new(), HashSet::new());
    if let Some(d) = get("word/numbering.xml") {
        for n in d.descendants() {
            if w(&n, "abstractNum") {
                abstracts.insert(
                    n.attribute((NS_W, "abstractNumId"))
                        .unwrap_or("")
                        .to_owned(),
                );
            }
        }
        for n in d.descendants().filter(|n| w(n, "num")) {
            nums.insert(n.attribute((NS_W, "numId")).unwrap_or("").to_owned());
            let abs = n.children().find(|c| w(c, "abstractNumId")).map(val);
            if !abs.is_some_and(|a| abstracts.contains(&a)) {
                problems.push("numbering.xml: a w:num without its abstract numbering".into());
            }
        }
    }
    let styles: HashSet<String> = get("word/styles.xml")
        .map(|d| {
            d.descendants()
                .filter(|n| w(n, "style"))
                .filter_map(|n| n.attribute((NS_W, "styleId")).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let mut bookmarks = HashSet::new();
    let mut ids = Vec::new();
    let mut ended = HashSet::new();
    let mut anchors = Vec::new();
    let mut pictures = HashSet::new();
    for part in parts {
        let Some(doc) = get(part) else {
            problems.push(format!("{part} is missing"));
            continue;
        };
        let own = rels.get(part).cloned().unwrap_or_default();
        for n in doc.descendants().filter(|n| n.is_element()) {
            for a in n.attributes().filter(|a| a.namespace() == Some(NS_R)) {
                match own.get(a.value()) {
                    None => problems.push(format!(
                        "{part}: r:{} {} is no relationship",
                        a.name(),
                        a.value()
                    )),
                    Some((kind, _)) if a.name() == "embed" && *kind != format!("{REL}image") => {
                        problems.push(format!("{part}: r:embed {} is no image", a.value()))
                    }
                    _ => {}
                }
            }
            let name = n.tag_name().name();
            if n.tag_name().namespace() == Some(NS_WP)
                && name == "docPr"
                && !pictures.insert(n.attribute("id").unwrap_or("").to_owned())
            {
                problems.push(format!(
                    "{part}: wp:docPr id {} twice",
                    n.attribute("id").unwrap_or("")
                ));
            }
            if n.tag_name().namespace() != Some(NS_W) {
                continue;
            }
            match name {
                "footnoteReference" => {
                    let id = n.attribute((NS_W, "id")).unwrap_or("");
                    if !notes.contains(id) {
                        problems.push(format!("{part}: footnote {id} has no note"));
                    }
                }
                "numId" if val(n) != "0" && !nums.contains(&val(n)) => {
                    problems.push(format!("{part}: w:numId {} is no w:num", val(n)));
                }
                "pStyle" | "rStyle" | "tblStyle" if !styles.contains(&val(n)) => {
                    problems.push(format!("{part}: style {} is missing", val(n)));
                }
                "bookmarkStart" => {
                    let name = n.attribute((NS_W, "name")).unwrap_or("").to_owned();
                    if !bookmarks.insert(name.clone()) {
                        problems.push(format!("{part}: bookmark {name} twice"));
                    }
                    ids.push(n.attribute((NS_W, "id")).unwrap_or("").to_owned());
                }
                "bookmarkEnd" => {
                    ended.insert(n.attribute((NS_W, "id")).unwrap_or("").to_owned());
                }
                "hyperlink" => anchors.extend(n.attribute((NS_W, "anchor")).map(str::to_owned)),
                _ => {}
            }
        }
    }
    for id in ids.iter().filter(|id| !ended.contains(*id)) {
        problems.push(format!("bookmark {id} never ends"));
    }
    for a in anchors.iter().filter(|a| !bookmarks.contains(*a)) {
        problems.push(format!("link to {a}: no such bookmark"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Cell, MathSize};

    fn t(s: &str) -> Inline {
        Inline::Text(s.into())
    }

    #[test]
    fn names_numbers_and_sizes() {
        let long = "x".repeat(50);
        let names = bookmark_names(&["fig:a-b", "1st", "Fig:a.b", &long]);
        assert_eq!(names["fig:a-b"], "fig_a_b");
        assert_eq!(names["1st"], "ref_1st");
        assert_eq!(names["Fig:a.b"], "Fig_a_b_2", "unique regardless of case");
        assert_eq!(names[long.as_str()].len(), 40);
        assert!(number_runs("Figure 2").contains("SEQ Figure"));
        assert!(!number_runs("Figure 1.2").contains("SEQ"));
        assert!(!number_runs("Abb. 3").contains("SEQ"));
        assert_eq!(uri("https://a.b/x y?ü"), "https://a.b/x%20y?%C3%BC");
        assert_eq!(word_lang("de"), "de-DE");
        assert_eq!(word_lang("de-CH"), "de-CH");
        assert_eq!(svg_length("12pt"), Some(12.0));
        assert_eq!(svg_length("16"), Some(12.0));
        assert_eq!(svg_length("50%"), None);
        let res = |mime: &str, bytes: &[u8]| Resource {
            name: "x".into(),
            mime: mime.into(),
            bytes: bytes.to_vec(),
            fallback: None,
        };
        let svg = res(
            "image/svg+xml",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10mm\" viewBox=\"0 0 20 10\"/>",
        );
        let (w, h) = dimensions(&svg).unwrap();
        assert!((w / h - 2.0).abs() < 1e-9);
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&[0, 0, 0, 3, 0, 0, 0, 2]);
        assert_eq!(dimensions(&res("image/png", &png)), Some((3.0, 2.0)));
        let gif = b"GIF89a\x05\x00\x04\x00";
        assert_eq!(dimensions(&res("image/gif", gif)), Some((5.0, 4.0)));
        // SOI, an APP0 of 4 bytes, SOF0: height 7, width 9.
        let jpeg = [
            0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0, 0xff, 0xc0, 0, 11, 8, 0, 7, 0, 9,
        ];
        assert_eq!(dimensions(&res("image/jpeg", &jpeg)), Some((9.0, 7.0)));
        assert_eq!(dimensions(&res("image/png", b"nope")), None);
    }

    #[test]
    fn writes_a_checked_document() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="20pt" height="10pt"/>"#;
        let doc = Doc {
            blocks: vec![
                Block::Heading {
                    level: 1,
                    content: vec![t("One")],
                    label: Some("one".into()),
                },
                Block::Para(vec![
                    t("See "),
                    Inline::Ref {
                        label: "one".into(),
                        text: None,
                    },
                    Inline::Footnote(vec![Block::Para(vec![
                        t("Outer"),
                        Inline::Footnote(vec![Block::Para(vec![t("inner")])]),
                    ])]),
                    Inline::Math(Math {
                        image: Some("m.svg".into()),
                        size: Some(MathSize {
                            width: 20.0,
                            height: 10.0,
                            baseline: 7.0,
                        }),
                        ..Math::typst("x^2")
                    }),
                    Inline::Image(Image {
                        resource: "a.webp".into(),
                        alt: Some("A".into()),
                        width: None,
                    }),
                ]),
                Block::Table(Table {
                    columns: 2,
                    header_rows: 1,
                    rows: vec![
                        vec![Cell {
                            content: vec![],
                            colspan: 1,
                            rowspan: 2,
                        }],
                        vec![Cell::new(vec![])],
                    ],
                }),
            ],
            resources: vec![
                Resource {
                    name: "m.svg".into(),
                    mime: "image/svg+xml".into(),
                    bytes: svg.to_vec(),
                    fallback: None,
                },
                Resource {
                    name: "a.webp".into(),
                    mime: "image/webp".into(),
                    bytes: vec![1],
                    fallback: None,
                },
            ],
            ..Doc::default()
        };
        let written = Docx.write(&doc, &WriteOptions::default()).unwrap();
        assert_eq!(check(&written.bytes), Vec::<String>::new());
        let zip = Archive::new(&written.bytes).unwrap();
        let part = |name: &str| String::from_utf8(zip.read(name).unwrap().unwrap()).unwrap();
        let body = part("word/document.xml");
        assert!(body.contains("<w:hyperlink w:anchor=\"one\""));
        assert!(body.contains("<w:position w:val=\"-6\"/>"), "lowered 3 pt");
        assert!(body.contains("[A]"), "WebP left out");
        assert!(body.contains("<w:vMerge w:val=\"restart\"/>") && body.contains("<w:vMerge/>"));
        assert!(
            part("word/footnotes.xml")
                .contains("Outer</w:t></w:r><w:r><w:t xml:space=\"preserve\"> (</w:t>")
        );
        let kinds: Vec<_> = written.warnings.iter().map(|w| w.kind).collect();
        assert_eq!(kinds, [WarningKind::ImageLeftOut, WarningKind::NestedNote]);
        // The check finds what is wrong.
        let mut z = ZipWriter::new();
        z.add("[Content_Types].xml", b"<Types/>", true).unwrap();
        z.add("word/document.xml", b"<broken", true).unwrap();
        assert!(check(&z.finish().unwrap()).len() >= 3);
    }
}
