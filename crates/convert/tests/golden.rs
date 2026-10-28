//! Golden tests: every `fixtures/<format>/<case>.<ext>` (`<format>` a
//! registry id) is read with that format's reader and written as Lethora
//! markup, which must equal `<case>.typ` next to it: the markup, then the
//! metadata and warnings as `//` comments. The markup must parse without
//! errors. Images resolve to files next to the case; resources become
//! `assets/<name>`; the known citation keys are [`KEYS`].
//!
//! Zip-based formats (DOCX) keep their cases unzipped, as folders
//! `<case>/` (with `[Content_Types].xml` or `mimetype` in them), zipped at
//! test time; `<case>.typ` is next to the folder. Zipped files work too.
//!
//! A bug report becomes a fixture: drop the file in (or unzip it into a
//! folder) and run
//! `UPDATE_GOLDEN=1 cargo test -p lethora-convert --test golden`, which
//! writes every `.typ`; check the diff, fix the format, run again.
//!
//! Writers: every `fixtures/<format>/write/<case>.json` (a model) is
//! written with that format's writer (side files named after the case,
//! the time of writing fixed), which must equal `<case>.<ext>`; a zip
//! (EPUB, DOCX) is compared as a text dump `<case>.<ext>.txt` (its entries in
//! order with their sizes, text files in full). The side files and
//! warnings go to `<case>.log`. `UPDATE_GOLDEN=1` writes those too.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use lethora_convert::model::{
    Block, Cell, Cite, CiteItem, Figure, FigureBody, Image, Inline, List, Meta, Resource, Table,
};
use lethora_convert::typst::{MarkupOptions, to_markup};
use lethora_convert::zip::{self, Archive, ZipWriter};
use lethora_convert::{Doc, ReadOptions, Warnings, WriteOptions, Writer, docx, epub, registry};

/// The sources a fixture may cite.
const KEYS: &[&str] = &["doe2020", "roe"];

fn sorted(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    entries
}

/// Whether `dir` is an unzipped case of a zip-based format.
fn is_unzipped(dir: &Path) -> bool {
    dir.join("[Content_Types].xml").is_file() || dir.join("mimetype").is_file()
}

/// The files under `dir`, as zip paths, sorted.
fn files(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) {
    for path in sorted(dir) {
        let name = format!("{prefix}{}", path.file_name().unwrap().to_string_lossy());
        if path.is_dir() {
            files(&path, &format!("{name}/"), out);
        } else {
            out.push((name, path));
        }
    }
}

/// The folder `dir` zipped: `mimetype` first and stored (EPUB), the rest
/// deflated.
fn zip_folder(dir: &Path) -> Vec<u8> {
    let mut entries = Vec::new();
    files(dir, "", &mut entries);
    entries.sort_by_key(|(name, _)| (name != "mimetype", name.clone()));
    let mut w = ZipWriter::new();
    for (name, path) in entries {
        w.add(&name, &fs::read(path).unwrap(), name != "mimetype")
            .unwrap();
    }
    w.finish().unwrap()
}

/// The expectation for the case at `path` (a file, or an unzipped folder).
fn expected(path: &Path, format: &registry::Format) -> String {
    let bytes = match path.is_dir() {
        true => zip_folder(path),
        false => fs::read(path).unwrap(),
    };
    let base = path.parent().unwrap().to_owned();
    let resolve = move |rel: &str| fs::read(base.join(rel)).ok();
    let mut name = path.file_name().unwrap().to_string_lossy().into_owned();
    if path.is_dir() {
        name = format!("{name}.{}", format.extensions[0]);
    }
    let options = ReadOptions {
        name: &name,
        resolve: &resolve,
    };
    let imported = format.reader.unwrap().read(&bytes, &options).unwrap();
    let assets: BTreeMap<String, String> = imported
        .doc
        .resources
        .iter()
        .map(|r| (r.name.clone(), format!("assets/{}", r.name)))
        .collect();
    let keys: BTreeSet<String> = KEYS.iter().map(|k| k.to_string()).collect();
    let markup = to_markup(
        &imported.doc,
        &MarkupOptions {
            assets: &assets,
            keys: &keys,
        },
    )
    .unwrap();
    let mut warnings = Warnings::default();
    warnings.extend(imported.warnings);
    warnings.extend(markup.warnings);
    let mut out = markup.text;
    out.push_str(&meta(&imported.doc));
    for w in warnings.into_vec() {
        let code = serde_json::to_value(w.kind).unwrap();
        out.push_str(&format!(
            "// warning {} ×{}: {}\n",
            code.as_str().unwrap(),
            w.count,
            w.message
        ));
    }
    out
}

fn meta(doc: &Doc) -> String {
    let m = &doc.meta;
    let mut out = String::new();
    let mut line = |key: &str, value: Option<&str>| {
        if let Some(v) = value {
            out.push_str(&format!("// {key}: {v}\n"));
        }
    };
    line("title", m.title.as_deref());
    line("subtitle", m.subtitle.as_deref());
    let authors = m.authors.join("; ");
    line("authors", Some(authors.as_str()).filter(|a| !a.is_empty()));
    line("date", m.date.as_deref());
    line("lang", m.lang.as_deref());
    for r in &doc.resources {
        out.push_str(&format!(
            "// resource: {} ({}, {} bytes)\n",
            r.name,
            r.mime,
            r.bytes.len()
        ));
    }
    out
}

#[test]
fn golden() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let update = std::env::var_os("UPDATE_GOLDEN").is_some();
    let mut failures = Vec::new();
    let mut cases = 0;
    for dir in sorted(&root).into_iter().filter(|d| d.is_dir()) {
        let id = dir.file_name().unwrap().to_string_lossy().into_owned();
        let format = registry::by_id(&id).unwrap_or_else(|| panic!("no format {id}"));
        for path in sorted(&dir) {
            let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase());
            let file =
                path.is_file() && ext.is_some_and(|e| format.extensions.contains(&e.as_str()));
            if !file && !(path.is_dir() && is_unzipped(&path)) {
                continue;
            }
            cases += 1;
            let actual = expected(&path, format);
            let errors = typst_syntax::parse(&actual).errors_and_warnings().0;
            if !errors.is_empty() {
                failures.push(format!("{}: markup has errors: {errors:?}", path.display()));
            }
            let typ = match path.is_dir() {
                true => path.with_file_name(format!(
                    "{}.typ",
                    path.file_name().unwrap().to_string_lossy()
                )),
                false => path.with_extension("typ"),
            };
            if update {
                fs::write(&typ, &actual).unwrap();
                continue;
            }
            let want = fs::read_to_string(&typ)
                .unwrap_or_default()
                .replace("\r\n", "\n");
            if want != actual {
                failures.push(format!(
                    "{}:\n--- expected\n{want}\n--- actual\n{actual}",
                    path.display()
                ));
            }
        }
    }
    assert!(cases > 0, "no fixtures found");
    assert!(
        failures.is_empty(),
        "{} of {cases} cases differ (UPDATE_GOLDEN=1 rewrites them):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// Damaged copies of the zipped fixtures (truncated, bytes changed) fail
/// with an error or read anyway, never panic.
#[test]
fn damaged_zips_never_panic() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut seed = 1u64;
    let mut next = move || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as usize
    };
    let mut cases = 0;
    for dir in sorted(&root).into_iter().filter(|d| d.is_dir()) {
        let id = dir.file_name().unwrap().to_string_lossy().into_owned();
        let Some(format) = registry::by_id(&id) else {
            continue;
        };
        for case in sorted(&dir).into_iter().filter(|p| is_unzipped(p)) {
            cases += 1;
            let good = zip_folder(&case);
            let read = |bytes: &[u8]| {
                let _ = format
                    .reader
                    .unwrap()
                    .read(bytes, &ReadOptions::new("damaged"));
            };
            for len in (0..good.len()).step_by(good.len() / 40 + 1) {
                read(&good[..len]);
            }
            for _ in 0..60 {
                let mut bad = good.clone();
                for _ in 0..1 + next() % 3 {
                    let at = next() % bad.len();
                    bad[at] = next() as u8;
                }
                read(&bad);
            }
        }
    }
    assert!(cases > 0, "no zipped fixtures found");
}

#[test]
fn writers() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let update = std::env::var_os("UPDATE_GOLDEN").is_some();
    let mut failures = Vec::new();
    let mut cases = 0;
    for dir in sorted(&root)
        .into_iter()
        .filter(|d| d.join("write").is_dir())
    {
        let id = dir.file_name().unwrap().to_string_lossy().into_owned();
        let format = registry::by_id(&id).unwrap_or_else(|| panic!("no format {id}"));
        let writer = format
            .writer
            .unwrap_or_else(|| panic!("{id} has no writer"));
        for path in sorted(&dir.join("write")) {
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            cases += 1;
            let doc: Doc = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            let mut options = WriteOptions::default();
            options.stem = path.file_stem().unwrap().to_string_lossy().into_owned();
            options.modified = "2026-10-26T12:00:00Z".into();
            let mut written = writer.write(&doc, &options).unwrap();
            let mut log = String::new();
            for (name, bytes) in &written.files {
                log.push_str(&format!("file {name} ({} bytes)\n", bytes.len()));
            }
            for w in &written.warnings {
                let code = serde_json::to_value(w.kind).unwrap();
                log.push_str(&format!(
                    "warning {} ×{}: {}\n",
                    code.as_str().unwrap(),
                    w.count,
                    w.message
                ));
            }
            let mut out = path.with_extension(format.extensions[0]);
            if zip::is_zip(&written.bytes) {
                out = path.with_extension(format!("{}.txt", format.extensions[0]));
                written.bytes = dump(&written.bytes).into_bytes();
            }
            let log_path = path.with_extension("log");
            if update {
                fs::write(&out, &written.bytes).unwrap();
                fs::write(&log_path, &log).unwrap();
                continue;
            }
            let want = fs::read(&out).unwrap_or_default();
            let want = match out.extension().is_some_and(|e| e == "txt") {
                true => String::from_utf8_lossy(&want)
                    .replace("\r\n", "\n")
                    .into_bytes(),
                false => want,
            };
            let want_log = fs::read_to_string(&log_path).unwrap_or_default();
            if want != written.bytes || want_log.replace("\r\n", "\n") != log {
                failures.push(format!(
                    "{}:\n--- expected\n{}{want_log}\n--- actual\n{}{log}",
                    path.display(),
                    String::from_utf8_lossy(&want),
                    String::from_utf8_lossy(&written.bytes)
                ));
            }
        }
    }
    assert!(cases > 0, "no writer fixtures found");
    assert!(
        failures.is_empty(),
        "{} of {cases} cases differ (UPDATE_GOLDEN=1 rewrites them):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// A zip as text: each entry in order with its size, text files in full.
fn dump(bytes: &[u8]) -> String {
    let archive = Archive::new(bytes).unwrap();
    let mut out = String::new();
    for name in archive.names() {
        let data = archive.read(name).unwrap().unwrap();
        out.push_str(&format!("=== {name} ({} bytes)\n", data.len()));
        let ext = name.rsplit('.').next().unwrap_or("");
        if name == "mimetype" || ["xhtml", "opf", "ncx", "css", "xml", "rels"].contains(&ext) {
            out.push_str(&String::from_utf8_lossy(&data));
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    out
}

/// Every writer fixture's model as an EPUB passes the structural check
/// (what epubcheck would fail first).
#[test]
fn epub_structure() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut cases = 0;
    for dir in sorted(&root)
        .into_iter()
        .filter(|d| d.join("write").is_dir())
    {
        for path in sorted(&dir.join("write")) {
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            cases += 1;
            let doc: Doc = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            let written = epub::Epub.write(&doc, &WriteOptions::default()).unwrap();
            let problems = epub::check(&written.bytes);
            assert!(problems.is_empty(), "{}: {problems:#?}", path.display());
        }
    }
    assert!(cases > 0, "no writer fixtures found");
    // An empty book, and one with only a title, are books too.
    for title in [None, Some("Only a title".to_owned())] {
        let mut doc = Doc::default();
        doc.meta.title = title;
        let written = epub::Epub.write(&doc, &WriteOptions::default()).unwrap();
        assert_eq!(epub::check(&written.bytes), Vec::<String>::new());
    }
    // The check finds what is wrong.
    let mut w = ZipWriter::new();
    w.add("mimetype", b"application/epub+zip", true).unwrap();
    assert!(!epub::check(&w.finish().unwrap()).is_empty());
}

/// Markdown written and read back is the same model, for what both do:
/// metadata, headings with labels, emphasis, code, links, line breaks,
/// lists, quotes, code blocks, rules, tables with a header, footnotes,
/// figures captioned by their alt text and citations as written.
#[test]
fn markdown_round_trip() {
    let text = |s: &str| Inline::Text(s.into());
    let para = Block::Para;
    let cell = |s: &str| Cell::new(vec![Block::Para(vec![Inline::Text(s.into())])]);
    let doc = Doc {
        meta: Meta {
            title: Some("A \"Title\"".into()),
            subtitle: Some("Sub".into()),
            authors: vec!["Ada Lovelace".into(), "Charles Babbage".into()],
            date: Some("2026-10-26".into()),
            lang: Some("en-GB".into()),
        },
        blocks: vec![
            Block::Heading {
                level: 1,
                content: vec![text("Intro "), Inline::Emph(vec![text("now")])],
                label: Some("intro".into()),
            },
            para(vec![
                text("1. Plain *text* with "),
                Inline::Strong(vec![text("bold")]),
                text(", "),
                Inline::Strike(vec![text("gone")]),
                text(", "),
                Inline::Code("a`b".into()),
                text(" and "),
                Inline::Link {
                    url: "https://example.com/x y".into(),
                    content: vec![text("a link")],
                },
                text(" or "),
                Inline::Link {
                    url: "https://typst.app".into(),
                    content: vec![text("https://typst.app")],
                },
                text(" [x] snake_case"),
                Inline::LineBreak,
                text("# next line, cited "),
                Inline::Cite(Cite {
                    items: vec![CiteItem {
                        key: "doe2020".into(),
                        locator: Some("p. 4".into()),
                    }],
                    text: Some(vec![text("[@doe2020, p. 4]")]),
                }),
                text("."),
                Inline::Footnote(vec![para(vec![text("A note.")])]),
            ]),
            Block::List(List {
                ordered: true,
                start: 3,
                items: vec![
                    vec![para(vec![text("three")])],
                    vec![
                        para(vec![text("four")]),
                        Block::List(List {
                            ordered: false,
                            start: 1,
                            items: vec![vec![para(vec![text("nested")])]],
                        }),
                    ],
                ],
            }),
            Block::Quote(vec![para(vec![text("Quoted.")])]),
            Block::Code {
                lang: Some("rust".into()),
                // As the reader has it: ending in a line break.
                text: "fn main() {}\n```\n".into(),
            },
            Block::Rule,
            Block::Table(Table {
                columns: 2,
                header_rows: 1,
                rows: vec![vec![cell("A | B"), cell("C")], vec![cell("1"), cell("2")]],
            }),
            Block::Figure(Figure {
                body: FigureBody::Image(Image {
                    resource: "dot.png".into(),
                    alt: Some("A dot".into()),
                    width: None,
                }),
                caption: vec![text("A dot")],
                label: None,
                number: None,
            }),
        ],
        resources: vec![Resource {
            name: "dot.png".into(),
            mime: "image/png".into(),
            bytes: b"not really a PNG".to_vec(),
            fallback: None,
        }],
    };
    let mut options = WriteOptions::default();
    options.stem = "round".into();
    let markdown = registry::by_id("markdown").unwrap();
    let written = markdown.writer.unwrap().write(&doc, &options).unwrap();
    assert!(written.warnings.is_empty(), "{:?}", written.warnings);
    let files = written.files.clone();
    let resolve = move |rel: &str| files.iter().find(|(p, _)| p == rel).map(|(_, b)| b.clone());
    let options = ReadOptions {
        name: "round.md",
        resolve: &resolve,
    };
    let read = markdown
        .reader
        .unwrap()
        .read(&written.bytes, &options)
        .unwrap();
    assert!(read.warnings.is_empty(), "{:?}", read.warnings);
    // Images come back named by their path.
    let mut back = read.doc;
    for r in &mut back.resources {
        r.name = r.name.replace("round_images/", "");
    }
    if let Some(Block::Figure(f)) = back.blocks.last_mut()
        && let FigureBody::Image(img) = &mut f.body
    {
        img.resource = img.resource.replace("round_images/", "");
    }
    assert_eq!(
        serde_json::to_value(&back).unwrap(),
        serde_json::to_value(&doc).unwrap(),
        "{}",
        String::from_utf8_lossy(&written.bytes)
    );
}

/// Every writer fixture's model as DOCX passes the structural check, and
/// so do an empty document and one with only a title.
#[test]
fn docx_structure() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut cases = 0;
    for dir in sorted(&root)
        .into_iter()
        .filter(|d| d.join("write").is_dir())
    {
        for path in sorted(&dir.join("write")) {
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            cases += 1;
            let doc: Doc = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            let written = docx::Docx.write(&doc, &WriteOptions::default()).unwrap();
            let problems = docx::check(&written.bytes);
            assert!(problems.is_empty(), "{}: {problems:#?}", path.display());
        }
    }
    assert!(cases > 0, "no writer fixtures found");
    for title in [None, Some("Only a title".to_owned())] {
        let mut doc = Doc::default();
        doc.meta.title = title;
        let written = docx::Docx.write(&doc, &WriteOptions::default()).unwrap();
        assert_eq!(docx::check(&written.bytes), Vec::<String>::new());
    }
}

/// DOCX written and read back (the R3b reader) keeps the main constructs:
/// title and subtitle, headings, emphasis, links, footnotes, lists with
/// their starts and nesting, tables with spans and a header row, figures
/// with captions (image and table), images by their bytes and width.
/// Different by design: labels, figure numbers and references are gone
/// (references keep their text, the "Figure 1:" label is dropped as the
/// reader drops Word's), the authors come back from `docProps/core.xml`
/// and their title-page paragraphs as paragraphs, images are named by
/// their media file, inline code is plain text (Word's code is a font),
/// a quote's paragraphs are plain ones, a code block one paragraph with
/// line breaks, a rule nothing, and formulas (not here) come back as their
/// images.
#[test]
fn docx_round_trip() {
    let text = |s: &str| Inline::Text(s.into());
    let para = Block::Para;
    let cell = |s: &str| Cell::new(vec![Block::Para(vec![Inline::Text(s.into())])]);
    let span = |colspan, rowspan, s: &str| Cell {
        colspan,
        rowspan,
        ..cell(s)
    };
    let table = Table {
        columns: 3,
        header_rows: 1,
        rows: vec![
            vec![cell("A"), cell("B"), cell("C")],
            vec![span(2, 1, "wide"), span(1, 2, "tall")],
            vec![cell("1"), cell("2")],
        ],
    };
    let list = |ordered, start, items: Vec<Vec<Block>>| {
        Block::List(List {
            ordered,
            start,
            items,
        })
    };
    let lists = vec![
        list(
            true,
            3,
            vec![
                vec![para(vec![text("three")])],
                vec![
                    para(vec![text("four")]),
                    list(false, 1, vec![vec![para(vec![text("nested")])]]),
                ],
            ],
        ),
        list(
            false,
            1,
            vec![vec![para(vec![text("a")])], vec![para(vec![text("b")])]],
        ),
    ];
    let png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\0\x04\0\0\0\x02 not really".to_vec();
    let emphasis = vec![
        text("Plain "),
        Inline::Strong(vec![text("bold")]),
        text(", "),
        Inline::Emph(vec![text("italic")]),
        text(", "),
        Inline::Underline(vec![text("under")]),
        text(", "),
        Inline::Strike(vec![text("gone")]),
        text(", H"),
        Inline::Sub(vec![text("2")]),
        text("O, x"),
        Inline::Super(vec![text("2")]),
        text(" and "),
        Inline::Link {
            url: "https://example.com/".into(),
            content: vec![text("a link")],
        },
        text("."),
        Inline::Footnote(vec![para(vec![
            text("A note with "),
            Inline::Emph(vec![text("emphasis")]),
            text("."),
        ])]),
    ];
    let doc = Doc {
        meta: Meta {
            title: Some("Round trip".into()),
            subtitle: Some("Sub".into()),
            authors: vec!["Ada Lovelace".into(), "Charles Babbage".into()],
            date: None,
            lang: Some("en-GB".into()),
        },
        blocks: [
            vec![
                Block::Heading {
                    level: 1,
                    content: vec![text("1. Intro")],
                    label: Some("intro".into()),
                },
                para(emphasis.clone()),
                para(vec![
                    text("See "),
                    Inline::Ref {
                        label: "intro".into(),
                        text: Some(vec![text("Section 1")]),
                    },
                    text(", "),
                    Inline::Code("code".into()),
                    text(" and "),
                    Inline::Cite(Cite {
                        items: vec![CiteItem {
                            key: "doe2020".into(),
                            locator: None,
                        }],
                        text: Some(vec![text("(Doe, 2020)")]),
                    }),
                    text("."),
                ]),
                Block::Heading {
                    level: 2,
                    content: vec![text("1.1. Lists")],
                    label: None,
                },
            ],
            lists.clone(),
            vec![
                Block::Table(table.clone()),
                Block::Figure(Figure {
                    body: FigureBody::Image(Image {
                        resource: "dot.png".into(),
                        alt: Some("A dot".into()),
                        width: Some(0.5),
                    }),
                    caption: vec![text("A "), Inline::Emph(vec![text("dot")]), text(".")],
                    label: Some("dot".into()),
                    number: Some("Figure 1".into()),
                }),
                Block::Figure(Figure {
                    body: FigureBody::Table(table.clone()),
                    caption: vec![text("Values.")],
                    label: None,
                    number: Some("Table 1".into()),
                }),
                Block::Quote(vec![para(vec![text("Quoted.")])]),
                Block::Code {
                    lang: None,
                    text: "a\nb".into(),
                },
                Block::Rule,
            ],
        ]
        .concat(),
        resources: vec![Resource {
            name: "dot.png".into(),
            mime: "image/png".into(),
            bytes: png.clone(),
            fallback: None,
        }],
    };
    let format = registry::by_id("docx").unwrap();
    let written = format
        .writer
        .unwrap()
        .write(&doc, &WriteOptions::default())
        .unwrap();
    assert!(written.warnings.is_empty(), "{:?}", written.warnings);
    assert_eq!(docx::check(&written.bytes), Vec::<String>::new());
    let mut back = format
        .reader
        .unwrap()
        .read(&written.bytes, &ReadOptions::new("round.docx"))
        .unwrap();
    assert!(back.warnings.is_empty(), "{:?}", back.warnings);
    // Widths come back from EMU over twips: equal to the per mille.
    for b in &mut back.doc.blocks {
        if let Block::Figure(f) = b
            && let FigureBody::Image(img) = &mut f.body
        {
            img.width = img.width.map(|w| (w * 1000.0).round() / 1000.0);
        }
    }
    let figure = |body, caption| {
        Block::Figure(Figure {
            body,
            caption,
            label: None,
            number: None,
        })
    };
    let expected = Doc {
        meta: doc.meta.clone(),
        blocks: [
            vec![
                para(vec![text("Ada Lovelace")]),
                para(vec![text("Charles Babbage")]),
                Block::Heading {
                    level: 1,
                    content: vec![text("1. Intro")],
                    label: None,
                },
                para(emphasis),
                para(vec![text("See Section 1, code and (Doe, 2020).")]),
                Block::Heading {
                    level: 2,
                    content: vec![text("1.1. Lists")],
                    label: None,
                },
            ],
            lists,
            vec![
                Block::Table(table.clone()),
                figure(
                    FigureBody::Image(Image {
                        resource: "image1.png".into(),
                        alt: Some("A dot".into()),
                        width: Some(0.5),
                    }),
                    vec![text("A "), Inline::Emph(vec![text("dot")]), text(".")],
                ),
                figure(FigureBody::Table(table), vec![text("Values.")]),
                para(vec![text("Quoted.")]),
                para(vec![text("a"), Inline::LineBreak, text("b")]),
            ],
        ]
        .concat(),
        resources: vec![Resource {
            name: "image1.png".into(),
            mime: "image/png".into(),
            bytes: png,
            fallback: None,
        }],
    };
    assert_eq!(
        serde_json::to_value(&back.doc).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
}
