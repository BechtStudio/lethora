//! Model → Lethora markup: the Typst subset the app itself writes, in the
//! same forms (`app/src/format.ts`, `breaks.ts`, `figures.ts`, `tables.ts`,
//! `palette.ts`). Every import ends here.
//!
//! - Headings `= …`, paragraphs separated by blank lines, lists `- ` and
//!   `+ ` (a numbered list starting elsewhere than 1 says `N. ` first),
//!   nested items indented under their text.
//! - Bold `*…*` and italic `_…_`, or `#strong[…]`/`#emph[…]` where a
//!   letter or digit touches them (Typst reads the delimiters inside a word
//!   as text); `#underline[…]`, `#strike[…]`, `#super[…]`, `#sub[…]`;
//!   forced line breaks ` \` at the line end.
//! - Figures `#figure(image("assets/…", width: 80%), caption: […]) <label>`
//!   and tables `table(columns: N, table.header(…), …)` one row per line,
//!   bare (`#table(`) without a caption; math `$x$` and `$ x $`.
//! - Citations `@key` only for keys of the project's sources (or labels of
//!   the document): an unknown key would break the compile, so it stays
//!   text.
//!
//! Text is escaped wherever Typst would read it as markup, so it comes out
//! as written.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use crate::model::{Block, Cell, Cite, Doc, Figure, FigureBody, Image, Inline, List, Table};
use crate::{Error, MAX_DEPTH, Result, Warning, WarningKind, Warnings};

/// What the markup may refer to in the project.
pub struct MarkupOptions<'a> {
    /// The project path (`assets/…`) of each resource the markup shows, by
    /// [`crate::model::Resource::name`]; images without one are left out.
    pub assets: &'a BTreeMap<String, String>,
    /// The keys of the project's sources.
    pub keys: &'a BTreeSet<String>,
}

/// Markup and what didn't make it into it.
#[derive(Debug, Clone, PartialEq)]
pub struct Markup {
    /// Blocks separated by blank lines, ending in a line break (empty if
    /// there is nothing).
    pub text: String,
    pub warnings: Vec<Warning>,
}

/// `doc` as Lethora markup.
pub fn to_markup(doc: &Doc, options: &MarkupOptions) -> Result<Markup> {
    let mut labels = BTreeSet::new();
    collect_labels(&doc.blocks, &mut labels, 0)?;
    let mut w = Markuper {
        options,
        labels,
        warnings: Warnings::default(),
        depth: 0,
    };
    let mut text = w.blocks(&doc.blocks)?;
    if !text.is_empty() {
        text.push('\n');
    }
    Ok(Markup {
        text,
        warnings: w.warnings.into_vec(),
    })
}

/// The labels of headings, figures and formulas, as written.
fn collect_labels(blocks: &[Block], labels: &mut BTreeSet<String>, depth: usize) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(Error::TooDeep);
    }
    for b in blocks {
        let label = match b {
            Block::Heading { label, .. } | Block::Math { label, .. } => label,
            Block::Figure(f) => &f.label,
            Block::Quote(inner) => {
                collect_labels(inner, labels, depth + 1)?;
                continue;
            }
            Block::List(l) => {
                for item in &l.items {
                    collect_labels(item, labels, depth + 1)?;
                }
                continue;
            }
            _ => continue,
        };
        if let Some(l) = label.as_deref().and_then(clean_label) {
            labels.insert(l);
        }
    }
    Ok(())
}

/// What the previous output was, for the next character.
#[derive(Clone, Copy, PartialEq)]
enum After {
    Nothing,
    /// A `#call(…)` or `#f[…]`: a `(`, `[`, `;` or `.name` right after it
    /// would continue it.
    Code,
    /// `@key`: a letter, digit, `_` or `-` right after it would join the key.
    Ref,
}

/// A line being written.
struct Out {
    s: String,
    /// At the start of a line or content block: list and heading markers
    /// would apply.
    start: bool,
    after: After,
    /// A heading: line breaks become spaces.
    single_line: bool,
}

impl Out {
    fn new(single_line: bool) -> Self {
        Out {
            s: String::new(),
            start: true,
            after: After::Nothing,
            single_line,
        }
    }

    fn push(&mut self, s: &str, after: After) {
        self.s.push_str(s);
        self.start = false;
        self.after = after;
    }

    fn space(&mut self) {
        if !self.start && !self.s.ends_with(' ') && !self.s.is_empty() {
            self.s.push(' ');
        }
        self.after = After::Nothing;
    }
}

struct Markuper<'a> {
    options: &'a MarkupOptions<'a>,
    labels: BTreeSet<String>,
    warnings: Warnings,
    depth: usize,
}

impl Markuper<'_> {
    fn enter(&mut self) -> Result<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Error::TooDeep);
        }
        Ok(())
    }

    /// Blocks separated by blank lines.
    fn blocks(&mut self, blocks: &[Block]) -> Result<String> {
        self.enter()?;
        let mut parts = Vec::new();
        for b in blocks {
            let s = self.block(b)?;
            if !s.is_empty() {
                parts.push(s);
            }
        }
        self.depth -= 1;
        Ok(parts.join("\n\n"))
    }

    fn block(&mut self, b: &Block) -> Result<String> {
        Ok(match b {
            Block::Para(content) => self.line(content, false)?,
            Block::Heading {
                level,
                content,
                label,
            } => {
                let text = self.line(content, true)?;
                if text.is_empty() {
                    return Ok(text);
                }
                let marker = "=".repeat(usize::from((*level).clamp(1, 6)));
                format!("{marker} {text}{}", label_suffix(label))
            }
            Block::List(l) => self.list(l)?,
            Block::Quote(inner) => {
                let inner = self.blocks(inner)?;
                if inner.is_empty() {
                    return Ok(inner);
                }
                format!("#quote(block: true)[\n  {}\n]", indent(&inner, "  "))
            }
            Block::Code { lang, text } => code_block(lang.as_deref(), text),
            Block::Math { math, label, .. } => {
                let source = math_source(&math.typst);
                if source.is_empty() {
                    return Ok(source);
                }
                format!("$ {source} ${}", label_suffix(label))
            }
            Block::Figure(f) => self.figure(f)?,
            Block::Table(t) => format!("#{}", self.table(t, "")?),
            Block::Rule => "#line(length: 100%)".to_owned(),
        })
    }

    /// One paragraph, heading text, caption or cell: inline content on a
    /// line of its own (several with forced breaks), trimmed.
    fn line(&mut self, content: &[Inline], single_line: bool) -> Result<String> {
        let start = content
            .iter()
            .position(|i| !matches!(i, Inline::LineBreak))
            .unwrap_or(content.len());
        let end = content
            .iter()
            .rposition(|i| !matches!(i, Inline::LineBreak))
            .map_or(start, |i| i + 1);
        let mut out = Out::new(single_line);
        self.inlines(&content[start..end], &mut out, None)?;
        Ok(out.s.trim_end().to_owned())
    }

    /// `items` into `out`; `follow` is the character after them, if known.
    fn inlines(&mut self, items: &[Inline], out: &mut Out, follow: Option<char>) -> Result<()> {
        self.enter()?;
        let items = merge_texts(items);
        for (i, item) in items.iter().enumerate() {
            let next = items.get(i + 1).map(first_char).or(follow);
            self.inline(item, out, next)?;
        }
        self.depth -= 1;
        Ok(())
    }

    fn inline(&mut self, item: &Inline, out: &mut Out, next: Option<char>) -> Result<()> {
        match item {
            Inline::Text(t) => text(t, out, next),
            Inline::Emph(c) => self.span(c, out, next, "emph", Some('_'))?,
            Inline::Strong(c) => self.span(c, out, next, "strong", Some('*'))?,
            Inline::Underline(c) => self.span(c, out, next, "underline", None)?,
            Inline::Strike(c) => self.span(c, out, next, "strike", None)?,
            Inline::Super(c) => self.span(c, out, next, "super", None)?,
            Inline::Sub(c) => self.span(c, out, next, "sub", None)?,
            Inline::Code(code) => {
                let code = code.replace(['\n', '\r'], " ");
                if code.is_empty() {
                } else if code.contains('`') {
                    out.push(&format!("#raw({})", string(&code)), After::Code);
                } else {
                    out.push(&format!("`{code}`"), After::Nothing);
                }
            }
            Inline::LineBreak => {
                if out.single_line {
                    out.space();
                } else if !out.start {
                    if !out.s.ends_with(' ') {
                        out.s.push(' ');
                    }
                    out.s.push_str("\\\n");
                    out.start = true;
                    out.after = After::Nothing;
                }
            }
            Inline::Link { url, content } => {
                let mut s = format!("#link({})", string(url));
                let plain = matches!(&content[..], [Inline::Text(t)] if t == url);
                if !content.is_empty() && !plain {
                    s.push('[');
                    s.push_str(&self.block_content(content, out.single_line)?);
                    s.push(']');
                }
                out.push(&s, After::Code);
            }
            Inline::Math(m) => {
                let source = math_source(&m.typst);
                if !source.is_empty() {
                    out.push(&format!("${source}$"), After::Nothing);
                }
            }
            Inline::Image(img) => {
                if let Some(path) = self.image_path(img) {
                    out.push(
                        &format!("#box(image({}, height: 1em))", string(&path)),
                        After::Code,
                    );
                }
            }
            Inline::Footnote(blocks) => {
                let note = match &blocks[..] {
                    [Block::Para(content)] => self.line(content, false)?,
                    _ => self.blocks(blocks)?,
                };
                if !note.is_empty() {
                    out.push(&format!("#footnote[{note}]"), After::Code);
                }
            }
            Inline::Cite(c) => self.cite(c, out, next)?,
            Inline::Ref { label, text: shown } => {
                if self.known(label) {
                    reference(label, None, out);
                } else {
                    self.warnings
                        .add_example(WarningKind::UnknownCitation, label.clone());
                    match shown {
                        Some(shown) => self.inlines(shown, out, next)?,
                        None => text(&format!("@{label}"), out, next),
                    }
                }
            }
        }
        Ok(())
    }

    /// Content in brackets (`[…]`): a fresh start, as at a line start.
    fn block_content(&mut self, content: &[Inline], single_line: bool) -> Result<String> {
        let mut inner = Out::new(single_line);
        self.inlines(content, &mut inner, Some(']'))?;
        Ok(inner.s.trim_end().to_owned())
    }

    /// A formatted span: with its delimiter where Typst reads it so, else
    /// as a call. Empty spans go.
    fn span(
        &mut self,
        content: &[Inline],
        out: &mut Out,
        next: Option<char>,
        call: &str,
        delim: Option<char>,
    ) -> Result<()> {
        if content
            .iter()
            .all(|i| matches!(i, Inline::Text(t) if t.trim().is_empty()))
        {
            if !content.is_empty() {
                out.space();
            }
            return Ok(());
        }
        let prev = out.s.chars().last();
        let delim = delim.filter(|&d| {
            out.after != After::Ref
                && !prev.is_some_and(char::is_alphanumeric)
                && !next.is_some_and(char::is_alphanumeric)
                && !contains_span(content, d, 0)
        });
        match delim {
            Some(d) => {
                out.push(&d.to_string(), After::Nothing);
                self.inlines(content, out, Some(d))?;
                out.push(&d.to_string(), After::Nothing);
            }
            None => {
                let inner = self.block_content(content, out.single_line)?;
                out.push(&format!("#{call}[{inner}]"), After::Code);
            }
        }
        Ok(())
    }

    fn cite(&mut self, c: &Cite, out: &mut Out, next: Option<char>) -> Result<()> {
        if let Some(unknown) = c.items.iter().find(|i| !self.known(&i.key)) {
            self.warnings
                .add_example(WarningKind::UnknownCitation, unknown.key.clone());
        } else if !c.items.is_empty() {
            for (n, item) in c.items.iter().enumerate() {
                if n > 0 {
                    out.space();
                }
                let supplement = match &item.locator {
                    Some(l) => Some(self.block_content(&[Inline::Text(l.clone())], false)?),
                    None => None,
                };
                reference(&item.key, supplement.as_deref(), out);
            }
            return Ok(());
        }
        match &c.text {
            Some(shown) => self.inlines(shown, out, next),
            None => {
                let keys: Vec<String> = c.items.iter().map(|i| format!("@{}", i.key)).collect();
                text(&keys.join("; "), out, next);
                Ok(())
            }
        }
    }

    /// Whether `key` is a source of the project or a label of the document.
    fn known(&self, key: &str) -> bool {
        self.options.keys.contains(key) || self.labels.contains(key)
    }

    fn image_path(&mut self, img: &Image) -> Option<String> {
        let path = self.options.assets.get(&img.resource).cloned();
        if path.is_none() {
            self.warnings
                .add_example(WarningKind::ImageLeftOut, img.resource.clone());
        }
        path
    }

    fn list(&mut self, l: &List) -> Result<String> {
        self.enter()?;
        let mut items = Vec::new();
        for (n, item) in l.items.iter().enumerate() {
            let marker = match (l.ordered, n) {
                (false, _) => "-".to_owned(),
                (true, 0) if l.start != 1 => format!("{}.", l.start),
                (true, _) => "+".to_owned(),
            };
            let mut body = String::new();
            for (k, b) in item.iter().enumerate() {
                let s = self.block(b)?;
                if s.is_empty() {
                    continue;
                }
                if !body.is_empty() {
                    body.push_str(if k > 0 && matches!(b, Block::List(_)) {
                        "\n"
                    } else {
                        "\n\n"
                    });
                }
                body.push_str(&s);
            }
            let pad = " ".repeat(marker.len() + 1);
            items.push(match body.is_empty() {
                true => marker,
                false => format!("{marker} {}", indent(&body, &pad)),
            });
        }
        self.depth -= 1;
        Ok(items.join("\n"))
    }

    fn figure(&mut self, f: &Figure) -> Result<String> {
        let body = match &f.body {
            FigureBody::Image(img) => match self.image_path(img) {
                Some(path) => format!("image({}, width: {}%)", string(&path), percent(img.width)),
                None => return Ok(String::new()),
            },
            FigureBody::Table(t) => self.table(t, "  ")?,
        };
        let mut s = format!("#figure(\n  {body},\n");
        let caption = self.line(&f.caption, false)?;
        if !caption.is_empty() {
            s.push_str(&format!("  caption: [{}],\n", indent(&caption, "  ")));
        }
        s.push(')');
        s.push_str(&label_suffix(&f.label));
        Ok(s)
    }

    /// `table(…)` with its lines indented by `indent` (the call's own
    /// line is the caller's).
    fn table(&mut self, t: &Table, indent: &str) -> Result<String> {
        self.enter()?;
        let inner = format!("{indent}  ");
        let widest = t
            .rows
            .iter()
            .map(|r| r.iter().map(|c| c.colspan.max(1) as usize).sum::<usize>())
            .max()
            .unwrap_or(0);
        let columns = if t.columns > 0 { t.columns } else { widest }.max(1);
        let mut rows = Vec::new();
        for (r, row) in t.rows.iter().enumerate() {
            let left = t.rows.len() - r;
            let cells: Vec<String> = row
                .iter()
                .map(|c| self.cell(c, columns, left, &inner))
                .collect::<Result<_>>()?;
            rows.push(cells.join(", "));
        }
        let mut lines = vec!["table(".to_owned(), format!("{inner}columns: {columns},")];
        let head = t.header_rows.min(rows.len());
        if head == 1 {
            lines.push(format!("{inner}table.header({}),", rows[0]));
        } else if head > 1 {
            lines.push(format!("{inner}table.header("));
            for r in rows[..head].iter().filter(|r| !r.is_empty()) {
                lines.push(format!("{inner}  {r},"));
            }
            lines.push(format!("{inner}),"));
        }
        for r in rows[head..].iter().filter(|r| !r.is_empty()) {
            lines.push(format!("{inner}{r},"));
        }
        lines.push(format!("{indent})"));
        self.depth -= 1;
        Ok(lines.join("\n"))
    }

    /// A cell (`[…]`, `table.cell(colspan: 2)[…]`); spans are clipped to
    /// the table.
    fn cell(&mut self, c: &Cell, columns: usize, rows_left: usize, inner: &str) -> Result<String> {
        let body = match &c.content[..] {
            [] => String::new(),
            [Block::Para(content)] => self.line(content, false)?,
            blocks => self.blocks(blocks)?,
        };
        let body = format!("[{}]", indent(&body, inner));
        let colspan = (c.colspan as usize).clamp(1, columns);
        let rowspan = (c.rowspan as usize).clamp(1, rows_left.max(1));
        let mut spans = Vec::new();
        if colspan > 1 {
            spans.push(format!("colspan: {colspan}"));
        }
        if rowspan > 1 {
            spans.push(format!("rowspan: {rowspan}"));
        }
        Ok(match spans.is_empty() {
            true => body,
            false => format!("table.cell({}){body}", spans.join(", ")),
        })
    }
}

/// Adjacent text runs as one (so escaping sees the characters around).
fn merge_texts(items: &[Inline]) -> Cow<'_, [Inline]> {
    let mergeable = items
        .windows(2)
        .any(|w| matches!(w, [Inline::Text(_), Inline::Text(_)]))
        || items
            .iter()
            .any(|i| matches!(i, Inline::Text(t) if t.is_empty()));
    if !mergeable {
        return Cow::Borrowed(items);
    }
    let mut out: Vec<Inline> = Vec::with_capacity(items.len());
    for item in items {
        match (out.last_mut(), item) {
            (_, Inline::Text(t)) if t.is_empty() => {}
            (Some(Inline::Text(prev)), Inline::Text(t)) => prev.push_str(t),
            _ => out.push(item.clone()),
        }
    }
    Cow::Owned(out)
}

/// The first character `item` writes, near enough for escaping the one
/// before it.
fn first_char(item: &Inline) -> char {
    match item {
        Inline::Text(t) => t
            .chars()
            .find(|c| is_space(*c) || !c.is_control())
            .map_or(' ', |c| if is_space(c) { ' ' } else { c }),
        Inline::Emph(_) => '_',
        Inline::Strong(_) => '*',
        Inline::LineBreak => ' ',
        Inline::Math(_) => '$',
        Inline::Code(_) => '`',
        Inline::Cite(_) | Inline::Ref { .. } => '@',
        _ => '#',
    }
}

/// Whether a span delimited by `delim` (`*` bold, `_` italic) is inside
/// `items` (nesting it would close the outer one). Deep nesting counts as
/// yes.
fn contains_span(items: &[Inline], delim: char, depth: usize) -> bool {
    if depth > MAX_DEPTH {
        return true;
    }
    items.iter().any(|i| match i {
        Inline::Emph(c) => delim == '_' || contains_span(c, delim, depth + 1),
        Inline::Strong(c) => delim == '*' || contains_span(c, delim, depth + 1),
        Inline::Underline(c) | Inline::Strike(c) | Inline::Super(c) | Inline::Sub(c) => {
            contains_span(c, delim, depth + 1)
        }
        Inline::Link { content, .. } => contains_span(content, delim, depth + 1),
        _ => false,
    })
}

/// Typst markup's whitespace (other spaces, such as no-break spaces, are
/// text).
fn is_space(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

/// Can continue a reference's key (`@key`): escaped right after one.
fn joins_key(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// Writes `t` as text: whitespace runs as one space (none at a line
/// start), control characters dropped, and everything Typst would read as
/// markup escaped.
fn text(t: &str, out: &mut Out, follow: Option<char>) {
    let chars: Vec<char> = t
        .chars()
        .map(|c| if is_space(c) { ' ' } else { c })
        .filter(|c| *c == ' ' || !c.is_control())
        .collect();
    // The `.` of a would-be enum marker (`1. `) at the line start.
    let mut dot = None;
    for (i, &c) in chars.iter().enumerate() {
        if c == ' ' {
            out.space();
            continue;
        }
        let next = chars.get(i + 1).copied().or(follow);
        if out.start && c.is_ascii_digit() {
            let end = chars[i..]
                .iter()
                .position(|c| !c.is_ascii_digit())
                .map_or(chars.len(), |n| i + n);
            let after = chars.get(end + 1).copied().or(follow);
            if chars.get(end) == Some(&'.') && after.is_none_or(char::is_whitespace) {
                dot = Some(end);
            }
        }
        // Typst's markers end at any whitespace, a no-break space too.
        let ends_marker = next.is_none_or(char::is_whitespace);
        let escape = match c {
            '\\' | '*' | '_' | '#' | '$' | '@' | '<' | '[' | ']' | '`' | '~' => true,
            // `//`, `/*` and `*/` (after a bold delimiter) are comments.
            '/' => {
                matches!(next, Some('/' | '*'))
                    || out.s.ends_with('*')
                    || (out.start && ends_marker)
            }
            '-' => {
                matches!(next, Some('-' | '?'))
                    || next.is_some_and(char::is_numeric)
                    || (out.start && ends_marker)
            }
            '.' => next == Some('.') || dot == Some(i),
            '=' | '+' => out.start,
            _ => false,
        } || match out.after {
            After::Nothing => false,
            After::Code => {
                matches!(c, '(' | ';')
                    || (c == '.' && next.is_some_and(|n| n.is_alphabetic() || n == '_'))
            }
            After::Ref => {
                joins_key(c)
                    || (matches!(c, '.' | ':')
                        && next.is_some_and(|n| joins_key(n) || matches!(n, '.' | ':')))
            }
        };
        if escape {
            out.s.push('\\');
        }
        out.s.push(c);
        out.start = false;
        out.after = After::Nothing;
    }
}

/// `@key` (with `[supplement]`), or `#cite(label("key"))` for a key Typst's
/// reference syntax can't hold.
fn reference(key: &str, supplement: Option<&str>, out: &mut Out) {
    let literal = key
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))
        && !key.ends_with(['.', ':']);
    match (literal, supplement) {
        (true, None) => out.push(&format!("@{key}"), After::Ref),
        (true, Some(s)) => out.push(&format!("@{key}[{s}]"), After::Nothing),
        (false, None) => out.push(&format!("#cite(label({}))", string(key)), After::Code),
        (false, Some(s)) => out.push(
            &format!("#cite(label({}), supplement: [{s}])", string(key)),
            After::Code,
        ),
    }
}

/// A label Typst's `<…>` syntax holds: other characters become `-`; `None`
/// if nothing is left.
fn clean_label(label: &str) -> Option<String> {
    let s: String = label
        .trim()
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' | '.' | ':' => c,
            _ => '-',
        })
        .collect();
    let s = s.trim_start_matches(['.', ':']);
    (!s.is_empty()).then(|| s.to_owned())
}

/// ` <label>`, or nothing.
fn label_suffix(label: &Option<String>) -> String {
    label
        .as_deref()
        .and_then(clean_label)
        .map(|l| format!(" <{l}>"))
        .unwrap_or_default()
}

/// A Typst string literal.
pub(crate) fn string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Typst math as given, trimmed, with any bare `$` escaped (it would end the
/// formula).
fn math_source(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.trim().chars();
    let mut quoted = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                out.push(c);
                out.extend(chars.next());
            }
            '"' => {
                quoted = !quoted;
                out.push(c);
            }
            '$' if !quoted => out.push_str("\\$"),
            c => out.push(c),
        }
    }
    out
}

/// A raw block fenced longer than any run of backticks in it.
fn code_block(lang: Option<&str>, text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat((longest + 1).max(3));
    let lang = lang
        .map(str::trim)
        .filter(|l| {
            l.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
        .unwrap_or("");
    format!(
        "{fence}{lang}\n{}\n{fence}",
        text.trim_end_matches(['\n', '\r'])
    )
}

/// An image width in percent of the column: the app's 80% if unknown.
fn percent(width: Option<f64>) -> u32 {
    width
        .filter(|w| w.is_finite() && *w > 0.0)
        .map_or(80, |w| (w * 100.0).round().clamp(1.0, 100.0) as u32)
}

/// `s` with every line after the first indented by `pad` (blank lines stay
/// empty).
fn indent(s: &str, pad: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, line) in s.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
            if !line.is_empty() {
                out.push_str(pad);
            }
        }
        out.push_str(line);
    }
    out
}
