//! The compile worker: a process that compiles whole documents, renders
//! their pages and exports them (PDF, and the convert formats through
//! [`crate::export`]), so heavy or hostile Typst code, or a writer's bug,
//! can be stopped (timeout, memory cap) without taking the app down. See
//! [`crate::Supervisor`] for the other side.
//!
//! # Protocol
//!
//! Requests and responses are frames on stdin/stdout: `[u32 LE length][JSON]`.
//! The responses [`Response::Page`] (SVG), [`Response::Pdf`] and
//! [`Response::Exported`] are followed by one more frame with the bytes.
//! Nothing else is written to stdout; logs go to stderr. One request at a
//! time; every request gets one response.

use std::io::{self, BufReader, BufWriter, Read, Write};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use lethora_convert::{Warning, Written};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::compile::{CompileRequest, CompiledDocument, Diagnostic, DocCompiler, JumpResult};
use crate::math::TYPESET_STACK;
use crate::overrides::OverrideStatus;

/// Largest frame accepted (a garbled length fails instead of allocating).
pub const MAX_FRAME: usize = 1 << 30;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Compile `request` as document version `version`.
    Compile {
        version: u64,
        request: Box<CompileRequest>,
    },
    /// Page `index` (0-based) of the held version as SVG.
    RenderPage { version: u64, index: usize },
    /// The held version as PDF.
    ExportPdf { version: u64 },
    /// The held version in the convert format `format` (a registry id);
    /// `stem` names its side files (the output file's name without its
    /// extension).
    Export {
        version: u64,
        format: String,
        stem: String,
    },
    /// Where a click at (`x_pt`, `y_pt`) on page `page` (0-based) of the
    /// held version leads.
    Jump {
        version: u64,
        page: usize,
        x_pt: f64,
        y_pt: f64,
    },
    /// The text font families (embedded + system).
    Fonts,
    /// Answers [`Response::Ok`] and exits.
    Shutdown,
    /// Test hooks.
    #[cfg(debug_assertions)]
    TestSleep { ms: u64 },
    #[cfg(debug_assertions)]
    TestPanic,
    #[cfg(debug_assertions)]
    TestAlloc { mb: u64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// A compilation finished. `ok`: it produced pages, now held as
    /// `version`. Otherwise the previously held version (`pages_version`,
    /// if any) is kept; `pages` and `page_sizes` describe the held pages,
    /// `diagnostics` the requested version.
    Compiled {
        version: u64,
        ok: bool,
        pages_version: Option<u64>,
        pages: usize,
        page_sizes: Vec<[f64; 2]>,
        diagnostics: Vec<Diagnostic>,
        /// What became of each override in the requested version.
        #[serde(default)]
        overrides: Vec<OverrideStatus>,
        duration_ms: u64,
    },
    /// Followed by a frame with the page's SVG.
    Page {
        version: u64,
        index: usize,
    },
    /// Followed by a frame with the PDF.
    Pdf {
        version: u64,
    },
    /// Followed by a frame with the file, then the side files' bytes in
    /// the order of `files` (path, length), all in one.
    Exported {
        version: u64,
        warnings: Vec<Warning>,
        #[serde(default)]
        files: Vec<(String, usize)>,
    },
    Jump {
        version: u64,
        jump: JumpResult,
    },
    Fonts {
        families: Vec<String>,
        /// How long the system font search took (once per worker).
        scan_ms: u64,
    },
    Ok,
    /// The request could not be served (e.g. a version no longer held).
    Error {
        message: String,
    },
}

impl Response {
    /// Whether a frame with bytes follows.
    pub fn has_payload(&self) -> bool {
        matches!(
            self,
            Response::Page { .. } | Response::Pdf { .. } | Response::Exported { .. }
        )
    }
}

/// An export as a response and its payload (see [`Response::Exported`]).
pub fn exported(version: u64, written: Written) -> (Response, Vec<u8>) {
    let mut payload = written.bytes;
    let files = written
        .files
        .into_iter()
        .map(|(path, bytes)| {
            payload.extend_from_slice(&bytes);
            (path, bytes.len())
        })
        .collect();
    let response = Response::Exported {
        version,
        warnings: written.warnings,
        files,
    };
    (response, payload)
}

/// The export a [`Response::Exported`] and its payload carry; `None` if
/// the lengths don't add up.
pub fn written(
    warnings: Vec<Warning>,
    files: Vec<(String, usize)>,
    mut payload: Vec<u8>,
) -> Option<Written> {
    let side = files
        .iter()
        .try_fold(0usize, |sum, (_, len)| sum.checked_add(*len))?;
    let mut at = payload.len().checked_sub(side)?;
    let mut out = Vec::with_capacity(files.len());
    for (path, len) in files {
        out.push((path, payload[at..at + len].to_vec()));
        at += len;
    }
    payload.truncate(payload.len() - side);
    Some(Written {
        bytes: payload,
        files: out,
        warnings,
    })
}

// ---- Framing ----

/// Writes one frame.
pub fn write_frame(w: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    let len = u32::try_from(bytes.len())
        .ok()
        .filter(|&n| n as usize <= MAX_FRAME)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(bytes)
}

/// Reads one frame; `None` on end of input before a frame starts.
pub fn read_frame(r: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match r.read(&mut len[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    // Grows as data arrives: a garbled length can't allocate up front.
    let mut bytes = Vec::new();
    r.take(len as u64).read_to_end(&mut bytes)?;
    if bytes.len() < len {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    Ok(Some(bytes))
}

/// Writes `message` as JSON, then `payload` (if any), and flushes.
pub fn write_message<T: Serialize>(
    w: &mut impl Write,
    message: &T,
    payload: Option<&[u8]>,
) -> io::Result<()> {
    let json = serde_json::to_vec(message).map_err(io::Error::other)?;
    write_frame(w, &json)?;
    if let Some(payload) = payload {
        write_frame(w, payload)?;
    }
    w.flush()
}

fn parse<T: DeserializeOwned>(bytes: &[u8]) -> io::Result<T> {
    serde_json::from_slice(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Reads one request; `None` at the end of input. A frame that isn't a
/// request is `InvalidData` (the stream stays in sync).
pub fn read_request(r: &mut impl Read) -> io::Result<Option<Request>> {
    read_frame(r)?.map(|bytes| parse(&bytes)).transpose()
}

/// Reads one response and its payload; `None` at the end of input.
pub fn read_response(r: &mut impl Read) -> io::Result<Option<(Response, Vec<u8>)>> {
    let Some(bytes) = read_frame(r)? else {
        return Ok(None);
    };
    let response: Response = parse(&bytes)?;
    let payload = if response.has_payload() {
        read_frame(r)?.ok_or(io::ErrorKind::UnexpectedEof)?
    } else {
        Vec::new()
    };
    Ok(Some((response, payload)))
}

// ---- The worker loop ----

/// After a compile, Typst's cache is trimmed once no request has arrived for
/// this long, so the page renders that follow a compile aren't delayed by it.
const IDLE_EVICT: Duration = Duration::from_millis(250);

type Incoming = io::Result<Option<Request>>;

/// Serves requests from `input` until it ends or [`Request::Shutdown`].
/// Runs on a thread with a large stack (Typst recurses deeply). Requests are
/// read on a separate thread (detached: the process exits when this returns),
/// so the worker can tell when it is idle.
pub fn run<R, W>(input: R, output: W) -> io::Result<()>
where
    R: Read + Send + 'static,
    W: Write + Send,
{
    let (tx, rx) = mpsc::sync_channel::<Incoming>(16);
    thread::Builder::new()
        .name("worker-input".into())
        .spawn(move || {
            let mut input = BufReader::with_capacity(1 << 16, input);
            loop {
                let next = read_request(&mut input);
                let more = match &next {
                    Ok(Some(_)) => true,
                    Ok(None) => false,
                    Err(e) => e.kind() == io::ErrorKind::InvalidData,
                };
                if tx.send(next).is_err() || !more {
                    break;
                }
            }
        })?;
    thread::scope(|scope| {
        thread::Builder::new()
            .name("compile-worker".into())
            .stack_size(TYPESET_STACK)
            .spawn_scoped(scope, || serve(rx, output))?
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("the worker panicked")))
    })
}

struct Held {
    version: u64,
    document: CompiledDocument,
    page_sizes: Vec<[f64; 2]>,
    /// What it was compiled from (exports compile it again, for HTML).
    request: Box<CompileRequest>,
}

fn serve(input: mpsc::Receiver<Incoming>, output: impl Write) -> io::Result<()> {
    let mut output = BufWriter::with_capacity(1 << 16, output);
    let mut compiler = DocCompiler::with_system_fonts();
    let mut held: Option<Held> = None;
    let mut fonts_logged = false;
    let mut evict_pending = false;
    loop {
        let next = if evict_pending {
            match input.recv_timeout(IDLE_EVICT) {
                Ok(next) => next,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    crate::compile::evict_cache();
                    evict_pending = false;
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            }
        } else {
            match input.recv() {
                Ok(next) => next,
                Err(_) => return Ok(()),
            }
        };
        let request = match next {
            Ok(Some(request)) => request,
            Ok(None) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                let message = format!("Bad request: {e}");
                write_message(&mut output, &Response::Error { message }, None)?;
                continue;
            }
            Err(e) => return Err(e),
        };
        let (response, payload) = match request {
            Request::Compile { version, request } => {
                // Never idle since the last compile: trim now, so two
                // compiles' worth of cache never pile up.
                if evict_pending {
                    crate::compile::evict_cache();
                }
                evict_pending = true;
                (compile(&mut compiler, &mut held, version, request), None)
            }
            Request::RenderPage { version, index } => match current(&held, version) {
                Ok(h) => match h.document.render_page_svg(index) {
                    Some(svg) => (Response::Page { version, index }, Some(svg.into_bytes())),
                    None => (error(format!("There is no page {}.", index + 1)), None),
                },
                Err(e) => (e, None),
            },
            Request::ExportPdf { version } => match current(&held, version) {
                Ok(h) => match h.document.export_pdf() {
                    Ok(pdf) => (Response::Pdf { version }, Some(pdf)),
                    Err(e) => (error(e), None),
                },
                Err(e) => (e, None),
            },
            Request::Export {
                version,
                format,
                stem,
            } => match current(&held, version) {
                Ok(h) => {
                    // A compile of its own: trimmed once idle.
                    evict_pending = true;
                    match compiler.export(&h.request, &format, &stem) {
                        Ok(written) => {
                            let (response, payload) = exported(version, written);
                            (response, Some(payload))
                        }
                        Err(e) => (error(e), None),
                    }
                }
                Err(e) => (e, None),
            },
            Request::Jump {
                version,
                page,
                x_pt,
                y_pt,
            } => match current(&held, version) {
                Ok(h) => (
                    Response::Jump {
                        version,
                        jump: h.document.jump(page, x_pt, y_pt),
                    },
                    None,
                ),
                Err(e) => (e, None),
            },
            Request::Fonts => {
                let (families, scan) = crate::layout::system_fonts();
                let scan_ms = scan.as_millis() as u64;
                (Response::Fonts { families, scan_ms }, None)
            }
            Request::Shutdown => {
                write_message(&mut output, &Response::Ok, None)?;
                return Ok(());
            }
            #[cfg(debug_assertions)]
            Request::TestSleep { ms } => {
                thread::sleep(std::time::Duration::from_millis(ms));
                (Response::Ok, None)
            }
            #[cfg(debug_assertions)]
            Request::TestPanic => panic!("test panic"),
            #[cfg(debug_assertions)]
            Request::TestAlloc { mb } => {
                let mut v: Vec<u8> = Vec::new();
                for _ in 0..mb {
                    // Commit and touch every page, a megabyte at a time.
                    v.extend(std::iter::repeat_n(1u8, 1 << 20));
                }
                std::hint::black_box(&v);
                (Response::Ok, None)
            }
        };
        if !fonts_logged && matches!(response, Response::Compiled { .. } | Response::Fonts { .. }) {
            fonts_logged = true;
            let (families, scan) = crate::layout::system_fonts();
            eprintln!(
                "lethora worker: {} font families; searching the system took {} ms",
                families.len(),
                scan.as_millis()
            );
        }
        write_message(&mut output, &response, payload.as_deref())?;
    }
}

fn error(message: impl Into<String>) -> Response {
    Response::Error {
        message: message.into(),
    }
}

/// The held document if it is `version`.
fn current(held: &Option<Held>, version: u64) -> Result<&Held, Response> {
    match held {
        Some(h) if h.version == version => Ok(h),
        Some(h) => Err(error(format!(
            "Version {version} is not available (version {} is).",
            h.version
        ))),
        None => Err(error(format!("Version {version} is not available."))),
    }
}

fn compile(
    compiler: &mut DocCompiler,
    held: &mut Option<Held>,
    version: u64,
    request: Box<CompileRequest>,
) -> Response {
    let start = Instant::now();
    let out = compiler.compile(&request);
    let duration_ms = start.elapsed().as_millis() as u64;
    let ok = out.document.is_some();
    if let Some(document) = out.document {
        let replaced = held.replace(Held {
            version,
            document,
            page_sizes: out.page_sizes,
            request,
        });
        // Freeing a book's pages takes ~0.1 s: not in anyone's way.
        if let Some(replaced) = replaced {
            let _ = thread::Builder::new()
                .name("free-document".into())
                .spawn(move || drop(replaced));
        }
    }
    Response::Compiled {
        version,
        ok,
        pages_version: held.as_ref().map(|h| h.version),
        pages: held.as_ref().map_or(0, |h| h.page_sizes.len()),
        page_sizes: held
            .as_ref()
            .map_or_else(Vec::new, |h| h.page_sizes.clone()),
        diagnostics: out.diagnostics,
        overrides: out.overrides,
        duration_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let mut buf = Vec::new();
        write_message(
            &mut buf,
            &Response::Page {
                version: 3,
                index: 1,
            },
            Some(b"<svg/>"),
        )
        .unwrap();
        write_message(&mut buf, &Response::Ok, None).unwrap();
        write_message(&mut buf, &Request::ExportPdf { version: 7 }, None).unwrap();
        let mut r = &buf[..];
        assert_eq!(
            read_response(&mut r).unwrap(),
            Some((
                Response::Page {
                    version: 3,
                    index: 1
                },
                b"<svg/>".to_vec()
            ))
        );
        assert_eq!(
            read_response(&mut r).unwrap(),
            Some((Response::Ok, Vec::new()))
        );
        assert!(matches!(
            read_request(&mut r).unwrap(),
            Some(Request::ExportPdf { version: 7 })
        ));
        assert!(read_request(&mut r).unwrap().is_none());
        assert!(read_frame(&mut r).unwrap().is_none());
    }

    #[test]
    fn exports_carry_their_side_files() {
        let file = Written {
            bytes: b"# Doc\n".to_vec(),
            files: vec![
                ("a_images/x.png".into(), vec![1, 2, 3]),
                ("a_images/y.svg".into(), Vec::new()),
            ],
            warnings: Vec::new(),
        };
        let (response, payload) = exported(4, file.clone());
        let mut buf = Vec::new();
        write_message(&mut buf, &response, Some(&payload)).unwrap();
        let (back, bytes) = read_response(&mut &buf[..]).unwrap().unwrap();
        let Response::Exported {
            version: 4,
            warnings,
            files,
        } = back
        else {
            panic!("{back:?}");
        };
        assert_eq!(written(warnings, files, bytes), Some(file));
        // Lengths that don't fit the payload.
        assert_eq!(
            written(Vec::new(), vec![("x".into(), 10)], vec![0; 3]),
            None
        );
        let huge = vec![("x".into(), usize::MAX), ("y".into(), 2)];
        assert_eq!(written(Vec::new(), huge, vec![0; 3]), None);
    }

    #[test]
    fn truncated_and_garbled_frames_fail() {
        let mut buf = Vec::new();
        write_message(&mut buf, &Response::Pdf { version: 1 }, Some(b"%PDF-1.7")).unwrap();
        // Cut anywhere inside: an error, never a wrong message.
        for cut in 1..buf.len() {
            let e = read_response(&mut &buf[..cut]).unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof, "cut at {cut}");
        }
        // Not JSON, or not a response.
        for body in [&b"{nope"[..], b"{\"type\":\"unknown\"}", b"[1,2]"] {
            let mut frame = Vec::new();
            write_frame(&mut frame, body).unwrap();
            let e = read_response(&mut &frame[..]).unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        }
        // A garbled length: refused before reading.
        let huge = (MAX_FRAME as u32 + 1).to_le_bytes();
        let e = read_frame(&mut &huge[..]).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        // A plausible length without the data: end of input, no allocation.
        let e = read_frame(&mut &(1u32 << 29).to_le_bytes()[..]).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn bad_requests_get_an_error_and_the_worker_goes_on() {
        let mut input = Vec::new();
        write_frame(&mut input, b"garbage").unwrap();
        write_message(
            &mut input,
            &Request::RenderPage {
                version: 1,
                index: 0,
            },
            None,
        )
        .unwrap();
        write_message(&mut input, &Request::Shutdown, None).unwrap();
        let mut output = Vec::new();
        run(io::Cursor::new(input), &mut output).unwrap();
        let mut r = &output[..];
        for _ in 0..2 {
            let (response, _) = read_response(&mut r).unwrap().unwrap();
            assert!(matches!(response, Response::Error { .. }), "{response:?}");
        }
        assert_eq!(read_response(&mut r).unwrap().unwrap().0, Response::Ok);
        assert!(read_response(&mut r).unwrap().is_none());
    }
}
