use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use claude_monitor::cli::{self, Cli};
use claude_monitor::env::Env;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = cli.json();
    let env = Env::real();
    let state_dir = cli.state_dir().map(PathBuf::as_path);
    let result = cli::confirm::confirm_on_terminal(cli.command(), &env, state_dir, json)
        .and_then(|command| cli::run_with(command, &env, state_dir, cli.run_options()));
    match result {
        Ok(output) => {
            print!("{}", output.render(json));
            ExitCode::SUCCESS
        }
        Err(error) => {
            // Json mode keeps stdout machine-readable, so the error goes there too.
            if json {
                print!("{}", error.render(true));
            } else {
                eprint!("{}", error.render(false));
            }
            ExitCode::from(error.exit_code())
        }
    }
}
