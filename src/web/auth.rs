//! Everything a request passes before it is routed: Host allow-list (DNS
//! rebinding), login by single-use code, a server-side session behind an
//! opaque cookie, CSRF on anything that changes state, and a throttle on
//! failures.
//!
//! The only secret in a URL is a login code, good for one use within five
//! minutes. Visiting `/login?code=...` trades it for a session cookie holding
//! a random id; the id, the CSRF value and the code are three unrelated random
//! values and none is stored on disk.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, COOKIE, HOST, LOCATION, REFERRER_POLICY,
    RETRY_AFTER, SET_COOKIE, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use zeroize::Zeroizing;

use super::clock::Ticks;
use super::login::LoginCodes;
use super::secret::ct_eq;
use super::session::{NewSession, Sessions};
use super::throttle::Throttle;
use super::{AppState, WebError};

/// `__Host-` makes the browser refuse the cookie unless it is `Secure`, has
/// `Path=/` and names no `Domain`.
pub const COOKIE_NAME: &str = "__Host-cc_session";
pub const CSRF_HEADER: &str = "x-cc-token";
pub const LOGIN_PATH: &str = "/login";

const UNAUTHORIZED: &str = "unauthorized\n";
const LOGIN_CODE_KEY: &str = "code";

/// Scripts, fonts, images and connections only from this origin; inline styles
/// stay allowed because the progress bars set a width and htmx adds a style
/// rule, but no inline script ever runs.
const CONTENT_SECURITY_POLICY_VALUE: &str =
    "default-src 'self'; style-src 'self' 'unsafe-inline'; \
     base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

tokio::task_local! {
    static CSRF: String;
    static SESSION_ID: Zeroizing<String>;
}

/// The id of the session making the current request, for a long-lived
/// response to recheck. `None` outside a guarded request.
pub fn current_session_id() -> Option<Zeroizing<String>> {
    SESSION_ID.try_with(Clone::clone).ok()
}

/// The CSRF value of the session making the current request, for pages to
/// embed. Empty outside a guarded request.
pub fn current_csrf() -> String {
    CSRF.try_with(Clone::clone).unwrap_or_default()
}

pub struct Auth {
    hosts: Vec<String>,
    codes: LoginCodes,
    sessions: Sessions,
    throttle: Throttle,
}

enum Verdict {
    Allow {
        csrf: String,
        session_id: Zeroizing<String>,
    },
    LoggedIn(NewSession),
    Reject(StatusCode, &'static str),
    Throttled {
        wait: Duration,
    },
}

impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Auth").finish_non_exhaustive()
    }
}

impl Auth {
    pub fn new(
        port: u16,
        clock: Arc<dyn Ticks>,
        max_failures: usize,
        failure_window: Duration,
    ) -> Self {
        Self {
            hosts: vec![
                format!("127.0.0.1:{port}"),
                format!("localhost:{port}"),
                format!("[::1]:{port}"),
            ],
            codes: LoginCodes::new(Arc::clone(&clock)),
            sessions: Sessions::new(Arc::clone(&clock)),
            throttle: Throttle::new(clock, max_failures, failure_window),
        }
    }

    /// A fresh single-use login code.
    pub fn issue_login_code(&self) -> Result<Zeroizing<String>, WebError> {
        self.codes.issue()
    }

    /// Whether the session is still live, without counting as a use.
    pub fn session_is_live(&self, id: &str) -> bool {
        self.sessions.is_live(id)
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
        if method == Method::GET && uri.path() == LOGIN_PATH {
            return self.login(uri);
        }
        let session =
            cookie_value(headers).and_then(|id| self.sessions.lookup(id).map(|csrf| (csrf, id)));
        if let Some((csrf, id)) = session {
            if !reads {
                let offered = headers.get(CSRF_HEADER).and_then(|v| v.to_str().ok());
                if !offered.is_some_and(|o| ct_eq(&csrf, o)) {
                    return Verdict::Reject(StatusCode::FORBIDDEN, "forbidden\n");
                }
            }
            return Verdict::Allow {
                csrf,
                session_id: Zeroizing::new(id.to_owned()),
            };
        }
        self.refuse()
    }

    fn login(&self, uri: &Uri) -> Verdict {
        if let Err(wait) = self.throttle.check() {
            return Verdict::Throttled { wait };
        }
        let offered = login_code(uri);
        if !offered.is_some_and(|code| self.codes.redeem(&code)) {
            self.throttle.record_failure();
            return Verdict::Reject(StatusCode::UNAUTHORIZED, UNAUTHORIZED);
        }
        match self.sessions.create() {
            Ok(session) => Verdict::LoggedIn(session),
            Err(_) => Verdict::Reject(StatusCode::INTERNAL_SERVER_ERROR, "cannot log in\n"),
        }
    }

    /// An unauthenticated request is refused but not counted: any page can
    /// send cookieless requests to loopback, and counting them would let it
    /// spend the budget a real login needs.
    fn refuse(&self) -> Verdict {
        Verdict::Reject(StatusCode::UNAUTHORIZED, UNAUTHORIZED)
    }
}

fn login_code(uri: &Uri) -> Option<Zeroizing<String>> {
    form_urlencoded::parse(uri.query()?.as_bytes())
        .find(|(key, _)| key == LOGIN_CODE_KEY)
        .map(|(_, value)| Zeroizing::new(value.into_owned()))
}

fn cookie_value(headers: &HeaderMap) -> Option<&str> {
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
/// without a session. Every response, errors included, is `no-store` and
/// carries the content policy; the server never adds CORS headers.
pub async fn guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let auth = &state.auth;
    let mut response = match auth.judge(request.method(), request.uri(), request.headers()) {
        Verdict::Allow { csrf, session_id } => {
            SESSION_ID
                .scope(session_id, CSRF.scope(csrf, next.run(request)))
                .await
        }
        Verdict::Reject(status, body) => text(status, body),
        Verdict::LoggedIn(session) => logged_in(&session),
        Verdict::Throttled { wait } => too_many(wait),
    };
    let headers = response.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY_VALUE),
    );
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    response
}

fn too_many(wait: Duration) -> Response {
    let mut response = text(StatusCode::TOO_MANY_REQUESTS, "too many attempts\n");
    let seconds = wait
        .as_secs()
        .saturating_add(u64::from(wait.subsec_nanos() > 0));
    if let Ok(value) = HeaderValue::from_str(&seconds.max(1).to_string()) {
        response.headers_mut().insert(RETRY_AFTER, value);
    }
    response
}

fn logged_in(session: &NewSession) -> Response {
    let cookie = Zeroizing::new(format!(
        "{COOKIE_NAME}={}; Secure; HttpOnly; SameSite=Strict; Path=/",
        session.id.as_str()
    ));
    let Ok(mut cookie) = HeaderValue::from_str(&cookie) else {
        return text(StatusCode::INTERNAL_SERVER_ERROR, "cannot log in\n");
    };
    cookie.set_sensitive(true);
    let mut response = StatusCode::SEE_OTHER.into_response();
    response.headers_mut().insert(SET_COOKIE, cookie);
    response
        .headers_mut()
        .insert(LOCATION, HeaderValue::from_static("/"));
    response
}
