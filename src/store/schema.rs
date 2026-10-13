pub(super) const V4: &str = "
ALTER TABLE sessions ADD COLUMN tmux_pane_index TEXT;
ALTER TABLE sessions ADD COLUMN tmux_window_panes TEXT;
ALTER TABLE sessions ADD COLUMN tmux_window_layout TEXT;
";

pub(super) const V3: &str = "
ALTER TABLE sessions ADD COLUMN tmux_session TEXT;
ALTER TABLE sessions ADD COLUMN tmux_window_name TEXT;
ALTER TABLE sessions ADD COLUMN tmux_window_index TEXT;
";

pub(super) const V2: &str = "
CREATE TRIGGER events_no_update BEFORE UPDATE ON events
BEGIN
    SELECT RAISE(ABORT, 'events is append-only');
END;

CREATE TRIGGER events_no_delete BEFORE DELETE ON events
BEGIN
    SELECT RAISE(ABORT, 'events is append-only');
END;
";

pub(super) const V1: &str = "
CREATE TABLE progress_binding (
    id TEXT PRIMARY KEY,
    owner_kind TEXT NOT NULL CHECK (owner_kind IN ('session', 'project')),
    owner_id TEXT NOT NULL,
    source_kind TEXT NOT NULL CHECK (source_kind IN ('agent_todo', 'speckit_tasks', 'markdown_checklist')),
    source_path TEXT,
    chosen_by TEXT NOT NULL CHECK (chosen_by IN ('auto', 'user')),
    read_at TEXT NOT NULL
);

CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'parked', 'done', 'archived')),
    note TEXT,
    progress_binding_id TEXT REFERENCES progress_binding (id),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    agent_kind TEXT NOT NULL,
    agent_session_id TEXT NOT NULL,
    resumed_from TEXT REFERENCES sessions (id),
    forked_from TEXT REFERENCES sessions (id),
    repo_root TEXT,
    cwd TEXT NOT NULL,
    branch TEXT,
    worktree_exists INTEGER NOT NULL DEFAULT 1 CHECK (worktree_exists IN (0, 1)),
    state TEXT NOT NULL DEFAULT 'unknown' CHECK (state IN ('working', 'needs_permission', 'idle', 'finished', 'interrupted', 'unknown')),
    end_reason TEXT,
    task_description TEXT,
    left_off TEXT,
    pid INTEGER,
    pid_start INTEGER,
    boot_id TEXT,
    tmux_pane TEXT,
    started_at TEXT NOT NULL,
    last_activity_at TEXT NOT NULL,
    state_since TEXT NOT NULL,
    project_id TEXT REFERENCES projects (id),
    parked INTEGER NOT NULL DEFAULT 0 CHECK (parked IN (0, 1)),
    dismissed INTEGER NOT NULL DEFAULT 0 CHECK (dismissed IN (0, 1)),
    done_at TEXT,
    CHECK (done_at IS NULL OR state = 'finished')
);

CREATE INDEX idx_sessions_state_last_activity ON sessions (state, last_activity_at);
CREATE INDEX idx_sessions_project ON sessions (project_id);
CREATE INDEX idx_sessions_repo_root ON sessions (repo_root);
CREATE UNIQUE INDEX idx_sessions_agent_session ON sessions (agent_kind, agent_session_id);

CREATE TABLE steps (
    binding_id TEXT NOT NULL REFERENCES progress_binding (id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    text TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('todo', 'doing', 'done', 'blocked')),
    group_name TEXT,
    external_id TEXT,
    PRIMARY KEY (binding_id, ordinal)
);

CREATE TABLE notes (
    id TEXT PRIMARY KEY,
    owner_kind TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at TEXT NOT NULL,
    author TEXT NOT NULL
);

CREATE TABLE summaries (
    id TEXT PRIMARY KEY,
    owner_kind TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    body TEXT NOT NULL,
    model TEXT,
    model_written INTEGER NOT NULL CHECK (model_written IN (0, 1)),
    created_at TEXT NOT NULL
);

CREATE TABLE links (
    id TEXT PRIMARY KEY,
    owner_kind TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    cortex_node_id TEXT NOT NULL,
    cortex_doc_id TEXT,
    cortex_kind TEXT NOT NULL CHECK (cortex_kind IN ('project', 'task', 'memory')),
    last_agreed_at TEXT,
    last_remote_status TEXT
);

CREATE TABLE respawn_request (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions (id),
    requested_at TEXT NOT NULL,
    outcome TEXT NOT NULL,
    tmux_target TEXT,
    new_session_id TEXT REFERENCES sessions (id)
);

CREATE TABLE events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    at TEXT NOT NULL,
    entity_kind TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    payload_json TEXT NOT NULL DEFAULT '{}',
    author_kind TEXT NOT NULL CHECK (author_kind IN ('user', 'agent', 'system')),
    author_name TEXT NOT NULL
);
";
