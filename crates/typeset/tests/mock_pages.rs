//! Pre-renders the browser preview's sample (`app/src/sample.typ`) with the
//! layout and overrides the mock backend (`app/src/devmock.ts`) starts with,
//! so its Publisher view shows real pages. Regenerate after changing the
//! sample, the mock's initial layout or the typesetting with
//! `cargo test -p lethora-typeset --test mock_pages -- --ignored`.
//! Writes `app/src/fixtures/pages/page-N.svg` and `pages.json` (page sizes
//! in points and the overrides' places).

use std::fs;
use std::path::Path;

mod common;

use image::{ImageFormat, Rgb, RgbImage};
use lethora_typeset::layout::{self, Entries, Length, Meta, TableStyle};
use lethora_typeset::overrides::{Kind, Placement, ResolvedOverride};
use lethora_typeset::{CompileRequest, Severity, compile};
use serde_json::json;

/// The mock's sources (`bib` in devmock.ts), as BibLaTeX, plus `smith2020`:
/// the mock warns that it is missing, a real compile fails.
const REFERENCES: &str = r"@book{admiralty1898, author = {{Admiralty, Hydrographic Department}}, title = {The Admiralty List of Lights: British Isles}, year = {1898}}
@online{elliot2023, author = {Elliot, Hannah}, title = {Light and Memory on the Northern Coast}, year = {2023}}
@article{fresnel1822, author = {Fresnel, Augustin}, title = {Mémoire sur un nouveau système d'éclairage des phares}, year = {1822}}
@book{hague1975, author = {Hague, Douglas B. and Christie, Rosemary}, title = {Lighthouses: Their Architecture, History and Archaeology}, year = {1975}}
@book{levitt2013, author = {Levitt, Theresa}, title = {A Short Bright Flash: Augustin Fresnel and the Birth of the Modern Lighthouse}, year = {2013}}
@book{stevenson1959, author = {Stevenson, D. Alan}, title = {The World's Lighthouses Before 1820}, year = {1959}}
@book{woolf1927, author = {Woolf, Virginia}, title = {To the Lighthouse}, year = {1927}}
@article{smith2020, author = {Smith, Jane}, title = {Notes on Coastal Weather}, year = {2020}}
";

/// The mock's generated `assets/lighthouse.png` (`assetBytes`), drawn again.
fn lighthouse() -> RgbImage {
    let (w, h) = (640u32, 360u32);
    let mix = |a: [u8; 3], b: [u8; 3], t: f32| {
        Rgb([0, 1, 2].map(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8))
    };
    RgbImage::from_fn(w, h, |x, y| {
        let (xf, yf) = (x as f32, y as f32);
        let in_rect = |x0: f32, y0: f32, rw: f32, rh: f32| {
            xf >= x0 && xf < x0 + rw && yf >= y0 && yf < y0 + rh
        };
        if in_rect(0.0, 300.0, 640.0, 60.0) {
            return Rgb([0x1d, 0x3b, 0x4f]);
        }
        if in_rect(298.0, 88.0, 54.0, 24.0) {
            return Rgb([0xb0, 0x3a, 0x2e]);
        }
        if in_rect(305.0, 110.0, 40.0, 200.0) {
            return Rgb([0xf2, 0xef, 0xe8]);
        }
        let sky = mix([0x2b, 0x2a, 0x4a], [0xd9, 0x8a, 0x5f], yf / h as f32);
        // The beam: from (330, 110) to the right edge between y 40 and 190.
        let t = (xf - 330.0) / 310.0;
        if t >= 0.0 && yf >= 110.0 - 70.0 * t && yf <= 110.0 + 80.0 * t {
            return mix(sky.0, [255, 230, 150], 0.45);
        }
        sky
    })
}

#[test]
#[ignore]
fn mock_pages() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/src");
    let content = fs::read_to_string(root.join("sample.typ")).unwrap();
    let project = tempfile::tempdir().unwrap();
    fs::create_dir(project.path().join("assets")).unwrap();
    let picture = lighthouse();
    picture
        .save_with_format(
            project
                .path()
                .join("assets")
                .join(common::blob("assets/lighthouse.png")),
            ImageFormat::Png,
        )
        .unwrap();
    // Missing in the mock (a warning there), but an error in a real compile:
    // a pale grey copy stands in.
    let mut sketch = picture;
    for p in sketch.pixels_mut() {
        let grey = (p.0.iter().map(|&c| c as u32).sum::<u32>() / 3) as u8;
        *p = Rgb([grey / 2 + 128; 3]);
    }
    sketch
        .save_with_format(
            project
                .path()
                .join("assets")
                .join(common::blob("assets/sketch.png")),
            ImageFormat::Png,
        )
        .unwrap();

    // devmock.ts: `layoutDoc` and `sampleOverrides` (the invalid one left out).
    let entries: Entries = [
        ("preset".to_owned(), json!("novel")),
        ("text.size".to_owned(), json!("11.5pt")),
        ("headings.numbering".to_owned(), json!("1.1.")),
        ("equations.numbering".to_owned(), json!(true)),
    ]
    .into();
    let at = |id: &str, kind, needle: &str, detached| ResolvedOverride {
        id: id.into(),
        kind,
        content_byte_offset: content.find(needle).expect(needle),
        detached,
    };
    let overrides = vec![
        at(
            "sp1",
            Kind::SpaceBefore {
                length: Length::parse("18pt").unwrap(),
            },
            "== The Arithmetic of Light",
            false,
        ),
        at(
            "fp1",
            Kind::FigurePlacement {
                placement: Placement::Top,
            },
            "figure(",
            false,
        ),
        at(
            "ts1",
            Kind::TableStyle {
                style: TableStyle::Grid,
            },
            "table(",
            false,
        ),
        at("cb1", Kind::ColumnBreak, "The keepers before them", true),
        at("pb2", Kind::PageBreak { to_odd: false }, "  = 1/3 $", false),
        at(
            "pb1",
            Kind::PageBreak { to_odd: true },
            "=== A Few Formulas",
            false,
        ),
    ];
    let assets = common::index(&content);
    let request = CompileRequest {
        project_dir: project.path().to_path_buf(),
        layout: layout::resolve(&entries),
        meta: Meta {
            title: "The Lighthouse Keeper's Daughter".into(),
            author: "M. Ashdown".into(),
            ..Meta::default()
        },
        custom: String::new(),
        content,
        today: None,
        assets,
        overrides,
        bibliography: REFERENCES.into(),
    };
    let out = compile(&request);
    for d in &out.diagnostics {
        println!("{:?}: {} ({:?})", d.severity, d.message, d.location);
    }
    assert!(
        !out.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    );
    let document = out.document.as_ref().expect("pages");

    let dir = root.join("fixtures/pages");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for i in 0..out.pages {
        let svg = document.render_page_svg(i).unwrap();
        fs::write(dir.join(format!("page-{}.svg", i + 1)), svg).unwrap();
    }
    let json = json!({ "page_sizes": out.page_sizes, "overrides": out.overrides });
    fs::write(
        dir.join("pages.json"),
        serde_json::to_string_pretty(&json).unwrap() + "\n",
    )
    .unwrap();
    println!("{} pages written to {}", out.pages, dir.display());
}
