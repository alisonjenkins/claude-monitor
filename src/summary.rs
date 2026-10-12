//! Session summaries: the verbatim "task" and "left off" text taken from the
//! session itself, and the on-demand model-written summary kept apart from it
//! (FR-017, FR-017a, FR-020).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::net::SocketAddr;
use std::path::Path;

use serde_json::Value;

use crate::env::Vars;

/// Most bytes read from the end of a transcript when looking for the latest
/// assistant message; a transcript can run to many megabytes and a hook has a
/// 50 ms budget.
pub const TRANSCRIPT_TAIL_BYTES: u64 = 256 * 1024;

/// The latest assistant message in the last `max_bytes` of the transcript at
/// `path`, text only and untouched. `None` when there is none in that window or
/// the file cannot be read.
pub fn transcript_tail_message(path: &Path, max_bytes: u64) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    // A FIFO or device would block the hook.
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
    match stop_message {
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
        Err(EndpointError::Invalid {
            url: url.to_owned(),
            reason: "not implemented",
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
    pub fn from_vars(_vars: &dyn Vars) -> Result<Self, ConfigError> {
        Err(ConfigError::NotConfigured { var: URL_VAR })
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
        _task: Option<&str>,
        _left_off: Option<&str>,
    ) -> Result<String, ModelError> {
        Err(ModelError::BadResponse("not implemented"))
    }
}
