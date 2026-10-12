use std::path::Path;

use serde_json::{json, Value};

use super::Output;
use crate::env::Env;
use crate::error::Error;
use crate::session;
use crate::store::legacy::{self, LegacyError, MigrateReport};
use crate::store::{resolve_state_dir, StateDirEnv, Store, StoreError, StoreOptions};

pub fn store_error(error: &StoreError) -> Error {
    Error::Store {
        operation: error.operation().to_owned(),
        path: error.path().map(Path::to_owned),
        detail: error.to_string(),
    }
}

fn legacy_error(error: &LegacyError) -> Error {
    Error::Store {
        operation: "import legacy runtime sessions".to_owned(),
        path: None,
        detail: error.to_string(),
    }
}

pub fn store_error_message(
    operation: &str,
    path: Option<&Path>,
    error: &dyn std::fmt::Display,
) -> Error {
    Error::Store {
        operation: operation.to_owned(),
        path: path.map(Path::to_owned),
        detail: error.to_string(),
    }
}

/// Opens the store for a viewer command and runs the first-start legacy
/// import. An import failure is reported on stderr and never stops the
/// command (FR-021).
pub fn open_viewer_store(env: &Env, state_dir: Option<&Path>) -> Result<Store, Error> {
    let dir =
        resolve_state_dir(state_dir, &StateDirEnv::from_process()).map_err(|e| store_error(&e))?;
    let store = Store::open_in_dir(&dir, &StoreOptions::viewer()).map_err(|e| store_error(&e))?;
    if let Err(error) = store_migrate_if_needed(&store, env) {
        eprintln!("claude-monitor: legacy import skipped: {error}");
    }
    Ok(store)
}

/// `claude-monitor migrate`: import the legacy runtime JSON files into the store.
pub fn run(state_dir: &Path, runtime_dir: &Path, env: &Env) -> Result<Output, Error> {
    let store =
        Store::open_in_dir(state_dir, &StoreOptions::viewer()).map_err(|e| store_error(&e))?;
    let report =
        legacy::migrate(&store, runtime_dir, env.clock.as_ref()).map_err(|e| legacy_error(&e))?;
    if report.all_failed() {
        return Err(Error::Partial {
            operation: "migrate".to_owned(),
            detail: format!(
                "none of the {} legacy files could be imported: {}",
                report.candidates(),
                report.corrupt_files.join(", ")
            ),
        });
    }
    Ok(Output::Json(report_json(&report)))
}

fn report_json(report: &MigrateReport) -> Value {
    let mut fields = serde_json::Map::new();
    fields.insert("ok".to_owned(), json!(true));
    fields.insert("imported".to_owned(), json!(report.imported));
    fields.insert(
        "skipped_existing".to_owned(),
        json!(report.skipped_existing),
    );
    fields.insert("skipped_corrupt".to_owned(), json!(report.skipped_corrupt));
    if !report.corrupt_files.is_empty() {
        fields.insert("corrupt_files".to_owned(), json!(report.corrupt_files));
    }
    Value::Object(fields)
}

/// The first-start import for viewer commands: runs once per store, never on
/// the hook path.
pub fn store_migrate_if_needed(
    store: &Store,
    env: &Env,
) -> Result<Option<MigrateReport>, LegacyError> {
    legacy::migrate_if_needed(store, &session::status_dir(), env.clock.as_ref())
}

/// Opens the store and runs [`store_migrate_if_needed`] for a viewer that has
/// no store handle of its own yet. A failure is reported on stderr and never
/// stops the viewer (FR-021).
pub fn auto_migrate(state_dir: Option<&Path>, env: &Env) {
    let outcome = resolve_state_dir(state_dir, &StateDirEnv::from_process())
        .map_err(|e| store_error(&e))
        .and_then(|dir| {
            Store::open_in_dir(&dir, &StoreOptions::viewer()).map_err(|e| store_error(&e))
        })
        .and_then(|store| store_migrate_if_needed(&store, env).map_err(|e| legacy_error(&e)));
    if let Err(error) = outcome {
        eprintln!("claude-monitor: legacy import skipped: {error}");
    }
}
