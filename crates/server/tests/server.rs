//! In-process tests: a real server on `127.0.0.1:0` with a temporary data
//! directory, and a small client built on `lethora-protocol`.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use lethora_protocol::envelope::{Context, Keyring, seal_v2};
use lethora_protocol::frame::frame;
use lethora_protocol::invite::InviteLink;
use lethora_protocol::keys::{DeviceKeys, Signer};
use lethora_protocol::members::{Members, Op, Role};
use lethora_protocol::wire::{
    ClientVersion, Code, DeviceVersion, Hello, Message, NONCE_LEN, Position, deletion_body,
};
use lethora_protocol::{LEVEL, random, random_id};
use lethora_server::config::{Config, Limits};
use lethora_server::{Running, VERSION, now, start};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// The public URL every test server uses (clients sign it).
const URL: &str = "http://localhost:1";
const WAIT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------- server

struct Server {
    running: Option<Running>,
    addr: SocketAddr,
    dir: PathBuf,
    _temp: tempfile::TempDir,
}

fn config(dir: &Path, limits: &impl Fn(&mut Limits)) -> Config {
    let mut config = Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        data_dir: dir.to_owned(),
        public_url: Some(URL.into()),
        ..Config::default()
    };
    limits(&mut config.limits);
    config
}

impl Server {
    async fn start() -> Self {
        Self::with(|_| {}).await
    }

    async fn with(limits: impl Fn(&mut Limits)) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("data");
        let running = start(config(&dir, &limits)).await.unwrap();
        assert_eq!(running.public_url, URL);
        Self {
            addr: running.addr,
            running: Some(running),
            dir,
            _temp: temp,
        }
    }

    /// Stops the server (unless stopped) and starts a new one on the same
    /// data directory.
    async fn restart(&mut self) {
        if let Some(running) = self.running.take() {
            running.stop().await;
        }
        let running = start(config(&self.dir, &|_| {})).await.unwrap();
        self.addr = running.addr;
        self.running = Some(running);
    }

    async fn stop(mut self) {
        self.running.take().unwrap().stop().await;
    }

    fn project_dir(&self, project: &str) -> PathBuf {
        self.dir.join("projects").join(project)
    }

    async fn connect(&self) -> Client {
        Client::connect(self.addr).await
    }

    async fn http(&self, method: &str, path: &str, token: Option<&str>, body: &[u8]) -> Http {
        http(self.addr, method, path, token, body).await
    }
}

// ---------------------------------------------------------------- http

struct Http {
    status: u16,
    head: String,
    body: Vec<u8>,
}

/// One HTTP/1.1 request on a new connection.
async fn http(
    addr: SocketAddr,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: &[u8],
) -> Http {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some(token) = token {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.write_all(body).await.unwrap();
    let mut response = Vec::new();
    let _ = tokio::time::timeout(WAIT, stream.read_to_end(&mut response))
        .await
        .expect("an HTTP response in time");
    let split = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("a header");
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
    let status = head[9..12].parse().unwrap();
    Http { status, head, body }
}

// ---------------------------------------------------------------- client

struct Client {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    nonce: [u8; NONCE_LEN],
}

impl Client {
    async fn connect(addr: SocketAddr) -> Self {
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/v1/ws"))
            .await
            .unwrap();
        let mut client = Self {
            ws,
            nonce: [0; NONCE_LEN],
        };
        match client.next().await {
            Message::Challenge { nonce, server_time } => {
                assert!(server_time.abs_diff(now()) < 5);
                client.nonce = nonce;
            }
            other => panic!("expected a challenge, got {other:?}"),
        }
        client
    }

    async fn send(&mut self, message: &Message) {
        self.send_raw(message.encode().unwrap()).await;
    }

    async fn send_raw(&mut self, bytes: Vec<u8>) {
        self.ws.send(WsMessage::Binary(bytes.into())).await.unwrap();
    }

    /// The next message, or `None` once the connection closes.
    async fn try_next(&mut self) -> Option<Message> {
        loop {
            let frame = tokio::time::timeout(WAIT, self.ws.next())
                .await
                .expect("a message in time");
            match frame {
                Some(Ok(WsMessage::Binary(bytes))) => {
                    return Some(Message::decode(&bytes).unwrap());
                }
                Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_))) => {}
                Some(Ok(WsMessage::Close(_))) | None | Some(Err(_)) => return None,
                Some(Ok(other)) => panic!("unexpected frame {other:?}"),
            }
        }
    }

    async fn next(&mut self) -> Message {
        self.try_next().await.expect("a message, not the end")
    }

    /// The first message `pick` accepts, skipping others.
    async fn until<T>(&mut self, mut pick: impl FnMut(Message) -> Option<T>) -> T {
        loop {
            if let Some(found) = pick(self.next().await) {
                return found;
            }
        }
    }

    /// Expects `Refused` with `code`, then the end of the connection.
    async fn refused(&mut self, code: Code) {
        match self.next().await {
            Message::Refused { code: got, .. } => assert_eq!(got, code),
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert_eq!(self.try_next().await, None);
    }

    /// Sends `hello` signed by `signer`; returns the reply.
    async fn hello(&mut self, hello: Hello, signer: &dyn Signer) -> Message {
        self.hello_at(hello, LEVEL, signer).await
    }

    /// Sends `hello` of a client at `level` signed by `signer`; returns the
    /// reply.
    async fn hello_at(&mut self, hello: Hello, level: u32, signer: &dyn Signer) -> Message {
        let client = ClientVersion {
            level,
            app: format!("test {level}"),
        };
        let message = hello.sign(client, signer, &self.nonce, URL).unwrap();
        self.send(&message).await;
        self.next().await
    }

    /// Everything up to `Synced` after subscribing.
    async fn subscribe(&mut self, members: u64, streams: Vec<Position>) -> Vec<Message> {
        self.send(&Message::Subscribe { members, streams }).await;
        let mut got = Vec::new();
        loop {
            match self.next().await {
                Message::Synced => return got,
                message => got.push(message),
            }
        }
    }

    /// Checks that nothing else is queued: a ping is answered right away.
    async fn nothing_pending(&mut self) {
        self.send(&Message::Ping(99)).await;
        assert_eq!(self.next().await, Message::Pong(99));
    }
}

// ---------------------------------------------------------------- project

struct Dev {
    id: String,
    keys: DeviceKeys,
}

impl Dev {
    fn new() -> Self {
        Self {
            id: random_id().unwrap(),
            keys: DeviceKeys::generate().unwrap(),
        }
    }

    fn hello(&self, project: &str) -> Hello {
        Hello::Device {
            project: project.into(),
            device: self.id.clone(),
        }
    }
}

/// A project as the test knows it: the owner, the members log and the key.
struct Fixture {
    project: String,
    owner: Dev,
    members: Members,
    log: Vec<u8>,
    keyring: Keyring,
    key: [u8; 32],
}

/// A welcomed connection.
struct Session {
    client: Client,
    token: String,
}

fn welcome(reply: Message) -> (String, Option<Role>) {
    match reply {
        Message::Welcome { token, role, .. } => (token, role),
        other => panic!("expected a welcome, got {other:?}"),
    }
}

impl Fixture {
    /// Creates a project on `server`; returns it and the owner's session.
    async fn create(server: &Server) -> (Self, Session) {
        let owner = Dev::new();
        let project = random_id().unwrap();
        let user = random_id().unwrap();
        let log = Members::genesis_entry(&project, &user, &owner.id, &owner.keys, now()).unwrap();
        let members = Members::genesis(&log).unwrap();
        let mut client = server.connect().await;
        let reply = client
            .hello(
                Hello::Create {
                    genesis: log.clone(),
                },
                &owner.keys,
            )
            .await;
        let (token, role) = welcome(reply);
        assert_eq!(role, Some(Role::Owner));
        let key = random().unwrap();
        let fixture = Self {
            project,
            owner,
            members,
            log,
            keyring: Keyring::new(key),
            key,
        };
        (fixture, Session { client, token })
    }

    /// A new members record by `dev`.
    fn entry(&self, dev: &Dev, op: Op) -> Vec<u8> {
        self.members
            .entry(&dev.id, now(), op, &[&dev.keys])
            .unwrap()
    }

    /// Applies a record the server accepted.
    fn accepted(&mut self, record: &[u8]) {
        self.members.apply(record).unwrap();
        self.log.extend_from_slice(record);
    }

    /// Appends members `record` through `session`; returns the reply.
    async fn append_members(&mut self, client: &mut Client, record: Vec<u8>) -> Message {
        client
            .send(&Message::AppendMembers {
                batch: 1,
                record: record.clone(),
            })
            .await;
        let reply = client
            .until(|m| matches!(m, Message::MembersAck { .. } | Message::Nack { .. }).then_some(m))
            .await;
        if matches!(reply, Message::MembersAck { .. }) {
            self.accepted(&record);
        }
        reply
    }

    /// A role invite by the owner (through `owner`, the owner's client).
    async fn invite(
        &mut self,
        owner: &mut Client,
        role: Role,
        expires: u64,
        uses: u32,
    ) -> InviteLink {
        let link = InviteLink::generate(URL, &self.project).unwrap();
        let op = Op::Invite {
            id: link.id.clone(),
            key: link.keys().verifying_key(),
            exchange: link.keys().exchange_key(),
            role: Some(role),
            user: None,
            expires,
            uses,
        };
        let record = self.entry(&self.owner, op);
        let reply = self.append_members(owner, record).await;
        assert!(matches!(reply, Message::MembersAck { .. }), "{reply:?}");
        link
    }

    /// The join record of a new device `dev` through `link` at `time`.
    fn join(&self, link: &InviteLink, dev: &Dev, time: u64) -> Vec<u8> {
        let op = Op::Join {
            invite: link.id.clone(),
            user: random_id().unwrap(),
            device: dev.id.clone(),
            public: dev.keys.public(),
            profile: None,
        };
        self.members
            .entry(&dev.id, time, op, &[&link.keys(), &dev.keys])
            .unwrap()
    }

    /// Connects as invitee of `link`.
    async fn invitee(&self, server: &Server, link: &InviteLink) -> Session {
        let mut client = server.connect().await;
        let hello = Hello::Invitee {
            project: self.project.clone(),
            invite: link.id.clone(),
        };
        let (token, role) = welcome(client.hello(hello, &link.keys()).await);
        assert_eq!(role, None);
        Session { client, token }
    }

    /// A new member with `role`: invite, invitee hello, join.
    async fn add(&mut self, server: &Server, owner: &mut Client, role: Role) -> Dev {
        let link = self.invite(owner, role, now() + 3600, 1).await;
        let mut invitee = self.invitee(server, &link).await;
        let dev = Dev::new();
        let record = self.join(&link, &dev, now());
        let reply = self.append_members(&mut invitee.client, record).await;
        assert!(matches!(reply, Message::MembersAck { .. }), "{reply:?}");
        dev
    }

    /// Connects device `dev`.
    async fn device(&self, server: &Server, dev: &Dev) -> Session {
        self.device_at(server, dev, LEVEL).await
    }

    /// Connects device `dev` as a client at `level`.
    async fn device_at(&self, server: &Server, dev: &Dev, level: u32) -> Session {
        let mut client = server.connect().await;
        let hello = dev.hello(&self.project);
        let (token, _) = welcome(client.hello_at(hello, level, &dev.keys).await);
        Session { client, token }
    }

    /// A framed log record with `payload` by `dev` for its `document`
    /// stream.
    fn record(&self, dev: &Dev, document: &str, payload: &[u8]) -> Vec<u8> {
        self.record_for(dev, document, &dev.id, payload)
    }

    /// A framed log record by `dev`, sealed for the stream of `name`.
    fn record_for(&self, dev: &Dev, document: &str, name: &str, payload: &[u8]) -> Vec<u8> {
        self.record_at(dev, document, name, 1, payload)
    }

    /// A framed log record by `dev` for the stream of `name`, needing
    /// `level`.
    fn record_at(
        &self,
        dev: &Dev,
        document: &str,
        name: &str,
        level: u32,
        payload: &[u8],
    ) -> Vec<u8> {
        let context = Context {
            project: &self.project,
            document,
            kind: "log",
            name,
        };
        frame(&seal_v2(&self.keyring, &dev.id, &dev.keys, &context, level, payload).unwrap())
            .unwrap()
    }

    fn sealed(&self, dev: &Dev, document: &str, kind: &str, name: &str, payload: &[u8]) -> Vec<u8> {
        let context = Context {
            project: &self.project,
            document,
            kind,
            name,
        };
        seal_v2(&self.keyring, &dev.id, &dev.keys, &context, 1, payload).unwrap()
    }
}

async fn append(client: &mut Client, document: &str, batch: u64, records: Vec<u8>) -> Message {
    client
        .send(&Message::Append {
            document: document.into(),
            batch,
            records,
        })
        .await;
    client
        .until(|m| matches!(m, Message::Ack { .. } | Message::Nack { .. }).then_some(m))
        .await
}

fn ack_length(reply: Message) -> u64 {
    match reply {
        Message::Ack { length, .. } => length,
        other => panic!("expected an ack, got {other:?}"),
    }
}

fn nack_code(reply: Message) -> Code {
    match reply {
        Message::Nack { code, .. } => code,
        other => panic!("expected a nack, got {other:?}"),
    }
}

/// Every file under `dir`, recursively.
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(files(&path));
        } else {
            out.push(path);
        }
    }
    out
}

// ---------------------------------------------------------------- tests

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_join_catch_up_and_live() {
    let server = Server::start().await;
    let (mut p, mut owner) = Fixture::create(&server).await;
    assert!(
        owner
            .client
            .subscribe(p.log.len() as u64, vec![])
            .await
            .is_empty()
    );

    let first = [
        p.record(&p.owner, "main", b"one"),
        p.record(&p.owner, "main", b"two"),
    ]
    .concat();
    let length = ack_length(append(&mut owner.client, "main", 1, first.clone()).await);
    assert_eq!(length, first.len() as u64);

    // A second device: role invite, wrapped key, invitee, join.
    let link = p
        .invite(&mut owner.client, Role::Editor, now() + 3600, 1)
        .await;
    let wrapped = link.keys().wrap(&p.project, &p.keyring).unwrap();
    let path = format!("/v1/projects/{}/invites/{}", p.project, link.id);
    assert_eq!(
        server
            .http("PUT", &path, Some(&owner.token), &wrapped)
            .await
            .status,
        204
    );
    let key_file = server
        .project_dir(&p.project)
        .join(format!("invites/{}.key", link.id));
    assert!(key_file.exists());

    let mut invitee = p.invitee(&server, &link).await;
    // The invitee reads the members log and the wrapped key.
    let got = invitee.client.subscribe(0, vec![]).await;
    let log: Vec<u8> = got
        .iter()
        .flat_map(|m| match m {
            Message::MembersRecords { bytes, .. } => bytes.clone(),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(log, p.log);
    let members = Members::from_log(&log).members.unwrap();
    assert_eq!(members.head(), p.members.head());
    let fetched = server.http("GET", &path, Some(&invitee.token), b"").await;
    assert_eq!(fetched.status, 200);
    assert_eq!(
        *link
            .keys()
            .unwrap(&p.project, &fetched.body)
            .unwrap()
            .newest()
            .1,
        p.key
    );
    // The invitee may not append to streams.
    let dev = Dev::new();
    invitee
        .client
        .send(&Message::Append {
            document: "main".into(),
            batch: 1,
            records: p.record(&dev, "main", b"x"),
        })
        .await;
    invitee.client.refused(Code::Forbidden).await;

    let mut invitee = p.invitee(&server, &link).await;
    let join = p.join(&link, &dev, now());
    let reply = p.append_members(&mut invitee.client, join.clone()).await;
    assert_eq!(reply, Message::MembersAck { batch: 1, seq: 2 });
    // Used up: the key is gone.
    assert!(!key_file.exists());
    assert_eq!(
        server
            .http("GET", &path, Some(&invitee.token), b"")
            .await
            .status,
        410
    );
    // The subscribed owner got the join live (and no copy of its invite).
    let live = owner.client.next().await;
    assert_eq!(
        live,
        Message::MembersRecords {
            offset: (p.log.len() - join.len()) as u64,
            bytes: join
        }
    );

    // The new device catches up.
    let mut second = p.device(&server, &dev).await;
    let got = second.client.subscribe(0, vec![]).await;
    let mut log = Vec::new();
    let mut records = Vec::new();
    for message in got {
        match message {
            Message::MembersRecords { offset, bytes } => {
                assert_eq!(offset, log.len() as u64);
                log.extend(bytes);
            }
            Message::Records {
                document,
                device,
                generation,
                offset,
                bytes,
            } => {
                assert_eq!((document.as_str(), generation, offset), ("main", 0, 0));
                assert_eq!(device, p.owner.id);
                records.extend(bytes);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(log, p.log);
    assert_eq!(records, first);

    // Live: a new append by the owner reaches the second device.
    let more = p.record(&p.owner, "main", b"three");
    append(&mut owner.client, "main", 2, more.clone()).await;
    assert_eq!(
        second.client.next().await,
        Message::Records {
            document: "main".into(),
            device: p.owner.id.clone(),
            generation: 0,
            offset: first.len() as u64,
            bytes: more.clone(),
        }
    );
    // And the second device's own append reaches the owner.
    let own = p.record(&dev, "review", b"note");
    assert_eq!(
        ack_length(append(&mut second.client, "review", 1, own.clone()).await),
        own.len() as u64
    );
    let live = owner.client.next().await;
    assert!(matches!(live, Message::Records { ref device, offset: 0, .. } if *device == dev.id));
    // On disk: exactly the records appended.
    let stored = std::fs::read(
        server
            .project_dir(&p.project)
            .join(format!("main/{}.log", p.owner.id)),
    )
    .unwrap();
    assert_eq!(stored, [first, more].concat());
    // A welcome lists the device's own streams.
    let mut again = server.connect().await;
    match again.hello(p.owner.hello(&p.project), &p.owner.keys).await {
        Message::Welcome { streams, .. } => {
            assert_eq!(streams.len(), 1);
            assert_eq!(
                (streams[0].document.as_str(), streams[0].batch),
                ("main", 2)
            );
            assert_eq!(streams[0].length, stored.len() as u64);
        }
        other => panic!("{other:?}"),
    }
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn roles_and_signatures() {
    let server = Server::start().await;
    let (mut p, mut owner) = Fixture::create(&server).await;
    let reader = p.add(&server, &mut owner.client, Role::Reader).await;
    let reviewer = p.add(&server, &mut owner.client, Role::Reviewer).await;
    let editor = p.add(&server, &mut owner.client, Role::Editor).await;

    let mut r = p.device(&server, &reader).await;
    let reply = append(&mut r.client, "main", 1, p.record(&reader, "main", b"x")).await;
    assert_eq!(nack_code(reply), Code::Forbidden);
    let reply = append(
        &mut r.client,
        "review",
        1,
        p.record(&reader, "review", b"x"),
    )
    .await;
    assert_eq!(nack_code(reply), Code::Forbidden);

    let mut v = p.device(&server, &reviewer).await;
    let reply = append(&mut v.client, "main", 1, p.record(&reviewer, "main", b"x")).await;
    assert_eq!(nack_code(reply), Code::Forbidden);
    let reply = append(
        &mut v.client,
        "review",
        1,
        p.record(&reviewer, "review", b"x"),
    )
    .await;
    assert!(matches!(reply, Message::Ack { .. }), "{reply:?}");

    // A stranger's hello.
    let stranger = Dev::new();
    let mut s = server.connect().await;
    let reply = s.hello(stranger.hello(&p.project), &stranger.keys).await;
    assert!(
        matches!(
            reply,
            Message::Refused {
                code: Code::NotAMember,
                ..
            }
        ),
        "{reply:?}"
    );
    // A member's hello signed by the wrong key, or for an unknown project.
    let mut s = server.connect().await;
    let reply = s.hello(editor.hello(&p.project), &stranger.keys).await;
    assert!(
        matches!(
            reply,
            Message::Refused {
                code: Code::Auth,
                ..
            }
        ),
        "{reply:?}"
    );
    let mut s = server.connect().await;
    let reply = s
        .hello(editor.hello(&random_id().unwrap()), &editor.keys)
        .await;
    assert!(
        matches!(
            reply,
            Message::Refused {
                code: Code::UnknownProject,
                ..
            }
        ),
        "{reply:?}"
    );

    let mut e = p.device(&server, &editor).await;
    // Signed by another device (the owner), for another stream (the
    // owner's), for another document.
    let bad = [
        p.record(&p.owner, "main", b"x"),
        p.record_for(&editor, "main", &p.owner.id, b"x"),
        p.record(&editor, "review", b"x"),
    ];
    for record in bad {
        let reply = append(&mut e.client, "main", 1, record).await;
        assert_eq!(nack_code(reply), Code::Auth);
    }
    // A record whose signature doesn't verify (forged signer id).
    let forged = {
        let context = Context {
            project: &p.project,
            document: "main",
            kind: "log",
            name: &editor.id,
        };
        frame(&seal_v2(&p.keyring, &editor.id, &stranger.keys, &context, 1, b"x").unwrap()).unwrap()
    };
    assert_eq!(
        nack_code(append(&mut e.client, "main", 1, forged).await),
        Code::Auth
    );
    // Damaged framing and an unknown document.
    let mut damaged = p.record(&editor, "main", b"x");
    damaged[9] ^= 1;
    assert_eq!(
        nack_code(append(&mut e.client, "main", 1, damaged).await),
        Code::BadRequest
    );
    let reply = append(
        &mut e.client,
        "layout",
        1,
        p.record(&editor, "layout", b"x"),
    )
    .await;
    assert_eq!(nack_code(reply), Code::BadRequest);
    // Nothing was stored for the editor.
    assert!(
        !server
            .project_dir(&p.project)
            .join(format!("main/{}.log", editor.id))
            .exists()
    );
    let good = append(&mut e.client, "main", 1, p.record(&editor, "main", b"ok")).await;
    assert!(matches!(good, Message::Ack { .. }));

    // A role change takes effect on the next append; a removal closes the
    // connection.
    let user = p.members.device(&editor.id).unwrap().user.clone();
    let record = p.entry(
        &p.owner,
        Op::Role {
            user: user.clone(),
            role: Role::Reader,
        },
    );
    assert!(matches!(
        p.append_members(&mut owner.client, record).await,
        Message::MembersAck { .. }
    ));
    let reply = append(&mut e.client, "main", 2, p.record(&editor, "main", b"no")).await;
    assert_eq!(nack_code(reply), Code::Forbidden);
    let record = p.entry(
        &p.owner,
        Op::Remove {
            user: Some(user),
            device: None,
        },
    );
    assert!(matches!(
        p.append_members(&mut owner.client, record).await,
        Message::MembersAck { .. }
    ));
    e.client.refused(Code::NotAMember).await;
    // A stale members record (built on the previous head) is refused.
    let before = Members::from_log(&p.log[..p.log.len() - 1])
        .members
        .unwrap();
    let record = before
        .entry(
            &p.owner.id,
            now(),
            Op::Profile { profile: vec![1] },
            &[&p.owner.keys],
        )
        .unwrap();
    let reply = p.append_members(&mut owner.client, record).await;
    assert_eq!(nack_code(reply), Code::Stale);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_batches_are_stored_once() {
    let server = Server::start().await;
    let (p, mut owner) = Fixture::create(&server).await;
    let records = p.record(&p.owner, "main", b"once");
    let first = append(&mut owner.client, "main", 7, records.clone()).await;
    let again = append(&mut owner.client, "main", 7, records.clone()).await;
    assert_eq!(first, again);
    // An older batch id is a resend too.
    let older = append(&mut owner.client, "main", 3, records.clone()).await;
    assert_eq!(ack_length(older), records.len() as u64);
    let path = server
        .project_dir(&p.project)
        .join(format!("main/{}.log", p.owner.id));
    assert_eq!(std::fs::read(&path).unwrap(), records);
    // Also after a reconnect: the welcome says the last batch.
    let mut client = server.connect().await;
    match client.hello(p.owner.hello(&p.project), &p.owner.keys).await {
        Message::Welcome { streams, .. } => assert_eq!(streams[0].batch, 7),
        other => panic!("{other:?}"),
    }
    let reply = append(&mut client, "main", 7, records.clone()).await;
    assert_eq!(ack_length(reply), records.len() as u64);
    assert_eq!(std::fs::read(&path).unwrap(), records);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compaction() {
    let server = Server::start().await;
    let (mut p, mut owner) = Fixture::create(&server).await;
    let editor = p.add(&server, &mut owner.client, Role::Editor).await;
    let mut sub = p.device(&server, &editor).await;
    sub.client.subscribe(0, vec![]).await;

    let a = p.record(&p.owner, "main", b"a");
    let b = p.record(&p.owner, "main", b"bb");
    append(&mut owner.client, "main", 1, a.clone()).await;
    append(&mut owner.client, "main", 2, b.clone()).await;
    for _ in 0..2 {
        assert!(matches!(sub.client.next().await, Message::Records { .. }));
    }

    let snapshot = p.sealed(&p.owner, "main", "snapshot", &p.owner.id, b"everything");
    let path = format!("/v1/projects/{}/main/{}/snapshot", p.project, p.owner.id);
    // Not a record boundary; by another device; past the end.
    let bad = server
        .http(
            "PUT",
            &format!("{path}?covers=3"),
            Some(&owner.token),
            &snapshot,
        )
        .await;
    assert_eq!(bad.status, 400);
    let total = a.len() + b.len();
    let bad = server
        .http(
            "PUT",
            &format!("{path}?covers={}", total + 8),
            Some(&owner.token),
            &snapshot,
        )
        .await;
    assert_eq!(bad.status, 400);
    let other = server
        .http(
            "PUT",
            &format!("{path}?covers=0"),
            Some(&sub.token),
            &snapshot,
        )
        .await;
    assert_eq!(other.status, 403);
    let wrong = p.sealed(&p.owner, "main", "snapshot", &editor.id, b"x");
    let bad = server
        .http(
            "PUT",
            &format!("{path}?covers=0"),
            Some(&owner.token),
            &wrong,
        )
        .await;
    assert_eq!(bad.status, 401);

    let put = server
        .http(
            "PUT",
            &format!("{path}?covers={}", a.len()),
            Some(&owner.token),
            &snapshot,
        )
        .await;
    assert_eq!(put.status, 200, "{}", String::from_utf8_lossy(&put.body));
    let json: serde_json::Value = serde_json::from_slice(&put.body).unwrap();
    assert_eq!(json["generation"], 1);
    assert_eq!(json["length"], b.len() as u64);
    let log = server
        .project_dir(&p.project)
        .join(format!("main/{}.log", p.owner.id));
    assert_eq!(std::fs::read(&log).unwrap(), b);

    // The subscriber past covers shifts and continues.
    assert_eq!(
        sub.client.next().await,
        Message::Compacted {
            document: "main".into(),
            device: p.owner.id.clone(),
            generation: 1,
            covers: a.len() as u64,
        }
    );
    let c = p.record(&p.owner, "main", b"ccc");
    let reply = append(&mut owner.client, "main", 3, c.clone()).await;
    assert!(
        matches!(reply, Message::Ack { generation: 1, .. }),
        "{reply:?}"
    );
    assert_eq!(
        sub.client.next().await,
        Message::Records {
            document: "main".into(),
            device: p.owner.id.clone(),
            generation: 1,
            offset: b.len() as u64,
            bytes: c.clone(),
        }
    );

    // A fresh subscriber: snapshot changed, then the remaining records.
    let mut fresh = p.device(&server, &editor).await;
    let got = fresh.client.subscribe(p.log.len() as u64, vec![]).await;
    assert_eq!(
        got,
        [
            Message::SnapshotChanged {
                document: "main".into(),
                device: p.owner.id.clone(),
                generation: 1,
                covers: a.len() as u64,
            },
            Message::Records {
                document: "main".into(),
                device: p.owner.id.clone(),
                generation: 1,
                offset: 0,
                bytes: [b.clone(), c.clone()].concat(),
            }
        ]
    );
    let got = server.http("GET", &path, Some(&fresh.token), b"").await;
    assert_eq!(got.status, 200);
    assert_eq!(got.body, snapshot);
    assert!(got.head.contains("x-lethora-generation: 1"));

    // Positions from before the compaction: past covers shifts, before
    // covers needs the snapshot.
    let position = |generation, offset| Position {
        document: "main".into(),
        device: p.owner.id.clone(),
        generation,
        offset,
    };
    let mut late = p.device(&server, &editor).await;
    let got = late
        .client
        .subscribe(p.log.len() as u64, vec![position(0, total as u64)])
        .await;
    assert!(
        matches!(got[0], Message::Compacted { generation: 1, .. }),
        "{got:?}"
    );
    assert!(
        matches!(&got[1], Message::Records { offset, bytes, .. } if *offset == b.len() as u64 && *bytes == c)
    );
    assert_eq!(got.len(), 2);
    let mut early = p.device(&server, &editor).await;
    let got = early
        .client
        .subscribe(p.log.len() as u64, vec![position(0, 0)])
        .await;
    assert!(
        matches!(got[0], Message::SnapshotChanged { generation: 1, .. }),
        "{got:?}"
    );
    assert!(matches!(&got[1], Message::Records { offset: 0, .. }));
    // Up to date: nothing to send.
    let length = (b.len() + c.len()) as u64;
    let mut current = p.device(&server, &editor).await;
    let got = current
        .client
        .subscribe(p.log.len() as u64, vec![position(1, length)])
        .await;
    assert!(got.is_empty(), "{got:?}");
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invites_close() {
    let server = Server::start().await;
    let (mut p, mut owner) = Fixture::create(&server).await;
    let project_dir = server.project_dir(&p.project);
    let key_file = |link: &InviteLink| project_dir.join(format!("invites/{}.key", link.id));
    let put_key = async |p: &Fixture, link: &InviteLink, token: &str| {
        let wrapped = link.keys().wrap(&p.project, &p.keyring).unwrap();
        let path = format!("/v1/projects/{}/invites/{}", p.project, link.id);
        server
            .http("PUT", &path, Some(token), &wrapped)
            .await
            .status
    };

    // Expired by the server's clock, though the join claims an earlier time.
    let expires = now() + 1;
    let link = p.invite(&mut owner.client, Role::Editor, expires, 1).await;
    assert_eq!(put_key(&p, &link, &owner.token).await, 204);
    let mut invitee = p.invitee(&server, &link).await;
    tokio::time::sleep(Duration::from_millis(2200)).await;
    let join = p.join(&link, &Dev::new(), expires - 1);
    let reply = p.append_members(&mut invitee.client, join).await;
    assert_eq!(nack_code(reply), Code::InviteClosed);
    assert!(!key_file(&link).exists());
    let mut late = server.connect().await;
    let hello = Hello::Invitee {
        project: p.project.clone(),
        invite: link.id.clone(),
    };
    let reply = late.hello(hello, &link.keys()).await;
    assert!(
        matches!(
            reply,
            Message::Refused {
                code: Code::InviteClosed,
                ..
            }
        ),
        "{reply:?}"
    );
    assert_eq!(put_key(&p, &link, &owner.token).await, 410);

    // Only the creator stores the key.
    let editor = p.add(&server, &mut owner.client, Role::Editor).await;
    let e = p.device(&server, &editor).await;
    let link = p
        .invite(&mut owner.client, Role::Reader, now() + 3600, 2)
        .await;
    assert_eq!(put_key(&p, &link, &e.token).await, 403);
    assert_eq!(put_key(&p, &link, &owner.token).await, 204);
    // Two uses: the key stays after the first join, goes after the second.
    for left in [true, false] {
        let mut invitee = p.invitee(&server, &link).await;
        let reply = p
            .append_members(&mut invitee.client, p.join(&link, &Dev::new(), now()))
            .await;
        assert!(matches!(reply, Message::MembersAck { .. }), "{reply:?}");
        assert_eq!(key_file(&link).exists(), left);
    }

    // Revoked.
    let link = p
        .invite(&mut owner.client, Role::Reader, now() + 3600, 1)
        .await;
    assert_eq!(put_key(&p, &link, &owner.token).await, 204);
    let mut invitee = p.invitee(&server, &link).await;
    let record = p.entry(
        &p.owner,
        Op::Revoke {
            id: link.id.clone(),
        },
    );
    assert!(matches!(
        p.append_members(&mut owner.client, record).await,
        Message::MembersAck { .. }
    ));
    assert!(!key_file(&link).exists());
    let path = format!("/v1/projects/{}/invites/{}", p.project, link.id);
    assert_eq!(
        server
            .http("GET", &path, Some(&invitee.token), b"")
            .await
            .status,
        410
    );
    // An invitee appends nothing but its own join.
    let record = p.entry(&p.owner, Op::Profile { profile: vec![1] });
    invitee
        .client
        .send(&Message::AppendMembers { batch: 5, record })
        .await;
    let reply = invitee.client.next().await;
    assert!(
        matches!(
            reply,
            Message::Nack {
                batch: 5,
                code: Code::Forbidden,
                ..
            }
        ),
        "{reply:?}"
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assets() {
    let server = Server::with(|l| l.asset_bytes = 64 * 1024).await;
    let (mut p, mut owner) = Fixture::create(&server).await;
    let reader = p.add(&server, &mut owner.client, Role::Reader).await;
    let r = p.device(&server, &reader).await;
    let blob = lethora_protocol::to_hex(&random::<32>().unwrap());
    let path = format!("/v1/projects/{}/assets/{blob}", p.project);
    let envelope = p.sealed(&p.owner, "main", "asset", &blob, b"picture");

    assert_eq!(server.http("PUT", &path, None, &envelope).await.status, 401);
    assert_eq!(
        server
            .http("PUT", &path, Some(&r.token), &envelope)
            .await
            .status,
        403
    );
    assert_eq!(
        server.http("GET", &path, Some(&r.token), b"").await.status,
        404
    );
    // Sealed for another name.
    let other = p.sealed(&p.owner, "main", "asset", &"0".repeat(64), b"picture");
    assert_eq!(
        server
            .http("PUT", &path, Some(&owner.token), &other)
            .await
            .status,
        401
    );
    assert_eq!(
        server
            .http("PUT", &path, Some(&owner.token), &envelope)
            .await
            .status,
        201
    );
    let got = server.http("GET", &path, Some(&r.token), b"").await;
    assert_eq!((got.status, got.body), (200, envelope.clone()));
    // Immutable.
    let replaced = p.sealed(&p.owner, "main", "asset", &blob, b"another");
    assert_eq!(
        server
            .http("PUT", &path, Some(&owner.token), &replaced)
            .await
            .status,
        200
    );
    assert_eq!(
        server.http("GET", &path, Some(&r.token), b"").await.body,
        envelope
    );
    // Too large.
    let blob = lethora_protocol::to_hex(&random::<32>().unwrap());
    let path = format!("/v1/projects/{}/assets/{blob}", p.project);
    let big = p.sealed(&p.owner, "main", "asset", &blob, &vec![0; 100 * 1024]);
    assert_eq!(
        server
            .http("PUT", &path, Some(&owner.token), &big)
            .await
            .status,
        413
    );
    // A bad name; a token of another project.
    let bad = format!("/v1/projects/{}/assets/{}", p.project, "A".repeat(64));
    assert_eq!(
        server
            .http("PUT", &bad, Some(&owner.token), &envelope)
            .await
            .status,
        400
    );
    let (q, _) = Fixture::create(&server).await;
    let foreign = format!("/v1/projects/{}/assets/{blob}", q.project);
    assert_eq!(
        server
            .http("GET", &foreign, Some(&owner.token), b"")
            .await
            .status,
        401
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tokens_live_as_long_as_their_connection() {
    let server = Server::start().await;
    let (mut p, mut owner) = Fixture::create(&server).await;
    let blob = lethora_protocol::to_hex(&random::<32>().unwrap());
    let path = format!("/v1/projects/{}/assets/{blob}", p.project);
    // 404: the token is good, the blob isn't there.
    let status = async |token: &str| server.http("GET", &path, Some(token), b"").await.status;
    assert_eq!(status(&owner.token).await, 404);

    // A removed device loses HTTP access with its connection.
    let editor = p.add(&server, &mut owner.client, Role::Editor).await;
    let mut e = p.device(&server, &editor).await;
    assert_eq!(status(&e.token).await, 404);
    let user = p.members.device(&editor.id).unwrap().user.clone();
    let record = p.entry(
        &p.owner,
        Op::Remove {
            user: Some(user),
            device: None,
        },
    );
    assert!(matches!(
        p.append_members(&mut owner.client, record).await,
        Message::MembersAck { .. }
    ));
    e.client.refused(Code::NotAMember).await;
    assert_eq!(status(&e.token).await, 401);

    // Closing a connection revokes its token (no fixed expiry); a new
    // connection gets a new one.
    let old = owner.token.clone();
    owner.client.ws.close(None).await.unwrap();
    let deadline = tokio::time::Instant::now() + WAIT;
    while status(&old).await != 401 {
        assert!(tokio::time::Instant::now() < deadline, "still valid");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let again = p.device(&server, &p.owner).await;
    assert_ne!(again.token, old);
    assert_eq!(status(&again.token).await, 404);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn presence_is_forwarded_not_stored() {
    let server = Server::start().await;
    let (mut p, mut owner) = Fixture::create(&server).await;
    let editor = p.add(&server, &mut owner.client, Role::Editor).await;
    let reviewer = p.add(&server, &mut owner.client, Role::Reviewer).await;
    let mut e = p.device(&server, &editor).await;
    let mut v = p.device(&server, &reviewer).await;
    let members = p.log.len() as u64;
    owner.client.subscribe(members, vec![]).await;
    e.client.subscribe(members, vec![]).await;
    v.client.subscribe(members, vec![]).await;

    let marker = random::<16>().unwrap().to_vec();
    owner
        .client
        .send(&Message::Presence {
            bytes: marker.clone(),
        })
        .await;
    let expected = Message::PeerPresence {
        device: p.owner.id.clone(),
        bytes: marker.clone(),
    };
    assert_eq!(e.client.next().await, expected);
    assert_eq!(v.client.next().await, expected);
    owner.client.nothing_pending().await;
    for file in files(&server.dir) {
        let bytes = std::fs::read(&file).unwrap();
        assert!(
            !bytes.windows(marker.len()).any(|w| w == marker),
            "{file:?}"
        );
    }
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn limits() {
    let server = Server::with(|l| {
        l.message_bytes = 16 * 1024;
        l.chunk_bytes = 4 * 1024;
        l.projects_per_ip_per_day = 2;
        l.connections_per_ip = 3;
    })
    .await;
    let (p, mut owner) = Fixture::create(&server).await;
    // An oversized message.
    let big = p.record(&p.owner, "main", &vec![0; 20 * 1024]);
    owner
        .client
        .send(&Message::Append {
            document: "main".into(),
            batch: 1,
            records: big,
        })
        .await;
    owner.client.refused(Code::TooLarge).await;
    // Garbage and messages of the wrong direction close the connection.
    let mut c = p.device(&server, &p.owner).await;
    c.client.send_raw(vec![2, 200]).await;
    c.client.refused(Code::BadRequest).await;
    // Protocol versions out of the range, newer and older.
    for version in [9, 1] {
        let mut c = p.device(&server, &p.owner).await;
        c.client.send_raw(vec![version, 11]).await;
        c.client.refused(Code::Version).await;
    }
    let mut c = p.device(&server, &p.owner).await;
    c.client.send(&Message::Synced).await;
    c.client.refused(Code::BadRequest).await;

    // Projects per address and day: one more is allowed, then refused.
    let (_q, _) = Fixture::create(&server).await;
    let dev = Dev::new();
    let project = random_id().unwrap();
    let genesis =
        Members::genesis_entry(&project, &random_id().unwrap(), &dev.id, &dev.keys, now()).unwrap();
    let mut client = server.connect().await;
    let reply = client.hello(Hello::Create { genesis }, &dev.keys).await;
    assert!(
        matches!(
            reply,
            Message::Refused {
                code: Code::RateLimited,
                ..
            }
        ),
        "{reply:?}"
    );
    assert!(!server.project_dir(&project).exists());

    // Connections per address: the fourth is refused with 429.
    let open = [
        server.connect().await,
        server.connect().await,
        server.connect().await,
    ];
    let error = tokio_tungstenite::connect_async(format!("ws://{}/v1/ws", server.addr))
        .await
        .unwrap_err();
    match error {
        tokio_tungstenite::tungstenite::Error::Http(response) => {
            assert_eq!(response.status(), 429);
        }
        other => panic!("expected 429, got {other:?}"),
    }
    drop(open);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn large_catch_up_comes_in_chunks() {
    let server = Server::with(|l| {
        l.message_bytes = 64 * 1024;
        l.chunk_bytes = 8 * 1024;
    })
    .await;
    let (p, mut owner) = Fixture::create(&server).await;
    let mut all = Vec::new();
    for batch in 1..=10 {
        let records = p.record(&p.owner, "main", &vec![batch as u8; 3000]);
        all.extend_from_slice(&records);
        append(&mut owner.client, "main", batch, records).await;
    }
    // One record larger than a chunk goes alone.
    let large = p.record(&p.owner, "main", &[7; 20_000]);
    all.extend_from_slice(&large);
    append(&mut owner.client, "main", 11, large).await;
    let mut reader = p.device(&server, &p.owner).await;
    let got = reader.client.subscribe(0, vec![]).await;
    let mut log = Vec::new();
    let mut chunks = 0;
    for message in got {
        if let Message::Records { offset, bytes, .. } = message {
            assert_eq!(offset, log.len() as u64);
            assert!(bytes.len() <= 8 * 1024 || bytes.len() > 20_000);
            log.extend(bytes);
            chunks += 1;
        }
    }
    assert_eq!(log, all);
    assert!(chunks >= 5, "{chunks}");
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_and_deletion() {
    let mut server = Server::start().await;
    let (mut p, mut owner) = Fixture::create(&server).await;
    let editor = p.add(&server, &mut owner.client, Role::Editor).await;
    let records = p.record(&p.owner, "main", b"kept");
    append(&mut owner.client, "main", 1, records.clone()).await;
    drop(owner);
    // A torn tail (a crash mid-append) and a damaged members tail.
    let dir = server.project_dir(&p.project);
    let log = dir.join(format!("main/{}.log", p.owner.id));
    let mut torn = std::fs::read(&log).unwrap();
    torn.extend_from_slice(&[50, 0, 0, 0, 1, 2]);
    let members = dir.join("members.log");
    let mut damaged = std::fs::read(&members).unwrap();
    damaged.extend_from_slice(&frame(b"not an entry").unwrap());

    server.running.take().unwrap().stop().await;
    std::fs::write(&log, &torn).unwrap();
    std::fs::write(&members, &damaged).unwrap();
    server.restart().await;
    // Loaded on first use: both tails are cut off.
    let mut e = p.device(&server, &editor).await;
    assert!(std::fs::read(&members).unwrap() == p.log);
    assert!(dir.join("members.log.damaged").exists());
    assert!(std::fs::read(&log).unwrap() == records);
    let got = e.client.subscribe(p.log.len() as u64, vec![]).await;
    assert_eq!(
        got,
        [Message::Records {
            document: "main".into(),
            device: p.owner.id.clone(),
            generation: 0,
            offset: 0,
            bytes: records,
        }]
    );

    let mut owner = p.device(&server, &p.owner).await;
    owner.client.subscribe(p.log.len() as u64, vec![]).await;
    let path = format!("/v1/projects/{}", p.project);
    // A non-owner can't delete; a stale or forged statement doesn't either.
    let body = deletion_body(&editor.keys, &p.project, now()).unwrap();
    assert_eq!(
        server
            .http("DELETE", &path, Some(&e.token), &body)
            .await
            .status,
        403
    );
    let old = deletion_body(&p.owner.keys, &p.project, now() - 3600).unwrap();
    assert_eq!(
        server
            .http("DELETE", &path, Some(&owner.token), &old)
            .await
            .status,
        401
    );
    assert_eq!(
        server
            .http("DELETE", &path, Some(&owner.token), &body)
            .await
            .status,
        401
    );
    assert!(server.project_dir(&p.project).exists());

    let body = deletion_body(&p.owner.keys, &p.project, now()).unwrap();
    assert_eq!(
        server
            .http("DELETE", &path, Some(&owner.token), &body)
            .await
            .status,
        204
    );
    assert!(!server.project_dir(&p.project).exists());
    owner.client.refused(Code::UnknownProject).await;
    e.client.refused(Code::UnknownProject).await;
    let mut again = server.connect().await;
    let reply = again.hello(p.owner.hello(&p.project), &p.owner.keys).await;
    assert!(
        matches!(
            reply,
            Message::Refused {
                code: Code::UnknownProject,
                ..
            }
        ),
        "{reply:?}"
    );
    assert_eq!(
        server
            .http("DELETE", &path, Some(&owner.token), &body)
            .await
            .status,
        401
    );
    let health = server.http("GET", "/healthz", None, b"").await.body;
    let health = String::from_utf8(health).unwrap();
    assert!(health.starts_with("ok ("), "{health}");
    assert!(
        health.contains(VERSION) && health.contains("level 1"),
        "{health}"
    );
    server.stop().await;
}

/// The message of a refusal (then the end of the connection).
async fn refusal(client: &mut Client) -> (Code, String) {
    let got = match client.next().await {
        Message::Refused { code, message } => (code, message),
        other => panic!("expected a refusal, got {other:?}"),
    };
    assert_eq!(client.try_next().await, None);
    got
}

/// A members record by `dev` whose op is `op` (raw JSON), signed by it: what
/// a newer app would write.
fn raw_entry(p: &Fixture, dev: &Dev, op: &str) -> Vec<u8> {
    let head = p.members.head();
    let raw = format!(
        "{{\"seq\":{},\"prev\":\"{}\",\"project\":\"{}\",\"time\":{},\"by\":\"{}\",\"op\":{op}}}",
        head.seq + 1,
        head.hash,
        p.project,
        now(),
        dev.id
    );
    let mut entry = (raw.len() as u32).to_le_bytes().to_vec();
    entry.extend_from_slice(raw.as_bytes());
    entry.push(1);
    entry.extend_from_slice(&dev.keys.sign(b"lethora-members-1\0", raw.as_bytes()));
    frame(&entry).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn levels() {
    let mut server = Server::start().await;
    let (mut p, mut owner) = Fixture::create(&server).await;
    let editor = p.add(&server, &mut owner.client, Role::Editor).await;
    let mut e = p.device(&server, &editor).await;
    e.client.subscribe(p.log.len() as u64, vec![]).await;
    // The welcome says the server's version and the devices' last hellos.
    let mut client = server.connect().await;
    let reply = client
        .hello_at(p.owner.hello(&p.project), 2, &p.owner.keys)
        .await;
    let Message::Welcome { app, devices, .. } = &reply else {
        panic!("expected a welcome, got {reply:?}");
    };
    assert_eq!(app, VERSION);
    let seen = |devices: &[DeviceVersion], dev: &Dev| {
        devices
            .iter()
            .find(|d| d.device == dev.id)
            .map(|d| (d.version.level, d.version.app.clone()))
    };
    assert_eq!(seen(devices, &p.owner), Some((2, "test 2".into())));
    assert_eq!(seen(devices, &editor), Some((1, "test 1".into())));
    let mut owner2 = Session {
        client,
        token: welcome(reply).0,
    };
    owner2.client.subscribe(p.log.len() as u64, vec![]).await;

    // A record above the project's level is refused until it is required.
    let level2 = p.record_at(&p.owner, "main", &p.owner.id, 2, b"new");
    let reply = append(&mut owner2.client, "main", 1, level2.clone()).await;
    assert_eq!(nack_code(reply), Code::BadRequest);
    // A members op this server doesn't know (a newer app's): refused.
    let record = raw_entry(&p, &p.owner, "{\"type\":\"future\"}");
    match p.append_members(&mut owner2.client, record).await {
        Message::Nack { code, message, .. } => {
            assert_eq!(code, Code::Version);
            assert!(message.contains("server needs an update"), "{message}");
        }
        other => panic!("expected a nack, got {other:?}"),
    }
    assert_eq!(
        std::fs::read(server.project_dir(&p.project).join("members.log")).unwrap(),
        p.log
    );

    // Requiring level 2 closes the connections below it before the entry
    // goes out.
    let require = p.entry(
        &p.owner,
        Op::Require {
            level: 2,
            app: "0.2.0".into(),
        },
    );
    let reply = p.append_members(&mut owner2.client, require).await;
    assert!(matches!(reply, Message::MembersAck { .. }), "{reply:?}");
    let expected = "This project needs Lethora 0.2.0 or later.".to_owned();
    assert_eq!(
        refusal(&mut e.client).await,
        (Code::Version, expected.clone())
    );
    assert_eq!(
        refusal(&mut owner.client).await,
        (Code::Version, expected.clone())
    );
    // Hellos below it are refused, at it welcomed.
    let mut client = server.connect().await;
    let reply = client.hello(editor.hello(&p.project), &editor.keys).await;
    assert_eq!(
        reply,
        Message::Refused {
            code: Code::Version,
            message: expected
        }
    );
    let mut e = p.device_at(&server, &editor, 2).await;
    e.client.subscribe(p.log.len() as u64, vec![]).await;
    // Now level 2 records are taken, and level 1 ones still.
    let reply = append(&mut owner2.client, "main", 2, level2).await;
    assert!(matches!(reply, Message::Ack { .. }), "{reply:?}");
    let level1 = p.record(&p.owner, "main", b"old");
    let reply = append(&mut owner2.client, "main", 3, level1).await;
    assert!(matches!(reply, Message::Ack { .. }), "{reply:?}");

    // The last hellos are kept across a restart.
    drop((e, owner2));
    server.restart().await;
    let mut client = server.connect().await;
    let reply = client
        .hello_at(p.owner.hello(&p.project), 3, &p.owner.keys)
        .await;
    let Message::Welcome { devices, .. } = &reply else {
        panic!("expected a welcome, got {reply:?}");
    };
    assert_eq!(seen(devices, &p.owner), Some((3, "test 3".into())));
    assert_eq!(seen(devices, &editor), Some((2, "test 2".into())));
    server.stop().await;
}
