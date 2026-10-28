use lethora_typeset::{format_reference, import_bib, parse_bib, styles};

const SAMPLE: &str = r#"% My sources
@string{ cup = {Cambridge University Press} }

@article{turing1950,
  author  = {Turing, Alan M.},
  title   = {Computing Machinery and Intelligence},
  journal = {Mind},
  volume  = {59},
  number  = {236},
  pages   = {433--460},
  year    = {1950},
  doi     = {10.1093/mind/LIX.236.433},
}

@book{knuth1984,
  author    = {Donald E. Knuth},
  title     = {The {\TeX}book},
  publisher = {Addison-Wesley},
  address   = {Reading, MA},
  year      = {1984},
}

@inproceedings{vaswani2017,
  author    = {Vaswani, Ashish and Shazeer, Noam and Parmar, Niki},
  title     = {Attention Is All You Need},
  booktitle = {Advances in Neural Information Processing Systems},
  pages     = {5998--6008},
  year      = {2017},
}

@book{hardy1938,
  author    = {Hardy, G. H. and Wright, E. M.},
  title     = {An Introduction to the Theory of Numbers},
  publisher = cup,
  date      = {1938},
}
"#;

const MALFORMED: &str = r#"@article{broken,
  author = {Nobody},
  title  = {Unclosed {brace,
  year   = {2001},
"#;

#[test]
fn parses_entries_sorted_by_key() {
    let lib = parse_bib(SAMPLE);
    assert!(lib.errors.is_empty(), "{:?}", lib.errors);
    let keys: Vec<_> = lib.entries.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(
        keys,
        ["hardy1938", "knuth1984", "turing1950", "vaswani2017"]
    );

    let turing = &lib.entries[2];
    assert_eq!(turing.entry_type, "article");
    assert_eq!(
        turing.title.as_deref(),
        Some("Computing Machinery and Intelligence")
    );
    assert_eq!(turing.authors, ["Turing, Alan M."]);
    assert_eq!(turing.year.as_deref(), Some("1950"));

    let knuth = &lib.entries[1];
    assert_eq!(knuth.entry_type, "book");
    assert_eq!(knuth.authors, ["Knuth, Donald E."]);

    let vaswani = &lib.entries[3];
    assert_eq!(vaswani.entry_type, "inproceedings");
    assert_eq!(vaswani.authors.len(), 3);
    assert_eq!(vaswani.year.as_deref(), Some("2017"));

    assert_eq!(lib.entries[0].year.as_deref(), Some("1938"));
}

#[test]
fn malformed_entry_keeps_the_others_and_is_located() {
    let text =
        format!("{SAMPLE}\n{MALFORMED}\n@misc{{after, title = {{Still here}}, year = {{2020}}}}\n");
    let lib = parse_bib(&text);
    let keys: Vec<_> = lib.entries.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "after",
            "hardy1938",
            "knuth1984",
            "turing1950",
            "vaswani2017"
        ]
    );
    assert_eq!(lib.errors.len(), 1, "{:?}", lib.errors);
    let broken_line = text
        .lines()
        .position(|l| l.contains("@article{broken"))
        .unwrap()
        + 1;
    let error = &lib.errors[0];
    assert!(error.line >= broken_line, "{error:?} vs {broken_line}");
    assert!(error.line < broken_line + 6, "{error:?} vs {broken_line}");
    assert!(!error.message.is_empty());
}

#[test]
fn empty_library_is_fine() {
    let lib = parse_bib("");
    assert!(lib.entries.is_empty() && lib.errors.is_empty());
}

#[test]
fn mirror_marker_line_is_ignored() {
    // The first line of the `references.bib` mirror the core writes.
    let marker =
        "% Lethora mirror crc32=0badf00d; outside edits are merged into the Lethora project";
    let lib = parse_bib(&format!("{marker}\n{SAMPLE}"));
    assert!(lib.errors.is_empty(), "{:?}", lib.errors);
    assert_eq!(lib.entries.len(), 4);
}

#[test]
fn formats_in_two_styles() {
    let apa = format_reference(SAMPLE, "turing1950", "apa").unwrap();
    assert!(apa.contains("Turing, A. M."), "{apa}");
    assert!(apa.contains("(1950)"), "{apa}");
    assert!(
        apa.contains("Computing Machinery and Intelligence"),
        "{apa}"
    );

    let ieee = format_reference(SAMPLE, "turing1950", "ieee").unwrap();
    assert!(ieee.contains("A. M. Turing"), "{ieee}");
    assert!(ieee.contains("1950"), "{ieee}");
    assert_ne!(apa, ieee);
    println!(
        "{apa}
{ieee}"
    );

    let book = format_reference(SAMPLE, "hardy1938", "chicago-author-date").unwrap();
    assert!(book.contains("Cambridge University Press"), "{book}");

    assert!(format_reference(SAMPLE, "missing", "apa").is_err());
    assert!(format_reference(SAMPLE, "turing1950", "no-such-style").is_err());
}

#[test]
fn curated_styles_exist_and_format() {
    let list = styles();
    assert!(list.len() >= 8, "{list:?}");
    for (id, name) in list {
        assert!(!name.is_empty());
        format_reference(SAMPLE, "vaswani2017", id).unwrap_or_else(|e| panic!("{id}: {e}"));
    }
}

#[test]
fn import_appends_new_entries_and_skips_duplicates() {
    let incoming = r#"@book{knuth1984,
  author = {Someone Else},
  title = {Different},
  year = {2000},
}

@misc{new2024,
    title = {A   New   Entry},
    author = {Doe, Jane},
    year = {2024},
}
"#;
    let (merged, report) = import_bib(SAMPLE, incoming).unwrap();
    assert_eq!(report.added, ["new2024"]);
    assert_eq!(report.skipped_duplicates, ["knuth1984"]);
    assert!(merged.starts_with(SAMPLE));
    // Raw text kept as written.
    assert!(merged.contains("    title = {A   New   Entry},"));
    assert!(!merged.contains("Someone Else"));

    let lib = parse_bib(&merged);
    assert!(lib.errors.is_empty(), "{:?}", lib.errors);
    assert_eq!(lib.entries.len(), 5);

    // Importing into an empty library; a second import adds nothing.
    let (fresh, report) = import_bib("", SAMPLE).unwrap();
    assert_eq!(report.added.len(), 4);
    assert_eq!(parse_bib(&fresh).entries, parse_bib(SAMPLE).entries);
    let (again, report) = import_bib(&fresh, SAMPLE).unwrap();
    assert!(report.added.is_empty());
    assert_eq!(report.skipped_duplicates.len(), 4);
    assert_eq!(again, fresh);
}

#[test]
fn import_rejects_invalid_text() {
    let e = import_bib(SAMPLE, MALFORMED).unwrap_err();
    assert!(e.contains("line"), "{e}");
    let twice = "@misc{a, title = {A}}
@misc{a, title = {B}}
";
    let e = import_bib(SAMPLE, twice).unwrap_err();
    assert!(e.contains("duplicate key"), "{e}");
}
