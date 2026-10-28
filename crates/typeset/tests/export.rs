//! Exports: every `fixtures/export/<case>.typ` is compiled for HTML and
//! walked into the convert model, which must equal `<case>.json` next to
//! it (the model and the warnings, pretty JSON; resources by size). The
//! cases cite `references.bib` and show images from `assets/`; a first
//! line `// preset: <id>` picks the layout preset (else `paper`). Every
//! writer takes every case, and each case's EPUB and DOCX pass their
//! structural checks.
//!
//! `UPDATE_GOLDEN=1 cargo test -p lethora-typeset --test export` writes
//! every `.json`; check the diff.

mod common;

use std::fs;
use std::path::Path;

use lethora_convert::zip::Archive;
use lethora_convert::{docx, epub, registry};
use lethora_typeset::layout::{self, Meta};
use lethora_typeset::{CompileRequest, DocCompiler};
use serde_json::Value;

fn request(dir: &Path, root: &Path, content: &str) -> CompileRequest {
    let preset = content
        .lines()
        .next()
        .and_then(|l| l.strip_prefix("// preset: "))
        .unwrap_or("paper");
    for entry in fs::read_dir(root.join("assets")).unwrap() {
        let path = entry.unwrap().path();
        let name = format!("assets/{}", path.file_name().unwrap().to_string_lossy());
        common::put(dir, &name, fs::read(&path).unwrap());
    }
    CompileRequest {
        project_dir: dir.to_owned(),
        layout: layout::preset(preset).unwrap(),
        meta: Meta {
            title: "Export test".into(),
            author: "Ada Lovelace".into(),
            ..Meta::default()
        },
        custom: String::new(),
        content: content.into(),
        today: None,
        overrides: Vec::new(),
        bibliography: fs::read_to_string(root.join("references.bib")).unwrap(),
        assets: common::index(content),
    }
}

/// The model as JSON, resources' bytes as their size.
fn json(imported: &lethora_convert::Imported) -> String {
    let mut value = serde_json::to_value(imported).unwrap();
    for r in value["doc"]["resources"].as_array_mut().unwrap() {
        let len = r["bytes"].as_array().unwrap().len();
        r["bytes"] = Value::String(format!("{len} bytes"));
    }
    serde_json::to_string_pretty(&value).unwrap() + "\n"
}

#[test]
fn golden() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/export");
    let update = std::env::var_os("UPDATE_GOLDEN").is_some();
    let compiler = DocCompiler::new();
    let mut failures = Vec::new();
    let mut cases = 0;
    let mut paths: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "typ"))
        .collect();
    paths.sort();
    for path in paths {
        cases += 1;
        let dir = tempfile::tempdir().unwrap();
        let content = fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
        let request = request(dir.path(), &root, &content);
        let imported = compiler
            .export_model(&request)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        // Every writer takes every model; the EPUB's structure holds.
        for format in registry::FORMATS.iter().filter(|f| f.writer.is_some()) {
            let written = compiler.export(&request, format.id, "out");
            assert!(
                written.is_ok(),
                "{} as {}: {written:?}",
                path.display(),
                format.id
            );
            let check = match format.id {
                "epub" => epub::check,
                "docx" => docx::check,
                _ => continue,
            };
            let problems = check(&written.unwrap().bytes);
            assert!(
                problems.is_empty(),
                "{} as {}: {problems:#?}",
                path.display(),
                format.id
            );
        }
        let actual = json(&imported);
        let expected = path.with_extension("json");
        if update {
            fs::write(&expected, &actual).unwrap();
            continue;
        }
        let want = fs::read_to_string(&expected)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        if want != actual {
            failures.push(format!(
                "{}:\n--- expected\n{want}\n--- actual\n{actual}",
                path.display()
            ));
        }
    }
    assert!(cases > 0, "no fixtures found");
    assert!(
        failures.is_empty(),
        "{} of {cases} cases differ (UPDATE_GOLDEN=1 rewrites them):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// Formulas keep their MathML and get no images unless a writer wants
/// them; HTML shows the MathML.
#[test]
fn formulas_as_mathml() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/export");
    let dir = tempfile::tempdir().unwrap();
    let request = request(dir.path(), &root, "Inline $x^2$.\n\n$ a/b $\n");
    let compiler = DocCompiler::new();
    let model = compiler.export_model(&request).unwrap();
    let json = serde_json::to_string(&model.doc).unwrap();
    assert!(json.contains("\"image\":null"), "no images unless asked");
    assert!(json.contains("<math xmlns="), "MathML kept");
    let html = compiler.export(&request, "html", "out").unwrap();
    let html = String::from_utf8(html.bytes).unwrap();
    assert!(html.contains("<math") && !html.contains("math-1.svg"));
}

/// Word gets formulas as images at their rendered size with PNG twins, the
/// page from the layout (A4 and its margins for `paper`).
#[test]
fn docx_formulas_and_page() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/export");
    let dir = tempfile::tempdir().unwrap();
    let request = request(
        dir.path(),
        &root,
        "Inline $y_1$.\n\n$ a/b $\n\n#image(\"assets/plot.svg\")\n",
    );
    let compiler = DocCompiler::new();
    let written = compiler.export(&request, "docx", "out").unwrap();
    assert!(written.warnings.is_empty(), "{:?}", written.warnings);
    assert_eq!(docx::check(&written.bytes), Vec::<String>::new());
    let zip = Archive::new(&written.bytes).unwrap();
    let names: Vec<&str> = zip.names().collect();
    let svgs = names.iter().filter(|n| n.ends_with(".svg")).count();
    let pngs = names.iter().filter(|n| n.ends_with(".png")).count();
    assert_eq!((svgs, pngs), (3, 3), "{names:?}");
    let body = String::from_utf8(zip.read("word/document.xml").unwrap().unwrap()).unwrap();
    assert!(body.contains("asvg:svgBlip"));
    assert!(body.contains("descr=\"y_1\""), "the source as alt text");
    assert!(
        body.contains("<w:position w:val=\"-"),
        "inline formula lowered"
    );
    // A4 (11906 × 16838 twips), 30 mm side margins (1701 twips).
    assert!(
        body.contains("<w:pgSz w:w=\"11906\" w:h=\"16838\"/>"),
        "{body}"
    );
    assert!(body.contains("w:left=\"1701\""));
    let styles = String::from_utf8(zip.read("word/styles.xml").unwrap().unwrap()).unwrap();
    assert!(styles.contains("w:ascii=\"New Computer Modern\""));
}
