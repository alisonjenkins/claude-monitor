//! Respawn planning: turn a stored session into "start this command in this
//! directory", or into a refusal that names the problem and the ways out.
//!
//! The planner is pure over the [`Env`] providers: it reads the filesystem,
//! process table and tmux availability and returns a [`Plan`] or [`Refusal`].
//! It never creates a directory, runs git or starts a process; the executor
//! does that, and only for what the plan lists.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::agents::{AgentKind, Launch, LaunchError, LaunchMode, LauncherConfig, Precondition};
use crate::env::{Env, EnvError, SpawnedWindow, Vars};
use crate::model::{Author, Session};
use crate::store::events::NewEvent;
use crate::store::Store;

/// Why a respawn was refused. Codes are the strings stored in
/// `RespawnRequest.outcome` as `refused:<code>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalCode {
    AlreadyRunning,
    MissingDirectory,
    MissingTranscript,
    BranchGone,
    NoLauncher,
    NoTmux,
    /// The flags contradict each other (`--in` without `--fresh`).
    InvalidOptions,
}

impl RefusalCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AlreadyRunning => "already_running",
            Self::MissingDirectory => "missing_directory",
            Self::MissingTranscript => "missing_transcript",
            Self::BranchGone => "branch_gone",
            Self::NoLauncher => "no_launcher",
            Self::NoTmux => "no_tmux",
            Self::InvalidOptions => "invalid_options",
        }
    }

    /// The command line exit code: 2 for a refused precondition, 1 for a
    /// usage error.
    pub fn exit_code(self) -> u8 {
        match self {
            Self::InvalidOptions => 1,
            _ => 2,
        }
    }
}

/// Something the user can do instead, as a description and a command to run.
/// Never taken automatically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alternative {
    pub description: String,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: RefusalCode,
    pub message: String,
    pub alternatives: Vec<Alternative>,
}

/// What a respawn will do, in order: run each `setup` command (for example
/// `git worktree add`), then start `launch`. `notices` are shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub session_id: String,
    pub mode: LaunchMode,
    pub launch: Launch,
    pub setup: Vec<Launch>,
    pub notices: Vec<String>,
    /// The tmux window name: the one recorded for the session, else the
    /// branch, else the directory name.
    pub label: String,
    /// The tmux session the window opens in: the configured override, else the
    /// one recorded for the session, else [`DEFAULT_TMUX_SESSION`].
    pub tmux_session: String,
    /// The window index recorded for the session, used only to order windows.
    pub window_index: Option<u32>,
    /// Where the session sat inside its window, when all of that was recorded.
    pub pane: Option<PanePlacement>,
}

/// A session's recorded place in a tmux window, enough to put it back as a
/// pane of that window next to the sessions that shared it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanePlacement {
    /// The tmux session recorded for the session, even when an override sends
    /// the respawn elsewhere: windows only group within one recorded session.
    pub recorded_session: String,
    pub window_name: String,
    pub window_index: u32,
    pub pane_index: u32,
    /// The window's pane count when the session was last seen.
    pub window_panes: Option<u32>,
    pub window_layout: Option<String>,
    /// Set by [`plan_batch`] when other plans of the batch open panes in the
    /// same window.
    pub grouped: bool,
}

impl PanePlacement {
    fn recorded(session: &Session) -> Option<Self> {
        let non_empty = |value: &Option<String>| value.clone().filter(|v| !v.is_empty());
        Some(Self {
            recorded_session: non_empty(&session.tmux_session)?,
            window_name: non_empty(&session.tmux_window_name)?,
            window_index: session.tmux_window_index.as_deref()?.parse().ok()?,
            pane_index: session.tmux_pane_index.as_deref()?.parse().ok()?,
            window_panes: session
                .tmux_window_panes
                .as_deref()
                .and_then(|count| count.parse().ok()),
            window_layout: non_empty(&session.tmux_window_layout),
            grouped: false,
        })
    }
}

impl Plan {
    /// The recorded pane index, when the session opens as one of several
    /// panes of its window.
    pub fn grouped_pane(&self) -> Option<u32> {
        self.pane
            .as_ref()
            .filter(|pane| pane.grouped)
            .map(|pane| pane.pane_index)
    }

    /// `session:window-name`, where the window will open, plus `.pane` when
    /// the session opens as one of several panes of that window.
    pub fn tmux_target(&self) -> String {
        match self.grouped_pane() {
            Some(pane) => format!("{}:{}.{pane}", self.tmux_session, self.label),
            None => format!("{}:{}", self.tmux_session, self.label),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// Run `git worktree add` for a missing worktree whose branch still exists.
    pub recreate_worktree: bool,
    /// Start without resuming the conversation.
    pub fresh: bool,
    /// With `fresh`: start in this directory instead of the recorded one.
    pub in_dir: Option<PathBuf>,
    /// The launcher template configured for the session's agent kind.
    pub launcher: LauncherConfig,
}

/// Plan a respawn of `session` using the registered agent kind it names.
pub fn plan_respawn(session: &Session, env: &Env, options: &Options) -> Result<Plan, Refusal> {
    let kinds = crate::agents::kinds();
    let kind = kinds.iter().find(|k| k.id() == session.agent_kind);
    match kind {
        Some(kind) => plan_respawn_with(kind.as_ref(), session, env, options),
        None => Err(Refusal {
            code: RefusalCode::NoLauncher,
            message: format!("no launcher for agent kind `{}`", session.agent_kind),
            alternatives: Vec::new(),
        }),
    }
}

/// [`plan_respawn`] for an explicit agent kind.
///
/// Checks run in this order and the first failure is the refusal: usage
/// (`--in` needs `--fresh`), already running, tmux installed, target
/// directory, launcher, then the kind's resume preconditions (only when the
/// conversation is to be resumed).
pub fn plan_respawn_with(
    kind: &dyn AgentKind,
    session: &Session,
    env: &Env,
    options: &Options,
) -> Result<Plan, Refusal> {
    if options.in_dir.is_some() && !options.fresh {
        return Err(refuse(
            RefusalCode::InvalidOptions,
            "--in only applies together with --fresh".to_owned(),
        ));
    }
    if is_running(session, env) {
        let place = session
            .tmux_pane
            .as_deref()
            .map_or_else(String::new, |pane| format!(" in tmux pane {pane}"));
        return Err(refuse(
            RefusalCode::AlreadyRunning,
            format!("session {} is already running{place}", session.id),
        ));
    }
    if !env.tmux.is_installed() {
        return Err(refuse(
            RefusalCode::NoTmux,
            "tmux is not installed, so there is nowhere to host the session".to_owned(),
        ));
    }

    let mut target = session.clone();
    let mut notices = Vec::new();
    if let Some(dir) = &options.in_dir {
        target.cwd = dir.to_string_lossy().into_owned();
    }
    let mut setup = Vec::new();
    if !env.fs.exists(Path::new(&target.cwd)) {
        setup.push(recreate_or_refuse(session, &target, env, options)?);
    }
    if options.in_dir.is_some() {
        notices.push(format!(
            "starting a new session in {}, not the recorded {}",
            target.cwd, session.cwd
        ));
    }

    let mut mode = if options.fresh {
        notices.push(NOT_RESTORED.to_owned());
        LaunchMode::Fresh
    } else {
        LaunchMode::Resume
    };
    let launch = match kind.launch(&target, mode, &options.launcher) {
        Ok(launch) => launch,
        Err(LaunchError::ResumeUnsupported { kind: id }) if mode == LaunchMode::Resume => {
            mode = LaunchMode::Fresh;
            notices.push(format!("{id} cannot resume a conversation; {NOT_RESTORED}"));
            kind.launch(&target, mode, &options.launcher)
                .map_err(|e| no_launcher(session, &e))?
        }
        Err(e) => return Err(no_launcher(session, &e)),
    };

    if mode == LaunchMode::Resume {
        for precondition in kind.resume_preconditions(session, env) {
            if !env.fs.exists(&precondition.required_path) {
                return Err(missing_transcript(session, &precondition));
            }
        }
    }

    Ok(Plan {
        session_id: session.id.clone(),
        mode,
        launch,
        setup,
        notices,
        label: window_label(&target),
        tmux_session: target_tmux_session(session, env),
        window_index: session
            .tmux_window_index
            .as_deref()
            .and_then(|index| index.parse().ok()),
        pane: PanePlacement::recorded(session),
    })
}

/// The tmux session a respawn of `session` goes to: the explicit override,
/// else the recorded session, else [`DEFAULT_TMUX_SESSION`].
pub fn target_tmux_session(session: &Session, env: &Env) -> String {
    tmux_session_override(env)
        .or_else(|| session.tmux_session.clone().filter(|name| !name.is_empty()))
        .unwrap_or_else(|| DEFAULT_TMUX_SESSION.to_owned())
}

/// The name of the window a respawn of `session` opens: the recorded window
/// name, else one made from the branch or directory.
pub fn window_label(session: &Session) -> String {
    session
        .tmux_window_name
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| session_name(session))
}

/// The tmux session respawned windows go into.
pub const DEFAULT_TMUX_SESSION: &str = "agents";

/// The variable that overrides [`DEFAULT_TMUX_SESSION`].
pub const TMUX_SESSION_VAR: &str = "CLAUDE_MONITOR_TMUX_SESSION";

/// The variable holding the resume command template for `agent_kind`, for
/// example `CLAUDE_MONITOR_LAUNCHER_CLAUDE`.
pub fn launcher_var(agent_kind: &str) -> String {
    format!(
        "CLAUDE_MONITOR_LAUNCHER_{}",
        agent_kind.to_ascii_uppercase().replace('-', "_")
    )
}

/// The launcher template configured for `agent_kind` (see [`launcher_var`]).
pub fn launcher_from_env(vars: &dyn Vars, agent_kind: &str) -> LauncherConfig {
    LauncherConfig {
        template: vars.var(&launcher_var(agent_kind)),
    }
}

/// The effective launcher template for `agent_kind`: the environment variable,
/// else `config.json`, else none.
pub fn launcher_for(env: &Env, agent_kind: &str) -> LauncherConfig {
    let from_env = launcher_from_env(env.vars.as_ref(), agent_kind);
    if from_env.template.is_some() {
        return from_env;
    }
    crate::config::load_launchers(env)
        .remove(agent_kind)
        .unwrap_or(from_env)
}

/// The tmux session the user forces every respawn into: the environment
/// variable, else `config.json`; `None` when neither is set.
pub fn tmux_session_override(env: &Env) -> Option<String> {
    env.vars
        .var(TMUX_SESSION_VAR)
        .or_else(|| crate::config::tmux_session_override(env))
}

/// The tmux session a respawn goes to when the session has no recorded
/// placement: the override, else [`DEFAULT_TMUX_SESSION`].
pub fn tmux_session_for(env: &Env) -> String {
    tmux_session_override(env).unwrap_or_else(|| DEFAULT_TMUX_SESSION.to_owned())
}

/// A respawn that started: the new window and the request awaiting the new
/// session's `SessionStart`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    pub session_id: String,
    pub pane_id: String,
    pub window_id: String,
    pub request_id: String,
}

/// Why executing a plan failed. Nothing is rolled back: a setup command that
/// ran stays done, and a window that started stays open.
#[derive(Debug, thiserror::Error)]
pub enum ExecuteError {
    #[error("setup command `{command}` failed: {detail}")]
    Setup { command: String, detail: String },
    #[error("{0}")]
    Tmux(#[from] EnvError),
    #[error("cannot {operation}: {detail}")]
    Store {
        operation: &'static str,
        detail: String,
    },
}

pub type Outcome = Result<Started, ExecuteError>;

/// Carry out `plan` and nothing else: run the plan's setup commands, ensure
/// the plan's tmux session, open one window for the launch, record the respawn
/// request for its pane and append `session.respawn_requested`. Never starts
/// any session the plan does not name.
///
/// Setup runs before tmux is touched: a failed `git worktree add` leaves no
/// window and no tmux server behind, and the new window's directory exists.
pub fn execute(store: &Store, env: &Env, plan: &Plan, now: &str) -> Outcome {
    open(store, env, plan, now, Open::Window)
}

/// Where a plan's launch opens.
#[derive(Clone, Copy)]
enum Open<'a> {
    /// A new window of the plan's tmux session.
    Window,
    /// A new pane in the window of an already opened pane.
    Split {
        first_pane: &'a str,
        window_id: &'a str,
    },
}

fn open(store: &Store, env: &Env, plan: &Plan, now: &str, target: Open<'_>) -> Outcome {
    let tmux_session = plan.tmux_session.as_str();
    for command in &plan.setup {
        run_setup(command)?;
    }
    let argv = launch_argv(&plan.launch);
    let window = match target {
        Open::Window => {
            env.tmux.ensure_session(tmux_session, &plan.launch.cwd)?;
            env.tmux
                .spawn_window(tmux_session, &plan.label, &plan.launch.cwd, &argv)?
        }
        Open::Split {
            first_pane,
            window_id,
        } => SpawnedWindow {
            pane_id: env.tmux.split_pane(first_pane, &plan.launch.cwd, &argv)?,
            window_id: window_id.to_owned(),
        },
    };

    let store_error = |operation: &'static str, e: &dyn std::fmt::Display| ExecuteError::Store {
        operation,
        detail: format!(
            "{e} (window {} is open in {tmux_session})",
            window.window_id
        ),
    };
    let request = store
        .create_respawn_request(&plan.session_id, &window.pane_id, now)
        .map_err(|e| store_error("record respawn request", &e))?;
    let payload = serde_json::json!({
        "pane": window.pane_id,
        "window": window.window_id,
        "tmux_session": tmux_session,
        "mode": plan.mode.as_str(),
        "request": request.id,
    });
    let author = Author::User {
        name: env.vars.var("USER").unwrap_or_else(|| "user".to_owned()),
    };
    store
        .append_event(&NewEvent {
            at: now,
            entity_kind: ENTITY_SESSION,
            entity_id: &plan.session_id,
            kind: "session.respawn_requested",
            payload: &payload,
            author: &author,
        })
        .map_err(|e| store_error("record respawn event", &e))?;
    Ok(Started {
        session_id: plan.session_id.clone(),
        pane_id: window.pane_id,
        window_id: window.window_id,
        request_id: request.id,
    })
}

/// Why a spawn that was planned did not happen or finish.
pub const SPAWN_FAILED: &str = "spawn_failed";

/// Each session of a batch with its plan or the reason it has none.
pub type Planned = Vec<(Session, Result<Plan, Refusal>)>;

/// The flags that shape every plan in a batch.
#[derive(Debug, Clone, Default)]
pub struct PlanFlags {
    pub fresh: bool,
    pub in_dir: Option<PathBuf>,
    pub recreate_worktree: bool,
}

/// A session of a batch that could not be read at all, which fails the whole
/// batch (a refusal, by contrast, is data).
#[derive(Debug, thiserror::Error)]
pub enum PlanBatchError {
    #[error("cannot read session {id}: {detail}")]
    ReadSession { id: String, detail: String },
    #[error("no session has id {id:?}")]
    UnknownSession { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedItem {
    pub started: Started,
    pub plan: Plan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedItem {
    pub session_id: String,
    pub code: &'static str,
    pub message: String,
    pub alternatives: Vec<Alternative>,
}

/// What became of one session of a batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Started(Box<StartedItem>),
    Refused(RefusedItem),
}

/// The panes of one window: same target tmux session, same recorded tmux
/// session (an override can merge several), same recorded window index and name.
type WindowKey = (String, String, u32, String);

/// The window `plan` reopens as a pane of, when a pane placement was recorded.
fn window_key(plan: &Plan) -> Option<WindowKey> {
    let pane = plan.pane.as_ref()?;
    Some((
        plan.tmux_session.clone(),
        pane.recorded_session.clone(),
        pane.window_index,
        pane.window_name.clone(),
    ))
}

/// Plan each session id with `flags`. Plans only; nothing starts. The
/// sessions that can start are gathered into windows (see [`WindowKey`]; a
/// session without a recorded pane is a window of its own), the windows
/// ordered by target tmux session, then recorded window index, then the order
/// of `ids`, and the panes of a window by pane index, so windows and panes
/// reopen in their original order. A window of several panes marks each plan's
/// pane as grouped. Refusals keep the places they were asked in.
pub fn plan_batch(
    store: &Store,
    env: &Env,
    ids: &[String],
    flags: &PlanFlags,
) -> Result<Planned, PlanBatchError> {
    let planned = plan_each(store, env, ids, flags)?;
    let mut windows: Vec<Vec<(Session, Result<Plan, Refusal>)>> = Vec::new();
    let mut by_key: HashMap<WindowKey, usize> = HashMap::new();
    for entry in planned.iter().filter(|(_, plan)| plan.is_ok()).cloned() {
        let key = entry.1.as_ref().ok().and_then(window_key);
        match key.as_ref().and_then(|key| by_key.get(key)).copied() {
            Some(window) => {
                if let Some(panes) = windows.get_mut(window) {
                    panes.push(entry);
                }
            }
            None => {
                if let Some(key) = key {
                    by_key.insert(key, windows.len());
                }
                windows.push(vec![entry]);
            }
        }
    }
    windows.sort_by_key(|panes| {
        panes
            .first()
            .and_then(|(_, plan)| plan.as_ref().ok())
            .map(|plan| {
                (
                    plan.tmux_session.clone(),
                    plan.window_index.unwrap_or(u32::MAX),
                )
            })
    });
    let grouped = |plan: &mut Result<Plan, Refusal>| {
        if let Some(pane) = plan.as_mut().ok().and_then(|plan| plan.pane.as_mut()) {
            pane.grouped = true;
        }
    };
    let mut startable = Vec::new();
    for mut panes in windows {
        panes.sort_by_key(|(_, plan)| {
            plan.as_ref()
                .ok()
                .and_then(|plan| plan.pane.as_ref())
                .map(|pane| pane.pane_index)
        });
        if panes.len() > 1 {
            panes.iter_mut().for_each(|(_, plan)| grouped(plan));
        }
        startable.extend(panes);
    }
    let mut sorted = startable.into_iter();
    Ok(planned
        .into_iter()
        .map(|entry| {
            if entry.1.is_ok() {
                sorted.next().unwrap_or(entry)
            } else {
                entry
            }
        })
        .collect())
}

fn plan_each(
    store: &Store,
    env: &Env,
    ids: &[String],
    flags: &PlanFlags,
) -> Result<Planned, PlanBatchError> {
    ids.iter()
        .map(|id| {
            let session = store
                .get_session(id)
                .map_err(|e| PlanBatchError::ReadSession {
                    id: id.clone(),
                    detail: e.to_string(),
                })?
                .ok_or_else(|| PlanBatchError::UnknownSession { id: id.clone() })?;
            let options = Options {
                recreate_worktree: flags.recreate_worktree,
                fresh: flags.fresh,
                in_dir: flags.in_dir.clone(),
                launcher: launcher_for(env, &session.agent_kind),
            };
            let plan = plan_respawn(&session, env, &options);
            Ok((session, plan))
        })
        .collect()
}

/// Start one planned session, or report its refusal. A failure to start is a
/// [`SPAWN_FAILED`] refusal rather than an error so a batch carries on. The
/// session opens a window of its own; [`BatchStarter`] opens panes.
pub fn start_one(
    store: &Store,
    env: &Env,
    session: &Session,
    plan: Result<Plan, Refusal>,
    now: &str,
) -> Item {
    start_at(store, env, session, plan, now, Open::Window)
}

fn start_at(
    store: &Store,
    env: &Env,
    session: &Session,
    plan: Result<Plan, Refusal>,
    now: &str,
    target: Open<'_>,
) -> Item {
    match plan {
        Err(refusal) => Item::Refused(RefusedItem {
            session_id: session.id.clone(),
            code: refusal.code.as_str(),
            message: refusal.message,
            alternatives: refusal.alternatives,
        }),
        Ok(plan) => match open(store, env, &plan, now, target) {
            Ok(started) => Item::Started(Box::new(StartedItem { started, plan })),
            Err(error) => Item::Refused(RefusedItem {
                session_id: session.id.clone(),
                code: SPAWN_FAILED,
                message: error.to_string(),
                alternatives: Vec::new(),
            }),
        },
    }
}

/// The layout a window's panes were given by `select-layout` and tmux refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutFailure {
    /// The last pane of the window that opened.
    pub session_id: String,
    pub detail: String,
}

impl LayoutFailure {
    pub fn notice(&self) -> String {
        format!("could not arrange the window's panes: {}", self.detail)
    }
}

/// The window a [`BatchStarter`] is adding panes to.
struct OpenWindow {
    key: WindowKey,
    /// The pane and window opened first; `None` when that failed, so the rest
    /// open windows of their own rather than split a window that is not there.
    first: Option<SpawnedWindow>,
    /// Panes in the window so far.
    panes: u32,
    /// What the first plan recorded about the original window.
    recorded_panes: Option<u32>,
    recorded_layout: Option<String>,
    last_session: String,
}

/// Starts the plans of a batch in order: the first pane of a window opens the
/// window, the grouped plans after it split that window, and once a window's
/// last pane is open it gets a layout. Feed it every plan in the order
/// [`plan_batch`] returned, then call [`BatchStarter::finish`].
pub struct BatchStarter<'a> {
    store: &'a Store,
    env: &'a Env,
    now: String,
    window: Option<OpenWindow>,
    layout_failures: Vec<LayoutFailure>,
}

impl<'a> BatchStarter<'a> {
    pub fn new(store: &'a Store, env: &'a Env) -> Self {
        Self {
            store,
            env,
            now: env.clock.now(),
            window: None,
            layout_failures: Vec::new(),
        }
    }

    pub fn start(&mut self, session: &Session, plan: Result<Plan, Refusal>) -> Item {
        let key = plan
            .as_ref()
            .ok()
            .filter(|plan| plan.pane.as_ref().is_some_and(|pane| pane.grouped))
            .and_then(window_key);
        let continues = key.is_some() && self.window.as_ref().map(|w| &w.key) == key.as_ref();
        if plan.is_ok() && !continues {
            self.close_window();
        }
        let Some(key) = key.filter(|_| plan.is_ok()) else {
            return start_at(self.store, self.env, session, plan, &self.now, Open::Window);
        };
        let recorded = plan.as_ref().ok().and_then(|plan| plan.pane.clone());
        let first = self.window.as_ref().and_then(|window| window.first.clone());
        let target = match &first {
            Some(first) if continues => Open::Split {
                first_pane: &first.pane_id,
                window_id: &first.window_id,
            },
            _ => Open::Window,
        };
        let item = start_at(self.store, self.env, session, plan, &self.now, target);
        let started = match &item {
            Item::Started(item) => Some(item.started.clone()),
            Item::Refused(_) => None,
        };
        if continues {
            if let (Some(window), Some(_)) = (self.window.as_mut(), &started) {
                if window.first.is_some() {
                    window.panes = window.panes.saturating_add(1);
                    session.id.clone_into(&mut window.last_session);
                }
            }
        } else {
            self.window = Some(OpenWindow {
                key,
                first: started.map(|started| SpawnedWindow {
                    pane_id: started.pane_id,
                    window_id: started.window_id,
                }),
                panes: 1,
                recorded_panes: recorded.as_ref().and_then(|pane| pane.window_panes),
                recorded_layout: recorded.and_then(|pane| pane.window_layout),
                last_session: session.id.clone(),
            });
        }
        item
    }

    /// Lays out the last window and returns every layout tmux refused.
    pub fn finish(mut self) -> Vec<LayoutFailure> {
        self.close_window();
        self.layout_failures
    }

    fn close_window(&mut self) {
        let Some(window) = self.window.take() else {
            return;
        };
        let Some(first) = window.first.filter(|_| window.panes > 1) else {
            return;
        };
        let layout = if window.recorded_panes == Some(window.panes) {
            window.recorded_layout.as_deref().unwrap_or(TILED)
        } else {
            TILED
        };
        if let Err(error) = self.env.tmux.select_layout(&first.pane_id, layout) {
            self.layout_failures.push(LayoutFailure {
                session_id: window.last_session,
                detail: error.to_string(),
            });
        }
    }
}

/// The layout for a window whose recorded layout does not fit the panes that
/// came back.
const TILED: &str = "tiled";

/// Execute every plan that exists, in order. A failure to start one does not
/// stop the others; a layout tmux refuses is a notice on the window's last
/// started pane.
pub fn start_batch(store: &Store, env: &Env, planned: Planned) -> Vec<Item> {
    let mut starter = BatchStarter::new(store, env);
    let mut items: Vec<Item> = planned
        .into_iter()
        .map(|(session, plan)| starter.start(&session, plan))
        .collect();
    for failure in starter.finish() {
        for item in &mut items {
            if let Item::Started(item) = item {
                if item.started.session_id == failure.session_id {
                    item.plan.notices.push(failure.notice());
                }
            }
        }
    }
    items
}

const ENTITY_SESSION: &str = "session";

/// The words tmux runs. tmux has no environment option for a new window, so
/// extra variables go through `env`.
fn launch_argv(launch: &Launch) -> Vec<String> {
    let mut argv = Vec::new();
    if !launch.env.is_empty() {
        argv.push("env".to_owned());
        argv.extend(launch.env.iter().map(|(k, v)| format!("{k}={v}")));
    }
    argv.push(launch.program.clone());
    argv.extend(launch.args.iter().cloned());
    argv
}

fn run_setup(command: &Launch) -> Result<(), ExecuteError> {
    let shown = launch_argv(command).join(" ");
    let output = std::process::Command::new(&command.program)
        .args(&command.args)
        .envs(command.env.iter().map(|(k, v)| (k, v)))
        .current_dir(&command.cwd)
        .output()
        .map_err(|e| ExecuteError::Setup {
            command: shown.clone(),
            detail: e.to_string(),
        })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(ExecuteError::Setup {
        command: shown,
        detail: format!("{}: {}", output.status, stderr.trim()),
    })
}

const NOT_RESTORED: &str = "the previous conversation is not restored";

fn refuse(code: RefusalCode, message: String) -> Refusal {
    Refusal {
        code,
        message,
        alternatives: Vec::new(),
    }
}

/// Alive means the recorded process still exists with the recorded start
/// time; a recycled pid does not count.
fn is_running(session: &Session, env: &Env) -> bool {
    match (session.pid, session.pid_start) {
        (Some(pid), Some(start)) => env.processes.is_alive(pid, start),
        _ => false,
    }
}

fn no_launcher(session: &Session, error: &LaunchError) -> Refusal {
    refuse(
        RefusalCode::NoLauncher,
        format!("cannot launch session {}: {error}", session.id),
    )
}

fn missing_transcript(session: &Session, precondition: &Precondition) -> Refusal {
    let path = precondition.required_path.display();
    let mut refusal = refuse(
        RefusalCode::MissingTranscript,
        format!(
            "cannot resume session {}: {} but {path} is missing",
            session.id, precondition.description
        ),
    );
    refusal.alternatives.push(Alternative {
        description: format!("start a fresh session; {NOT_RESTORED}"),
        command: format!("claude-monitor respawn {} --fresh", session.id),
    });
    refusal
}

/// The target directory does not exist. Returns the `git worktree add`
/// command when the recorded worktree's branch still exists and the user
/// asked for it; otherwise the refusal, with the alternatives that apply.
/// A missing `--in` directory is never recreated.
fn recreate_or_refuse(
    session: &Session,
    target: &Session,
    env: &Env,
    options: &Options,
) -> Result<Launch, Refusal> {
    let missing = format!("directory {} does not exist", target.cwd);
    let (Some(repo), true) = (&session.repo_root, options.in_dir.is_none()) else {
        return Err(refuse(RefusalCode::MissingDirectory, missing));
    };
    let fresh_in_repo = Alternative {
        description: format!(
            "start a new session in the main checkout {repo}; the previous conversation is not restored"
        ),
        command: format!("claude-monitor respawn {} --fresh --in {repo}", session.id),
    };
    let Some(branch) = &session.branch else {
        return Err(Refusal {
            code: RefusalCode::MissingDirectory,
            message: missing,
            alternatives: vec![fresh_in_repo],
        });
    };
    if !branch_exists(env, Path::new(repo), branch) {
        return Err(Refusal {
            code: RefusalCode::BranchGone,
            message: format!("{missing} and branch `{branch}` no longer exists in {repo}"),
            alternatives: vec![fresh_in_repo],
        });
    }
    if options.recreate_worktree {
        return Ok(Launch {
            program: "git".to_owned(),
            args: ["worktree", "add", &target.cwd, branch]
                .map(str::to_owned)
                .to_vec(),
            cwd: PathBuf::from(repo),
            env: Vec::new(),
        });
    }
    Err(Refusal {
        code: RefusalCode::MissingDirectory,
        message: format!("{missing}, but branch `{branch}` still exists in {repo}"),
        alternatives: vec![
            Alternative {
                description: format!("recreate the worktree from branch `{branch}`"),
                command: format!("git -C {repo} worktree add {} {branch}", target.cwd),
            },
            Alternative {
                description: "recreate the worktree and respawn".to_owned(),
                command: format!("claude-monitor respawn {} --recreate-worktree", session.id),
            },
            fresh_in_repo,
        ],
    })
}

/// Whether `branch` is a local branch of the repository at `repo`, read from
/// `.git/refs/heads` and `.git/packed-refs`. A repository whose `.git` is not
/// a directory (itself a linked worktree) cannot be read this way; its
/// branches count as existing so that `git worktree add` reports the truth.
fn branch_exists(env: &Env, repo: &Path, branch: &str) -> bool {
    let git = repo.join(".git");
    match env.fs.metadata(&git) {
        Ok(meta) if meta.is_dir => {}
        Ok(_) => return true,
        Err(_) => return false,
    }
    if env.fs.exists(&git.join("refs/heads").join(branch)) {
        return true;
    }
    let wanted = format!("refs/heads/{branch}");
    env.fs
        .read_to_string(&git.join("packed-refs"))
        .is_ok_and(|packed| {
            packed
                .lines()
                .any(|line| line.split_whitespace().nth(1) == Some(wanted.as_str()))
        })
}

/// Why a launcher template could not be expanded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TemplateError {
    #[error("the template is empty")]
    Empty,
    #[error("the template has an unterminated {quote} quote")]
    UnterminatedQuote { quote: char },
}

/// Split a launcher template into words (single and double quotes group,
/// nothing else is special), then replace `{session_id}`, `{cwd}` and `{name}`
/// in each word. Substituting after splitting keeps a value with spaces in
/// one argument. The first word is the program.
pub fn expand_template(
    template: &str,
    session: &Session,
) -> Result<(String, Vec<String>), TemplateError> {
    let name = session_name(session);
    let mut words = split_words(template)?.into_iter().map(|word| {
        word.replace("{session_id}", &session.agent_session_id)
            .replace("{cwd}", &session.cwd)
            .replace("{name}", &name)
    });
    let program = words.next().ok_or(TemplateError::Empty)?;
    Ok((program, words.collect()))
}

/// What `{name}` stands for: the branch, else the directory's last component,
/// else the internal session id.
fn session_name(session: &Session) -> String {
    session
        .branch
        .clone()
        .or_else(|| {
            Path::new(&session.cwd)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| session.id.clone())
}

fn split_words(text: &str) -> Result<Vec<String>, TemplateError> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    for c in text.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => current.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            (None, c) => {
                current.push(c);
                in_word = true;
            }
        }
    }
    if let Some(quote) = quote {
        return Err(TemplateError::UnterminatedQuote { quote });
    }
    if in_word {
        words.push(current);
    }
    Ok(words)
}
