use std::path::PathBuf;
use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::Parser;
use claude_monitor::cli::{self, Cli};
use claude_monitor::env::Env;
use claude_monitor::error::{Error, EXIT_USAGE};

/// Print a clap parse failure and pick the exit code. clap's own exit 2 would
/// read as "block" to Claude Code's hook runner and as "refused" to scripts.
fn report_parse_error(error: &clap::Error, json: bool) -> ExitCode {
    match error.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
            print!("{error}");
            ExitCode::SUCCESS
        }
        _ if json => {
            let usage = Error::Usage(error.render().to_string().trim_end().to_owned());
            print!("{}", usage.render(true));
            ExitCode::from(EXIT_USAGE)
        }
        _ => {
            eprint!("{error}");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            // The parse failed, so `Cli::json()` is unavailable; look at the raw args.
            let json = std::env::args_os().any(|arg| arg == "--json");
            return report_parse_error(&error, json);
        }
    };
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
