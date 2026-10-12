//! `claude-monitor respawn <session>... [--fresh [--in DIR]] [--recreate-worktree]`
//! and the batch machinery `restore` shares: plan every session, start the
//! ones that can start, report the ones that cannot.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::migrate::{open_viewer_store, store_error_message};
use super::show::resolve;
use super::Output;
use crate::env::Env;
use crate::error::Error;
use crate::model::Session;
use crate::respawn::{
    execute, launcher_for, plan_respawn, tmux_session_for, Alternative, Options, Plan, Refusal,
    RefusalCode, Started,
};
use crate::store::Store;

/// Why a spawn that was planned did not happen or finish.
const SPAWN_FAILED: &str = "spawn_failed";

/// Each session of a batch with its plan or the reason it has none.
pub(super) type Planned = Vec<(Session, Result<Plan, Refusal>)>;

/// The flags that shape every plan in a batch.
#[derive(Debug, Clone, Default)]
pub(super) struct PlanFlags {
    pub fresh: bool,
    pub in_dir: Option<PathBuf>,
    pub recreate_worktree: bool,
}

pub(super) struct StartedItem {
    started: Started,
    plan: Plan,
}

pub(super) struct RefusedItem {
    session_id: String,
    code: &'static str,
    message: String,
    alternatives: Vec<Alternative>,
}

/// What became of one session of a batch.
pub(super) enum Item {
    Started(Box<StartedItem>),
    Refused(RefusedItem),
}

pub fn run(
    env: &Env,
    state_dir: Option<&Path>,
    sessions: &[String],
    flags: PlanFlags,
) -> Result<Output, Error> {
    if sessions.is_empty() {
        return Err(Error::Usage(
            "respawn needs at least one session".to_owned(),
        ));
    }
    let store = open_viewer_store(env, state_dir)?;
    let mut ids = Vec::new();
    for id in sessions {
        let id = resolve(&store, id)?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let planned = plan_all(&store, env, &ids, &flags)?;
    if let Some(error) = usage_error(&planned) {
        return Err(error);
    }
    let items = start_all(&store, env, planned);
    finish("respawn", &items, &[])
}

/// Plan each session id with `flags`; a session that cannot be read is an
/// error for the whole command, a refusal is data.
pub(super) fn plan_all(
    store: &Store,
    env: &Env,
    ids: &[String],
    flags: &PlanFlags,
) -> Result<Planned, Error> {
    ids.iter()
        .map(|id| {
            let session = store
                .get_session(id)
                .map_err(|e| store_error_message("read session", Some(store.path()), &e))?
                .ok_or_else(|| Error::Refused {
                    code: "unknown_session",
                    message: format!("no session has id {id:?}"),
                })?;
            let options = Options {
                recreate_worktree: flags.recreate_worktree,
                fresh: flags.fresh,
                in_dir: flags.in_dir.clone(),
                launcher: launcher_for(env, &session.agent_kind),
            };
            let plan = plan_respawn(&session, env, &options);
            Ok((session, plan))
        })
        .collect()
}

/// Contradictory flags are a usage error (exit 1), not a refusal of one session.
pub(super) fn usage_error(planned: &[(Session, Result<Plan, Refusal>)]) -> Option<Error> {
    planned.iter().find_map(|(_, plan)| match plan {
        Err(refusal) if refusal.code == RefusalCode::InvalidOptions => {
            Some(Error::Usage(refusal.message.clone()))
        }
        _ => None,
    })
}

pub(super) fn tmux_session(env: &Env) -> String {
    tmux_session_for(env)
}

/// Execute every plan that exists, in order. A failure to start one does not
/// stop the others.
pub(super) fn start_all(
    store: &Store,
    env: &Env,
    planned: Vec<(Session, Result<Plan, Refusal>)>,
) -> Vec<Item> {
    let tmux_session = tmux_session(env);
    let now = env.clock.now();
    planned
        .into_iter()
        .map(|(session, plan)| match plan {
            Err(refusal) => Item::Refused(RefusedItem {
                session_id: session.id,
                code: refusal.code.as_str(),
                message: refusal.message,
                alternatives: refusal.alternatives,
            }),
            Ok(plan) => match execute(store, env, &plan, &tmux_session, &now) {
                Ok(started) => Item::Started(Box::new(StartedItem { started, plan })),
                Err(error) => Item::Refused(RefusedItem {
                    session_id: session.id,
                    code: SPAWN_FAILED,
                    message: error.to_string(),
                    alternatives: Vec::new(),
                }),
            },
        })
        .collect()
}

/// Exit 0 when everything started, 2 when nothing did, 3 for a mix.
pub(super) fn finish(operation: &str, items: &[Item], skipped: &[String]) -> Result<Output, Error> {
    let started = items
        .iter()
        .filter(|item| matches!(item, Item::Started(_)))
        .count();
    let refused: Vec<&RefusedItem> = items
        .iter()
        .filter_map(|item| match item {
            Item::Refused(r) => Some(r),
            Item::Started(_) => None,
        })
        .collect();
    if refused.is_empty() {
        return Ok(Output::Dual {
            text: started_text(items, skipped),
            json: json!({
                "ok": true,
                "executed": true,
                "started": started_json(items),
                "refused": [],
                "skipped": skipped,
            }),
        });
    }
    let lines: Vec<String> = refused.iter().map(|r| refusal_text(r)).collect();
    if started == 0 {
        // The first refusal names the exit code; the message carries them all.
        let code = refused.first().map_or(SPAWN_FAILED, |r| r.code);
        return Err(Error::Refused {
            code,
            message: lines.join("\n"),
        });
    }
    Err(Error::Partial {
        operation: operation.to_owned(),
        detail: format!(
            "started {} of {}:\n{}\n{}",
            started,
            items.len(),
            started_text(items, &[]),
            lines.join("\n")
        ),
    })
}

fn started_text(items: &[Item], skipped: &[String]) -> String {
    let mut lines = Vec::new();
    for item in items {
        if let Item::Started(item) = item {
            lines.push(format!(
                "started {} ({}) in {} as pane {}",
                item.started.session_id,
                item.plan.mode.as_str(),
                item.plan.launch.cwd.display(),
                item.started.pane_id
            ));
            lines.extend(item.plan.notices.iter().map(|n| format!("  note: {n}")));
        }
    }
    lines.extend(skipped.iter().map(|id| format!("skipped {id}")));
    lines.join("\n")
}

fn refusal_text(item: &RefusedItem) -> String {
    let mut lines = vec![format!(
        "{}: {}: {}",
        item.session_id, item.code, item.message
    )];
    lines.extend(
        item.alternatives
            .iter()
            .map(|a| format!("  instead: {}: {}", a.description, a.command)),
    );
    lines.join("\n")
}

fn started_json(items: &[Item]) -> Vec<Value> {
    items
        .iter()
        .filter_map(|item| match item {
            Item::Started(item) => Some(json!({
                "session": item.started.session_id,
                "mode": item.plan.mode.as_str(),
                "pane": item.started.pane_id,
                "window": item.started.window_id,
                "request": item.started.request_id,
                "cwd": item.plan.launch.cwd,
                "label": item.plan.label,
                "notices": item.plan.notices,
            })),
            Item::Refused(_) => None,
        })
        .collect()
}

/// A plan as shown by `restore` without `--yes`.
pub(super) fn would_start_json(session: &Session, plan: &Plan) -> Value {
    json!({
        "session": session.id,
        "mode": plan.mode.as_str(),
        "cwd": plan.launch.cwd,
        "label": plan.label,
        "setup": plan.setup.iter().map(|s| {
            let mut words = vec![s.program.clone()];
            words.extend(s.args.iter().cloned());
            words.join(" ")
        }).collect::<Vec<_>>(),
        "notices": plan.notices,
    })
}

pub(super) fn would_refuse_json(session: &Session, refusal: &Refusal) -> Value {
    json!({
        "session": session.id,
        "code": refusal.code.as_str(),
        "message": refusal.message,
        "alternatives": refusal.alternatives.iter().map(|a| json!({
            "description": a.description,
            "command": a.command,
        })).collect::<Vec<_>>(),
    })
}
