//! Helpers shared by the integration tests: images stored the way the app
//! stores them, as blobs in `assets/` named by an asset index (see
//! `lethora-core`, "Assets"). The blob names here are not keyed hashes, just
//! 64 lowercase hex characters, which is all the compiler checks.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// The blob name the tests use for `path` (at most 32 bytes): its bytes in
/// hex, padded with zeros to 64 characters.
pub fn blob(path: &str) -> String {
    assert!(path.len() <= 32, "{path} is too long for a test blob name");
    let hex: String = path.bytes().map(|b| format!("{b:02x}")).collect();
    format!("{hex:0>64}")
}

/// Writes `bytes` as the blob of `path` (`assets/...`) in `<dir>/assets/`.
pub fn put(dir: &Path, path: &str, bytes: impl AsRef<[u8]>) {
    fs::create_dir_all(dir.join("assets")).unwrap();
    fs::write(dir.join("assets").join(blob(path)), bytes).unwrap();
}

/// The asset index for every literal `"assets/..."` path in `content`.
pub fn index(content: &str) -> BTreeMap<String, String> {
    content
        .split("\"assets/")
        .skip(1)
        .filter_map(|rest| rest.split_once('"'))
        .map(|(name, _)| {
            let path = format!("assets/{name}");
            (path.clone(), blob(&path))
        })
        .collect()
}
