//! `claude-monitor progress [<id>]` and `progress bind`: a session's plan
//! progress with its source, and the choice of source (FR-015a).

use std::path::{Path, PathBuf};

use super::Output;
use crate::env::Env;
use crate::error::Error;

pub fn run(
    env: &Env,
    state_dir: Option<&Path>,
    owner: &str,
    file: Option<PathBuf>,
    source: Option<String>,
) -> Result<Output, Error> {
    let _ = (env, state_dir, owner, file, source);
    Err(Error::NotImplemented("progress-bind".to_owned()))
}
