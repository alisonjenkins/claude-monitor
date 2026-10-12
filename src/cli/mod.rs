use clap::{Parser, Subcommand};

use crate::hook;

#[derive(Parser)]
#[command(
    name = "claude-monitor",
    about = "Monitor multiple Claude Code sessions in tmux"
)]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Update session state from a Claude Code hook event (reads hook JSON from stdin)
    Hook,
}

pub fn run(args: Cli) -> anyhow::Result<()> {
    match args.command {
        #[cfg(feature = "tui")]
        None => crate::tui::dashboard::run(),
        #[cfg(not(feature = "tui"))]
        None => anyhow::bail!(
            "the dashboard needs the `tui` feature; rebuild with `--features tui` \
             or use a subcommand (see --help)"
        ),
        Some(Commands::Hook) => {
            hook::run();
            Ok(())
        }
    }
}
