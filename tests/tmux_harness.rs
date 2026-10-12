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
