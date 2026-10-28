//! DOCX (Word 2007 and later; `.docm`, `.dotx` and `.dotm` too, macros
//! ignored): the main document part with its styles, numbering, notes,
//! relationships and media, read with [`crate::zip`] and `roxmltree`
//! (namespaces by URI, transitional and strict; no DTDs).
//!
//! - Headings by the built-in heading styles (by style name, so in any UI
//!   language, through `basedOn`) or an outline level; the Title and
//!   Subtitle styles give the metadata, `docProps/core.xml` (and the
//!   default language in the styles) what they don't.
//! - Bold, italic, underline, strike, super- and subscript from direct
//!   formatting and character styles (not paragraph styles: those are the
//!   layout's), runs of equal formatting merged; fonts, sizes, colours and
//!   page setup dropped; hidden text dropped.
//! - Lists from the numbering part: bulleted or numbered by the level's
//!   format, nested by level, one list per run of paragraphs with the same
//!   numbering instance, numbers continued as Word counts them.
//! - Footnotes and endnotes become footnotes; hyperlinks (elements and
//!   fields) links, internal ones their text; tables with spans and
//!   repeated header rows (a table in a cell flattened into it).
//! - Images from the package with their width relative to the section's
//!   text column; alone in a paragraph, or floating, a figure, captioned by
//!   a Caption paragraph next to it (below an image, above a table: a
//!   table gets captioned too), its "Figure 1:" label dropped. EMF, WMF
//!   and VML pictures, charts, shapes and embedded objects are left out.
//!   Word equations through [`crate::omml`].
//! - Fields keep their result (citation fields of Zotero, Mendeley,
//!   EndNote, Citavi and Word as text, with a warning; tables of contents
//!   and indexes dropped). Tracked changes are accepted; comments, headers
//!   and footers dropped; text boxes' paragraphs follow the paragraph they
//!   are anchored in. Content controls, smart tags and custom XML are
//!   unwrapped; `mc:AlternateContent` is read as its first choice whose
//!   namespaces are understood, else its fallback.
//!
//! The writer is in `docx_write.rs` (on the same [`Docx`]), with
//! [`check`].

use std::collections::{HashMap, HashSet};

use roxmltree::{Document, Node, ParsingOptions};

use crate::markdown::{mime, percent_decode, plain, trim};
use crate::model::{
    Block, Cell, Doc, Figure, FigureBody, Image, Inline, List, Math, Meta, Resource, Table,
};
use crate::zip::{self, Archive};
use crate::{
    Error, Imported, MAX_DEPTH, ReadOptions, Reader, Result, WarningKind, Warnings, omml, text,
};

pub use crate::docx_write::check;

/// The DOCX reader and writer.
pub struct Docx;

/// The most XML nodes a part may have.
pub const MAX_XML_NODES: u32 = 8_000_000;
/// The deepest a part's elements may nest (the XML parser recurses; Word
/// rarely goes past 40).
pub const MAX_XML_DEPTH: usize = 120;

pub(crate) const W: &[&str] = &[
    "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
    "http://purl.oclc.org/ooxml/wordprocessingml/main",
];
pub(crate) const M: &[&str] = &[
    "http://schemas.openxmlformats.org/officeDocument/2006/math",
    "http://purl.oclc.org/ooxml/officeDocument/math",
];
const R: &[&str] = &[
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    "http://purl.oclc.org/ooxml/officeDocument/relationships",
];
const A: &[&str] = &[
    "http://schemas.openxmlformats.org/drawingml/2006/main",
    "http://purl.oclc.org/ooxml/drawingml/main",
];
const WP: &[&str] = &[
    "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing",
    "http://purl.oclc.org/ooxml/drawingml/wordprocessingDrawing",
];
pub(crate) const MC: &[&str] = &["http://schemas.openxmlformats.org/markup-compatibility/2006"];
const V: &[&str] = &["urn:schemas-microsoft-com:vml"];
const SVG: &[&str] = &["http://schemas.microsoft.com/office/drawing/2016/SVG/main"];
const DC: &[&str] = &["http://purl.org/dc/elements/1.1/"];
const DCTERMS: &[&str] = &["http://purl.org/dc/terms/"];

/// The namespaces an `mc:Choice` may require for it to be read: drawings,
/// shapes and groups (text boxes and pictures are found in them), Word's
/// own extensions and math.
const UNDERSTOOD: &[&str] = &[
    "http://schemas.microsoft.com/office/word/2010/wordprocessingShape",
    "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup",
    "http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas",
    "http://schemas.microsoft.com/office/word/2010/wordprocessingDrawing",
    "http://schemas.microsoft.com/office/word/2010/wordml",
    "http://schemas.microsoft.com/office/word/2012/wordml",
    "http://schemas.microsoft.com/office/drawing/2010/main",
    "http://schemas.openxmlformats.org/officeDocument/2006/math",
];

/// Whether `n` is the element `name` in one of the namespaces `ns`.
pub(crate) fn is(n: Node, ns: &[&str], name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name && in_ns(n, ns)
}

fn in_ns(n: Node, ns: &[&str]) -> bool {
    n.tag_name().namespace().is_some_and(|u| ns.contains(&u))
}

/// The first child element `name` of `n`.
pub(crate) fn child<'a, 'i>(n: Node<'a, 'i>, ns: &[&str], name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| is(*c, ns, name))
}

/// The attribute `name` in one of the namespaces `ns`.
pub(crate) fn attr<'a>(n: Node<'a, '_>, ns: &[&str], name: &str) -> Option<&'a str> {
    ns.iter().find_map(|u| n.attribute((*u, name)))
}

/// The `w:val` of `n`'s child `name`.
fn child_val<'a>(n: Node<'a, '_>, name: &str) -> Option<&'a str> {
    child(n, W, name).and_then(|c| attr(c, W, "val"))
}

/// An on/off property element: on unless its `w:val` says off.
fn flag(n: Node) -> bool {
    !matches!(attr(n, W, "val"), Some("0" | "false" | "off"))
}

/// The branch of an `mc:AlternateContent` to read: the first choice whose
/// required namespaces are all understood, else the fallback.
pub(crate) fn alternate<'a, 'i>(n: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    n.children()
        .find(|c| {
            is(*c, MC, "Choice")
                && c.attribute("Requires").is_some_and(|r| {
                    r.split_whitespace().all(|p| {
                        c.lookup_namespace_uri(Some(p))
                            .is_some_and(|u| UNDERSTOOD.contains(&u))
                    })
                })
        })
        .or_else(|| child(n, MC, "Fallback"))
}

/// Whether `bytes` are a Word document: a zip with `word/document.xml`.
pub(crate) fn sniff(bytes: &[u8]) -> bool {
    zip::is_zip(bytes) && Archive::new(bytes).is_ok_and(|a| a.contains("word/document.xml"))
}

/// A part's text: UTF-8 (a byte order mark dropped) or UTF-16 with one.
fn decode(bytes: Vec<u8>) -> String {
    if let Some(rest) = bytes.strip_prefix(b"\xff\xfe") {
        return text::utf16(rest, u16::from_le_bytes);
    }
    if let Some(rest) = bytes.strip_prefix(b"\xfe\xff") {
        return text::utf16(rest, u16::from_be_bytes);
    }
    let rest = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
    String::from_utf8_lossy(rest).into_owned()
}

/// The text of the part `name`, if there is one.
fn xml(archive: &Archive, name: &str) -> Result<Option<String>> {
    Ok(archive.read(name)?.map(decode))
}

fn parse<'t>(text: &'t str, name: &str) -> Result<Document<'t>> {
    if too_deep(text) {
        return Err(Error::TooDeep);
    }
    let options = ParsingOptions {
        allow_dtd: false,
        nodes_limit: MAX_XML_NODES,
        ..ParsingOptions::default()
    };
    Document::parse_with_options(text, options).map_err(|e| match e {
        roxmltree::Error::NodesLimitReached => Error::TooLarge,
        e => Error::Invalid(format!("{name} is broken: {e}.")),
    })
}

/// Whether elements in `text` nest deeper than [`MAX_XML_DEPTH`], counted
/// roughly (comments and CDATA may count too).
fn too_deep(text: &str) -> bool {
    let b = text.as_bytes();
    let mut depth = 0usize;
    let mut i = 0;
    while let Some(off) = b[i..].iter().position(|&c| c == b'<') {
        i += off + 1;
        match b.get(i) {
            Some(b'/') => depth = depth.saturating_sub(1),
            Some(b'?' | b'!') | None => {}
            Some(_) => {
                // To the tag's end, skipping quoted attribute values.
                let mut quote = None;
                while i < b.len() {
                    match (quote, b[i]) {
                        (Some(q), c) if c == q => quote = None,
                        (None, c @ (b'"' | b'\'')) => quote = Some(c),
                        (None, b'>') => break,
                        _ => {}
                    }
                    i += 1;
                }
                if b.get(i.wrapping_sub(1)) != Some(&b'/') {
                    depth += 1;
                    if depth > MAX_XML_DEPTH {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// One relationship of a part.
#[derive(Debug, Clone)]
struct Rel {
    /// The type's last segment (`image`, `hyperlink`).
    kind: String,
    /// The part's name in the package, or the URL if external.
    target: String,
    external: bool,
}

type Rels = HashMap<String, Rel>;

/// The folder of a part (`word` for `word/document.xml`).
fn folder(part: &str) -> &str {
    part.rsplit_once('/').map_or("", |(d, _)| d)
}

/// The part `target` names relative to `dir`.
pub(crate) fn resolve(dir: &str, target: &str) -> String {
    let target = percent_decode(target);
    let (base, rel) = match target.strip_prefix('/') {
        Some(abs) => ("", abs.to_owned()),
        None => (dir, target),
    };
    let mut parts: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    for seg in rel.split(['/', '\\']) {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// The relationships of `part` (`""` for the package's own).
fn rels(archive: &Archive, part: &str) -> Result<Rels> {
    let (dir, file) = part.rsplit_once('/').unwrap_or(("", part));
    let path = match dir {
        "" => format!("_rels/{file}.rels"),
        d => format!("{d}/_rels/{file}.rels"),
    };
    let mut out = Rels::new();
    let Some(text) = xml(archive, &path)? else {
        return Ok(out);
    };
    let doc = parse(&text, &path)?;
    for r in doc
        .root_element()
        .children()
        .filter(|c| c.tag_name().name() == "Relationship")
    {
        let (Some(id), Some(target)) = (r.attribute("Id"), r.attribute("Target")) else {
            continue;
        };
        let external = r.attribute("TargetMode") == Some("External");
        let kind = r.attribute("Type").unwrap_or("");
        out.insert(
            id.to_owned(),
            Rel {
                kind: kind.rsplit('/').next().unwrap_or("").to_owned(),
                target: match external {
                    true => target.to_owned(),
                    false => resolve(dir, target),
                },
                external,
            },
        );
    }
    Ok(out)
}

/// The target of the first internal relationship of `kind`.
fn target(rels: &Rels, kind: &str) -> Option<String> {
    let mut found: Vec<(&String, &Rel)> = rels
        .iter()
        .filter(|(_, r)| r.kind == kind && !r.external)
        .collect();
    found.sort_by_key(|(id, _)| *id);
    found.first().map(|(_, r)| r.target.clone())
}

/// The media types of `[Content_Types].xml`.
#[derive(Default)]
struct Types {
    /// By lowercase extension.
    defaults: HashMap<String, String>,
    /// By lowercase part name.
    overrides: HashMap<String, String>,
}

impl Types {
    fn read(doc: Option<&Document>) -> Self {
        let mut t = Types::default();
        for n in doc.into_iter().flat_map(|d| d.root_element().children()) {
            let ct = n.attribute("ContentType").unwrap_or("").to_owned();
            match n.tag_name().name() {
                "Default" => {
                    if let Some(ext) = n.attribute("Extension") {
                        t.defaults.insert(ext.to_ascii_lowercase(), ct);
                    }
                }
                "Override" => {
                    if let Some(p) = n.attribute("PartName") {
                        let p = p.trim_start_matches('/').to_ascii_lowercase();
                        t.overrides.insert(p, ct);
                    }
                }
                _ => {}
            }
        }
        t
    }

    fn mime(&self, part: &str) -> String {
        let ext = part
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        self.overrides
            .get(&part.to_ascii_lowercase())
            .or_else(|| self.defaults.get(&ext))
            .filter(|m| !m.is_empty())
            .cloned()
            .unwrap_or_else(|| mime(part).to_owned())
    }
}

/// Run properties as set (`None`: not said).
#[derive(Debug, Clone, Copy, Default)]
struct Props {
    /// Bold, italic, underline, strike, superscript, subscript.
    flags: [Option<bool>; 6],
    hidden: Option<bool>,
}

impl Props {
    fn read(rpr: Option<Node>) -> Self {
        let mut p = Props::default();
        let Some(rpr) = rpr else {
            return p;
        };
        for c in rpr.children().filter(|c| in_ns(*c, W)) {
            let on = flag(c);
            match c.tag_name().name() {
                "b" => p.flags[0] = Some(on),
                "i" => p.flags[1] = Some(on),
                "u" => p.flags[2] = Some(attr(c, W, "val").is_none_or(|v| v != "none")),
                "strike" | "dstrike" => p.flags[3] = Some(on || p.flags[3] == Some(true)),
                "vertAlign" => {
                    let v = attr(c, W, "val").unwrap_or("");
                    p.flags[4] = Some(v == "superscript");
                    p.flags[5] = Some(v == "subscript");
                }
                "vanish" => p.hidden = Some(on),
                _ => {}
            }
        }
        p
    }

    /// Fills what isn't said from `other` (a style further away).
    fn fill(&mut self, other: &Props) {
        for (a, b) in self.flags.iter_mut().zip(other.flags) {
            *a = a.or(b);
        }
        self.hidden = self.hidden.or(other.hidden);
    }
}

#[derive(Debug, Default)]
struct Style {
    /// Lowercase (`heading 1`).
    name: String,
    based_on: Option<String>,
    outline: Option<u8>,
    num_id: Option<String>,
    ilvl: Option<u8>,
    run: Props,
}

#[derive(Debug, Default)]
struct Styles {
    by_id: HashMap<String, Style>,
    default_para: Option<String>,
    /// The document's default language.
    lang: Option<String>,
}

impl Styles {
    fn read(doc: Option<&Document>) -> Self {
        let mut s = Styles::default();
        let Some(root) = doc.map(|d| d.root_element()) else {
            return s;
        };
        s.lang = child(root, W, "docDefaults")
            .and_then(|d| child(d, W, "rPrDefault"))
            .and_then(|d| child(d, W, "rPr"))
            .and_then(|r| child(r, W, "lang"))
            .and_then(|l| attr(l, W, "val"))
            .map(str::to_owned);
        for st in root.children().filter(|c| is(*c, W, "style")) {
            let Some(id) = attr(st, W, "styleId") else {
                continue;
            };
            let ppr = child(st, W, "pPr");
            let num = ppr.and_then(|p| child(p, W, "numPr"));
            let style = Style {
                name: child_val(st, "name").unwrap_or("").to_lowercase(),
                based_on: child_val(st, "basedOn").map(str::to_owned),
                outline: ppr
                    .and_then(|p| child_val(p, "outlineLvl"))
                    .and_then(|v| v.parse().ok()),
                num_id: num.and_then(|n| child_val(n, "numId")).map(str::to_owned),
                ilvl: num
                    .and_then(|n| child_val(n, "ilvl"))
                    .and_then(|v| v.parse().ok()),
                run: Props::read(child(st, W, "rPr")),
            };
            if attr(st, W, "type") == Some("paragraph")
                && attr(st, W, "default").is_some_and(|d| matches!(d, "1" | "true" | "on"))
            {
                s.default_para = Some(id.to_owned());
            }
            s.by_id.entry(id.to_owned()).or_insert(style);
        }
        s
    }

    /// The style `id` and those it is based on, nearest first.
    fn chain(&self, id: &str) -> Vec<&Style> {
        let mut out: Vec<&Style> = Vec::new();
        let mut next = Some(id);
        while let Some(id) = next {
            let Some(s) = self.by_id.get(id) else {
                break;
            };
            if out.len() > 16 || out.iter().any(|o| std::ptr::eq(*o, s)) {
                break;
            }
            out.push(s);
            next = s.based_on.as_deref();
        }
        out
    }
}

/// A numbering level.
#[derive(Debug, Clone, Copy)]
struct Level {
    ordered: bool,
    start: u64,
    /// No number or bullet at all.
    none: bool,
}

fn levels(n: Node) -> HashMap<u8, Level> {
    let mut out = HashMap::new();
    for lvl in n.children().filter(|c| is(*c, W, "lvl")) {
        let Some(i) = attr(lvl, W, "ilvl").and_then(|v| v.parse().ok()) else {
            continue;
        };
        let fmt = child_val(lvl, "numFmt").unwrap_or("decimal");
        out.insert(
            i,
            Level {
                ordered: !matches!(fmt, "bullet" | "none"),
                start: child_val(lvl, "start")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1),
                none: fmt == "none",
            },
        );
    }
    out
}

#[derive(Debug, Default)]
struct Abstract {
    levels: HashMap<u8, Level>,
    /// A numbering style it takes its levels from.
    link: Option<String>,
}

#[derive(Debug, Default)]
struct Num {
    abstract_id: String,
    /// Levels restarting at a number (`w:startOverride`).
    starts: HashMap<u8, u64>,
    levels: HashMap<u8, Level>,
}

#[derive(Debug, Default)]
struct Numbering {
    abstracts: HashMap<String, Abstract>,
    nums: HashMap<String, Num>,
}

impl Numbering {
    fn read(doc: Option<&Document>) -> Self {
        let mut n = Numbering::default();
        let Some(root) = doc.map(|d| d.root_element()) else {
            return n;
        };
        for a in root.children().filter(|c| is(*c, W, "abstractNum")) {
            if let Some(id) = attr(a, W, "abstractNumId") {
                n.abstracts.insert(
                    id.to_owned(),
                    Abstract {
                        levels: levels(a),
                        link: child_val(a, "numStyleLink").map(str::to_owned),
                    },
                );
            }
        }
        for num in root.children().filter(|c| is(*c, W, "num")) {
            let Some(id) = attr(num, W, "numId") else {
                continue;
            };
            let mut entry = Num {
                abstract_id: child_val(num, "abstractNumId").unwrap_or("").to_owned(),
                ..Num::default()
            };
            for o in num.children().filter(|c| is(*c, W, "lvlOverride")) {
                let Some(i) = attr(o, W, "ilvl").and_then(|v| v.parse().ok()) else {
                    continue;
                };
                if let Some(s) = child_val(o, "startOverride").and_then(|v| v.parse().ok()) {
                    entry.starts.insert(i, s);
                }
                entry.levels.extend(levels(o));
            }
            n.nums.insert(id.to_owned(), entry);
        }
        n
    }
}

/// What a paragraph is.
#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Normal,
    Heading(u8),
    Title,
    Subtitle,
    Caption,
    Item {
        num: String,
        level: u8,
        ordered: bool,
        number: u64,
    },
}

/// Bold, italic, underline, strike, superscript, subscript.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Fmt([bool; 6]);

const UNDERLINE: usize = 2;

/// Something a paragraph holds, in order.
#[derive(Debug)]
enum Atom {
    Inline(Fmt, Inline),
    /// A display formula: the paragraph is split around it.
    Display(Math),
}

/// Inline content being collected: the paragraph's, or a link's in it.
#[derive(Debug, Default)]
struct Frame {
    atoms: Vec<Atom>,
    link: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
enum FieldKind {
    Plain,
    Link(String),
    /// A reference manager's citation or bibliography, by its name.
    Citation(String),
    Contents(String),
    /// A caption's number (`SEQ Figure`).
    Seq,
}

/// A complex field (`w:fldChar` begin … separate … end).
#[derive(Debug)]
struct Field {
    instr: String,
    /// Past its `separate`: its result is showing.
    result: bool,
    kind: FieldKind,
    /// The link frame its result goes into.
    frame: Option<usize>,
}

/// Splits a field instruction into words, quoted ones as one.
fn words(instr: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in instr.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            c => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// What a field instruction makes of its result.
fn classify(instr: &str) -> FieldKind {
    let w = words(instr);
    let first = w
        .first()
        .map(|s| s.to_ascii_uppercase())
        .unwrap_or_default();
    match first.as_str() {
        "HYPERLINK" => {
            let (mut url, mut anchor) = (None, None);
            let mut it = w[1..].iter();
            while let Some(s) = it.next() {
                match s.as_str() {
                    "\\l" => anchor = it.next(),
                    "\\o" | "\\t" => {
                        it.next();
                    }
                    s if s.starts_with('\\') => {}
                    _ if url.is_none() => url = Some(s),
                    _ => {}
                }
            }
            // Internal links (`\l` only) keep their text.
            match (url.filter(|u| !u.is_empty()), anchor) {
                (Some(u), Some(a)) => FieldKind::Link(format!("{u}#{a}")),
                (Some(u), None) => FieldKind::Link(u.clone()),
                _ => FieldKind::Link(String::new()),
            }
        }
        "TOC" | "INDEX" | "TOA" => FieldKind::Contents(first),
        "SEQ" => FieldKind::Seq,
        "CITATION" | "BIBLIOGRAPHY" => FieldKind::Citation(first),
        "ADDIN" => {
            let name = w.get(1).cloned().unwrap_or_default();
            let known = ["ZOTERO_", "EN.", "CSL_", "MENDELEY", "CITAVI", "PAPERS2"];
            match known
                .iter()
                .any(|k| name.to_ascii_uppercase().starts_with(k))
            {
                true => FieldKind::Citation(name),
                false => FieldKind::Plain,
            }
        }
        _ => FieldKind::Plain,
    }
}

/// The Symbol font's letters (A–Z, a–z) as Unicode.
const SYMBOL_UPPER: &str = "ΑΒΧΔΕΦΓΗΙϑΚΛΜΝΟΠΘΡΣΤΥςΩΞΨΖ";
const SYMBOL_LOWER: &str = "αβχδεφγηιϕκλμνοπθρστυϖωξψζ";
/// Its common symbols.
const SYMBOLS: &[(u8, char)] = &[
    (0x22, '∀'),
    (0x24, '∃'),
    (0x27, '∋'),
    (0x40, '≅'),
    (0x5c, '∴'),
    (0x5e, '⊥'),
    (0x7e, '∼'),
    (0xa2, '′'),
    (0xa3, '≤'),
    (0xa5, '∞'),
    (0xac, '←'),
    (0xae, '→'),
    (0xb0, '°'),
    (0xb1, '±'),
    (0xb2, '″'),
    (0xb3, '≥'),
    (0xb4, '×'),
    (0xb6, '∂'),
    (0xb7, '•'),
    (0xb8, '÷'),
    (0xb9, '≠'),
    (0xba, '≡'),
    (0xbb, '≈'),
    (0xbc, '…'),
    (0xc4, '⊗'),
    (0xc5, '⊕'),
    (0xc6, '∅'),
    (0xc7, '∩'),
    (0xc8, '∪'),
    (0xc9, '⊃'),
    (0xcc, '⊂'),
    (0xcd, '⊆'),
    (0xce, '∈'),
    (0xcf, '∉'),
    (0xd1, '∇'),
    (0xd5, '∏'),
    (0xd6, '√'),
    (0xd7, '⋅'),
    (0xd8, '¬'),
    (0xd9, '∧'),
    (0xda, '∨'),
    (0xdb, '⇔'),
    (0xde, '⇒'),
    (0xe1, '〈'),
    (0xe5, '∑'),
    (0xf1, '〉'),
    (0xf2, '∫'),
];

/// The character of a `w:sym`: the Symbol font's mapped to Unicode, other
/// fonts' as given.
fn sym(n: Node) -> Option<char> {
    let code = u32::from_str_radix(attr(n, W, "char")?, 16).ok()?;
    let symbol = attr(n, W, "font").is_some_and(|f| f.eq_ignore_ascii_case("symbol"));
    let low = code & 0xff;
    if symbol && (code < 0x100 || (0xf000..=0xf0ff).contains(&code)) {
        let b = low as u8;
        return match b {
            b'A'..=b'Z' => SYMBOL_UPPER.chars().nth(usize::from(b - b'A')),
            b'a'..=b'z' => SYMBOL_LOWER.chars().nth(usize::from(b - b'a')),
            _ => SYMBOLS
                .iter()
                .find(|(k, _)| *k == b)
                .map(|(_, c)| *c)
                .or_else(|| (0x20..0x7f).contains(&b).then(|| char::from(b))),
        };
    }
    char::from_u32(code).filter(|c| !c.is_control())
}

/// A paragraph or block on its way to the model.
#[derive(Debug)]
enum Piece {
    Block(Block),
    Para {
        kind: Kind,
        content: Vec<Inline>,
    },
    /// An image alone: a figure, captioned if a caption claims it.
    Image(Image),
}

/// A list paragraph.
struct Entry {
    num: String,
    level: u8,
    ordered: bool,
    number: u64,
    content: Vec<Inline>,
}

impl Reader for Docx {
    fn read(&self, bytes: &[u8], _: &ReadOptions) -> Result<Imported> {
        let archive = Archive::new(bytes)?;
        let package = rels(&archive, "")?;
        let main = target(&package, "officeDocument").unwrap_or("word/document.xml".into());
        let Some(main_text) = xml(&archive, &main)? else {
            return Err(Error::Invalid(
                "Not a Word document: it has no main document part.".into(),
            ));
        };
        let doc_rels = rels(&archive, &main)?;
        let dir = folder(&main);
        let part =
            |kind: &str, file: &str| target(&doc_rels, kind).unwrap_or_else(|| resolve(dir, file));
        let (styles_name, numbering_name) = (
            part("styles", "styles.xml"),
            part("numbering", "numbering.xml"),
        );
        let (foot_name, end_name) = (
            part("footnotes", "footnotes.xml"),
            part("endnotes", "endnotes.xml"),
        );
        let core_name = target(&package, "core-properties").unwrap_or("docProps/core.xml".into());
        let texts = [
            xml(&archive, &styles_name)?,
            xml(&archive, &numbering_name)?,
            xml(&archive, &foot_name)?,
            xml(&archive, &end_name)?,
            xml(&archive, &core_name)?,
            xml(&archive, "[Content_Types].xml")?,
        ];
        let names = [
            &styles_name,
            &numbering_name,
            &foot_name,
            &end_name,
            &core_name,
            "[Content_Types].xml",
        ];
        let mut docs = Vec::new();
        for (t, name) in texts.iter().zip(names) {
            docs.push(t.as_deref().map(|t| parse(t, name)).transpose()?);
        }
        let main_doc = parse(&main_text, &main)?;
        let mut c = Converter {
            archive: &archive,
            styles: Styles::read(docs[0].as_ref()),
            numbering: Numbering::read(docs[1].as_ref()),
            types: Types::read(docs[5].as_ref()),
            footnotes: notes(docs[2].as_ref(), "footnote"),
            endnotes: notes(docs[3].as_ref(), "endnote"),
            rels: [
                doc_rels.clone(),
                rels(&archive, &foot_name)?,
                rels(&archive, &end_name)?,
            ],
            part: 0,
            active: HashSet::new(),
            widths: Vec::new(),
            section: 0,
            resources: Vec::new(),
            by_part: HashMap::new(),
            counters: HashMap::new(),
            seen_nums: HashSet::new(),
            fields: Vec::new(),
            frames: Vec::new(),
            after: Vec::new(),
            base: 0,
            seq_end: None,
            cells: 0,
            depth: 0,
            meta: Meta::default(),
            warnings: Warnings::default(),
        };
        let body = child(main_doc.root_element(), W, "body");
        let sections: Vec<Node> = body
            .into_iter()
            .flat_map(|b| b.descendants())
            .filter(|n| {
                is(*n, W, "sectPr")
                    && n.parent()
                        .is_some_and(|p| is(p, W, "pPr") || is(p, W, "body"))
            })
            .collect();
        c.widths = sections.iter().map(|s| text_width(*s)).collect();
        let mut pieces = Vec::new();
        if let Some(body) = body {
            c.block_children(body, &mut pieces)?;
        }
        let blocks = c.assemble(pieces, true);
        c.headers_footers(&sections, &doc_rels)?;
        let mut meta = std::mem::take(&mut c.meta);
        core(docs[4].as_ref(), &mut meta);
        if meta.lang.is_none() {
            meta.lang = c.styles.lang.clone();
        }
        Ok(Imported {
            doc: Doc {
                meta,
                blocks,
                resources: c.resources,
            },
            warnings: c.warnings.into_vec(),
        })
    }
}

/// The notes (`footnote` or `endnote`) by id, separators left out.
fn notes<'d>(doc: Option<&'d Document<'d>>, name: &str) -> HashMap<String, Node<'d, 'd>> {
    let mut out = HashMap::new();
    for n in doc.into_iter().flat_map(|d| d.root_element().children()) {
        let real = attr(n, W, "type").is_none_or(|t| t == "normal");
        if is(n, W, name)
            && real
            && let Some(id) = attr(n, W, "id")
        {
            out.entry(id.to_owned()).or_insert(n);
        }
    }
    out
}

/// The text column's width of a section in EMU (page width minus the
/// margins), `NaN` if not given.
fn text_width(sect: Node) -> f64 {
    let num = |n: Option<Node>, names: &[&str]| {
        names
            .iter()
            .find_map(|a| n.and_then(|n| attr(n, W, a)))
            .and_then(|v| v.parse::<f64>().ok())
    };
    let page = num(child(sect, W, "pgSz"), &["w"]);
    let mar = child(sect, W, "pgMar");
    let left = num(mar, &["left", "start"]).unwrap_or(0.0);
    let right = num(mar, &["right", "end"]).unwrap_or(0.0);
    // Twips → EMU.
    page.map_or(f64::NAN, |p| (p - left - right) * 635.0)
}

/// Title, authors, date and language from `docProps/core.xml`, where the
/// body didn't give them.
fn core(doc: Option<&Document>, meta: &mut Meta) {
    let Some(root) = doc.map(|d| d.root_element()) else {
        return;
    };
    let text = |ns: &[&str], name: &str| {
        child(root, ns, name)
            .and_then(|n| n.text())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
    };
    if meta.title.is_none() {
        meta.title = text(DC, "title");
    }
    if meta.authors.is_empty() {
        meta.authors = text(DC, "creator")
            .map(|c| {
                c.split(';')
                    .map(|a| a.trim().to_owned())
                    .filter(|a| !a.is_empty())
                    .collect()
            })
            .unwrap_or_default();
    }
    if meta.date.is_none() {
        meta.date = text(DCTERMS, "created").map(|d| match d.split_once('T') {
            Some((day, _)) => day.to_owned(),
            None => d,
        });
    }
    if meta.lang.is_none() {
        meta.lang = text(DC, "language");
    }
}

struct Converter<'d> {
    archive: &'d Archive<'d>,
    styles: Styles,
    numbering: Numbering,
    types: Types,
    footnotes: HashMap<String, Node<'d, 'd>>,
    endnotes: HashMap<String, Node<'d, 'd>>,
    /// The relationships of the document, its footnotes and endnotes.
    rels: [Rels; 3],
    /// Which of them the part being read uses.
    part: usize,
    /// Notes being read (a note referring to itself).
    active: HashSet<(bool, String)>,
    /// Each section's text width (EMU), and the section being read.
    widths: Vec<f64>,
    section: usize,
    resources: Vec<Resource>,
    /// Resource names by part (`None`: left out).
    by_part: HashMap<String, Option<String>>,
    /// List counters by abstract numbering, per level.
    counters: HashMap<String, [Option<u64>; 9]>,
    seen_nums: HashSet<String>,
    fields: Vec<Field>,
    frames: Vec<Frame>,
    /// Pieces to place after the paragraph being read (text boxes,
    /// floating images), one list per paragraph level.
    after: Vec<Vec<Piece>>,
    /// The paragraph's frame.
    base: usize,
    /// Where a caption's number ended (in its paragraph's atoms).
    seq_end: Option<usize>,
    /// Inside a table cell.
    cells: usize,
    depth: usize,
    meta: Meta,
    warnings: Warnings,
}

impl<'d> Converter<'d> {
    fn enter(&mut self) -> Result<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Error::TooDeep);
        }
        Ok(())
    }

    /// The paragraphs and tables in `parent` (a body, cell, note or text
    /// box).
    fn block_children(&mut self, parent: Node<'d, 'd>, out: &mut Vec<Piece>) -> Result<()> {
        self.enter()?;
        for c in parent.children().filter(Node::is_element) {
            if is(c, MC, "AlternateContent") {
                if let Some(b) = alternate(c) {
                    self.block_children(b, out)?;
                }
                continue;
            }
            if !in_ns(c, W) {
                continue;
            }
            match c.tag_name().name() {
                "p" => self.paragraph(c, out)?,
                "tbl" if self.cells > 0 => {
                    self.warnings.add(WarningKind::NestedTable);
                    if let Block::Table(t) = self.table(c)? {
                        for cell in t.rows.into_iter().flatten() {
                            out.extend(cell.content.into_iter().map(Piece::Block));
                        }
                    }
                }
                "tbl" => {
                    let t = self.table(c)?;
                    out.push(Piece::Block(t));
                }
                "sdt" => {
                    if let Some(content) = child(c, W, "sdtContent") {
                        self.block_children(content, out)?;
                    }
                }
                "customXml" | "sdtContent" => self.block_children(c, out)?,
                "ins" | "moveTo" => {
                    self.warnings.add(WarningKind::TrackedChange);
                    self.block_children(c, out)?;
                }
                "del" | "moveFrom" => self.warnings.add(WarningKind::TrackedChange),
                "altChunk" => self
                    .warnings
                    .add_example(WarningKind::Object, "embedded document"),
                _ => {}
            }
        }
        self.depth -= 1;
        Ok(())
    }

    /// What the paragraph with properties `ppr` is.
    fn kind(&mut self, ppr: Option<Node>) -> Kind {
        let style = ppr
            .and_then(|p| child_val(p, "pStyle"))
            .map(str::to_owned)
            .or_else(|| self.styles.default_para.clone());
        let chain = style
            .as_deref()
            .map(|s| self.styles.chain(s))
            .unwrap_or_default();
        for s in &chain {
            let heading = s
                .name
                .strip_prefix("heading ")
                .and_then(|n| n.parse::<u8>().ok())
                .filter(|n| (1..=9).contains(n));
            match s.name.as_str() {
                "title" => return Kind::Title,
                "subtitle" => return Kind::Subtitle,
                "caption" => return Kind::Caption,
                _ if heading.is_some() => return Kind::Heading(heading.unwrap_or(1).min(6)),
                _ => {}
            }
        }
        let outline = ppr
            .and_then(|p| child_val(p, "outlineLvl"))
            .and_then(|v| v.parse::<u8>().ok())
            .or_else(|| chain.iter().find_map(|s| s.outline));
        if let Some(o) = outline.filter(|o| *o < 9) {
            return Kind::Heading((o + 1).min(6));
        }
        let direct = ppr.and_then(|p| child(p, W, "numPr"));
        let num_id = direct
            .and_then(|n| child_val(n, "numId"))
            .or_else(|| chain.iter().find_map(|s| s.num_id.as_deref()))
            .map(str::to_owned);
        let ilvl = direct
            .and_then(|n| child_val(n, "ilvl"))
            .and_then(|v| v.parse::<u8>().ok())
            .or_else(|| chain.iter().find_map(|s| s.ilvl))
            .unwrap_or(0)
            .min(8);
        match num_id.filter(|n| n != "0") {
            Some(num) => match self.number(&num, ilvl) {
                Some((ordered, number)) => Kind::Item {
                    num,
                    level: ilvl,
                    ordered,
                    number,
                },
                None => Kind::Normal,
            },
            None => Kind::Normal,
        }
    }

    /// The level `ilvl` of numbering `num_id`: its abstract numbering's id,
    /// the level and a restart number.
    fn level(&self, num_id: &str, ilvl: u8) -> Option<(String, Level, Option<u64>)> {
        let num = self.numbering.nums.get(num_id)?;
        let mut abstract_id = num.abstract_id.clone();
        let mut abs = self.numbering.abstracts.get(&abstract_id)?;
        // A numbering style's levels live where the style's numbering points.
        for _ in 0..4 {
            let Some(link) = &abs.link else {
                break;
            };
            let id = self
                .styles
                .chain(link)
                .iter()
                .find_map(|s| s.num_id.clone())?;
            abstract_id = self.numbering.nums.get(&id)?.abstract_id.clone();
            abs = self.numbering.abstracts.get(&abstract_id)?;
        }
        let level = num.levels.get(&ilvl).or_else(|| abs.levels.get(&ilvl))?;
        Some((abstract_id, *level, num.starts.get(&ilvl).copied()))
    }

    /// The next item of numbering `num_id` at `ilvl`: whether numbered,
    /// and its number as Word counts it.
    fn number(&mut self, num_id: &str, ilvl: u8) -> Option<(bool, u64)> {
        let (abs, level, start) = self.level(num_id, ilvl)?;
        if level.none {
            return None;
        }
        let first = self.seen_nums.insert(num_id.to_owned());
        let restarts: Vec<u8> = self
            .numbering
            .nums
            .get(num_id)
            .map(|n| n.starts.keys().copied().collect())
            .unwrap_or_default();
        let counters = self.counters.entry(abs).or_insert([None; 9]);
        if first {
            for l in restarts {
                if let Some(c) = counters.get_mut(usize::from(l)) {
                    *c = None;
                }
            }
        }
        let i = usize::from(ilvl.min(8));
        let n = match counters[i] {
            Some(k) => k.saturating_add(1),
            None => start.unwrap_or(level.start),
        };
        counters[i] = Some(n);
        for c in &mut counters[i + 1..] {
            *c = None;
        }
        Some((level.ordered, n))
    }

    fn paragraph(&mut self, p: Node<'d, 'd>, out: &mut Vec<Piece>) -> Result<()> {
        self.enter()?;
        let ppr = child(p, W, "pPr");
        let kind = self.kind(ppr);
        let saved = (self.base, self.seq_end.take());
        self.base = self.frames.len();
        self.frames.push(Frame::default());
        self.after.push(Vec::new());
        let walked = self.inlines(p);
        while self.frames.len() > self.base + 1 {
            self.close_frame();
        }
        let frame = self.frames.pop().unwrap_or_default();
        for f in &mut self.fields {
            if f.frame.is_some_and(|i| i >= self.base) {
                f.frame = None;
            }
        }
        let after = self.after.pop().unwrap_or_default();
        let seq = std::mem::replace(&mut self.seq_end, saved.1);
        self.base = saved.0;
        walked?;
        if ppr.is_some_and(|p| child(p, W, "sectPr").is_some()) {
            self.section += 1;
        }
        let mut atoms = frame.atoms;
        if kind == Kind::Caption
            && let Some(n) = seq
        {
            atoms.drain(..n.min(atoms.len()));
            strip_label(&mut atoms);
        }
        let mut kind = Some(kind);
        let mut seg = Vec::new();
        for atom in atoms {
            match atom {
                Atom::Inline(f, i) => seg.push((f, i)),
                Atom::Display(math) => {
                    push_para(&mut kind, std::mem::take(&mut seg), out);
                    out.push(Piece::Block(Block::Math {
                        math,
                        label: None,
                        number: None,
                    }));
                }
            }
        }
        push_para(&mut kind, seg, out);
        out.extend(after);
        self.depth -= 1;
        Ok(())
    }

    fn push(&mut self, fmt: Fmt, inline: Inline) {
        if let Some(f) = self.frames.last_mut() {
            f.atoms.push(Atom::Inline(fmt, inline));
        }
    }

    fn text(&mut self, fmt: Fmt, s: &str) {
        if !s.is_empty() {
            self.push(fmt, Inline::Text(s.to_owned()));
        }
    }

    /// Ends the innermost frame: a link around its content, or its content
    /// into the frame around it (an internal link's without the
    /// underline it is styled with).
    fn close_frame(&mut self) {
        let Some(frame) = self.frames.pop() else {
            return;
        };
        let Some(url) = frame.link else {
            if let Some(top) = self.frames.last_mut() {
                top.atoms.extend(frame.atoms);
            }
            return;
        };
        // Links show underlined anyway.
        let atoms = frame.atoms.into_iter().map(|a| match a {
            Atom::Inline(mut f, i) => {
                f.0[UNDERLINE] = false;
                Atom::Inline(f, i)
            }
            display => display,
        });
        if url.is_empty() {
            if let Some(top) = self.frames.last_mut() {
                top.atoms.extend(atoms);
            }
            return;
        }
        let atoms = atoms
            .map(|a| match a {
                Atom::Inline(f, i) => (f, i),
                Atom::Display(m) => (Fmt::default(), Inline::Math(m)),
            })
            .collect();
        let (lead, content, trail) = hoist(build(atoms));
        self.text(Fmt::default(), &lead);
        if !content.is_empty() {
            self.push(Fmt::default(), Inline::Link { url, content });
        }
        self.text(Fmt::default(), &trail);
    }

    /// Opens a link frame (`url` empty: an internal link); its index.
    fn open_link(&mut self, url: String) -> usize {
        self.frames.push(Frame {
            atoms: Vec::new(),
            link: Some(url),
        });
        self.frames.len() - 1
    }

    /// Closes frames down to and including `index`, if it is still open in
    /// this paragraph.
    fn close_to(&mut self, index: usize) {
        if index <= self.base || index >= self.frames.len() {
            return;
        }
        while self.frames.len() > index {
            self.close_frame();
        }
    }

    /// Whether content is hidden now: a field's instruction, or the result
    /// of a field dropped.
    fn hidden(&self) -> bool {
        self.fields
            .iter()
            .any(|f| !f.result || matches!(f.kind, FieldKind::Contents(_)))
    }

    /// The inline content of `n` (a paragraph, link, change, control).
    fn inlines(&mut self, n: Node<'d, 'd>) -> Result<()> {
        self.enter()?;
        for c in n.children().filter(Node::is_element) {
            if is(c, MC, "AlternateContent") {
                if let Some(b) = alternate(c) {
                    self.inlines(b)?;
                }
                continue;
            }
            if in_ns(c, M) {
                if self.hidden() {
                    continue;
                }
                match c.tag_name().name() {
                    "oMathPara" => {
                        for m in c.children().filter(|m| is(*m, M, "oMath")) {
                            let math = omml::to_typst(m, &mut self.warnings);
                            if !math.is_empty()
                                && let Some(f) = self.frames.last_mut()
                            {
                                f.atoms.push(Atom::Display(Math::typst(math)));
                            }
                        }
                    }
                    "oMath" => {
                        let math = omml::to_typst(c, &mut self.warnings);
                        if !math.is_empty() {
                            self.push(Fmt::default(), Inline::Math(Math::typst(math)));
                        }
                    }
                    _ => {}
                }
                continue;
            }
            if !in_ns(c, W) {
                continue;
            }
            match c.tag_name().name() {
                "r" => {
                    let (fmt, hidden) = self.format(child(c, W, "rPr"));
                    self.run(c, fmt, hidden)?;
                }
                "hyperlink" => self.hyperlink(c)?,
                "fldSimple" => self.simple_field(c)?,
                "ins" | "moveTo" => {
                    self.warnings.add(WarningKind::TrackedChange);
                    self.inlines(c)?;
                }
                "del" | "moveFrom" => self.warnings.add(WarningKind::TrackedChange),
                "smartTag" | "customXml" | "dir" | "bdo" | "sdtContent" => self.inlines(c)?,
                "sdt" => {
                    if let Some(content) = child(c, W, "sdtContent") {
                        self.inlines(content)?;
                    }
                }
                _ => {}
            }
        }
        self.depth -= 1;
        Ok(())
    }

    /// A run's formatting: its own, then its character style's; and
    /// whether it is hidden.
    fn format(&self, rpr: Option<Node>) -> (Fmt, bool) {
        let mut p = Props::read(rpr);
        if let Some(id) = rpr.and_then(|r| child_val(r, "rStyle")) {
            for s in self.styles.chain(id) {
                p.fill(&s.run);
            }
        }
        (
            Fmt(p.flags.map(|f| f.unwrap_or(false))),
            p.hidden.unwrap_or(false),
        )
    }

    /// The content of a run (or of a branch of alternate content in one).
    fn run(&mut self, r: Node<'d, 'd>, fmt: Fmt, hidden: bool) -> Result<()> {
        for c in r.children().filter(Node::is_element) {
            if is(c, MC, "AlternateContent") {
                if let Some(b) = alternate(c) {
                    self.run(b, fmt, hidden)?;
                }
                continue;
            }
            if !in_ns(c, W) {
                continue;
            }
            let name = c.tag_name().name();
            match name {
                "fldChar" => {
                    self.field_char(c);
                    continue;
                }
                "instrText" => {
                    if let Some(f) = self.fields.last_mut().filter(|f| !f.result) {
                        f.instr.push_str(c.text().unwrap_or(""));
                    }
                    continue;
                }
                _ if hidden || self.hidden() => continue,
                _ => {}
            }
            match name {
                "t" => self.text(fmt, c.text().unwrap_or("")),
                "tab" | "ptab" => self.text(fmt, " "),
                "br" => match attr(c, W, "type") {
                    Some("page" | "column") => self.warnings.add(WarningKind::Break),
                    _ => self.push(fmt, Inline::LineBreak),
                },
                "cr" => self.push(fmt, Inline::LineBreak),
                "sym" => {
                    if let Some(ch) = sym(c) {
                        self.text(fmt, &ch.to_string());
                    }
                }
                "noBreakHyphen" => self.text(fmt, "\u{2011}"),
                "softHyphen" => self.text(fmt, "\u{ad}"),
                "footnoteReference" | "endnoteReference" => {
                    let id = attr(c, W, "id").unwrap_or("");
                    let note = self.note(name == "endnoteReference", id)?;
                    self.push(Fmt::default(), note);
                }
                "drawing" => self.drawing(c)?,
                "pict" => self.pict(c)?,
                "object" => self
                    .warnings
                    .add_example(WarningKind::Object, "embedded object"),
                "commentReference" => self.warnings.add(WarningKind::Comment),
                "ruby" => {
                    if let Some(base) = child(c, W, "rubyBase") {
                        self.inlines(base)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn field_char(&mut self, c: Node) {
        match attr(c, W, "fldCharType") {
            Some("begin") => self.fields.push(Field {
                instr: String::new(),
                result: false,
                kind: FieldKind::Plain,
                frame: None,
            }),
            Some("separate") => {
                let Some(f) = self.fields.last_mut().filter(|f| !f.result) else {
                    return;
                };
                f.result = true;
                f.kind = classify(&f.instr);
                let kind = f.kind.clone();
                // A field inside another's instruction or a dropped result.
                let hidden = self.fields[..self.fields.len() - 1]
                    .iter()
                    .any(|f| !f.result || matches!(f.kind, FieldKind::Contents(_)));
                if hidden {
                    return;
                }
                match kind {
                    FieldKind::Link(url) => {
                        let index = self.open_link(url);
                        if let Some(f) = self.fields.last_mut() {
                            f.frame = Some(index);
                        }
                    }
                    FieldKind::Citation(name) => {
                        self.warnings.add_example(WarningKind::CitationField, name);
                    }
                    FieldKind::Contents(name) => {
                        self.warnings.add_example(WarningKind::Contents, name);
                    }
                    _ => {}
                }
            }
            Some("end") => {
                let Some(f) = self.fields.pop() else {
                    return;
                };
                if let Some(index) = f.frame {
                    self.close_to(index);
                }
                if f.kind == FieldKind::Seq && self.frames.len() == self.base + 1 {
                    self.seq_end = self.frames.last().map(|f| f.atoms.len());
                }
            }
            _ => {}
        }
    }

    /// `w:fldSimple`: the result of a field given whole.
    fn simple_field(&mut self, f: Node<'d, 'd>) -> Result<()> {
        if self.hidden() {
            // Still read for the fields in it.
            return self.inlines(f);
        }
        match classify(attr(f, W, "instr").unwrap_or("")) {
            FieldKind::Contents(name) => self.warnings.add_example(WarningKind::Contents, name),
            FieldKind::Link(url) => {
                let index = self.open_link(url);
                self.inlines(f)?;
                self.close_to(index);
            }
            FieldKind::Citation(name) => {
                self.warnings.add_example(WarningKind::CitationField, name);
                self.inlines(f)?;
            }
            FieldKind::Seq => {
                self.inlines(f)?;
                if self.frames.len() == self.base + 1 {
                    self.seq_end = self.frames.last().map(|f| f.atoms.len());
                }
            }
            FieldKind::Plain => self.inlines(f)?,
        }
        Ok(())
    }

    fn rels(&self) -> &Rels {
        &self.rels[self.part]
    }

    /// `w:hyperlink`: a link if it goes outside the document, else its
    /// content.
    fn hyperlink(&mut self, h: Node<'d, 'd>) -> Result<()> {
        let url = attr(h, R, "id")
            .and_then(|id| self.rels().get(id))
            .filter(|r| r.external)
            .map(|r| match attr(h, W, "anchor") {
                Some(a) => format!("{}#{a}", r.target),
                None => r.target.clone(),
            });
        // Hidden, it is still read for the fields in it.
        let index = (!self.hidden()).then(|| self.open_link(url.unwrap_or_default()));
        self.inlines(h)?;
        if let Some(index) = index {
            self.close_to(index);
        }
        Ok(())
    }

    /// A footnote or endnote's content as a footnote.
    fn note(&mut self, end: bool, id: &str) -> Result<Inline> {
        let key = (end, id.to_owned());
        let notes = if end { &self.endnotes } else { &self.footnotes };
        let Some(node) = notes
            .get(id)
            .copied()
            .filter(|_| !self.active.contains(&key))
        else {
            let written = format!("[{id}]");
            self.warnings
                .add_example(WarningKind::MissingFootnote, written.clone());
            return Ok(Inline::Text(written));
        };
        self.active.insert(key.clone());
        let fields = std::mem::take(&mut self.fields);
        let part = std::mem::replace(&mut self.part, if end { 2 } else { 1 });
        let mut pieces = Vec::new();
        let read = self.block_children(node, &mut pieces);
        self.part = part;
        self.fields = fields;
        self.active.remove(&key);
        read?;
        Ok(Inline::Footnote(self.assemble(pieces, false)))
    }

    /// A text box's paragraphs, after the paragraph holding it.
    fn text_box(&mut self, content: Node<'d, 'd>) -> Result<()> {
        self.warnings.add(WarningKind::TextBox);
        let fields = std::mem::take(&mut self.fields);
        let mut pieces = Vec::new();
        let read = self.block_children(content, &mut pieces);
        self.fields = fields;
        read?;
        if let Some(after) = self.after.last_mut() {
            after.extend(pieces);
        }
        Ok(())
    }

    /// The outermost text boxes in `n`.
    fn text_boxes(n: Node<'d, 'd>) -> Vec<Node<'d, 'd>> {
        n.descendants()
            .filter(|t| {
                is(*t, W, "txbxContent")
                    && t.ancestors()
                        .skip(1)
                        .take_while(|a| *a != n)
                        .all(|a| !is(a, W, "txbxContent"))
            })
            .collect()
    }

    /// `w:drawing`: pictures (inline in the text, or floating: after the
    /// paragraph), text boxes; charts and shapes left out.
    fn drawing(&mut self, d: Node<'d, 'd>) -> Result<()> {
        for shape in d
            .children()
            .filter(|c| is(*c, WP, "inline") || is(*c, WP, "anchor"))
        {
            let inline = is(shape, WP, "inline");
            let alt = child(shape, WP, "docPr").and_then(|p| {
                [p.attribute("descr"), p.attribute("title")]
                    .into_iter()
                    .flatten()
                    .map(str::trim)
                    .find(|a| !a.is_empty())
                    .map(str::to_owned)
            });
            let cx = child(shape, WP, "extent")
                .and_then(|e| e.attribute("cx"))
                .and_then(|v| v.parse::<f64>().ok());
            let blip = shape
                .descendants()
                .find(|n| is(*n, SVG, "svgBlip"))
                .or_else(|| shape.descendants().find(|n| is(*n, A, "blip")));
            let boxes = Self::text_boxes(shape);
            match blip {
                Some(b) => {
                    if let Some(id) = attr(b, R, "embed") {
                        if let Some(img) = self.image(id, cx, alt) {
                            match inline {
                                true => self.push(Fmt::default(), Inline::Image(img)),
                                false => {
                                    if let Some(after) = self.after.last_mut() {
                                        after.push(Piece::Image(img));
                                    }
                                }
                            }
                        }
                    } else if let Some(id) = attr(b, R, "link") {
                        let shown = self.rels().get(id).map_or(id, |r| r.target.as_str());
                        let shown: String = shown.chars().take(40).collect();
                        self.warnings.add_example(WarningKind::MissingImage, shown);
                    }
                }
                None if boxes.is_empty() => {
                    let uri = shape
                        .descendants()
                        .find(|n| is(*n, A, "graphicData"))
                        .and_then(|g| g.attribute("uri"))
                        .unwrap_or("");
                    let what = match uri {
                        u if u.contains("chart") => "chart",
                        u if u.contains("diagram") => "diagram",
                        _ => "shape",
                    };
                    self.warnings.add_example(WarningKind::Object, what);
                }
                None => {}
            }
            for t in boxes {
                self.text_box(t)?;
            }
        }
        Ok(())
    }

    /// `w:pict` (VML): text boxes kept, pictures left out.
    fn pict(&mut self, p: Node<'d, 'd>) -> Result<()> {
        for t in Self::text_boxes(p) {
            self.text_box(t)?;
        }
        for img in p.descendants().filter(|n| is(*n, V, "imagedata")) {
            let name = attr(img, R, "id")
                .and_then(|id| self.rels().get(id))
                .map_or("VML picture".to_owned(), |r| {
                    file_name(&r.target).to_owned()
                });
            self.warnings.add_example(WarningKind::ImageLeftOut, name);
        }
        Ok(())
    }

    /// The image of relationship `id` as a resource of the document.
    fn image(&mut self, id: &str, cx: Option<f64>, alt: Option<String>) -> Option<Image> {
        let Some(rel) = self.rels().get(id).cloned() else {
            self.warnings.add_example(WarningKind::MissingImage, id);
            return None;
        };
        if rel.external {
            let shown: String = rel.target.chars().take(40).collect();
            self.warnings.add_example(WarningKind::MissingImage, shown);
            return None;
        }
        let file = file_name(&rel.target).to_owned();
        let name = match self.by_part.get(&rel.target) {
            Some(known) => known.clone(),
            None => {
                let name = self.resource(&rel.target, &file);
                self.by_part.insert(rel.target.clone(), name.clone());
                name
            }
        }?;
        let width = self
            .widths
            .get(self.section)
            .or(self.widths.last())
            .copied()
            .filter(|w| w.is_finite() && *w > 0.0)
            .zip(cx.filter(|c| c.is_finite() && *c > 0.0))
            .map(|(w, c)| (c / w).min(1.0));
        Some(Image {
            resource: name,
            alt,
            width,
        })
    }

    /// Adds the part `part` as a resource; its name, `None` if left out.
    fn resource(&mut self, part: &str, file: &str) -> Option<String> {
        let ext = file
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        if matches!(ext.as_str(), "emf" | "wmf" | "emz" | "wmz") {
            self.warnings
                .add_example(WarningKind::ImageLeftOut, file.to_owned());
            return None;
        }
        let Ok(Some(bytes)) = self.archive.read(part) else {
            self.warnings
                .add_example(WarningKind::MissingImage, file.to_owned());
            return None;
        };
        let (stem, dot_ext) = match file.rsplit_once('.') {
            Some((s, e)) => (s, format!(".{e}")),
            None => (file, String::new()),
        };
        let mut name = file.to_owned();
        let mut n = 2;
        while self.resources.iter().any(|r| r.name == name) {
            name = format!("{stem}-{n}{dot_ext}");
            n += 1;
        }
        self.resources.push(Resource {
            name: name.clone(),
            mime: self.types.mime(part),
            bytes,
            fallback: None,
        });
        Some(name)
    }

    /// `w:tbl` as a table.
    fn table(&mut self, tbl: Node<'d, 'd>) -> Result<Block> {
        self.enter()?;
        self.cells += 1;
        let grid = child(tbl, W, "tblGrid")
            .map(|g| g.children().filter(|c| is(*c, W, "gridCol")).count())
            .unwrap_or(0);
        let mut rows: Vec<Vec<Cell>> = Vec::new();
        let mut header_rows = 0;
        let mut leading = true;
        // Cells open for vertical merging, by their first slot.
        let mut open: HashMap<usize, (usize, usize)> = HashMap::new();
        let mut widest = 0;
        let mut trs = Vec::new();
        items(tbl, "tr", &mut trs, 0);
        for tr in trs {
            let trpr = child(tr, W, "trPr");
            if trpr.and_then(|p| child(p, W, "del")).is_some() {
                self.warnings.add(WarningKind::TrackedChange);
                continue;
            }
            let header = trpr
                .and_then(|p| child(p, W, "tblHeader"))
                .is_some_and(flag);
            let r = rows.len();
            let mut row = Vec::new();
            let mut slot = 0usize;
            let before = trpr
                .and_then(|p| child_val(p, "gridBefore"))
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(0)
                .min(1000);
            if before > 0 {
                row.push(Cell {
                    content: Vec::new(),
                    colspan: before,
                    rowspan: 1,
                });
                slot += before as usize;
            }
            let mut tcs = Vec::new();
            items(tr, "tc", &mut tcs, 0);
            for tc in tcs {
                let tcpr = child(tc, W, "tcPr");
                let span = tcpr
                    .and_then(|p| child_val(p, "gridSpan"))
                    .and_then(|v| v.parse::<u32>().ok())
                    .unwrap_or(1)
                    .clamp(1, 1000);
                let merge = tcpr
                    .and_then(|p| child(p, W, "vMerge"))
                    .map(|v| attr(v, W, "val").unwrap_or("continue"));
                if merge == Some("continue")
                    && let Some(&(rr, ci)) = open.get(&slot)
                {
                    rows[rr][ci].rowspan += 1;
                    slot += span as usize;
                    continue;
                }
                for s in slot..slot + span as usize {
                    open.remove(&s);
                }
                let mut pieces = Vec::new();
                self.block_children(tc, &mut pieces)?;
                let content = self.assemble(pieces, false);
                if merge == Some("restart") {
                    open.insert(slot, (r, row.len()));
                }
                row.push(Cell {
                    content,
                    colspan: span,
                    rowspan: 1,
                });
                slot += span as usize;
            }
            widest = widest.max(slot);
            if header && leading {
                header_rows += 1;
            } else {
                leading = false;
            }
            rows.push(row);
        }
        self.cells -= 1;
        self.depth -= 1;
        Ok(Block::Table(Table {
            columns: grid.max(widest),
            header_rows,
            rows,
        }))
    }

    /// Paragraph pieces into blocks: captions to their figures, list
    /// paragraphs into lists, the title into the metadata (at the `top`).
    fn assemble(&mut self, pieces: Vec<Piece>, top: bool) -> Vec<Block> {
        let n = pieces.len();
        let mut claimed = vec![false; n];
        let mut captions: HashMap<usize, usize> = HashMap::new();
        let image = |i: usize| matches!(pieces.get(i), Some(Piece::Image(_)));
        let table = |i: usize| matches!(pieces.get(i), Some(Piece::Block(Block::Table(_))));
        for (k, piece) in pieces.iter().enumerate() {
            if !matches!(
                piece,
                Piece::Para {
                    kind: Kind::Caption,
                    ..
                }
            ) {
                continue;
            }
            // Below an image, above a table, else the other way round.
            let candidates = [
                k.checked_sub(1).filter(|&i| image(i)),
                Some(k + 1).filter(|&i| table(i)),
                Some(k + 1).filter(|&i| image(i)),
                k.checked_sub(1).filter(|&i| table(i)),
            ];
            if let Some(t) = candidates.into_iter().flatten().find(|&t| !claimed[t]) {
                claimed[t] = true;
                captions.insert(t, k);
            }
        }
        let mut slots: Vec<Option<Piece>> = pieces.into_iter().map(Some).collect();
        let mut caption_of: HashMap<usize, Vec<Inline>> = HashMap::new();
        for (t, k) in captions {
            if let Some(Piece::Para { content, .. }) = slots[k].take() {
                caption_of.insert(t, content);
            }
        }
        let figure = |body, caption: Option<Vec<Inline>>| {
            Block::Figure(Figure {
                body,
                caption: caption.unwrap_or_default(),
                label: None,
                number: None,
            })
        };
        let mut out = Vec::new();
        let mut i = 0;
        while i < n {
            let Some(piece) = slots[i].take() else {
                i += 1;
                continue;
            };
            let caption = caption_of.remove(&i);
            i += 1;
            match piece {
                Piece::Image(img) => out.push(figure(FigureBody::Image(img), caption)),
                Piece::Block(Block::Table(t)) if caption.is_some() => {
                    out.push(figure(FigureBody::Table(t), caption));
                }
                Piece::Block(b) => out.push(b),
                Piece::Para {
                    kind:
                        Kind::Item {
                            num,
                            level,
                            ordered,
                            number,
                        },
                    content,
                } => {
                    let mut entries = vec![Entry {
                        num,
                        level,
                        ordered,
                        number,
                        content,
                    }];
                    while let Some(Some(Piece::Para {
                        kind: Kind::Item { .. },
                        ..
                    })) = slots.get(i)
                    {
                        if let Some(Piece::Para {
                            kind:
                                Kind::Item {
                                    num,
                                    level,
                                    ordered,
                                    number,
                                },
                            content,
                        }) = slots[i].take()
                        {
                            entries.push(Entry {
                                num,
                                level,
                                ordered,
                                number,
                                content,
                            });
                        }
                        i += 1;
                    }
                    out.extend(lists(&mut entries));
                }
                Piece::Para {
                    kind: Kind::Heading(level),
                    content,
                } => out.push(Block::Heading {
                    level,
                    content: unwrap_strong(content),
                    label: None,
                }),
                Piece::Para {
                    kind: Kind::Title,
                    content,
                } if top && self.meta.title.is_none() => {
                    self.meta.title = Some(plain(&content).trim().to_owned());
                }
                Piece::Para {
                    kind: Kind::Subtitle,
                    content,
                } if top && self.meta.subtitle.is_none() => {
                    self.meta.subtitle = Some(plain(&content).trim().to_owned());
                }
                Piece::Para { content, .. } => out.push(Block::Para(content)),
            }
        }
        out
    }

    /// Warns of headers and footers with text (they are dropped).
    fn headers_footers(&mut self, sections: &[Node], rels: &Rels) -> Result<()> {
        let mut seen = HashSet::new();
        for r in sections.iter().flat_map(|s| s.children()) {
            if !(is(r, W, "headerReference") || is(r, W, "footerReference")) {
                continue;
            }
            let Some(rel) = attr(r, R, "id").and_then(|id| rels.get(id)) else {
                continue;
            };
            if rel.external || !seen.insert(rel.target.clone()) {
                continue;
            }
            let Some(text) = xml(self.archive, &rel.target)? else {
                continue;
            };
            let doc = parse(&text, &rel.target)?;
            let has_text = doc
                .descendants()
                .any(|t| is(t, W, "t") && t.text().is_some_and(|s| !s.trim().is_empty()));
            if has_text {
                self.warnings
                    .add_example(WarningKind::HeaderFooter, file_name(&rel.target));
            }
        }
        Ok(())
    }
}

/// The elements `name` in `n`, also inside content controls, custom XML
/// and tracked insertions (rows, cells); deleted ones skipped.
fn items<'d>(n: Node<'d, 'd>, name: &str, out: &mut Vec<Node<'d, 'd>>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    for c in n.children().filter(|c| in_ns(*c, W)) {
        match c.tag_name().name() {
            x if x == name => out.push(c),
            "sdt" | "sdtContent" | "customXml" | "ins" | "moveTo" => items(c, name, out, depth + 1),
            _ => {}
        }
    }
}

fn file_name(part: &str) -> &str {
    part.rsplit('/').next().unwrap_or(part)
}

/// Adds a paragraph's segment as a piece of `kind` (the first non-empty
/// segment takes it; later ones are plain).
fn push_para(kind: &mut Option<Kind>, seg: Vec<(Fmt, Inline)>, out: &mut Vec<Piece>) {
    let content = trim(build(seg));
    if content.is_empty() {
        return;
    }
    let kind = kind.take().unwrap_or(Kind::Normal);
    match (&kind, &content[..]) {
        (Kind::Normal, [Inline::Image(img)]) => out.push(Piece::Image(img.clone())),
        _ => out.push(Piece::Para { kind, content }),
    }
}

/// Drops what is left of a caption's label after its number (`: `, `. `,
/// ` – `).
fn strip_label(atoms: &mut Vec<Atom>) {
    let sep = |c: char| c.is_whitespace() || matches!(c, ':' | '.' | '-' | '–' | '—' | '|');
    while let Some(Atom::Inline(_, Inline::Text(t))) = atoms.first_mut() {
        let rest = t.trim_start_matches(sep);
        if rest.is_empty() {
            atoms.remove(0);
        } else {
            *t = rest.to_owned();
            break;
        }
    }
}

/// A heading wholly bold is just the heading.
fn unwrap_strong(content: Vec<Inline>) -> Vec<Inline> {
    match <[Inline; 1]>::try_from(content) {
        Ok([Inline::Strong(inner)]) => inner,
        Ok(one) => one.into(),
        Err(content) => content,
    }
}

/// Lists from consecutive list paragraphs.
fn lists(entries: &mut [Entry]) -> Vec<Block> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < entries.len() {
        let level = entries[i].level;
        let (list, next) = list(entries, i, level);
        out.push(Block::List(list));
        i = next.max(i + 1);
    }
    out
}

/// The list starting at `start` on `level`, and where it ended: deeper
/// paragraphs nest in the item before them; a shallower one, or another
/// numbering or kind on this level, ends it.
fn list(entries: &mut [Entry], start: usize, level: u8) -> (List, usize) {
    let (num, ordered) = (entries[start].num.clone(), entries[start].ordered);
    let mut list = List {
        ordered,
        start: entries[start].number,
        items: Vec::new(),
    };
    let mut i = start;
    while i < entries.len() {
        let e = &mut entries[i];
        if e.level < level {
            break;
        }
        if e.level == level {
            if !list.items.is_empty() && (e.num != num || e.ordered != ordered) {
                break;
            }
            list.items
                .push(vec![Block::Para(std::mem::take(&mut e.content))]);
            i += 1;
        } else {
            let deeper = e.level;
            let (sub, next) = self::list(entries, i, deeper);
            match list.items.last_mut() {
                Some(item) => item.push(Block::List(sub)),
                None => list.items.push(vec![Block::List(sub)]),
            }
            i = next;
        }
    }
    (list, i)
}

/// Splits leading and trailing whitespace off the ends of `inner`.
fn hoist(mut inner: Vec<Inline>) -> (String, Vec<Inline>, String) {
    let mut lead = String::new();
    let mut trail = String::new();
    if let Some(Inline::Text(t)) = inner.first_mut() {
        let rest = t.trim_start();
        lead = t[..t.len() - rest.len()].to_owned();
        *t = rest.to_owned();
        if t.is_empty() {
            inner.remove(0);
        }
    }
    if let Some(Inline::Text(t)) = inner.last_mut() {
        let rest = t.trim_end();
        trail = t[rest.len()..].to_owned();
        t.truncate(rest.len());
        if t.is_empty() {
            inner.pop();
        }
    }
    (lead, inner, trail)
}

/// Formatted atoms as nested inlines: equal formatting merged, spans
/// outermost in the order bold, italic, underline, strike, super, sub,
/// whitespace at their ends moved out.
fn build(atoms: Vec<(Fmt, Inline)>) -> Vec<Inline> {
    merge(group(atoms, 0))
}

fn group(mut atoms: Vec<(Fmt, Inline)>, flag: usize) -> Vec<Inline> {
    if flag == 6 {
        return atoms.into_iter().map(|(_, i)| i).collect();
    }
    // A space between two spans of a kind joins them.
    for i in 1..atoms.len().saturating_sub(1) {
        let space = matches!(&atoms[i].1, Inline::Text(t) if t.trim().is_empty());
        if space && atoms[i - 1].0.0[flag] && atoms[i + 1].0.0[flag] {
            atoms[i].0.0[flag] = true;
        }
    }
    let mut out = Vec::new();
    let mut run: Vec<(Fmt, Inline)> = Vec::new();
    let mut set = false;
    let emit = |run: Vec<(Fmt, Inline)>, set: bool, out: &mut Vec<Inline>| {
        let inner = merge(group(run, flag + 1));
        if !set {
            out.extend(inner);
            return;
        }
        let (lead, inner, trail) = hoist(inner);
        if !lead.is_empty() {
            out.push(Inline::Text(lead));
        }
        if !inner.is_empty() {
            out.push(match flag {
                0 => Inline::Strong(inner),
                1 => Inline::Emph(inner),
                2 => Inline::Underline(inner),
                3 => Inline::Strike(inner),
                4 => Inline::Super(inner),
                _ => Inline::Sub(inner),
            });
        }
        if !trail.is_empty() {
            out.push(Inline::Text(trail));
        }
    };
    for (f, i) in atoms {
        let s = f.0[flag];
        if s != set && !run.is_empty() {
            emit(std::mem::take(&mut run), set, &mut out);
        }
        set = s;
        run.push((f, i));
    }
    if !run.is_empty() {
        emit(run, set, &mut out);
    }
    out
}

/// Adjacent text joined.
fn merge(items: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::with_capacity(items.len());
    for item in items {
        match (out.last_mut(), item) {
            (Some(Inline::Text(prev)), Inline::Text(t)) => prev.push_str(&t),
            (_, item) => out.push(item),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zip::ZipWriter;

    const DOC: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math"><w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Intro</w:t></w:r></w:p>
<w:p><w:r><w:rPr><w:b/></w:rPr><w:t xml:space="preserve">Bold </w:t></w:r><w:r><w:t>and </w:t></w:r><w:hyperlink r:id="rId9"><w:r><w:t>a link</w:t></w:r></w:hyperlink><w:r><w:footnoteReference w:id="1"/></w:r><m:oMath><m:r><m:t>x</m:t></m:r></m:oMath></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Item</w:t></w:r></w:p>
<w:tbl><w:tblGrid><w:gridCol/><w:gridCol/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:left="1440" w:right="1440"/></w:sectPr>
</w:body></w:document>"#;

    const STYLES: &str = r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/></w:style></w:styles>"#;
    const NUMBERING: &str = r#"<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#;
    const NOTES: &str = r#"<w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:footnote w:type="separator" w:id="-1"><w:p/></w:footnote><w:footnote w:id="1"><w:p><w:r><w:t>A note.</w:t></w:r></w:p></w:footnote></w:footnotes>"#;
    const RELS: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId9" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/" TargetMode="External"/></Relationships>"#;

    fn docx(document: &str) -> Vec<u8> {
        let mut w = ZipWriter::new();
        for (name, data) in [
            ("word/document.xml", document),
            ("word/styles.xml", STYLES),
            ("word/numbering.xml", NUMBERING),
            ("word/footnotes.xml", NOTES),
            ("word/_rels/document.xml.rels", RELS),
        ] {
            w.add(name, data.as_bytes(), true).unwrap();
        }
        w.finish().unwrap()
    }

    fn read(bytes: &[u8]) -> Result<Imported> {
        Docx.read(bytes, &ReadOptions::new("a.docx"))
    }

    #[test]
    fn reads_the_basics() {
        let doc = read(&docx(DOC)).unwrap().doc;
        assert!(matches!(&doc.blocks[0], Block::Heading { level: 1, .. }));
        let Block::Para(p) = &doc.blocks[1] else {
            panic!("{:?}", doc.blocks[1]);
        };
        assert!(matches!(&p[0], Inline::Strong(_)));
        assert!(matches!(&p[1], Inline::Text(t) if t == " and "));
        assert!(matches!(&p[2], Inline::Link { url, .. } if url == "https://example.com/"));
        assert!(matches!(&p[3], Inline::Footnote(_)));
        assert!(matches!(&p[4], Inline::Math(m) if m.typst == "x"));
        assert!(matches!(&doc.blocks[2], Block::List(l) if l.ordered && l.items.len() == 1));
        assert!(matches!(&doc.blocks[3], Block::Table(t) if t.columns == 2));
        assert!(sniff(&docx(DOC)));
        assert!(!sniff(b"PK\x03\x04 nothing"));
    }

    #[test]
    fn field_words() {
        assert_eq!(
            words(r#" HYPERLINK "https://a.b/c d" \o "tip" "#),
            ["HYPERLINK", "https://a.b/c d", "\\o", "tip"]
        );
        assert_eq!(
            classify(r#"HYPERLINK "https://a.b" \l "x""#),
            FieldKind::Link("https://a.b#x".into())
        );
        assert_eq!(
            classify(r#"HYPERLINK \l "_Toc1""#),
            FieldKind::Link(String::new())
        );
        assert_eq!(
            classify(r#" ADDIN ZOTERO_ITEM CSL_CITATION {"a":1} "#),
            FieldKind::Citation("ZOTERO_ITEM".into())
        );
        assert_eq!(classify(" SEQ Figure \\* ARABIC "), FieldKind::Seq);
        assert_eq!(resolve("word", "../media/a%20b.png"), "media/a b.png");
        assert_eq!(resolve("word", "/word/media/x.png"), "word/media/x.png");
    }

    /// A small deterministic random sequence.
    fn lcg(seed: &mut u64) -> u64 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *seed >> 33
    }

    #[test]
    fn broken_input_never_panics() {
        let good = docx(DOC);
        for len in (0..good.len()).step_by(3) {
            let _ = read(&good[..len]);
        }
        let mut seed = 7;
        for _ in 0..300 {
            let mut bad = good.clone();
            for _ in 0..1 + lcg(&mut seed) % 4 {
                let at = lcg(&mut seed) as usize % bad.len();
                bad[at] = lcg(&mut seed) as u8;
            }
            let _ = read(&bad);
        }
        // Mutated XML, zipped intact: cuts and swapped characters.
        let bytes = DOC.as_bytes();
        for _ in 0..300 {
            let mut xml = bytes.to_vec();
            let at = lcg(&mut seed) as usize % xml.len();
            match lcg(&mut seed) % 3 {
                0 => xml.truncate(at),
                1 => xml[at] = b"<>/\"=&w:"[lcg(&mut seed) as usize % 8],
                _ => {
                    let to = (at + lcg(&mut seed) as usize % 40).min(xml.len());
                    xml.drain(at..to);
                }
            }
            let _ = read(&docx(&String::from_utf8_lossy(&xml)));
        }
    }

    #[test]
    fn deep_nesting_fails_cleanly() {
        let deep =
            "<w:sdt><w:sdtContent>".repeat(200) + "<w:p/>" + &"</w:sdtContent></w:sdt>".repeat(200);
        let doc = DOC.replace("<w:sectPr>", &format!("{deep}<w:sectPr>"));
        assert!(matches!(read(&docx(&doc)), Err(Error::TooDeep)));
        // Shallow enough for the parser, too deep for the reader.
        let deep = "<w:customXml>".repeat(80) + "<w:p/>" + &"</w:customXml>".repeat(80);
        let doc = DOC.replace("<w:sectPr>", &format!("{deep}<w:sectPr>"));
        assert!(matches!(read(&docx(&doc)), Err(Error::TooDeep)));
        assert!(!too_deep(DOC) && too_deep(&"<a>".repeat(121)));
    }
}
