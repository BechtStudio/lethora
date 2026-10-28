//! Folder sync in the app: the open replica next to the document, the
//! project keys in the app config dir, the sync status for the web view, and
//! whether the project folder lies inside a sync client's folder (a project
//! synced in plain text). Also where the sync apps keep their folders on this
//! device, finding a synced project by its code there, and the names of new
//! projects in the default projects folder.
//!
//! Failures never fail opening a project: they become the status. While the
//! sync folder is unavailable (a USB stick unplugged, Dropbox not mounted),
//! the pull path tries to open the replica again at most every
//! [`RETRY`].

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lethora_core::{
    DeviceId, Document, Error, FsStorage, Identity, ProjectCode, Pulled, Replica, SyncSettings,
    replica_project, stored_sync_settings,
};
use serde::{Deserialize, Serialize};

use crate::{message, write_atomic};

/// Folder of the project keys in the app config dir: `<project id>.key`.
const KEYS: &str = "keys";
/// Dismissed plain-sync notices in the app config dir.
const NOTICES: &str = "notices.json";
/// Marks a complete replica (see `lethora_core::sync`).
const REPLICA_FILE: &str = "replica.json";
/// Suffix of the replica folder made at setup.
const SYNC_SUFFIX: &str = ".lethora-sync";
/// Lethora's folder inside a sync app's folder (and in Documents).
pub(crate) const LETHORA_FOLDER: &str = "Lethora";
/// How often an unavailable sync folder is tried again.
const RETRY: Duration = Duration::from_secs(10);

/// How encrypted sync is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Health {
    Ok,
    /// The sync folder can't be reached; tried again every [`RETRY`].
    Unavailable,
    /// No key stored, or the stored one doesn't unlock the replica.
    NeedsCode,
    /// The project uses features of a newer Lethora (folder or server sync).
    NeedsUpdate,
    Error,
    // Server sync (see `serversync`):
    Online,
    Connecting,
    /// The server can't be reached; tried again at `retryAt`.
    Offline,
    /// This device was removed from the project.
    Removed,
}

/// The sync status for the web view (`sync_status`).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    /// `off`, `encrypted`, `plain` or `server`.
    mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    folder: Option<String>,
    state: Health,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    /// Unix ms of the last successful push or pull.
    #[serde(skip_serializing_if = "Option::is_none")]
    last_sync: Option<u64>,
    /// The sync client whose folder holds the project.
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<&'static str>,
    /// The one-time notice about plain-text sync, until dismissed.
    #[serde(skip_serializing_if = "Option::is_none")]
    notice: Option<Notice>,
    /// Server sync: the server's origin.
    #[serde(skip_serializing_if = "Option::is_none")]
    server: Option<String>,
    /// Server sync: this device's role (`owner`, `editor`, ...).
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    /// Server sync while offline: Unix ms of the next try.
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_at: Option<u64>,
}

impl Status {
    /// The status of a project shared on server `server`.
    pub fn server(
        server: &str,
        state: Health,
        message: Option<String>,
        role: Option<String>,
        last_sync: Option<u64>,
        retry_at: Option<u64>,
    ) -> Self {
        Self {
            mode: "server",
            folder: None,
            state,
            message,
            last_sync,
            provider: None,
            notice: None,
            server: Some(server.to_owned()),
            role,
            retry_at,
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct Notice {
    provider: &'static str,
}

/// The sync side of an open project.
pub(crate) struct Link {
    replica: Option<Replica>,
    code: Option<ProjectCode>,
    health: Health,
    message: Option<String>,
    last_sync: Option<u64>,
    last_try: Option<Instant>,
    /// The sync client whose folder holds the project.
    provider: Option<&'static str>,
    notice_dismissed: bool,
    /// Other devices' files are in the project folder.
    others: bool,
    /// The canonical project folder (notice dismissals are stored by it).
    canonical: PathBuf,
}

impl Link {
    /// The link of `doc` in folder `path`: if sync is set up, reads the key
    /// from `config` (the app config dir) and opens the replica.
    pub fn open(doc: &Document, path: &Path, identity: &Identity, config: Option<&Path>) -> Self {
        let mut link = Self::new(path, identity, config);
        if let Some(settings) = doc.sync_settings() {
            link.code = config.and_then(|c| read_key(c, &settings.project));
            link.connect(doc);
        }
        link
    }

    /// The link of a project just joined with `replica` and `code`.
    pub fn joined(
        path: &Path,
        identity: &Identity,
        config: Option<&Path>,
        replica: Replica,
        code: ProjectCode,
    ) -> Self {
        let mut link = Self::new(path, identity, config);
        link.replica = Some(replica);
        link.code = Some(code);
        link.synced();
        link
    }

    fn new(path: &Path, identity: &Identity, config: Option<&Path>) -> Self {
        let canonical = canonical(path);
        let provider = sync_provider(&canonical, |dir, marker| dir.join(marker).exists());
        Self {
            replica: None,
            code: None,
            health: Health::Ok,
            message: None,
            last_sync: None,
            last_try: None,
            provider,
            notice_dismissed: config.is_some_and(|c| notice_dismissed(c, &canonical)),
            others: others_present(path, &identity.device),
            canonical,
        }
    }

    /// Opens the replica named by `doc`'s sync settings with the known code.
    fn connect(&mut self, doc: &Document) {
        self.last_try = Some(Instant::now());
        let Some(settings) = doc.sync_settings() else {
            return;
        };
        let Some(code) = &self.code else {
            self.set(
                Health::NeedsCode,
                "This device has no project code for this project yet. Enter it to connect.".into(),
            );
            return;
        };
        match open_replica(doc, settings, code) {
            Ok(replica) => {
                self.replica = Some(replica);
                self.synced();
            }
            Err((health, message)) => self.set(health, message),
        }
    }

    fn set(&mut self, health: Health, message: String) {
        if self.health != health || self.message.as_ref() != Some(&message) {
            eprintln!("Sync: {message}");
        }
        self.health = health;
        self.message = Some(message);
    }

    /// A push or pull worked; reading another device may have stopped at
    /// a newer version's data (`Replica::needs_update`).
    fn synced(&mut self) {
        self.last_sync = Some(now_ms());
        if let Some(message) = self.replica.as_ref().and_then(Replica::needs_update) {
            self.set(Health::NeedsUpdate, message.into());
            return;
        }
        self.health = Health::Ok;
        self.message = None;
    }

    /// A push or pull failed: with the sync folder gone the replica is
    /// dropped (and opened again later), else the error is shown.
    fn failed(&mut self, e: Error) {
        let folder = self.replica.as_ref().map(Replica::location);
        match folder {
            Some(folder) if !folder.is_dir() => {
                self.replica = None;
                self.last_try = Some(Instant::now());
                let (health, message) = unavailable(&folder);
                self.set(health, message);
            }
            _ => self.set(Health::Error, format!("Syncing failed: {}", message(e))),
        }
    }

    /// Imports what other devices wrote: from the replica if one is open
    /// (an unavailable one is tried again first, at most every [`RETRY`]),
    /// else from other devices' plain files in the project folder. Replica
    /// failures go to the status, not to the caller.
    pub fn pull(&mut self, doc: &mut Document) -> Result<Pulled, String> {
        if self.replica.is_none()
            && doc.sync_settings().is_some()
            && matches!(self.health, Health::Unavailable | Health::Error)
            && self.last_try.is_none_or(|t| t.elapsed() >= RETRY)
        {
            self.connect(doc);
        }
        match &mut self.replica {
            Some(replica) => match doc.sync_pull(replica) {
                Ok(pulled) => {
                    self.synced();
                    Ok(pulled)
                }
                Err(e) => {
                    self.failed(e);
                    Ok(Pulled::default())
                }
            },
            None => {
                let pulled = doc.pull_local().map_err(message)?;
                if pulled.changed {
                    self.others = true;
                }
                Ok(pulled)
            }
        }
    }

    /// Copies what is new into the own replica files (autosaving first), if
    /// a replica is open.
    pub fn push(&mut self, doc: &mut Document) {
        let Some(replica) = &mut self.replica else {
            return;
        };
        match doc.sync_push(replica) {
            Ok(()) => self.synced(),
            Err(e) => self.failed(e),
        }
    }

    /// Starts encrypted sync in a new folder inside `parent` (named after the
    /// project folder `path`); stores the key and the settings. Returns the
    /// project code.
    pub fn setup(
        &mut self,
        doc: &mut Document,
        path: &Path,
        parent: &Path,
        config: Option<&Path>,
    ) -> Result<String, String> {
        if doc.sync_settings().is_some() {
            return Err(
                "This project is synced already. Stop syncing first to use another folder.".into(),
            );
        }
        if doc.server_settings().is_some() {
            return Err(
                "This project is shared on a Lethora server, so it can't sync through a folder too."
                    .into(),
            );
        }
        if parent.join(REPLICA_FILE).exists() {
            return Err(
                "This folder already holds a synced project; use Open shared project… instead."
                    .into(),
            );
        }
        if !parent.is_dir() {
            return Err(format!("The folder {} was not found.", parent.display()));
        }
        let config = config.ok_or(NO_CONFIG)?;
        let target = replica_folder(parent, &crate::project_name(path), |p| p.exists());
        nesting_error(&canonical(path), &canonical(&target)).map_or(Ok(()), Err)?;
        let (replica, code) = doc
            .sync_setup(Box::new(FsStorage::new(&target)))
            .map_err(message)?;
        let project = replica.project_id().to_owned();
        write_key(config, &project, &code)
            .map_err(|e| format!("Could not store the project code: {e}"))?;
        doc.set_sync_settings(Some(SyncSettings {
            folder: target,
            project,
            level: None,
        }))
        .map_err(message)?;
        self.replica = Some(replica);
        self.code = Some(code.clone());
        self.synced();
        Ok(code.to_string())
    }

    /// Connects with a project code typed by the user (no key stored, or
    /// the stored one was wrong) and stores it.
    pub fn enter_code(
        &mut self,
        doc: &Document,
        code: &str,
        config: Option<&Path>,
    ) -> Result<(), String> {
        let settings = doc.sync_settings().ok_or(NOT_SYNCED)?.clone();
        let code = ProjectCode::parse(code).map_err(message)?;
        let config = config.ok_or(NO_CONFIG)?;
        let replica = match open_replica(doc, &settings, &code) {
            Ok(replica) => replica,
            Err((Health::NeedsCode, _)) => {
                return Err("This project code does not unlock the sync folder.".into());
            }
            Err((_, message)) => return Err(message),
        };
        write_key(config, &settings.project, &code)
            .map_err(|e| format!("Could not store the project code: {e}"))?;
        self.replica = Some(replica);
        self.code = Some(code);
        self.synced();
        Ok(())
    }

    /// The project code, while encrypted sync is set up and the code known.
    pub fn code(&self, doc: &Document) -> Result<String, String> {
        doc.sync_settings().ok_or(NOT_SYNCED)?;
        self.code
            .as_ref()
            .map(ToString::to_string)
            .ok_or_else(|| "This device has no project code for this project.".into())
    }

    /// Stops encrypted sync on this device: clears the settings, closes the
    /// replica and deletes the key. The replica stays in the sync folder.
    pub fn stop(&mut self, doc: &mut Document, config: Option<&Path>) -> Result<(), String> {
        let Some(settings) = doc.sync_settings().cloned() else {
            return Ok(());
        };
        doc.set_sync_settings(None).map_err(message)?;
        self.replica = None;
        self.code = None;
        self.health = Health::Ok;
        self.message = None;
        self.last_sync = None;
        self.last_try = None;
        if let Some(config) = config {
            match fs::remove_file(key_path(config, &settings.project)) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => {
                    eprintln!("Could not delete the project key: {e}");
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Never shows the plain-sync notice for this project again.
    pub fn dismiss_notice(&mut self, config: Option<&Path>) -> Result<(), String> {
        self.notice_dismissed = true;
        let config = config.ok_or(NO_CONFIG)?;
        dismiss_notice(config, &self.canonical)
            .map_err(|e| format!("Could not store the choice: {e}"))
    }

    pub fn status(&self, doc: &Document) -> Status {
        let off = Status {
            mode: "off",
            folder: None,
            state: Health::Ok,
            message: None,
            last_sync: None,
            provider: self.provider,
            notice: None,
            server: None,
            role: None,
            retry_at: None,
        };
        match doc.sync_settings() {
            Some(settings) => Status {
                mode: "encrypted",
                folder: Some(settings.folder.display().to_string()),
                state: self.health,
                message: self.message.clone(),
                last_sync: self.last_sync,
                ..off
            },
            None if self.provider.is_some() || self.others => Status {
                mode: "plain",
                notice: self
                    .provider
                    .filter(|_| !self.notice_dismissed)
                    .map(|provider| Notice { provider }),
                ..off
            },
            None => off,
        }
    }
}

const NO_CONFIG: &str =
    "Lethora can't store the project code: the app's settings folder is unknown.";
const NOT_SYNCED: &str = "Encrypted sync is not set up for this project.";

/// Joins the synced project in `replica` with `code` as a new local
/// project at `path` (with `.lethora` already added); stores the key and the
/// settings. Checks the code and the folders before writing anything, and
/// refuses if one of `known` (project folders on this device) syncs with the
/// project already: two local copies would be two writers of this device's
/// replica files.
pub(crate) fn join(
    replica: &Path,
    path: &Path,
    code: &str,
    identity: &Identity,
    config: Option<&Path>,
    known: &[PathBuf],
) -> Result<(Document, Replica, ProjectCode), String> {
    let code = ProjectCode::parse(code).map_err(message)?;
    if !replica.join(REPLICA_FILE).is_file() {
        return Err(format!(
            "{} is not a synced Lethora project. Choose the folder whose name ends in {SYNC_SUFFIX}.",
            replica.display()
        ));
    }
    if let Ok(Some(project)) = replica_project(&FsStorage::new(replica), &code)
        && let Some(local) = local_copy(known, &identity.device, &project)
    {
        return Err(already_here(&local));
    }
    nesting_error(&canonical(path), &canonical(replica)).map_or(Ok(()), Err)?;
    let config = config.ok_or(NO_CONFIG)?;
    let (mut doc, opened) = Document::sync_join(
        Box::new(FsStorage::new(path)),
        Box::new(FsStorage::new(replica)),
        &code,
        identity,
    )
    .map_err(|e| match e {
        Error::WrongCode(_) => "This project code does not unlock that synced project.".into(),
        e => message(e),
    })?;
    let project = opened.project_id().to_owned();
    write_key(config, &project, &code)
        .map_err(|e| format!("Could not store the project code: {e}"))?;
    doc.set_sync_settings(Some(SyncSettings {
        folder: replica.to_path_buf(),
        project,
        level: None,
    }))
    .map_err(message)?;
    Ok((doc, opened, code))
}

/// Opens the replica of `settings` with `code` and checks its project id.
fn open_replica(
    doc: &Document,
    settings: &SyncSettings,
    code: &ProjectCode,
) -> Result<Replica, (Health, String)> {
    let folder = &settings.folder;
    if !folder.join(REPLICA_FILE).is_file() {
        return Err(unavailable(folder));
    }
    let replica = doc
        .sync_open(Box::new(FsStorage::new(folder)), code)
        .map_err(|e| match e {
            Error::WrongCode(_) => (
                Health::NeedsCode,
                "The stored project code does not unlock the sync folder. Enter the code again."
                    .into(),
            ),
            Error::NotFound(_) => unavailable(folder),
            Error::Io(_) if !folder.is_dir() => unavailable(folder),
            e => (Health::Error, message(e)),
        })?;
    if replica.project_id() != settings.project {
        return Err((
            Health::Error,
            "The sync folder holds a different project.".into(),
        ));
    }
    Ok(replica)
}

fn unavailable(folder: &Path) -> (Health, String) {
    (
        Health::Unavailable,
        format!(
            "The sync folder {} is not available. Is the drive connected and the sync app running?",
            folder.display()
        ),
    )
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

// ---- Keys ----

pub(crate) fn key_path(config: &Path, project: &str) -> PathBuf {
    config.join(KEYS).join(format!("{project}.key"))
}

/// The stored key of replica project `project`, if any (and valid).
pub(crate) fn read_key(config: &Path, project: &str) -> Option<ProjectCode> {
    let hex = fs::read_to_string(key_path(config, project)).ok()?;
    let hex = hex.trim();
    if hex.len() != 64 {
        return None;
    }
    let mut key = [0u8; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(ProjectCode::from_key(key))
}

pub(crate) fn write_key(config: &Path, project: &str, code: &ProjectCode) -> io::Result<()> {
    let path = key_path(config, project);
    fs::create_dir_all(config.join(KEYS))?;
    let hex: String = code.key().iter().map(|b| format!("{b:02x}")).collect();
    write_atomic(&path, hex.as_bytes())
}

// ---- Notices ----

/// `notices.json`: project folders whose plain-sync notice was dismissed.
#[derive(Default, Serialize, Deserialize)]
struct Notices {
    #[serde(default)]
    dismissed: Vec<PathBuf>,
}

fn read_notices(config: &Path) -> Notices {
    fs::read(config.join(NOTICES))
        .ok()
        .and_then(|json| serde_json::from_slice(&json).ok())
        .unwrap_or_default()
}

fn notice_dismissed(config: &Path, project: &Path) -> bool {
    read_notices(config)
        .dismissed
        .iter()
        .any(|p| same_folder(p, project))
}

fn dismiss_notice(config: &Path, project: &Path) -> io::Result<()> {
    let mut notices = read_notices(config);
    if notices.dismissed.iter().any(|p| same_folder(p, project)) {
        return Ok(());
    }
    notices.dismissed.push(project.to_path_buf());
    fs::create_dir_all(config)?;
    let json = serde_json::to_vec_pretty(&notices).map_err(io::Error::other)?;
    write_atomic(&config.join(NOTICES), &json)
}

// ---- Folders ----

/// `path` with its nearest existing ancestor resolved (links, `..`, case on
/// Windows), so paths that don't exist yet compare with ones that do.
pub(crate) fn canonical(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut base = path;
    loop {
        if let Ok(resolved) = fs::canonicalize(base) {
            return rest.iter().rev().fold(resolved, |p, name| p.join(name));
        }
        match (base.parent(), base.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                base = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Whether `a` and `b` name the same folder (case-insensitively on Windows).
fn same_folder(a: &Path, b: &Path) -> bool {
    let (a, b): (Vec<_>, Vec<_>) = (parts(a).collect(), parts(b).collect());
    a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| same_name(x, y))
}

/// Whether `inner` is `outer` or lies inside it (compare canonical paths).
pub(crate) fn is_inside(inner: &Path, outer: &Path) -> bool {
    let inner: Vec<_> = parts(inner).collect();
    let outer: Vec<_> = parts(outer).collect();
    inner.len() >= outer.len() && outer.iter().zip(&inner).all(|(x, y)| same_name(x, y))
}

fn parts(path: &Path) -> impl Iterator<Item = String> + '_ {
    path.components().filter_map(|c| match c {
        Component::CurDir => None,
        c => Some(c.as_os_str().to_string_lossy().into_owned()),
    })
}

fn same_name(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.to_lowercase() == b.to_lowercase()
    } else {
        a == b
    }
}

/// Why a project at `project` and a replica at `replica` (canonical paths)
/// can't go together: one inside the other.
pub(crate) fn nesting_error(project: &Path, replica: &Path) -> Option<String> {
    if is_inside(replica, project) {
        Some("The sync folder can't be inside the project folder. Choose another folder.".into())
    } else if is_inside(project, replica) {
        Some("The project can't be inside the sync folder. Choose another place.".into())
    } else {
        None
    }
}

/// The replica folder for project `name` in `parent`: `<name>.lethora-sync`,
/// else with ` 2`, ` 3`… before the suffix, the first that doesn't `exist`.
pub(crate) fn replica_folder(parent: &Path, name: &str, exists: impl Fn(&Path) -> bool) -> PathBuf {
    free_name(parent, name, SYNC_SUFFIX, exists)
}

/// A new project's folder in `parent` (the default projects folder) for the
/// name the user gave: `<name>.lethora` with the name made safe
/// ([`safe_name`]), numbered like [`replica_folder`].
pub(crate) fn project_folder(parent: &Path, name: &str, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let suffix = format!(".{}", crate::EXTENSION);
    free_name(parent, &safe_name(name), &suffix, exists)
}

/// `<name><suffix>` in `parent`, else with ` 2`, ` 3`… before the suffix,
/// the first that doesn't `exist`.
fn free_name(parent: &Path, name: &str, suffix: &str, exists: impl Fn(&Path) -> bool) -> PathBuf {
    (1..)
        .map(|n| match n {
            1 => parent.join(format!("{name}{suffix}")),
            n => parent.join(format!("{name} {n}{suffix}")),
        })
        .find(|p| !exists(p))
        .expect("some name is free")
}

/// Longest project name kept, in characters.
const MAX_NAME: usize = 100;
/// Characters no file name may hold on Windows (control characters too).
const FORBIDDEN: &str = "\\/:*?\"<>|";
/// Names Windows reserves for devices, with any extension.
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
    "COM8", "COM9", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// `name` as a folder name every system accepts (projects move between
/// systems): [`FORBIDDEN`] and control characters become `-`, no spaces
/// around it and no dots at its end, at most [`MAX_NAME`] characters, a
/// reserved Windows name gets a `_`; `Untitled` if nothing is left.
pub(crate) fn safe_name(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| {
            if c.is_control() || FORBIDDEN.contains(c) {
                '-'
            } else {
                c
            }
        })
        .take(MAX_NAME)
        .collect();
    let mut name = replaced
        .trim_start()
        .trim_end_matches(|c: char| c == '.' || c.is_whitespace())
        .to_owned();
    let stem = name.split('.').next().unwrap_or_default().trim_end();
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
        name.insert(stem.len(), '_');
    }
    if name.is_empty() {
        "Untitled".into()
    } else {
        name
    }
}

/// The name of the project joined from replica folder `replica`: the
/// folder's name without `.lethora-sync`.
pub(crate) fn joined_name(replica: &Path) -> String {
    let name = replica
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = name
        .len()
        .checked_sub(SYNC_SUFFIX.len())
        .filter(|&at| name.is_char_boundary(at) && name[at..].eq_ignore_ascii_case(SYNC_SUFFIX));
    match stem {
        Some(at) => name[..at].to_owned(),
        None => name,
    }
}

/// The project folders (`*.lethora`) directly in `dir`.
pub(crate) fn project_folders(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(crate::EXTENSION))
                && p.is_dir()
        })
        .collect()
}

/// The first of `projects` (project folders) whose stored sync settings for
/// `device` name replica project `project`.
pub(crate) fn local_copy(
    projects: &[PathBuf],
    device: &DeviceId,
    project: &str,
) -> Option<PathBuf> {
    projects
        .iter()
        .find(|p| {
            stored_sync_settings(&FsStorage::new(p.as_path()), device)
                .is_some_and(|s| s.project == project)
        })
        .cloned()
}

/// The refusal to join a project this device has already.
fn already_here(local: &Path) -> String {
    format!(
        "This project is already on this device, in {}. Open it instead.",
        local.display()
    )
}

// ---- Finding a synced project by its code ----

/// A synced project found by its code (`sync_find`).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Found {
    /// The replica folder the code unlocks (the newest if several do).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replica: Option<PathBuf>,
    /// The sync apps searched (none when a folder was given).
    pub searched: Vec<String>,
    /// The project on this device that syncs with it already: open it
    /// instead of joining.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local: Option<PathBuf>,
}

/// Looks for the synced project `code` unlocks in `dirs` (see
/// [`find_replicas`]) and for a project in `known` already syncing with it.
pub(crate) fn find(
    dirs: &[PathBuf],
    code: &str,
    device: &DeviceId,
    known: &[PathBuf],
    searched: Vec<String>,
) -> Result<Found, String> {
    let code = ProjectCode::parse(code).map_err(message)?;
    let newest = find_replicas(dirs, &code)?.into_iter().next();
    let local = newest
        .as_ref()
        .and_then(|(_, project)| local_copy(known, device, project));
    Ok(Found {
        replica: newest.map(|(replica, _)| replica),
        searched,
        local,
    })
}

/// The replica folders in `dirs` that `code` unlocks, with their project
/// id, newest first: a dir itself if it holds a replica, else its
/// `*.lethora-sync` subfolders. Reads `replica.json` only, writes nothing.
/// A replica of a newer format fails the search if nothing matched (it might
/// be the one).
pub(crate) fn find_replicas(
    dirs: &[PathBuf],
    code: &ProjectCode,
) -> Result<Vec<(PathBuf, String)>, String> {
    let mut found = Vec::new();
    let mut problem = None;
    for replica in dirs.iter().flat_map(|d| replica_candidates(d)) {
        match replica_project(&FsStorage::new(&replica), code) {
            Ok(Some(project)) => found.push((modified(&replica), replica, project)),
            Ok(None) => {}
            Err(e @ Error::UnsupportedFormat(_)) => problem = problem.or(Some(message(e))),
            Err(e) => eprintln!("Sync: skipping {}: {e}", replica.display()),
        }
    }
    if found.is_empty()
        && let Some(problem) = problem
    {
        return Err(problem);
    }
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    Ok(found.into_iter().map(|(_, r, p)| (r, p)).collect())
}

/// `dir` if it holds a replica, else its subfolders named `*.lethora-sync`
/// that hold one.
fn replica_candidates(dir: &Path) -> Vec<PathBuf> {
    if dir.join(REPLICA_FILE).is_file() {
        return vec![dir.to_path_buf()];
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .to_lowercase()
                .ends_with(SYNC_SUFFIX)
        })
        .map(|e| e.path())
        .filter(|p| p.join(REPLICA_FILE).is_file())
        .collect()
}

/// When replica `dir` last changed: the newest of `replica.json` and the
/// devices' files.
fn modified(dir: &Path) -> SystemTime {
    let time = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok();
    fs::read_dir(dir.join("main"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| time(&e.path()))
        .chain(time(&dir.join(REPLICA_FILE)))
        .max()
        .unwrap_or(UNIX_EPOCH)
}

// ---- Sync apps on this device ----

/// A sync app's folder on this device (`sync_apps`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SyncApp {
    /// `dropbox`, `onedrive`, `icloud` or `googledrive`, with `-2`, `-3`…
    /// for more folders of the same app.
    pub id: String,
    pub label: String,
    pub path: PathBuf,
}

/// What detecting the sync apps looks at: [`System`] asks the OS, tests
/// fake it.
pub(crate) trait Probe {
    /// A non-empty environment variable, as a path.
    fn var(&self, name: &str) -> Option<PathBuf>;
    fn home(&self) -> Option<PathBuf>;
    fn read(&self, path: &Path) -> Option<Vec<u8>>;
    fn is_dir(&self, path: &Path) -> bool;
    /// The names of the folders in `dir`.
    fn subfolders(&self, dir: &Path) -> Vec<String>;
}

/// Whose rules apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Platform {
    Windows,
    Mac,
    Other,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::Other
        }
    }
}

/// The real environment and file system.
pub(crate) struct System {
    pub home: Option<PathBuf>,
}

impl Probe for System {
    fn var(&self, name: &str) -> Option<PathBuf> {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    }

    fn home(&self) -> Option<PathBuf> {
        self.home.clone()
    }

    fn read(&self, path: &Path) -> Option<Vec<u8>> {
        fs::read(path).ok()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn subfolders(&self, dir: &Path) -> Vec<String> {
        let Ok(entries) = fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect()
    }
}

/// The sync apps' folders on this device: existing folders only, each once,
/// Dropbox, OneDrive, iCloud Drive, then Google Drive. Only looks for files
/// and folders:
///
/// - Dropbox: `personal.path` and `business.path` from `info.json` in
///   `%LOCALAPPDATA%\Dropbox` or `%APPDATA%\Dropbox` (`~/.dropbox`
///   elsewhere), else `~/Dropbox`.
/// - OneDrive: on Windows `%OneDriveConsumer%`, `%OneDriveCommercial%` and
///   `%OneDrive%` (a folder named `OneDrive - <org>` is labelled with the
///   organisation); on macOS `~/Library/CloudStorage/OneDrive-*`.
/// - iCloud Drive: on Windows `~/iCloudDrive`; on macOS `~/Library/Mobile
///   Documents/com~apple~CloudDocs`.
/// - Google Drive: on macOS `~/Library/CloudStorage/GoogleDrive-*/My
///   Drive`; on Windows the first drive with a `My Drive` folder, `G:`
///   first.
pub(crate) fn sync_apps(platform: Platform, probe: &impl Probe) -> Vec<SyncApp> {
    let home = probe.home();
    let in_home = |rel: &str| home.as_ref().map(|h| h.join(rel));
    let cloud_storage = in_home("Library/CloudStorage");
    let mut found: Vec<(&str, String, PathBuf)> = Vec::new();

    // Dropbox.
    let infos: Vec<PathBuf> = match platform {
        Platform::Windows => ["LOCALAPPDATA", "APPDATA"]
            .iter()
            .filter_map(|v| probe.var(v))
            .map(|d| d.join("Dropbox").join("info.json"))
            .collect(),
        _ => in_home(".dropbox/info.json").into_iter().collect(),
    };
    let mut dropbox: Vec<(String, PathBuf)> = infos
        .iter()
        .filter_map(|info| probe.read(info))
        .flat_map(|json| dropbox_folders(&json))
        .collect();
    if dropbox.is_empty() {
        dropbox.extend(in_home("Dropbox").map(|p| ("Dropbox".to_owned(), p)));
    }
    found.extend(dropbox.into_iter().map(|(l, p)| ("dropbox", l, p)));

    // OneDrive.
    match platform {
        Platform::Windows => {
            for var in ["OneDriveConsumer", "OneDriveCommercial", "OneDrive"] {
                let Some(path) = probe.var(var) else {
                    continue;
                };
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let label = match name.strip_prefix("OneDrive - ") {
                    Some(org) if !org.trim().is_empty() => format!("OneDrive – {}", org.trim()),
                    _ => "OneDrive".to_owned(),
                };
                found.push(("onedrive", label, path));
            }
        }
        Platform::Mac => {
            for (dir, name) in subfolders(probe, cloud_storage.as_deref()) {
                let Some(rest) = name.strip_prefix("OneDrive-") else {
                    continue;
                };
                if rest.starts_with("SharedLibraries") {
                    continue;
                }
                let label = match rest {
                    "" | "Personal" => "OneDrive".to_owned(),
                    org => format!("OneDrive – {org}"),
                };
                found.push(("onedrive", label, dir.join(&name)));
            }
        }
        Platform::Other => {}
    }

    // iCloud Drive.
    let icloud = match platform {
        Platform::Windows => in_home("iCloudDrive"),
        Platform::Mac => in_home("Library/Mobile Documents/com~apple~CloudDocs"),
        Platform::Other => None,
    };
    found.extend(icloud.map(|p| ("icloud", "iCloud Drive".to_owned(), p)));

    // Google Drive.
    match platform {
        Platform::Windows => {
            let drive = std::iter::once('G')
                .chain(('D'..='Z').filter(|&c| c != 'G'))
                .map(|c| PathBuf::from(format!("{c}:\\")).join("My Drive"))
                .find(|p| probe.is_dir(p));
            found.extend(drive.map(|p| ("googledrive", "Google Drive".to_owned(), p)));
        }
        Platform::Mac => {
            let accounts: Vec<(PathBuf, String)> = subfolders(probe, cloud_storage.as_deref())
                .into_iter()
                .filter_map(|(dir, name)| {
                    let account = name.strip_prefix("GoogleDrive-")?.to_owned();
                    Some((dir.join(&name).join("My Drive"), account))
                })
                .collect();
            let several = accounts.len() > 1;
            for (path, account) in accounts {
                let label = if several {
                    format!("Google Drive – {account}")
                } else {
                    "Google Drive".to_owned()
                };
                found.push(("googledrive", label, path));
            }
        }
        Platform::Other => {}
    }

    let mut apps: Vec<SyncApp> = Vec::new();
    for (id, label, path) in found {
        if !probe.is_dir(&path) || apps.iter().any(|a| same_folder(&a.path, &path)) {
            continue;
        }
        let id = match apps
            .iter()
            .filter(|a| a.id.split('-').next() == Some(id))
            .count()
        {
            0 => id.to_owned(),
            n => format!("{id}-{}", n + 1),
        };
        apps.push(SyncApp { id, label, path });
    }
    apps
}

/// The folders in `dir`, sorted, each with `dir`.
fn subfolders(probe: &impl Probe, dir: Option<&Path>) -> Vec<(PathBuf, String)> {
    let Some(dir) = dir else {
        return Vec::new();
    };
    let mut names = probe.subfolders(dir);
    names.sort();
    names.into_iter().map(|n| (dir.to_path_buf(), n)).collect()
}

/// The folders Dropbox's `info.json` names, with their label.
fn dropbox_folders(json: &[u8]) -> Vec<(String, PathBuf)> {
    #[derive(Deserialize)]
    struct Account {
        path: PathBuf,
    }
    #[derive(Deserialize)]
    struct Info {
        personal: Option<Account>,
        business: Option<Account>,
    }
    let Ok(info) = serde_json::from_slice::<Info>(json) else {
        return Vec::new();
    };
    let personal = info.personal.map(|a| ("Dropbox".to_owned(), a.path));
    let business = info
        .business
        .map(|a| ("Dropbox (Business)".to_owned(), a.path));
    personal.into_iter().chain(business).collect()
}

/// Folder names of sync clients and the provider they belong to.
fn provider_by_name(name: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    match lower.as_str() {
        "dropbox" => Some("Dropbox"),
        "onedrive" => Some("OneDrive"),
        _ if lower.starts_with("onedrive - ") => Some("OneDrive"),
        "iclouddrive" | "icloud drive" | "mobile documents" => Some("iCloud Drive"),
        "google drive" | "my drive" | "googledrive" => Some("Google Drive"),
        "box" | "box sync" => Some("Box"),
        _ => None,
    }
}

/// The sync client whose folder holds `path` (canonical), nearest first: an
/// ancestor named like a client's folder, or one holding `.stfolder`
/// (Syncthing) or `.dropbox`; `has(dir, name)` tells whether `dir` holds
/// `name`.
pub(crate) fn sync_provider(
    path: &Path,
    has: impl Fn(&Path, &str) -> bool,
) -> Option<&'static str> {
    for dir in path.ancestors() {
        if let Some(provider) = dir
            .file_name()
            .and_then(|n| provider_by_name(&n.to_string_lossy()))
        {
            return Some(provider);
        }
        if has(dir, ".stfolder") {
            return Some("Syncthing");
        }
        if has(dir, ".dropbox") {
            return Some("Dropbox");
        }
    }
    None
}

/// The default projects folder: `<documents>/Lethora`, unless a sync client
/// syncs the Documents folder (OneDrive's or iCloud's Documents backup),
/// then `<home>/Lethora`, so projects never sync in plain text by default.
/// `has` as for [`sync_provider`].
pub(crate) fn default_projects_dir(
    documents: Option<&Path>,
    home: Option<&Path>,
    has: impl Fn(&Path, &str) -> bool,
) -> Option<PathBuf> {
    let synced = |dir: &Path| sync_provider(&canonical(dir), &has).is_some();
    match (documents, home) {
        (Some(docs), Some(home)) if synced(docs) && !synced(home) => Some(home),
        (Some(docs), _) => Some(docs),
        (None, home) => home,
    }
    .map(|dir| dir.join(LETHORA_FOLDER))
}

/// Whether other devices' files are in the project folder `path`.
fn others_present(path: &Path, own: &DeviceId) -> bool {
    let Ok(entries) = fs::read_dir(path.join("main")) else {
        return false;
    };
    entries.flatten().any(|e| {
        let name = e.file_name();
        let name = name.to_string_lossy();
        name.strip_suffix(".snapshot")
            .or_else(|| name.strip_suffix(".log"))
            .and_then(|d| DeviceId::parse(d).ok())
            .is_some_and(|d| &d != own)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_go_to_home_when_documents_sync() {
        let none = |_: &Path, _: &str| false;
        let p = PathBuf::from;
        let docs = p("/u/a/Documents");
        let synced_docs = p("/u/a/OneDrive/Documents");
        let home = p("/u/a");
        assert_eq!(
            default_projects_dir(Some(&docs), Some(&home), none),
            Some(docs.join(LETHORA_FOLDER))
        );
        assert_eq!(
            default_projects_dir(Some(&synced_docs), Some(&home), none),
            Some(home.join(LETHORA_FOLDER))
        );
        // A synced home doesn't help: Documents stays.
        let synced_home = p("/u/Dropbox/a");
        assert_eq!(
            default_projects_dir(Some(&synced_docs), Some(&synced_home), none),
            Some(synced_docs.join(LETHORA_FOLDER))
        );
        assert_eq!(
            default_projects_dir(None, Some(&home), none),
            Some(home.join(LETHORA_FOLDER))
        );
        assert_eq!(default_projects_dir(None, None, none), None);
    }

    #[test]
    fn sync_clients_are_found_by_folder_name_or_marker() {
        let none = |_: &Path, _: &str| false;
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(
            sync_provider(&p("/home/a/Dropbox/Books/Novel.lethora"), none),
            Some("Dropbox")
        );
        assert_eq!(
            sync_provider(&p("/home/a/dropbox/x"), none),
            Some("Dropbox")
        );
        assert_eq!(
            sync_provider(&p("/u/OneDrive/x.lethora"), none),
            Some("OneDrive")
        );
        assert_eq!(
            sync_provider(&p("/u/OneDrive - Uni Bonn/x"), none),
            Some("OneDrive")
        );
        assert_eq!(sync_provider(&p("/u/OneDriveBackup/x"), none), None);
        assert_eq!(
            sync_provider(&p("/u/iCloudDrive/x"), none),
            Some("iCloud Drive")
        );
        assert_eq!(
            sync_provider(&p("/u/iCloud Drive/x"), none),
            Some("iCloud Drive")
        );
        assert_eq!(
            sync_provider(
                &p("/Users/a/Library/Mobile Documents/com~apple~CloudDocs/x"),
                none
            ),
            Some("iCloud Drive")
        );
        assert_eq!(
            sync_provider(&p("/g/Google Drive/x"), none),
            Some("Google Drive")
        );
        assert_eq!(
            sync_provider(&p("/g/My Drive/x"), none),
            Some("Google Drive")
        );
        assert_eq!(
            sync_provider(&p("/g/GoogleDrive/x"), none),
            Some("Google Drive")
        );
        assert_eq!(sync_provider(&p("/b/Box/x"), none), Some("Box"));
        assert_eq!(sync_provider(&p("/b/Box Sync/x"), none), Some("Box"));
        assert_eq!(
            sync_provider(&p("/home/a/Documents/Novel.lethora"), none),
            None
        );

        // Markers, nearest ancestor first.
        let sync = |dir: &Path, name: &str| dir == Path::new("/data/shared") && name == ".stfolder";
        assert_eq!(
            sync_provider(&p("/data/shared/books/x"), sync),
            Some("Syncthing")
        );
        let dropbox = |dir: &Path, name: &str| dir == Path::new("/mnt/team") && name == ".dropbox";
        assert_eq!(sync_provider(&p("/mnt/team/x"), dropbox), Some("Dropbox"));
        assert_eq!(
            sync_provider(&p("/mnt/team/OneDrive/x"), dropbox),
            Some("OneDrive")
        );
    }

    #[test]
    fn replica_and_project_never_nest() {
        let p = |s: &str| PathBuf::from(s);
        assert!(is_inside(&p("/a/b/c"), &p("/a/b")));
        assert!(is_inside(&p("/a/b"), &p("/a/b")));
        assert!(is_inside(&p("/a/./b"), &p("/a/b")));
        assert!(!is_inside(&p("/a/bc"), &p("/a/b")));
        assert!(!is_inside(&p("/a"), &p("/a/b")));
        assert_eq!(is_inside(&p("/A/B/c"), &p("/a/b")), cfg!(windows));

        let project = p("/home/a/Novel.lethora");
        assert!(nesting_error(&project, &p("/home/a/Dropbox/Novel.lethora-sync")).is_none());
        let inner = nesting_error(
            &project,
            &p("/home/a/Novel.lethora/sync/Novel.lethora-sync"),
        );
        assert!(inner.unwrap().contains("inside the project"));
        let outer = nesting_error(&p("/home/a/Sync/Novel.lethora"), &p("/home/a/Sync"));
        assert!(outer.unwrap().contains("inside the sync folder"));
        assert!(nesting_error(&project, &project).is_some());
    }

    #[test]
    fn replica_folders_get_free_names() {
        let parent = Path::new("/d/Dropbox");
        let taken = [
            parent.join("Novel.lethora-sync"),
            parent.join("Novel 2.lethora-sync"),
        ];
        assert_eq!(
            replica_folder(parent, "Essay", |p| taken.iter().any(|t| t == p)),
            parent.join("Essay.lethora-sync")
        );
        assert_eq!(
            replica_folder(parent, "Novel", |p| taken.iter().any(|t| t == p)),
            parent.join("Novel 3.lethora-sync")
        );
    }

    #[test]
    fn project_names_are_safe_and_free() {
        assert_eq!(safe_name("Notes: Part 1/2"), "Notes- Part 1-2");
        assert_eq!(safe_name("  A \"quoted\" <b>?*|\\ "), "A -quoted- -b-----");
        assert_eq!(safe_name("Tab\there"), "Tab-here");
        assert_eq!(safe_name("Draft... "), "Draft");
        assert_eq!(safe_name(" . "), "Untitled");
        assert_eq!(safe_name(""), "Untitled");
        assert_eq!(safe_name("con"), "con_");
        assert_eq!(safe_name("LPT1.final"), "LPT1_.final");
        assert_eq!(safe_name("Console"), "Console");
        assert_eq!(safe_name(&"é".repeat(150)).chars().count(), MAX_NAME);

        let dir = Path::new("/docs/Lethora");
        let taken = [dir.join("Novel.lethora"), dir.join("Novel 2.lethora")];
        let exists = |p: &Path| taken.iter().any(|t| t == p);
        assert_eq!(
            project_folder(dir, "Essay", exists),
            dir.join("Essay.lethora")
        );
        assert_eq!(
            project_folder(dir, "Novel", exists),
            dir.join("Novel 3.lethora")
        );
        assert_eq!(project_folder(dir, "a/b", exists), dir.join("a-b.lethora"));

        assert_eq!(
            joined_name(Path::new("/d/Dropbox/Lethora/Novel 2.lethora-sync")),
            "Novel 2"
        );
        assert_eq!(joined_name(Path::new("/d/X.LETHORA-SYNC")), "X");
        assert_eq!(joined_name(Path::new("/media/stick/Essay")), "Essay");
        assert_eq!(safe_name(&joined_name(Path::new("/"))), "Untitled");
    }

    /// A fake environment and file system for detecting sync apps.
    #[derive(Default)]
    struct Fake {
        vars: Vec<(&'static str, PathBuf)>,
        home: Option<PathBuf>,
        files: Vec<(PathBuf, &'static str)>,
        dirs: Vec<PathBuf>,
    }

    impl Probe for Fake {
        fn var(&self, name: &str) -> Option<PathBuf> {
            self.vars.iter().find(|v| v.0 == name).map(|v| v.1.clone())
        }
        fn home(&self) -> Option<PathBuf> {
            self.home.clone()
        }
        fn read(&self, path: &Path) -> Option<Vec<u8>> {
            let file = self.files.iter().find(|f| f.0 == path)?;
            Some(file.1.as_bytes().to_vec())
        }
        fn is_dir(&self, path: &Path) -> bool {
            self.dirs.iter().any(|d| d == path)
        }
        fn subfolders(&self, dir: &Path) -> Vec<String> {
            let names = self.dirs.iter().filter(|d| d.parent() == Some(dir));
            names
                .filter_map(|d| Some(d.file_name()?.to_string_lossy().into_owned()))
                .collect()
        }
    }

    fn labels(apps: &[SyncApp]) -> Vec<(&str, &str)> {
        apps.iter()
            .map(|a| (a.id.as_str(), a.label.as_str()))
            .collect()
    }

    // Backslash paths are only paths on Windows.
    #[cfg(windows)]
    #[test]
    fn sync_apps_on_windows() {
        let p = PathBuf::from;
        let home = p(r"C:\Users\a");
        let mut fake = Fake {
            home: Some(home.clone()),
            vars: vec![
                ("LOCALAPPDATA", p(r"C:\Users\a\AppData\Local")),
                ("APPDATA", p(r"C:\Users\a\AppData\Roaming")),
                ("OneDriveConsumer", p(r"C:\Users\a\OneDrive")),
                ("OneDriveCommercial", p(r"C:\Users\a\OneDrive - Uni Bonn")),
                // The same folder again.
                ("OneDrive", p(r"C:\Users\a\OneDrive - Uni Bonn")),
            ],
            files: vec![(
                p(r"C:\Users\a\AppData\Local\Dropbox\info.json"),
                r#"{"personal": {"path": "D:\\Dropbox", "host": 1}, "business": {"path": "D:\\Dropbox (Firma)"}}"#,
            )],
            dirs: vec![
                p(r"D:\Dropbox"),
                p(r"D:\Dropbox (Firma)"),
                p(r"C:\Users\a\OneDrive"),
                p(r"C:\Users\a\OneDrive - Uni Bonn"),
                p(r"C:\Users\a\iCloudDrive"),
                // Not the first drive letter tried.
                PathBuf::from("H:\\").join("My Drive"),
                // Not used: info.json names the Dropbox folders.
                home.join("Dropbox"),
            ],
        };
        let apps = sync_apps(Platform::Windows, &fake);
        assert_eq!(
            labels(&apps),
            [
                ("dropbox", "Dropbox"),
                ("dropbox-2", "Dropbox (Business)"),
                ("onedrive", "OneDrive"),
                ("onedrive-2", "OneDrive – Uni Bonn"),
                ("icloud", "iCloud Drive"),
                ("googledrive", "Google Drive"),
            ]
        );
        assert_eq!(apps[0].path, p(r"D:\Dropbox"));
        assert_eq!(apps[5].path, PathBuf::from("H:\\").join("My Drive"));

        // G: first; no info.json: ~/Dropbox; missing folders don't count.
        fake.files.clear();
        fake.dirs.push(PathBuf::from("G:\\").join("My Drive"));
        fake.dirs.retain(|d| d != &p(r"C:\Users\a\OneDrive"));
        let apps = sync_apps(Platform::Windows, &fake);
        assert_eq!(
            labels(&apps),
            [
                ("dropbox", "Dropbox"),
                ("onedrive", "OneDrive – Uni Bonn"),
                ("icloud", "iCloud Drive"),
                ("googledrive", "Google Drive"),
            ]
        );
        assert_eq!(apps[0].path, home.join("Dropbox"));
        assert_eq!(apps[3].path, PathBuf::from("G:\\").join("My Drive"));

        assert!(sync_apps(Platform::Windows, &Fake::default()).is_empty());
    }

    #[test]
    fn sync_apps_on_macos() {
        let home = PathBuf::from("/Users/a");
        let cloud = home.join("Library/CloudStorage");
        let fake = Fake {
            home: Some(home.clone()),
            files: vec![(
                home.join(".dropbox/info.json"),
                r#"{"personal": {"path": "/Users/a/Dropbox"}}"#,
            )],
            dirs: vec![
                home.join("Dropbox"),
                cloud.join("OneDrive-Personal"),
                cloud.join("OneDrive-Contoso"),
                cloud.join("OneDrive-SharedLibraries-Contoso"),
                cloud.join("GoogleDrive-a@example.com"),
                cloud.join("GoogleDrive-a@example.com/My Drive"),
                cloud.join("GoogleDrive-b@example.com"),
                cloud.join("GoogleDrive-b@example.com/My Drive"),
                home.join("Library/Mobile Documents/com~apple~CloudDocs"),
            ],
            ..Fake::default()
        };
        let apps = sync_apps(Platform::Mac, &fake);
        assert_eq!(
            labels(&apps),
            [
                ("dropbox", "Dropbox"),
                ("onedrive", "OneDrive – Contoso"),
                ("onedrive-2", "OneDrive"),
                ("icloud", "iCloud Drive"),
                ("googledrive", "Google Drive – a@example.com"),
                ("googledrive-2", "Google Drive – b@example.com"),
            ]
        );
        assert_eq!(
            apps[4].path,
            cloud.join("GoogleDrive-a@example.com/My Drive")
        );
    }

    #[test]
    fn replicas_are_found_by_code() {
        let base = std::env::temp_dir().join(format!("lethora-find-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let lethora = base.join("Dropbox").join(LETHORA_FOLDER);
        fs::create_dir_all(&lethora).unwrap();
        fs::create_dir_all(lethora.join("Not a replica.lethora-sync")).unwrap();
        let a = identity('a');
        let mut codes = Vec::new();
        for name in ["Novel", "Essay"] {
            let path = base.join(format!("{name}.lethora"));
            let mut doc = Document::create(&path, &a).unwrap();
            let target = replica_folder(&lethora, name, |p| p.exists());
            let (_replica, code) = doc.sync_setup(Box::new(FsStorage::new(&target))).unwrap();
            codes.push(code);
        }
        let novel = lethora.join("Novel.lethora-sync");
        let essay = lethora.join("Essay.lethora-sync");
        let dirs = [lethora.clone(), base.join("Dropbox"), base.join("missing")];
        let found = find_replicas(&dirs, &codes[0]).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, novel);
        assert_eq!(find_replicas(&dirs, &codes[1]).unwrap()[0].0, essay);
        let wrong = ProjectCode::from_key([3; 32]);
        assert!(find_replicas(&dirs, &wrong).unwrap().is_empty());
        // A replica folder itself.
        assert_eq!(
            find_replicas(std::slice::from_ref(&essay), &codes[1]).unwrap()[0].0,
            essay
        );
        // A copy of the same replica: the newest first.
        let copy = lethora.join("Novel copy.lethora-sync");
        fs::create_dir_all(copy.join("main")).unwrap();
        fs::copy(novel.join(REPLICA_FILE), copy.join(REPLICA_FILE)).unwrap();
        fs::write(copy.join("main").join("later"), "").unwrap();
        let found = find_replicas(&dirs, &codes[0]).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].0, copy);
        assert_eq!(found[0].1, found[1].1);
        // A newer format is reported when nothing matched.
        let newer = lethora.join("Newer.lethora-sync");
        fs::create_dir_all(&newer).unwrap();
        fs::write(newer.join(REPLICA_FILE), r#"{"format": 9}"#).unwrap();
        assert!(find_replicas(&dirs, &wrong).unwrap_err().contains("newer"));
        assert_eq!(find_replicas(&dirs, &codes[1]).unwrap()[0].0, essay);
        // Typos are caught before searching.
        let found = find(&dirs, "1234", &a.device, &[], Vec::new());
        assert!(found.unwrap_err().contains("characters"));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn canonical_resolves_the_existing_part() {
        let dir = std::env::temp_dir().join(format!("lethora-canon-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("not").join("yet");
        let resolved = canonical(&missing);
        assert!(resolved.ends_with(Path::new("not").join("yet")));
        assert!(is_inside(&resolved, &canonical(&dir)));
        let _ = fs::remove_dir_all(&dir);
    }

    fn identity(device: char) -> Identity {
        Identity {
            device: DeviceId::parse(&device.to_string().repeat(32)).unwrap(),
            user: lethora_core::UserId::parse(&"f".repeat(32)).unwrap(),
        }
    }

    #[test]
    fn setup_join_push_pull_reopen_and_stop() {
        let base = std::env::temp_dir().join(format!("lethora-link-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let config = base.join("config");
        let cloud = base.join("cloud");
        fs::create_dir_all(&cloud).unwrap();
        let (a, b) = (identity('a'), identity('b'));
        let path_a = base.join("Novel.lethora");
        let mut doc_a = Document::create(&path_a, &a).unwrap();
        doc_a.insert(0, "Hello").unwrap();
        let mut link_a = Link::open(&doc_a, &path_a, &a, Some(&config));
        assert_eq!(link_a.status(&doc_a).mode, "off");

        // Nesting and an existing replica are refused.
        let inner = link_a.setup(&mut doc_a, &path_a, &path_a, Some(&config));
        assert!(inner.unwrap_err().contains("inside the project"));
        let code = link_a
            .setup(&mut doc_a, &path_a, &cloud, Some(&config))
            .unwrap();
        let replica = cloud.join("Novel.lethora-sync");
        assert!(replica.join(REPLICA_FILE).is_file());
        let status = link_a.status(&doc_a);
        assert_eq!((status.mode, status.state), ("encrypted", Health::Ok));
        assert_eq!(link_a.code(&doc_a).unwrap(), code);
        let again = link_a.setup(&mut doc_a, &path_a, &cloud, Some(&config));
        assert!(again.unwrap_err().contains("synced already"));

        // A second local copy on device A is refused (found and joined).
        let copy = base.join("Copy.lethora");
        let known = [base.join("Other.lethora"), path_a.clone()];
        let found = find(
            std::slice::from_ref(&cloud),
            &code,
            &a.device,
            &known,
            Vec::new(),
        )
        .unwrap();
        assert_eq!(found.replica.as_deref(), Some(replica.as_path()));
        assert_eq!(found.local.as_deref(), Some(path_a.as_path()));
        let twice = join(&replica, &copy, &code, &a, Some(&config), &known);
        assert!(twice.err().unwrap().contains("already on this device"));
        assert!(!copy.exists());
        let elsewhere = find(
            std::slice::from_ref(&cloud),
            &code,
            &b.device,
            &known,
            Vec::new(),
        )
        .unwrap();
        assert_eq!(elsewhere.local, None);

        // Another device joins: a typo, a wrong folder, then the right code.
        let path_b = base.join("Joined.lethora");
        let mut typo: Vec<char> = code.chars().collect();
        typo[0] = if typo[0] == '0' { '1' } else { '0' };
        let typo: String = typo.into_iter().collect();
        let failed = |r: Result<(Document, Replica, ProjectCode), String>| r.err().unwrap();
        assert!(failed(join(&replica, &path_b, &typo, &b, Some(&config), &[])).contains("typo"));
        assert!(
            failed(join(&cloud, &path_b, &code, &b, Some(&config), &[])).contains("not a synced")
        );
        assert!(!path_b.exists());
        let (mut doc_b, replica_b, code_b) =
            join(&replica, &path_b, &code, &b, Some(&config), &[]).unwrap();
        assert_eq!(doc_b.text(), "Hello");
        let mut link_b = Link::joined(&path_b, &b, Some(&config), replica_b, code_b);

        // B's edit reaches A through the replica.
        doc_b.insert(5, " world").unwrap();
        link_b.push(&mut doc_b);
        let pulled = link_a.pull(&mut doc_a).unwrap();
        assert!(pulled.changed);
        assert_eq!(doc_a.text(), "Hello world");

        // Reopened with the stored key; without it the code is needed.
        drop(link_a);
        drop(doc_a);
        let doc_a = Document::open(&path_a, &a).unwrap();
        let link = Link::open(&doc_a, &path_a, &a, Some(&config));
        assert_eq!(link.status(&doc_a).state, Health::Ok);
        let project = doc_a.sync_settings().unwrap().project.clone();
        fs::remove_file(key_path(&config, &project)).unwrap();
        let mut link_a = Link::open(&doc_a, &path_a, &a, Some(&config));
        assert_eq!(link_a.status(&doc_a).state, Health::NeedsCode);
        let wrong = ProjectCode::from_key([1; 32]).to_string();
        assert!(
            link_a
                .enter_code(&doc_a, &wrong, Some(&config))
                .unwrap_err()
                .contains("does not unlock")
        );
        link_a
            .enter_code(&doc_a, &code.to_lowercase(), Some(&config))
            .unwrap();
        assert_eq!(link_a.status(&doc_a).state, Health::Ok);
        assert!(key_path(&config, &project).is_file());

        // An unplugged sync folder: unavailable, back once it returns (the
        // replica's files must be closed to move it on Windows).
        drop((link, link_a, link_b));
        let moved = base.join("away");
        fs::rename(&replica, &moved).unwrap();
        let mut doc_a = doc_a;
        let mut link_a = Link::open(&doc_a, &path_a, &a, Some(&config));
        assert_eq!(link_a.status(&doc_a).state, Health::Unavailable);
        link_a.pull(&mut doc_a).unwrap();
        assert_eq!(link_a.status(&doc_a).state, Health::Unavailable);
        fs::rename(&moved, &replica).unwrap();
        link_a.pull(&mut doc_a).unwrap();
        assert_eq!(
            link_a.status(&doc_a).state,
            Health::Unavailable,
            "retried too soon"
        );
        link_a.last_try = None;
        link_a.pull(&mut doc_a).unwrap();
        assert_eq!(link_a.status(&doc_a).state, Health::Ok);

        // Stopping keeps the replica and forgets the key.
        link_a.stop(&mut doc_a, Some(&config)).unwrap();
        assert_eq!(link_a.status(&doc_a).mode, "off");
        assert!(doc_a.sync_settings().is_none());
        assert!(!key_path(&config, &project).exists());
        assert!(replica.join(REPLICA_FILE).is_file());
        drop((doc_a, doc_b, link_a));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn keys_and_notices_round_trip() {
        let dir = std::env::temp_dir().join(format!("lethora-keys-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let code = ProjectCode::from_key([42; 32]);
        let project = "0123456789abcdef0123456789abcdef";
        assert!(read_key(&dir, project).is_none());
        write_key(&dir, project, &code).unwrap();
        assert_eq!(read_key(&dir, project), Some(code));
        fs::write(key_path(&dir, project), "zz").unwrap();
        assert!(read_key(&dir, project).is_none());

        let folder = Path::new("/home/a/Dropbox/Novel.lethora");
        assert!(!notice_dismissed(&dir, folder));
        dismiss_notice(&dir, folder).unwrap();
        dismiss_notice(&dir, folder).unwrap();
        assert!(notice_dismissed(&dir, folder));
        assert_eq!(read_notices(&dir).dismissed.len(), 1);
        assert!(!notice_dismissed(
            &dir,
            Path::new("/home/a/Dropbox/Other.lethora")
        ));
        let _ = fs::remove_dir_all(&dir);
    }
}
