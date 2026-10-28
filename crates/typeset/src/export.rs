//! Exports: the document compiled for Typst's HTML target and its DOM
//! walked into the convert model ([`lethora_convert::Doc`]), which a writer
//! of the convert registry turns into a file. Reading the compiled document
//! rather than the markup brings citations, the bibliography, numbering,
//! cross-references and footnotes out resolved.
//!
//! The source is the one the pages are compiled from (content, expert code,
//! bibliography), with [`layout::export_preamble`] instead of the paged
//! preamble and without overrides (content never contains layout). Typst's
//! HTML export is experimental (0.15); its "… was ignored during HTML
//! export" warnings become [`WarningKind::NotExported`].
//!
//! How the DOM maps: `<p>`, `<h2>`–`<h6>` (Typst offsets heading levels by
//! one), `<strong>`, `<em>`, `<s>`, `<sub>`, `<sup>`, underline spans,
//! lists, `<blockquote>`, `<pre>`/`<code>`, links and `<br>` map one to one.
//! The introspection tags in the DOM (an element's start and end, with the
//! Typst element) give the labels of headings, figures and equations,
//! references ([`Inline::Ref`] with their typeset text), citation groups
//! ([`Inline::Cite`]: keys and supplements from the element, the typeset
//! text from the DOM) and footnotes, whose notes Typst puts at the end
//! (`doc-endnotes`) and which go back into [`Inline::Footnote`] at the
//! reference. A figure's caption loses its typeset prefix ("Figure 1: ")
//! to the number. `<math>` keeps its MathML and gets its Typst source from
//! the equation's span. Images come as data URLs and become resources
//! named after the file the source names. The bibliography is its heading
//! and a list.

use std::collections::HashMap;
use std::thread;

use lethora_convert::model::{
    Block, Cell, Cite, CiteItem, Doc, Figure, FigureBody, Image, Inline, List, Math, MathSize,
    Meta, Resource, Table,
};
use lethora_convert::{
    Imported, PageHints, WarningKind, Warnings, WriteOptions, Written, registry,
};
use resvg::{tiny_skia, usvg};
use typst::comemo::{Track, Tracked};
use typst::foundations::Content;
use typst::introspection::Tag;
use typst::math::EquationElem;
use typst::model::{
    CiteElem, CiteGroup, FigureElem, FootnoteElem, HeadingElem, LateLinkResolver, RefElem,
};
use typst::syntax::{Source, SyntaxKind, SyntaxNode};
use typst_html::{HtmlDocument, HtmlElement, HtmlNode};

use crate::compile::{CUSTOM, CompileRequest, DocCompiler, DocWorld, MAIN, compile_source};
use crate::layout::{self, CaptionPosition, ChapterBreak, Layout, Length};
use crate::math::{MathRenderer, MathStyle, TYPESET_STACK};

/// The end of the warnings Typst gives for what HTML export leaves out.
const IGNORED: &str = " was ignored during HTML export";

impl DocCompiler {
    /// The document of `request`, compiled for HTML, as the model.
    pub fn export_model(&self, request: &CompileRequest) -> Result<Imported, String> {
        let world = DocWorld::new(self.fonts, request, Source::new(*MAIN, source(request)));
        // Large stack: Typst recurses deeply; panics are caught by the join.
        thread::scope(|scope| {
            thread::Builder::new()
                .name("export-document".into())
                .stack_size(TYPESET_STACK)
                .spawn_scoped(scope, || to_model(&world, request))
                .map_err(|e| format!("Could not start the export: {e}"))?
                .join()
                .unwrap_or_else(|_| Err("Typst failed unexpectedly on this document.".into()))
        })
    }

    /// The document of `request` written in the registry format `format`;
    /// `stem` names the side files (see [`WriteOptions::stem`]).
    pub fn export(
        &self,
        request: &CompileRequest,
        format: &str,
        stem: &str,
    ) -> Result<Written, String> {
        let format = registry::by_id(format)
            .filter(|f| f.writer.is_some())
            .ok_or_else(|| format!("Lethora can't export {format}."))?;
        let mut model = self.export_model(request)?;
        if format.math_images {
            add_math_images(&mut model.doc, request);
        }
        if format.raster_fallbacks {
            add_raster_fallbacks(&mut model.doc);
        }
        let mut options = WriteOptions::default();
        options.stem = stem.to_owned();
        options.page = Some(page_hints(&request.layout));
        let written = format
            .writer
            .expect("filtered")
            .write(&model.doc, &options)
            .map_err(|e| e.to_string())?;
        let mut warnings = Warnings::default();
        warnings.extend(model.warnings);
        warnings.extend(written.warnings);
        Ok(Written {
            warnings: warnings.into_vec(),
            ..written
        })
    }
}

/// The source to compile for an export.
fn source(request: &CompileRequest) -> String {
    let preamble = layout::export_preamble(&request.layout, &request.meta);
    // Expert code with syntax errors is left out, as for the pages.
    let custom = Source::new(*CUSTOM, request.custom.clone());
    let custom = match custom.root().errors_and_warnings().0.is_empty() {
        true => request.custom.as_str(),
        false => "",
    };
    let bibliography = (!request.bibliography.trim().is_empty()
        && crate::compile::cites_sources(&request.content))
    .then(|| layout::bibliography(&request.layout));
    let (text, _) = compile_source(
        &preamble,
        custom,
        &request.content,
        &[],
        bibliography.as_deref(),
    );
    text
}

fn to_model(world: &DocWorld, request: &CompileRequest) -> Result<Imported, String> {
    let warned = typst::compile::<HtmlDocument>(world);
    let document = warned.output.map_err(|errors| {
        errors
            .first()
            .map_or_else(|| "The export failed.".into(), |e| e.message.to_string())
    })?;
    let mut warnings = Warnings::default();
    for w in &warned.warnings {
        if let Some(what) = w.message.strip_suffix(IGNORED) {
            warnings.add_example(WarningKind::NotExported, what);
        }
    }
    let root = document.root();
    let mut notes = HashMap::new();
    endnotes(&root.children, &mut notes);
    let resolver = LateLinkResolver::new(None, document.introspector().as_ref());
    let mut walker = Walker {
        main: &world.main,
        notes,
        resources: Vec::new(),
        warnings,
        equations: 0,
        number_equations: request.layout.equations.numbering,
        label: None,
        resolver: resolver.track(),
    };
    let mut items = Vec::new();
    walker.items(&root.children, &mut items);
    let m = &request.meta;
    let some = |s: &str| Some(s.trim().to_owned()).filter(|s| !s.is_empty());
    let meta = Meta {
        title: some(&m.title),
        subtitle: some(&m.subtitle),
        authors: some(&m.author).into_iter().collect(),
        date: some(&m.date),
        lang: some(&request.layout.text.lang),
    };
    Ok(Imported {
        doc: Doc {
            meta,
            blocks: blocks(items),
            resources: walker.resources,
        },
        warnings: walker.warnings.into_vec(),
    })
}

/// The footnotes' notes (`doc-endnotes`) by the id their references link
/// to: the `<li>`'s children (the backlink is skipped when walking them).
fn endnotes<'a>(nodes: &'a [HtmlNode], out: &mut HashMap<String, &'a [HtmlNode]>) {
    for e in elements(nodes) {
        if attr(e, "role") == Some("doc-endnotes") {
            for li in elements(&e.children).flat_map(|ol| elements(&ol.children)) {
                if let Some(id) = attr(li, "id") {
                    out.insert(id.to_owned(), &li.children);
                }
            }
        } else if matches!(name(e).as_str(), "html" | "body") {
            endnotes(&e.children, out);
        }
    }
}

/// What a node becomes: blocks and inlines, sorted into paragraphs later.
enum Item {
    Block(Block),
    Inline(Inline),
}

/// The elements a label goes to.
#[derive(Clone, Copy, PartialEq)]
enum Labelled {
    Heading,
    Figure,
    Equation,
}

struct Walker<'a> {
    main: &'a Source,
    /// The footnotes' notes by id (see [`endnotes`]).
    notes: HashMap<String, &'a [HtmlNode]>,
    resources: Vec<Resource>,
    warnings: Warnings,
    /// Display equations so far, and whether they are numbered.
    equations: usize,
    number_equations: bool,
    /// The label of the heading, figure or equation whose start tag came
    /// last, for its element.
    label: Option<(Labelled, String)>,
    /// For drawings (`html.frame`) linking into the document.
    resolver: Tracked<'a, LateLinkResolver<'a>>,
}

impl<'a> Walker<'a> {
    fn items(&mut self, nodes: &'a [HtmlNode], out: &mut Vec<Item>) {
        let mut i = 0;
        while i < nodes.len() {
            match &nodes[i] {
                HtmlNode::Tag(Tag::Start(content, _)) => {
                    if let Some(end) = self.tagged(content, &nodes[i + 1..], out) {
                        // Past the end tag.
                        i += end + 2;
                        continue;
                    }
                }
                HtmlNode::Tag(Tag::End(..)) => {}
                HtmlNode::Text(text, _) => out.push(Item::Inline(Inline::Text(text.to_string()))),
                HtmlNode::Element(e) => self.element(e, out),
                HtmlNode::Frame(frame) => {
                    let svg = typst_svg::svg_in_html(
                        &frame.inner,
                        frame.text_size,
                        false,
                        None,
                        "",
                        &[],
                        self.resolver,
                    );
                    let resource = self.resource("drawing.svg", "image/svg+xml", svg.into_bytes());
                    out.push(Item::Inline(Inline::Image(Image {
                        resource,
                        alt: None,
                        width: None,
                    })));
                }
            }
            i += 1;
        }
    }

    /// An element's start tag, followed by `rest`. References, citations
    /// and footnotes take the nodes up to their end tag (its index in
    /// `rest` is returned); headings, figures and equations leave their
    /// label for their element.
    fn tagged(
        &mut self,
        content: &Content,
        rest: &'a [HtmlNode],
        out: &mut Vec<Item>,
    ) -> Option<usize> {
        let labelled = if content.is::<HeadingElem>() {
            Some(Labelled::Heading)
        } else if content.is::<FigureElem>() {
            Some(Labelled::Figure)
        } else if content.is::<EquationElem>() {
            Some(Labelled::Equation)
        } else {
            None
        };
        if let Some(kind) = labelled {
            self.label = content
                .label()
                .map(|l| (kind, l.resolve().as_str().to_owned()));
            return None;
        }
        if !(content.is::<RefElem>() || content.is::<CiteGroup>() || content.is::<FootnoteElem>()) {
            return None;
        }
        let loc = content.location()?;
        let end = rest
            .iter()
            .position(|n| matches!(n, HtmlNode::Tag(Tag::End(l, ..)) if *l == loc))?;
        let inner = &rest[..end];
        if let Some(reference) = content.to_packed::<RefElem>() {
            // A reference to a source is a citation: its group follows (or,
            // for the later citations of a group, came with the first).
            let cites = |n: &HtmlNode| matches!(n, HtmlNode::Tag(Tag::Start(c, _)) if c.is::<CiteElem>() || c.is::<CiteGroup>());
            if inner.iter().any(cites) {
                return None;
            }
            let text = self.inlines(inner);
            out.push(Item::Inline(Inline::Ref {
                label: reference.target.resolve().as_str().to_owned(),
                text: Some(text),
            }));
        } else if let Some(group) = content.to_packed::<CiteGroup>() {
            let items = group
                .children
                .iter()
                .filter_map(|c| c.to_packed::<CiteElem>())
                .map(|c| CiteItem {
                    key: c.key.resolve().as_str().to_owned(),
                    locator: c
                        .supplement
                        .as_option()
                        .clone()
                        .flatten()
                        .map(|s| s.plain_text().to_string())
                        .filter(|s| !s.is_empty()),
                })
                .collect();
            let text = self.inlines(inner);
            out.push(Item::Inline(Inline::Cite(Cite {
                items,
                text: Some(text),
            })));
        } else {
            let note = links(inner)
                .find_map(|href| href.strip_prefix('#'))
                .and_then(|id| self.notes.get(id).copied());
            match note {
                Some(note) => {
                    let blocks = self.blocks(note);
                    out.push(Item::Inline(Inline::Footnote(blocks)));
                }
                None => self.warnings.add(WarningKind::MissingFootnote),
            }
        }
        Some(end)
    }

    fn element(&mut self, e: &'a HtmlElement, out: &mut Vec<Item>) {
        let tag = name(e);
        let role = attr(e, "role");
        let span = |make: fn(Vec<Inline>) -> Inline, w: &mut Self| make(w.inlines(&e.children));
        let item = match tag.as_str() {
            "head" => return,
            "nav" => {
                self.warnings.add(WarningKind::Contents);
                return;
            }
            // The notes go back to their references.
            "section" if role == Some("doc-endnotes") => return,
            "sup" if role == Some("doc-backlink") => return,
            "p" => {
                out.extend(self.blocks(&e.children).into_iter().map(Item::Block));
                return;
            }
            "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = tag.as_str()[1..].parse::<u8>().unwrap_or(2) - 1;
                self.heading(level, e)
            }
            "div" if role == Some("heading") => {
                let level = attr(e, "aria-level").and_then(|l| l.parse::<u8>().ok());
                self.heading(level.unwrap_or(7).saturating_sub(1).clamp(1, 6), e)
            }
            "strong" | "b" => Item::Inline(span(Inline::Strong, self)),
            "em" | "i" => Item::Inline(span(Inline::Emph, self)),
            "s" | "del" => Item::Inline(span(Inline::Strike, self)),
            "sub" => Item::Inline(span(Inline::Sub, self)),
            "sup" => Item::Inline(span(Inline::Super, self)),
            "u" => Item::Inline(span(Inline::Underline, self)),
            "span" if attr(e, "style").is_some_and(|s| s.contains("underline")) => {
                Item::Inline(span(Inline::Underline, self))
            }
            "code" => Item::Inline(Inline::Code(code_text(&e.children))),
            "br" => Item::Inline(Inline::LineBreak),
            "a" => match attr(e, "href") {
                Some(url) if !url.starts_with('#') => Item::Inline(Inline::Link {
                    url: url.to_owned(),
                    content: self.inlines(&e.children),
                }),
                // A link inside the document: its text.
                _ => return self.items(&e.children, out),
            },
            "math" => self.math(e),
            "img" => Item::Inline(self.image(e)),
            "ul" | "ol" => Item::Block(self.list(e)),
            "blockquote" => Item::Block(Block::Quote(self.blocks(&e.children))),
            "pre" => {
                let lang = elements(&e.children)
                    .find(|c| name(c).as_str() == "code")
                    .and_then(|c| attr(c, "data-lang"))
                    .map(str::to_owned);
                Item::Block(Block::Code {
                    lang,
                    text: code_text(&e.children),
                })
            }
            "figure" => {
                out.extend(self.figure(e).into_iter().map(Item::Block));
                return;
            }
            "table" => Item::Block(Block::Table(self.table(e))),
            "hr" => Item::Block(Block::Rule),
            // Term lists: the term in bold, then the description.
            "dl" => {
                for c in elements(&e.children) {
                    if name(c).as_str() == "dt" {
                        let term = Inline::Strong(self.inlines(&c.children));
                        out.push(Item::Block(Block::Para(vec![term])));
                    } else {
                        out.extend(self.blocks(&c.children).into_iter().map(Item::Block));
                    }
                }
                return;
            }
            // Containers (html, body, div, section, …) and spans that only
            // style: their content.
            _ => return self.items(&e.children, out),
        };
        out.push(item);
    }

    fn blocks(&mut self, nodes: &'a [HtmlNode]) -> Vec<Block> {
        let mut items = Vec::new();
        self.items(nodes, &mut items);
        blocks(items)
    }

    fn inlines(&mut self, nodes: &'a [HtmlNode]) -> Vec<Inline> {
        let mut items = Vec::new();
        self.items(nodes, &mut items);
        inlines(items)
    }

    /// The label left by the last start tag, if it is for `kind`.
    fn take_label(&mut self, kind: Labelled) -> Option<String> {
        match self.label.take() {
            Some((k, label)) if k == kind => Some(label),
            _ => None,
        }
    }

    fn heading(&mut self, level: u8, e: &'a HtmlElement) -> Item {
        let label = self.take_label(Labelled::Heading);
        Item::Block(Block::Heading {
            level,
            content: trim(self.inlines(&e.children)),
            label,
        })
    }

    fn math(&mut self, e: &'a HtmlElement) -> Item {
        let mut mathml = String::new();
        write_mathml(e, true, &mut mathml);
        let typst = self
            .main
            .find(e.span)
            .filter(|n| n.kind() == SyntaxKind::Equation)
            .map(|n| {
                self.main.text()[n.range()]
                    .trim_matches('$')
                    .trim()
                    .to_owned()
            })
            .unwrap_or_else(|| code_text(&e.children));
        let math = Math {
            mathml: Some(mathml),
            ..Math::typst(typst)
        };
        if attr(e, "display") != Some("block") {
            return Item::Inline(Inline::Math(math));
        }
        let label = self.take_label(Labelled::Equation);
        let number = self.number_equations.then(|| {
            self.equations += 1;
            format!("({})", self.equations)
        });
        Item::Block(Block::Math {
            math,
            label,
            number,
        })
    }

    fn image(&mut self, e: &'a HtmlElement) -> Inline {
        let (mime, bytes) = attr(e, "src").and_then(data_url).unwrap_or_default();
        let ext = match mime.as_str() {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/svg+xml" => "svg",
            "image/webp" => "webp",
            _ => "bin",
        };
        // The file the source names (`image("assets/plot.png")`), if any.
        let named = self
            .main
            .find(e.span)
            .and_then(|n| first_str(n.get()))
            .and_then(|path| path.rsplit('/').next().map(str::to_owned))
            .filter(|n| n.contains('.') && !n.starts_with('.'));
        let file = named.unwrap_or_else(|| format!("image.{ext}"));
        let resource = self.resource(&file, &mime, bytes);
        Inline::Image(Image {
            resource,
            alt: attr(e, "alt").map(str::to_owned).filter(|a| !a.is_empty()),
            width: attr(e, "style").and_then(percent_width),
        })
    }

    /// Adds a resource named `file` (or `file-2`, … if another one has that
    /// name), reusing one with the same name and bytes; its name.
    fn resource(&mut self, file: &str, mime: &str, bytes: Vec<u8>) -> String {
        let (stem, ext) = file.rsplit_once('.').unwrap_or((file, "bin"));
        let mut n = 1;
        loop {
            let name = match n {
                1 => file.to_owned(),
                n => format!("{stem}-{n}.{ext}"),
            };
            match self.resources.iter().find(|r| r.name == name) {
                Some(r) if r.bytes == bytes => return name,
                Some(_) => n += 1,
                None => {
                    self.resources.push(Resource {
                        name: name.clone(),
                        mime: mime.to_owned(),
                        bytes,
                        fallback: None,
                    });
                    return name;
                }
            }
        }
    }

    fn list(&mut self, e: &'a HtmlElement) -> Block {
        let start = attr(e, "start").and_then(|s| s.parse().ok()).unwrap_or(1);
        let items = elements(&e.children)
            .filter(|li| name(li).as_str() == "li")
            .map(|li| self.blocks(&li.children))
            .collect();
        Block::List(List {
            ordered: name(e).as_str() == "ol",
            start,
            items,
        })
    }

    /// A figure: an image or table with its caption, number and label;
    /// another body (code, a formula) as its blocks, then the caption.
    fn figure(&mut self, e: &'a HtmlElement) -> Vec<Block> {
        let label = self.take_label(Labelled::Figure);
        let is_caption =
            |n: &HtmlNode| matches!(n, HtmlNode::Element(c) if name(c).as_str() == "figcaption");
        let (number, caption) = match e.children.iter().find(|n| is_caption(n)) {
            Some(HtmlNode::Element(c)) => self.caption(&c.children),
            _ => (None, Vec::new()),
        };
        let mut items = Vec::new();
        for node in e.children.iter().filter(|n| !is_caption(n)) {
            self.items(std::slice::from_ref(node), &mut items);
        }
        let mut body = blocks(items);
        let figure_body = match body.as_slice() {
            [Block::Figure(f)] if f.caption.is_empty() => Some(f.body.clone()),
            [Block::Table(t)] => Some(FigureBody::Table(t.clone())),
            _ => None,
        };
        if let Some(body) = figure_body {
            return vec![Block::Figure(Figure {
                body,
                caption,
                label,
                number,
            })];
        }
        let mut text: Vec<Item> = number
            .map(|n| Item::Inline(Inline::Text(format!("{n}: "))))
            .into_iter()
            .collect();
        text.extend(caption.into_iter().map(Item::Inline));
        body.extend(blocks(text));
        body
    }

    /// A caption's number (its typeset prefix without the separator) and
    /// its text.
    fn caption(&mut self, nodes: &'a [HtmlNode]) -> (Option<String>, Vec<Inline>) {
        let mut prefix = String::new();
        let mut i = 0;
        while let Some(node) = nodes.get(i) {
            match node {
                HtmlNode::Tag(_) => {}
                // Generated text has no place in the source.
                HtmlNode::Text(t, span) if span.is_detached() => prefix.push_str(t),
                _ => break,
            }
            i += 1;
        }
        let number = prefix
            .trim_end()
            .trim_end_matches([':', '.', '–', '—'])
            .trim_end();
        let number = Some(number.to_owned()).filter(|n| !n.is_empty());
        (number, trim(self.inlines(&nodes[i..])))
    }

    fn table(&mut self, e: &'a HtmlElement) -> Table {
        let mut rows = Vec::new();
        let mut header_rows = 0;
        for c in elements(&e.children) {
            match name(c).as_str() {
                "tr" => rows.push(self.row(c)),
                group => {
                    for tr in elements(&c.children).filter(|r| name(r).as_str() == "tr") {
                        rows.push(self.row(tr));
                        header_rows += usize::from(group == "thead");
                    }
                }
            }
        }
        Table {
            columns: columns(&rows),
            header_rows,
            rows,
        }
    }

    fn row(&mut self, tr: &'a HtmlElement) -> Vec<Cell> {
        elements(&tr.children)
            .filter(|c| matches!(name(c).as_str(), "td" | "th"))
            .map(|c| {
                let span = |a| {
                    attr(c, a)
                        .and_then(|s| s.parse::<u32>().ok())
                        .unwrap_or(1)
                        .max(1)
                };
                Cell {
                    content: self.blocks(&c.children),
                    colspan: span("colspan"),
                    rowspan: span("rowspan"),
                }
            })
            .collect()
    }
}

/// The number of columns of a table whose rows hold the cells starting in
/// them (a cell spanning down covers slots of later rows).
fn columns(rows: &[Vec<Cell>]) -> usize {
    // Per column: how many more rows a cell from above covers.
    let mut covered: Vec<u32> = Vec::new();
    let mut width = 0;
    for row in rows {
        let mut col = 0;
        for cell in row {
            while covered.get(col).is_some_and(|&n| n > 0) {
                col += 1;
            }
            let span = cell.colspan as usize;
            if covered.len() < col + span {
                covered.resize(col + span, 0);
            }
            covered[col..col + span].fill(cell.rowspan);
            col += span;
        }
        width = width.max(covered.len());
        for slot in &mut covered {
            *slot = slot.saturating_sub(1);
        }
    }
    width
}

/// Items as blocks: runs of inlines become paragraphs, an image alone a
/// figure without a caption (as readers do).
fn blocks(items: Vec<Item>) -> Vec<Block> {
    let mut out = Vec::new();
    let mut run = Vec::new();
    let flush = |run: &mut Vec<Item>, out: &mut Vec<Block>| {
        let content = trim(inlines(std::mem::take(run)));
        match &content[..] {
            [] => {}
            [Inline::Image(img)] => out.push(Block::Figure(Figure {
                body: FigureBody::Image(img.clone()),
                caption: Vec::new(),
                label: None,
                number: None,
            })),
            _ => {
                // An image in the line (an icon): its width is no share of
                // the text column (the layout's default width applies to
                // every image).
                let mut content = content;
                for i in &mut content {
                    if let Inline::Image(img) = i {
                        img.width = None;
                    }
                }
                out.push(Block::Para(content));
            }
        }
    };
    for item in items {
        match item {
            Item::Block(b) => {
                flush(&mut run, &mut out);
                out.push(b);
            }
            inline => run.push(inline),
        }
    }
    flush(&mut run, &mut out);
    out
}

/// Items as inlines (blocks inside inline content keep their text and
/// formulas), adjacent text merged.
fn inlines(items: Vec<Item>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    for item in items {
        let new = match item {
            Item::Inline(i) => vec![i],
            Item::Block(Block::Para(c) | Block::Heading { content: c, .. }) => c,
            Item::Block(Block::Math { math, .. }) => vec![Inline::Math(math)],
            Item::Block(Block::Figure(Figure {
                body: FigureBody::Image(img),
                ..
            })) => vec![Inline::Image(img)],
            Item::Block(_) => Vec::new(),
        };
        for i in new {
            match (out.last_mut(), i) {
                (Some(Inline::Text(prev)), Inline::Text(t)) => prev.push_str(&t),
                (_, i) => out.push(i),
            }
        }
    }
    out
}

/// Without whitespace and line breaks at either end.
fn trim(mut content: Vec<Inline>) -> Vec<Inline> {
    while let Some(Inline::Text(t)) = content.first_mut() {
        *t = t.trim_start().to_owned();
        if !t.is_empty() {
            break;
        }
        content.remove(0);
    }
    while let Some(last) = content.last_mut() {
        match last {
            Inline::Text(t) => {
                *t = t.trim_end().to_owned();
                if !t.is_empty() {
                    break;
                }
            }
            Inline::LineBreak => {}
            _ => break,
        }
        content.pop();
    }
    content
}

fn name(e: &HtmlElement) -> typst::utils::ResolvedPicoStr {
    e.tag.resolve()
}

fn attr<'a>(e: &'a HtmlElement, key: &str) -> Option<&'a str> {
    e.attrs
        .0
        .iter()
        .find(|(k, _)| k.resolve().as_str() == key)
        .map(|(_, v)| v.as_str())
}

fn elements(nodes: &[HtmlNode]) -> impl Iterator<Item = &HtmlElement> {
    nodes.iter().filter_map(|n| match n {
        HtmlNode::Element(e) => Some(e),
        _ => None,
    })
}

/// The `href`s of the links in `nodes`, depth first.
fn links(nodes: &[HtmlNode]) -> Box<dyn Iterator<Item = &str> + '_> {
    Box::new(elements(nodes).flat_map(|e| {
        let own = (name(e).as_str() == "a").then(|| attr(e, "href")).flatten();
        own.into_iter().chain(links(&e.children))
    }))
}

/// The text of code: its text nodes, `<br>` as a line break.
fn code_text(nodes: &[HtmlNode]) -> String {
    fn walk(nodes: &[HtmlNode], out: &mut String) {
        for n in nodes {
            match n {
                HtmlNode::Text(t, _) => out.push_str(t),
                HtmlNode::Element(e) if name(e).as_str() == "br" => out.push('\n'),
                HtmlNode::Element(e) => walk(&e.children, out),
                _ => {}
            }
        }
    }
    let mut out = String::new();
    walk(nodes, &mut out);
    out
}

/// A `<math>` element as MathML: the root gets the MathML namespace, ids
/// are dropped.
fn write_mathml(e: &HtmlElement, root: bool, out: &mut String) {
    let tag = name(e);
    out.push('<');
    out.push_str(tag.as_str());
    if root {
        out.push_str(" xmlns=\"http://www.w3.org/1998/Math/MathML\"");
    }
    for (k, v) in e.attrs.0.iter() {
        let k = k.resolve();
        if k.as_str() != "id" {
            out.push(' ');
            out.push_str(k.as_str());
            out.push_str("=\"");
            escape(v, out);
            out.push('"');
        }
    }
    out.push('>');
    for child in e.children.iter() {
        match child {
            HtmlNode::Text(t, _) => escape(t, out),
            HtmlNode::Element(c) => write_mathml(c, false, out),
            _ => {}
        }
    }
    out.push_str("</");
    out.push_str(tag.as_str());
    out.push('>');
}

fn escape(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}

/// The first string literal in `node` (an `image` call's path).
fn first_str(node: &SyntaxNode) -> Option<String> {
    if node.kind() == SyntaxKind::Str {
        return Some(node.leaf_text().trim_matches('"').to_owned());
    }
    node.children().find_map(first_str)
}

/// A percentage width in a style attribute (`width: 50%`) as a fraction.
fn percent_width(style: &str) -> Option<f64> {
    style.split(';').find_map(|decl| {
        let (k, v) = decl.split_once(':')?;
        let v = v.trim().strip_suffix('%')?;
        (k.trim() == "width").then(|| v.parse::<f64>().ok().map(|p| p / 100.0))?
    })
}

/// The media type and bytes of a base64 `data:` URL.
fn data_url(url: &str) -> Option<(String, Vec<u8>)> {
    let (head, data) = url.strip_prefix("data:")?.split_once(',')?;
    let mime = head.strip_suffix(";base64")?;
    Some((mime.to_owned(), base64(data)?))
}

fn base64(s: &str) -> Option<Vec<u8>> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes().filter(|&c| c != b'=') {
        acc = (acc << 6 | u32::from(value(c)?)) & 0xffff;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Renders every formula of `doc` as SVG, in the body text size, for
/// writers that can't show MathML (see `registry::Format::math_images`).
fn add_math_images(doc: &mut Doc, request: &CompileRequest) {
    let style = MathStyle {
        size_pt: request.layout.text.size.to_pt().unwrap_or(11.0),
        color: "#000000".into(),
    };
    let mut renderer = MathRenderer::new();
    let mut resources = Vec::new();
    let mut render = |math: &mut Math, display: bool| {
        if let Ok(r) = renderer.render_math(&math.typst, display, &style) {
            let name = format!("math-{}.svg", resources.len() + 1);
            resources.push(Resource {
                name: name.clone(),
                mime: "image/svg+xml".into(),
                bytes: r.svg.into_bytes(),
                fallback: None,
            });
            math.image = Some(name);
            math.size = Some(MathSize {
                width: r.width_pt,
                height: r.height_pt,
                baseline: r.baseline_pt,
            });
        }
    };
    each_math(&mut doc.blocks, &mut render);
    doc.resources.extend(resources);
}

/// The most pixels a side of a PNG twin has.
const MAX_TWIN_SIDE: f32 = 4096.0;

/// Gives every SVG resource of `doc` a PNG twin at 300 dpi (at most
/// [`MAX_TWIN_SIDE`] a side) for writers whose readers may not show SVG
/// (see `registry::Format::raster_fallbacks`): `Resource::fallback` names
/// it. An SVG resvg can't read gets none. Text in an SVG renders only as
/// paths (Typst's drawings and formulas are paths).
fn add_raster_fallbacks(doc: &mut Doc) {
    let mut twins: Vec<Resource> = Vec::new();
    for i in 0..doc.resources.len() {
        let r = &doc.resources[i];
        if r.mime != "image/svg+xml" || r.fallback.is_some() {
            continue;
        }
        let Some(png) = rasterize(&r.bytes) else {
            continue;
        };
        let stem = r.name.strip_suffix(".svg").unwrap_or(&r.name);
        let taken = |n: &str| doc.resources.iter().chain(&twins).any(|r| r.name == n);
        let mut name = format!("{stem}.png");
        let mut n = 2;
        while taken(&name) {
            name = format!("{stem}-{n}.png");
            n += 1;
        }
        doc.resources[i].fallback = Some(name.clone());
        twins.push(Resource {
            name,
            mime: "image/png".into(),
            bytes: png,
            fallback: None,
        });
    }
    doc.resources.extend(twins);
}

/// An SVG as a PNG at 300 dpi; `None` if it can't be read or is empty.
fn rasterize(svg: &[u8]) -> Option<Vec<u8>> {
    let mut options = usvg::Options::default();
    // Nothing from disk: an SVG's images come inside it or not at all.
    options.image_href_resolver.resolve_string = Box::new(|_, _| None);
    let tree = usvg::Tree::from_data(svg, &options).ok()?;
    // usvg's pixels are 96 to the inch.
    let size = tree.size();
    let scale = (300.0 / 96.0_f32).min(MAX_TWIN_SIDE / size.width().max(size.height()));
    let side = |v: f32| (v * scale).ceil().clamp(1.0, MAX_TWIN_SIDE) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(side(size.width()), side(size.height()))?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().ok()
}

/// What a writer that sets up pages (DOCX) takes from `layout`: the body
/// font and size, heading sizes, page size and margins, the paragraph look
/// and where captions go.
pub fn page_hints(layout: &Layout) -> PageHints {
    let font = layout.text.size.to_pt().unwrap_or(11.0);
    // Lengths in points; `em` relative to the body size.
    let abs = |l: Length| l.to_pt().unwrap_or(l.value() * font);
    let (width, height) = layout::page_size(layout);
    let (p, t) = (&layout.page, &layout.text);
    let h = &layout.headings;
    let leading = abs(t.leading);
    let mut hints = PageHints::default();
    hints.font = t.font.clone();
    hints.size_pt = font;
    hints.heading_sizes_pt = vec![abs(h.level1_size), abs(h.level2_size), abs(h.level3_size)];
    hints.width_pt = abs(width);
    hints.height_pt = abs(height);
    hints.margin_top_pt = abs(p.margin_top);
    hints.margin_bottom_pt = abs(p.margin_bottom);
    hints.margin_inside_pt = abs(p.margin_inside);
    hints.margin_outside_pt = abs(p.margin_outside);
    hints.two_sided = p.two_sided;
    hints.justify = t.justify;
    hints.hyphenate = t.hyphenate;
    hints.first_line_indent_pt = abs(t.first_line_indent);
    // Typst's leading runs from a line's baseline to the next one's cap
    // height (about 0.7 em); Word's single spacing is about 1.15 em from
    // baseline to baseline. Paragraph spacing replaces the leading.
    hints.line_spacing = (0.7 + leading / font) / 1.15;
    hints.paragraph_spacing_pt = (abs(t.paragraph_spacing) - leading).max(0.0);
    hints.chapter_page_break = h.chapter_pagebreak != ChapterBreak::None;
    hints.image_width = f64::from(layout.figures.default_width) / 100.0;
    hints.figure_caption_top = layout.figures.caption_position == CaptionPosition::Top;
    hints.table_caption_top = layout.tables.caption_position == CaptionPosition::Top;
    hints
}

/// Calls `f` on every formula in `blocks`, with whether it is a display one.
fn each_math(blocks: &mut [Block], f: &mut impl FnMut(&mut Math, bool)) {
    for block in blocks {
        match block {
            Block::Para(c) | Block::Heading { content: c, .. } => each_inline_math(c, f),
            Block::List(l) => l.items.iter_mut().for_each(|i| each_math(i, f)),
            Block::Quote(b) => each_math(b, f),
            Block::Math { math, .. } => f(math, true),
            Block::Figure(fig) => {
                each_inline_math(&mut fig.caption, f);
                if let FigureBody::Table(t) = &mut fig.body {
                    each_table_math(t, f);
                }
            }
            Block::Table(t) => each_table_math(t, f),
            Block::Code { .. } | Block::Rule => {}
        }
    }
}

fn each_table_math(t: &mut Table, f: &mut impl FnMut(&mut Math, bool)) {
    for cell in t.rows.iter_mut().flatten() {
        each_math(&mut cell.content, f);
    }
}

fn each_inline_math(inlines: &mut [Inline], f: &mut impl FnMut(&mut Math, bool)) {
    for i in inlines {
        match i {
            Inline::Math(m) => f(m, false),
            Inline::Emph(c)
            | Inline::Strong(c)
            | Inline::Underline(c)
            | Inline::Strike(c)
            | Inline::Super(c)
            | Inline::Sub(c)
            | Inline::Link { content: c, .. } => each_inline_math(c, f),
            Inline::Footnote(b) => each_math(b, f),
            Inline::Cite(Cite { text: Some(c), .. }) | Inline::Ref { text: Some(c), .. } => {
                each_inline_math(c, f)
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_urls_and_widths() {
        assert_eq!(base64("aGk=").unwrap(), b"hi");
        assert_eq!(base64("SGVsbG8sIFdvcmxkIQ==").unwrap(), b"Hello, World!");
        assert!(base64("a$b").is_none());
        assert_eq!(
            data_url("data:image/png;base64,aGk="),
            Some(("image/png".into(), b"hi".to_vec()))
        );
        assert_eq!(percent_width("display: block; width: 50%"), Some(0.5));
        assert_eq!(percent_width("width: 3cm"), None);
    }

    #[test]
    fn columns_count_spans() {
        let cell = |colspan, rowspan| Cell {
            content: Vec::new(),
            colspan,
            rowspan,
        };
        // A wide cell over two, then one two rows tall beside two.
        let rows = vec![
            vec![cell(2, 1), cell(1, 1)],
            vec![cell(1, 2), cell(1, 1), cell(1, 1)],
            vec![cell(1, 1), cell(1, 1)],
        ];
        assert_eq!(columns(&rows), 3);
        assert_eq!(columns(&[]), 0);
    }

    #[test]
    fn math_images_name_svg_resources() {
        let math = |s: &str| Math::typst(s);
        let mut doc = Doc {
            blocks: vec![
                Block::Para(vec![Inline::Footnote(vec![Block::Para(vec![
                    Inline::Math(math("x^2")),
                ])])]),
                Block::Math {
                    math: math("a/b"),
                    label: None,
                    number: None,
                },
                // Not a formula Typst takes: no image.
                Block::Math {
                    math: math("#eval(\"1\")"),
                    label: None,
                    number: None,
                },
            ],
            ..Doc::default()
        };
        let request = CompileRequest {
            project_dir: Default::default(),
            layout: layout::preset("paper").unwrap(),
            meta: Default::default(),
            custom: String::new(),
            content: String::new(),
            today: None,
            overrides: Vec::new(),
            bibliography: String::new(),
            assets: Default::default(),
        };
        add_math_images(&mut doc, &request);
        assert_eq!(doc.resources.len(), 2);
        assert!(doc.resources.iter().all(|r| r.bytes.starts_with(b"<svg")));
        let Block::Math { math, .. } = &doc.blocks[1] else {
            unreachable!()
        };
        assert_eq!(math.image.as_deref(), Some("math-2.svg"));
        let Block::Math { math, .. } = &doc.blocks[2] else {
            unreachable!()
        };
        assert_eq!(math.image, None);
        // Sizes come along; every SVG gets a PNG twin.
        let Block::Math { math, .. } = &doc.blocks[1] else {
            unreachable!()
        };
        let size = math.size.unwrap();
        assert!(size.height > size.baseline && size.baseline > 0.0);
        add_raster_fallbacks(&mut doc);
        assert_eq!(doc.resources.len(), 4);
        assert_eq!(doc.resources[1].fallback.as_deref(), Some("math-2.png"));
        let png = doc.resource("math-2.png").unwrap();
        assert!(png.bytes.starts_with(b"\x89PNG"));
    }

    #[test]
    fn page_hints_follow_the_layout() {
        let hints = page_hints(&layout::preset("novel").unwrap());
        assert_eq!((hints.width_pt, hints.height_pt), (432.0, 648.0));
        assert!(hints.two_sided && hints.chapter_page_break);
        assert_eq!(hints.heading_sizes_pt, [20.0, 14.0, 11.0]);
        assert_eq!(hints.image_width, 1.0);
        assert!(!hints.figure_caption_top && !hints.table_caption_top);
        let paper = page_hints(&layout::preset("paper").unwrap());
        assert!(paper.table_caption_top && paper.justify);
        assert_eq!(
            paper.paragraph_spacing_pt, 0.0,
            "spacing equal to the leading"
        );
        assert!((paper.line_spacing - 1.13).abs() < 0.01);
    }
}
