//! Private tmux server harness. Every test gets its own socket in its own
//! tempdir and the server is killed on drop, so the user's tmux is never
//! touched. Tests skip with a message when tmux is not installed.
// Panicking on bad test setup is the correct failure mode here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use claude_monitor::env::{SystemTmux, Tmux};
use tempfile::TempDir;

pub struct TmuxServer {
    _dir: TempDir,
    socket: PathBuf,
}

impl TmuxServer {
    /// A harness with no server running yet, or `None` when tmux is absent.
    pub fn start() -> Option<Self> {
        if Command::new("tmux").arg("-V").output().is_err() {
            assert!(
                std::env::var_os("CLAUDE_MONITOR_REQUIRE_TMUX").is_none(),
                "tmux is required but not installed"
            );
            eprintln!("skipping: tmux is not installed");
            return None;
        }
        let dir = TempDir::new().unwrap();
        let socket = dir.path().join("sock");
        Some(Self { _dir: dir, socket })
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    pub fn tmux(&self, args: &[&str]) -> Output {
        Command::new("tmux")
            .arg("-S")
            .arg(&self.socket)
            .args(["-f", "/dev/null"])
            .args(args)
            .env("TERM", "dumb")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .output()
            .unwrap()
    }

    /// Stdout of a command that must succeed.
    pub fn run(&self, args: &[&str]) -> String {
        let out = self.tmux(args);
        assert!(
            out.status.success(),
            "tmux {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }
}

impl Drop for TmuxServer {
    fn drop(&mut self) {
        let _ = self.tmux(&["kill-server"]);
    }
}

#[test]
fn a_private_server_starts_runs_a_command_and_is_killed_on_drop() {
    let Some(server) = TmuxServer::start() else {
        return;
    };
    let cwd = TempDir::new().unwrap();
    let cwd_str = cwd.path().to_str().unwrap();
    server.run(&["new-session", "-d", "-s", "probe", "-c", cwd_str]);
    assert_eq!(
        server.run(&["list-sessions", "-F", "#{session_name}"]),
        "probe"
    );

    let pane = server.run(&["list-panes", "-a", "-F", "#{pane_id}"]);
    assert!(pane.starts_with('%'), "pane id: {pane}");

    let socket = server.socket().to_owned();
    drop(server);
    let after = Command::new("tmux")
        .arg("-S")
        .arg(&socket)
        .args(["-f", "/dev/null", "list-sessions"])
        .env("TERM", "dumb")
        .output()
        .unwrap();
    assert!(!after.status.success(), "server survived drop");
}

#[test]
fn the_harness_never_sees_the_users_server() {
    let Some(server) = TmuxServer::start() else {
        return;
    };
    let out = server.tmux(&["list-sessions"]);
    assert!(
        !out.status.success(),
        "a fresh private socket must have no server"
    );
}

fn sleep_argv() -> Vec<String> {
    vec!["sleep".to_owned(), "300".to_owned()]
}

#[test]
fn a_spawned_window_lands_in_the_given_cwd_with_the_given_label() {
    let Some(server) = TmuxServer::start() else {
        return;
    };
    let tmux = SystemTmux::with_socket(server.socket());
    let cwd = TempDir::new().unwrap();
    let cwd_path = cwd.path().canonicalize().unwrap();
    tmux.ensure_session("agents", &cwd_path).unwrap();

    let win = tmux
        .spawn_window("agents", "my-proj", &cwd_path, &sleep_argv())
        .unwrap();

    assert!(win.pane_id.starts_with('%'), "pane id: {}", win.pane_id);
    assert!(
        win.window_id.starts_with('@'),
        "window id: {}",
        win.window_id
    );
    let expected = format!(
        "{}|my-proj|{}|sleep",
        win.window_id,
        cwd_path.to_str().unwrap()
    );
    // Until the child execs, tmux reports its own cwd and name for the pane.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let info = loop {
        let info = server.run(&[
            "display-message",
            "-p",
            "-t",
            &win.pane_id,
            "#{window_id}|#{window_name}|#{pane_current_path}|#{pane_current_command}",
        ]);
        if info == expected || std::time::Instant::now() > deadline {
            break info;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert_eq!(info, expected);
}

#[test]
fn five_windows_in_a_row_get_distinct_ids() {
    let Some(server) = TmuxServer::start() else {
        return;
    };
    let tmux = SystemTmux::with_socket(server.socket());
    let cwd = TempDir::new().unwrap();
    tmux.ensure_session("agents", cwd.path()).unwrap();

    let wins: Vec<_> = (0..5)
        .map(|i| {
            tmux.spawn_window("agents", &format!("w{i}"), cwd.path(), &sleep_argv())
                .unwrap()
        })
        .collect();

    let ids: std::collections::BTreeSet<_> = wins.iter().map(|w| w.pane_id.clone()).collect();
    assert_eq!(ids.len(), 5);
    let names = server.run(&["list-windows", "-t", "agents", "-F", "#{window_name}"]);
    for i in 0..5 {
        assert!(names.lines().any(|n| n == format!("w{i}")), "{names}");
    }
    assert_eq!(
        names.lines().count(),
        6,
        "initial window plus five: {names}"
    );
}

#[test]
fn ensure_session_creates_when_absent_and_reuses_when_present() {
    let Some(server) = TmuxServer::start() else {
        return;
    };
    let tmux = SystemTmux::with_socket(server.socket());
    let cwd = TempDir::new().unwrap();

    assert!(tmux.ensure_session("agents", cwd.path()).unwrap());
    assert!(!tmux.ensure_session("agents", cwd.path()).unwrap());

    assert_eq!(
        server.run(&["list-sessions", "-F", "#{session_name}"]),
        "agents"
    );
    assert_eq!(
        server
            .run(&["list-windows", "-t", "agents"])
            .lines()
            .count(),
        1
    );
}

#[test]
fn an_existing_session_with_a_similar_name_is_not_reused() {
    let Some(server) = TmuxServer::start() else {
        return;
    };
    let tmux = SystemTmux::with_socket(server.socket());
    let cwd = TempDir::new().unwrap();
    server.run(&[
        "new-session",
        "-d",
        "-s",
        "agents-old",
        "-c",
        cwd.path().to_str().unwrap(),
    ]);

    assert!(tmux.ensure_session("agents", cwd.path()).unwrap());
}

#[test]
fn spawn_window_fails_when_the_session_is_missing() {
    let Some(server) = TmuxServer::start() else {
        return;
    };
    let tmux = SystemTmux::with_socket(server.socket());
    let cwd = TempDir::new().unwrap();

    let err = tmux
        .spawn_window("nope", "x", cwd.path(), &sleep_argv())
        .unwrap_err();
    assert!(err.to_string().starts_with("spawn tmux window:"), "{err}");
}
