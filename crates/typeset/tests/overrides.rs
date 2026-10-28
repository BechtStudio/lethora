//! Anchored overrides injected at compile time, their source mapping, and
//! click-to-source.

use std::path::Path;

use lethora_typeset::layout::TableStyle;
use lethora_typeset::layout::{self, Entries, Length, Meta};
use lethora_typeset::overrides::{Kind, Placement, ResolvedOverride};
use lethora_typeset::{
    CompileOutput, CompileRequest, JumpTarget, Location, Severity, compile, compile_source,
    overrides,
};
use serde_json::{Value, json};
use typst::layout::{Frame, FrameItem, Point};
use typst::visualize::Geometry;

fn request(dir: &Path, pairs: &[(&str, Value)], content: &str) -> CompileRequest {
    let mut entries: Entries = [("preset".to_owned(), json!("paper"))].into();
    for (k, v) in pairs {
        entries.insert((*k).to_owned(), v.clone());
    }
    CompileRequest {
        project_dir: dir.to_path_buf(),
        layout: layout::resolve(&entries),
        meta: Meta::default(),
        custom: String::new(),
        content: content.into(),
        today: None,
        overrides: Vec::new(),
        bibliography: String::new(),
        assets: Default::default(),
    }
}

fn at(id: &str, kind: Kind, content: &str, needle: &str) -> ResolvedOverride {
    ResolvedOverride {
        id: id.into(),
        kind,
        content_byte_offset: content.find(needle).expect(needle),
        detached: false,
    }
}

fn errors(out: &CompileOutput) -> Vec<String> {
    out.diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| format!("{} ({:?})", d.message, d.location))
        .collect()
}

fn compile_ok(req: &CompileRequest) -> CompileOutput {
    let out = compile(req);
    assert!(errors(&out).is_empty(), "{:?}", errors(&out));
    assert!(out.document.is_some());
    out
}

/// Position (baseline start, absolute) and font size of the first text run
/// containing `needle`, and its page.
fn find(out: &CompileOutput, needle: &str) -> Option<(usize, Point, f64)> {
    (0..out.pages).find_map(|i| find_on(out, i, needle).map(|(at, s)| (i, at, s)))
}

/// Like [`find`], on page `page`.
fn find_on(out: &CompileOutput, page: usize, needle: &str) -> Option<(Point, f64)> {
    fn walk(frame: &Frame, offset: Point, needle: &str) -> Option<(Point, f64)> {
        for (pos, item) in frame.items() {
            let at = offset + *pos;
            match item {
                FrameItem::Group(g) => {
                    if let Some(found) = walk(&g.frame, at, needle) {
                        return Some(found);
                    }
                }
                FrameItem::Text(t) if t.text.contains(needle) => {
                    return Some((at, t.size.to_pt()));
                }
                _ => {}
            }
        }
        None
    }
    let doc = out.document.as_ref()?.paged();
    walk(&doc.pages().get(page)?.frame, Point::zero(), needle)
}

fn page_of(out: &CompileOutput, needle: &str) -> usize {
    find(out, needle).expect(needle).0
}

fn utf16(s: &str, byte: usize) -> usize {
    s[..byte].encode_utf16().count()
}

const BREAKS: &str = "= Alpha\n\nFirst words. #lorem(40)\n\n== Bravo\n\nSecond words. #lorem(40)\n";

#[test]
fn page_breaks_start_the_anchored_block_on_a_new_page() {
    let dir = tempfile::tempdir().unwrap();
    let mut req = request(dir.path(), &[], BREAKS);
    let plain = compile_ok(&req);
    assert_eq!(page_of(&plain, "Bravo"), 0);

    // Anchored anywhere in the heading's line.
    req.overrides = vec![at("p1", Kind::PageBreak { to_odd: false }, BREAKS, "Bravo")];
    let out = compile_ok(&req);
    assert_eq!(page_of(&out, "Bravo"), 1);
    assert_eq!(page_of(&out, "First"), 0);
    assert!(out.overrides[0].applied && !out.overrides[0].detached);
    assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
    // The heading opens the page: nothing of the previous text on it.
    let text = out.document.as_ref().unwrap().page_text(1).unwrap();
    assert!(!text.contains("First"), "{text}");

    // To an odd page: page 3, with an empty page 2 in between.
    req.overrides = vec![at(
        "p1",
        Kind::PageBreak { to_odd: true },
        BREAKS,
        "== Bravo",
    )];
    let out = compile_ok(&req);
    assert_eq!(page_of(&out, "Bravo"), 2);
    assert_eq!(out.pages, 3);
    // Weak: a page break where a page starts anyway adds nothing.
    req.overrides = vec![at(
        "p1",
        Kind::PageBreak { to_odd: false },
        BREAKS,
        "= Alpha",
    )];
    assert_eq!(compile_ok(&req).pages, plain.pages);
}

#[test]
fn column_break_and_space_before() {
    let dir = tempfile::tempdir().unwrap();
    let content = "First words. #lorem(30)\n\nZebra words. #lorem(30)\n";
    let mut req = request(dir.path(), &[("page.columns", json!(2))], content);
    let plain = compile_ok(&req);
    let (_, zebra, _) = find(&plain, "Zebra").unwrap();
    let width = plain.page_sizes[0][0];
    assert!(zebra.x.to_pt() < width / 2.0);

    req.overrides = vec![at("c1", Kind::ColumnBreak, content, "Zebra")];
    let out = compile_ok(&req);
    let (page, moved, _) = find(&out, "Zebra").unwrap();
    assert_eq!(page, 0);
    assert!(moved.x.to_pt() > width / 2.0, "{moved:?}");

    let mut req = request(dir.path(), &[], content);
    let (_, before, _) = find(&compile_ok(&req), "Zebra").unwrap();
    req.overrides = vec![at(
        "s1",
        Kind::SpaceBefore {
            length: Length::parse("36pt").unwrap(),
        },
        content,
        "Zebra",
    )];
    let (_, after, _) = find(&compile_ok(&req), "Zebra").unwrap();
    let shift = (after.y - before.y).to_pt();
    assert!(shift > 30.0 && shift < 42.0, "{shift}");
}

const FIGURES: &str = "#lorem(120)\n\n#figure(rect(width: 3cm, height: 2cm, fill: blue), caption: [Boxcap])\n\n#lorem(120)\n";

#[test]
fn figure_placement_moves_the_figure() {
    let dir = tempfile::tempdir().unwrap();
    let mut req = request(dir.path(), &[], FIGURES);
    let y = |req: &CompileRequest| find(&compile_ok(req), "Boxcap").unwrap().1.y.to_pt();
    let here = y(&req);
    let place = |p| Kind::FigurePlacement { placement: p };
    req.overrides = vec![at("f1", place(Placement::Top), FIGURES, "Boxcap")];
    let top = y(&req);
    req.overrides = vec![at("f1", place(Placement::Bottom), FIGURES, "#figure")];
    let bottom = y(&req);
    assert!(top < here && here < bottom, "{top} {here} {bottom}");

    // An existing `placement:` argument is replaced, not duplicated.
    let content = FIGURES.replace("#figure(", "#figure(placement: none, ");
    let mut req = request(dir.path(), &[], &content);
    assert!((y(&req) - here).abs() < 1.0);
    req.overrides = vec![at("f1", place(Placement::Top), &content, "Boxcap")];
    assert!((y(&req) - top).abs() < 1.0);
    // Back to "here" (Typst's none) keeps it in the flow.
    req.overrides = vec![at("f1", place(Placement::Here), &content, "Boxcap")];
    assert!((y(&req) - here).abs() < 1.0);
}

#[test]
fn overrides_in_raw_math_comments_and_code_are_not_applied() {
    let dir = tempfile::tempdir().unwrap();
    let content = "Text.\n\n```\nraw line\n```\n\n$ x\n+ y $\n\n/* a\ncomment */\n\n#{\n\"code\"\n}\n\nNo figure here.\n";
    let mut req = request(dir.path(), &[], content);
    let pb = Kind::PageBreak { to_odd: false };
    req.overrides = vec![
        at("r", pb, content, "raw line"),
        at("m", Kind::ColumnBreak, content, "+ y"),
        at("c", pb, content, "comment */"),
        at("k", pb, content, "\"code\""),
        at(
            "f",
            Kind::FigurePlacement {
                placement: Placement::Top,
            },
            content,
            "No figure",
        ),
    ];
    let out = compile_ok(&req);
    assert_eq!(out.pages, 1);
    assert_eq!(out.overrides.len(), 5);
    assert!(
        out.overrides.iter().all(|s| !s.applied),
        "{:?}",
        out.overrides
    );
    for id in ["r", "m", "c", "k", "f"] {
        assert!(
            out.diagnostics
                .iter()
                .any(|d| d.severity == Severity::Warning
                    && d.location == Location::Override { id: id.into() }),
            "{id}: {:?}",
            out.diagnostics
        );
    }
}

#[test]
fn detached_overrides_apply_and_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let mut req = request(dir.path(), &[], BREAKS);
    let mut o = at("d1", Kind::PageBreak { to_odd: false }, BREAKS, "Bravo");
    o.detached = true;
    req.overrides = vec![o];
    let out = compile_ok(&req);
    assert_eq!(page_of(&out, "Bravo"), 1);
    assert!(out.overrides[0].applied && out.overrides[0].detached);
    assert!(
        out.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Warning
                && d.location == Location::Override { id: "d1".into() })
    );
}

#[test]
fn diagnostics_after_injections_map_to_the_content() {
    let dir = tempfile::tempdir().unwrap();
    let content =
        "= Über 🌍\n\nOne.\n\n= Zwei\n\nTwo 中.\n\n= Drei\n\nText with a #nope(1) call.\n";
    let mut req = request(dir.path(), &[], content);
    let pb = Kind::PageBreak { to_odd: false };
    let space = Kind::SpaceBefore {
        length: Length::parse("1em").unwrap(),
    };
    req.overrides = vec![
        at("a", pb, content, "= Zwei"),
        at("b", space, content, "= Zwei"),
        at("c", Kind::ColumnBreak, content, "Two"),
        at("d", pb, content, "= Drei"),
    ];
    let out = compile(&req);
    assert!(out.overrides.iter().all(|s| s.applied));
    let d = out
        .diagnostics
        .iter()
        .find(|d| d.severity == Severity::Error)
        .expect("an error");
    let Location::Content {
        start,
        end,
        start_utf16,
        end_utf16,
    } = d.location
    else {
        panic!("{d:?}");
    };
    assert_eq!(&content[start..end], "nope");
    let nope = content.find("nope").unwrap();
    assert_eq!(
        (start_utf16, end_utf16),
        (utf16(content, nope), utf16(content, nope) + 4)
    );
}

#[test]
fn source_map_is_piecewise_exact() {
    let preamble = layout::preamble(&layout::resolve(&Entries::new()), &Meta::default());
    let content = "aé\nb🌍\nc\n";
    let edits = [
        overrides::Edit {
            range: 4..4,
            text: "#v(nope)\n".into(),
            id: "x".into(),
        },
        overrides::Edit {
            range: 9..10,
            text: "ZZ".into(),
            id: "y".into(),
        },
    ];
    let (text, map) = compile_source(&preamble, "", content, &edits, None);
    let body = &text[map.content.clone()];
    assert_eq!(body, "aé\n#v(nope)\nb🌍ZZc\n");
    // Every content byte maps back to itself (and there again); injected
    // bytes to their edit.
    for (i, _) in body.char_indices() {
        let pos = map.content.start + i;
        match map.content_offset(pos) {
            Ok(c) => {
                assert_eq!(content[c..].chars().next(), body[i..].chars().next());
                assert_eq!(map.source_offset(c), pos);
            }
            Err(e) => assert!(e.id == "x" || e.id == "y"),
        }
    }
    let nope = text.find("nope").unwrap();
    assert_eq!(
        map.locate(nope..nope + 4),
        Location::Override { id: "x".into() }
    );
    let c = map.content.start + body.find('c').unwrap();
    assert!(matches!(
        map.locate(c..c + 1),
        Location::Content {
            start: 10,
            end: 11,
            ..
        }
    ));
}

#[test]
fn clicks_lead_to_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let content = format!("= Introduction\n\nSome words.\n\n{FIGURES}");
    let mut req = request(dir.path(), &[], &content);
    req.overrides = vec![at(
        "s",
        Kind::SpaceBefore {
            length: Length::parse("12pt").unwrap(),
        },
        &content,
        "Some words",
    )];
    let out = compile_ok(&req);
    let doc = out.document.as_ref().unwrap();

    let (page, p, size) = find(&out, "Introduction").unwrap();
    let jump = doc.jump(page, p.x.to_pt() + 2.0, p.y.to_pt() - size * 0.3);
    let start = utf16(&content, content.find("Introduction").unwrap());
    assert_eq!(
        jump.target,
        JumpTarget::Content {
            pos_utf16: start,
            start_utf16: start,
            end_utf16: start + "Introduction".len(),
            block_start_utf16: 0,
        }
    );
    assert_eq!(jump.figure_utf16, None);

    // Text after an injection maps to its own line.
    let (page, p, size) = find(&out, "Some").unwrap();
    let jump = doc.jump(page, p.x.to_pt() + 1.0, p.y.to_pt() - size * 0.3);
    let JumpTarget::Content {
        block_start_utf16, ..
    } = jump.target
    else {
        panic!("{jump:?}");
    };
    assert_eq!(
        block_start_utf16,
        utf16(&content, content.find("Some").unwrap())
    );

    // Into the figure: its rectangle and its caption.
    let figure = utf16(&content, content.find("figure(").unwrap());
    let (page, cap, size) = find(&out, "Boxcap").unwrap();
    let jump = doc.jump(page, cap.x.to_pt() + 2.0, cap.y.to_pt() - size * 0.3);
    assert_eq!(jump.figure_utf16, Some(figure), "{jump:?}");
    // The rectangle (2cm high) sits above the caption.
    let jump = doc.jump(page, out.page_sizes[page][0] / 2.0, cap.y.to_pt() - 40.0);
    assert_eq!(jump.figure_utf16, Some(figure), "{jump:?}");
    assert!(matches!(jump.target, JumpTarget::Content { .. }));

    // Margins, and pages that don't exist.
    assert_eq!(doc.jump(0, 3.0, 3.0).target, JumpTarget::None);
    assert_eq!(doc.jump(99, 100.0, 100.0).target, JumpTarget::None);
    assert_eq!(doc.jump(0, f64::NAN, 1.0).target, JumpTarget::None);
}

/// The content offset (UTF-16) a click leads to.
fn pos(target: &JumpTarget) -> usize {
    match target {
        JumpTarget::Content { pos_utf16, .. } => *pos_utf16,
        _ => panic!("{target:?}"),
    }
}

const PARAS: &str =
    "Alpha words here. #lorem(50) Endone.\n\nStarttwo words. #lorem(30)\n\nShort last line.\n";

#[test]
fn clicks_off_the_text_snap_to_the_nearest_line() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        &[("text.paragraph_spacing", json!("2em"))],
        PARAS,
    );
    let out = compile_ok(&req);
    let doc = out.document.as_ref().unwrap();
    let byte = |s: &str| utf16(PARAS, PARAS.find(s).unwrap());

    // A direct hit, as before.
    let (page, alpha, size) = find(&out, "Alpha").unwrap();
    let y = alpha.y.to_pt() - size / 2.0;
    let jump = doc.jump(page, alpha.x.to_pt() + 1.0, y);
    assert_eq!(pos(&jump.target), byte("Alpha"));
    assert_eq!(jump.figure_utf16, None);
    // Left margin: the line's start; right margin: the end of a short line.
    assert_eq!(pos(&doc.jump(page, 5.0, y).target), byte("Alpha"));
    let (_, short, size) = find(&out, "Short").unwrap();
    let width = out.page_sizes[page][0];
    let jump = doc.jump(page, width - 5.0, short.y.to_pt() - size / 2.0);
    assert_eq!(pos(&jump.target), byte("Short") + "Short last line.".len());

    // Between paragraphs: the nearer line.
    let (_, one, _) = find(&out, "Endone").unwrap();
    let (_, two, _) = find(&out, "Starttwo").unwrap();
    let gap = (two.y - one.y).to_pt();
    assert!(gap > 25.0, "{gap}");
    let first = pos(&doc
        .jump(page, width / 2.0, one.y.to_pt() + gap * 0.3)
        .target);
    assert!(
        (byte("Alpha")..byte("Starttwo")).contains(&first),
        "{first}"
    );
    let second = doc
        .jump(page, width / 2.0, two.y.to_pt() - gap * 0.3)
        .target;
    assert!(
        (byte("Starttwo")..byte("Short")).contains(&pos(&second)),
        "{second:?}"
    );

    // Far from any text: nothing.
    let below = short.y.to_pt() + 60.0;
    assert_eq!(doc.jump(page, width / 2.0, below).target, JumpTarget::None);
}

#[test]
fn clicks_near_page_furniture_do_not_snap_to_the_body() {
    let dir = tempfile::tempdir().unwrap();
    let mut req = request(
        dir.path(),
        &[
            ("furniture.running_head", json!("title")),
            ("page.margin_top", json!("1.2cm")),
            ("page.margin_bottom", json!("1.2cm")),
        ],
        "Body words. #lorem(1500)
",
    );
    req.meta.title = "Headtitle".into();
    let out = compile_ok(&req);
    let doc = out.document.as_ref().unwrap();
    // The second page: the first opens with the title block. The body text
    // has no digits.
    let page = 1;
    assert!(out.pages > 2);
    let (head, size) = find_on(&out, page, "Headtitle").unwrap();
    let (number, number_size) = find_on(&out, page, "2").unwrap();

    // Just below the running head, with the body's first line in reach.
    let x = head.x.to_pt() + 2.0;
    let below = head.y.to_pt() + size * 0.5;
    let body = doc.jump(page, x, below + size).target;
    assert!(matches!(body, JumpTarget::Content { .. }), "{body:?}");
    assert_eq!(doc.jump(page, x, below).target, JumpTarget::None);

    // Just above the page number, with the body's last line in reach.
    let x = number.x.to_pt() + 1.0;
    let above = number.y.to_pt() - number_size * 1.1;
    let body = doc.jump(page, x, above - number_size).target;
    assert!(matches!(body, JumpTarget::Content { .. }), "{body:?}");
    assert_eq!(doc.jump(page, x, above).target, JumpTarget::None);
}

#[test]
fn block_overrides_apply_at_the_paragraph_start() {
    let dir = tempfile::tempdir().unwrap();
    // One Typst paragraph over three source lines (`*bravo*` is a run of its own).
    let content =
        "Intro words.\n\nAlpha starts here\n*bravo* goes on\ncharlie ends it.\n\nAfter.\n";
    let alpha = content.find("Alpha").unwrap();
    let mut req = request(dir.path(), &[], content);
    let plain = compile_ok(&req);
    let line_y = |out: &CompileOutput, needle| find(out, needle).expect(needle).1.y;
    assert_eq!(line_y(&plain, "Alpha"), line_y(&plain, "charlie"));

    // Anchored on the third line: inserted before the first.
    req.overrides = vec![at(
        "p1",
        Kind::PageBreak { to_odd: false },
        content,
        "charlie",
    )];
    let plan = overrides::plan(content, &req.overrides, req.layout.tables.style);
    assert_eq!(plan.edits.len(), 1);
    assert_eq!(plan.edits[0].range, alpha..alpha);
    let out = compile_ok(&req);
    assert!(out.overrides[0].applied);
    assert_eq!(page_of(&out, "Intro"), 0);
    assert_eq!(page_of(&out, "Alpha"), 1);
    // Not split: still one line.
    assert_eq!(page_of(&out, "charlie"), 1);
    assert_eq!(line_y(&out, "Alpha"), line_y(&out, "charlie"));

    // A click on the second source line leads to the paragraph start.
    let doc = out.document.as_ref().unwrap();
    let (page, p, size) = find(&out, "bravo").unwrap();
    let jump = doc.jump(page, p.x.to_pt() + 2.0, p.y.to_pt() - size * 0.3);
    let JumpTarget::Content {
        pos_utf16,
        block_start_utf16,
        ..
    } = jump.target
    else {
        panic!("{jump:?}");
    };
    assert!(pos_utf16 > utf16(content, content.find("*bravo*").unwrap()));
    assert_eq!(block_start_utf16, utf16(content, alpha));
}

#[test]
fn clicks_in_a_figure_box_lead_to_the_figure() {
    let dir = tempfile::tempdir().unwrap();
    let content = "#lorem(80)\n\n#figure(rect(width: 30%, height: 2cm, fill: blue), caption: [A caption clearly wider than the narrow box above it.])\n\n#lorem(80)\n";
    let figure = utf16(content, content.find("figure(").unwrap());
    let expected = JumpTarget::Content {
        pos_utf16: figure,
        start_utf16: figure,
        end_utf16: figure,
        block_start_utf16: utf16(content, content.find("#figure").unwrap()),
    };
    let place = |p| Kind::FigurePlacement { placement: p };
    // In the text, and floated to the top.
    for p in [Placement::Here, Placement::Top] {
        let mut req = request(dir.path(), &[], content);
        req.overrides = vec![at("f", place(p), content, "#figure")];
        let out = compile_ok(&req);
        let doc = out.document.as_ref().unwrap();
        let s = &out.overrides[0];
        let page = s.page.unwrap();
        let (x, y, w) = (s.x_pt.unwrap(), s.y_pt.unwrap(), s.width_pt.unwrap());
        let (cap, size) = find_on(&out, page, "caption clearly").unwrap();
        // Beside the narrow rectangle, and between it (2cm high) and the
        // caption.
        let beside = (x + 2.0, y + 2.0);
        let gap = (
            x + w / 2.0,
            (y + 2.0 * 72.0 / 2.54 + cap.y.to_pt() - size) / 2.0,
        );
        let mut docs = vec![doc];
        let plain;
        if p == Placement::Top {
            let (_, text, _) = find(&out, "Lorem").unwrap();
            assert!(y < text.y.to_pt(), "floated above the text: {s:?}");
        } else {
            // Any figure, not only the placed ones: the same without the
            // override.
            req.overrides.clear();
            plain = compile_ok(&req);
            docs.push(plain.document.as_ref().unwrap());
        }
        for doc in docs {
            for (x, y) in [beside, gap] {
                let jump = doc.jump(page, x, y);
                assert_eq!(jump.target, expected, "{p:?}");
                assert_eq!(jump.figure_utf16, Some(figure), "{p:?}");
            }
        }
    }
}

/// Every visible item (text runs, shapes, images) per page with its absolute
/// Every visible item (text runs, shapes, images) per page with its absolute
/// position, rounded to 0.01 pt.
fn items(out: &CompileOutput) -> Vec<Vec<String>> {
    fn walk(frame: &Frame, offset: Point, out: &mut Vec<String>) {
        for (pos, item) in frame.items() {
            let at = offset + *pos;
            let xy = format!("{:.2},{:.2}", at.x.to_pt(), at.y.to_pt());
            match item {
                FrameItem::Group(g) => walk(&g.frame, at, out),
                FrameItem::Text(t) => out.push(format!("text {xy} {}", t.text)),
                FrameItem::Shape(..) => out.push(format!("shape {xy}")),
                FrameItem::Image(_, size, _) => out.push(format!("image {xy} {size:?}")),
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

/// `content` with the Typst of `overrides` written into it, as injected
/// but without the markers (block code at the start of the line,
/// `placement:` as the figure's first argument).
fn written(content: &str, overrides: &[ResolvedOverride]) -> String {
    let mut inserts: Vec<(usize, String)> = overrides
        .iter()
        .map(|o| {
            let pos = o.content_byte_offset;
            match o.kind {
                Kind::FigurePlacement { placement } => {
                    let at = content[..pos].rfind("figure(").unwrap() + "figure(".len();
                    let p = match placement {
                        Placement::Top => "top",
                        Placement::Bottom => "bottom",
                        Placement::Here => "none",
                        Placement::Auto => "auto",
                    };
                    (at, format!("placement: {p}, "))
                }
                kind => {
                    let code = match kind {
                        Kind::PageBreak { to_odd: false } => "#pagebreak(weak: true)\n".into(),
                        Kind::PageBreak { to_odd: true } => {
                            "#pagebreak(weak: true, to: \"odd\")\n".into()
                        }
                        Kind::ColumnBreak => "#colbreak(weak: true)\n".into(),
                        Kind::SpaceBefore { length } => format!("#v({length})\n"),
                        Kind::FigurePlacement { .. }
                        | Kind::ImageQuality { .. }
                        | Kind::TableStyle { .. } => {
                            unreachable!()
                        }
                    };
                    (overrides::block_start(content, pos), code)
                }
            }
        })
        .collect();
    // From the end, keeping the order of insertions at the same place.
    inserts.reverse();
    inserts.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    let mut s = content.to_owned();
    for (at, code) in inserts {
        s.insert_str(at, &code);
    }
    s
}

const MARKED: &str = "First words. #lorem(60)\n\n= Heading two\n\n- An item\n- Bravo item\n\n#figure(rect(width: 3cm, height: 2cm, fill: blue), caption: [Boxcap])\n\nAfter the figure. #lorem(50)\n\n#pagebreak()\nFresh page words.\n\n#figure(rect(width: 2cm, height: 1cm), caption: [Labelled]) <fig:lab>\n\nSee @fig:lab. #lorem(80)\n\nLast words.\n";

#[test]
fn markers_do_not_change_the_layout() {
    let dir = tempfile::tempdir().unwrap();
    let pb = |to_odd| Kind::PageBreak { to_odd };
    let space = |l: &str| Kind::SpaceBefore {
        length: Length::parse(l).unwrap(),
    };
    let place = |placement| Kind::FigurePlacement { placement };
    let cases: Vec<(&str, Vec<(&str, Kind)>)> = vec![
        ("paper", vec![("First words", pb(false))]),
        ("paper", vec![("First words", space("12pt"))]),
        ("paper", vec![("= Heading two", pb(false))]),
        ("paper", vec![("= Heading two", space("2em"))]),
        ("paper", vec![("Bravo item", pb(true))]),
        ("paper", vec![("Bravo item", space("18pt"))]),
        ("paper", vec![("#figure(rect(width: 3cm", pb(false))]),
        ("paper", vec![("#figure(rect(width: 3cm", space("1.5em"))]),
        ("paper", vec![("Fresh page", pb(false))]),
        ("paper", vec![("Fresh page", pb(true))]),
        (
            "paper",
            vec![
                ("= Heading two", pb(false)),
                ("= Heading two", space("1em")),
                ("After the figure", space("30pt")),
            ],
        ),
        ("paper", vec![("Boxcap", place(Placement::Top))]),
        ("paper", vec![("Boxcap", place(Placement::Bottom))]),
        ("paper", vec![("Boxcap", place(Placement::Here))]),
        ("paper", vec![("Boxcap", place(Placement::Auto))]),
        ("paper", vec![("Labelled", place(Placement::Top))]),
        ("novel", vec![("= Heading two", pb(false))]),
        ("novel", vec![("= Heading two", pb(true))]),
        ("novel", vec![("After the figure", space("2em"))]),
    ];
    for (preset, list) in cases {
        let mut req = request(dir.path(), &[("preset", json!(preset))], MARKED);
        let overrides: Vec<_> = list
            .iter()
            .enumerate()
            .map(|(i, (needle, kind))| at(&format!("m{i}"), *kind, MARKED, needle))
            .collect();
        req.overrides = overrides.clone();
        let marked = compile_ok(&req);
        assert!(
            marked
                .overrides
                .iter()
                .all(|s| s.applied && s.page.is_some()),
            "{preset} {list:?}: {:?}",
            marked.overrides
        );
        let plain = compile_ok(&request(
            dir.path(),
            &[("preset", json!(preset))],
            &written(MARKED, &overrides),
        ));
        assert_eq!(marked.pages, plain.pages, "{preset} {list:?}");
        assert_eq!(items(&marked), items(&plain), "{preset} {list:?}");
    }

    // Two columns: a column break.
    let content = "First words. #lorem(30)\n\nZebra words. #lorem(30)\n";
    let mut req = request(dir.path(), &[("page.columns", json!(2))], content);
    req.overrides = vec![at("c", Kind::ColumnBreak, content, "Zebra")];
    let marked = compile_ok(&req);
    let plain = compile_ok(&request(
        dir.path(),
        &[("page.columns", json!(2))],
        &written(content, &req.overrides),
    ));
    assert_eq!(items(&marked), items(&plain));
}

#[test]
fn markers_report_positions() {
    let dir = tempfile::tempdir().unwrap();
    let mut req = request(dir.path(), &[], BREAKS);
    let one = |req: &CompileRequest| {
        let out = compile_ok(req);
        let s = out.overrides[0].clone();
        (out, s)
    };

    // A page break: at the top of the heading on the next page.
    req.overrides = vec![at("p", Kind::PageBreak { to_odd: false }, BREAKS, "Bravo")];
    let (out, s) = one(&req);
    let (page, bravo, size) = find(&out, "Bravo").unwrap();
    assert_eq!(s.page, Some(page), "{s:?}");
    let (x, y) = (s.x_pt.unwrap(), s.y_pt.unwrap());
    // At the start of the line (the heading's number comes before "Bravo").
    assert!(
        x <= bravo.x.to_pt() && x > bravo.x.to_pt() - 40.0,
        "{s:?} {bravo:?}"
    );
    assert!(
        y <= bravo.y.to_pt() && y > bravo.y.to_pt() - 2.0 * size,
        "{s:?} {bravo:?}"
    );
    assert_eq!((s.width_pt, s.height_pt), (None, None));
    // To a right-hand page: page 3.
    req.overrides = vec![at("p", Kind::PageBreak { to_odd: true }, BREAKS, "Bravo")];
    assert_eq!(one(&req).1.page, Some(2));
    // A weak break where a page starts anyway: page 1, at the top.
    req.overrides = vec![at("p", Kind::PageBreak { to_odd: false }, BREAKS, "Alpha")];
    let (out, s) = one(&req);
    let (_, alpha, size) = find(&out, "Alpha").unwrap();
    assert_eq!(s.page, Some(0));
    let y = s.y_pt.unwrap();
    assert!(
        y <= alpha.y.to_pt() && y > alpha.y.to_pt() - 2.0 * size,
        "{s:?}"
    );

    // Space before: its height, ending at the block.
    for length in ["36pt", "2em"] {
        req.overrides = vec![at(
            "s",
            Kind::SpaceBefore {
                length: Length::parse(length).unwrap(),
            },
            BREAKS,
            "Bravo",
        )];
        let (out, s) = one(&req);
        let (page, bravo, size) = find(&out, "Bravo").unwrap();
        let (_, _, body) = find(&out, "Second").unwrap();
        let h = s.height_pt.unwrap();
        let expect = if length == "2em" { 2.0 * body } else { 36.0 };
        assert!((h - expect).abs() < 0.01, "{length}: {s:?} {body}");
        assert_eq!(s.page, Some(page));
        let bottom = s.y_pt.unwrap() + h;
        assert!(
            bottom <= bravo.y.to_pt() && bottom > bravo.y.to_pt() - 2.0 * size,
            "{s:?} {bravo:?}"
        );
    }

    // A column break: the top of the second column.
    let content = "First words. #lorem(30)\n\nZebra words. #lorem(30)\n";
    let mut req = request(dir.path(), &[("page.columns", json!(2))], content);
    req.overrides = vec![at("c", Kind::ColumnBreak, content, "Zebra")];
    let (out, s) = one(&req);
    let (_, zebra, _) = find(&out, "Zebra").unwrap();
    assert_eq!(s.page, Some(0));
    assert!(
        (s.x_pt.unwrap() - zebra.x.to_pt()).abs() < 1.0,
        "{s:?} {zebra:?}"
    );
    assert!(s.x_pt.unwrap() > out.page_sizes[0][0] / 2.0);

    // A page break before a chapter heading that breaks the page itself.
    let content = "= One\n\nWords.\n\n= Two\n\nMore words.\n";
    let mut req = request(dir.path(), &[("preset", json!("novel"))], content);
    req.overrides = vec![at("p", Kind::PageBreak { to_odd: false }, content, "= Two")];
    let (out, s) = one(&req);
    let (page, two, size) = find(&out, "Two").unwrap();
    assert_eq!(s.page, Some(page), "{s:?}");
    let y = s.y_pt.unwrap();
    assert!(
        y <= two.y.to_pt() && y > two.y.to_pt() - 3.0 * size,
        "{s:?} {two:?}"
    );

    // Figures: the box of the placed figure (rectangle and caption).
    let content = format!(
        "{FIGURES}\n#figure(rect(width: 2cm, height: 1cm), caption: [Other]) <fig:o>\n\nSee @fig:o.\n"
    );
    let place = |p| Kind::FigurePlacement { placement: p };
    let mut boxes = Vec::new();
    for (p, needle) in [
        (Placement::Top, "Boxcap"),
        (Placement::Bottom, "Boxcap"),
        (Placement::Here, "Boxcap"),
        (Placement::Auto, "Boxcap"),
        (Placement::Top, "Other"),
    ] {
        let mut req = request(dir.path(), &[], &content);
        req.overrides = vec![at("f", place(p), &content, needle)];
        let (out, s) = one(&req);
        let (page, cap, _) = find(&out, needle).unwrap();
        let [w, h] = out.page_sizes[page];
        let (x, y) = (s.x_pt.unwrap(), s.y_pt.unwrap());
        let (fw, fh) = (s.width_pt.unwrap(), s.height_pt.unwrap());
        assert_eq!(s.page, Some(page), "{p:?} {s:?}");
        assert!(
            x >= 0.0 && y >= 0.0 && x + fw <= w && y + fh <= h,
            "{p:?} {s:?}"
        );
        // The caption is inside; the box is about as wide as the rectangle
        // and higher.
        assert!(
            y < cap.y.to_pt() && cap.y.to_pt() < y + fh,
            "{p:?} {s:?} {cap:?}"
        );
        let (rw, rh) = if needle == "Boxcap" {
            (3.0, 2.0)
        } else {
            (2.0, 1.0)
        };
        let cm = 72.0 / 2.54;
        assert!(fw >= rw * cm - 0.5 && fw < rw * cm + 100.0, "{p:?} {s:?}");
        assert!(fh > rh * cm, "{p:?} {s:?}");
        boxes.push((y, y + fh, h));
    }
    let [(top, ..), (_, bottom, h), (here, ..), ..] = boxes[..] else {
        unreachable!()
    };
    assert!(top < here && top < 100.0, "{boxes:?}");
    assert!(bottom > h - 100.0, "{boxes:?}");
}

/// The lines on page `page`: (vertical, thickness rounded to 0.01pt).
fn lines(out: &CompileOutput, page: usize) -> Vec<(bool, f64)> {
    fn walk(frame: &Frame, out: &mut Vec<(bool, f64)>) {
        for (_, item) in frame.items() {
            match item {
                FrameItem::Group(g) => walk(&g.frame, out),
                FrameItem::Shape(shape, _) => {
                    if let Geometry::Line(to) = shape.geometry {
                        let w = shape.stroke.as_ref().map_or(0.0, |s| s.thickness.to_pt());
                        out.push((to.x.to_pt().abs() < 0.01, (w * 100.0).round() / 100.0));
                    }
                }
                _ => {}
            }
        }
    }
    let mut list = Vec::new();
    walk(
        &out.document.as_ref().unwrap().paged().pages()[page].frame,
        &mut list,
    );
    list
}

const TABLE: &str = "Intro words.\n\n#figure(\n  table(columns: 2, table.header([Ha], [Hb]), [xa], [xb], [ya], [yb]),\n  caption: [Tabcap],\n) <tab:1>\n\nAfter words.\n";

#[test]
fn table_styles_are_exceptions_of_the_default() {
    let dir = tempfile::tempdir().unwrap();
    let style = |s| Kind::TableStyle { style: s };
    let (thick, thin) = (0.88, 0.55);

    // The default (paper: academic).
    let mut req = request(dir.path(), &[], TABLE);
    let out = compile_ok(&req);
    let mut rules: Vec<f64> = lines(&out, 0).iter().map(|l| l.1).collect();
    rules.sort_by(f64::total_cmp);
    assert_eq!(rules, [thin, thick, thick], "{:?}", lines(&out, 0));

    // A grid exception: its lines only, nothing of the academic default.
    req.overrides = vec![at("t", style(TableStyle::Grid), TABLE, "[ya]")];
    let out = compile_ok(&req);
    assert!(out.overrides[0].applied, "{:?}", out.overrides);
    let grid = lines(&out, 0);
    assert!(grid.iter().any(|l| l.0), "{grid:?}");
    assert!(grid.iter().all(|l| l.1 == 0.5), "{grid:?}");
    // Plain: none.
    req.overrides = vec![at("t", style(TableStyle::Plain), TABLE, "Tabcap")];
    assert!(lines(&compile_ok(&req), 0).is_empty());

    // An academic exception where the default is plain (novel).
    let mut req = request(dir.path(), &[("preset", json!("novel"))], TABLE);
    let page = |out: &CompileOutput| page_of(out, "xa");
    let out = compile_ok(&req);
    assert!(lines(&out, page(&out)).is_empty());
    req.overrides = vec![at("t", style(TableStyle::Academic), TABLE, "#figure")];
    let out = compile_ok(&req);
    let mut rules: Vec<f64> = lines(&out, page(&out)).iter().map(|l| l.1).collect();
    rules.sort_by(f64::total_cmp);
    assert_eq!(rules, [thin, thick, thick]);
    assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);

    // Its marker: the table's box, around its cells.
    let s = &out.overrides[0];
    let (p, xa, _) = find(&out, "xa").unwrap();
    let (_, yb, _) = find(&out, "yb").unwrap();
    assert_eq!(s.page, Some(p), "{s:?}");
    let (x, y, w, h) = (
        s.x_pt.unwrap(),
        s.y_pt.unwrap(),
        s.width_pt.unwrap(),
        s.height_pt.unwrap(),
    );
    assert!(x <= xa.x.to_pt() && y < xa.y.to_pt(), "{s:?} {xa:?}");
    assert!(x + w > yb.x.to_pt() && y + h > yb.y.to_pt(), "{s:?} {yb:?}");
    // Not the caption (the table's box, not the figure's).
    let (_, cap, _) = find(&out, "Tabcap").unwrap();
    assert!(cap.y.to_pt() > y + h, "{s:?} {cap:?}");

    // Clicks in a cell and on the caption name the table.
    let doc = out.document.as_ref().unwrap();
    let table = utf16(TABLE, TABLE.find("table(").unwrap());
    let figure = utf16(TABLE, TABLE.find("figure(").unwrap());
    for needle in ["xa", "Tabcap"] {
        let (page, p, size) = find(&out, needle).unwrap();
        let jump = doc.jump(page, p.x.to_pt() + 1.0, p.y.to_pt() - size * 0.3);
        assert_eq!(jump.table_utf16, Some(table), "{needle}: {jump:?}");
        assert_eq!(jump.figure_utf16, Some(figure), "{needle}: {jump:?}");
    }
    let (page, p, size) = find(&out, "After").unwrap();
    assert_eq!(
        doc.jump(page, p.x.to_pt() + 1.0, p.y.to_pt() - size * 0.3)
            .table_utf16,
        None
    );

    // No table there: not applied, with a warning.
    let mut req = request(dir.path(), &[], TABLE);
    req.overrides = vec![at("t", style(TableStyle::Grid), TABLE, "Intro")];
    let out = compile_ok(&req);
    assert!(!out.overrides[0].applied);
    assert!(
        out.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Warning
                && d.location == Location::Override { id: "t".into() })
    );
}

#[test]
fn default_table_rules_map_to_the_content() {
    let dir = tempfile::tempdir().unwrap();
    // An error after the injected rules still points at its text.
    let content = format!("{TABLE}\nText with a #nope(1) call.\n");
    let out = compile(&request(dir.path(), &[], &content));
    let d = out
        .diagnostics
        .iter()
        .find(|d| d.severity == Severity::Error)
        .expect("an error");
    let Location::Content { start, end, .. } = d.location else {
        panic!("{d:?}");
    };
    assert_eq!(&content[start..end], "nope");
    // The plan's edits for the default have no override.
    let plan = overrides::plan(&content, &[], TableStyle::Academic);
    assert_eq!(plan.edits.len(), 2);
    assert!(plan.edits.iter().all(|e| e.id.is_empty()));
    assert!(plan.statuses.is_empty());
}
