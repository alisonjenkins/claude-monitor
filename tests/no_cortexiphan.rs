//! SC-010: with no `cortexiphan` on `PATH`, the product still does everything
//! except read the other side of a link. The real binary runs with a `PATH`
//! that is an empty directory (so neither cortexiphan nor tmux resolves) over
//! a store with sessions, a project and a plan.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod export_support;

use std::process::{Command as Process, Output, Stdio};

use serde_json::Value;
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_claude-monitor");

struct Rig {
    state: TempDir,
    home: TempDir,
    empty_path: TempDir,
}

impl Rig {
    fn new() -> Self {
        let fixture = export_support::project20();
        let state = fixture.dir;
        Self {
            state,
            home: TempDir::new().unwrap(),
            empty_path: TempDir::new().unwrap(),
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        Process::new(BIN)
            .args(["--json", "--no-refresh", "--state-dir"])
            .arg(self.state.path())
            .args(args)
            .env_clear()
            .env("PATH", self.empty_path.path())
            .env("HOME", self.home.path())
            .env("XDG_RUNTIME_DIR", self.home.path())
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?} exited {:?}: {}{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }
}

#[test]
fn list_show_and_progress_work() {
    let rig = Rig::new();
    let list = rig.ok(&["list", "--include-dismissed", "--expand-finished"]);
    assert!(list.to_string().contains("sess-a"));
    let show = rig.ok(&["show", "sess-b"]);
    assert!(show.to_string().contains("sess-b"));
    let plan = rig.home.path().join("tasks.md");
    std::fs::write(&plan, "- [x] T001 first\n- [ ] T002 second\n").unwrap();
    rig.ok(&["progress", "bind", "sess-a", plan.to_str().unwrap()]);
    let progress = rig.ok(&["progress", "sess-a"]);
    assert!(progress.to_string().contains("T002"), "{progress}");
}

#[test]
fn restore_without_yes_prints_the_plan_and_starts_nothing() {
    let rig = Rig::new();
    let plan = rig.ok(&["restore", "--all-interrupted"]);
    assert!(plan.to_string().contains("sess-c"));
}

#[test]
fn the_organise_commands_work() {
    let rig = Rig::new();
    rig.ok(&["project", "add", "side"]);
    rig.ok(&["group", "side", "sess-a"]);
    rig.ok(&["note", "sess-a", "a note"]);
    rig.ok(&["park", "sess-a"]);
    rig.ok(&["unpark", "sess-a"]);
    rig.ok(&["dismiss", "sess-a"]);
    rig.ok(&["undismiss", "sess-a"]);
    rig.ok(&["done", "sess-a"]);
    rig.ok(&["undone", "sess-a"]);
    rig.ok(&["project", "set-status", "side", "parked"]);
}

#[test]
fn export_writes_json_lines() {
    let rig = Rig::new();
    let out = rig.home.path().join("export.jsonl");
    let out_arg = out.to_str().unwrap();
    rig.ok(&["export", "cortexiphan", "--out", out_arg]);
    let text = std::fs::read_to_string(&out).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines.len() > 20);
    for line in lines {
        serde_json::from_str::<Value>(line).unwrap();
    }
}

#[test]
fn links_check_reports_unreachable_without_failing() {
    let rig = Rig::new();
    rig.ok(&["link", "sess-a", "7", "--doc", "3"]);
    let check = rig.run(&["links", "check"]);
    assert!(
        check.status.success(),
        "links check exited {:?}: {}",
        check.status.code(),
        String::from_utf8_lossy(&check.stdout)
    );
    let text = String::from_utf8_lossy(&check.stdout).to_lowercase();
    assert!(
        text.contains("unreachable") || text.contains("skipped"),
        "{text}"
    );
}
