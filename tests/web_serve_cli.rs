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
use std::process::{Command as Process, Stdio};

use tempfile::TempDir;
use web_support::{KillOnDrop, Request};

struct Serving {
    child: KillOnDrop,
    addr: SocketAddr,
    token: String,
    state: TempDir,
}

fn is_gone(pid: u32) -> bool {
    let pid = rustix::process::Pid::from_raw(i32::try_from(pid).unwrap()).unwrap();
    rustix::process::test_kill_process(pid).is_err()
}

#[test]
fn a_dropped_guard_kills_and_reaps_its_child() {
    let child = KillOnDrop::spawn(Process::new("sleep").arg("60")).unwrap();
    let pid = child.id();
    assert!(!is_gone(pid));

    drop(child);

    assert!(is_gone(pid));
}

const HELPER_ENV: &str = "CM_PDEATHSIG_HELPER";

/// Runs only as the child of `a_killed_test_runner_does_not_orphan_its_child`:
/// spawns a guarded `sleep`, prints its pid, then waits to be SIGKILLed.
#[test]
fn pdeathsig_helper() {
    if std::env::var_os(HELPER_ENV).is_none() {
        return;
    }
    let child = KillOnDrop::spawn(Process::new("sleep").arg("60")).unwrap();
    println!("pid={}", child.id());
    std::thread::sleep(std::time::Duration::from_secs(60));
}

#[test]
fn a_killed_test_runner_does_not_orphan_its_child() {
    let mut runner = Process::new(std::env::current_exe().unwrap())
        .args(["--exact", "pdeathsig_helper", "--nocapture"])
        .env(HELPER_ENV, "1")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(runner.stdout.take().unwrap());
    let pid = loop {
        let mut line = String::new();
        assert_ne!(
            reader.read_line(&mut line).unwrap(),
            0,
            "helper exited early"
        );
        if let Some(pid) = line.trim().strip_prefix("pid=") {
            break pid.parse::<u32>().unwrap();
        }
    };
    assert!(!is_gone(pid));

    runner.kill().unwrap();
    runner.wait().unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !is_gone(pid) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let gone = is_gone(pid);
    if !gone {
        let _ = rustix::process::kill_process(
            rustix::process::Pid::from_raw(i32::try_from(pid).unwrap()).unwrap(),
            rustix::process::Signal::KILL,
        );
    }
    assert!(gone, "child {pid} outlived its killed parent");
}

#[test]
fn a_panicking_test_does_not_leave_its_child_running() {
    let pid = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let seen = std::sync::Arc::clone(&pid);
    let result = std::panic::catch_unwind(move || {
        let child = KillOnDrop::spawn(Process::new("sleep").arg("60")).unwrap();
        seen.store(child.id(), std::sync::atomic::Ordering::SeqCst);
        panic!("test failed mid-way");
    });

    assert!(result.is_err());
    let pid = pid.load(std::sync::atomic::Ordering::SeqCst);
    assert_ne!(pid, 0);
    assert!(is_gone(pid));
}

/// Starts `serve --port 0` against a temporary state directory and reads the
/// URL it prints.
fn serve() -> Serving {
    let state = TempDir::new().unwrap();
    let mut child = KillOnDrop::spawn(
        Process::new(env!("CARGO_BIN_EXE_claude-monitor"))
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
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let url = line.trim().strip_prefix("https://").unwrap_or_else(|| {
        panic!("expected a URL on the first line of stdout, got {line:?}");
    });
    let (authority, rest) = url.split_once('/').unwrap();
    let token = rest.strip_prefix("?token=").unwrap().to_owned();
    let addr: std::net::SocketAddr = authority.parse().unwrap();
    web_support::trust(addr.port(), &state.path().join("tls").join("ca.pem"));
    Serving {
        child,
        addr,
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
    let out = Process::new(env!("CARGO_BIN_EXE_claude-monitor"))
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
