//! Server sync end to end: `Document`s on several devices through a real
//! `lethora-server` in-process (`127.0.0.1:0`, temporary folders), driven by
//! a small WebSocket and HTTP test transport.

use std::collections::{BTreeSet, HashMap};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use lethora_core::protocol::envelope::{Context, Keyring, header_v2, open_v2, seal_v2};
use lethora_core::protocol::frame::{Frame, frame, parse_frame};
use lethora_core::protocol::keys::DeviceKeys;
use lethora_core::protocol::members::{Members, Op, Role, entry_body};
use lethora_core::protocol::random_id;
use lethora_core::protocol::wire::{Code, Message};
use lethora_core::{
    Action, DeviceId, Document, Error, FsStorage, HttpResponse, Identity, Method, ReviewId,
    ServerState, ServerSync, UserId,
};
use lethora_server::Running;
use lethora_server::config::Config;
use loro::{
    Container, ExportMode, LoroDoc, LoroList, LoroMap, LoroText, LoroValue, PeerID,
    ValueOrContainer,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const WAIT: Duration = Duration::from_secs(20);

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// A device: its ids, keys and folder.
struct Device {
    identity: Identity,
    keys: DeviceKeys,
    dir: PathBuf,
}

impl Device {
    fn new(root: &Path, name: &str) -> Self {
        Self {
            identity: Identity {
                device: DeviceId::parse(&random_id().unwrap()).unwrap(),
                user: UserId::parse(&random_id().unwrap()).unwrap(),
            },
            keys: DeviceKeys::generate().unwrap(),
            dir: root.join(name),
        }
    }
}

/// The test transport of one device: a WebSocket to the server, HTTP on new
/// connections.
struct Client {
    addr: SocketAddr,
    ws: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    /// Drops the next `Ack` and the connection with it.
    lose_ack: bool,
}

impl Client {
    fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            ws: None,
            lose_ack: false,
        }
    }

    async fn connect(&mut self, sync: &mut ServerSync) {
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/v1/ws", self.addr))
            .await
            .unwrap();
        self.ws = Some(ws);
        sync.connected();
    }

    async fn disconnect(&mut self, sync: &mut ServerSync) {
        if let Some(mut ws) = self.ws.take() {
            let _ = ws.close(None).await;
        }
        sync.disconnected();
    }

    /// Carries out `sync`'s actions until there are none, pulling into and
    /// pushing from `doc` when asked.
    async fn act(&mut self, sync: &mut ServerSync, doc: &mut Option<&mut Document>) {
        loop {
            sync.tick(now());
            sync.presence_tick(now_ms());
            let actions = sync.take_actions();
            if actions.is_empty() {
                return;
            }
            for action in actions {
                match action {
                    Action::Send(message) => {
                        if let Some(ws) = &mut self.ws {
                            let bytes = message.encode().unwrap();
                            let _ = ws.send(WsMessage::Binary(bytes.into())).await;
                        }
                    }
                    Action::Http(request) => {
                        let result = http(self.addr, &request).await;
                        sync.http_result(request.id, Ok(result));
                    }
                    Action::RemoteChanges => {
                        if let Some(doc) = doc {
                            let pulled = doc.server_pull(sync).unwrap();
                            assert!(pulled.skipped.is_empty(), "{:?}", pulled.skipped);
                        }
                    }
                    Action::Push => {
                        if let Some(doc) = doc {
                            doc.server_push(sync).unwrap();
                        }
                    }
                    Action::Reconnect => {
                        self.disconnect(sync).await;
                        self.connect(sync).await;
                    }
                    Action::Presence => {}
                }
            }
        }
    }

    /// Drives `sync` (with `doc`, if given) until `done` holds; a sync that
    /// stops must satisfy `done` then.
    async fn drive(
        &mut self,
        sync: &mut ServerSync,
        mut doc: Option<&mut Document>,
        done: impl Fn(&ServerSync, Option<&Document>) -> bool,
    ) {
        let start = Instant::now();
        loop {
            assert!(
                start.elapsed() < WAIT,
                "timed out: {:?} idle {} text {:?}",
                sync.status(),
                sync.is_idle(),
                doc.as_ref().map(|d| d.text())
            );
            if sync.is_stopped() {
                assert!(done(sync, doc.as_deref()), "stopped: {:?}", sync.status());
                return;
            }
            if self.ws.is_none() {
                self.connect(sync).await;
            }
            self.act(sync, &mut doc).await;
            if let Some(doc) = &mut doc {
                doc.server_pull(sync).unwrap();
                doc.server_push(sync).unwrap();
                self.act(sync, &mut Some(&mut **doc)).await;
            }
            if done(sync, doc.as_deref()) {
                return;
            }
            let ws = self.ws.as_mut().unwrap();
            match tokio::time::timeout(Duration::from_millis(100), ws.next()).await {
                Err(_) => {}
                Ok(Some(Ok(WsMessage::Binary(bytes)))) => {
                    let message = Message::decode(&bytes).unwrap();
                    if self.lose_ack && matches!(message, Message::Ack { .. }) {
                        self.lose_ack = false;
                        self.disconnect(sync).await;
                        continue;
                    }
                    sync.handle(message);
                }
                Ok(Some(Ok(_))) => {}
                Ok(_) => {
                    self.ws = None;
                    sync.disconnected();
                }
            }
        }
    }
}

/// One HTTP/1.1 request on a new connection.
async fn http(addr: SocketAddr, request: &lethora_core::HttpRequest) -> HttpResponse {
    let method = match request.method {
        Method::Get => "GET",
        Method::Head => "HEAD",
        Method::Put => "PUT",
        Method::Delete => "DELETE",
    };
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let head = format!(
        "{method} {} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\
         Authorization: Bearer {}\r\nContent-Length: {}\r\n\r\n",
        request.path,
        request.token,
        request.body.len()
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.write_all(&request.body).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(WAIT, stream.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    let split = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&response[..split]).to_lowercase();
    let mut body = response[split + 4..].to_vec();
    if head.contains("transfer-encoding: chunked") {
        let mut plain = Vec::new();
        let mut at = 0;
        loop {
            let end = at + body[at..].windows(2).position(|w| w == b"\r\n").unwrap();
            let len =
                usize::from_str_radix(std::str::from_utf8(&body[at..end]).unwrap(), 16).unwrap();
            if len == 0 {
                break;
            }
            plain.extend_from_slice(&body[end + 2..end + 2 + len]);
            at = end + 2 + len + 2;
        }
        body = plain;
    }
    let generation = head
        .lines()
        .find_map(|l| l.strip_prefix("x-lethora-generation:"))
        .and_then(|v| v.trim().parse().ok());
    HttpResponse {
        status: head[9..12].parse().unwrap(),
        body,
        generation,
    }
}

fn online(sync: &ServerSync, _: Option<&Document>) -> bool {
    sync.status().state == ServerState::Online && sync.is_idle()
}

/// The peers whose changes the updates in `log` (a stream's framed records
/// of `device`) hold.
fn peers_in_log(
    log: &[u8],
    project: &str,
    device: &str,
    key: [u8; 32],
    public: &lethora_core::protocol::keys::DevicePublic,
    kind: &str,
) -> BTreeSet<PeerID> {
    let keyring = Keyring::new(key);
    let context = Context {
        project,
        document: "main",
        kind,
        name: device,
    };
    let mut peers = BTreeSet::new();
    let mut at = 0;
    let mut records = Vec::new();
    if kind == "snapshot" {
        records.push(log.to_vec());
    } else {
        while let Frame::Record(envelope, next) = parse_frame(log, at) {
            records.push(envelope.to_vec());
            at = next;
        }
    }
    for envelope in records {
        let update = open_v2(&keyring, public, &context, &envelope).unwrap();
        let meta = LoroDoc::decode_import_blob_meta(&update, true).unwrap();
        peers.extend(meta.partial_end_vv.keys().copied());
    }
    peers
}

/// How many records a stream's log holds.
fn record_count(log: &[u8]) -> usize {
    let mut at = 0;
    let mut n = 0;
    while let Frame::Record(_, next) = parse_frame(log, at) {
        at = next;
        n += 1;
    }
    n
}

/// Joins through `link` as `device`, building its project.
async fn join(
    client: &mut Client,
    link: &str,
    device: &Device,
    name: &str,
) -> (ServerSync, Document) {
    let mut sync = ServerSync::join(
        link,
        &device.identity.device,
        device.keys.clone(),
        &device.identity.user,
        name,
    )
    .unwrap();
    client.drive(&mut sync, None, |s, _| s.join_ready()).await;
    // As the app does: the joining connection closes before the project is
    // built, and the join stays ready.
    client.disconnect(&mut sync).await;
    assert!(sync.join_ready());
    let doc = Document::server_join(
        Box::new(FsStorage::new(&device.dir)),
        &mut sync,
        &device.identity,
    )
    .unwrap();
    (sync, doc)
}

#[tokio::test(flavor = "multi_thread")]
async fn devices_share_join_edit_compact_and_catch_up() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("server");
    let running = lethora_server::start(Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        data_dir: data.clone(),
        public_url: None,
        ..Config::default()
    })
    .await
    .unwrap();
    let addr = running.addr;
    let url = running.public_url.clone();

    // Device A shares a project with an image.
    let a = Device::new(temp.path(), "a");
    let mut doc_a = Document::create(&a.dir, &a.identity).unwrap();
    doc_a.insert(0, "Chapter one.").unwrap();
    let image = b"\x89PNG not really an image".to_vec();
    doc_a.put_asset("assets/figure.png", &image).unwrap();
    let (mut sync_a, key) = doc_a
        .server_share(&url, "Alice", a.keys.clone(), now())
        .unwrap();
    let project = sync_a.project().to_owned();
    let mut client_a = Client::new(addr);
    client_a.drive(&mut sync_a, Some(&mut doc_a), online).await;
    assert_eq!(sync_a.role(), Some(Role::Owner));
    assert_eq!(
        sync_a.profile_of(a.identity.user.as_str()).unwrap().name,
        "Alice"
    );
    doc_a.server_save(&sync_a).unwrap();
    assert_eq!(doc_a.server_settings().unwrap().project, project);
    let project_dir = data.join("projects").join(&project);
    assert!(
        project_dir
            .join(format!("main/{}.snapshot", a.identity.device.as_str()))
            .is_file()
    );
    // Folder sync is refused while shared.
    assert!(
        doc_a
            .sync_setup(Box::new(FsStorage::new(temp.path().join("replica"))))
            .is_err()
    );

    // A invites B as an editor.
    let invite = sync_a.invite(Some(Role::Editor), 7, 1).unwrap();
    client_a
        .drive(&mut sync_a, Some(&mut doc_a), |s, _| {
            s.invite_result(&invite).is_some()
        })
        .await;
    let link = sync_a.invite_result(&invite).unwrap().unwrap();
    assert!(link.starts_with(&format!("{url}/join/{project}/")));

    // B joins and gets the text and, by pulling, the image.
    let b = Device::new(temp.path(), "b");
    let mut client_b = Client::new(addr);
    let (mut sync_b, mut doc_b) = join(&mut client_b, &link, &b, "Bob").await;
    assert_eq!(doc_b.text(), "Chapter one.");
    assert_eq!(sync_b.role(), Some(Role::Editor));
    assert_eq!(doc_b.identity().user, b.identity.user);
    client_b
        .drive(&mut sync_b, Some(&mut doc_b), |s, d| {
            online(s, d)
                && d.unwrap()
                    .asset_bytes("assets/figure.png")
                    .unwrap()
                    .is_some()
        })
        .await;
    assert_eq!(doc_b.asset_bytes("assets/figure.png").unwrap(), Some(image));
    assert_eq!(
        sync_b.profile_of(b.identity.user.as_str()).unwrap().name,
        "Bob"
    );

    // Concurrent edits on both converge.
    doc_a.insert(0, "A: ").unwrap();
    let len = doc_b.len();
    doc_b.insert(len, " B wrote this.").unwrap();
    let expected = "A: Chapter one. B wrote this.";
    let converged =
        |s: &ServerSync, d: Option<&Document>| s.is_idle() && d.unwrap().text() == expected;
    client_a
        .drive(&mut sync_a, Some(&mut doc_a), |s, _| s.is_idle())
        .await;
    client_b
        .drive(&mut sync_b, Some(&mut doc_b), converged)
        .await;
    client_a
        .drive(&mut sync_a, Some(&mut doc_a), converged)
        .await;

    // B's stream holds only B's own changes: none of the peers in A's
    // uploads.
    let public = |device: &Device| device.keys.public();
    let a_name = a.identity.device.as_str();
    let b_name = b.identity.device.as_str();
    let a_snapshot = std::fs::read(project_dir.join(format!("main/{a_name}.snapshot"))).unwrap();
    let a_log = std::fs::read(project_dir.join(format!("main/{a_name}.log"))).unwrap();
    let b_log = std::fs::read(project_dir.join(format!("main/{b_name}.log"))).unwrap();
    let mut a_peers = peers_in_log(&a_snapshot, &project, a_name, key, &public(&a), "snapshot");
    a_peers.extend(peers_in_log(
        &a_log,
        &project,
        a_name,
        key,
        &public(&a),
        "log",
    ));
    let b_peers = peers_in_log(&b_log, &project, b_name, key, &public(&b), "log");
    assert!(!b_peers.is_empty());
    assert!(a_peers.is_disjoint(&b_peers), "{a_peers:?} {b_peers:?}");

    // A lost ack: the batch is not sent again after reconnecting.
    let before =
        record_count(&std::fs::read(project_dir.join(format!("main/{a_name}.log"))).unwrap());
    doc_a.insert(0, "!").unwrap();
    client_a.lose_ack = true;
    client_a
        .drive(&mut sync_a, Some(&mut doc_a), |s, _| online(s, None))
        .await;
    assert!(!client_a.lose_ack, "the ack was lost");
    client_a
        .drive(&mut sync_a, Some(&mut doc_a), |s, _| s.is_idle())
        .await;
    let after =
        record_count(&std::fs::read(project_dir.join(format!("main/{a_name}.log"))).unwrap());
    assert_eq!(after, before + 1);
    client_b
        .drive(&mut sync_b, Some(&mut doc_b), |_, d| {
            d.unwrap().text() == format!("!{expected}")
        })
        .await;

    // A compacts its stream; a new device C catches up from the snapshot.
    doc_a.set_compaction_threshold(1);
    doc_a.insert(0, "Title\n").unwrap();
    client_a
        .drive(&mut sync_a, Some(&mut doc_a), |s, _| {
            s.is_idle() && s.settings().own.generation >= 2
        })
        .await;
    doc_a.server_save(&sync_a).unwrap();
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(project_dir.join("meta.json")).unwrap()).unwrap();
    assert!(
        meta["streams"][format!("main/{a_name}")]["generation"]
            .as_u64()
            .unwrap()
            >= 2
    );
    let invite = sync_a.invite(Some(Role::Reader), 1, 1).unwrap();
    client_a
        .drive(&mut sync_a, Some(&mut doc_a), |s, _| {
            s.invite_result(&invite).is_some()
        })
        .await;
    let link = sync_a.invite_result(&invite).unwrap().unwrap();
    let c = Device::new(temp.path(), "c");
    let mut client_c = Client::new(addr);
    let (sync_c, doc_c) = join(&mut client_c, &link, &c, "Carol").await;
    assert_eq!(doc_c.text(), doc_a.text());
    assert_eq!(sync_c.role(), Some(Role::Reader));

    // B adds another device of its own: it joins as B's user.
    let invite = sync_b.invite(None, 1, 1).unwrap();
    client_b
        .drive(&mut sync_b, Some(&mut doc_b), |s, _| {
            s.invite_result(&invite).is_some()
        })
        .await;
    let link = sync_b.invite_result(&invite).unwrap().unwrap();
    let b2 = Device::new(temp.path(), "b2");
    let mut client_b2 = Client::new(addr);
    let (mut sync_b2, doc_b2) = join(&mut client_b2, &link, &b2, "Bob").await;
    assert_eq!(doc_b2.identity().user, b.identity.user);
    assert_eq!(sync_b2.role(), Some(Role::Editor));

    // Reopened, B2 is still B's user and picks up where it left off.
    let mut doc_b2 = doc_b2;
    doc_b2.server_save(&sync_b2).unwrap();
    drop(doc_b2);
    client_b2.disconnect(&mut sync_b2).await;
    let mut doc_b2 = Document::open(&b2.dir, &b2.identity).unwrap();
    assert_eq!(doc_b2.identity().user, b.identity.user);
    let mut sync_b2 = doc_b2
        .server_open(b2.keys.clone(), Keyring::new(key), None)
        .unwrap();
    client_b2
        .drive(&mut sync_b2, Some(&mut doc_b2), |s, d| {
            online(s, d) && d.unwrap().text() == doc_a.text()
        })
        .await;
    running.stop().await;
}

/// Creates an invite on `sync` (A's, with `doc`) and returns its link.
async fn invite_link(
    client: &mut Client,
    sync: &mut ServerSync,
    doc: &mut Document,
    role: Option<Role>,
) -> String {
    let invite = sync.invite(role, 7, 1).unwrap();
    client
        .drive(sync, Some(doc), |s, _| s.invite_result(&invite).is_some())
        .await;
    sync.invite_result(&invite).unwrap().unwrap()
}

/// Waits until `sync` knows member `user`.
async fn knows(client: &mut Client, sync: &mut ServerSync, doc: &mut Document, user: &UserId) {
    client
        .drive(sync, Some(doc), |s, _| {
            s.member_list().iter().any(|m| m.user == user.as_str())
        })
        .await;
}

/// Waits until change `id` of `sync` is done.
async fn change_done(client: &mut Client, sync: &mut ServerSync, doc: &mut Document, id: u64) {
    client
        .drive(sync, Some(doc), |s, _| s.change_result(id).is_some())
        .await;
    assert_eq!(sync.change_result(id), Some(Ok(())));
}

fn removed(sync: &ServerSync, _: Option<&Document>) -> bool {
    sync.status().state == ServerState::Removed
}

/// Roles, removals, revoked invites, leaving, reopening from the server and
/// deleting the project, through a real server.
#[tokio::test(flavor = "multi_thread")]
async fn members_roles_leaving_reopening_and_deletion() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("server");
    let running = lethora_server::start(Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        data_dir: data.clone(),
        public_url: None,
        ..Config::default()
    })
    .await
    .unwrap();
    let addr = running.addr;
    let url = running.public_url.clone();

    let a = Device::new(temp.path(), "a");
    let mut doc_a = Document::create(&a.dir, &a.identity).unwrap();
    doc_a.insert(0, "Hello.").unwrap();
    let (mut sync_a, key) = doc_a
        .server_share(&url, "Alice", a.keys.clone(), now())
        .unwrap();
    let project = sync_a.project().to_owned();
    let mut client_a = Client::new(addr);
    client_a.drive(&mut sync_a, Some(&mut doc_a), online).await;

    // B joins as an editor; A makes B a reader: B's edits are refused.
    let link = invite_link(&mut client_a, &mut sync_a, &mut doc_a, Some(Role::Editor)).await;
    let b = Device::new(temp.path(), "b");
    let mut client_b = Client::new(addr);
    let (mut sync_b, mut doc_b) = join(&mut client_b, &link, &b, "Bob").await;
    client_b.drive(&mut sync_b, Some(&mut doc_b), online).await;
    assert!(doc_b.is_writable());
    let b_user = b.identity.user.as_str().to_owned();
    knows(&mut client_a, &mut sync_a, &mut doc_a, &b.identity.user).await;
    let id = sync_a.set_role(&b_user, Role::Reader).unwrap();
    change_done(&mut client_a, &mut sync_a, &mut doc_a, id).await;
    client_b
        .drive(&mut sync_b, Some(&mut doc_b), |s, _| {
            s.role() == Some(Role::Reader)
        })
        .await;
    doc_b.server_save(&sync_b).unwrap();
    assert!(matches!(
        doc_b.insert(0, "x"),
        Err(lethora_core::Error::ReadOnly(_))
    ));
    // A's edits still reach the reader.
    doc_a.insert(0, "A: ").unwrap();
    client_a
        .drive(&mut sync_a, Some(&mut doc_a), |s, _| s.is_idle())
        .await;
    client_b
        .drive(&mut sync_b, Some(&mut doc_b), |_, d| {
            d.unwrap().text() == "A: Hello."
        })
        .await;
    // Back to editor: B writes again, and it reaches A.
    let id = sync_a.set_role(&b_user, Role::Editor).unwrap();
    change_done(&mut client_a, &mut sync_a, &mut doc_a, id).await;
    client_b
        .drive(&mut sync_b, Some(&mut doc_b), |s, _| {
            s.role() == Some(Role::Editor)
        })
        .await;
    doc_b.server_save(&sync_b).unwrap();
    let len = doc_b.len();
    doc_b.insert(len, " B.").unwrap();
    client_b
        .drive(&mut sync_b, Some(&mut doc_b), |s, _| s.is_idle())
        .await;
    client_a
        .drive(&mut sync_a, Some(&mut doc_a), |_, d| {
            d.unwrap().text() == "A: Hello. B."
        })
        .await;
    let names: Vec<_> = sync_a
        .member_list()
        .into_iter()
        .filter_map(|m| m.name)
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");

    // A revoked invite can't be used.
    let link = invite_link(&mut client_a, &mut sync_a, &mut doc_a, Some(Role::Reader)).await;
    let invite = sync_a.open_invites()[0].id.clone();
    assert!(link.contains(&invite));
    let id = sync_a.revoke(&invite).unwrap();
    change_done(&mut client_a, &mut sync_a, &mut doc_a, id).await;
    assert!(sync_a.open_invites().is_empty());
    let r = Device::new(temp.path(), "r");
    let mut sync_r = ServerSync::join(
        &link,
        &r.identity.device,
        r.keys.clone(),
        &r.identity.user,
        "Rita",
    )
    .unwrap();
    Client::new(addr)
        .drive(&mut sync_r, None, |s, _| s.is_stopped())
        .await;
    assert!(
        sync_r.status().message.unwrap().contains("can't be used"),
        "{:?}",
        sync_r.status()
    );

    // C joins and is removed by the owner: C stops, and its copy turns local.
    let link = invite_link(&mut client_a, &mut sync_a, &mut doc_a, Some(Role::Editor)).await;
    let c = Device::new(temp.path(), "c");
    let mut client_c = Client::new(addr);
    let (mut sync_c, mut doc_c) = join(&mut client_c, &link, &c, "Carol").await;
    client_c.drive(&mut sync_c, Some(&mut doc_c), online).await;
    knows(&mut client_a, &mut sync_a, &mut doc_a, &c.identity.user).await;
    let id = sync_a.remove_user(c.identity.user.as_str()).unwrap();
    change_done(&mut client_a, &mut sync_a, &mut doc_a, id).await;
    client_c.drive(&mut sync_c, Some(&mut doc_c), removed).await;
    drop(sync_c);
    doc_c.server_forget().unwrap();
    assert!(doc_c.server_settings().is_none() && doc_c.is_writable());
    doc_c.insert(0, "Mine now. ").unwrap();
    drop(doc_c);
    let doc_c = Document::open(&c.dir, &c.identity).unwrap();
    assert!(doc_c.server_settings().is_none());
    assert!(doc_c.text().starts_with("Mine now. "));

    // B leaves (removes its own device): B stops, A sees B removed.
    let id = sync_b.remove_device(b.identity.device.as_str()).unwrap();
    client_b
        .drive(&mut sync_b, Some(&mut doc_b), |s, _| {
            s.change_result(id).is_some() || s.is_stopped()
        })
        .await;
    assert_eq!(sync_b.status().state, ServerState::Removed);
    client_a
        .drive(&mut sync_a, Some(&mut doc_a), |s, _| {
            s.member_list()
                .iter()
                .any(|m| m.user == b_user && m.removed)
        })
        .await;

    // D joins, loses its copy and reopens the project from the server with
    // the key and user it kept: no invite needed.
    let link = invite_link(&mut client_a, &mut sync_a, &mut doc_a, Some(Role::Editor)).await;
    let d = Device::new(temp.path(), "d");
    let mut client_d = Client::new(addr);
    let (mut sync_d, mut doc_d) = join(&mut client_d, &link, &d, "Dan").await;
    let len = doc_d.len();
    doc_d.insert(len, " D.").unwrap();
    client_d
        .drive(&mut sync_d, Some(&mut doc_d), |s, d| {
            online(s, d) && d.unwrap().text() == "A: Hello. B. D."
        })
        .await;
    client_a
        .drive(&mut sync_a, Some(&mut doc_a), |_, d| {
            d.unwrap().text() == "A: Hello. B. D."
        })
        .await;
    client_d.disconnect(&mut sync_d).await;
    // As the app keeps it: every epoch D got (removals started new ones).
    let keyring = sync_d.keyring().unwrap().clone();
    assert!(keyring.newest().0 > 1);
    drop((sync_d, doc_d));
    std::fs::remove_dir_all(&d.dir).unwrap();
    let mut sync_d = ServerSync::reopen(
        &url,
        &project,
        &d.identity.device,
        d.keys.clone(),
        &d.identity.user,
        keyring,
        Some("Dan"),
    )
    .unwrap();
    let mut client_d = Client::new(addr);
    client_d
        .drive(&mut sync_d, None, |s, _| s.join_ready())
        .await;
    client_d.disconnect(&mut sync_d).await;
    let mut doc_d =
        Document::server_join(Box::new(FsStorage::new(&d.dir)), &mut sync_d, &d.identity).unwrap();
    assert_eq!(doc_d.text(), "A: Hello. B. D.");
    assert_eq!(sync_d.role(), Some(Role::Editor));
    client_d.drive(&mut sync_d, Some(&mut doc_d), online).await;

    // A removed device can't reopen.
    let mut sync_b = ServerSync::reopen(
        &url,
        &project,
        &b.identity.device,
        b.keys.clone(),
        &b.identity.user,
        Keyring::new(key),
        None,
    )
    .unwrap();
    Client::new(addr).drive(&mut sync_b, None, removed).await;

    // The owner deletes the project: the others stop, local copies stay.
    let id = sync_a.delete_project().unwrap();
    client_a
        .drive(&mut sync_a, Some(&mut doc_a), |s, _| {
            s.change_result(id).is_some()
        })
        .await;
    assert_eq!(sync_a.change_result(id), Some(Ok(())));
    assert_eq!(sync_a.status().state, ServerState::Removed);
    assert!(!data.join("projects").join(&project).exists());
    client_d.drive(&mut sync_d, Some(&mut doc_d), removed).await;
    doc_a.server_forget().unwrap();
    assert_eq!(doc_a.text(), "A: Hello. B. D.");
    running.stop().await;
}

/// A device that lost its local copy joins again, here through another
/// user's "add my other device" link: it stays the member it was, gets its
/// own earlier changes back from its stream (so later edits building on them
/// arrive), uploads nothing again and can invite its own devices.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_device_rejoins_after_losing_its_copy() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("server");
    let running = lethora_server::start(Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        data_dir: data.clone(),
        public_url: None,
        ..Config::default()
    })
    .await
    .unwrap();
    let addr = running.addr;
    let url = running.public_url.clone();

    let a = Device::new(temp.path(), "a");
    let mut doc_a = Document::create(&a.dir, &a.identity).unwrap();
    doc_a.insert(0, "Hello.").unwrap();
    let (mut sync_a, _) = doc_a
        .server_share(&url, "Alice", a.keys.clone(), now())
        .unwrap();
    let project = sync_a.project().to_owned();
    let mut client_a = Client::new(addr);
    client_a.drive(&mut sync_a, Some(&mut doc_a), online).await;
    let invite = sync_a.invite(Some(Role::Editor), 7, 1).unwrap();
    client_a
        .drive(&mut sync_a, Some(&mut doc_a), |s, _| {
            s.invite_result(&invite).is_some()
        })
        .await;
    let link = sync_a.invite_result(&invite).unwrap().unwrap();

    // B joins and writes.
    let b = Device::new(temp.path(), "b");
    let mut client_b = Client::new(addr);
    let (mut sync_b, mut doc_b) = join(&mut client_b, &link, &b, "Bob").await;
    let len = doc_b.len();
    doc_b.insert(len, " B wrote this.").unwrap();
    let written = "Hello. B wrote this.";
    client_b
        .drive(&mut sync_b, Some(&mut doc_b), |s, d| {
            online(s, d) && d.unwrap().text() == written
        })
        .await;
    client_a
        .drive(&mut sync_a, Some(&mut doc_a), |s, d| {
            online(s, d) && d.unwrap().text() == written
        })
        .await;
    let b_log = data
        .join("projects")
        .join(&project)
        .join(format!("main/{}.log", b.identity.device.as_str()));
    let records = record_count(&std::fs::read(&b_log).unwrap());

    // B loses its copy and opens A's "add my other device" link.
    client_b.disconnect(&mut sync_b).await;
    drop((sync_b, doc_b));
    std::fs::remove_dir_all(&b.dir).unwrap();
    let invite = sync_a.invite(None, 7, 1).unwrap();
    client_a
        .drive(&mut sync_a, Some(&mut doc_a), |s, _| {
            s.invite_result(&invite).is_some()
        })
        .await;
    let link = sync_a.invite_result(&invite).unwrap().unwrap();
    let mut client_b = Client::new(addr);
    let (mut sync_b, mut doc_b) = join(&mut client_b, &link, &b, "Bob").await;
    assert_eq!(doc_b.text(), written, "B's own earlier changes came back");
    assert_eq!(doc_b.identity().user, b.identity.user);
    assert_eq!(sync_b.role(), Some(Role::Editor));

    // A's edit after B's text reaches B.
    let len = doc_a.len();
    doc_a.insert(len, " A again.").unwrap();
    let expected = "Hello. B wrote this. A again.";
    client_a
        .drive(&mut sync_a, Some(&mut doc_a), |s, _| s.is_idle())
        .await;
    client_b
        .drive(&mut sync_b, Some(&mut doc_b), |s, d| {
            online(s, d) && d.unwrap().text() == expected
        })
        .await;
    // B uploaded nothing again.
    assert_eq!(record_count(&std::fs::read(&b_log).unwrap()), records);

    // B invites a device of its own (it is B's user again).
    let invite = sync_b.invite(None, 1, 1).unwrap();
    client_b
        .drive(&mut sync_b, Some(&mut doc_b), |s, _| {
            s.invite_result(&invite).is_some()
        })
        .await;
    assert!(sync_b.invite_result(&invite).unwrap().is_ok());
    running.stop().await;
}

/// The last record (an envelope) of a stream's log.
fn last_record(log: &[u8]) -> Vec<u8> {
    let mut at = 0;
    let mut last = Vec::new();
    while let Frame::Record(envelope, next) = parse_frame(log, at) {
        last = envelope.to_vec();
        at = next;
    }
    last
}

/// The epoch entries of a members log, as (epoch, signing device).
fn epoch_entries(log: &[u8]) -> Vec<(u32, String)> {
    let mut at = 0;
    let mut epochs = Vec::new();
    while let Frame::Record(_, next) = parse_frame(log, at) {
        let body = entry_body(&log[at..next]).unwrap();
        if let Op::Epoch { epoch, .. } = body.op {
            epochs.push((epoch, body.by));
        }
        at = next;
    }
    epochs
}

/// Removing a member starts a new key epoch: the remover writes it, new
/// records use it, the removed device can't open them, the others can; a
/// leave makes a remaining device rotate; a join through an invite made
/// before a rotation reads every epoch; devices racing to rotate write one
/// epoch per removal.
#[tokio::test(flavor = "multi_thread")]
async fn removals_start_new_key_epochs() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("server");
    let running = lethora_server::start(Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        data_dir: data.clone(),
        public_url: None,
        ..Config::default()
    })
    .await
    .unwrap();
    let addr = running.addr;
    let url = running.public_url.clone();
    let epoch = |s: &ServerSync| s.members().unwrap().epoch();

    let a = Device::new(temp.path(), "a");
    let mut doc_a = Document::create(&a.dir, &a.identity).unwrap();
    doc_a.insert(0, "One.").unwrap();
    let (mut sync_a, _) = doc_a
        .server_share(&url, "Alice", a.keys.clone(), now())
        .unwrap();
    let project = sync_a.project().to_owned();
    let project_dir = data.join("projects").join(&project);
    let mut client_a = Client::new(addr);
    client_a.drive(&mut sync_a, Some(&mut doc_a), online).await;
    // An invite made before any rotation, used later.
    let late = invite_link(&mut client_a, &mut sync_a, &mut doc_a, Some(Role::Reader)).await;

    // B and C join as editors.
    let link = invite_link(&mut client_a, &mut sync_a, &mut doc_a, Some(Role::Editor)).await;
    let b = Device::new(temp.path(), "b");
    let mut client_b = Client::new(addr);
    let (mut sync_b, mut doc_b) = join(&mut client_b, &link, &b, "Bob").await;
    client_b.drive(&mut sync_b, Some(&mut doc_b), online).await;
    let link = invite_link(&mut client_a, &mut sync_a, &mut doc_a, Some(Role::Editor)).await;
    let c = Device::new(temp.path(), "c");
    let mut client_c = Client::new(addr);
    let (mut sync_c, mut doc_c) = join(&mut client_c, &link, &c, "Carol").await;
    client_c.drive(&mut sync_c, Some(&mut doc_c), online).await;
    knows(&mut client_a, &mut sync_a, &mut doc_a, &b.identity.user).await;
    knows(&mut client_a, &mut sync_a, &mut doc_a, &c.identity.user).await;
    assert_eq!(epoch(&sync_a), 1);

    // A removes B and starts epoch 2 right after.
    let id = sync_a.remove_user(b.identity.user.as_str()).unwrap();
    change_done(&mut client_a, &mut sync_a, &mut doc_a, id).await;
    client_a
        .drive(&mut sync_a, Some(&mut doc_a), |s, d| {
            online(s, d) && epoch(s) == 2 && s.keyring().unwrap().newest().0 == 2
        })
        .await;
    assert!(
        sync_a
            .take_notes()
            .iter()
            .any(|n| n.contains("Starting key epoch 2"))
    );
    client_b.drive(&mut sync_b, Some(&mut doc_b), removed).await;
    let b_keyring = sync_b.keyring().unwrap().clone();
    assert_eq!(b_keyring.newest().0, 1);

    // A's new records are sealed with epoch 2: B's keyring can't open them.
    doc_a.insert(4, " Two.").unwrap();
    client_a
        .drive(&mut sync_a, Some(&mut doc_a), |s, _| s.is_idle())
        .await;
    let a_name = a.identity.device.as_str();
    let a_log = std::fs::read(project_dir.join(format!("main/{a_name}.log"))).unwrap();
    let record = last_record(&a_log);
    assert_eq!(header_v2(&record).unwrap().epoch, 2);
    let context = Context {
        project: &project,
        document: "main",
        kind: "log",
        name: a_name,
    };
    let public = a.keys.public();
    assert!(open_v2(&b_keyring, &public, &context, &record).is_err());
    open_v2(sync_a.keyring().unwrap(), &public, &context, &record).unwrap();

    // C saw the removal too and raced A to rotate (refused as stale), then
    // took A's epoch: it reads everything.
    client_c
        .drive(&mut sync_c, Some(&mut doc_c), |s, d| {
            online(s, d) && d.unwrap().text() == "One. Two."
        })
        .await;
    assert_eq!(sync_c.keyring().unwrap().newest().0, 2);
    assert!(
        sync_c
            .take_notes()
            .iter()
            .any(|n| n.contains("Starting key epoch 2")),
        "C tried too"
    );

    // D joins through the invite made before the rotation: its keyring
    // holds epoch 1 from the invite and epoch 2 sealed to the invite.
    let d = Device::new(temp.path(), "d");
    let mut client_d = Client::new(addr);
    let (mut sync_d, mut doc_d) = join(&mut client_d, &late, &d, "Dan").await;
    assert_eq!(doc_d.text(), "One. Two.");
    let epochs: Vec<u32> = sync_d.keyring().unwrap().epochs().map(|(e, _)| e).collect();
    assert_eq!(epochs, [1, 2]);
    client_d.drive(&mut sync_d, Some(&mut doc_d), online).await;
    knows(&mut client_a, &mut sync_a, &mut doc_a, &d.identity.user).await;

    // C leaves: a remaining device (D, online first) starts epoch 3; A,
    // racing it later, takes D's.
    let id = sync_c.remove_device(c.identity.device.as_str()).unwrap();
    client_c
        .drive(&mut sync_c, Some(&mut doc_c), |s, _| {
            s.change_result(id).is_some() || s.is_stopped()
        })
        .await;
    assert_eq!(sync_c.status().state, ServerState::Removed);
    client_d
        .drive(&mut sync_d, Some(&mut doc_d), |s, d| {
            online(s, d) && epoch(s) == 3
        })
        .await;
    assert_eq!(sync_d.keyring().unwrap().newest().0, 3);
    client_a
        .drive(&mut sync_a, Some(&mut doc_a), |s, d| {
            online(s, d) && s.keyring().unwrap().newest().0 == 3
        })
        .await;

    // Exactly one epoch per removal, the second by D.
    let log = std::fs::read(project_dir.join("members.log")).unwrap();
    assert_eq!(
        epoch_entries(&log),
        [
            (2, a.identity.device.as_str().to_owned()),
            (3, d.identity.device.as_str().to_owned())
        ]
    );
    assert!(!Members::from_log(&log).members.unwrap().needs_rotation());

    // A edits in epoch 3; D (a reader) reads it all.
    let len = doc_a.len();
    doc_a.insert(len, " Three.").unwrap();
    client_a
        .drive(&mut sync_a, Some(&mut doc_a), |s, _| s.is_idle())
        .await;
    client_d
        .drive(&mut sync_d, Some(&mut doc_d), |_, d| {
            d.unwrap().text() == "One. Two. Three."
        })
        .await;
    let a_log = std::fs::read(project_dir.join(format!("main/{a_name}.log"))).unwrap();
    assert_eq!(header_v2(&last_record(&a_log)).unwrap().epoch, 3);

    // Reopened with only epoch 1, A finds the epochs sealed to it in its
    // members log.
    doc_a.server_save(&sync_a).unwrap();
    client_a.disconnect(&mut sync_a).await;
    drop(sync_a);
    let mut sync_a = doc_a.server_open(a.keys.clone(), b_keyring, None).unwrap();
    assert_eq!(sync_a.keyring().unwrap().newest().0, 3);
    client_a.drive(&mut sync_a, Some(&mut doc_a), online).await;
    running.stop().await;
}

/// Two devices see each other's selection through the server, a reader's
/// too; the one online first learns of the newcomer and answers.
#[tokio::test(flavor = "multi_thread")]
async fn devices_see_each_others_presence() {
    let temp = tempfile::tempdir().unwrap();
    let running = lethora_server::start(Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        data_dir: temp.path().join("server"),
        public_url: None,
        ..Config::default()
    })
    .await
    .unwrap();
    let addr = running.addr;
    let url = running.public_url.clone();

    let a = Device::new(temp.path(), "a");
    let mut doc_a = Document::create(&a.dir, &a.identity).unwrap();
    doc_a.insert(0, "Hello world.").unwrap();
    let (mut sync_a, _) = doc_a
        .server_share(&url, "Alice", a.keys.clone(), now())
        .unwrap();
    let mut client_a = Client::new(addr);
    client_a.drive(&mut sync_a, Some(&mut doc_a), online).await;
    // A is in the text before B comes.
    doc_a
        .server_set_presence(&mut sync_a, "author", 6, 11, now_ms())
        .unwrap();
    client_a.act(&mut sync_a, &mut Some(&mut doc_a)).await;

    let link = invite_link(&mut client_a, &mut sync_a, &mut doc_a, Some(Role::Reader)).await;
    let b = Device::new(temp.path(), "b");
    let mut client_b = Client::new(addr);
    let (mut sync_b, mut doc_b) = join(&mut client_b, &link, &b, "Bob").await;
    client_b.drive(&mut sync_b, Some(&mut doc_b), online).await;
    assert!(!doc_b.is_writable());
    doc_b
        .server_set_presence(&mut sync_b, "science", 0, 0, now_ms())
        .unwrap();
    client_b.act(&mut sync_b, &mut Some(&mut doc_b)).await;

    // A sees B, and answers with its own presence, which B then sees.
    client_a
        .drive(&mut sync_a, Some(&mut doc_a), |s, d| {
            d.unwrap().server_presence(s).len() == 1
        })
        .await;
    let seen = doc_a.server_presence(&sync_a);
    assert_eq!(seen[0].device, b.identity.device.as_str());
    assert_eq!(seen[0].user, b.identity.user.as_str());
    assert_eq!(seen[0].name.as_deref(), Some("Bob"));
    assert_eq!(seen[0].role, Role::Reader);
    assert_eq!(
        (seen[0].view.as_str(), seen[0].anchor, seen[0].head),
        ("science", Some(0), Some(0))
    );
    // The answer waits out PRESENCE_GAP_MS (100 ms) since A's last send,
    // which on a fast machine was just now (the app wakes on `presence_due`).
    tokio::time::sleep(Duration::from_millis(150)).await;
    client_a.act(&mut sync_a, &mut Some(&mut doc_a)).await;
    client_b
        .drive(&mut sync_b, Some(&mut doc_b), |s, d| {
            d.unwrap().server_presence(s).len() == 1
        })
        .await;
    let seen = doc_b.server_presence(&sync_b);
    assert_eq!(seen[0].name.as_deref(), Some("Alice"));
    assert_eq!(seen[0].role, Role::Owner);
    assert_eq!(
        (seen[0].view.as_str(), seen[0].anchor, seen[0].head),
        ("author", Some(6), Some(11))
    );

    // A moves; B follows.
    doc_a
        .server_set_presence(&mut sync_a, "publisher", 2, 2, now_ms() + 1_000)
        .unwrap();
    client_a.act(&mut sync_a, &mut Some(&mut doc_a)).await;
    client_b
        .drive(&mut sync_b, Some(&mut doc_b), |s, d| {
            d.unwrap()
                .server_presence(s)
                .first()
                .is_some_and(|p| p.view == "publisher" && p.head == Some(2))
        })
        .await;
    running.stop().await;
}

/// Two members both type; each sees who wrote what, with names, and itself
/// as "you".
#[tokio::test(flavor = "multi_thread")]
async fn members_see_who_wrote_what() {
    let temp = tempfile::tempdir().unwrap();
    let running = lethora_server::start(Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        data_dir: temp.path().join("server"),
        public_url: None,
        ..Config::default()
    })
    .await
    .unwrap();
    let addr = running.addr;
    let url = running.public_url.clone();

    let a = Device::new(temp.path(), "a");
    let mut doc_a = Document::create(&a.dir, &a.identity).unwrap();
    doc_a.insert(0, "Hello world.").unwrap();
    let (mut sync_a, _) = doc_a
        .server_share(&url, "Alice", a.keys.clone(), now())
        .unwrap();
    let mut client_a = Client::new(addr);
    client_a.drive(&mut sync_a, Some(&mut doc_a), online).await;
    let link = invite_link(&mut client_a, &mut sync_a, &mut doc_a, Some(Role::Editor)).await;
    let b = Device::new(temp.path(), "b");
    let mut client_b = Client::new(addr);
    let (mut sync_b, mut doc_b) = join(&mut client_b, &link, &b, "Bob").await;
    client_b.drive(&mut sync_b, Some(&mut doc_b), online).await;

    // B writes into A's text; A then adds to the end.
    doc_b.insert(5, ", 🌍").unwrap();
    let expected = "Hello, 🌍 world. Bye.";
    client_b
        .drive(&mut sync_b, Some(&mut doc_b), |s, _| s.is_idle())
        .await;
    client_a
        .drive(&mut sync_a, Some(&mut doc_a), |_, d| {
            d.unwrap().text() == "Hello, 🌍 world."
        })
        .await;
    let len = doc_a.len();
    doc_a.insert(len, " Bye.").unwrap();
    let converged =
        |s: &ServerSync, d: Option<&Document>| s.is_idle() && d.unwrap().text() == expected;
    client_a
        .drive(&mut sync_a, Some(&mut doc_a), converged)
        .await;
    client_b
        .drive(&mut sync_b, Some(&mut doc_b), converged)
        .await;

    let (ua, ub) = (a.identity.user.as_str(), b.identity.user.as_str());
    for (doc, sync, me) in [(&doc_a, &sync_a, ua), (&doc_b, &sync_b, ub)] {
        let authors = doc.server_authors(sync, 0, 999);
        let runs: Vec<_> = authors
            .runs
            .iter()
            .map(|r| (r.from, r.to, r.user.as_ref().map(|u| u.as_str())))
            .collect();
        // "Hello" A, ", 🌍" B (UTF-16 5..9), " world. Bye." A.
        assert_eq!(runs[..2], [(0, 5, Some(ua)), (5, 9, Some(ub))]);
        assert!(runs[2..].iter().all(|r| r.2 == Some(ua)), "{runs:?}");
        assert_eq!(runs.last().unwrap().1, 21);
        let mut legend: Vec<_> = authors
            .authors
            .iter()
            .map(|p| (p.user.as_str(), p.name.as_deref(), p.you))
            .collect();
        legend.sort();
        let mut want = vec![(ua, Some("Alice"), ua == me), (ub, Some("Bob"), ub == me)];
        want.sort();
        assert_eq!(legend, want);
        assert_eq!(
            authors
                .authors
                .iter()
                .find(|p| p.user == ua)
                .unwrap()
                .colour,
            sync.profile_of(ua).unwrap().colour
        );
        // Only the range asked for (widened to the whole 🌍), and its authors.
        let part = doc.server_authors(sync, 6, 8);
        assert_eq!(part.runs.len(), 1);
        assert_eq!((part.runs[0].from, part.runs[0].to), (6, 9));
        assert_eq!(part.authors.len(), 1);
        assert_eq!(part.authors[0].name.as_deref(), Some("Bob"));
    }
    running.stop().await;
}

// ---- Review over the server, roles per document, grounded authors ----

/// A server in `temp`, and its data folder.
async fn server(temp: &Path) -> (Running, PathBuf) {
    let data = temp.join("server");
    let running = lethora_server::start(Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        data_dir: data.clone(),
        public_url: None,
        ..Config::default()
    })
    .await
    .unwrap();
    (running, data)
}

/// A member device in a test: its device, transport, sync and document.
struct Member {
    device: Device,
    client: Client,
    sync: ServerSync,
    doc: Document,
}

impl Member {
    /// Drives until `done` holds.
    async fn until(&mut self, done: impl Fn(&ServerSync, Option<&Document>) -> bool) {
        self.client
            .drive(&mut self.sync, Some(&mut self.doc), done)
            .await;
    }

    /// Drives until online with nothing in flight.
    async fn settle(&mut self) {
        self.until(online).await;
    }

    /// Drives until online, idle and `done(doc)`.
    async fn until_doc(&mut self, done: impl Fn(&Document) -> bool) {
        self.until(|s, d| online(s, d) && done(d.unwrap())).await;
    }

    async fn offline(&mut self) {
        self.client.disconnect(&mut self.sync).await;
    }

    fn user(&self) -> &UserId {
        &self.device.identity.user
    }

    fn id(&self) -> &str {
        self.device.identity.device.as_str()
    }

    /// Sends `message` as is (bypassing `Document`) and returns the
    /// server's `Ack` or `Nack`, handing anything else to the sync.
    async fn append_raw(&mut self, message: Message) -> Message {
        let ws = self.client.ws.as_mut().expect("connected");
        let bytes = message.encode().unwrap();
        ws.send(WsMessage::Binary(bytes.into())).await.unwrap();
        loop {
            match tokio::time::timeout(WAIT, ws.next()).await.unwrap() {
                Some(Ok(WsMessage::Binary(bytes))) => match Message::decode(&bytes).unwrap() {
                    m @ (Message::Ack { .. } | Message::Nack { .. }) => return m,
                    other => self.sync.handle(other),
                },
                Some(Ok(_)) => {}
                other => panic!("connection ended: {other:?}"),
            }
        }
    }
}

/// A project holding `text` and what `prepare` adds, shared on `running` by
/// device "a" (Alice), online.
async fn share(
    temp: &Path,
    running: &Running,
    text: &str,
    prepare: impl FnOnce(&mut Document),
) -> Member {
    let device = Device::new(temp, "a");
    let mut doc = Document::create(&device.dir, &device.identity).unwrap();
    doc.insert(0, text).unwrap();
    prepare(&mut doc);
    let (sync, _) = doc
        .server_share(&running.public_url, "Alice", device.keys.clone(), now())
        .unwrap();
    let mut owner = Member {
        device,
        client: Client::new(running.addr),
        sync,
        doc,
    };
    owner.settle().await;
    owner
}

/// A new device `name` that joins `owner`'s project with `role`, online.
async fn invite(owner: &mut Member, temp: &Path, name: &str, role: Role) -> Member {
    let link = invite_link(
        &mut owner.client,
        &mut owner.sync,
        &mut owner.doc,
        Some(role),
    )
    .await;
    let device = Device::new(temp, name);
    let mut client = Client::new(owner.client.addr);
    let (sync, doc) = join(&mut client, &link, &device, name).await;
    let mut member = Member {
        device,
        client,
        sync,
        doc,
    };
    member.settle().await;
    let user = member.user().clone();
    knows(&mut owner.client, &mut owner.sync, &mut owner.doc, &user).await;
    member
}

/// Owner `owner` gives `member` the role `role` while `member` is offline.
async fn set_role(owner: &mut Member, member: &mut Member, role: Role) {
    member.offline().await;
    let id = owner.sync.set_role(member.user().as_str(), role).unwrap();
    change_done(&mut owner.client, &mut owner.sync, &mut owner.doc, id).await;
}

/// The Loro document in `device`'s own plain files of `document`.
fn local_doc(device: &Device, document: &str) -> LoroDoc {
    let doc = LoroDoc::new();
    let dir = device.dir.join(document);
    let name = device.identity.device.as_str();
    if let Ok(snapshot) = std::fs::read(dir.join(format!("{name}.snapshot"))) {
        doc.import(&snapshot).unwrap();
    }
    if let Ok(log) = std::fs::read(dir.join(format!("{name}.log"))) {
        let mut at = 0;
        while let Frame::Record(payload, next) = parse_frame(&log, at) {
            doc.import(payload).unwrap();
            at = next;
        }
    }
    doc
}

/// What `change` adds to `member`'s `document` as a raw Loro update, made in
/// a copy of its local files under Loro peer `peer`: what a device bypassing
/// `Document` could send.
fn forge(
    member: &mut Member,
    document: &str,
    peer: PeerID,
    change: impl FnOnce(&LoroDoc),
) -> Vec<u8> {
    member.doc.server_save(&member.sync).unwrap();
    let doc = local_doc(&member.device, document);
    let before = doc.oplog_vv();
    doc.set_peer_id(peer).unwrap();
    change(&doc);
    doc.commit();
    doc.export(ExportMode::updates(&before)).unwrap()
}

/// `update` as a framed record of `member`'s stream of `document`, sealed
/// with its newest key and signed by its device key.
fn sealed(member: &Member, document: &str, update: &[u8]) -> Vec<u8> {
    let context = Context {
        project: member.sync.project(),
        document,
        kind: "log",
        name: member.id(),
    };
    let envelope = seal_v2(
        member.sync.keyring().unwrap(),
        member.id(),
        &member.device.keys,
        &context,
        1,
        update,
    )
    .unwrap();
    frame(&envelope).unwrap()
}

/// A `peers` entry naming `user` and `device`.
fn peer_entry(user: &UserId, device: &DeviceId) -> LoroValue {
    LoroValue::from(HashMap::from([
        ("user", LoroValue::from(user.as_str())),
        ("device", LoroValue::from(device.as_str())),
    ]))
}

/// The users of `doc`'s review items, in order.
fn item_authors(doc: &Document) -> Vec<Option<String>> {
    doc.review_items()
        .iter()
        .map(|i| i.author.as_ref().map(|a| a.user.as_str().to_owned()))
        .collect()
}

/// Who wrote the character at the start of `needle` in `doc`'s text.
fn author_of(doc: &Document, needle: &str) -> Option<UserId> {
    let text = doc.text();
    let at = text.find(needle).expect("in the text");
    let utf16 = text[..at].encode_utf16().count();
    doc.author_at(utf16).unwrap().map(|a| a.user)
}

/// Every role writes exactly the documents it may: through `Document` (read
/// only otherwise), as the server accepts it (`Ack` or `Forbidden`), and as
/// the others import it, also when a refused record reaches them some other
/// way.
#[tokio::test(flavor = "multi_thread")]
async fn every_role_writes_just_its_documents() {
    let temp = tempfile::tempdir().unwrap();
    let (running, data) = server(temp.path()).await;
    let mut a = share(temp.path(), &running, "Hello world.", |doc| {
        doc.add_comment(0, 5, "A, before sharing.").unwrap();
    })
    .await;
    let project_dir = data.join("projects").join(a.sync.project());
    let mut e = invite(&mut a, temp.path(), "e", Role::Editor).await;
    let mut r = invite(&mut a, temp.path(), "r", Role::Reviewer).await;
    let mut d = invite(&mut a, temp.path(), "d", Role::Reader).await;
    let ua = a.user().as_str().to_owned();
    // The comment from before sharing came with the owner's review snapshot.
    for m in [&e, &r, &d] {
        assert_eq!(item_authors(&m.doc), [Some(ua.clone())]);
    }

    // Each writes what its role lets it.
    a.doc.insert(0, "A: ").unwrap();
    a.doc.add_comment(0, 1, "A.").unwrap();
    let len = e.doc.len();
    e.doc.insert(len, " E.").unwrap();
    e.doc.add_comment(3, 8, "E.").unwrap();
    let read_only = |r: lethora_core::Result<()>| matches!(r, Err(Error::ReadOnly(_)));
    assert!(read_only(r.doc.insert(0, "R")));
    r.doc.add_comment(6, 11, "R.").unwrap();
    assert!(read_only(d.doc.insert(0, "D")));
    assert!(read_only(d.doc.add_comment(0, 1, "D.").map(drop)));
    for m in [&mut a, &mut e, &mut r, &mut d] {
        m.settle().await;
    }
    let mut want = vec![
        Some(ua.clone()),
        Some(ua.clone()),
        Some(e.user().as_str().to_owned()),
        Some(r.user().as_str().to_owned()),
    ];
    want.sort();
    for m in [&mut a, &mut e, &mut r, &mut d] {
        m.until_doc(|doc| doc.text() == "A: Hello world. E." && doc.review_items().len() == 4)
            .await;
        let mut authors = item_authors(&m.doc);
        authors.sort();
        assert_eq!(authors, want);
    }
    // The server has a stream per document a role writes, and no other.
    let stream = |document: &str, m: &Member| {
        project_dir
            .join(format!("{document}/{}.log", m.id()))
            .is_file()
    };
    assert!(stream("main", &e) && stream("review", &e));
    assert!(!stream("main", &r) && stream("review", &r));
    assert!(!stream("main", &d) && !stream("review", &d));

    // Records made bypassing `Document`, every role × both documents: the
    // server takes exactly what the role writes.
    let mut refused = Vec::new();
    for m in [&mut a, &mut e, &mut r, &mut d] {
        let role = m.sync.role().unwrap();
        for (i, document) in ["main", "review"].into_iter().enumerate() {
            let update = forge(m, document, 0x7e57_0000 + i as u64, |doc| {
                if document == "main" {
                    doc.get_text("content").insert(0, "FORBIDDEN ").unwrap();
                } else {
                    doc.get_map("x-test").insert("by", "raw").unwrap();
                }
            });
            let records = sealed(m, document, &update);
            let answer = m
                .append_raw(Message::Append {
                    document: document.into(),
                    batch: 1_000_000,
                    records: records.clone(),
                })
                .await;
            if role.writes(document) {
                assert!(
                    matches!(answer, Message::Ack { .. }),
                    "{role:?} {document}: {answer:?}"
                );
            } else {
                assert!(
                    matches!(
                        answer,
                        Message::Nack {
                            code: Code::Forbidden,
                            ..
                        }
                    ),
                    "{role:?} {document}: {answer:?}"
                );
                refused.push((document, m.id().to_owned(), records));
            }
        }
    }
    assert_eq!(refused.len(), 3, "the reviewer's main, the reader's both");
    // Delivered some other way, the refused records reach no document.
    let before = (a.doc.text(), a.doc.review_items());
    for (document, device, records) in refused {
        a.sync.handle(Message::Records {
            document: document.into(),
            device,
            generation: 0,
            offset: 0,
            bytes: records,
        });
        let pulled = a.doc.server_pull(&mut a.sync).unwrap();
        assert!(!pulled.changed && !pulled.review, "{document}: {pulled:?}");
        assert!(
            pulled.skipped.iter().any(|s| s.contains("may not write")),
            "{:?}",
            pulled.skipped
        );
    }
    assert_eq!((a.doc.text(), a.doc.review_items()), before);
    running.stop().await;
}

/// Live role changes both ways, Reviewer → Editor → Reader → Reviewer →
/// Editor, each with changes made offline under the old role: nothing is
/// lost, nothing goes out the new role doesn't write, and it goes out once
/// a role writes it again. A device that has caught up since a demotion
/// takes nothing more from the demoted stream.
#[tokio::test(flavor = "multi_thread")]
async fn role_changes_keep_unpushed_changes_until_allowed() {
    let temp = tempfile::tempdir().unwrap();
    let (running, data) = server(temp.path()).await;
    let mut a = share(temp.path(), &running, "Hello.", |_| {}).await;
    let project_dir = data.join("projects").join(a.sync.project());
    let mut r = invite(&mut a, temp.path(), "r", Role::Reviewer).await;
    let r_main = project_dir.join(format!("main/{}.log", r.id()));
    // Sorted: same range, same second.
    let comments = |doc: &Document| -> Vec<String> {
        let mut texts: Vec<String> = doc
            .review_items()
            .iter()
            .map(|i| i.messages[0].text.clone())
            .collect();
        texts.sort();
        texts
    };

    // A reviewer's comment made offline goes out once it is an editor.
    r.offline().await;
    r.doc.add_comment(0, 5, "First.").unwrap();
    set_role(&mut a, &mut r, Role::Editor).await;
    r.until(|s, d| online(s, d) && s.role() == Some(Role::Editor))
        .await;
    a.until_doc(|doc| comments(doc) == ["First."]).await;

    // An editor's text and comment made offline stay on it as a reader.
    r.offline().await;
    let len = r.doc.len();
    r.doc.insert(len, " R.").unwrap();
    r.doc.add_comment(0, 5, "Second.").unwrap();
    set_role(&mut a, &mut r, Role::Reader).await;
    r.until(|s, d| online(s, d) && s.role() == Some(Role::Reader))
        .await;
    r.doc.server_save(&r.sync).unwrap();
    assert!(!r.doc.is_writable() && !r.doc.is_review_writable());
    assert_eq!(r.doc.text(), "Hello. R.");
    assert_eq!(comments(&r.doc), ["First.", "Second."]);
    assert!(!r_main.exists(), "nothing of main went out");
    assert_eq!(r.sync.status().message, None, "nothing was refused");
    a.settle().await;
    assert_eq!(a.doc.text(), "Hello.");
    assert_eq!(comments(&a.doc), ["First."]);

    // A reviewer again: the comment goes out, the text still waits.
    set_role(&mut a, &mut r, Role::Reviewer).await;
    r.until(|s, d| online(s, d) && s.role() == Some(Role::Reviewer))
        .await;
    a.until_doc(|doc| comments(doc) == ["First.", "Second."])
        .await;
    assert_eq!(a.doc.text(), "Hello.");
    assert!(!r_main.exists());

    // An editor again: the text goes out too, and writing goes on.
    set_role(&mut a, &mut r, Role::Editor).await;
    r.until(|s, d| online(s, d) && s.role() == Some(Role::Editor))
        .await;
    a.until_doc(|doc| doc.text() == "Hello. R.").await;
    r.doc.insert(0, "Yes. ").unwrap();
    r.settle().await;
    a.until_doc(|doc| doc.text() == "Yes. Hello. R.").await;
    assert_eq!(author_of(&a.doc, "Yes."), Some(r.user().clone()));

    // A reader now. Once the owner has caught up since, a main record of
    // its, delivered some other way, is not taken any more.
    set_role(&mut a, &mut r, Role::Reader).await;
    a.offline().await;
    a.settle().await;
    let update = forge(&mut r, "main", 0x7e57_1000, |doc| {
        doc.get_text("content").insert(0, "LATE ").unwrap();
    });
    let offset = std::fs::metadata(&r_main).unwrap().len();
    let generation = r.sync.settings().own.generation;
    a.sync.handle(Message::Records {
        document: "main".into(),
        device: r.id().to_owned(),
        generation,
        offset,
        bytes: sealed(&r, "main", &update),
    });
    let pulled = a.doc.server_pull(&mut a.sync).unwrap();
    assert!(!pulled.changed, "{pulled:?}");
    assert!(pulled.skipped.iter().any(|s| s.contains("may not write")));
    assert_eq!(a.doc.text(), "Yes. Hello. R.");
    running.stop().await;
}

/// Review items reach a reader live, a joining device (also from a
/// compacted review stream's snapshot) and a device that lost its copy,
/// always with their authors; a device rejoined keeps its author's rights.
#[tokio::test(flavor = "multi_thread")]
async fn review_items_reach_readers_joiners_and_rejoiners() {
    let temp = tempfile::tempdir().unwrap();
    let (running, _) = server(temp.path()).await;
    let mut a = share(temp.path(), &running, "Hello world, all of it.", |_| {}).await;
    let mut e = invite(&mut a, temp.path(), "e", Role::Editor).await;
    let mut r = invite(&mut a, temp.path(), "r", Role::Reviewer).await;
    let mut d = invite(&mut a, temp.path(), "d", Role::Reader).await;

    // The reader sees a reviewer's comment arrive, as a review change only.
    let item = r.doc.add_comment(0, 5, "R notes.").unwrap();
    r.settle().await;
    d.client.drive(&mut d.sync, None, |s, _| !s.is_idle()).await;
    let pulled = d.doc.server_pull(&mut d.sync).unwrap();
    assert!(pulled.review && !pulled.changed, "{pulled:?}");
    let items = d.doc.review_items();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].author.as_ref().unwrap().user, *r.user());
    assert!(matches!(d.doc.reply(item, "No."), Err(Error::ReadOnly(_))));

    // More of every kind, by everyone who writes review.
    a.until_doc(|doc| doc.review_items().len() == 1).await;
    a.doc.reply(item, "A answers.").unwrap();
    a.doc.add_mark(6, 11, "yellow").unwrap();
    a.settle().await;
    // E's review stream compacts on its second push.
    e.doc.set_compaction_threshold(1);
    e.doc.suggest_change(12, 15, "most").unwrap();
    e.settle().await;
    e.doc.add_comment(16, 18, "E, compacted.").unwrap();
    e.until(|s, d| online(s, d) && s.settings().review.own.generation >= 1)
        .await;
    let all = |doc: &Document| doc.review_items().len() == 4;
    for m in [&mut a, &mut e, &mut r, &mut d] {
        m.until_doc(all).await;
    }
    let authors = item_authors(&a.doc);
    assert!(authors.iter().all(Option::is_some), "{authors:?}");
    let replies = |doc: &Document| -> Vec<Option<UserId>> {
        doc.review_items()
            .iter()
            .find(|i| i.id == item)
            .unwrap()
            .messages
            .iter()
            .map(|m| m.author.as_ref().map(|a| a.user.clone()))
            .collect()
    };
    let expected = vec![Some(r.user().clone()), Some(a.user().clone())];
    assert_eq!(replies(&a.doc), expected);

    // A new device catches up, E's review from its snapshot.
    let mut j = invite(&mut a, temp.path(), "j", Role::Reader).await;
    j.until_doc(all).await;
    assert_eq!(item_authors(&j.doc), authors);
    assert_eq!(replies(&j.doc), expected);

    // R loses its copy and opens the project from the server again: its
    // comment is its own, so it may edit it.
    let keyring = r.sync.keyring().unwrap().clone();
    r.offline().await;
    let Member { device, client, .. } = r;
    std::fs::remove_dir_all(&device.dir).unwrap();
    let mut sync = ServerSync::reopen(
        &running.public_url,
        a.sync.project(),
        &device.identity.device,
        device.keys.clone(),
        &device.identity.user,
        keyring,
        None,
    )
    .unwrap();
    let mut client = Client { ..client };
    client.drive(&mut sync, None, |s, _| s.join_ready()).await;
    client.disconnect(&mut sync).await;
    let doc = Document::server_join(
        Box::new(FsStorage::new(&device.dir)),
        &mut sync,
        &device.identity,
    )
    .unwrap();
    let mut r = Member {
        device,
        client,
        sync,
        doc,
    };
    assert_eq!(item_authors(&r.doc), authors);
    assert_eq!(replies(&r.doc), expected);
    let first = r
        .doc
        .review_items()
        .into_iter()
        .find(|i| i.id == item)
        .unwrap();
    r.doc
        .edit_message(item, first.messages[0].id, "R notes, edited.")
        .unwrap();
    assert!(matches!(
        r.doc.edit_message(item, first.messages[1].id, "Not mine."),
        Err(Error::NotAuthor(_))
    ));
    r.settle().await;
    a.until_doc(|doc| {
        doc.review_items().iter().any(|i| {
            i.messages
                .first()
                .is_some_and(|m| m.text == "R notes, edited.")
        })
    })
    .await;
    running.stop().await;
}

/// After a member's removal starts a new key epoch, review records are
/// sealed with it and still open for the others.
#[tokio::test(flavor = "multi_thread")]
async fn review_records_use_the_newest_key_epoch() {
    let temp = tempfile::tempdir().unwrap();
    let (running, data) = server(temp.path()).await;
    let mut a = share(temp.path(), &running, "Hello.", |_| {}).await;
    let project_dir = data.join("projects").join(a.sync.project());
    let b = invite(&mut a, temp.path(), "b", Role::Editor).await;
    let mut c = invite(&mut a, temp.path(), "c", Role::Reviewer).await;
    let id = a.sync.remove_user(b.user().as_str()).unwrap();
    change_done(&mut a.client, &mut a.sync, &mut a.doc, id).await;
    a.until(|s, d| online(s, d) && s.keyring().unwrap().newest().0 == 2)
        .await;
    c.until(|s, d| online(s, d) && s.keyring().unwrap().newest().0 == 2)
        .await;
    c.doc.add_comment(0, 5, "After the removal.").unwrap();
    c.settle().await;
    a.until_doc(|doc| doc.review_items().len() == 1).await;
    assert_eq!(
        a.doc.review_items()[0].author.as_ref().unwrap().user,
        *c.user()
    );
    let log = std::fs::read(project_dir.join(format!("review/{}.log", c.id()))).unwrap();
    assert_eq!(header_v2(&last_record(&log)).unwrap().epoch, 2);
    running.stop().await;
}

/// `device`'s Loro peer in `doc` with the most ops, by the `peers` map.
fn busiest_peer(doc: &LoroDoc, device: &DeviceId) -> PeerID {
    let peers = doc.get_map("peers");
    let vv = doc.oplog_vv();
    let mut found = Vec::new();
    peers.for_each(|key, value| {
        if let ValueOrContainer::Value(LoroValue::Map(entry)) = value
            && entry.get("device") == Some(&LoroValue::from(device.as_str()))
        {
            let peer: PeerID = key.parse().unwrap();
            found.push((vv.get(&peer).copied().unwrap_or(0), peer));
        }
    });
    found.into_iter().max().expect("a peer of the device").1
}

/// Adds a message `text` to the thread of item `item` in review document
/// `doc`, the way `Document` does.
fn forge_reply(doc: &LoroDoc, item: ReviewId, text: &str) {
    let Some(ValueOrContainer::Container(Container::Map(map))) =
        doc.get_map("items").get(&item.0.to_string())
    else {
        panic!("no item {item:?}");
    };
    let Some(ValueOrContainer::Container(Container::List(list))) = map.get("messages") else {
        panic!("no messages");
    };
    let list: LoroList = list;
    let message = list.push_container(LoroMap::new()).unwrap();
    message.insert("id", 4242i64).unwrap();
    message.insert("created", 1i64).unwrap();
    let body = message.insert_container("body", LoroText::new()).unwrap();
    body.insert(0, text).unwrap();
}

/// A member device forges authorship in both documents: a `peers` entry
/// naming another user's device for a peer of its own, ops under that
/// device's own peer, and an entry claiming the other's review peer as its
/// own. None of it shows as the impersonated user (unknown instead), the
/// victim's own text stays its own and its later work too, and the forger
/// gains no author's rights.
#[tokio::test(flavor = "multi_thread")]
async fn forged_authors_stay_unknown() {
    let temp = tempfile::tempdir().unwrap();
    let (running, _) = server(temp.path()).await;
    let mut a = share(temp.path(), &running, "Hello.", |_| {}).await;
    let mut v = invite(&mut a, temp.path(), "v", Role::Editor).await;
    let mut f = invite(&mut a, temp.path(), "f", Role::Editor).await;
    let (uv, dv) = (v.user().clone(), v.device.identity.device.clone());
    let (uf, df) = (f.user().clone(), f.device.identity.device.clone());
    let ua = a.user().clone();

    // V writes and comments; everyone has it.
    let len = v.doc.len();
    v.doc.insert(len, " Victim text.").unwrap();
    let item = v.doc.add_comment(0, 5, "V's comment.").unwrap();
    v.settle().await;
    let has_victim =
        |doc: &Document| doc.text().contains("Victim") && doc.review_items().len() == 1;
    a.until_doc(has_victim).await;
    f.until_doc(has_victim).await;
    v.doc.server_save(&v.sync).unwrap();
    let v_main = busiest_peer(&local_doc(&v.device, "main"), &dv);
    let v_review = busiest_peer(&local_doc(&v.device, "review"), &dv);

    // F forges, in records of its own streams (the server accepts them: F
    // is an editor).
    let (euv, edv) = (uv.clone(), dv.clone());
    let main = forge(&mut f, "main", 0x5eed_0001, |doc| {
        doc.get_map("peers")
            .insert(&doc.peer_id().to_string(), peer_entry(&euv, &edv))
            .unwrap();
        doc.get_text("content").insert(0, "FORGED ").unwrap();
    });
    let impostor = forge(&mut f, "main", v_main, |doc| {
        let text = doc.get_text("content");
        text.insert(text.len_unicode(), " IMPOSTOR").unwrap();
    });
    let (euv, edv, euf, edf) = (uv.clone(), dv.clone(), uf.clone(), df.clone());
    let review = forge(&mut f, "review", 0x5eed_0002, |doc| {
        let peers = doc.get_map("peers");
        peers
            .insert(&doc.peer_id().to_string(), peer_entry(&euv, &edv))
            .unwrap();
        forge_reply(doc, item, "Forged reply.");
        // V's review peer claimed as F's.
        peers
            .insert(&v_review.to_string(), peer_entry(&euf, &edf))
            .unwrap();
    });
    let review_impostor = forge(&mut f, "review", v_review, |doc| {
        forge_reply(doc, item, "Impostor reply.");
    });
    let mut records = sealed(&f, "main", &main);
    records.extend(sealed(&f, "main", &impostor));
    let answer = f
        .append_raw(Message::Append {
            document: "main".into(),
            batch: 1_000_000,
            records,
        })
        .await;
    assert!(matches!(answer, Message::Ack { .. }), "{answer:?}");
    let mut records = sealed(&f, "review", &review);
    records.extend(sealed(&f, "review", &review_impostor));
    let answer = f
        .append_raw(Message::Append {
            document: "review".into(),
            batch: 1_000_001,
            records,
        })
        .await;
    assert!(matches!(answer, Message::Ack { .. }), "{answer:?}");

    // A and V import it all, attributing none of it to V (or F).
    let forged = |doc: &Document| {
        doc.text().contains("FORGED")
            && doc.text().contains("IMPOSTOR")
            && doc.review_items()[0].messages.len() == 3
    };
    for m in [&mut a, &mut v] {
        m.until_doc(forged).await;
        let doc = &m.doc;
        assert_eq!(author_of(doc, "FORGED"), None);
        assert_eq!(author_of(doc, "IMPOSTOR"), None);
        assert_eq!(author_of(doc, "Victim"), Some(uv.clone()));
        assert_eq!(author_of(doc, "Hello"), Some(ua.clone()));
        let thread = &doc.review_items()[0];
        let authors: Vec<_> = thread.messages.iter().map(|m| m.author.clone()).collect();
        assert!(authors.iter().all(Option::is_none), "{authors:?}");
        assert_eq!(thread.author, None);
    }
    // V goes on under a fresh peer: its new work is its own again.
    let len = v.doc.len();
    v.doc.insert(len, " Still V.").unwrap();
    let second = v.doc.add_comment(0, 5, "V again.").unwrap();
    v.settle().await;
    a.until_doc(|doc| doc.text().contains("Still V") && doc.review_items().len() == 2)
        .await;
    assert_eq!(author_of(&a.doc, "Still V"), Some(uv.clone()));
    let again = a
        .doc
        .review_items()
        .into_iter()
        .find(|i| i.id == second)
        .unwrap();
    assert_eq!(again.author.unwrap().user, uv);

    // F, rejoined from the server (so its copy holds its own forgeries),
    // can't edit V's comment it claimed.
    let keyring = f.sync.keyring().unwrap().clone();
    f.offline().await;
    let Member { device, client, .. } = f;
    std::fs::remove_dir_all(&device.dir).unwrap();
    let mut sync = ServerSync::reopen(
        &running.public_url,
        a.sync.project(),
        &device.identity.device,
        device.keys.clone(),
        &device.identity.user,
        keyring,
        None,
    )
    .unwrap();
    let mut client = client;
    client.drive(&mut sync, None, |s, _| s.join_ready()).await;
    client.disconnect(&mut sync).await;
    let mut doc = Document::server_join(
        Box::new(FsStorage::new(&device.dir)),
        &mut sync,
        &device.identity,
    )
    .unwrap();
    let thread = doc
        .review_items()
        .into_iter()
        .find(|i| i.id == item)
        .unwrap();
    assert_eq!(thread.author, None);
    assert!(matches!(
        doc.edit_message(item, thread.messages[0].id, "Hijacked."),
        Err(Error::NotAuthor(_))
    ));
    running.stop().await;
}
