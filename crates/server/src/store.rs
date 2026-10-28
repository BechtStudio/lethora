//! Where the server keeps its files: a small interface over paths relative
//! to the data directory, with `/` separators, and a filesystem backend.
//! Other backends (SQLite, S3) can come later behind the same interface.
//! All calls block; the server runs them off the async threads.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// The server's storage (see the module docs).
pub trait Store: Send + Sync {
    /// The whole file at `path`, or `None` if there is none.
    fn read(&self, path: &str) -> io::Result<Option<Vec<u8>>>;

    /// At most `max` bytes of the file at `path` from `offset` (empty past
    /// its end), or `None` if there is no file.
    fn read_range(&self, path: &str, offset: u64, max: u64) -> io::Result<Option<Vec<u8>>>;

    /// Appends `bytes` to the file at `path` (created with its directories
    /// if missing) and syncs the data.
    fn append(&self, path: &str, bytes: &[u8]) -> io::Result<()>;

    /// Replaces the file at `path` with `bytes` atomically and durably
    /// (creating its directories).
    fn write_atomic(&self, path: &str, bytes: &[u8]) -> io::Result<()>;

    /// The size of the file at `path`, or `None` if there is none.
    fn size(&self, path: &str) -> io::Result<Option<u64>>;

    /// The names of the files (not directories) in `dir`; empty if it
    /// doesn't exist.
    fn list(&self, dir: &str) -> io::Result<Vec<String>>;

    /// Deletes the file at `path`; nothing if there is none.
    fn delete(&self, path: &str) -> io::Result<()>;

    /// Deletes directory `dir` with everything in it; nothing if there is
    /// none.
    fn delete_dir(&self, dir: &str) -> io::Result<()>;
}

/// [`Store`] in a directory of the filesystem. Atomic writes go through a
/// fsynced temp file (`<name>.tmp`) renamed over the target, as the core's
/// project storage does.
pub struct FsStore {
    root: PathBuf,
}

impl FsStore {
    /// Storage in directory `root` (created when something is written).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path(&self, path: &str) -> PathBuf {
        self.root.join(path)
    }
}

fn not_found<T>(result: io::Result<T>) -> io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn create_parent(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(parent) => fs::create_dir_all(parent),
        None => Ok(()),
    }
}

impl Store for FsStore {
    fn read(&self, path: &str) -> io::Result<Option<Vec<u8>>> {
        not_found(fs::read(self.path(path)))
    }

    fn read_range(&self, path: &str, offset: u64, max: u64) -> io::Result<Option<Vec<u8>>> {
        let Some(mut file) = not_found(File::open(self.path(path)))? else {
            return Ok(None);
        };
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::new();
        file.take(max).read_to_end(&mut bytes)?;
        Ok(Some(bytes))
    }

    fn append(&self, path: &str, bytes: &[u8]) -> io::Result<()> {
        let path = self.path(path);
        create_parent(&path)?;
        let mut file = OpenOptions::new().append(true).create(true).open(path)?;
        file.write_all(bytes)?;
        file.sync_data()
    }

    fn write_atomic(&self, path: &str, bytes: &[u8]) -> io::Result<()> {
        let path = self.path(path);
        create_parent(&path)?;
        let mut tmp = OsString::from(path.as_os_str());
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, &path)?;
        if cfg!(unix)
            && let Some(dir) = path.parent()
            && let Ok(dir) = File::open(dir)
        {
            let _ = dir.sync_all();
        }
        Ok(())
    }

    fn size(&self, path: &str) -> io::Result<Option<u64>> {
        Ok(not_found(fs::metadata(self.path(path)))?
            .filter(|meta| meta.is_file())
            .map(|meta| meta.len()))
    }

    fn list(&self, dir: &str) -> io::Result<Vec<String>> {
        let Some(entries) = not_found(fs::read_dir(self.path(dir)))? else {
            return Ok(Vec::new());
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && let Ok(name) = entry.file_name().into_string()
            {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }

    fn delete(&self, path: &str) -> io::Result<()> {
        not_found(fs::remove_file(self.path(path))).map(drop)
    }

    fn delete_dir(&self, dir: &str) -> io::Result<()> {
        not_found(fs::remove_dir_all(self.path(dir))).map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fs_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsStore::new(dir.path());
        assert_eq!(store.read("p/a.log").unwrap(), None);
        assert_eq!(store.size("p/a.log").unwrap(), None);
        assert_eq!(store.read_range("p/a.log", 0, 10).unwrap(), None);
        assert!(store.list("p").unwrap().is_empty());

        store.append("p/main/a.log", b"hello").unwrap();
        store.append("p/main/a.log", b" world").unwrap();
        assert_eq!(store.read("p/main/a.log").unwrap().unwrap(), b"hello world");
        assert_eq!(store.size("p/main/a.log").unwrap(), Some(11));
        assert_eq!(
            store.read_range("p/main/a.log", 6, 3).unwrap().unwrap(),
            b"wor"
        );
        assert_eq!(
            store.read_range("p/main/a.log", 6, 100).unwrap().unwrap(),
            b"world"
        );
        assert!(
            store
                .read_range("p/main/a.log", 50, 3)
                .unwrap()
                .unwrap()
                .is_empty()
        );

        store.write_atomic("p/meta.json", b"{}").unwrap();
        store.write_atomic("p/meta.json", b"{\"a\":1}").unwrap();
        assert_eq!(store.read("p/meta.json").unwrap().unwrap(), b"{\"a\":1}");
        assert_eq!(store.list("p").unwrap(), ["meta.json"]);
        assert_eq!(store.list("p/main").unwrap(), ["a.log"]);
        // A directory isn't a file.
        assert_eq!(store.size("p/main").unwrap(), None);

        store.delete("p/meta.json").unwrap();
        store.delete("p/meta.json").unwrap();
        assert_eq!(store.read("p/meta.json").unwrap(), None);
        store.delete_dir("p").unwrap();
        store.delete_dir("p").unwrap();
        assert_eq!(store.read("p/main/a.log").unwrap(), None);
    }
}
