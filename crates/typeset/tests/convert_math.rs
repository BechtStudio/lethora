//! Every formula in the import fixtures' expected markup (`crates/convert`)
//! compiles, not only parses.

use std::path::Path;

use lethora_typeset::{MathRenderer, MathStyle};

/// The `$…$` formulas of `text` (outside raw text) and whether each is a
/// display formula (`$ x $`).
fn formulas(text: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut chars = text.chars();
    let mut cur: Option<String> = None;
    let mut in_raw = false;
    while let Some(c) = chars.next() {
        match (c, &mut cur) {
            ('`', None) => in_raw = !in_raw,
            ('\\', Some(s)) => {
                s.push(c);
                s.extend(chars.next());
            }
            ('\\', None) => {
                chars.next();
            }
            ('$', None) if !in_raw => cur = Some(String::new()),
            ('$', Some(s)) => {
                let display = s.starts_with(' ') && s.ends_with(' ');
                out.push((s.trim().to_owned(), display));
                cur = None;
            }
            (_, Some(s)) => s.push(c),
            _ => {}
        }
    }
    out
}

#[test]
fn imported_formulas_compile() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../convert/tests/fixtures");
    let style = MathStyle {
        size_pt: 12.0,
        color: "#000000".into(),
    };
    let mut renderer = MathRenderer::new();
    let mut failures = Vec::new();
    let mut count = 0;
    for format in std::fs::read_dir(&root).unwrap() {
        for file in std::fs::read_dir(format.unwrap().path()).unwrap() {
            let path = file.unwrap().path();
            if path.extension().is_none_or(|e| e != "typ") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for (src, display) in formulas(&text) {
                count += 1;
                if let Err(e) = renderer.render_math(&src, display, &style) {
                    failures.push(format!("{}: `{src}`: {e}", path.display()));
                }
            }
        }
    }
    assert!(count > 10, "found only {count} formulas");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
