//! The sources library: a BibLaTeX file read, formatted and extended with
//! hayagriva.
//!
//! hayagriva parses a file all-or-nothing, so when the whole file fails, each
//! entry is parsed on its own (with the file's `@string` abbreviations) to keep
//! the good entries and locate the bad ones. Entries are found by a small
//! splitter that mirrors the top level of the `biblatex` parser; it also gives
//! the raw text of each entry, so imports keep the user's formatting.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::LazyLock;

use hayagriva::archive::ArchivedStyle;
use hayagriva::citationberg::{Locale, LocaleCode, Style};
use hayagriva::io::{BibLaTeXError, from_biblatex_str};
use hayagriva::{
    BibliographyDriver, BibliographyRequest, BufWriteFormat, CitationItem, CitationRequest, Entry,
};
use serde::Serialize;

/// Curated CSL styles offered in the UI: (archive name, display name).
const STYLES: &[&str] = &[
    "apa",
    "chicago-author-date",
    "modern-language-association",
    "ieee",
    "harvard-cite-them-right",
    "vancouver",
    "american-medical-association",
    "nature",
    "association-for-computing-machinery",
    "springer-basic-author-date",
];

/// One source in the library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BibEntry {
    pub key: String,
    /// The BibLaTeX entry type as written, lowercased (`article`, `book`, ...).
    pub entry_type: String,
    pub title: Option<String>,
    /// Authors as "Family, Given".
    pub authors: Vec<String>,
    pub year: Option<String>,
}

/// A problem in the library file. Line and column are 1-based; the column
/// counts characters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BibError {
    pub message: String,
    pub line: usize,
    pub column: usize,
}

/// The readable part of a library file: entries sorted by key, plus errors
/// for the entries that could not be read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct BibLibrary {
    pub entries: Vec<BibEntry>,
    pub errors: Vec<BibError>,
}

/// What [`import_bib`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ImportReport {
    pub added: Vec<String>,
    /// Keys already in the library; not imported.
    pub skipped_duplicates: Vec<String>,
}

/// Reads a BibLaTeX library. Never fails: unreadable entries are reported in
/// [`BibLibrary::errors`] and left out.
pub fn parse_bib(text: &str) -> BibLibrary {
    let (loaded, errors) = load(text);
    let mut entries: Vec<BibEntry> = loaded
        .iter()
        .map(|(entry, kind)| summarize(entry, kind))
        .collect();
    entries.sort_by(|a, b| a.key.cmp(&b.key));
    BibLibrary { entries, errors }
}

/// The curated citation styles as (id, display name), for [`format_reference`].
pub fn styles() -> Vec<(&'static str, &'static str)> {
    STYLES
        .iter()
        .filter_map(|&name| ArchivedStyle::by_name(name).map(|s| (name, s.display_name())))
        .collect()
}

/// Formats the entry `key` of the library `text` as a plain-text
/// bibliography entry in `style` (an archive name, see [`styles`]), en-US.
pub fn format_reference(text: &str, key: &str, style: &str) -> Result<String, String> {
    let (loaded, _) = load(text);
    let entry = loaded
        .iter()
        .map(|(entry, _)| entry)
        .find(|entry| entry.key() == key)
        .ok_or_else(|| format!("No readable source with key \"{key}\"."))?;
    let style = match ArchivedStyle::by_name(style).map(ArchivedStyle::get) {
        Some(Style::Independent(style)) => style,
        Some(Style::Dependent(_)) | None => return Err(format!("Unknown style \"{style}\".")),
    };

    let mut driver = BibliographyDriver::new();
    driver.citation(CitationRequest::from_items(
        vec![CitationItem::with_entry(entry)],
        &style,
        &LOCALES,
    ));
    let rendered = driver.finish(BibliographyRequest::new(
        &style,
        Some(LocaleCode::en_us()),
        &LOCALES,
    ));
    let item = rendered
        .bibliography
        .and_then(|b| b.items.into_iter().next())
        .ok_or("This style has no bibliography.")?;

    // The item's first field (a label such as "[1]") is left out: it is
    // meaningless for a single reference.
    let mut out = String::new();
    item.content
        .write_buf(&mut out, BufWriteFormat::Plain)
        .map_err(|e| e.to_string())?;
    Ok(out.trim().to_owned())
}

/// Merges the BibLaTeX text `incoming` into the library text `existing`.
/// `incoming` must parse completely. Entries whose key exists already are
/// skipped; new entries (and `@string` abbreviations not yet defined) are
/// appended as written.
pub fn import_bib(existing: &str, incoming: &str) -> Result<(String, ImportReport), String> {
    if let Err(errors) = from_biblatex_str(incoming) {
        let error = errors
            .first()
            .map(|e| error_at(incoming, span_start(e), message(e)))
            .unwrap_or_else(|| BibError {
                message: "unreadable".into(),
                line: 1,
                column: 1,
            });
        return Err(format!(
            "The imported text is not valid BibLaTeX (line {}, column {}): {}.",
            error.line, error.column, error.message
        ));
    }

    let mut keys = HashSet::new();
    let mut strings = HashSet::new();
    for raw in split(existing) {
        match raw.kind.as_str() {
            "string" => {
                strings.insert(string_name(&existing[raw.range]));
            }
            "comment" | "preamble" => {}
            _ => {
                keys.insert(raw.key);
            }
        }
    }

    let mut out = existing.to_owned();
    let mut report = ImportReport::default();
    for raw in split(incoming) {
        let chunk = &incoming[raw.range.clone()];
        match raw.kind.as_str() {
            "comment" | "preamble" => continue,
            "string" => {
                if !strings.insert(string_name(chunk)) {
                    continue;
                }
            }
            _ if !keys.insert(raw.key.clone()) => {
                report.skipped_duplicates.push(raw.key);
                continue;
            }
            _ => report.added.push(raw.key),
        }
        if !out.is_empty() {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
        }
        out.push_str(chunk);
        out.push('\n');
    }
    Ok((out, report))
}

static LOCALES: LazyLock<Vec<Locale>> = LazyLock::new(hayagriva::archive::locales);

/// Parses `text` into hayagriva entries with their raw entry type.
fn load(text: &str) -> (Vec<(Entry, String)>, Vec<BibError>) {
    let raws = split(text);
    let kind_of = |key: &str| {
        raws.iter()
            .find(|raw| raw.key == key)
            .map(|raw| raw.kind.clone())
            .unwrap_or_default()
    };
    if let Ok(library) = from_biblatex_str(text) {
        let entries = library
            .iter()
            .map(|entry| (entry.clone(), kind_of(entry.key())))
            .collect();
        return (entries, Vec::new());
    }

    // Parse entry by entry, each with all `@string` definitions in front.
    let mut prefix = String::new();
    for raw in raws.iter().filter(|raw| raw.kind == "string") {
        prefix.push_str(&text[raw.range.clone()]);
        prefix.push('\n');
    }
    let mut entries = Vec::new();
    let mut errors = Vec::new();
    let mut seen = HashSet::new();
    for raw in &raws {
        if matches!(raw.kind.as_str(), "string" | "comment" | "preamble") {
            continue;
        }
        let chunk = format!("{prefix}{}", &text[raw.range.clone()]);
        match from_biblatex_str(&chunk) {
            Ok(library) => {
                for entry in library.iter() {
                    if seen.insert(entry.key().to_owned()) {
                        entries.push((entry.clone(), raw.kind.clone()));
                    } else {
                        errors.push(error_at(
                            text,
                            raw.range.start,
                            format!("duplicate key \"{}\"", entry.key()),
                        ));
                    }
                }
            }
            Err(errs) => {
                let start = raw.range.start;
                errors.extend(errs.iter().map(|e| {
                    // Errors inside the `@string` prefix are blamed on the entry.
                    let offset = span_start(e)
                        .checked_sub(prefix.len())
                        .map_or(start, |o| start + o);
                    // The chunk ends where the entry does, not the file.
                    let message = message(e).replace("end of file", "end of entry");
                    let message = if raw.key.is_empty() {
                        message
                    } else {
                        format!("{}: {message}", raw.key)
                    };
                    error_at(text, offset, message)
                }));
            }
        }
    }
    (entries, errors)
}

fn summarize(entry: &Entry, kind: &str) -> BibEntry {
    let year = std::iter::successors(Some(entry), |e| e.parents().first())
        .find_map(Entry::date)
        .map(|date| date.year.to_string());
    BibEntry {
        key: entry.key().to_owned(),
        entry_type: if kind.is_empty() {
            format!("{:?}", entry.entry_type()).to_lowercase()
        } else {
            kind.to_owned()
        },
        title: entry.title().map(|t| t.to_string()),
        authors: entry
            .authors()
            .unwrap_or_default()
            .iter()
            .map(|p| p.name_first(false, false))
            .collect(),
        year,
    }
}

fn span_start(error: &BibLaTeXError) -> usize {
    match error {
        BibLaTeXError::Parse(e) => e.span.start,
        BibLaTeXError::Type(e) => e.span.start,
    }
}

fn message(error: &BibLaTeXError) -> String {
    match error {
        BibLaTeXError::Parse(e) => e.kind.to_string(),
        BibLaTeXError::Type(e) => e.kind.to_string(),
    }
}

fn error_at(text: &str, offset: usize, message: String) -> BibError {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &text[..offset];
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    BibError {
        message,
        line: before.matches('\n').count() + 1,
        column: before[line_start..].chars().count() + 1,
    }
}

/// The name an `@string{name = ...}` defines, lowercased.
fn string_name(chunk: &str) -> String {
    let body = chunk.split_once('{').map_or("", |(_, rest)| rest);
    body.split('=')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

/// An entry found by [`split`].
struct RawEntry {
    /// Entry type, lowercased.
    kind: String,
    /// The citation key (empty for `@string`, `@comment`, `@preamble`).
    key: String,
    /// The entry's text, from `@` to the closing brace (or to where the next
    /// entry starts, if it is unterminated).
    range: Range<usize>,
}

/// Finds the entries in `text` like the top level of the `biblatex` parser:
/// whitespace is skipped, `%` starts a line comment, `@` an entry, anything
/// else is ignored. An entry ends at its matching closing brace, or early at
/// a line that starts another entry (so one broken entry can't swallow the
/// rest).
fn split(text: &str) -> Vec<RawEntry> {
    let bytes = text.as_bytes();
    let mut entries = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => i = text[i..].find('\n').map_or(bytes.len(), |n| i + n + 1),
            b'@' => {
                let start = i;
                let end = entry_end(text, start);
                entries.push(raw_entry(text, start..end));
                i = end.max(start + 1);
            }
            _ => i += 1,
        }
    }
    entries
}

fn entry_end(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return i + 1;
                }
            }
            b'\n' if depth > 0 && starts_entry(&text[i + 1..]) => return i,
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}

/// Whether `rest` starts (after indentation) with `@type{`.
fn starts_entry(rest: &str) -> bool {
    let rest = rest.trim_start_matches([' ', '\t']);
    let Some(rest) = rest.strip_prefix('@') else {
        return false;
    };
    let name_len = rest
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(rest.len());
    name_len > 0 && rest[name_len..].trim_start().starts_with('{')
}

fn raw_entry(text: &str, range: Range<usize>) -> RawEntry {
    let chunk = &text[range.clone()];
    let after_at = &chunk[1..];
    let kind_len = after_at
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(after_at.len());
    let kind = after_at[..kind_len].to_ascii_lowercase();
    let key = match kind.as_str() {
        "string" | "comment" | "preamble" => String::new(),
        _ => after_at[kind_len..]
            .split_once('{')
            .map(|(_, rest)| rest)
            .unwrap_or("")
            .split([',', '}'])
            .next()
            .unwrap_or("")
            .trim()
            .to_owned(),
    };
    RawEntry { kind, key, range }
}
