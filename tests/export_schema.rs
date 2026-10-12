//! The export's JSON Schema: generated from the Rust description of the wire
//! format, checked in at `docs/cortexiphan-export.schema.json`, and used to
//! validate records (T099). A diverging checked-in copy fails here.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::PathBuf;

use claude_monitor::export::schema::{generate, validate_record, SCHEMA_VERSION, SYSTEM};
use serde_json::{json, Value};

fn checked_in() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/cortexiphan-export.schema.json")
}

fn rendered() -> String {
    format!("{}\n", serde_json::to_string_pretty(&generate()).unwrap())
}

fn project() -> Value {
    json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "project",
        "external": {"system": SYSTEM, "id": "p1", "url": null},
        "author": {"type": "user", "name": "alison"},
        "updated_at": "2026-10-12T14:03:22Z",
        "node": {
            "title": "alpha",
            "status": "active",
            "repos": [{"name": "r", "uri": null, "clones": [{"machine": "m", "path": "/r"}]}]
        }
    })
}

fn session() -> Value {
    json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "session",
        "external": {"system": SYSTEM, "id": "s1", "url": null},
        "author": {"type": "llm", "name": "claude-opus-5-5"},
        "updated_at": "2026-10-12T14:03:22Z",
        "node": {
            "agent_kind": "claude", "agent_session_id": "a", "cwd": "/w", "branch": null,
            "state": "idle", "last_activity_at": "2026-10-12T14:03:22Z", "done_at": null,
            "resumed_from": null, "project": null
        }
    })
}

fn edit(mut record: Value, path: &[&str], value: Value) -> Value {
    let mut cursor = &mut record;
    for key in &path[..path.len() - 1] {
        cursor = &mut cursor[*key];
    }
    cursor[path[path.len() - 1]] = value;
    record
}

#[test]
#[ignore = "implemented by T099"]
fn the_checked_in_schema_matches_the_generated_one() {
    let on_disk = std::fs::read_to_string(checked_in()).unwrap();
    assert_eq!(
        on_disk,
        rendered(),
        "{} is out of date; regenerate it from `claude_monitor::export::schema::generate`",
        checked_in().display()
    );
}

#[test]
#[ignore = "implemented by T099"]
fn the_schema_is_draft_2020_12_and_names_every_record_kind() {
    let schema = generate();
    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    let kinds = schema["$defs"]["record"]["properties"]["kind"]["enum"]
        .as_array()
        .unwrap();
    for kind in ["project", "task", "memory", "session", "edge"] {
        assert!(kinds.contains(&json!(kind)), "{kind}");
        assert!(
            schema["$defs"][format!("{kind}_node")].is_object(),
            "{kind}"
        );
    }
    assert_eq!(
        schema["$defs"]["record"]["properties"]["schema_version"]["const"],
        1
    );
}

#[test]
#[ignore = "implemented by T099"]
fn every_required_field_is_a_declared_property_and_no_extras_are_allowed() {
    let schema = generate();
    for (name, def) in schema["$defs"].as_object().unwrap() {
        let properties = def["properties"].as_object().unwrap();
        for required in def["required"].as_array().unwrap() {
            assert!(
                properties.contains_key(required.as_str().unwrap()),
                "{name}"
            );
        }
        assert_eq!(def["additionalProperties"], false, "{name}");
    }
}

#[test]
#[ignore = "implemented by T099"]
fn valid_records_pass() {
    validate_record(&project()).unwrap();
    validate_record(&session()).unwrap();
    validate_record(&edit(
        session(),
        &["node", "task_description"],
        json!("text"),
    ))
    .unwrap();
}

#[test]
#[ignore = "implemented by T099"]
fn records_that_break_the_wire_format_fail() {
    let cases = [
        (
            "wrong version",
            edit(project(), &["schema_version"], json!(2)),
        ),
        ("unknown kind", edit(project(), &["kind"], json!("note"))),
        (
            "foreign system",
            edit(project(), &["external", "system"], json!("other")),
        ),
        (
            "bad author type",
            edit(project(), &["author", "type"], json!("robot")),
        ),
        (
            "bad status",
            edit(project(), &["node", "status"], json!("parked")),
        ),
        (
            "non-UTC time",
            edit(
                project(),
                &["updated_at"],
                json!("2026-10-12T14:03:22+01:00"),
            ),
        ),
        (
            "fractional time",
            edit(project(), &["updated_at"], json!("2026-10-12T14:03:22.5Z")),
        ),
        (
            "extra node key",
            edit(project(), &["node", "extra"], json!(1)),
        ),
        ("extra top key", edit(project(), &["extra"], json!(1))),
        (
            "wrong node for kind",
            edit(project(), &["kind"], json!("session")),
        ),
        (
            "bad state",
            edit(session(), &["node", "state"], json!("sleeping")),
        ),
        (
            "title not a string",
            edit(project(), &["node", "title"], json!(3)),
        ),
    ];
    for (name, record) in cases {
        assert!(
            validate_record(&record).is_err(),
            "{name} should fail: {record}"
        );
    }
}

#[test]
#[ignore = "implemented by T099"]
fn a_missing_required_field_fails_and_the_error_names_it() {
    let mut record = project();
    record.as_object_mut().unwrap().remove("author");
    let error = validate_record(&record).unwrap_err();
    assert!(error.contains("author"), "{error}");
}
