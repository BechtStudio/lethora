//! Whole-document compilation with the layout layer.

use std::fs;
use std::path::Path;
use std::time::Instant;

use lethora_typeset::layout::{self, Entries, Layout, Meta, Preamble};
use lethora_typeset::{
    CompileOutput, CompileRequest, Date, DocCompiler, Location, Severity, compile,
};
use serde_json::{Value, json};
use typst::layout::{Frame, FrameItem, Point};
use typst::visualize::Geometry;

const BIB: &str = r#"
@book{knuth84,
  author = {Knuth, Donald E.},
  title = {The {\TeX}book},
  publisher = {Addison-Wesley},
  year = {1984},
}

@article{doe2020,
  author = {Doe, Jane and Roe, Richard},
  title = {On Layout},
  journal = {Journal of Typesetting},
  year = {2020},
}
"#;

/// The blob `assets/plot.svg` maps to (any 64 lowercase hex characters).
const PLOT_BLOB: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="60"><rect width="120" height="60" fill="green"/></svg>"#;

/// A sample with three heading levels, paragraphs, inline and display math
/// (labelled), a figure with a project image, a table and citations.
const SAMPLE: &str = r#"= Introduction

This is the first paragraph with inline math $a^2 + b^2 = c^2$ and a citation @knuth84. #lorem(80)

Another paragraph, pointing at @fig:plot. #lorem(120)

$ e^(i pi) + 1 = 0 $ <eq:euler>

== Background

#lorem(150)

=== Details

#figure(image("assets/plot.svg"), caption: [A small plot.]) <fig:plot>

#figure(
  table(columns: 3, [A], [B], [C], [1], [2], [3]),
  caption: [A table.],
)

As @doe2020 argues, layout matters. #lorem(100)

= Results

#lorem(300)

== Discussion

#lorem(200)
"#;

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("assets")).unwrap();
    fs::write(dir.path().join("assets").join(PLOT_BLOB), SVG).unwrap();
    fs::write(dir.path().join("secret.svg"), SVG).unwrap();
    dir
}

fn meta() -> Meta {
    Meta {
        title: "A Study of Pages".into(),
        subtitle: "With a Subtitle".into(),
        author: "Ada Lovelace".into(),
        date: "October 2026".into(),
        contact: "Ada Lovelace\n12 Analytical Row\nada@example.org".into(),
    }
}

fn layout_of(pairs: &[(&str, Value)]) -> Layout {
    let entries: Entries = pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect();
    layout::resolve(&entries)
}

fn request(dir: &Path, layout: Layout, content: &str) -> CompileRequest {
    CompileRequest {
        project_dir: dir.to_path_buf(),
        layout,
        meta: meta(),
        custom: String::new(),
        content: content.into(),
        today: Some(Date {
            year: 2026,
            month: 10,
            day: 12,
        }),
        overrides: Vec::new(),
        bibliography: BIB.into(),
        assets: [("assets/plot.svg".to_owned(), PLOT_BLOB.to_owned())].into(),
    }
}

fn errors(out: &CompileOutput) -> Vec<String> {
    out.diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| format!("{} ({:?})", d.message, d.location))
        .collect()
}

fn texts(out: &CompileOutput) -> Vec<String> {
    let doc = out.document.as_ref().expect("compiled");
    (0..doc.pages())
        .map(|i| doc.page_text(i).unwrap())
        .collect()
}

fn compile_ok(req: &CompileRequest, what: &str) -> CompileOutput {
    let out = compile(req);
    assert!(errors(&out).is_empty(), "{what}: {:?}", errors(&out));
    assert!(out.document.is_some(), "{what}");
    out
}

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.5
}

#[test]
fn presets_and_variants_compile_the_sample() {
    let dir = project();
    type Variant<'a> = (&'a str, Vec<(&'a str, Value)>, [f64; 2]);
    let variants: Vec<Variant> = vec![
        ("paper", vec![("preset", json!("paper"))], [595.28, 841.89]),
        ("novel", vec![("preset", json!("novel"))], [432.0, 648.0]),
        (
            "thesis",
            vec![("preset", json!("thesis"))],
            [595.28, 841.89],
        ),
        (
            "manuscript",
            vec![("preset", json!("manuscript"))],
            [612.0, 792.0],
        ),
        (
            "paper, two columns, I. numbering, toc, title running head",
            vec![
                ("preset", json!("paper")),
                ("page.columns", json!(2)),
                ("headings.numbering", json!("I.")),
                ("front.toc", json!(true)),
                ("furniture.running_head", json!("title")),
                ("furniture.page_numbers", json!("top-outside")),
                ("figures.caption_position", json!("top")),
                ("citations.style", json!("ieee")),
                ("citations.bibliography_title", json!("Works \"Cited\"")),
            ],
            [595.28, 841.89],
        ),
        (
            "novel, custom size, A. numbering, page breaks, no numbers",
            vec![
                ("preset", json!("novel")),
                ("page.size", json!("custom")),
                ("page.width", json!("5in")),
                ("page.height", json!("8in")),
                ("headings.numbering", json!("A.")),
                ("headings.chapter_pagebreak", json!("page")),
                ("furniture.page_numbers", json!("none")),
                ("front.title_page", json!(false)),
                ("figures.numbering", json!(true)),
                ("equations.numbering", json!(true)),
            ],
            [360.0, 576.0],
        ),
        (
            "thesis, one-sided A5, 1. numbering, bottom center",
            vec![
                ("preset", json!("thesis")),
                ("page.size", json!("a5")),
                ("page.two_sided", json!(false)),
                ("headings.numbering", json!("1.")),
                ("furniture.page_numbers", json!("bottom-center")),
                ("text.lang", json!("de-DE")),
                ("text.font", json!("New Computer Modern")),
            ],
            [419.53, 595.28],
        ),
        (
            "manuscript on A4, mono font",
            vec![
                ("preset", json!("manuscript")),
                ("page.size", json!("a4")),
                ("text.font", json!("DejaVu Sans Mono")),
                ("furniture.running_head", json!("chapter")),
            ],
            [595.28, 841.89],
        ),
    ];
    for (name, pairs, [w, h]) in variants {
        let req = request(dir.path(), layout_of(&pairs), SAMPLE);
        let start = Instant::now();
        let out = compile_ok(&req, name);
        println!("{name}: {} pages in {:?}", out.pages, start.elapsed());
        assert!((2..40).contains(&out.pages), "{name}: {} pages", out.pages);
        assert_eq!(out.page_sizes.len(), out.pages);
        for [pw, ph] in &out.page_sizes {
            assert!(approx(*pw, w) && approx(*ph, h), "{name}: {pw}x{ph}");
        }
        // The bibliography is added because the content cites sources.
        let all = texts(&out).join(" ");
        assert!(all.contains("Knuth"), "{name}: no bibliography");
        assert!(all.contains("Layout"), "{name}");
    }
}

#[test]
fn no_bibliography_without_citations_or_library() {
    let dir = project();
    let content = "= Hello <h>\n\nSee @h. No sources here.";
    let req = request(
        dir.path(),
        layout_of(&[("preset", json!("paper"))]),
        content,
    );
    let out = compile_ok(&req, "no citations");
    assert!(!texts(&out).join(" ").contains("Bibliography"));

    for library in [
        "", " 

",
    ] {
        let mut req = request(
            dir.path(),
            layout_of(&[]),
            "Cites @knuth84 without a library.",
        );
        req.bibliography = library.into();
        let out = compile(&req);
        assert!(
            out.diagnostics
                .iter()
                .any(|d| matches!(d.location, Location::Content { .. })
                    && d.message.contains("knuth84")),
            "{:?}",
            out.diagnostics
        );
    }
}

/// Minimum x of the text on a page (points).
fn text_left(frame: &Frame, offset: Point, min: &mut f64) {
    for (pos, item) in frame.items() {
        let at = offset + *pos;
        match item {
            FrameItem::Group(g) => text_left(&g.frame, at, min),
            FrameItem::Text(_) => *min = min.min(at.x.to_pt()),
            _ => {}
        }
    }
}

#[test]
fn two_sided_margins_alternate() {
    let dir = project();
    let content = format!("= One\n\n{}", "#lorem(400)\n\n".repeat(6));
    for two_sided in [true, false] {
        let layout = layout_of(&[
            ("preset", json!("novel")),
            ("page.two_sided", json!(two_sided)),
            ("front.title_page", json!(false)),
            ("furniture.page_numbers", json!("none")),
            ("furniture.running_head", json!("none")),
            ("text.first_line_indent", json!("0pt")),
        ]);
        let out = compile_ok(&request(dir.path(), layout, &content), "margins");
        let doc = out.document.as_ref().unwrap().paged();
        assert!(doc.pages().len() >= 4);
        let left = |i: usize| {
            let mut min = f64::MAX;
            text_left(&doc.pages()[i].frame, Point::zero(), &mut min);
            min
        };
        // Inside 0.875in (63pt), outside 0.625in (45pt).
        // Physical pages 3 and 4 (after the title block's page, a chapter
        // opens on the right-hand page 3 when two-sided).
        if two_sided {
            assert!(approx(left(2), 63.0), "recto {}", left(2));
            assert!(approx(left(3), 45.0), "verso {}", left(3));
        } else {
            assert!(approx(left(2), 63.0) && approx(left(3), 63.0));
        }
    }
}

#[test]
fn title_page_toc_and_chapters_on_right_pages() {
    let dir = project();
    let out = compile_ok(
        &request(
            dir.path(),
            layout_of(&[("preset", json!("thesis"))]),
            SAMPLE,
        ),
        "thesis",
    );
    let pages = texts(&out);
    assert!(
        pages[0].contains("Study") && pages[0].contains("Ada"),
        "{}",
        pages[0]
    );
    assert!(pages[0].contains("Analytical"), "contact on the title page");
    assert!(pages[1].trim().is_empty(), "blank verso: {}", pages[1]);
    assert!(pages[2].contains("Contents"), "{}", pages[2]);
    assert!(pages[2].contains("Introduction") && pages[2].contains("Background"));
    // The page left blank to start the first chapter on the right has no
    // header or footer.
    assert!(pages[3].trim().is_empty(), "blank: {}", pages[3]);
    // Numbered chapters start on right-hand (odd) pages.
    for heading in ["Introduction", "Results"] {
        // The first mention after the contents is the chapter's opening page.
        let page = (3..pages.len())
            .find(|&i| pages[i].contains(heading))
            .unwrap();
        assert_eq!(page % 2, 0, "{heading} on physical page {}", page + 1);
        // Chapter openers carry no running head: the page starts with the
        // chapter number, not with the previous chapter's title.
        let before = pages[page].split(heading).next().unwrap().trim();
        assert!(
            !before.is_empty() && before.chars().all(|c| c.is_ascii_digit() || c == '.'),
            "running head on the opener of {heading}: {before:?}"
        );
    }

    // Without a title page, a title block opens the first page.
    let out = compile_ok(
        &request(
            dir.path(),
            layout_of(&[("preset", json!("paper")), ("front.toc", json!(true))]),
            SAMPLE,
        ),
        "paper",
    );
    let first = &texts(&out)[0];
    assert!(
        first.contains("Study") && first.contains("Contents") && first.contains("Introduction")
    );
    // A title page needs something to show.
    let mut req = request(
        dir.path(),
        layout_of(&[("preset", json!("thesis"))]),
        SAMPLE,
    );
    req.meta = Meta::default();
    let out = compile_ok(&req, "thesis without meta");
    assert!(texts(&out)[0].contains("Contents"));
}

/// Every text run and shape per page: position, size and text, rounded to
/// 0.01 pt.
fn runs(out: &CompileOutput) -> Vec<Vec<String>> {
    fn walk(frame: &Frame, offset: Point, out: &mut Vec<String>) {
        for (pos, item) in frame.items() {
            let at = offset + *pos;
            let xy = format!("{:.2},{:.2}", at.x.to_pt(), at.y.to_pt());
            match item {
                FrameItem::Group(g) => walk(&g.frame, at, out),
                FrameItem::Text(t) => out.push(format!("{xy} {:.2} {}", t.size.to_pt(), t.text)),
                FrameItem::Shape(..) => out.push(format!("{xy} shape")),
                _ => {}
            }
        }
    }
    let doc = out.document.as_ref().unwrap().paged();
    doc.pages()
        .iter()
        .map(|p| {
            let mut list = Vec::new();
            walk(&p.frame, Point::zero(), &mut list);
            list
        })
        .collect()
}

/// The first text run containing `needle`: page, position (baseline start),
/// size and weight.
fn run(out: &CompileOutput, needle: &str) -> (usize, Point, f64, u16) {
    fn walk(frame: &Frame, offset: Point, needle: &str) -> Option<(Point, f64, u16)> {
        frame.items().find_map(|(pos, item)| match item {
            FrameItem::Group(g) => walk(&g.frame, offset + *pos, needle),
            FrameItem::Text(t) if t.text.contains(needle) => Some((
                offset + *pos,
                t.size.to_pt(),
                t.font.info().variant.weight.to_number(),
            )),
            _ => None,
        })
    }
    let doc = out.document.as_ref().unwrap().paged();
    doc.pages()
        .iter()
        .enumerate()
        .find_map(|(i, p)| walk(&p.frame, Point::zero(), needle).map(|(at, s, w)| (i, at, s, w)))
        .expect(needle)
}

#[test]
fn heading_and_contents_defaults_keep_the_layout() {
    // Against the code generated before level 3 sized deeper headings and
    // before heading spacing and the contents settings existed.
    let dir = project();
    let mut compiler = DocCompiler::new();
    for preset in ["paper", "novel", "thesis", "manuscript"] {
        for toc in [false, true] {
            let req = request(
                dir.path(),
                layout_of(&[("preset", json!(preset)), ("front.toc", json!(toc))]),
                SAMPLE,
            );
            let new = layout::preamble(&req.layout, &req.meta);
            let h = &req.layout.headings;
            let deeper = format!("#show heading: set text(size: {})\n", h.level3_size);
            let level2 = format!(
                "#show heading.where(level: 2): set text(size: {})\n",
                h.level2_size
            );
            assert!(new.rules.contains(&deeper) && new.rules.contains(&level2));
            assert!(!new.rules.contains("set block("), "{preset}");
            assert_eq!(new.front.contains("#outline(depth: 3)\n"), toc, "{preset}");
            let old = Preamble {
                rules: new.rules.replace(&deeper, "").replace(
                    &level2,
                    &format!(
                        "{level2}#show heading.where(level: 3): set text(size: {})\n",
                        h.level3_size
                    ),
                ),
                front: new.front.clone(),
            };
            let now = compiler.compile(&req);
            assert!(errors(&now).is_empty(), "{:?}", errors(&now));
            let before = compiler.compile_with(&req, &old);
            assert!(errors(&before).is_empty(), "{:?}", errors(&before));
            assert_eq!(runs(&now), runs(&before), "{preset}, toc {toc}");
        }
    }
}

#[test]
fn deeper_headings_take_the_level_3_size() {
    let dir = project();
    let content = "= One\n\n== Two\n\n=== Three\n\n==== Four\n\n===== Five\n\nText.\n";
    let mut req = request(
        dir.path(),
        layout_of(&[("headings.level3_size", json!("8pt"))]),
        content,
    );
    let out = compile_ok(&req, "levels");
    let size = |out: &CompileOutput, needle| run(out, needle).2;
    assert!(approx(size(&out, "One"), 14.0) && approx(size(&out, "Two"), 12.0));
    for deeper in ["Three", "Four", "Five"] {
        assert!(approx(size(&out, deeper), 8.0), "{deeper}");
    }
    // Expert code still sizes a level on its own.
    req.custom = "#show heading.where(level: 4): set text(size: 7pt)".into();
    let out = compile_ok(&req, "expert");
    assert!(approx(size(&out, "Four"), 7.0));
    assert!(approx(size(&out, "Three"), 8.0) && approx(size(&out, "Five"), 8.0));
}

#[test]
fn heading_spacing() {
    let dir = project();
    let content = "First line.\n\n== Bravo\n\nSecond line.\n";
    let compile_with = |pairs: &[(&str, Value)]| {
        compile_ok(
            &request(dir.path(), layout_of(pairs), content),
            &format!("{pairs:?}"),
        )
    };
    // Space above and below the heading (baseline to baseline).
    let gaps = |out: &CompileOutput| {
        let y = |needle| run(out, needle).1.y.to_pt();
        (y("Bravo") - y("First"), y("Second") - y("Bravo"))
    };
    let default = compile_with(&[]);
    let (above, below) = gaps(&default);
    // Typst's own spacing for a level-2 heading, here 12pt: 1.44em / 1.2
    // above, 0.75em / 1.2 below.
    let typst = compile_with(&[
        ("headings.space_above", json!("14.4pt")),
        ("headings.space_below", json!("7.5pt")),
    ]);
    assert_eq!(runs(&typst), runs(&default));
    let more = compile_with(&[
        ("headings.space_above", json!("40pt")),
        ("headings.space_below", json!("2.5em")),
    ]);
    let (a, b) = gaps(&more);
    assert!(approx(a - above, 40.0 - 14.4), "{a} {above}");
    assert!(approx(b - below, 30.0 - 7.5), "{b} {below}");
    // Paragraph spacing does not reach headings.
    let spaced = compile_with(&[("text.paragraph_spacing", json!("3em"))]);
    let (a, b) = gaps(&spaced);
    assert!(approx(a, above) && approx(b, below), "{a} {b}");

    // A chapter opening a page has no space above it.
    let content = "= One\n\nText.\n\n= Two\n\nMore.\n";
    let chapters = |pairs: &[(&str, Value)]| {
        let mut all = vec![("headings.chapter_pagebreak", json!("page"))];
        all.extend_from_slice(pairs);
        let out = compile_ok(&request(dir.path(), layout_of(&all), content), "chapters");
        run(&out, "Two")
    };
    let (page, at, ..) = chapters(&[]);
    let (spaced_page, spaced_at, ..) = chapters(&[("headings.space_above", json!("50pt"))]);
    // After the title block's page and chapter One's.
    assert_eq!((page, spaced_page), (2, 2));
    assert!(approx(at.y.to_pt(), spaced_at.y.to_pt()));
}

#[test]
fn contents_settings() {
    let dir = project();
    let content = "= Introduction\n\n#lorem(30)\n\n== Background\n\n=== Details\n\n==== Finer\n\n= Results\n\n#lorem(30)\n";
    // A title page, then the contents on the second page.
    let toc = |pairs: &[(&str, Value)]| {
        let mut all = vec![
            ("front.toc", json!(true)),
            ("front.title_page", json!(true)),
        ];
        all.extend_from_slice(pairs);
        let out = compile_ok(&request(dir.path(), layout_of(&all), content), "toc");
        let page = texts(&out)[1].clone();
        (out, page)
    };
    let dots = |page: &str| page.matches('.').count();
    let (default, page) = toc(&[]);
    assert!(
        page.contains("Contents") && page.contains("Details"),
        "{page}"
    );
    assert!(!page.contains("Finer"), "{page}");
    assert!(dots(&page) > 50, "{page}");
    let bravo = run(&default, "Background");
    assert_eq!(bravo.0, 1);
    let intro = run(&default, "Introduction");
    assert_eq!(intro.3, 400);

    let (_, page) = toc(&[("front.toc_depth", json!(1))]);
    assert!(
        page.contains("Results") && !page.contains("Background"),
        "{page}"
    );
    let (_, page) = toc(&[("front.toc_depth", json!(4))]);
    assert!(page.contains("Finer"), "{page}");
    let (_, page) = toc(&[("front.toc_title", json!("Inhalt \"x\""))]);
    assert!(
        page.contains("Inhalt") && !page.contains("Contents"),
        "{page}"
    );
    let (_, page) = toc(&[("front.toc_dots", json!(false))]);
    assert!(dots(&page) < 20, "{page}");
    let (out, _) = toc(&[("front.toc_indent", json!(false))]);
    assert!(run(&out, "Background").1.x < bravo.1.x);
    let (out, _) = toc(&[("front.toc_bold_top", json!(true))]);
    assert_eq!(run(&out, "Introduction").3, 700);
    assert_eq!(run(&out, "Background").3, 400);
    // The default spacing is the line spacing.
    let (out, _) = toc(&[("front.toc_spacing", json!("0.6em"))]);
    assert_eq!(runs(&out), runs(&default));
    let (out, _) = toc(&[("front.toc_spacing", json!("2em"))]);
    let y = |out: &CompileOutput, needle| run(out, needle).1.y.to_pt();
    assert!(
        y(&out, "Background") - y(&out, "Introduction")
            > y(&default, "Background") - y(&default, "Introduction") + 10.0
    );
}

#[test]
fn hostile_settings_are_inert() {
    let dir = project();
    let hostile = "\"); #read(\"/etc/passwd\") // \\ ] [*x*] $x$ <l> @knuth84 #{";
    for key in ["title", "subtitle", "author", "date", "contact"] {
        assert_eq!(
            layout::validate_meta(key, &json!(hostile)),
            Ok(json!(hostile))
        );
    }
    let meta_entries: Entries = layout::META_KEYS
        .iter()
        .map(|k| ((*k).to_owned(), json!(hostile)))
        .collect();
    let meta = layout::resolve_meta(&meta_entries);
    // Hostile layout values are ignored by resolve.
    let mut entries: Entries = layout::flatten(&layout::preset("novel").unwrap())
        .keys()
        .map(|k| (k.clone(), json!("12pt\")\n#read(\"/etc/passwd\")")))
        .collect();
    entries.insert("preset".into(), json!("novel"));
    entries.insert("citations.bibliography_title".into(), json!(hostile));
    let layout = layout::resolve(&entries);
    let mut expected = layout::preset("novel").unwrap();
    expected.citations.bibliography_title = hostile.into();
    assert_eq!(layout, expected);

    let mut req = request(dir.path(), layout, SAMPLE);
    req.meta = meta;
    let out = compile_ok(&req, "hostile");
    let all = texts(&out).join("");
    let squashed: String = all.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(squashed.contains("#read(\"/etc/passwd\")"), "{squashed}");
}

/// Font family and size of the first text run containing `needle`.
fn font_of(out: &CompileOutput, needle: &str) -> (String, f64) {
    fn walk(frame: &Frame, needle: &str) -> Option<(String, f64)> {
        frame.items().find_map(|(_, item)| match item {
            FrameItem::Group(g) => walk(&g.frame, needle),
            FrameItem::Text(t) if t.text.contains(needle) => {
                Some((t.font.info().family.clone(), t.size.to_pt()))
            }
            _ => None,
        })
    }
    let doc = out.document.as_ref().unwrap().paged();
    doc.pages()
        .iter()
        .find_map(|p| walk(&p.frame, needle))
        .expect(needle)
}

#[test]
fn character_styles() {
    let dir = project();
    let layout = layout_of(&[
        ("styles.mono-14pt.font", json!("DejaVu Sans Mono")),
        ("styles.mono-14pt.size", json!("14pt")),
        ("styles.small.size", json!("8pt")),
        ("styles.small.name", json!("Small \"print\"")),
    ]);
    assert_eq!(layout.styles.len(), 2);
    let content = "Plain #styled(\"mono-14pt\")[Bravo] and #styled(\"small\")[Tiny], \
                   #styled(\"nope\")[Missing].\n";
    let out = compile_ok(&request(dir.path(), layout, content), "styles");
    let body = font_of(&out, "Plain");
    assert_eq!(body.0, "New Computer Modern");
    assert!(approx(body.1, 11.0), "{body:?}");
    let mono = font_of(&out, "Bravo");
    assert_eq!(mono.0, "DejaVu Sans Mono");
    assert!(approx(mono.1, 14.0), "{mono:?}");
    // A size-only style keeps the body font.
    let small = font_of(&out, "Tiny");
    assert_eq!(small.0, body.0);
    assert!(approx(small.1, 8.0), "{small:?}");
    // An unknown style: body text, no error.
    assert_eq!(font_of(&out, "Missing"), body);

    // Without styles, `styled` still exists.
    let out = compile_ok(&request(dir.path(), layout_of(&[]), content), "no styles");
    assert_eq!(font_of(&out, "Bravo"), font_of(&out, "Plain"));
}

/// What a table compiled to: its lines (`h`/`v`, y or x, thickness), the
/// y of its filled rows, and each text run's y and weight.
#[derive(Debug, Default)]
struct Drawn {
    lines: Vec<(char, f64, f64)>,
    fills: Vec<f64>,
    runs: Vec<(String, f64, u16)>,
}

impl Drawn {
    fn of(out: &CompileOutput) -> Self {
        fn walk(frame: &Frame, offset: Point, d: &mut Drawn) {
            for (pos, item) in frame.items() {
                let at = offset + *pos;
                match item {
                    FrameItem::Group(g) => walk(&g.frame, at, d),
                    FrameItem::Text(t) => d.runs.push((
                        t.text.to_string(),
                        at.y.to_pt(),
                        t.font.info().variant.weight.to_number(),
                    )),
                    FrameItem::Shape(shape, _) => match shape.geometry {
                        Geometry::Line(to) => {
                            let w = shape.stroke.as_ref().map_or(0.0, |s| s.thickness.to_pt());
                            let (dir, at) = if to.y.to_pt().abs() < 0.01 {
                                ('h', at.y.to_pt())
                            } else {
                                ('v', at.x.to_pt())
                            };
                            d.lines.push((dir, (at * 10.0).round() / 10.0, w));
                        }
                        Geometry::Rect(_) if shape.fill.is_some() => d.fills.push(at.y.to_pt()),
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
        let mut d = Drawn::default();
        // The last page (after a title page).
        let page = out
            .document
            .as_ref()
            .unwrap()
            .paged()
            .pages()
            .last()
            .unwrap();
        walk(&page.frame, Point::zero(), &mut d);
        d
    }

    fn run(&self, text: &str) -> (f64, u16) {
        // Exactly, else in a caption ("Table 1: Tcap").
        self.runs
            .iter()
            .find(|r| r.0 == text)
            .or_else(|| {
                self.runs
                    .iter()
                    .find(|r| r.0.ends_with(&format!(" {text}")))
            })
            .map(|r| (r.1, r.2))
            .expect(text)
    }

    /// The thicknesses of the horizontal lines, top to bottom (one per y).
    fn rules(&self) -> Vec<f64> {
        let mut h: Vec<(f64, f64)> = self
            .lines
            .iter()
            .filter(|l| l.0 == 'h')
            .map(|l| (l.1, (l.2 * 100.0).round() / 100.0))
            .collect();
        h.sort_by(|a, b| a.partial_cmp(b).unwrap());
        h.dedup_by(|a, b| (a.0 - b.0).abs() < 1.0);
        h.into_iter().map(|l| l.1).collect()
    }
}

/// A table figure with a header row and three body rows.
const HEADED: &str = "#figure(table(columns: 2, table.header([Hd], [He]), [aa], [ba], [ab], [bb], [ac], [bc]), caption: [Tcap]) <tab:1>\n";
/// A table without a header, outside a figure.
const BARE: &str = "#table(columns: 2, [Na], [Nb], [na], [nb])\n";

#[test]
fn table_styles() {
    let dir = project();
    let draw = |pairs: &[(&str, Value)], content: &str| {
        let what = format!("{pairs:?} {content}");
        Drawn::of(&compile_ok(
            &request(dir.path(), layout_of(pairs), content),
            &what,
        ))
    };
    // 11pt text: the academic rules are 0.08em and 0.05em.
    let (thick, thin) = (0.88, 0.55);

    // Paper: academic, bold header, caption above the table.
    let d = draw(&[], HEADED);
    assert_eq!(d.rules(), [thick, thin, thick], "{d:?}");
    assert!(
        d.lines.iter().all(|l| l.0 == 'h') && d.fills.is_empty(),
        "{d:?}"
    );
    let (head, weight) = d.run("Hd");
    assert_eq!((weight, d.run("He").1, d.run("aa").1), (700, 700, 400));
    assert!(d.run("Tcap").0 < head);
    // The thin rule is between the header and the first row.
    let mid = d
        .lines
        .iter()
        .find(|l| (l.2 - thin).abs() < 0.01)
        .unwrap()
        .1;
    assert!(head < mid && mid < d.run("aa").0, "{d:?}");
    // No header: no thin rule, nothing bold.
    let d = draw(&[], BARE);
    assert_eq!(d.rules(), [thick, thick], "{d:?}");
    assert_eq!(d.run("Na").1, 400);

    // Plain: no lines; header not bold when switched off; caption below.
    let d = draw(
        &[
            ("tables.style", json!("plain")),
            ("tables.header_bold", json!(false)),
            ("tables.caption_position", json!("bottom")),
        ],
        HEADED,
    );
    assert!(d.lines.is_empty() && d.fills.is_empty(), "{d:?}");
    assert_eq!(d.run("Hd").1, 400);
    assert!(d.run("Tcap").0 > d.run("bc").0);
    // Other figures keep their own caption setting.
    let figure = "#figure(rect(width: 1cm, fill: red), caption: [Rcap])\n";
    let d = Drawn::of(&compile_ok(
        &request(dir.path(), layout_of(&[]), figure),
        "figure",
    ));
    assert!(d.run("Rcap").0 > d.fills[0], "{d:?}");

    // Grid: thin lines around every cell, vertical ones too.
    let d = draw(&[("tables.style", json!("grid"))], HEADED);
    assert_eq!(d.rules(), [0.5; 5], "{d:?}");
    assert!(d.lines.iter().any(|l| l.0 == 'v'));
    assert!(d.lines.iter().all(|l| (l.2 - 0.5).abs() < 0.01), "{d:?}");

    // Striped: no lines, the second and fourth rows (never the header) filled.
    let d = draw(&[("tables.style", json!("striped"))], HEADED);
    assert!(d.lines.is_empty(), "{d:?}");
    let rows: Vec<f64> = ["Hd", "aa", "ab", "ac"]
        .iter()
        .map(|t| d.run(t).0)
        .collect();
    let filled = |y0: f64, y1: f64| d.fills.iter().any(|&f| f > y0 && f < y1);
    assert!(!filled(0.0, rows[0]), "{d:?}");
    assert!(
        filled(rows[0], rows[1]) && !filled(rows[1], rows[2]) && filled(rows[2], rows[3]),
        "{d:?} {rows:?}"
    );

    // Novel: plain, caption below, header bold.
    let d = draw(&[("preset", json!("novel"))], HEADED);
    assert!(d.lines.is_empty());
    assert_eq!(d.run("Hd").1, 700);
    assert!(d.run("Tcap").0 > d.run("bc").0);
}

#[test]
fn content_diagnostics_map_to_content_offsets() {
    let dir = project();
    let content = "= Über 🌍\n\nText with a #nope(1) call.\n";
    let out = compile(&request(dir.path(), layout_of(&[]), content));
    assert!(out.document.is_none());
    let d = out
        .diagnostics
        .iter()
        .find(|d| d.severity == Severity::Error)
        .unwrap();
    let Location::Content {
        start,
        end,
        start_utf16,
        end_utf16,
    } = d.location
    else {
        panic!("{d:?}");
    };
    assert_eq!(&content[start..end], "nope", "{d:?}");
    let utf16: Vec<u16> = content.encode_utf16().collect();
    assert_eq!(
        String::from_utf16(&utf16[start_utf16..end_utf16]).unwrap(),
        "nope"
    );

    // A syntax error at the very end of the content.
    let content = "Unclosed #emph[strong";
    let out = compile(&request(dir.path(), layout_of(&[]), content));
    assert!(
        out.diagnostics
            .iter()
            .any(|d| matches!(d.location, Location::Content { .. })),
        "{:?}",
        out.diagnostics
    );
}

#[test]
fn custom_code_errors_are_reported_as_custom() {
    let dir = project();
    // A syntax error: the expert code is left out, the document still compiles.
    let mut req = request(dir.path(), layout_of(&[]), SAMPLE);
    req.custom = "#set text(size: 11pt)\n#let x = (1, 2".into();
    let out = compile(&req);
    assert!(out.document.is_some());
    assert!(
        out.diagnostics
            .iter()
            .all(|d| matches!(d.location, Location::Custom { .. })),
        "{:?}",
        out.diagnostics
    );
    assert!(!out.diagnostics.is_empty());

    // A runtime error inside the expert code.
    req.custom = "#set page(fill: luma(250))\n#set text(nonsense: 1)\n".into();
    let out = compile(&req);
    let d = out
        .diagnostics
        .iter()
        .find(|d| d.severity == Severity::Error)
        .unwrap();
    let Location::Custom { start, end } = d.location else {
        panic!("{d:?}");
    };
    assert!(req.custom[start..end].contains("nonsense"), "{d:?}");

    // Working expert code applies after the generated rules.
    req.custom = "#set page(numbering: \"i\")\n#set heading(numbering: \"A.1\")".into();
    compile_ok(&req, "custom");
}

#[test]
fn file_access_is_confined() {
    let dir = project();
    fs::write(dir.path().join("snapshot.loro"), b"secret").unwrap();
    // A file in `assets/` that the index doesn't name is not served.
    fs::write(dir.path().join("assets/loose.svg"), SVG).unwrap();
    for (content, needle) in [
        ("#read(\"snapshot.loro\")", "only references.bib"),
        ("#read(\"/snapshot.loro\")", "only references.bib"),
        ("#image(\"secret.svg\")", "only references.bib"),
        ("#image(\"assets/../secret.svg\")", "only references.bib"),
        ("#image(\"assets/missing.svg\")", "not found"),
        ("#image(\"assets/loose.svg\")", "not found"),
        ("#include \"assets/x.typ\"", "only references.bib"),
        (
            "#import \"@preview/cetz:0.3.0\": *",
            "packages are not available",
        ),
    ] {
        let out = compile(&request(dir.path(), layout_of(&[]), content));
        assert!(out.document.is_none(), "{content}");
        assert!(
            out.diagnostics.iter().any(|d| d.message.contains(needle)),
            "{content}: {:?}",
            out.diagnostics
        );
    }
    let out = compile_ok(
        &request(
            dir.path(),
            layout_of(&[]),
            "#read(\"references.bib\").len() #datetime.today().display()",
        ),
        "allowed",
    );
    assert!(texts(&out)[0].contains("2026-10-12"));

    // Index entries must name a blob inside `assets/`.
    for blob in ["../secret.svg", "loose.svg", &PLOT_BLOB.to_uppercase()] {
        let mut req = request(dir.path(), layout_of(&[]), "#image(\"assets/plot.svg\")");
        req.assets.insert("assets/plot.svg".into(), blob.into());
        let out = compile(&req);
        assert!(out.document.is_none(), "{blob}");
        assert!(
            out.diagnostics
                .iter()
                .any(|d| d.message.contains("Not an image blob")),
            "{blob}: {:?}",
            out.diagnostics
        );
    }
}

#[test]
fn render_and_export() {
    let dir = project();
    let mut compiler = DocCompiler::new();
    let req = request(dir.path(), layout_of(&[("preset", json!("paper"))]), SAMPLE);
    let out = compiler.compile(&req);
    assert!(errors(&out).is_empty(), "{:?}", errors(&out));
    let doc = out.document.as_ref().unwrap();
    let svg = lethora_typeset::render_page_svg(doc, 0).unwrap();
    assert!(svg.starts_with("<svg"));
    assert!(lethora_typeset::render_page_svg(doc, 999).is_none());
    let pdf = lethora_typeset::export_pdf(doc).unwrap();
    assert!(pdf.starts_with(b"%PDF"));

    // Recompiling after an edit reuses the compiler's source.
    let mut req = req;
    req.content.push_str("\nOne more line.");
    let again = compiler.compile(&req);
    assert!(errors(&again).is_empty());
    assert!(texts(&again).last().unwrap().contains("Knuth"));
}

#[test]
fn changed_files_are_read_again() {
    // The same text compiled again: a replaced image (a new blob in the
    // index) and a changed library must show, though the compiler keeps its
    // caches (Typst checks every file it read again).
    let dir = project();
    let mut compiler = DocCompiler::new();
    let mut req = request(
        dir.path(),
        layout_of(&[]),
        "See @doe2020. #image(\"assets/plot.svg\") Size #read(\"assets/plot.svg\").len() bytes.",
    );
    let compile_text = |compiler: &mut DocCompiler, req: &CompileRequest| {
        let out = compiler.compile(req);
        assert!(errors(&out).is_empty(), "{:?}", errors(&out));
        texts(&out).join(" ")
    };
    let before = compile_text(&mut compiler, &req);
    req.bibliography = BIB.replace("On Layout", "On Paging");
    let replaced = "f".repeat(64);
    fs::write(
        dir.path().join("assets").join(&replaced),
        SVG.replace("green", "darkgreen"),
    )
    .unwrap();
    req.assets.insert("assets/plot.svg".into(), replaced);
    let after = compile_text(&mut compiler, &req);
    assert!(before.contains("On Layout"), "{before}");
    assert!(
        after.contains("On Paging") && !after.contains("On Layout"),
        "{after}"
    );
    let size = |n: usize| format!("Size {n} bytes");
    assert!(before.contains(&size(SVG.len())), "{before}");
    assert!(after.contains(&size(SVG.len() + 4)), "{after}");
}

/// Compile time of a ~100-page document. Run with
/// `cargo test --release -p lethora-typeset --test document -- --ignored --nocapture`.
#[test]
#[ignore]
fn hundred_pages() {
    let dir = project();
    let mut content = String::new();
    for chapter in 1..=32 {
        content.push_str(&format!("= Chapter {chapter}\n\n"));
        for section in 1..=3 {
            content.push_str(&format!("== Section {chapter}.{section}\n\n"));
            for p in 0..6 {
                content.push_str(&format!(
                    "Paragraph {p} with $x_{p}^2 + y = {chapter}$ and a citation @knuth84. {}\n\n",
                    "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod \
                     tempor incididunt ut labore et dolore magna aliqua. "
                        .repeat(3)
                ));
            }
            content.push_str(&format!(
                "$ sum_(k=1)^n k = (n(n+1))/2 $ <eq:{chapter}-{section}>\n\n"
            ));
            content.push_str(&format!(
                "#figure(image(\"assets/plot.svg\", width: 40%), caption: [Plot {chapter}.{section}.])\n\n"
            ));
        }
    }
    for preset in ["paper", "novel", "thesis"] {
        let mut compiler = DocCompiler::new();
        let mut req = request(
            dir.path(),
            layout_of(&[("preset", json!(preset))]),
            &content,
        );
        let start = Instant::now();
        let out = compiler.compile(&req);
        let cold = start.elapsed();
        assert!(errors(&out).is_empty(), "{:?}", errors(&out));

        let mid = req.content.len() / 2;
        let at = req.content[..mid].rfind("\n\n").unwrap();
        req.content.insert_str(at, "\n\nAn inserted sentence.");
        let start = Instant::now();
        let warm = compiler.compile(&req);
        let edit = start.elapsed();
        assert!(errors(&warm).is_empty());

        // A layout change: everything is laid out again.
        let mut restyled = req.clone();
        restyled.layout.text.size = layout::Length::parse("10.5pt").unwrap();
        let start = Instant::now();
        let out2 = compiler.compile(&restyled);
        let restyle = start.elapsed();
        assert!(errors(&out2).is_empty());

        let doc = warm.document.as_ref().unwrap();
        let start = Instant::now();
        for i in 0..doc.pages() {
            doc.render_page_svg(i).unwrap();
        }
        let svg = start.elapsed();
        let start = Instant::now();
        let pdf = doc.export_pdf().unwrap();
        let pdf_time = start.elapsed();
        println!(
            "{preset}: {} chars, {} pages; cold compile {cold:?}; recompile after an edit \
             {edit:?}; after a font size change {restyle:?}; all pages to SVG {svg:?}; \
             PDF {pdf_time:?} ({} KB)",
            req.content.len(),
            out.pages,
            pdf.len() / 1024
        );
    }
}
