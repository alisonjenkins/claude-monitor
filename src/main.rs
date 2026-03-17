mod commands;
mod desktop_notify;
mod session;
mod tmux;
mod ui;
mod watcher;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "claude-monitor", about = "Monitor multiple Claude Code sessions in tmux")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Write a status file for a Claude session (called by hooks)
    Notify {
        /// The notification type: idle_prompt or permission_prompt
        status: String,
    },
    /// Install Claude Code notification hooks into ~/.claude/settings.json
    Setup,
    /// Remove Claude Code notification hooks from ~/.claude/settings.json
    Teardown,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None => commands::dashboard::run(),
        Some(Commands::Notify { status }) => commands::notify_cmd::run(&status),
        Some(Commands::Setup) => commands::setup::run(),
        Some(Commands::Teardown) => commands::teardown::run(),
    }
}
