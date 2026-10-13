//! A web server on a loopback port 0 and a bare HTTP/1.1 client over a real
//! TLS socket, for the web tests. No HTTP client crate: the wire format is the
//! thing under test. The client trusts only the CA the server under test
//! generated in its own state directory; `trust` records which CA belongs to
//! which port so `Request::send(addr)` keeps its one-argument shape.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

use claude_monitor::cli::{run_in, Command};
use claude_monitor::env::Env;
use claude_monitor::feed::{ChangeFeed, FakeHandle, FakeSource, SystemMonotonic};
use claude_monitor::model::{Session, State};
use claude_monitor::store::{legacy, Store, StoreOptions};
use claude_monitor::testing::FakeEnv;
use claude_monitor::web::clock::ManualTicks;
use claude_monitor::web::{self, Server, WebConfig};
use tempfile::TempDir;

pub const IO_TIMEOUT: Duration = Duration::from_secs(5);

pub type TlsStream = StreamOwned<ClientConnection, TcpStream>;

static TRUSTED_CA: Mutex<Option<HashMap<u16, PathBuf>>> = Mutex::new(None);

/// Tells the client which CA certificate (PEM file) the server on `port`
/// presents a leaf from.
pub fn trust(port: u16, ca_pem: &Path) {
    TRUSTED_CA
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(port, ca_pem.to_owned());
}

pub fn ca_der(ca_pem: &Path) -> CertificateDer<'static> {
    let pem = std::fs::read(ca_pem).unwrap();
    let (_, parsed) = x509_parser::pem::parse_x509_pem(&pem).unwrap();
    CertificateDer::from(parsed.contents)
}

/// A TLS 1.3 client that trusts exactly the given CA files.
pub fn client_config(roots: &[&Path]) -> ClientConfig {
    let mut store = RootCertStore::empty();
    for root in roots {
        store.add(ca_der(root)).unwrap();
    }
    ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(store)
        .with_no_client_auth()
}

/// Opens a TCP connection and wraps it in a client session; the handshake
/// runs on first read or write.
pub fn tls_over(addr: SocketAddr, config: ClientConfig) -> TlsStream {
    let tcp = tcp(addr);
    let name = ServerName::IpAddress(addr.ip().into());
    let session = ClientConnection::new(Arc::new(config), name).unwrap();
    StreamOwned::new(session, tcp)
}

pub fn tcp(addr: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    stream.set_write_timeout(Some(IO_TIMEOUT)).unwrap();
    stream
}

/// Reads to the end, treating a close without `close_notify` as the end too.
pub fn read_all(stream: &mut impl Read) -> Vec<u8> {
    let mut raw = Vec::new();
    match stream.read_to_end(&mut raw) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {}
        Err(e) => panic!("read failed: {e}"),
    }
    raw
}

/// A spawned child that is killed and reaped when this drops, so a panicking
/// test cannot leave a `serve` process running.
pub struct KillOnDrop(Child);

impl KillOnDrop {
    pub fn spawn(command: &mut Process) -> std::io::Result<Self> {
        use std::os::unix::process::CommandExt;

        // SAFETY: the hook runs between fork and exec and only makes one
        // async-signal-safe prctl call, with no allocation or locking.
        // The signal means a SIGKILLed test runner (which skips Drop) still
        // takes its server with it.
        unsafe {
            command.pre_exec(|| {
                rustix::process::set_parent_process_death_signal(Some(
                    rustix::process::Signal::KILL,
                ))
                .map_err(std::io::Error::from)
            });
        }
        command.spawn().map(Self)
    }
}

impl Deref for KillOnDrop {
    type Target = Child;

    fn deref(&self) -> &Child {
        &self.0
    }
}

impl DerefMut for KillOnDrop {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A server over a fresh store with a fake clock and no tmux, plus the means
/// to seed sessions and to run the same command through the CLI.
pub struct Plain {
    pub dir: TempDir,
    pub fake: FakeEnv,
    pub store: Store,
    pub rig: Rig,
}

impl Plain {
    pub fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let fake = FakeEnv::new();
        let store = Store::open_in_dir(dir.path(), &StoreOptions::viewer()).unwrap();
        let empty = TempDir::new().unwrap();
        legacy::migrate(&store, empty.path(), fake.env.clock.as_ref()).unwrap();
        let rig = Rig::start_on(dir.path(), fake.env.clone());
        Self {
            dir,
            fake,
            store,
            rig,
        }
    }

    pub fn put(&self, id: &str, state: State) {
        let mut session = Session::new(
            id,
            "claude",
            &format!("agent-{id}"),
            &format!("/work/{id}"),
            "2026-07-01T00:00:00Z",
        );
        session.state = state;
        session.last_activity_at = "2026-10-11T09:00:00Z".to_owned();
        session.task_description = Some(format!("task of {id}"));
        session.left_off = Some(format!("left off in {id}"));
        self.store.upsert_session(&session).unwrap();
    }

    pub fn session(&self, id: &str) -> Session {
        self.store.get_session(id).unwrap().unwrap()
    }

    pub fn save(&self, session: &Session) {
        self.store.upsert_session(session).unwrap();
    }

    pub fn cli(&self, command: Command) -> serde_json::Value {
        let output = run_in(command, &self.fake.env, Some(self.dir.path())).unwrap();
        serde_json::from_str(&output.render(true)).unwrap()
    }
}

/// What a browser holds after logging in: the session cookie and the CSRF
/// value the page handed it.
#[derive(Clone, Debug)]
pub struct Login {
    pub cookie: String,
    pub csrf: String,
}

pub struct Rig {
    pub dir: TempDir,
    pub server: Server,
    pub session: Login,
    pub ticks: Arc<ManualTicks>,
    pub wake: FakeHandle,
    pub writer: Store,
}

impl Rig {
    pub fn start() -> Self {
        Self::start_with(|_| {})
    }

    /// A server over the store in `state_dir`, running its actions against
    /// `env` (a private tmux, a fake clock).
    pub fn start_on(state_dir: &std::path::Path, env: Env) -> Self {
        Self::build(TempDir::new().unwrap(), state_dir.join("state.db"), |c| {
            c.env = env;
        })
    }

    pub fn start_with(tweak: impl FnOnce(&mut WebConfig)) -> Self {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("state.db");
        Self::build(dir, db, tweak)
    }

    fn build(dir: TempDir, db: std::path::PathBuf, tweak: impl FnOnce(&mut WebConfig)) -> Self {
        let writer = Store::open(&db, &StoreOptions::default()).unwrap();
        let (source, wake) = FakeSource::new();
        let feed = ChangeFeed::new(
            Store::open(&db, &StoreOptions::default()).unwrap(),
            source,
            SystemMonotonic,
        )
        .unwrap();
        let ca = db.parent().unwrap().join("tls").join("ca.pem");
        let ticks = Arc::new(ManualTicks::new());
        let mut config = WebConfig::new(db);
        config.port = 0;
        config.clock = ticks.clone();
        config.max_auth_failures = usize::MAX;
        tweak(&mut config);
        let server = web::start(config, feed).unwrap();
        trust(server.addr().port(), &ca);
        let session = log_in(&server);
        Self {
            dir,
            server,
            session,
            ticks,
            wake,
            writer,
        }
    }

    /// A second, independent login.
    pub fn another_session(&self) -> Login {
        log_in(&self.server)
    }

    pub fn csrf(&self) -> &str {
        &self.session.csrf
    }

    pub fn addr(&self) -> SocketAddr {
        self.server.addr()
    }

    pub fn host(&self) -> String {
        format!("127.0.0.1:{}", self.addr().port())
    }

    pub fn cookie(&self) -> String {
        self.session.cookie.clone()
    }
}

/// Logs in the way a browser does: the single-use URL, then the page that
/// carries the CSRF value.
pub fn log_in(server: &Server) -> Login {
    let code = server.issue_login_code().unwrap();
    let addr = server.addr();
    let response = Request::get(&format!("/login?code={}", code.as_str())).send(addr);
    assert_eq!(response.status, 303, "login: {response:?}");
    let cookie = cookie_pair(&response).expect("a session cookie");
    let page = Request::get("/")
        .header("Cookie", cookie.clone())
        .send(addr);
    assert_eq!(page.status, 200, "{page:?}");
    let marker = "<meta name=\"cc-csrf\" content=\"";
    let start = page.body.find(marker).expect("csrf meta tag") + marker.len();
    let csrf = page.body[start..].split('"').next().unwrap().to_owned();
    Login { cookie, csrf }
}

/// `name=value` of the first `Set-Cookie`, as a `Cookie` header wants it.
pub fn cookie_pair(response: &Response) -> Option<String> {
    let line = response.headers_named("set-cookie").into_iter().next()?;
    Some(line.split(';').next()?.to_owned())
}

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {:?}", self.body))
    }

    pub fn headers_named(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .collect()
    }
}

pub struct Request<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub host: Option<String>,
    pub headers: Vec<(&'a str, String)>,
    pub body: String,
}

impl<'a> Request<'a> {
    pub fn get(path: &'a str) -> Self {
        Self::new("GET", path)
    }

    pub fn new(method: &'a str, path: &'a str) -> Self {
        Self {
            method,
            path,
            host: None,
            headers: Vec::new(),
            body: String::new(),
        }
    }

    /// A JSON body with its content type.
    pub fn json_body(mut self, value: &serde_json::Value) -> Self {
        self.headers
            .push(("Content-Type", "application/json".to_owned()));
        self.body = value.to_string();
        self
    }

    /// A form body, already urlencoded.
    pub fn form_body(mut self, encoded: &str) -> Self {
        self.headers.push((
            "Content-Type",
            "application/x-www-form-urlencoded".to_owned(),
        ));
        self.body = encoded.to_owned();
        self
    }

    /// The headers a browser sends once signed in, plus the CSRF header.
    pub fn signed_in(self, rig: &Rig) -> Self {
        self.header("Cookie", rig.cookie())
            .header("X-CC-Token", rig.csrf())
    }

    pub fn host(mut self, host: impl Into<String>) -> Self {
        self.host = Some(host.into());
        self
    }

    pub fn header(mut self, name: &'a str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }

    fn wire(&self, default_host: &str) -> String {
        let host = self.host.as_deref().unwrap_or(default_host);
        let mut out = format!(
            "{} {} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n",
            self.method, self.path
        );
        for (name, value) in &self.headers {
            out.push_str(&format!("{name}: {value}\r\n"));
        }
        if self.method != "GET" && self.method != "OPTIONS" {
            out.push_str(&format!("Content-Length: {}\r\n", self.body.len()));
        }
        out.push_str("\r\n");
        out.push_str(&self.body);
        out
    }

    /// Sends the request and reads the whole response (the server closes).
    pub fn send(&self, addr: SocketAddr) -> Response {
        let mut stream = connect(addr);
        stream
            .write_all(self.wire(&format!("127.0.0.1:{}", addr.port())).as_bytes())
            .unwrap();
        let raw = read_all(&mut stream);
        parse(&String::from_utf8_lossy(&raw))
    }

    /// Sends the request and returns after the response headers, leaving the
    /// body to be read line by line (for an event stream).
    pub fn open(&self, addr: SocketAddr) -> Stream {
        let mut stream = connect(addr);
        stream
            .write_all(self.wire(&format!("127.0.0.1:{}", addr.port())).as_bytes())
            .unwrap();
        let mut reader = BufReader::new(stream);
        let mut head = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" || line.is_empty() {
                break;
            }
            head.push_str(&line);
        }
        Stream {
            head: parse(&format!("{head}\r\n")),
            reader,
        }
    }
}

pub fn connect(addr: SocketAddr) -> TlsStream {
    let ca = TRUSTED_CA
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(&addr.port()).cloned())
        .unwrap_or_else(|| panic!("no CA registered for port {}", addr.port()));
    tls_over(addr, client_config(&[&ca]))
}

fn parse(raw: &str) -> Response {
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw.trim_end(), ""));
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no status line in {raw:?}"));
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Response {
        status,
        headers,
        body: body.to_owned(),
    }
}

pub struct Stream {
    pub head: Response,
    reader: BufReader<TlsStream>,
}

impl Stream {
    /// The next line of the body, without its line ending; `None` at EOF.
    /// Chunk-size lines of a chunked body are skipped.
    pub fn next_line(&mut self) -> Option<String> {
        loop {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line).ok()?;
            if n == 0 {
                return None;
            }
            let line = line.trim_end_matches(['\r', '\n']).to_owned();
            if line == "0" {
                return None;
            }
            if is_chunk_size(&line) {
                continue;
            }
            return Some(line);
        }
    }

    /// Lines up to and including the next blank one: one SSE message or one
    /// comment, as the lines the client would see.
    pub fn next_message(&mut self) -> Option<Vec<String>> {
        let mut lines = Vec::new();
        loop {
            let line = self.next_line()?;
            if line.is_empty() {
                if lines.is_empty() {
                    continue;
                }
                return Some(lines);
            }
            lines.push(line);
        }
    }
}

/// Every SSE line starts with `id:`, `data:` or `:`, so a bare hex line can
/// only be a chunk size.
fn is_chunk_size(line: &str) -> bool {
    !line.is_empty() && line.chars().all(|c| c.is_ascii_hexdigit())
}
