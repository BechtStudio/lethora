//! Anchored overrides: per-element exceptions of the layout layer (a page
//! break before a block, extra space, a column break, a figure's placement,
//! an image's quality in the exported PDF, a table's style).
//!
//! They are stored in the document's `overrides` map as flattened scalar
//! entries, `<id>.kind`, `<id>.anchor` (an anchor string from the core) and
//! `<id>.<param>`, and turned into Typst only at compile time: the app resolves
//! each anchor to a content offset ([`ResolvedOverride`]) and [`plan`] turns
//! those into insertions into the compiled source. The stored content is never
//! changed.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{BuildHasher, Hasher};
use std::num::NonZeroUsize;
use std::ops::Range;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use typst::foundations::{self, ContextElem, Element, Label, Selector};
use typst::introspection::{self, Introspector, MetadataElem, Tag};
use typst::layout::{Abs, Frame, FrameItem, PlaceElem, Point, Rect, Transform};
use typst::model::{FigureElem, TableElem};
use typst::syntax::{LinkedNode, Side, Source, Span, SyntaxKind, SyntaxNode, ast};
use typst::text::TextItem;
use typst::utils::PicoStr;
use typst_layout::PagedDocument;

use crate::compile::{Diagnostic, Location, Severity, SourceMap};
use crate::layout::{self, Entries, ImageQuality, Length, TableStyle};

/// Field holding an override's kind.
pub const KIND_FIELD: &str = "kind";
/// Field holding an override's anchor.
pub const ANCHOR_FIELD: &str = "anchor";
/// Longest accepted id.
const MAX_ID_LEN: usize = 32;
/// Label of the invisible markers injected with block overrides, to find
/// where they ended up on the pages.
const MARKER: &str = "lethora-override";
/// Pages searched for a placed figure, from the one before the page of its
/// position in the text.
const FLOAT_PAGES: usize = 8;

/// What an override does.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Kind {
    /// A (weak) page break before the block; `to_odd`: continue on an odd
    /// (right-hand) page.
    PageBreak { to_odd: bool },
    /// A (weak) column break before the block.
    ColumnBreak,
    /// Extra vertical space before the block.
    SpaceBefore { length: Length },
    /// Where the figure goes.
    FigurePlacement { placement: Placement },
    /// The image's resolution in the exported PDF (see [`crate::images`]).
    ImageQuality { quality: ImageQuality },
    /// The table's style instead of the layout's default.
    TableStyle { style: TableStyle },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Placement {
    /// Floats to the top of the page/column.
    Top,
    /// Floats to the bottom.
    Bottom,
    /// Stays in the text flow (Typst's `none`).
    Here,
    /// Floats to the top or bottom, whichever is closer.
    Auto,
}

impl Placement {
    fn typst(self) -> &'static str {
        match self {
            Placement::Top => "top",
            Placement::Bottom => "bottom",
            Placement::Here => "none",
            Placement::Auto => "auto",
        }
    }
}

impl Kind {
    /// The stored name (`<id>.kind`).
    pub fn name(&self) -> &'static str {
        match self {
            Kind::PageBreak { .. } => "page_break",
            Kind::ColumnBreak => "column_break",
            Kind::SpaceBefore { .. } => "space_before",
            Kind::FigurePlacement { .. } => "figure_placement",
            Kind::ImageQuality { .. } => "image_quality",
            Kind::TableStyle { .. } => "table_style",
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Kind::PageBreak { .. } => "page break",
            Kind::ColumnBreak => "column break",
            Kind::SpaceBefore { .. } => "space before",
            Kind::FigurePlacement { .. } => "figure placement",
            Kind::ImageQuality { .. } => "image quality",
            Kind::TableStyle { .. } => "table style",
        }
    }

    /// Whether it applies before a block (at the start of a line), as
    /// opposed to a figure, an image or a table.
    pub fn is_block(&self) -> bool {
        !matches!(
            self,
            Kind::FigurePlacement { .. } | Kind::ImageQuality { .. } | Kind::TableStyle { .. }
        )
    }

    /// The parameters, as stored (without kind and anchor).
    pub fn params(&self) -> Map<String, Value> {
        let mut map = Map::new();
        match self {
            Kind::PageBreak { to_odd } => {
                map.insert("to_odd".into(), Value::Bool(*to_odd));
            }
            Kind::ColumnBreak => {}
            Kind::SpaceBefore { length } => {
                map.insert("length".into(), Value::String(length.to_string()));
            }
            Kind::FigurePlacement { placement } => {
                map.insert(
                    "placement".into(),
                    serde_json::to_value(placement).unwrap_or(Value::Null),
                );
            }
            Kind::ImageQuality { quality } => {
                map.insert(
                    "quality".into(),
                    serde_json::to_value(quality).unwrap_or(Value::Null),
                );
            }
            Kind::TableStyle { style } => {
                map.insert("style".into(), Value::from(style.name()));
            }
        }
        map
    }

    /// Sort order of block insertions at the same line: breaks first.
    fn order(&self) -> u8 {
        match self {
            Kind::PageBreak { .. } => 0,
            Kind::ColumnBreak => 1,
            Kind::SpaceBefore { .. } => 2,
            Kind::FigurePlacement { .. } => 3,
            Kind::ImageQuality { .. } => 4,
            Kind::TableStyle { .. } => 5,
        }
    }

    /// The Typst lines inserted before the block of override `id`: the
    /// override, then an invisible marker. A marker is only tags, so it
    /// changes no layout: tags go with the next thing laid out, the top of
    /// the block after the break or the space (a weak break on a fresh page
    /// is still skipped). A space's marker also holds its height in points.
    fn block_code(&self, id: &str) -> String {
        let code = match self {
            Kind::PageBreak { to_odd: false } => "#pagebreak(weak: true)\n".into(),
            Kind::PageBreak { to_odd: true } => "#pagebreak(weak: true, to: \"odd\")\n".into(),
            Kind::ColumnBreak => "#colbreak(weak: true)\n".into(),
            // A validated length prints as plain digits and a unit.
            Kind::SpaceBefore { length } => format!("#v({length})\n"),
            Kind::FigurePlacement { .. } | Kind::ImageQuality { .. } | Kind::TableStyle { .. } => {
                return String::new();
            }
        };
        if !is_id(id) {
            return code;
        }
        let marker = match self {
            Kind::SpaceBefore { length } => format!(
                "#context [#metadata((\"{id}\", ({length}).to-absolute().pt()))<{MARKER}>]\n"
            ),
            _ => format!("#metadata(\"{id}\")<{MARKER}>\n"),
        };
        code + &marker
    }
}

/// Checks a new override of kind `kind` (its stored name) with `params`
/// and returns it normalized.
pub fn validate_new(kind: &str, params: &Map<String, Value>) -> Result<Kind, String> {
    let allow = |keys: &[&str]| match params.keys().find(|k| !keys.contains(&k.as_str())) {
        Some(k) => Err(format!("{kind}: unknown parameter \"{}\".", clip(k))),
        None => Ok(()),
    };
    match kind {
        "page_break" => {
            allow(&["to_odd"])?;
            let to_odd = match params.get("to_odd") {
                None => false,
                Some(v) => v
                    .as_bool()
                    .ok_or_else(|| format!("{kind}: to_odd must be true or false."))?,
            };
            Ok(Kind::PageBreak { to_odd })
        }
        "column_break" => {
            allow(&[])?;
            Ok(Kind::ColumnBreak)
        }
        "space_before" => {
            allow(&["length"])?;
            let text = params
                .get("length")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{kind}: length must be text like 12pt or 1em."))?;
            let length = layout::check_length(
                &format!("{kind}.length"),
                text,
                0.0..=288.0,
                Some(0.0..=20.0),
            )?;
            Ok(Kind::SpaceBefore { length })
        }
        "figure_placement" => {
            allow(&["placement"])?;
            let placement = params
                .get("placement")
                .and_then(|v| serde_json::from_value::<Placement>(v.clone()).ok())
                .ok_or_else(|| format!("{kind}: placement must be top, bottom, here or auto."))?;
            Ok(Kind::FigurePlacement { placement })
        }
        "image_quality" => {
            allow(&["quality"])?;
            let quality = params
                .get("quality")
                .and_then(|v| serde_json::from_value::<ImageQuality>(v.clone()).ok())
                .ok_or_else(|| {
                    format!("{kind}: quality must be original, print, screen or email.")
                })?;
            Ok(Kind::ImageQuality { quality })
        }
        "table_style" => {
            allow(&["style"])?;
            let style = params
                .get("style")
                .and_then(|v| serde_json::from_value::<TableStyle>(v.clone()).ok())
                .ok_or_else(|| {
                    format!("{kind}: style must be plain, academic, grid or striped.")
                })?;
            Ok(Kind::TableStyle { style })
        }
        _ => Err(format!("Unknown override kind \"{}\".", clip(kind))),
    }
}

/// A stored override.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Override {
    pub id: String,
    pub kind: Kind,
    /// Anchor string from the core (`Document::anchor_at`).
    pub anchor: String,
}

impl Override {
    /// The flattened map entries of this override.
    pub fn entries(&self) -> Entries {
        let mut entries = Entries::new();
        entries.insert(key(&self.id, KIND_FIELD), Value::from(self.kind.name()));
        entries.insert(
            key(&self.id, ANCHOR_FIELD),
            Value::from(self.anchor.as_str()),
        );
        for (field, value) in self.kind.params() {
            entries.insert(key(&self.id, &field), value);
        }
        entries
    }
}

/// The map key of `field` of override `id`.
pub fn key(id: &str, field: &str) -> String {
    format!("{id}.{field}")
}

/// The keys of `entries` that belong to override `id`.
pub fn keys_of<'a>(entries: &'a Entries, id: &str) -> Vec<&'a String> {
    entries
        .keys()
        .filter(|k| k.split_once('.').is_some_and(|(i, _)| i == id))
        .collect()
}

/// An override whose stored entries are not valid (not applied).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invalid {
    pub id: String,
    pub message: String,
}

/// Parses the flattened `overrides` map: valid overrides sorted by id, and
/// the ids of invalid ones with the reason.
pub fn parse(entries: &Entries) -> (Vec<Override>, Vec<Invalid>) {
    let mut groups: BTreeMap<&str, Map<String, Value>> = BTreeMap::new();
    let mut invalid = Vec::new();
    for (k, value) in entries {
        match k.split_once('.') {
            Some((id, field)) if !field.is_empty() => {
                groups
                    .entry(id)
                    .or_default()
                    .insert(field.to_owned(), value.clone());
            }
            _ => invalid.push(Invalid {
                id: k.clone(),
                message: format!("\"{}\" is not an override entry.", clip(k)),
            }),
        }
    }
    let mut valid = Vec::new();
    for (id, mut fields) in groups {
        let result = (|| {
            if !is_id(id) {
                return Err(format!("\"{}\" is not an override id.", clip(id)));
            }
            let kind = fields.remove(KIND_FIELD);
            let kind = kind
                .as_ref()
                .and_then(Value::as_str)
                .ok_or("The override has no kind.")?;
            let anchor = match fields.remove(ANCHOR_FIELD) {
                Some(Value::String(a)) if !a.is_empty() => a,
                _ => return Err("The override has no anchor.".to_owned()),
            };
            Ok(Override {
                id: id.to_owned(),
                kind: validate_new(kind, &fields)?,
                anchor,
            })
        })();
        match result {
            Ok(o) => valid.push(o),
            Err(message) => invalid.push(Invalid {
                id: id.to_owned(),
                message,
            }),
        }
    }
    (valid, invalid)
}

fn is_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A new override id (`o` and 8 base-36 characters) that `taken` rejects.
pub fn new_id(taken: impl Fn(&str) -> bool) -> String {
    loop {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        let mut n = hasher.finish();
        let mut id = String::from("o");
        for _ in 0..8 {
            id.push(char::from_digit((n % 36) as u32, 36).expect("digit"));
            n /= 36;
        }
        if !taken(&id) {
            return id;
        }
    }
}

// ---- Compile time ----

/// An override with its anchor resolved against the content compiled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedOverride {
    pub id: String,
    #[serde(flatten)]
    pub kind: Kind,
    /// Byte offset into the content.
    pub content_byte_offset: usize,
    /// The anchored text was deleted; applied where it was.
    pub detached: bool,
}

/// What became of an override in a compilation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OverrideStatus {
    pub id: String,
    /// Injected; `false`: not applicable at its position (see the
    /// diagnostics).
    pub applied: bool,
    pub detached: bool,
    /// Where it is on the pages, if applied and the compilation produced
    /// pages: the page (0-based) and, in points from its top left corner,
    /// the top of the block after a break, the top of a space, or a figure's
    /// box (its size if known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x_pt: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y_pt: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width_pt: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height_pt: Option<f64>,
}

/// A change to the compiled copy of the content: replaces the content bytes
/// `range` with `text` (an insertion if empty). Made for override `id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
    pub id: String,
}

/// The edits for a compilation, sorted and non-overlapping, with a status
/// (and possibly diagnostics) per override.
#[derive(Debug, Default)]
pub struct Plan {
    pub edits: Vec<Edit>,
    pub statuses: Vec<OverrideStatus>,
    pub diagnostics: Vec<Diagnostic>,
    /// The content offset of each placed figure's call, by override id.
    pub figures: Vec<(String, usize)>,
    /// The content offset of each `image` call with a quality of its own,
    /// by override id.
    pub images: Vec<(String, usize, ImageQuality)>,
    /// The content offset of each `table` call with a style of its own, by
    /// override id.
    pub tables: Vec<(String, usize)>,
}

/// Plans the injection of `overrides` into `content`. Block kinds go on
/// their own line at the start of the paragraph or block holding their
/// position ([`Scan::block_start`]), if that line starts at block level in
/// markup (not inside raw text, a comment,
/// math, code or a content argument); a figure placement sets the
/// `placement:` argument of the figure call at (or starting on the line of)
/// its position; an image quality injects nothing but needs an `image` call
/// at or around its position (it applies at export); a table style gives
/// the `table` call at its position (see [`Scan::table_at`]) the style's
/// arguments.
///
/// Every `table` call whose style (its own, else `tables`, the layout's
/// default) is academic also gets the rules that need its rows: a thin one
/// after its `table.header(...)` and a thick one after its last cell (see
/// [`layout::preamble`]); those of the default are made for no override
/// (an empty id).
pub fn plan(content: &str, overrides: &[ResolvedOverride], tables: TableStyle) -> Plan {
    let mut out = Plan::default();
    let academic = tables == TableStyle::Academic
        && (content.contains("table(") || content.contains("table["));
    if overrides.is_empty() && !academic {
        return out;
    }
    let scan = Scan::new(content);
    let mut order: Vec<&ResolvedOverride> = overrides.iter().collect();
    order.sort_by(|a, b| {
        (a.content_byte_offset, a.kind.order(), &a.id).cmp(&(
            b.content_byte_offset,
            b.kind.order(),
            &b.id,
        ))
    });
    let mut figures = HashSet::new();
    let mut images = HashSet::new();
    let mut styled = HashSet::new();
    let mut edits: Vec<(Edit, u8)> = Vec::new();
    for o in order {
        let pos = floor_boundary(content, o.content_byte_offset);
        if let Kind::TableStyle { style } = o.kind {
            let found = scan.table_at(pos).filter(|&t| styled.insert(t));
            let table = found.map(|t| scan.table_edits(t, Some(style), &o.id));
            let applied = table.as_ref().is_some_and(|e| !e.is_empty());
            if let (Some(t), true) = (found, applied) {
                out.tables.push((o.id.clone(), t));
            }
            edits.extend(table.into_iter().flatten().map(|e| (e, o.kind.order())));
            if !applied {
                out.diagnostics.push(warning(
                    format!(
                        "This {} is not applied: there is no table at its position (or another \
                         table style already applies to it).",
                        o.kind.label()
                    ),
                    &o.id,
                ));
            }
            out.status(o, applied);
            continue;
        }
        let edit = match o.kind {
            _ if o.kind.is_block() => {
                let line = scan.block_start(pos);
                scan.is_block_level(line).then(|| Edit {
                    range: line..line,
                    text: o.kind.block_code(&o.id),
                    id: o.id.clone(),
                })
            }
            Kind::FigurePlacement { placement } => match scan.figure_at(pos) {
                Some(figure) if figures.insert(figure) => {
                    let edit = scan.placement_edit(figure, placement, &o.id);
                    if edit.is_some() {
                        out.figures.push((o.id.clone(), figure));
                    }
                    edit
                }
                _ => None,
            },
            // Applied at export: nothing to inject.
            Kind::ImageQuality { quality } => {
                if let Some(image) = scan.image_at(pos).filter(|&i| images.insert(i)) {
                    out.images.push((o.id.clone(), image, quality));
                }
                None
            }
            _ => None,
        };
        let applied = edit.is_some() || out.images.last().is_some_and(|i| i.0 == o.id);
        if let Some(edit) = edit {
            edits.push((edit, o.kind.order()));
        } else if !applied {
            let why = match o.kind {
                Kind::FigurePlacement { .. } => {
                    "there is no figure at its position (or another placement already applies to it)"
                }
                Kind::ImageQuality { .. } => {
                    "there is no image at its position (or another image quality already applies to it)"
                }
                _ => {
                    "its position is inside raw text, a comment, math, code or a function's argument"
                }
            };
            out.diagnostics.push(warning(
                format!("This {} is not applied: {why}.", o.kind.label()),
                &o.id,
            ));
        }
        out.status(o, applied);
    }
    if academic {
        // The default's rules, for the tables without a style of their own.
        let order = Kind::TableStyle {
            style: TableStyle::Academic,
        }
        .order();
        for table in scan.tables() {
            if !styled.contains(&table) {
                edits.extend(
                    scan.table_edits(table, None, "")
                        .into_iter()
                        .map(|e| (e, order)),
                );
            }
        }
    }
    // Stable: the rules of a table at the same place keep their order.
    edits.sort_by_key(|(e, order)| (e.range.start, e.range.end, *order));
    out.edits = edits.into_iter().map(|(e, _)| e).collect();
    out
}

impl Plan {
    /// Records what became of `o` (and a warning if it is detached).
    fn status(&mut self, o: &ResolvedOverride, applied: bool) {
        if o.detached {
            self.diagnostics.push(warning(
                format!(
                    "The text this {} was anchored to was deleted; it applies where that text was.",
                    o.kind.label()
                ),
                &o.id,
            ));
        }
        self.statuses.push(OverrideStatus {
            id: o.id.clone(),
            applied,
            detached: o.detached,
            ..OverrideStatus::default()
        });
    }
}

fn warning(message: String, id: &str) -> Diagnostic {
    Diagnostic {
        severity: Severity::Warning,
        message,
        hints: Vec::new(),
        location: Location::Override { id: id.to_owned() },
    }
}

/// The start of the paragraph or block holding byte `pos` of `content`
/// (where a block override at `pos` applies). Parses `content`; see
/// [`Scan::block_start`].
pub fn block_start(content: &str, pos: usize) -> usize {
    Scan::new(content).block_start(pos)
}

/// The start of the line holding byte `pos` of `text`.
fn line_start(text: &str, pos: usize) -> usize {
    let pos = floor_boundary(text, pos);
    text[..pos].rfind('\n').map_or(0, |i| i + 1)
}

/// The start of the Typst paragraph (or block) holding byte `pos` of
/// `text`, parsed as `root`: the start of its line, moved back over the
/// lines before it while they continue the same paragraph. A line break
/// continues it if it is a space in the top-level markup (not a blank line,
/// nor inside a list item, raw text, math, code, ...), the line before
/// starts at top level too, and neither side is block syntax (see
/// [`is_block_syntax`]). A forced line break (`\`) continues it.
pub(crate) fn block_start_in(root: &LinkedNode, text: &str, pos: usize) -> usize {
    let mut start = line_start(text, pos);
    if start == 0 {
        return 0;
    }
    let Some(mut space) = root.leaf_at(start - 1, Side::After) else {
        return start;
    };
    loop {
        let top_level = space.kind() == SyntaxKind::Space
            && space.parent().is_some_and(|p| p.parent().is_none());
        if !top_level {
            return start;
        }
        let (Some(mut before), Some(after)) = (space.prev_sibling(), space.next_sibling()) else {
            return start;
        };
        // A label belongs to what it follows (`$ x $ <eq>`).
        if before.kind() == SyntaxKind::Label {
            match before.prev_sibling() {
                Some(b) => before = b,
                None => return start,
            }
        }
        if is_block_syntax(&before, text) || is_block_syntax(&after, text) {
            return start;
        }
        // The line break before the previous line: between top-level
        // nodes, else that line starts inside a node.
        let mut node = space;
        let found = loop {
            match node.prev_sibling_with_trivia() {
                None => return 0,
                Some(n) => node = n,
            }
            if let Some(i) = text[node.range()].rfind('\n') {
                break node.offset() + i + 1;
            }
        };
        if !matches!(node.kind(), SyntaxKind::Space | SyntaxKind::Parbreak) {
            return start;
        }
        start = found;
        space = node;
    }
}

/// Whether a top-level node next to a line break is a block of its own: a
/// heading, a list, enum or term item, a display equation, a raw block, or
/// a `#` expression starting its line (conservative: `#emph[..]` there
/// counts too).
fn is_block_syntax(node: &LinkedNode, text: &str) -> bool {
    match node.kind() {
        SyntaxKind::Heading
        | SyntaxKind::ListItem
        | SyntaxKind::EnumItem
        | SyntaxKind::TermItem
        | SyntaxKind::Hash => true,
        SyntaxKind::Equation => node.cast::<ast::Equation>().is_some_and(|e| e.block()),
        SyntaxKind::Raw => node.cast::<ast::Raw>().is_some_and(|r| r.block()),
        _ => node.prev_sibling().is_some_and(|hash| {
            hash.kind() == SyntaxKind::Hash
                && text[line_start(text, hash.offset())..hash.offset()]
                    .trim()
                    .is_empty()
        }),
    }
}

fn floor_boundary(s: &str, mut pos: usize) -> usize {
    pos = pos.min(s.len());
    while !s.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

/// The parsed content, to check where overrides can apply.
pub struct Scan<'a> {
    content: &'a str,
    root: SyntaxNode,
}

impl<'a> Scan<'a> {
    /// Parses `content`.
    pub fn new(content: &'a str) -> Self {
        Self {
            content,
            root: typst::syntax::parse(content),
        }
    }

    /// The start of the paragraph or block holding byte `pos` (see
    /// [`block_start_in`]).
    pub fn block_start(&self, pos: usize) -> usize {
        block_start_in(&LinkedNode::new(&self.root), self.content, pos)
    }

    /// Whether a block can be inserted at `line_start`: at the end, or the
    /// syntax there starts a node directly in the top-level markup (or is
    /// whitespace there).
    pub fn is_block_level(&self, line_start: usize) -> bool {
        is_block_level(&LinkedNode::new(&self.root), line_start)
    }

    /// The offset of the `figure(...)` call containing byte `pos`, else of
    /// the first one starting on its line.
    pub fn figure_at(&self, pos: usize) -> Option<usize> {
        let root = LinkedNode::new(&self.root);
        if let Some(f) = enclosing_figure(&root, pos) {
            return Some(f.offset());
        }
        first_call_on(&root, line_of(self.content, pos), "figure").map(|f| f.offset())
    }

    /// The offset of the `image(...)` call containing byte `pos`.
    pub fn image_at(&self, pos: usize) -> Option<usize> {
        enclosing_call(&LinkedNode::new(&self.root), pos, "image").map(|i| i.offset())
    }

    /// The offset of the `table(...)` call containing byte `pos`, else of
    /// the first one in the figure there (see [`Scan::figure_at`]), else of
    /// the first one starting on its line.
    pub fn table_at(&self, pos: usize) -> Option<usize> {
        let root = LinkedNode::new(&self.root);
        if let Some(t) = enclosing_call(&root, pos, "table") {
            return Some(t.offset());
        }
        let figure = self
            .figure_at(pos)
            .and_then(|f| enclosing_figure(&root, f))
            .and_then(|f| first_call_in(&f, "table"));
        figure
            .or_else(|| first_call_on(&root, line_of(self.content, pos), "table"))
            .map(|t| t.offset())
    }

    /// The offsets of every `table(...)` call, in order.
    fn tables(&self) -> Vec<usize> {
        fn walk(node: &LinkedNode, out: &mut Vec<usize>) {
            if is_call(node, "table") {
                out.push(node.offset());
            }
            for child in node.children() {
                walk(&child, out);
            }
        }
        let mut out = Vec::new();
        walk(&LinkedNode::new(&self.root), &mut out);
        out
    }

    /// The edits of the `table` call at `table` for override `id`: the
    /// arguments of `style` (if any) as its first ones, and for an
    /// academic table (`style`, else the default asked for with `None`) the
    /// rule after its header and the one after its last argument. The
    /// rules need the arguments in parentheses (no trailing content
    /// blocks).
    fn table_edits(&self, table: usize, style: Option<TableStyle>, id: &str) -> Vec<Edit> {
        let root = LinkedNode::new(&self.root);
        let Some(args) = enclosing_call(&root, table, "table")
            .and_then(|call| call.children().find(|c| c.kind() == SyntaxKind::Args))
        else {
            return Vec::new();
        };
        let edit = |at: usize, text: String| Edit {
            range: at..at,
            text,
            id: id.into(),
        };
        let mut out = Vec::new();
        let children: Vec<LinkedNode> = args.children().collect();
        let paren = children
            .first()
            .filter(|c| c.kind() == SyntaxKind::LeftParen);
        if let Some(style) = style {
            let name = style.name();
            out.push(match paren {
                Some(p) => edit(p.range().end, format!("..lethora-tables.{name}, ")),
                None => edit(args.offset(), format!("(..lethora-tables.{name})")),
            });
        }
        if style.unwrap_or(TableStyle::Academic) != TableStyle::Academic || paren.is_none() {
            return out;
        }
        let Some(close) = children
            .iter()
            .position(|c| c.kind() == SyntaxKind::RightParen)
        else {
            return out;
        };
        if close + 1 != children.len() {
            return out;
        }
        if let Some(header) = children[..close].iter().find(|c| is_header_call(c)) {
            out.push(edit(
                header.range().end,
                ", lethora-table-rules.head".into(),
            ));
        }
        let last = children[..close]
            .iter()
            .rev()
            .find(|c| !c.kind().is_trivia())
            .map(|c| c.kind());
        let comma = if matches!(last, Some(SyntaxKind::Comma | SyntaxKind::LeftParen)) {
            ""
        } else {
            ", "
        };
        out.push(edit(
            children[close].offset(),
            format!("{comma}lethora-table-rules.bottom"),
        ));
        out
    }

    fn placement_edit(&self, figure: usize, placement: Placement, id: &str) -> Option<Edit> {
        let root = LinkedNode::new(&self.root);
        let call = enclosing_figure(&root, figure)?;
        placement_edit(&call, placement, id)
    }
}

pub(crate) fn is_block_level(root: &LinkedNode, line_start: usize) -> bool {
    let Some(mut node) = root.leaf_at(line_start, Side::After) else {
        return line_start >= root.len();
    };
    while let Some(parent) = node.parent() {
        if parent.parent().is_none() {
            // `node` is a child of the top-level markup.
            return parent.kind() == SyntaxKind::Markup
                && (node.offset() == line_start
                    || matches!(node.kind(), SyntaxKind::Space | SyntaxKind::Parbreak));
        }
        if parent.offset() != line_start {
            return false;
        }
        node = parent.clone();
    }
    false
}

/// Whether `node` is a call of the function named `name`.
fn is_call(node: &LinkedNode, name: &str) -> bool {
    node.kind() == SyntaxKind::FuncCall
        && node
            .children()
            .next()
            .is_some_and(|c| c.kind() == SyntaxKind::Ident && c.leaf_text() == name)
}

/// Whether `node` is a `table.header(...)` call.
fn is_header_call(node: &LinkedNode) -> bool {
    node.kind() == SyntaxKind::FuncCall
        && node.children().next().is_some_and(|callee| {
            callee.kind() == SyntaxKind::FieldAccess
                && callee
                    .children()
                    .filter(|c| c.kind() == SyntaxKind::Ident)
                    .map(|c| c.leaf_text().clone())
                    .eq(["table", "header"])
        })
}

/// The bytes of the line holding byte `pos` of `text` (without its break).
fn line_of(text: &str, pos: usize) -> Range<usize> {
    let start = line_start(text, pos);
    let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    start..end
}

/// The innermost `figure(...)` call containing byte `pos`.
pub(crate) fn enclosing_figure<'a>(root: &LinkedNode<'a>, pos: usize) -> Option<LinkedNode<'a>> {
    enclosing_call(root, pos, "figure")
}

/// The innermost call of `name` containing byte `pos`.
pub(crate) fn enclosing_call<'a>(
    root: &LinkedNode<'a>,
    pos: usize,
    name: &str,
) -> Option<LinkedNode<'a>> {
    let mut node = root.leaf_at(pos, Side::After)?;
    loop {
        if is_call(&node, name) {
            return Some(node);
        }
        node = node.parent()?.clone();
    }
}

/// The first call of `name` inside `node` (or `node` itself).
pub(crate) fn first_call_in<'a>(node: &LinkedNode<'a>, name: &str) -> Option<LinkedNode<'a>> {
    if is_call(node, name) {
        return Some(node.clone());
    }
    node.children().find_map(|c| first_call_in(&c, name))
}

/// The compiled-source offset of the `image` call that made the image of
/// `span`, if it is in `source`.
pub(crate) fn image_call(source: &Source, span: Span) -> Option<usize> {
    let node = source.find(span)?;
    let mut node = Some(&node);
    while let Some(n) = node {
        if is_call(n, "image") {
            return Some(n.offset());
        }
        node = n.parent();
    }
    None
}

/// The first call of `name` inside `node` that starts in `line`.
fn first_call_on<'a>(
    node: &LinkedNode<'a>,
    line: Range<usize>,
    name: &str,
) -> Option<LinkedNode<'a>> {
    for child in node.children() {
        let range = child.range();
        if range.end <= line.start {
            continue;
        }
        if range.start >= line.end {
            break;
        }
        if is_call(&child, name) && line.contains(&child.offset()) {
            return Some(child);
        }
        if let Some(found) = first_call_on(&child, line.clone(), name) {
            return Some(found);
        }
    }
    None
}

/// Sets the `placement:` argument of `call`: replaces its value, or adds it.
fn placement_edit(call: &LinkedNode, placement: Placement, id: &str) -> Option<Edit> {
    let args = call.children().find(|c| c.kind() == SyntaxKind::Args)?;
    let value = placement.typst();
    for named in args.children().filter(|c| c.kind() == SyntaxKind::Named) {
        let mut parts = named.children();
        if !parts
            .next()
            .is_some_and(|n| n.kind() == SyntaxKind::Ident && n.leaf_text() == "placement")
        {
            continue;
        }
        let expr = parts
            .skip_while(|c| c.kind() != SyntaxKind::Colon)
            .skip(1)
            .find(|c| !c.kind().is_trivia())?;
        return Some(Edit {
            range: expr.range(),
            text: value.into(),
            id: id.into(),
        });
    }
    let first = args.children().next()?;
    Some(if first.kind() == SyntaxKind::LeftParen {
        let at = first.range().end;
        Edit {
            range: at..at,
            text: format!("placement: {value}, "),
            id: id.into(),
        }
    } else {
        // Only trailing content blocks: `figure[...]`.
        let at = args.offset();
        Edit {
            range: at..at,
            text: format!("(placement: {value})"),
            id: id.into(),
        }
    })
}

// ---- Positions on the pages ----

/// Fills in where the applied overrides of `statuses` are in `document`,
/// compiled from `source` (mapped by `map`): block overrides by their
/// markers, figure placements (`figures`, see [`Plan`]) by the figure made
/// by the call, matched by its source range, image qualities (`images`) by
/// the first image their call made, table styles (`tables`) like figures by
/// the table their call made.
pub(crate) fn locate(
    statuses: &mut [OverrideStatus],
    document: &PagedDocument,
    source: &Source,
    map: &SourceMap,
    figures: &[(String, usize)],
    images: &[(String, usize, ImageQuality)],
    tables: &[(String, usize)],
) {
    let index: HashMap<String, usize> = statuses
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.clone(), i))
        .collect();
    let introspector = document.introspector();
    let elements = introspector.elements();
    let label = Label::new(PicoStr::intern(MARKER)).expect("not empty");
    for marker in introspector.query(&Selector::Label(label)).iter() {
        let Some(meta) = marker.to_packed::<MetadataElem>() else {
            continue;
        };
        let (id, height) = match &meta.value {
            foundations::Value::Str(id) => (id.as_str(), None),
            foundations::Value::Array(a) => match a.as_slice() {
                [foundations::Value::Str(id), foundations::Value::Float(h)] => {
                    (id.as_str(), Some(h.max(0.0)))
                }
                _ => continue,
            },
            _ => continue,
        };
        let (Some(&i), Some(loc)) = (index.get(id), marker.location()) else {
            continue;
        };
        let Some(mut pos) = elements.position(loc).copied() else {
            continue;
        };
        if pos.point == Point::zero() {
            // Tags between pages (before a block that starts with a page
            // break of its own, like a chapter heading) sit in the corner of
            // the next page, maybe a blank one: take the next element, but
            // not a page header's `context`.
            let next = elements
                .all()
                .enumerate()
                .skip(elements.loc_index(&loc).saturating_add(1))
                .map(|(k, c)| (c, elements.get_pos_by_idx(k)))
                .find(|(c, p)| !c.is::<ContextElem>() && p.point != Point::zero());
            if let Some((_, p)) = next {
                pos = *p;
            }
        }
        let s = &mut statuses[i];
        let y = pos.point.y.to_pt();
        s.page = Some(pos.page.get() - 1);
        s.x_pt = Some(pos.point.x.to_pt());
        // A space's marker is below it.
        s.y_pt = Some(height.map_or(y, |h| (y - h).max(0.0)));
        s.height_pt = height;
    }

    if !images.is_empty() {
        let mut calls: HashMap<usize, usize> = images
            .iter()
            .filter_map(|(id, call, _)| Some((map.source_offset(*call), *index.get(id)?)))
            .collect();
        for (k, page) in document.pages().iter().enumerate() {
            crate::images::visit(
                &page.frame,
                Transform::identity(),
                &mut |_, size, span, ts| {
                    let Some(i) = image_call(source, span).and_then(|c| calls.remove(&c)) else {
                        return;
                    };
                    let b = transformed(Rect::new(Point::zero(), size.to_point()), ts);
                    let s = &mut statuses[i];
                    s.page = Some(k);
                    s.x_pt = Some(b.min.x.to_pt());
                    s.y_pt = Some(b.min.y.to_pt());
                    s.width_pt = Some((b.max.x - b.min.x).to_pt());
                    s.height_pt = Some((b.max.y - b.min.y).to_pt());
                },
            );
            if calls.is_empty() {
                break;
            }
        }
    }

    locate_calls(
        statuses,
        &index,
        document,
        source,
        map,
        figures,
        "figure",
        Element::of::<FigureElem>(),
    );
    locate_calls(
        statuses,
        &index,
        document,
        source,
        map,
        tables,
        "table",
        Element::of::<TableElem>(),
    );
}

/// Fills in the boxes of the elements of kind `element` made by the calls
/// of `name` at `calls` (content offset by override id).
#[allow(clippy::too_many_arguments)]
fn locate_calls(
    statuses: &mut [OverrideStatus],
    index: &HashMap<String, usize>,
    document: &PagedDocument,
    source: &Source,
    map: &SourceMap,
    calls: &[(String, usize)],
    name: &str,
    element: Element,
) {
    if calls.is_empty() {
        return;
    }
    let introspector = document.introspector();
    let elements = introspector.elements();
    let root = LinkedNode::new(source.root());
    let mut ranges: HashMap<Range<usize>, usize> = HashMap::new();
    for (id, offset) in calls {
        let call = enclosing_call(&root, map.source_offset(*offset), name);
        if let (Some(&i), Some(call)) = (index.get(id), call) {
            ranges.insert(call.range(), i);
        }
    }
    let query = introspector.query(&element.select());
    for figure in query.iter() {
        let Some(range) = source.find(figure.span()).map(|n| n.range()) else {
            continue;
        };
        let (Some(i), Some(loc)) = (ranges.remove(&range), figure.location()) else {
            continue;
        };
        let Some(at) = elements.position(loc).copied() else {
            continue;
        };
        let s = &mut statuses[i];
        match figure_box(document, loc, float_pages(at.page)) {
            Some((k, b)) => {
                s.page = Some(k);
                s.x_pt = Some(b.min.x.to_pt());
                s.y_pt = Some(b.min.y.to_pt());
                s.width_pt = Some((b.max.x - b.min.x).to_pt());
                s.height_pt = Some((b.max.y - b.min.y).to_pt());
            }
            None => {
                s.page = Some(at.page.get() - 1);
                s.x_pt = Some(at.point.x.to_pt());
                s.y_pt = Some(at.point.y.to_pt());
            }
        }
    }
}

/// The pages (0-based) a figure positioned in the text on page `at`
/// (1-based) can be drawn on: a float is on that page, the one before (the
/// text after the figure went on) or a later one.
pub(crate) fn float_pages(at: NonZeroUsize) -> Range<usize> {
    let start = at.get().saturating_sub(2);
    start..start + FLOAT_PAGES
}

/// The first of `pages` (0-based) showing figure `loc`, and the figure's
/// bounds there. A float is a group of the figure's `place` element.
pub(crate) fn figure_box(
    document: &PagedDocument,
    loc: introspection::Location,
    pages: Range<usize>,
) -> Option<(usize, Rect)> {
    let elements = document.introspector().elements();
    let place = elements
        .loc_range(&loc)
        .find_map(|k| elements.get_by_idx(k).to_packed::<PlaceElem>()?.location());
    let all = document.pages();
    let start = pages.start;
    let pages = all.get(start..pages.end.min(all.len()))?;
    pages.iter().enumerate().find_map(|(k, page)| {
        let mut bounds = None;
        figure_bounds(
            &page.frame,
            Transform::identity(),
            (loc, place),
            &mut false,
            &mut bounds,
        );
        bounds.map(|b| (start + k, b))
    })
}

/// The box of a text run, relative to its baseline start: ascender to
/// descender.
pub(crate) fn text_box(text: &TextItem) -> Rect {
    let m = text.font.metrics();
    Rect::new(
        Point::new(Abs::zero(), -m.ascender.at(text.size)),
        Point::new(text.width(), -m.descender.at(text.size)),
    )
}

/// The bounding box of `rect` placed by `ts`.
pub(crate) fn transformed(rect: Rect, ts: Transform) -> Rect {
    let corners = [
        rect.min,
        rect.max,
        Point::new(rect.min.x, rect.max.y),
        Point::new(rect.max.x, rect.min.y),
    ]
    .map(|c| c.transform(ts));
    corners[1..]
        .iter()
        .fold(Rect::new(corners[0], corners[0]), |b, p| {
            Rect::new(b.min.min(*p), b.max.max(*p))
        })
}

/// Grows `bounds` by the visible items of `frame` (placed by `ts`) that
/// belong to figure `fig.0`: between its start and end tags (`inside`), or
/// in the float of its `place` element `fig.1`.
fn figure_bounds(
    frame: &Frame,
    ts: Transform,
    fig: (introspection::Location, Option<introspection::Location>),
    inside: &mut bool,
    bounds: &mut Option<Rect>,
) {
    for (pos, item) in frame.items() {
        let rect = match item {
            FrameItem::Tag(tag) => {
                if tag.location() == fig.0 {
                    *inside = matches!(tag, Tag::Start(..));
                }
                continue;
            }
            FrameItem::Group(group) => {
                let ts = ts
                    .pre_concat(Transform::translate(pos.x, pos.y))
                    .pre_concat(group.transform);
                if fig.1.is_some() && group.parent.map(|p| p.location) == fig.1 {
                    figure_bounds(&group.frame, ts, fig, &mut true, bounds);
                } else {
                    figure_bounds(&group.frame, ts, fig, inside, bounds);
                }
                continue;
            }
            _ if !*inside => continue,
            FrameItem::Text(text) => text_box(text),
            FrameItem::Shape(shape, _) => shape.geometry.bbox(shape.stroke.as_ref()),
            FrameItem::Image(_, size, _) => Rect::new(Point::zero(), size.to_point()),
            FrameItem::Link(..) => continue,
        };
        let r = transformed(rect, ts.pre_concat(Transform::translate(pos.x, pos.y)));
        let b = bounds.get_or_insert(r);
        *b = Rect::new(b.min.min(r.min), b.max.max(r.max));
    }
}

fn clip(s: &str) -> String {
    let mut out: String = s.chars().take(40).collect();
    if out.len() < s.len() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn params(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn validation_and_round_trip() {
        assert_eq!(
            validate_new("page_break", &Map::new()),
            Ok(Kind::PageBreak { to_odd: false })
        );
        let space = validate_new("space_before", &params(json!({"length": " 12PT"}))).unwrap();
        assert_eq!(space.params()["length"], json!("12pt"));
        assert!(validate_new("space_before", &params(json!({"length": "300pt"}))).is_err());
        assert!(validate_new("space_before", &params(json!({"length": "2em"}))).is_ok());
        assert!(validate_new("space_before", &params(json!({"length": "1pt)#x"}))).is_err());
        assert!(validate_new("page_break", &params(json!({"to_odd": "yes"}))).is_err());
        assert!(validate_new("page_break", &params(json!({"x": 1}))).is_err());
        assert!(validate_new("figure_placement", &params(json!({"placement": "left"}))).is_err());
        let quality = validate_new("image_quality", &params(json!({"quality": "email"}))).unwrap();
        assert_eq!(
            quality,
            Kind::ImageQuality {
                quality: ImageQuality::Email
            }
        );
        assert_eq!(quality.params(), params(json!({"quality": "email"})));
        assert!(!quality.is_block());
        assert!(validate_new("image_quality", &params(json!({"quality": "high"}))).is_err());
        assert!(validate_new("image_quality", &Map::new()).is_err());
        assert!(validate_new("nope", &Map::new()).is_err());

        let o = Override {
            id: "o1".into(),
            kind: Kind::FigurePlacement {
                placement: Placement::Bottom,
            },
            anchor: "00ab".into(),
        };
        let mut entries = o.entries();
        assert_eq!(entries["o1.placement"], json!("bottom"));
        entries.insert("bad.kind".into(), json!("column_break"));
        entries.insert("o2.kind".into(), json!("space_before"));
        entries.insert("o2.anchor".into(), json!("ff"));
        entries.insert("o2.length".into(), json!("1000pt"));
        entries.insert("plain".into(), json!(1));
        let (valid, invalid) = parse(&entries);
        assert_eq!(valid, vec![o]);
        let ids: Vec<&str> = invalid.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["plain", "bad", "o2"], "{invalid:?}");
        assert_eq!(keys_of(&entries, "o2").len(), 3);
    }

    #[test]
    fn ids_are_short_and_unique() {
        let id = new_id(|_| false);
        assert!(is_id(&id) && id.len() == 9 && id.starts_with('o'));
        let again = new_id(|i| i == id);
        assert_ne!(again, id);
    }

    fn block(content: &str, at: &str) -> bool {
        let pos = content.find(at).unwrap();
        Scan::new(content).is_block_level(block_start(content, pos))
    }

    #[test]
    fn block_level_positions() {
        let doc = "= Head\n\nPara one\nline two\n\n- item\n\n```\nraw\n```\n\n$ x\n= y $\n\n/* a\nb */\n#{\nlet x = 1\n}\n#figure(\n  [x],\n)\n  indented\n";
        for ok in [
            "= Head",
            "Para one",
            "line two",
            "- item",
            "```\nraw",
            "/* a",
            "#{",
            "#figure",
            "  indented",
        ] {
            assert!(block(doc, ok), "{ok}");
        }
        for bad in ["raw\n", "= y", "b */", "let x", "  [x]"] {
            assert!(!block(doc, bad), "{bad}");
        }
        assert!(Scan::new("a\n").is_block_level(2));
        assert!(Scan::new("").is_block_level(0));
    }

    #[test]
    fn block_start_is_the_paragraph_start() {
        const LISTS: &str = "Text\n- one\n- two\n+ three\n/ T: d\nafter";
        const FIGURE: &str = "Text\n#figure(\n  [x],\n) <f>\nafter";
        // (content, position at, the line its block starts with)
        let cases = [
            ("Para one\nline two\nline three\n", "line three", "Para one"),
            ("Para one\nline two\n", "line two", "Para one"),
            ("Para one\nline two\n", "Para one", "Para one"),
            ("A\n\nAfter blank\nmore", "more", "After blank"),
            ("A\n  \nAfter spaces\nmore", "more", "After spaces"),
            ("Text\n= Head\nText under\nmore", "= Head", "= Head"),
            ("Text\n= Head\nText under\nmore", "more", "Text under"),
            (LISTS, "- one", "- one"),
            (LISTS, "- two", "- two"),
            (LISTS, "+ three", "+ three"),
            (LISTS, "/ T", "/ T: d"),
            (LISTS, "after", "after"),
            ("- item\n  more", "more", "  more"),
            ("first \\\nafter break\nthird", "third", "first \\"),
            (FIGURE, "#figure", "#figure("),
            (FIGURE, "after", "after"),
            ("#figure(\n  [x],\n) text\nmore", "more", "more"),
            ("Text\n$ x $ <eq>\nafter", "$ x $", "$ x $ <eq>"),
            ("Text\n$ x $ <eq>\nafter", "after", "after"),
            ("Text with $x$\ninline eq", "inline eq", "Text with $x$"),
            ("Text\n```\nraw\n```\nafter", "after", "after"),
            ("#v(1em)\n#emph[a] tail\nmore", "more", "#emph[a] tail"),
            ("Text // note\nmore", "more", "Text // note"),
        ];
        for (content, at, line) in cases {
            let start = block_start(content, content.find(at).unwrap());
            let got = content[start..].lines().next().unwrap_or("");
            assert_eq!(got, line, "{content:?} at {at:?}");
        }
        assert_eq!(block_start("", 0), 0);
        assert_eq!(block_start("a\nb", 99), 0);
    }

    #[test]
    fn figure_lookup_and_placement_edits() {
        let content = "Text\n\n#figure(image(\"a.png\"), caption: [Cap]) <f>\n\n#figure(placement: none, [x])\n\n#figure[only]\n";
        let scan = Scan::new(content);
        let first = content.find("figure(image").unwrap();
        assert_eq!(scan.figure_at(content.find("Cap").unwrap()), Some(first));
        assert_eq!(scan.figure_at(content.find("<f>").unwrap()), Some(first));
        assert_eq!(
            scan.figure_at(content.find("#figure(image").unwrap()),
            Some(first)
        );
        assert_eq!(scan.figure_at(0), None);

        let edit = |at: &str, p| {
            let f = scan.figure_at(content.find(at).unwrap()).unwrap();
            let e = scan.placement_edit(f, p, "id").unwrap();
            let mut s = content.to_owned();
            s.replace_range(e.range, &e.text);
            s
        };
        assert!(edit("Cap", Placement::Top).contains("#figure(placement: top, image("));
        let replaced = edit("[x]", Placement::Bottom);
        assert!(
            replaced.contains("#figure(placement: bottom, [x])"),
            "{replaced}"
        );
        assert!(edit("only", Placement::Here).contains("#figure(placement: none)[only]"));
    }

    #[test]
    fn image_qualities_need_an_image_call_and_inject_nothing() {
        let content =
            "Text\n\n#figure(image(\"a.png\", width: 50%), caption: [Cap])\n\n#image(\"b.png\")\n";
        let at = |id: &str, needle: &str| ResolvedOverride {
            id: id.into(),
            kind: Kind::ImageQuality {
                quality: ImageQuality::Print,
            },
            content_byte_offset: content.find(needle).unwrap(),
            detached: false,
        };
        let a = content.find("image(\"a").unwrap();
        let b = content.find("image(\"b").unwrap();
        let scan = Scan::new(content);
        assert_eq!(scan.image_at(content.find("50%").unwrap()), Some(a));
        assert_eq!(scan.image_at(content.find("#figure").unwrap()), None);
        assert_eq!(scan.image_at(content.find("Cap").unwrap()), None);

        // In a call and at a call's start; another on the first image, and
        // one in the caption, are not applied.
        let plan = plan(
            content,
            &[
                at("i1", "50%"),
                at("i2", "image(\"b"),
                at("i3", "a.png"),
                at("i4", "Cap"),
            ],
            TableStyle::Academic,
        );
        assert!(plan.edits.is_empty());
        let applied: Vec<(&str, bool)> = plan
            .statuses
            .iter()
            .map(|s| (s.id.as_str(), s.applied))
            .collect();
        assert_eq!(
            applied,
            [("i3", true), ("i1", false), ("i4", false), ("i2", true)]
        );
        assert_eq!(
            plan.images,
            [
                ("i3".to_owned(), a, ImageQuality::Print),
                ("i2".to_owned(), b, ImageQuality::Print)
            ]
        );
        assert!(
            plan.diagnostics
                .iter()
                .all(|d| d.message.contains("no image"))
        );
        assert_eq!(plan.diagnostics.len(), 2);
    }

    /// `content` with the edits of `plan` applied.
    fn with_edits(content: &str, plan: &Plan) -> String {
        let mut s = content.to_owned();
        for e in plan.edits.iter().rev() {
            s.replace_range(e.range.clone(), &e.text);
        }
        s
    }

    #[test]
    fn table_lookup() {
        let content = "Text\n\n#figure(\n  table(columns: 2, [a], [b]),\n  caption: [Cap],\n) <tab:1>\n\n#table(columns: 1, [x]) #table[y]\n\n#figure(image(\"a.png\"))\n";
        let scan = Scan::new(content);
        let first = content.find("table(columns: 2").unwrap();
        let second = content.find("table(columns: 1").unwrap();
        // In the call, in its figure (caption, label, the figure's line).
        assert_eq!(scan.table_at(content.find("[b]").unwrap()), Some(first));
        assert_eq!(scan.table_at(content.find("Cap").unwrap()), Some(first));
        assert_eq!(
            scan.table_at(content.find("#figure(").unwrap()),
            Some(first)
        );
        // Starting on the line: the first.
        assert_eq!(
            scan.table_at(content.find("#table(").unwrap()),
            Some(second)
        );
        assert_eq!(
            scan.table_at(content.find("[y]").unwrap()),
            content.find("table[y]")
        );
        // Not `table.header`, `set table` or a figure without a table.
        assert_eq!(scan.table_at(0), None);
        assert_eq!(scan.table_at(content.find("a.png").unwrap()), None);
        let other = "#set table(stroke: none)\n#table.cell[x]\n";
        assert_eq!(Scan::new(other).tables(), Vec::<usize>::new());
    }

    #[test]
    fn table_styles_and_academic_rules() {
        let at = |id: &str, style, content: &str, needle: &str| ResolvedOverride {
            id: id.into(),
            kind: Kind::TableStyle { style },
            content_byte_offset: content.find(needle).unwrap(),
            detached: false,
        };
        let content = "#table(columns: 2, table.header([H], [I]), [a], [b])\n\n#table(\n  columns: 1,\n  [x],\n)\n\n#table(columns: 1)[y]\n";

        // The default academic: rules for every table that can take them.
        let planned = super::plan(content, &[], TableStyle::Academic);
        assert!(planned.edits.iter().all(|e| e.id.is_empty()));
        let out = with_edits(content, &planned);
        assert!(
            out.contains("table.header([H], [I]), lethora-table-rules.head, [a], [b], lethora-table-rules.bottom)"),
            "{out}"
        );
        assert!(out.contains("  [x],\nlethora-table-rules.bottom)"), "{out}");
        // Trailing content blocks: no rules.
        assert!(out.contains("#table(columns: 1)[y]"), "{out}");
        // Not academic: nothing to do without overrides.
        assert!(plan_is_empty(content, TableStyle::Grid));
        assert!(plan_is_empty(
            "No tables, a table word.",
            TableStyle::Academic
        ));

        // Exceptions: the style's arguments first; an academic one gets
        // rules too; a second on the same table is not applied.
        let overrides = [
            at("g", TableStyle::Grid, content, "[a]"),
            at("g2", TableStyle::Striped, content, "[b]"),
            at("p", TableStyle::Academic, content, "[x]"),
            at("y", TableStyle::Plain, content, "[y]"),
        ];
        let planned = super::plan(content, &overrides, TableStyle::Academic);
        let out = with_edits(content, &planned);
        assert!(
            out.starts_with(
                "#table(..lethora-tables.grid, columns: 2, table.header([H], [I]), [a], [b])\n"
            ),
            "{out}"
        );
        assert!(
            out.contains("#table(..lethora-tables.academic, \n  columns: 1,\n  [x],\nlethora-table-rules.bottom)"),
            "{out}"
        );
        assert!(
            out.contains("#table(..lethora-tables.plain, columns: 1)[y]"),
            "{out}"
        );
        let statuses: Vec<(&str, bool)> = planned
            .statuses
            .iter()
            .map(|s| (s.id.as_str(), s.applied))
            .collect();
        assert_eq!(
            statuses,
            [("g", true), ("g2", false), ("p", true), ("y", true)]
        );
        assert_eq!(planned.tables.len(), 3);
        assert!(planned.diagnostics[0].message.contains("no table"));
        assert_eq!(planned.diagnostics.len(), 1);
        // No parentheses at all.
        let planned = plan_of(
            "#table[z]\n",
            &[at("z", TableStyle::Grid, "#table[z]\n", "z")],
        );
        assert_eq!(
            with_edits("#table[z]\n", &planned),
            "#table(..lethora-tables.grid)[z]\n"
        );
    }

    fn plan_is_empty(content: &str, style: TableStyle) -> bool {
        let p = plan(content, &[], style);
        p.edits.is_empty() && p.statuses.is_empty()
    }

    fn plan_of(content: &str, overrides: &[ResolvedOverride]) -> Plan {
        plan(content, overrides, TableStyle::Plain)
    }

    #[test]
    fn table_style_validates() {
        let style = validate_new("table_style", &params(json!({"style": "grid"}))).unwrap();
        assert_eq!(
            style,
            Kind::TableStyle {
                style: TableStyle::Grid
            }
        );
        assert_eq!(style.params(), params(json!({"style": "grid"})));
        assert!(!style.is_block());
        assert_eq!(style.name(), "table_style");
        assert!(validate_new("table_style", &params(json!({"style": "fancy"}))).is_err());
        assert!(validate_new("table_style", &Map::new()).is_err());
        assert!(validate_new("table_style", &params(json!({"style": "grid", "x": 1}))).is_err());
    }
}
