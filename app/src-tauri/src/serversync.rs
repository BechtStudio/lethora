//! Server sync in the app: one async task per open project (and one per
//! join in progress) on Tauri's tokio runtime, driving the core's sans-I/O
//! [`ServerSync`] over a WebSocket (`tokio-tungstenite`, the OS's TLS) and
//! HTTP (`reqwest`, HTTP/1 only).
//!
//! The task connects to `<server>/v1/ws` (`ws://` for `http://` servers,
//! `wss://` for `https://`), lets the core say hello and subscribe, sends a
//! `Ping` every [`PING`], and reconnects after a lost connection with
//! exponential backoff ([`Backoff`]: 1 s doubling to 60 s, with jitter).
//! Edits nudge it ([`ServerLink::edited`]); it pushes [`PUSH_DELAY`] after the
//! last nudge. Queued remote changes become a `remote-changes` event, on
//! which the web view pulls through its edit queue (`sync_pull`). Presence
//! goes out when the core says it is due (`presence_due`); others' presence
//! changing (or going with the connection) becomes a `presence-changed`
//! event, on which the web view asks for it (`presence_get`).
//!
//! Locks: the document lock (the app state) is always taken before the
//! sync's own lock, never the other way round; incoming messages need only
//! the sync's lock.
//!
//! A sync that stops because this device was removed or the project deleted
//! makes the open project local again ([`crate::forget_server`]) and tells
//! the web view (`server-left`, with the reason).
//!
//! Also here: the servers used last, the projects shared or joined here (to
//! reopen them from the server) and the user's display name in the app
//! config dir (`servers.json`, `server_projects.json`, `profile.json`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use lethora_core::protocol::envelope::Keyring;
use lethora_core::protocol::wire::Message;
use lethora_core::protocol::{from_hex_array, to_hex};
use lethora_core::{Action, HttpRequest, HttpResponse, Method, ServerState, ServerSync};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep, sleep_until, timeout};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::foldersync::{self, Health, Status, now_ms};
use crate::{AppState, write_atomic};

/// How often the client pings (the server closes connections idle for 120 s).
const PING: Duration = Duration::from_secs(30);
/// How long after an edit the push goes out.
const PUSH_DELAY: Duration = Duration::from_millis(200);
/// How long a connection attempt may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// How long an HTTP request may take (snapshots and images can be large).
const HTTP_TIMEOUT: Duration = Duration::from_secs(300);
/// The servers used last, in the app config dir.
const SERVERS: &str = "servers.json";
/// Most servers remembered.
const SERVERS_MAX: usize = 5;
/// The projects shared or joined on this device, in the app config dir.
const SERVER_PROJECTS: &str = "server_projects.json";

/// Distinguishes the links of different projects.
static NEXT_LINK: AtomicU64 = AtomicU64::new(1);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// The WebSocket URL of server `origin` (`http(s)://host[:port]`).
pub(crate) fn ws_url(origin: &str) -> String {
    let rest = origin
        .strip_prefix("https://")
        .map(|r| format!("wss://{r}"))
        .or_else(|| origin.strip_prefix("http://").map(|r| format!("ws://{r}")))
        .unwrap_or_else(|| origin.to_owned());
    format!("{rest}/v1/ws")
}

/// Waits between connection attempts: 1 s, doubling up to 60 s, each with
/// ±25 % jitter so devices don't retry in step.
#[derive(Debug, Default)]
pub(crate) struct Backoff {
    attempt: u32,
}

impl Backoff {
    /// The wait before the next attempt; `random` in `0..1` sets the jitter.
    pub fn next(&mut self, random: f64) -> Duration {
        let base = 2f64.powi(self.attempt.min(6) as i32).min(60.0);
        self.attempt = self.attempt.saturating_add(1);
        Duration::from_secs_f64(base * (0.75 + 0.5 * random.clamp(0.0, 1.0)))
    }

    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}

/// A random number in `0..1`.
fn random() -> f64 {
    let mut bytes = [0u8; 4];
    let _ = getrandom::fill(&mut bytes);
    f64::from(u32::from_le_bytes(bytes)) / f64::from(u32::MAX)
}

fn now_secs() -> u64 {
    now_ms() / 1000
}

/// What the task tells the UI about the connection.
#[derive(Debug, Default, Clone)]
struct Conn {
    connected: bool,
    /// Unix ms of the next attempt while offline.
    retry_at: Option<u64>,
    error: Option<String>,
    /// Unix ms the device was last online.
    last_sync: Option<u64>,
}

enum Nudge {
    /// The document changed: push soon.
    Edited,
    /// The sync has new actions (a command made some).
    Act,
    Stop,
}

/// Server sync for one project (or a join in progress): the core's state,
/// shared with the task that drives it.
pub(crate) struct ServerLink {
    pub id: u64,
    pub sync: Arc<Mutex<ServerSync>>,
    conn: Arc<Mutex<Conn>>,
    nudge: mpsc::UnboundedSender<Nudge>,
    done: Option<tokio::sync::oneshot::Receiver<()>>,
}

impl ServerLink {
    /// Starts the task for `sync`; `project` says whether it belongs to the
    /// open project (pushes, `remote-changes`) or to a join.
    pub fn start(app: &AppHandle, sync: Arc<Mutex<ServerSync>>, project: bool) -> Self {
        let id = NEXT_LINK.fetch_add(1, Ordering::Relaxed);
        let conn = Arc::new(Mutex::new(Conn::default()));
        let (nudge, nudges) = mpsc::unbounded_channel();
        let (done_tx, done) = tokio::sync::oneshot::channel();
        let task = Task {
            app: app.clone(),
            id,
            project,
            stored_epoch: lock(&sync).keyring().map_or(0, |k| k.newest().0),
            sync: sync.clone(),
            conn: conn.clone(),
            nudges,
            http: reqwest::Client::builder()
                .timeout(HTTP_TIMEOUT)
                .build()
                .unwrap_or_default(),
        };
        tauri::async_runtime::spawn(async move {
            task.run().await;
            let _ = done_tx.send(());
        });
        Self {
            id,
            sync,
            conn,
            nudge,
            done: Some(done),
        }
    }

    /// The document changed: push soon.
    pub fn edited(&self) {
        let _ = self.nudge.send(Nudge::Edited);
    }

    /// A command gave the sync something to do.
    pub fn act(&self) {
        let _ = self.nudge.send(Nudge::Act);
    }

    /// Stops the task and waits until it has let go of the connection.
    pub async fn stop(mut self) {
        let _ = self.nudge.send(Nudge::Stop);
        if let Some(done) = self.done.take() {
            let _ = timeout(Duration::from_secs(5), done).await;
        }
    }

    /// The status for the web view.
    pub fn status(&self) -> Status {
        let sync = lock(&self.sync);
        let conn = lock(&self.conn).clone();
        let status = sync.status();
        let role = status
            .role
            .map(|r| {
                serde_json::to_value(r)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
            })
            .unwrap_or_default();
        let (state, message, retry_at) = match status.state {
            ServerState::Removed => (Health::Removed, status.message, None),
            ServerState::NeedsUpdate => (Health::NeedsUpdate, status.message, None),
            ServerState::Error => (Health::Error, status.message, None),
            _ if !conn.connected && conn.retry_at.is_some() => (
                Health::Offline,
                conn.error.map(|e| format!("Can't reach the server: {e}")),
                conn.retry_at,
            ),
            ServerState::Online => (Health::Online, status.message, None),
            ServerState::Connecting => (Health::Connecting, status.message, None),
        };
        Status::server(sync.url(), state, message, role, conn.last_sync, retry_at)
    }
}

impl Drop for ServerLink {
    fn drop(&mut self) {
        let _ = self.nudge.send(Nudge::Stop);
    }
}

/// Why a connection ended.
enum Ended {
    Lost(String),
    Reconnect,
    Stop,
}

struct Task {
    app: AppHandle,
    id: u64,
    project: bool,
    /// The newest key epoch in the key file (see [`Task::keep_keys`]).
    stored_epoch: u32,
    sync: Arc<Mutex<ServerSync>>,
    conn: Arc<Mutex<Conn>>,
    nudges: mpsc::UnboundedReceiver<Nudge>,
    http: reqwest::Client,
}

type Results = mpsc::UnboundedSender<(u64, Result<HttpResponse, String>)>;

impl Task {
    async fn run(mut self) {
        let mut backoff = Backoff::default();
        loop {
            if lock(&self.sync).is_stopped() {
                let status = lock(&self.sync).status();
                if self.project
                    && status.state == ServerState::Removed
                    && crate::forget_server(&self.app, self.id)
                {
                    let _ = self
                        .app
                        .emit("server-left", status.message.unwrap_or_default());
                }
                // Nothing more to do; wait to be dropped.
                while let Some(nudge) = self.nudges.recv().await {
                    if matches!(nudge, Nudge::Stop) {
                        break;
                    }
                }
                return;
            }
            let url = ws_url(lock(&self.sync).url());
            let ended = match timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(&url)).await
            {
                Ok(Ok((ws, _))) => self.session(ws, &mut backoff).await,
                Ok(Err(e)) => Ended::Lost(e.to_string()),
                Err(_) => Ended::Lost("no answer in time".into()),
            };
            {
                let mut conn = lock(&self.conn);
                conn.connected = false;
            }
            lock(&self.sync).disconnected();
            if self.project {
                // Others' presence went with the connection.
                let _ = self.app.emit("presence-changed", ());
            }
            let error = match ended {
                Ended::Stop => return,
                Ended::Reconnect => continue,
                Ended::Lost(_) if lock(&self.sync).is_stopped() => continue,
                Ended::Lost(error) => error,
            };
            let wait = backoff.next(random());
            {
                let mut conn = lock(&self.conn);
                conn.retry_at = Some(now_ms() + wait.as_millis() as u64);
                conn.error = Some(error.clone());
            }
            eprintln!("Server sync: {error}; trying again in {} s", wait.as_secs());
            let until = Instant::now() + wait;
            loop {
                tokio::select! {
                    _ = sleep_until(until) => break,
                    nudge = self.nudges.recv() => match nudge {
                        None | Some(Nudge::Stop) => return,
                        Some(_) => {}
                    },
                }
            }
        }
    }

    /// One connection, until it ends.
    async fn session(
        &mut self,
        ws: tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        backoff: &mut Backoff,
    ) -> Ended {
        let (mut sink, mut stream) = ws.split();
        {
            let mut conn = lock(&self.conn);
            conn.connected = true;
            conn.retry_at = None;
            conn.error = None;
        }
        lock(&self.sync).connected();
        let (results, mut results_rx) = mpsc::unbounded_channel();
        let mut ping = tokio::time::interval(PING);
        ping.tick().await;
        let mut pings = 0u64;
        let mut push_at: Option<Instant> = None;
        let ended = loop {
            let acted = self.act(&mut sink, &results).await;
            self.keep_keys();
            match acted {
                Ok(Some(ended)) => break ended,
                Ok(None) => {}
                Err(e) => break Ended::Lost(e),
            }
            if lock(&self.sync).is_stopped() {
                break Ended::Lost("stopped".into());
            }
            if lock(&self.sync).status().state == ServerState::Online {
                backoff.reset();
                lock(&self.conn).last_sync = Some(now_ms());
            }
            let due = push_at;
            let push_due = async move {
                match due {
                    Some(at) => sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            let presence_at = lock(&self.sync).presence_due();
            let presence_due = async move {
                match presence_at {
                    Some(at) => sleep(Duration::from_millis(at.saturating_sub(now_ms()))).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                frame = stream.next() => match frame {
                    Some(Ok(WsMessage::Binary(bytes))) => match Message::decode(&bytes) {
                        Ok(message) => {
                            let mut sync = lock(&self.sync);
                            sync.tick(now_secs());
                            sync.handle(message);
                        }
                        Err(e) => break Ended::Lost(format!("a message didn't decode: {e}")),
                    },
                    Some(Ok(WsMessage::Close(frame))) => {
                        break Ended::Lost(frame.map_or("closed".into(), |f| f.reason.to_string()));
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => break Ended::Lost(e.to_string()),
                    None => break Ended::Lost("closed".into()),
                },
                nudge = self.nudges.recv() => match nudge {
                    None | Some(Nudge::Stop) => break Ended::Stop,
                    Some(Nudge::Edited) => {
                        push_at.get_or_insert(Instant::now() + PUSH_DELAY);
                    }
                    Some(Nudge::Act) => {}
                },
                result = results_rx.recv() => {
                    if let Some((id, result)) = result {
                        let mut sync = lock(&self.sync);
                        sync.tick(now_secs());
                        sync.http_result(id, result);
                    }
                }
                _ = ping.tick() => {
                    pings += 1;
                    lock(&self.sync).tick(now_secs());
                    if sink.send(binary(&Message::Ping(pings))).await.is_err() {
                        break Ended::Lost("the connection broke".into());
                    }
                    // The autosave tick nudges too; this catches anything left.
                    push_at.get_or_insert(Instant::now());
                }
                _ = push_due => {
                    push_at = None;
                    self.push();
                }
                _ = presence_due => lock(&self.sync).presence_tick(now_ms()),
            }
        };
        let _ = sink.close().await;
        ended
    }

    /// Carries out the sync's actions until there are none. Returns how the
    /// connection should end, if it should.
    async fn act(
        &self,
        sink: &mut (impl SinkExt<WsMessage> + Unpin),
        results: &Results,
    ) -> Result<Option<Ended>, String> {
        let mut ended = None;
        loop {
            let actions = lock(&self.sync).take_actions();
            if actions.is_empty() {
                return Ok(ended);
            }
            for action in actions {
                match action {
                    Action::Send(message) => {
                        if sink.send(binary(&message)).await.is_err() {
                            return Err("the connection broke".into());
                        }
                    }
                    Action::Http(request) => self.request(request, results.clone()),
                    Action::RemoteChanges => {
                        if self.project {
                            let _ = self.app.emit("remote-changes", ());
                        }
                    }
                    Action::Push => self.push(),
                    Action::Reconnect => ended = Some(Ended::Reconnect),
                    Action::Presence => {
                        if self.project {
                            let _ = self.app.emit("presence-changed", ());
                        }
                    }
                }
            }
        }
    }

    /// Logs the sync's notes and, for the open project, stores its keys
    /// when a new key epoch arrived (keys sealed to this device could be
    /// found in the members log again, but not those that came with an
    /// invite).
    fn keep_keys(&mut self) {
        let mut sync = lock(&self.sync);
        for note in sync.take_notes() {
            eprintln!("Server sync: {note}");
        }
        let Some(keyring) = sync.keyring().filter(|k| k.newest().0 != self.stored_epoch) else {
            return;
        };
        if !self.project || sync.is_stopped() {
            return;
        }
        let Some(config) = self.app.path().app_config_dir().ok() else {
            return;
        };
        match store_key(&config, sync.project(), keyring) {
            Ok(()) => self.stored_epoch = keyring.newest().0,
            Err(e) => eprintln!("Server sync: {e}"),
        }
    }

    /// Pushes from the open project's document, if this link is its.
    fn push(&self) {
        if !self.project {
            return;
        }
        let state = self.app.state::<AppState>();
        let mut guard = state.lock();
        let Some(project) = guard.as_mut() else {
            return;
        };
        if project.server.as_ref().map(|s| s.id) != Some(self.id) {
            return;
        }
        let mut sync = lock(&self.sync);
        sync.tick(now_secs());
        if let Err(e) = project.doc.server_push(&mut sync) {
            eprintln!("Server sync: pushing failed: {e}");
        }
    }

    /// Makes `request` in the background; the result goes to `results`.
    fn request(&self, request: HttpRequest, results: Results) {
        let client = self.http.clone();
        let base = lock(&self.sync).url().to_owned();
        tauri::async_runtime::spawn(async move {
            let url = format!("{base}{}", request.path);
            let builder = match request.method {
                Method::Get => client.get(&url),
                Method::Head => client.head(&url),
                Method::Put => client.put(&url).body(request.body),
                Method::Delete => client.delete(&url).body(request.body),
            };
            let result = async {
                let response = builder
                    .bearer_auth(&request.token)
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                let status = response.status().as_u16();
                let generation = response
                    .headers()
                    .get("x-lethora-generation")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse().ok());
                let body = response.bytes().await.map_err(|e| e.to_string())?.to_vec();
                Ok(HttpResponse {
                    status,
                    body,
                    generation,
                })
            }
            .await;
            let _ = results.send((request.id, result));
        });
    }
}

fn binary(message: &Message) -> WsMessage {
    WsMessage::Binary(message.encode().unwrap_or_default().into())
}

/// Waits (up to `limit`) until `done` says something about `sync`.
pub(crate) async fn wait_for<T>(
    sync: &Mutex<ServerSync>,
    limit: Duration,
    done: impl Fn(&ServerSync) -> Option<T>,
) -> Option<T> {
    let until = Instant::now() + limit;
    while Instant::now() < until {
        let out = {
            let sync = lock(sync);
            done(&sync)
        };
        if out.is_some() {
            return out;
        }
        sleep(Duration::from_millis(100)).await;
    }
    None
}

/// Locks `sync` (for the app's commands).
pub(crate) fn locked(sync: &Mutex<ServerSync>) -> MutexGuard<'_, ServerSync> {
    lock(sync)
}

// ---- Keys, servers and the display name ----

/// Stores the project keys of server project `project` in
/// `keys/<project>.key`: a line `<epoch>:<64 hex>` per epoch.
pub(crate) fn store_key(config: &Path, project: &str, keyring: &Keyring) -> Result<(), String> {
    let path = foldersync::key_path(config, project);
    let lines: String = keyring
        .epochs()
        .map(|(epoch, key)| format!("{epoch}:{}\n", to_hex(key)))
        .collect();
    path.parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| write_atomic(&path, lines.as_bytes()))
        .map_err(|e| format!("Could not store the project key: {e}"))
}

/// The stored project keys of server project `project` (see [`store_key`];
/// a bare 64-hex line, as folder sync writes it, is epoch 1). `None` if
/// there are none or a line is damaged.
pub(crate) fn read_key(config: &Path, project: &str) -> Option<Keyring> {
    let text = std::fs::read_to_string(foldersync::key_path(config, project)).ok()?;
    let keys = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|line| {
            let (epoch, hex) = line.split_once(':').unwrap_or(("1", line));
            Some((epoch.parse().ok()?, from_hex_array(hex)?))
        })
        .collect::<Option<Vec<_>>>()?;
    Keyring::from_epochs(keys)
}

/// Deletes the stored key of `project` (a share that failed).
pub(crate) fn forget_key(config: &Path, project: &str) {
    let _ = std::fs::remove_file(foldersync::key_path(config, project));
}

/// `servers.json`: the servers used last, newest first.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Servers {
    #[serde(default)]
    pub servers: Vec<String>,
}

impl Servers {
    pub fn read(config: &Path) -> Self {
        std::fs::read(config.join(SERVERS))
            .ok()
            .and_then(|json| serde_json::from_slice(&json).ok())
            .unwrap_or_default()
    }

    /// Puts `url` first (once, at most [`SERVERS_MAX`]).
    pub fn add(&mut self, url: &str) {
        self.servers.retain(|s| s != url);
        self.servers.insert(0, url.to_owned());
        self.servers.truncate(SERVERS_MAX);
    }

    pub fn write(&self, config: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(config)?;
        write_atomic(&config.join(SERVERS), &serde_json::to_vec_pretty(self)?)
    }
}

/// Remembers server `url` (a failure is only logged).
pub(crate) fn remember_server(config: Option<&PathBuf>, url: &str) {
    let Some(config) = config else {
        return;
    };
    let mut servers = Servers::read(config);
    servers.add(url);
    if let Err(e) = servers.write(config) {
        eprintln!("Could not write {SERVERS}: {e}");
    }
}

/// A project shared or joined on this device (see [`Remembered`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RememberedProject {
    /// The server's origin.
    pub server: String,
    pub project: String,
    pub title: String,
    /// This device's user in the project.
    pub user: String,
}

/// `server_projects.json`: the projects shared or joined on this device,
/// newest first, so one whose local copy is gone can be reopened from the
/// server (the key stays in `keys/`).
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Remembered {
    #[serde(default)]
    pub projects: Vec<RememberedProject>,
}

impl Remembered {
    pub fn read(config: &Path) -> Self {
        std::fs::read(config.join(SERVER_PROJECTS))
            .ok()
            .and_then(|json| serde_json::from_slice(&json).ok())
            .unwrap_or_default()
    }

    /// Puts `entry` first, replacing an older one of its project.
    pub fn add(&mut self, entry: RememberedProject) {
        self.projects.retain(|p| p.project != entry.project);
        self.projects.insert(0, entry);
    }

    pub fn remove(&mut self, project: &str) {
        self.projects.retain(|p| p.project != project);
    }

    pub fn write(&self, config: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(config)?;
        write_atomic(
            &config.join(SERVER_PROJECTS),
            &serde_json::to_vec_pretty(self)?,
        )
    }
}

/// Changes `server_projects.json` in `config` (a failure is only logged).
pub(crate) fn update_remembered(config: &Path, f: impl FnOnce(&mut Remembered)) {
    let mut remembered = Remembered::read(config);
    f(&mut remembered);
    if let Err(e) = remembered.write(config) {
        eprintln!("Could not write {SERVER_PROJECTS}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn websocket_urls_follow_the_scheme() {
        assert_eq!(
            ws_url("https://lethora.example"),
            "wss://lethora.example/v1/ws"
        );
        assert_eq!(
            ws_url("http://192.168.1.5:8080"),
            "ws://192.168.1.5:8080/v1/ws"
        );
    }

    #[test]
    fn backoff_doubles_to_a_minute_with_jitter() {
        let mut b = Backoff::default();
        let waits: Vec<f64> = (0..9).map(|_| b.next(0.5).as_secs_f64()).collect();
        assert_eq!(waits, [1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 60.0, 60.0, 60.0]);
        let mut b = Backoff::default();
        assert_eq!(b.next(0.0), Duration::from_millis(750));
        let mut b = Backoff::default();
        assert_eq!(b.next(1.0), Duration::from_millis(1250));
        b.next(0.5);
        b.reset();
        assert_eq!(b.next(0.5), Duration::from_secs(1));
    }

    #[test]
    fn servers_are_newest_first_once_and_at_most_five() {
        let mut s = Servers::default();
        for url in ["a", "b", "c", "a", "d", "e", "f"] {
            s.add(url);
        }
        assert_eq!(s.servers, ["f", "e", "d", "a", "c"]);
        let dir = std::env::temp_dir().join(format!("lethora-servers-{}", now_ms()));
        s.write(&dir).unwrap();
        assert_eq!(Servers::read(&dir), s);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn key_files_hold_every_epoch_and_read_folder_sync_keys() {
        let dir = std::env::temp_dir().join(format!("lethora-keys-{}", now_ms()));
        let project = "00112233445566778899aabbccddeeff";
        assert!(read_key(&dir, project).is_none());
        // A folder-sync key file (a bare hex line) is epoch 1.
        let code = lethora_core::ProjectCode::from_key([3; 32]);
        foldersync::write_key(&dir, project, &code).unwrap();
        let keyring = read_key(&dir, project).unwrap();
        assert_eq!(keyring.newest(), (1, &[3; 32]));
        let mut keyring = Keyring::new([1; 32]);
        keyring.insert(2, [2; 32]);
        keyring.insert(4, [4; 32]);
        store_key(&dir, project, &keyring).unwrap();
        let read: Vec<_> = read_key(&dir, project)
            .unwrap()
            .epochs()
            .map(|(e, k)| (e, *k))
            .collect();
        assert_eq!(read, [(1, [1; 32]), (2, [2; 32]), (4, [4; 32])]);
        std::fs::write(foldersync::key_path(&dir, project), "1:zz\n").unwrap();
        assert!(read_key(&dir, project).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn remembered_projects_are_newest_first_once_and_removed() {
        let entry = |project: &str, title: &str| RememberedProject {
            server: "https://lethora.example".into(),
            project: project.into(),
            title: title.into(),
            user: "u".into(),
        };
        let dir = std::env::temp_dir().join(format!("lethora-remembered-{}", now_ms()));
        update_remembered(&dir, |r| r.add(entry("a", "A")));
        update_remembered(&dir, |r| r.add(entry("b", "B")));
        update_remembered(&dir, |r| r.add(entry("a", "A again")));
        let titles: Vec<String> = Remembered::read(&dir)
            .projects
            .into_iter()
            .map(|p| p.title)
            .collect();
        assert_eq!(titles, ["A again", "B"]);
        update_remembered(&dir, |r| r.remove("a"));
        assert_eq!(Remembered::read(&dir).projects, [entry("b", "B")]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
