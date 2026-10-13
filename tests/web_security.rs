//! The security rules of `contracts/web-api.md`, over a real loopback socket:
//! Host allow-list, single-use login, sessions, CSRF, throttling.
#![cfg(feature = "web")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod web_support;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use claude_monitor::feed::{ChangeFeed, FakeSource, SystemMonotonic};
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::web::{self, WebConfig, WebError};
use tempfile::TempDir;
use web_support::{cookie_pair, Request, Response, Rig};

const HOUR: Duration = Duration::from_secs(3600);
const UNAUTHORIZED_BODY: &str = "unauthorized\n";

fn authed(rig: &Rig, path: &str) -> Response {
    Request::get(path)
        .header("Cookie", rig.cookie())
        .send(rig.addr())
}

fn login(rig: &Rig, code: &str) -> Response {
    Request::get(&format!("/login?code={code}")).send(rig.addr())
}

fn throttled_rig() -> Rig {
    Rig::start_with(|c| {
        c.max_auth_failures = 5;
        c.failure_window = Duration::from_secs(60);
    })
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_under(&path, out);
        } else if path.is_file() {
            out.push(path);
        }
    }
}

#[test]
fn binds_to_loopback_on_the_port_it_was_given() {
    let rig = Rig::start();

    assert!(rig.addr().ip().is_loopback(), "{}", rig.addr());
    assert_ne!(rig.addr().port(), 0, "port 0 must resolve to a real port");
    assert_eq!(authed(&rig, "/healthz").status, 200);
}

#[test]
fn a_non_loopback_bind_is_refused_outright() {
    for bind in [
        IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
    ] {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("state.db");
        let (source, _wake) = FakeSource::new();
        let feed = ChangeFeed::new(
            Store::open(&db, &StoreOptions::default()).unwrap(),
            source,
            SystemMonotonic,
        )
        .unwrap();
        let mut config = WebConfig::new(db);
        config.bind = bind;
        config.port = 0;

        let result = web::start(config, feed);

        assert!(
            matches!(result, Err(WebError::NotLoopback { addr }) if addr == bind),
            "{bind}: {result:?}"
        );
    }
}

#[test]
fn a_foreign_host_gets_421_even_with_a_valid_session() {
    let rig = Rig::start();
    let port = rig.addr().port();

    for host in [
        "evil.example".to_owned(),
        format!("evil.example:{port}"),
        format!("127.0.0.1:{}", port.wrapping_add(1)),
        "127.0.0.1".to_owned(),
        format!("192.168.1.10:{port}"),
        format!("localhost.evil.example:{port}"),
    ] {
        let response = Request::get("/healthz")
            .host(host.clone())
            .header("Cookie", rig.cookie())
            .send(rig.addr());
        assert_eq!(response.status, 421, "{host}");
    }
}

#[test]
fn the_loopback_names_with_the_right_port_are_allowed() {
    let rig = Rig::start();
    let port = rig.addr().port();

    for host in [
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        format!("LOCALHOST:{port}"),
        format!("[::1]:{port}"),
    ] {
        let response = Request::get("/healthz")
            .host(host.clone())
            .header("Cookie", rig.cookie())
            .send(rig.addr());
        assert_eq!(response.status, 200, "{host}");
    }
}

#[test]
fn a_request_without_a_session_gets_a_401_that_says_nothing() {
    let rig = Rig::start();

    for path in ["/healthz", "/", "/events", "/nonexistent"] {
        let response = Request::get(path).send(rig.addr());
        assert_eq!(response.status, 401, "{path}");
        assert_eq!(response.body, UNAUTHORIZED_BODY, "{path}");
    }
}

#[test]
fn a_made_up_cookie_gets_the_same_401() {
    let rig = Rig::start();
    let real = rig.session.cookie.split_once('=').unwrap().1.to_owned();
    let name = rig.session.cookie.split_once('=').unwrap().0.to_owned();

    for value in [
        "0".repeat(64),
        real[..8].to_owned(),
        format!("{real}0"),
        rig.csrf().to_owned(),
        String::new(),
    ] {
        let response = Request::get("/healthz")
            .header("Cookie", format!("{name}={value}"))
            .send(rig.addr());
        assert_eq!(response.status, 401, "{value}");
        assert_eq!(response.body, UNAUTHORIZED_BODY);
        assert!(response.header("set-cookie").is_none());
    }
}

#[test]
fn the_old_token_mechanism_is_gone() {
    let rig = Rig::start();

    let by_query = Request::get("/healthz?token=anything").send(rig.addr());
    let old_cookie = Request::get("/healthz")
        .header("Cookie", "cc_token=anything")
        .send(rig.addr());

    assert_eq!(by_query.status, 401);
    assert!(by_query.header("set-cookie").is_none());
    assert_eq!(old_cookie.status, 401);
    assert!(!rig.dir.path().join("web.token").exists());
}

#[test]
fn the_login_url_sets_a_hardened_session_cookie_and_redirects_clean() {
    let rig = Rig::start();
    let code = rig.server.issue_login_code().unwrap();

    let response = login(&rig, &code);

    assert_eq!(response.status, 303, "{response:?}");
    assert_eq!(response.header("location"), Some("/"));
    let cookies = response.headers_named("set-cookie");
    assert_eq!(cookies.len(), 1, "{cookies:?}");
    let cookie = cookies[0];
    for flag in ["Secure", "HttpOnly", "SameSite=Strict", "Path=/"] {
        assert!(
            cookie.split("; ").any(|part| part == flag),
            "{flag}: {cookie}"
        );
    }
    assert!(!cookie.contains("Domain="), "{cookie}");
    let pair = cookie_pair(&response).unwrap();
    let value = pair.split_once('=').unwrap().1;
    assert_eq!(value.len(), 64);
    assert!(value.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(value, code.as_str(), "the cookie must not be the code");
    let health = Request::get("/healthz")
        .header("Cookie", pair)
        .send(rig.addr());
    assert_eq!(health.status, 200);
}

#[test]
fn a_login_code_works_exactly_once() {
    let rig = Rig::start();
    let code = rig.server.issue_login_code().unwrap();

    let first = login(&rig, &code);
    let replay = login(&rig, &code);

    assert_eq!(first.status, 303);
    assert_eq!(replay.status, 401);
    assert_eq!(replay.body, UNAUTHORIZED_BODY);
    assert!(replay.header("set-cookie").is_none());
}

#[test]
fn a_login_code_expires_after_five_minutes() {
    let rig = Rig::start();
    let code = rig.server.issue_login_code().unwrap();

    rig.ticks.advance(Duration::from_secs(300));
    let response = login(&rig, &code);

    assert_eq!(response.status, 401);
}

#[test]
fn a_login_code_just_inside_five_minutes_still_works() {
    let rig = Rig::start();
    let code = rig.server.issue_login_code().unwrap();

    rig.ticks.advance(Duration::from_secs(299));

    assert_eq!(login(&rig, &code).status, 303);
}

#[test]
fn neither_the_code_nor_the_session_secrets_are_ever_written_to_disk() {
    let rig = Rig::start();
    let code = rig.server.issue_login_code().unwrap().to_string();
    let response = login(&rig, &code);
    let pair = cookie_pair(&response).unwrap();
    let cookie_value = pair.split_once('=').unwrap().1.to_owned();
    let csrf = rig.csrf().to_owned();
    let mut files = Vec::new();
    files_under(rig.dir.path(), &mut files);
    assert!(files.len() >= 4, "{files:?}");

    for file in files {
        let bytes = std::fs::read(&file).unwrap();
        for (what, secret) in [("code", &code), ("cookie", &cookie_value), ("csrf", &csrf)] {
            assert!(
                !bytes.windows(secret.len()).any(|w| w == secret.as_bytes()),
                "{what} found in {}",
                file.display()
            );
        }
    }
}

#[test]
fn a_session_goes_idle_after_eight_hours_but_use_keeps_it_alive() {
    let rig = Rig::start();

    for _ in 0..3 {
        rig.ticks.advance(HOUR * 7);
        assert_eq!(authed(&rig, "/healthz").status, 200);
    }
    rig.ticks.advance(HOUR * 8);

    assert_eq!(authed(&rig, "/healthz").status, 401);
}

#[test]
fn a_session_ends_24_hours_after_login_even_when_busy() {
    let rig = Rig::start();

    for _ in 0..3 {
        rig.ticks.advance(HOUR * 7);
        assert_eq!(authed(&rig, "/healthz").status, 200);
    }
    rig.ticks.advance(HOUR * 4);

    assert_eq!(authed(&rig, "/healthz").status, 401);
}

#[test]
fn a_post_needs_the_csrf_value_of_its_own_session() {
    let rig = Rig::start();
    let other = rig.another_session();
    assert_ne!(rig.csrf(), other.csrf, "one CSRF value per session");
    let cookie_value = rig.session.cookie.split_once('=').unwrap().1;
    assert_ne!(rig.csrf(), cookie_value);

    let no_header = Request::new("POST", "/healthz")
        .header("Cookie", rig.cookie())
        .send(rig.addr());
    let anothers = Request::new("POST", "/healthz")
        .header("Cookie", rig.cookie())
        .header("X-CC-Token", other.csrf.clone())
        .send(rig.addr());
    let cookie_as_header = Request::new("POST", "/healthz")
        .header("Cookie", rig.cookie())
        .header("X-CC-Token", cookie_value)
        .send(rig.addr());
    let no_cookie = Request::new("POST", "/healthz")
        .header("X-CC-Token", rig.csrf())
        .send(rig.addr());
    let good = Request::new("POST", "/healthz")
        .header("Cookie", rig.cookie())
        .header("X-CC-Token", rig.csrf())
        .send(rig.addr());

    assert_eq!(no_header.status, 403);
    assert_eq!(anothers.status, 403);
    assert_eq!(cookie_as_header.status, 403);
    assert_eq!(no_cookie.status, 401);
    assert_eq!(
        good.status, 405,
        "past the guards, /healthz only answers GET"
    );
}

#[test]
fn five_failed_logins_a_minute_then_429_with_retry_after() {
    let rig = throttled_rig();

    for attempt in 0..5 {
        assert_eq!(login(&rig, "bad").status, 401, "attempt {attempt}");
    }
    let blocked = login(&rig, "bad");

    assert_eq!(blocked.status, 429);
    let wait: u64 = blocked.header("retry-after").unwrap().parse().unwrap();
    assert!((1..=60).contains(&wait), "{wait}");
    assert_eq!(blocked.header("cache-control"), Some("no-store"));
}

#[test]
fn unauthenticated_requests_elsewhere_do_not_use_up_the_login_budget() {
    let rig = throttled_rig();
    let code = rig.server.issue_login_code().unwrap().to_string();

    for _ in 0..8 {
        assert_eq!(Request::get("/x").send(rig.addr()).status, 401);
    }

    assert_eq!(
        login(&rig, &code).status,
        303,
        "cookieless requests from another origin must not lock the owner out"
    );
}

#[test]
fn a_valid_code_is_refused_while_throttled_and_not_spent() {
    let rig = throttled_rig();
    let code = rig.server.issue_login_code().unwrap().to_string();
    for _ in 0..5 {
        login(&rig, "bad");
    }

    assert_eq!(login(&rig, &code).status, 429);
    rig.ticks.advance(Duration::from_secs(60));

    assert_eq!(login(&rig, &code).status, 303, "recovered, code unspent");
}

#[test]
fn a_signed_in_browser_is_not_throttled() {
    let rig = throttled_rig();
    for _ in 0..8 {
        login(&rig, "bad");
    }

    assert_eq!(authed(&rig, "/healthz").status, 200);
}

#[test]
fn no_response_carries_cors_headers_and_all_are_no_store() {
    let rig = Rig::start();
    let port = rig.addr().port();
    let origin = "https://evil.example";

    let responses = [
        Request::get("/healthz")
            .header("Cookie", rig.cookie())
            .header("Origin", origin)
            .send(rig.addr()),
        Request::get("/healthz")
            .header("Origin", origin)
            .send(rig.addr()),
        Request::get("/healthz")
            .host(format!("evil.example:{port}"))
            .header("Origin", origin)
            .send(rig.addr()),
        Request::new("OPTIONS", "/healthz")
            .header("Origin", origin)
            .header("Access-Control-Request-Method", "POST")
            .send(rig.addr()),
        Request::new("OPTIONS", "/healthz")
            .header("Cookie", rig.cookie())
            .header("Origin", origin)
            .header("Access-Control-Request-Method", "POST")
            .send(rig.addr()),
    ];

    for response in &responses {
        let cors: Vec<_> = response
            .headers
            .iter()
            .filter(|(name, _)| name.to_ascii_lowercase().starts_with("access-control-"))
            .collect();
        assert!(cors.is_empty(), "{cors:?} on {}", response.status);
        assert_eq!(response.header("cache-control"), Some("no-store"));
    }
}

#[test]
fn every_response_restricts_content_to_its_own_origin() {
    let rig = Rig::start();

    for response in [
        authed(&rig, "/"),
        Request::get("/").send(rig.addr()),
        Request::get("/healthz")
            .host("evil.example")
            .send(rig.addr()),
    ] {
        let csp = response.header("content-security-policy").unwrap();
        assert!(csp.contains("default-src 'self'"), "{csp}");
        assert!(!csp.contains("script-src 'unsafe"), "{csp}");
        assert_eq!(response.header("referrer-policy"), Some("no-referrer"));
    }
}

#[test]
fn no_route_serves_a_conversation_transcript() {
    let rig = Rig::start();

    for path in [
        "/transcript",
        "/transcripts",
        "/transcripts/abc",
        "/sessions/abc/transcript",
        "/sessions/abc/conversation",
        "/sessions/abc/messages",
    ] {
        let response = authed(&rig, path);
        assert_eq!(response.status, 404, "{path}");
    }
}
