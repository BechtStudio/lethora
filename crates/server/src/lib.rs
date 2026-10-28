//! `lethora-server`: the sync server. It stores and forwards sealed records
//! and never reads content: no Loro here, only `lethora-protocol`.
//!
//! Clients talk to it over a WebSocket at `/v1/ws` (the messages of
//! `lethora_protocol::wire`, see [`conn`]) and over HTTP for large or
//! one-off things (snapshots, assets, invite keys, project deletion; see
//! [`http`]). Projects live in a [`store`] (the filesystem for now), in the
//! layout of [`project`]. Settings: [`config`]. TLS comes from a reverse
//! proxy.
//!
//! [`start`] runs a server inside the caller's tokio runtime (what the
//! binary and the tests do).

pub mod config;
pub mod conn;
pub mod http;
pub mod project;
pub mod store;

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lethora_protocol::invite::server_origin;
use lethora_protocol::members::Members;
use lethora_protocol::wire::{self, Code};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{error, info};

pub use config::Config;
use project::{Project, Refusal, Who};
use store::{FsStore, Store};

/// The server's version, for people and bug reports (in the welcome, at
/// startup and in `/healthz`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What `/healthz` answers and the startup line says: the server's version,
/// the protocol versions it serves and the feature level it knows.
pub fn version_line() -> String {
    format!(
        "lethora-server {VERSION}, protocol {}-{}, level {}",
        wire::MIN_VERSION,
        wire::VERSION,
        lethora_protocol::LEVEL
    )
}

/// Seconds since the unix epoch, by the server's clock.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Waits until `stop` turns true (or its sender is gone).
pub async fn stopped(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|stop| *stop).await;
}

/// One day, the window of the project creation limit.
const DAY: Duration = Duration::from_secs(24 * 60 * 60);
/// How often idle projects and invite keys are swept.
const SWEEP: Duration = Duration::from_secs(30);

/// An HTTP bearer token from a welcome, valid while its connection is open.
#[derive(Debug, Clone)]
pub struct Token {
    pub project: String,
    pub who: Who,
}

/// A token a connection holds; revoked when dropped (the connection ended).
pub struct IssuedToken {
    app: Arc<App>,
    pub token: String,
}

impl Drop for IssuedToken {
    fn drop(&mut self) {
        lock(&self.app.tokens).remove(&self.token);
    }
}

/// What every connection and request shares.
pub struct App {
    pub config: Config,
    /// The normalized public URL (an origin, no trailing slash).
    pub public_url: String,
    store: Arc<dyn Store>,
    projects: Mutex<HashMap<String, Arc<Project>>>,
    tokens: Mutex<HashMap<String, Token>>,
    creations: Mutex<HashMap<IpAddr, Vec<Instant>>>,
    connections: Mutex<HashMap<IpAddr, u32>>,
    next_peer: AtomicU64,
    shutdown: watch::Receiver<bool>,
    /// Each connection holds a clone until it ends; `None` once stopping.
    alive: Mutex<Option<mpsc::Sender<()>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Counts an open connection of an IP address until dropped.
pub struct ConnectionSlot {
    app: Arc<App>,
    ip: IpAddr,
    _alive: mpsc::Sender<()>,
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        let mut connections = lock(&self.app.connections);
        if let Some(n) = connections.get_mut(&self.ip) {
            *n -= 1;
            if *n == 0 {
                connections.remove(&self.ip);
            }
        }
    }
}

impl App {
    /// A slot for a new connection from `ip`, or why there is none.
    pub fn connection_slot(self: &Arc<Self>, ip: IpAddr) -> Result<ConnectionSlot, Refusal> {
        let Some(alive) = lock(&self.alive).clone() else {
            return Err(Refusal::new(Code::Internal, "the server is stopping"));
        };
        let mut connections = lock(&self.connections);
        let n = connections.entry(ip).or_default();
        if *n >= self.config.limits.connections_per_ip {
            return Err(Refusal::new(
                Code::RateLimited,
                "too many connections from this address",
            ));
        }
        *n += 1;
        Ok(ConnectionSlot {
            app: self.clone(),
            ip,
            _alive: alive,
        })
    }

    /// A receiver that turns true when the server stops.
    pub fn shutdown(&self) -> watch::Receiver<bool> {
        self.shutdown.clone()
    }

    /// A new connection id.
    pub fn peer_id(&self) -> u64 {
        self.next_peer.fetch_add(1, Ordering::Relaxed)
    }

    /// Project `id`, loaded if needed; `None` if there is none. Blocks.
    pub fn project(&self, id: &str) -> Result<Option<Arc<Project>>, Refusal> {
        let mut projects = lock(&self.projects);
        if let Some(project) = projects.get(id) {
            return Ok(Some(project.clone()));
        }
        let loaded = Project::load(self.store.clone(), self.config.limits.clone(), id)?;
        Ok(loaded.map(|project| {
            let project = Arc::new(project);
            projects.insert(id.to_owned(), project.clone());
            project
        }))
    }

    /// Creates the project of checked genesis record `genesis` for client
    /// `ip`. Blocks.
    pub fn create_project(
        &self,
        ip: IpAddr,
        genesis: &[u8],
        members: Members,
    ) -> Result<Arc<Project>, Refusal> {
        let id = members.project().to_owned();
        {
            let mut creations = lock(&self.creations);
            let times = creations.entry(ip).or_default();
            times.retain(|t| t.elapsed() < DAY);
            if times.len() >= self.config.limits.projects_per_ip_per_day as usize {
                return Err(Refusal::new(
                    Code::RateLimited,
                    "too many new projects from this address today",
                ));
            }
        }
        let mut projects = lock(&self.projects);
        let members_path = format!("{}/members.log", Project::dir_of(&id));
        let exists = self
            .store
            .size(&members_path)
            .map_err(|e| Refusal::internal(format!("storage: {e}")))?
            .is_some();
        if projects.contains_key(&id) || exists {
            return Err(Refusal::new(Code::Exists, "the project exists already"));
        }
        let project = Arc::new(Project::create(
            self.store.clone(),
            self.config.limits.clone(),
            &id,
            genesis,
            members,
        )?);
        projects.insert(id.clone(), project.clone());
        lock(&self.creations)
            .entry(ip)
            .or_default()
            .push(Instant::now());
        info!(project = id, "project created");
        Ok(project)
    }

    /// Deletes project `id`: closes its connections, forgets its tokens and
    /// deletes its files. Blocks.
    pub fn delete_project(&self, project: &Project) -> Result<(), Refusal> {
        let mut projects = lock(&self.projects);
        project.close_for_deletion();
        projects.remove(&project.id);
        lock(&self.tokens).retain(|_, t| t.project != project.id);
        self.store
            .delete_dir(&Project::dir_of(&project.id))
            .map_err(|e| Refusal::internal(format!("storage: {e}")))?;
        info!(project = project.id, "project deleted");
        Ok(())
    }

    /// A new token for `who` in `project`, valid until the result is
    /// dropped.
    pub fn issue_token(self: &Arc<Self>, project: &str, who: Who) -> Result<IssuedToken, Refusal> {
        let token =
            lethora_protocol::to_hex(&lethora_protocol::random::<32>().map_err(Refusal::internal)?);
        lock(&self.tokens).insert(
            token.clone(),
            Token {
                project: project.to_owned(),
                who,
            },
        );
        Ok(IssuedToken {
            app: self.clone(),
            token,
        })
    }

    /// The token `token`, if it is valid.
    pub fn token(&self, token: &str) -> Option<Token> {
        lock(&self.tokens).get(token).cloned()
    }

    /// Unloads idle projects, forgets old creation times and deletes the
    /// keys of closed invites. Blocks.
    fn sweep(&self) {
        let now = now();
        let unload = self.config.limits.unload_seconds;
        let loaded: Vec<Arc<Project>> = {
            let mut projects = lock(&self.projects);
            projects.retain(|_, p| Arc::strong_count(p) > 1 || !p.is_idle(unload));
            projects.values().cloned().collect()
        };
        for project in loaded {
            project.sweep_invites(now);
        }
        lock(&self.creations).retain(|_, times| {
            times.retain(|t| t.elapsed() < DAY);
            !times.is_empty()
        });
    }

    /// The client's address: the socket's, or for a trusted proxy the last
    /// `X-Forwarded-For` entry that isn't one.
    pub fn client_ip(&self, socket: SocketAddr, forwarded: Option<&str>) -> IpAddr {
        let ip = socket.ip();
        if !self.config.trusted_proxies.contains(&ip) {
            return ip;
        }
        forwarded
            .into_iter()
            .flat_map(|h| h.rsplit(','))
            .filter_map(|s| s.trim().parse::<IpAddr>().ok())
            .find(|ip| !self.config.trusted_proxies.contains(ip))
            .unwrap_or(ip)
    }
}

/// A running server (see [`start`]).
pub struct Running {
    /// The address it listens on.
    pub addr: SocketAddr,
    /// Its public URL, as clients sign it.
    pub public_url: String,
    stop: watch::Sender<bool>,
    app: Arc<App>,
    alive: mpsc::Receiver<()>,
    server: JoinHandle<io::Result<()>>,
    sweeper: JoinHandle<()>,
}

impl Running {
    /// Stops the server: no new connections, open ones closed, and waits
    /// until every connection has ended.
    pub async fn stop(mut self) {
        let _ = self.stop.send(true);
        lock(&self.app.alive).take();
        match self.server.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => error!("the server failed: {e}"),
            Err(e) => error!("the server task failed: {e}"),
        }
        let _ = self.sweeper.await;
        // Every connection drops its sender when it ends.
        while self.alive.recv().await.is_some() {}
        info!("stopped");
    }
}

/// The public URL of a server with `config` listening on `addr`.
fn public_url(config: &Config, addr: SocketAddr) -> io::Result<String> {
    let invalid = |e: String| io::Error::new(io::ErrorKind::InvalidInput, e);
    let url = match &config.public_url {
        Some(url) => url.trim().trim_end_matches('/').to_owned(),
        None if addr.ip().is_loopback() => format!("http://127.0.0.1:{}", addr.port()),
        None => {
            return Err(invalid(
                "public_url is required when not listening on a loopback address".into(),
            ));
        }
    };
    // Normalized as invite links are.
    server_origin(&url).map_err(|e| invalid(format!("public_url: {e}")))
}

/// Starts a server with `config` in the current tokio runtime.
pub async fn start(config: Config) -> io::Result<Running> {
    let listener = TcpListener::bind(config.listen).await?;
    let addr = listener.local_addr()?;
    let public_url = public_url(&config, addr)?;
    std::fs::create_dir_all(&config.data_dir)?;
    let store: Arc<dyn Store> = Arc::new(FsStore::new(config.data_dir.clone()));
    let (stop, shutdown) = watch::channel(false);
    let (alive_tx, alive) = mpsc::channel(1);
    let app = Arc::new(App {
        config,
        public_url: public_url.clone(),
        store,
        projects: Mutex::new(HashMap::new()),
        tokens: Mutex::new(HashMap::new()),
        creations: Mutex::new(HashMap::new()),
        connections: Mutex::new(HashMap::new()),
        next_peer: AtomicU64::new(1),
        shutdown: shutdown.clone(),
        alive: Mutex::new(Some(alive_tx)),
    });
    let router = http::router(app.clone());
    let mut until_stop = shutdown.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move { stopped(&mut until_stop).await })
        .await
    });
    let sweeper = {
        let app = app.clone();
        let mut until_stop = shutdown;
        tokio::spawn(async move {
            let mut every = tokio::time::interval(SWEEP);
            loop {
                tokio::select! {
                    _ = stopped(&mut until_stop) => break,
                    _ = every.tick() => {
                        let app = app.clone();
                        let _ = tokio::task::spawn_blocking(move || app.sweep()).await;
                    }
                }
            }
        })
    };
    info!(%addr, public_url, "{}: listening", version_line());
    Ok(Running {
        addr,
        public_url,
        stop,
        app,
        alive,
        server,
        sweeper,
    })
}
