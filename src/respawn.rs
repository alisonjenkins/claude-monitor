//! Respawn planning: turn a stored session into "start this command in this
//! directory", or into a refusal that names the problem and the ways out.
//!
//! The planner is pure over the [`Env`] providers: it reads the filesystem,
//! process table and tmux availability and returns a [`Plan`] or [`Refusal`].
//! It never creates a directory, runs git or starts a process; the executor
//! does that, and only for what the plan lists.

use std::path::{Path, PathBuf};

use crate::agents::{AgentKind, Launch, LaunchError, LaunchMode, LauncherConfig, Precondition};
use crate::env::{Env, EnvError, Vars};
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
    /// The tmux window name: the branch, else the directory name.
    pub label: String,
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
        label: session_name(&target),
    })
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

/// The effective tmux session: the environment variable, else `config.json`,
/// else [`DEFAULT_TMUX_SESSION`].
pub fn tmux_session_for(env: &Env) -> String {
    env.vars
        .var(TMUX_SESSION_VAR)
        .unwrap_or_else(|| crate::config::tmux_session(env))
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

/// Carry out `plan` and nothing else: ensure `tmux_session`, run the plan's
/// setup commands, open one window for the launch, record the respawn request
/// for its pane and append `session.respawn_requested`. Never starts any
/// session the plan does not name.
///
/// Setup runs before tmux is touched: a failed `git worktree add` leaves no
/// window and no tmux server behind, and the new window's directory exists.
pub fn execute(store: &Store, env: &Env, plan: &Plan, tmux_session: &str, now: &str) -> Outcome {
    for command in &plan.setup {
        run_setup(command)?;
    }
    env.tmux.ensure_session(tmux_session, &plan.launch.cwd)?;
    let window = env.tmux.spawn_window(
        tmux_session,
        &plan.label,
        &plan.launch.cwd,
        &launch_argv(&plan.launch),
    )?;

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
