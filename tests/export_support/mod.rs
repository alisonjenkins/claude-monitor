//! A real store filled by direct inserts, for the export tests. Projects, steps,
//! notes and summaries have no writers in the library yet, so rows go in as SQL.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use claude_monitor::export::cortexiphan::{export_jsonl, ExportOptions};
use claude_monitor::model::{Author, Session, State};
use claude_monitor::store::events::NewEvent;
use claude_monitor::store::{Store, StoreOptions};
use rusqlite::params;
use serde_json::{json, Value};
use tempfile::TempDir;

pub const T0: &str = "2026-10-12T10:00:00Z";
pub const MACHINE: &str = "workstation";

pub struct Fixture {
    pub dir: TempDir,
    pub store: Store,
}

pub fn options() -> ExportOptions {
    ExportOptions {
        project: None,
        include_text: false,
        machine: MACHINE.to_owned(),
    }
}

pub fn user(name: &str) -> Author {
    Author::User {
        name: name.to_owned(),
    }
}

pub fn agent(kind: &str, model: Option<&str>) -> Author {
    Author::Agent {
        kind: kind.to_owned(),
        model: model.map(str::to_owned),
    }
}

pub fn system(component: &str) -> Author {
    Author::System {
        component: component.to_owned(),
    }
}

impl Fixture {
    pub fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let store = Store::open_in_dir(dir.path(), &StoreOptions::default()).unwrap();
        // A project and its binding reference each other, so rows go in any order.
        store
            .connection()
            .pragma_update(None, "foreign_keys", false)
            .unwrap();
        Self { dir, store }
    }

    pub fn project(&self, id: &str, name: &str, status: &str, binding: Option<&str>) {
        self.store
            .connection()
            .execute(
                "INSERT INTO projects (id, name, status, note, progress_binding_id, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?5)",
                params![id, name, status, binding, T0],
            )
            .unwrap();
    }

    pub fn project_note(&self, id: &str, note: &str) {
        self.store
            .connection()
            .execute(
                "UPDATE projects SET note = ?2 WHERE id = ?1",
                params![id, note],
            )
            .unwrap();
    }

    pub fn binding(&self, id: &str, project_id: &str, source_path: &str) {
        self.store
            .connection()
            .execute(
                "INSERT INTO progress_binding (id, owner_kind, owner_id, source_kind, source_path, chosen_by, read_at) \
                 VALUES (?1, 'project', ?2, 'speckit_tasks', ?3, 'auto', ?4)",
                params![id, project_id, source_path, T0],
            )
            .unwrap();
    }

    pub fn step(
        &self,
        binding: &str,
        ordinal: u32,
        external_id: Option<&str>,
        text: &str,
        status: &str,
    ) {
        self.store
            .connection()
            .execute(
                "INSERT INTO steps (binding_id, ordinal, text, status, group_name, external_id) \
                 VALUES (?1, ?2, ?3, ?4, 'Phase 1', ?5)",
                params![binding, ordinal, text, status, external_id],
            )
            .unwrap();
    }

    pub fn set_step_status(&self, binding: &str, ordinal: u32, status: &str) {
        self.store
            .connection()
            .execute(
                "UPDATE steps SET status = ?3 WHERE binding_id = ?1 AND ordinal = ?2",
                params![binding, ordinal, status],
            )
            .unwrap();
    }

    pub fn session(&self, id: &str, project: Option<&str>, mutate: impl FnOnce(&mut Session)) {
        let mut session = Session::new(id, "claude", &format!("agent-{id}"), "/work", T0);
        session.project_id = project.map(str::to_owned);
        mutate(&mut session);
        self.store.upsert_session(&session).unwrap();
    }

    pub fn note(&self, id: &str, owner_kind: &str, owner_id: &str, body: &str, author: &str) {
        self.store
            .connection()
            .execute(
                "INSERT INTO notes (id, owner_kind, owner_id, body, created_at, author) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, owner_kind, owner_id, body, T0, author],
            )
            .unwrap();
    }

    pub fn summary(&self, id: &str, owner_kind: &str, owner_id: &str, body: &str, model: &str) {
        self.store
            .connection()
            .execute(
                "INSERT INTO summaries (id, owner_kind, owner_id, body, model, model_written, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
                params![id, owner_kind, owner_id, body, model, T0],
            )
            .unwrap();
    }

    pub fn event(&self, at: &str, entity_kind: &str, entity_id: &str, author: &Author) {
        self.store
            .append_event(&NewEvent {
                at,
                entity_kind,
                entity_id,
                kind: "changed",
                payload: &json!({}),
                author,
            })
            .unwrap();
    }

    pub fn export(&self, options: &ExportOptions) -> String {
        export_jsonl(&self.store, options).unwrap()
    }

    pub fn records(&self, options: &ExportOptions) -> Vec<Value> {
        parse(&self.export(options))
    }
}

pub fn parse(jsonl: &str) -> Vec<Value> {
    jsonl
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

pub fn of_kind<'a>(records: &'a [Value], kind: &str) -> Vec<&'a Value> {
    records.iter().filter(|r| r["kind"] == kind).collect()
}

pub fn by_id<'a>(records: &'a [Value], kind: &str, id: &str) -> &'a Value {
    records
        .iter()
        .find(|r| r["kind"] == kind && r["external"]["id"] == id)
        .unwrap_or_else(|| panic!("no {kind} record {id}"))
}

/// A project with a 20-step spec-kit plan and four sessions: a `/clear`
/// lineage pair, a Copilot session, and one that finished and was marked done.
pub fn project20() -> Fixture {
    const STATUSES: [&str; 4] = ["done", "doing", "todo", "blocked"];
    let fix = Fixture::new();
    fix.project("proj-20", "ai-control-centre", "active", Some("bind-20"));
    fix.binding("bind-20", "proj-20", "specs/008-ai-control-centre/tasks.md");
    for ordinal in 0..20u32 {
        let status = STATUSES[(ordinal % 4) as usize];
        fix.step(
            "bind-20",
            ordinal,
            Some(&format!("T{:03}", ordinal + 1)),
            &format!("Step number {}", ordinal + 1),
            status,
        );
    }
    fix.session("sess-a", Some("proj-20"), |s| {
        s.repo_root = Some("/home/ali/git/personal/claude-monitor".to_owned());
        s.cwd = "/home/ali/git/personal/claude-monitor".to_owned();
        s.branch = Some("008-ai-control-centre".to_owned());
        s.state = State::Finished;
        s.last_activity_at = "2026-10-12T11:00:00Z".to_owned();
    });
    fix.session("sess-b", Some("proj-20"), |s| {
        s.resumed_from = Some("sess-a".to_owned());
        s.repo_root = Some("/home/ali/git/personal/claude-monitor".to_owned());
        s.cwd = "/home/ali/git/personal/claude-monitor".to_owned();
        s.branch = Some("008-ai-control-centre".to_owned());
        s.state = State::Working;
        s.last_activity_at = "2026-10-12T12:00:00Z".to_owned();
    });
    fix.session("sess-c", Some("proj-20"), |s| {
        s.agent_kind = "copilot".to_owned();
        s.repo_root = Some("/home/ali/git/personal/nix-config".to_owned());
        s.cwd = "/home/ali/git/personal/nix-config/.claude/worktrees/x".to_owned();
        s.state = State::Interrupted;
        s.last_activity_at = "2026-10-12T11:30:00Z".to_owned();
    });
    fix.session("sess-d", Some("proj-20"), |s| {
        s.repo_root = Some("/home/ali/git/personal/nix-config".to_owned());
        s.cwd = "/home/ali/git/personal/nix-config".to_owned();
        s.state = State::Finished;
        s.done_at = Some("2026-10-12T13:00:00Z".to_owned());
        s.last_activity_at = "2026-10-12T12:30:00Z".to_owned();
    });
    fix.note(
        "note-1",
        "project",
        "proj-20",
        "Ship the export first.",
        "alison",
    );
    fix.event(
        "2026-10-12T10:00:01Z",
        "project",
        "proj-20",
        &user("alison"),
    );
    fix.event(
        "2026-10-12T10:05:00Z",
        "progress_binding",
        "bind-20",
        &system("reconcile"),
    );
    fix.event(
        "2026-10-12T12:00:01Z",
        "session",
        "sess-b",
        &agent("claude", Some("claude-opus-5-5")),
    );
    fix.event("2026-10-12T13:00:00Z", "session", "sess-d", &user("alison"));
    fix
}
