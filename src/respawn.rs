//! Respawn planning: turn a stored session into "start this command in this
//! directory", or into a refusal that names the problem and the ways out.
//!
//! The planner is pure over the [`Env`] providers: it reads the filesystem,
//! process table and tmux availability and returns a [`Plan`] or [`Refusal`].
//! It never creates a directory, runs git or starts a process; the executor
//! does that, and only for what the plan lists.

use std::path::{Path, PathBuf};

use crate::agents::{AgentKind, Launch, LaunchError, LaunchMode, LauncherConfig, Precondition};
use crate::env::Env;
use crate::model::Session;

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
