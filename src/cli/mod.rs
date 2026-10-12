use clap::{Parser, Subcommand};

use crate::{hook, tui};

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
        None => tui::dashboard::run(),
        Some(Commands::Hook) => {
            hook::run();
            Ok(())
        }
    }
}
