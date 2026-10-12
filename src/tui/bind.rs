//! The progress-source chooser (`b`): the plan files that could be the
//! selected session's, and "none". Picking one is `progress bind`.

use std::path::PathBuf;

use super::app::Msg;
use crate::env::Env;
use crate::progress::binding::{discover_candidates, display_path, session_root};
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

    /// Moves the highlight one row, stopping at either end.
    pub fn step(&mut self, forward: bool) {
        let last = self.options.len().saturating_sub(1);
        self.cursor = if forward {
            self.cursor.saturating_add(1).min(last)
        } else {
            self.cursor.saturating_sub(1)
        };
    }

    pub fn highlighted(&self) -> Option<&BindOption> {
        self.options.get(self.cursor)
    }
}

const NONE_LABEL: &str = "none (show no progress for this session)";

/// Reads the candidate plans of session `id` and answers with
/// [`Msg::Candidates`], or [`Msg::Failed`] when the session cannot be read.
pub fn load(env: &Env, store: &Store, id: &str) -> Msg {
    let session = match store.get_session(id) {
        Ok(Some(session)) => session,
        Ok(None) => return Msg::Failed(format!("session {id} no longer exists")),
        Err(e) => return Msg::Failed(format!("cannot read session {id}: {e}")),
    };
    let root = session_root(&session);
    let mut options: Vec<BindOption> = discover_candidates(&session, env)
        .into_iter()
        .map(|candidate| BindOption {
            label: format!(
                "{}  ({} of {})",
                display_path(&root, &candidate.path),
                candidate.done,
                candidate.total
            ),
            path: Some(candidate.path),
        })
        .collect();
    options.push(BindOption {
        label: NONE_LABEL.to_owned(),
        path: None,
    });
    Msg::Candidates {
        id: id.to_owned(),
        options,
    }
}
