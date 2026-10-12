//! The bearer token and the checks every request passes before it is routed:
//! Host allow-list (DNS rebinding), token (URL once, then cookie), CSRF header
//! on anything that changes state.
//!
//! The token is generated per `serve` start and kept in a 0600 file. It reaches
//! the browser once, in the URL `serve` prints, and is traded there for an
//! `HttpOnly; SameSite=Strict` cookie and a redirect to a token-free URL.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use axum::extract::{Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, COOKIE, HOST, LOCATION, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::{AppState, WebError};

pub const TOKEN_FILE_NAME: &str = "web.token";
pub const COOKIE_NAME: &str = "cc_token";
pub const CSRF_HEADER: &str = "x-cc-token";

const TOKEN_BYTES: usize = 32;
const PRIVATE_FILE_MODE: u32 = 0o600;
const QUERY_TOKEN_KEY: &str = "token";

/// 256 bits from the OS RNG, hex encoded.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    pub fn generate() -> Result<Self, WebError> {
        let mut bytes = [0u8; TOKEN_BYTES];
        getrandom::fill(&mut bytes).map_err(|e| WebError::Random {
            detail: e.to_string(),
        })?;
        Ok(Self(bytes.iter().map(|b| format!("{b:02x}")).collect()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Compares without stopping at the first differing byte, so response time
    /// does not leak how much of a guess was right.
    fn matches(&self, candidate: &str) -> bool {
        let ours = self.0.as_bytes();
        let theirs = candidate.as_bytes();
        if ours.len() != theirs.len() {
            return false;
        }
        ours.iter()
            .zip(theirs)
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(..)")
    }
}

/// Writes a fresh token to `<dir>/web.token` (0600), replacing any old one. The
/// new file is private before it holds the token and is renamed into place.
pub fn issue_token_file(dir: &Path) -> Result<Token, WebError> {
    let token = Token::generate()?;
    let path = dir.join(TOKEN_FILE_NAME);
    let staging = dir.join(format!("{TOKEN_FILE_NAME}.new"));
    let fail = |source| WebError::TokenFile {
        path: path.clone(),
        source,
    };
    fs::create_dir_all(dir).map_err(fail)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(PRIVATE_FILE_MODE)
        .open(&staging)
        .map_err(fail)?;
    fs::set_permissions(&staging, fs::Permissions::from_mode(PRIVATE_FILE_MODE)).map_err(fail)?;
    writeln!(file, "{}", token.as_str()).map_err(fail)?;
    drop(file);
    fs::rename(&staging, &path).map_err(fail)?;
    Ok(token)
}

#[derive(Debug)]
pub struct Auth {
    token: Token,
    hosts: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Allow,
    /// Trade the URL token for the cookie and redirect to `location`.
    Handoff {
        location: String,
    },
    Reject(StatusCode, &'static str),
}

impl Auth {
    pub fn new(token: Token, port: u16) -> Self {
        Self {
            token,
            hosts: vec![
                format!("127.0.0.1:{port}"),
                format!("localhost:{port}"),
                format!("[::1]:{port}"),
            ],
        }
    }

    pub fn token(&self) -> &Token {
        &self.token
    }

    fn host_allowed(&self, headers: &HeaderMap) -> bool {
        headers
            .get(HOST)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|host| self.hosts.iter().any(|h| h.eq_ignore_ascii_case(host)))
    }

    fn judge(&self, method: &Method, uri: &Uri, headers: &HeaderMap) -> Verdict {
        if !self.host_allowed(headers) {
            return Verdict::Reject(StatusCode::MISDIRECTED_REQUEST, "unexpected Host header\n");
        }
        let reads = method == Method::GET || method == Method::HEAD;
        if reads {
            if let Some(offered) = query_token(uri) {
                return if self.token.matches(offered) {
                    Verdict::Handoff {
                        location: token_free_location(uri),
                    }
                } else {
                    Verdict::Reject(StatusCode::UNAUTHORIZED, UNAUTHORIZED)
                };
            }
        }
        let cookie = cookie_token(headers);
        if !cookie.is_some_and(|c| self.token.matches(c)) {
            return Verdict::Reject(StatusCode::UNAUTHORIZED, UNAUTHORIZED);
        }
        if !reads {
            let header = headers.get(CSRF_HEADER).and_then(|v| v.to_str().ok());
            if !header.is_some_and(|h| self.token.matches(h)) {
                return Verdict::Reject(
                    StatusCode::FORBIDDEN,
                    "missing or wrong X-CC-Token header\n",
                );
            }
        }
        Verdict::Allow
    }
}

const UNAUTHORIZED: &str = "unauthorized: open the URL that `claude-monitor serve` printed\n";

fn query_token(uri: &Uri) -> Option<&str> {
    uri.query()?.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == QUERY_TOKEN_KEY).then_some(value)
    })
}

/// The request's path and every query pair except the token, with a single
/// leading slash so the redirect can never name another origin.
fn token_free_location(uri: &Uri) -> String {
    let mut location = format!("/{}", uri.path().trim_start_matches('/'));
    let kept: Vec<&str> = uri
        .query()
        .unwrap_or_default()
        .split('&')
        .filter(|pair| !pair.is_empty() && pair.split('=').next() != Some(QUERY_TOKEN_KEY))
        .collect();
    if !kept.is_empty() {
        location.push('?');
        location.push_str(&kept.join("&"));
    }
    location
}

fn cookie_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|line| line.split(';'))
        .find_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name == COOKIE_NAME).then_some(value)
        })
}

fn text(status: StatusCode, body: &'static str) -> Response {
    let mut response = (status, body).into_response();
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

/// Runs before routing, so an unknown path is as unreadable as a known one
/// without the token. Every response, errors included, is `no-store`; the
/// server never adds CORS headers.
pub async fn guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let auth = &state.auth;
    let mut response = match auth.judge(request.method(), request.uri(), request.headers()) {
        Verdict::Allow => next.run(request).await,
        Verdict::Reject(status, body) => text(status, body),
        Verdict::Handoff { location } => handoff(auth, &location),
    };
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn handoff(auth: &Auth, location: &str) -> Response {
    let cookie = format!(
        "{COOKIE_NAME}={}; HttpOnly; SameSite=Strict; Path=/",
        auth.token.as_str()
    );
    let (Ok(cookie), Ok(location)) = (
        HeaderValue::from_str(&cookie),
        HeaderValue::from_str(location),
    ) else {
        return text(StatusCode::BAD_REQUEST, "cannot build the redirect\n");
    };
    let mut response = StatusCode::SEE_OTHER.into_response();
    response.headers_mut().insert(SET_COOKIE, cookie);
    response.headers_mut().insert(LOCATION, location);
    response
}
