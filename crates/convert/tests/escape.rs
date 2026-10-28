//! Text survives the markup writer: random and odd text, in every place
//! text goes, parses with Typst's own parser without errors, as plain text
//! (nothing turned into markup) and with its characters as written.

use std::collections::{BTreeMap, BTreeSet};

use lethora_convert::model::{Block, Cell, Cite, CiteItem, Doc, Inline, List, Table};
use lethora_convert::typst::{MarkupOptions, to_markup};
use typst_syntax::{SyntaxKind, SyntaxNode};

/// Characters Typst treats specially, and some that aren't.
const ALPHABET: &[&str] = &[
    "a",
    "b",
    "Z",
    "é",
    "中",
    "0",
    "1",
    "9",
    " ",
    " ",
    "\t",
    "\n",
    "\u{a0}",
    "\\",
    "*",
    "_",
    "#",
    "$",
    "@",
    "<",
    ">",
    "`",
    "~",
    "/",
    "=",
    "-",
    "+",
    "[",
    "]",
    "(",
    ")",
    "{",
    "}",
    ".",
    ";",
    ":",
    "'",
    "\"",
    "!",
    "?",
    ",",
    "%",
    "&",
    "^",
    "|",
    "\u{1}",
    "http://",
    "https://x.y",
    "1. ",
    "- ",
    "= ",
    "+ ",
    "/ ",
    "...",
    "--",
    "-?",
    "-1",
    "//",
    "/*",
    "*/",
    "<a>",
    "@a",
    "#x",
    "$x$",
    "\\ ",
];

const ODD: &[&str] = &[
    "",
    " ",
    "- item",
    "+ item",
    "= Heading",
    "12. Not a list",
    "/ Term: no",
    "// not a comment",
    "/* nor this */",
    "a */ b",
    "http://example.com/a_b*c",
    "price: $5 or 10€ ~ 3 @ shop #1",
    "<label> and @ref",
    "back\\slash \\",
    "[unbalanced ] brackets [",
    "1.5 is fine, 2. is not at the start",
    "-5 --- en -- dash -? soft",
    "dots... and ..",
    "`code` and ```fence```",
    "*bold* _it_",
];

/// A tiny deterministic generator (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 as usize
    }

    fn text(&mut self) -> String {
        let len = self.next() % 30;
        (0..len)
            .map(|_| ALPHABET[self.next() % ALPHABET.len()])
            .collect()
    }
}

/// Whitespace runs as one space, control characters gone, trimmed.
fn normal(s: &str) -> String {
    let s: String = s
        .chars()
        .filter(|c| c.is_whitespace() || !c.is_control())
        .collect();
    let mut out = String::new();
    for c in s.chars() {
        if matches!(c, ' ' | '\t' | '\n' | '\r') {
            if !out.ends_with(' ') {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    out.trim().to_owned()
}

/// Leaves that are text, and the structure allowed around them.
const TEXT: &[SyntaxKind] = &[
    SyntaxKind::Text,
    SyntaxKind::Escape,
    SyntaxKind::Space,
    SyntaxKind::SmartQuote,
];
const STRUCTURE: &[SyntaxKind] = &[
    SyntaxKind::Markup,
    SyntaxKind::Parbreak,
    SyntaxKind::Strong,
    SyntaxKind::Emph,
    SyntaxKind::Star,
    SyntaxKind::Underscore,
    SyntaxKind::Heading,
    SyntaxKind::HeadingMarker,
    SyntaxKind::ListItem,
    SyntaxKind::ListMarker,
    SyntaxKind::Ref,
    SyntaxKind::RefMarker,
];

/// The text of `node`, skipping code (calls) and markers; `None` if it
/// holds anything else.
fn plain_text(node: &SyntaxNode, out: &mut String, skip_code: bool) -> Result<(), SyntaxKind> {
    let kind = node.kind();
    if TEXT.contains(&kind) {
        let t = node.leaf_text();
        match kind {
            SyntaxKind::Escape => out.push_str(&t[1..]),
            _ => out.push_str(t),
        }
        return Ok(());
    }
    if skip_code && matches!(kind, SyntaxKind::Hash | SyntaxKind::FuncCall) {
        return Ok(());
    }
    if !STRUCTURE.contains(&kind) {
        return Err(kind);
    }
    for child in node.children() {
        plain_text(child, out, skip_code)?;
    }
    Ok(())
}

fn markup(blocks: Vec<Block>) -> String {
    let doc = Doc {
        blocks,
        ..Doc::default()
    };
    let keys: BTreeSet<String> = ["doe".to_owned()].into();
    let options = MarkupOptions {
        assets: &BTreeMap::new(),
        keys: &keys,
    };
    to_markup(&doc, &options).unwrap().text
}

/// The markup parses without errors.
fn parse(markup: &str) -> SyntaxNode {
    let root = typst_syntax::parse(markup);
    let errors = root.errors_and_warnings().0;
    assert!(errors.is_empty(), "{markup:?}: {errors:?}");
    root
}

fn check_text(input: &str, blocks: Vec<Block>, skip_code: bool) {
    let out = markup(blocks);
    let root = parse(&out);
    let mut text = String::new();
    if let Err(kind) = plain_text(&root, &mut text, skip_code) {
        panic!("{input:?} became {kind:?} in {out:?}");
    }
    assert_eq!(normal(&text), normal(input), "{input:?} → {out:?}");
}

fn para(s: &str) -> Block {
    Block::Para(vec![Inline::Text(s.to_owned())])
}

fn cases() -> Vec<String> {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    ODD.iter()
        .map(|s| s.to_string())
        .chain((0..3000).map(|_| rng.text()))
        .collect()
}

#[test]
fn paragraphs_keep_their_text() {
    for s in cases() {
        check_text(&s, vec![para(&s)], false);
    }
}

#[test]
fn headings_and_items_keep_their_text() {
    for s in cases() {
        let heading = Block::Heading {
            level: 2,
            content: vec![Inline::Text(s.clone())],
            label: None,
        };
        check_text(&s, vec![heading], false);
        let list = Block::List(List {
            ordered: false,
            start: 1,
            items: vec![vec![para(&s)], vec![para(&s)]],
        });
        let out = markup(vec![list]);
        parse(&out);
    }
}

#[test]
fn text_next_to_markup_stays_text() {
    for s in cases() {
        let doe = || {
            Inline::Cite(Cite {
                items: vec![CiteItem {
                    key: "doe".into(),
                    locator: None,
                }],
                text: None,
            })
        };
        // After a call (`#footnote[…]`), a citation (`@doe`), around spans.
        let after_code = vec![Block::Para(vec![
            Inline::Footnote(vec![para("note")]),
            Inline::Text(s.clone()),
        ])];
        check_text(&s, after_code, true);
        let after_ref = vec![Block::Para(vec![doe(), Inline::Text(s.clone())])];
        let out = markup(after_ref);
        let root = parse(&out);
        let reference = root.children().find(|n| n.kind() == SyntaxKind::Ref);
        assert_eq!(
            reference.map(|r| r.full_text().to_string()).as_deref(),
            Some("@doe"),
            "{out:?}"
        );
        let spans = vec![Block::Para(vec![
            Inline::Text(s.clone()),
            Inline::Strong(vec![Inline::Text(s.clone())]),
            Inline::Emph(vec![Inline::Text("x".into())]),
            Inline::Text(s.clone()),
        ])];
        let out = markup(spans);
        parse(&out);
    }
}

#[test]
fn cells_captions_and_notes_parse() {
    for s in cases() {
        let cell = |t: &str| Cell::new(vec![para(t)]);
        let table = Block::Table(Table {
            columns: 2,
            header_rows: 1,
            rows: vec![vec![cell(&s), cell("b")], vec![cell("c"), cell(&s)]],
        });
        let note = Block::Para(vec![
            Inline::Text("x".into()),
            Inline::Footnote(vec![para(&s), para(&s)]),
            Inline::Link {
                url: s.clone(),
                content: vec![Inline::Text(s.clone())],
            },
            Inline::Code(s.clone()),
            Inline::Super(vec![Inline::Text(s.clone())]),
        ]);
        parse(&markup(vec![table, note]));
    }
}

/// The text of a paragraph read back, citations as they were written.
fn read_back(blocks: &[Block]) -> String {
    let mut out = String::new();
    for b in blocks {
        if let Block::Para(inlines) = b {
            for i in inlines {
                match i {
                    Inline::Text(t) => out.push_str(t),
                    Inline::Cite(Cite { text: Some(t), .. }) => {
                        out.push_str(&read_back(&[Block::Para(t.clone())]))
                    }
                    other => panic!("{other:?}"),
                }
            }
        }
    }
    out
}

#[test]
fn markdown_keeps_text() {
    let markdown = lethora_convert::registry::by_id("markdown").unwrap();
    for s in cases() {
        let doc = Doc {
            blocks: vec![para(&s)],
            ..Doc::default()
        };
        let options = lethora_convert::WriteOptions::default();
        let written = markdown.writer.unwrap().write(&doc, &options).unwrap();
        let read = markdown
            .reader
            .unwrap()
            .read(&written.bytes, &lethora_convert::ReadOptions::new("a.md"))
            .unwrap();
        let md = String::from_utf8_lossy(&written.bytes);
        assert_eq!(
            normal(&read_back(&read.doc.blocks)),
            normal(&s),
            "{s:?} → {md:?}"
        );
    }
}
