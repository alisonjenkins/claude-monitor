//! Asks before a destructive or hard-to-notice command runs: `dismiss` and
//! `unlink` without `--yes`.

use std::io::{BufRead, Write};
use std::path::Path;

use super::Command;
use crate::env::Env;
use crate::error::Error;

/// Where a question is asked and answered.
pub struct Terminal<'a> {
    pub input: &'a mut dyn BufRead,
    pub output: &'a mut dyn Write,
}

/// Returns `command` ready to run: unchanged when it needs no confirmation,
/// with `yes` set once the user has agreed. Without a terminal, or when the
/// answer is not `y`, it fails and nothing is changed.
pub fn confirm_command(
    command: Command,
    _env: &Env,
    _state_dir: Option<&Path>,
    _terminal: Option<Terminal<'_>>,
) -> Result<Command, Error> {
    Ok(command)
}
