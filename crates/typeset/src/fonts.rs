//! The standard library and the embedded fonts, shared by all Typst worlds,
//! and the system fonts for whole documents.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use typst::foundations::Bytes;
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Feature, Library, LibraryExt};
use typst_kit::fonts::FontStore;

/// With Typst's HTML export enabled (exports compile for it, see
/// [`crate::export`]); paged compiles are unaffected.
pub(crate) static LIBRARY: LazyLock<LazyHash<Library>> = LazyLock::new(|| {
    LazyHash::new(
        Library::builder()
            .with_features([Feature::Html].into_iter().collect())
            .build(),
    )
});

pub(crate) struct Fonts {
    pub book: LazyHash<FontBook>,
    pub fonts: Vec<Font>,
}

/// The fonts embedded in `typst-assets`, loaded on first use.
pub(crate) static FONTS: LazyLock<Fonts> = LazyLock::new(|| {
    let fonts: Vec<Font> = typst_assets::fonts()
        .flat_map(|data| Font::iter(Bytes::new(data)))
        .collect();
    Fonts {
        book: LazyHash::new(FontBook::from_fonts(&fonts)),
        fonts,
    }
});

/// Fonts for whole documents: the embedded set (shared, not copied), plus
/// optionally the system's fonts, loaded from disk on first use.
pub(crate) struct DocFonts {
    pub store: FontStore,
    /// How long the system font search took (zero without system fonts).
    pub scan: Duration,
}

/// The embedded fonts only.
pub(crate) static EMBEDDED_DOC_FONTS: LazyLock<DocFonts> = LazyLock::new(|| DocFonts {
    store: embedded_store(),
    scan: Duration::ZERO,
});

/// The embedded fonts, then the system's; searched once, on first use.
pub(crate) static SYSTEM_DOC_FONTS: LazyLock<DocFonts> = LazyLock::new(|| {
    let mut store = embedded_store();
    let start = Instant::now();
    store.extend(typst_kit::fonts::system());
    DocFonts {
        store,
        scan: start.elapsed(),
    }
});

fn embedded_store() -> FontStore {
    let mut store = FontStore::new();
    store.extend(FONTS.fonts.iter().map(|f| (f.clone(), f.info().clone())));
    store
}
