//! Asks before a destructive or hard-to-notice command runs: `dismiss` and
//! `unlink` without `--yes`.

use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;

use super::{link, markers, Command};
use crate::env::Env;
use crate::error::Error;

/// Where a question is asked and answered.
pub struct Terminal<'a> {
    pub input: &'a mut dyn BufRead,
    pub output: &'a mut dyn Write,
}

/// What a command that needs confirmation will do: `action` names the target
/// ("Dismiss session s1"), `consequence` says what follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub action: String,
    pub consequence: String,
}

impl std::fmt::Display for Question {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.action, self.consequence)
    }
}

/// The question to ask before running `command`, or `None` when it needs no
/// confirmation (it carries `yes`, or is not a destructive command). Fails the
/// way the command would on an unknown target or a bad node id.
pub fn question_for(
    command: &Command,
    env: &Env,
    state_dir: Option<&Path>,
) -> Result<Option<Question>, Error> {
    match command {
        Command::Dismiss { target, yes: false } => {
            markers::dismiss_question(env, state_dir, target).map(Some)
        }
        Command::Unlink {
            owner,
            cortex,
            yes: false,
        } => link::unlink_question(env, state_dir, owner, cortex).map(Some),
        _ => Ok(None),
    }
}

fn with_yes(command: Command) -> Command {
    match command {
        Command::Dismiss { target, .. } => Command::Dismiss { target, yes: true },
        Command::Unlink { owner, cortex, .. } => Command::Unlink {
            owner,
            cortex,
            yes: true,
        },
        other => other,
    }
}

/// Returns `command` ready to run: unchanged when it needs no confirmation,
/// with `yes` set once the user has agreed. Without a terminal, or when the
/// answer is not `y`, it fails and nothing is changed.
pub fn confirm_command(
    command: Command,
    env: &Env,
    state_dir: Option<&Path>,
    terminal: Option<Terminal<'_>>,
) -> Result<Command, Error> {
    let Some(question) = question_for(&command, env, state_dir)? else {
        return Ok(command);
    };
    let confirmed = with_yes(command);
    let Some(terminal) = terminal else {
        return Err(Error::Usage(format!(
            "{question}; nothing was changed. Pass --yes to proceed without being asked"
        )));
    };
    if ask(&question, terminal)? {
        Ok(confirmed)
    } else {
        Err(Error::Usage(format!(
            "not confirmed; nothing was changed ({question})"
        )))
    }
}

/// [`confirm_command`] on the process's own terminal. A prompt is only shown
/// when stdin and stderr are both terminals and the output is not JSON;
/// otherwise the command must carry `--yes`.
pub fn confirm_on_terminal(
    command: Command,
    env: &Env,
    state_dir: Option<&Path>,
    json: bool,
) -> Result<Command, Error> {
    let stdin = std::io::stdin();
    let mut stderr = std::io::stderr();
    let interactive = !json && stdin.is_terminal() && stderr.is_terminal();
    if !interactive {
        return confirm_command(command, env, state_dir, None);
    }
    let mut input = stdin.lock();
    confirm_command(
        command,
        env,
        state_dir,
        Some(Terminal {
            input: &mut input,
            output: &mut stderr,
        }),
    )
}

fn ask(question: &Question, terminal: Terminal<'_>) -> Result<bool, Error> {
    let io_error = |operation: &str, source| Error::Io {
        operation: operation.to_owned(),
        path: None,
        source,
    };
    write!(terminal.output, "{question}. Proceed? [y/N] ")
        .and_then(|()| terminal.output.flush())
        .map_err(|e| io_error("write confirmation prompt", e))?;
    let mut answer = String::new();
    terminal
        .input
        .read_line(&mut answer)
        .map_err(|e| io_error("read confirmation answer", e))?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}
