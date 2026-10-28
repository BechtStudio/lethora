use std::time::{Duration, Instant};

use lethora_typeset::{MathRenderer, MathStyle, Rendered};

fn style() -> MathStyle {
    MathStyle {
        size_pt: 12.0,
        color: "#000000".into(),
    }
}

fn render(src: &str, display: bool) -> Result<Rendered, String> {
    MathRenderer::new().render_math(src, display, &style())
}

#[test]
fn inline_and_display_render_svg_with_sane_size() {
    for display in [false, true] {
        let r = render("a^2 + b^2 = c^2", display).unwrap();
        assert!(r.svg.starts_with("<svg"), "{}", &r.svg[..80]);
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.width_pt > 20.0 && r.width_pt < 200.0, "{r:?}");
        assert!(r.height_pt > 5.0 && r.height_pt < 30.0, "{r:?}");
    }
    let inline = render("a/b", false).unwrap();
    let display = render("a/b", true).unwrap();
    assert!(
        display.height_pt > inline.height_pt,
        "{inline:?} {display:?}"
    );
}

#[test]
fn baselines_follow_the_formula() {
    let x = render("x", false).unwrap();
    assert!(x.height_pt - x.baseline_pt < 0.5, "x: {x:?}");

    for src in ["x_1", "sum_(i=1)^n"] {
        let r = render(src, false).unwrap();
        assert!(r.height_pt - r.baseline_pt > 1.5, "{src}: {r:?}");
        assert!(r.baseline_pt > 1.0, "{src}: {r:?}");
    }

    let frac = render("a/b", false).unwrap();
    let ratio = frac.baseline_pt / frac.height_pt;
    assert!((0.3..0.8).contains(&ratio), "a/b: {frac:?}");
}

#[test]
fn colour_and_size_apply() {
    let red = MathStyle {
        size_pt: 24.0,
        color: "#ff0000".into(),
    };
    let big = MathRenderer::new().render_math("x", false, &red).unwrap();
    assert!(big.svg.contains("#ff0000"), "{}", big.svg);
    let small = render("x", false).unwrap();
    assert!((big.width_pt / small.width_pt - 2.0).abs() < 0.01);
}

#[test]
fn invalid_formula_gives_readable_error() {
    let e = render("frac(a", false).unwrap_err();
    assert!(e.to_lowercase().contains("unclosed"), "{e}");
    let e = render("undefinedname + 1", false).unwrap_err();
    assert!(e.contains("undefinedname"), "{e}");
}

#[test]
fn common_math_and_safe_code_work() {
    for src in [
        "mat(1, 2; 3, 4)",
        "cases(x \"if\" y > 0, 0 \"otherwise\")",
        "upright(A) + bold(v) + cal(F) + bb(R)",
        "lim_(x -> 0) (sin x) / x = 1",
        "integral_0^oo e^(-x^2) dif x = sqrt(pi) / 2",
        "#text(fill: blue)[x] + #h(1em) y",
        "arrow.r.double + #sym.alpha + std.sym.beta",
        r"x \$ y",
    ] {
        render(src, false).unwrap_or_else(|e| panic!("{src}: {e}"));
    }
}

#[test]
fn untrusted_code_errors_quickly() {
    for src in [
        "#read(\"/etc/passwd\")",
        "#image(\"secret.png\")",
        "#import \"@preview/cetz:0.3.0\": *",
        "#import \"other.typ\"",
        "#include \"other.typ\"",
        "#while true {}",
        "#{ let i = 0; while i >= 0 { i += 1 } }",
        "#for i in range(100000) { for j in range(100000) {} }",
        "#let f(n) = f(n + 1)",
        "#(x => x)(1)",
        "#eval(\"1\")",
        "#std.eval(\"1\")",
        "#dictionary(std).at(\"eval\")",
        "range(1000000000)",
        "#(\"x\" * 1000000000)",
        "#lorem(100000000)",
        "#box(width: 1e9pt, repeat[x])",
        "#grid(columns: 1000000000)[x]",
        "#show \"x\": \"xx\"",
        "#context here()",
    ] {
        let start = Instant::now();
        let result = render(src, false);
        assert!(result.is_err(), "{src} rendered");
        assert!(start.elapsed() < Duration::from_secs(2), "{src} was slow");
    }
}

#[test]
fn formula_cannot_leave_its_delimiters() {
    for src in ["x$ #set page(width: 1pt) $y", "x$", "x\\", "\"$\" $ x"] {
        let e = render(src, false).unwrap_err();
        assert!(!e.is_empty(), "{src}");
    }
    assert!(render(&"x".repeat(lethora_typeset::MAX_SOURCE_LEN + 1), false).is_err());
}

#[test]
fn invalid_style_is_rejected() {
    let mut renderer = MathRenderer::new();
    for color in [
        "red",
        "#12345",
        "#1234567",
        "#12345g",
        "#000000\"); #read(\"x",
        "",
    ] {
        let style = MathStyle {
            size_pt: 12.0,
            color: color.into(),
        };
        let e = renderer.render_math("x", false, &style).unwrap_err();
        assert!(e.contains("#rrggbb"), "{color}: {e}");
    }
    for size in [0.0, -1.0, 1000.0, f64::NAN, f64::INFINITY] {
        let style = MathStyle {
            size_pt: size,
            color: "#000000".into(),
        };
        assert!(renderer.render_math("x", false, &style).is_err(), "{size}");
    }
}

#[test]
fn cache_returns_identical_results_fast() {
    let mut renderer = MathRenderer::new();
    let first = renderer.render_math("sum_(k=1)^n k^2", true, &style());
    let start = Instant::now();
    let second = renderer.render_math("sum_(k=1)^n k^2", true, &style());
    assert!(start.elapsed() < Duration::from_millis(5));
    assert_eq!(first, second);
    assert!(first.is_ok());

    let err = renderer.render_math("frac(", false, &style());
    assert_eq!(err, renderer.render_math("frac(", false, &style()));
    assert_eq!(renderer.cached(), 2);
}

#[test]
fn deep_nesting_renders_or_errors_without_crashing() {
    // Typst's parser stops at depth 256; layout recursion up to there must not
    // overflow the stack (which would abort the process).
    for depth in [40, 250, 300] {
        let nest =
            |open: &str, close: &str| format!("{}x{}", open.repeat(depth), close.repeat(depth));
        for src in [
            nest("(", ")"),
            nest("sqrt(", ")"),
            nest("frac(1, ", ")"),
            nest("x^(", ")"),
            nest("#box[$", "$]"),
            nest("{", "}"),
        ] {
            let result = render(&src, false);
            if depth == 40 {
                result.unwrap_or_else(|e| panic!("{}: {e}", &src[..10]));
            } else if depth == 300 {
                let e = result.unwrap_err();
                assert!(e.contains("depth"), "{e}");
            }
        }
    }
}
