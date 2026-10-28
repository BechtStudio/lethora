//! Image quality at PDF export: raster images downscaled to what their
//! placement needs, with the layout unchanged.

use std::io::Cursor;
use std::path::Path;
use std::time::Instant;

mod common;

use image::{DynamicImage, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};
use lethora_typeset::layout::{self, Entries, Meta};
use lethora_typeset::overrides::{Kind, ResolvedOverride};
use lethora_typeset::{CompileOutput, CompileRequest, ImageQuality, Severity, compile};
use serde_json::{Value, json};
use typst::layout::{Frame, FrameItem, Transform};
use typst::visualize::{ExchangeFormat, Image, ImageKind, RasterFormat, RasterImage};
use typst_layout::PagedDocument;

fn request(dir: &Path, quality: &str, content: &str) -> CompileRequest {
    let entries: Entries = [
        ("preset".to_owned(), json!("paper")),
        ("export.image_quality".to_owned(), Value::from(quality)),
    ]
    .into();
    CompileRequest {
        project_dir: dir.to_path_buf(),
        layout: layout::resolve(&entries),
        meta: Meta::default(),
        custom: String::new(),
        content: content.into(),
        today: None,
        overrides: Vec::new(),
        bibliography: String::new(),
        assets: common::index(content),
    }
}

fn compile_ok(req: &CompileRequest) -> CompileOutput {
    let out = compile(req);
    let errors: Vec<_> = out
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(out.document.is_some());
    out
}

/// Stores `image` as the blob of `assets/<name>` in the format of its extension.
fn asset(dir: &Path, name: &str, image: DynamicImage) {
    let format = ImageFormat::from_path(name).unwrap();
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, format).unwrap();
    common::put(dir, &format!("assets/{name}"), bytes.into_inner());
}

/// A smooth RGBA picture with alpha falling from left to right.
fn rgba(w: u32, h: u32) -> DynamicImage {
    DynamicImage::ImageRgba8(RgbaImage::from_fn(w, h, |x, y| {
        Rgba([
            (x * 255 / w) as u8,
            (y * 255 / h) as u8,
            ((x + y) % 256) as u8,
            255 - (x * 255 / w) as u8,
        ])
    }))
}

/// A photo-like RGB picture: gradients with some noise (`seed` varies it).
fn photo(w: u32, h: u32, seed: u32) -> DynamicImage {
    DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let n = (x.wrapping_mul(7919) ^ y.wrapping_mul(104_729) ^ seed.wrapping_mul(31))
            .wrapping_mul(2_654_435_761)
            >> 27;
        Rgb([
            ((x * 200 / w) + n) as u8,
            ((y * 200 / h) + n) as u8,
            (((x + y + seed) % 200) + n) as u8,
        ])
    }))
}

/// A placed image: page, image, and box (x, y, w, h) in points.
type Placed = (usize, Image, [f64; 4]);

fn placed(doc: &PagedDocument) -> Vec<Placed> {
    fn walk(frame: &Frame, ts: Transform, page: usize, out: &mut Vec<Placed>) {
        for (pos, item) in frame.items() {
            let ts = ts.pre_concat(Transform::translate(pos.x, pos.y));
            match item {
                FrameItem::Group(g) => walk(&g.frame, ts.pre_concat(g.transform), page, out),
                FrameItem::Image(image, size, _) => out.push((
                    page,
                    image.clone(),
                    [
                        ts.tx.to_pt(),
                        ts.ty.to_pt(),
                        size.x.to_pt() * ts.sx.get(),
                        size.y.to_pt() * ts.sy.get(),
                    ],
                )),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    for (i, page) in doc.pages().iter().enumerate() {
        walk(&page.frame, Transform::identity(), i, &mut out);
    }
    out
}

fn raster(image: &Image) -> &RasterImage {
    match image.kind() {
        ImageKind::Raster(r) => r,
        _ => panic!("not a raster image"),
    }
}

/// The pages and image boxes of `before` and `after` are the same.
fn same_layout(before: &PagedDocument, after: &PagedDocument) {
    assert_eq!(before.pages().len(), after.pages().len());
    let boxes = |doc| {
        placed(doc)
            .into_iter()
            .map(|(page, _, b)| (page, b))
            .collect::<Vec<_>>()
    };
    assert_eq!(boxes(before), boxes(after));
    for (a, b) in before.pages().iter().zip(after.pages()) {
        assert_eq!(a.frame.size(), b.frame.size());
        assert_eq!(a.frame.items().len(), b.frame.items().len());
    }
}

/// Pixels at 150 dpi for `cm` centimetres.
fn screen_px(cm: f64) -> f64 {
    cm / 2.54 * 150.0
}

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|w| *w == needle)
        .count()
}

#[test]
fn screen_downscales_a_large_png_and_keeps_the_layout() {
    let dir = tempfile::tempdir().unwrap();
    asset(dir.path(), "big.png", rgba(3000, 2000));
    asset(dir.path(), "small.png", photo(200, 100, 1));
    common::put(
        dir.path(),
        "assets/plot.svg",
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><rect width="100" height="50" fill="teal"/></svg>"#,
    );
    // The big image twice, at 4 and 8 cm; a small one (needs more pixels
    // than it has) and a vector image; headings and links for the outline.
    let content = "= Intro <intro>\n\n#image(\"assets/big.png\", width: 4cm)\n\nSee @intro and #link(\"https://example.org\")[the web].\n\n#figure(image(\"assets/big.png\", width: 8cm), caption: [Big.])\n\n#image(\"assets/small.png\", width: 8cm)\n\n#image(\"assets/plot.svg\", width: 3cm)\n\n= Outro\n";

    let original = compile_ok(&request(dir.path(), "original", content));
    let doc = original.document.as_ref().unwrap();
    assert!(doc.export_document().is_none(), "Original changes nothing");
    let original_pdf = doc.export_pdf().unwrap();
    // Deterministic, and exactly what Typst exports.
    assert_eq!(original_pdf, doc.export_pdf().unwrap());
    assert_eq!(
        original_pdf,
        typst_pdf::pdf(doc.paged(), &typst_pdf::PdfOptions::default()).unwrap()
    );

    let screen = compile_ok(&request(dir.path(), "screen", content));
    let doc = screen.document.as_ref().unwrap();
    let exported = doc.export_document().expect("images change");
    same_layout(doc.paged(), &exported);
    let before = placed(doc.paged());
    let after = placed(&exported);
    assert_eq!(after.len(), 4);
    // One new image for both placements, sized for the 8 cm one.
    let big = &after[0].1;
    assert_eq!(&after[1].1, big);
    assert_ne!(&before[0].1, big);
    let r = raster(big);
    assert!(
        (f64::from(r.width()) - screen_px(8.0)).abs() <= 1.0,
        "{}",
        r.width()
    );
    assert!((f64::from(r.height()) - screen_px(8.0) * 2.0 / 3.0).abs() <= 1.0);
    assert_eq!(r.format(), RasterFormat::Exchange(ExchangeFormat::Png));
    assert!(r.dynamic().color().has_alpha());
    let alpha = |r: &RasterImage, x: f64| {
        let img = r.dynamic().to_rgba8();
        img.get_pixel((x * f64::from(img.width() - 1)) as u32, 0)[3]
    };
    let old = raster(&before[0].1);
    for x in [0.0, 0.5, 1.0] {
        assert!(alpha(r, x).abs_diff(alpha(old, x)) <= 3, "alpha at {x}");
    }
    // The small and the vector image are the same as before.
    assert_eq!(after[2].1, before[2].1);
    assert_eq!(raster(&after[2].1).data(), raster(&before[2].1).data());
    assert_eq!(after[3].1, before[3].1);

    // Much smaller; links, outline and tags still there.
    let pdf = doc.export_pdf().unwrap();
    println!(
        "PDF with a 3000×2000 PNG: original {} KB, screen {} KB",
        original_pdf.len() / 1024,
        pdf.len() / 1024
    );
    assert!(pdf.len() * 4 < original_pdf.len());
    for needle in [&b"/Outlines"[..], b"/URI", b"/Link", b"/StructTreeRoot"] {
        assert_eq!(
            count(&pdf, needle),
            count(&original_pdf, needle),
            "{}",
            String::from_utf8_lossy(needle)
        );
    }
    assert!(count(&pdf, b"/Outlines") > 0 && count(&pdf, b"/URI") > 0);
}

#[test]
fn jpegs_stay_jpegs_and_an_image_can_keep_its_own_quality() {
    let dir = tempfile::tempdir().unwrap();
    asset(dir.path(), "photo.jpg", photo(1600, 1200, 2));
    let content = "Text first.\n\n#image(\"assets/photo.jpg\", width: 8cm)\n\n#image(\"assets/photo.jpg\", width: 6cm)\n";
    let second = content.rfind("image(").unwrap();
    let own = |id: &str, quality, at: usize| ResolvedOverride {
        id: id.into(),
        kind: Kind::ImageQuality { quality },
        content_byte_offset: at,
        detached: false,
    };

    let mut req = request(dir.path(), "screen", content);
    let out = compile_ok(&req);
    let doc = out.document.as_ref().unwrap();
    let exported = doc.export_document().unwrap();
    let after = placed(&exported);
    let r = raster(&after[0].1);
    assert_eq!(r.format(), RasterFormat::Exchange(ExchangeFormat::Jpg));
    assert!((f64::from(r.width()) - screen_px(8.0)).abs() <= 1.0);
    assert_eq!(after[0].1, after[1].1);

    // The second image keeps the original (an override in its call); one
    // on plain text is not applied.
    req.overrides = vec![
        own("keep", ImageQuality::Original, second + 10),
        own("text", ImageQuality::Email, 2),
    ];
    let out = compile_ok(&req);
    let doc = out.document.as_ref().unwrap();
    let status = |id: &str| out.overrides.iter().find(|s| s.id == id).unwrap();
    assert!(status("keep").applied);
    assert!(!status("text").applied);
    assert!(out.diagnostics.iter().any(|d| {
        d.severity == Severity::Warning
            && d.message
                .contains("image quality is not applied: there is no image")
    }));
    let before = placed(doc.paged());
    let exported = doc.export_document().unwrap();
    same_layout(doc.paged(), &exported);
    let after = placed(&exported);
    assert_ne!(after[0].1, before[0].1);
    assert_eq!(after[1].1, before[1].1);
    assert!((f64::from(raster(&after[0].1).width()) - screen_px(8.0)).abs() <= 1.0);
    // Its marker is at the image's box.
    let s = status("keep");
    let [x, y, w, h] = before[1].2;
    assert_eq!(s.page, Some(before[1].0));
    let near = |a: Option<f64>, b: f64| (a.unwrap() - b).abs() < 0.01;
    assert!(near(s.x_pt, x) && near(s.y_pt, y) && near(s.width_pt, w) && near(s.height_pt, h));

    // Print (300 dpi) as the document's quality, Email for the second.
    let mut req = request(dir.path(), "print", content);
    req.overrides = vec![own("e", ImageQuality::Email, second)];
    let out = compile_ok(&req);
    let after = placed(&out.document.as_ref().unwrap().export_document().unwrap());
    let width = |i: usize| f64::from(raster(&after[i].1).width());
    assert!((width(0) - 8.0 / 2.54 * 300.0).abs() <= 1.0, "{}", width(0));
    assert!((width(1) - 6.0 / 2.54 * 96.0).abs() <= 1.0, "{}", width(1));
}

/// `jpeg` with an EXIF APP1 segment setting orientation `value`.
fn with_orientation(jpeg: &[u8], value: u8) -> Vec<u8> {
    let tiff: &[u8] = &[
        b'M', b'M', 0, 42, 0, 0, 0, 8, // header, first IFD at 8
        0, 1, // one entry
        0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, value, 0, 0, // orientation, SHORT
        0, 0, 0, 0, // no next IFD
    ];
    let len = (2 + 6 + tiff.len()) as u16;
    let mut out = jpeg[..2].to_vec();
    out.extend([0xFF, 0xE1]);
    out.extend(len.to_be_bytes());
    out.extend(b"Exif\0\0");
    out.extend(tiff);
    out.extend(&jpeg[2..]);
    out
}

#[test]
fn exif_rotated_jpegs_keep_their_orientation() {
    let dir = tempfile::tempdir().unwrap();
    // Left half red, right half blue; turned a quarter clockwise by EXIF,
    // red is on top.
    let image = RgbImage::from_fn(1600, 800, |x, _| {
        if x < 800 {
            Rgb([220, 20, 20])
        } else {
            Rgb([20, 20, 220])
        }
    });
    let mut jpeg = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image)
        .write_to(&mut jpeg, ImageFormat::Jpeg)
        .unwrap();
    common::put(
        dir.path(),
        "assets/turned.jpg",
        with_orientation(&jpeg.into_inner(), 6),
    );
    let out = compile_ok(&request(
        dir.path(),
        "screen",
        "#image(\"assets/turned.jpg\", width: 3cm)\n",
    ));
    let doc = out.document.as_ref().unwrap();
    let exported = doc.export_document().unwrap();
    same_layout(doc.paged(), &exported);
    let top_and_bottom = |r: &RasterImage| {
        let img = r.dynamic().to_rgb8();
        let (w, h) = img.dimensions();
        (
            *img.get_pixel(w / 2, h / 8),
            *img.get_pixel(w / 2, h * 7 / 8),
        )
    };
    let old = raster(&placed(doc.paged())[0].1).clone();
    let new = raster(&placed(&exported)[0].1).clone();
    assert_eq!(old.exif_rotation(), Some(6));
    assert_eq!((old.width(), old.height()), (800, 1600));
    assert_eq!(new.exif_rotation(), None);
    assert_eq!(new.format(), RasterFormat::Exchange(ExchangeFormat::Jpg));
    assert!((f64::from(new.width()) - screen_px(3.0)).abs() <= 1.0);
    assert_eq!(new.height(), new.width() * 2);
    for r in [&old, &new] {
        let (top, bottom) = top_and_bottom(r);
        assert!(top[0] > 180 && top[2] < 60, "{top:?}");
        assert!(bottom[2] > 180 && bottom[0] < 60, "{bottom:?}");
    }
}

#[test]
fn clicks_on_images_and_in_figures_name_the_image_call() {
    let dir = tempfile::tempdir().unwrap();
    asset(dir.path(), "a.png", photo(300, 200, 3));
    let content = "#lorem(30)\n\n#image(\"assets/a.png\", width: 5cm)\n\n#figure(image(\"assets/a.png\", width: 30%), caption: [A caption clearly wider than the narrow image above it.])\n";
    let utf16 = |needle: &str| {
        content[..content.find(needle).unwrap()]
            .encode_utf16()
            .count()
    };
    let out = compile_ok(&request(dir.path(), "original", content));
    let doc = out.document.as_ref().unwrap();
    let images = placed(doc.paged());
    let plain = utf16("image(\"assets/a.png\", width: 5cm");
    let in_figure = utf16("image(\"assets/a.png\", width: 30%");

    // On the plain image.
    let (page, _, [x, y, w, h]) = images[0];
    let jump = doc.jump(page, x + w / 2.0, y + h / 2.0);
    assert_eq!((jump.image_utf16, jump.figure_utf16), (Some(plain), None));
    // On the figure's image, and beside it in the figure's box.
    let (page, _, [x, y, w, h]) = images[1];
    let jump = doc.jump(page, x + w / 2.0, y + h / 2.0);
    let figure = Some(utf16("figure("));
    assert_eq!(
        (jump.image_utf16, jump.figure_utf16),
        (Some(in_figure), figure)
    );
    let jump = doc.jump(page, x - 5.0, y + 5.0);
    assert_eq!(
        (jump.image_utf16, jump.figure_utf16),
        (Some(in_figure), figure)
    );
    // In the text: none.
    assert_eq!(doc.jump(0, x + w / 2.0, 80.0).image_utf16, None);
}

/// Export times and sizes with 20 large images. Run with
/// `cargo test --release -p lethora-typeset --test images -- --ignored --nocapture`.
#[test]
#[ignore]
fn export_timings_with_twenty_large_images() {
    let dir = tempfile::tempdir().unwrap();
    let start = Instant::now();
    let mut content = String::new();
    for i in 0..20 {
        let (name, image) = if i % 2 == 0 {
            (format!("p{i}.jpg"), photo(4000, 3000, i))
        } else {
            (format!("p{i}.png"), photo(4000, 3000, i))
        };
        asset(dir.path(), &name, image);
        content.push_str(&format!(
            "#figure(image(\"assets/{name}\", width: 100%), caption: [Picture {i}.])\n\n#lorem(40)\n\n"
        ));
    }
    println!("writing 20 images of 4000×3000: {:?}", start.elapsed());
    for quality in ["original", "print", "screen", "email"] {
        let start = Instant::now();
        let out = compile_ok(&request(dir.path(), quality, &content));
        let compiled = start.elapsed();
        let doc = out.document.as_ref().unwrap();
        let start = Instant::now();
        let pdf = doc.export_pdf().unwrap();
        println!(
            "{quality:>8}: compile {compiled:?}, export {:?}, PDF {:.1} MB",
            start.elapsed(),
            pdf.len() as f64 / 1e6
        );
    }
}
