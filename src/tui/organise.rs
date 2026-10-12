//! The organise actions the dashboard offers on the selected row (T111,
//! FR-018, FR-019b, FR-019c). Each one is the CLI command of the same name,
//! so the store checks, the user-authored event and the refusals are the ones
//! the command line has.

use std::path::PathBuf;

use super::app::{Msg, Snapshot};
use crate::cli::{run_in, Command};
use crate::env::Env;
use crate::store::Store;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Organise {
    Done {
        id: String,
    },
    Undone {
        id: String,
    },
    Dismiss {
        id: String,
    },
    Undismiss {
        id: String,
    },
    Note {
        id: String,
        text: String,
    },
    Park {
        id: String,
    },
    Unpark {
        id: String,
    },
    /// `project` is a name, or `-` to take the session out of its project.
    Group {
        id: String,
        project: String,
    },
    /// A plan file, or `None` for no plan.
    ProgressBind {
        id: String,
        path: Option<PathBuf>,
    },
    Link {
        id: String,
        node_id: String,
        doc: Option<String>,
    },
}

/// What `--source` takes to choose no plan.
const SOURCE_NONE: &str = "none";

/// The `kind` a link made from the dashboard has; the CLI's default.
const LINK_KIND: &str = "task";

impl Organise {
    fn command(&self) -> Command {
        match self {
            Self::Done { id } => Command::Done {
                sessions: vec![id.clone()],
            },
            Self::Undone { id } => Command::Undone {
                sessions: vec![id.clone()],
            },
            Self::Dismiss { id } => Command::Dismiss {
                target: id.clone(),
                yes: true,
            },
            Self::Undismiss { id } => Command::Undismiss { target: id.clone() },
            Self::Note { id, text } => Command::Note {
                target: id.clone(),
                text: text.clone(),
            },
            Self::Park { id } => Command::Park { target: id.clone() },
            Self::Unpark { id } => Command::Unpark { target: id.clone() },
            Self::Group { id, project } => Command::Group {
                project: project.clone(),
                sessions: vec![id.clone()],
            },
            Self::ProgressBind { id, path } => Command::ProgressBind {
                owner: id.clone(),
                file: path.clone(),
                source: path.is_none().then(|| SOURCE_NONE.to_owned()),
            },
            Self::Link { id, node_id, doc } => Command::Link {
                owner: id.clone(),
                cortex: node_id.clone(),
                kind: LINK_KIND.to_owned(),
                doc: doc.clone(),
            },
        }
    }
}

/// Carries the action out against `store`'s state directory and answers with
/// the reloaded rows, or with the command's own refusal as a footer message.
pub fn run(env: &Env, store: &Store, action: &Organise) -> Msg {
    let Some(dir) = store.path().parent() else {
        return Msg::Failed(format!(
            "database path {} has no directory",
            store.path().display()
        ));
    };
    if let Err(error) = run_in(action.command(), env, Some(dir)) {
        return Msg::Failed(error.to_string());
    }
    match Snapshot::load(store, &env.clock.now()) {
        Ok(snapshot) => Msg::Refreshed(snapshot),
        Err(error) => Msg::Failed(format!("cannot read sessions: {error}")),
    }
}
