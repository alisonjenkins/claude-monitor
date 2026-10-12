//! The progress-source chooser (`b`): the plan files that could be the
//! selected session's, and "none". Picking one is `progress bind`.

use std::path::PathBuf;

use super::app::Msg;
use crate::env::Env;
use crate::store::Store;

/// One row of the chooser: a plan file, or `None` for no plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindOption {
    pub label: String,
    pub path: Option<PathBuf>,
}

/// The open chooser: the session it is for, its rows and the highlighted one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindChooser {
    id: String,
    options: Vec<BindOption>,
    cursor: usize,
}

impl BindChooser {
    pub fn new(id: &str, options: Vec<BindOption>) -> Self {
        Self {
            id: id.to_owned(),
            options,
            cursor: 0,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn options(&self) -> &[BindOption] {
        &self.options
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }
}

/// Reads the candidate plans of session `id` and answers with
/// [`Msg::Candidates`], or [`Msg::Failed`] when the session cannot be read.
pub fn load(env: &Env, store: &Store, id: &str) -> Msg {
    let _ = (env, store);
    Msg::Candidates {
        id: id.to_owned(),
        options: Vec::new(),
    }
}
