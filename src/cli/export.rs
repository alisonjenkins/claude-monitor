//! `claude-monitor export cortexiphan`: JSON Lines for a future cortexiphan
//! loader, to stdout or a file (FR-025).

use std::ffi::OsString;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;

use super::migrate::open_viewer_store;
use super::Output;
use crate::env::Env;
use crate::error::Error;
use crate::export::cortexiphan::{export_with_report, Export, ExportError, ExportOptions};
use crate::text::single_line;

const HOSTNAME_VAR: &str = "HOSTNAME";
const UNKNOWN_MACHINE: &str = "unknown";
/// An export can carry conversation text, so only the owner may read it.
const PRIVATE_FILE_MODE: u32 = 0o600;

pub struct ExportArgs {
    pub project: Option<String>,
    pub include_text: bool,
    pub out: Option<PathBuf>,
}

/// `$HOSTNAME` when exported, else the `hostname` command, else `unknown`.
fn machine_name(env: &Env) -> String {
    if let Some(name) = env.vars.var(HOSTNAME_VAR) {
        return name;
    }
    Command::new("hostname")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| UNKNOWN_MACHINE.to_owned())
}

fn write_atomically(path: &Path, text: &str) -> Result<(), Error> {
    let io = |source| Error::Io {
        operation: "write export".to_owned(),
        path: Some(path.to_owned()),
        source,
    };
    let mut temp: OsString = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);
    // A leftover temp file keeps its old mode, so start from a fresh one.
    match std::fs::remove_file(&temp) {
        Ok(()) => {}
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(io(source)),
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(PRIVATE_FILE_MODE)
        .open(&temp)
        .map_err(io)?;
    let written = file.write_all(text.as_bytes()).and_then(|()| file.flush());
    if let Err(source) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(io(source));
    }
    std::fs::rename(&temp, path).map_err(|source| {
        let _ = std::fs::remove_file(&temp);
        io(source)
    })
}

fn skipped_warning(ids: &[String]) -> String {
    format!(
        "claude-monitor: export: skipped {} corrupt session(s), not in the export: {}",
        ids.len(),
        ids.iter()
            .map(|id| single_line(id))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

pub fn run(env: &Env, state_dir: Option<&Path>, args: ExportArgs) -> Result<Output, Error> {
    let store = open_viewer_store(env, state_dir)?;
    let options = ExportOptions {
        project: args.project,
        include_text: args.include_text,
        machine: machine_name(env),
    };
    let Export {
        jsonl,
        skipped_sessions,
    } = export_with_report(&store, &options).map_err(|error| match error {
        ExportError::UnknownProject { .. } => Error::Refused {
            code: "unknown_project",
            message: error.to_string(),
        },
        other => Error::Store {
            operation: "export".to_owned(),
            path: Some(store.path().to_owned()),
            detail: other.to_string(),
        },
    })?;
    if !skipped_sessions.is_empty() {
        eprintln!("{}", skipped_warning(&skipped_sessions));
    }
    let Some(out) = args.out else {
        return Ok(Output::Raw(jsonl));
    };
    write_atomically(&out, &jsonl)?;
    let records = jsonl.lines().count();
    let path = out.display().to_string();
    Ok(Output::Dual {
        text: format!("exported {records} records to {}", single_line(&path)),
        json: json!({
            "ok": true,
            "records": records,
            "out": path,
            "skipped": skipped_sessions,
        }),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn a_hostile_session_id_cannot_forge_a_line_or_drive_the_terminal() {
        let ids = vec![
            "x\n\u{1b}[2Jclaude-monitor: forged\ty".to_owned(),
            "ok".to_owned(),
        ];
        let warning = skipped_warning(&ids);
        assert_eq!(warning.lines().count(), 1, "{warning:?}");
        assert!(!warning.contains(['\u{1b}', '\t']), "{warning:?}");
        assert!(
            warning.ends_with("x [2Jclaude-monitor: forged y, ok"),
            "{warning:?}"
        );
    }

    #[test]
    fn the_export_file_is_owner_only() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("out.jsonl");
        write_atomically(&target, "{}\n").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "{}\n");
        assert_eq!(mode_of(&target), 0o600);
    }

    #[test]
    fn a_stale_world_readable_temp_file_does_not_leak_its_mode() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("out.jsonl");
        let stale = dir.path().join("out.jsonl.tmp");
        std::fs::write(&stale, "old").unwrap();
        std::fs::set_permissions(&stale, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_atomically(&target, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        assert_eq!(mode_of(&target), 0o600);
    }
}
