//! Lethora typesetting: single Typst math formulas to SVG, the BibLaTeX
//! sources library (parsing, formatting with CSL styles, importing), the
//! layout layer (settings, presets, generated Typst) and whole-document
//! compilation to pages, SVG and PDF, with anchored overrides injected at
//! compile time and click-to-source.
//!
//! UI-free and independent of the document core.

pub mod assets;
mod bib;
mod compile;
pub mod export;
mod fonts;
pub mod images;
pub mod layout;
mod math;
pub mod overrides;
mod supervisor;
pub mod worker;

pub use bib::{
    BibEntry, BibError, BibLibrary, ImportReport, format_reference, import_bib, parse_bib, styles,
};
pub use compile::{
    CompileOutput, CompileRequest, CompiledDocument, Date, Diagnostic, DocCompiler, JumpResult,
    JumpTarget, Location, MAX_REFERENCES_BYTES, MappedEdit, REFERENCES, Severity, SourceMap,
    cites_sources, compile, compile_source, export_pdf, render_page_svg,
};
pub use layout::{ImageQuality, Layout, Meta, Preamble, PresetInfo, fonts, presets, system_fonts};
pub use math::{MAX_SOURCE_LEN, MathRenderer, MathStyle, Rendered, SIZE_RANGE};
pub use supervisor::{Compiled, Error as WorkerError, FontList, Limits, Supervisor};
