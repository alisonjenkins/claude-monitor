//! `claude-monitor restore [--all-interrupted] [--include-finished] [--fresh]
//! [--skip ID]... [--yes]`: respawn every interrupted session at once. Without
//! `--yes` it prints what would start and starts nothing; the interactive
//! confirmation is the dashboard's. Starting needs `--all-interrupted`
//! as well (FR-034), so no script restores by accident.

use std::path::Path;

use serde_json::json;

use super::migrate::{open_viewer_store, store_error_message};
use super::respawn::{
    finish, plan_all, start_batch, tmux_session, usage_error, would_refuse_json, would_start_json,
    PlanFlags,
};
use super::show::resolve;
use super::Output;
use crate::env::Env;
use crate::error::Error;
use crate::model::{Session, State};
use crate::store::sessions::SessionRow;
use crate::store::Store;

pub struct RestoreArgs {
    pub all_interrupted: bool,
    pub include_finished: bool,
    pub fresh: bool,
    pub skip: Vec<String>,
    pub yes: bool,
}

pub fn run(env: &Env, state_dir: Option<&Path>, args: RestoreArgs) -> Result<Output, Error> {
    if args.yes && !args.all_interrupted {
        return Err(Error::Usage(
            "restore --yes needs --all-interrupted, to say that every interrupted \
             session should start"
                .to_owned(),
        ));
    }
    let store = open_viewer_store(env, state_dir)?;
    let mut skipped = Vec::new();
    for id in &args.skip {
        let id = resolve(&store, id)?;
        if !skipped.contains(&id) {
            skipped.push(id);
        }
    }
    let ids: Vec<String> = candidates(&store, args.include_finished)?
        .into_iter()
        .map(|s| s.id)
        .filter(|id| !skipped.contains(id))
        .collect();
    let flags = PlanFlags {
        fresh: args.fresh,
        ..PlanFlags::default()
    };
    let planned = plan_all(&store, env, &ids, &flags)?;
    if let Some(error) = usage_error(&planned) {
        return Err(error);
    }

    if !args.yes {
        return Ok(preview(env, &planned, &skipped));
    }
    if planned.is_empty() {
        return Ok(Output::Dual {
            text: "nothing to restore".to_owned(),
            json: json!({
                "ok": true,
                "executed": true,
                "started": [],
                "refused": [],
                "skipped": skipped,
            }),
        });
    }
    let items = start_batch(&store, env, planned);
    finish("restore", &items, &skipped)
}

/// Interrupted sessions the user has not dismissed, plus finished ones when
/// asked, oldest first.
pub(crate) fn candidates(store: &Store, include_finished: bool) -> Result<Vec<Session>, Error> {
    let rows = store
        .list_sessions_lenient()
        .map_err(|e| store_error_message("list sessions", Some(store.path()), &e))?;
    let mut sessions: Vec<Session> = rows
        .into_iter()
        .filter_map(|row| match row {
            SessionRow::Readable(s) => Some(*s),
            _ => None,
        })
        .filter(|s| {
            !s.dismissed
                && (s.state == State::Interrupted
                    || (include_finished && s.state == State::Finished))
        })
        .collect();
    sessions.sort_by(|a, b| (&a.started_at, &a.id).cmp(&(&b.started_at, &b.id)));
    Ok(sessions)
}

fn preview(
    env: &Env,
    planned: &[(
        Session,
        Result<crate::respawn::Plan, crate::respawn::Refusal>,
    )],
    skipped: &[String],
) -> Output {
    let mut start = Vec::new();
    let mut refuse = Vec::new();
    let mut lines = Vec::new();
    for (session, plan) in planned {
        match plan {
            Ok(plan) => {
                lines.push(format!(
                    "would start {} ({}) in {}",
                    session.id,
                    plan.mode.as_str(),
                    plan.launch.cwd.display()
                ));
                lines.extend(plan.notices.iter().map(|n| format!("  note: {n}")));
                start.push(would_start_json(session, plan));
            }
            Err(refusal) => {
                lines.push(format!(
                    "would refuse {}: {}: {}",
                    session.id,
                    refusal.code.as_str(),
                    refusal.message
                ));
                lines.extend(
                    refusal
                        .alternatives
                        .iter()
                        .map(|a| format!("  instead: {}: {}", a.description, a.command)),
                );
                refuse.push(would_refuse_json(session, refusal));
            }
        }
    }
    lines.extend(skipped.iter().map(|id| format!("skipping {id}")));
    if planned.is_empty() {
        lines.push("nothing to restore".to_owned());
    } else {
        lines.push(format!(
            "nothing started; run with --all-interrupted --yes to start these in tmux session {}",
            tmux_session(env)
        ));
    }
    Output::Dual {
        text: lines.join("\n"),
        json: json!({
            "ok": true,
            "executed": false,
            "tmux_session": tmux_session(env),
            "would_start": start,
            "would_refuse": refuse,
            "skipped": skipped,
        }),
    }
}
