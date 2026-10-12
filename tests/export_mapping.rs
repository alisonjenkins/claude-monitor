//! Cortexiphan export mapping (FR-023, FR-024, FR-025): statuses, authors, the
//! `external` idempotency key, and no conversation text unless asked for.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod export_support;

use chrono::DateTime;
use claude_monitor::export::cortexiphan::export_jsonl;
use claude_monitor::export::schema::validate_record;
use export_support::{agent, by_id, of_kind, options, parse, project20, system, user, Fixture, T0};
use serde_json::{json, Value};

fn with_text() -> claude_monitor::export::cortexiphan::ExportOptions {
    let mut opts = options();
    opts.include_text = true;
    opts
}

fn text_fixture() -> Fixture {
    let fix = Fixture::new();
    fix.project("p1", "alpha", "active", None);
    fix.session("s1", Some("p1"), |s| {
        s.task_description = Some("SECRET-PROMPT".to_owned());
        s.left_off = Some("SECRET-REPLY".to_owned());
    });
    fix.summary("sum-1", "session", "s1", "SECRET-SUMMARY", "qwen-local");
    fix.note("n1", "session", "s1", "plain user note", "alison");
    fix
}

#[test]
fn project_statuses_map_to_cortexiphan_vocabulary() {
    let fix = Fixture::new();
    for (id, status) in [
        ("p-active", "active"),
        ("p-parked", "parked"),
        ("p-done", "done"),
        ("p-archived", "archived"),
    ] {
        fix.project(id, id, status, None);
    }
    let records = fix.records(&options());
    for (id, expected) in [
        ("p-active", "active"),
        ("p-parked", "paused"),
        ("p-done", "done"),
        ("p-archived", "archived"),
    ] {
        assert_eq!(
            by_id(&records, "project", id)["node"]["status"],
            expected,
            "{id}"
        );
    }
}

#[test]
fn step_statuses_map_one_to_one_and_each_step_is_part_of_the_project() {
    let fix = Fixture::new();
    fix.project("p1", "alpha", "active", Some("b1"));
    fix.binding("b1", "p1", "specs/x/tasks.md");
    for (ordinal, status) in ["todo", "doing", "done", "blocked"].iter().enumerate() {
        fix.step(
            "b1",
            ordinal as u32,
            Some(&format!("T00{ordinal}")),
            "work",
            status,
        );
    }
    let records = fix.records(&options());
    let tasks = of_kind(&records, "task");
    assert_eq!(tasks.len(), 4);
    let mut statuses: Vec<_> = tasks
        .iter()
        .map(|t| t["node"]["status"].as_str().unwrap().to_owned())
        .collect();
    statuses.sort();
    assert_eq!(statuses, ["blocked", "doing", "done", "todo"]);
    for task in &tasks {
        assert_eq!(task["node"]["part_of"], "p1");
        assert_eq!(task["node"]["source"], "specs/x/tasks.md");
    }
    let edges = of_kind(&records, "edge");
    let part_of = edges
        .iter()
        .filter(|e| e["node"]["type"] == "part_of")
        .count();
    assert_eq!(part_of, 4);
}

#[test]
fn step_title_carries_the_plan_id_once() {
    let fix = Fixture::new();
    fix.project("p1", "alpha", "active", Some("b1"));
    fix.binding("b1", "p1", "tasks.md");
    fix.step("b1", 0, Some("T005"), "Add liveness probe", "todo");
    fix.step("b1", 1, Some("T006"), "T006 Already prefixed", "todo");
    fix.step("b1", 2, None, "No id", "todo");
    let records = fix.records(&options());
    let mut titles: Vec<_> = of_kind(&records, "task")
        .iter()
        .map(|t| t["node"]["title"].as_str().unwrap().to_owned())
        .collect();
    titles.sort();
    assert_eq!(
        titles,
        ["No id", "T005 Add liveness probe", "T006 Already prefixed"]
    );
}

#[test]
fn event_authors_map_to_cortexiphan_author_types() {
    let fix = Fixture::new();
    fix.project("p-user", "u", "active", None);
    fix.project("p-llm", "l", "active", None);
    fix.project("p-llm-bare", "lb", "active", None);
    fix.project("p-system", "s", "active", None);
    fix.event("2026-10-12T10:00:01Z", "project", "p-user", &user("alison"));
    fix.event(
        "2026-10-12T10:00:02Z",
        "project",
        "p-llm",
        &agent("claude", Some("claude-opus-5-5")),
    );
    fix.event(
        "2026-10-12T10:00:03Z",
        "project",
        "p-llm-bare",
        &agent("copilot", None),
    );
    fix.event(
        "2026-10-12T10:00:04Z",
        "project",
        "p-system",
        &system("reconcile"),
    );
    let records = fix.records(&options());
    let author = |id: &str| by_id(&records, "project", id)["author"].clone();
    assert_eq!(author("p-user"), json!({"type": "user", "name": "alison"}));
    assert_eq!(
        author("p-llm"),
        json!({"type": "llm", "name": "claude-opus-5-5"})
    );
    assert_eq!(
        author("p-llm-bare"),
        json!({"type": "llm", "name": "copilot"})
    );
    assert_eq!(
        author("p-system"),
        json!({"type": "connector", "name": "ai-control-centre"})
    );
}

#[test]
fn the_last_event_decides_author_and_updated_at() {
    let fix = Fixture::new();
    fix.project("p1", "alpha", "active", None);
    fix.event(
        "2026-10-12T10:00:01Z",
        "project",
        "p1",
        &system("reconcile"),
    );
    fix.event("2026-10-12T11:00:00Z", "project", "p1", &user("alison"));
    let records = fix.records(&options());
    let project = by_id(&records, "project", "p1");
    assert_eq!(project["author"], json!({"type": "user", "name": "alison"}));
    assert_eq!(project["updated_at"], "2026-10-12T11:00:00Z");
}

#[test]
fn an_entity_without_events_is_attributed_to_the_connector() {
    let fix = Fixture::new();
    fix.project("p1", "alpha", "active", None);
    let records = fix.records(&options());
    let project = by_id(&records, "project", "p1");
    assert_eq!(
        project["author"],
        json!({"type": "connector", "name": "ai-control-centre"})
    );
    assert_eq!(project["updated_at"], T0);
}

#[test]
fn every_record_has_the_external_key_and_an_author() {
    let records = project20().records(&options());
    assert!(!records.is_empty());
    for record in &records {
        assert_eq!(record["schema_version"], 1);
        assert_eq!(
            record["external"]["system"], "ai-control-centre",
            "{record}"
        );
        assert!(record["external"]["id"]
            .as_str()
            .is_some_and(|id| !id.is_empty()));
        assert_eq!(record["external"]["url"], Value::Null);
        assert!(record["author"]["type"].is_string(), "{record}");
        assert!(record["author"]["name"].is_string(), "{record}");
        validate_record(record).unwrap();
    }
}

#[test]
fn entity_ids_are_the_external_ids() {
    let records = project20().records(&options());
    by_id(&records, "project", "proj-20");
    for id in ["sess-a", "sess-b", "sess-c", "sess-d"] {
        by_id(&records, "session", id);
    }
    by_id(&records, "memory", "note-1");
}

#[test]
fn keys_do_not_depend_on_status_time_or_other_entities() {
    let fix = project20();
    let ids = |records: &[Value]| -> Vec<(String, String)> {
        records
            .iter()
            .map(|r| {
                (
                    r["kind"].as_str().unwrap().to_owned(),
                    r["external"]["id"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    };
    let before = ids(&fix.records(&options()));
    fix.set_step_status("bind-20", 3, "done");
    fix.event(
        "2026-10-13T09:00:00Z",
        "project",
        "proj-20",
        &user("alison"),
    );
    let after = ids(&fix.records(&options()));
    assert_eq!(before, after);
}

#[test]
fn repeated_exports_are_byte_identical() {
    let fix = project20();
    assert_eq!(fix.export(&options()), fix.export(&options()));
    assert_eq!(fix.export(&with_text()), fix.export(&with_text()));
}

#[test]
fn records_are_sorted_by_kind_then_external_id() {
    let records = project20().records(&options());
    let keys: Vec<_> = records
        .iter()
        .map(|r| {
            (
                r["kind"].as_str().unwrap().to_owned(),
                r["external"]["id"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
}

#[test]
fn no_conversation_text_by_default() {
    let fix = text_fixture();
    let out = fix.export(&options());
    for secret in ["SECRET-PROMPT", "SECRET-REPLY", "SECRET-SUMMARY"] {
        assert!(!out.contains(secret), "{secret} leaked:\n{out}");
    }
    let records = parse(&out);
    let node = &by_id(&records, "session", "s1")["node"];
    assert!(node.get("task_description").is_none());
    assert!(node.get("left_off").is_none());
    assert!(of_kind(&records, "memory")
        .iter()
        .all(|m| m["node"].get("model_written").is_none()));
}

#[test]
fn a_corrupt_session_row_does_not_stop_the_export() {
    let fix = Fixture::new();
    fix.project("p1", "alpha", "active", None);
    fix.session("s1", Some("p1"), |_| {});
    fix.session("bad", Some("p1"), |_| {});
    let conn = fix.store.connection();
    conn.execute_batch("PRAGMA ignore_check_constraints = ON")
        .unwrap();
    conn.execute("UPDATE sessions SET state = 'bogus' WHERE id = 'bad'", [])
        .unwrap();
    conn.execute_batch("PRAGMA ignore_check_constraints = OFF")
        .unwrap();

    let records = fix.records(&options());

    let ids: Vec<_> = of_kind(&records, "session")
        .iter()
        .map(|r| r["external"]["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids, ["s1"]);
}

#[test]
fn include_text_adds_the_session_text_and_summaries() {
    let fix = text_fixture();
    let records = fix.records(&with_text());
    let node = &by_id(&records, "session", "s1")["node"];
    assert_eq!(node["task_description"], "SECRET-PROMPT");
    assert_eq!(node["left_off"], "SECRET-REPLY");
    let summary = by_id(&records, "memory", "sum-1");
    assert_eq!(summary["node"]["body"], "SECRET-SUMMARY");
    assert_eq!(summary["node"]["model_written"], true);
    assert_eq!(summary["node"]["about"], "s1");
    assert_eq!(
        summary["author"],
        json!({"type": "llm", "name": "qwen-local"})
    );
    for record in &records {
        validate_record(record).unwrap();
    }
}

#[test]
fn user_notes_export_as_memory_without_the_flag() {
    let fix = text_fixture();
    let records = fix.records(&options());
    let note = by_id(&records, "memory", "n1");
    assert_eq!(note["node"]["body"], "plain user note");
    assert_eq!(note["node"]["about"], "s1");
    assert_eq!(note["author"], json!({"type": "user", "name": "alison"}));
}

#[test]
fn timestamps_are_iso8601_utc() {
    let fix = project20();
    for record in fix.records(&options()) {
        let updated = record["updated_at"].as_str().unwrap();
        assert!(updated.ends_with('Z'), "{updated}");
        DateTime::parse_from_rfc3339(updated).unwrap();
        if record["kind"] == "session" {
            for field in ["last_activity_at", "done_at"] {
                if let Some(at) = record["node"][field].as_str() {
                    assert!(at.ends_with('Z'), "{at}");
                    DateTime::parse_from_rfc3339(at).unwrap();
                }
            }
        }
    }
}

#[test]
fn sessions_and_lineage_are_exported_with_edges() {
    let records = project20().records(&options());
    let session = &by_id(&records, "session", "sess-b")["node"];
    assert_eq!(session["agent_kind"], "claude");
    assert_eq!(session["agent_session_id"], "agent-sess-b");
    assert_eq!(session["state"], "working");
    assert_eq!(session["resumed_from"], "sess-a");
    assert_eq!(session["project"], "proj-20");
    assert_eq!(
        by_id(&records, "session", "sess-d")["node"]["done_at"],
        "2026-10-12T13:00:00Z"
    );
    assert_eq!(
        by_id(&records, "session", "sess-a")["node"]["resumed_from"],
        Value::Null
    );
    let resumed: Vec<_> = of_kind(&records, "edge")
        .into_iter()
        .filter(|e| e["node"]["type"] == "resumed_from")
        .collect();
    assert_eq!(resumed.len(), 1);
    assert_eq!(resumed[0]["node"]["from"], "sess-b");
    assert_eq!(resumed[0]["node"]["to"], "sess-a");
}

#[test]
fn project_repos_come_from_member_sessions_with_this_machine_as_the_clone_host() {
    let records = project20().records(&options());
    let repos = by_id(&records, "project", "proj-20")["node"]["repos"].clone();
    assert_eq!(
        repos,
        json!([
            {
                "name": "claude-monitor",
                "uri": null,
                "clones": [{"machine": "workstation", "path": "/home/ali/git/personal/claude-monitor"}]
            },
            {
                "name": "nix-config",
                "uri": null,
                "clones": [{"machine": "workstation", "path": "/home/ali/git/personal/nix-config"}]
            }
        ])
    );
}

#[test]
fn project_filter_selects_by_name_or_id_and_keeps_only_its_records() {
    let fix = project20();
    fix.project("other", "other-project", "active", None);
    fix.session("sess-x", Some("other"), |_| {});
    fix.session("sess-free", None, |_| {});
    let mut opts = options();
    opts.project = Some("ai-control-centre".to_owned());
    let by_name = fix.records(&opts);
    opts.project = Some("proj-20".to_owned());
    let by_id_ = fix.records(&opts);
    assert_eq!(by_name, by_id_);
    assert_eq!(of_kind(&by_name, "project").len(), 1);
    assert!(by_name.iter().all(|r| r["external"]["id"] != "sess-x"));
    assert!(by_name.iter().all(|r| r["external"]["id"] != "sess-free"));

    let all = fix.records(&options());
    assert_eq!(of_kind(&all, "project").len(), 2);
    by_id(&all, "session", "sess-free");
}

#[test]
fn an_unknown_project_is_an_error() {
    let fix = project20();
    let mut opts = options();
    opts.project = Some("nope".to_owned());
    let err = export_jsonl(&fix.store, &opts).unwrap_err();
    assert!(err.to_string().contains("nope"), "{err}");
}

#[test]
fn an_empty_store_exports_nothing() {
    assert_eq!(Fixture::new().export(&options()), "");
}
