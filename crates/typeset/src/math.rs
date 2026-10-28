//! Rendering of single Typst math formulas to SVG.

use std::collections::{HashMap, VecDeque};
use std::sync::LazyLock;
use std::thread;

use typst::diag::{FileError, FileResult, SourceDiagnostic};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::layout::{Abs, Frame, FrameItem, FrameKind, Point, Transform};
use typst::syntax::{FileId, Source, SyntaxKind, SyntaxNode, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, World};
use typst_layout::PagedDocument;

use crate::fonts::{FONTS, LIBRARY};

/// Longest accepted formula source, in bytes.
pub const MAX_SOURCE_LEN: usize = 8 * 1024;
/// Accepted text sizes, in points.
pub const SIZE_RANGE: std::ops::RangeInclusive<f64> = 1.0..=400.0;
/// Cached results beyond this count evict the oldest ones.
const CACHE_CAPACITY: usize = 2000;
/// Every this many compilations, Typst's global memoization cache is trimmed.
const EVICT_EVERY: u64 = 64;
/// Memoized results unused for this many evictions are dropped.
const EVICT_MAX_AGE: usize = 4;
/// Stack size of the typesetting thread (reserved, committed as used).
pub(crate) const TYPESET_STACK: usize = 64 * 1024 * 1024;

/// Identifiers that can produce unbounded work or output from a short
/// formula, reach outside it, or evaluate strings as code. Checked against
/// every identifier and field name in the formula.
const DENIED_IDENTS: &[&str] = &[
    "eval",
    "dictionary",
    "range",
    "repeat",
    "lorem",
    "read",
    "plugin",
    "image",
    "json",
    "yaml",
    "toml",
    "csv",
    "xml",
    "cbor",
    "bibliography",
    "cite",
    "grid",
    "table",
    "query",
    "counter",
    "state",
    "here",
    "locate",
    "layout",
    "measure",
    "to-bytes",
    "tiling",
    "pattern",
];

/// Code constructs a formula may not contain: loops, functions, bindings,
/// modules, show rules and context (unbounded or exponential work).
const DENIED_KINDS: &[SyntaxKind] = &[
    SyntaxKind::ForLoop,
    SyntaxKind::WhileLoop,
    SyntaxKind::Closure,
    SyntaxKind::LetBinding,
    SyntaxKind::DestructAssignment,
    SyntaxKind::ModuleImport,
    SyntaxKind::ModuleInclude,
    SyntaxKind::ShowRule,
    SyntaxKind::Contextual,
];

/// How a formula is typeset.
#[derive(Debug, Clone, PartialEq)]
pub struct MathStyle {
    /// Text size in points, within [`SIZE_RANGE`].
    pub size_pt: f64,
    /// Text colour as `#rrggbb`.
    pub color: String,
}

/// A rendered formula. Lengths are in points; the SVG has the same size.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    pub svg: String,
    pub width_pt: f64,
    pub height_pt: f64,
    /// Distance of the formula's baseline from the top. Inline formulas align
    /// with surrounding text via `vertical-align: -(height - baseline)`.
    pub baseline_pt: f64,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    src: String,
    display: bool,
    size_bits: u64,
    color: String,
}

/// Renders formulas with embedded fonts and no file, package or network
/// access, caching results (successes and errors).
#[derive(Default)]
pub struct MathRenderer {
    cache: HashMap<Key, Result<Rendered, String>>,
    order: VecDeque<Key>,
    compiled: u64,
}

impl MathRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Renders `src` (the text between the `$` delimiters) as inline or
    /// display math. Errors are readable messages (first error plus hints).
    pub fn render_math(
        &mut self,
        src: &str,
        display: bool,
        style: &MathStyle,
    ) -> Result<Rendered, String> {
        let key = Key {
            src: src.to_owned(),
            display,
            size_bits: style.size_pt.to_bits(),
            color: style.color.clone(),
        };
        if let Some(hit) = self.cache.get(&key) {
            return hit.clone();
        }
        let result = self.compile(src, display, style);
        if self.order.len() >= CACHE_CAPACITY
            && let Some(oldest) = self.order.pop_front()
        {
            self.cache.remove(&oldest);
        }
        self.order.push_back(key.clone());
        self.cache.insert(key, result.clone());
        result
    }

    /// Number of cached results.
    pub fn cached(&self) -> usize {
        self.cache.len()
    }

    fn compile(&mut self, src: &str, display: bool, style: &MathStyle) -> Result<Rendered, String> {
        if !SIZE_RANGE.contains(&style.size_pt) {
            return Err(format!(
                "Text size must be between {} and {} pt.",
                SIZE_RANGE.start(),
                SIZE_RANGE.end()
            ));
        }
        if !is_hex_color(&style.color) {
            return Err("Colour must have the form #rrggbb.".into());
        }
        if src.len() > MAX_SOURCE_LEN {
            return Err(format!(
                "Formula is too long (over {MAX_SOURCE_LEN} bytes)."
            ));
        }

        let equation = if display {
            format!("$ {src} $")
        } else {
            format!("${src}$")
        };
        // Validated above: the size is a plain finite number, the colour hex.
        // Glyph-bounds edges make the box (and page) fit the ink tightly; the
        // box keeps the baseline findable (see `find_baseline`).
        let text = format!(
            "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n\
             #set text(size: {}pt, fill: rgb(\"{}\"), top-edge: \"bounds\", bottom-edge: \"bounds\")\n\
             #box[{equation}]\n",
            style.size_pt, style.color
        );
        let source = Source::new(*MAIN, text);
        check_syntax(source.root(), &equation)?;

        self.compiled += 1;
        if self.compiled.is_multiple_of(EVICT_EVERY) {
            typst::comemo::evict(EVICT_MAX_AGE);
        }

        // Typst recurses deeply on nested formulas (up to its parser depth
        // limit); a stack overflow would abort the process, so typeset on a
        // thread with a large stack. Panics are caught by the join.
        thread::scope(|scope| {
            thread::Builder::new()
                .name("typeset-math".into())
                .stack_size(TYPESET_STACK)
                .spawn_scoped(scope, || typeset(source))
                .map_err(|e| format!("Could not start typesetting: {e}"))?
                .join()
                .unwrap_or_else(|_| Err("Typst failed unexpectedly on this formula.".into()))
        })
    }
}

fn typeset(source: Source) -> Result<Rendered, String> {
    let world = MathWorld { source };
    let document = typst::compile::<PagedDocument>(&world)
        .output
        .map_err(|errors| describe(&errors))?;
    let page = document
        .pages()
        .first()
        .ok_or("The formula produced no output.")?;
    let svg = typst_svg::svg(page, &typst_svg::SvgOptions::default());
    let size = page.frame.size();
    let baseline = find_baseline(&page.frame, Point::zero()).unwrap_or(size.y);
    Ok(Rendered {
        svg,
        width_pt: size.x.to_pt(),
        height_pt: size.y.to_pt(),
        baseline_pt: baseline.to_pt(),
    })
}

fn is_hex_color(color: &str) -> bool {
    color.len() == 7 && color.starts_with('#') && color[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// Checks that the formula stayed inside its delimiters and contains no
/// denied constructs (see [`DENIED_KINDS`] and [`DENIED_IDENTS`]).
fn check_syntax(root: &SyntaxNode, equation: &str) -> Result<(), String> {
    // The wrapper contains no equation, so the first one is the formula. If the
    // formula closed it early (or escaped the closing `$`), the text differs.
    let Some(ours) =
        first_of_kind(root, SyntaxKind::Equation).filter(|node| node.full_text() == equation)
    else {
        // Report a syntax error (e.g. too deep nesting) in preference.
        let (errors, _) = root.errors_and_warnings();
        return Err(errors.first().map_or_else(
            || "The formula must not contain an unescaped `$`.".into(),
            |e| capitalize(&e.message),
        ));
    };
    check_node(ours)
}

fn first_of_kind(node: &SyntaxNode, kind: SyntaxKind) -> Option<&SyntaxNode> {
    if node.kind() == kind {
        return Some(node);
    }
    node.children().find_map(|child| first_of_kind(child, kind))
}

fn check_node(node: &SyntaxNode) -> Result<(), String> {
    let kind = node.kind();
    if DENIED_KINDS.contains(&kind) {
        return Err(format!(
            "{} is not allowed in a formula.",
            capitalize(kind.name())
        ));
    }
    if matches!(kind, SyntaxKind::Ident | SyntaxKind::MathIdent)
        && DENIED_IDENTS.contains(&node.leaf_text().as_str())
    {
        return Err(format!(
            "`{}` is not allowed in a formula.",
            node.leaf_text()
        ));
    }
    if kind == SyntaxKind::Binary && node.children().any(|c| c.kind() == SyntaxKind::Star) {
        return Err("Multiplication in code is not allowed in a formula.".into());
    }
    node.children().try_for_each(check_node)
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The first error's message plus its hints.
fn describe(errors: &[SourceDiagnostic]) -> String {
    let Some(first) = errors.first() else {
        return "The formula could not be rendered.".into();
    };
    let mut message = capitalize(&first.message);
    for hint in &first.hints {
        message.push_str("\nHint: ");
        message.push_str(&hint.v);
    }
    message
}

/// Finds the formula box (the first hard frame with a baseline, i.e. the
/// `#box` around the equation) and returns its baseline in page coordinates.
fn find_baseline(frame: &Frame, offset: Point) -> Option<Abs> {
    for (pos, item) in frame.items() {
        let FrameItem::Group(group) = item else {
            continue;
        };
        if group.transform != Transform::identity() {
            continue;
        }
        let at = offset + *pos;
        if group.frame.kind() == FrameKind::Hard && group.frame.has_baseline() {
            return Some(at.y + group.frame.baseline());
        }
        if let Some(found) = find_baseline(&group.frame, at) {
            return Some(found);
        }
    }
    None
}

// ---- The Typst world ----

static MAIN: LazyLock<FileId> = LazyLock::new(|| Source::detached("").id());

/// A world with the standard library, embedded fonts and one source file.
struct MathWorld {
    source: Source,
}

impl World for MathWorld {
    fn library(&self) -> &LazyHash<Library> {
        &LIBRARY
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &FONTS.book
    }

    fn main(&self) -> FileId {
        *MAIN
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == *MAIN {
            Ok(self.source.clone())
        } else {
            Err(no_access(id))
        }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        Err(no_access(id))
    }

    fn font(&self, index: usize) -> Option<Font> {
        FONTS.fonts.get(index).cloned()
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        None
    }
}

fn no_access(id: FileId) -> FileError {
    match id.get().root() {
        VirtualRoot::Package(_) => {
            FileError::Other(Some("packages are not available in formulas".into()))
        }
        VirtualRoot::Project => {
            FileError::Other(Some("files are not available in formulas".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    /// Compiles raw Typst without the syntax guard, to check the world and
    /// Typst's own limits behind it.
    fn compile_unguarded(text: &str) -> Result<(), String> {
        let source = Source::new(*MAIN, text.to_owned());
        let start = Instant::now();
        let result = typeset(source).map(|_| ());
        assert!(start.elapsed() < Duration::from_secs(5), "{text}: slow");
        result
    }

    #[test]
    fn world_has_no_files_or_packages() {
        let e = compile_unguarded("#read(\"/etc/hosts\")").unwrap_err();
        assert!(e.contains("files are not available"), "{e}");
        let e = compile_unguarded("#import \"@preview/cetz:0.3.0\": *").unwrap_err();
        assert!(e.contains("packages are not available"), "{e}");
    }

    #[test]
    fn typst_bounds_loops_and_recursion() {
        let e = compile_unguarded("#while true {}").unwrap_err();
        assert!(e.contains("always true"), "{e}");
        let e = compile_unguarded("#let i = 0\n#while i >= 0 { i += 1 }").unwrap_err();
        assert!(e.contains("infinite"), "{e}");
        let e = compile_unguarded("#let f(n) = f(n + 1)\n#f(0)").unwrap_err();
        assert!(e.contains("depth"), "{e}");
    }
}
