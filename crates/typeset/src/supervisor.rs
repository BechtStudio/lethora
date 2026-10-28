//! Runs the compile worker ([`crate::worker`]) as a child process: started on
//! first use, kept alive for incremental compiles, killed after a timeout,
//! capped in memory, and restarted after a crash. Calls block; one request is
//! in flight at a time.

use std::fmt;
use std::io::{self, BufRead, BufReader};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use lethora_convert::Written;

use crate::compile::{CompileRequest, Diagnostic, JumpResult};
use crate::overrides::OverrideStatus;
use crate::worker::{self, Request, Response};

/// Limits for the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Longest wait for an answer. Compilations get more for long texts
    /// (see [`Limits::compile_timeout`]).
    pub timeout: Duration,
    /// The worker's memory cap (Windows: committed memory; Linux: address
    /// space; macOS: none).
    pub memory_mb: u64,
}

impl Default for Limits {
    /// 60 s; 60% of the physical memory, at least 2 GB. (A 5M-character
    /// book peaks at ~6 GB.)
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(60),
            memory_mb: (physical_memory_mb().unwrap_or(8192) * 3 / 5).max(2048),
        }
    }
}

/// The machine's physical memory in MB, if known.
fn physical_memory_mb() -> Option<u64> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        // SAFETY: a zeroed struct with its size set, as the API requires.
        unsafe {
            let mut status: MEMORYSTATUSEX = std::mem::zeroed();
            status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
            (GlobalMemoryStatusEx(&mut status) != 0).then_some(status.ullTotalPhys >> 20)
        }
    }
    #[cfg(target_os = "linux")]
    {
        // SAFETY: sysconf has no preconditions.
        let (pages, size) = unsafe {
            (
                libc::sysconf(libc::_SC_PHYS_PAGES),
                libc::sysconf(libc::_SC_PAGESIZE),
            )
        };
        (pages > 0 && size > 0).then(|| (pages as u64).saturating_mul(size as u64) >> 20)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        None
    }
}

impl Limits {
    /// Extra time per million bytes of text, on top of `timeout`.
    const PER_MB_OF_TEXT: Duration = Duration::from_secs(20);

    /// The timeout for compiling a text of `len` bytes.
    pub fn compile_timeout(&self, len: usize) -> Duration {
        self.timeout + Self::PER_MB_OF_TEXT.mul_f64(len as f64 / 1e6)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// No answer in time; the worker was killed and restarts on next use.
    Timeout,
    /// The worker died or misbehaved (the message says how, e.g. out of
    /// memory); it restarts on next use.
    Crashed(String),
    /// The worker could not serve the request (e.g. a version that is no
    /// longer held); it keeps running.
    Failed(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Timeout => f.write_str("Typesetting took too long and was stopped."),
            Error::Crashed(m) | Error::Failed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

/// A finished compilation (see [`Response::Compiled`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Compiled {
    pub version: u64,
    /// Whether it produced pages (now held as `version`).
    pub ok: bool,
    /// The version whose pages are held (`version` if `ok`).
    pub pages_version: Option<u64>,
    pub pages: usize,
    pub page_sizes: Vec<[f64; 2]>,
    pub diagnostics: Vec<Diagnostic>,
    /// What became of each override.
    pub overrides: Vec<OverrideStatus>,
    /// Time spent compiling in the worker.
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontList {
    pub families: Vec<String>,
    /// How long the worker's system font search took.
    pub scan_ms: u64,
}

type Answer = io::Result<Option<(Response, Vec<u8>)>>;

struct Worker {
    child: Child,
    stdin: ChildStdin,
    answers: Receiver<Answer>,
    /// Disconnects when the worker's stderr has been read to the end.
    stderr_done: Receiver<()>,
    /// The worker reported a failed allocation on stderr.
    out_of_memory: Arc<AtomicBool>,
    #[cfg(windows)]
    job: job::Job,
}

/// Owns the worker process.
pub struct Supervisor {
    command: Command,
    limits: Limits,
    worker: Option<Worker>,
    held: Option<u64>,
}

impl Supervisor {
    /// A supervisor for the worker started by `command` (e.g. the app with
    /// `--compile-worker`). Nothing starts until the first request.
    pub fn new(mut command: Command, limits: Limits) -> Self {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        // Not on macOS: it refuses RLIMIT_AS below the process's (huge)
        // reserved address space (EINVAL), so the worker runs uncapped there.
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            let bytes = limits.memory_mb.saturating_mul(1024 * 1024) as libc::rlim_t;
            // SAFETY: only an async-signal-safe libc call between fork and exec.
            unsafe {
                command.pre_exec(move || {
                    let limit = libc::rlimit {
                        rlim_cur: bytes,
                        rlim_max: bytes,
                    };
                    if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        Self {
            command,
            limits,
            worker: None,
            held: None,
        }
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// The version whose pages the worker holds, if any.
    pub fn held_version(&self) -> Option<u64> {
        self.held
    }

    /// Compiles `request` as `version`.
    pub fn compile(&mut self, version: u64, request: CompileRequest) -> Result<Compiled, Error> {
        let timeout = self.limits.compile_timeout(request.content.len());
        let request = Request::Compile {
            version,
            request: Box::new(request),
        };
        match self.request(&request, timeout)? {
            (
                Response::Compiled {
                    version,
                    ok,
                    pages_version,
                    pages,
                    page_sizes,
                    diagnostics,
                    overrides,
                    duration_ms,
                },
                _,
            ) => {
                self.held = pages_version;
                Ok(Compiled {
                    version,
                    ok,
                    pages_version,
                    pages,
                    page_sizes,
                    diagnostics,
                    overrides,
                    duration_ms,
                })
            }
            (other, _) => Err(self.unexpected(&other)),
        }
    }

    /// Page `index` of the held `version` as SVG.
    pub fn render_page(&mut self, version: u64, index: usize) -> Result<Vec<u8>, Error> {
        match self.request(&Request::RenderPage { version, index }, self.limits.timeout)? {
            (Response::Page { .. }, svg) => Ok(svg),
            (other, _) => Err(self.unexpected(&other)),
        }
    }

    /// The held `version` as PDF.
    pub fn export_pdf(&mut self, version: u64) -> Result<Vec<u8>, Error> {
        let timeout = self.limits.timeout * 2;
        match self.request(&Request::ExportPdf { version }, timeout)? {
            (Response::Pdf { .. }, pdf) => Ok(pdf),
            (other, _) => Err(self.unexpected(&other)),
        }
    }

    /// The held `version` in the convert format `format` (a registry id),
    /// its side files named after `stem` (see [`Request::Export`]).
    pub fn export(&mut self, version: u64, format: &str, stem: &str) -> Result<Written, Error> {
        // Compiled again (for HTML), then written: more than a PDF.
        let timeout = self.limits.timeout * 3;
        let request = Request::Export {
            version,
            format: format.to_owned(),
            stem: stem.to_owned(),
        };
        match self.request(&request, timeout)? {
            (
                Response::Exported {
                    warnings, files, ..
                },
                payload,
            ) => match worker::written(warnings, files, payload) {
                Some(written) => Ok(written),
                None => Err(self.unexpected(&Response::Ok)),
            },
            (other, _) => Err(self.unexpected(&other)),
        }
    }

    /// Where a click at (`x_pt`, `y_pt`) on page `page` (0-based) of the
    /// held `version` leads.
    pub fn jump(
        &mut self,
        version: u64,
        page: usize,
        x_pt: f64,
        y_pt: f64,
    ) -> Result<JumpResult, Error> {
        let request = Request::Jump {
            version,
            page,
            x_pt,
            y_pt,
        };
        match self.request(&request, self.limits.timeout)? {
            (Response::Jump { jump, .. }, _) => Ok(jump),
            (other, _) => Err(self.unexpected(&other)),
        }
    }

    /// The text font families: embedded and the system's.
    pub fn fonts(&mut self) -> Result<FontList, Error> {
        match self.request(&Request::Fonts, self.limits.timeout)? {
            (Response::Fonts { families, scan_ms }, _) => Ok(FontList { families, scan_ms }),
            (other, _) => Err(self.unexpected(&other)),
        }
    }

    /// Asks the worker to exit (killing it if it doesn't answer quickly).
    pub fn shutdown(&mut self) {
        if self.worker.is_some() {
            let _ = self.request(&Request::Shutdown, Duration::from_secs(2));
        }
        self.stop();
    }

    /// The worker's peak memory use so far, in MB (Windows; Linux), if a
    /// worker is running.
    pub fn peak_memory_mb(&self) -> Option<u64> {
        let worker = self.worker.as_ref()?;
        #[cfg(windows)]
        {
            worker.job.peak_bytes().map(|b| b / (1024 * 1024))
        }
        #[cfg(target_os = "linux")]
        {
            let status = std::fs::read_to_string(format!("/proc/{}/status", worker.child.id()));
            status.ok()?.lines().find_map(|l| {
                let kb: u64 = l
                    .strip_prefix("VmHWM:")?
                    .trim()
                    .strip_suffix("kB")?
                    .trim()
                    .parse()
                    .ok()?;
                Some(kb / 1024)
            })
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            let _ = worker;
            None
        }
    }

    #[cfg(debug_assertions)]
    pub fn test_sleep(&mut self, ms: u64) -> Result<(), Error> {
        self.request(&Request::TestSleep { ms }, self.limits.timeout)
            .map(drop)
    }

    #[cfg(debug_assertions)]
    pub fn test_panic(&mut self) -> Result<(), Error> {
        self.request(&Request::TestPanic, self.limits.timeout)
            .map(drop)
    }

    #[cfg(debug_assertions)]
    pub fn test_alloc(&mut self, mb: u64) -> Result<(), Error> {
        self.request(&Request::TestAlloc { mb }, self.limits.timeout)
            .map(drop)
    }

    /// Sends `request` and waits up to `timeout` for the answer.
    fn request(
        &mut self,
        request: &Request,
        timeout: Duration,
    ) -> Result<(Response, Vec<u8>), Error> {
        if self.worker.is_none() {
            self.held = None;
            self.worker = Some(
                self.spawn()
                    .map_err(|e| Error::Crashed(format!("Could not start the typesetter: {e}")))?,
            );
        }
        let worker = self.worker.as_mut().expect("just started");
        if worker::write_message(&mut worker.stdin, request, None).is_err() {
            return Err(self.crashed());
        }
        match worker.answers.recv_timeout(timeout) {
            Ok(Ok(Some((Response::Error { message }, _)))) => Err(Error::Failed(message)),
            Ok(Ok(Some(answer))) => Ok(answer),
            Ok(Ok(None) | Err(_)) | Err(RecvTimeoutError::Disconnected) => Err(self.crashed()),
            Err(RecvTimeoutError::Timeout) => {
                self.stop();
                Err(Error::Timeout)
            }
        }
    }

    /// A well-formed answer of the wrong kind: the stream can't be trusted.
    fn unexpected(&mut self, response: &Response) -> Error {
        self.stop();
        Error::Crashed(format!(
            "The typesetter answered unexpectedly ({response:?})."
        ))
    }

    fn spawn(&mut self) -> io::Result<Worker> {
        let mut child = self.command.spawn()?;
        #[cfg(windows)]
        let job = match job::Job::new(self.limits.memory_mb).and_then(|job| {
            job.assign(&child)?;
            Ok(job)
        }) {
            Ok(job) => job,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");

        let (tx, answers) = mpsc::channel();
        thread::Builder::new()
            .name("compile-worker-out".into())
            .spawn(move || {
                let mut stdout = BufReader::with_capacity(1 << 16, stdout);
                loop {
                    let answer = worker::read_response(&mut stdout);
                    let end = !matches!(answer, Ok(Some(_)));
                    if tx.send(answer).is_err() || end {
                        break;
                    }
                }
            })?;

        // The worker's log goes to ours; a failed allocation is noted.
        let out_of_memory = Arc::new(AtomicBool::new(false));
        let oom = out_of_memory.clone();
        let (done, stderr_done) = mpsc::channel::<()>();
        thread::Builder::new()
            .name("compile-worker-err".into())
            .spawn(move || {
                let _done = done;
                let mut stderr = BufReader::new(stderr);
                let mut line = Vec::new();
                while matches!(stderr.read_until(b'\n', &mut line), Ok(n) if n > 0) {
                    let text = String::from_utf8_lossy(&line);
                    if text.contains("memory allocation of") {
                        oom.store(true, Ordering::Relaxed);
                    }
                    eprint!("{text}");
                    line.clear();
                }
            })?;

        Ok(Worker {
            child,
            stdin,
            answers,
            stderr_done,
            out_of_memory,
            #[cfg(windows)]
            job,
        })
    }

    /// Ends the worker after it died or broke the protocol; says why.
    fn crashed(&mut self) -> Error {
        let Some(mut worker) = self.worker.take() else {
            return Error::Crashed("The typesetter is not running.".into());
        };
        self.held = None;
        #[cfg(windows)]
        let near_cap = worker
            .job
            .peak_bytes()
            .is_some_and(|b| b / (1024 * 1024) * 100 >= self.limits.memory_mb * 95);
        #[cfg(not(windows))]
        let near_cap = false;
        // It may still be running (e.g. it wrote garbage).
        let _ = worker.child.kill();
        let status = worker.child.wait();
        // Its last words may say why (bounded: another process could hold
        // the pipe open).
        let _ = worker.stderr_done.recv_timeout(Duration::from_secs(1));
        if near_cap || worker.out_of_memory.load(Ordering::Relaxed) {
            Error::Crashed(format!(
                "Typesetting ran out of memory (the limit is {} MB).",
                self.limits.memory_mb
            ))
        } else {
            let how = status.map_or_else(|e| e.to_string(), |s| s.to_string());
            Error::Crashed(format!("The typesetter stopped unexpectedly ({how})."))
        }
    }

    /// Kills the worker, if any.
    fn stop(&mut self) {
        self.held = None;
        if let Some(mut worker) = self.worker.take() {
            let _ = worker.child.kill();
            let _ = worker.child.wait();
        }
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A Windows job object holding the worker: caps its memory, accounts its
/// peak use, and kills it when the handle closes (also when the app dies).
#[cfg(windows)]
mod job {
    use std::io;
    use std::mem::{size_of, zeroed};
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        QueryInformationJobObject, SetInformationJobObject,
    };

    pub struct Job(HANDLE);

    // SAFETY: a kernel handle, usable from any thread.
    unsafe impl Send for Job {}

    impl Job {
        pub fn new(memory_mb: u64) -> io::Result<Self> {
            // SAFETY: plain Win32 calls with valid arguments; the handle is
            // owned by the returned value.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let job = Job(handle);
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY
                    | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                    | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
                info.ProcessMemoryLimit =
                    usize::try_from(memory_mb.saturating_mul(1024 * 1024)).unwrap_or(usize::MAX);
                let ok = SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&raw const info).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(job)
            }
        }

        pub fn assign(&self, child: &Child) -> io::Result<()> {
            // SAFETY: both handles are valid for the duration of the call.
            let ok = unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle()) };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        /// The highest committed memory of any process in the job.
        pub fn peak_bytes(&self) -> Option<u64> {
            // SAFETY: the buffer is the size passed.
            unsafe {
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
                let ok = QueryInformationJobObject(
                    self.0,
                    JobObjectExtendedLimitInformation,
                    (&raw mut info).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                    std::ptr::null_mut(),
                );
                (ok != 0).then_some(info.PeakProcessMemoryUsed as u64)
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: owned handle, closed once. Kills the worker if alive.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}
