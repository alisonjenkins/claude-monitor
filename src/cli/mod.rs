use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

use crate::env::Env;
use crate::error::Error;
use crate::hook;
use crate::session;
use crate::store::{resolve_state_dir, StateDirEnv};

mod command;
pub mod migrate;
mod output;

pub use command::{Command, CommandInfo, ProjectAction};
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

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Update session state from an agent hook event (reads hook JSON from stdin)
    Hook {
        /// Agent kind whose payload shape is on stdin (default: claude)
        #[arg(long, value_name = "KIND")]
        agent: Option<String>,
    },
}

impl Cli {
    pub fn json(&self) -> bool {
        self.json
    }

    pub fn state_dir(&self) -> Option<&PathBuf> {
        self.state_dir.as_ref()
    }

    pub fn command(&self) -> Command {
        match &self.command {
            None => Command::Dashboard,
            Some(Commands::Hook { agent }) => Command::Hook {
                agent: agent.clone(),
            },
        }
    }
}

/// Run one command against the given providers and the default state directory.
pub fn run(cmd: Command, env: &Env) -> Result<Output, Error> {
    run_in(cmd, env, None)
}

/// Run one command; `state_dir` is the `--state-dir` override. `Hook` and
/// `Dashboard` and `Migrate` are implemented; every other command reports
/// `NotImplemented` until its task lands.
pub fn run_in(cmd: Command, env: &Env, state_dir: Option<&Path>) -> Result<Output, Error> {
    match cmd {
        Command::Hook { agent } => {
            hook::run(agent.as_deref(), state_dir, env);
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

#[cfg(feature = "tui")]
fn run_dashboard(env: &Env, state_dir: Option<&Path>) -> Result<Output, Error> {
    migrate::auto_migrate(state_dir, env);
    crate::tui::dashboard::run().map_err(|e| Error::Io {
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
    fn hook_subcommand_maps_to_hook() {
        let cli = Cli::try_parse_from(["claude-monitor", "hook"]).unwrap();
        assert_eq!(cli.command(), Command::Hook { agent: None });
    }

    #[test]
    fn hook_agent_flag_is_passed_through() {
        let cli = Cli::try_parse_from(["claude-monitor", "hook", "--agent", "copilot"]).unwrap();
        assert_eq!(
            cli.command(),
            Command::Hook {
                agent: Some("copilot".to_owned())
            }
        );
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
        let err = run(Command::Watch, &fake.env).unwrap_err();
        assert!(matches!(&err, Error::NotImplemented(name) if name == "watch"));
        assert_eq!(err.exit_code(), 1);
    }
}
