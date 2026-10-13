//! The owner-only control socket a running `serve` listens on, and
//! `web login`, which asks it for a fresh login URL.
#![cfg(feature = "web")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod web_support;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::process::{Command as Process, Stdio};

use claude_monitor::web::control::{request_login_url, socket_path};
use tempfile::TempDir;
use web_support::{cookie_pair, KillOnDrop, Request, Rig};

const BIN: &str = env!("CARGO_BIN_EXE_claude-monitor");

fn code_of(url: &str) -> &str {
    url.split_once("/login?code=").unwrap().1
}

#[test]
fn the_socket_is_owner_only_and_a_real_unix_socket() {
    let rig = Rig::start();

    let path = socket_path(rig.dir.path());

    let meta = std::fs::metadata(&path).unwrap();
    assert!(meta.file_type().is_socket(), "{path:?}");
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
}

#[test]
fn asking_for_a_login_returns_a_url_that_logs_in_once() {
    let rig = Rig::start();

    let url = request_login_url(rig.dir.path()).unwrap();

    assert_eq!(
        url.as_str().split("/login").next().unwrap(),
        format!("https://127.0.0.1:{}", rig.addr().port())
    );
    let code = code_of(&url);
    assert_eq!(code.len(), 64);
    let first = Request::get(&format!("/login?code={code}")).send(rig.addr());
    let replay = Request::get(&format!("/login?code={code}")).send(rig.addr());
    assert_eq!(first.status, 303);
    assert!(cookie_pair(&first).is_some());
    assert_eq!(replay.status, 401);
}

#[test]
fn every_request_gets_a_fresh_code() {
    let rig = Rig::start();

    let a = request_login_url(rig.dir.path()).unwrap();
    let b = request_login_url(rig.dir.path()).unwrap();

    assert_ne!(code_of(&a), code_of(&b));
}

#[test]
fn anything_but_the_one_request_is_refused() {
    let rig = Rig::start();

    for line in ["frobnicate\n", "login extra\n", "\n", "GET / HTTP/1.1\n"] {
        let mut stream = UnixStream::connect(socket_path(rig.dir.path())).unwrap();
        stream.write_all(line.as_bytes()).unwrap();
        let mut reply = String::new();
        BufReader::new(stream).read_line(&mut reply).unwrap();
        assert!(reply.starts_with("err "), "{line:?} -> {reply:?}");
        assert!(!reply.contains("code="), "{reply}");
    }
}

#[test]
fn the_socket_is_removed_when_the_server_stops() {
    let rig = Rig::start();
    let path = socket_path(rig.dir.path());
    assert!(path.exists());
    let Rig { dir, server, .. } = rig;

    server.shutdown();

    assert!(!path.exists());
    drop(dir);
}

#[test]
fn a_second_server_on_the_same_state_dir_leaves_the_first_ones_socket_alone() {
    let first = Rig::start();
    let fake = claude_monitor::testing::FakeEnv::new();
    let second = Rig::start_on(first.dir.path(), fake.env.clone());

    let url = request_login_url(first.dir.path()).unwrap();

    assert!(
        url.contains(&format!("127.0.0.1:{}", first.addr().port())),
        "{}",
        url.as_str()
    );
    assert_ne!(first.addr().port(), second.addr().port());
}

#[test]
fn a_state_dir_too_deep_for_a_unix_socket_still_serves_just_without_web_login() {
    let root = TempDir::new().unwrap();
    let deep = root
        .path()
        .join("a".repeat(45))
        .join("b".repeat(45))
        .join("c".repeat(45));
    std::fs::create_dir_all(&deep).unwrap();
    let fake = claude_monitor::testing::FakeEnv::new();

    let rig = Rig::start_on(&deep, fake.env.clone());

    assert_eq!(
        Request::get("/healthz")
            .header("Cookie", rig.cookie())
            .send(rig.addr())
            .status,
        200
    );
    assert!(request_login_url(&deep).is_err());
}

#[test]
fn web_login_with_no_server_running_fails_and_prints_no_url() {
    let state = TempDir::new().unwrap();

    let output = Process::new(BIN)
        .args(["--no-refresh", "--state-dir"])
        .arg(state.path())
        .args(["web", "login"])
        .env("HOME", state.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("serve"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn web_login_asks_the_running_serve_for_a_fresh_url() {
    let state = TempDir::new().unwrap();
    let mut serve = KillOnDrop::spawn(
        Process::new(BIN)
            .args(["--no-refresh", "--state-dir"])
            .arg(state.path())
            .args(["serve", "--port", "0"])
            .env("HOME", state.path())
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("COPILOT_HOME")
            .stdout(Stdio::piped())
            .stderr(Stdio::null()),
    )
    .unwrap();
    let mut printed = String::new();
    BufReader::new(serve.stdout.take().unwrap())
        .read_line(&mut printed)
        .unwrap();
    let addr: std::net::SocketAddr = printed
        .trim()
        .strip_prefix("https://")
        .unwrap()
        .split('/')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    web_support::trust(addr.port(), &state.path().join("tls").join("ca.pem"));

    let output = Process::new(BIN)
        .args(["--no-refresh", "--state-dir"])
        .arg(state.path())
        .args(["web", "login"])
        .env("HOME", state.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let url = String::from_utf8(output.stdout).unwrap();
    let url = url.trim();
    assert!(
        url.starts_with(&format!("https://{addr}/login?code=")),
        "{url}"
    );
    assert_ne!(code_of(url), code_of(printed.trim()), "a fresh code");
    let login = Request::get(&format!("/login?code={}", code_of(url))).send(addr);
    assert_eq!(login.status, 303);
}
