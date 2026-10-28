use std::path::Path;
use std::process::Command;

fn main() {
    // The short git hash for "About Lethora" and bug reports, `-dirty` with
    // uncommitted changes; "unknown" without git (e.g. a source archive).
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
    };
    let hash = match git(&["rev-parse", "--short", "HEAD"]) {
        Some(hash) if !hash.is_empty() => {
            let dirty = git(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
            if dirty { format!("{hash}-dirty") } else { hash }
        }
        _ => "unknown".to_owned(),
    };
    println!("cargo:rustc-env=LETHORA_GIT_HASH={hash}");
    // Again after a commit, a checkout or staging (HEAD, the branch it
    // names, the index); only files that exist, or Cargo reruns it always.
    if let Some(dir) = git(&["rev-parse", "--git-dir"]) {
        let head = git(&["symbolic-ref", "-q", "HEAD"]);
        for file in ["HEAD", "index"].into_iter().chain(head.as_deref()) {
            let path = Path::new(&dir).join(file);
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
    tauri_build::build()
}
