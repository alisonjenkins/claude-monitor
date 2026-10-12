//! `claude-monitor summarize <session>`: one on-demand model-written summary,
//! from a model on this machine only (FR-017a, FR-020).

use std::path::Path;

use super::Output;
use crate::env::Env;
use crate::error::Error;

pub fn run(_env: &Env, _state_dir: Option<&Path>, _target: &str) -> Result<Output, Error> {
    Err(Error::NotImplemented("summarize".to_owned()))
}
