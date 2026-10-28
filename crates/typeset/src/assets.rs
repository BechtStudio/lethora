//! Project assets (figure images): the asset types by extension, the paths
//! the text uses (`assets/...`) and folder paths (`assets/figures/`), a safe,
//! unique path for an imported file, reading the file the user picked, and
//! reading blobs. The document's asset index (`lethora-core`, "Assets") maps
//! each path to an immutable blob `<project>/assets/<blob>`; a read can never
//! reach outside the assets folder, and nothing here writes into a project.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

/// The assets folder inside a project.
pub const DIR: &str = "assets";

/// Image formats Typst 0.15 reads by extension (lowercase; Typst matches the
/// extension case-sensitively).
pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "svg", "webp"];

/// Largest image the editor imports or loads.
pub const MAX_BYTES: u64 = 50 * 1024 * 1024;

/// The kind of images in [`TYPES`].
pub const IMAGE: &str = "image";

/// A kind of asset, recognized by extension, and its import check. The web
/// view has the same kinds in `app/src/assettypes.ts` (label, icon, preview,
/// inserted snippet).
#[derive(Debug, PartialEq, Eq)]
pub struct AssetType {
    /// `image`; data files (for graphs) can be added later.
    pub kind: &'static str,
    /// Lowercase extensions (Typst matches them case-sensitively).
    pub extensions: &'static [&'static str],
    /// Largest file the editor imports or loads.
    pub max_bytes: u64,
}

/// Every asset type the editor knows.
pub const TYPES: &[AssetType] = &[AssetType {
    kind: IMAGE,
    extensions: IMAGE_EXTENSIONS,
    max_bytes: MAX_BYTES,
}];

const MAX_STEM: usize = 60;

const WINDOWS_RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// A file name stem made safe for every platform and for Typst paths:
/// lowercase ASCII letters, digits, `-` and `_`; other runs become one `-`.
pub fn sanitize_stem(stem: &str) -> String {
    let mut out = String::new();
    for c in stem.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut out: String = out
        .trim_matches(['-', '_'])
        .chars()
        .take(MAX_STEM)
        .collect();
    while out.ends_with(['-', '_']) {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("image");
    }
    if WINDOWS_RESERVED.contains(&out.as_str()) {
        out.push_str("-file");
    }
    out
}

/// The type of `path` by its extension (any case), and that extension in
/// lowercase.
pub fn asset_type(path: &Path) -> Option<(&'static AssetType, String)> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let t = TYPES
        .iter()
        .find(|t| t.extensions.contains(&ext.as_str()))?;
    Some((t, ext))
}

/// The lowercase image extension of `path`, if it is a supported format.
pub fn image_extension(path: &Path) -> Option<String> {
    asset_type(path)
        .filter(|(t, _)| t.kind == IMAGE)
        .map(|(_, ext)| ext)
}

/// The import check of a file named `name` of `len` bytes: a known type and
/// at most its `max_bytes`. Returns the type and the lowercase extension.
pub fn check_import(name: &Path, len: u64) -> Result<(&'static AssetType, String), String> {
    let (t, ext) = asset_type(name).ok_or_else(unsupported)?;
    if len > t.max_bytes {
        return Err(too_large(t));
    }
    Ok((t, ext))
}

fn unsupported() -> String {
    let all: Vec<&str> = TYPES.iter().flat_map(|t| t.extensions).copied().collect();
    format!("Unsupported file type. Use one of: {}.", all.join(", "))
}

fn too_large(t: &AssetType) -> String {
    format!(
        "The {} is larger than {} MB.",
        t.kind,
        t.max_bytes / (1024 * 1024)
    )
}

/// The `n`-th candidate name: `stem.ext`, then `stem-2.ext`, `stem-3.ext`, ...
pub fn candidate(stem: &str, ext: &str, n: u32) -> String {
    if n <= 1 {
        format!("{stem}.{ext}")
    } else {
        format!("{stem}-{n}.{ext}")
    }
}

/// Whether image extensions `a` and `b` (lowercase) name the same format;
/// Typst picks the decoder by extension, so only a path of the same format
/// can be reused for an image.
pub fn same_format(a: &str, b: &str) -> bool {
    let canonical = |e: &str| if e == "jpeg" { "jpg" } else { e }.to_owned();
    canonical(a) == canonical(b)
}

/// Reads the file `source` (picked by the user) with the import check of its
/// type ([`check_import`]): a known extension, a file, at most the type's
/// `max_bytes`. Returns its lowercase extension and its bytes.
pub fn read_source(source: &Path) -> Result<(String, Vec<u8>), String> {
    let (t, ext) = asset_type(source).ok_or_else(unsupported)?;
    let cant = |e: io::Error| format!("Could not read {}: {e}", source.display());
    let file = File::open(source).map_err(cant)?;
    let meta = file.metadata().map_err(cant)?;
    if !meta.is_file() {
        return Err(format!("{} is not a file.", source.display()));
    }
    check_import(source, meta.len())?;
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(t.max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(cant)?;
    check_import(source, bytes.len() as u64)?;
    Ok((ext, bytes))
}

/// The path to import `source` under, in `folder` (`assets/` or a folder
/// below it, see [`validate_folder`]): `<folder><stem>.<ext>` with the stem
/// of its file name made safe ([`sanitize_stem`]), else
/// `<folder><stem>-2.<ext>`, `-3`, ... while `taken` (by the document's asset
/// index).
pub fn import_path(
    source: &Path,
    ext: &str,
    folder: &str,
    taken: impl Fn(&str) -> bool,
) -> Result<String, String> {
    validate_folder(folder)?;
    let stem = sanitize_stem(&source.file_stem().unwrap_or_default().to_string_lossy());
    (1..10_000)
        .map(|n| format!("{folder}{}", candidate(&stem, ext, n)))
        .find(|path| !taken(path))
        .ok_or_else(|| "Too many files with this name.".to_owned())
}

/// Checks the shape of a project-relative asset path: `assets/` plus
/// components separated by `/` and the extension of a known type. Names are
/// free (spaces, Unicode, case; they never reach the disk), but a component
/// is never empty, `.` or `..`, never starts or ends with a space or dot and
/// holds no `"`, backslash, colon or control character: no drive, stream,
/// absolute or UNC path, and the path quoted in the text is one plain literal.
pub fn validate_relative(rel: &str) -> Result<(), String> {
    if !valid_parts(rel) || !rel.contains('/') || asset_type(Path::new(rel)).is_none() {
        return Err(format!("Not an asset path: \"{rel}\"."));
    }
    Ok(())
}

/// Checks the shape of an asset folder path: `assets/` (the root), or
/// components as in [`validate_relative`] after it, ending in `/` (e.g.
/// `assets/figures/ch1/`).
pub fn validate_folder(rel: &str) -> Result<(), String> {
    match rel.strip_suffix('/') {
        Some(inner) if valid_parts(inner) => Ok(()),
        _ => Err(format!("Not an asset folder: \"{rel}\".")),
    }
}

/// Checks that the item `old` may be renamed or moved to `new`: a valid path
/// with an extension of the same format, in lowercase (Typst matches
/// extensions case-sensitively).
pub fn validate_rename(old: &str, new: &str) -> Result<(), String> {
    validate_relative(new)?;
    let (t, old_ext) =
        asset_type(Path::new(old)).ok_or_else(|| format!("Not an asset path: \"{old}\"."))?;
    let ext = Path::new(new).extension().and_then(|e| e.to_str());
    if !ext.is_some_and(|e| e == e.to_ascii_lowercase() && same_format(e, &old_ext)) {
        return Err(format!(
            "A renamed {} keeps its format: end the name with .{old_ext}.",
            t.kind
        ));
    }
    Ok(())
}

/// Whether `rel` is `assets`, then valid components (see
/// [`validate_relative`]), at most 1024 bytes.
fn valid_parts(rel: &str) -> bool {
    let mut parts = rel.split('/');
    rel.len() <= 1024
        && parts.next() == Some(DIR)
        && parts.all(|p| {
            !p.is_empty()
                && !p.starts_with([' ', '.'])
                && !p.ends_with([' ', '.'])
                && !p
                    .chars()
                    .any(|c| matches!(c, '"' | '\\' | ':') || c.is_control())
        })
}

/// Whether `s` names a blob: 64 lowercase hex characters.
pub fn is_blob_name(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The bytes of blob `blob` in `<project>/assets/` (checked again after
/// resolving links), at most [`MAX_BYTES`]; `rel` is the path the text uses,
/// for messages.
pub fn read_blob(project: &Path, blob: &str, rel: &str) -> Result<Vec<u8>, String> {
    let denied = || format!("Not an image blob: \"{blob}\".");
    if !is_blob_name(blob) {
        return Err(denied());
    }
    let not_found = || format!("Image not found: {rel}");
    let root = fs::canonicalize(project.join(DIR)).map_err(|_| not_found())?;
    let path = fs::canonicalize(root.join(blob)).map_err(|_| not_found())?;
    if !path.starts_with(&root) || path == root {
        return Err(denied());
    }
    let file = File::open(&path).map_err(|e| format!("Could not read {rel}: {e}"))?;
    let meta = file
        .metadata()
        .map_err(|e| format!("Could not read {rel}: {e}"))?;
    if !meta.is_file() {
        return Err(not_found());
    }
    let too_large = || format!("{rel} is larger than {} MB.", MAX_BYTES / (1024 * 1024));
    if meta.len() > MAX_BYTES {
        return Err(too_large());
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Could not read {rel}: {e}"))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(too_large());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A fresh, empty folder under the system temp dir.
    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lethora-assets-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn stems_are_sanitized() {
        assert_eq!(
            sanitize_stem("Lighthouse at Dusk (2)"),
            "lighthouse-at-dusk-2"
        );
        assert_eq!(sanitize_stem("Café—über"), "caf-ber");
        assert_eq!(sanitize_stem("../../etc/passwd"), "etc-passwd");
        assert_eq!(sanitize_stem("  ...  "), "image");
        assert_eq!(sanitize_stem("CON"), "con-file");
        assert_eq!(sanitize_stem("snake_case-name"), "snake_case-name");
        assert_eq!(sanitize_stem(&"a".repeat(200)).len(), MAX_STEM);
        assert_eq!(
            sanitize_stem(&format!("{}-b", "a".repeat(59))),
            "a".repeat(59)
        );
    }

    #[test]
    fn extensions() {
        assert_eq!(
            image_extension(Path::new("a/B.PNG")).as_deref(),
            Some("png")
        );
        assert_eq!(
            image_extension(Path::new("x.jpeg")).as_deref(),
            Some("jpeg")
        );
        assert_eq!(
            image_extension(Path::new("x.webp")).as_deref(),
            Some("webp")
        );
        assert_eq!(image_extension(Path::new("x.pdf")), None);
        assert_eq!(image_extension(Path::new("x.exe")), None);
        assert_eq!(image_extension(Path::new("png")), None);
    }

    #[test]
    fn import_paths_are_unique_against_the_index() {
        let taken = ["assets/fig.png", "assets/fig-2.png", "assets/ch 1/fig.png"];
        let path = |source: &str, ext: &str, folder: &str| {
            import_path(Path::new(source), ext, folder, |p| taken.contains(&p))
        };
        assert_eq!(
            path("C:/x/Fig.PNG", "png", "assets/").unwrap(),
            "assets/fig-3.png"
        );
        assert_eq!(path("fig.svg", "svg", "assets/").unwrap(), "assets/fig.svg");
        assert_eq!(
            path("My Photo.jpeg", "jpeg", "assets/").unwrap(),
            "assets/my-photo.jpeg"
        );
        // Into a folder: unique within it.
        assert_eq!(
            path("Fig.png", "png", "assets/ch 1/").unwrap(),
            "assets/ch 1/fig-2.png"
        );
        assert_eq!(
            path("fig.png", "png", "assets/ch 2/").unwrap(),
            "assets/ch 2/fig.png"
        );
        for bad in ["", "assets", "other/", "assets/../", "assets/a"] {
            assert!(path("fig.png", "png", bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn types_come_from_the_table() {
        let (t, ext) = asset_type(Path::new("assets/a.JPEG")).unwrap();
        assert_eq!((t.kind, ext.as_str()), (IMAGE, "jpeg"));
        assert_eq!(t.max_bytes, MAX_BYTES);
        assert!(asset_type(Path::new("data.csv")).is_none());
        assert!(asset_type(Path::new("png")).is_none());
        // Every extension is lowercase and belongs to one type.
        let all: Vec<&str> = TYPES.iter().flat_map(|t| t.extensions).copied().collect();
        assert!(all.iter().all(|e| *e == e.to_ascii_lowercase()));
        assert_eq!(
            all.iter().collect::<std::collections::BTreeSet<_>>().len(),
            all.len()
        );

        assert_eq!(
            check_import(Path::new("x.png"), MAX_BYTES).unwrap().1,
            "png"
        );
        assert_eq!(
            check_import(Path::new("x.png"), MAX_BYTES + 1).unwrap_err(),
            "The image is larger than 50 MB."
        );
        assert!(
            check_import(Path::new("x.exe"), 1)
                .unwrap_err()
                .starts_with("Unsupported file type. Use one of: png, jpg")
        );
    }

    #[test]
    fn sources_are_read_and_checked() {
        let dir = temp_dir("source");
        let source = dir.join("My Photo.PNG");
        fs::write(&source, b"first").unwrap();
        assert_eq!(
            read_source(&source).unwrap(),
            ("png".into(), b"first".to_vec())
        );
        let text = dir.join("notes.txt");
        fs::write(&text, b"x").unwrap();
        assert!(read_source(&text).unwrap_err().contains("Unsupported"));
        assert!(read_source(&dir.join("missing.png")).is_err());
        fs::create_dir_all(dir.join("folder.png")).unwrap();
        assert!(read_source(&dir.join("folder.png")).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn formats() {
        assert!(same_format("jpg", "jpeg") && same_format("jpeg", "jpg"));
        assert!(same_format("png", "png") && !same_format("png", "svg"));
    }

    #[test]
    fn relative_paths_are_validated() {
        for ok in [
            "assets/a.png",
            "assets/sub/b.JPG",
            "assets/x-2.webp",
            // Free names: spaces, Unicode, case, inner dots.
            "assets/Figures/Ch 1/Leuchtturm bei Nacht (2).png",
            "assets/图/ü.v2.svg",
        ] {
            assert!(validate_relative(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "a.png",
            "assets",
            "assets/",
            "assets/../secret.png",
            "assets/./a.png",
            "assets//a.png",
            "/assets/a.png",
            "\\\\server\\share\\a.png",
            "C:/assets/a.png",
            "assets\\a.png",
            "assets/a.png:stream",
            "assets/a.exe",
            "assets/a.png.",
            "assets/a\0.png",
            "other/a.png",
            "../assets/a.png",
            "assets/a\".png",
            "assets/a\n.png",
            "assets/a\u{7f}.png",
            "assets/a\u{85}.png",
            "assets/ a.png",
            "assets/.a.png",
            "assets/sub /a.png",
            "assets/.sub/a.png",
            "assets/sub/",
        ] {
            assert!(validate_relative(bad).is_err(), "{bad:?}");
        }
        assert!(validate_relative(&format!("assets/{}.png", "a".repeat(1020))).is_err());
    }

    #[test]
    fn folder_paths_are_validated() {
        for ok in ["assets/", "assets/figures/", "assets/Ch 1/Überblick/"] {
            assert!(validate_folder(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "/",
            "assets",
            "assets//",
            "assets/a",
            "assets/a.png",
            "assets/../",
            "assets/./",
            "assets/a /",
            "assets/.a/",
            "assets/a\"b/",
            "assets/a:b/",
            "assets/a\\b/",
            "assets/a\tb/",
            "other/",
            "/assets/",
        ] {
            assert!(validate_folder(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn renames_keep_the_format() {
        for (old, new) in [
            ("assets/a.png", "assets/Mein Bild.png"),
            ("assets/a.jpg", "assets/b/c.jpeg"),
            ("assets/a.jpeg", "assets/a.jpg"),
            ("assets/OLD.PNG", "assets/old.png"),
        ] {
            assert!(validate_rename(old, new).is_ok(), "{old} -> {new}");
        }
        assert_eq!(
            validate_rename("assets/a.png", "assets/a.svg").unwrap_err(),
            "A renamed image keeps its format: end the name with .png."
        );
        for (old, new) in [
            ("assets/a.png", "assets/a.PNG"),
            ("assets/a.png", "assets/a"),
            ("assets/a.png", "assets/a\".png"),
            ("assets/a.png", "assets/a.png/"),
            ("assets/a.txt", "assets/a.png"),
        ] {
            assert!(validate_rename(old, new).is_err(), "{old} -> {new}");
        }
    }

    #[test]
    fn blobs_stay_inside_assets() {
        let project = temp_dir("blobs");
        let blob = "ab".repeat(32);
        fs::create_dir_all(project.join("assets")).unwrap();
        fs::write(project.join("assets").join(&blob), b"a").unwrap();
        assert_eq!(read_blob(&project, &blob, "assets/a.png").unwrap(), b"a");
        let missing = "cd".repeat(32);
        assert_eq!(
            read_blob(&project, &missing, "assets/b.png").unwrap_err(),
            "Image not found: assets/b.png"
        );
        for bad in ["../secret.png", "AB".repeat(32).as_str(), "ab", ""] {
            assert!(
                read_blob(&project, bad, "assets/a.png")
                    .unwrap_err()
                    .starts_with("Not an image blob"),
                "{bad:?}"
            );
        }
        fs::remove_dir_all(&project).unwrap();
        // No assets folder at all.
        assert!(
            read_blob(&project, &blob, "assets/a.png")
                .unwrap_err()
                .contains("not found")
        );
    }
}
