//! Whole-document compilation: layout preamble + expert code + content
//! (+ bibliography) typeset into pages, rendered to SVG or exported to PDF.
//!
//! The API is plain data in ([`CompileRequest`]), plain data out
//! ([`CompileOutput`], apart from the document handle), so it can run in an
//! isolated worker process. File access is confined to the request's
//! bibliography, served as the virtual `references.bib`, and the images the
//! request's asset index names, read from their blobs in the project folder's
//! `assets/` (see [`crate::assets`]). No packages, no other Typst files.
//!
//! Anchored overrides ([`crate::overrides`]) are injected into the compiled
//! copy of the content; the [`SourceMap`] maps positions back. The export
//! image quality ([`crate::images`]) is kept with the compiled document and
//! applied when it is exported.

use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroUsize;
use std::ops::Range;
use std::path::PathBuf;
use std::thread;

use serde::{Deserialize, Serialize};
use typst::diag::{FileError, FileResult, SourceDiagnostic};
use typst::foundations::{Bytes, Datetime, Duration, Element};
use typst::introspection::{Introspector, PagedPosition};
use typst::layout::{Abs, Frame, FrameItem, Point, Transform};
use typst::model::FigureElem;
use typst::syntax::{
    DiagSpan, DiagSpanKind, FileId, LinkedNode, RootedPath, Side, Source, Span, SyntaxKind,
    SyntaxNode, VirtualPath, VirtualRoot,
};
use typst::text::{Font, FontBook, TextItem};
use typst::utils::LazyHash;
use typst::{Library, World};
use typst_kit::fonts::FontStore;
use typst_layout::PagedDocument;

use crate::assets;
use crate::fonts::{DocFonts, EMBEDDED_DOC_FONTS, LIBRARY, SYSTEM_DOC_FONTS};
use crate::layout::{self, ImageQuality, Layout, Meta, Preamble};
use crate::math::TYPESET_STACK;
use crate::overrides::{self, Edit, OverrideStatus, ResolvedOverride};

/// The path the bibliography is served at.
pub const REFERENCES: &str = "references.bib";
/// Largest bibliography served.
pub const MAX_REFERENCES_BYTES: u64 = 16 * 1024 * 1024;
/// Memoized results unused by the last compilation are dropped. At book size
/// (5M characters) every edit leaves gigabytes of stale results: keeping them
/// for 10 compilations ran a 16 GB cap out of memory within 7 edits; keeping
/// one generation holds ~3 GB and peaks at ~6 GB, with ~15% slower edits.
const EVICT_MAX_AGE: usize = 1;
/// A click off the text snaps to the nearest run of text at most this many
/// of the run's heights (ascender to descender, about a line) above or
/// below it.
const SNAP_LINES: f64 = 1.0;

/// What to compile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompileRequest {
    /// The project folder (its `assets/` holds the blobs).
    pub project_dir: PathBuf,
    pub layout: Layout,
    pub meta: Meta,
    /// Expert Typst code, placed after the generated rules.
    pub custom: String,
    /// The document text.
    pub content: String,
    /// The date `datetime.today()` returns; `None` makes it unavailable.
    pub today: Option<Date>,
    /// Anchored overrides, resolved against `content`.
    #[serde(default)]
    pub overrides: Vec<ResolvedOverride>,
    /// The sources (BibLaTeX), served as [`REFERENCES`]; the bibliography is
    /// printed only if this is not blank and the content cites.
    #[serde(default)]
    pub bibliography: String,
    /// The asset index: path used in the text (`assets/...`) → blob name.
    #[serde(default)]
    pub assets: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Date {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

/// A compiled document: summary, problems, and the pages if compilation
/// succeeded.
pub struct CompileOutput {
    pub pages: usize,
    /// Width and height of each page in points.
    pub page_sizes: Vec<[f64; 2]>,
    pub diagnostics: Vec<Diagnostic>,
    /// What became of each override (sorted by position).
    pub overrides: Vec<OverrideStatus>,
    pub document: Option<CompiledDocument>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub hints: Vec<String>,
    pub location: Location,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// Where a problem is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "in", rename_all = "lowercase")]
pub enum Location {
    /// In the document text: byte and UTF-16 offsets into the content.
    Content {
        start: usize,
        end: usize,
        start_utf16: usize,
        end_utf16: usize,
    },
    /// In the expert code: byte offsets into it.
    Custom { start: usize, end: usize },
    /// In the code generated from the layout settings.
    Preamble,
    /// In another project file (`references.bib`, an image).
    File { path: String },
    /// In the code injected for an override, or about an override.
    Override { id: String },
    /// Nowhere in particular.
    Unknown,
}

/// The parts of a compiled source, as byte ranges.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceMap {
    pub rules: Range<usize>,
    pub custom: Range<usize>,
    pub front: Range<usize>,
    /// The content with the injected edits.
    pub content: Range<usize>,
    pub back: Range<usize>,
    /// The edits inside `content`, in order.
    pub edits: Vec<MappedEdit>,
}

/// Where an [`Edit`] landed in the compiled source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappedEdit {
    /// The replaced content bytes.
    pub content: Range<usize>,
    /// The inserted text in the compiled source.
    pub source: Range<usize>,
    pub id: String,
}

impl SourceMap {
    /// The content offset of compiled-source offset `pos` (inside the
    /// content part): `Err` with the edit if `pos` is in injected text.
    pub fn content_offset(&self, pos: usize) -> Result<usize, &MappedEdit> {
        let i = self.edits.partition_point(|e| e.source.start <= pos);
        match i.checked_sub(1).map(|i| &self.edits[i]) {
            None => Ok(pos.saturating_sub(self.content.start)),
            Some(e) if pos < e.source.end => Err(e),
            Some(e) => Ok(e.content.end + (pos - e.source.end)),
        }
    }

    /// The compiled-source offset of content offset `pos` (after text
    /// injected at `pos`).
    pub fn source_offset(&self, pos: usize) -> usize {
        let i = self.edits.partition_point(|e| e.content.end <= pos);
        match i.checked_sub(1).map(|i| &self.edits[i]) {
            None => self.content.start + pos,
            Some(e) => e.source.end + (pos - e.content.end),
        }
    }

    /// Where the byte range `range` of the compiled source lies. Content
    /// locations are in bytes here; see [`Location::Content`].
    /// A range is placed by its start (an unclosed delimiter in the content
    /// may run to the end of the source) and clipped to that part.
    pub fn locate(&self, range: Range<usize>) -> Location {
        let within = |part: &Range<usize>| part.start <= range.start && range.start <= part.end;
        if within(&self.content) {
            let start = match self.content_offset(range.start) {
                Ok(start) => start,
                // Code injected for no override (the layout's): where it went.
                Err(e) if e.id.is_empty() => e.content.start,
                Err(e) => return Location::Override { id: e.id.clone() },
            };
            let end = self
                .content_offset(range.end.clamp(range.start, self.content.end))
                .unwrap_or_else(|e| e.content.start)
                .max(start);
            Location::Content {
                start,
                end,
                start_utf16: 0,
                end_utf16: 0,
            }
        } else if within(&self.custom) && !self.custom.is_empty() {
            Location::Custom {
                start: range.start - self.custom.start,
                end: range.end.min(self.custom.end) - self.custom.start,
            }
        } else {
            Location::Preamble
        }
    }
}

/// Assembles the main source: generated rules, expert code, front matter,
/// content with `edits` applied (sorted, non-overlapping; others are
/// skipped), and the bibliography call (if any), each on its own lines.
pub fn compile_source(
    preamble: &Preamble,
    custom: &str,
    content: &str,
    edits: &[Edit],
    bibliography: Option<&str>,
) -> (String, SourceMap) {
    let mut text = String::with_capacity(
        preamble.rules.len() + custom.len() + preamble.front.len() + content.len() + 200,
    );
    let part = |text: &mut String, s: &str| {
        let start = text.len();
        text.push_str(s);
        let range = start..text.len();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        range
    };
    let rules = part(&mut text, &preamble.rules);
    let custom = part(&mut text, custom);
    let front = part(&mut text, &preamble.front);
    // A blank line ends any paragraph the front matter may have started.
    text.push('\n');
    let content_start = text.len();
    let mut mapped = Vec::with_capacity(edits.len());
    let mut copied = 0;
    for e in edits {
        let ok = copied <= e.range.start
            && e.range.start <= e.range.end
            && e.range.end <= content.len()
            && content.is_char_boundary(e.range.start)
            && content.is_char_boundary(e.range.end)
            && !e.text.is_empty();
        if !ok {
            continue;
        }
        text.push_str(&content[copied..e.range.start]);
        let start = text.len();
        text.push_str(&e.text);
        mapped.push(MappedEdit {
            content: e.range.clone(),
            source: start..text.len(),
            id: e.id.clone(),
        });
        copied = e.range.end;
    }
    let content = part(&mut text, &content[copied..]);
    let content = content_start..content.end;
    text.push('\n');
    let back = part(&mut text, bibliography.unwrap_or(""));
    (
        text,
        SourceMap {
            rules,
            custom,
            front,
            content,
            back,
            edits: mapped,
        },
    )
}

/// Whether `content` cites a source: `#cite(...)` or an `@key` reference to a
/// key that is not a label in the content.
pub fn cites_sources(content: &str) -> bool {
    if !content.contains('@') && !content.contains("cite") {
        return false;
    }
    let root = typst::syntax::parse(content);
    let mut labels = std::collections::HashSet::new();
    let mut refs = Vec::new();
    let mut cite_call = false;
    walk(&root, &mut |node| match node.kind() {
        SyntaxKind::Label => {
            labels.insert(node.leaf_text().trim_matches(['<', '>']).to_owned());
        }
        SyntaxKind::RefMarker => refs.push(node.leaf_text().trim_start_matches('@').to_owned()),
        SyntaxKind::FuncCall => {
            cite_call |= node
                .children()
                .next()
                .is_some_and(|c| c.kind() == SyntaxKind::Ident && c.leaf_text().as_str() == "cite");
        }
        _ => {}
    });
    cite_call || refs.iter().any(|r| !labels.contains(r))
}

fn walk(node: &SyntaxNode, f: &mut impl FnMut(&SyntaxNode)) {
    f(node);
    for child in node.children() {
        walk(child, f);
    }
}

/// Compiles documents, keeping the last source so the next compilation
/// reparses and re-lays out only what changed.
pub struct DocCompiler {
    source: Option<Source>,
    pub(crate) fonts: &'static DocFonts,
}

impl Default for DocCompiler {
    fn default() -> Self {
        Self::new()
    }
}

/// Compiles `request` once (see [`DocCompiler`]).
pub fn compile(request: &CompileRequest) -> CompileOutput {
    let out = DocCompiler::new().compile(request);
    evict_cache();
    out
}

/// Drops memoized results the last compile didn't use. Call it after every
/// [`DocCompiler::compile`]; the worker does so after sending the result, so
/// the eviction (up to ~0.5 s at book size) doesn't delay the preview.
pub fn evict_cache() {
    typst::comemo::evict(EVICT_MAX_AGE);
}

impl DocCompiler {
    /// A compiler with the embedded fonts only.
    pub fn new() -> Self {
        Self {
            source: None,
            fonts: &EMBEDDED_DOC_FONTS,
        }
    }

    /// A compiler with the embedded and the system's fonts. The system is
    /// searched once per process, at the first compilation.
    pub fn with_system_fonts() -> Self {
        Self {
            source: None,
            fonts: &SYSTEM_DOC_FONTS,
        }
    }

    pub fn compile(&mut self, request: &CompileRequest) -> CompileOutput {
        let preamble = layout::preamble(&request.layout, &request.meta);
        self.compile_with(request, &preamble)
    }

    /// [`Self::compile`] with `preamble` instead of the one the layout
    /// generates (for tests).
    #[doc(hidden)]
    pub fn compile_with(&mut self, request: &CompileRequest, preamble: &Preamble) -> CompileOutput {
        let mut diagnostics = Vec::new();

        // Expert code with syntax errors is left out (so an unclosed bracket
        // can't swallow the document) and its errors reported.
        let custom_source = Source::new(*CUSTOM, request.custom.clone());
        let (errors, _) = custom_source.root().errors_and_warnings();
        let custom = if errors.is_empty() {
            request.custom.as_str()
        } else {
            for e in errors {
                let range = span_range(&custom_source, e.span).unwrap_or(0..0);
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    message: e.message.to_string(),
                    hints: e.hints.iter().map(|h| h.v.to_string()).collect(),
                    location: Location::Custom {
                        start: range.start,
                        end: range.end,
                    },
                });
            }
            ""
        };

        let bibliography = (!request.bibliography.trim().is_empty()
            && cites_sources(&request.content))
        .then(|| layout::bibliography(&request.layout));
        let plan = overrides::plan(
            &request.content,
            &request.overrides,
            request.layout.tables.style,
        );
        diagnostics.extend(plan.diagnostics);
        let (text, map) = compile_source(
            preamble,
            custom,
            &request.content,
            &plan.edits,
            bibliography.as_deref(),
        );

        let source = match &mut self.source {
            Some(source) => {
                source.replace(&text);
                source.clone()
            }
            None => {
                let source = Source::new(*MAIN, text);
                self.source = Some(source.clone());
                source
            }
        };
        let world = DocWorld::new(self.fonts, request, source);

        // Large stack: Typst recurses deeply on nested content; a stack
        // overflow would abort the process. Panics are caught by the join.
        let result = thread::scope(|scope| {
            thread::Builder::new()
                .name("typeset-document".into())
                .stack_size(TYPESET_STACK)
                .spawn_scoped(scope, || {
                    let warned = typst::compile::<PagedDocument>(&world);
                    let (document, errors) = match warned.output {
                        Ok(document) => (Some(document), Vec::new()),
                        Err(errors) => (None, errors.to_vec()),
                    };
                    (document, errors, warned.warnings.to_vec())
                })
                .ok()
                .map(|handle| handle.join())
        });
        let document = match result {
            Some(Ok((document, errors, warnings))) => {
                for d in errors.iter().chain(&warnings) {
                    diagnostics.push(convert(&world, &map, d));
                }
                document
            }
            Some(Err(_)) | None => {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    message: "Typst failed unexpectedly on this document.".into(),
                    hints: Vec::new(),
                    location: Location::Unknown,
                });
                None
            }
        };
        add_utf16(&request.content, &mut diagnostics);

        let page_sizes: Vec<[f64; 2]> = document.as_ref().map_or_else(Vec::new, |d| {
            d.pages()
                .iter()
                .map(|p| {
                    let size = p.frame.size();
                    [size.x.to_pt(), size.y.to_pt()]
                })
                .collect()
        });
        let document = document.map(|paged| CompiledDocument {
            paged,
            world,
            map,
            content: request.content.clone(),
            figures: plan.figures,
            image_quality: request.layout.export.image_quality,
            images: plan.images,
            tables: plan.tables,
        });
        let mut statuses = plan.statuses;
        if let Some(document) = &document {
            document.locate_overrides(&mut statuses);
        }
        CompileOutput {
            pages: page_sizes.len(),
            page_sizes,
            diagnostics,
            overrides: statuses,
            document,
        }
    }
}

fn convert(world: &DocWorld, map: &SourceMap, d: &SourceDiagnostic) -> Diagnostic {
    let location = match d.span.id() {
        Some(id) if id == world.main.id() => {
            span_range(&world.main, d.span).map_or(Location::Unknown, |range| map.locate(range))
        }
        Some(id) => Location::File {
            path: id.vpath().get_without_slash().to_owned(),
        },
        None => Location::Unknown,
    };
    Diagnostic {
        severity: match d.severity {
            typst::diag::Severity::Error => Severity::Error,
            typst::diag::Severity::Warning => Severity::Warning,
        },
        message: d.message.to_string(),
        hints: d.hints.iter().map(|h| h.v.to_string()).collect(),
        location,
    }
}

pub(crate) fn span_range(source: &Source, span: DiagSpan) -> Option<Range<usize>> {
    if span.id() != Some(source.id()) {
        return None;
    }
    match span.get() {
        DiagSpanKind::Detached => None,
        DiagSpanKind::Number { num, sub_range, .. } => source.range(num, sub_range),
        DiagSpanKind::Range { range, .. } => Some(range),
    }
}

/// Replaces byte offsets into `content` with UTF-16 offsets, in one pass.
fn to_utf16(content: &str, offsets: &mut [usize]) {
    let mut order: Vec<usize> = (0..offsets.len()).collect();
    order.sort_by_key(|&i| offsets[i]);
    let mut units = 0;
    let mut chars = content.char_indices().peekable();
    for i in order {
        while let Some(&(at, c)) = chars.peek() {
            if at >= offsets[i] {
                break;
            }
            units += c.len_utf16();
            chars.next();
        }
        offsets[i] = units;
    }
}

/// Fills in the UTF-16 offsets of content locations, in one pass.
fn add_utf16(content: &str, diagnostics: &mut [Diagnostic]) {
    let mut offsets: Vec<usize> = diagnostics
        .iter()
        .filter_map(|d| match d.location {
            Location::Content { start, end, .. } => Some([start, end]),
            _ => None,
        })
        .flatten()
        .collect();
    if offsets.is_empty() {
        return;
    }
    offsets.sort_unstable();
    offsets.dedup();
    let mut table = Vec::with_capacity(offsets.len());
    let mut units = 0;
    let mut chars = content.char_indices().peekable();
    for &target in &offsets {
        while let Some(&(i, c)) = chars.peek() {
            if i >= target {
                break;
            }
            units += c.len_utf16();
            chars.next();
        }
        table.push((target, units));
    }
    let lookup = |b: usize| table[table.partition_point(|(t, _)| *t < b)].1;
    for d in diagnostics {
        if let Location::Content {
            start,
            end,
            start_utf16,
            end_utf16,
        } = &mut d.location
        {
            *start_utf16 = lookup(*start);
            *end_utf16 = lookup(*end);
        }
    }
}

/// A compiled document, ready to render or export, with what it was
/// compiled from (for click-to-source).
pub struct CompiledDocument {
    paged: PagedDocument,
    world: DocWorld,
    map: SourceMap,
    content: String,
    /// The placed figures' calls in the content, by override id.
    figures: Vec<(String, usize)>,
    /// The document's export image quality.
    image_quality: ImageQuality,
    /// The `image` calls in the content with a quality of their own, by
    /// override id.
    images: Vec<(String, usize, ImageQuality)>,
    /// The styled tables' calls in the content, by override id.
    tables: Vec<(String, usize)>,
}

/// Where a click on a page leads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JumpResult {
    pub target: JumpTarget,
    /// The UTF-16 offset of the `figure` call the click is in, if any.
    pub figure_utf16: Option<usize>,
    /// The UTF-16 offset of the `image` call the click is in, or for a click
    /// in a figure's box the first one in the figure, if any.
    pub image_utf16: Option<usize>,
    /// The UTF-16 offset of the `table` call the click is in, else of the
    /// first one in the figure the click is in, if any.
    #[serde(default)]
    pub table_utf16: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "in", rename_all = "lowercase")]
pub enum JumpTarget {
    /// The clicked syntax element in the content, and the start of its
    /// paragraph or block (where a block override would anchor); UTF-16
    /// offsets.
    Content {
        pos_utf16: usize,
        start_utf16: usize,
        end_utf16: usize,
        block_start_utf16: usize,
    },
    /// The code injected for override `id`.
    Override { id: String },
    /// Not in the content (margins, page furniture, front matter, links).
    None,
}

impl CompiledDocument {
    pub fn pages(&self) -> usize {
        self.paged.pages().len()
    }

    /// The Typst document, for inspection.
    pub fn paged(&self) -> &PagedDocument {
        &self.paged
    }

    /// Fills in where the overrides of `statuses` (from this compilation)
    /// are on the pages (done by [`DocCompiler::compile`]).
    pub fn locate_overrides(&self, statuses: &mut [OverrideStatus]) {
        overrides::locate(
            statuses,
            &self.paged,
            &self.world.main,
            &self.map,
            &self.figures,
            &self.images,
            &self.tables,
        );
    }

    /// Where a click at (`x_pt`, `y_pt`) on page `index` (0-based) leads:
    /// the text there, else the figure whose box it is in, else the nearest
    /// line of text (see [`SNAP_LINES`]).
    pub fn jump(&self, index: usize, x_pt: f64, y_pt: f64) -> JumpResult {
        let none = JumpResult {
            target: JumpTarget::None,
            figure_utf16: None,
            image_utf16: None,
            table_utf16: None,
        };
        if index >= self.pages() || !x_pt.is_finite() || !y_pt.is_finite() {
            return none;
        }
        let point = Point::new(Abs::pt(x_pt), Abs::pt(y_pt));
        self.jump_at(index, point)
            .or_else(|| self.figure_at(index, point))
            .or_else(|| self.jump_at(index, self.nearest_line(index, point)?))
            .unwrap_or(none)
    }

    /// The content element at `point` on page `index`, if any.
    fn jump_at(&self, index: usize, point: Point) -> Option<JumpResult> {
        let position = PagedPosition {
            page: NonZeroUsize::new(index + 1)?,
            point,
        };
        let typst_ide::Jump::File(id, offset) =
            typst_ide::jump_from_click(&self.world, &self.paged, &position)?
        else {
            return None;
        };
        let content = &self.map.content;
        if id != self.world.main.id() || !(content.start..=content.end).contains(&offset) {
            return None;
        }
        let pos = match self.map.content_offset(offset) {
            Ok(pos) => pos,
            // Code injected for no override (the layout's): where it went.
            Err(e) if e.id.is_empty() => e.content.start,
            Err(e) => {
                return Some(JumpResult {
                    target: JumpTarget::Override { id: e.id.clone() },
                    figure_utf16: None,
                    image_utf16: None,
                    table_utf16: None,
                });
            }
        };
        let root = LinkedNode::new(self.world.main.root());
        let to_content = |p: usize| self.map.content_offset(p.clamp(content.start, content.end));
        let (start, end) = match root.leaf_at(offset, Side::After) {
            Some(leaf) => (
                to_content(leaf.offset()).unwrap_or(pos).min(pos),
                to_content(leaf.range().end).unwrap_or(pos).max(pos),
            ),
            None => (pos, pos),
        };
        let figure_call = overrides::enclosing_figure(&root, offset);
        let figure = figure_call
            .as_ref()
            .and_then(|f| to_content(f.offset()).ok());
        let image = overrides::enclosing_call(&root, offset, "image")
            .and_then(|i| to_content(i.offset()).ok());
        let table = overrides::enclosing_call(&root, offset, "table")
            .or_else(|| overrides::first_call_in(figure_call.as_ref()?, "table"))
            .and_then(|t| to_content(t.offset()).ok());
        Some(self.content_jump(pos, start, end, figure, image, table))
    }

    /// The innermost figure (called in the content) whose box on page
    /// `index` contains `point`, as a jump to its call.
    fn figure_at(&self, index: usize, point: Point) -> Option<JumpResult> {
        let introspector = self.paged.introspector();
        let elements = introspector.elements();
        let query = introspector.query(&Element::of::<FigureElem>().select());
        let mut found = None;
        for figure in query.iter() {
            let Some(loc) = figure.location() else {
                continue;
            };
            let Some(at) = elements.position(loc) else {
                continue;
            };
            if !overrides::float_pages(at.page).contains(&index) {
                continue;
            }
            let Some((_, b)) = overrides::figure_box(&self.paged, loc, index..index + 1) else {
                continue;
            };
            if (b.min.x..=b.max.x).contains(&point.x) && (b.min.y..=b.max.y).contains(&point.y) {
                found = self
                    .content_pos(figure.span())
                    .map(|c| (c, figure.span()))
                    .or(found);
            }
        }
        let (call, span) = found?;
        let node = self.world.main.find(span);
        let first = |name| {
            node.as_ref()
                .and_then(|f| overrides::first_call_in(f, name))
                .and_then(|i| self.map.content_offset(i.offset()).ok())
        };
        Some(self.content_jump(call, call, call, Some(call), first("image"), first("table")))
    }

    /// A point on the text run nearest to `point` on page `index`, within
    /// [`SNAP_LINES`] of the run's height above or below it: at `point`'s x
    /// clamped into the run. `None` if there is none, or if it is not
    /// content (running heads, page numbers, front matter).
    fn nearest_line(&self, index: usize, point: Point) -> Option<Point> {
        let mut runs = Vec::new();
        text_runs(
            &self.paged.pages()[index].frame,
            Transform::identity(),
            &mut runs,
        );
        let dist = |lo: Abs, hi: Abs, v: Abs| (lo - v).max(v - hi).max(Abs::zero());
        let mut near: Vec<_> = runs
            .into_iter()
            .filter_map(|(ts, text)| {
                let b = overrides::transformed(overrides::text_box(text), ts);
                let dy = dist(b.min.y, b.max.y, point.y);
                let dx = dist(b.min.x, b.max.x, point.x);
                (dy <= (b.max.y - b.min.y) * SNAP_LINES).then_some((dy, dx, ts, text))
            })
            .collect();
        near.sort_by_key(|&(dy, dx, ..)| (dy, dx));
        // The nearest run of content on the nearest line.
        let line = near.first()?.0;
        let (_, _, ts, text) = near.into_iter().take_while(|r| r.0 == line).find(|r| {
            r.3.glyphs
                .iter()
                .any(|g| self.content_pos(g.span.0).is_some())
        })?;
        // Inside the run: slightly in from its ends, halfway up the glyphs.
        let local = point.transform(ts.invert()?);
        let width = text.width();
        let inset = width.min(Abs::pt(1.0)) / 2.0;
        let x = local.x.clamp(inset, width - inset);
        Some(Point::new(x, -text.size / 2.0).transform(ts))
    }

    /// The compiled-source offset of the node `span` points to, if it is in
    /// the content (not injected).
    fn content_pos(&self, span: Span) -> Option<usize> {
        let main = &self.world.main;
        if span.id() != Some(main.id()) {
            return None;
        }
        let offset = main.find(span)?.offset();
        let content = &self.map.content;
        if !(content.start..content.end).contains(&offset) {
            return None;
        }
        self.map.content_offset(offset).ok()
    }

    /// A jump to content bytes `pos` (`start..end`: the clicked element),
    /// in figure call `figure`, image call `image` and table call `table`.
    fn content_jump(
        &self,
        pos: usize,
        start: usize,
        end: usize,
        figure: Option<usize>,
        image: Option<usize>,
        table: Option<usize>,
    ) -> JumpResult {
        // In the compiled source (parsed already); injected block code
        // stands on lines of its own before a block, so the start is the
        // same.
        let main = &self.world.main;
        let block = overrides::block_start_in(
            &LinkedNode::new(main.root()),
            main.text(),
            self.map.source_offset(pos),
        );
        let block = self
            .map
            .content_offset(block.max(self.map.content.start))
            .unwrap_or_else(|e| e.content.end)
            .min(pos);
        let mut offsets = [
            pos,
            start,
            end,
            block,
            figure.unwrap_or(0),
            image.unwrap_or(0),
            table.unwrap_or(0),
        ];
        to_utf16(&self.content, &mut offsets);
        let [
            pos_utf16,
            start_utf16,
            end_utf16,
            block_start_utf16,
            figure_utf16,
            image_utf16,
            table_utf16,
        ] = offsets;
        JumpResult {
            target: JumpTarget::Content {
                pos_utf16,
                start_utf16,
                end_utf16,
                block_start_utf16,
            },
            figure_utf16: figure.map(|_| figure_utf16),
            image_utf16: image.map(|_| image_utf16),
            table_utf16: table.map(|_| table_utf16),
        }
    }

    /// Page `index` (0-based) as SVG.
    pub fn render_page_svg(&self, index: usize) -> Option<String> {
        let page = self.paged.pages().get(index)?;
        Some(typst_svg::svg(page, &typst_svg::SvgOptions::default()))
    }

    /// The pages as exported: raster images at the export image quality
    /// (the document's, or an image's own override); `None` if no image
    /// changes. See [`crate::images`].
    pub fn export_document(&self) -> Option<PagedDocument> {
        let main = &self.world.main;
        let own: HashMap<usize, ImageQuality> = self
            .images
            .iter()
            .map(|(_, call, quality)| (self.map.source_offset(*call), *quality))
            .collect();
        let quality_of = |span| {
            if own.is_empty() {
                return self.image_quality;
            }
            overrides::image_call(main, span)
                .and_then(|call| own.get(&call).copied())
                .unwrap_or(self.image_quality)
        };
        if own.is_empty() && self.image_quality == ImageQuality::Original {
            return None;
        }
        crate::images::downscale(&self.paged, quality_of)
    }

    /// The whole document as PDF, with the images at the export image
    /// quality.
    pub fn export_pdf(&self) -> Result<Vec<u8>, String> {
        let exported = self.export_document();
        let paged = exported.as_ref().unwrap_or(&self.paged);
        typst_pdf::pdf(paged, &typst_pdf::PdfOptions::default()).map_err(|errors| {
            errors
                .first()
                .map_or_else(|| "PDF export failed.".into(), |e| e.message.to_string())
        })
    }

    /// The text on page `index`, in layout order (for tests and search).
    pub fn page_text(&self, index: usize) -> Option<String> {
        let page = self.paged.pages().get(index)?;
        let mut out = String::new();
        collect_text(&page.frame, &mut out);
        Some(out)
    }
}

/// The text runs of `frame` (placed by `ts`), each with its placement.
fn text_runs<'a>(frame: &'a Frame, ts: Transform, out: &mut Vec<(Transform, &'a TextItem)>) {
    for (pos, item) in frame.items() {
        let ts = ts.pre_concat(Transform::translate(pos.x, pos.y));
        match item {
            FrameItem::Group(group) => text_runs(&group.frame, ts.pre_concat(group.transform), out),
            FrameItem::Text(text) => out.push((ts, text)),
            _ => {}
        }
    }
}

fn collect_text(frame: &Frame, out: &mut String) {
    for (_, item) in frame.items() {
        match item {
            FrameItem::Group(group) => collect_text(&group.frame, out),
            FrameItem::Text(text) => {
                out.push_str(&text.text);
                out.push(' ');
            }
            _ => {}
        }
    }
}

/// Page `index` of `document` as SVG (see [`CompiledDocument::render_page_svg`]).
pub fn render_page_svg(document: &CompiledDocument, index: usize) -> Option<String> {
    document.render_page_svg(index)
}

/// `document` as PDF (see [`CompiledDocument::export_pdf`]).
pub fn export_pdf(document: &CompiledDocument) -> Result<Vec<u8>, String> {
    document.export_pdf()
}

// ---- The Typst world ----

pub(crate) static MAIN: std::sync::LazyLock<FileId> =
    std::sync::LazyLock::new(|| file_id("/main.typ"));
pub(crate) static CUSTOM: std::sync::LazyLock<FileId> =
    std::sync::LazyLock::new(|| file_id("/lethora-custom.typ"));

fn file_id(path: &str) -> FileId {
    RootedPath::new(
        VirtualRoot::Project,
        VirtualPath::new(path).expect("valid path"),
    )
    .intern()
}

pub(crate) struct DocWorld {
    fonts: &'static FontStore,
    root: PathBuf,
    /// Served as [`REFERENCES`].
    bibliography: Bytes,
    /// Path → blob name; images are served from their blobs.
    assets: BTreeMap<String, String>,
    pub(crate) main: Source,
    today: Option<Date>,
}

impl DocWorld {
    /// The world compiling `main` with `request`'s files and date.
    pub(crate) fn new(fonts: &'static DocFonts, request: &CompileRequest, main: Source) -> Self {
        Self {
            fonts: &fonts.store,
            root: request.project_dir.clone(),
            bibliography: Bytes::new(request.bibliography.clone().into_bytes()),
            assets: request.assets.clone(),
            main,
            today: request.today,
        }
    }
}

impl typst_ide::IdeWorld for DocWorld {
    fn upcast(&self) -> &dyn World {
        self
    }
}

impl World for DocWorld {
    fn library(&self) -> &LazyHash<Library> {
        &LIBRARY
    }

    fn book(&self) -> &LazyHash<FontBook> {
        self.fonts.book()
    }

    fn main(&self) -> FileId {
        self.main.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() {
            Ok(self.main.clone())
        } else {
            Err(denied(id))
        }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        if let VirtualRoot::Package(_) = id.root() {
            return Err(denied(id));
        }
        let rel = id.vpath().get_without_slash();
        let bytes = if rel == REFERENCES {
            if self.bibliography.len() as u64 > MAX_REFERENCES_BYTES {
                Err(format!(
                    "{REFERENCES} is larger than {} MB.",
                    MAX_REFERENCES_BYTES / (1024 * 1024)
                ))
            } else {
                return Ok(self.bibliography.clone());
            }
        } else if rel.starts_with(&format!("{}/", assets::DIR)) {
            assets::validate_relative(rel).and_then(|()| match self.assets.get(rel) {
                Some(blob) => assets::read_blob(&self.root, blob, rel),
                None => Err(format!("Image not found: {rel}")),
            })
        } else {
            return Err(denied(id));
        };
        bytes
            .map(Bytes::new)
            .map_err(|e| FileError::Other(Some(e.into())))
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.font(index)
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        let d = self.today?;
        Datetime::from_ymd(d.year, d.month, d.day)
    }
}

fn denied(id: FileId) -> FileError {
    FileError::Other(Some(match id.root() {
        VirtualRoot::Package(_) => "packages are not available".into(),
        VirtualRoot::Project => format!(
            "only {REFERENCES} and images in {}/ can be read",
            assets::DIR
        )
        .into(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn citations_are_detected() {
        assert!(!cites_sources("Plain text, mail me at x"));
        assert!(cites_sources("As @knuth84 shows."));
        assert!(cites_sources("#cite(<knuth84>)"));
        assert!(!cites_sources("= Intro <intro>\nSee @intro."));
        assert!(cites_sources("= Intro <intro>\nSee @intro and @doe."));
        assert!(!cites_sources("`@notacite` and mail\\@x.org"));
    }

    #[test]
    fn utf16_offsets() {
        let content = "a🌍b中c";
        let mut d = vec![Diagnostic {
            severity: Severity::Error,
            message: String::new(),
            hints: Vec::new(),
            location: Location::Content {
                start: 5,
                end: 9,
                start_utf16: 0,
                end_utf16: 0,
            },
        }];
        add_utf16(content, &mut d);
        assert_eq!(
            d[0].location,
            Location::Content {
                start: 5,
                end: 9,
                start_utf16: 3,
                end_utf16: 5
            }
        );
    }
}
