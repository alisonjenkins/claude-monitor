//! `claude-monitor serve` as a process: the printed login URL, the private
//! TLS files and the code-for-cookie hand-off over a real loopback socket.
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
    code: String,
    state: TempDir,
}

fn is_gone(pid: u32) -> bool {
    let pid = libc::pid_t::try_from(pid).unwrap();
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) != 0 }
}

#[test]
fn a_dropped_guard_kills_and_reaps_its_child() {
    let child = KillOnDrop::spawn(Process::new("sleep").arg("60")).unwrap();
    let pid = child.id();
    assert!(!is_gone(pid));

    drop(child);

    assert!(is_gone(pid));
}

#[cfg(target_os = "linux")]
const HELPER_ENV: &str = "CM_PDEATHSIG_HELPER";

/// Runs only as the child of `a_killed_test_runner_does_not_orphan_its_child`:
/// spawns a guarded `sleep`, prints its pid, then waits to be SIGKILLed.
#[cfg(target_os = "linux")]
#[test]
fn pdeathsig_helper() {
    if std::env::var_os(HELPER_ENV).is_none() {
        return;
    }
    let child = KillOnDrop::spawn(Process::new("sleep").arg("60")).unwrap();
    println!("pid={}", child.id());
    std::thread::sleep(std::time::Duration::from_secs(60));
}

#[cfg(target_os = "linux")]
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
        // SAFETY: plain kill(2) on a pid this test spawned.
        unsafe {
            libc::kill(libc::pid_t::try_from(pid).unwrap(), libc::SIGKILL);
        }
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
    let code = rest.strip_prefix("login?code=").unwrap().to_owned();
    let addr: std::net::SocketAddr = authority.parse().unwrap();
    web_support::trust(addr.port(), &state.path().join("tls").join("ca.pem"));
    Serving {
        child,
        addr,
        code,
        state,
    }
}

#[test]
fn serve_prints_a_loopback_login_url_whose_code_opens_the_page_once() {
    let serving = serve();

    assert!(serving.addr.ip().is_loopback());
    assert_eq!(serving.code.len(), 64);
    let first = Request::get(&format!("/login?code={}", serving.code)).send(serving.addr);
    assert_eq!(first.status, 303);
    assert_eq!(first.header("location"), Some("/"));
    let cookie = web_support::cookie_pair(&first).unwrap();
    let health = Request::get("/healthz")
        .header("Cookie", cookie)
        .send(serving.addr);
    assert_eq!(health.status, 200);
    assert_eq!(Request::get("/healthz").send(serving.addr).status, 401);
    let replay = Request::get(&format!("/login?code={}", serving.code)).send(serving.addr);
    assert_eq!(replay.status, 401);
}

#[test]
fn serve_writes_no_token_file_and_keeps_its_tls_files_private() {
    let serving = serve();
    let state = serving.state.path();

    assert!(!state.join("web.token").exists());
    let mode =
        |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&state.join("tls")), 0o700);
    for file in ["ca.pem", "ca.key", "server.pem", "server.key"] {
        assert_eq!(mode(&state.join("tls").join(file)), 0o600, "{file}");
    }
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
