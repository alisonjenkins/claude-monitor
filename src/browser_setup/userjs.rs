//! The marked block `--auto-select` keeps in a profile's `user.js`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const BEGIN: &str = "// claude-monitor begin";
pub const END: &str = "// claude-monitor end";
const PREF: &str = r#"user_pref("security.default_personal_cert", "Select Automatically");"#;

#[derive(Debug, thiserror::Error)]
pub enum UserJsError {
    #[error("{path} has `{BEGIN}` with no `{END}`; fix it by hand")]
    Unterminated { path: PathBuf },
    #[error("cannot {operation} {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
}

fn block() -> String {
    format!("{BEGIN}\n{PREF}\n{END}\n")
}

/// `existing` with the block inserted at the end, or replaced where it
/// already is. Everything outside the markers is returned byte for byte.
/// `None` when a begin marker has no end marker.
pub fn with_block(existing: &str) -> Option<String> {
    let lines: Vec<&str> = existing.split_inclusive('\n').collect();
    let Some(begin) = lines.iter().position(|l| l.trim_end() == BEGIN) else {
        let mut out = existing.to_owned();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&block());
        return Some(out);
    };
    let (end, _) = lines
        .iter()
        .enumerate()
        .skip(begin.saturating_add(1))
        .find(|(_, l)| l.trim_end() == END)?;
    let before: String = lines.iter().take(begin).copied().collect();
    let after: String = lines.iter().skip(end.saturating_add(1)).copied().collect();
    Some(format!("{before}{}{after}", block()))
}

/// Whether [`apply`] would change the file.
pub fn needs_update(path: &Path) -> Result<bool, UserJsError> {
    let existing = read(path)?;
    Ok(with_block(&existing).ok_or_else(|| unterminated(path))? != existing)
}

/// Writes the block into `path` (created if missing), leaving the rest of
/// the file as it was. Does nothing when it is already current.
pub fn apply(path: &Path) -> Result<bool, UserJsError> {
    let existing = read(path)?;
    let updated = with_block(&existing).ok_or_else(|| unterminated(path))?;
    if updated == existing {
        return Ok(false);
    }
    let io_err = |operation, source| UserJsError::Io {
        operation,
        path: path.to_owned(),
        source,
    };
    let mut staging = path.as_os_str().to_owned();
    staging.push(".claude-monitor-new");
    let staging = PathBuf::from(staging);
    fs::write(&staging, updated).map_err(|e| io_err("write", e))?;
    if let Ok(meta) = fs::metadata(path) {
        let _ = fs::set_permissions(&staging, meta.permissions());
    }
    fs::rename(&staging, path).map_err(|e| {
        let _ = fs::remove_file(&staging);
        io_err("replace", e)
    })?;
    Ok(true)
}

fn unterminated(path: &Path) -> UserJsError {
    UserJsError::Unterminated {
        path: path.to_owned(),
    }
}

fn read(path: &Path) -> Result<String, UserJsError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(source) => Err(UserJsError::Io {
            operation: "read",
            path: path.to_owned(),
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use tempfile::TempDir;

    const USER_LINES: &str = "user_pref(\"browser.startup.page\", 3);\n// my note\n";

    #[test]
    fn an_empty_file_gets_just_the_block() {
        assert_eq!(with_block("").unwrap(), block());
        assert!(block().contains("Select Automatically"));
    }

    #[test]
    fn the_block_is_appended_and_the_rest_is_untouched() {
        let got = with_block(USER_LINES).unwrap();
        assert!(got.starts_with(USER_LINES), "{got}");
        assert!(got.ends_with(&block()), "{got}");
    }

    #[test]
    fn a_file_without_a_final_newline_still_gets_the_block_on_its_own_lines() {
        let got = with_block("user_pref(\"a\", 1);").unwrap();
        assert_eq!(got, format!("user_pref(\"a\", 1);\n{}", block()));
    }

    #[test]
    fn an_existing_block_is_replaced_in_place() {
        let stale = format!("{USER_LINES}{BEGIN}\nuser_pref(\"old\", true);\n{END}\n// after\n");
        let got = with_block(&stale).unwrap();
        assert_eq!(got, format!("{USER_LINES}{}// after\n", block()));
    }

    #[test]
    fn applying_twice_changes_nothing_the_second_time() {
        let once = with_block(USER_LINES).unwrap();
        assert_eq!(with_block(&once).unwrap(), once);
    }

    #[test]
    fn an_unterminated_block_is_refused_not_guessed_at() {
        assert_eq!(
            with_block(&format!("{BEGIN}\nuser_pref(\"x\", 1);\n")),
            None
        );
    }

    #[test]
    fn apply_creates_updates_and_then_reports_no_change() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("user.js");
        assert!(needs_update(&path).unwrap());

        assert!(apply(&path).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), block());
        assert!(!needs_update(&path).unwrap());
        assert!(!apply(&path).unwrap());
        assert_eq!(
            fs::read_dir(dir.path()).unwrap().count(),
            1,
            "no staging file left"
        );
    }

    #[test]
    fn apply_leaves_an_unterminated_file_alone() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("user.js");
        let broken = format!("{BEGIN}\nuser_pref(\"x\", 1);\n");
        fs::write(&path, &broken).unwrap();

        let err = apply(&path).unwrap_err();

        assert!(matches!(err, UserJsError::Unterminated { .. }), "{err}");
        assert_eq!(fs::read_to_string(&path).unwrap(), broken);
    }
}
