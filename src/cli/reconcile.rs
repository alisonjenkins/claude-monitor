//! `claude-monitor reconcile [--pane %N | --session ID]`: apply the liveness
//! rules to everything, or to one pane or session.

use std::path::Path;

use serde_json::{json, Value};

use super::migrate::{store_error, store_error_message};
use super::Output;
use crate::env::Env;
use crate::error::Error;
use crate::reconcile::{reconcile, ReconcileReport, Scope};
use crate::store::{resolve_state_dir, StateDirEnv, Store, StoreOptions};

pub fn run(
    env: &Env,
    state_dir: Option<&Path>,
    pane: Option<String>,
    session: Option<String>,
) -> Result<Output, Error> {
    let scope = match (pane, session) {
        (None, None) => Scope::All,
        (Some(pane), None) => Scope::Pane(pane),
        (None, Some(session)) => Scope::Session(session),
        (Some(_), Some(_)) => {
            return Err(Error::Usage(
                "reconcile takes --pane or --session, not both".to_owned(),
            ))
        }
    };
    let dir =
        resolve_state_dir(state_dir, &StateDirEnv::from_process()).map_err(|e| store_error(&e))?;
    let store = Store::open_in_dir(&dir, &StoreOptions::viewer()).map_err(|e| store_error(&e))?;
    let report = reconcile(&store, env, scope.clone(), &env.clock.now())
        .map_err(|e| store_error_message("reconcile", Some(store.path()), &e))?;
    if let Scope::Session(id) = &scope {
        if report.checked == 0 && report.degraded.is_empty() {
            return Err(Error::Refused {
                code: "unknown_session",
                message: format!("no session has id {id:?}"),
            });
        }
    }
    Ok(Output::Dual {
        text: text(&report),
        json: report_json(&report),
    })
}

fn text(report: &ReconcileReport) -> String {
    let mut lines = vec![format!(
        "checked {}, interrupted {}, unchanged {}, degraded {}",
        report.checked,
        report.interrupted,
        report.unchanged,
        report.degraded.len()
    )];
    lines.extend(
        report
            .degraded
            .iter()
            .map(|d| format!("skipped {}: {}", d.id, d.error)),
    );
    lines.join("\n")
}

fn report_json(report: &ReconcileReport) -> Value {
    json!({
        "ok": true,
        "checked": report.checked,
        "interrupted": report.interrupted,
        "revived": report.revived,
        "unchanged": report.unchanged,
        "degraded": report.degraded.len(),
        "degraded_sessions": report
            .degraded
            .iter()
            .map(|d| json!({ "id": d.id, "error": d.error }))
            .collect::<Vec<_>>(),
    })
}
