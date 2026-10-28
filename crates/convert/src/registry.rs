//! The formats: what the menus and file dialogs list and how a file finds
//! its reader.

use serde::Serialize;

use crate::{Reader, Writer, docx, epub, html, markdown, text};

/// One format.
pub struct Format {
    /// Stable id (`markdown`), also the fixtures folder's name.
    pub id: &'static str,
    /// For people ("Markdown").
    pub name: &'static str,
    /// Lowercase, without the dot; the first is the one to save with.
    pub extensions: &'static [&'static str],
    /// Whether bytes are of this format whatever the file is called (for
    /// files with an unknown extension).
    pub sniff: fn(&[u8]) -> bool,
    pub reader: Option<&'static dyn Reader>,
    pub writer: Option<&'static dyn Writer>,
    /// Whether the writer wants formulas rendered as images
    /// ([`crate::model::Math::image`]); exports fill them in.
    pub math_images: bool,
    /// Whether the writer wants a PNG twin of every SVG image and formula
    /// ([`crate::model::Resource::fallback`]); exports fill them in.
    pub raster_fallbacks: bool,
}

/// Every format, in the order the open dialog lists them (the save dialog
/// puts Word and EPUB first, [`export_filters`]); sniffing tries them in
/// this order too, so the vaguest (plain text) goes last.
pub static FORMATS: &[Format] = &[
    Format {
        id: "markdown",
        name: "Markdown",
        extensions: &["md", "markdown"],
        sniff: |_| false,
        reader: Some(&markdown::Markdown),
        writer: Some(&markdown::Markdown),
        math_images: false,
        raster_fallbacks: false,
    },
    Format {
        id: "docx",
        name: "Word",
        extensions: &["docx", "docm", "dotx", "dotm"],
        sniff: docx::sniff,
        reader: Some(&docx::Docx),
        writer: Some(&docx::Docx),
        math_images: true,
        raster_fallbacks: true,
    },
    Format {
        id: "text",
        name: "Plain text",
        extensions: &["txt", "text"],
        sniff: text::looks_like_text,
        reader: Some(&text::PlainText),
        writer: Some(&text::PlainText),
        math_images: false,
        raster_fallbacks: false,
    },
    Format {
        id: "html",
        name: "HTML",
        extensions: &["html", "htm"],
        sniff: |_| false,
        reader: None,
        writer: Some(&html::Html),
        math_images: false,
        raster_fallbacks: false,
    },
    Format {
        id: "epub",
        name: "EPUB",
        extensions: &["epub"],
        sniff: |_| false,
        reader: None,
        writer: Some(&epub::Epub),
        math_images: false,
        raster_fallbacks: false,
    },
];

/// The format with id `id`.
pub fn by_id(id: &str) -> Option<&'static Format> {
    FORMATS.iter().find(|f| f.id == id)
}

/// The format to read the file `name` with: by its extension (any case),
/// else the first whose sniffing accepts `bytes`.
pub fn reader_for(name: &str, bytes: &[u8]) -> Option<&'static Format> {
    let readers = || FORMATS.iter().filter(|f| f.reader.is_some());
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .filter(|e| !e.contains(['/', '\\']));
    ext.and_then(|ext| readers().find(|f| f.extensions.contains(&ext.as_str())))
        .or_else(|| readers().find(|f| (f.sniff)(bytes)))
}

/// A file dialog filter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Filter {
    /// The format's id; none for a filter of several formats.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<&'static str>,
    pub name: String,
    pub extensions: Vec<&'static str>,
}

/// The open dialog's filters: every readable format at once first, then one
/// per format.
pub fn import_filters() -> Vec<Filter> {
    let readers: Vec<&Format> = FORMATS.iter().filter(|f| f.reader.is_some()).collect();
    let all = Filter {
        id: None,
        name: "All supported files".into(),
        extensions: readers
            .iter()
            .flat_map(|f| f.extensions.iter().copied())
            .collect(),
    };
    std::iter::once(all)
        .chain(readers.iter().map(|f| filter(f)))
        .collect()
}

/// The formats the save dialog lists first, in this order; the others
/// follow in [`FORMATS`]' order.
const EXPORT_FIRST: &[&str] = &["docx", "epub"];

/// Extensions a writer doesn't write (macro-enabled files and templates).
const READ_ONLY: &[&str] = &["docm", "dotx", "dotm"];

/// The save dialog's filters: one per writable format, Word and EPUB first.
pub fn export_filters() -> Vec<Filter> {
    let mut writers: Vec<&Format> = FORMATS.iter().filter(|f| f.writer.is_some()).collect();
    writers.sort_by_key(|f| {
        EXPORT_FIRST
            .iter()
            .position(|id| *id == f.id)
            .unwrap_or(EXPORT_FIRST.len())
    });
    writers
        .into_iter()
        .map(|f| Filter {
            extensions: f
                .extensions
                .iter()
                .copied()
                .filter(|e| !READ_ONLY.contains(e))
                .collect(),
            ..filter(f)
        })
        .collect()
}

fn filter(f: &Format) -> Filter {
    Filter {
        id: Some(f.id),
        name: f.name.into(),
        extensions: f.extensions.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readers_by_extension_then_content() {
        assert_eq!(reader_for("Notes.MD", b"").unwrap().id, "markdown");
        assert_eq!(reader_for("a.txt", b"# x").unwrap().id, "text");
        assert_eq!(reader_for("README", b"hello").unwrap().id, "text");
        assert_eq!(reader_for("dir.x/file", b"hello").unwrap().id, "text");
        assert!(reader_for("image.bin", b"\x89PNG\x00\x00").is_none());
    }

    #[test]
    fn ids_are_unique_and_filters_list_them() {
        for (i, f) in FORMATS.iter().enumerate() {
            assert!(FORMATS[..i].iter().all(|g| g.id != f.id));
            assert_eq!(by_id(f.id).unwrap().name, f.name);
        }
        let filters = import_filters();
        assert_eq!(
            filters[0].extensions,
            [
                "md", "markdown", "docx", "docm", "dotx", "dotm", "txt", "text"
            ]
        );
        assert_eq!(filters[1].name, "Markdown");
        let ids: Vec<_> = export_filters().iter().map(|f| f.id.unwrap()).collect();
        assert_eq!(ids, ["docx", "epub", "markdown", "text", "html"]);
        assert_eq!(export_filters()[0].extensions, ["docx"]);
    }
}
