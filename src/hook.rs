use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use rusqlite::{Transaction, TransactionBehavior};
use serde_json::{json, Value};

use crate::agents::claude_registry::{self, RegistryError};
use crate::agents::{self, AgentKind, ParseError, ParsedHook};
use crate::env::{Clock, Env, EnvError};
use crate::model::{Author, Event, Session, StartSource};
use crate::store::events::{EventsError, NewEvent};
use crate::store::sessions::SessionsError;
use crate::store::{resolve_state_dir, StateDirEnv, Store, StoreError, StoreOptions};

pub const LOG_FILE_NAME: &str = "hook.log";
pub const DEFAULT_AGENT: &str = "claude";

const PRIVATE_DIR_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;
const HOOK_COMPONENT: &str = "hook";
const ENTITY_SESSION: &str = "session";
const NO_EVENT: &str = "-";
const GIT_DIR_PREFIX: &str = "gitdir:";
const GIT_BRANCH_PREFIX: &str = "ref: refs/heads/";

/// Git facts about a working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitInfo {
    pub root: String,
    pub branch: Option<String>,
}

pub trait GitProbe: Send + Sync {
    /// `None` when `cwd` is not inside a git work tree.
    fn probe(&self, cwd: &str) -> Option<GitInfo>;
}

#[derive(Debug, Clone, Copy)]
pub struct NoGit;

impl GitProbe for NoGit {
    fn probe(&self, _cwd: &str) -> Option<GitInfo> {
        None
    }
}

/// Reads `.git/HEAD` directly: a hook has a 50 ms budget, too tight to spawn git.
/// A detached HEAD has a root but no branch.
#[derive(Debug, Clone, Copy)]
pub struct FsGit;

impl GitProbe for FsGit {
    fn probe(&self, cwd: &str) -> Option<GitInfo> {
        for dir in Path::new(cwd).ancestors() {
            let dot_git = dir.join(".git");
            let Ok(meta) = fs::metadata(&dot_git) else {
                continue;
            };
            let git_dir = if meta.is_dir() {
                dot_git
            } else {
                let text = fs::read_to_string(&dot_git).ok()?;
                let target = text
                    .lines()
                    .find_map(|line| line.strip_prefix(GIT_DIR_PREFIX))?
                    .trim();
                dir.join(target)
            };
            let branch = fs::read_to_string(git_dir.join("HEAD"))
                .ok()
                .and_then(|head| {
                    head.trim()
                        .strip_prefix(GIT_BRANCH_PREFIX)
                        .map(str::to_owned)
                });
            return Some(GitInfo {
                root: dir.to_str()?.to_owned(),
                branch,
            });
        }
        None
    }
}

pub struct HookContext<'a> {
    pub agent: &'a str,
    pub state_dir: &'a Path,
    pub tmux_pane: Option<&'a str>,
    pub clock: &'a dyn Clock,
    pub git: &'a dyn GitProbe,
    /// The agent process: the hook's parent. `None` records no pid.
    pub agent_pid: Option<u32>,
    /// Providers used to probe the agent process and the boot id. `None`
    /// records neither.
    pub env: Option<&'a Env>,
}

#[derive(Debug, thiserror::Error)]
enum HookError {
    #[error("cannot read hook payload from stdin: {0}")]
    ReadStdin(std::io::Error),
    #[error("payload is not valid JSON: {0}")]
    InvalidJson(serde_json::Error),
    #[error("unknown agent kind `{0}`")]
    UnknownAgent(String),
    #[error("{0}")]
    Parse(#[from] ParseError),
    #[error("{0}")]
    Store(#[from] StoreError),
    #[error("{0}")]
    Sessions(#[from] SessionsError),
    #[error("{0}")]
    Events(#[from] EventsError),
    #[error("cannot probe the agent process: {0}")]
    Probe(EnvError),
    #[error("cannot read the Claude session registry: {0}")]
    Registry(RegistryError),
    #[error("cannot begin write transaction: {0}")]
    Begin(rusqlite::Error),
    #[error("cannot commit write transaction: {0}")]
    Commit(rusqlite::Error),
}

/// `input` with `hook_event_name` set to `event` when the payload is a JSON
/// object without a non-null `hook_event_name`. Anything else is returned as is.
pub fn inject_event(input: &str, _event: Option<&str>) -> String {
    input.to_owned()
}

/// Ingest one hook payload. Never fails and never prints to stdout: errors go
/// to `<state-dir>/hook.log` and the update is dropped.
pub fn ingest(ctx: &HookContext<'_>, input: &str) {
    let registry = agents::kinds();
    let kinds: Vec<&dyn AgentKind> = registry.iter().map(|kind| kind.as_ref()).collect();
    ingest_with(ctx, &kinds, input);
}

/// [`ingest`] against an explicit registry instead of [`agents::kinds`].
pub fn ingest_with(ctx: &HookContext<'_>, kinds: &[&dyn AgentKind], input: &str) {
    let raw: Value = match serde_json::from_str(input) {
        Ok(raw) => raw,
        Err(e) => return log_failure(ctx, NO_EVENT, &HookError::InvalidJson(e)),
    };
    let event_name = raw
        .get("hook_event_name")
        .and_then(Value::as_str)
        .unwrap_or(NO_EVENT);
    if let Err(e) = ingest_payload(ctx, kinds, &raw) {
        log_failure(ctx, event_name, &e);
    }
}

fn ingest_payload(
    ctx: &HookContext<'_>,
    kinds: &[&dyn AgentKind],
    raw: &Value,
) -> Result<(), HookError> {
    let kind = kinds
        .iter()
        .find(|kind| kind.id() == ctx.agent)
        .ok_or_else(|| HookError::UnknownAgent(ctx.agent.to_owned()))?;
    let hooks = kind.parse_hook(raw)?;
    if hooks.is_empty() {
        return Ok(());
    }

    let store = Store::open_in_dir(ctx.state_dir, &StoreOptions::hook())?;
    let now = ctx.clock.now();
    for hook in &hooks {
        apply(ctx, &store, hook, &now)?;
    }
    Ok(())
}

fn current_dir_string() -> Option<String> {
    std::env::current_dir()
        .ok()
        .map(|p| p.display().to_string())
}

fn apply(
    ctx: &HookContext<'_>,
    store: &Store,
    hook: &ParsedHook,
    now: &str,
) -> Result<(), HookError> {
    // The guard only issues BEGIN IMMEDIATE / COMMIT; the Store methods below
    // run on the same connection, so they are inside the transaction.
    let tx = Transaction::new_unchecked(store.connection(), TransactionBehavior::Immediate)
        .map_err(HookError::Begin)?;

    let existing = store.get_session_by_agent_id(ctx.agent, &hook.session_id)?;
    let created = existing.is_none();
    let mut session = match existing {
        Some(session) => session,
        None => new_session(ctx, store, hook, now)?,
    };
    let before = (!created).then(|| session.clone());

    session.apply_event(&hook.event, now);
    now.clone_into(&mut session.last_activity_at);
    if matches!(hook.event, Event::SessionStart { .. }) {
        session.clear_done_on_respawn();
    }
    if let Some(pane) = ctx.tmux_pane {
        if session.tmux_pane.as_deref() != Some(pane) {
            session.tmux_pane = Some(pane.to_owned());
        }
    }
    if !matches!(hook.event, Event::SessionEnd { .. }) {
        capture_liveness(ctx, &mut session);
    }
    match ctx.git.probe(&session.cwd) {
        Some(git) => {
            session.repo_root = Some(git.root);
            session.branch = git.branch;
        }
        None => {
            session.repo_root = None;
            session.branch = None;
        }
    }
    if matches!(hook.event, Event::UserPromptSubmit) && session.task_description.is_none() {
        session.task_description = hook
            .prompt
            .as_deref()
            .filter(|prompt| !prompt.trim().is_empty())
            .map(str::to_owned);
    }
    if matches!(hook.event, Event::Stop) {
        if let Some(message) = &hook.last_assistant_message {
            session.left_off = Some(message.clone());
        }
    }

    store.upsert_session(&session)?;
    append_events(ctx, store, before.as_ref(), &session, now)?;
    tx.commit().map_err(HookError::Commit)
}

/// Records the agent pid, its start time and the boot id, overwriting values
/// left by an earlier run of a resumed session. A probe failure is logged and
/// never fails the hook.
fn capture_liveness(ctx: &HookContext<'_>, session: &mut Session) {
    let Some(env) = ctx.env else {
        return;
    };
    match env.boot_id.current() {
        Ok(boot_id) => session.boot_id = Some(boot_id),
        Err(e) => log_failure(ctx, "probe", &HookError::Probe(e)),
    }
    let Some(ppid) = ctx.agent_pid else {
        return;
    };
    let pid = registry_pid(ctx, env, session).unwrap_or(ppid);
    match env.processes.start_time(pid) {
        Ok(Some(start)) => {
            session.pid = Some(pid);
            session.pid_start = Some(start);
        }
        Ok(None) => clear_stale_pid(session, pid),
        Err(e) => {
            log_failure(ctx, "probe", &HookError::Probe(e));
            clear_stale_pid(session, pid);
        }
    }
}

/// The pid Claude's own session registry gives for this session, which stays
/// right when a wrapper process sits between the agent and the hook. `None`
/// keeps the caller's `$PPID`; a registry error is logged, never fatal.
fn registry_pid(ctx: &HookContext<'_>, env: &Env, session: &Session) -> Option<u32> {
    if session.agent_kind != DEFAULT_AGENT {
        return None;
    }
    match claude_registry::live_pid_for_session(env, &session.agent_session_id) {
        Ok(pid) => pid,
        Err(RegistryError::NoConfigDir) => None,
        Err(e) => {
            log_failure(ctx, "probe", &HookError::Registry(e));
            None
        }
    }
}

/// A pid with no readable start time cannot be checked against reuse, and the
/// start time recorded for a different pid would wrongly mark this one dead.
fn clear_stale_pid(session: &mut Session, pid: u32) {
    if session.pid != Some(pid) {
        session.pid = None;
        session.pid_start = None;
    }
}

fn new_session(
    ctx: &HookContext<'_>,
    store: &Store,
    hook: &ParsedHook,
    now: &str,
) -> Result<Session, HookError> {
    let cwd = hook
        .cwd
        .clone()
        .or_else(current_dir_string)
        .unwrap_or_default();
    let mut session = Session::new(
        &uuid::Uuid::new_v4().to_string(),
        ctx.agent,
        &hook.session_id,
        &cwd,
        now,
    );
    if matches!(
        hook.event,
        Event::SessionStart {
            source: StartSource::Clear
        }
    ) {
        session.resumed_from = store
            .find_cleared_predecessor(ctx.tmux_pane, &cwd)?
            .map(|predecessor| predecessor.id);
    }
    Ok(session)
}

fn state_payload(session: &Session) -> Value {
    json!({ "state": session.state.as_str(), "end_reason": session.end_reason })
}

fn location_payload(session: &Session) -> Value {
    json!({
        "cwd": session.cwd,
        "repo_root": session.repo_root,
        "branch": session.branch,
        "tmux_pane": session.tmux_pane,
    })
}

/// One row per real change; hooks that change nothing append nothing.
fn append_events(
    ctx: &HookContext<'_>,
    store: &Store,
    before: Option<&Session>,
    after: &Session,
    now: &str,
) -> Result<(), HookError> {
    let agent = Author::Agent {
        kind: ctx.agent.to_owned(),
        model: None,
    };
    let system = Author::System {
        component: HOOK_COMPONENT.to_owned(),
    };
    match before {
        None => {
            let payload = json!({
                "agent_kind": after.agent_kind,
                "agent_session_id": after.agent_session_id,
                "resumed_from": after.resumed_from,
                "state": after.state.as_str(),
                "cwd": after.cwd,
                "repo_root": after.repo_root,
                "branch": after.branch,
                "tmux_pane": after.tmux_pane,
            });
            store.append_event(&session_event(
                now,
                &after.id,
                "session.created",
                &payload,
                &system,
            ))?;
        }
        Some(before) => {
            let payload = state_payload(after);
            store.append_event_if_changed(
                &state_payload(before),
                &session_event(now, &after.id, "session.state_changed", &payload, &agent),
            )?;
        }
    }

    let old_task = before.and_then(|s| s.task_description.as_ref());
    if let (None, Some(task)) = (old_task, &after.task_description) {
        let payload = json!({ "task_description": task });
        store.append_event(&session_event(
            now,
            &after.id,
            "session.task_description_set",
            &payload,
            &agent,
        ))?;
    }

    let old_left_off = before.and_then(|s| s.left_off.as_ref());
    if let Some(left_off) = after.left_off.as_ref().filter(|l| Some(*l) != old_left_off) {
        let payload = json!({ "left_off_chars": left_off.chars().count() });
        store.append_event(&session_event(
            now,
            &after.id,
            "session.left_off_changed",
            &payload,
            &agent,
        ))?;
    }

    if let Some(before) = before {
        let payload = location_payload(after);
        store.append_event_if_changed(
            &location_payload(before),
            &session_event(
                now,
                &after.id,
                "session.location_changed",
                &payload,
                &system,
            ),
        )?;
    }
    Ok(())
}

fn session_event<'a>(
    at: &'a str,
    session_id: &'a str,
    kind: &'a str,
    payload: &'a Value,
    author: &'a Author,
) -> NewEvent<'a> {
    NewEvent {
        at,
        entity_kind: ENTITY_SESSION,
        entity_id: session_id,
        kind,
        payload,
        author,
    }
}

/// Reduces `text` to one line with no spaces when `spaces` is false, so a log
/// line stays parseable as `key=value` pairs.
fn one_line(text: &str, spaces: bool) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() || (!spaces && c.is_whitespace()) {
                if spaces {
                    ' '
                } else {
                    '_'
                }
            } else {
                c
            }
        })
        .collect()
}

fn log_failure(ctx: &HookContext<'_>, event: &str, error: &HookError) {
    let line = format!(
        "{} agent={} event={} error={}\n",
        ctx.clock.now(),
        one_line(ctx.agent, false),
        one_line(event, false),
        one_line(&error.to_string(), true),
    );
    if let Err(e) = append_log(ctx.state_dir, &line) {
        eprintln!("claude-monitor hook: cannot write {LOG_FILE_NAME}: {e}; dropped: {line}");
    }
}

fn append_log(state_dir: &Path, line: &str) -> std::io::Result<()> {
    if !state_dir.is_dir() {
        DirBuilder::new()
            .recursive(true)
            .mode(PRIVATE_DIR_MODE)
            .create(state_dir)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(PRIVATE_FILE_MODE)
        .open(state_dir.join(LOG_FILE_NAME))?;
    file.write_all(line.as_bytes())
}

/// Entry point for `claude-monitor hook`, called by the agent's hook
/// mechanism. Always returns normally (exit 0) and never writes to stdout:
/// Claude Code may interpret stdout for some events.
pub fn run(agent: Option<&str>, state_dir: Option<&Path>, env: &Env) {
    let agent = agents::resolve_agent(agent, env.vars.as_ref());
    let state_dir: PathBuf = match resolve_state_dir(state_dir, &StateDirEnv::from_process()) {
        Ok(dir) => dir,
        Err(e) => return eprintln!("claude-monitor hook: {e}"),
    };
    let tmux_pane = std::env::var("TMUX_PANE").ok();
    let ctx = HookContext {
        agent,
        state_dir: &state_dir,
        tmux_pane: tmux_pane.as_deref(),
        clock: env.clock.as_ref(),
        git: &FsGit,
        // The agent runs the hook as a direct child, so its parent is the agent.
        agent_pid: Some(std::os::unix::process::parent_id()),
        env: Some(env),
    };

    let mut input = String::new();
    match std::io::stdin().read_to_string(&mut input) {
        Ok(_) => ingest(&ctx, &input),
        Err(e) => log_failure(&ctx, NO_EVENT, &HookError::ReadStdin(e)),
    }
}
