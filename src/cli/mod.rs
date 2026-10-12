use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

use crate::env::Env;
use crate::error::Error;
use crate::hook;
use crate::session;
use crate::store::{resolve_state_dir, StateDirEnv};

mod command;
mod list;
pub mod migrate;
mod output;
mod reconcile;
mod show;
mod watch;

pub use command::{Command, CommandInfo, ProjectAction};
#[cfg(feature = "tui")]
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
/// `Dashboard`, `Migrate`, `List`, `Show`, `Reconcile` and `Watch` are implemented; every other
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
        Command::List { .. } | Command::Show { .. } | Command::Dashboard
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
    #[ignore = "implemented by T075"]
    fn boot_check_subcommand_maps_to_boot_check() {
        let cli = Cli::try_parse_from(["claude-monitor", "boot-check"]).unwrap();
        assert_eq!(cli.command(), Command::BootCheck { banner: false });
        let cli = Cli::try_parse_from(["claude-monitor", "boot-check", "--banner"]).unwrap();
        assert_eq!(cli.command(), Command::BootCheck { banner: true });
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
        let err = run(Command::LinksCheck, &fake.env).unwrap_err();
        assert!(matches!(&err, Error::NotImplemented(name) if name == "links-check"));
        assert_eq!(err.exit_code(), 1);
    }
}
