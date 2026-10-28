//! Whole-document compilation for the publisher view: a thread owning the
//! compile worker ([`Supervisor`]), compiling the latest document version
//! while watching (debounced), rendering pages and exporting (PDF and the
//! convert formats) on request.
//! Results are reported as `compile-status` events.

use std::collections::HashMap;
use std::process::Command;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use lethora_convert::Written;
use lethora_core::{Document, MapName};
use lethora_typeset::layout;
use lethora_typeset::overrides::{self, OverrideStatus, ResolvedOverride};
use lethora_typeset::{
    CompileRequest, Date, Diagnostic, JumpResult, Limits, Supervisor, WorkerError,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::AppState;

/// The command-line flag that makes the app binary a compile worker.
pub const WORKER_FLAG: &str = "--compile-worker";
/// Quiet time after the last change before compiling.
const DEBOUNCE: Duration = Duration::from_millis(400);
/// Longest wait while changes keep coming.
const MAX_DELAY: Duration = Duration::from_secs(2);
/// How often the version is checked while watching.
const POLL: Duration = Duration::from_millis(100);

type Reply<T> = Sender<Result<T, String>>;

enum Job {
    Watch(bool),
    Now,
    Page {
        version: u64,
        index: usize,
        reply: Reply<Vec<u8>>,
    },
    Pdf {
        reply: Reply<Vec<u8>>,
    },
    Export {
        format: String,
        stem: String,
        reply: Reply<Written>,
    },
    Jump {
        version: u64,
        page: usize,
        x_pt: f64,
        y_pt: f64,
        reply: Reply<JumpResult>,
    },
    Fonts {
        reply: Reply<Vec<String>>,
    },
}

/// The handle commands use to reach the compile thread.
pub struct CompileState {
    jobs: Sender<Job>,
    /// Font families (embedded + system), fetched once from the worker.
    fonts: Arc<OnceLock<Vec<String>>>,
    /// Whether each override was applied in the last compilation.
    applied: Applied,
}

type Applied = Arc<Mutex<HashMap<String, bool>>>;

/// What a `compile-status` event carries.
#[derive(Debug, Clone, Serialize)]
struct CompileStatus {
    /// `compiling`, `done`, `failed` (errors; the previous pages stay),
    /// `timeout` or `crashed` (no pages until the next compile).
    state: &'static str,
    /// The document version compiled; `diagnostics` refer to its text.
    version: u64,
    /// The version whose pages `page_svg` serves, if any.
    pages_version: Option<u64>,
    pages: usize,
    page_sizes: Vec<[f64; 2]>,
    diagnostics: Vec<Diagnostic>,
    /// What became of each override in `version`.
    overrides: Vec<OverrideStatus>,
    duration_ms: u64,
    message: Option<String>,
}

impl CompileState {
    /// Starts the compile thread (the worker process starts on first use).
    pub fn start(app: AppHandle) -> std::io::Result<Self> {
        let (jobs, rx) = mpsc::channel();
        let fonts = Arc::new(OnceLock::new());
        let cache = fonts.clone();
        let applied = Applied::default();
        let shared = applied.clone();
        thread::Builder::new()
            .name("compile".into())
            .spawn(move || Compiler::new(app, cache, shared).run(rx))?;
        Ok(Self {
            jobs,
            fonts,
            applied,
        })
    }

    /// Whether each override was applied in the last compilation (overrides
    /// added since are missing).
    pub fn applied(&self) -> HashMap<String, bool> {
        self.applied
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Where a click on page `page` (0-based) of compiled version `version`
    /// leads.
    pub async fn jump(
        &self,
        version: u64,
        page: usize,
        x_pt: f64,
        y_pt: f64,
    ) -> Result<JumpResult, String> {
        self.ask(|reply| Job::Jump {
            version,
            page,
            x_pt,
            y_pt,
            reply,
        })
        .await
    }

    pub fn watch(&self, active: bool) {
        let _ = self.jobs.send(Job::Watch(active));
    }

    pub fn now(&self) {
        let _ = self.jobs.send(Job::Now);
    }

    pub async fn page_svg(&self, version: u64, index: usize) -> Result<Vec<u8>, String> {
        self.ask(|reply| Job::Page {
            version,
            index,
            reply,
        })
        .await
    }

    /// The latest version as PDF (compiled first if needed).
    pub async fn pdf(&self) -> Result<Vec<u8>, String> {
        self.ask(|reply| Job::Pdf { reply }).await
    }

    /// The latest version in the convert format `format` (compiled first if
    /// needed); `stem` names its side files.
    pub async fn export(&self, format: String, stem: String) -> Result<Written, String> {
        self.ask(|reply| Job::Export {
            format,
            stem,
            reply,
        })
        .await
    }

    /// The font families documents can use (embedded only if the worker
    /// can't answer).
    pub async fn fonts(&self) -> Vec<String> {
        if let Some(fonts) = self.fonts.get() {
            return fonts.clone();
        }
        match self.ask(|reply| Job::Fonts { reply }).await {
            Ok(fonts) => fonts,
            Err(e) => {
                eprintln!("Listing fonts failed: {e}");
                layout::fonts()
            }
        }
    }

    async fn ask<T: Send + 'static>(&self, job: impl FnOnce(Reply<T>) -> Job) -> Result<T, String> {
        let (reply, answer) = mpsc::channel();
        self.jobs
            .send(job(reply))
            .map_err(|_| "The typesetter is not running.".to_owned())?;
        tauri::async_runtime::spawn_blocking(move || answer.recv())
            .await
            .map_err(|e| e.to_string())?
            .map_err(|_| "The typesetter stopped.".to_owned())?
    }
}

struct Compiler {
    app: AppHandle,
    supervisor: Supervisor,
    fonts: Arc<OnceLock<Vec<String>>>,
    applied: Applied,
    watching: bool,
    /// The last version seen, and when changes started and last came.
    seen: u64,
    first_change: Option<Instant>,
    last_change: Instant,
    /// The last version compiled (successfully or not) and its status.
    attempted: Option<u64>,
    last_status: Option<CompileStatus>,
}

impl Compiler {
    fn new(app: AppHandle, fonts: Arc<OnceLock<Vec<String>>>, applied: Applied) -> Self {
        let exe = std::env::current_exe().unwrap_or_default();
        let mut command = Command::new(exe);
        command.arg(WORKER_FLAG);
        Self {
            app,
            supervisor: Supervisor::new(command, Limits::default()),
            fonts,
            applied,
            watching: false,
            seen: 0,
            first_change: None,
            last_change: Instant::now(),
            attempted: None,
            last_status: None,
        }
    }

    fn version(&self) -> u64 {
        self.app.state::<AppState>().version()
    }

    fn run(mut self, jobs: Receiver<Job>) {
        loop {
            let job = if self.watching {
                match jobs.recv_timeout(self.wait()) {
                    Ok(job) => Some(job),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            } else {
                match jobs.recv() {
                    Ok(job) => Some(job),
                    Err(_) => return,
                }
            };
            if let Some(job) = job {
                self.handle(job);
            }
            if self.watching {
                self.watch_tick();
            }
        }
    }

    /// How long to wait for a job before checking for changes again.
    fn wait(&self) -> Duration {
        match self.first_change {
            Some(first) => {
                let due = (self.last_change + DEBOUNCE).min(first + MAX_DELAY);
                due.saturating_duration_since(Instant::now()).min(POLL)
            }
            None => POLL,
        }
    }

    /// Notes new changes and compiles once they have settled.
    fn watch_tick(&mut self) {
        let now = Instant::now();
        let version = self.version();
        if version != self.seen {
            self.seen = version;
            self.last_change = now;
            self.first_change.get_or_insert(now);
        }
        let Some(first) = self.first_change else {
            return;
        };
        if now >= (self.last_change + DEBOUNCE).min(first + MAX_DELAY) {
            self.first_change = None;
            if self.attempted != Some(version) {
                self.compile_latest();
            }
        }
    }

    fn handle(&mut self, job: Job) {
        match job {
            Job::Watch(active) => {
                self.watching = active;
                let version = self.version();
                if active && self.attempted != Some(version) {
                    // Compile right away.
                    let now = Instant::now();
                    let long_ago = now.checked_sub(MAX_DELAY).unwrap_or(now);
                    self.seen = version;
                    self.first_change = Some(long_ago);
                    self.last_change = long_ago;
                }
            }
            Job::Now => {
                if self.attempted == Some(self.version())
                    && self.supervisor.held_version().is_some()
                {
                    if let Some(status) = self.last_status.clone() {
                        self.emit(status);
                    }
                } else {
                    self.compile_latest();
                }
            }
            Job::Page {
                version,
                index,
                reply,
            } => {
                let svg = self.supervisor.render_page(version, index);
                let _ = reply.send(svg.map_err(|e| e.to_string()));
            }
            Job::Pdf { reply } => {
                let pdf = self.typeset().and_then(|version| {
                    self.supervisor
                        .export_pdf(version)
                        .map_err(|e| e.to_string())
                });
                let _ = reply.send(pdf);
            }
            Job::Export {
                format,
                stem,
                reply,
            } => {
                let written = self.typeset().and_then(|version| {
                    self.supervisor
                        .export(version, &format, &stem)
                        .map_err(|e| e.to_string())
                });
                let _ = reply.send(written);
            }
            Job::Jump {
                version,
                page,
                x_pt,
                y_pt,
                reply,
            } => {
                let jump = self.supervisor.jump(version, page, x_pt, y_pt);
                let _ = reply.send(jump.map_err(|e| e.to_string()));
            }
            Job::Fonts { reply } => {
                let fonts = match self.fonts.get() {
                    Some(fonts) => Ok(fonts.clone()),
                    None => self.supervisor.fonts().map(|list| {
                        eprintln!("System font search took {} ms.", list.scan_ms);
                        self.fonts.get_or_init(|| list.families).clone()
                    }),
                };
                let _ = reply.send(fonts.map_err(|e| e.to_string()));
            }
        }
    }

    /// The latest version, compiled first if the worker doesn't hold it;
    /// why not if it can't be typeset.
    fn typeset(&mut self) -> Result<u64, String> {
        let version = self.version();
        if self.supervisor.held_version() != Some(version) {
            self.compile_latest();
        }
        if self.supervisor.held_version() != Some(version) {
            let status = self.last_status.as_ref();
            let reason = status.and_then(|s| {
                s.message.clone().or_else(|| {
                    s.diagnostics
                        .iter()
                        .find(|d| d.severity == lethora_typeset::Severity::Error)
                        .map(|d| d.message.clone())
                })
            });
            return Err(format!(
                "The document could not be typeset{}",
                reason.map_or(".".into(), |r| format!(": {r}"))
            ));
        }
        Ok(version)
    }

    /// Snapshots the open document and compiles it, reporting the result.
    fn compile_latest(&mut self) {
        let Some((version, request)) = snapshot(&self.app) else {
            self.attempted = Some(self.version());
            return;
        };
        self.attempted = Some(version);
        self.emit(CompileStatus {
            state: "compiling",
            version,
            pages_version: self.supervisor.held_version(),
            pages: 0,
            page_sizes: Vec::new(),
            diagnostics: Vec::new(),
            overrides: Vec::new(),
            duration_ms: 0,
            message: None,
        });
        let status = match self.supervisor.compile(version, request) {
            Ok(c) => {
                *self.applied.lock().unwrap_or_else(|e| e.into_inner()) = c
                    .overrides
                    .iter()
                    .map(|s| (s.id.clone(), s.applied))
                    .collect();
                CompileStatus {
                    state: if c.ok { "done" } else { "failed" },
                    version,
                    pages_version: c.pages_version,
                    pages: c.pages,
                    page_sizes: c.page_sizes,
                    diagnostics: c.diagnostics,
                    overrides: c.overrides,
                    duration_ms: c.duration_ms,
                    message: None,
                }
            }
            Err(e) => CompileStatus {
                state: match e {
                    WorkerError::Timeout => "timeout",
                    WorkerError::Crashed(_) | WorkerError::Failed(_) => "crashed",
                },
                version,
                pages_version: None,
                pages: 0,
                page_sizes: Vec::new(),
                diagnostics: Vec::new(),
                overrides: Vec::new(),
                duration_ms: 0,
                message: Some(e.to_string()),
            },
        };
        self.last_status = Some(status.clone());
        self.emit(status);
    }

    fn emit(&self, status: CompileStatus) {
        let _ = self.app.emit("compile-status", status);
    }
}

/// The open document's version and what to compile, read under the
/// document lock (briefly); `None` without a project.
fn snapshot(app: &AppHandle) -> Option<(u64, CompileRequest)> {
    let state = app.state::<AppState>();
    let (version, project_dir, content, layout, meta, custom, overrides, bibliography, assets) = {
        let guard = state.lock();
        let project = guard.as_ref()?;
        let doc = &project.doc;
        (
            state.version(),
            project.path.clone(),
            doc.text(),
            doc.map_entries(MapName::Layout),
            doc.map_entries(MapName::Meta),
            doc.custom_text(),
            resolve_overrides(doc),
            doc.bibliography(),
            doc.assets(),
        )
    };
    Some((
        version,
        CompileRequest {
            project_dir,
            layout: layout::resolve(&layout),
            meta: layout::resolve_meta(&meta),
            custom,
            content,
            today: Some(today()),
            overrides,
            bibliography,
            assets,
        },
    ))
}

/// The document's valid overrides with their anchors resolved; invalid or
/// unresolvable ones are left out (`overrides_get` lists them).
pub fn resolve_overrides(doc: &Document) -> Vec<ResolvedOverride> {
    let (valid, _) = overrides::parse(&doc.map_entries(MapName::Overrides));
    valid
        .into_iter()
        .filter_map(|o| {
            let pos = doc.resolve_anchor(&o.anchor).ok()?;
            Some(ResolvedOverride {
                id: o.id,
                kind: o.kind,
                content_byte_offset: pos.pos_byte,
                detached: pos.detached,
            })
        })
        .collect()
}

/// Today's date (UTC).
fn today() -> Date {
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    civil_date(secs / 86_400)
}

/// The calendar date `days` after 1970-01-01 (H. Hinnant's algorithm).
fn civil_date(days: u64) -> Date {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = (yoe + era * 400 + i64::from(month <= 2)) as i32;
    Date { year, month, day }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        let d = |y, m, day| Date {
            year: y,
            month: m,
            day,
        };
        assert_eq!(civil_date(0), d(1970, 1, 1));
        assert_eq!(civil_date(11_016), d(2000, 2, 29));
        assert_eq!(civil_date(20_738), d(2026, 10, 12));
        assert_eq!(civil_date(20_513), d(2026, 3, 1));
    }
}
