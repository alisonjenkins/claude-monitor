//! The security rules of `contracts/web-api.md`, over a real loopback socket.
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
use std::os::unix::fs::PermissionsExt;

use claude_monitor::feed::{ChangeFeed, FakeSource, SystemMonotonic};
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::web::auth::{issue_token_file, Token, TOKEN_FILE_NAME};
use claude_monitor::web::{self, WebConfig, WebError};
use tempfile::TempDir;
use web_support::{Request, Rig};

fn authed(rig: &Rig, path: &str) -> web_support::Response {
    Request::get(path)
        .header("Cookie", rig.cookie())
        .send(rig.addr())
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
        let mut config = WebConfig::new(Token::generate().unwrap(), db);
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
fn a_foreign_host_gets_421_even_with_a_valid_token() {
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
fn a_request_without_the_token_gets_401() {
    let rig = Rig::start();

    for path in ["/healthz", "/", "/events", "/nonexistent"] {
        let response = Request::get(path).send(rig.addr());
        assert_eq!(response.status, 401, "{path}");
    }
}

#[test]
fn a_wrong_cookie_or_wrong_query_token_gets_401() {
    let rig = Rig::start();
    let wrong = "0".repeat(rig.token.as_str().len());

    let by_cookie = Request::get("/healthz")
        .header("Cookie", format!("cc_token={wrong}"))
        .send(rig.addr());
    let by_query = Request::get(&format!("/healthz?token={wrong}")).send(rig.addr());
    let prefix = &rig.token.as_str()[..8];
    let by_prefix = Request::get("/healthz")
        .header("Cookie", format!("cc_token={prefix}"))
        .send(rig.addr());

    assert_eq!(by_cookie.status, 401);
    assert_eq!(by_query.status, 401);
    assert_eq!(by_prefix.status, 401);
    assert!(by_query.header("set-cookie").is_none());
}

#[test]
fn the_token_in_the_url_becomes_an_http_only_strict_cookie_and_a_clean_redirect() {
    let rig = Rig::start();
    let token = rig.token.as_str();

    let first = Request::get(&format!("/healthz?token={token}")).send(rig.addr());

    assert!(
        (302..=303).contains(&first.status) || first.status == 307,
        "{first:?}"
    );
    assert_eq!(first.header("location"), Some("/healthz"));
    let cookies = first.headers_named("set-cookie");
    assert_eq!(cookies.len(), 1, "{cookies:?}");
    let cookie = cookies[0];
    assert!(cookie.starts_with(&format!("cc_token={token}")), "{cookie}");
    assert!(cookie.contains("HttpOnly"), "{cookie}");
    assert!(cookie.contains("SameSite=Strict"), "{cookie}");
    assert!(cookie.contains("Path=/"), "{cookie}");

    let second = Request::get("/healthz")
        .header("Cookie", format!("cc_token={token}"))
        .send(rig.addr());
    assert_eq!(second.status, 200);
}

#[test]
fn the_redirect_keeps_the_other_query_parameters_and_never_leaves_the_origin() {
    let rig = Rig::start();
    let token = rig.token.as_str();

    let kept =
        Request::get(&format!("/sessions?state=idle&token={token}&text=db")).send(rig.addr());
    let root = Request::get(&format!("/?token={token}")).send(rig.addr());
    let sneaky = Request::get(&format!("//evil.example/x?token={token}")).send(rig.addr());

    assert_eq!(
        kept.header("location"),
        Some("/sessions?state=idle&text=db")
    );
    assert_eq!(root.header("location"), Some("/"));
    let location = sneaky.header("location").unwrap();
    assert!(
        location.starts_with('/') && !location.starts_with("//"),
        "{location}"
    );
}

#[test]
fn a_post_needs_the_csrf_header_matching_the_cookie() {
    let rig = Rig::start();
    let wrong = "f".repeat(rig.token.as_str().len());

    let no_header = Request::new("POST", "/healthz")
        .header("Cookie", rig.cookie())
        .send(rig.addr());
    let wrong_header = Request::new("POST", "/healthz")
        .header("Cookie", rig.cookie())
        .header("X-CC-Token", wrong)
        .send(rig.addr());
    let no_cookie = Request::new("POST", "/healthz")
        .header("X-CC-Token", rig.token.as_str())
        .send(rig.addr());
    let good = Request::new("POST", "/healthz")
        .header("Cookie", rig.cookie())
        .header("X-CC-Token", rig.token.as_str())
        .send(rig.addr());

    assert_eq!(no_header.status, 403);
    assert_eq!(wrong_header.status, 403);
    assert_eq!(no_cookie.status, 401);
    assert_eq!(
        good.status, 405,
        "past the guards, /healthz only answers GET"
    );
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

#[test]
fn the_token_file_is_private_and_holds_256_bits_of_hex() {
    let dir = TempDir::new().unwrap();

    let token = issue_token_file(dir.path()).unwrap();

    let path = dir.path().join(TOKEN_FILE_NAME);
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let contents = std::fs::read_to_string(&path).unwrap();
    assert_eq!(contents.trim(), token.as_str());
    assert_eq!(token.as_str().len(), 64);
    assert!(token.as_str().chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn the_token_rotates_on_every_start_and_a_loose_old_file_is_tightened() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join(TOKEN_FILE_NAME);
    std::fs::write(&path, "old").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

    let first = issue_token_file(dir.path()).unwrap();
    let second = issue_token_file(dir.path()).unwrap();

    assert_ne!(first, second);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap().trim(),
        second.as_str()
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn the_token_does_not_appear_in_debug_output() {
    let token = Token::generate().unwrap();
    assert!(!format!("{token:?}").contains(token.as_str()));
}
