mod commands;
mod desktop_notify;
mod session;
mod tmux;
mod ui;
mod watcher;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "claude-monitor",
    about = "Monitor multiple Claude Code sessions in tmux"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Update session state from a Claude Code hook event (reads hook JSON from stdin)
    Hook,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None => commands::dashboard::run(),
        Some(Commands::Hook) => {
            commands::hook_cmd::run();
            Ok(())
        }
    }
}
