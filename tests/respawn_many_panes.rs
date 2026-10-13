//! Reopening more panes than a default detached window has room for, against
//! a real tmux server on a private socket.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::process::Command;
use std::sync::Arc;

use claude_monitor::env::{SystemTmux, Tmux};
use claude_monitor::model::{Session, State};
use claude_monitor::respawn::{plan_batch, start_batch, Item, PlanFlags};
use claude_monitor::store::{Store, StoreOptions};
use claude_monitor::testing::FakeEnv;
use tempfile::TempDir;

const AT: &str = "2026-10-12T00:00:00Z";
const PANES: u32 = 9;

struct Server(std::path::PathBuf);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .arg("-S")
            .arg(&self.0)
            .arg("kill-server")
            .output();
    }
}

#[test]
fn nine_panes_of_one_window_all_open_in_a_default_sized_detached_window() {
    let dir = TempDir::new().unwrap();
    let socket = dir.path().join("sock");
    let tmux = SystemTmux::with_socket(&socket);
    if !tmux.is_installed() {
        return;
    }
    let _server = Server(socket);

    let fake = FakeEnv::new();
    fake.vars.set("HOME", "/home/u");
    fake.vars.set("CLAUDE_MONITOR_LAUNCHER_CLAUDE", "sleep 60");
    let mut env = fake.env.clone();
    env.tmux = Arc::new(tmux);
    let store = Store::open_in_dir(dir.path(), &StoreOptions::default()).unwrap();
    let mut ids = Vec::new();
    for n in 0..PANES {
        let id = format!("s{n}");
        let cwd = format!("/w/{id}");
        fake.fs.add_dir(&cwd);
        fake.fs.add_file(
            format!("/home/u/.claude/projects/-w-{id}/agent-{id}.jsonl"),
            "{}\n",
        );
        let mut s = Session::new(&id, "claude", &format!("agent-{id}"), &cwd, AT);
        s.state = State::Interrupted;
        s.tmux_session = Some("work".to_owned());
        s.tmux_window_name = Some("api".to_owned());
        s.tmux_window_index = Some("1".to_owned());
        s.tmux_pane_index = Some(n.to_string());
        s.tmux_window_panes = Some(PANES.to_string());
        store.upsert_session(&s).unwrap();
        ids.push(id);
    }
    let real_cwd = dir.path().to_path_buf();

    let planned: Vec<_> = plan_batch(&store, &env, &ids, &PlanFlags::default())
        .unwrap()
        .into_iter()
        .map(|(s, plan)| {
            let mut plan = plan.unwrap();
            plan.launch.cwd.clone_from(&real_cwd);
            (s, Ok(plan))
        })
        .collect();
    let items = start_batch(&store, &env, planned);

    let refused: Vec<_> = items
        .iter()
        .filter_map(|item| match item {
            Item::Refused(refused) => Some(refused.message.as_str()),
            Item::Started(_) => None,
        })
        .collect();
    assert!(refused.is_empty(), "{refused:?}");
}
