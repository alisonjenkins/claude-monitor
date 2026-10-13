use std::net::IpAddr;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

use crate::env::Env;
use crate::error::Error;
use crate::hook;
use crate::session;
use crate::store::markers::Marker;
use crate::store::{resolve_state_dir, StateDirEnv};

mod boot_check;
mod command;
pub mod confirm;
pub mod detail;
mod done;
mod export;
mod link;
mod list;
mod markers;
pub mod migrate;
mod output;
mod progress;
mod project;
mod reconcile;
pub(crate) mod respawn;
pub(crate) mod restore;
mod serve;
mod serve_admin;
pub(crate) mod show;
mod summarize;
mod watch;

pub use command::{Command, CommandInfo, ProjectAction, WebAction};
#[cfg(any(feature = "tui", feature = "web"))]
pub(crate) use list::age;
pub use output::Output;

#[derive(Parser)]
#[command(
    name = "claude-monitor",
    about = "Monitor multiple Claude Code sessions in tmux"
)]
pub struct Cli {
    /// Machine-readable output; errors print as {"ok":false,"error":"..."}
    #[arg(long, global = true)]
    json: bool,

    /// Override the state directory (default: the XDG state dir)
    #[arg(long, global = true, value_name = "PATH")]
    state_dir: Option<PathBuf>,

    /// Skip the refresh pass that list, show and the dashboard run first
    /// (finding Claude and Copilot sessions on disk, reading Claude's live
    /// session registry, marking dead sessions interrupted); show the store as is
    #[arg(long, global = true)]
    no_refresh: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// List sessions grouped by view section
    List {
        /// Only sessions in this state; repeat for several
        #[arg(long = "state", value_name = "S")]
        states: Vec<String>,
        /// Only sessions of this agent kind (for example claude)
        #[arg(long, value_name = "KIND")]
        agent: Option<String>,
        /// Only sessions in this repository (name or path)
        #[arg(long, value_name = "REPO")]
        repo: Option<String>,
        /// Only sessions in this project; `-` for sessions in no project
        #[arg(long, value_name = "PROJECT")]
        project: Option<String>,
        /// Only sessions whose task or directory contains this text
        #[arg(long, value_name = "TEXT")]
        text: Option<String>,
        /// Only one section: active, interrupted, finished, done or dismissed
        #[arg(long, value_name = "SECTION")]
        section: Option<String>,
        /// Include dismissed sessions
        #[arg(long)]
        include_dismissed: bool,
        /// Accepted for compatibility: finished sessions are never collapsed
        #[arg(long)]
        expand_finished: bool,
        /// List the unseen sessions (found on disk, never reported by a hook)
        #[arg(long)]
        expand_unseen: bool,
    },
    /// Show one session in full
    Show {
        /// Session id, or the start of one
        session: String,
    },
    /// Update session state from an agent hook event (reads hook JSON from stdin)
    Hook {
        /// Agent kind whose payload shape is on stdin (default: claude)
        #[arg(long, value_name = "KIND")]
        agent: Option<String>,
        /// Event name to use when the payload carries none (Copilot's native
        /// payloads do not), for example sessionStart or preToolUse
        #[arg(long, value_name = "NAME")]
        event: Option<String>,
    },
    /// Mark sessions whose process, tmux pane or boot is gone as interrupted
    Reconcile {
        /// Only the sessions in this tmux pane (for example %3)
        #[arg(long, value_name = "%N", conflicts_with = "session")]
        pane: Option<String>,
        /// Only this session
        #[arg(long, value_name = "ID")]
        session: Option<String>,
    },
    /// Log one line per session state change until interrupted (SIGINT or SIGTERM)
    Watch,
    /// Start interrupted sessions again in tmux; without --yes, only print the plan
    Restore {
        /// Restore every interrupted session (required with --yes)
        #[arg(long)]
        all_interrupted: bool,
        /// Also restore finished sessions
        #[arg(long)]
        include_finished: bool,
        /// Start new sessions instead of resuming the old conversations
        #[arg(long)]
        fresh: bool,
        /// Leave this session out (id or start of one); repeat for several
        #[arg(long, value_name = "ID")]
        skip: Vec<String>,
        /// Start the sessions instead of printing the plan
        #[arg(long)]
        yes: bool,
    },
    /// Start named sessions again in tmux, resuming their conversations
    Respawn {
        /// Session ids, or the start of each
        #[arg(required = true)]
        sessions: Vec<String>,
        /// Start a new session instead of resuming the conversation
        #[arg(long)]
        fresh: bool,
        /// With --fresh: start in this directory instead of the recorded one
        #[arg(long = "in", value_name = "DIR")]
        in_dir: Option<PathBuf>,
        /// Run `git worktree add` for a missing worktree whose branch still exists
        #[arg(long)]
        recreate_worktree: bool,
        /// Accepted so scripts can pass it to respawn and restore alike; a
        /// respawn names its sessions and never asks
        #[arg(long)]
        yes: bool,
    },
    /// Count sessions a reboot or crash interrupted, notify once and start nothing
    BootCheck {
        /// Show the pending notice as a tmux message once, then clear it
        #[arg(long)]
        banner: bool,
    },
    /// Ask a model on this machine for a short summary of one session or project; never automatic
    Summarize {
        /// Session id (or the start of one), or a project name
        target: String,
    },
    /// Serve the web view on loopback; prints the URL that opens it
    Serve {
        /// Port to listen on (0 picks a free one)
        #[arg(long, value_name = "PORT")]
        port: Option<u16>,
        /// Loopback address to listen on (127.0.0.1 or ::1); nothing else is accepted
        #[arg(long, value_name = "ADDR", value_parser = serve::loopback_addr)]
        bind: Option<IpAddr>,
    },
    /// Set up and reach the web view's TLS and login
    Web {
        #[command(subcommand)]
        action: WebCommands,
    },
    /// Show a session's plan progress and the file it comes from
    #[command(args_conflicts_with_subcommands = true, arg_required_else_help = true)]
    Progress {
        /// Session id, or the start of one
        session: Option<String>,
        #[command(subcommand)]
        action: Option<ProgressAction>,
    },
    /// Create, rename or set the status of a project
    Project {
        #[command(subcommand)]
        action: ProjectCommands,
    },
    /// Attach a note to a session or project
    Note {
        /// Session id (or the start of one), or a project name
        target: String,
        /// The note; several words are joined with spaces
        #[arg(required = true)]
        text: Vec<String>,
    },
    /// Flag a session or project as parked; it stays listed
    Park {
        /// Session id (or the start of one), or a project name
        target: String,
    },
    /// Clear the parked flag
    Unpark {
        /// Session id (or the start of one), or a project name
        target: String,
    },
    /// Hide a session or project from the default views; nothing is deleted
    Dismiss {
        /// Session id (or the start of one), or a project name
        target: String,
        /// Do not ask for confirmation
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Show a dismissed session or project again
    Undismiss {
        /// Session id (or the start of one), or a project name
        target: String,
    },
    /// Mark finished sessions done: they move to their project's Done section
    Done {
        /// Session ids, or the start of each
        #[arg(required = true)]
        sessions: Vec<String>,
    },
    /// Return done sessions to the finished group
    Undone {
        /// Session ids, or the start of each
        #[arg(required = true)]
        sessions: Vec<String>,
    },
    /// Put sessions in a project (created if new); `-` as the project removes them from theirs
    Group {
        /// Project name, or `-` for no project
        project: String,
        /// Session ids, or the start of each
        #[arg(required = true)]
        sessions: Vec<String>,
    },
    /// Record that a session or project corresponds to a node in the external graph
    Link {
        /// Session id (or the start of one), or a project name
        owner: String,
        /// The node's id: a decimal number
        node_id: String,
        /// What the node is
        #[arg(long, default_value = "task", value_parser = ["project", "task", "memory"])]
        kind: String,
        /// The node's shard (document) id: a decimal number
        #[arg(long)]
        doc: Option<String>,
    },
    /// Remove a link made by `link`
    Unlink {
        /// Session id (or the start of one), or a project name
        owner: String,
        /// The node's id
        node_id: String,
        /// Do not ask for confirmation
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Work with links to the external graph
    Links {
        #[command(subcommand)]
        action: LinksAction,
    },
    /// Write projects, steps, notes and sessions for another tool to import
    Export {
        #[command(subcommand)]
        target: ExportTarget,
    },
}

#[derive(Subcommand)]
enum LinksAction {
    /// Show linked records whose two sides disagree; reads both sides and writes neither
    Check,
}

#[derive(Subcommand)]
enum ProgressAction {
    /// Choose the plan a session's progress comes from; with neither a path
    /// nor a flag, show the candidates
    Bind {
        /// Session id, or the start of one
        session: String,
        /// Plan file: a spec-kit tasks.md or a Markdown checklist
        #[arg(conflicts_with_all = ["none", "source"])]
        path: Option<PathBuf>,
        /// Show no progress for this session
        #[arg(long, conflicts_with = "source")]
        none: bool,
        /// Source other than a file: agent_todo or none
        #[arg(long, value_name = "SOURCE")]
        source: Option<String>,
    },
}

#[derive(Subcommand)]
enum WebCommands {
    /// Show where the CA certificate is and how to trust it once in a browser
    Ca {
        /// Write the certificate (PEM) to stdout instead
        #[arg(long)]
        print: bool,
    },
    /// Print a fresh single-use login URL from the running `serve`
    Login,
}

#[derive(Subcommand)]
enum ProjectCommands {
    /// Create an active project
    Add { name: String },
    /// Give a project another name
    Rename { name: String, new_name: String },
    /// Set a project's status: active, parked, done or archived
    SetStatus { name: String, status: String },
}

#[derive(Subcommand)]
enum ExportTarget {
    /// JSON Lines in cortexiphan's vocabulary; conversation text is left out
    Cortexiphan {
        /// Only this project (name or id)
        #[arg(long, value_name = "P")]
        project: Option<String>,
        /// Also export prompts, last assistant messages and model-written summaries
        #[arg(long)]
        include_text: bool,
        /// Write to this file instead of stdout
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
    },
}

impl Cli {
    pub fn json(&self) -> bool {
        self.json
    }

    pub fn state_dir(&self) -> Option<&PathBuf> {
        self.state_dir.as_ref()
    }

    pub fn run_options(&self) -> RunOptions {
        RunOptions {
            refresh: !self.no_refresh,
        }
    }

    pub fn command(&self) -> Command {
        match &self.command {
            None => Command::Dashboard,
            Some(Commands::Hook { agent, event }) => Command::Hook {
                agent: agent.clone(),
                event: event.clone(),
            },
            Some(Commands::List {
                states,
                agent,
                repo,
                project,
                text,
                section,
                include_dismissed,
                expand_finished,
                expand_unseen,
            }) => Command::List {
                states: states.clone(),
                agent: agent.clone(),
                repo: repo.clone(),
                project: project.clone(),
                text: text.clone(),
                section: section.clone(),
                include_dismissed: *include_dismissed,
                expand_finished: *expand_finished,
                expand_unseen: *expand_unseen,
            },
            Some(Commands::Show { session }) => Command::Show {
                session: session.clone(),
            },
            Some(Commands::Reconcile { pane, session }) => Command::Reconcile {
                pane: pane.clone(),
                session: session.clone(),
            },
            Some(Commands::Watch) => Command::Watch,
            Some(Commands::Restore {
                all_interrupted,
                include_finished,
                fresh,
                skip,
                yes,
            }) => Command::Restore {
                all_interrupted: *all_interrupted,
                include_finished: *include_finished,
                fresh: *fresh,
                skip: skip.clone(),
                yes: *yes,
            },
            Some(Commands::Respawn {
                sessions,
                fresh,
                in_dir,
                recreate_worktree,
                yes: _,
            }) => Command::Respawn {
                sessions: sessions.clone(),
                fresh: *fresh,
                in_dir: in_dir.clone(),
                recreate_worktree: *recreate_worktree,
            },
            Some(Commands::BootCheck { banner }) => Command::BootCheck { banner: *banner },
            Some(Commands::Summarize { target }) => Command::Summarize {
                target: target.clone(),
            },
            Some(Commands::Progress {
                action:
                    Some(ProgressAction::Bind {
                        session,
                        path,
                        none,
                        source,
                    }),
                ..
            }) => Command::ProgressBind {
                owner: session.clone(),
                file: path.clone(),
                source: if *none {
                    Some("none".to_owned())
                } else {
                    source.clone()
                },
            },
            Some(Commands::Progress {
                session,
                action: None,
            }) => Command::ProgressBind {
                owner: session.clone().unwrap_or_default(),
                file: None,
                source: None,
            },
            Some(Commands::Project { action }) => match action {
                ProjectCommands::Add { name } => Command::Project {
                    action: ProjectAction::Add,
                    name: name.clone(),
                    value: None,
                },
                ProjectCommands::Rename { name, new_name } => Command::Project {
                    action: ProjectAction::Rename,
                    name: name.clone(),
                    value: Some(new_name.clone()),
                },
                ProjectCommands::SetStatus { name, status } => Command::Project {
                    action: ProjectAction::SetStatus,
                    name: name.clone(),
                    value: Some(status.clone()),
                },
            },
            Some(Commands::Group { project, sessions }) => Command::Group {
                project: project.clone(),
                sessions: sessions.clone(),
            },
            Some(Commands::Done { sessions }) => Command::Done {
                sessions: sessions.clone(),
            },
            Some(Commands::Undone { sessions }) => Command::Undone {
                sessions: sessions.clone(),
            },
            Some(Commands::Note { target, text }) => Command::Note {
                target: target.clone(),
                text: text.join(" "),
            },
            Some(Commands::Park { target }) => Command::Park {
                target: target.clone(),
            },
            Some(Commands::Unpark { target }) => Command::Unpark {
                target: target.clone(),
            },
            Some(Commands::Dismiss { target, yes }) => Command::Dismiss {
                target: target.clone(),
                yes: *yes,
            },
            Some(Commands::Undismiss { target }) => Command::Undismiss {
                target: target.clone(),
            },
            Some(Commands::Link {
                owner,
                node_id,
                kind,
                doc,
            }) => Command::Link {
                owner: owner.clone(),
                cortex: node_id.clone(),
                kind: kind.clone(),
                doc: doc.clone(),
            },
            Some(Commands::Unlink {
                owner,
                node_id,
                yes,
            }) => Command::Unlink {
                owner: owner.clone(),
                cortex: node_id.clone(),
                yes: *yes,
            },
            Some(Commands::Links {
                action: LinksAction::Check,
            }) => Command::LinksCheck,
            Some(Commands::Export {
                target:
                    ExportTarget::Cortexiphan {
                        project,
                        include_text,
                        out,
                    },
            }) => Command::ExportCortexiphan {
                project: project.clone(),
                include_text: *include_text,
                out: out.clone(),
            },
            Some(Commands::Serve { port, bind }) => Command::Serve {
                port: *port,
                bind: *bind,
            },
            Some(Commands::Web { action }) => Command::Web {
                action: match action {
                    WebCommands::Ca { print } => WebAction::Ca { print: *print },
                    WebCommands::Login => WebAction::Login,
                },
            },
        }
    }
}

/// Run one command against the given providers and the default state directory.
pub fn run(cmd: Command, env: &Env) -> Result<Output, Error> {
    run_in(cmd, env, None)
}

/// How a command runs beyond what the command itself says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunOptions {
    /// Run a [`crate::refresh::refresh`] pass before `list`, `show` and the
    /// dashboard read the store.
    pub refresh: bool,
}

/// Run one command; `state_dir` is the `--state-dir` override. `Hook`,
/// `Dashboard`, `Migrate`, `List`, `Show`, `Reconcile`, `Watch`, `BootCheck`,
/// `Restore`, `Respawn`, `Summarize`, `ProgressBind`, `Project`, `Group`, `Note`,
/// `Park`, `Unpark`, `Dismiss`, `Undismiss`, `Done`, `Undone`, `ExportCortexiphan`
/// and `Serve` are implemented; every other
/// command reports `NotImplemented` until its task lands. Reads the store as it
/// is; [`run_with`] adds the refresh pass the binary runs.
pub fn run_in(cmd: Command, env: &Env, state_dir: Option<&Path>) -> Result<Output, Error> {
    run_with(cmd, env, state_dir, RunOptions { refresh: false })
}

pub fn run_with(
    cmd: Command,
    env: &Env,
    state_dir: Option<&Path>,
    options: RunOptions,
) -> Result<Output, Error> {
    let cmd = confirm::confirm_command(cmd, env, state_dir, None)?;
    if options.refresh && reads_sessions(&cmd) {
        refresh_first(env, state_dir)?;
    }
    match cmd {
        Command::List {
            states,
            agent,
            repo,
            project,
            text,
            section,
            include_dismissed,
            expand_finished,
            expand_unseen,
        } => list::run(
            env,
            state_dir,
            list::ListArgs {
                states,
                agent,
                repo,
                project,
                text,
                section,
                include_dismissed,
                expand_finished,
                expand_unseen,
            },
        ),
        Command::Show { session } => show::run(env, state_dir, &session),
        Command::Reconcile { pane, session } => reconcile::run(env, state_dir, pane, session),
        Command::Watch => watch::run(env, state_dir),
        Command::Restore {
            all_interrupted,
            include_finished,
            fresh,
            skip,
            yes,
        } => restore::run(
            env,
            state_dir,
            restore::RestoreArgs {
                all_interrupted,
                include_finished,
                fresh,
                skip,
                yes,
            },
        ),
        Command::Respawn {
            sessions,
            fresh,
            in_dir,
            recreate_worktree,
        } => respawn::run(
            env,
            state_dir,
            &sessions,
            respawn::PlanFlags {
                fresh,
                in_dir,
                recreate_worktree,
            },
        ),
        Command::BootCheck { banner } => boot_check::run(env, state_dir, banner),
        Command::Summarize { target } => summarize::run(env, state_dir, &target),
        Command::ProgressBind {
            owner,
            file,
            source,
        } => progress::run(env, state_dir, &owner, file, source),
        Command::Project {
            action,
            name,
            value,
        } => project::run_project(env, state_dir, &action, &name, value.as_deref()),
        Command::Group { project, sessions } => {
            project::run_group(env, state_dir, &project, &sessions)
        }
        Command::Done { sessions } => done::run(env, state_dir, &sessions, true),
        Command::Undone { sessions } => done::run(env, state_dir, &sessions, false),
        Command::Note { target, text } => markers::run_note(env, state_dir, &target, &text),
        Command::Park { target } => {
            markers::run_marker(env, state_dir, Marker::Parked, true, &target)
        }
        Command::Unpark { target } => {
            markers::run_marker(env, state_dir, Marker::Parked, false, &target)
        }
        Command::Dismiss { target, yes: _ } => {
            markers::run_marker(env, state_dir, Marker::Dismissed, true, &target)
        }
        Command::Undismiss { target } => {
            markers::run_marker(env, state_dir, Marker::Dismissed, false, &target)
        }
        Command::ExportCortexiphan {
            project,
            include_text,
            out,
        } => export::run(
            env,
            state_dir,
            export::ExportArgs {
                project,
                include_text,
                out,
            },
        ),
        Command::Link {
            owner,
            cortex,
            kind,
            doc,
        } => link::run_link(env, state_dir, &owner, &cortex, &kind, doc.as_deref()),
        Command::Unlink {
            owner,
            cortex,
            yes: _,
        } => link::run_unlink(env, state_dir, &owner, &cortex),
        Command::LinksCheck => link::run_links_check(env, state_dir),
        Command::Serve { port, bind } => serve::run(env, state_dir, port, bind),
        Command::Web { action } => serve_admin::run(state_dir, action),
        Command::Hook { agent, event } => {
            hook::run(agent.as_deref(), event.as_deref(), state_dir, env);
            Ok(Output::empty())
        }
        Command::Dashboard => run_dashboard(env, state_dir),
        Command::Migrate => {
            let dir = resolve_state_dir(state_dir, &StateDirEnv::from_process())
                .map_err(|e| migrate::store_error(&e))?;
            migrate::run(&dir, &session::status_dir(), env)
        }
        other => Err(Error::NotImplemented(other.name().to_owned())),
    }
}

fn reads_sessions(cmd: &Command) -> bool {
    matches!(
        cmd,
        Command::List { .. }
            | Command::Show { .. }
            | Command::Dashboard
            | Command::ProgressBind { .. }
            | Command::Restore { .. }
            | Command::Respawn { .. }
            | Command::Group { .. }
            | Command::Done { .. }
            | Command::Undone { .. }
            | Command::Note { .. }
            | Command::Park { .. }
            | Command::Unpark { .. }
            | Command::Dismiss { .. }
            | Command::Undismiss { .. }
            | Command::Link { .. }
            | Command::Unlink { .. }
    )
}

/// Brings the store up to date with what is on disk. Warnings go to stderr so
/// stdout stays the command's own output.
fn refresh_first(env: &Env, state_dir: Option<&Path>) -> Result<(), Error> {
    let store = migrate::open_viewer_store(env, state_dir)?;
    let report = crate::refresh::refresh(&store, env, &env.clock.now());
    for warning in &report.warnings {
        eprintln!("claude-monitor: refresh: {warning}");
    }
    Ok(())
}

#[cfg(feature = "tui")]
fn run_dashboard(env: &Env, state_dir: Option<&Path>) -> Result<Output, Error> {
    crate::tui::dashboard::run(env, state_dir).map_err(|e| Error::Io {
        operation: "run dashboard".to_owned(),
        path: None,
        source: std::io::Error::other(format!("{e:#}")),
    })?;
    Ok(Output::empty())
}

#[cfg(not(feature = "tui"))]
fn run_dashboard(_env: &Env, _state_dir: Option<&Path>) -> Result<Output, Error> {
    Err(Error::Usage(
        "the dashboard needs the `tui` feature; rebuild with `--features tui` \
         or use a subcommand (see --help)"
            .to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::testing::FakeEnv;

    #[test]
    fn no_subcommand_is_the_dashboard() {
        let cli = Cli::try_parse_from(["claude-monitor"]).unwrap();
        assert_eq!(cli.command(), Command::Dashboard);
        assert!(!cli.json());
        assert!(cli.state_dir().is_none());
    }

    #[test]
    fn refresh_is_on_unless_no_refresh_is_given() {
        let cli = Cli::try_parse_from(["claude-monitor", "list"]).unwrap();
        assert!(cli.run_options().refresh);
        let cli = Cli::try_parse_from(["claude-monitor", "list", "--no-refresh"]).unwrap();
        assert!(!cli.run_options().refresh);
        let cli = Cli::try_parse_from(["claude-monitor", "--no-refresh"]).unwrap();
        assert!(!cli.run_options().refresh);
    }

    #[test]
    fn watch_subcommand_maps_to_watch() {
        let cli = Cli::try_parse_from(["claude-monitor", "watch"]).unwrap();
        assert_eq!(cli.command(), Command::Watch);
    }

    #[test]
    fn boot_check_subcommand_maps_to_boot_check() {
        let cli = Cli::try_parse_from(["claude-monitor", "boot-check"]).unwrap();
        assert_eq!(cli.command(), Command::BootCheck { banner: false });
        let cli = Cli::try_parse_from(["claude-monitor", "boot-check", "--banner"]).unwrap();
        assert_eq!(cli.command(), Command::BootCheck { banner: true });
    }

    #[test]
    fn restore_flags_map_to_the_restore_command() {
        let cli = Cli::try_parse_from(["claude-monitor", "restore"]).unwrap();
        assert_eq!(
            cli.command(),
            Command::Restore {
                all_interrupted: false,
                include_finished: false,
                fresh: false,
                skip: vec![],
                yes: false,
            }
        );
        let cli = Cli::try_parse_from([
            "claude-monitor",
            "restore",
            "--all-interrupted",
            "--include-finished",
            "--fresh",
            "--skip",
            "a",
            "--skip",
            "b",
            "--yes",
        ])
        .unwrap();
        assert_eq!(
            cli.command(),
            Command::Restore {
                all_interrupted: true,
                include_finished: true,
                fresh: true,
                skip: vec!["a".to_owned(), "b".to_owned()],
                yes: true,
            }
        );
    }

    #[test]
    fn respawn_takes_sessions_and_flags() {
        assert!(Cli::try_parse_from(["claude-monitor", "respawn"]).is_err());
        let cli = Cli::try_parse_from([
            "claude-monitor",
            "respawn",
            "a",
            "b",
            "--fresh",
            "--in",
            "/repo",
            "--recreate-worktree",
            "--yes",
        ])
        .unwrap();
        assert_eq!(
            cli.command(),
            Command::Respawn {
                sessions: vec!["a".to_owned(), "b".to_owned()],
                fresh: true,
                in_dir: Some(PathBuf::from("/repo")),
                recreate_worktree: true,
            }
        );
    }

    #[test]
    fn serve_flags_map_to_the_serve_command() {
        let cli = Cli::try_parse_from(["claude-monitor", "serve"]).unwrap();
        assert_eq!(
            cli.command(),
            Command::Serve {
                port: None,
                bind: None
            }
        );
        let cli =
            Cli::try_parse_from(["claude-monitor", "serve", "--port", "9000", "--bind", "::1"])
                .unwrap();
        assert_eq!(
            cli.command(),
            Command::Serve {
                port: Some(9000),
                bind: Some("::1".parse().unwrap())
            }
        );
    }

    #[test]
    fn serve_refuses_a_non_loopback_bind_when_parsing() {
        for bind in ["0.0.0.0", "::", "192.168.1.10", "example.com"] {
            let parsed = Cli::try_parse_from(["claude-monitor", "serve", "--bind", bind]);
            assert!(parsed.is_err(), "{bind} must be refused");
        }
        let loopback = Cli::try_parse_from(["claude-monitor", "serve", "--bind", "127.0.0.1"]);
        assert!(loopback.is_ok());
    }

    #[test]
    fn hook_subcommand_maps_to_hook() {
        let cli = Cli::try_parse_from(["claude-monitor", "hook"]).unwrap();
        assert_eq!(
            cli.command(),
            Command::Hook {
                agent: None,
                event: None
            }
        );
    }

    #[test]
    fn hook_agent_flag_is_passed_through() {
        let cli = Cli::try_parse_from(["claude-monitor", "hook", "--agent", "copilot"]).unwrap();
        assert_eq!(
            cli.command(),
            Command::Hook {
                agent: Some("copilot".to_owned()),
                event: None
            }
        );
    }

    #[test]
    fn hook_event_flag_is_passed_through() {
        let cli = Cli::try_parse_from([
            "claude-monitor",
            "hook",
            "--agent",
            "copilot",
            "--event",
            "preToolUse",
        ])
        .unwrap();
        assert_eq!(
            cli.command(),
            Command::Hook {
                agent: Some("copilot".to_owned()),
                event: Some("preToolUse".to_owned())
            }
        );
    }

    #[test]
    fn list_flags_map_to_the_list_command() {
        let cli = Cli::try_parse_from([
            "claude-monitor",
            "list",
            "--state",
            "working",
            "--state",
            "idle",
            "--agent",
            "claude",
            "--repo",
            "nix-config",
            "--project",
            "-",
            "--text",
            "db",
            "--section",
            "finished",
            "--include-dismissed",
            "--expand-finished",
            "--expand-unseen",
        ])
        .unwrap();
        assert_eq!(
            cli.command(),
            Command::List {
                states: vec!["working".to_owned(), "idle".to_owned()],
                agent: Some("claude".to_owned()),
                repo: Some("nix-config".to_owned()),
                project: Some("-".to_owned()),
                text: Some("db".to_owned()),
                section: Some("finished".to_owned()),
                include_dismissed: true,
                expand_finished: true,
                expand_unseen: true,
            }
        );
    }

    #[test]
    fn show_takes_a_session() {
        let cli = Cli::try_parse_from(["claude-monitor", "show", "abc"]).unwrap();
        assert_eq!(
            cli.command(),
            Command::Show {
                session: "abc".to_owned()
            }
        );
    }

    #[test]
    fn reconcile_takes_a_pane_or_a_session_but_not_both() {
        let cli = Cli::try_parse_from(["claude-monitor", "reconcile"]).unwrap();
        assert_eq!(
            cli.command(),
            Command::Reconcile {
                pane: None,
                session: None
            }
        );
        let cli = Cli::try_parse_from(["claude-monitor", "reconcile", "--pane", "%3"]).unwrap();
        assert_eq!(
            cli.command(),
            Command::Reconcile {
                pane: Some("%3".to_owned()),
                session: None
            }
        );
        let cli = Cli::try_parse_from(["claude-monitor", "reconcile", "--session", "abc"]).unwrap();
        assert_eq!(
            cli.command(),
            Command::Reconcile {
                pane: None,
                session: Some("abc".to_owned())
            }
        );
        assert!(Cli::try_parse_from([
            "claude-monitor",
            "reconcile",
            "--pane",
            "%3",
            "--session",
            "abc"
        ])
        .is_err());
    }

    #[test]
    fn global_flags_parse_before_or_after_the_subcommand() {
        let cli =
            Cli::try_parse_from(["claude-monitor", "--json", "--state-dir", "/s", "hook"]).unwrap();
        assert!(cli.json());
        assert_eq!(cli.state_dir(), Some(&PathBuf::from("/s")));
        let cli = Cli::try_parse_from(["claude-monitor", "hook", "--json"]).unwrap();
        assert!(cli.json());
    }

    #[test]
    fn unimplemented_commands_report_not_implemented_with_exit_1() {
        let fake = FakeEnv::new();
        let delete = Command::Delete {
            session: "s".to_owned(),
            yes: true,
        };
        let err = run(delete, &fake.env).unwrap_err();
        assert!(matches!(&err, Error::NotImplemented(name) if name == "delete"));
        assert_eq!(err.exit_code(), 1);
    }
}
