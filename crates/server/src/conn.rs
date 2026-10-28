//! One WebSocket connection: challenge, hello, welcome, then the messages of
//! `lethora_protocol::wire` until either side closes.
//!
//! A device may subscribe, append to its own streams, append members
//! records and send presence. An invitee may subscribe to the members log
//! only and append its own `join`; it then reconnects as a device. Anything
//! else, a message that doesn't decode or one larger than the limit closes
//! the connection with a `Refused` saying why. Messages beyond a
//! connection's rate (messages and bytes per second, with bursts) are read
//! later, not refused; presence beyond its rate is dropped.
//!
//! A hello of a protocol version outside the served range, or below the
//! project's minimum level, is refused with `Version` (see
//! `lethora_protocol::wire`, "Versions"); the welcome carries the server's
//! version and the project's devices' last levels.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{CloseFrame, Message as WsMessage, WebSocket};
use lethora_protocol::members::Members;
use lethora_protocol::wire::{
    ClientVersion, Code, Hello, MIN_VERSION, Message, NONCE_LEN, VERSION,
};
use lethora_protocol::{Error as ProtocolError, random};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::time::{sleep, sleep_until, timeout};
use tracing::{debug, info};

use crate::project::{Peer, Project, Refusal, Who};
use crate::{App, ConnectionSlot, now, stopped};

/// Messages queued for a connection before live ones are held back (it
/// then catches up from the files instead).
const QUEUE: usize = 64;
/// Room above the message limit for the WebSocket itself, so a message
/// slightly too large gets a clear refusal.
pub const WS_SLACK: usize = 64 * 1024;

/// A token bucket: `rate` per second, at most `burst` saved up.
struct Bucket {
    tokens: f64,
    rate: f64,
    burst: f64,
    last: Instant,
}

impl Bucket {
    fn new(rate: f64, burst: f64) -> Self {
        Self {
            tokens: burst,
            rate,
            burst,
            last: Instant::now(),
        }
    }

    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.rate).min(self.burst);
        self.last = now;
    }

    /// Takes `n` tokens; returns how long to wait until they are paid for.
    fn take(&mut self, n: f64) -> Duration {
        self.refill();
        self.tokens -= n;
        if self.tokens >= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(-self.tokens / self.rate)
        }
    }

    /// Takes one token if there is one.
    fn try_take(&mut self) -> bool {
        self.refill();
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Encodes and sends `message`; `false` if the connection is gone.
async fn send(socket: &mut WebSocket, message: &Message) -> bool {
    match message.encode() {
        Ok(bytes) => socket.send(WsMessage::Binary(bytes.into())).await.is_ok(),
        Err(e) => {
            Refusal::internal(format!("can't encode a message: {e}"));
            false
        }
    }
}

/// Sends `refusal` and closes the connection.
async fn refuse(mut socket: WebSocket, refusal: Refusal) {
    debug!(code = ?refusal.code, message = refusal.message, "refusing");
    let message = Message::Refused {
        code: refusal.code,
        message: refusal.message.clone(),
    };
    send(&mut socket, &message).await;
    let mut reason = refusal.message;
    while reason.len() > 120 {
        reason.pop();
    }
    let _ = socket
        .send(WsMessage::Close(Some(CloseFrame {
            code: 1008,
            reason: reason.into(),
        })))
        .await;
}

/// What came in.
enum Incoming {
    Message(Message, usize),
    Closed,
}

/// The next message from the client, skipping WebSocket pings and pongs.
async fn receive(socket: &mut WebSocket, limit: u64) -> Result<Incoming, Refusal> {
    loop {
        let Some(Ok(frame)) = socket.recv().await else {
            return Ok(Incoming::Closed);
        };
        match frame {
            WsMessage::Binary(bytes) => {
                if bytes.len() as u64 > limit {
                    return Err(Refusal::new(
                        Code::TooLarge,
                        format!("a message is at most {limit} bytes"),
                    ));
                }
                return match Message::decode(&bytes) {
                    Ok(message) => Ok(Incoming::Message(message, bytes.len())),
                    Err(ProtocolError::Unsupported(_)) if bytes[0] < MIN_VERSION => {
                        Err(Refusal::new(
                            Code::Version,
                            format!(
                                "This version of Lethora is too old for this server (protocol {}, the server speaks {MIN_VERSION} to {VERSION}). Update Lethora.",
                                bytes[0]
                            ),
                        ))
                    }
                    Err(ProtocolError::Unsupported(_)) if bytes[0] > VERSION => Err(Refusal::new(
                        Code::Version,
                        format!(
                            "This server is older than this version of Lethora (protocol {}, the server speaks {MIN_VERSION} to {VERSION}). Ask its admin to update it.",
                            bytes[0]
                        ),
                    )),
                    Err(e) => Err(Refusal::new(Code::BadRequest, e.to_string())),
                };
            }
            WsMessage::Text(_) => {
                return Err(Refusal::new(Code::BadRequest, "text messages aren't used"));
            }
            WsMessage::Close(_) => return Ok(Incoming::Closed),
            WsMessage::Ping(_) | WsMessage::Pong(_) => {}
        }
    }
}

/// Runs `f` off the async threads.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, Refusal> + Send + 'static,
) -> Result<T, Refusal> {
    tokio::task::spawn_blocking(f)
        .await
        .unwrap_or_else(|e| Err(Refusal::internal(e)))
}

/// An accepted hello.
struct Session {
    project: Arc<Project>,
    who: Who,
}

/// Checks `hello` of a client at `client` and its `signature` against
/// challenge `nonce`; remembers a device's level and app version.
async fn authenticate(
    app: &Arc<App>,
    ip: IpAddr,
    hello: Hello,
    client: ClientVersion,
    signature: [u8; 64],
    nonce: [u8; NONCE_LEN],
) -> Result<Session, Refusal> {
    let app = app.clone();
    blocking(move || {
        let bad_signature = || Refusal::new(Code::Auth, "the hello's signature doesn't match");
        let url = app.public_url.clone();
        let find = |project: &str| {
            app.project(project)?
                .ok_or_else(|| Refusal::new(Code::UnknownProject, "there is no such project here"))
        };
        match &hello {
            Hello::Device { project, device } => {
                let project = find(project)?;
                project.member(device)?;
                let public = project.with_members(|m| m.device(device).map(|d| d.public.sign));
                let public = public.ok_or_else(bad_signature)?;
                hello
                    .verify(&client, &public, &nonce, &url, &signature)
                    .map_err(|_| bad_signature())?;
                project.saw(device, &client);
                Ok(Session {
                    project,
                    who: Who::Device(device.clone()),
                })
            }
            Hello::Invitee { project, invite } => {
                let project = find(project)?;
                let public = project.open_invite_key(invite, now())?;
                hello
                    .verify(&client, &public, &nonce, &url, &signature)
                    .map_err(|_| bad_signature())?;
                Ok(Session {
                    project,
                    who: Who::Invitee(invite.clone()),
                })
            }
            Hello::Create { genesis } => {
                let members = Members::genesis(genesis)
                    .map_err(|e| Refusal::new(Code::BadRequest, e.to_string()))?;
                let device = members
                    .devices_of(members.owner())
                    .next()
                    .map(|(id, d)| (id.to_owned(), d.public.sign))
                    .expect("genesis has a device");
                hello
                    .verify(&client, &device.1, &nonce, &url, &signature)
                    .map_err(|_| bad_signature())?;
                let project = app.create_project(ip, genesis, members)?;
                project.saw(&device.0, &client);
                Ok(Session {
                    project,
                    who: Who::Device(device.0),
                })
            }
        }
    })
    .await
}

/// Serves one connection from `ip`.
pub async fn serve(app: Arc<App>, mut socket: WebSocket, ip: IpAddr, slot: ConnectionSlot) {
    let _slot = slot;
    let limits = app.config.limits.clone();
    let nonce: [u8; NONCE_LEN] = match random() {
        Ok(nonce) => nonce,
        Err(e) => return refuse(socket, Refusal::internal(e)).await,
    };
    let challenge = Message::Challenge {
        nonce,
        server_time: now(),
    };
    if !send(&mut socket, &challenge).await {
        return;
    }
    let first = timeout(
        Duration::from_secs(limits.hello_seconds),
        receive(&mut socket, limits.message_bytes),
    )
    .await;
    let (hello, client, signature) = match first {
        Err(_) => return refuse(socket, Refusal::new(Code::BadRequest, "no hello in time")).await,
        Ok(Err(refusal)) => return refuse(socket, refusal).await,
        Ok(Ok(Incoming::Closed)) => return,
        Ok(Ok(Incoming::Message(
            Message::Hello {
                hello,
                client,
                signature,
            },
            _,
        ))) => (hello, client, signature),
        Ok(Ok(Incoming::Message(..))) => {
            return refuse(socket, Refusal::new(Code::BadRequest, "expected a hello")).await;
        }
    };
    let level = client.level;
    let session = match authenticate(&app, ip, hello, client, signature, nonce).await {
        Ok(session) => session,
        Err(refusal) => return refuse(socket, refusal).await,
    };
    let Session { project, who } = session;

    let id = app.peer_id();
    let (tx, mut rx) = mpsc::channel(QUEUE);
    let wake = Arc::new(Notify::new());
    let (kick_tx, kick_rx) = oneshot::channel();
    let peer = Peer::new(who.clone(), level, tx.clone(), wake.clone(), kick_tx);
    if let Err(refusal) = project.register(id, peer) {
        return refuse(socket, refusal).await;
    }
    let welcome = {
        let (app, project, who) = (app.clone(), project.clone(), who.clone());
        blocking(move || {
            let (role, streams, devices) = match &who {
                Who::Device(device) => (
                    Some(project.member(device)?),
                    project.own_streams(device),
                    project.device_versions(),
                ),
                Who::Invitee(_) => (None, Vec::new(), Vec::new()),
            };
            let token = app.issue_token(&project.id, who)?;
            let welcome = Message::Welcome {
                token: token.token.clone(),
                role,
                streams,
                app: crate::VERSION.into(),
                devices,
            };
            Ok((welcome, token))
        })
        .await
    };
    // The token is revoked when this connection ends (dropped).
    let token = match welcome {
        Ok((welcome, token)) => {
            if !send(&mut socket, &welcome).await {
                project.unregister(id);
                return;
            }
            token
        }
        Err(refusal) => {
            project.unregister(id);
            return refuse(socket, refusal).await;
        }
    };
    info!(project = project.id, who = ?who, "connected");

    let mut connection = Connection {
        app: app.clone(),
        project: project.clone(),
        id,
        who,
        messages: Bucket::new(
            f64::from(limits.messages_per_second),
            f64::from(limits.message_burst),
        ),
        bytes: Bucket::new(limits.bytes_per_second as f64, limits.byte_burst as f64),
        presence: Bucket::new(
            f64::from(limits.presence_per_second),
            f64::from(limits.presence_per_second),
        ),
        subscribed: false,
    };
    let mut shutdown = app.shutdown();
    let mut kick = Some(kick_rx);
    let mut catching_up = false;
    let idle = Duration::from_secs(limits.idle_seconds);
    let mut last_heard = Instant::now();
    let outcome: Option<Refusal> = loop {
        tokio::select! {
            _ = stopped(&mut shutdown) => {
                let _ = socket.send(WsMessage::Close(Some(CloseFrame {
                    code: 1001,
                    reason: "the server is stopping".into(),
                }))).await;
                break None;
            }
            refusal = async { kick.as_mut().expect("checked").await }, if kick.is_some() => {
                kick = None;
                if let Ok(refusal) = refusal {
                    break Some(refusal);
                }
            }
            _ = wake.notified() => catching_up = true,
            permit = tx.clone().reserve_owned(), if catching_up => {
                let Ok(permit) = permit else { break None };
                let project = project.clone();
                catching_up = tokio::task::spawn_blocking(move || project.pull(id, permit))
                    .await
                    .unwrap_or(false);
            }
            Some(out) = rx.recv() => {
                if !send(&mut socket, &out).await {
                    break None;
                }
            }
            _ = sleep_until((last_heard + idle).into()) => {
                break Some(Refusal::new(Code::BadRequest, "nothing heard for too long"));
            }
            incoming = receive(&mut socket, limits.message_bytes) => {
                last_heard = Instant::now();
                let (message, len) = match incoming {
                    Ok(Incoming::Message(message, len)) => (message, len),
                    Ok(Incoming::Closed) => break None,
                    Err(refusal) => break Some(refusal),
                };
                let wait = connection
                    .messages
                    .take(1.0)
                    .max(connection.bytes.take(len as f64));
                if !wait.is_zero() {
                    sleep(wait).await;
                }
                match connection.handle(message).await {
                    Ok(Reply::None) => {}
                    Ok(Reply::Send(reply)) => {
                        if !send(&mut socket, &reply).await {
                            break None;
                        }
                    }
                    Ok(Reply::CatchUp) => catching_up = true,
                    Err(refusal) => break Some(refusal),
                }
            }
        }
    };
    project.unregister(id);
    drop(token);
    info!(project = project.id, who = ?connection.who, "disconnected");
    if let Some(refusal) = outcome {
        refuse(socket, refusal).await;
    }
}

/// What a handled message asks of the connection.
enum Reply {
    None,
    Send(Message),
    CatchUp,
}

/// A connection after its welcome.
struct Connection {
    app: Arc<App>,
    project: Arc<Project>,
    id: u64,
    who: Who,
    messages: Bucket,
    bytes: Bucket,
    presence: Bucket,
    subscribed: bool,
}

impl Connection {
    async fn handle(&mut self, message: Message) -> Result<Reply, Refusal> {
        let (project, id) = (self.project.clone(), self.id);
        let device = match &self.who {
            Who::Device(device) => Some(device.clone()),
            Who::Invitee(_) => None,
        };
        match message {
            Message::Ping(value) => Ok(Reply::Send(Message::Pong(value))),
            Message::Pong(_) => Ok(Reply::None),
            Message::Subscribe { members, streams } => {
                if self.subscribed {
                    return Err(Refusal::new(Code::BadRequest, "subscribed already"));
                }
                self.subscribed = true;
                blocking(move || project.subscribe(id, members, &streams)).await?;
                Ok(Reply::CatchUp)
            }
            Message::Append {
                document,
                batch,
                records,
            } => {
                let Some(device) = device else {
                    return Err(Refusal::new(Code::Forbidden, "an invitee doesn't append"));
                };
                let ack =
                    blocking(move || Ok(project.append(id, &device, &document, batch, &records)))
                        .await?;
                Ok(Reply::Send(ack))
            }
            Message::AppendMembers { batch, record } => {
                let who = self.who.clone();
                let ack =
                    blocking(move || Ok(project.append_members(id, &who, batch, &record, now())))
                        .await?;
                Ok(Reply::Send(ack))
            }
            Message::Presence { bytes } => {
                let Some(device) = device else {
                    return Err(Refusal::new(
                        Code::Forbidden,
                        "an invitee doesn't send presence",
                    ));
                };
                let max = self.app.config.limits.presence_bytes;
                if bytes.len() as u64 > max {
                    return Err(Refusal::new(
                        Code::TooLarge,
                        format!("presence is at most {max} bytes"),
                    ));
                }
                if self.presence.try_take() {
                    blocking(move || {
                        project.presence(id, &device, bytes);
                        Ok(())
                    })
                    .await?;
                }
                Ok(Reply::None)
            }
            other => Err(Refusal::new(
                Code::BadRequest,
                format!("unexpected message {}", name(&other)),
            )),
        }
    }
}

/// The name of `message`'s kind, for errors.
fn name(message: &Message) -> &'static str {
    match message {
        Message::Challenge { .. } => "Challenge",
        Message::Hello { .. } => "Hello",
        Message::Welcome { .. } => "Welcome",
        Message::Refused { .. } => "Refused",
        Message::Subscribe { .. } => "Subscribe",
        Message::MembersRecords { .. } => "MembersRecords",
        Message::Records { .. } => "Records",
        Message::SnapshotChanged { .. } => "SnapshotChanged",
        Message::Synced => "Synced",
        Message::Append { .. } => "Append",
        Message::Ack { .. } => "Ack",
        Message::Nack { .. } => "Nack",
        Message::AppendMembers { .. } => "AppendMembers",
        Message::MembersAck { .. } => "MembersAck",
        Message::Compacted { .. } => "Compacted",
        Message::Presence { .. } => "Presence",
        Message::PeerPresence { .. } => "PeerPresence",
        Message::Ping(_) => "Ping",
        Message::Pong(_) => "Pong",
    }
}
