//! `claude-monitor list`: the overview as text or JSON.

use std::path::Path;

use chrono::DateTime;
use serde_json::json;

use super::migrate::{open_viewer_store, store_error_message};
use super::output::strip_control;
use super::Output;
use crate::env::Env;
use crate::error::Error;
use crate::model::State;
use crate::store::queries::{OverviewEntry, OverviewQuery, SectionName};
use crate::store::sessions::parse_state;

const SHORT_ID_CHARS: usize = 8;
const TASK_CHARS: usize = 60;
const STATE_NAMES: &str = "working, needs_permission, idle, finished, interrupted or unknown";
const SECTION_NAMES: &str = "active, interrupted, finished, done, unseen or dismissed";

#[derive(Debug, Clone, Default)]
pub struct ListArgs {
    pub states: Vec<String>,
    pub agent: Option<String>,
    pub repo: Option<String>,
    pub project: Option<String>,
    pub text: Option<String>,
    pub section: Option<String>,
    pub include_dismissed: bool,
    pub expand_finished: bool,
    pub expand_unseen: bool,
}

fn parse_states(values: &[String]) -> Result<Vec<State>, Error> {
    values
        .iter()
        .map(|value| {
            parse_state(value).ok_or_else(|| {
                Error::Usage(format!("unknown state {value:?}; expected {STATE_NAMES}"))
            })
        })
        .collect()
}

fn parse_section(value: Option<&str>) -> Result<Option<SectionName>, Error> {
    value
        .map(|value| {
            SectionName::parse(value).ok_or_else(|| {
                Error::Usage(format!(
                    "unknown section {value:?}; expected {SECTION_NAMES}"
                ))
            })
        })
        .transpose()
}

pub fn run(env: &Env, state_dir: Option<&Path>, args: ListArgs) -> Result<Output, Error> {
    // Compatibility only: finished sessions are never collapsed (FR-019).
    let _ = args.expand_finished;
    let states = parse_states(&args.states)?;
    let section = parse_section(args.section.as_deref())?;
    let asked_for_unknown = states.contains(&State::Unknown);
    let store = open_viewer_store(env, state_dir)?;
    let query = OverviewQuery {
        states,
        agent: args.agent,
        repo: args.repo,
        project: args.project,
        text: args.text,
        include_dismissed: args.include_dismissed || section == Some(SectionName::Dismissed),
    };
    let now = env.clock.now();
    let overview = store
        .overview(&query)
        .map_err(|e| store_error_message("list sessions", Some(store.path()), &e))?;

    let mut entries = Vec::new();
    for part in overview.sections {
        if section.is_some_and(|wanted| wanted != part.name) {
            continue;
        }
        entries.extend(part.sessions);
    }

    // Presentation only: the JSON keeps every unseen session.
    let show_unseen =
        args.expand_unseen || asked_for_unknown || section == Some(SectionName::Unseen);
    let mut lines: Vec<String> = entries
        .iter()
        .filter(|entry| show_unseen || entry.section != SectionName::Unseen)
        .map(|entry| line(entry, &now))
        .collect();
    let hidden = entries
        .iter()
        .filter(|entry| entry.section == SectionName::Unseen)
        .count();
    if !show_unseen && hidden > 0 {
        lines.push(format!(
            "({hidden} unseen sessions hidden; use --expand-unseen)"
        ));
    }
    Ok(Output::Dual {
        text: lines.join("\n"),
        json: json!({ "sessions": entries }),
    })
}

/// `5s`, `12m`, `3h` or `2d`; `?` when either time does not parse.
pub(crate) fn age(now: &str, then: &str) -> String {
    let (Ok(now), Ok(then)) = (
        DateTime::parse_from_rfc3339(now),
        DateTime::parse_from_rfc3339(then),
    ) else {
        return "?".to_owned();
    };
    let seconds = now.signed_duration_since(then).num_seconds().max(0);
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds.div_euclid(60)),
        3600..=86399 => format!("{}h", seconds.div_euclid(3600)),
        _ => format!("{}d", seconds.div_euclid(86400)),
    }
}

fn shorten(text: &str) -> String {
    let single_line = strip_control(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut chars = single_line.chars();
    let head: String = chars.by_ref().take(TASK_CHARS).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

fn location(entry: &OverviewEntry) -> String {
    match (&entry.repo, &entry.branch) {
        (Some(repo), Some(branch)) => strip_control(&format!("{repo}/{branch}")),
        (Some(repo), None) => strip_control(repo),
        (None, _) => strip_control(&entry.cwd),
    }
}

fn line(entry: &OverviewEntry, now: &str) -> String {
    let id: String = entry.id.chars().take(SHORT_ID_CHARS).collect();
    let task = entry
        .error
        .as_deref()
        .or(entry.task_description.as_deref())
        .map(shorten)
        .unwrap_or_default();
    let flags = if entry.flags.is_empty() {
        String::new()
    } else {
        let names: Vec<&str> = entry.flags.iter().map(|flag| flag.as_str()).collect();
        format!(" [{}]", names.join(","))
    };
    format!(
        "{id:<8} {:<16} {:>4}  {}  {task}{flags}",
        entry.state.as_str(),
        age(now, &entry.last_activity_at),
        location(entry),
    )
    .trim_end()
    .to_owned()
}
