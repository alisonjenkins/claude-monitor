use clap::Parser;
use claude_monitor::cli::{self, Cli};

fn main() -> anyhow::Result<()> {
    cli::run(Cli::parse())
}
