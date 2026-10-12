// Panicking on bad test setup is the correct failure mode here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use claude_monitor::store::{
    resolve_state_dir, StateDirEnv, Store, StoreError, StoreOptions, SCHEMA_VERSION,
};
use rusqlite::Connection;
use tempfile::TempDir;

const V1_FIXTURE: &str = include_str!("fixtures/store_v1.sql");

const TABLES: [&str; 9] = [
    "sessions",
    "projects",
    "steps",
    "progress_binding",
    "notes",
    "summaries",
    "links",
    "respawn_request",
    "events",
];

const INDEXES: [&str; 4] = [
    "idx_sessions_state_last_activity",
    "idx_sessions_project",
    "idx_sessions_repo_root",
    "idx_sessions_agent_session",
];

fn db_path(dir: &TempDir) -> PathBuf {
    dir.path().join("state.db")
}

fn open(path: &Path) -> Store {
    Store::open(path, &StoreOptions::default()).unwrap()
}

fn master_names(conn: &Connection, kind: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = ?1 AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap();
    stmt.query_map([kind], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn schema_snapshot(conn: &Connection) -> Vec<(String, String)> {
    let mut stmt = conn
        .prepare("SELECT name, sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap();
    stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn user_version(conn: &Connection) -> u32 {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn insert_session(conn: &Connection, id: &str, agent_session_id: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "INSERT INTO sessions (id, agent_kind, agent_session_id, cwd, started_at, last_activity_at, state_since) \
         VALUES (?1, 'claude', ?2, '/work', '2026-10-12T14:03:22Z', '2026-10-12T14:03:22Z', '2026-10-12T14:03:22Z')",
        [id, agent_session_id],
    )
}

#[test]
fn creates_every_table_and_index_from_empty() {
    let dir = TempDir::new().unwrap();
    let store = open(&db_path(&dir));

    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    let mut want_tables: Vec<String> = TABLES.iter().map(|t| (*t).to_owned()).collect();
    want_tables.sort();
    let mut tables = master_names(store.connection(), "table");
    tables.retain(|t| t != "sqlite_sequence");
    assert_eq!(tables, want_tables);

    let mut want_indexes: Vec<String> = INDEXES.iter().map(|t| (*t).to_owned()).collect();
    want_indexes.sort();
    assert_eq!(master_names(store.connection(), "index"), want_indexes);
}

#[test]
fn migrates_a_zero_version_database_file() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir);
    drop(Connection::open(&path).unwrap());

    let store = open(&path);

    assert_eq!(user_version(store.connection()), SCHEMA_VERSION);
    assert_eq!(
        master_names(store.connection(), "table").len(),
        TABLES.len()
    );
}

#[test]
fn reopening_is_idempotent_and_keeps_rows() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir);
    let first = open(&path);
    insert_session(first.connection(), "s-1", "conv-1").unwrap();
    let before = schema_snapshot(first.connection());
    drop(first);

    let second = open(&path);
    let third = open(&path);

    assert_eq!(schema_snapshot(second.connection()), before);
    assert_eq!(schema_snapshot(third.connection()), before);
    assert_eq!(second.schema_version().unwrap(), SCHEMA_VERSION);
    let rows: u32 = third
        .connection()
        .query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 1);
}

#[test]
fn recorded_v1_database_migrates_to_v2_without_data_loss() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir);
    let raw = Connection::open(&path).unwrap();
    raw.execute_batch(V1_FIXTURE).unwrap();
    raw.execute(
        "INSERT INTO events (at, entity_kind, entity_id, kind, author_kind, author_name) \
         VALUES ('2026-10-12T14:03:22Z', 'session', 's-1', 'state_changed', 'system', 'hook')",
        [],
    )
    .unwrap();
    let before = schema_snapshot(&raw);
    assert_eq!(user_version(&raw), 1);
    drop(raw);

    let store = open(&path);

    let mut expected = before;
    expected.extend(
        ["events_no_delete", "events_no_update"]
            .iter()
            .map(|name| (String::from(*name), String::new())),
    );
    let after: Vec<String> = schema_snapshot(store.connection())
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let mut want: Vec<String> = expected.into_iter().map(|(name, _)| name).collect();
    want.sort();
    assert_eq!(after, want);
    assert_eq!(user_version(store.connection()), 2);
    let events = store.events_since(0, 10).unwrap();
    assert_eq!(
        events
            .iter()
            .map(|e| e.entity_id.as_str())
            .collect::<Vec<_>>(),
        ["s-1"]
    );
    assert!(store
        .connection()
        .execute("DELETE FROM events", [])
        .is_err());
    let (name, state): (String, String) = store
        .connection()
        .query_row(
            "SELECT p.name, s.state FROM sessions s JOIN projects p ON p.id = s.project_id WHERE s.id = 's-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((name.as_str(), state.as_str()), ("recorded", "idle"));
}

#[test]
fn wal_journal_and_foreign_keys_are_on() {
    let dir = TempDir::new().unwrap();
    let store = open(&db_path(&dir));

    let journal: String = store
        .connection()
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    let foreign_keys: u32 = store
        .connection()
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .unwrap();
    assert_eq!(journal, "wal");
    assert_eq!(foreign_keys, 1);
    let orphan = store.connection().execute(
        "INSERT INTO sessions (id, agent_kind, agent_session_id, cwd, started_at, last_activity_at, state_since, project_id) \
         VALUES ('s-2', 'claude', 'conv-2', '/work', 'a', 'a', 'a', 'missing-project')",
        [],
    );
    assert!(orphan.is_err());
}

#[test]
fn busy_timeout_follows_the_options() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir);
    let hook = Store::open(&path, &StoreOptions::hook()).unwrap();
    let viewer = Store::open(&path, &StoreOptions::viewer()).unwrap();

    let timeout = |store: &Store| -> i64 {
        store
            .connection()
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap()
    };
    assert_eq!(timeout(&hook), 200);
    assert_eq!(timeout(&viewer), 5000);
}

#[test]
fn database_file_is_0600_and_new_directory_is_0700() {
    let dir = TempDir::new().unwrap();
    let state_dir = dir.path().join("nested").join("ai-control-centre");

    let store = Store::open_in_dir(&state_dir, &StoreOptions::default()).unwrap();

    assert_eq!(mode(store.path()), 0o600);
    assert_eq!(mode(&state_dir), 0o700);
}

#[test]
fn loose_existing_file_mode_is_tightened() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir);
    drop(open(&path));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

    drop(open(&path));

    assert_eq!(mode(&path), 0o600);
}

#[test]
fn state_dir_override_wins_over_the_environment() {
    let env = StateDirEnv {
        xdg_state_home: Some(PathBuf::from("/xdg")),
        home: Some(PathBuf::from("/home/u")),
    };

    let resolved = resolve_state_dir(Some(Path::new("/custom/dir")), &env).unwrap();

    assert_eq!(resolved, Path::new("/custom/dir"));
}

#[test]
fn state_dir_defaults_follow_xdg_then_home() {
    let xdg = StateDirEnv {
        xdg_state_home: Some(PathBuf::from("/xdg")),
        home: Some(PathBuf::from("/home/u")),
    };
    assert_eq!(
        resolve_state_dir(None, &xdg).unwrap(),
        Path::new("/xdg/ai-control-centre")
    );

    let home_only = StateDirEnv {
        xdg_state_home: None,
        home: Some(PathBuf::from("/home/u")),
    };
    let want = if cfg!(target_os = "macos") {
        "/home/u/Library/Application Support/ai-control-centre"
    } else {
        "/home/u/.local/state/ai-control-centre"
    };
    assert_eq!(
        resolve_state_dir(None, &home_only).unwrap(),
        Path::new(want)
    );

    let relative_xdg = StateDirEnv {
        xdg_state_home: Some(PathBuf::from("relative/xdg")),
        home: Some(PathBuf::from("/home/u")),
    };
    assert_eq!(
        resolve_state_dir(None, &relative_xdg).unwrap(),
        resolve_state_dir(None, &home_only).unwrap()
    );

    let neither = StateDirEnv::default();
    assert!(matches!(
        resolve_state_dir(None, &neither),
        Err(StoreError::NoStateDir)
    ));
}

#[test]
fn override_directory_holds_state_db() {
    let dir = TempDir::new().unwrap();
    let state_dir = dir.path().join("override");

    let store = Store::open_in_dir(&state_dir, &StoreOptions::default()).unwrap();

    assert_eq!(store.path(), state_dir.join("state.db"));
    assert!(state_dir.join("state.db").is_file());
}

#[test]
fn corrupt_database_is_a_typed_error_naming_path_and_operation() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir);
    fs::write(&path, "this is not a sqlite database, ".repeat(20)).unwrap();

    let err = Store::open(&path, &StoreOptions::default()).unwrap_err();

    assert_eq!(err.path(), Some(path.as_path()));
    assert!(!err.operation().is_empty());
    assert!(!matches!(err, StoreError::NewerSchema { .. }));
    let message = err.to_string();
    assert!(message.contains(path.to_str().unwrap()), "{message}");
    assert!(message.contains(err.operation().split(' ').next().unwrap()));
}

#[test]
fn newer_schema_version_is_refused_and_left_untouched() {
    let dir = TempDir::new().unwrap();
    let path = db_path(&dir);
    let raw = Connection::open(&path).unwrap();
    raw.execute_batch("PRAGMA user_version = 99;").unwrap();
    drop(raw);

    let err = Store::open(&path, &StoreOptions::default()).unwrap_err();

    match &err {
        StoreError::NewerSchema {
            path: p,
            found,
            supported,
        } => {
            assert_eq!(p, &path);
            assert_eq!(*found, 99);
            assert_eq!(*supported, SCHEMA_VERSION);
        }
        other => panic!("expected NewerSchema, got {other:?}"),
    }
    assert!(err.to_string().contains(path.to_str().unwrap()));
    let raw = Connection::open(&path).unwrap();
    assert_eq!(user_version(&raw), 99);
    assert!(master_names(&raw, "table").is_empty());
}

#[test]
fn schema_constraints_hold() {
    let dir = TempDir::new().unwrap();
    let store = open(&db_path(&dir));
    let conn = store.connection();

    insert_session(conn, "s-1", "conv-1").unwrap();
    assert!(
        insert_session(conn, "s-2", "conv-1").is_err(),
        "(agent_kind, agent_session_id) must be unique"
    );
    let done_while_working = conn.execute(
        "UPDATE sessions SET state = 'working', done_at = '2026-10-12T15:00:00Z' WHERE id = 's-1'",
        [],
    );
    assert!(
        done_while_working.is_err(),
        "done_at is valid only when finished"
    );
    let done_while_finished = conn.execute(
        "UPDATE sessions SET state = 'finished', done_at = '2026-10-12T15:00:00Z' WHERE id = 's-1'",
        [],
    );
    assert_eq!(done_while_finished.unwrap(), 1);
}
