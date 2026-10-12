//! A web server on a loopback port 0 and a bare HTTP/1.1 client over a real
//! socket, for the web tests. No HTTP client crate: the wire format is the
//! thing under test.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use claude_monitor::cli::{run_in, Command};
use claude_monitor::env::Env;
use claude_monitor::feed::{ChangeFeed, FakeHandle, FakeSource, SystemMonotonic};
use claude_monitor::model::{Session, State};
use claude_monitor::store::{legacy, Store, StoreOptions};
use claude_monitor::testing::FakeEnv;
use claude_monitor::web::{self, Server, Token, WebConfig};
use tempfile::TempDir;

pub const IO_TIMEOUT: Duration = Duration::from_secs(5);

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

pub struct Rig {
    pub dir: TempDir,
    pub server: Server,
    pub token: Token,
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
        let token = Token::generate().unwrap();
        let mut config = WebConfig::new(token.clone(), db);
        config.port = 0;
        tweak(&mut config);
        let server = web::start(config, feed).unwrap();
        Self {
            dir,
            server,
            token,
            wake,
            writer,
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.server.addr()
    }

    pub fn host(&self) -> String {
        format!("127.0.0.1:{}", self.addr().port())
    }

    pub fn cookie(&self) -> String {
        format!("cc_token={}", self.token.as_str())
    }
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
            .header("X-CC-Token", rig.token.as_str())
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
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
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

pub fn connect(addr: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    stream.set_write_timeout(Some(IO_TIMEOUT)).unwrap();
    stream
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
    reader: BufReader<TcpStream>,
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
