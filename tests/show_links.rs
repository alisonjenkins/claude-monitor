#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! `show` and `show --json` list the session's links (T103, FR-026).

use claude_monitor::cli::{run_in, Command};
use claude_monitor::env::Env;
use claude_monitor::model::Session;
use claude_monitor::store::{legacy, Store, StoreOptions};
use serde_json::{json, Value};
use tempfile::TempDir;

const T0: &str = "2026-10-12T10:00:00Z";

struct Rig {
    state: TempDir,
    env: Env,
}

fn rig() -> Rig {
    let state = TempDir::new().unwrap();
    let env = Env::real();
    let store = Store::open_in_dir(state.path(), &StoreOptions::viewer()).unwrap();
    let empty = TempDir::new().unwrap();
    legacy::migrate(&store, empty.path(), env.clock.as_ref()).unwrap();
    for id in ["s1", "s2"] {
        let session = Session::new(id, "claude", &format!("agent-{id}"), "/work", T0);
        store.upsert_session(&session).unwrap();
    }
    Rig { state, env }
}

impl Rig {
    fn run(&self, command: Command) -> claude_monitor::cli::Output {
        run_in(command, &self.env, Some(self.state.path())).unwrap()
    }

    fn link(&self, owner: &str, node: &str, kind: &str, doc: Option<&str>) {
        self.run(Command::Link {
            owner: owner.to_owned(),
            cortex: node.to_owned(),
            kind: kind.to_owned(),
            doc: doc.map(str::to_owned),
        });
    }

    fn show(&self, id: &str) -> claude_monitor::cli::Output {
        self.run(Command::Show {
            session: id.to_owned(),
        })
    }

    fn show_json(&self, id: &str) -> Value {
        serde_json::from_str(&self.show(id).render(true)).unwrap()
    }
}

#[test]
#[ignore = "T103: implemented in the next commit"]
fn json_lists_the_sessions_links_in_the_link_command_shape() {
    let rig = rig();
    rig.link("s1", "42", "task", Some("7"));
    rig.link("s1", "43", "memory", Some("7"));
    rig.link("s2", "99", "task", Some("7"));

    let shown = rig.show_json("s1");

    let links = shown["links"].as_array().unwrap();
    assert_eq!(links.len(), 2, "{links:?}");
    let first = &links[0];
    assert_eq!(first["owner"], json!({ "kind": "session", "id": "s1" }));
    assert_eq!(first["node_id"], "42");
    assert_eq!(first["doc_id"], "7");
    assert_eq!(first["kind"], "task");
    assert_eq!(first["last_agreed_at"], Value::Null);
    assert_eq!(first["last_remote_status"], Value::Null);
    assert_eq!(links[1]["node_id"], "43");
}

#[test]
#[ignore = "T103: implemented in the next commit"]
fn json_has_an_empty_links_list_when_nothing_is_linked() {
    let rig = rig();
    rig.link("s2", "99", "task", Some("7"));

    let shown = rig.show_json("s1");

    assert_eq!(shown["links"], json!([]));
}

#[test]
#[ignore = "T103: implemented in the next commit"]
fn text_has_one_link_line_per_link_and_none_for_other_sessions() {
    let rig = rig();
    rig.link("s1", "42", "task", Some("7"));
    rig.link("s2", "99", "task", Some("7"));

    let text = rig.show("s1").render(false);

    let lines: Vec<&str> = text.lines().filter(|l| l.starts_with("link")).collect();
    assert_eq!(lines.len(), 1, "{text}");
    assert!(lines[0].contains("task 42"), "{}", lines[0]);
    assert!(lines[0].contains("doc 7"), "{}", lines[0]);
    assert!(!text.contains("99"), "{text}");
}

#[test]
#[ignore = "T103: implemented in the next commit"]
fn text_has_no_link_line_without_links() {
    let rig = rig();

    let text = rig.show("s1").render(false);

    assert!(!text.lines().any(|l| l.starts_with("link")), "{text}");
}
