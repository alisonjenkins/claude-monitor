use std::path::Path;

use super::Output;
use crate::env::Env;
use crate::error::Error;

/// `claude-monitor migrate`: import the legacy runtime JSON files into the store.
pub fn run(_state_dir: &Path, _runtime_dir: &Path, _env: &Env) -> Result<Output, Error> {
    Err(Error::NotImplemented("migrate".to_owned()))
}
