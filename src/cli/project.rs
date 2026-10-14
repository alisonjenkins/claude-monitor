//! `claude-monitor project add|rename|set-status <name>` and
//! `claude-monitor group <project> <session>...` (FR-018).

use std::path::Path;

use serde_json::json;

use super::command::ProjectAction;
use super::migrate::{open_viewer_store, store_error_message};
use super::show::{resolve, user_author};
use super::Output;
use crate::env::Env;
use crate::error::Error;
use crate::model::{Project, ProjectStatus};
use crate::store::projects::ProjectsError;
use crate::text::single_line;

/// Names the `group` command reads as "no project".
pub const NO_PROJECT: &str = "-";
const STATUS_NAMES: &str = "active, parked, done or archived";

enum Change<'a> {
    Add,
    Rename(&'a str),
    SetStatus(ProjectStatus),
}

fn refused(code: &'static str, message: String) -> Error {
    Error::Refused { code, message }
}

fn project_error(operation: &str, error: &ProjectsError) -> Error {
    match error {
        ProjectsError::Duplicate { .. } => refused("project_exists", error.to_string()),
        ProjectsError::UnknownProject { .. } => refused("unknown_project", error.to_string()),
        ProjectsError::UnknownSession { .. } => refused("unknown_session", error.to_string()),
        _ => store_error_message(operation, None, error),
    }
}

fn check_name(name: &str) -> Result<(), Error> {
    if name.trim().is_empty() || name == NO_PROJECT {
        return Err(Error::Usage(format!(
            "{name:?} is not a usable project name; {NO_PROJECT:?} means no project"
        )));
    }
    Ok(())
}

fn project_output(verb: &str, project: &Project) -> Output {
    Output::Dual {
        text: format!(
            "{verb} project {} ({})",
            single_line(&project.name),
            project.status.as_str()
        ),
        json: json!({ "ok": true, "project": project }),
    }
}

pub fn run_project(
    env: &Env,
    state_dir: Option<&Path>,
    action: &ProjectAction,
    name: &str,
    value: Option<&str>,
) -> Result<Output, Error> {
    check_name(name)?;
    let author = user_author(env);
    let now = env.clock.now();
    let required = |what: &str| {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::Usage(format!("project {action:?} needs {what}")))
    };
    // Parse before opening the store so a usage error never creates one.
    let change = match action {
        ProjectAction::Add => Change::Add,
        ProjectAction::Rename => {
            let new_name = required("the new name")?;
            check_name(new_name)?;
            Change::Rename(new_name)
        }
        ProjectAction::SetStatus => {
            let text = required("a status")?;
            Change::SetStatus(ProjectStatus::parse(text).ok_or_else(|| {
                Error::Usage(format!("unknown status {text:?}; expected {STATUS_NAMES}"))
            })?)
        }
    };
    let store = open_viewer_store(env, state_dir)?;
    let (verb, result) = match change {
        Change::Add => ("created", store.create_project(name, &now, &author)),
        Change::Rename(new_name) => (
            "renamed",
            store.rename_project(name, new_name, &now, &author),
        ),
        Change::SetStatus(status) => (
            "updated",
            store.set_project_status(name, status, &now, &author),
        ),
    };
    let project = result.map_err(|e| project_error("update project", &e))?;
    Ok(project_output(verb, &project))
}

pub fn run_group(
    env: &Env,
    state_dir: Option<&Path>,
    project: &str,
    sessions: &[String],
) -> Result<Output, Error> {
    if sessions.is_empty() {
        return Err(Error::Usage("group needs at least one session".to_owned()));
    }
    let target = (project != NO_PROJECT).then_some(project);
    if let Some(name) = target {
        check_name(name)?;
    }
    let store = open_viewer_store(env, state_dir)?;
    let mut ids = Vec::new();
    for session in sessions {
        let id = resolve(&store, session)?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let report = store
        .group_sessions(target, &ids, &env.clock.now(), &user_author(env))
        .map_err(|e| project_error("group sessions", &e))?;
    let text = match target {
        Some(name) => format!(
            "{} in project {} ({} already there)",
            report.changed.len(),
            single_line(name),
            report.unchanged.len()
        ),
        None => format!(
            "{} removed from their project ({} had none)",
            report.changed.len(),
            report.unchanged.len()
        ),
    };
    Ok(Output::Dual {
        text,
        json: json!({
            "ok": true,
            "project": target,
            "changed": report.changed,
            "unchanged": report.unchanged,
        }),
    })
}
