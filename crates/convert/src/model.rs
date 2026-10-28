//! The one document model between all formats: blocks and inlines, the
//! metadata and the files they show (resources). Content only: fonts,
//! sizes and page setup never get in; the layout comes from the project's
//! preset. Serde, so a model can cross a process boundary (the export
//! worker).
//!
//! Readers fill what their format has; writers use what theirs can show.
//! Resolved parts (a citation's text, a figure's number) are optional:
//! exports from the compiled document fill them, imports mostly don't.

use serde::{Deserialize, Serialize};

/// A document.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Doc {
    pub meta: Meta,
    pub blocks: Vec<Block>,
    /// The files images refer to, by [`Resource::name`].
    pub resources: Vec<Resource>,
}

impl Doc {
    /// The resource named `name`.
    pub fn resource(&self, name: &str) -> Option<&Resource> {
        self.resources.iter().find(|r| r.name == name)
    }
}

/// What the document says about itself.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub title: Option<String>,
    pub subtitle: Option<String>,
    /// In the order given, each as written ("Ada Lovelace").
    pub authors: Vec<String>,
    /// As written; no format is assumed.
    pub date: Option<String>,
    /// A BCP 47 tag (`en-US`).
    pub lang: Option<String>,
}

/// A file the document shows (an image), carried with it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resource {
    /// Unique in the document: the path as the source wrote it
    /// (`figures/a.png`), or a made-up name. Its extension tells the type.
    pub name: String,
    /// The media type (`image/png`), `application/octet-stream` if unknown.
    pub mime: String,
    pub bytes: Vec<u8>,
    /// The [`Resource::name`] of a raster twin (a PNG) for formats that
    /// can't show this one alone (an SVG in Word), if an export made one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
}

/// A block: something that starts on a line of its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Block {
    /// A paragraph.
    Para(Vec<Inline>),
    /// A heading, `level` 1 (chapter) to 6.
    Heading {
        level: u8,
        content: Vec<Inline>,
        /// The name references use (`<intro>`), if any.
        label: Option<String>,
    },
    /// A bulleted or numbered list; each item is blocks (paragraphs, nested
    /// lists).
    List(List),
    /// A block quotation.
    Quote(Vec<Block>),
    /// Code shown as written, in `lang` if known (`rust`).
    Code { lang: Option<String>, text: String },
    /// A display formula.
    Math {
        math: Math,
        label: Option<String>,
        /// The number as typeset ("(3)"), if resolved.
        number: Option<String>,
    },
    /// An image or table with a caption.
    Figure(Figure),
    /// A table without a caption.
    Table(Table),
    /// A thematic break (a scene break in fiction).
    Rule,
}

/// A list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct List {
    /// Numbered (`1.`, `2.`, ...) rather than bulleted.
    pub ordered: bool,
    /// The first number of a numbered list.
    pub start: u64,
    pub items: Vec<Vec<Block>>,
}

/// A figure: its body, caption, label and number.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Figure {
    pub body: FigureBody,
    /// Empty if none.
    pub caption: Vec<Inline>,
    pub label: Option<String>,
    /// The number as typeset ("Figure 2"), if resolved.
    pub number: Option<String>,
}

/// What a figure shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FigureBody {
    Image(Image),
    Table(Table),
}

/// An image: a resource of the document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Image {
    /// The [`Resource::name`] of its file.
    pub resource: String,
    /// The text standing in for it, if given.
    pub alt: Option<String>,
    /// Its width as a fraction of the text column (0.8 = 80%), if known.
    pub width: Option<f64>,
}

/// A table: cells row by row, the first `header_rows` rows the header.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub columns: usize,
    pub header_rows: usize,
    /// Each row holds the cells that start in it, left to right; a slot
    /// covered by a cell spanning into it from above or the left has none
    /// (as in HTML).
    pub rows: Vec<Vec<Cell>>,
}

/// A table cell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    pub content: Vec<Block>,
    #[serde(default = "one")]
    pub colspan: u32,
    #[serde(default = "one")]
    pub rowspan: u32,
}

impl Cell {
    /// A cell of one slot.
    pub fn new(content: Vec<Block>) -> Self {
        Cell {
            content,
            colspan: 1,
            rowspan: 1,
        }
    }
}

fn one() -> u32 {
    1
}

/// A formula: its Typst source always, rendered forms when an export has
/// them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Math {
    /// Typst math, without the `$`.
    pub typst: String,
    /// MathML (`<math>…</math>`), if rendered.
    pub mathml: Option<String>,
    /// The [`Resource::name`] of a rendered image (SVG or PNG), if any.
    pub image: Option<String>,
    /// The rendered image's size, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<MathSize>,
}

impl Math {
    /// A formula known only by its Typst source.
    pub fn typst(source: impl Into<String>) -> Self {
        Math {
            typst: source.into(),
            mathml: None,
            image: None,
            size: None,
        }
    }
}

/// A rendered formula's size in points, as typeset in the body text size.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MathSize {
    pub width: f64,
    pub height: f64,
    /// The baseline's distance from the top: an inline formula sits
    /// `height - baseline` below the line's baseline.
    pub baseline: f64,
}

/// Inline content: runs inside a paragraph, heading or caption.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Inline {
    /// Plain text; line breaks in it are spaces.
    Text(String),
    /// Italic.
    Emph(Vec<Inline>),
    /// Bold.
    Strong(Vec<Inline>),
    Underline(Vec<Inline>),
    /// Strikethrough.
    Strike(Vec<Inline>),
    /// Superscript.
    Super(Vec<Inline>),
    /// Subscript.
    Sub(Vec<Inline>),
    /// Code in the line, shown as written.
    Code(String),
    /// A forced line break inside the paragraph.
    LineBreak,
    /// A link to `url`, showing `content` (the URL itself if empty).
    Link {
        url: String,
        content: Vec<Inline>,
    },
    /// An inline formula.
    Math(Math),
    /// An image in the line (an icon); images standing alone are figures.
    Image(Image),
    /// A footnote, with its note's blocks.
    Footnote(Vec<Block>),
    /// A citation of one or more sources.
    Cite(Cite),
    /// A cross-reference to a label of the document ("Figure 2").
    Ref {
        label: String,
        /// The reference as typeset, if resolved.
        text: Option<Vec<Inline>>,
    },
}

/// A citation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cite {
    /// The sources cited, in order.
    pub items: Vec<CiteItem>,
    /// How it reads: as typeset (exports) or as written in the source
    /// (imports, e.g. `[@doe, p. 4]`).
    pub text: Option<Vec<Inline>>,
}

/// One source of a citation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CiteItem {
    /// The source's key (`doe2020`).
    pub key: String,
    /// Where in it ("p. 4"), if given.
    pub locator: Option<String>,
}
