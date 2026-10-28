//! Lethora desktop app: thin Tauri commands over `lethora-core`,
//! `lethora-typeset`, `lethora-spell` and `lethora-convert`, plus the
//! autosave/auto-checkpoint thread.
//!
//! All edits from the editor arrive as [`TextChange`] batches (UTF-16,
//! CodeMirror change-set semantics); undo, redo and restore answer with the
//! changes they made relative to the text before the operation.
//!
//! Layout settings, metadata, expert code and anchored overrides are
//! document changes too: they go through the document lock and are undoable
//! like edits. Every change counts a new document version, which the compile
//! thread ([`compile`]) typesets in a separate worker process.

mod compile;
mod foldersync;
mod serversync;
mod settings;
#[cfg(windows)]
mod snap;
mod updater;

pub use compile::WORKER_FLAG;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, SystemTime};

use lethora_convert as convert;
use lethora_core::protocol::envelope::Keyring;
use lethora_core::protocol::keys::DeviceKeys;
use lethora_core::{
    Checkpoint, CheckpointId, DeviceId, Document, Entries, Error, FsStorage, Identity, MapName,
    Recovered, ReviewId, ReviewItem, ServerSync, Storage, TextChange, UserId,
};
use lethora_spell::Speller;
use lethora_typeset::layout::{self, Layout, Meta, PresetInfo};
use lethora_typeset::overrides::{self, Kind, Override};
use lethora_typeset::{BibLibrary, ImportReport, JumpResult, MathRenderer, MathStyle, assets};

use crate::compile::CompileState;
use crate::foldersync::Link;
use crate::serversync::ServerLink;
use crate::settings::SettingsState;
use crate::updater::Updates;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tauri::ipc::{InvokeBody, Response};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WebviewWindow};

const EXTENSION: &str = "lethora";
/// The last opened project, before the recent list (read on upgrade).
const LAST_PROJECT: &str = "last_project.json";
const RECENT_PROJECTS: &str = "recent_projects.json";
/// Most projects in the recent list.
const RECENT_MAX: usize = 10;
const DEVICE: &str = "device.json";
const PROFILE: &str = "profile.json";
/// The personal word list: one word per line, sorted.
const DICTIONARY: &str = "dictionary.txt";
const NO_PROJECT: &str = "No project is open.";
const AUTO_CHECKPOINT_EVERY_SECS: u64 = 60;
/// Longest accepted expert Typst code, in bytes.
const MAX_CUSTOM_LEN: usize = 256 * 1024;

struct Project {
    path: PathBuf,
    doc: Document,
    /// Folder sync: the open replica and the sync status.
    link: Link,
    /// Server sync, while the project is shared on a server (and its key is
    /// here).
    server: Option<ServerLink>,
}

impl Project {
    /// Autosaves, then pushes to the replica if one is open (its failures
    /// go to the sync status); with server sync, keeps its settings and
    /// nudges its push.
    fn save(&mut self) -> lethora_core::Result<()> {
        if let Some(server) = &self.server {
            self.doc.server_save(&serversync::locked(&server.sync))?;
            server.edited();
            return Ok(());
        }
        self.doc.autosave()?;
        self.link.push(&mut self.doc);
        Ok(())
    }

    /// The sync status for the web view: server sync, a server-shared
    /// project without its key here, or folder sync.
    fn status(&self) -> foldersync::Status {
        if let Some(server) = &self.server {
            return server.status();
        }
        if let Some(settings) = self.doc.server_settings() {
            return foldersync::Status::server(
                &settings.url,
                foldersync::Health::NeedsCode,
                Some(
                    "This device has no key for this shared project. Join it again with an invite link."
                        .into(),
                ),
                None,
                None,
                None,
            );
        }
        self.link.status(&self.doc)
    }

    /// The document changed: server sync pushes soon.
    fn edited(&self) {
        if let Some(server) = &self.server {
            server.edited();
        }
    }
}

/// This device's secret keys (from `device.json`).
struct Keys(DeviceKeys);

#[derive(Default)]
struct AppState {
    project: Mutex<Option<Project>>,
    /// Counts document changes (content, layout, metadata, expert code, a
    /// different project); changed under the document lock.
    version: AtomicU64,
}

impl AppState {
    fn lock(&self) -> MutexGuard<'_, Option<Project>> {
        self.project.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }

    /// Runs `f` on the open document, mapping errors to UI messages.
    fn with<T>(
        &self,
        f: impl FnOnce(&mut Document) -> lethora_core::Result<T>,
    ) -> Result<T, String> {
        let mut guard = self.lock();
        let project = guard.as_mut().ok_or(NO_PROJECT)?;
        f(&mut project.doc).map_err(message)
    }

    /// [`AppState::with`] for a change: counts a new version if `f` succeeds.
    fn change<T>(
        &self,
        f: impl FnOnce(&mut Document) -> lethora_core::Result<T>,
    ) -> Result<T, String> {
        let mut guard = self.lock();
        let project = guard.as_mut().ok_or(NO_PROJECT)?;
        let out = f(&mut project.doc).map_err(message)?;
        self.version.fetch_add(1, Ordering::SeqCst);
        project.edited();
        Ok(out)
    }

    /// [`AppState::with`] for a change of `review` only: nothing to typeset
    /// (`main` is unchanged), but server sync pushes soon.
    fn review_change<T>(
        &self,
        f: impl FnOnce(&mut Document) -> lethora_core::Result<T>,
    ) -> Result<T, String> {
        let mut guard = self.lock();
        let project = guard.as_mut().ok_or(NO_PROJECT)?;
        let out = f(&mut project.doc).map_err(message)?;
        project.edited();
        Ok(out)
    }

    /// [`AppState::with`] for steps that fail with UI messages.
    fn try_with<T>(&self, f: impl FnOnce(&mut Document) -> Result<T, String>) -> Result<T, String> {
        let mut guard = self.lock();
        let project = guard.as_mut().ok_or(NO_PROJECT)?;
        f(&mut project.doc)
    }

    /// [`AppState::change`] for changes that fail with UI messages.
    fn try_change<T>(
        &self,
        f: impl FnOnce(&mut Document) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut guard = self.lock();
        let project = guard.as_mut().ok_or(NO_PROJECT)?;
        let out = f(&mut project.doc)?;
        self.version.fetch_add(1, Ordering::SeqCst);
        project.edited();
        Ok(out)
    }
}

/// Science view state, separate from the document so rendering never holds
/// the document lock.
#[derive(Default)]
struct TypesetState {
    math: Mutex<MathRenderer>,
}

/// Spell check: the bundled dictionaries and the personal word list, kept in
/// `file` (the app config directory's [`DICTIONARY`]).
struct Spell {
    speller: Speller,
    file: Option<PathBuf>,
    /// Held while adding a word and writing the list, so writes stay in
    /// order.
    writing: Mutex<()>,
}

impl Spell {
    /// The personal list from `dir`; none yet (or unreadable) is empty.
    fn load(dir: Option<&Path>) -> Self {
        let file = dir.map(|dir| dir.join(DICTIONARY));
        let text = file.as_ref().and_then(|f| fs::read_to_string(f).ok());
        let words = text.iter().flat_map(|t| t.lines()).map(str::to_owned);
        Self {
            speller: Speller::with_words(words),
            file,
            writing: Mutex::new(()),
        }
    }

    /// Adds `word` to the personal list and writes the list.
    fn add(&self, word: &str) -> io::Result<()> {
        let _writing = self.writing.lock().unwrap_or_else(|e| e.into_inner());
        if !self.speller.add_word(word) {
            return Ok(());
        }
        let Some(file) = &self.file else {
            return Ok(());
        };
        let mut text = self.speller.words().join("\n");
        text.push('\n');
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)?;
        }
        write_atomic(file, text.as_bytes())
    }
}

/// A rendered formula (`ok`) or why it could not be rendered (`error`).
#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum MathResult {
    Ok {
        svg: String,
        width: f64,
        height: f64,
        baseline: f64,
    },
    Error(String),
}

#[derive(Serialize)]
struct StyleInfo {
    id: &'static str,
    name: &'static str,
}

/// A freshly opened project, for the editor to load.
#[derive(Serialize)]
struct Opened {
    path: String,
    name: String,
    text: String,
    len16: usize,
    /// Own logs this open cut back (see `Document::open_at_last_good`);
    /// sent only when any.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    recovered: Vec<Recovered>,
    /// Other devices' data this open skipped (see
    /// `Document::skipped_on_open`); sent only when any.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    skipped: Vec<String>,
}

/// What opening a project came to: `{"opened": …}`, or `{"damaged": {path,
/// file}}` when a change in an own log (`file`) can't be read, to ask
/// whether to open it at the last good state (`project_open` with
/// `recover`).
#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum Opening {
    Opened(Opened),
    Damaged { path: String, file: String },
}

/// What undo, redo or restore changed, relative to the text before the
/// operation, and the UTF-16 length after it.
#[derive(Serialize)]
struct Reply {
    changes: Vec<TextChange>,
    len16: usize,
    /// Undo or redo changed the asset index (sent only when true).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    assets: bool,
}

#[derive(Serialize)]
struct CheckpointInfo {
    id: u64,
    name: Option<String>,
    auto: bool,
    created: u64,
}

impl From<&Checkpoint> for CheckpointInfo {
    fn from(c: &Checkpoint) -> Self {
        Self {
            id: c.id.0,
            name: c.name.clone(),
            auto: c.auto,
            created: c.created,
        }
    }
}

/// `last_project.json`, before the recent list.
#[derive(Deserialize)]
struct LastProject {
    path: PathBuf,
}

/// `recent_projects.json`: the projects opened most recently, newest first.
#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
struct Recent {
    paths: Vec<PathBuf>,
    /// The next start reopens the newest one (Close project clears it).
    #[serde(default)]
    reopen: bool,
}

impl Recent {
    /// Reads the list from `dir` (the app config directory), else the last
    /// opened project from before the list.
    fn read(dir: &Path) -> Self {
        if let Ok(json) = fs::read(dir.join(RECENT_PROJECTS)) {
            return serde_json::from_slice(&json).unwrap_or_default();
        }
        fs::read(dir.join(LAST_PROJECT))
            .ok()
            .and_then(|json| serde_json::from_slice::<LastProject>(&json).ok())
            .map(|last| Self {
                paths: vec![last.path],
                reopen: true,
            })
            .unwrap_or_default()
    }

    /// Writes the list to `dir`; the old last-project file goes.
    fn write(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        write_atomic(&dir.join(RECENT_PROJECTS), &serde_json::to_vec(self)?)?;
        let _ = fs::remove_file(dir.join(LAST_PROJECT));
        Ok(())
    }

    /// Puts `path` first (listed once, at most [`RECENT_MAX`] kept); the next
    /// start reopens it.
    fn add(&mut self, path: &Path) {
        self.remove(path);
        self.paths.insert(0, path.to_path_buf());
        self.paths.truncate(RECENT_MAX);
        self.reopen = true;
    }

    /// Takes `path` off the list; if it was the newest, the next start
    /// reopens nothing.
    fn remove(&mut self, path: &Path) {
        if self.paths.first().is_some_and(|p| same_path(p, path)) {
            self.reopen = false;
        }
        self.paths.retain(|p| !same_path(p, path));
    }

    /// Empties the list but for `keep` (the open project).
    fn clear(&mut self, keep: Option<&Path>) {
        self.paths
            .retain(|p| keep.is_some_and(|keep| same_path(p, keep)));
    }

    /// The project the next start reopens.
    fn last(&self) -> Option<&Path> {
        self.paths
            .first()
            .filter(|_| self.reopen)
            .map(PathBuf::as_path)
    }
}

/// A recent project, for File → Open recent and the start screen.
#[derive(Serialize)]
struct RecentProject {
    name: String,
    path: String,
    /// Its folder is still there.
    exists: bool,
}

/// `device.json`: the device id and its secret keys (lowercase hex).
#[derive(Serialize, Deserialize)]
struct DeviceFile {
    id: String,
    #[serde(default)]
    sign: String,
    #[serde(default)]
    exchange: String,
}

/// `profile.json`.
#[derive(Serialize, Deserialize)]
struct ProfileFile {
    user: String,
    name: Option<String>,
}

fn message(e: Error) -> String {
    match e {
        Error::Locked(_) => "This project is already open in another Lethora window.".into(),
        Error::AlreadyExists(dir) => {
            format!("There is already a Lethora project in {}.", dir.display())
        }
        Error::NotFound(what) => format!("Not found: {what}."),
        Error::UnsupportedFormat(why) => {
            format!("This project can't be opened: {why}.")
        }
        Error::InvalidCode(why) => format!("This project code is not valid: {why}."),
        Error::WrongCode(_) => "This project code does not unlock the synced project.".into(),
        Error::ReadOnly(role) => format!("This project is read only for you ({role})."),
        Error::NotAllowed(why) => format!("Not allowed: {why}."),
        Error::NotAuthor(what) => format!("Only its author can {what}."),
        Error::Stale(why) => format!("This suggestion can't be accepted: {why}."),
        Error::Damaged { file } => format!("A change in {file} can't be read."),
        e => e.to_string(),
    }
}

// ---- Projects ----

/// Creates a project at `path`, or without one as `<name>.lethora` in the
/// default projects folder (name made safe, numbered if taken), starting
/// from layout preset `preset` (the default preset if `None`) with metadata
/// `meta` (title, author, ...).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn project_new(
    path: Option<PathBuf>,
    name: Option<String>,
    preset: Option<String>,
    meta: Option<BTreeMap<String, Option<Value>>>,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    identity: State<'_, Identity>,
) -> Result<Opened, String> {
    let path = match path {
        Some(path) => with_extension(path),
        None => foldersync::project_folder(
            &projects_dir(&app)?,
            name.as_deref().unwrap_or_default(),
            |p| p.exists(),
        ),
    };
    let doc = create_project(
        &path,
        &identity,
        preset.as_deref(),
        meta.unwrap_or_default(),
    )?;
    Ok(install(&app, &window, &state, path, doc, None, None))
}

/// Creates a project whose initial state has `preset` and `meta` (checked
/// before anything is written). The settings are saved and the project
/// reopened, so its undo history starts empty: there is no step back to a
/// document without them.
fn create_project(
    path: &Path,
    identity: &Identity,
    preset: Option<&str>,
    meta: BTreeMap<String, Option<Value>>,
) -> Result<Document, String> {
    let preset = preset
        .map(|id| {
            layout::presets()
                .into_iter()
                .find(|p| p.id == id)
                .ok_or_else(|| format!("Unknown preset \"{id}\"."))
        })
        .transpose()?;
    let meta: BTreeMap<String, Option<Value>> = validate_changes(meta, layout::validate_meta)?
        .into_iter()
        .filter(|(_, v)| {
            v.as_ref()
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
        })
        .collect();
    let mut doc = Document::create(path, identity).map_err(message)?;
    if preset.is_none() && meta.is_empty() {
        return Ok(doc);
    }
    if let Some(preset) = preset {
        let entries: Entries = [
            (layout::SCHEMA_KEY.to_owned(), Value::from(layout::SCHEMA)),
            (layout::PRESET_KEY.to_owned(), Value::from(preset.id)),
        ]
        .into();
        doc.map_replace(MapName::Layout, &entries)
            .map_err(message)?;
    }
    if !meta.is_empty() {
        doc.map_update(MapName::Meta, &meta).map_err(message)?;
    }
    doc.autosave().map_err(message)?;
    // Releases the project lock before reopening.
    drop(doc);
    Document::open(path, identity).map_err(message)
}

/// Opens the project at `path`; with `recover`, a damaged own log is cut
/// back to its last good state (see [`Opening`]).
#[tauri::command]
async fn project_open(
    path: PathBuf,
    recover: Option<bool>,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    identity: State<'_, Identity>,
) -> Result<Opening, String> {
    if let Some(project) = state.lock().as_ref()
        && same_path(&project.path, &path)
    {
        return Ok(Opening::Opened(opened(project)));
    }
    if !path.exists() {
        update_recent(&app, |r| r.remove(&path));
        return Err(format!(
            "Not found: {} (moved, renamed or deleted?). It is no longer listed under recent projects.",
            path.display()
        ));
    }
    let doc = if recover == Some(true) {
        Document::open_at_last_good(&path, &identity)
    } else {
        Document::open(&path, &identity)
    };
    match doc {
        Ok(doc) => Ok(Opening::Opened(install(
            &app, &window, &state, path, doc, None, None,
        ))),
        Err(Error::Damaged { file }) => Ok(damaged(&path, file)),
        Err(e) => Err(message(e)),
    }
}

fn damaged(path: &Path, file: String) -> Opening {
    Opening::Damaged {
        path: path.display().to_string(),
        file,
    }
}

/// File → Close project: saves and closes the open project (its lock is
/// released, sync stops); the next start shows the start screen. If saving
/// fails, the project stays open.
#[tauri::command]
async fn project_close(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let project = {
        let mut guard = state.lock();
        if let Some(project) = guard.as_mut() {
            project
                .save()
                .map_err(|e| format!("Saving failed, so the project stays open: {}", message(e)))?;
        }
        state.version.fetch_add(1, Ordering::SeqCst);
        guard.take()
    };
    drop(project);
    update_recent(&app, |r| r.reopen = false);
    let _ = window.set_title("Lethora");
    Ok(())
}

/// The open project (e.g. after a webview reload), else the last opened one
/// unless it was closed (File → Close project) or Settings → General says
/// not to; `None` if there is none or it can't be opened. A damaged one is
/// reported (see [`Opening`]), so the start screen asks whether to recover
/// it with `project_open`.
#[tauri::command]
async fn project_last(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    identity: State<'_, Identity>,
    settings: State<'_, SettingsState>,
) -> Result<Option<Opening>, String> {
    if let Some(project) = state.lock().as_ref() {
        set_title(&window, &project.path);
        return Ok(Some(Opening::Opened(opened(project))));
    }
    if !settings.get().reopen_last_project {
        return Ok(None);
    }
    let Some(path) = last_project(&app) else {
        return Ok(None);
    };
    match Document::open(&path, &identity) {
        Ok(doc) => Ok(Some(Opening::Opened(install(
            &app, &window, &state, path, doc, None, None,
        )))),
        Err(Error::Damaged { file }) => Ok(Some(damaged(&path, file))),
        Err(e) => {
            eprintln!("Not reopening {}: {e}", path.display());
            Ok(None)
        }
    }
}

/// Makes `doc` the open project, closing (and saving) the previous one.
/// Its sync `link` is opened from its settings unless given (a join); a
/// project shared on a server follows its role there and is remembered
/// (see `serversync::Remembered`). Loose files in its `assets/` are then
/// adopted (see [`adopt_assets`]).
fn install(
    app: &AppHandle,
    window: &WebviewWindow,
    state: &AppState,
    path: PathBuf,
    mut doc: Document,
    link: Option<Link>,
    server: Option<ServerLink>,
) -> Opened {
    let link = link.unwrap_or_else(|| {
        let identity = app.state::<Identity>();
        Link::open(&doc, &path, &identity, config_dir(app).as_deref())
    });
    let server = server.or_else(|| open_server(app, &doc));
    if let Some(server) = &server {
        let sync = serversync::locked(&server.sync);
        // Read only from the start for reviewers and readers.
        doc.set_role(sync.role());
        // Remembered (also projects shared before), to reopen from the server.
        if let Some(config) = config_dir(app) {
            let entry = serversync::RememberedProject {
                server: sync.url().to_owned(),
                project: sync.project().to_owned(),
                title: project_title(&doc, &path),
                user: sync.user().to_owned(),
            };
            serversync::update_remembered(&config, |r| r.add(entry));
        }
    }
    if let Err(e) = adopt_assets(&mut doc) {
        eprintln!(
            "Adopting the loose images of {} failed: {e}",
            path.display()
        );
    }
    let project = Project {
        path,
        doc,
        link,
        server,
    };
    // What this open recovered or skipped (not repeated on a webview reload).
    let result = Opened {
        recovered: project.doc.recovered().to_vec(),
        skipped: project.doc.skipped_on_open().to_vec(),
        ..opened(&project)
    };
    set_title(window, &project.path);
    remember(app, &project.path);
    let previous = {
        let mut guard = state.lock();
        state.version.fetch_add(1, Ordering::SeqCst);
        guard.replace(project)
    };
    if let Some(mut previous) = previous
        && let Err(e) = previous.save()
    {
        let _ = app.emit(
            "save-error",
            format!("Saving the previous project failed: {}", message(e)),
        );
    }
    result
}

/// The server link of `doc` if it is shared on a server and its key is here.
fn open_server(app: &AppHandle, doc: &Document) -> Option<ServerLink> {
    let settings = doc.server_settings()?;
    let config = config_dir(app)?;
    let Some(keyring) = serversync::read_key(&config, &settings.project) else {
        eprintln!("Server sync: no key for project {}", settings.project);
        return None;
    };
    let keys = app.state::<Keys>().0.clone();
    let name = profile_name(Some(&config));
    match doc.server_open(keys, keyring, name.as_deref()) {
        Ok(sync) => Some(ServerLink::start(app, Arc::new(Mutex::new(sync)), true)),
        Err(e) => {
            eprintln!("Server sync: can't open: {e}");
            None
        }
    }
}

fn opened(project: &Project) -> Opened {
    Opened {
        path: project.path.display().to_string(),
        name: project_name(&project.path),
        text: project.doc.text(),
        len16: project.doc.len_utf16(),
        recovered: Vec::new(),
        skipped: Vec::new(),
    }
}

fn with_extension(path: PathBuf) -> PathBuf {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION))
    {
        return path;
    }
    let mut s = path.into_os_string();
    s.push(".");
    s.push(EXTENSION);
    s.into()
}

fn project_name(path: &Path) -> String {
    let name = if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION))
    {
        path.file_stem()
    } else {
        path.file_name()
    };
    name.unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn set_title(window: &WebviewWindow, path: &Path) {
    let _ = window.set_title(&format!("{} — Lethora", project_name(path)));
}

/// Reads who is editing from `dir` (the app config directory): this
/// installation's device id and its user's id, created on first start. A
/// corrupt file gets a new id: a new device id only means this device starts a
/// new set of own files in its projects, while its old files are read like
/// another device's. Device keys missing or unreadable (installs before keys
/// existed) are generated, keeping the id.
fn load_identity(dir: Option<&Path>) -> Identity {
    let storage = dir.map(FsStorage::new);
    let read = |name: &str| storage.as_ref()?.read(name).ok().flatten();
    let save = |name: &str, json: serde_json::Result<Vec<u8>>| {
        let Some(storage) = &storage else { return };
        let result = json.map_err(|e| e.to_string()).and_then(|json| {
            storage
                .create_dir("")
                .and_then(|()| storage.write_atomic(name, &json))
                .map_err(|e| e.to_string())
        });
        if let Err(e) = result {
            eprintln!("Could not write {name}: {e}");
        }
    };

    let file = read(DEVICE).and_then(|json| serde_json::from_slice::<DeviceFile>(&json).ok());
    let device = file.as_ref().and_then(|f| DeviceId::parse(&f.id).ok());
    let has_keys = file
        .as_ref()
        .is_some_and(|f| DeviceKeys::from_hex(&f.sign, &f.exchange).is_ok());
    let device = match device {
        Some(device) if has_keys => device,
        device => {
            let id = device.map_or_else(random_id, |d| d.as_str().to_owned());
            let keys = DeviceKeys::generate().expect("the OS provides random numbers");
            let (sign, exchange) = keys.to_hex();
            let file = DeviceFile {
                id: id.clone(),
                sign,
                exchange,
            };
            save(DEVICE, serde_json::to_vec(&file));
            DeviceId::parse(&id).expect("random ids are valid")
        }
    };
    let user = read(PROFILE)
        .and_then(|json| serde_json::from_slice::<ProfileFile>(&json).ok())
        .and_then(|p| UserId::parse(&p.user).ok());
    let user = user.unwrap_or_else(|| {
        let id = random_id();
        let profile = ProfileFile {
            user: id.clone(),
            name: None,
        };
        save(PROFILE, serde_json::to_vec(&profile));
        UserId::parse(&id).expect("random ids are valid")
    });
    Identity { device, user }
}

/// This device's secret keys from `device.json` in `dir` (written by
/// [`load_identity`]); new ones (lost at exit) if they can't be read.
fn load_keys(dir: Option<&Path>) -> DeviceKeys {
    dir.and_then(|dir| fs::read(dir.join(DEVICE)).ok())
        .and_then(|json| serde_json::from_slice::<DeviceFile>(&json).ok())
        .and_then(|f| DeviceKeys::from_hex(&f.sign, &f.exchange).ok())
        .unwrap_or_else(|| DeviceKeys::generate().expect("the OS provides random numbers"))
}

/// The user's display name from `profile.json` in `dir`, if set.
fn profile_name(dir: Option<&Path>) -> Option<String> {
    let json = fs::read(dir?.join(PROFILE)).ok()?;
    serde_json::from_slice::<ProfileFile>(&json)
        .ok()?
        .name
        .filter(|n| !n.trim().is_empty())
}

/// Stores the user's display name in `profile.json` in `dir` (the user id
/// stays; a failure is only logged).
fn set_profile_name(dir: Option<&Path>, name: &str) {
    let Some(dir) = dir else {
        return;
    };
    let path = dir.join(PROFILE);
    let Some(mut profile) = fs::read(&path)
        .ok()
        .and_then(|json| serde_json::from_slice::<ProfileFile>(&json).ok())
    else {
        return;
    };
    profile.name = Some(name.to_owned());
    let result = serde_json::to_vec(&profile)
        .map_err(io::Error::other)
        .and_then(|json| write_atomic(&path, &json));
    if let Err(e) = result {
        eprintln!("Could not write {PROFILE}: {e}");
    }
}

/// 16 random bytes as 32 lowercase hex characters.
fn random_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the OS provides random numbers");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn config_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok()
}

/// The default projects folder, `<Documents>/Lethora`, or `<home>/Lethora`
/// when a sync client syncs Documents (created when a project is made in
/// it).
fn projects_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let documents = app.path().document_dir().ok();
    let home = app.path().home_dir().ok();
    foldersync::default_projects_dir(documents.as_deref(), home.as_deref(), |dir, name| {
        dir.join(name).exists()
    })
    .ok_or_else(|| "Lethora can't find your Documents or home folder.".into())
}

/// The default projects folder, to show where new projects go.
#[tauri::command]
async fn projects_folder(app: AppHandle) -> Result<String, String> {
    Ok(projects_dir(&app)?.display().to_string())
}

/// The projects this device knows: the open one, the recent ones and those
/// in the default projects folder.
fn known_projects(app: &AppHandle, state: &AppState) -> Vec<PathBuf> {
    let open = state.lock().as_ref().map(|p| p.path.clone());
    let mut known: Vec<PathBuf> = open.into_iter().chain(recent(app).paths).collect();
    if let Ok(dir) = projects_dir(app) {
        known.extend(foldersync::project_folders(&dir));
    }
    known
}

fn recent(app: &AppHandle) -> Recent {
    config_dir(app)
        .map(|dir| Recent::read(&dir))
        .unwrap_or_default()
}

/// Changes the recent list (a failure to write it is only logged).
fn update_recent(app: &AppHandle, f: impl FnOnce(&mut Recent)) {
    let Some(dir) = config_dir(app) else {
        return;
    };
    let mut recent = Recent::read(&dir);
    f(&mut recent);
    if let Err(e) = recent.write(&dir) {
        eprintln!(
            "Could not write {}: {e}",
            dir.join(RECENT_PROJECTS).display()
        );
    }
}

fn last_project(app: &AppHandle) -> Option<PathBuf> {
    recent(app).last().map(Path::to_path_buf)
}

/// Puts `path` first in the recent list; the next start reopens it.
fn remember(app: &AppHandle, path: &Path) {
    update_recent(app, |r| r.add(path));
}

/// The recent projects, newest first, without the open one.
fn recent_projects(app: &AppHandle, state: &AppState) -> Vec<RecentProject> {
    let open = state.lock().as_ref().map(|p| p.path.clone());
    recent(app)
        .paths
        .into_iter()
        .filter(|p| !open.as_deref().is_some_and(|open| same_path(open, p)))
        .map(|p| RecentProject {
            name: project_name(&p),
            exists: p.exists(),
            path: p.display().to_string(),
        })
        .collect()
}

#[tauri::command]
async fn projects_recent(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<RecentProject>, String> {
    Ok(recent_projects(&app, &state))
}

/// Takes `path` off the recent projects (the project stays on disk).
#[tauri::command]
async fn projects_recent_remove(
    path: PathBuf,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<RecentProject>, String> {
    update_recent(&app, |r| r.remove(&path));
    Ok(recent_projects(&app, &state))
}

/// Clears the recent projects (the open one stays, for the next start).
#[tauri::command]
async fn projects_recent_clear(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<RecentProject>, String> {
    let open = state.lock().as_ref().map(|p| p.path.clone());
    update_recent(&app, |r| r.clear(open.as_deref()));
    Ok(recent_projects(&app, &state))
}

// ---- Editing ----

#[tauri::command]
async fn text(state: State<'_, AppState>) -> Result<String, String> {
    state.with(|doc| Ok(doc.text()))
}

/// Applies one change batch; `split` starts a new undo step first.
/// Returns the UTF-16 length afterwards.
#[tauri::command]
async fn edit(
    changes: Vec<TextChange>,
    split: bool,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    state.change(|doc| {
        if split {
            doc.end_undo_step();
        }
        doc.apply_changes(&changes)?;
        Ok(doc.len_utf16())
    })
}

#[tauri::command]
async fn undo(state: State<'_, AppState>) -> Result<Option<Reply>, String> {
    state.change(|doc| undo_step(doc, Document::undo))
}

#[tauri::command]
async fn redo(state: State<'_, AppState>) -> Result<Option<Reply>, String> {
    state.change(|doc| undo_step(doc, Document::redo))
}

/// Runs undo or redo `op`; the reply says whether the asset index changed,
/// so the status can name an asset step.
fn undo_step(
    doc: &mut Document,
    op: fn(&mut Document) -> lethora_core::Result<Option<Vec<TextChange>>>,
) -> lethora_core::Result<Option<Reply>> {
    let before = (doc.assets(), doc.asset_folders());
    Ok(op(doc)?.map(|changes| Reply {
        assets: (doc.assets(), doc.asset_folders()) != before,
        ..reply(doc, changes)
    }))
}

fn reply(doc: &Document, changes: Vec<TextChange>) -> Reply {
    Reply {
        changes,
        len16: doc.len_utf16(),
        assets: false,
    }
}

#[tauri::command]
async fn save_now(state: State<'_, AppState>) -> Result<(), String> {
    state.with(Document::autosave)
}

// ---- Checkpoints ----

#[tauri::command]
async fn checkpoints(state: State<'_, AppState>) -> Result<Vec<CheckpointInfo>, String> {
    state.with(|doc| Ok(doc.checkpoints().iter().map(CheckpointInfo::from).collect()))
}

#[tauri::command]
async fn checkpoint_create(
    name: Option<String>,
    state: State<'_, AppState>,
) -> Result<CheckpointInfo, String> {
    state.with(|doc| {
        let id = doc.checkpoint(name.as_deref(), SystemTime::now())?;
        let cp = doc.checkpoints().into_iter().find(|c| c.id == id);
        Ok(CheckpointInfo::from(&cp.expect("checkpoint just created")))
    })
}

#[tauri::command]
async fn checkpoint_text(id: u64, state: State<'_, AppState>) -> Result<String, String> {
    state.with(|doc| doc.text_at(CheckpointId(id)))
}

#[tauri::command]
async fn checkpoint_restore(id: u64, state: State<'_, AppState>) -> Result<Reply, String> {
    state.change(|doc| {
        let changes = doc.restore(CheckpointId(id))?;
        Ok(reply(doc, changes))
    })
}

/// FNV-1a (32 bit) over the UTF-16 code units of the text; the editor
/// computes the same to detect drift in development.
#[cfg(debug_assertions)]
#[tauri::command]
async fn text_hash(state: State<'_, AppState>) -> Result<u32, String> {
    state.with(|doc| {
        Ok(doc.text().encode_utf16().fold(0x811c_9dc5_u32, |h, unit| {
            (h ^ u32::from(unit)).wrapping_mul(0x0100_0193)
        }))
    })
}

// ---- Spell check ----

/// Runs `f` on the spell checker on a blocking thread: a dictionary loads on
/// first use and suggestions take a while.
async fn with_spell<R: Send + 'static>(
    app: AppHandle,
    f: impl FnOnce(&Spell) -> R + Send + 'static,
) -> Result<R, String> {
    tauri::async_runtime::spawn_blocking(move || f(&app.state::<Spell>()))
        .await
        .map_err(|e| e.to_string())
}

/// The words of `words` misspelled in `lang` (a `text.lang`); none for a
/// language without a dictionary.
#[tauri::command]
async fn spell_check(
    lang: String,
    words: Vec<String>,
    app: AppHandle,
) -> Result<Vec<String>, String> {
    let Some(lang) = lethora_spell::dictionary_for(&lang) else {
        return Ok(Vec::new());
    };
    with_spell(app, move |spell| spell.speller.check(lang, &words)).await
}

/// Corrections for `word` in `lang`, best first.
#[tauri::command]
async fn spell_suggest(lang: String, word: String, app: AppHandle) -> Result<Vec<String>, String> {
    let Some(lang) = lethora_spell::dictionary_for(&lang) else {
        return Ok(Vec::new());
    };
    with_spell(app, move |spell| spell.speller.suggest(lang, &word)).await
}

/// Adds `word` to the personal word list (all projects, this computer).
#[tauri::command]
async fn spell_add(word: String, app: AppHandle) -> Result<(), String> {
    with_spell(app, move |spell| spell.add(&word))
        .await?
        .map_err(|e| format!("Could not save the word to your dictionary: {e}"))
}

// ---- Science view: math and sources ----

/// Renders one formula; sizes in points, `color` as `#rrggbb`.
#[tauri::command]
async fn render_math(
    source: String,
    display: bool,
    size_pt: f64,
    color: String,
    state: State<'_, TypesetState>,
) -> Result<MathResult, String> {
    let style = MathStyle { size_pt, color };
    let mut renderer = state.math.lock().unwrap_or_else(|e| e.into_inner());
    Ok(match renderer.render_math(&source, display, &style) {
        Ok(r) => MathResult::Ok {
            svg: r.svg,
            width: r.width_pt,
            height: r.height_pt,
            baseline: r.baseline_pt,
        },
        Err(e) => MathResult::Error(e),
    })
}

/// The document's sources, after merging an outside edit of the
/// `references.bib` mirror (Zotero, JabRef, by hand) into them. Runs on
/// project load, window focus and F5; a merge that changed the sources counts
/// a new version.
#[tauri::command]
async fn bib_entries(state: State<'_, AppState>) -> Result<BibLibrary, String> {
    let text = {
        let mut guard = state.lock();
        let doc = &mut guard.as_mut().ok_or(NO_PROJECT)?.doc;
        if doc.merge_bibliography_mirror().map_err(message)? {
            state.version.fetch_add(1, Ordering::SeqCst);
        }
        doc.bibliography()
    };
    Ok(lethora_typeset::parse_bib(&text))
}

/// Merges BibLaTeX `text` into the sources, skipping known keys; one undo
/// step, and a new version if something was added.
#[tauri::command]
async fn bib_import(text: String, state: State<'_, AppState>) -> Result<ImportReport, String> {
    import_references(&state, &text)
}

/// Merges the BibLaTeX file at `path` into the sources (as `bib_import`).
#[tauri::command]
async fn bib_import_file(
    path: PathBuf,
    state: State<'_, AppState>,
) -> Result<ImportReport, String> {
    let bytes = fs::read(&path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let text =
        String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8 text.", path.display()))?;
    import_references(&state, text.trim_start_matches('\u{feff}'))
}

/// Merges `text` into the document's sources under the document lock.
fn import_references(state: &AppState, text: &str) -> Result<ImportReport, String> {
    let mut guard = state.lock();
    let doc = &mut guard.as_mut().ok_or(NO_PROJECT)?.doc;
    let (merged, report) = lethora_typeset::import_bib(&doc.bibliography(), text)?;
    if !report.added.is_empty() {
        doc.set_bibliography(&merged).map_err(message)?;
        state.version.fetch_add(1, Ordering::SeqCst);
    }
    Ok(report)
}

/// The source `key` as a plain-text reference in citation style `style`.
#[tauri::command]
async fn bib_format(
    key: String,
    style: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let text = state.with(|doc| Ok(doc.bibliography()))?;
    lethora_typeset::format_reference(&text, &key, &style)
}

#[tauri::command]
async fn bib_styles() -> Result<Vec<StyleInfo>, String> {
    Ok(lethora_typeset::styles()
        .into_iter()
        .map(|(id, name)| StyleInfo { id, name })
        .collect())
}

// ---- Science view: figures ----

/// Imports the file at `path` into `folder` (`assets/` if `None`, else a
/// folder path ending in `/`): a blob plus an index entry under a new, safe
/// path, and the folder's marker; one undo step, a new version. If the same
/// content in the same format is indexed in that folder already, that path
/// is reused. Returns the path the text uses (`assets/<name>`).
#[tauri::command]
async fn asset_import(
    path: PathBuf,
    folder: Option<String>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let folder = import_folder(folder)?;
    let (ext, bytes) = assets::read_source(&path)?;
    state.try_change(|doc| import_asset(doc, &path, &ext, &bytes, &folder))
}

/// Header of [`asset_import_bytes`] with the file's original name.
const NAME_HEADER: &str = "x-file-name";
/// Header of [`asset_import_bytes`] with the target folder.
const FOLDER_HEADER: &str = "x-folder";

/// Imports a file the web view read (drag & drop, later the browser build):
/// the raw bytes as the request body, the original file name and optionally
/// the target folder percent-encoded (`encodeURIComponent`) in the
/// `x-file-name` and `x-folder` headers. Checked and stored like
/// `asset_import`; returns the path.
#[tauri::command]
async fn asset_import_bytes(
    request: tauri::ipc::Request<'_>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let (bytes, name) = bytes_request(&request)?;
    let folder = header(&request, FOLDER_HEADER)?;
    state.try_change(|doc| import_bytes(doc, &name, folder, bytes))
}

/// The body and the file name of a raw-bytes request ([`asset_import_bytes`]).
fn bytes_request<'r>(request: &'r tauri::ipc::Request<'_>) -> Result<(&'r [u8], String), String> {
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err("The file's bytes are missing.".into());
    };
    let name = header(request, NAME_HEADER)?.ok_or("The file name is missing.")?;
    Ok((bytes, name))
}

/// The percent-encoded header `name` of `request`, decoded.
fn header(request: &tauri::ipc::Request<'_>, name: &str) -> Result<Option<String>, String> {
    request
        .headers()
        .get(name)
        .map(|v| {
            v.to_str()
                .ok()
                .and_then(percent_decode)
                .ok_or_else(|| format!("The {name} header is not percent-encoded UTF-8."))
        })
        .transpose()
}

/// See [`asset_import_bytes`].
fn import_bytes(
    doc: &mut Document,
    name: &str,
    folder: Option<String>,
    bytes: &[u8],
) -> Result<String, String> {
    let folder = import_folder(folder)?;
    let (source, ext) = bytes_source(name, bytes)?;
    import_asset(doc, source, &ext, bytes, &folder)
}

/// The file name of a file the web view read (only the name counts,
/// whichever separators it came with) and its checked extension.
fn bytes_source<'n>(name: &'n str, bytes: &[u8]) -> Result<(&'n Path, String), String> {
    let source = Path::new(name.rsplit(['/', '\\']).next().unwrap_or(name));
    let (_, ext) = assets::check_import(source, bytes.len() as u64)?;
    Ok((source, ext))
}

/// Decodes `%XX` escapes (as `encodeURIComponent` writes them) to UTF-8.
fn percent_decode(s: &str) -> Option<String> {
    let mut out = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let mut digit = || char::from(bytes.next()?).to_digit(16);
            let (hi, lo) = (digit()?, digit()?);
            out.push((hi * 16 + lo) as u8);
        } else {
            out.push(b);
        }
    }
    String::from_utf8(out).ok()
}

/// The validated import folder: `folder`, or `assets/` if `None`.
fn import_folder(folder: Option<String>) -> Result<String, String> {
    let folder = folder.unwrap_or_else(|| format!("{}/", assets::DIR));
    assets::validate_folder(&folder)?;
    Ok(folder)
}

fn import_asset(
    doc: &mut Document,
    source: &Path,
    ext: &str,
    bytes: &[u8],
    folder: &str,
) -> Result<String, String> {
    let path = asset_path(doc, source, ext, bytes, folder, &[])?;
    doc.put_asset(&path, bytes).map_err(message)?;
    Ok(path)
}

/// The path in `folder` to store `bytes` (format `ext`, picked as `source`)
/// under: a path directly in `folder` holding the same content in the same
/// format, else a new, safe one that is neither indexed nor in `taken`.
fn asset_path(
    doc: &Document,
    source: &Path,
    ext: &str,
    bytes: &[u8],
    folder: &str,
    taken: &[String],
) -> Result<String, String> {
    let index = doc.assets();
    let name = doc.asset_blob_name(bytes);
    let same = |(path, blob): &(&String, &String)| {
        Some(*blob) == name.as_ref()
            && path
                .strip_prefix(folder)
                .is_some_and(|rest| !rest.contains('/'))
            && assets::validate_relative(path).is_ok()
            && assets::asset_type(Path::new(path))
                .is_some_and(|(_, e)| assets::same_format(&e, ext))
    };
    if let Some((path, _)) = index.iter().find(same) {
        return Ok(path.clone());
    }
    assets::import_path(source, ext, folder, |p| {
        index.contains_key(p) || taken.iter().any(|t| t == p)
    })
}

/// Header of [`asset_stage_bytes`] with the paths proposed already in the
/// same drop: each percent-encoded, separated by `,`.
const TAKEN_HEADER: &str = "x-taken";

/// A staged file: its blob is stored, `path` is proposed for it (not yet
/// indexed; [`figure_insert`] indexes it).
#[derive(Serialize, Deserialize)]
struct Staged {
    path: String,
    blob: String,
}

/// Stages the file at `path` for [`figure_insert`]: checked and read like
/// `asset_import`, its blob stored, and a path in `folder` (`assets/` if
/// `None`) proposed as `asset_import` would pick it, but never one of
/// `taken` (proposed already for the same insert). Changes no version.
#[tauri::command]
async fn asset_stage(
    path: PathBuf,
    folder: Option<String>,
    taken: Vec<String>,
    state: State<'_, AppState>,
) -> Result<Staged, String> {
    let folder = import_folder(folder)?;
    let (ext, bytes) = assets::read_source(&path)?;
    state.try_with(|doc| stage_asset(doc, &path, &ext, &bytes, &folder, &taken))
}

/// [`asset_stage`] for a file the web view read (drag & drop), sent like
/// [`asset_import_bytes`]; the `x-taken` header lists the proposed paths.
#[tauri::command]
async fn asset_stage_bytes(
    request: tauri::ipc::Request<'_>,
    state: State<'_, AppState>,
) -> Result<Staged, String> {
    let (bytes, name) = bytes_request(&request)?;
    let folder = import_folder(header(&request, FOLDER_HEADER)?)?;
    let taken = match request.headers().get(TAKEN_HEADER) {
        Some(v) => v
            .to_str()
            .ok()
            .and_then(taken_list)
            .ok_or("The x-taken header is not a list of percent-encoded paths.")?,
        None => Vec::new(),
    };
    let (source, ext) = bytes_source(&name, bytes)?;
    state.try_with(|doc| stage_asset(doc, source, &ext, bytes, &folder, &taken))
}

/// The paths of an `x-taken` header (see [`TAKEN_HEADER`]).
fn taken_list(value: &str) -> Option<Vec<String>> {
    value
        .split(',')
        .filter(|p| !p.is_empty())
        .map(percent_decode)
        .collect()
}

/// See [`asset_stage`].
fn stage_asset(
    doc: &mut Document,
    source: &Path,
    ext: &str,
    bytes: &[u8],
    folder: &str,
    taken: &[String],
) -> Result<Staged, String> {
    let blob = doc.stage_blob(bytes).map_err(message)?;
    let path = asset_path(doc, source, ext, bytes, folder, taken)?;
    Ok(Staged { path, blob })
}

/// Inserts `text` (figures, built by the web view) at UTF-16 offset `pos`
/// and indexes the staged `assets` under their proposed paths, with their
/// folders' markers: one undo step, a new version. Answers like a restore.
/// Fails, changing nothing, if a path is invalid or was given other
/// content since it was proposed.
#[tauri::command]
async fn figure_insert(
    pos: usize,
    text: String,
    assets: Vec<Staged>,
    state: State<'_, AppState>,
) -> Result<Reply, String> {
    state.try_change(|doc| insert_figures(doc, pos, &text, &assets))
}

/// See [`figure_insert`].
fn insert_figures(
    doc: &mut Document,
    pos: usize,
    text: &str,
    staged: &[Staged],
) -> Result<Reply, String> {
    for s in staged {
        assets::validate_relative(&s.path)?;
        if doc.asset_blob(&s.path).is_some_and(|blob| blob != s.blob) {
            return Err(format!(
                "{} was just taken by another file; try again.",
                s.path
            ));
        }
    }
    let entries: Vec<(String, String)> = staged
        .iter()
        .map(|s| (s.path.clone(), s.blob.clone()))
        .collect();
    let changes = doc
        .insert_with_assets(pos, text, &entries)
        .map_err(message)?;
    Ok(reply(doc, changes))
}

// ---- Import (File → Import…, Edit → Insert file…) ----

/// The open dialog's filters for importable files, all of them first.
#[tauri::command]
async fn import_formats() -> Result<Vec<convert::registry::Filter>, String> {
    Ok(convert::registry::import_filters())
}

/// What [`import_read`] made of a file.
#[derive(Serialize)]
struct ImportReply {
    /// The format's name ("Markdown").
    format: &'static str,
    /// Lethora markup, ending in a line break (empty if nothing came out).
    text: String,
    /// Its images, staged for [`figure_insert`].
    assets: Vec<Staged>,
    /// What was dropped or approximated.
    warnings: Vec<convert::Warning>,
    title: Option<String>,
    /// The authors, joined by ", ".
    author: Option<String>,
}

/// Reads the file at `path` in the format its name (or content) says and
/// converts it to markup, citing only the open project's sources. With
/// `stage`, its images are stored as blobs under proposed paths
/// `assets/<file stem>/<image>` for [`figure_insert`], which inserts the
/// text and indexes them in one undo step; without, no project is needed
/// and images are left out (the New project dialog's title, a suggestion).
/// Changes no version.
#[tauri::command]
async fn import_read(
    path: PathBuf,
    stage: bool,
    state: State<'_, AppState>,
) -> Result<ImportReply, String> {
    let (format, imported) = read_import(&path)?;
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let folder = format!("{}/{}/", assets::DIR, assets::sanitize_stem(&stem));
    let mut guard = state.lock();
    let doc = guard.as_mut().map(|p| &mut p.doc);
    if stage && doc.is_none() {
        return Err(NO_PROJECT.into());
    }
    import_markup(format.name, imported, doc.filter(|_| stage), &folder)
}

/// Reads `path` with the registry's reader for it; images it names are read
/// from next to it (at most an image's import size).
fn read_import(
    path: &Path,
) -> Result<(&'static convert::registry::Format, convert::Imported), String> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let len = fs::metadata(path)
        .map_err(|e| format!("Could not read {}: {e}", path.display()))?
        .len();
    if len > convert::MAX_INPUT_BYTES as u64 {
        return Err(format!("{name}: {}", convert::Error::TooLarge));
    }
    let bytes = fs::read(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let (format, reader) = convert::registry::reader_for(&name, &bytes)
        .and_then(|f| Some((f, f.reader?)))
        .ok_or_else(|| format!("{name} is not in a format Lethora can import."))?;
    let base = path.parent().map(Path::to_owned).unwrap_or_default();
    let resolve = |rel: &str| {
        let file = base.join(rel);
        let meta = fs::metadata(&file).ok()?;
        (meta.is_file() && meta.len() <= assets::MAX_BYTES)
            .then(|| fs::read(&file).ok())
            .flatten()
    };
    let options = convert::ReadOptions {
        name: &name,
        resolve: &resolve,
    };
    let imported = reader
        .read(&bytes, &options)
        .map_err(|e| format!("{name}: {e}"))?;
    Ok((format, imported))
}

/// The reply for `imported`: with `doc`, its images staged in `folder`
/// (unsupported or too large ones left out) and its sources' keys known.
fn import_markup(
    format: &'static str,
    imported: convert::Imported,
    doc: Option<&mut Document>,
    folder: &str,
) -> Result<ImportReply, String> {
    let mut staged = Vec::new();
    let mut paths = BTreeMap::new();
    let mut keys = BTreeSet::new();
    if let Some(doc) = doc {
        keys = lethora_typeset::parse_bib(&doc.bibliography())
            .entries
            .into_iter()
            .map(|e| e.key)
            .collect();
        for r in &imported.doc.resources {
            let source = Path::new(&r.name);
            let Ok((_, ext)) = assets::check_import(source, r.bytes.len() as u64) else {
                continue;
            };
            let taken: Vec<String> = staged.iter().map(|s: &Staged| s.path.clone()).collect();
            let s = stage_asset(doc, source, &ext, &r.bytes, folder, &taken)?;
            paths.insert(r.name.clone(), s.path.clone());
            staged.push(s);
        }
    }
    let options = convert::typst::MarkupOptions {
        assets: &paths,
        keys: &keys,
    };
    let markup = convert::typst::to_markup(&imported.doc, &options).map_err(|e| e.to_string())?;
    let mut warnings = convert::Warnings::default();
    warnings.extend(imported.warnings);
    warnings.extend(markup.warnings);
    let meta = imported.doc.meta;
    Ok(ImportReply {
        format,
        text: markup.text,
        assets: staged,
        warnings: warnings.into_vec(),
        title: meta.title,
        author: Some(meta.authors.join(", ")).filter(|a| !a.is_empty()),
    })
}

/// What `asset_replace` and `asset_move` changed: the new path, and the
/// text changes like a restore's reply.
#[derive(Serialize)]
struct Replaced {
    path: String,
    #[serde(flatten)]
    reply: Reply,
}

/// Replaces the image of the image call at UTF-16 offset `pos`, whose path
/// literal must read `path`, with the image file `source`: stored like an
/// import (any format) and only that literal changed to the new path; the
/// old entry stays for other figures and checkpoints. One undo step, a new
/// version.
#[tauri::command]
async fn asset_replace(
    path: String,
    source: PathBuf,
    pos: usize,
    state: State<'_, AppState>,
) -> Result<Replaced, String> {
    let (ext, bytes) = assets::read_source(&source)?;
    state.try_change(|doc| replace_asset(doc, &path, pos, &source, &ext, &bytes))
}

fn replace_asset(
    doc: &mut Document,
    path: &str,
    pos: usize,
    source: &Path,
    ext: &str,
    bytes: &[u8],
) -> Result<Replaced, String> {
    assets::validate_relative(path)?;
    let (from, to) = image_path_range(&doc.text(), pos, path)
        .ok_or_else(|| format!("The image {path} is no longer there; try again."))?;
    let new = asset_path(doc, source, ext, bytes, &format!("{}/", assets::DIR), &[])?;
    let changes = doc
        .repoint_asset(from, to, path, &new, bytes)
        .map_err(message)?;
    Ok(Replaced {
        path: new,
        reply: reply(doc, changes),
    })
}

/// The UTF-16 range of the path in the image call at UTF-16 offset `pos`
/// (`image("…"`, after an optional `#`), if that literal reads `path`.
fn image_path_range(text: &str, pos: usize, path: &str) -> Option<(usize, usize)> {
    let start = utf16_to_byte(text, pos).ok()?;
    let call = &text[start..];
    let args = call
        .strip_prefix('#')
        .unwrap_or(call)
        .strip_prefix("image(")?;
    let literal = args.trim_start().strip_prefix('"')?;
    let end = literal.find(['"', '\\', '\n'])?;
    if &literal[..end] != path || !literal[end..].starts_with('"') {
        return None;
    }
    let from = pos + byte_to_utf16(call, call.len() - literal.len());
    Some((from, from + path.encode_utf16().count()))
}

/// The raw bytes of the image the text refers to as `path` (`assets/...`,
/// validated), read from its blob.
#[tauri::command]
async fn asset_bytes(path: String, state: State<'_, AppState>) -> Result<Response, String> {
    assets::validate_relative(&path)?;
    let (dir, blob) = {
        let guard = state.lock();
        let project = guard.as_ref().ok_or(NO_PROJECT)?;
        (project.path.clone(), project.doc.asset_blob(&path))
    };
    let blob = blob.ok_or_else(|| format!("Image not found: {path}"))?;
    Ok(Response::new(assets::read_blob(&dir, &blob, &path)?))
}

/// Gives the asset at `path` the content of the file `source`, which must
/// be of the same format (jpg and jpeg are one): every figure using the
/// path shows the new file. One undo step, a new version; the old blob
/// stays for checkpoints.
#[tauri::command]
async fn asset_update(
    path: String,
    source: PathBuf,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let (ext, bytes) = assets::read_source(&source)?;
    state.try_change(|doc| update_asset(doc, &path, &ext, &bytes))
}

/// See [`asset_update`].
fn update_asset(doc: &mut Document, path: &str, ext: &str, bytes: &[u8]) -> Result<(), String> {
    assets::validate_relative(path)?;
    if doc.asset_blob(path).is_none() {
        return Err(format!("There is no asset {path}."));
    }
    let (_, own) = assets::asset_type(Path::new(path))
        .ok_or_else(|| format!("Not an asset path: \"{path}\"."))?;
    if !assets::same_format(&own, ext) {
        return Err(format!(
            "Pick a .{own} file to replace {path} (this one is .{ext})."
        ));
    }
    doc.put_asset(path, bytes).map_err(message)
}

/// Writes the bytes of the asset at `path` to the file `target` (atomically,
/// like a PDF export).
#[tauri::command]
async fn asset_save(
    path: String,
    target: PathBuf,
    state: State<'_, AppState>,
) -> Result<(), String> {
    assets::validate_relative(&path)?;
    let (dir, blob) = {
        let guard = state.lock();
        let project = guard.as_ref().ok_or(NO_PROJECT)?;
        (project.path.clone(), project.doc.asset_blob(&path))
    };
    save_asset(&dir, blob.as_deref(), &path, &target)
}

/// See [`asset_save`]: `blob` is the indexed blob of `path` in project `dir`.
fn save_asset(dir: &Path, blob: Option<&str>, path: &str, target: &Path) -> Result<(), String> {
    let blob = blob.ok_or_else(|| format!("Image not found: {path}"))?;
    let bytes = assets::read_blob(dir, blob, path)?;
    write_atomic(target, &bytes).map_err(|e| format!("Could not write {}: {e}", target.display()))
}

/// The asset explorer's listing.
#[derive(Serialize)]
struct AssetList {
    /// Every folder below `assets/` (ending in `/`), from markers and paths.
    folders: Vec<String>,
    items: Vec<AssetItem>,
}

#[derive(Serialize)]
struct AssetItem {
    path: String,
    /// The asset type (`image`).
    kind: &'static str,
    /// Bytes of the blob; `None` while it is missing.
    size: Option<u64>,
    /// How many string literals in the text name it.
    used_by: usize,
    status: AssetStatus,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum AssetStatus {
    Available,
    /// The blob is not there (e.g. it has not arrived from another device).
    Missing,
}

/// The indexed assets with valid paths and the folders, sorted by path.
#[tauri::command]
async fn asset_list(state: State<'_, AppState>) -> Result<AssetList, String> {
    state.with(|doc| asset_listing(doc))
}

/// See [`asset_list`].
fn asset_listing(doc: &Document) -> lethora_core::Result<AssetList> {
    let usage = doc.asset_usage();
    let items = doc
        .asset_sizes()?
        .into_iter()
        .filter(|(path, _)| assets::validate_relative(path).is_ok())
        .filter_map(|(path, size)| {
            let (t, _) = assets::asset_type(Path::new(&path))?;
            Some(AssetItem {
                kind: t.kind,
                size,
                used_by: usage.get(&path).copied().unwrap_or(0),
                status: match size {
                    Some(_) => AssetStatus::Available,
                    None => AssetStatus::Missing,
                },
                path,
            })
        })
        .collect();
    Ok(AssetList {
        folders: listed_folders(doc).into_iter().collect(),
        items,
    })
}

/// The folders below `assets/`: the valid markers, and every folder above a
/// valid marker or item path.
fn listed_folders(doc: &Document) -> BTreeSet<String> {
    let items = doc
        .assets()
        .into_keys()
        .filter(|p| assets::validate_relative(p).is_ok());
    let markers = doc
        .asset_folders()
        .into_iter()
        .filter(|p| assets::validate_folder(p).is_ok());
    let mut folders = BTreeSet::new();
    for path in items.chain(markers) {
        // Each `/` after the first ends a folder.
        for (i, _) in path.match_indices('/').skip(1) {
            folders.insert(path[..=i].to_owned());
        }
    }
    folders
}

/// The index entries (items and markers) at or below `folder`.
fn folder_entries(doc: &Document, folder: &str) -> Vec<String> {
    let markers = doc.asset_folders();
    doc.assets()
        .into_keys()
        .chain(markers)
        .filter(|p| p.starts_with(folder))
        .collect()
}

/// Whether `folder` is the root `assets/`.
fn is_root(folder: &str) -> bool {
    folder.strip_suffix('/') == Some(assets::DIR)
}

/// Renames or moves the item or folder (a path ending in `/`) `from` to
/// `to`: every entry and marker below a folder moves along, and every string
/// literal in the text naming a moved item is changed (one undo step, a new
/// version). New names are free, but an item keeps its format (see
/// [`assets::validate_rename`]). Answers like `asset_replace`.
#[tauri::command]
async fn asset_move(
    from: String,
    to: String,
    state: State<'_, AppState>,
) -> Result<Replaced, String> {
    state.try_change(|doc| move_asset(doc, &from, &to))
}

/// See [`asset_move`].
fn move_asset(doc: &mut Document, from: &str, to: &str) -> Result<Replaced, String> {
    let moves = if from.ends_with('/') {
        assets::validate_folder(from)?;
        assets::validate_folder(to)?;
        if is_root(from) || is_root(to) {
            return Err("The assets folder itself can't be moved.".into());
        }
        if to.starts_with(from) && to != from {
            return Err("A folder can't move into itself.".into());
        }
        let entries = folder_entries(doc, from);
        if entries.is_empty() {
            return Err(format!("There is no folder {from}."));
        }
        let mut moves = Vec::with_capacity(entries.len());
        for old in entries {
            let new = format!("{to}{}", &old[from.len()..]);
            if new.ends_with('/') {
                assets::validate_folder(&new)?;
            } else {
                assets::validate_relative(&new)?;
            }
            moves.push((old, new));
        }
        moves
    } else {
        assets::validate_relative(from)?;
        assets::validate_rename(from, to)?;
        vec![(from.to_owned(), to.to_owned())]
    };
    let changes = doc.move_assets(&moves).map_err(message)?;
    Ok(Replaced {
        path: to.to_owned(),
        reply: reply(doc, changes),
    })
}

/// Removes the item or folder (a path ending in `/`, with its subfolders and
/// all their items) `path` from the index (one undo step, a new version);
/// the blobs stay for checkpoints. Refused if the text uses an item in it.
#[tauri::command]
async fn asset_remove(path: String, state: State<'_, AppState>) -> Result<(), String> {
    state.try_change(|doc| remove_asset(doc, &path))
}

/// See [`asset_remove`].
fn remove_asset(doc: &mut Document, path: &str) -> Result<(), String> {
    let paths = if path.ends_with('/') {
        assets::validate_folder(path)?;
        if is_root(path) {
            return Err("The assets folder itself can't be removed.".into());
        }
        let entries = folder_entries(doc, path);
        if entries.is_empty() {
            return Err(format!("There is no folder {path}."));
        }
        entries
    } else {
        vec![path.to_owned()]
    };
    doc.remove_assets(&paths).map_err(message)
}

/// Creates the (empty) folder `path` (ending in `/`; one undo step, a new
/// version); an existing folder changes nothing.
#[tauri::command]
async fn asset_folder_create(path: String, state: State<'_, AppState>) -> Result<(), String> {
    assets::validate_folder(&path)?;
    state.try_change(|doc| create_folder(doc, &path))
}

/// See [`asset_folder_create`].
fn create_folder(doc: &mut Document, path: &str) -> Result<(), String> {
    if is_root(path) || listed_folders(doc).contains(path) {
        return Ok(());
    }
    doc.add_asset_folder(path).map_err(message)
}

/// Adopts the loose image files in the project's `assets/` (from before the
/// asset index, or copied in by hand) into the document; they move to
/// `backup/assets/`. Runs on open and F5.
fn adopt_assets(doc: &mut Document) -> lethora_core::Result<Vec<String>> {
    doc.adopt_loose_assets(|path| assets::validate_relative(path).is_ok())
}

/// Writes `path.tmp`, syncs it and renames it over `path`.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut file = File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path)
}

// ---- Layout layer ----

/// The layout settings as the settings UI needs them.
#[derive(Serialize)]
struct LayoutState {
    /// The preset id.
    preset: &'static str,
    /// Every setting, resolved (preset + overrides), with the character
    /// styles under `styles`.
    layout: Layout,
    /// The settings that differ from the preset, by dotted key.
    overrides: Entries,
    meta: Meta,
    /// Expert Typst code.
    custom: String,
    /// The text area the science view sizes figures against.
    text_area: layout::TextArea,
}

fn layout_state(doc: &Document) -> LayoutState {
    let entries = doc.map_entries(MapName::Layout);
    let preset = layout::preset_id(&entries);
    let resolved = layout::resolve(&entries);
    LayoutState {
        preset,
        overrides: layout::diff(preset, &resolved),
        text_area: layout::text_area(&resolved),
        layout: resolved,
        meta: layout::resolve_meta(&doc.map_entries(MapName::Meta)),
        custom: doc.custom_text(),
    }
}

/// Validates every change (`null` deletes); fails listing all invalid keys.
fn validate_changes(
    changes: BTreeMap<String, Option<Value>>,
    validate: impl Fn(&str, &Value) -> Result<Value, String>,
) -> Result<BTreeMap<String, Option<Value>>, String> {
    let mut valid = BTreeMap::new();
    let mut invalid = Vec::new();
    for (key, value) in changes {
        match value.map(|v| validate(&key, &v)).transpose() {
            Ok(value) => {
                valid.insert(key, value);
            }
            Err(e) => invalid.push(e),
        }
    }
    if invalid.is_empty() {
        Ok(valid)
    } else {
        Err(format!(
            "Invalid settings (nothing was changed): {}",
            invalid.join(" ")
        ))
    }
}

#[tauri::command]
async fn layout_get(state: State<'_, AppState>) -> Result<LayoutState, String> {
    state.with(|doc| Ok(layout_state(doc)))
}

/// Overrides layout settings by dotted key; `null` returns a setting to the
/// preset's value. All or nothing. Fonts: any family `layout_fonts` lists
/// (a character style's: any family name, matched to that list's spelling).
#[tauri::command]
async fn layout_set(
    changes: BTreeMap<String, Option<Value>>,
    state: State<'_, AppState>,
    compile: State<'_, CompileState>,
) -> Result<LayoutState, String> {
    let sets_font = changes.iter().any(|(key, value)| {
        value.is_some()
            && (key == "text.font" || layout::style_key(key).is_some_and(|(_, f)| f == "font"))
    });
    let fonts = if sets_font {
        compile.fonts().await
    } else {
        layout::fonts()
    };
    let mut changes = validate_changes(changes, |key, value| {
        layout::validate_change_in(key, value, &fonts)
    })?;
    changes.insert(layout::SCHEMA_KEY.into(), Some(layout::SCHEMA.into()));
    state.change(|doc| {
        doc.map_update(MapName::Layout, &changes)?;
        Ok(layout_state(doc))
    })
}

/// Switches to preset `id`, dropping all overrides (one undo step).
#[tauri::command]
async fn layout_apply_preset(
    id: String,
    state: State<'_, AppState>,
) -> Result<LayoutState, String> {
    let preset = layout::presets()
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("Unknown preset \"{id}\"."))?;
    let entries: Entries = [
        (layout::SCHEMA_KEY.to_owned(), Value::from(layout::SCHEMA)),
        (layout::PRESET_KEY.to_owned(), Value::from(preset.id)),
    ]
    .into();
    state.change(|doc| {
        doc.end_undo_step();
        doc.map_replace(MapName::Layout, &entries)?;
        doc.end_undo_step();
        Ok(layout_state(doc))
    })
}

#[tauri::command]
async fn layout_presets() -> Result<Vec<PresetInfo>, String> {
    Ok(layout::presets())
}

/// The font families documents can use: embedded and the system's.
#[tauri::command]
async fn layout_fonts(compile: State<'_, CompileState>) -> Result<Vec<String>, String> {
    Ok(compile.fonts().await)
}

/// The embedded font families (part of `layout_fonts`; the web view lacks
/// them, see `layout_font_file`).
#[tauri::command]
async fn layout_embedded_fonts() -> Result<Vec<String>, String> {
    Ok(layout::fonts())
}

/// The font file of an embedded family's regular face, for the editor's
/// previews; empty for other families (the web view finds the system's by
/// name).
#[tauri::command]
async fn layout_font_file(family: String) -> Result<Response, String> {
    Ok(Response::new(
        layout::embedded_font(&family).map_or_else(Vec::new, <[u8]>::to_vec),
    ))
}

/// Sets metadata fields (title, subtitle, author, date, contact); `null`
/// clears one. All or nothing.
#[tauri::command]
async fn meta_set(
    changes: BTreeMap<String, Option<Value>>,
    state: State<'_, AppState>,
) -> Result<LayoutState, String> {
    let changes = validate_changes(changes, layout::validate_meta)?;
    state.change(|doc| {
        doc.map_update(MapName::Meta, &changes)?;
        Ok(layout_state(doc))
    })
}

/// Replaces the expert Typst code.
#[tauri::command]
async fn layout_custom_set(
    text: String,
    state: State<'_, AppState>,
) -> Result<LayoutState, String> {
    if text.len() > MAX_CUSTOM_LEN {
        return Err(format!(
            "The custom Typst code is longer than {} KB.",
            MAX_CUSTOM_LEN / 1024
        ));
    }
    state.change(|doc| {
        doc.set_custom_text(&text)?;
        Ok(layout_state(doc))
    })
}

// ---- Layout overrides ----

/// Longest anchor preview, in characters.
const PREVIEW_CHARS: usize = 60;

/// An override as the UI lists it.
#[derive(Serialize)]
struct OverrideInfo {
    id: String,
    /// Its kind (`page_break`, `column_break`, `space_before`,
    /// `figure_placement`, `image_quality`, `table_style`); `None` if the
    /// stored entries are invalid.
    kind: Option<&'static str>,
    params: Map<String, Value>,
    /// The text at its position (up to the line end, at most 60 characters).
    anchor_preview: String,
    pos_utf16: Option<usize>,
    /// The anchored text was deleted; it applies where that text was.
    detached: bool,
    /// Whether the last compilation could apply it (`None`: not compiled
    /// since it was added).
    applicable: Option<bool>,
    /// Why it is invalid (not applied); the UI can only remove it.
    error: Option<String>,
}

fn override_infos(
    doc: &Document,
    applied: &std::collections::HashMap<String, bool>,
) -> Vec<OverrideInfo> {
    let (valid, invalid) = overrides::parse(&doc.map_entries(MapName::Overrides));
    let text = doc.text();
    let mut infos: Vec<OverrideInfo> = valid
        .into_iter()
        .map(|o| {
            let pos = doc.resolve_anchor(&o.anchor);
            let preview = pos.as_ref().map_or(String::new(), |p| {
                text[p.pos_byte.min(text.len())..]
                    .chars()
                    .take_while(|c| *c != '\n')
                    .take(PREVIEW_CHARS)
                    .collect()
            });
            OverrideInfo {
                kind: Some(o.kind.name()),
                params: o.kind.params(),
                anchor_preview: preview,
                pos_utf16: pos.as_ref().ok().map(|p| p.pos_utf16),
                detached: pos.as_ref().is_ok_and(|p| p.detached),
                applicable: applied.get(&o.id).copied(),
                error: pos.err().map(|_| "Its anchor is not valid.".to_owned()),
                id: o.id,
            }
        })
        .collect();
    infos.extend(invalid.into_iter().map(|i| OverrideInfo {
        id: i.id,
        kind: None,
        params: Map::new(),
        anchor_preview: String::new(),
        pos_utf16: None,
        detached: false,
        applicable: None,
        error: Some(i.message),
    }));
    infos.sort_by_key(|i| (i.pos_utf16.is_none(), i.pos_utf16, i.id.clone()));
    infos
}

/// Byte offset of UTF-16 offset `pos` in `text`.
fn utf16_to_byte(text: &str, pos: usize) -> Result<usize, String> {
    let mut units = 0;
    for (i, c) in text.char_indices() {
        if units == pos {
            return Ok(i);
        }
        units += c.len_utf16();
        if units > pos {
            return Err(format!("Position {pos} splits a character."));
        }
    }
    if units == pos {
        Ok(text.len())
    } else {
        Err(format!("Position {pos} is beyond the end of the text."))
    }
}

fn byte_to_utf16(text: &str, byte: usize) -> usize {
    text[..byte].encode_utf16().count()
}

/// Every key of `entries` belonging to override `id`, set to delete.
fn removal(entries: &Entries, id: &str) -> BTreeMap<String, Option<Value>> {
    overrides::keys_of(entries, id)
        .into_iter()
        .map(|k| (k.clone(), None))
        .collect()
}

/// Applies override `changes` as one undo step of its own.
fn commit_overrides(
    doc: &mut Document,
    changes: &BTreeMap<String, Option<Value>>,
) -> Result<(), String> {
    doc.end_undo_step();
    doc.map_update(MapName::Overrides, changes)
        .map_err(message)?;
    doc.end_undo_step();
    Ok(())
}

/// The overrides, sorted by position (invalid ones last).
#[tauri::command]
async fn overrides_get(
    state: State<'_, AppState>,
    compile: State<'_, CompileState>,
) -> Result<Vec<OverrideInfo>, String> {
    let applied = compile.applied();
    state.with(|doc| Ok(override_infos(doc, &applied)))
}

/// Adds an override of `kind` with `params` at UTF-16 offset `pos_utf16`
/// and returns its id (one undo step). Block kinds anchor at the start of
/// the paragraph or block holding `pos_utf16`, which must be at block level
/// in markup; a figure placement anchors at the figure call containing (or
/// starting on the line of) `pos_utf16`, an image quality at the image call
/// containing it. An override of the same kind at the same block start,
/// figure or image is replaced (its id is returned).
#[tauri::command]
async fn override_add(
    kind: String,
    params: Map<String, Value>,
    pos_utf16: usize,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let kind = overrides::validate_new(&kind, &params)?;
    state.try_change(|doc| add_override(doc, kind, pos_utf16))
}

/// See [`override_add`].
fn add_override(doc: &mut Document, kind: Kind, pos_utf16: usize) -> Result<String, String> {
    let text = doc.text();
    let byte = utf16_to_byte(&text, pos_utf16)?;
    let entries = doc.map_entries(MapName::Overrides);
    let (stored, _) = overrides::parse(&entries);
    let scan = overrides::Scan::new(&text);
    let same_kind = stored.iter().filter(|o| o.kind.name() == kind.name());
    let (at, existing) = if kind.is_block() {
        let line = scan.block_start(byte);
        if !scan.is_block_level(line) {
            return Err(
                "This can't go here: the line is inside raw text, a comment, \
                 math, code or a function's argument."
                    .into(),
            );
        }
        let existing = same_kind
            .filter(|o| {
                doc.resolve_anchor(&o.anchor)
                    .is_ok_and(|p| scan.block_start(p.pos_byte) == line)
            })
            .map(|o| o.id.clone())
            .next();
        (line, existing)
    } else {
        let call_at = |pos| match kind {
            Kind::ImageQuality { .. } => scan.image_at(pos),
            Kind::TableStyle { .. } => scan.table_at(pos),
            _ => scan.figure_at(pos),
        };
        let call = call_at(byte).ok_or(match kind {
            Kind::ImageQuality { .. } => "There is no image here.",
            Kind::TableStyle { .. } => "There is no table here.",
            _ => "There is no figure here.",
        })?;
        let existing = same_kind
            .filter(|o| {
                doc.resolve_anchor(&o.anchor)
                    .is_ok_and(|p| call_at(p.pos_byte) == Some(call))
            })
            .map(|o| o.id.clone())
            .next();
        (call, existing)
    };
    let anchor = doc.anchor_at(byte_to_utf16(&text, at)).map_err(message)?;
    let id = existing.unwrap_or_else(|| {
        overrides::new_id(|id| entries.keys().any(|k| k.split('.').next() == Some(id)))
    });
    let mut changes = removal(&entries, &id);
    let new = Override {
        id: id.clone(),
        kind,
        anchor,
    };
    for (key, value) in new.entries() {
        changes.insert(key, Some(value));
    }
    commit_overrides(doc, &changes)?;
    Ok(id)
}

/// Removes override `id` (one undo step).
#[tauri::command]
async fn override_remove(id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.try_change(|doc| {
        let changes = remove_changes(&doc.map_entries(MapName::Overrides), &id)?;
        commit_overrides(doc, &changes)
    })
}

/// The changes that remove override `id`.
fn remove_changes(entries: &Entries, id: &str) -> Result<BTreeMap<String, Option<Value>>, String> {
    let changes = removal(entries, id);
    if changes.is_empty() {
        return Err(format!("There is no override \"{id}\"."));
    }
    Ok(changes)
}

/// Replaces the parameters of override `id` (one undo step); its kind and
/// anchor stay.
#[tauri::command]
async fn override_update(
    id: String,
    params: Map<String, Value>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.try_change(|doc| {
        let changes = update_changes(&doc.map_entries(MapName::Overrides), &id, &params)?;
        commit_overrides(doc, &changes)
    })
}

/// The changes that give override `id` new `params` (validated).
fn update_changes(
    entries: &Entries,
    id: &str,
    params: &Map<String, Value>,
) -> Result<BTreeMap<String, Option<Value>>, String> {
    let kind = entries
        .get(&overrides::key(id, overrides::KIND_FIELD))
        .and_then(Value::as_str)
        .ok_or_else(|| format!("There is no override \"{id}\"."))?;
    let kind = overrides::validate_new(kind, params)?;
    let mut changes: BTreeMap<String, Option<Value>> = removal(entries, id)
        .into_iter()
        .filter(|(k, _)| {
            k != &overrides::key(id, overrides::KIND_FIELD)
                && k != &overrides::key(id, overrides::ANCHOR_FIELD)
        })
        .collect();
    for (field, value) in kind.params() {
        changes.insert(overrides::key(id, &field), Some(value));
    }
    Ok(changes)
}

/// One operation of [`overrides_batch`].
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum BatchOp {
    Update {
        id: String,
        params: Map<String, Value>,
    },
    Remove {
        id: String,
    },
}

/// Updates and removes several overrides as one undo step. All or nothing:
/// the first invalid operation (or a second one on the same override) fails
/// the batch, naming that override.
#[tauri::command]
async fn overrides_batch(ops: Vec<BatchOp>, state: State<'_, AppState>) -> Result<(), String> {
    if ops.is_empty() {
        return Ok(());
    }
    state.try_change(|doc| apply_batch(doc, &ops))
}

/// See [`overrides_batch`].
fn apply_batch(doc: &mut Document, ops: &[BatchOp]) -> Result<(), String> {
    let entries = doc.map_entries(MapName::Overrides);
    let mut changes = BTreeMap::new();
    let mut seen = std::collections::HashSet::new();
    for op in ops {
        let (id, op_changes) = match op {
            BatchOp::Update { id, params } => (id, update_changes(&entries, id, params)),
            BatchOp::Remove { id } => (id, remove_changes(&entries, id)),
        };
        let fail = |e: String| format!("{e} (exception \"{id}\"; nothing was changed)");
        if !seen.insert(id.as_str()) {
            return Err(fail("It is named twice.".to_owned()));
        }
        changes.extend(op_changes.map_err(fail)?);
    }
    commit_overrides(doc, &changes)
}

// ---- Publisher view: compilation ----

/// While `active`, the document is compiled after changes (debounced);
/// results arrive as `compile-status` events.
#[tauri::command]
async fn compile_watch(active: bool, compile: State<'_, CompileState>) -> Result<(), String> {
    compile.watch(active);
    Ok(())
}

/// The document version now: `compile-status` versions equal to it describe
/// the current text (diagnostic offsets are up to date).
#[tauri::command]
async fn doc_version(state: State<'_, AppState>) -> Result<u64, String> {
    Ok(state.version())
}

/// Compiles the latest version now (or repeats the last status if it is
/// compiled already). `force` (F5) first adopts loose image files (see
/// [`adopt_assets`]) and counts a new version, so the document is compiled
/// again and its pages fetched anew: the compiler reads the images from disk
/// on every compile. A failed adoption is reported after compiling.
#[tauri::command]
async fn compile_now(
    force: bool,
    state: State<'_, AppState>,
    compile: State<'_, CompileState>,
) -> Result<(), String> {
    let mut adopted = Ok(());
    if force {
        let mut guard = state.lock();
        if let Some(project) = guard.as_mut() {
            adopted = adopt_assets(&mut project.doc).map(drop).map_err(message);
        }
        state.version.fetch_add(1, Ordering::SeqCst);
    }
    compile.now();
    adopted
}

/// Page `index` (0-based) of compiled version `version` as SVG bytes. Fails
/// if that version's pages are no longer held: fetch the latest instead.
#[tauri::command]
async fn page_svg(
    version: u64,
    index: usize,
    compile: State<'_, CompileState>,
) -> Result<Response, String> {
    Ok(Response::new(compile.page_svg(version, index).await?))
}

/// Where a click at (`x_pt`, `y_pt`) (points from the top left) on page
/// `page` (0-based) of compiled version `version` leads. Fails like
/// `page_svg` if that version's pages are no longer held.
#[tauri::command]
async fn page_jump(
    version: u64,
    page: usize,
    x_pt: f64,
    y_pt: f64,
    compile: State<'_, CompileState>,
) -> Result<JumpResult, String> {
    compile.jump(version, page, x_pt, y_pt).await
}

/// Exports the latest version (compiled first if needed) as PDF to `path`.
#[tauri::command]
async fn export_pdf(path: PathBuf, compile: State<'_, CompileState>) -> Result<(), String> {
    let pdf = compile.pdf().await?;
    write_atomic(&path, &pdf).map_err(|e| format!("Could not write {}: {e}", path.display()))
}

/// The save dialog's filters for the formats Lethora writes (PDF aside),
/// each with its format id.
#[tauri::command]
async fn export_formats() -> Result<Vec<convert::registry::Filter>, String> {
    Ok(convert::registry::export_filters())
}

/// Exports the latest version (compiled first if needed) in the registry
/// format `format` to `path`, its side files (Markdown's images) next to
/// it; answers what was dropped or approximated.
#[tauri::command]
async fn export_file(
    path: PathBuf,
    format: String,
    compile: State<'_, CompileState>,
) -> Result<Vec<convert::Warning>, String> {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let written = compile.export(format, stem.into_owned()).await?;
    let folder = path.parent().unwrap_or(Path::new("."));
    let fail = |p: &Path, e: io::Error| format!("Could not write {}: {e}", p.display());
    for (rel, bytes) in &written.files {
        let target = side_file(folder, rel)?;
        if let Some(dir) = target.parent() {
            fs::create_dir_all(dir).map_err(|e| fail(dir, e))?;
        }
        write_atomic(&target, bytes).map_err(|e| fail(&target, e))?;
    }
    write_atomic(&path, &written.bytes).map_err(|e| fail(&path, e))?;
    Ok(written.warnings)
}

/// Where a writer's side file `rel` (`/`-separated, relative) goes in
/// `folder`; refused if it would leave it.
fn side_file(folder: &Path, rel: &str) -> Result<PathBuf, String> {
    let mut path = folder.to_path_buf();
    for part in rel.split('/') {
        if matches!(part, "" | "." | "..") || part.contains(['\\', ':']) {
            return Err(format!(
                "Refused to write {rel}: not a plain relative path."
            ));
        }
        path.push(part);
    }
    Ok(path)
}

// ---- Folder sync ----

/// What a pull changed: like [`Reply`] (text changes relative to the text
/// before, UTF-16 length after), plus what else to reload (flags sent only
/// when true) and the sync status.
#[derive(Serialize)]
struct PullReply {
    changes: Vec<TextChange>,
    len16: usize,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    changed: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    settings: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    bibliography: bool,
    /// The asset index changed or blobs arrived.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    assets: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    checkpoints: bool,
    /// Review items arrived (or changed): list them again.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    review: bool,
    status: foldersync::Status,
}

/// The new project code and the sync status (`sync_setup`).
#[derive(Serialize)]
struct SetupReply {
    code: String,
    status: foldersync::Status,
}

/// Runs `f` on the open project's document, sync link and folder.
fn with_link<T>(
    state: &AppState,
    f: impl FnOnce(&mut Document, &mut Link, &Path) -> Result<T, String>,
) -> Result<T, String> {
    let mut guard = state.lock();
    let project = guard.as_mut().ok_or(NO_PROJECT)?;
    f(&mut project.doc, &mut project.link, &project.path)
}

#[tauri::command]
async fn sync_status(state: State<'_, AppState>) -> Result<foldersync::Status, String> {
    let guard = state.lock();
    Ok(guard.as_ref().ok_or(NO_PROJECT)?.status())
}

/// What Help → About Lethora shows (see `app_info`).
#[derive(Serialize)]
struct AppInfo {
    /// SemVer, from the workspace `Cargo.toml`.
    version: &'static str,
    /// The build's short git hash (see `build.rs`).
    git: &'static str,
    protocol: u8,
    level: u32,
    os: &'static str,
    arch: &'static str,
    /// The open project's server's version, once it said so.
    #[serde(skip_serializing_if = "Option::is_none")]
    server: Option<String>,
}

/// The app's version and build, the wire protocol and feature level it
/// speaks, the OS, and the version of the server the open project syncs
/// with: for people and bug reports, never for compatibility.
#[tauri::command]
async fn app_info(state: State<'_, AppState>) -> Result<AppInfo, String> {
    let server = state.lock().as_ref().and_then(|p| {
        let server = p.server.as_ref()?;
        serversync::locked(&server.sync)
            .server_version()
            .map(str::to_owned)
    });
    Ok(AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        git: env!("LETHORA_GIT_HASH"),
        protocol: lethora_core::protocol::wire::VERSION,
        level: lethora_core::protocol::LEVEL,
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        server,
    })
}

/// Starts encrypted sync in a new `<project>.lethora-sync` folder inside
/// `folder`; returns the project code.
#[tauri::command]
async fn sync_setup(
    folder: PathBuf,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<SetupReply, String> {
    let config = config_dir(&app);
    with_link(&state, |doc, link, path| {
        let code = link.setup(doc, path, &folder, config.as_deref())?;
        Ok(SetupReply {
            code,
            status: link.status(doc),
        })
    })
}

/// Starts encrypted sync in a new `<project>.lethora-sync` folder inside
/// `Lethora/` (created if missing) in the folder of sync app `id` (from
/// `sync_apps`); returns the project code.
#[tauri::command]
async fn sync_setup_app(
    id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<SetupReply, String> {
    let root = sync_apps_here(&app)
        .into_iter()
        .find(|a| a.id == id)
        .ok_or("That sync app's folder was not found on this device any more.")?
        .path;
    let folder = root.join(foldersync::LETHORA_FOLDER);
    fs::create_dir_all(&folder)
        .map_err(|e| format!("Could not create {}: {e}", folder.display()))?;
    sync_setup(folder, app, state).await
}

fn sync_apps_here(app: &AppHandle) -> Vec<foldersync::SyncApp> {
    let probe = foldersync::System {
        home: app.path().home_dir().ok(),
    };
    foldersync::sync_apps(foldersync::Platform::current(), &probe)
}

/// The sync apps' folders on this device (Dropbox, OneDrive, iCloud Drive,
/// Google Drive).
#[tauri::command]
async fn sync_apps(app: AppHandle) -> Result<Vec<foldersync::SyncApp>, String> {
    Ok(sync_apps_here(&app))
}

/// Finds the synced project `code` unlocks: in `folder` (a replica, or a
/// folder holding replicas) if given, else in the sync apps' `Lethora/`
/// folders and their top folders. Also names the project on this device
/// that syncs with it already.
#[tauri::command]
async fn sync_find(
    code: String,
    folder: Option<PathBuf>,
    app: AppHandle,
    state: State<'_, AppState>,
    identity: State<'_, Identity>,
) -> Result<foldersync::Found, String> {
    let (dirs, searched) = match folder {
        Some(folder) => (vec![folder], Vec::new()),
        None => {
            let apps = sync_apps_here(&app);
            let dirs = apps
                .iter()
                .flat_map(|a| [a.path.join(foldersync::LETHORA_FOLDER), a.path.clone()])
                .collect();
            (dirs, apps.into_iter().map(|a| a.label).collect())
        }
    };
    let known = known_projects(&app, &state);
    foldersync::find(&dirs, &code, &identity.device, &known, searched)
}

/// Joins the synced project in folder `replica` with `code` as a new local
/// project at `path` (`.lethora` added), by default in the default projects
/// folder named after the replica folder, and opens it like `project_new`.
/// Refused if a project on this device syncs with it already.
#[tauri::command]
async fn sync_join(
    replica: PathBuf,
    path: Option<PathBuf>,
    code: String,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    identity: State<'_, Identity>,
) -> Result<Opened, String> {
    let path = match path {
        Some(path) => with_extension(path),
        None => foldersync::project_folder(
            &projects_dir(&app)?,
            &foldersync::joined_name(&replica),
            |p| p.exists(),
        ),
    };
    let config = config_dir(&app);
    let known = known_projects(&app, &state);
    let (doc, replica, code) =
        foldersync::join(&replica, &path, &code, &identity, config.as_deref(), &known)?;
    let link = Link::joined(&path, &identity, config.as_deref(), replica, code);
    Ok(install(&app, &window, &state, path, doc, Some(link), None))
}

/// The project code, to show it again.
#[tauri::command]
async fn sync_code(state: State<'_, AppState>) -> Result<String, String> {
    with_link(&state, |doc, link, _| link.code(doc))
}

/// Connects with a typed project code (none stored, or a wrong one).
#[tauri::command]
async fn sync_enter_code(
    code: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<foldersync::Status, String> {
    let config = config_dir(&app);
    with_link(&state, |doc, link, _| {
        link.enter_code(doc, &code, config.as_deref())?;
        Ok(link.status(doc))
    })
}

/// Stops encrypted sync on this device (the copy stays in the sync folder).
#[tauri::command]
async fn sync_stop(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<foldersync::Status, String> {
    let config = config_dir(&app);
    with_link(&state, |doc, link, _| {
        link.stop(doc, config.as_deref())?;
        Ok(link.status(doc))
    })
}

/// Imports what other devices wrote (see [`Link::pull`]); a change, or
/// blobs arriving, counts a new version.
#[tauri::command]
async fn sync_pull(state: State<'_, AppState>) -> Result<PullReply, String> {
    let mut guard = state.lock();
    let project = guard.as_mut().ok_or(NO_PROJECT)?;
    let pulled = match &project.server {
        Some(server) => project
            .doc
            .server_pull(&mut serversync::locked(&server.sync))
            .map_err(message)?,
        // A server-shared project without its key pulls nothing.
        None if project.doc.server_settings().is_some() => Default::default(),
        None => project.link.pull(&mut project.doc)?,
    };
    if let Some(server) = &project.server {
        // Blobs to download, asked for by the pull.
        server.act();
    }
    for skipped in &pulled.skipped {
        eprintln!("Sync: skipped {skipped}");
    }
    if pulled.changed || pulled.blobs > 0 {
        state.version.fetch_add(1, Ordering::SeqCst);
    }
    Ok(PullReply {
        changes: pulled.changes,
        len16: project.doc.len_utf16(),
        changed: pulled.changed,
        settings: pulled.settings,
        bibliography: pulled.bibliography,
        assets: pulled.assets || pulled.blobs > 0,
        checkpoints: pulled.checkpoints,
        review: pulled.review,
        status: project.status(),
    })
}

/// Never shows the plain-sync notice for this project again.
#[tauri::command]
async fn sync_notice_dismiss(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let config = config_dir(&app);
    with_link(&state, |_, link, _| link.dismiss_notice(config.as_deref()))
}

// ---- Server sync ----

/// How long sharing may take until the project is on the server.
const SHARE_WAIT: Duration = Duration::from_secs(120);
/// How long creating an invite may take.
const INVITE_WAIT: Duration = Duration::from_secs(30);
/// How long a join may take until the project is caught up.
const JOIN_WAIT: Duration = Duration::from_secs(300);
/// Longest display name kept, in characters.
const MAX_DISPLAY_NAME: usize = 100;
const NO_KEY_FOLDER: &str =
    "Lethora can't store the project key: the app's settings folder is unknown.";
const NO_ANSWER: &str = "No answer from the server in time. Check the address and try again.";

/// The servers used last and the user's display name, to prefill the share
/// and join forms.
#[derive(Serialize)]
struct ServerRecent {
    servers: Vec<String>,
    name: Option<String>,
}

#[tauri::command]
async fn server_recent(app: AppHandle) -> Result<ServerRecent, String> {
    let config = config_dir(&app);
    Ok(ServerRecent {
        servers: config
            .as_deref()
            .map(|c| serversync::Servers::read(c).servers)
            .unwrap_or_default(),
        name: profile_name(config.as_deref()),
    })
}

/// A display name as typed: trimmed, not empty, at most
/// [`MAX_DISPLAY_NAME`] characters.
fn display_name(name: &str) -> Result<String, String> {
    let name: String = name.trim().chars().take(MAX_DISPLAY_NAME).collect();
    if name.is_empty() {
        return Err("Enter your name, as others will see it.".into());
    }
    Ok(name)
}

/// Shares the open project on the Lethora server at `url` as its owner
/// (`name`: the user's display name) and waits until it is there. Refused
/// while folder sync is on. Returns the sync status.
#[tauri::command]
async fn server_share(
    url: String,
    name: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<foldersync::Status, String> {
    let name = display_name(&name)?;
    let config = config_dir(&app).ok_or(NO_KEY_FOLDER)?;
    let keys = app.state::<Keys>().0.clone();
    let (sync, id, project) = {
        let mut guard = state.lock();
        let project = guard.as_mut().ok_or(NO_PROJECT)?;
        if project.doc.sync_settings().is_some() {
            return Err(
                "This project syncs through a folder. Stop folder sync first to share it on a server."
                    .into(),
            );
        }
        if project.doc.server_settings().is_some() {
            return Err("This project is shared on a server already.".into());
        }
        let (sync, key) = project
            .doc
            .server_share(&url, &name, keys, foldersync::now_ms() / 1000)
            .map_err(message)?;
        let id = sync.project().to_owned();
        serversync::store_key(&config, &id, &Keyring::new(key))?;
        let link = ServerLink::start(&app, Arc::new(Mutex::new(sync)), true);
        let sync = link.sync.clone();
        let link_id = link.id;
        project.server = Some(link);
        (sync, link_id, id)
    };
    let outcome = serversync::wait_for(&sync, SHARE_WAIT, |s| {
        if s.is_established() {
            Some(Ok(()))
        } else {
            s.is_stopped()
                .then(|| Err(s.status().message.unwrap_or_default()))
        }
    })
    .await
    .unwrap_or_else(|| Err(NO_ANSWER.into()));
    let mut guard = state.lock();
    let open = guard
        .as_mut()
        .filter(|p| p.server.as_ref().is_some_and(|s| s.id == id));
    match (outcome, open) {
        (Ok(()), Some(open)) => {
            open.save().map_err(message)?;
            let (url, user) = {
                let sync = serversync::locked(&sync);
                (sync.url().to_owned(), sync.user().to_owned())
            };
            serversync::remember_server(Some(&config), &url);
            let title = project_title(&open.doc, &open.path);
            serversync::update_remembered(&config, |r| {
                r.add(serversync::RememberedProject {
                    server: url,
                    project: project.clone(),
                    title,
                    user,
                })
            });
            set_profile_name(Some(&config), &name);
            Ok(open.status())
        }
        (outcome, open) => {
            if let Some(open) = open {
                open.server = None;
            }
            serversync::forget_key(&config, &project);
            Err(match outcome {
                Err(e) => format!("Sharing failed: {e}"),
                Ok(()) => "The project was closed while sharing.".into(),
            })
        }
    }
}

/// Creates an invite link for the open shared project: for a new member
/// with `role` (`editor`, `reviewer` or `reader`; owners only), or without
/// one for another device of this user. It expires after `days` and works
/// `uses` times.
#[tauri::command]
async fn server_invite(
    role: Option<String>,
    days: u64,
    uses: u32,
    state: State<'_, AppState>,
) -> Result<String, String> {
    use lethora_core::protocol::members::Role;
    let role = match role.as_deref() {
        None => None,
        Some("editor") => Some(Role::Editor),
        Some("reviewer") => Some(Role::Reviewer),
        Some("reader") => Some(Role::Reader),
        Some(other) => return Err(format!("Unknown role \"{other}\".")),
    };
    if !(1..=30).contains(&days) || !(1..=100).contains(&uses) {
        return Err("An invite lasts 1 to 30 days and works 1 to 100 times.".into());
    }
    let (sync, id) = {
        let guard = state.lock();
        let project = guard.as_ref().ok_or(NO_PROJECT)?;
        let server = project
            .server
            .as_ref()
            .ok_or("This project is not shared on a server.")?;
        let id = serversync::locked(&server.sync)
            .invite(role, days, uses)
            .map_err(message)?;
        server.act();
        (server.sync.clone(), id)
    };
    match serversync::wait_for(&sync, INVITE_WAIT, |s| s.invite_result(&id)).await {
        Some(Ok(link)) => Ok(link),
        Some(Err(e)) => Err(format!("Creating the invite failed: {e}")),
        None => Err(NO_ANSWER.into()),
    }
}

/// Joins the project of invite link `link` (`name`: the user's display
/// name) as a new local project at `path` (`.lethora` added), by default in
/// the default projects folder named after the project's title, and opens
/// it like `project_new`. Refused if the project is on this device already.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn server_join(
    link: String,
    name: String,
    path: Option<PathBuf>,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    identity: State<'_, Identity>,
) -> Result<Opened, String> {
    use lethora_core::protocol::invite::InviteLink;
    let name = display_name(&name)?;
    let config = config_dir(&app).ok_or(NO_KEY_FOLDER)?;
    let parsed =
        InviteLink::parse(&link).map_err(|e| format!("This is not a Lethora invite link: {e}"))?;
    let known = known_projects(&app, &state);
    if let Some(local) = known.iter().find(|p| {
        lethora_core::stored_server_settings(&FsStorage::new(p.as_path()), &identity.device)
            .is_some_and(|s| s.project == parsed.project)
    }) {
        return Err(format!(
            "This project is already on this device, in {}. Open it instead.",
            local.display()
        ));
    }
    let keys = app.state::<Keys>().0.clone();
    let sync =
        ServerSync::join(&link, &identity.device, keys, &identity.user, &name).map_err(message)?;
    let sync = Arc::new(Mutex::new(sync));
    match catch_up(&app, &sync).await {
        Some(Ok(())) => {}
        Some(Err(e)) => return Err(format!("Joining failed: {e}")),
        None => return Err(NO_ANSWER.into()),
    }
    set_profile_name(Some(&config), &name);
    open_joined(&app, &window, &state, &identity, &config, sync, path, None)
}

/// Drives a join (or reopen) until the project can be built: `Ok`, or why
/// it stopped; `None` without an answer in time.
async fn catch_up(app: &AppHandle, sync: &Arc<Mutex<ServerSync>>) -> Option<Result<(), String>> {
    let joining = ServerLink::start(app, sync.clone(), false);
    let ready = serversync::wait_for(sync, JOIN_WAIT, |s| {
        if s.join_ready() {
            Some(Ok(()))
        } else {
            s.is_stopped()
                .then(|| Err(s.status().message.unwrap_or_default()))
        }
    })
    .await;
    joining.stop().await;
    ready
}

/// Builds the local project of a join (or reopen) that is ready at `path`,
/// by default in the default projects folder named after its title (else
/// `title`), keeps its key and opens it (which remembers it, see
/// [`install`]).
#[allow(clippy::too_many_arguments)]
fn open_joined(
    app: &AppHandle,
    window: &WebviewWindow,
    state: &AppState,
    identity: &Identity,
    config: &Path,
    sync: Arc<Mutex<ServerSync>>,
    path: Option<PathBuf>,
    title: Option<&str>,
) -> Result<Opened, String> {
    let title = serversync::locked(&sync)
        .queued_meta()
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|t| !t.trim().is_empty())
        .or_else(|| title.map(str::to_owned))
        .unwrap_or_else(|| "Shared project".into());
    let path = match path {
        Some(path) => with_extension(path),
        None => foldersync::project_folder(&projects_dir(app)?, &title, |p| p.exists()),
    };
    let doc = {
        let mut sync = serversync::locked(&sync);
        let doc = Document::server_join(Box::new(FsStorage::new(&path)), &mut sync, identity)
            .map_err(message)?;
        let keyring = sync.keyring().ok_or("The project key is missing.")?;
        serversync::store_key(config, sync.project(), keyring)?;
        serversync::remember_server(Some(&config.to_path_buf()), sync.url());
        doc
    };
    let link = ServerLink::start(app, sync, true);
    Ok(install(app, window, state, path, doc, None, Some(link)))
}

/// The title of `doc` (its metadata), else the name of its folder `path`.
fn project_title(doc: &Document, path: &Path) -> String {
    doc.map_entries(MapName::Meta)
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map_or_else(|| project_name(path), str::to_owned)
}

/// How long a members change (role, removal, revoke, deletion) may take.
const CHANGE_WAIT: Duration = Duration::from_secs(30);

/// Makes the open project local again after leaving, being removed or the
/// project's deletion, if `link` is its server link: the link stops, the
/// server settings go (see `Document::server_forget`), and the key and the
/// remembered entry are forgotten. Returns whether it did (once per link).
fn forget_server(app: &AppHandle, link: u64) -> bool {
    let state = app.state::<AppState>();
    let mut guard = state.lock();
    let Some(project) = guard
        .as_mut()
        .filter(|p| p.server.as_ref().is_some_and(|s| s.id == link))
    else {
        return false;
    };
    project.server = None;
    let id = project.doc.server_settings().map(|s| s.project.clone());
    if let Err(e) = project.doc.server_forget() {
        eprintln!("Server sync: forgetting the server failed: {e}");
    }
    if let (Some(config), Some(id)) = (config_dir(app), id) {
        serversync::forget_key(&config, &id);
        serversync::update_remembered(&config, |r| r.remove(&id));
    }
    true
}

/// The open project's server link, its id and its sync.
fn server_of(state: &AppState) -> Result<(u64, Arc<Mutex<ServerSync>>), String> {
    let guard = state.lock();
    let project = guard.as_ref().ok_or(NO_PROJECT)?;
    let server = project
        .server
        .as_ref()
        .ok_or("This project is not shared on a server.")?;
    Ok((server.id, server.sync.clone()))
}

/// Starts change `start` on the open project's sync and waits for its
/// result; `removed` also counts as done once this device is removed (its
/// own removal or the deletion stop the sync first).
async fn server_change(
    state: &AppState,
    removed: bool,
    start: impl FnOnce(&mut ServerSync) -> lethora_core::Result<u64>,
) -> Result<u64, String> {
    let (link, sync, id) = {
        let guard = state.lock();
        let project = guard.as_ref().ok_or(NO_PROJECT)?;
        let server = project
            .server
            .as_ref()
            .ok_or("This project is not shared on a server.")?;
        let id = start(&mut serversync::locked(&server.sync)).map_err(message)?;
        server.act();
        (server.id, server.sync.clone(), id)
    };
    let result = serversync::wait_for(&sync, CHANGE_WAIT, |s| {
        s.change_result(id).or_else(|| {
            (removed && s.status().state == lethora_core::ServerState::Removed).then_some(Ok(()))
        })
    })
    .await;
    match result {
        Some(Ok(())) => Ok(link),
        Some(Err(e)) => Err(format!("The server refused: {e}")),
        None => Err(NO_ANSWER.into()),
    }
}

/// The members screen: who is in the project (with this device's user and
/// device marked), the open invites, the project's title (to confirm a
/// deletion with) and the colours a member may pick.
#[derive(Serialize)]
struct MembersReply {
    user: String,
    device: String,
    title: String,
    members: Vec<lethora_core::MemberInfo>,
    invites: Vec<lethora_core::InviteInfo>,
    palette: [&'static str; 8],
}

#[tauri::command]
async fn server_members(
    state: State<'_, AppState>,
    identity: State<'_, Identity>,
) -> Result<MembersReply, String> {
    let guard = state.lock();
    let project = guard.as_ref().ok_or(NO_PROJECT)?;
    let server = project
        .server
        .as_ref()
        .ok_or("This project is not shared on a server.")?;
    let sync = serversync::locked(&server.sync);
    Ok(MembersReply {
        user: sync.user().to_owned(),
        device: identity.device.as_str().to_owned(),
        title: project_title(&project.doc, &project.path),
        members: sync.member_list(),
        invites: sync.open_invites(),
        palette: lethora_core::PALETTE,
    })
}

/// Publishes where this device's user is in a server-shared project: view
/// `view`, selection `anchor`..`head` (UTF-16 offsets into the core's text).
/// Does nothing in other projects.
#[tauri::command]
async fn presence_set(
    view: String,
    anchor: usize,
    head: usize,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let guard = state.lock();
    let Some(project) = guard.as_ref() else {
        return Ok(());
    };
    let Some(server) = &project.server else {
        return Ok(());
    };
    project
        .doc
        .server_set_presence(
            &mut serversync::locked(&server.sync),
            &view,
            anchor,
            head,
            foldersync::now_ms(),
        )
        .map_err(message)?;
    server.act();
    Ok(())
}

/// The other devices' presence in a server-shared project (none otherwise).
#[tauri::command]
async fn presence_get(
    state: State<'_, AppState>,
) -> Result<Vec<lethora_core::PresenceInfo>, String> {
    let guard = state.lock();
    Ok(guard
        .as_ref()
        .and_then(|p| {
            Some(
                p.doc
                    .server_presence(&serversync::locked(&p.server.as_ref()?.sync)),
            )
        })
        .unwrap_or_default())
}

/// Who wrote the text from `from` to `to` (UTF-16 offsets into the core's
/// text) in a server-shared project: runs and their authors (none otherwise).
#[tauri::command]
async fn authors_get(
    from: usize,
    to: usize,
    state: State<'_, AppState>,
) -> Result<lethora_core::Authors, String> {
    let guard = state.lock();
    Ok(guard
        .as_ref()
        .and_then(|p| {
            let sync = serversync::locked(&p.server.as_ref()?.sync);
            Some(p.doc.server_authors(&sync, from, to))
        })
        .unwrap_or_default())
}

// ---- Review: comments and highlights (crates/core/src/review.rs) ----

/// `review_list`: this device's user (whose items and messages are its own),
/// the review items (UTF-16 offsets into the core's text, in text order) and
/// whether review undo and redo can act.
#[derive(Serialize)]
struct ReviewList {
    me: UserId,
    items: Vec<ReviewItem>,
    undo: bool,
    redo: bool,
}

fn review_list_of(doc: &Document) -> ReviewList {
    ReviewList {
        me: doc.identity().user.clone(),
        items: doc.review_items(),
        undo: doc.can_review_undo(),
        redo: doc.can_review_redo(),
    }
}

#[tauri::command]
async fn review_list(state: State<'_, AppState>) -> Result<ReviewList, String> {
    state.with(|doc| Ok(review_list_of(doc)))
}

/// A comment on `from`..`to` (core offsets: the web view sends its pending
/// edits first) with first message `text`; returns its id.
#[tauri::command]
async fn review_comment(
    from: usize,
    to: usize,
    text: String,
    state: State<'_, AppState>,
) -> Result<ReviewId, String> {
    state.review_change(|doc| doc.add_comment(from, to, &text))
}

/// A highlight in `colour` (one of `MARK_COLOURS`) on `from`..`to`.
#[tauri::command]
async fn review_mark(
    from: usize,
    to: usize,
    colour: String,
    state: State<'_, AppState>,
) -> Result<ReviewId, String> {
    state.review_change(|doc| doc.add_mark(from, to, &colour))
}

/// Adds message `text` to item `item` (a highlight becomes a comment).
#[tauri::command]
async fn review_reply(
    item: u64,
    text: String,
    state: State<'_, AppState>,
) -> Result<ReviewId, String> {
    state.review_change(|doc| doc.reply(ReviewId(item), &text))
}

#[tauri::command]
async fn review_edit(
    item: u64,
    message: u64,
    text: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.review_change(|doc| doc.edit_message(ReviewId(item), ReviewId(message), &text))
}

#[tauri::command]
async fn review_delete(item: u64, message: u64, state: State<'_, AppState>) -> Result<(), String> {
    state.review_change(|doc| doc.delete_message(ReviewId(item), ReviewId(message)))
}

/// Resolves item `item` (`resolved`) or opens it again.
#[tauri::command]
async fn review_resolve(
    item: u64,
    resolved: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.review_change(|doc| {
        if resolved {
            doc.resolve(ReviewId(item))
        } else {
            doc.reopen(ReviewId(item))
        }
    })
}

#[tauri::command]
async fn review_colour(
    item: u64,
    colour: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.review_change(|doc| doc.set_mark_colour(ReviewId(item), &colour))
}

#[tauri::command]
async fn review_remove(item: u64, state: State<'_, AppState>) -> Result<(), String> {
    state.review_change(|doc| doc.remove_item(ReviewId(item)))
}

/// Suggest mode: suggests replacing `from`..`to` (core offsets) with
/// `insert`, merged into this user's open suggestion next to it; `main`
/// stays as it is. `split` (the caret moved) starts a new review undo step
/// first. Returns the suggestion made or extended, if any.
#[tauri::command]
async fn review_suggest(
    from: usize,
    to: usize,
    insert: String,
    split: bool,
    state: State<'_, AppState>,
) -> Result<Option<ReviewId>, String> {
    state.review_change(|doc| suggest(doc, from, to, &insert, split))
}

fn suggest(
    doc: &mut Document,
    from: usize,
    to: usize,
    insert: &str,
    split: bool,
) -> lethora_core::Result<Option<ReviewId>> {
    if split {
        doc.end_undo_step();
    }
    doc.suggest_change(from, to, insert)
}

/// Accepts suggestion `item`: a `main` edit (one undo step, a new version
/// to typeset), replied like undo so the web view applies it in step.
#[tauri::command]
async fn review_accept(item: u64, state: State<'_, AppState>) -> Result<Reply, String> {
    state.change(|doc| accept(doc, ReviewId(item)))
}

fn accept(doc: &mut Document, item: ReviewId) -> lethora_core::Result<Reply> {
    let changes = doc.accept_suggestion(item)?;
    Ok(reply(doc, changes))
}

#[tauri::command]
async fn review_reject(item: u64, state: State<'_, AppState>) -> Result<(), String> {
    state.review_change(|doc| doc.reject_suggestion(ReviewId(item)))
}

/// Undoes the last step of the review's own history; whether there was one.
#[tauri::command]
async fn review_undo(state: State<'_, AppState>) -> Result<bool, String> {
    state.review_change(Document::review_undo)
}

#[tauri::command]
async fn review_redo(state: State<'_, AppState>) -> Result<bool, String> {
    state.review_change(Document::review_redo)
}

/// Gives member `user` the role `role` (`editor`, `reviewer` or `reader`;
/// the owner only).
#[tauri::command]
async fn server_set_role(
    user: String,
    role: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    use lethora_core::protocol::members::Role;
    let role = match role.as_str() {
        "editor" => Role::Editor,
        "reviewer" => Role::Reviewer,
        "reader" => Role::Reader,
        other => return Err(format!("Unknown role \"{other}\".")),
    };
    server_change(&state, false, |s| s.set_role(&user, role)).await?;
    Ok(())
}

/// Gives this device's user `colour` (one of the profile palette) in the
/// open project.
#[tauri::command]
async fn server_set_colour(colour: String, state: State<'_, AppState>) -> Result<(), String> {
    server_change(&state, false, |s| s.set_colour(&colour)).await?;
    Ok(())
}

/// Removes member `user` with its devices (the owner only), or one device
/// `device` (the owner's, or one of this user's).
#[tauri::command]
async fn server_remove(
    user: Option<String>,
    device: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    match (user, device) {
        (Some(user), None) => server_change(&state, false, |s| s.remove_user(&user)).await?,
        (None, Some(device)) => server_change(&state, false, |s| s.remove_device(&device)).await?,
        _ => return Err("Remove either a member or a device.".into()),
    };
    Ok(())
}

/// Revokes invite `invite` (the owner, or the invite's creator).
#[tauri::command]
async fn server_revoke(invite: String, state: State<'_, AppState>) -> Result<(), String> {
    server_change(&state, false, |s| s.revoke(&invite)).await?;
    Ok(())
}

/// Leaves the project with this device; the local copy stays, as a local
/// project. Returns the new sync status.
#[tauri::command]
async fn server_leave(
    app: AppHandle,
    state: State<'_, AppState>,
    identity: State<'_, Identity>,
) -> Result<foldersync::Status, String> {
    let device = identity.device.as_str().to_owned();
    let link = server_change(&state, true, |s| s.remove_device(&device)).await?;
    forget_server(&app, link);
    let guard = state.lock();
    Ok(guard.as_ref().ok_or(NO_PROJECT)?.status())
}

/// Deletes the project from the server (the owner only); every local copy
/// stays, this one as a local project. Returns the new sync status.
#[tauri::command]
async fn server_delete(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<foldersync::Status, String> {
    let (_, sync) = server_of(&state)?;
    if serversync::locked(&sync).status().state != lethora_core::ServerState::Online {
        return Err("Deleting needs a connection to the server. Try again when online.".into());
    }
    let link = server_change(&state, true, |s| {
        s.tick(foldersync::now_ms() / 1000);
        s.delete_project()
    })
    .await?;
    forget_server(&app, link);
    let guard = state.lock();
    Ok(guard.as_ref().ok_or(NO_PROJECT)?.status())
}

/// A project shared or joined on this device whose local copy is gone, to
/// reopen from the server.
#[derive(Serialize)]
struct RememberedReply {
    project: String,
    server: String,
    title: String,
}

/// The remembered projects (see `serversync::Remembered`) without a local
/// copy on this device and with their key here.
#[tauri::command]
async fn server_remembered(
    app: AppHandle,
    state: State<'_, AppState>,
    identity: State<'_, Identity>,
) -> Result<Vec<RememberedReply>, String> {
    let Some(config) = config_dir(&app) else {
        return Ok(Vec::new());
    };
    let here: BTreeSet<String> = known_projects(&app, &state)
        .iter()
        .filter_map(|p| {
            lethora_core::stored_server_settings(&FsStorage::new(p.as_path()), &identity.device)
        })
        .map(|s| s.project)
        .collect();
    Ok(serversync::Remembered::read(&config)
        .projects
        .into_iter()
        .filter(|p| !here.contains(&p.project))
        .filter(|p| serversync::read_key(&config, &p.project).is_some())
        .map(|p| RememberedReply {
            project: p.project,
            server: p.server,
            title: p.title,
        })
        .collect())
}

/// Reopens remembered project `project` from its server as a new local
/// project at `path` (`.lethora` added), by default in the default projects
/// folder named after its title, like a join without an invite (this device
/// is still a member, its key is here). A device removed meanwhile (or a
/// project deleted) is told so and the entry is dropped.
#[tauri::command]
async fn server_reopen(
    project: String,
    path: Option<PathBuf>,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    identity: State<'_, Identity>,
) -> Result<Opened, String> {
    let config = config_dir(&app).ok_or(NO_KEY_FOLDER)?;
    let entry = serversync::Remembered::read(&config)
        .projects
        .into_iter()
        .find(|p| p.project == project)
        .ok_or("This project is not remembered on this device.")?;
    if let Some(local) = known_projects(&app, &state).iter().find(|p| {
        lethora_core::stored_server_settings(&FsStorage::new(p.as_path()), &identity.device)
            .is_some_and(|s| s.project == project)
    }) {
        return Err(format!(
            "This project is already on this device, in {}. Open it instead.",
            local.display()
        ));
    }
    let keyring = serversync::read_key(&config, &project)
        .ok_or("This device has no key for this project any more.")?;
    let user = UserId::parse(&entry.user).map_err(message)?;
    let keys = app.state::<Keys>().0.clone();
    let name = profile_name(Some(&config));
    let sync = ServerSync::reopen(
        &entry.server,
        &project,
        &identity.device,
        keys,
        &user,
        keyring,
        name.as_deref(),
    )
    .map_err(message)?;
    let sync = Arc::new(Mutex::new(sync));
    match catch_up(&app, &sync).await {
        Some(Ok(())) => {}
        Some(Err(e)) => {
            if serversync::locked(&sync).status().state == lethora_core::ServerState::Removed {
                serversync::forget_key(&config, &project);
                serversync::update_remembered(&config, |r| r.remove(&project));
                return Err(format!(
                    "Reopening failed: {e} It is no longer listed here."
                ));
            }
            return Err(format!("Reopening failed: {e}"));
        }
        None => return Err(NO_ANSWER.into()),
    }
    open_joined(
        &app,
        &window,
        &state,
        &identity,
        &config,
        sync,
        path,
        Some(&entry.title),
    )
}

// ---- Background saving and lifecycle ----

/// Autosaves every second and tries an automatic checkpoint every minute.
/// Failures are reported to the window as `save-error` (once per distinct
/// message), recovery as `save-ok`.
fn autosave_loop(app: AppHandle) {
    let mut tick: u64 = 0;
    let mut failing: Option<String> = None;
    loop {
        thread::sleep(Duration::from_secs(1));
        tick += 1;
        let result = match app.state::<AppState>().lock().as_mut() {
            None => continue,
            Some(project) => (|| {
                if tick.is_multiple_of(AUTO_CHECKPOINT_EVERY_SECS) {
                    project.doc.maybe_auto_checkpoint(SystemTime::now())?;
                }
                // Push failures go to the sync status, not the save error.
                project.save()
            })(),
        };
        match result.map_err(message) {
            Ok(()) => {
                if failing.take().is_some() {
                    let _ = app.emit("save-ok", ());
                }
            }
            Err(e) => {
                if failing.as_ref() != Some(&e) {
                    eprintln!("Autosave failed: {e}");
                    let _ = app.emit("save-error", format!("Saving failed: {e}"));
                    failing = Some(e);
                }
            }
        }
    }
}

/// Saves and closes the open project; managed state may never be dropped.
fn close_project(app: &AppHandle) {
    let project = app.state::<AppState>().lock().take();
    if let Some(mut project) = project
        && let Err(e) = project.save()
    {
        eprintln!("Saving {} on exit failed: {e}", project.path.display());
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(AppState::default())
        .manage(TypesetState::default())
        .manage(Updates::default())
        .setup(|app| {
            let config = app.path().app_config_dir().ok();
            app.manage(load_identity(config.as_deref()));
            app.manage(Keys(load_keys(config.as_deref())));
            app.manage(Spell::load(config.as_deref()));
            let settings = SettingsState::load(config.as_deref());
            // The windows exist already but haven't painted yet.
            settings::apply_theme(app.handle(), settings.get().theme);
            app.manage(settings);
            app.manage(CompileState::start(app.handle().clone())?);
            let handle = app.handle().clone();
            thread::Builder::new()
                .name("autosave".into())
                .spawn(move || autosave_loop(handle))?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            project_new,
            project_open,
            project_last,
            project_close,
            projects_recent,
            projects_recent_remove,
            projects_recent_clear,
            text,
            edit,
            undo,
            redo,
            save_now,
            checkpoints,
            checkpoint_create,
            checkpoint_text,
            checkpoint_restore,
            render_math,
            spell_check,
            spell_suggest,
            spell_add,
            bib_entries,
            bib_import,
            bib_import_file,
            bib_format,
            bib_styles,
            asset_import,
            asset_import_bytes,
            asset_stage,
            asset_stage_bytes,
            figure_insert,
            import_formats,
            import_read,
            asset_replace,
            asset_bytes,
            asset_update,
            asset_save,
            asset_list,
            asset_move,
            asset_remove,
            asset_folder_create,
            layout_get,
            layout_set,
            layout_apply_preset,
            layout_presets,
            layout_fonts,
            layout_embedded_fonts,
            layout_font_file,
            meta_set,
            layout_custom_set,
            overrides_get,
            override_add,
            override_remove,
            override_update,
            overrides_batch,
            compile_watch,
            doc_version,
            compile_now,
            page_svg,
            page_jump,
            export_pdf,
            export_formats,
            export_file,
            sync_status,
            app_info,
            settings::settings_get,
            settings::settings_set,
            updater::update_check,
            updater::update_install,
            sync_setup,
            sync_setup_app,
            sync_apps,
            sync_find,
            sync_join,
            projects_folder,
            sync_code,
            sync_enter_code,
            sync_stop,
            sync_pull,
            sync_notice_dismiss,
            server_recent,
            server_share,
            server_invite,
            server_join,
            server_members,
            server_set_role,
            server_set_colour,
            server_remove,
            server_revoke,
            server_leave,
            server_delete,
            server_remembered,
            server_reopen,
            presence_set,
            presence_get,
            authors_get,
            review_list,
            review_comment,
            review_mark,
            review_reply,
            review_edit,
            review_delete,
            review_resolve,
            review_colour,
            review_remove,
            review_suggest,
            review_accept,
            review_reject,
            review_undo,
            review_redo,
            #[cfg(windows)]
            snap::snap_rect,
            #[cfg(debug_assertions)]
            text_hash,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Lethora")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                close_project(app);
            }
        });
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn side_files_stay_in_the_folder() {
        let folder = Path::new("out");
        assert_eq!(
            side_file(folder, "a_images/x.png").unwrap(),
            folder.join("a_images").join("x.png")
        );
        for bad in [
            "../x.png",
            "a/../../x",
            "/abs",
            "a//b",
            "C:x",
            "a\\b",
            "",
            "./x",
        ] {
            assert!(side_file(folder, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn layout_changes_are_all_or_nothing() {
        let ok: BTreeMap<String, Option<Value>> = [
            ("text.size".to_owned(), Some(json!(" 12PT"))),
            ("page.columns".to_owned(), None),
        ]
        .into();
        let valid = validate_changes(ok, layout::validate_change).unwrap();
        assert_eq!(valid["text.size"], Some(json!("12pt")));
        assert_eq!(valid["page.columns"], None);

        let bad: BTreeMap<String, Option<Value>> = [
            ("text.size".to_owned(), Some(json!("12pt"))),
            ("text.font".to_owned(), Some(json!("\")#read(\"x\")"))),
            ("nope".to_owned(), Some(json!(1))),
        ]
        .into();
        let e = validate_changes(bad, layout::validate_change).unwrap_err();
        assert!(e.contains("text.font") && e.contains("nope"), "{e}");
        assert!(!e.contains("text.size"), "{e}");
    }

    #[test]
    fn new_project_starts_with_preset_and_meta_and_nothing_to_undo() {
        let base = std::env::temp_dir().join(format!("lethora-new-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let meta: BTreeMap<String, Option<Value>> = [
            ("title".to_owned(), Some(json!("The Keeper"))),
            ("author".to_owned(), Some(json!(""))),
        ]
        .into();
        let doc = create_project(&base.join("a"), &test_identity(), Some("novel"), meta).unwrap();
        assert!(!doc.can_undo());
        let state = layout_state(&doc);
        assert_eq!(state.preset, "novel");
        assert_eq!(state.meta.title, "The Keeper");
        assert!(state.overrides.is_empty());
        assert!(!doc.map_entries(MapName::Meta).contains_key("author"));
        drop(doc);

        // Checked before anything is written.
        assert!(
            create_project(
                &base.join("b"),
                &test_identity(),
                Some("nope"),
                BTreeMap::new()
            )
            .is_err()
        );
        let bad: BTreeMap<String, Option<Value>> =
            [("title".to_owned(), Some(json!("a\nb")))].into();
        assert!(create_project(&base.join("c"), &test_identity(), None, bad).is_err());
        assert!(!base.join("b").exists() && !base.join("c").exists());

        // No choices: a plain new project.
        let plain =
            create_project(&base.join("d"), &test_identity(), None, BTreeMap::new()).unwrap();
        assert_eq!(layout_state(&plain).preset, layout::DEFAULT_PRESET);
        drop(plain);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn character_styles_are_set_and_listed() {
        let dir = tempfile_dir("styles");
        let mut doc = Document::create(&dir, &test_identity()).unwrap();
        let changes: BTreeMap<String, Option<Value>> = [
            ("styles.big.size".to_owned(), Some(json!(" 14PT"))),
            ("styles.big.name".to_owned(), Some(json!(" Big "))),
            (
                "styles.mono.font".to_owned(),
                Some(json!("dejavu sans mono")),
            ),
        ]
        .into();
        let changes = validate_changes(changes, layout::validate_change).unwrap();
        doc.map_update(MapName::Layout, &changes).unwrap();
        let state = serde_json::to_value(layout_state(&doc)).unwrap();
        assert_eq!(
            state["layout"]["styles"],
            json!({
                "big": { "name": "Big", "size": "14pt" },
                "mono": { "font": "DejaVu Sans Mono" },
            })
        );
        assert_eq!(state["overrides"]["styles.big.size"], json!("14pt"));
        // `null` removes a field; a style left without font and size goes.
        let remove: BTreeMap<String, Option<Value>> = [("styles.big.size".to_owned(), None)].into();
        doc.map_update(MapName::Layout, &remove).unwrap();
        let state = serde_json::to_value(layout_state(&doc)).unwrap();
        assert_eq!(
            state["layout"]["styles"],
            json!({ "mono": { "font": "DejaVu Sans Mono" } })
        );
        drop(doc);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn table_styles_anchor_at_the_table() {
        let dir = tempfile_dir("table-styles");
        let mut doc = Document::create(&dir, &test_identity()).unwrap();
        let text = "Intro.

#figure(
  table(columns: 1, [a]),
  caption: [Cap],
) <tab:1>
";
        doc.insert(0, text).unwrap();
        let u16 = |needle: &str| byte_to_utf16(text, text.find(needle).unwrap());
        let style = |s| Kind::TableStyle { style: s };
        // In a cell, on the caption: the same table, replaced.
        let id = add_override(&mut doc, style(layout::TableStyle::Grid), u16("[a]")).unwrap();
        assert_eq!(
            add_override(&mut doc, style(layout::TableStyle::Plain), u16("Cap")).unwrap(),
            id
        );
        assert_eq!(
            add_override(&mut doc, style(layout::TableStyle::Striped), u16("#figure")).unwrap(),
            id
        );
        assert_eq!(
            add_override(&mut doc, style(layout::TableStyle::Grid), u16("Intro")),
            Err("There is no table here.".to_owned())
        );
        let infos = override_infos(&doc, &Default::default());
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].kind, Some("table_style"));
        assert_eq!(infos[0].pos_utf16, Some(u16("table(")));
        assert_eq!(infos[0].params["style"], json!("striped"));
        drop(doc);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn overrides_are_added_replaced_listed_and_resolved() {
        let dir = tempfile_dir("overrides");
        let mut doc = Document::create(&dir, &test_identity()).unwrap();
        let text = "Intro.

= Über 🌍

Text.

```
raw
```

#figure(rect(), caption: [Cap])

#figure(image(\"a.png\"))
";
        doc.insert(0, text).unwrap();
        let u16 = |needle: &str| byte_to_utf16(text, text.find(needle).unwrap());
        let pb = |to_odd| Kind::PageBreak { to_odd };
        let place = |p| Kind::FigurePlacement { placement: p };

        // Anywhere in the heading's line: anchored at the line start.
        let id = add_override(&mut doc, pb(false), u16("🌍")).unwrap();
        // Same kind, same block: replaced.
        assert_eq!(add_override(&mut doc, pb(true), u16("Über")).unwrap(), id);
        let space = add_override(
            &mut doc,
            overrides::validate_new(
                "space_before",
                &json!({"length": "1em"}).as_object().unwrap().clone(),
            )
            .unwrap(),
            u16("= Über"),
        )
        .unwrap();
        assert_ne!(space, id);
        // An image quality anchors at the image call around the position.
        let quality = |q| Kind::ImageQuality { quality: q };
        let img = add_override(
            &mut doc,
            quality(layout::ImageQuality::Screen),
            u16("a.png"),
        )
        .unwrap();
        assert_eq!(
            add_override(
                &mut doc,
                quality(layout::ImageQuality::Email),
                u16("image(")
            )
            .unwrap(),
            img
        );
        assert!(add_override(&mut doc, quality(layout::ImageQuality::Print), u16("Cap")).is_err());
        let fig = add_override(&mut doc, place(overrides::Placement::Top), u16("Cap")).unwrap();
        assert_eq!(
            add_override(
                &mut doc,
                place(overrides::Placement::Bottom),
                u16("#figure")
            )
            .unwrap(),
            fig
        );
        // Not at block level, no figure, bad positions.
        assert!(add_override(&mut doc, pb(false), u16("raw")).is_err());
        assert!(add_override(&mut doc, place(overrides::Placement::Top), u16("Text")).is_err());
        assert!(add_override(&mut doc, pb(false), 10_000).is_err());
        assert!(add_override(&mut doc, pb(false), u16("🌍") + 1).is_err());

        let applied = [(id.clone(), true)].into();
        let infos = override_infos(&doc, &applied);
        assert_eq!(infos.len(), 4);
        assert_eq!(infos[0].pos_utf16, Some(u16("= Über")));
        assert!(infos[0].anchor_preview.starts_with("= Über 🌍"));
        let page_break = infos.iter().find(|i| i.id == id).unwrap();
        assert_eq!(page_break.params["to_odd"], json!(true));
        assert_eq!(page_break.applicable, Some(true));
        assert_eq!(infos[2].id, fig);
        assert_eq!(infos[2].params["placement"], json!("bottom"));
        assert_eq!(infos[2].applicable, None);
        assert_eq!(infos[3].id, img);
        assert_eq!(infos[3].pos_utf16, Some(u16("image(")));
        assert_eq!(infos[3].params["quality"], json!("email"));

        // Resolved for compiling; text inserted before moves them along.
        doc.insert(0, "More. ").unwrap();
        let resolved = compile::resolve_overrides(&doc);
        let heading = doc.text().find("= Über").unwrap();
        assert!(
            resolved
                .iter()
                .filter(|r| r.kind.is_block())
                .all(|r| r.content_byte_offset == heading)
        );
        // Each change is one undo step.
        doc.undo().unwrap();
        doc.undo().unwrap();
        assert_eq!(
            override_infos(&doc, &applied)[2].params["placement"],
            json!("top")
        );
        drop(doc);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn override_batches_apply_together_and_undo_in_one_step() {
        let dir = tempfile_dir("batch");
        let mut doc = Document::create(&dir, &test_identity()).unwrap();
        let text = "= One

A.

= Two

B.
";
        doc.insert(0, text).unwrap();
        let u16 = |needle: &str| byte_to_utf16(text, text.find(needle).unwrap());
        let space = |length: &str| {
            overrides::validate_new(
                "space_before",
                &json!({ "length": length }).as_object().unwrap().clone(),
            )
            .unwrap()
        };
        let a = add_override(&mut doc, space("1em"), u16("A.")).unwrap();
        let b = add_override(&mut doc, space("2em"), u16("B.")).unwrap();
        let c = add_override(&mut doc, Kind::PageBreak { to_odd: false }, u16("= Two")).unwrap();
        let params = |length: &str| json!({ "length": length }).as_object().unwrap().clone();
        let lengths = |doc: &Document| -> Vec<(String, Value)> {
            override_infos(doc, &Default::default())
                .into_iter()
                .map(|i| (i.id, i.params.get("length").cloned().unwrap_or(Value::Null)))
                .collect()
        };
        let before = lengths(&doc);

        // A bad operation anywhere: nothing applies, the error names it.
        let bad = [
            BatchOp::Update {
                id: a.clone(),
                params: params("12pt"),
            },
            BatchOp::Update {
                id: b.clone(),
                params: params("12px"),
            },
        ];
        let e = apply_batch(&mut doc, &bad).unwrap_err();
        assert!(e.contains(&b) && !e.contains(&a), "{e}");
        let twice = [
            BatchOp::Remove { id: c.clone() },
            BatchOp::Remove { id: c.clone() },
        ];
        assert!(apply_batch(&mut doc, &twice).unwrap_err().contains(&c));
        assert!(apply_batch(&mut doc, &[BatchOp::Remove { id: "nope".into() }]).is_err());
        assert_eq!(lengths(&doc), before);

        let ops = [
            BatchOp::Update {
                id: a.clone(),
                params: params("12pt"),
            },
            BatchOp::Update {
                id: b.clone(),
                params: params("12pt"),
            },
            BatchOp::Remove { id: c.clone() },
        ];
        apply_batch(&mut doc, &ops).unwrap();
        assert_eq!(
            lengths(&doc),
            [(a.clone(), json!("12pt")), (b.clone(), json!("12pt"))]
        );
        // One undo step for the whole batch.
        doc.undo().unwrap();
        assert_eq!(lengths(&doc), before);
        drop(doc);
        let _ = fs::remove_dir_all(&dir);
    }

    fn test_identity() -> Identity {
        Identity {
            device: DeviceId::parse("0123456789abcdef0123456789abcdef").unwrap(),
            user: UserId::parse("fedcba9876543210fedcba9876543210").unwrap(),
        }
    }

    #[test]
    fn personal_words_are_saved_sorted_once_and_read_back() {
        let dir = tempfile_dir("dictionary");
        let spell = Spell::load(Some(&dir));
        spell.add("Quorbel").unwrap();
        spell.add("Lethora").unwrap();
        spell.add("Quorbel").unwrap();
        assert_eq!(
            fs::read_to_string(dir.join(DICTIONARY)).unwrap(),
            "Lethora\nQuorbel\n"
        );
        let again = Spell::load(Some(&dir));
        assert_eq!(again.speller.words(), ["Lethora", "Quorbel"]);
        let words = ["Lethora".to_owned(), "Quorbl".to_owned()];
        assert_eq!(
            again.speller.check(lethora_spell::Lang::EnUs, &words),
            ["Quorbl"]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn identity_is_created_once_and_renewed_when_corrupt() {
        let dir = tempfile_dir("identity");
        let first = load_identity(Some(&dir));
        assert_eq!(first.device.as_str().len(), 32);
        assert_eq!(first.user.as_str().len(), 32);
        let again = load_identity(Some(&dir));
        assert_eq!((&again.device, &again.user), (&first.device, &first.user));
        fs::write(dir.join(DEVICE), "{").unwrap();
        let renewed = load_identity(Some(&dir));
        assert_ne!(renewed.device, first.device);
        assert_eq!(renewed.user, first.user);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn images_are_imported_reused_replaced_and_adopted() {
        let dir = tempfile_dir("assets");
        let mut doc = Document::create(&dir, &test_identity()).unwrap();
        let import = |doc: &mut Document, name: &str, ext: &str, bytes: &[u8]| {
            import_asset(doc, Path::new(name), ext, bytes, "assets/").unwrap()
        };
        let a = import(&mut doc, "C:/x/My Photo.png", "png", b"one");
        assert_eq!(a, "assets/my-photo.png");
        // The same content in the same format is reused; otherwise a new path.
        assert_eq!(import(&mut doc, "other.png", "png", b"one"), a);
        assert_eq!(
            import(&mut doc, "My Photo.svg", "svg", b"one"),
            "assets/my-photo.svg"
        );
        assert_eq!(
            import(&mut doc, "my-photo.png", "png", b"two"),
            "assets/my-photo-2.png"
        );

        // Replacing repoints only the image call at `pos`, which must read
        // `path`, to a path of its own (any format); the old entry stays.
        let text = format!("#image(\"{a}\")\n#figure(image( \"{a}\"))");
        doc.insert(0, &text).unwrap();
        let second = text.rfind("image(").unwrap();
        let replace = |doc: &mut Document, path: &str, pos: usize, name: &str, ext: &str| {
            replace_asset(doc, path, pos, Path::new(name), ext, b"three")
        };
        for (path, pos) in [
            ("assets/my-photo.svg", 0),
            (a.as_str(), second + 1),
            (a.as_str(), text.len() + 1),
            ("assets/../my-photo.png", 0),
        ] {
            assert!(
                replace(&mut doc, path, pos, "new.png", "png").is_err(),
                "{path} {pos}"
            );
        }
        let r = replace(&mut doc, &a, second, "D:/New Shot.JPG", "jpg").unwrap();
        assert_eq!(r.path, "assets/new-shot.jpg");
        let after = format!("#image(\"{a}\")\n#figure(image( \"assets/new-shot.jpg\"))");
        assert_eq!(doc.text(), after);
        assert_eq!(r.reply.len16, after.len());
        assert_eq!(doc.asset_bytes(&a).unwrap().unwrap(), b"one");
        assert_eq!(
            doc.asset_bytes("assets/new-shot.jpg").unwrap().unwrap(),
            b"three"
        );
        doc.undo().unwrap();
        assert_eq!(doc.text(), text);
        // The same content in the same format reuses its path.
        let r = replace_asset(&mut doc, &a, 0, Path::new("copy.png"), "png", b"two").unwrap();
        assert_eq!(r.path, "assets/my-photo-2.png");
        assert!(
            doc.text()
                .starts_with("#image(\"assets/my-photo-2.png\")\n")
        );
        drop(doc);

        // Loose image files are adopted on open (others stay).
        fs::write(dir.join("assets/loose.png"), b"loose").unwrap();
        fs::write(dir.join("assets/notes.txt"), b"x").unwrap();
        let mut doc = Document::open(&dir, &test_identity()).unwrap();
        assert_eq!(adopt_assets(&mut doc).unwrap(), ["assets/loose.png"]);
        assert_eq!(
            doc.asset_bytes("assets/loose.png").unwrap().unwrap(),
            b"loose"
        );
        assert!(dir.join("backup/assets/loose.png").exists());
        assert!(dir.join("assets/notes.txt").exists());
        drop(doc);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn files_are_imported_into_folders_and_from_bytes() {
        let dir = tempfile_dir("asset-folders");
        let mut doc = Document::create(&dir, &test_identity()).unwrap();
        let into = |doc: &mut Document, name: &str, folder: Option<&str>, bytes: &[u8]| {
            import_bytes(doc, name, folder.map(str::to_owned), bytes)
        };
        assert_eq!(
            into(&mut doc, "Photo.PNG", None, b"one").unwrap(),
            "assets/photo.png"
        );
        let a = into(
            &mut doc,
            "C:\\x\\Photo.png",
            Some("assets/Kapitel 1/"),
            b"one",
        )
        .unwrap();
        assert_eq!(a, "assets/Kapitel 1/photo.png");
        assert!(doc.asset_folders().contains("assets/Kapitel 1/"));
        // Reused within the folder only; the marker came in the same step.
        assert_eq!(
            into(&mut doc, "again.png", Some("assets/Kapitel 1/"), b"one").unwrap(),
            a
        );
        doc.undo().unwrap();
        assert!(doc.asset_blob(&a).is_none() && doc.asset_folders().is_empty());
        doc.redo().unwrap();
        let path = import_asset(
            &mut doc,
            Path::new("D:/b.svg"),
            "svg",
            b"<svg/>",
            "assets/x/",
        )
        .unwrap();
        assert_eq!(path, "assets/x/b.svg");

        // The import check and the folder are validated.
        for (name, folder) in [
            ("notes.txt", None),
            ("a.png", Some("assets")),
            ("a.png", Some("other/")),
            ("a.png", Some("assets/../")),
        ] {
            assert!(
                into(&mut doc, name, folder, b"x").is_err(),
                "{name} {folder:?}"
            );
        }
        let huge = vec![0; assets::MAX_BYTES as usize + 1];
        assert!(
            into(&mut doc, "big.png", None, &huge)
                .unwrap_err()
                .contains("larger")
        );
        assert_eq!(
            percent_decode("Caf%C3%A9%20%2F%22x%22.png").as_deref(),
            Some("Café /\"x\".png")
        );
        for bad in ["%", "%4", "%zz", "%+1", "%C3"] {
            assert_eq!(percent_decode(bad), None, "{bad}");
        }

        // The listing: folders from markers and paths, usage, status.
        create_folder(&mut doc, "assets/empty/sub/").unwrap();
        create_folder(&mut doc, "assets/empty/").unwrap();
        create_folder(&mut doc, "assets/").unwrap();
        assert!(!doc.asset_folders().contains("assets/empty/"));
        doc.insert(0, &format!("#image(\"{a}\") #image(\"{a}\")"))
            .unwrap();
        let blob = doc.asset_blob("assets/x/b.svg").unwrap();
        fs::remove_file(dir.join("assets").join(blob)).unwrap();
        let list = serde_json::to_value(asset_listing(&doc).unwrap()).unwrap();
        assert_eq!(
            list["folders"],
            json!([
                "assets/Kapitel 1/",
                "assets/empty/",
                "assets/empty/sub/",
                "assets/x/"
            ])
        );
        assert_eq!(
            list["items"],
            json!([
                { "path": a, "kind": "image", "size": 3, "used_by": 2, "status": "available" },
                { "path": "assets/photo.png", "kind": "image", "size": 3, "used_by": 0, "status": "available" },
                { "path": "assets/x/b.svg", "kind": "image", "size": null, "used_by": 0, "status": "missing" },
            ])
        );
        drop(doc);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn staged_files_get_free_paths_and_go_in_with_their_figures() {
        let dir = tempfile_dir("asset-stage");
        let mut doc = Document::create(&dir, &test_identity()).unwrap();
        let a = import_asset(&mut doc, Path::new("a.png"), "png", b"one", "assets/").unwrap();
        doc.insert(0, "Text.").unwrap();
        let stage = |doc: &mut Document, name: &str, folder: &str, bytes: &[u8], taken: &[&str]| {
            let taken: Vec<String> = taken.iter().map(|t| (*t).to_owned()).collect();
            let (source, ext) = bytes_source(name, bytes).unwrap();
            stage_asset(doc, source, &ext, bytes, folder, &taken).unwrap()
        };

        // Sanitized, unique against the index and the paths proposed already;
        // the same content in the same format reuses its path. Nothing is
        // indexed yet.
        let b = stage(&mut doc, "C:\\x\\My Photo.PNG", "assets/", b"two", &[]);
        assert_eq!(b.path, "assets/my-photo.png");
        let c = stage(&mut doc, "My Photo.png", "assets/", b"three", &[&b.path]);
        assert_eq!(c.path, "assets/my-photo-2.png");
        assert_eq!(
            stage(&mut doc, "a.png", "assets/", b"four", &[]).path,
            "assets/a-2.png"
        );
        let same = stage(&mut doc, "copy.png", "assets/", b"one", &[&a]);
        assert_eq!(same.path, a);
        let d = stage(&mut doc, "d.svg", "assets/Kapitel 1/", b"<svg/>", &[]);
        assert_eq!(d.path, "assets/Kapitel 1/d.svg");
        assert_eq!(doc.assets().len(), 1);
        assert!(!doc.can_redo());
        assert!(bytes_source("notes.txt", b"x").is_err());
        assert_eq!(
            taken_list("assets%2Fa.png,assets%2Fb%2C%20c.png,").unwrap(),
            ["assets/a.png", "assets/b, c.png"]
        );
        assert_eq!(taken_list(""), Some(vec![]));
        assert_eq!(taken_list("assets%2"), None);

        // Inserted with their figures in one step.
        let text = format!("\n\n#image(\"{}\") #image(\"{}\")", b.path, d.path);
        let r = insert_figures(&mut doc, 5, &text, &[b, d, same]).unwrap();
        assert_eq!(doc.text(), format!("Text.{text}"));
        assert_eq!(r.len16, doc.len_utf16());
        assert_eq!(
            doc.asset_bytes("assets/my-photo.png").unwrap().unwrap(),
            b"two"
        );
        assert!(doc.asset_folders().contains("assets/Kapitel 1/"));
        let undone = undo_step(&mut doc, Document::undo).unwrap().unwrap();
        assert!(undone.assets);
        assert_eq!(doc.text(), "Text.");
        assert_eq!(doc.assets().len(), 1);

        // Validated; a proposed path given other content meanwhile is refused.
        let e = stage(&mut doc, "e.png", "assets/", b"five", &[]);
        let bad = Staged {
            path: "assets/../e.png".into(),
            blob: e.blob.clone(),
        };
        assert!(insert_figures(&mut doc, 0, "x", &[bad]).is_err());
        import_asset(&mut doc, Path::new("e.png"), "png", b"other", "assets/").unwrap();
        let err = insert_figures(&mut doc, 0, "x", &[e])
            .map(|_| ())
            .unwrap_err();
        assert!(err.contains("try again"), "{err}");
        assert!(insert_figures(&mut doc, 99, "x", &[]).is_err());
        assert_eq!(doc.text(), "Text.");
        drop(doc);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn imports_stage_their_images_and_cite_only_known_sources() {
        let dir = tempfile_dir("import");
        fs::create_dir_all(dir.join("in/pics")).unwrap();
        let file = dir.join("in/My Notes.md");
        fs::write(
            &file,
            "# Notes\n\n![Dot](pics/dot.svg)\n\n![Raw](pics/x.tiff)\n\nAs @doe says, and @nobody.\n",
        )
        .unwrap();
        fs::write(dir.join("in/pics/dot.svg"), "<svg/>").unwrap();
        fs::write(dir.join("in/pics/x.tiff"), "tiff").unwrap();
        let mut doc = Document::create(&dir.join("p"), &test_identity()).unwrap();
        doc.set_bibliography("@book{doe, title = {T}, author = {Doe, J.}, year = {2020}}")
            .unwrap();

        let (format, imported) = read_import(&file).unwrap();
        assert_eq!(format.id, "markdown");
        let reply =
            import_markup(format.name, imported, Some(&mut doc), "assets/my-notes/").unwrap();
        assert_eq!(reply.assets.len(), 1);
        assert_eq!(reply.assets[0].path, "assets/my-notes/dot.svg");
        assert!(
            reply
                .text
                .contains("image(\"assets/my-notes/dot.svg\", width: 80%)")
        );
        assert!(
            reply.text.contains("As @doe says, and \\@nobody."),
            "{}",
            reply.text
        );
        let kinds: Vec<_> = reply.warnings.iter().map(|w| w.kind).collect();
        assert_eq!(
            kinds,
            [
                convert::WarningKind::ImageLeftOut,
                convert::WarningKind::UnknownCitation
            ]
        );
        // Nothing indexed until the insert.
        assert!(doc.assets().is_empty());
        insert_figures(&mut doc, 0, &reply.text, &reply.assets).unwrap();
        assert_eq!(
            doc.asset_bytes("assets/my-notes/dot.svg").unwrap().unwrap(),
            b"<svg/>"
        );

        // Without a document: no images, no sources.
        let (format, imported) = read_import(&file).unwrap();
        let bare = import_markup(format.name, imported, None, "assets/x/").unwrap();
        assert!(bare.assets.is_empty() && !bare.text.contains("image("));
        assert!(
            read_import(&dir.join("in/pics/x.tiff")).is_ok(),
            "text by sniffing"
        );
        assert!(read_import(&dir.join("in/none.md")).is_err());
        drop(doc);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn assets_move_and_go_by_folder_with_free_names() {
        let dir = tempfile_dir("asset-moves");
        let mut doc = Document::create(&dir, &test_identity()).unwrap();
        for (name, folder) in [
            ("a.png", "assets/"),
            ("b.png", "assets/f/"),
            ("c.jpg", "assets/f/sub/"),
        ] {
            import_asset(
                &mut doc,
                Path::new(name),
                name.rsplit('.').next().unwrap(),
                name.as_bytes(),
                folder,
            )
            .unwrap();
        }
        let text = "#image(\"assets/a.png\") #image(\"assets/f/sub/c.jpg\")";
        doc.insert(0, text).unwrap();

        // Free names; the format stays.
        let r = move_asset(&mut doc, "assets/a.png", "assets/Größe 2 (final).png").unwrap();
        assert_eq!(r.path, "assets/Größe 2 (final).png");
        assert_eq!(
            doc.text(),
            "#image(\"assets/Größe 2 (final).png\") #image(\"assets/f/sub/c.jpg\")"
        );
        assert_eq!(r.reply.len16, doc.len_utf16());
        let before = doc.text();
        for (from, to) in [
            ("assets/Größe 2 (final).png", "assets/a.svg"),
            ("assets/Größe 2 (final).png", "assets/a.PNG"),
            ("assets/Größe 2 (final).png", "assets/a\".png"),
            ("assets/Größe 2 (final).png", "assets/ a.png"),
            ("assets/Größe 2 (final).png", "assets/a:b.png"),
            ("assets/Größe 2 (final).png", "assets/a\tb.png"),
            ("assets/Größe 2 (final).png", "assets/f/b.png"),
            ("assets/none.png", "assets/x.png"),
            ("assets/f/", "assets/f/g/"),
            ("assets/f/", "assets/"),
            ("assets/", "assets/g/"),
            ("assets/f/", "assets/g"),
            ("assets/none/", "assets/g/"),
        ] {
            assert!(move_asset(&mut doc, from, to).is_err(), "{from} -> {to}");
        }
        assert_eq!(doc.text(), before);
        assert!(move_asset(&mut doc, "assets/f/sub/c.jpg", "assets/f/sub/c.jpeg").is_ok());
        doc.undo().unwrap();

        // A folder moves with its subfolders, markers and items in one step.
        let r = move_asset(&mut doc, "assets/f/", "assets/Bilder/").unwrap();
        assert_eq!(r.path, "assets/Bilder/");
        assert!(doc.text().ends_with("#image(\"assets/Bilder/sub/c.jpg\")"));
        assert_eq!(
            listed_folders(&doc),
            BTreeSet::from(["assets/Bilder/".to_owned(), "assets/Bilder/sub/".to_owned()])
        );
        doc.undo().unwrap();
        assert_eq!(doc.text(), before);
        assert!(doc.asset_blob("assets/f/b.png").is_some());

        // Removing: an unused item, a folder only if nothing in it is used.
        remove_asset(&mut doc, "assets/f/b.png").unwrap();
        assert!(
            doc.asset_folders().contains("assets/f/"),
            "the folder stays"
        );
        assert!(remove_asset(&mut doc, "assets/f/").is_err());
        assert!(remove_asset(&mut doc, "assets/Größe 2 (final).png").is_err());
        assert!(remove_asset(&mut doc, "assets/").is_err());
        assert!(remove_asset(&mut doc, "assets/none/").is_err());
        doc.delete(0, doc.len()).unwrap();
        remove_asset(&mut doc, "assets/f/").unwrap();
        assert!(listed_folders(&doc).is_empty());
        assert_eq!(doc.assets().len(), 1);
        doc.undo().unwrap();
        assert_eq!(listed_folders(&doc).len(), 2);
        assert!(doc.asset_blob("assets/f/sub/c.jpg").is_some());
        drop(doc);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn assets_get_new_content_of_their_format_and_are_saved() {
        let dir = tempfile_dir("asset-update");
        let mut doc = Document::create(&dir, &test_identity()).unwrap();
        let a = import_asset(&mut doc, Path::new("a.jpeg"), "jpeg", b"one", "assets/").unwrap();
        let text = format!("#image(\"{a}\") #image(\"{a}\")");
        doc.insert(0, &text).unwrap();

        // Same path, new content: jpg and jpeg are one format; the text stays.
        for (path, ext) in [
            (a.as_str(), "png"),
            (a.as_str(), "svg"),
            ("assets/none.jpeg", "jpeg"),
            ("assets/../a.jpeg", "jpeg"),
        ] {
            assert!(
                update_asset(&mut doc, path, ext, b"x").is_err(),
                "{path} {ext}"
            );
        }
        assert!(
            update_asset(&mut doc, &a, "png", b"x")
                .unwrap_err()
                .contains(".jpeg")
        );
        update_asset(&mut doc, &a, "jpg", b"two").unwrap();
        assert_eq!(doc.asset_bytes(&a).unwrap().unwrap(), b"two");
        assert_eq!(doc.text(), text);
        doc.undo().unwrap();
        assert_eq!(doc.asset_bytes(&a).unwrap().unwrap(), b"one");
        doc.redo().unwrap();

        // Saving writes the blob's bytes; missing blobs and paths are refused.
        let target = dir.join("saved.jpeg");
        let blob = doc.asset_blob(&a);
        save_asset(&dir, blob.as_deref(), &a, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"two");
        assert!(save_asset(&dir, None, "assets/none.png", &target).is_err());
        fs::remove_file(dir.join("assets").join(blob.as_deref().unwrap())).unwrap();
        assert!(save_asset(&dir, blob.as_deref(), &a, &dir.join("gone.jpeg")).is_err());
        assert!(!dir.join("gone.jpeg").exists());
        drop(doc);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn undo_says_whether_the_asset_index_changed() {
        let dir = tempfile_dir("asset-undo");
        let mut doc = Document::create(&dir, &test_identity()).unwrap();
        doc.insert(0, "x").unwrap();
        import_asset(&mut doc, Path::new("a.png"), "png", b"a", "assets/").unwrap();
        let undone = undo_step(&mut doc, Document::undo).unwrap().unwrap();
        assert!(undone.assets && undone.changes.is_empty());
        assert!(undo_step(&mut doc, Document::redo).unwrap().unwrap().assets);
        doc.end_undo_step();
        doc.insert(1, "y").unwrap();
        let typing = undo_step(&mut doc, Document::undo).unwrap().unwrap();
        assert!(!typing.assets);
        assert_eq!(serde_json::to_value(&typing).unwrap().get("assets"), None);
        drop(doc);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn recent_projects_are_newest_first_once_and_at_most_ten() {
        let mut recent = Recent::default();
        assert_eq!(recent.last(), None);
        for i in 0..12 {
            recent.add(Path::new(&format!("/p/{i}.lethora")));
        }
        assert_eq!(recent.paths.len(), RECENT_MAX);
        assert_eq!(recent.paths[0], Path::new("/p/11.lethora"));
        assert_eq!(recent.paths[9], Path::new("/p/2.lethora"));
        assert_eq!(recent.last(), Some(Path::new("/p/11.lethora")));

        // Opening one again moves it to the top, listed once.
        recent.add(Path::new("/p/5.lethora"));
        assert_eq!(recent.paths.len(), RECENT_MAX);
        assert_eq!(recent.paths[0], Path::new("/p/5.lethora"));
        assert_eq!(
            recent
                .paths
                .iter()
                .filter(|p| p.as_path() == Path::new("/p/5.lethora"))
                .count(),
            1
        );

        // Removing another one keeps the reopen; removing the newest drops it.
        recent.remove(Path::new("/p/7.lethora"));
        assert_eq!(recent.paths.len(), RECENT_MAX - 1);
        assert_eq!(recent.last(), Some(Path::new("/p/5.lethora")));
        recent.remove(Path::new("/p/5.lethora"));
        assert_eq!(recent.paths[0], Path::new("/p/11.lethora"));
        assert_eq!(recent.last(), None);

        // Closing keeps the list; clearing keeps only the open project.
        recent.add(Path::new("/p/3.lethora"));
        recent.reopen = false;
        assert_eq!(recent.last(), None);
        assert_eq!(recent.paths[0], Path::new("/p/3.lethora"));
        recent.clear(Some(Path::new("/p/3.lethora")));
        assert_eq!(recent.paths, vec![PathBuf::from("/p/3.lethora")]);
        recent.clear(None);
        assert!(recent.paths.is_empty());
    }

    #[test]
    fn recent_projects_are_stored_and_read_from_the_old_last_project_file() {
        let dir = tempfile_dir("recent");
        assert_eq!(Recent::read(&dir), Recent::default());

        // An installation from before the list: its last project reopens.
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(LAST_PROJECT), br#"{"path":"/p/old.lethora"}"#).unwrap();
        let mut recent = Recent::read(&dir);
        assert_eq!(recent.paths, vec![PathBuf::from("/p/old.lethora")]);
        assert_eq!(recent.last(), Some(Path::new("/p/old.lethora")));

        // Writing the list replaces the old file.
        recent.add(Path::new("/p/new.lethora"));
        recent.write(&dir).unwrap();
        assert!(!dir.join(LAST_PROJECT).exists());
        assert_eq!(Recent::read(&dir), recent);
        assert_eq!(
            Recent::read(&dir).paths,
            vec![
                PathBuf::from("/p/new.lethora"),
                PathBuf::from("/p/old.lethora")
            ]
        );

        // A corrupt file is an empty list.
        fs::write(dir.join(RECENT_PROJECTS), b"{").unwrap();
        assert_eq!(Recent::read(&dir), Recent::default());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_same_project_is_listed_once_however_its_path_is_written() {
        let dir = tempfile_dir("recent-same");
        let project = dir.join("Book.lethora");
        fs::create_dir_all(&project).unwrap();
        let mut recent = Recent::default();
        recent.add(&project);
        recent.add(&dir.join(".").join("Book.lethora"));
        assert_eq!(recent.paths.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn review_list_names_this_user_and_follows_review_undo() {
        let dir = tempfile_dir("review");
        let mut doc = Document::create(&dir, &test_identity()).unwrap();
        doc.apply_changes(&[TextChange {
            from: 0,
            to: 0,
            insert: "Hello world".into(),
        }])
        .unwrap();
        let empty = review_list_of(&doc);
        assert!(empty.items.is_empty() && !empty.undo && !empty.redo);

        let id = doc.add_comment(0, 5, "Nice").unwrap();
        let list = review_list_of(&doc);
        assert_eq!(list.me, test_identity().user);
        assert_eq!(list.items.len(), 1);
        assert_eq!(list.items[0].id, id);
        let author = list.items[0].author.as_ref().unwrap();
        assert_eq!(author.user, list.me);
        assert!(list.undo && !list.redo);
        let json = serde_json::to_value(&list).unwrap();
        assert_eq!(json["me"], json!(test_identity().user.as_str()));
        assert_eq!(json["items"][0]["kind"], json!("comment"));
        assert_eq!(json["items"][0]["messages"][0]["text"], json!("Nice"));

        assert!(doc.review_undo().unwrap());
        let undone = review_list_of(&doc);
        assert!(undone.items.is_empty() && !undone.undo && undone.redo);
        drop(doc);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn review_errors_read_well() {
        assert_eq!(
            message(Error::NotAuthor("edit this message".into())),
            "Only its author can edit this message."
        );
        assert_eq!(
            message(Error::ReadOnly("Reader".into())),
            "This project is read only for you (Reader)."
        );
    }

    #[test]
    fn suggestions_split_on_request_and_accept_replies_like_undo() {
        let dir = tempfile_dir("suggest");
        let mut doc = Document::create(&dir, &test_identity()).unwrap();
        doc.apply_changes(&[TextChange {
            from: 0,
            to: 0,
            insert: "Hello world".into(),
        }])
        .unwrap();
        // Two places within the merge interval: one review undo step unless split.
        suggest(&mut doc, 0, 0, "Oh, ", false).unwrap();
        let id = suggest(&mut doc, 6, 11, "there", true).unwrap().unwrap();
        assert!(doc.review_undo().unwrap());
        assert_eq!(doc.review_items().len(), 1);
        assert!(doc.review_redo().unwrap());

        let reply = accept(&mut doc, id).unwrap();
        assert_eq!(doc.text(), "Hello there");
        assert_eq!(reply.len16, 11);
        assert_eq!(reply.changes.len(), 1);
        let json = serde_json::to_value(&reply).unwrap();
        assert_eq!(json["changes"][0]["insert"], json!("there"));
        assert!(json.get("assets").is_none());
        // Accepted once; the accept is a `main` undo step.
        assert!(accept(&mut doc, id).is_err());
        assert!(doc.undo().unwrap().is_some());
        assert_eq!(doc.text(), "Hello world");
        drop(doc);
        let _ = fs::remove_dir_all(&dir);
    }

    fn tempfile_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lethora-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// The compile snapshot taken under the document lock, at book size. Run
    /// with `cargo test --release -p lethora-app -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn snapshot_at_book_size() {
        let dir = std::env::temp_dir().join(format!("lethora-snapshot-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut doc = Document::create(&dir, &test_identity()).unwrap();
        let paragraph = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(10);
        let text = format!("{paragraph}\n\n").repeat(5_000_000 / (paragraph.len() + 2));
        doc.insert(0, &text).unwrap();
        let start = std::time::Instant::now();
        let content = doc.text();
        let layout_entries = doc.map_entries(MapName::Layout);
        let meta = doc.map_entries(MapName::Meta);
        let custom = doc.custom_text();
        let locked = start.elapsed();
        let _ = (
            layout::resolve(&layout_entries),
            layout::resolve_meta(&meta),
            custom,
        );
        println!(
            "snapshot of {} chars: {locked:?} under the lock, {:?} with resolving",
            content.len(),
            start.elapsed()
        );

        // 200 overrides after 2000 scattered edits; 10 of them detached.
        let len = doc.len();
        for i in 0..2000 {
            doc.insert(i * 2477 % len, "x").unwrap();
        }
        let mut changes = BTreeMap::new();
        let step = doc.len_utf16() / 200;
        for i in 0..200 {
            let o = Override {
                id: format!("o{i}"),
                kind: Kind::ColumnBreak,
                anchor: doc.anchor_at(i * step).unwrap(),
            };
            changes.extend(o.entries().into_iter().map(|(k, v)| (k, Some(v))));
        }
        doc.map_update(MapName::Overrides, &changes).unwrap();
        for i in (0..10).rev() {
            doc.delete(i * 20 * step, 1).unwrap();
        }
        let start = std::time::Instant::now();
        let resolved = compile::resolve_overrides(&doc);
        println!(
            "resolving {} anchors ({} detached): {:?}",
            resolved.len(),
            resolved.iter().filter(|r| r.detached).count(),
            start.elapsed()
        );
        drop(doc);
        let _ = fs::remove_dir_all(&dir);
    }
}
