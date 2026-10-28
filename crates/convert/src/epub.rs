//! EPUB 3 (3.3), from the model alone: a zip with `mimetype` first and
//! stored, `META-INF/container.xml`, and under `OEBPS/` the package
//! document (`content.opf`), the navigation document (`nav.xhtml`: the
//! table of contents from headings of levels 1–3, and landmarks), an NCX
//! for older readers (`toc.ncx`), a stylesheet, a title page from the
//! metadata (if there is a title), one XHTML file per chapter and the
//! images under `images/`.
//!
//! Chapters start at level-1 headings (what comes before the first is a
//! file of its own); the content documents are the polyglot markup HTML
//! shares (`xhtml.rs`), with headings from `<h1>`. References link across
//! files (`chapter-3.xhtml#label`); footnotes are pop-up notes
//! (`epub:type="noteref"` and an `<aside epub:type="footnote">` at the end
//! of the chapter, numbered through the book). Images of the types every
//! reading system shows (PNG, JPEG, GIF, SVG, WebP) become files, others
//! are left out with a warning; a figure's image without alt text is
//! described by its caption. The bibliography (a bulleted list after a
//! heading at the end of a document that cites) shows without bullets.
//!
//! The identifier is a `urn:uuid:` hashed from the title and authors, so
//! the same book keeps it from export to export. [`check`] tests the
//! structure (epubcheck needs Java).

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::{Block, Doc, Resource};
use crate::xhtml::{Markup, Target, attr, bibliography, has_scheme, labels, plain, text};
use crate::zip::{Archive, ZipWriter};
use crate::{Error, Result, WriteOptions, Writer, Written};

/// The EPUB writer.
pub struct Epub;

const MIMETYPE: &str = "application/epub+zip";
const XHTML: &str = "application/xhtml+xml";
const NCX: &str = "application/x-dtbncx+xml";
const XHTML_NS: &str = "http://www.w3.org/1999/xhtml";
const MATHML_NS: &str = "http://www.w3.org/1998/Math/MathML";

const CONTAINER: &str = "\
<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">
<rootfiles>
<rootfile full-path=\"OEBPS/content.opf\" media-type=\"application/oebps-package+xml\" />
</rootfiles>
</container>
";

/// Small and reader-friendly: no colours, sizes in `em`, the reading
/// system's fonts where the reader picks them.
const STYLE: &str = "\
body { font-family: Georgia, \"Times New Roman\", serif; line-height: 1.5; }
h1, h2, h3, h4, h5, h6 { line-height: 1.25; text-align: left; page-break-after: avoid; break-after: avoid; }
.titlepage { text-align: center; margin-top: 3em; }
.titlepage h1 { font-size: 2em; text-align: center; }
.titlepage p { margin: 0.5em 0; }
.titlepage .subtitle { font-size: 1.25em; font-style: italic; }
figure { margin: 1.5em 0; text-align: center; page-break-inside: avoid; break-inside: avoid; }
img { max-width: 100%; height: auto; }
figcaption { font-size: 0.9em; margin: 0.5em 0; }
table { border-collapse: collapse; margin: 1em auto; }
th, td { border: 1px solid; padding: 0.25em 0.5em; text-align: left; vertical-align: top; }
blockquote { margin: 1em 1.5em; }
pre { white-space: pre-wrap; margin: 1em 0; }
code { font-family: monospace; font-size: 0.9em; }
.equation { display: flex; align-items: center; justify-content: center; gap: 2em; margin: 1em 0; }
a[role=\"doc-noteref\"] { text-decoration: none; }
aside.footnote { font-size: 0.9em; }
ul.bibliography { list-style: none; padding-left: 0; }
ul.bibliography li { margin: 0.5em 0; padding-left: 1.5em; text-indent: -1.5em; }
nav ol { list-style: none; }
mtable.aligned mtd:nth-child(odd) { text-align: right; padding-right: 0; }
mtable.aligned mtd:nth-child(even) { text-align: left; padding-left: 0; }
";

/// An entry of the table of contents.
struct Entry {
    level: u8,
    text: String,
    href: String,
}

impl Writer for Epub {
    fn write(&self, doc: &Doc, options: &WriteOptions) -> Result<Written> {
        let m = &doc.meta;
        let lang = some(&m.lang).unwrap_or("en");
        let title = some(&m.title);
        let book = title.unwrap_or("Untitled");

        // Chapters at level-1 headings; what comes before the first is one
        // too. A book needs one file to read.
        let mut parts: Vec<&[Block]> = Vec::new();
        let mut start = 0;
        for (i, b) in doc.blocks.iter().enumerate() {
            if i > start && matches!(b, Block::Heading { level: 1, .. }) {
                parts.push(&doc.blocks[start..i]);
                start = i;
            }
        }
        if start < doc.blocks.len() || (parts.is_empty() && title.is_none()) {
            parts.push(&doc.blocks[start..]);
        }
        let files: Vec<String> = (1..=parts.len())
            .map(|n| format!("chapter-{n}.xhtml"))
            .collect();
        // Where each label is, so references link across files.
        let mut targets = HashMap::new();
        for (part, file) in parts.iter().zip(&files) {
            let mut found = Vec::new();
            labels(part, &mut found);
            for label in found {
                targets.entry(label).or_insert_with(|| file.clone());
            }
        }
        let mut w = Markup::new(doc, Target::Epub, targets);
        w.bibliography = bibliography(doc);
        w.outline = Some(Vec::new());
        let mut images: Vec<(String, &Resource)> = Vec::new();
        for r in &doc.resources {
            let Some(ext) = image_ext(&r.mime) else {
                continue;
            };
            let path = image_path(&r.name, ext, |p| images.iter().any(|(q, _)| q == p));
            w.images.insert(&r.name, path.clone());
            images.push((path, r));
        }

        let mut toc = Vec::new();
        let mut chapters = Vec::new();
        for (part, file) in parts.iter().zip(&files) {
            w.file = file.clone();
            let mut body = w.blocks(part)?;
            body.push_str(&w.notes()?);
            for h in w.outline.as_mut().map(std::mem::take).unwrap_or_default() {
                toc.push(Entry {
                    level: h.level,
                    text: h.text,
                    href: format!("{file}#{}", h.id),
                });
            }
            let head = match part.first() {
                Some(Block::Heading {
                    level: 1, content, ..
                }) => plain(content),
                _ => String::new(),
            };
            let head = if head.is_empty() { book } else { &head };
            chapters.push((file.as_str(), page(lang, head, &body)));
        }
        let titlepage = title.map(|t| {
            let mut body = String::from("<section epub:type=\"titlepage\" class=\"titlepage\">\n");
            body.push_str(&format!("<h1 class=\"title\">{}</h1>\n", text(t)));
            if let Some(s) = some(&m.subtitle) {
                body.push_str(&format!("<p class=\"subtitle\">{}</p>\n", text(s)));
            }
            for a in &m.authors {
                body.push_str(&format!("<p class=\"author\">{}</p>\n", text(a)));
            }
            if let Some(d) = some(&m.date) {
                body.push_str(&format!("<p class=\"date\">{}</p>\n", text(d)));
            }
            body.push_str("</section>\n");
            page(lang, t, &body)
        });

        let first = match titlepage {
            Some(_) => "titlepage.xhtml",
            None => files[0].as_str(),
        };
        if toc.is_empty() {
            toc.push(Entry {
                level: 1,
                text: book.to_owned(),
                href: first.to_owned(),
            });
        }
        for e in &mut toc {
            if e.text.is_empty() {
                e.text = "Untitled".into();
            }
        }
        let mut landmarks = vec![("toc", "nav.xhtml#toc", "Contents")];
        if titlepage.is_some() {
            landmarks.push(("titlepage", "titlepage.xhtml", "Title page"));
        }
        if let Some(f) = files.first() {
            landmarks.push(("bodymatter", f, "Start"));
        }
        let nav = nav(lang, &toc, &landmarks);
        let id = identifier(title, &m.authors);
        let ncx = ncx(&id, book, lang, &toc);
        images.retain(|(_, r)| w.shown.contains(r.name.as_str()));

        // The package document.
        let mut manifest = String::new();
        let mut spine = String::new();
        let mut item = |id: &str, href: &str, media: &str, props: &str| {
            let props = match props.is_empty() {
                true => String::new(),
                false => format!(" properties=\"{props}\""),
            };
            manifest.push_str(&format!(
                "<item id=\"{id}\" href=\"{}\" media-type=\"{media}\"{props} />\n",
                attr(href)
            ));
        };
        item("nav", "nav.xhtml", XHTML, "nav");
        item("ncx", "toc.ncx", NCX, "");
        item("style", "style.css", "text/css", "");
        if let Some(t) = &titlepage {
            item("titlepage", "titlepage.xhtml", XHTML, &properties(t));
            spine.push_str("<itemref idref=\"titlepage\" />\n");
        }
        spine.push_str("<itemref idref=\"nav\" linear=\"no\" />\n");
        let mut math = false;
        for (n, (file, xhtml)) in chapters.iter().enumerate() {
            let props = properties(xhtml);
            math |= props.contains("mathml");
            item(&format!("chapter-{}", n + 1), file, XHTML, &props);
            spine.push_str(&format!("<itemref idref=\"chapter-{}\" />\n", n + 1));
        }
        for (n, (path, r)) in images.iter().enumerate() {
            item(&format!("image-{}", n + 1), path, &r.mime, "");
        }
        let modified = match options.modified.is_empty() {
            true => utc(SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs())),
            false => options.modified.clone(),
        };
        let mut meta = format!("<dc:identifier id=\"pub-id\">{id}</dc:identifier>\n");
        meta.push_str(&format!(
            "<dc:title id=\"title\">{}</dc:title>\n",
            text(book)
        ));
        if let Some(s) = some(&m.subtitle) {
            meta.push_str("<meta refines=\"#title\" property=\"title-type\">main</meta>\n");
            meta.push_str(&format!(
                "<dc:title id=\"subtitle\">{}</dc:title>\n",
                text(s)
            ));
            meta.push_str("<meta refines=\"#subtitle\" property=\"title-type\">subtitle</meta>\n");
        }
        for (n, a) in m.authors.iter().enumerate() {
            let n = n + 1;
            meta.push_str(&format!(
                "<dc:creator id=\"creator-{n}\">{}</dc:creator>\n",
                text(a)
            ));
            meta.push_str(&format!(
                "<meta refines=\"#creator-{n}\" property=\"role\" scheme=\"marc:relators\">aut</meta>\n"
            ));
        }
        meta.push_str(&format!("<dc:language>{}</dc:language>\n", text(lang)));
        if let Some(date) = some(&m.date).and_then(w3c_date) {
            meta.push_str(&format!("<dc:date>{}</dc:date>\n", text(&date)));
        }
        meta.push_str(&format!(
            "<meta property=\"dcterms:modified\">{}</meta>\n",
            text(&modified)
        ));
        // Accessibility: what the book needs to be read and what helps.
        let described = w.without_alt == 0;
        let mut a11y = vec![("accessMode", "textual")];
        if !images.is_empty() {
            a11y.push(("accessMode", "visual"));
        }
        if described {
            a11y.push(("accessModeSufficient", "textual"));
        }
        a11y.push(("accessibilityFeature", "structuralNavigation"));
        a11y.push(("accessibilityFeature", "tableOfContents"));
        if math {
            a11y.push(("accessibilityFeature", "MathML"));
        }
        if !images.is_empty() && described {
            a11y.push(("accessibilityFeature", "alternativeText"));
        }
        a11y.push(("accessibilityHazard", "none"));
        for (property, value) in a11y {
            meta.push_str(&format!(
                "<meta property=\"schema:{property}\">{value}</meta>\n"
            ));
        }
        let mut summary = String::from("Reflowable text with headings and a table of contents.");
        if math {
            summary.push_str(" Formulas are MathML.");
        }
        match (images.is_empty(), described) {
            (true, _) => {}
            (false, true) => summary.push_str(" Images have a text alternative."),
            (false, false) => summary.push_str(" Some images have no text alternative."),
        }
        meta.push_str(&format!(
            "<meta property=\"schema:accessibilitySummary\">{summary}</meta>\n"
        ));
        let opf = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"pub-id\" xml:lang=\"{}\">\n\
<metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n{meta}</metadata>\n\
<manifest>\n{manifest}</manifest>\n\
<spine toc=\"ncx\">\n{spine}</spine>\n\
</package>\n",
            attr(lang)
        );

        let mut zip = ZipWriter::new();
        zip.add("mimetype", MIMETYPE.as_bytes(), false)?;
        zip.add("META-INF/container.xml", CONTAINER.as_bytes(), true)?;
        zip.add("OEBPS/content.opf", opf.as_bytes(), true)?;
        zip.add("OEBPS/nav.xhtml", nav.as_bytes(), true)?;
        zip.add("OEBPS/toc.ncx", ncx.as_bytes(), true)?;
        zip.add("OEBPS/style.css", STYLE.as_bytes(), true)?;
        if let Some(t) = &titlepage {
            zip.add("OEBPS/titlepage.xhtml", t.as_bytes(), true)?;
        }
        for (file, xhtml) in &chapters {
            zip.add(&format!("OEBPS/{file}"), xhtml.as_bytes(), true)?;
        }
        for (path, r) in &images {
            zip.add(&format!("OEBPS/{path}"), &r.bytes, true)?;
        }
        Ok(Written {
            bytes: zip.finish()?,
            files: Vec::new(),
            warnings: w.warnings.into_vec(),
        })
    }
}

/// A metadata field trimmed, `None` if empty.
fn some(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// An XHTML content document.
fn page(lang: &str, title: &str, body: &str) -> String {
    let lang = attr(lang);
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE html>\n\
<html xmlns=\"{XHTML_NS}\" xmlns:epub=\"http://www.idpf.org/2007/ops\" lang=\"{lang}\" xml:lang=\"{lang}\">\n\
<head>\n<meta charset=\"utf-8\" />\n<title>{}</title>\n\
<link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\" />\n</head>\n\
<body>\n{body}</body>\n</html>\n",
        text(title)
    )
}

/// A content document's manifest properties: `mathml`, `svg` (inline).
fn properties(xhtml: &str) -> String {
    let mut props = Vec::new();
    if xhtml.contains("<math") {
        props.push("mathml");
    }
    if xhtml.contains("<svg") {
        props.push("svg");
    }
    props.join(" ")
}

/// Each entry with the deeper entries after it (its children).
fn nested(entries: &[Entry]) -> Vec<(&Entry, &[Entry])> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < entries.len() {
        let level = entries[i].level;
        let end = entries[i + 1..]
            .iter()
            .position(|e| e.level <= level)
            .map_or(entries.len(), |p| i + 1 + p);
        out.push((&entries[i], &entries[i + 1..end]));
        i = end;
    }
    out
}

/// The navigation document: the table of contents and the landmarks.
fn nav(lang: &str, toc: &[Entry], landmarks: &[(&str, &str, &str)]) -> String {
    fn list(toc: &[Entry], out: &mut String) {
        out.push_str("<ol>\n");
        for (e, sub) in nested(toc) {
            out.push_str(&format!(
                "<li><a href=\"{}\">{}</a>",
                attr(&e.href),
                text(&e.text)
            ));
            if !sub.is_empty() {
                out.push('\n');
                list(sub, out);
            }
            out.push_str("</li>\n");
        }
        out.push_str("</ol>\n");
    }
    let mut body =
        String::from("<nav epub:type=\"toc\" role=\"doc-toc\" id=\"toc\">\n<h1>Contents</h1>\n");
    list(toc, &mut body);
    body.push_str(
        "</nav>\n<nav epub:type=\"landmarks\" hidden=\"hidden\">\n<h2>Landmarks</h2>\n<ol>\n",
    );
    for (kind, href, label) in landmarks {
        body.push_str(&format!(
            "<li><a epub:type=\"{kind}\" href=\"{}\">{label}</a></li>\n",
            attr(href)
        ));
    }
    body.push_str("</ol>\n</nav>\n");
    page(lang, "Contents", &body)
}

/// The NCX: the table of contents for EPUB 2 reading systems.
fn ncx(id: &str, title: &str, lang: &str, toc: &[Entry]) -> String {
    fn points(toc: &[Entry], order: &mut usize, out: &mut String) {
        for (e, sub) in nested(toc) {
            *order += 1;
            out.push_str(&format!(
                "<navPoint id=\"np-{o}\" playOrder=\"{o}\"><navLabel><text>{}</text></navLabel><content src=\"{}\" />\n",
                text(&e.text),
                attr(&e.href),
                o = *order
            ));
            points(sub, order, out);
            out.push_str("</navPoint>\n");
        }
    }
    fn depth(toc: &[Entry]) -> usize {
        nested(toc)
            .iter()
            .map(|(_, sub)| 1 + depth(sub))
            .max()
            .unwrap_or(0)
    }
    let mut map = String::new();
    points(toc, &mut 0, &mut map);
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\" xml:lang=\"{}\">\n\
<head>\n<meta name=\"dtb:uid\" content=\"{id}\" />\n<meta name=\"dtb:depth\" content=\"{}\" />\n\
<meta name=\"dtb:totalPageCount\" content=\"0\" />\n<meta name=\"dtb:maxPageNumber\" content=\"0\" />\n</head>\n\
<docTitle><text>{}</text></docTitle>\n<navMap>\n{map}</navMap>\n</ncx>\n",
        attr(lang),
        depth(toc).max(1),
        text(title)
    )
}

/// The extension for an image of a core media type, `None` for others.
fn image_ext(mime: &str) -> Option<&'static str> {
    Some(match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        "image/webp" => "webp",
        _ => return None,
    })
}

/// `images/<name>.<ext>` for the resource `name` (its file name, safe
/// characters only), numbered on if `taken`.
fn image_path(name: &str, ext: &str, taken: impl Fn(&str) -> bool) -> String {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    let mut stem: String = stem
        .chars()
        .map(
            |c| match c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                true => c,
                false => '_',
            },
        )
        .collect();
    if stem.is_empty() {
        stem = "image".into();
    }
    let mut n = 1;
    loop {
        let path = match n {
            1 => format!("images/{stem}.{ext}"),
            n => format!("images/{stem}-{n}.{ext}"),
        };
        if !taken(&path) {
            return path;
        }
        n += 1;
    }
}

/// A `urn:uuid:` (version 8, RFC 9562) from the title and authors: the
/// same book, the same identifier.
fn identifier(title: Option<&str>, authors: &[String]) -> String {
    let mut key = title.unwrap_or("").to_owned();
    for a in authors {
        key.push('\0');
        key.push_str(a.trim());
    }
    // FNV-1a, twice with different offsets.
    let fnv = |seed: u64| {
        key.bytes().fold(seed, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        })
    };
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&fnv(0xcbf2_9ce4_8422_2325).to_be_bytes());
    b[8..].copy_from_slice(&fnv(0x6c62_272e_07bb_0142).to_be_bytes());
    b[6] = b[6] & 0x0f | 0x80;
    b[8] = b[8] & 0x3f | 0x80;
    let hex: String = b.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "urn:uuid:{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// `date` for `dc:date` (W3CDTF): as written if it is one (`2026`,
/// `2026-10`, `2026-10-26`, with a time), else the one year it names
/// (`26 October 2026`), else none.
fn w3c_date(date: &str) -> Option<String> {
    let b = date.as_bytes();
    let digits = |at: usize, n: usize| {
        b.get(at..at + n)
            .is_some_and(|s| s.iter().all(u8::is_ascii_digit))
    };
    let dash = |at: usize| b.get(at) == Some(&b'-');
    let iso = digits(0, 4)
        && match b.len() {
            4 => true,
            7 => dash(4) && digits(5, 2),
            n => {
                n >= 10
                    && dash(4)
                    && digits(5, 2)
                    && dash(7)
                    && digits(8, 2)
                    && (n == 10 || b[10] == b'T')
            }
        };
    if iso {
        return Some(date.to_owned());
    }
    let years: Vec<&str> = date
        .split(|c: char| !c.is_ascii_digit())
        .filter(|w| w.len() == 4)
        .collect();
    match years.as_slice() {
        [year] => Some((*year).to_owned()),
        _ => None,
    }
}

/// `secs` since 1970 as UTC (`2026-10-26T14:03:00Z`; H. Hinnant's
/// calendar algorithm).
pub(crate) fn utc(secs: u64) -> String {
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let s = secs % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        s / 3600,
        s / 60 % 60,
        s % 60
    )
}

/// What is wrong with an EPUB's structure, empty if nothing: what
/// epubcheck would fail first (it needs Java), for the tests. `mimetype`
/// first, stored, without an extra field; the package, every content
/// document and the NCX well-formed XML; the manifest and the zip agree;
/// spine items are content documents; ids unique per file; every link and
/// image without a scheme reaches a file and id that exist; `mathml`
/// exactly on the files with MathML.
#[doc(hidden)]
pub fn check(bytes: &[u8]) -> Vec<String> {
    let mut problems = Vec::new();
    if let Err(e) = check_into(bytes, &mut problems) {
        problems.push(e.to_string());
    }
    problems
}

fn xml<'t>(name: &str, text: &'t str) -> Result<roxmltree::Document<'t>> {
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..roxmltree::ParsingOptions::default()
    };
    roxmltree::Document::parse_with_options(text, options)
        .map_err(|e| Error::Invalid(format!("{name}: {e}")))
}

fn check_into(bytes: &[u8], problems: &mut Vec<String>) -> Result<()> {
    let zip = Archive::new(bytes)?;
    let names: Vec<&str> = zip.names().collect();
    // The local header: method at 8, extra field length at 28, the name
    // at 30, then the data.
    let at = |start: usize, len: usize| bytes.get(start..start + len);
    if names.first() != Some(&"mimetype")
        || at(8, 2) != Some(&[0, 0][..])
        || at(28, 2) != Some(&[0, 0][..])
        || at(30, 8) != Some(&b"mimetype"[..])
        || at(38, MIMETYPE.len()) != Some(MIMETYPE.as_bytes())
        || zip.read("mimetype")?.as_deref() != Some(MIMETYPE.as_bytes())
    {
        problems.push("mimetype: not first, stored, bare and application/epub+zip".into());
    }
    let read = |name: &str| -> Result<String> {
        let bytes = zip
            .read(name)?
            .ok_or_else(|| Error::Invalid(format!("{name} is missing")))?;
        String::from_utf8(bytes).map_err(|_| Error::Invalid(format!("{name} is not UTF-8")))
    };
    let container = read("META-INF/container.xml")?;
    let opf_path = xml("META-INF/container.xml", &container)?
        .descendants()
        .find(|n| n.has_tag_name("rootfile"))
        .and_then(|n| n.attribute("full-path"))
        .ok_or_else(|| Error::Invalid("container.xml names no package".into()))?
        .to_owned();
    let dir = opf_path
        .rsplit_once('/')
        .map_or(String::new(), |(d, _)| format!("{d}/"));
    let opf_text = read(&opf_path)?;
    let opf = xml(&opf_path, &opf_text)?;

    // The manifest: id → (path, media type, properties).
    let mut items: HashMap<&str, (String, &str, &str)> = HashMap::new();
    for item in opf.descendants().filter(|n| n.has_tag_name("item")) {
        let (Some(id), Some(href), Some(media)) = (
            item.attribute("id"),
            item.attribute("href"),
            item.attribute("media-type"),
        ) else {
            problems.push("an item lacks an id, href or media type".into());
            continue;
        };
        let path = format!("{dir}{href}");
        if !names.contains(&path.as_str()) {
            problems.push(format!("{path}: in the manifest, not in the zip"));
        }
        let props = item.attribute("properties").unwrap_or("");
        if items.insert(id, (path, media, props)).is_some() {
            problems.push(format!("manifest id {id} twice"));
        }
    }
    let listed: HashSet<&str> = items.values().map(|(p, _, _)| p.as_str()).collect();
    for name in &names {
        if !matches!(*name, "mimetype" | "META-INF/container.xml")
            && *name != opf_path
            && !listed.contains(name)
        {
            problems.push(format!("{name}: in the zip, not in the manifest"));
        }
    }
    let spine = opf.descendants().find(|n| n.has_tag_name("spine"));
    if !matches!(
        spine
            .and_then(|s| s.attribute("toc"))
            .and_then(|t| items.get(t)),
        Some((_, NCX, _))
    ) {
        problems.push("the spine's toc is not the NCX".into());
    }
    for r in opf.descendants().filter(|n| n.has_tag_name("itemref")) {
        let idref = r.attribute("idref").unwrap_or("");
        if !matches!(items.get(idref), Some((_, XHTML, _))) {
            problems.push(format!("spine item {idref}: not a content document"));
        }
    }
    let has = |props: &str, p: &str| props.split_whitespace().any(|q| q == p);
    if items
        .values()
        .filter(|(_, _, props)| has(props, "nav"))
        .count()
        != 1
    {
        problems.push("not exactly one nav document".into());
    }
    if !opf
        .descendants()
        .any(|n| n.attribute("property") == Some("dcterms:modified"))
    {
        problems.push("no dcterms:modified".into());
    }
    let uid = opf.root_element().attribute("unique-identifier");
    if !opf
        .descendants()
        .any(|n| n.has_tag_name("identifier") && uid.is_some() && n.attribute("id") == uid)
    {
        problems.push("no dc:identifier for unique-identifier".into());
    }

    // Content documents, NCX and SVG: well-formed, ids unique, MathML
    // where flagged.
    let mut texts = Vec::new();
    for (path, media, props) in items.values() {
        if matches!(*media, XHTML | NCX | "image/svg+xml") {
            texts.push((path.clone(), read(path)?, *media, *props));
        }
    }
    texts.sort();
    let mut docs = Vec::new();
    for (path, text, media, props) in &texts {
        let doc = match xml(path, text) {
            Ok(d) => d,
            Err(e) => {
                problems.push(e.to_string());
                continue;
            }
        };
        let mut ids = HashSet::new();
        for id in doc.descendants().filter_map(|n| n.attribute("id")) {
            if !ids.insert(id.to_owned()) {
                problems.push(format!("{path}: id {id} twice"));
            }
        }
        if *media == XHTML {
            if doc.root_element().tag_name().namespace() != Some(XHTML_NS) {
                problems.push(format!("{path}: not XHTML"));
            }
            let math = doc
                .descendants()
                .any(|n| n.tag_name().namespace() == Some(MATHML_NS) && n.has_tag_name("math"));
            if math != has(props, "mathml") {
                problems.push(format!("{path}: MathML {math}, property mathml {}", !math));
            }
        }
        docs.push((path.as_str(), doc, ids));
    }
    let ids: HashMap<&str, &HashSet<String>> = docs.iter().map(|(p, _, ids)| (*p, ids)).collect();
    for (path, doc, _) in &docs {
        let base = path.rsplit_once('/').map_or("", |(d, _)| d);
        for n in doc.descendants() {
            for link in ["href", "src"].into_iter().filter_map(|a| n.attribute(a)) {
                if has_scheme(link) {
                    continue;
                }
                let (file, fragment) = link.split_once('#').unwrap_or((link, ""));
                let target = match (file.is_empty(), base.is_empty()) {
                    (true, _) => path.to_string(),
                    (false, true) => file.to_owned(),
                    (false, false) => format!("{base}/{file}"),
                };
                if !names.contains(&target.as_str()) {
                    problems.push(format!("{path}: {link} is no file"));
                } else if !fragment.is_empty()
                    && !ids
                        .get(target.as_str())
                        .is_some_and(|i| i.contains(fragment))
                {
                    problems.push(format!("{path}: {link} is no id"));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapters_link_across_files() {
        use crate::WarningKind;
        use crate::model::{Image, Inline};
        let heading = |level, s: &str, label: Option<&str>| Block::Heading {
            level,
            content: vec![Inline::Text(s.into())],
            label: label.map(Into::into),
        };
        let reference = |label: &str| Inline::Ref {
            label: label.into(),
            text: None,
        };
        let doc = Doc {
            blocks: vec![
                Block::Para(vec![Inline::Text("Before.".into())]),
                heading(1, "One", Some("one")),
                Block::Para(vec![reference("two"), reference("one"), reference("gone")]),
                heading(3, "Deep", None),
                heading(1, "Two", Some("two")),
                Block::Para(vec![Inline::Image(Image {
                    resource: "a.bmp".into(),
                    alt: Some("A".into()),
                    width: None,
                })]),
            ],
            resources: vec![Resource {
                name: "a.bmp".into(),
                mime: "image/bmp".into(),
                bytes: vec![1],
                fallback: None,
            }],
            ..Doc::default()
        };
        let written = Epub.write(&doc, &WriteOptions::default()).unwrap();
        assert_eq!(check(&written.bytes), Vec::<String>::new());
        let zip = Archive::new(&written.bytes).unwrap();
        let file = |name: &str| String::from_utf8(zip.read(name).unwrap().unwrap()).unwrap();
        let one = file("OEBPS/chapter-2.xhtml");
        assert!(
            one.contains("<a href=\"chapter-3.xhtml#two\">two</a><a href=\"#one\">one</a>gone")
        );
        assert!(file("OEBPS/chapter-1.xhtml").contains("Before."));
        assert!(file("OEBPS/chapter-3.xhtml").contains("[A]"), "left out");
        let nav = file("OEBPS/nav.xhtml");
        assert!(nav.contains("#one\">One</a>\n<ol>\n<li><a href=\"chapter-2.xhtml#toc-1\">Deep"));
        assert!(!zip.names().any(|n| n.ends_with(".bmp")));
        let kinds: Vec<_> = written.warnings.iter().map(|w| w.kind).collect();
        assert_eq!(
            kinds,
            [WarningKind::ImageLeftOut, WarningKind::UnknownCitation]
        );
    }

    #[test]
    fn dates_ids_and_names() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc(951_868_800 + 3_723), "2000-03-01T01:02:03Z");
        assert_eq!(w3c_date("2026-10-26").as_deref(), Some("2026-10-26"));
        assert_eq!(w3c_date("2026-10").as_deref(), Some("2026-10"));
        assert_eq!(w3c_date("26 October 2026").as_deref(), Some("2026"));
        assert_eq!(w3c_date("Spring"), None);
        assert_eq!(w3c_date("1999 or 2000"), None);
        let a = identifier(Some("Book"), &["Ada".into()]);
        assert_eq!(a, identifier(Some("Book"), &["Ada".into()]), "stable");
        assert_ne!(a, identifier(Some("Book"), &["Bob".into()]));
        assert_eq!(a.len(), "urn:uuid:".len() + 36);
        assert_eq!(&a[23..24], "8", "version 8");
        assert_eq!(
            image_path("figs/a b.png", "png", |_| false),
            "images/a_b.png"
        );
        assert_eq!(
            image_path("x.png", "png", |p| p == "images/x.png"),
            "images/x-2.png"
        );
        assert_eq!(image_path(".png", "png", |_| false), "images/image.png");
    }
}
