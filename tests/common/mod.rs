//! A private tmux server, a store and an `Env` wired to both, for the
//! respawn and restore command tests. The agent a respawn launches is
//! `sleep 30` (through the launcher variable), never a real agent.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use claude_monitor::agents::kinds;
use claude_monitor::env::{Env, SystemFs, SystemTmux};
use claude_monitor::model::{Session, State};
use claude_monitor::store::events::EventRow;
use claude_monitor::store::{legacy, Store, StoreOptions};
use claude_monitor::testing::{FakeEnv, FakeProcesses, FakeVars};
use tempfile::TempDir;

pub const T0: &str = "2026-10-12T10:00:00Z";

pub struct Fixture {
    pub dir: TempDir,
    pub state_dir: PathBuf,
    pub home: PathBuf,
    pub env: Env,
    pub vars: Arc<FakeVars>,
    pub processes: Arc<FakeProcesses>,
    socket: PathBuf,
}

impl Fixture {
    /// `None` (after a message) when tmux is not installed.
    pub fn new() -> Option<Self> {
        if Command::new("tmux").arg("-V").output().is_err() {
            assert!(
                std::env::var_os("CLAUDE_MONITOR_REQUIRE_TMUX").is_none(),
                "tmux is required but not installed"
            );
            eprintln!("skipping: tmux is not installed");
            return None;
        }
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let socket = root.join("sock");
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let fake = FakeEnv::new();
        fake.vars.set("HOME", home.to_str().unwrap());
        fake.vars.set("CLAUDE_MONITOR_LAUNCHER_CLAUDE", "sleep 30");
        let mut env = fake.env.clone();
        env.tmux = Arc::new(SystemTmux::with_socket(&socket));
        env.fs = Arc::new(SystemFs);
        // Record the legacy import as done so opening the store never reads
        // the real runtime directory.
        let state_dir = root.join("state");
        let store = Store::open_in_dir(&state_dir, &StoreOptions::viewer()).unwrap();
        let empty = root.join("no-legacy-sessions");
        std::fs::create_dir_all(&empty).unwrap();
        legacy::migrate(&store, &empty, fake.env.clock.as_ref()).unwrap();
        Some(Self {
            state_dir,
            home,
            env,
            vars: fake.vars.clone(),
            processes: fake.processes.clone(),
            socket,
            dir,
        })
    }

    pub fn root(&self) -> PathBuf {
        self.dir.path().canonicalize().unwrap()
    }

    pub fn store(&self) -> Store {
        Store::open_in_dir(&self.state_dir, &StoreOptions::viewer()).unwrap()
    }

    /// The `session.respawn_requested` events recorded so far.
    pub fn respawn_events(&self) -> Vec<EventRow> {
        self.store()
            .events_since(0, 1000)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "session.respawn_requested")
            .collect()
    }

    /// A session of `state` in a new directory `name` under the fixture root,
    /// with the transcript a resume needs.
    pub fn add_session(&self, id: &str, state: State, name: &str) -> Session {
        let cwd = self.root().join(name);
        std::fs::create_dir_all(&cwd).unwrap();
        let mut session = Session::new(
            id,
            "claude",
            &format!("agent-{id}"),
            cwd.to_str().unwrap(),
            T0,
        );
        session.state = state;
        self.save(&session);
        session
    }

    /// Store `session` and create the transcript a resume of it needs.
    pub fn save(&self, session: &Session) {
        self.store().upsert_session(session).unwrap();
        let kind = kinds().into_iter().find(|k| k.id() == "claude").unwrap();
        for need in kind.resume_preconditions(session, &self.env) {
            let parent = need.required_path.parent().unwrap();
            std::fs::create_dir_all(parent).unwrap();
            std::fs::write(&need.required_path, "{}\n").unwrap();
        }
    }

    pub fn tmux(&self, args: &[&str]) -> std::process::Output {
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

    /// True when the private server has any session.
    pub fn server_has_sessions(&self) -> bool {
        self.tmux(&["list-sessions"]).status.success()
    }

    /// `(cwd, command)` of a pane once its child has exec'd the stub.
    pub fn pane_cwd_and_command(&self, pane: &str) -> (String, String) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let out = self.tmux(&[
                "display-message",
                "-p",
                "-t",
                pane,
                "#{pane_current_path} #{pane_current_command}",
            ]);
            let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            if let Some((cwd, command)) = text.rsplit_once(' ') {
                if command == "sleep" || Instant::now() > deadline {
                    return (cwd.to_owned(), command.to_owned());
                }
            } else if Instant::now() > deadline {
                panic!("no pane info for {pane}: {text:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn pane_ids(&self) -> Vec<String> {
        let out = self.tmux(&["list-panes", "-a", "-F", "#{pane_id}"]);
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.tmux(&["kill-server"]);
    }
}

pub fn path_str(path: &Path) -> &str {
    path.to_str().unwrap()
}
