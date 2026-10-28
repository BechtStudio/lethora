//! Project storage: the files of a project folder, addressed by relative
//! paths with `/` separators (e.g. `"main/<device>.log"`; `""` is the root).
//! [`Document`](crate::Document) does all its I/O through [`Storage`], so a
//! browser build can later keep projects somewhere other than the filesystem.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use crate::{Error, Result};

/// Holds a lock taken with [`Storage::lock`] until dropped.
pub type LockGuard = Box<dyn Send>;

/// The files of one project.
pub trait Storage: Send {
    /// Where the project lives, for messages.
    fn location(&self) -> PathBuf;

    /// The whole file at `path`, or `None` if it does not exist.
    fn read(&self, path: &str) -> Result<Option<Vec<u8>>>;

    /// The file at `path` from byte `offset` to its end (empty if it is not
    /// longer), or `None` if it does not exist. Reads only that part where
    /// the storage can.
    fn read_from(&self, path: &str, offset: u64) -> Result<Option<Vec<u8>>> {
        Ok(self.read(path)?.map(|bytes| {
            let start = usize::try_from(offset).map_or(bytes.len(), |o| o.min(bytes.len()));
            bytes[start..].to_vec()
        }))
    }

    /// The size in bytes of the file at `path`, or `None` if there is no file.
    fn size(&self, path: &str) -> Result<Option<u64>>;

    /// Replaces the file at `path` with `bytes` atomically and durably.
    fn write_atomic(&self, path: &str, bytes: &[u8]) -> Result<()>;

    /// The names of the files (not directories) in directory `dir`; empty if
    /// `dir` does not exist.
    fn list(&self, dir: &str) -> Result<Vec<String>>;

    /// Creates directory `dir` and its parents, if missing.
    fn create_dir(&self, dir: &str) -> Result<()>;

    /// Moves the file at `from` to `to`, replacing `to`.
    fn rename(&self, from: &str, to: &str) -> Result<()>;

    /// Opens (creating if missing) the append-only log at `path`.
    fn open_log(&self, path: &str) -> Result<Box<dyn LogFile>>;

    /// Takes an exclusive lock named `path`, held while the guard lives.
    /// Fails with [`Error::Locked`] if someone else holds it.
    fn lock(&self, path: &str) -> Result<LockGuard>;
}

/// An append-only log file. Appends go to the append position: the length
/// last read, appended or truncated to, so a failed append is overwritten
/// by the next one.
pub trait LogFile: Send {
    /// The whole log; its length becomes the append position.
    fn read_all(&mut self) -> Result<Vec<u8>>;

    /// Writes `bytes` at the append position and syncs the data.
    fn append(&mut self, bytes: &[u8]) -> Result<()>;

    /// Cuts the log to `len` bytes and syncs it.
    fn truncate(&mut self, len: u64) -> Result<()>;

    /// The append position.
    fn size(&self) -> u64;
}

/// [`Storage`] in a directory of the filesystem.
pub struct FsStorage {
    root: PathBuf,
}

impl FsStorage {
    /// Storage in directory `root` (not created until something is written).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path(&self, path: &str) -> PathBuf {
        if path.is_empty() {
            self.root.clone()
        } else {
            self.root.join(path)
        }
    }
}

impl Storage for FsStorage {
    fn location(&self) -> PathBuf {
        self.root.clone()
    }

    fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
        match fs::read(self.path(path)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn read_from(&self, path: &str, offset: u64) -> Result<Option<Vec<u8>>> {
        let mut file = match File::open(self.path(path)) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        // Seeking past the end is allowed; reading there yields nothing.
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(Some(bytes))
    }

    fn size(&self, path: &str) -> Result<Option<u64>> {
        match fs::metadata(self.path(path)) {
            Ok(meta) => Ok(meta.is_file().then_some(meta.len())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Writes via a fsynced temp file (`<path>.tmp`) renamed over the target.
    fn write_atomic(&self, path: &str, bytes: &[u8]) -> Result<()> {
        let path = self.path(path);
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

    fn list(&self, dir: &str) -> Result<Vec<String>> {
        let entries = match fs::read_dir(self.path(dir)) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
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
        Ok(names)
    }

    fn create_dir(&self, dir: &str) -> Result<()> {
        Ok(fs::create_dir_all(self.path(dir))?)
    }

    fn rename(&self, from: &str, to: &str) -> Result<()> {
        Ok(fs::rename(self.path(from), self.path(to))?)
    }

    fn open_log(&self, path: &str) -> Result<Box<dyn LogFile>> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.path(path))?;
        let len = file.metadata()?.len();
        Ok(Box::new(FsLog { file, len }))
    }

    fn lock(&self, path: &str) -> Result<LockGuard> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.path(path))?;
        match file.try_lock() {
            Ok(()) => Ok(Box::new(file)),
            Err(TryLockError::WouldBlock) => Err(Error::Locked(self.location())),
            Err(TryLockError::Error(e)) => Err(e.into()),
        }
    }
}

struct FsLog {
    file: File,
    len: u64,
}

impl LogFile for FsLog {
    fn read_all(&mut self) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.file.seek(SeekFrom::Start(0))?;
        self.file.read_to_end(&mut bytes)?;
        self.len = bytes.len() as u64;
        Ok(bytes)
    }

    fn append(&mut self, bytes: &[u8]) -> Result<()> {
        self.file.seek(SeekFrom::Start(self.len))?;
        self.file.write_all(bytes)?;
        self.file.sync_data()?;
        self.len += bytes.len() as u64;
        Ok(())
    }

    fn truncate(&mut self, len: u64) -> Result<()> {
        self.file.set_len(len)?;
        self.file.sync_all()?;
        self.len = len;
        Ok(())
    }

    fn size(&self) -> u64 {
        self.len
    }
}
