//! The layout layer: structured layout settings (a preset plus the keys the
//! user overrode), their validation, and the Typst code they generate.
//!
//! Settings are stored flat, under dotted keys such as `"page.size"` (see
//! [`flatten`]), next to `"preset"` (a preset id) and `"schema"`. Stored
//! values are untrusted (documents are shared): [`resolve`] validates each one
//! again and ignores what does not pass.
//!
//! # Safety of the generated code
//!
//! Nothing from the settings is interpolated into Typst code as written:
//! choices are Rust enums, lengths are re-printed from a parsed number and a
//! fixed unit, numbers are integers, and every text (font names, language,
//! titles, ...) is emitted as an escaped Typst string literal ([`typst_str`]).

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::ops::{Range, RangeInclusive};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use typst::text::{FontBook, FontVariant};

use crate::fonts::{FONTS, SYSTEM_DOC_FONTS};

/// Version of the settings schema.
pub const SCHEMA: u32 = 1;
/// The preset used when a document names none (or an unknown one).
pub const DEFAULT_PRESET: &str = "paper";
/// The storage key of the preset id.
pub const PRESET_KEY: &str = "preset";
/// The storage key of the schema version.
pub const SCHEMA_KEY: &str = "schema";
/// The section of the character styles in a [`Layout`].
const STYLES: &str = "styles";

/// Flat settings as stored: dotted key to JSON scalar.
pub type Entries = BTreeMap<String, Value>;

/// A resolved layout: every setting has a value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    pub schema: u32,
    pub page: PageSettings,
    pub text: TextSettings,
    pub headings: HeadingSettings,
    pub figures: FigureSettings,
    pub tables: TableSettings,
    pub equations: EquationSettings,
    pub citations: CitationSettings,
    pub furniture: FurnitureSettings,
    pub front: FrontSettings,
    pub export: ExportSettings,
    /// Character styles by id, stored as `styles.<id>.<field>` (see
    /// [`style_key`]).
    #[serde(default)]
    pub styles: BTreeMap<String, CharStyle>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageSettings {
    pub size: PageSize,
    /// Page width and height, used when `size` is `custom`.
    pub width: Length,
    pub height: Length,
    pub margin_top: Length,
    pub margin_bottom: Length,
    /// The binding side (left on one-sided pages).
    pub margin_inside: Length,
    pub margin_outside: Length,
    /// Inside/outside margins and page furniture alternate between pages.
    pub two_sided: bool,
    /// 1 or 2.
    pub columns: u8,
    pub column_gap: Length,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PageSize {
    A4,
    A5,
    UsLetter,
    /// 6 x 9 in.
    UsTrade,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextSettings {
    /// A family from [`fonts`].
    pub font: String,
    pub size: Length,
    /// Space between lines.
    pub leading: Length,
    pub justify: bool,
    pub hyphenate: bool,
    /// ISO 639-1 language, optionally with an ISO 3166 region (`en-US`).
    pub lang: String,
    /// Indent of every paragraph's first line except the first after a
    /// heading.
    pub first_line_indent: Length,
    /// Space between paragraphs.
    pub paragraph_spacing: Length,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeadingSettings {
    pub numbering: HeadingNumbering,
    pub chapter_pagebreak: ChapterBreak,
    pub level1_size: Length,
    pub level2_size: Length,
    /// Level 3 and deeper.
    pub level3_size: Length,
    /// Space above and below headings of every level; `None`: Typst's
    /// default, which depends on the level. `em` is relative to the
    /// heading's size.
    #[serde(with = "optional_length")]
    pub space_above: Option<Length>,
    #[serde(with = "optional_length")]
    pub space_below: Option<Length>,
}

/// Heading numbering. All but `1.1.` number only level-1 headings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HeadingNumbering {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "1.")]
    Arabic,
    #[serde(rename = "1.1.")]
    Hierarchical,
    #[serde(rename = "I.")]
    Roman,
    #[serde(rename = "A.")]
    Letter,
}

/// Where level-1 headings start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChapterBreak {
    /// Where they fall.
    None,
    /// On a new page.
    Page,
    /// On a new right-hand page (two-sided; a new page otherwise).
    Right,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FigureSettings {
    pub caption_position: CaptionPosition,
    pub numbering: bool,
    /// Default image width in percent of the column width (10-100).
    pub default_width: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaptionPosition {
    Top,
    Bottom,
}

/// Tables (`table(...)` calls, in a figure or not).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableSettings {
    /// The look of every table without a style of its own (an exception).
    pub style: TableStyle,
    /// Header cells (`table.header(...)`) in bold.
    pub header_bold: bool,
    /// Where table figures' captions go (other figures: `figures`).
    pub caption_position: CaptionPosition,
}

/// How a table looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TableStyle {
    /// No lines.
    Plain,
    /// Booktabs-like: a thick rule above and below the table, a thin one
    /// under the header; no vertical lines.
    Academic,
    /// Thin lines around every cell.
    Grid,
    /// No lines; every second row lightly filled (never the first).
    Striped,
}

impl TableStyle {
    /// The stored name, also its key in the generated `lethora-tables`.
    pub fn name(self) -> &'static str {
        match self {
            TableStyle::Plain => "plain",
            TableStyle::Academic => "academic",
            TableStyle::Grid => "grid",
            TableStyle::Striped => "striped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EquationSettings {
    pub numbering: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CitationSettings {
    /// A style id from [`crate::styles`].
    pub style: String,
    /// Empty: the language's default ("Bibliography").
    pub bibliography_title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FurnitureSettings {
    pub page_numbers: PageNumbers,
    pub running_head: RunningHead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PageNumbers {
    None,
    BottomCenter,
    /// Bottom right; alternating on two-sided pages.
    BottomOutside,
    /// Top right; alternating on two-sided pages.
    TopOutside,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunningHead {
    None,
    /// The document title.
    Title,
    /// The current chapter (level-1 heading); on two-sided pages the title
    /// on left-hand pages.
    Chapter,
    /// "Author / Title", followed by the page number when that is at the top
    /// (manuscript style).
    AuthorTitle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrontSettings {
    /// A separate title page (needs a title, subtitle or author).
    pub title_page: bool,
    /// A table of contents.
    pub toc: bool,
    /// Heading levels in the contents (1-4).
    pub toc_depth: u8,
    /// Empty: the language's default ("Contents").
    pub toc_title: String,
    /// Dot leaders between entries and page numbers.
    pub toc_dots: bool,
    /// Entries indented by level.
    pub toc_indent: bool,
    /// Space between entries; `None`: the line spacing.
    #[serde(with = "optional_length")]
    pub toc_spacing: Option<Length>,
    /// Level-1 entries in bold.
    pub toc_bold_top: bool,
}

/// PDF export. Changes only the exported file: never the generated code,
/// the preview or the layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportSettings {
    /// Resolution of the raster images in the PDF (see [`crate::images`]).
    pub image_quality: ImageQuality,
}

/// The resolution raster images are downscaled to at PDF export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageQuality {
    /// The image files as they are.
    Original,
    /// 300 dpi.
    Print,
    /// 150 dpi.
    Screen,
    /// 96 dpi.
    Email,
}

/// A character style: a font and/or size for marked text in the content
/// (`#styled("<id>")[...]`). A resolved style sets at least one of them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharStyle {
    /// Display name; `None`: an unnamed style (the UI names it from its
    /// settings).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A font family, possibly one this machine lacks (see
    /// [`is_family_name`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    /// Absolute, like `text.size`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<Length>,
}

impl ImageQuality {
    /// Pixels per inch of the placed image; `None`: unchanged.
    pub fn dpi(self) -> Option<f64> {
        match self {
            ImageQuality::Original => None,
            ImageQuality::Print => Some(300.0),
            ImageQuality::Screen => Some(150.0),
            ImageQuality::Email => Some(96.0),
        }
    }
}

/// Document metadata. Empty strings are unset.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Meta {
    pub title: String,
    pub subtitle: String,
    /// One or more authors, as written.
    pub author: String,
    /// Free text, as it should appear.
    pub date: String,
    /// Contact details for a manuscript title page (several lines).
    pub contact: String,
}

/// The metadata keys, see [`Meta`].
pub const META_KEYS: &[&str] = &["title", "subtitle", "author", "date", "contact"];

// ---- Lengths ----

/// A length as the user wrote it: a non-negative number and a unit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Length {
    value: f64,
    unit: Unit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Pt,
    Mm,
    Cm,
    In,
    /// Relative to the text size.
    Em,
}

impl Unit {
    fn name(self) -> &'static str {
        match self {
            Unit::Pt => "pt",
            Unit::Mm => "mm",
            Unit::Cm => "cm",
            Unit::In => "in",
            Unit::Em => "em",
        }
    }

    /// Points per unit; `None` for `em`.
    fn points(self) -> Option<f64> {
        match self {
            Unit::Pt => Some(1.0),
            Unit::Mm => Some(72.0 / 25.4),
            Unit::Cm => Some(72.0 / 2.54),
            Unit::In => Some(72.0),
            Unit::Em => None,
        }
    }
}

impl Length {
    const MAX_VALUE: f64 = 10_000.0;

    /// Parses `<number><unit>` (e.g. `12pt`, `2.5cm`, `0.65em`): digits with
    /// at most one decimal point, no sign or exponent.
    pub fn parse(s: &str) -> Result<Self, String> {
        let bad = || {
            format!(
                "\"{}\" is not a length (like 12pt, 2.5cm, 1in or 0.6em).",
                clip(s)
            )
        };
        let s = s.trim();
        let split = s
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .ok_or_else(bad)?;
        let (number, unit) = s.split_at(split);
        let unit = match unit.to_ascii_lowercase().as_str() {
            "pt" => Unit::Pt,
            "mm" => Unit::Mm,
            "cm" => Unit::Cm,
            "in" => Unit::In,
            "em" => Unit::Em,
            _ => return Err(bad()),
        };
        let digits = number.bytes().filter(u8::is_ascii_digit).count();
        if digits == 0
            || number.len() > 9
            || number.matches('.').count() > 1
            || number.starts_with('.')
            || number.ends_with('.')
        {
            return Err(bad());
        }
        let value: f64 = number.parse().map_err(|_| bad())?;
        if !value.is_finite() || !(0.0..=Self::MAX_VALUE).contains(&value) {
            return Err(bad());
        }
        Ok(Self { value, unit })
    }

    pub fn value(self) -> f64 {
        self.value
    }

    pub fn unit(self) -> Unit {
        self.unit
    }

    /// The length in points; `None` for `em`.
    pub fn to_pt(self) -> Option<f64> {
        self.unit.points().map(|p| self.value * p)
    }

    fn new(value: f64, unit: Unit) -> Self {
        Self { value, unit }
    }
}

impl fmt::Display for Length {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // A parsed, finite, non-negative number: Rust prints plain digits.
        let value = if self.value.is_finite() && self.value >= 0.0 {
            self.value.min(Self::MAX_VALUE)
        } else {
            0.0
        };
        write!(f, "{value}{}", self.unit.name())
    }
}

impl TryFrom<String> for Length {
    type Error = String;

    fn try_from(s: String) -> Result<Self, String> {
        Self::parse(&s)
    }
}

impl From<Length> for String {
    fn from(l: Length) -> String {
        l.to_string()
    }
}

/// An optional [`Length`] stored as text: empty for `None`.
mod optional_length {
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};

    use super::Length;

    pub fn serialize<S: Serializer>(value: &Option<Length>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&value.map(String::from).unwrap_or_default())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Length>, D::Error> {
        let s = String::deserialize(d)?;
        if s.is_empty() {
            return Ok(None);
        }
        Length::parse(&s).map(Some).map_err(D::Error::custom)
    }
}

fn pt(v: f64) -> Length {
    Length::new(v, Unit::Pt)
}
fn mm(v: f64) -> Length {
    Length::new(v, Unit::Mm)
}
fn inch(v: f64) -> Length {
    Length::new(v, Unit::In)
}
fn em(v: f64) -> Length {
    Length::new(v, Unit::Em)
}

// ---- Presets ----

/// A named starting point for a layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PresetInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
}

const PRESETS: &[PresetInfo] = &[
    PresetInfo {
        id: "paper",
        name: "Scientific paper",
        description: "A4, compact, numbered sections, equations and figures, bibliography.",
    },
    PresetInfo {
        id: "novel",
        name: "Novel / trade book",
        description: "6 × 9 in, justified serif text, indented paragraphs, chapters on right-hand pages, running heads.",
    },
    PresetInfo {
        id: "thesis",
        name: "Thesis / report",
        description: "A4 two-sided with a binding gutter, title page, contents, numbered chapters on new pages.",
    },
    PresetInfo {
        id: "manuscript",
        name: "Manuscript submission",
        description: "US Letter, 12 pt, double-spaced, ragged right, 1 in margins, title page, Author / Title / page header.",
    },
];

/// The available presets.
pub fn presets() -> Vec<PresetInfo> {
    PRESETS.to_vec()
}

/// The full layout of preset `id`.
pub fn preset(id: &str) -> Option<Layout> {
    let paper = Layout {
        schema: SCHEMA,
        page: PageSettings {
            size: PageSize::A4,
            width: mm(210.0),
            height: mm(297.0),
            margin_top: mm(25.0),
            margin_bottom: mm(25.0),
            margin_inside: mm(30.0),
            margin_outside: mm(30.0),
            two_sided: false,
            columns: 1,
            column_gap: mm(6.0),
        },
        text: TextSettings {
            font: "New Computer Modern".into(),
            size: pt(11.0),
            leading: em(0.6),
            justify: true,
            hyphenate: true,
            lang: "en".into(),
            first_line_indent: em(1.5),
            paragraph_spacing: em(0.6),
        },
        headings: HeadingSettings {
            numbering: HeadingNumbering::Hierarchical,
            chapter_pagebreak: ChapterBreak::None,
            level1_size: pt(14.0),
            level2_size: pt(12.0),
            level3_size: pt(11.0),
            space_above: None,
            space_below: None,
        },
        figures: FigureSettings {
            caption_position: CaptionPosition::Bottom,
            numbering: true,
            default_width: 80,
        },
        tables: TableSettings {
            style: TableStyle::Academic,
            header_bold: true,
            caption_position: CaptionPosition::Top,
        },
        equations: EquationSettings { numbering: true },
        citations: CitationSettings {
            style: "apa".into(),
            bibliography_title: String::new(),
        },
        furniture: FurnitureSettings {
            page_numbers: PageNumbers::BottomCenter,
            running_head: RunningHead::None,
        },
        front: FrontSettings {
            title_page: false,
            toc: false,
            toc_depth: 3,
            toc_title: String::new(),
            toc_dots: true,
            toc_indent: true,
            toc_spacing: None,
            toc_bold_top: false,
        },
        export: ExportSettings {
            image_quality: ImageQuality::Original,
        },
        // Presets may define (named) styles; none ship yet.
        styles: BTreeMap::new(),
    };
    Some(match id {
        "paper" => paper,
        "novel" => Layout {
            page: PageSettings {
                size: PageSize::UsTrade,
                width: inch(6.0),
                height: inch(9.0),
                margin_top: inch(0.75),
                margin_bottom: inch(0.85),
                margin_inside: inch(0.875),
                margin_outside: inch(0.625),
                two_sided: true,
                ..paper.page
            },
            text: TextSettings {
                font: "Libertinus Serif".into(),
                size: pt(11.0),
                leading: em(0.65),
                first_line_indent: em(1.2),
                paragraph_spacing: em(0.65),
                ..paper.text
            },
            headings: HeadingSettings {
                numbering: HeadingNumbering::None,
                chapter_pagebreak: ChapterBreak::Right,
                level1_size: pt(20.0),
                level2_size: pt(14.0),
                level3_size: pt(11.0),
                ..paper.headings
            },
            figures: FigureSettings {
                default_width: 100,
                ..paper.figures
            },
            tables: TableSettings {
                style: TableStyle::Plain,
                header_bold: true,
                caption_position: CaptionPosition::Bottom,
            },
            equations: EquationSettings { numbering: false },
            citations: CitationSettings {
                style: "chicago-author-date".into(),
                ..paper.citations
            },
            furniture: FurnitureSettings {
                page_numbers: PageNumbers::TopOutside,
                running_head: RunningHead::Chapter,
            },
            front: FrontSettings {
                title_page: true,
                toc: false,
                ..paper.front
            },
            ..paper
        },
        "thesis" => Layout {
            page: PageSettings {
                margin_top: mm(30.0),
                margin_bottom: mm(30.0),
                margin_inside: mm(35.0),
                margin_outside: mm(25.0),
                two_sided: true,
                ..paper.page
            },
            text: TextSettings {
                font: "Libertinus Serif".into(),
                size: pt(11.0),
                leading: em(0.8),
                first_line_indent: pt(0.0),
                paragraph_spacing: em(1.1),
                ..paper.text
            },
            headings: HeadingSettings {
                numbering: HeadingNumbering::Hierarchical,
                chapter_pagebreak: ChapterBreak::Right,
                level1_size: pt(22.0),
                level2_size: pt(15.0),
                level3_size: pt(12.0),
                ..paper.headings
            },
            furniture: FurnitureSettings {
                page_numbers: PageNumbers::BottomOutside,
                running_head: RunningHead::Chapter,
            },
            front: FrontSettings {
                title_page: true,
                toc: true,
                ..paper.front
            },
            ..paper
        },
        "manuscript" => Layout {
            page: PageSettings {
                size: PageSize::UsLetter,
                width: inch(8.5),
                height: inch(11.0),
                margin_top: inch(1.0),
                margin_bottom: inch(1.0),
                margin_inside: inch(1.0),
                margin_outside: inch(1.0),
                ..paper.page
            },
            text: TextSettings {
                font: "Libertinus Serif".into(),
                size: pt(12.0),
                leading: em(1.6),
                justify: false,
                hyphenate: false,
                lang: "en-US".into(),
                first_line_indent: inch(0.5),
                paragraph_spacing: em(1.6),
            },
            headings: HeadingSettings {
                numbering: HeadingNumbering::None,
                chapter_pagebreak: ChapterBreak::Page,
                level1_size: pt(12.0),
                level2_size: pt(12.0),
                level3_size: pt(12.0),
                ..paper.headings
            },
            figures: FigureSettings {
                default_width: 100,
                ..paper.figures
            },
            equations: EquationSettings { numbering: false },
            citations: CitationSettings {
                style: "chicago-author-date".into(),
                ..paper.citations
            },
            furniture: FurnitureSettings {
                page_numbers: PageNumbers::TopOutside,
                running_head: RunningHead::AuthorTitle,
            },
            front: FrontSettings {
                title_page: true,
                toc: false,
                ..paper.front
            },
            ..paper
        },
        _ => return None,
    })
}

// ---- Validation ----

/// What a setting accepts.
enum Kind {
    /// A choice, checked by deserializing into the enum.
    Choice(fn(&Value) -> bool),
    Length {
        /// Allowed range in points for absolute units.
        pt: RangeInclusive<f64>,
        /// Allowed range for `em`, if allowed at all.
        em: Option<RangeInclusive<f64>>,
    },
    /// A [`Kind::Length`] or empty (the default).
    OptionalLength {
        pt: RangeInclusive<f64>,
        em: Option<RangeInclusive<f64>>,
    },
    Bool,
    Columns,
    /// Contents depth: 1 to 4.
    Depth,
    Percent,
    Font,
    /// A font family that need not be in the font list (a character
    /// style's; see [`is_family_name`]).
    FamilyName,
    Lang,
    Style,
    Text(usize),
    /// A display name: trimmed, 1 to this many characters, no control
    /// characters.
    Name(usize),
}

/// The fields of a character style, see [`style_key`].
const STYLE_FIELDS: &[&str] = &["name", "font", "size", "removed"];
/// Longest accepted character style id.
const MAX_STYLE_ID_LEN: usize = 48;

/// The style id and field of a character style key, `styles.<id>.<field>`:
/// `font`, `size` (absolute), `name` (present: a named style) or `removed`
/// (`true` hides a preset's style). `None` for other keys and for invalid
/// ids or fields.
pub fn style_key(key: &str) -> Option<(&str, &str)> {
    let (id, field) = key.strip_prefix("styles.")?.split_once('.')?;
    (is_style_id(id) && STYLE_FIELDS.contains(&field)).then_some((id, field))
}

/// The key of `field` of character style `id`.
pub fn style_key_of(id: &str, field: &str) -> String {
    format!("styles.{id}.{field}")
}

/// Whether `id` is a character style id: a readable slug of lowercase ASCII
/// letters and digits joined by single hyphens (`eb-garamond-14pt`), at most
/// 48 characters. Ids never change once used.
pub fn is_style_id(id: &str) -> bool {
    id.len() <= MAX_STYLE_ID_LEN
        && id.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

fn is<T: DeserializeOwned>(value: &Value) -> bool {
    serde_json::from_value::<T>(value.clone()).is_ok()
}

fn kind(key: &str) -> Option<Kind> {
    let len = |pt: RangeInclusive<f64>, em: Option<RangeInclusive<f64>>| Kind::Length { pt, em };
    if let Some((_, field)) = style_key(key) {
        return Some(match field {
            "font" => Kind::FamilyName,
            // As `text.size`.
            "size" => len(4.0..=72.0, None),
            "name" => Kind::Name(64),
            "removed" => Kind::Bool,
            _ => return None,
        });
    }
    Some(match key {
        "page.size" => Kind::Choice(is::<PageSize>),
        "page.width" | "page.height" => len(72.0..=2880.0, None),
        "page.margin_top" | "page.margin_bottom" | "page.margin_inside" | "page.margin_outside" => {
            len(0.0..=288.0, None)
        }
        "page.two_sided" => Kind::Bool,
        "page.columns" => Kind::Columns,
        "page.column_gap" => len(0.0..=72.0, Some(0.0..=5.0)),
        "text.font" => Kind::Font,
        "text.size" => len(4.0..=72.0, None),
        "text.leading" => len(0.0..=72.0, Some(0.0..=5.0)),
        "text.justify" | "text.hyphenate" => Kind::Bool,
        "text.lang" => Kind::Lang,
        "text.first_line_indent" | "text.paragraph_spacing" => len(0.0..=144.0, Some(0.0..=10.0)),
        "headings.numbering" => Kind::Choice(is::<HeadingNumbering>),
        "headings.chapter_pagebreak" => Kind::Choice(is::<ChapterBreak>),
        "headings.level1_size" | "headings.level2_size" | "headings.level3_size" => {
            len(4.0..=96.0, Some(0.5..=5.0))
        }
        "headings.space_above" | "headings.space_below" => Kind::OptionalLength {
            pt: 0.0..=144.0,
            em: Some(0.0..=10.0),
        },
        "figures.caption_position" => Kind::Choice(is::<CaptionPosition>),
        "figures.numbering" | "equations.numbering" => Kind::Bool,
        "figures.default_width" => Kind::Percent,
        "tables.style" => Kind::Choice(is::<TableStyle>),
        "tables.header_bold" => Kind::Bool,
        "tables.caption_position" => Kind::Choice(is::<CaptionPosition>),
        "citations.style" => Kind::Style,
        "citations.bibliography_title" => Kind::Text(200),
        "furniture.page_numbers" => Kind::Choice(is::<PageNumbers>),
        "furniture.running_head" => Kind::Choice(is::<RunningHead>),
        "front.title_page" | "front.toc" | "front.toc_dots" | "front.toc_indent"
        | "front.toc_bold_top" => Kind::Bool,
        "front.toc_depth" => Kind::Depth,
        "front.toc_title" => Kind::Text(200),
        "front.toc_spacing" => Kind::OptionalLength {
            pt: 0.0..=72.0,
            em: Some(0.0..=5.0),
        },
        "export.image_quality" => Kind::Choice(is::<ImageQuality>),
        _ => return None,
    })
}

/// Checks a new value for the layout setting `key` and returns it
/// normalized (e.g. `" 12PT"` → `"12pt"`, `"en-us"` → `"en-US"`). Fonts must
/// be embedded ones; see [`validate_change_in`].
pub fn validate_change(key: &str, value: &Value) -> Result<Value, String> {
    validate_change_in(key, value, &fonts())
}

/// [`validate_change`], accepting any font family in `fonts` (e.g. from
/// [`system_fonts`]).
pub fn validate_change_in(key: &str, value: &Value, fonts: &[String]) -> Result<Value, String> {
    let kind = kind(key).ok_or_else(|| {
        if key.starts_with("styles.") {
            format!(
                "\"{}\" is not a character style setting (styles.<id>.font, size, name or \
                 removed, with an id like eb-garamond-14pt).",
                clip(key)
            )
        } else {
            format!("Unknown layout setting \"{}\".", clip(key))
        }
    })?;
    let string = || {
        value
            .as_str()
            .ok_or_else(|| format!("{key}: expected text, got {}.", describe(value)))
    };
    match kind {
        Kind::Choice(check) => {
            if check(value) {
                Ok(value.clone())
            } else {
                Err(format!(
                    "{key}: {} is not one of the choices.",
                    describe(value)
                ))
            }
        }
        Kind::Length { pt, em } => {
            check_length(key, string()?, pt, em).map(|l| Value::String(l.to_string()))
        }
        Kind::OptionalLength { pt, em } => match string()? {
            s if s.trim().is_empty() => Ok(Value::String(String::new())),
            s => check_length(key, s, pt, em).map(|l| Value::String(l.to_string())),
        },
        Kind::Bool => value
            .as_bool()
            .map(Value::Bool)
            .ok_or_else(|| format!("{key}: expected true or false, got {}.", describe(value))),
        Kind::Columns => match value.as_f64() {
            Some(n) if n == 1.0 || n == 2.0 => Ok(Value::from(n as u8)),
            _ => Err(format!("{key}: expected 1 or 2, got {}.", describe(value))),
        },
        Kind::Depth => match value.as_f64() {
            Some(n) if (1.0..=4.0).contains(&n) && n.fract() == 0.0 => Ok(Value::from(n as u8)),
            _ => Err(format!(
                "{key}: expected a whole number from 1 to 4, got {}.",
                describe(value)
            )),
        },
        Kind::Percent => match value.as_f64() {
            Some(n) if (10.0..=100.0).contains(&n) => Ok(Value::from(n.round() as u8)),
            _ => Err(format!(
                "{key}: expected a percentage from 10 to 100, got {}.",
                describe(value)
            )),
        },
        Kind::Font => {
            let name = string()?;
            fonts
                .iter()
                .find(|f| f.eq_ignore_ascii_case(name.trim()))
                .map(|f| Value::String(f.clone()))
                .ok_or_else(|| format!("{key}: the font \"{}\" is not available.", clip(name)))
        }
        Kind::FamilyName => {
            let name = string()?.trim();
            match fonts.iter().find(|f| f.eq_ignore_ascii_case(name)) {
                Some(f) => Ok(Value::String(f.clone())),
                None if is_family_name(name) => Ok(Value::String(name.to_owned())),
                None => Err(format!(
                    "{key}: \"{}\" is not a font family name.",
                    clip(name)
                )),
            }
        }
        Kind::Lang => normalize_lang(string()?).map(Value::String).ok_or_else(|| {
            format!(
                "{key}: \"{}\" is not a language code (like en, de or en-US).",
                clip(string().unwrap_or_default())
            )
        }),
        Kind::Style => {
            let id = string()?;
            crate::styles()
                .into_iter()
                .find(|(s, _)| *s == id)
                .map(|(s, _)| Value::String(s.to_owned()))
                .ok_or_else(|| format!("{key}: unknown citation style \"{}\".", clip(id)))
        }
        Kind::Text(max) => check_text(key, string()?, max, false).map(Value::String),
        Kind::Name(max) => {
            let name = string()?.trim();
            if name.is_empty() || name.chars().count() > max || name.chars().any(char::is_control) {
                Err(format!(
                    "{key}: expected a name of 1 to {max} characters, without line breaks or \
                     control characters."
                ))
            } else {
                Ok(Value::String(name.to_owned()))
            }
        }
    }
}

/// Parses the length `text` for `key` and checks it against `pt` (absolute
/// units, in points) and `em` (`None`: no `em` allowed).
pub fn check_length(
    key: &str,
    text: &str,
    pt: RangeInclusive<f64>,
    em: Option<RangeInclusive<f64>>,
) -> Result<Length, String> {
    let length = Length::parse(text).map_err(|e| format!("{key}: {e}"))?;
    let ok = match (length.to_pt(), &em) {
        (Some(p), _) => pt.contains(&p),
        (None, Some(range)) => range.contains(&length.value),
        (None, None) => {
            return Err(format!("{key}: use an absolute unit (pt, mm, cm or in)."));
        }
    };
    if !ok {
        let ems = em.map_or(String::new(), |r| {
            format!(" or {}em to {}em", r.start(), r.end())
        });
        return Err(format!(
            "{key}: {length} is out of range ({}pt to {}pt{ems}).",
            pt.start(),
            pt.end()
        ));
    }
    Ok(length)
}

/// Checks a new value for the metadata field `key` (see [`META_KEYS`]).
pub fn validate_meta(key: &str, value: &Value) -> Result<Value, String> {
    if !META_KEYS.contains(&key) {
        return Err(format!("Unknown metadata field \"{}\".", clip(key)));
    }
    let text = value
        .as_str()
        .ok_or_else(|| format!("{key}: expected text, got {}.", describe(value)))?;
    let (max, multiline) = if key == "contact" {
        (2000, true)
    } else {
        (500, false)
    };
    check_text(key, text, max, multiline).map(Value::String)
}

fn check_text(key: &str, text: &str, max: usize, multiline: bool) -> Result<String, String> {
    if text.chars().count() > max {
        return Err(format!("{key}: longer than {max} characters."));
    }
    let text = text.replace("\r\n", "\n");
    if text
        .chars()
        .any(|c| c.is_control() && !(multiline && c == '\n') && c != '\t')
    {
        return Err(format!(
            "{key}: contains {}.",
            if multiline {
                "control characters"
            } else {
                "line breaks or control characters"
            }
        ));
    }
    Ok(text)
}

/// `ll` or `ll-RR` (2-3 letter language, 2 letter region), case-normalized.
fn normalize_lang(s: &str) -> Option<String> {
    let s = s.trim();
    let (lang, region) = match s.split_once(['-', '_']) {
        Some((l, r)) => (l, Some(r)),
        None => (s, None),
    };
    let letters = |t: &str, n: Range<usize>| {
        n.contains(&t.len()) && t.bytes().all(|b| b.is_ascii_alphabetic())
    };
    if !letters(lang, 2..4) || !region.is_none_or(|r| letters(r, 2..3)) {
        return None;
    }
    let mut out = lang.to_ascii_lowercase();
    if let Some(r) = region {
        out.push('-');
        out.push_str(&r.to_ascii_uppercase());
    }
    Some(out)
}

fn describe(value: &Value) -> String {
    match value {
        Value::String(s) => format!("\"{}\"", clip(s)),
        Value::Null => "nothing".into(),
        v => clip(&v.to_string()),
    }
}

/// At most 40 characters of `s`, for error messages.
fn clip(s: &str) -> String {
    let mut out: String = s.chars().take(40).collect();
    if s.chars().nth(40).is_some() {
        out.push('…');
    }
    out
}

// ---- Resolving stored settings ----

/// The preset named in `entries`, or [`DEFAULT_PRESET`].
pub fn preset_id(entries: &Entries) -> &'static str {
    entries
        .get(PRESET_KEY)
        .and_then(Value::as_str)
        .and_then(|id| PRESETS.iter().find(|p| p.id == id))
        .map_or(DEFAULT_PRESET, |p| p.id)
}

/// The resolved layout of stored `entries`: the preset's values, overridden
/// by every stored key that passes [`validate_change`] (a font: or
/// [`is_family_name`]). Others are ignored.
///
/// Character styles ([`style_key`]) are the preset's plus stored ones,
/// merged per field; a style marked `removed` and one with neither a font
/// nor a size (e.g. its only font is invalid) are left out.
pub fn resolve(entries: &Entries) -> Layout {
    resolve_on(preset(preset_id(entries)).expect("known preset"), entries)
}

/// [`resolve`] with the preset layout `base`.
fn resolve_on(base: Layout, entries: &Entries) -> Layout {
    let mut flat = flatten(&base);
    let fonts = fonts();
    for (key, value) in entries {
        if !(flat.contains_key(key) || style_key(key).is_some()) || key == SCHEMA_KEY {
            continue;
        }
        let value = match validate_change_in(key, value, &fonts) {
            Ok(value) => value,
            // A font this machine may lack, e.g. a system font.
            Err(_) if key == "text.font" && value.as_str().is_some_and(is_family_name) => {
                value.clone()
            }
            Err(_) => continue,
        };
        flat.insert(key.clone(), value);
    }
    serde_json::from_value(unflatten(&flat)).unwrap_or(base)
}

/// The settings of `layout` that differ from preset `preset`'s; a preset's
/// character style the layout lacks is `styles.<id>.removed: true`.
pub fn diff(preset_id: &str, layout: &Layout) -> Entries {
    diff_from(
        &preset(preset_id).unwrap_or_else(|| preset(DEFAULT_PRESET).unwrap()),
        layout,
    )
}

/// [`diff`] against the preset layout `base`. (A field of a preset style
/// that the layout's style lacks cannot be stored and is left out.)
fn diff_from(base: &Layout, layout: &Layout) -> Entries {
    let flat = flatten(base);
    let mut out: Entries = flatten(layout)
        .into_iter()
        .filter(|(k, v)| k != SCHEMA_KEY && flat.get(k) != Some(v))
        .collect();
    for id in base.styles.keys() {
        if !layout.styles.contains_key(id) {
            out.insert(style_key_of(id, "removed"), Value::Bool(true));
        }
    }
    out
}

/// The metadata in stored `entries`; invalid fields are left empty.
pub fn resolve_meta(entries: &Entries) -> Meta {
    let get = |key: &str| {
        entries
            .get(key)
            .and_then(|v| validate_meta(key, v).ok())
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    };
    Meta {
        title: get("title"),
        subtitle: get("subtitle"),
        author: get("author"),
        date: get("date"),
        contact: get("contact"),
    }
}

/// `layout` as flat dotted keys (`"page.size"`, `"styles.<id>.font"`), plus
/// `"schema"`.
pub fn flatten(layout: &Layout) -> Entries {
    let mut out = Entries::new();
    if let Ok(Value::Object(sections)) = serde_json::to_value(layout) {
        for (section, value) in sections {
            match value {
                Value::Object(styles) if section == STYLES => {
                    for (id, style) in styles {
                        if let Value::Object(fields) = style {
                            for (field, v) in fields {
                                out.insert(style_key_of(&id, &field), v);
                            }
                        }
                    }
                }
                Value::Object(fields) => {
                    for (field, v) in fields {
                        out.insert(format!("{section}.{field}"), v);
                    }
                }
                v => {
                    out.insert(section, v);
                }
            }
        }
    }
    out
}

fn unflatten(flat: &Entries) -> Value {
    let mut root = Map::new();
    let mut styles: BTreeMap<&str, Map<String, Value>> = BTreeMap::new();
    for (key, value) in flat {
        if let Some((id, field)) = style_key(key) {
            styles
                .entry(id)
                .or_default()
                .insert(field.to_owned(), value.clone());
            continue;
        }
        match key.split_once('.') {
            Some((section, field)) => {
                let entry = root
                    .entry(section.to_owned())
                    .or_insert_with(|| Value::Object(Map::new()));
                if let Value::Object(fields) = entry {
                    fields.insert(field.to_owned(), value.clone());
                }
            }
            None => {
                root.insert(key.clone(), value.clone());
            }
        }
    }
    // Removed styles are left out, and so are styles that set neither a
    // font nor a size.
    let styles = styles
        .into_iter()
        .filter_map(|(id, mut style)| {
            let removed = style.remove("removed") == Some(Value::Bool(true));
            (!removed && (style.contains_key("font") || style.contains_key("size")))
                .then(|| (id.to_owned(), Value::Object(style)))
        })
        .collect();
    root.insert(STYLES.to_owned(), Value::Object(styles));
    Value::Object(root)
}

/// The embedded text font families (math fonts excluded).
pub fn fonts() -> Vec<String> {
    families(&FONTS.book)
}

/// The font file of embedded family `family`'s regular face (any case; the
/// nearest variant without one), for the editor to show text in it (the web
/// view lacks the embedded fonts); `None` for other families.
pub fn embedded_font(family: &str) -> Option<&'static [u8]> {
    let fonts = &*FONTS;
    let index = fonts
        .book
        .select(&family.to_lowercase(), FontVariant::default())?;
    let font = fonts.fonts.get(index)?;
    // A face inside a collection is no font file of its own.
    (font.index() == 0).then(|| font.data().as_slice())
}

/// The text font families of the embedded and the system's fonts, and how
/// long searching the system took (once per process; later calls are free).
/// Families whose names [`is_family_name`] rejects are left out.
pub fn system_fonts() -> (Vec<String>, std::time::Duration) {
    let fonts = &*SYSTEM_DOC_FONTS;
    (families(fonts.store.book()), fonts.scan)
}

fn families(book: &FontBook) -> Vec<String> {
    // The embedded text fonts carry MATH tables too, so go by name.
    let mut out: Vec<String> = book
        .families()
        .map(|(name, _)| name.to_owned())
        .filter(|name| !name.ends_with(" Math") && is_family_name(name))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Whether `name` looks like a font family name: 1-100 characters, letters,
/// digits, spaces and `-_.'&+`. A stored font setting that passes is kept
/// even if this machine lacks the font (Typst then warns and falls back), so
/// a system font chosen elsewhere survives; it is always emitted escaped.
pub fn is_family_name(name: &str) -> bool {
    (1..=100).contains(&name.chars().count())
        && name.trim() == name
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || " -_.'&+".contains(c))
}

// ---- Text area ----

/// The text area of a layout in points: content views draw relative sizes
/// (e.g. a figure's `width: 80%` or `5cm`) against it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct TextArea {
    /// Width of one text column (page width minus margins; with two columns,
    /// half of what remains after the gap).
    pub column_width_pt: f64,
    /// Height of the text area (page height minus margins).
    pub height_pt: f64,
    /// Body text size.
    pub font_size_pt: f64,
}

/// The page's width and height of `layout`.
pub fn page_size(layout: &Layout) -> (Length, Length) {
    let p = &layout.page;
    match p.size {
        PageSize::A4 => (mm(210.0), mm(297.0)),
        PageSize::A5 => (mm(148.0), mm(210.0)),
        PageSize::UsLetter => (inch(8.5), inch(11.0)),
        PageSize::UsTrade => (inch(6.0), inch(9.0)),
        PageSize::Custom => (p.width, p.height),
    }
}

/// The text area of `layout`, as the generated `#set page(...)` makes it.
pub fn text_area(layout: &Layout) -> TextArea {
    let p = &layout.page;
    let (width, height) = page_size(layout);
    // `text.size` is always absolute; an em length (the column gap) is relative to it.
    let font = layout.text.size.to_pt().unwrap_or(11.0);
    let abs = |l: Length| l.to_pt().unwrap_or(l.value() * font);
    let text_width = abs(width) - abs(p.margin_inside) - abs(p.margin_outside);
    let columns = f64::from(p.columns.clamp(1, 2));
    TextArea {
        column_width_pt: ((text_width - (columns - 1.0) * abs(p.column_gap)) / columns).max(1.0),
        height_pt: (abs(height) - abs(p.margin_top) - abs(p.margin_bottom)).max(1.0),
        font_size_pt: font,
    }
}

// ---- Generating Typst ----

/// `s` as a Typst string literal; safe for any input.
pub fn typst_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{{{:x}}}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The generated Typst code: set/show rules (placed before the expert code)
/// and front matter (title, contents; placed after it, so expert rules apply).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preamble {
    pub rules: String,
    pub front: String,
}

/// Generates the Typst code for `layout` and `meta`.
pub fn preamble(layout: &Layout, meta: &Meta) -> Preamble {
    Preamble {
        rules: rules(layout, meta, false),
        front: front_matter(layout, meta),
    }
}

/// The Typst code for exports, compiled for Typst's HTML target (see
/// [`crate::export`]): the rules that give the content its meaning
/// (language, numbering, caption positions, image width, character
/// styles), without the
/// page, the text and paragraph look, chapter page breaks and the front
/// matter (page rules are errors there; the writers make their own title).
pub fn export_preamble(layout: &Layout, meta: &Meta) -> Preamble {
    Preamble {
        rules: rules(layout, meta, true),
        front: String::new(),
    }
}

/// The set/show rules; with `html` only those that mean something in HTML.
fn rules(layout: &Layout, meta: &Meta, html: bool) -> String {
    let mut r = String::from("// Generated by Lethora from the layout settings.\n");
    let w = &mut r;

    let mut doc = Vec::new();
    if !meta.title.is_empty() {
        doc.push(format!("title: {}", typst_str(&meta.title)));
    }
    if !meta.author.is_empty() {
        doc.push(format!("author: {}", typst_str(&meta.author)));
    }
    if !doc.is_empty() {
        line(w, format!("#set document({})", doc.join(", ")));
    }

    // Page.
    let p = &layout.page;
    if !html {
        let size = match p.size {
            PageSize::A4 => "paper: \"a4\"".to_owned(),
            PageSize::A5 => "paper: \"a5\"".to_owned(),
            PageSize::UsLetter => "paper: \"us-letter\"".to_owned(),
            PageSize::UsTrade => "width: 6in, height: 9in".to_owned(),
            PageSize::Custom => format!("width: {}, height: {}", p.width, p.height),
        };
        let (inside, outside) = if p.two_sided {
            ("inside", "outside")
        } else {
            ("left", "right")
        };
        let (header, footer) = furniture(layout, meta);
        line(
            w,
            format!(
                "#set page({size}, margin: (top: {}, bottom: {}, {inside}: {}, {outside}: {}), \
             columns: {}, numbering: \"1\",\n  header: {header},\n  footer: {footer})",
                p.margin_top,
                p.margin_bottom,
                p.margin_inside,
                p.margin_outside,
                p.columns.clamp(1, 2),
            ),
        );
        line(w, format!("#set columns(gutter: {})", p.column_gap));
    }

    // Text and paragraphs.
    let t = &layout.text;
    let (lang, region) = match t.lang.split_once('-') {
        Some((l, r)) => (l, Some(r)),
        None => (t.lang.as_str(), None),
    };
    let region = region.map_or(String::new(), |r| format!(", region: {}", typst_str(r)));
    if html {
        line(w, format!("#set text(lang: {}{region})", typst_str(lang)));
    } else {
        line(
            w,
            format!(
                "#set text(font: {}, size: {}, lang: {}{region}, hyphenate: {})",
                typst_str(&t.font),
                t.size,
                typst_str(lang),
                t.hyphenate
            ),
        );
        line(
            w,
            format!(
                "#set par(justify: {}, leading: {}, spacing: {}, \
             first-line-indent: (amount: {}, all: false))",
                t.justify, t.leading, t.paragraph_spacing, t.first_line_indent
            ),
        );
    }

    // Headings.
    let h = &layout.headings;
    let numbering = match h.numbering {
        HeadingNumbering::None => "none",
        HeadingNumbering::Hierarchical => "\"1.1.\"",
        HeadingNumbering::Arabic => "\"1.\"",
        HeadingNumbering::Roman => "\"I.\"",
        HeadingNumbering::Letter => "\"A.\"",
    };
    line(w, format!("#set heading(numbering: {numbering})"));
    if !matches!(
        h.numbering,
        HeadingNumbering::None | HeadingNumbering::Hierarchical
    ) {
        line(
            w,
            "#show selector.or(..range(2, 7).map(l => heading.where(level: l))): \
             set heading(numbering: none)",
        );
    }
    if !html {
        // Level 3 and deeper share a size; the later rules win for levels 1 and 2.
        line(
            w,
            format!("#show heading: set text(size: {})", h.level3_size),
        );
        for (level, size) in [(1, h.level1_size), (2, h.level2_size)] {
            line(
                w,
                format!("#show heading.where(level: {level}): set text(size: {size})"),
            );
        }
        let space: Vec<String> = [("above", h.space_above), ("below", h.space_below)]
            .into_iter()
            .filter_map(|(side, l)| l.map(|l| format!("{side}: {l}")))
            .collect();
        if !space.is_empty() {
            line(w, format!("#show heading: set block({})", space.join(", ")));
        }
        let to_right = h.chapter_pagebreak == ChapterBreak::Right && p.two_sided;
        if h.chapter_pagebreak != ChapterBreak::None {
            // A blank page inserted to reach a right-hand page gets the styles
            // of the page break: no header or footer.
            let (scope, to) = if to_right {
                ("set page(header: none, footer: none); ", ", to: \"odd\"")
            } else {
                ("", "")
            };
            line(
                w,
                format!(
                    "#show heading.where(level: 1): it => {{ {{ {scope}pagebreak(weak: true{to}) }}; it }}"
                ),
            );
        }
    }

    // Figures and equations.
    let f = &layout.figures;
    let position = match f.caption_position {
        CaptionPosition::Top => "top",
        CaptionPosition::Bottom => "bottom",
    };
    line(w, format!("#set figure.caption(position: {position})"));
    if !f.numbering {
        line(w, "#set figure(numbering: none)");
    }
    // Also for exports: the width images are shown at.
    line(
        w,
        format!("#set image(width: {}%)", f.default_width.clamp(10, 100)),
    );
    tables(w, &layout.tables);
    let eq = if layout.equations.numbering {
        "\"(1)\""
    } else {
        "none"
    };
    line(w, format!("#set math.equation(numbering: {eq})"));

    // Character styles, applied in the content with `#styled("<id>")[...]`;
    // `styled` is always defined, and an unknown id leaves the text as it is.
    let styles: Vec<String> = layout
        .styles
        .iter()
        .map(|(id, s)| {
            let mut args = Vec::new();
            if let Some(font) = &s.font {
                args.push(format!("font: {}", typst_str(font)));
            }
            if let Some(size) = s.size {
                args.push(format!("size: {size}"));
            }
            format!("{}: ({})", typst_str(id), dict(&args))
        })
        .collect();
    line(w, format!("#let lethora-styles = ({})", dict(&styles)));
    line(
        w,
        "#let styled(id, body) = text(..lethora-styles.at(id, default: (:)), body)",
    );
    r
}

/// The thick rules of the academic style (above and below the table) and
/// the thin one (under the header), relative to the text size.
const ACADEMIC_RULES: (&str, &str) = ("0.08em", "0.05em");

/// The table rules. Every style is a set of `table` arguments in
/// `lethora-tables`: the default as a set rule, a table with a style of its
/// own (an exception) gets them as arguments, which beat set rules. No show
/// rule depends on the style, so nothing of the default leaks into an
/// exception. The academic rules under the header and below the table need
/// to know the table's rows: they are injected into its call at compile
/// time (`lethora-table-rules`, see [`crate::overrides::plan`]).
fn tables(w: &mut String, t: &TableSettings) {
    let (thick, thin) = ACADEMIC_RULES;
    line(
        w,
        format!(
            "#let lethora-tables = (plain: (stroke: none, fill: none), \
             grid: (stroke: 0.5pt, fill: none), \
             striped: (stroke: none, fill: (_, y) => if calc.odd(y) {{ luma(240) }}), \
             academic: (stroke: (_, y) => if y == 0 {{ (top: {thick}) }}, fill: none))"
        ),
    );
    line(
        w,
        format!(
            "#let lethora-table-rules = (head: table.hline(stroke: {thin}), \
             bottom: table.hline(stroke: {thick}))"
        ),
    );
    line(
        w,
        format!("#set table(..lethora-tables.{})", t.style.name()),
    );
    let position = match t.caption_position {
        CaptionPosition::Top => "top",
        CaptionPosition::Bottom => "bottom",
    };
    line(
        w,
        format!("#show figure.where(kind: table): set figure.caption(position: {position})"),
    );
    if t.header_bold {
        // A table has a header if its first cell-like child is a
        // `table.header`; its rows: the header's cells (by their column
        // spans) over the columns.
        line(
            w,
            "#show table: it => { \
             let h = it.children.find(c => c.func() != table.hline and c.func() != table.vline); \
             if h == none or h.func() != table.header { return it }; \
             let spans = h.children.filter(c => c.func() == table.cell).map(c => c.at(\"colspan\", default: 1)); \
             let rows = calc.max(1, calc.ceil(spans.sum(default: 0) / calc.max(1, it.columns.len()))); \
             show table.cell: c => if c.y < rows { set text(weight: \"bold\"); c } else { c }; \
             it }",
        );
    }
}

fn line(w: &mut String, s: impl AsRef<str>) {
    w.push_str(s.as_ref());
    w.push('\n');
}

/// The inside of a Typst dictionary literal of `pairs`; `:` when empty.
fn dict(pairs: &[String]) -> String {
    if pairs.is_empty() {
        ":".to_owned()
    } else {
        pairs.join(", ")
    }
}

/// The page header and footer expressions.
fn furniture(layout: &Layout, meta: &Meta) -> (String, String) {
    let two = layout.page.two_sided;
    let numbers = layout.furniture.page_numbers;
    let title = (!meta.title.is_empty()).then(|| typst_str(&meta.title));
    const CHAPTER: &str = "{ let hs = query(heading.where(level: 1).before(here())); \
                           if hs.len() > 0 { hs.last().body } }";
    let head = match layout.furniture.running_head {
        RunningHead::None => None,
        RunningHead::Title => title.clone(),
        RunningHead::Chapter => Some(match (&title, two) {
            (Some(title), true) => format!("if calc.even(p) {{ {title} }} else {CHAPTER}"),
            _ => CHAPTER.to_owned(),
        }),
        RunningHead::AuthorTitle => {
            let parts: Vec<&str> = [meta.author.as_str(), meta.title.as_str()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect();
            (!parts.is_empty()).then(|| typst_str(&parts.join(" / ")))
        }
    };
    let top = numbers == PageNumbers::TopOutside;
    let outside = if two {
        "if calc.odd(p) { right } else { left }"
    } else {
        "right"
    };
    let body = match (&head, top) {
        (None, false) => None,
        (Some(head), false) => Some(format!("align(center, {head})")),
        (None, true) => Some(format!("align({outside}, n)")),
        (Some(head), true) if layout.furniture.running_head == RunningHead::AuthorTitle => {
            Some(format!("align(right, ({head}) + \" / \" + n)"))
        }
        (Some(head), true) => {
            let odd = format!("grid(columns: (1fr, auto, 1fr), [], {head}, align(right, n))");
            Some(if two {
                format!(
                    "if calc.odd(p) {{ {odd} }} else {{ grid(columns: (1fr, auto, 1fr), \
                     align(left, n), {head}, []) }}"
                )
            } else {
                odd
            })
        }
    };
    // In books, chapter openers stay bare. Only the next chapter heading is
    // checked: asking every page for every chapter's page made memory grow
    // faster than the document (~1 GB extra at 800k words).
    let hide_header: Vec<String> = (layout.headings.chapter_pagebreak != ChapterBreak::None && two)
        .then(|| {
            "{ let nx = query(heading.where(level: 1).after(here())); \
             nx.len() > 0 and nx.first().location().page() == p }"
                .to_owned()
        })
        .into_iter()
        .collect();
    let guard = |hide: &[String], body: String| {
        if hide.is_empty() {
            body
        } else {
            format!("if not ({}) {{ {body} }}", hide.join(" or "))
        }
    };
    let header = match body {
        None => "none".to_owned(),
        Some(body) => format!(
            "context {{ let p = here().page(); let n = counter(page).display();              set text(size: 0.85em); {} }}",
            guard(&hide_header, body)
        ),
    };
    let footer = match numbers {
        PageNumbers::None | PageNumbers::TopOutside => "none".to_owned(),
        PageNumbers::BottomCenter => "context align(center, counter(page).display())".to_owned(),
        PageNumbers::BottomOutside => format!(
            "context {{ let p = here().page(); align({outside}, counter(page).display()) }}"
        ),
    };
    (header, footer)
}

/// Title page or title block, and the table of contents.
fn front_matter(layout: &Layout, meta: &Meta) -> String {
    let mut w = String::new();
    let has_title = [&meta.title, &meta.subtitle, &meta.author]
        .iter()
        .any(|s| !s.is_empty());
    let mut lines = Vec::new();
    if !meta.title.is_empty() {
        lines.push(format!(
            "text(size: 1.8em, weight: \"bold\", {})",
            typst_str(&meta.title)
        ));
    }
    if !meta.subtitle.is_empty() {
        lines.push(format!("text(size: 1.3em, {})", typst_str(&meta.subtitle)));
    }
    if !meta.author.is_empty() {
        lines.push(format!("text(size: 1.15em, {})", typst_str(&meta.author)));
    }
    if !meta.date.is_empty() {
        lines.push(typst_str(&meta.date));
    }
    let stack = format!("align(center, stack(spacing: 1.1em, {}))", lines.join(", "));

    if layout.front.title_page && has_title {
        let contact = if meta.contact.is_empty() {
            String::new()
        } else {
            format!(
                "align(left, text(size: 0.9em, {}))\n",
                typst_str(&meta.contact)
            )
        };
        line(
            &mut w,
            format!(
                "#page(header: none, footer: none, numbering: none, columns: 1, {{\n\
                 {contact}v(1fr)\n{stack}\nv(1.6fr)\n}})"
            ),
        );
        if layout.page.two_sided {
            line(
                &mut w,
                "#page(header: none, footer: none, numbering: none, columns: 1)[]",
            );
        }
        line(&mut w, "#counter(page).update(1)");
    } else if !lines.is_empty() {
        let block = format!("block(below: 2em, {stack})");
        if layout.page.columns > 1 {
            line(
                &mut w,
                format!("#place(top + center, float: true, scope: \"parent\", {block})"),
            );
        } else {
            line(&mut w, format!("#{block}"));
        }
    }
    let f = &layout.front;
    if f.toc {
        let mut args = format!("depth: {}", f.toc_depth.clamp(1, 4));
        if !f.toc_title.is_empty() {
            let _ = write!(args, ", title: {}", typst_str(&f.toc_title));
        }
        if !f.toc_indent {
            args.push_str(", indent: 0pt");
        }
        // Entry styles apply to this outline only.
        let mut rules = Vec::new();
        if !f.toc_dots {
            rules.push("set outline.entry(fill: none)".to_owned());
        }
        if let Some(spacing) = f.toc_spacing {
            rules.push(format!("show outline.entry: set block(above: {spacing})"));
        }
        if f.toc_bold_top {
            rules.push("show outline.entry.where(level: 1): set text(weight: \"bold\")".to_owned());
        }
        if rules.is_empty() {
            line(&mut w, format!("#outline({args})"));
        } else {
            line(
                &mut w,
                format!("#{{ {}; outline({args}) }}", rules.join("; ")),
            );
        }
        if f.title_page {
            line(&mut w, "#pagebreak(weak: true)");
        }
    }
    w
}

/// The bibliography call for the end of the document.
pub fn bibliography(layout: &Layout) -> String {
    let c = &layout.citations;
    let title = if c.bibliography_title.is_empty() {
        "auto".to_owned()
    } else {
        typst_str(&c.bibliography_title)
    };
    format!(
        "#bibliography(\"references.bib\", title: {title}, style: {})\n",
        typst_str(&c.style)
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn export_preambles_have_no_pages() {
        let meta = Meta {
            title: "T".into(),
            author: "A".into(),
            ..Meta::default()
        };
        for p in presets() {
            let layout = preset(p.id).unwrap();
            let export = export_preamble(&layout, &meta);
            assert!(export.front.is_empty());
            for paged in ["page(", "pagebreak", "set columns", "set par"] {
                assert!(!export.rules.contains(paged), "{}: {paged}", p.id);
            }
            assert!(export.rules.contains("#set text(lang: "), "{}", p.id);
            assert!(export.rules.contains("#let styled("), "{}", p.id);
            assert!(preamble(&layout, &meta).rules.contains("#set page("));
        }
    }

    #[test]
    fn presets_resolve_to_themselves() {
        for p in presets() {
            let layout = preset(p.id).unwrap();
            let entries: Entries = [(PRESET_KEY.to_owned(), json!(p.id))].into();
            assert_eq!(resolve(&entries), layout, "{}", p.id);
            assert!(diff(p.id, &layout).is_empty());
            // Every preset value passes validation unchanged.
            for (key, value) in flatten(&layout) {
                if key == SCHEMA_KEY {
                    continue;
                }
                assert_eq!(
                    validate_change(&key, &value).as_ref(),
                    Ok(&value),
                    "{}: {key}",
                    p.id
                );
            }
        }
        assert_eq!(resolve(&Entries::new()), preset(DEFAULT_PRESET).unwrap());
    }

    #[test]
    fn overrides_and_diff() {
        let entries: Entries = [
            ("preset".to_owned(), json!("novel")),
            ("text.size".to_owned(), json!("12PT")),
            ("page.columns".to_owned(), json!(2.0)),
            ("text.lang".to_owned(), json!("de_at")),
            // Ignored: unknown, invalid, hostile.
            ("text.colour".to_owned(), json!("red")),
            ("page.margin_top".to_owned(), json!("-1cm")),
            ("text.font".to_owned(), json!("\"); #read(\"x\")")),
        ]
        .into();
        let layout = resolve(&entries);
        assert_eq!(layout.text.size.to_string(), "12pt");
        assert_eq!(layout.page.columns, 2);
        assert_eq!(layout.text.lang, "de-AT");
        assert_eq!(layout.text.font, "Libertinus Serif");
        let d = diff("novel", &layout);
        assert_eq!(
            d.keys().collect::<Vec<_>>(),
            ["page.columns", "text.lang", "text.size"]
        );
    }

    #[test]
    fn lengths() {
        for (input, out) in [
            ("12pt", "12pt"),
            (" 2.5CM ", "2.5cm"),
            ("0.65em", "0.65em"),
            ("1in", "1in"),
            ("0mm", "0mm"),
            ("007pt", "7pt"),
        ] {
            assert_eq!(Length::parse(input).unwrap().to_string(), out);
        }
        for bad in [
            "",
            "12",
            "pt",
            "-1pt",
            "1e3pt",
            "1.2.3pt",
            ".5pt",
            "5.pt",
            "12 pt",
            "12px",
            "inf pt",
            "NaNpt",
            "99999999pt",
            "12pt)",
            "1pt + 2pt",
        ] {
            assert!(Length::parse(bad).is_err(), "{bad:?}");
        }
        let l: Length = serde_json::from_value(json!("3mm")).unwrap();
        assert_eq!(l.to_pt().unwrap(), 3.0 * 72.0 / 25.4);
        assert!(serde_json::from_value::<Length>(json!("3mm)")).is_err());
    }

    #[test]
    fn validation_rejects_hostile_values() {
        let hostile = [
            json!("\")\n#read(\"/etc/passwd\")"),
            json!("12pt)\n#read(\"x\")"),
            json!("a4\"), x: (\""),
            json!("\\"),
            json!(["12pt"]),
            json!({"a": 1}),
            json!(null),
            json!(1e308),
        ];
        for key in flatten(&preset("paper").unwrap()).keys() {
            // Free text is checked separately below: it is escaped, not rejected.
            if [
                SCHEMA_KEY,
                "citations.bibliography_title",
                "front.toc_title",
            ]
            .contains(&key.as_str())
            {
                continue;
            }
            for value in &hostile {
                assert!(
                    validate_change(key, value).is_err(),
                    "{key} accepted {value}"
                );
            }
        }
        assert!(validate_change("schema", &json!(1)).is_err());
        assert!(validate_change("preset", &json!("novel")).is_err());
        assert!(validate_change("page.size", &json!("A4")).is_err());
        assert_eq!(
            validate_change("page.size", &json!("us-trade")),
            Ok(json!("us-trade"))
        );
        assert_eq!(
            validate_change("headings.numbering", &json!("I.")),
            Ok(json!("I."))
        );
        assert!(validate_change("page.columns", &json!(3)).is_err());
        assert!(validate_change("page.margin_top", &json!("2em")).is_err());
        assert!(validate_change("page.margin_top", &json!("5in")).is_err());
        assert!(validate_change("text.size", &json!("3pt")).is_err());
        assert_eq!(
            validate_change("text.font", &json!("libertinus serif")),
            Ok(json!("Libertinus Serif"))
        );
        assert!(validate_change("text.font", &json!("Comic Sans")).is_err());
        assert!(validate_change("text.lang", &json!("english")).is_err());
        assert!(validate_change("citations.style", &json!("apa\")")).is_err());
        assert_eq!(
            validate_change("figures.default_width", &json!(79.6)),
            Ok(json!(80))
        );
        // Free text is accepted as written (and escaped when emitted), except
        // line breaks and control characters.
        let title = "\"); #read(\"/x\") // \\ [*]";
        assert_eq!(
            validate_change("citations.bibliography_title", &json!(title)),
            Ok(json!(title))
        );
        assert!(validate_change("citations.bibliography_title", &json!("a\nb")).is_err());
        assert!(validate_meta("title", &json!("a\u{0}b")).is_err());
        assert!(validate_meta("title", &json!("x".repeat(501))).is_err());
        assert_eq!(
            validate_meta("contact", &json!("a\r\nb")),
            Ok(json!("a\nb"))
        );
        assert!(validate_meta("other", &json!("x")).is_err());
    }

    #[test]
    fn heading_and_contents_settings_validate() {
        for key in [
            "headings.space_above",
            "headings.space_below",
            "front.toc_spacing",
        ] {
            // Empty: the default.
            assert_eq!(validate_change(key, &json!(" ")), Ok(json!("")), "{key}");
            assert_eq!(validate_change(key, &json!(" 12PT")), Ok(json!("12pt")));
            assert_eq!(validate_change(key, &json!("1.5em")), Ok(json!("1.5em")));
            assert_eq!(validate_change(key, &json!("0pt")), Ok(json!("0pt")));
            for bad in [
                json!("-1pt"),
                json!("12"),
                json!("80em"),
                json!("3in"),
                json!(12),
            ] {
                assert!(validate_change(key, &bad).is_err(), "{key} accepted {bad}");
            }
        }
        for depth in 1..=4 {
            assert_eq!(
                validate_change("front.toc_depth", &json!(depth as f64)),
                Ok(json!(depth))
            );
        }
        for bad in [json!(0), json!(5), json!(2.5), json!("3"), json!(true)] {
            assert!(validate_change("front.toc_depth", &bad).is_err(), "{bad}");
        }
        for key in ["front.toc_dots", "front.toc_indent", "front.toc_bold_top"] {
            assert_eq!(validate_change(key, &json!(false)), Ok(json!(false)));
            assert!(validate_change(key, &json!("false")).is_err());
        }
        let title = "\"); #read(\"/x\") [*]";
        assert_eq!(
            validate_change("front.toc_title", &json!(title)),
            Ok(json!(title))
        );
        assert!(validate_change("front.toc_title", &json!("a\nb")).is_err());
        // Stored values resolve; invalid ones fall back to the preset's.
        let entries: Entries = [
            ("headings.space_above".to_owned(), json!("2em")),
            ("headings.space_below".to_owned(), json!("lots")),
            ("front.toc_depth".to_owned(), json!(9)),
            ("front.toc_spacing".to_owned(), json!("4pt")),
        ]
        .into();
        let layout = resolve(&entries);
        assert_eq!(layout.headings.space_above, Some(em(2.0)));
        assert_eq!(layout.headings.space_below, None);
        assert_eq!(layout.front.toc_depth, 3);
        assert_eq!(layout.front.toc_spacing, Some(pt(4.0)));
        assert_eq!(
            diff("paper", &layout).keys().collect::<Vec<_>>(),
            ["front.toc_spacing", "headings.space_above"]
        );
    }

    #[test]
    fn heading_rules() {
        let mut layout = preset("paper").unwrap();
        let rules = |l: &Layout| preamble(l, &Meta::default()).rules;
        // Level 3 and deeper share the level-3 size; levels 1 and 2 come later and win.
        assert!(rules(&layout).contains(
            "#show heading: set text(size: 11pt)\n\
             #show heading.where(level: 1): set text(size: 14pt)\n\
             #show heading.where(level: 2): set text(size: 12pt)\n"
        ));
        // Spacing only when set.
        assert!(!rules(&layout).contains("set block("));
        layout.headings.space_above = Some(em(1.5));
        assert!(rules(&layout).contains("#show heading: set block(above: 1.5em)\n"));
        layout.headings.space_below = Some(pt(6.0));
        assert!(rules(&layout).contains("#show heading: set block(above: 1.5em, below: 6pt)\n"));
        layout.headings.space_above = None;
        assert!(rules(&layout).contains("#show heading: set block(below: 6pt)\n"));
    }

    #[test]
    fn contents_call() {
        let mut layout = preset("thesis").unwrap();
        let front = |l: &Layout| preamble(l, &Meta::default()).front;
        // The defaults: as before these settings existed.
        assert_eq!(
            front(&layout),
            "#outline(depth: 3)\n#pagebreak(weak: true)\n"
        );
        let f = &mut layout.front;
        f.toc_depth = 2;
        f.toc_title = "Inhalt \"x\"".into();
        f.toc_indent = false;
        assert_eq!(
            front(&layout),
            "#outline(depth: 2, title: \"Inhalt \\\"x\\\"\", indent: 0pt)\n#pagebreak(weak: true)\n"
        );
        let f = &mut layout.front;
        f.toc_dots = false;
        f.toc_spacing = Some(em(1.0));
        f.toc_bold_top = true;
        let code = front(&layout);
        assert_eq!(
            code.lines().next().unwrap(),
            "#{ set outline.entry(fill: none); show outline.entry: set block(above: 1em); \
             show outline.entry.where(level: 1): set text(weight: \"bold\"); \
             outline(depth: 2, title: \"Inhalt \\\"x\\\"\", indent: 0pt) }"
        );
        assert!(
            typst::syntax::parse(&code)
                .errors_and_warnings()
                .0
                .is_empty(),
            "{code}"
        );
        layout.front.toc = false;
        assert!(!front(&layout).contains("outline"));
    }

    #[test]
    fn strings_are_escaped() {
        assert_eq!(typst_str("a\"b\\c\nd\u{1}"), "\"a\\\"b\\\\c\\nd\\u{1}\"");
        let source = format!(
            "#let x = {}",
            typst_str("\"); #read(\"/etc/passwd\") //\n\\")
        );
        let root = typst::syntax::parse(&source);
        assert!(root.errors_and_warnings().0.is_empty(), "{source}");
        // One string literal and no call: nothing escaped into code.
        fn count(node: &typst::syntax::SyntaxNode, kind: typst::syntax::SyntaxKind) -> usize {
            usize::from(node.kind() == kind)
                + node.children().map(|c| count(c, kind)).sum::<usize>()
        }
        assert_eq!(count(&root, typst::syntax::SyntaxKind::Str), 1);
        assert_eq!(count(&root, typst::syntax::SyntaxKind::FuncCall), 0);
    }

    #[test]
    fn stored_fonts_this_machine_may_lack_are_kept() {
        let resolve_font = |font: &str| {
            let entries: Entries = [("text.font".to_owned(), json!(font))].into();
            resolve(&entries).text.font
        };
        assert_eq!(resolve_font("Segoe UI Semibold"), "Segoe UI Semibold");
        assert_eq!(resolve_font("Noto Sans CJK JP"), "Noto Sans CJK JP");
        let default = preset(DEFAULT_PRESET).unwrap().text.font;
        for bad in [
            "",
            " Arial",
            "a\"b",
            "x); #read(\"y\")",
            "a\nb",
            &"x".repeat(101),
        ] {
            assert_eq!(resolve_font(bad), default, "{bad:?}");
        }
        // New values must be in the list.
        assert!(validate_change("text.font", &json!("Segoe UI")).is_err());
    }

    #[test]
    fn fonts_are_text_fonts() {
        let fonts = fonts();
        assert!(fonts.contains(&"Libertinus Serif".to_owned()), "{fonts:?}");
        assert!(
            fonts.contains(&"New Computer Modern".to_owned()),
            "{fonts:?}"
        );
        assert!(!fonts.iter().any(|f| f.contains("Math")), "{fonts:?}");
    }

    #[test]
    fn embedded_font_files() {
        for family in fonts() {
            let file = embedded_font(&family.to_uppercase()).unwrap_or_else(|| panic!("{family}"));
            // An OpenType or TrueType file.
            assert!(
                [b"OTTO".as_slice(), &[0, 1, 0, 0], b"true"].contains(&&file[..4]),
                "{family}"
            );
        }
        assert_eq!(embedded_font("Segoe UI"), None);
    }

    #[test]
    fn text_area_columns() {
        let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
        let pt_mm = 72.0 / 25.4;
        // A4, 30 mm side margins, 25 mm top and bottom.
        let paper = preset("paper").unwrap();
        let a = text_area(&paper);
        assert!(close(a.column_width_pt, 150.0 * pt_mm), "{a:?}");
        assert!(close(a.height_pt, 247.0 * pt_mm), "{a:?}");
        assert!(close(a.font_size_pt, 11.0), "{a:?}");
        // Two columns with a 6 mm gap: (150 - 6) / 2 mm each.
        let mut two = paper.clone();
        two.page.columns = 2;
        assert!(close(text_area(&two).column_width_pt, 72.0 * pt_mm));
        // 6 x 9 in, 0.875 + 0.625 in sides, 0.75 + 0.85 in top and bottom.
        let novel = text_area(&preset("novel").unwrap());
        assert!(close(novel.column_width_pt, 4.5 * 72.0), "{novel:?}");
        assert!(close(novel.height_pt, 7.4 * 72.0), "{novel:?}");
        // US Letter, 1 in margins, 12 pt; a gap in em is relative to the text size.
        let mut manuscript = preset("manuscript").unwrap();
        assert!(close(text_area(&manuscript).column_width_pt, 6.5 * 72.0));
        manuscript.page.columns = 2;
        manuscript.page.column_gap = em(1.0);
        assert!(close(
            text_area(&manuscript).column_width_pt,
            (6.5 * 72.0 - 12.0) / 2.0
        ));
        // Custom pages use their own width and height.
        let mut custom = paper;
        custom.page.size = PageSize::Custom;
        custom.page.width = mm(170.0);
        assert!(close(text_area(&custom).column_width_pt, 110.0 * pt_mm));
    }

    #[test]
    fn image_quality_validates_and_stays_out_of_the_code() {
        for p in presets() {
            assert_eq!(
                preset(p.id).unwrap().export.image_quality,
                ImageQuality::Original
            );
        }
        assert_eq!(
            validate_change("export.image_quality", &json!("screen")),
            Ok(json!("screen"))
        );
        for bad in [json!("Screen"), json!("72dpi"), json!(150), json!(null)] {
            assert!(validate_change("export.image_quality", &bad).is_err());
        }
        let entries: Entries = [
            ("preset".to_owned(), json!("thesis")),
            ("export.image_quality".to_owned(), json!("email")),
        ]
        .into();
        let layout = resolve(&entries);
        assert_eq!(layout.export.image_quality, ImageQuality::Email);
        assert_eq!(layout.export.image_quality.dpi(), Some(96.0));
        assert_eq!(
            diff("thesis", &layout).keys().collect::<Vec<_>>(),
            ["export.image_quality"]
        );
        // The generated code is the same for every quality.
        let meta = Meta::default();
        assert_eq!(
            preamble(&layout, &meta),
            preamble(&preset("thesis").unwrap(), &meta)
        );
    }

    fn style(name: Option<&str>, font: Option<&str>, size: Option<Length>) -> CharStyle {
        CharStyle {
            name: name.map(str::to_owned),
            font: font.map(str::to_owned),
            size,
        }
    }

    fn entries_of(pairs: &[(&str, Value)]) -> Entries {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect()
    }

    #[test]
    fn style_keys_and_ids() {
        assert_eq!(
            style_key("styles.eb-garamond-14pt.font"),
            Some(("eb-garamond-14pt", "font"))
        );
        for field in ["name", "size", "removed"] {
            assert_eq!(style_key(&style_key_of("a1", field)), Some(("a1", field)));
        }
        for bad in [
            "styles.a",
            "styles..font",
            "styles.A.font",
            "styles.a b.font",
            "styles.-a.font",
            "styles.a-.font",
            "styles.a--b.font",
            "styles.a_b.font",
            "styles.é.font",
            "styles.a.colour",
            "styles.a.font.x",
            "text.font",
            "style.a.font",
        ] {
            assert_eq!(style_key(bad), None, "{bad}");
        }
        assert!(is_style_id(&"a".repeat(48)));
        assert!(!is_style_id(&"a".repeat(49)));
        assert!(!is_style_id(""));
    }

    #[test]
    fn character_styles_resolve() {
        let entries = entries_of(&[
            ("styles.libertinus-14pt.font", json!("libertinus serif")),
            ("styles.libertinus-14pt.size", json!(" 14PT")),
            ("styles.big.size", json!("18pt")),
            ("styles.big.name", json!("  Big  ")),
            ("styles.sys.font", json!("Segoe UI Semibold")),
            ("styles.sys.removed", json!(false)),
            // Ignored: a name only, no valid font or size, bad ids and fields.
            ("styles.named.name", json!("Only a name")),
            ("styles.bad-font.font", json!("x\"); #read(\"y\")")),
            ("styles.bad-size.size", json!("2em")),
            ("styles.Upper.font", json!("Arial")),
            ("styles.x.colour", json!("red")),
            ("styles.x", json!("Arial")),
            // A size out of range is dropped; the style keeps its font.
            ("styles.mixed.font", json!("Arial")),
            ("styles.mixed.size", json!("100pt")),
            ("styles.gone.font", json!("Arial")),
            ("styles.gone.removed", json!(true)),
        ]);
        let layout = resolve(&entries);
        assert_eq!(
            layout.styles,
            [
                ("big".to_owned(), style(Some("Big"), None, Some(pt(18.0)))),
                (
                    "libertinus-14pt".to_owned(),
                    style(None, Some("Libertinus Serif"), Some(pt(14.0)))
                ),
                ("mixed".to_owned(), style(None, Some("Arial"), None)),
                (
                    "sys".to_owned(),
                    style(None, Some("Segoe UI Semibold"), None)
                ),
            ]
            .into()
        );
        // Everything else resolves as without styles.
        assert_eq!(
            Layout {
                styles: BTreeMap::new(),
                ..layout.clone()
            },
            preset(DEFAULT_PRESET).unwrap()
        );
        // The diff stores what resolves, normalized, and resolves again to it.
        let d = diff(DEFAULT_PRESET, &layout);
        assert_eq!(d["styles.libertinus-14pt.size"], json!("14pt"));
        assert_eq!(d["styles.big.name"], json!("Big"));
        assert!(d.keys().all(|k| style_key(k).is_some()), "{d:?}");
        assert_eq!(resolve(&d), layout);
        assert_eq!(
            flatten(&layout).len(),
            flatten(&preset("paper").unwrap()).len() + d.len()
        );
    }

    #[test]
    fn preset_styles_merge_and_hide() {
        let mut base = preset("novel").unwrap();
        base.styles = [
            (
                "title".to_owned(),
                style(Some("Title"), Some("Libertinus Serif"), Some(pt(20.0))),
            ),
            (
                "small".to_owned(),
                style(Some("Small"), None, Some(pt(9.0))),
            ),
        ]
        .into();
        assert_eq!(resolve_on(base.clone(), &Entries::new()), base);
        assert!(diff_from(&base, &base).is_empty());

        let entries = entries_of(&[
            ("styles.title.size", json!("24pt")),
            ("styles.title.name", json!("Display")),
            ("styles.small.removed", json!(true)),
            ("styles.extra.font", json!("Arial")),
        ]);
        let layout = resolve_on(base.clone(), &entries);
        assert_eq!(
            layout.styles,
            [
                ("extra".to_owned(), style(None, Some("Arial"), None)),
                (
                    "title".to_owned(),
                    style(Some("Display"), Some("Libertinus Serif"), Some(pt(24.0)))
                ),
            ]
            .into()
        );
        let d = diff_from(&base, &layout);
        assert_eq!(d, entries);
        assert_eq!(resolve_on(base.clone(), &d), layout);
        // `removed: false` (or removing the key) shows it again.
        let mut shown = entries.clone();
        shown.insert("styles.small.removed".into(), json!(false));
        assert!(resolve_on(base, &shown).styles.contains_key("small"));
    }

    #[test]
    fn character_style_settings_validate() {
        let ok = |key: &str, value: Value| validate_change(key, &value);
        assert_eq!(
            ok("styles.eb-garamond-14pt.font", json!(" libertinus serif ")),
            Ok(json!("Libertinus Serif"))
        );
        // A family the list lacks (a system font) is accepted as written.
        assert_eq!(
            ok("styles.a.font", json!("Segoe UI")),
            Ok(json!("Segoe UI"))
        );
        assert_eq!(
            validate_change_in("styles.a.font", &json!("segoe ui"), &["Segoe UI".into()]),
            Ok(json!("Segoe UI"))
        );
        assert_eq!(ok("styles.a.size", json!(" 14PT")), Ok(json!("14pt")));
        assert_eq!(ok("styles.a.size", json!("5mm")), Ok(json!("5mm")));
        assert_eq!(ok("styles.a.name", json!("  Big  ")), Ok(json!("Big")));
        assert_eq!(
            ok("styles.a.name", json!("é".repeat(64))),
            Ok(json!("é".repeat(64)))
        );
        assert_eq!(ok("styles.a.removed", json!(true)), Ok(json!(true)));
        for (key, bad) in [
            ("styles.a.font", json!("a\"b")),
            ("styles.a.font", json!("x); #read(\"y\")")),
            ("styles.a.font", json!("")),
            ("styles.a.font", json!(12)),
            ("styles.a.size", json!("1.2em")),
            ("styles.a.size", json!("3pt")),
            ("styles.a.size", json!("73pt")),
            ("styles.a.size", json!("14")),
            ("styles.a.name", json!("   ")),
            ("styles.a.name", json!("a\nb")),
            ("styles.a.name", json!("a\tb")),
            ("styles.a.name", json!("x".repeat(65))),
            ("styles.a.removed", json!("true")),
            ("styles.A.font", json!("Arial")),
            ("styles.a.colour", json!("red")),
            ("styles.a", json!("Arial")),
        ] {
            assert!(validate_change(key, &bad).is_err(), "{key} accepted {bad}");
        }
        assert!(
            validate_change("styles.A.font", &json!("Arial"))
                .unwrap_err()
                .contains("character style")
        );
    }

    #[test]
    fn character_style_code() {
        let mut layout = preset("paper").unwrap();
        let rules = |l: &Layout| preamble(l, &Meta::default()).rules;
        // `styled` exists without styles too.
        let none = rules(&layout);
        assert!(none.ends_with(
            "#let lethora-styles = (:)\n\
             #let styled(id, body) = text(..lethora-styles.at(id, default: (:)), body)\n"
        ));
        layout.styles = [
            ("big".to_owned(), style(Some("Big"), None, Some(pt(14.0)))),
            (
                "evil".to_owned(),
                style(None, Some("A\"), #read(\"x\") \\"), Some(mm(5.0))),
            ),
            (
                "mono".to_owned(),
                style(None, Some("DejaVu Sans Mono"), None),
            ),
        ]
        .into();
        let code = rules(&layout);
        assert!(
            code.contains(
                "#let lethora-styles = (\"big\": (size: 14pt), \
                 \"evil\": (font: \"A\\\"), #read(\\\"x\\\") \\\\\", size: 5mm), \
                 \"mono\": (font: \"DejaVu Sans Mono\"))\n"
            ),
            "{code}"
        );
        let root = typst::syntax::parse(&code);
        assert!(root.errors_and_warnings().0.is_empty(), "{code}");
        // The names are free in Typst's standard library.
        let library = &crate::fonts::LIBRARY;
        for name in ["styled", "lethora-styles"] {
            assert!(library.global.scope().get(name).is_none(), "{name}");
            assert!(library.math.scope().get(name).is_none(), "{name}");
        }
    }

    #[test]
    fn table_settings() {
        // Preset defaults: academic with captions above, plain in novels.
        for (id, style, caption) in [
            ("paper", TableStyle::Academic, CaptionPosition::Top),
            ("thesis", TableStyle::Academic, CaptionPosition::Top),
            ("manuscript", TableStyle::Academic, CaptionPosition::Top),
            ("novel", TableStyle::Plain, CaptionPosition::Bottom),
        ] {
            let t = preset(id).unwrap().tables;
            assert_eq!(
                (t.style, t.caption_position, t.header_bold),
                (style, caption, true),
                "{id}"
            );
        }
        assert_eq!(
            validate_change("tables.style", &json!("grid")),
            Ok(json!("grid"))
        );
        assert!(validate_change("tables.style", &json!("Grid")).is_err());
        assert!(validate_change("tables.style", &json!("fancy")).is_err());
        assert_eq!(
            validate_change("tables.header_bold", &json!(false)),
            Ok(json!(false))
        );
        assert!(validate_change("tables.header_bold", &json!("no")).is_err());
        assert_eq!(
            validate_change("tables.caption_position", &json!("bottom")),
            Ok(json!("bottom"))
        );
        assert!(validate_change("tables.caption_position", &json!("left")).is_err());

        let entries: Entries = [
            ("tables.style".to_owned(), json!("striped")),
            ("tables.header_bold".to_owned(), json!(false)),
            ("tables.caption_position".to_owned(), json!("nowhere")),
        ]
        .into();
        let layout = resolve(&entries);
        assert_eq!(layout.tables.style, TableStyle::Striped);
        assert!(!layout.tables.header_bold);
        assert_eq!(layout.tables.caption_position, CaptionPosition::Top);
        assert_eq!(
            diff("paper", &layout).keys().collect::<Vec<_>>(),
            ["tables.header_bold", "tables.style"]
        );

        let rules = |l: &Layout| preamble(l, &Meta::default()).rules;
        let code = rules(&layout);
        assert!(
            code.contains("#set table(..lethora-tables.striped)\n"),
            "{code}"
        );
        assert!(
            code.contains("#show figure.where(kind: table): set figure.caption(position: top)\n")
        );
        // The figures' caption setting stays for the other figures.
        assert!(code.contains("#set figure.caption(position: bottom)\n"));
        assert!(!code.contains("#show table:"));
        let mut bold = layout.clone();
        bold.tables.header_bold = true;
        assert!(rules(&bold).contains("#show table: it => {"));
        // Each style is defined, whatever the default.
        for style in ["plain", "grid", "striped", "academic"] {
            assert!(code.contains(&format!("{style}: (")), "{style}");
        }
        assert!(
            typst::syntax::parse(&code)
                .errors_and_warnings()
                .0
                .is_empty(),
            "{code}"
        );
        assert!(
            typst::syntax::parse(&rules(&bold))
                .errors_and_warnings()
                .0
                .is_empty()
        );
    }
}
