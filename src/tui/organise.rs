//! The organise actions the dashboard offers on the selected row (T111,
//! FR-018, FR-019b, FR-019c). Each one is the CLI command of the same name,
//! so the store checks, the user-authored event and the refusals are the ones
//! the command line has.

use super::app::Msg;
use crate::env::Env;
use crate::store::Store;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Organise {
    Done { id: String },
    Undone { id: String },
    Dismiss { id: String },
    Undismiss { id: String },
    Note { id: String, text: String },
}

/// Carries the action out against `store`'s state directory and answers with
/// the reloaded rows, or with the command's own refusal as a footer message.
pub fn run(_env: &Env, _store: &Store, _action: &Organise) -> Msg {
    Msg::Failed("not implemented".to_owned())
}
