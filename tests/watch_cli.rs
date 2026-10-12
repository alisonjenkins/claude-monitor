#![cfg(target_os = "linux")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

//! The real binary: `watch` sees a live agent through the Claude registry,
//! logs its state changes and exits cleanly on SIGTERM.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use claude_monitor::env::{Processes, SystemProcesses};
use serde_json::json;
use tempfile::TempDir;

const PATIENCE: Duration = Duration::from_secs(10);

fn lines_of(child: &mut Child) -> Receiver<String> {
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

fn line_containing(rx: &Receiver<String>, needle: &str) -> String {
    let start = Instant::now();
    while let Some(left) = PATIENCE.checked_sub(start.elapsed()) {
        match rx.recv_timeout(left) {
            Ok(line) if line.contains(needle) => return line,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    panic!("no line containing {needle:?}");
}

#[test]
#[ignore = "the watch subcommand is not wired"]
fn watch_logs_a_live_agent_and_exits_on_sigterm() {
    let home = TempDir::new().unwrap();
    let sessions = home.path().join(".claude/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::create_dir_all(home.path().join(".claude/projects")).unwrap();
    let mut agent = Command::new("sleep").arg("60").spawn().unwrap();
    let pid = agent.id();
    let start = SystemProcesses.start_time(pid).unwrap().unwrap();
    let entry = json!({
        "pid": pid, "sessionId": "sess-1", "cwd": "/tmp", "startedAt": 1_791_800_000_000_u64,
        "procStart": start.to_string(), "version": "2.1.293", "kind": "interactive",
        "entrypoint": "cli", "status": "idle",
    });
    std::fs::write(sessions.join(format!("{pid}.json")), entry.to_string()).unwrap();

    let mut watcher = Command::new(env!("CARGO_BIN_EXE_claude-monitor"))
        .args(["watch", "--state-dir"])
        .arg(home.path().join("state"))
        .env("HOME", home.path())
        .env("XDG_RUNTIME_DIR", home.path().join("run"))
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("COPILOT_HOME")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let lines = lines_of(&mut watcher);

    let first = line_containing(&lines, "idle (new)");
    assert!(first.contains(" claude "), "{first}");
    agent.kill().unwrap();
    agent.wait().unwrap();
    line_containing(&lines, "interrupted (was idle)");

    let signalled = unsafe { libc::kill(i32::try_from(watcher.id()).unwrap(), libc::SIGTERM) };
    assert_eq!(signalled, 0);
    let status = watcher.wait().unwrap();
    assert!(status.success(), "{status}");
}
