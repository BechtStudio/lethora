//! Lethora's import and export formats, hub and spoke: every format reads
//! into or writes from one document model ([`model::Doc`]), and one writer
//! turns the model into Lethora markup ([`typst::to_markup`]) for every
//! import. UI-free; no Typst, no Loro.
//!
//! A format is a module implementing [`Reader`] (bytes → model) or
//! [`Writer`] (model → bytes), listed in [`registry::FORMATS`]. Both return
//! [`Warning`]s for what they dropped or approximated, one per kind with a
//! count, so the app can show them and bug reports can quote them. Hostile
//! input fails with an [`Error`] (too large, nested too deep), never a panic.
//!
//! # Adding a format
//!
//! 1. A module with a unit struct implementing [`Reader`] and/or
//!    [`Writer`], reading into or writing from the model only (a bug in it
//!    stays in it).
//! 2. One line in [`registry::FORMATS`]: id, name, extensions, sniffing.
//! 3. Golden fixtures in `tests/fixtures/<id>/`: `<case>.<ext>` and the
//!    expected markup `<case>.typ` next to it; for a writer, models
//!    `write/<case>.json` and the expected `write/<case>.<ext>` (see
//!    `tests/golden.rs`; `UPDATE_GOLDEN=1 cargo test -p lethora-convert`
//!    writes the expectations).
//!
//! Exports reach the writers from the compiled document
//! (`lethora_typeset::export`), with citations, numbers and references
//! resolved; imports go on to Lethora markup.
//!
//! New warning kinds go into [`WarningKind`]; the model grows by new
//! variants, never by reshaping existing ones.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub mod docx;
mod docx_write;
pub mod epub;
pub mod html;
mod latex;
pub mod markdown;
mod markdown_write;
pub mod model;
mod omml;
pub mod registry;
pub mod text;
pub mod typst;
mod xhtml;
pub mod zip;

pub use model::Doc;

/// The largest input file a reader takes.
pub const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;
/// The deepest nesting (blocks in blocks, inlines in inlines) a reader or
/// writer follows.
pub const MAX_DEPTH: usize = 64;

/// Why a file could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("The file is larger than {} MB.", MAX_INPUT_BYTES / (1024 * 1024))]
    TooLarge,
    #[error("The file nests deeper than {MAX_DEPTH} levels.")]
    TooDeep,
    /// The input is broken; the message says how.
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// What a reader gets besides the bytes.
pub struct ReadOptions<'a> {
    /// The file's name (no folders), for messages.
    pub name: &'a str,
    /// Reads a file the input refers to by its path as written (a Markdown
    /// image next to it), or `None` if it can't.
    pub resolve: &'a dyn Fn(&str) -> Option<Vec<u8>>,
}

impl<'a> ReadOptions<'a> {
    /// Options for a file named `name` whose references resolve to nothing.
    pub fn new(name: &'a str) -> Self {
        ReadOptions {
            name,
            resolve: &nothing,
        }
    }
}

fn nothing(_: &str) -> Option<Vec<u8>> {
    None
}

/// A file read into the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Imported {
    pub doc: Doc,
    pub warnings: Vec<Warning>,
}

/// What a writer gets besides the model.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct WriteOptions {
    /// The output file's name without its extension (`thesis`), for the
    /// names of side files (Markdown's `thesis_images/`); empty if unknown.
    pub stem: String,
    /// The time of writing in UTC (`2026-10-26T14:03:00Z`), for EPUB's
    /// `dcterms:modified` and DOCX's; empty: the clock's (the tests fix it).
    pub modified: String,
    /// The page and text look of the layout, for writers that set up pages
    /// (DOCX's styles and page); `None`: [`PageHints::default`].
    pub page: Option<PageHints>,
}

/// What a writer that sets up pages takes from the project's layout (the
/// layout itself never gets into the model). Lengths in points.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PageHints {
    /// The body font's family ("Libertinus Serif").
    pub font: String,
    pub size_pt: f64,
    /// Heading sizes, level 1 first; the last one is for deeper levels too.
    pub heading_sizes_pt: Vec<f64>,
    pub width_pt: f64,
    pub height_pt: f64,
    pub margin_top_pt: f64,
    pub margin_bottom_pt: f64,
    /// The binding side (left on one-sided pages).
    pub margin_inside_pt: f64,
    pub margin_outside_pt: f64,
    /// Inside and outside margins alternate (mirrored).
    pub two_sided: bool,
    pub justify: bool,
    pub hyphenate: bool,
    /// Of paragraphs following a paragraph.
    pub first_line_indent_pt: f64,
    /// Space between paragraphs beyond the line spacing.
    pub paragraph_spacing_pt: f64,
    /// Line spacing as a multiple of the font's single spacing.
    pub line_spacing: f64,
    /// Level-1 headings start a new page.
    pub chapter_page_break: bool,
    /// The width of an image without one, as a fraction of the text width.
    pub image_width: f64,
    /// Captions above image figures (else below).
    pub figure_caption_top: bool,
    /// Captions above table figures (else below).
    pub table_caption_top: bool,
}

impl Default for PageHints {
    /// A4, 25 mm margins, Times New Roman 11 pt.
    fn default() -> Self {
        let mm = 72.0 / 25.4;
        PageHints {
            font: "Times New Roman".into(),
            size_pt: 11.0,
            heading_sizes_pt: vec![16.0, 13.0, 11.0],
            width_pt: 210.0 * mm,
            height_pt: 297.0 * mm,
            margin_top_pt: 25.0 * mm,
            margin_bottom_pt: 25.0 * mm,
            margin_inside_pt: 25.0 * mm,
            margin_outside_pt: 25.0 * mm,
            two_sided: false,
            justify: false,
            hyphenate: false,
            first_line_indent_pt: 0.0,
            paragraph_spacing_pt: 6.0,
            line_spacing: 1.15,
            chapter_page_break: false,
            image_width: 0.8,
            figure_caption_top: false,
            table_caption_top: true,
        }
    }
}

/// A model written into a file's bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Written {
    pub bytes: Vec<u8>,
    /// Files to write next to it (images), by path relative to its folder
    /// (`thesis_images/plot.png`, `/`-separated).
    #[serde(default)]
    pub files: Vec<(String, Vec<u8>)>,
    pub warnings: Vec<Warning>,
}

/// Reads one format into the model.
pub trait Reader: Sync {
    fn read(&self, bytes: &[u8], options: &ReadOptions) -> Result<Imported>;
}

/// Writes the model as one format.
pub trait Writer: Sync {
    fn write(&self, doc: &Doc, options: &WriteOptions) -> Result<Written>;
}

/// What was dropped or approximated. Stable codes (kebab-case in JSON):
/// bug reports quote them, so never rename one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WarningKind {
    /// Neither UTF-8 nor UTF-16: read as Windows-1252.
    Encoding,
    /// Raw HTML dropped (Markdown; the text between inline tags stays).
    Html,
    /// Front matter fields other than title, subtitle, author, date and
    /// lang dropped.
    FrontMatter,
    /// Task list checkboxes dropped (the items stay).
    TaskList,
    /// LaTeX the math converter doesn't know, kept as text in the formula.
    Math,
    /// A remote image became a link to it.
    RemoteImage,
    /// An image file that couldn't be read.
    MissingImage,
    /// An image left out: a type or size the project or the output format
    /// can't hold (or, on import, inserted as a suggestion, which can't
    /// index images).
    ImageLeftOut,
    /// A citation or reference whose key is neither a source nor a label of
    /// the document, kept as text.
    UnknownCitation,
    /// A footnote reference without a note, kept as text.
    MissingFootnote,
    /// A footnote never referenced, dropped.
    UnusedFootnote,
    /// A reference manager's citation field (Zotero, Mendeley, EndNote,
    /// Word) kept as the text it shows.
    CitationField,
    /// A table of contents or index field dropped (the layout makes its
    /// own).
    Contents,
    /// Tracked changes accepted (insertions kept, deletions dropped).
    TrackedChange,
    /// A comment dropped.
    Comment,
    /// A non-empty header or footer dropped.
    HeaderFooter,
    /// A page or column break dropped.
    Break,
    /// A text box's paragraphs moved after the paragraph it is anchored in.
    TextBox,
    /// A table inside a table cell flattened into the cell's paragraphs.
    NestedTable,
    /// A drawing without a picture (chart, diagram, shape) or an embedded
    /// object left out.
    Object,
    /// Something only pages have (a page break, a line drawn, a placed
    /// box) left out of an export.
    NotExported,
    /// Formulas written as their Typst source (the format has no math).
    TypstMath,
    /// Formatting the format can't show (underline, small caps) dropped;
    /// the text stays.
    Formatting,
    /// A table simplified: merged cells split, or blocks in a cell joined
    /// into one line.
    TableSimplified,
    /// A footnote inside a footnote put in parentheses where it was (the
    /// format has no notes in notes).
    NestedNote,
}

impl WarningKind {
    /// The message for `count` cases, `example` being the first one's detail.
    fn message(self, count: u32, example: Option<&str>) -> String {
        let n = |one: &str, many: &str| {
            if count == 1 {
                format!("1 {one}")
            } else {
                format!("{count} {many}")
            }
        };
        let eg = example.map(|e| format!(" (e.g. {e})")).unwrap_or_default();
        match self {
            WarningKind::Encoding => "Not UTF-8: read as Windows-1252.".to_owned(),
            WarningKind::Html => format!("Dropped {}{eg}.", n("piece of HTML", "pieces of HTML")),
            WarningKind::FrontMatter => format!(
                "Dropped {}{eg}.",
                n("front matter field", "front matter fields")
            ),
            WarningKind::TaskList => format!(
                "{} became plain list items.",
                n("task list checkbox", "task list checkboxes")
            ),
            WarningKind::Math => format!(
                "Kept {} as text in formulas{eg}.",
                n("unknown math command", "unknown math commands")
            ),
            WarningKind::RemoteImage => {
                format!("{} became links{eg}.", n("remote image", "remote images"))
            }
            WarningKind::MissingImage => {
                format!("{} not found{eg}.", n("image", "images"))
            }
            WarningKind::ImageLeftOut => format!(
                "Left out {}{eg}: a type or size that can't be used here.",
                n("image", "images")
            ),
            WarningKind::UnknownCitation => format!(
                "Kept {} as text: not in the sources or the document{eg}.",
                n("citation", "citations")
            ),
            WarningKind::MissingFootnote => format!(
                "Kept {} as text: no such note{eg}.",
                n("footnote reference", "footnote references")
            ),
            WarningKind::UnusedFootnote => format!(
                "Dropped {}: never referenced{eg}.",
                n("footnote", "footnotes")
            ),
            WarningKind::CitationField => format!(
                "Kept {} as text{eg}.",
                n("citation field", "citation fields")
            ),
            WarningKind::Contents => format!(
                "Dropped {}{eg}: the layout makes its own.",
                n(
                    "table of contents or index",
                    "tables of contents or indexes"
                )
            ),
            WarningKind::TrackedChange => {
                format!("Accepted {}.", n("tracked change", "tracked changes"))
            }
            WarningKind::Comment => format!("Dropped {}.", n("comment", "comments")),
            WarningKind::HeaderFooter => format!(
                "Dropped {}{eg}.",
                n("header or footer", "headers and footers")
            ),
            WarningKind::Break => format!(
                "Dropped {}.",
                n("page or column break", "page and column breaks")
            ),
            WarningKind::TextBox => format!(
                "Moved {} below {}.",
                n("text box", "text boxes"),
                if count == 1 {
                    "its paragraph"
                } else {
                    "their paragraphs"
                }
            ),
            WarningKind::NestedTable => format!(
                "Flattened {} into the cells' paragraphs.",
                n("nested table", "nested tables")
            ),
            WarningKind::Object => format!(
                "Left out {}{eg}.",
                n(
                    "chart, shape or embedded object",
                    "charts, shapes and embedded objects"
                )
            ),
            WarningKind::NotExported => format!(
                "Left out {} that only work on pages{eg}.",
                n("element", "elements")
            ),
            WarningKind::TypstMath => {
                format!("Kept {} as Typst source{eg}.", n("formula", "formulas"))
            }
            WarningKind::Formatting => format!(
                "Dropped the formatting of {}{eg}; the text stays.",
                n("span", "spans")
            ),
            WarningKind::TableSimplified => format!(
                "Simplified {}: merged cells or several paragraphs in a cell{eg}.",
                n("table", "tables")
            ),
            WarningKind::NestedNote => format!(
                "Put {} in parentheses.",
                n("footnote inside a footnote", "footnotes inside footnotes")
            ),
        }
    }
}

/// One kind of problem, however often it happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Warning {
    pub kind: WarningKind,
    pub count: u32,
    /// The first case's detail (a file, a key, a command).
    pub example: Option<String>,
    /// For people: "Dropped 12 pieces of HTML (e.g. <div>)."
    pub message: String,
}

/// Collects warnings, one per kind.
#[derive(Debug, Default)]
pub struct Warnings {
    seen: BTreeMap<WarningKind, (u32, Option<String>)>,
}

impl Warnings {
    /// One more case of `kind`.
    pub fn add(&mut self, kind: WarningKind) {
        self.seen.entry(kind).or_default().0 += 1;
    }

    /// One more case of `kind`; `example` is kept if it is the first.
    pub fn add_example(&mut self, kind: WarningKind, example: impl Into<String>) {
        let entry = self.seen.entry(kind).or_default();
        entry.0 += 1;
        if entry.1.is_none() {
            entry.1 = Some(shorten(example.into()));
        }
    }

    /// Adds warnings collected elsewhere (counts add up).
    pub fn extend(&mut self, warnings: impl IntoIterator<Item = Warning>) {
        for w in warnings {
            let entry = self.seen.entry(w.kind).or_default();
            entry.0 += w.count;
            if entry.1.is_none() {
                entry.1 = w.example;
            }
        }
    }

    /// The warnings, in the order of [`WarningKind`].
    pub fn into_vec(self) -> Vec<Warning> {
        self.seen
            .into_iter()
            .map(|(kind, (count, example))| Warning {
                kind,
                count,
                message: kind.message(count, example.as_deref()),
                example,
            })
            .collect()
    }
}

/// At most 60 characters of an example, so a message stays one line.
fn shorten(s: String) -> String {
    const MAX: usize = 60;
    if s.chars().count() <= MAX {
        return s;
    }
    let mut out: String = s.chars().take(MAX - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warnings_collapse_by_kind() {
        let mut w = Warnings::default();
        w.add_example(WarningKind::Html, "<div>");
        w.add_example(WarningKind::Html, "<span>");
        w.add(WarningKind::Encoding);
        let list = w.into_vec();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].kind, WarningKind::Encoding);
        assert_eq!(list[1].count, 2);
        assert_eq!(list[1].message, "Dropped 2 pieces of HTML (e.g. <div>).");
        assert_eq!(
            serde_json::to_value(&list[1]).unwrap()["kind"],
            "html",
            "codes are stable kebab-case"
        );
    }

    #[test]
    fn extend_adds_counts() {
        let mut a = Warnings::default();
        a.add(WarningKind::MissingImage);
        let mut b = Warnings::default();
        b.add_example(WarningKind::MissingImage, "a.png");
        b.add(WarningKind::MissingImage);
        a.extend(b.into_vec());
        let list = a.into_vec();
        assert_eq!(list[0].count, 3);
        assert_eq!(list[0].message, "3 images not found (e.g. a.png).");
    }
}
