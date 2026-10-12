//! The JSON Schema of the export, generated from one description of the wire
//! format (`contracts/cortexiphan-export.md`). The same tables drive
//! [`validate_record`], so the schema and the check cannot drift apart.

use chrono::DateTime;
use serde_json::{json, Map, Value};

/// Bumps only for breaking changes; fields are added, never repurposed.
pub const SCHEMA_VERSION: u32 = 1;
/// The `external.system` of every record.
pub const SYSTEM: &str = "ai-control-centre";
/// The connector name every system-authored change is attributed to.
pub const CONNECTOR_NAME: &str = SYSTEM;

pub const KINDS: &[&str] = &["project", "task", "memory", "session", "edge"];
pub const AUTHOR_TYPES: &[&str] = &["user", "llm", "connector"];
pub const PROJECT_STATUSES: &[&str] = &["active", "paused", "done", "archived"];
pub const TASK_STATUSES: &[&str] = &["todo", "doing", "done", "blocked"];
pub const SESSION_STATES: &[&str] = &[
    "working",
    "needs_permission",
    "idle",
    "finished",
    "interrupted",
    "unknown",
];
pub const EDGE_TYPES: &[&str] = &["part_of", "resumed_from"];

const TIME_PATTERN: &str = r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$";
const TIME_LEN: usize = "2026-10-12T14:03:22Z".len();

#[derive(Clone, Copy)]
enum Ty {
    Str,
    NullableStr,
    Bool,
    Time,
    NullableTime,
    Enum(&'static [&'static str]),
    ConstStr(&'static str),
    ConstInt(u32),
    /// The record's `node`: an object whose shape depends on `kind`.
    Node,
    Object(&'static str),
    Array(&'static str),
}

struct Field {
    name: &'static str,
    ty: Ty,
    required: bool,
}

const fn req(name: &'static str, ty: Ty) -> Field {
    Field {
        name,
        ty,
        required: true,
    }
}

const fn opt(name: &'static str, ty: Ty) -> Field {
    Field {
        name,
        ty,
        required: false,
    }
}

const RECORD: &[Field] = &[
    req("schema_version", Ty::ConstInt(SCHEMA_VERSION)),
    req("kind", Ty::Enum(KINDS)),
    req("external", Ty::Object("external")),
    req("author", Ty::Object("author")),
    req("updated_at", Ty::Time),
    req("node", Ty::Node),
];

const EXTERNAL: &[Field] = &[
    req("system", Ty::ConstStr(SYSTEM)),
    req("id", Ty::Str),
    req("url", Ty::NullableStr),
];

const AUTHOR: &[Field] = &[req("type", Ty::Enum(AUTHOR_TYPES)), req("name", Ty::Str)];

const PROJECT_NODE: &[Field] = &[
    req("title", Ty::Str),
    req("status", Ty::Enum(PROJECT_STATUSES)),
    req("repos", Ty::Array("repo")),
];

const REPO: &[Field] = &[
    req("name", Ty::Str),
    req("uri", Ty::NullableStr),
    req("clones", Ty::Array("clone")),
];

const CLONE: &[Field] = &[req("machine", Ty::Str), req("path", Ty::Str)];

const TASK_NODE: &[Field] = &[
    req("title", Ty::Str),
    req("status", Ty::Enum(TASK_STATUSES)),
    req("part_of", Ty::Str),
    req("source", Ty::NullableStr),
];

const MEMORY_NODE: &[Field] = &[
    req("body", Ty::Str),
    req("about", Ty::Str),
    opt("model_written", Ty::Bool),
];

const SESSION_NODE: &[Field] = &[
    req("agent_kind", Ty::Str),
    req("agent_session_id", Ty::Str),
    req("cwd", Ty::Str),
    req("branch", Ty::NullableStr),
    req("state", Ty::Enum(SESSION_STATES)),
    req("last_activity_at", Ty::Time),
    req("done_at", Ty::NullableTime),
    req("resumed_from", Ty::NullableStr),
    req("project", Ty::NullableStr),
    opt("task_description", Ty::Str),
    opt("left_off", Ty::Str),
];

const EDGE_NODE: &[Field] = &[
    req("from", Ty::Str),
    req("to", Ty::Str),
    req("type", Ty::Enum(EDGE_TYPES)),
];

const DEFS: &[(&str, &[Field])] = &[
    ("record", RECORD),
    ("external", EXTERNAL),
    ("author", AUTHOR),
    ("project_node", PROJECT_NODE),
    ("repo", REPO),
    ("clone", CLONE),
    ("task_node", TASK_NODE),
    ("memory_node", MEMORY_NODE),
    ("session_node", SESSION_NODE),
    ("edge_node", EDGE_NODE),
];

fn def(name: &str) -> Option<&'static [Field]> {
    DEFS.iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, fields)| *fields)
}

fn node_def(kind: &str) -> String {
    format!("{kind}_node")
}

fn def_ref(name: &str) -> Value {
    json!({ "$ref": format!("#/$defs/{name}") })
}

fn time_schema() -> Value {
    json!({ "type": "string", "format": "date-time", "pattern": TIME_PATTERN })
}

fn field_schema(ty: Ty) -> Value {
    match ty {
        Ty::Str => json!({ "type": "string" }),
        Ty::NullableStr => json!({ "type": ["string", "null"] }),
        Ty::Bool => json!({ "type": "boolean" }),
        Ty::Time => time_schema(),
        Ty::NullableTime => json!({ "oneOf": [time_schema(), { "type": "null" }] }),
        Ty::Enum(values) => json!({ "type": "string", "enum": values }),
        Ty::ConstStr(value) => json!({ "const": value }),
        Ty::ConstInt(value) => json!({ "const": value }),
        Ty::Node => json!({ "type": "object" }),
        Ty::Object(name) => def_ref(name),
        Ty::Array(name) => json!({ "type": "array", "items": def_ref(name) }),
    }
}

fn object_schema(fields: &[Field]) -> Value {
    let properties: Map<String, Value> = fields
        .iter()
        .map(|field| (field.name.to_owned(), field_schema(field.ty)))
        .collect();
    let required: Vec<&str> = fields
        .iter()
        .filter(|field| field.required)
        .map(|field| field.name)
        .collect();
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

/// The JSON Schema (draft 2020-12) of one export record.
pub fn generate() -> Value {
    let defs: Map<String, Value> = DEFS
        .iter()
        .map(|(name, fields)| ((*name).to_owned(), object_schema(fields)))
        .collect();
    let by_kind: Vec<Value> = KINDS
        .iter()
        .map(|kind| {
            json!({
                "if": { "properties": { "kind": { "const": kind } }, "required": ["kind"] },
                "then": { "properties": { "node": def_ref(&node_def(kind)) } },
            })
        })
        .collect();
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "ai-control-centre cortexiphan export record",
        "description": "One line of `claude-monitor export cortexiphan` (JSON Lines).",
        "$ref": "#/$defs/record",
        "allOf": by_kind,
        "$defs": defs,
    })
}

fn is_utc_time(text: &str) -> bool {
    text.len() == TIME_LEN && text.ends_with('Z') && DateTime::parse_from_rfc3339(text).is_ok()
}

fn check_field(path: &str, ty: Ty, value: &Value) -> Result<(), String> {
    let (matches, expected) = match ty {
        Ty::Object(name) => return check_object(path, name, value),
        Ty::Array(name) => {
            let items = value
                .as_array()
                .ok_or_else(|| format!("{path}: expected an array, got {value}"))?;
            return items.iter().enumerate().try_for_each(|(index, item)| {
                check_object(&format!("{path}[{index}]"), name, item)
            });
        }
        Ty::Str => (value.is_string(), "a string".to_owned()),
        Ty::NullableStr => (
            value.is_string() || value.is_null(),
            "a string or null".to_owned(),
        ),
        Ty::Bool => (value.is_boolean(), "a boolean".to_owned()),
        Ty::Time => (
            value.as_str().is_some_and(is_utc_time),
            "a UTC timestamp like 2026-10-12T14:03:22Z".to_owned(),
        ),
        Ty::NullableTime => (
            value.is_null() || value.as_str().is_some_and(is_utc_time),
            "a UTC timestamp or null".to_owned(),
        ),
        Ty::Enum(values) => (
            value.as_str().is_some_and(|text| values.contains(&text)),
            format!("one of {values:?}"),
        ),
        Ty::ConstStr(expected) => (value.as_str() == Some(expected), format!("{expected:?}")),
        Ty::ConstInt(expected) => (
            value.as_u64() == Some(u64::from(expected)),
            expected.to_string(),
        ),
        Ty::Node => (value.is_object(), "an object".to_owned()),
    };
    if matches {
        Ok(())
    } else {
        Err(format!("{path}: expected {expected}, got {value}"))
    }
}

fn check_object(path: &str, def_name: &str, value: &Value) -> Result<(), String> {
    let fields = def(def_name).ok_or_else(|| format!("{path}: no definition {def_name}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| format!("{path}: expected an object, got {value}"))?;
    for field in fields {
        match object.get(field.name) {
            Some(found) => check_field(&format!("{path}.{}", field.name), field.ty, found)?,
            None if field.required => {
                return Err(format!("{path}: missing required field {}", field.name))
            }
            None => {}
        }
    }
    if let Some(unknown) = object
        .keys()
        .find(|key| !fields.iter().any(|field| field.name == key.as_str()))
    {
        return Err(format!("{path}: unknown field {unknown}"));
    }
    Ok(())
}

/// Checks one parsed record against the wire format: required fields, no
/// extra fields, enum values, UTC timestamps, and a `node` that matches `kind`.
pub fn validate_record(record: &Value) -> Result<(), String> {
    check_object("record", "record", record)?;
    let kind = record
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| "record.kind: missing".to_owned())?;
    let node = record
        .get("node")
        .ok_or_else(|| "record.node: missing".to_owned())?;
    check_object(&format!("record.node ({kind})"), &node_def(kind), node)
}
