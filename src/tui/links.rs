//! Link status on the detail page (T103, FR-026, FR-028): the session's links
//! and, on demand, how each compares with its remote node. The check runs off
//! the UI thread and never writes to the store or to the remote.

use std::path::Path;
use std::sync::mpsc::Sender;
use std::time::Duration;

use ratatui::text::Line;

use super::app::Msg;
use crate::cli::detail::SessionDetail;
use crate::env::{Env, Vars};
use crate::export::links::Check;
use crate::store::Store;

/// Where the detail page's link check stands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LinksState {
    #[default]
    Idle,
    Checking,
    Checked(Check),
    Failed(String),
}

/// Checks the links of one session and returns the answer as a message.
pub fn run_check(_store: &Store, _vars: &dyn Vars, id: &str, _timeout: Duration) -> Msg {
    Msg::LinksChecked {
        id: id.to_owned(),
        result: Err("not implemented".to_owned()),
    }
}

/// Runs [`run_check`] on a worker thread against its own store connection and
/// sends the answer on `tx`.
pub fn spawn_check(
    _env: &Env,
    _store_path: &Path,
    _id: &str,
    _timeout: Duration,
    _tx: Sender<Msg>,
) -> std::io::Result<()> {
    Ok(())
}

/// The detail page's link block: one line per link, with its check result
/// once there is one.
pub fn link_lines(_detail: &SessionDetail, _state: &LinksState) -> Vec<Line<'static>> {
    Vec::new()
}
