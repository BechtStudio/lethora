//! HTML: one self-contained HTML5 file (UTF-8) with a small readable
//! stylesheet, images as `data:` URLs and formulas as MathML (their Typst
//! source as code where there is none). Headings are `<h2>`–`<h6>` under
//! the title's `<h1>`; labels become ids, so references link to what they
//! name. Footnotes are numbered endnotes with links both ways. The blocks
//! and inlines are the polyglot markup EPUB shares (`xhtml.rs`).

use crate::model::Doc;
use crate::xhtml::{Markup, Target, attr, labels, text};
use crate::{Result, WriteOptions, Writer, Written};

/// The HTML writer.
pub struct Html;

const STYLE: &str = "\
body { max-width: 42em; margin: 2em auto; padding: 0 1em; font-family: Georgia, serif; \
line-height: 1.5; color: #222; background: #fff; }
h1, h2, h3, h4, h5, h6 { line-height: 1.25; }
header { margin-bottom: 2em; text-align: center; }
header p { margin: 0.25em 0; }
figure { margin: 1.5em 0; text-align: center; }
figure img { max-width: 100%; }
figcaption { font-size: 0.9em; margin: 0.5em 0; }
table { border-collapse: collapse; margin: 1em auto; }
th, td { border: 1px solid #999; padding: 0.25em 0.5em; text-align: left; vertical-align: top; }
blockquote { margin: 1em 0; padding-left: 1em; border-left: 3px solid #ccc; color: #444; }
pre { overflow-x: auto; background: #f5f5f5; padding: 0.75em; }
code { font-family: Consolas, monospace; font-size: 0.9em; }
.equation { display: flex; align-items: center; justify-content: center; gap: 2em; margin: 1em 0; }
.footnotes { font-size: 0.9em; border-top: 1px solid #ccc; margin-top: 3em; }
mtable.aligned mtd:nth-child(odd) { text-align: right; padding-right: 0; }
mtable.aligned mtd:nth-child(even) { text-align: left; padding-left: 0; }
";

impl Writer for Html {
    fn write(&self, doc: &Doc, _: &WriteOptions) -> Result<Written> {
        let mut found = Vec::new();
        labels(&doc.blocks, &mut found);
        let targets = found.into_iter().map(|l| (l, String::new())).collect();
        let mut w = Markup::new(doc, Target::Html, targets);
        let mut body = String::new();
        let m = &doc.meta;
        if m.title.is_some() || m.subtitle.is_some() || !m.authors.is_empty() {
            body.push_str("<header>\n");
            if let Some(t) = &m.title {
                body.push_str(&format!("<h1 class=\"title\">{}</h1>\n", text(t)));
            }
            if let Some(s) = &m.subtitle {
                body.push_str(&format!("<p class=\"subtitle\">{}</p>\n", text(s)));
            }
            for a in &m.authors {
                body.push_str(&format!("<p class=\"author\">{}</p>\n", text(a)));
            }
            if let Some(d) = &m.date {
                body.push_str(&format!("<p class=\"date\">{}</p>\n", text(d)));
            }
            body.push_str("</header>\n");
        }
        body.push_str(&w.blocks(&doc.blocks)?);
        body.push_str(&w.notes()?);
        let mut out = String::from("<!DOCTYPE html>\n");
        match &m.lang {
            Some(lang) => out.push_str(&format!("<html lang=\"{}\">\n", attr(lang))),
            None => out.push_str("<html>\n"),
        }
        out.push_str("<head>\n<meta charset=\"utf-8\" />\n");
        out.push_str(
            "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n",
        );
        let title = m.title.as_deref().unwrap_or("Untitled");
        out.push_str(&format!("<title>{}</title>\n", text(title)));
        if !m.authors.is_empty() {
            let authors = m.authors.join(", ");
            out.push_str(&format!(
                "<meta name=\"author\" content=\"{}\" />\n",
                attr(&authors)
            ));
        }
        out.push_str(&format!("<style>\n{STYLE}</style>\n</head>\n<body>\n"));
        out.push_str(&body);
        out.push_str("</body>\n</html>\n");
        Ok(Written {
            bytes: out.into_bytes(),
            files: Vec::new(),
            warnings: w.warnings.into_vec(),
        })
    }
}
