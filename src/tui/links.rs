//! Link status on the detail page (T103, FR-026, FR-028): the session's links
//! and, on demand, how each compares with its remote node. The check runs off
//! the UI thread and never writes to the store or to the remote.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::app::Msg;
use super::view::{dim, heading, GREEN, PEACH, RED};
use crate::cli::detail::{link_text, SessionDetail};
use crate::env::{Env, Vars};
use crate::export::links::{check_owner_links, Check, LinkCheck, Outcome};
use crate::model::OwnerKind;
use crate::store::links::Link;
use crate::store::{Store, StoreOptions};

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
pub fn run_check(store: &Store, vars: &dyn Vars, id: &str, timeout: Duration) -> Msg {
    let result =
        check_owner_links(store, vars, timeout, OwnerKind::Session, id).map_err(|e| e.to_string());
    Msg::LinksChecked {
        id: id.to_owned(),
        result,
    }
}

fn failed(id: &str, reason: String) -> Msg {
    Msg::LinksChecked {
        id: id.to_owned(),
        result: Err(reason),
    }
}

/// Runs [`run_check`] on a worker thread against its own store connection and
/// sends the answer on `tx`.
pub fn spawn_check(
    env: &Env,
    store_path: &Path,
    id: &str,
    timeout: Duration,
    tx: Sender<Msg>,
) -> std::io::Result<()> {
    let vars = env.vars.clone();
    let path: PathBuf = store_path.to_owned();
    let id = id.to_owned();
    thread::Builder::new()
        .name("links-check".to_owned())
        .spawn(move || {
            let msg = match Store::open(&path, &StoreOptions::viewer()) {
                Ok(store) => run_check(&store, vars.as_ref(), &id, timeout),
                Err(e) => failed(&id, format!("cannot open the store: {e}")),
            };
            let _ = tx.send(msg);
        })
        .map(|_| ())
}

fn outcome_span(outcome: &Outcome) -> Span<'static> {
    let (text, color) = match outcome {
        Outcome::Agree { status } => (format!("agree ({status})"), GREEN),
        Outcome::Disagree { local, remote } => {
            (format!("disagree: local {local}, remote {remote}"), PEACH)
        }
        Outcome::NoRemoteStatus => ("no remote status".to_owned(), GREEN),
        Outcome::OwnerMissing => ("owner gone".to_owned(), RED),
        Outcome::Unreachable { reason } => (format!("unreachable: {reason}"), RED),
    };
    let style = Style::default().fg(color);
    let style = if matches!(outcome, Outcome::Disagree { .. }) {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    };
    Span::styled(text, style)
}

fn result_for<'a>(checks: &'a [LinkCheck], link: &Link) -> Option<&'a LinkCheck> {
    checks.iter().find(|check| check.link.id == link.id)
}

fn link_line(link: &Link, state: &LinksState) -> Line<'static> {
    let mut spans = vec![Span::raw(format!("  {}", link_text(link)))];
    let note = match state {
        LinksState::Idle | LinksState::Failed(_) => None,
        LinksState::Checking => Some(Span::styled("checking…", Style::default())),
        LinksState::Checked(Check::Skipped { reason }) => Some(Span::styled(
            format!("skipped: {reason}"),
            Style::default().fg(PEACH),
        )),
        LinksState::Checked(Check::Checked(checks)) => {
            result_for(checks, link).map(|check| outcome_span(&check.outcome))
        }
    };
    if let Some(note) = note {
        spans.push(Span::raw("  "));
        spans.push(note);
    }
    Line::from(spans)
}

/// The detail page's link block: one line per link, with its check result
/// once there is one. Empty when the session has no links.
pub fn link_lines(detail: &SessionDetail, state: &LinksState) -> Vec<Line<'static>> {
    if detail.links.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![heading("Links".to_owned())];
    lines.extend(detail.links.iter().map(|link| link_line(link, state)));
    match state {
        LinksState::Idle => {
            lines.push(dim("  press L to check them against cortexiphan".to_owned()))
        }
        LinksState::Failed(reason) => {
            lines.push(Line::from(Span::styled(
                format!("  check failed: {reason}"),
                Style::default().fg(RED),
            )));
            lines.push(dim("  press L to try again".to_owned()));
        }
        LinksState::Checking | LinksState::Checked(_) => {}
    }
    lines
}
