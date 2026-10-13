//! Session summaries: the verbatim "task" and "left off" text taken from the
//! session itself, and the on-demand model-written summary kept apart from it
//! (FR-017, FR-017a, FR-020).

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};

use crate::env::Vars;

/// Most bytes read from the end of a transcript when looking for the latest
/// assistant message; a transcript can run to many megabytes and a hook has a
/// 50 ms budget.
pub const TRANSCRIPT_TAIL_BYTES: u64 = 256 * 1024;

/// The latest assistant message in the last `max_bytes` of the transcript at
/// `path`, text only and untouched. `None` when there is none in that window or
/// the file cannot be read.
pub fn transcript_tail_message(path: &Path, max_bytes: u64) -> Option<String> {
    // Opening a FIFO blocks until a writer appears, so refuse before opening.
    if !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    // O_NONBLOCK covers the file being swapped for a FIFO after the check.
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    let start = meta.len().saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.take(max_bytes).read_to_end(&mut buf).ok()?;

    // A window that starts mid-file starts mid-line; drop that partial line.
    let window = if start > 0 {
        let newline = buf.iter().position(|b| *b == b'\n')?;
        buf.get(newline.saturating_add(1)..)?
    } else {
        buf.as_slice()
    };
    window.split(|b| *b == b'\n').rev().find_map(assistant_text)
}

fn assistant_text(line: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(line).ok()?;
    if value.get("isSidechain").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let message = value.get("message")?;
    let is_assistant = value.get("type").and_then(Value::as_str) == Some("assistant")
        || message.get("role").and_then(Value::as_str) == Some("assistant");
    if !is_assistant {
        return None;
    }
    let text = match message.get("content")? {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    (!text.trim().is_empty()).then_some(text)
}

/// The "left off" text for a `Stop`: the payload's message when it has one,
/// else the transcript tail.
pub fn left_off(stop_message: Option<&str>, transcript_path: Option<&Path>) -> Option<String> {
    match stop_message.filter(|message| !message.trim().is_empty()) {
        Some(message) => Some(message.to_owned()),
        None => {
            transcript_path.and_then(|path| transcript_tail_message(path, TRANSCRIPT_TAIL_BYTES))
        }
    }
}

/// Environment variable naming the OpenAI-compatible server's base URL.
pub const URL_VAR: &str = "CLAUDE_MONITOR_SUMMARY_URL";
/// Environment variable naming the model to ask for.
pub const MODEL_VAR: &str = "CLAUDE_MONITOR_SUMMARY_MODEL";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EndpointError {
    #[error("summary endpoint {url:?} is not usable: {reason}")]
    Invalid { url: String, reason: &'static str },
    #[error(
        "summary endpoint host {host:?} is not loopback; session text never leaves this machine"
    )]
    NotLoopback { host: String },
}

const HTTP_SCHEME: &str = "http://";
const DEFAULT_PORT: u16 = 80;
const CHAT_PATH: &str = "/v1/chat/completions";

/// `(host, port)` of `host`, `host:port`, `[v6]` or `[v6]:port`.
fn split_host_port(authority: &str) -> Option<(&str, u16)> {
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, after) = bracketed.split_once(']')?;
        (host, after.strip_prefix(':'))
    } else {
        match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    if host.is_empty() || (!authority.starts_with('[') && host.contains(':')) {
        return None;
    }
    let port = match port {
        Some(port) => port.parse::<u16>().ok().filter(|p| *p != 0)?,
        None => DEFAULT_PORT,
    };
    Some((host, port))
}

/// The address `host` stands for when that is on this machine. Only the name
/// `localhost` and IP literals qualify; any other name is never resolved,
/// because the answer could point off-machine.
fn loopback_ip(host: &str) -> Option<IpAddr> {
    if host.eq_ignore_ascii_case("localhost") {
        return Some(IpAddr::V4(Ipv4Addr::LOCALHOST));
    }
    host.parse::<IpAddr>().ok().filter(IpAddr::is_loopback)
}

/// A parsed model endpoint that is known to be on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    addr: SocketAddr,
    base_path: String,
}

impl Endpoint {
    /// Accepts `http://` URLs whose host is `localhost` or a loopback IP
    /// literal, without userinfo, query or fragment. Never resolves a name.
    pub fn parse(url: &str) -> Result<Self, EndpointError> {
        let invalid = |reason| EndpointError::Invalid {
            url: url.to_owned(),
            reason,
        };
        let rest = url
            .get(..HTTP_SCHEME.len())
            .filter(|scheme| scheme.eq_ignore_ascii_case(HTTP_SCHEME))
            .and_then(|_| url.get(HTTP_SCHEME.len()..))
            .ok_or_else(|| invalid("only http:// URLs are supported"))?;
        if rest.chars().any(|c| !c.is_ascii_graphic()) {
            return Err(invalid("spaces, control and non-ASCII characters"));
        }
        if rest.contains(['?', '#']) {
            return Err(invalid("a query or fragment"));
        }
        let (authority, path) = match rest.find('/') {
            Some(slash) => rest.split_at(slash),
            None => (rest, ""),
        };
        if authority.contains('@') {
            return Err(invalid("a user or password in the URL"));
        }
        let (host, port) =
            split_host_port(authority).ok_or_else(|| invalid("a bad host or port"))?;
        let ip = loopback_ip(host).ok_or_else(|| EndpointError::NotLoopback {
            host: host.to_owned(),
        })?;
        Ok(Self {
            addr: SocketAddr::new(ip, port),
            base_path: format!("{}{CHAT_PATH}", path.trim_end_matches('/')),
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The request path: the URL's own path then `/v1/chat/completions`.
    pub fn chat_path(&self) -> String {
        self.base_path.clone()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{var} is not set")]
    NotConfigured { var: &'static str },
    #[error(transparent)]
    Endpoint(#[from] EndpointError),
}

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("cannot reach the summary model at {addr}: {source}")]
    Io {
        addr: SocketAddr,
        source: std::io::Error,
    },
    #[error(
        "the summary model answered with a redirect (HTTP {status}); redirects are not followed"
    )]
    Redirect { status: u16 },
    #[error("the summary model answered HTTP {status}")]
    Status { status: u16 },
    #[error("the summary model's answer is unusable: {0}")]
    BadResponse(&'static str),
}

/// An OpenAI-compatible chat client bound to one loopback endpoint.
#[derive(Debug, Clone)]
pub struct ModelClient {
    endpoint: Endpoint,
    model: String,
}

impl ModelClient {
    pub fn new(endpoint: Endpoint, model: &str) -> Self {
        Self {
            endpoint,
            model: model.to_owned(),
        }
    }

    /// Both variables must be set; there is no default endpoint.
    pub fn from_vars(vars: &dyn Vars) -> Result<Self, ConfigError> {
        let url = vars
            .var(URL_VAR)
            .ok_or(ConfigError::NotConfigured { var: URL_VAR })?;
        let model = vars
            .var(MODEL_VAR)
            .ok_or(ConfigError::NotConfigured { var: MODEL_VAR })?;
        Ok(Self::new(Endpoint::parse(&url)?, &model))
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// One chat completion over the session's task and left-off text.
    pub fn summarize(
        &self,
        task: Option<&str>,
        left_off: Option<&str>,
    ) -> Result<String, ModelError> {
        self.complete(SYSTEM_PROMPT, &user_prompt(task, left_off))
    }

    /// One chat completion over a project's notes and its sessions' lines.
    pub fn summarize_project(
        &self,
        notes: Option<&str>,
        sessions: &[String],
    ) -> Result<String, ModelError> {
        self.complete(PROJECT_SYSTEM_PROMPT, &project_prompt(notes, sessions))
    }

    fn complete(&self, system: &str, user: &str) -> Result<String, ModelError> {
        let body = json!({
            "model": self.model,
            "stream": false,
            "temperature": SUMMARY_TEMPERATURE,
            "max_tokens": SUMMARY_MAX_TOKENS,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
        })
        .to_string();
        let raw = self.exchange(&body)?;
        parse_completion(&raw)
    }

    /// Sends one request and returns the whole response. HTTP/1.0 with
    /// `Connection: close` keeps the answer unchunked and ends it at EOF.
    fn exchange(&self, body: &str) -> Result<Vec<u8>, ModelError> {
        let addr = self.endpoint.addr;
        let io = |source| ModelError::Io { addr, source };
        let mut stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).map_err(io)?;
        stream.set_read_timeout(Some(READ_TIMEOUT)).map_err(io)?;
        stream.set_write_timeout(Some(WRITE_TIMEOUT)).map_err(io)?;
        let request = format!(
            "POST {} HTTP/1.0\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.endpoint.chat_path(),
            body.len(),
        );
        stream.write_all(request.as_bytes()).map_err(io)?;
        let mut raw = Vec::new();
        stream
            .take(MAX_RESPONSE_BYTES.saturating_add(1))
            .read_to_end(&mut raw)
            .map_err(io)?;
        if u64::try_from(raw.len()).map_or(true, |len| len > MAX_RESPONSE_BYTES) {
            return Err(ModelError::BadResponse("the answer is too large"));
        }
        Ok(raw)
    }
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// A local model can take a while on a long session.
const READ_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;
const SUMMARY_MAX_TOKENS: u32 = 300;
const SUMMARY_TEMPERATURE: f64 = 0.2;
/// Characters of each verbatim field sent to the model.
const MAX_PROMPT_FIELD_CHARS: usize = 4000;
const SYSTEM_PROMPT: &str = "You summarise a coding-agent session for the person who ran it. \
Reply in at most three plain sentences: what the session set out to do and where it stopped. \
Use only the text given; do not invent details.";

const PROJECT_SYSTEM_PROMPT: &str = "You summarise a project, a group of coding-agent sessions, \
for the person who runs it. Reply in at most three plain sentences: what the project is for and \
where its sessions stand. Use only the text given; do not invent details.";

/// Sessions listed in a project prompt; the newest come first.
pub const MAX_PROJECT_PROMPT_SESSIONS: usize = 20;

/// Characters of each session's task or left-off line in a project prompt.
const MAX_PROJECT_LINE_CHARS: usize = 400;

fn project_prompt(notes: Option<&str>, sessions: &[String]) -> String {
    let notes: String = notes
        .map(|t| t.chars().take(MAX_PROMPT_FIELD_CHARS).collect())
        .unwrap_or_default();
    let notes = if notes.is_empty() { "(none)" } else { &notes };
    let listed: Vec<&str> = sessions
        .iter()
        .take(MAX_PROJECT_PROMPT_SESSIONS)
        .map(String::as_str)
        .collect();
    let sessions = if listed.is_empty() {
        "(none)".to_owned()
    } else {
        listed.join("\n")
    };
    format!("Project notes:\n{notes}\n\nSessions:\n{sessions}")
}

/// One session's verbatim lines for a project prompt, each cut to size.
pub fn project_session_line(task: Option<&str>, left_off: Option<&str>) -> Option<String> {
    let cut = |text: &str| -> String { text.chars().take(MAX_PROJECT_LINE_CHARS).collect() };
    let parts: Vec<String> = [("task", task), ("left off", left_off)]
        .into_iter()
        .filter_map(|(label, text)| text.map(|t| format!("{label}: {}", cut(t))))
        .collect();
    (!parts.is_empty()).then(|| format!("- {}", parts.join("; ")))
}

fn user_prompt(task: Option<&str>, left_off: Option<&str>) -> String {
    let field = |label: &str, text: Option<&str>| {
        let text: String = text
            .map(|t| t.chars().take(MAX_PROMPT_FIELD_CHARS).collect())
            .unwrap_or_default();
        let text = if text.is_empty() { "(none)" } else { &text };
        format!("{label}:\n{text}")
    };
    format!(
        "{}\n\n{}",
        field("Opening request", task),
        field("Latest agent message", left_off)
    )
}

fn parse_completion(raw: &[u8]) -> Result<String, ModelError> {
    let head_end = raw
        .windows(HEAD_END.len())
        .position(|w| w == HEAD_END)
        .ok_or(ModelError::BadResponse("no HTTP header in the answer"))?;
    let head = String::from_utf8_lossy(raw.get(..head_end).unwrap_or_default());
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or(ModelError::BadResponse("no HTTP status in the answer"))?;
    match status {
        200..=299 => {}
        300..=399 => return Err(ModelError::Redirect { status }),
        _ => return Err(ModelError::Status { status }),
    }
    let body = raw
        .get(head_end.saturating_add(HEAD_END.len())..)
        .unwrap_or_default();
    let value: Value = serde_json::from_slice(body).map_err(|_| {
        ModelError::BadResponse("the body is not JSON (chunked answers are unsupported)")
    })?;
    let text = value
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or(ModelError::BadResponse("no message text in the answer"))?;
    Ok(text.to_owned())
}

const HEAD_END: &[u8] = b"\r\n\r\n";
