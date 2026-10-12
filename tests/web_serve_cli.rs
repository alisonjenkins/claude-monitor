//! `claude-monitor serve` as a process: the printed URL, the token file and
//! the cookie hand-off over a real loopback socket.
#![cfg(feature = "web")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod web_support;

use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Stdio};

use tempfile::TempDir;
use web_support::Request;

struct Serving {
    child: Child,
    addr: SocketAddr,
    token: String,
    state: TempDir,
}

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts `serve --port 0` against a temporary state directory and reads the
/// URL it prints.
fn serve() -> Serving {
    let state = TempDir::new().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_claude-monitor"))
        .args(["--no-refresh", "--state-dir"])
        .arg(state.path())
        .args(["serve", "--port", "0"])
        .env("HOME", state.path())
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("COPILOT_HOME")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let url = line.trim().strip_prefix("http://").unwrap_or_else(|| {
        panic!("expected a URL on the first line of stdout, got {line:?}");
    });
    let (authority, rest) = url.split_once('/').unwrap();
    let token = rest.strip_prefix("?token=").unwrap().to_owned();
    Serving {
        child,
        addr: authority.parse().unwrap(),
        token,
        state,
    }
}

#[test]
fn serve_prints_a_loopback_url_whose_token_opens_the_page() {
    let serving = serve();

    assert!(serving.addr.ip().is_loopback());
    let first = Request::get(&format!("/?token={}", serving.token)).send(serving.addr);
    assert_eq!(first.status, 303);
    assert_eq!(first.header("location"), Some("/"));
    let health = Request::get("/healthz")
        .header("Cookie", format!("cc_token={}", serving.token))
        .send(serving.addr);
    assert_eq!(health.status, 200);
    assert_eq!(Request::get("/healthz").send(serving.addr).status, 401);
}

#[test]
fn serve_keeps_its_token_in_a_private_file_in_the_state_dir() {
    let serving = serve();

    let path = serving.state.path().join("web.token");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap().trim(),
        serving.token
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn serve_refuses_a_non_loopback_bind() {
    let out = Command::new(env!("CARGO_BIN_EXE_claude-monitor"))
        .args(["serve", "--bind", "0.0.0.0"])
        .output()
        .unwrap();

    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("loopback"));
    assert!(out.stdout.is_empty());
}

#[test]
fn serve_stops_cleanly_on_sigterm() {
    let mut serving = serve();

    // SAFETY: signals a child process this test spawned and still owns.
    let sent = unsafe { libc::kill(i32::try_from(serving.child.id()).unwrap(), libc::SIGTERM) };
    assert_eq!(sent, 0);

    let status = serving.child.wait().unwrap();
    assert_eq!(status.code(), Some(0));
}
