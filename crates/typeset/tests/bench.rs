//! Timings and memory of math rendering. Run with
//! `cargo test --release -p lethora-typeset --test bench -- --ignored --nocapture`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use lethora_typeset::{MathRenderer, MathStyle};

/// Counts live heap bytes.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn mib() -> f64 {
    LIVE.load(Ordering::Relaxed) as f64 / (1024.0 * 1024.0)
}

#[test]
#[ignore]
fn math_timings_and_memory() {
    let style = MathStyle {
        size_pt: 16.0,
        color: "#202020".into(),
    };
    let mut renderer = MathRenderer::new();

    let start = Instant::now();
    renderer
        .render_math("x^2 + y^2 = z^2", false, &style)
        .unwrap();
    println!(
        "cold first render: {:?} (live heap {:.1} MiB)",
        start.elapsed(),
        mib()
    );

    let formula = |i: usize| match i % 4 {
        0 => format!("sum_(k=1)^{i} k^2 = frac({i}, 6)"),
        1 => format!("integral_0^{i} e^(-x^2) dif x"),
        2 => format!("mat({i}, 2; 3, 4) vec(a_{i}, b)"),
        _ => format!("sqrt(x_{i} + y^{i}) / (alpha + beta_{i})"),
    };
    let n = 4000;
    let start = Instant::now();
    for i in 0..n {
        renderer
            .render_math(&formula(i), i % 3 == 0, &style)
            .unwrap();
        if (i + 1) % 1000 == 0 {
            println!(
                "after {} distinct renders: live heap {:.1} MiB, {} cached",
                i + 1,
                mib(),
                renderer.cached()
            );
        }
    }
    println!(
        "warm distinct renders: {:?} each",
        start.elapsed() / n as u32
    );

    let start = Instant::now();
    for i in (n - 1000)..n {
        renderer
            .render_math(&formula(i), i % 3 == 0, &style)
            .unwrap();
    }
    println!("cache hits: {:?} each", start.elapsed() / 1000);
}

#[test]
#[ignore]
fn bib_timings() {
    let mut text = String::new();
    for i in 0..500 {
        text.push_str(&format!(
            "@article{{key{i},\n  author = {{Doe, Jane and Roe, Richard}},\n  title = {{Title number {i}}},\n  journal = {{Journal}},\n  year = {{{}}},\n}}\n\n",
            1900 + i % 120
        ));
    }
    let start = Instant::now();
    let lib = lethora_typeset::parse_bib(&text);
    println!(
        "parse 500 entries: {:?} ({} entries)",
        start.elapsed(),
        lib.entries.len()
    );
    text.push_str("@article{broken, title = {x\n");
    let start = Instant::now();
    let lib = lethora_typeset::parse_bib(&text);
    println!(
        "parse 500 entries + 1 broken (per-entry fallback): {:?} ({} errors)",
        start.elapsed(),
        lib.errors.len()
    );
    for style in ["apa", "ieee"] {
        let start = Instant::now();
        lethora_typeset::format_reference(&text, "key250", style).unwrap();
        println!(
            "format_reference ({style}, 500-entry file): {:?}",
            start.elapsed()
        );
    }
}
