//! The compile worker in its own process, driven by the supervisor.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

mod common;

use lethora_typeset::layout::{self, Entries};
use lethora_typeset::overrides::{Kind, Placement, ResolvedOverride};
use lethora_typeset::{CompileRequest, JumpTarget, Limits, Meta, Supervisor, WorkerError};
use serde_json::json;

const BIB: &str = "@book{knuth84, author = {Knuth, Donald E.}, title = {The TeXbook}, \
                   publisher = {Addison-Wesley}, year = {1984}}";

fn sample() -> String {
    let mut s = String::new();
    for chapter in 1..=6 {
        s.push_str(&format!("= Chapter {chapter}\n\n"));
        for p in 0..8 {
            s.push_str(&format!(
                "Paragraph {p} with $x_{p}^2 + y = {chapter}$ and @knuth84. #lorem(90)\n\n"
            ));
        }
        s.push_str("$ sum_(k=1)^n k = (n(n+1))/2 $\n\n");
    }
    s
}

fn project() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn request(dir: &Path, content: &str) -> CompileRequest {
    let entries: Entries = [("preset".to_owned(), json!("paper"))].into();
    CompileRequest {
        project_dir: dir.to_path_buf(),
        layout: layout::resolve(&entries),
        meta: Meta::default(),
        custom: String::new(),
        content: content.into(),
        today: None,
        overrides: Vec::new(),
        bibliography: BIB.into(),
        assets: common::index(content),
    }
}

fn supervisor(limits: Limits) -> Supervisor {
    Supervisor::new(
        Command::new(env!("CARGO_BIN_EXE_lethora-typeset-worker")),
        limits,
    )
}

fn limits(timeout_ms: u64, memory_mb: u64) -> Limits {
    Limits {
        timeout: Duration::from_millis(timeout_ms),
        memory_mb,
    }
}

#[test]
fn compile_render_export_and_stale_versions() {
    let dir = project();
    let mut sup = supervisor(Limits::default());
    // Search the system fonts first, so the compile times compare fairly.
    sup.fonts().unwrap();

    let content = sample();
    let start = Instant::now();
    let first = sup.compile(1, request(dir.path(), &content)).unwrap();
    let first_wall = start.elapsed();
    assert!(first.ok, "{:?}", first.diagnostics);
    assert_eq!(first.pages_version, Some(1));
    assert!(first.pages >= 3, "{} pages", first.pages);
    assert_eq!(first.page_sizes.len(), first.pages);
    assert_eq!(sup.held_version(), Some(1));

    let svg = sup.render_page(1, 0).unwrap();
    assert!(svg.starts_with(b"<svg"));
    assert!(matches!(
        sup.render_page(1, 9999),
        Err(WorkerError::Failed(_))
    ));
    let pdf = sup.export_pdf(1).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
    // Exports: the compiled document, citations resolved.
    let md = sup.export(1, "markdown", "out").unwrap();
    let md = String::from_utf8(md.bytes).unwrap();
    assert!(
        md.contains("# 1. Chapter 1") && md.contains("(Knuth, 1984)"),
        "{md}"
    );
    assert!(matches!(
        sup.export(1, "pdf-or-so", "out"),
        Err(WorkerError::Failed(m)) if m.contains("can't export")
    ));
    // Clicks: the top left corner is margin.
    assert_eq!(sup.jump(1, 0, 2.0, 2.0).unwrap().target, JumpTarget::None);

    // An edit: incremental, faster than the first compile.
    let edited = content.replacen("Paragraph 3", "Paragraph three", 1);
    let second = sup.compile(2, request(dir.path(), &edited)).unwrap();
    assert!(second.ok);
    println!(
        "first {} ms ({first_wall:?} wall), after an edit {} ms",
        first.duration_ms, second.duration_ms
    );
    assert!(
        second.duration_ms < first.duration_ms,
        "{} >= {}",
        second.duration_ms,
        first.duration_ms
    );
    // Version 1 is gone.
    let e = sup.render_page(1, 0).unwrap_err();
    assert!(
        matches!(&e, WorkerError::Failed(m) if m.contains("not available")),
        "{e:?}"
    );
    assert!(sup.render_page(2, 0).is_ok());
    assert!(matches!(
        sup.export(1, "text", "out"),
        Err(WorkerError::Failed(_))
    ));
    assert!(matches!(
        sup.jump(1, 0, 100.0, 100.0),
        Err(WorkerError::Failed(_))
    ));
    assert!(sup.jump(2, 0, 100.0, 100.0).is_ok());

    // A failed compile keeps the previous pages and reports the problem.
    let broken = format!("{edited}\n#nope()");
    let third = sup.compile(3, request(dir.path(), &broken)).unwrap();
    assert!(!third.ok);
    assert_eq!(third.version, 3);
    assert_eq!(third.pages_version, Some(2));
    assert_eq!(third.pages, second.pages);
    assert!(third.diagnostics.iter().any(|d| d.message.contains("nope")));
    assert!(sup.render_page(2, 0).is_ok());
    assert!(sup.render_page(3, 0).is_err());

    // Overrides are located on the pages only by a compile that made them.
    let page_break = |content: &str| ResolvedOverride {
        id: "p".into(),
        kind: Kind::PageBreak { to_odd: false },
        content_byte_offset: content.find("Paragraph 5").unwrap(),
        detached: false,
    };
    let mut req = request(dir.path(), &broken);
    req.overrides = vec![page_break(&broken)];
    let failed = sup.compile(4, req).unwrap();
    assert!(!failed.ok && failed.overrides[0].applied);
    assert_eq!(failed.overrides[0].page, None);
    let mut req = request(dir.path(), &edited);
    req.overrides = vec![page_break(&edited)];
    let located = sup.compile(5, req).unwrap();
    assert!(located.ok);
    let s = &located.overrides[0];
    assert!(s.page.is_some() && s.y_pt.is_some(), "{s:?}");

    sup.shutdown();
    assert_eq!(sup.held_version(), None);
}

#[test]
fn export_image_quality_reaches_the_worker() {
    let dir = project();
    fs::create_dir_all(dir.path().join("assets")).unwrap();
    let photo = image::RgbImage::from_fn(1600, 1200, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x * y) % 239) as u8])
    });
    image::DynamicImage::ImageRgb8(photo)
        .save_with_format(
            dir.path()
                .join("assets")
                .join(common::blob("assets/photo.jpg")),
            image::ImageFormat::Jpeg,
        )
        .unwrap();
    let content = "Text.\n\n#image(\"assets/photo.jpg\", width: 6cm)\n";
    let mut sup = supervisor(Limits::default());
    let mut export = |version, quality: &str, keep: bool| {
        let mut req = request(dir.path(), content);
        req.layout.export.image_quality = serde_json::from_value(json!(quality)).unwrap();
        if keep {
            req.overrides = vec![ResolvedOverride {
                id: "keep".into(),
                kind: Kind::ImageQuality {
                    quality: lethora_typeset::ImageQuality::Original,
                },
                content_byte_offset: content.find("image(").unwrap(),
                detached: false,
            }];
        }
        let compiled = sup.compile(version, req).unwrap();
        assert!(compiled.ok && compiled.overrides.iter().all(|s| s.applied));
        sup.export_pdf(version).unwrap().len()
    };
    let original = export(1, "original", false);
    let screen = export(2, "screen", false);
    let kept = export(3, "screen", true);
    println!("PDF: original {original} B, screen {screen} B, kept {kept} B");
    assert!(screen * 2 < original);
    assert_eq!(kept, original);
    sup.shutdown();
}

#[test]
fn fonts_include_embedded_and_system_families() {
    let mut sup = supervisor(Limits::default());
    let fonts = sup.fonts().unwrap();
    println!(
        "{} families, system search {} ms",
        fonts.families.len(),
        fonts.scan_ms
    );
    for embedded in lethora_typeset::fonts() {
        assert!(fonts.families.contains(&embedded), "{embedded}");
    }
    assert!(
        fonts.families.len() > lethora_typeset::fonts().len(),
        "no system fonts: {:?}",
        fonts.families
    );
    let mut sorted = fonts.families.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted, fonts.families);
    // A system family passes validation against the list.
    let system = fonts
        .families
        .iter()
        .find(|f| !lethora_typeset::fonts().contains(f))
        .unwrap();
    assert_eq!(
        layout::validate_change_in("text.font", &json!(system.to_lowercase()), &fonts.families),
        Ok(json!(system))
    );
    assert!(
        layout::validate_change_in("text.font", &json!("No Such Font"), &fonts.families).is_err()
    );
}

#[cfg(debug_assertions)]
#[test]
fn timeout_kills_and_restarts() {
    // The timeout includes starting the worker, which on a fresh CI runner
    // (virus scan, Gatekeeper on a new exe) took over 500 ms.
    let mut sup = supervisor(limits(3_000, 2048));
    sup.test_sleep(10).unwrap();
    let start = Instant::now();
    assert_eq!(sup.test_sleep(20_000), Err(WorkerError::Timeout));
    assert!(
        start.elapsed() < Duration::from_secs(8),
        "{:?}",
        start.elapsed()
    );
    // The next request gets a fresh worker.
    sup.test_sleep(10).unwrap();
}

#[cfg(debug_assertions)]
#[test]
fn panic_is_a_crash_and_the_worker_restarts() {
    let dir = project();
    let mut sup = supervisor(Limits::default());
    assert!(sup.compile(1, request(dir.path(), "Hello.")).unwrap().ok);
    let e = sup.test_panic().unwrap_err();
    assert!(
        matches!(&e, WorkerError::Crashed(m) if m.contains("stopped")),
        "{e:?}"
    );
    // The held pages died with it.
    assert_eq!(sup.held_version(), None);
    assert!(sup.render_page(1, 0).is_err());
    assert!(
        sup.compile(2, request(dir.path(), "Hello again."))
            .unwrap()
            .ok
    );
    assert!(sup.render_page(2, 0).is_ok());
}

#[cfg(all(debug_assertions, any(windows, target_os = "linux")))]
#[test]
fn memory_cap_is_a_crash_and_the_worker_restarts() {
    let mut sup = supervisor(limits(30_000, 256));
    sup.test_alloc(16).unwrap();
    let e = sup.test_alloc(1024).unwrap_err();
    if cfg!(windows) {
        assert!(
            matches!(&e, WorkerError::Crashed(m) if m.contains("out of memory")),
            "{e:?}"
        );
    } else {
        assert!(matches!(e, WorkerError::Crashed(_)), "{e:?}");
    }
    sup.test_alloc(16).unwrap();
}

/// ~5M characters (~700k words): chapters, sections, paragraphs of varied
/// words, inline and display math, figures and citations.
fn book() -> String {
    const WORDS: &[&str] = &[
        "the",
        "of",
        "and",
        "a",
        "to",
        "in",
        "is",
        "was",
        "that",
        "for",
        "with",
        "as",
        "on",
        "by",
        "at",
        "from",
        "his",
        "her",
        "which",
        "or",
        "an",
        "were",
        "this",
        "had",
        "not",
        "but",
        "be",
        "their",
        "are",
        "they",
        "one",
        "all",
        "would",
        "there",
        "been",
        "time",
        "river",
        "morning",
        "letters",
        "distance",
        "remembered",
        "quietly",
        "lantern",
        "harbour",
        "experiment",
        "measurement",
        "equation",
        "observation",
        "particular",
        "afterwards",
        "nevertheless",
        "consideration",
        "extraordinary",
        "understanding",
    ];
    let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
    let mut word = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        WORDS[(seed % WORDS.len() as u64) as usize]
    };
    let mut s = String::with_capacity(5_200_000);
    let (mut chapter, mut n) = (0, 0u64);
    while s.len() < 5_000_000 {
        chapter += 1;
        s.push_str(&format!("= Chapter {chapter}\n\n"));
        for section in 1..=5 {
            s.push_str(&format!("== Section {chapter}.{section}\n\n"));
            for p in 0..12 {
                n += 1;
                let len = 60 + (n * 37 % 90) as usize;
                let mut para = String::new();
                for i in 0..len {
                    let w = word();
                    if i == 0 {
                        let mut c = w.chars();
                        para.extend(c.next().map(|c| c.to_ascii_uppercase()));
                        para.push_str(c.as_str());
                    } else {
                        para.push(' ');
                        para.push_str(w);
                    }
                    if i % 17 == 16 {
                        para.push(if i % 3 == 0 { ',' } else { '.' });
                    }
                }
                para.push('.');
                if p % 4 == 1 {
                    para.push_str(&format!(
                        " As shown in $x_{p}^2 + y = {chapter}$, see @knuth84."
                    ));
                }
                s.push_str(&para);
                s.push_str("\n\n");
            }
            s.push_str(&format!(
                "$ sum_(k=1)^n k = (n(n+1))/2 $ <eq:{chapter}-{section}>\n\n"
            ));
            if section % 2 == 0 {
                s.push_str(&format!(
                    "#figure(image(\"assets/plot.svg\", width: 40%), caption: [Plot {chapter}.{section}.])\n\n"
                ));
            }
        }
    }
    s
}

/// `n` overrides spread evenly over `content`: block kinds at paragraph
/// starts, placements on figures.
fn spread_overrides(content: &str, n: usize) -> Vec<ResolvedOverride> {
    let mut out: Vec<ResolvedOverride> = Vec::new();
    let step = content.len() / n;
    for i in 0..n {
        let from = i * step;
        let (needle, kind) = match i % 4 {
            0 => ("\n\n= ", Kind::PageBreak { to_odd: i % 8 == 0 }),
            1 => (
                "\n\n#figure",
                Kind::FigurePlacement {
                    placement: if i % 3 == 0 {
                        Placement::Top
                    } else {
                        Placement::Bottom
                    },
                },
            ),
            2 => (
                "\n\n",
                Kind::SpaceBefore {
                    length: layout::Length::parse("6pt").unwrap(),
                },
            ),
            _ => ("\n\n== ", Kind::ColumnBreak),
        };
        let Some(at) = content[from..].find(needle) else {
            continue;
        };
        let at = from + at + 2;
        if out.iter().any(|o| o.content_byte_offset == at) {
            continue;
        }
        out.push(ResolvedOverride {
            id: format!("o{i}"),
            kind,
            content_byte_offset: at,
            detached: false,
        });
    }
    out
}

/// Whole-document compilation at book size, in the worker. Run with
/// `cargo test --release -p lethora-typeset --test worker -- --ignored --nocapture`.
#[test]
#[ignore]
fn book_size() {
    let dir = project();
    fs::create_dir_all(dir.path().join("assets")).unwrap();
    common::put(
        dir.path(),
        "assets/plot.svg",
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="60"><rect width="120" height="60" fill="green"/></svg>"#,
    );
    let content = book();
    let words = content.split_whitespace().count();
    println!("book: {} chars, {words} words", content.len());

    let start = Instant::now();
    let json = serde_json::to_vec(&request(dir.path(), &content)).unwrap();
    println!(
        "serializing the request: {:?} ({} MB)",
        start.elapsed(),
        json.len() >> 20
    );

    for preset in ["paper", "novel"] {
        let entries: Entries = [("preset".to_owned(), json!(preset))].into();
        let mut req = request(dir.path(), &content);
        req.layout = layout::resolve(&entries);
        let mut sup = supervisor(limits(600_000, 16_384));
        sup.fonts().unwrap();

        let start = Instant::now();
        let cold = sup.compile(1, req.clone()).unwrap();
        let cold_wall = start.elapsed().as_millis() as u64;
        let cold_peak = sup.peak_memory_mb();
        assert!(
            cold.ok,
            "{:?}",
            &cold.diagnostics[..cold.diagnostics.len().min(3)]
        );

        // One word changed in the middle.
        let mid = req.content.len() / 2;
        let at = mid + req.content[mid..].find(" the ").unwrap();
        req.content.replace_range(at + 1..at + 4, "one");
        let start = Instant::now();
        let edit = sup.compile(2, req.clone()).unwrap();
        let edit_wall = start.elapsed().as_millis() as u64;
        let edit_peak = sup.peak_memory_mb();
        assert!(edit.ok);

        // Sustained typing: more one-word insertions across the book.
        let mut typing = Vec::new();
        for (i, at) in [0.3, 0.7, 0.5, 0.9, 0.1, 0.6].into_iter().enumerate() {
            let pos = (req.content.len() as f64 * at) as usize;
            let pos = pos + req.content[pos..].find(' ').unwrap();
            req.content.insert_str(pos, " word");
            let out = sup.compile(10 + i as u64, req.clone()).unwrap();
            assert!(out.ok);
            typing.push(out.duration_ms);
        }
        let typing_peak = sup.peak_memory_mb();

        // A layout change: everything is laid out again.
        req.layout.text.size = layout::Length::parse("10.5pt").unwrap();
        let start = Instant::now();
        let restyle = sup.compile(3, req.clone()).unwrap();
        let restyle_wall = start.elapsed().as_millis() as u64;
        assert!(restyle.ok);

        let start = Instant::now();
        let svg = sup.render_page(3, restyle.pages / 2).unwrap();
        let svg_time = start.elapsed();
        let start = Instant::now();
        let pdf = sup.export_pdf(3).unwrap();
        let pdf_time = start.elapsed();

        // ~200 overrides spread over the book.
        let overrides = spread_overrides(&req.content, 200);
        let start = Instant::now();
        let plan =
            lethora_typeset::overrides::plan(&req.content, &overrides, req.layout.tables.style);
        let plan_time = start.elapsed();
        let applied = plan.statuses.iter().filter(|s| s.applied).count();
        assert_eq!(applied, overrides.len());
        req.overrides = overrides;
        let with = sup.compile(20, req.clone()).unwrap();
        assert!(with.ok);
        let located = with.overrides.iter().filter(|s| s.page.is_some()).count();
        assert_eq!(located, applied);
        let with_req = req.clone();
        let pos = req.content.len() / 3;
        let pos = pos + req.content[pos..].find(' ').unwrap();
        req.content.insert_str(pos, " word");
        for o in &mut req.overrides {
            if o.content_byte_offset > pos {
                o.content_byte_offset += 5;
            }
        }
        let with_edit = sup.compile(21, req.clone()).unwrap();
        assert!(with_edit.ok);
        let start = Instant::now();
        let jump = sup.jump(21, with_edit.pages / 2, 200.0, 300.0).unwrap();
        let jump_time = start.elapsed();
        req.overrides.clear();
        println!(
            "{preset}: {} pages ({} after the font size change)\n  \
             cold compile {} ms (wall {cold_wall} ms)\n  \
             after a one-word edit {} ms (wall {edit_wall} ms)\n  \
             six more one-word edits {typing:?} ms\n  \
             after a font size change {} ms (wall {restyle_wall} ms)\n  \
             one page to SVG {svg_time:?} ({} KB)\n  \
             PDF export {pdf_time:?} ({} MB)\n  \
             {applied} overrides: planning {plan_time:?}, compile {} ms ({} pages), \
             then a one-word edit {} ms\n  \
             {located} of them located on the pages\n  \
             a click {jump_time:?} ({:?})\n  \
             worker peak memory (MB) after the cold compile {cold_peak:?}, the edit \
             {edit_peak:?}, the typing {typing_peak:?}, at the end {:?}",
            cold.pages,
            restyle.pages,
            cold.duration_ms,
            edit.duration_ms,
            restyle.duration_ms,
            svg.len() / 1024,
            pdf.len() >> 20,
            with.duration_ms,
            with.pages,
            with_edit.duration_ms,
            jump.target,
            sup.peak_memory_mb(),
        );
        sup.shutdown();

        // Locating the overrides on the pages, in-process.
        let out = lethora_typeset::compile(&with_req);
        let mut statuses = out.overrides.clone();
        let start = Instant::now();
        out.document
            .as_ref()
            .unwrap()
            .locate_overrides(&mut statuses);
        println!(
            "  locating {located} overrides on the pages {:?}",
            start.elapsed()
        );
        assert_eq!(statuses, out.overrides);
    }
}

/// Clicks that miss the text (margins, a corner) at book size, in-process,
/// and one in the middle of the page (most likely on the text).
/// Run with
/// `cargo test --release -p lethora-typeset --test worker -- --ignored --nocapture`.
#[test]
#[ignore]
fn book_size_clicks() {
    let dir = project();
    fs::create_dir_all(dir.path().join("assets")).unwrap();
    common::put(
        dir.path(),
        "assets/plot.svg",
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="60"><rect width="120" height="60" fill="green"/></svg>"#,
    );
    let start = Instant::now();
    let out = lethora_typeset::compile(&request(dir.path(), &book()));
    println!("book: {} pages in {:?}", out.pages, start.elapsed());
    let doc = out.document.as_ref().unwrap();
    let n = out.pages;
    let mut worst = Duration::ZERO;
    for page in [1, n / 4, n / 2, 3 * n / 4, n - 2] {
        let [w, h] = out.page_sizes[page];
        let mut times = Vec::new();
        for (x, y) in [
            (10.0, h / 2.0),
            (w - 10.0, h / 3.0),
            (10.0, 5.0),
            (w / 2.0, h / 2.0),
        ] {
            let start = Instant::now();
            let jump = doc.jump(page, x, y);
            let time = start.elapsed();
            worst = worst.max(time);
            times.push((time, matches!(jump.target, JumpTarget::Content { .. })));
        }
        println!("page {page}: (time, hit) left/right margin, corner, middle {times:?}");
    }
    println!("slowest click {worst:?}");
}
