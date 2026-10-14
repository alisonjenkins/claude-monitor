//! The one error type commands return, and its exit-code mapping
//! (`contracts/cli.md`): 0 success, 1 usage/config, 2 refused, 3 partial.

use std::fmt::Display;
use std::path::PathBuf;

use crate::text::{single_line, strip_control};

pub const EXIT_OK: u8 = 0;
pub const EXIT_USAGE: u8 = 1;
pub const EXIT_REFUSED: u8 = 2;
pub const EXIT_PARTIAL: u8 = 3;

fn context(operation: &str, path: Option<&PathBuf>, detail: &dyn Display) -> String {
    match path {
        Some(path) => format!("{operation} {}: {detail}", path.display()),
        None => format!("{operation}: {detail}"),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Bad arguments or flags (exit 1).
    #[error("usage: {0}")]
    Usage(String),
    /// Unusable configuration (exit 1).
    #[error("config: {0}")]
    Config(String),
    /// A precondition failed and nothing was changed (exit 2). `code` is a
    /// stable machine-readable reason such as `worktree_missing`.
    #[error("{code}: {message}")]
    Refused { code: &'static str, message: String },
    /// Some items succeeded and some failed (exit 3).
    #[error("{operation}: {detail}")]
    Partial { operation: String, detail: String },
    #[error("{}", context(.operation, .path.as_ref(), .source))]
    Io {
        operation: String,
        path: Option<PathBuf>,
        source: std::io::Error,
    },
    #[error("{}", context(.operation, .path.as_ref(), .detail))]
    Store {
        operation: String,
        path: Option<PathBuf>,
        detail: String,
    },
    /// The command is parsed but its task has not landed yet (exit 1).
    #[error("{0}: not implemented yet")]
    NotImplemented(String),
}

impl Error {
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Usage(_) | Self::Config(_) | Self::NotImplemented(_) => EXIT_USAGE,
            Self::Refused { .. } => EXIT_REFUSED,
            Self::Partial { .. } => EXIT_PARTIAL,
            // Operational failures are not usage errors, but the contract has
            // no other code; 1 is the generic failure.
            Self::Io { .. } | Self::Store { .. } => EXIT_USAGE,
        }
    }

    /// The error as printed: `{"ok":false,"error":"<operation>: <detail>"}`
    /// in `--json` mode, otherwise one `claude-monitor: ...` line.
    pub fn render(&self, json: bool) -> String {
        if json {
            let value = serde_json::json!({ "ok": false, "error": self.to_string() });
            format!("{value}\n")
        } else {
            // Paths and details in these two come from untrusted data and
            // must stay on this one line; the other variants may span lines.
            let message = match self {
                Self::Io { .. } | Self::Store { .. } => single_line(&self.to_string()),
                other => strip_control(&other.to_string()),
            };
            format!("claude-monitor: {message}\n")
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn all_variants() -> Vec<(Error, u8)> {
        vec![
            (Error::Usage("bad flag".into()), 1),
            (Error::Config("no endpoint".into()), 1),
            (
                Error::Refused {
                    code: "worktree_missing",
                    message: "/w is gone".into(),
                },
                2,
            ),
            (
                Error::Partial {
                    operation: "respawn".into(),
                    detail: "2 of 5 failed".into(),
                },
                3,
            ),
            (
                Error::Io {
                    operation: "read file".into(),
                    path: Some("/x".into()),
                    source: std::io::Error::from(std::io::ErrorKind::NotFound),
                },
                1,
            ),
            (
                Error::Store {
                    operation: "open store".into(),
                    path: None,
                    detail: "locked".into(),
                },
                1,
            ),
            (Error::NotImplemented("serve".into()), 1),
        ]
    }

    #[test]
    fn exit_codes_follow_the_contract() {
        for (error, code) in all_variants() {
            assert_eq!(error.exit_code(), code, "{error}");
        }
    }

    #[test]
    fn display_carries_operation_path_and_detail() {
        let io = Error::Io {
            operation: "read file".into(),
            path: Some("/x/y".into()),
            source: std::io::Error::other("boom"),
        };
        assert_eq!(io.to_string(), "read file /x/y: boom");
        let store = Error::Store {
            operation: "open store".into(),
            path: None,
            detail: "locked".into(),
        };
        assert_eq!(store.to_string(), "open store: locked");
        assert_eq!(
            Error::NotImplemented("serve".into()).to_string(),
            "serve: not implemented yet"
        );
    }

    #[test]
    fn json_rendering_has_the_ok_false_shape_for_every_variant() {
        for (error, _) in all_variants() {
            let rendered = error.render(true);
            assert!(rendered.ends_with('\n'));
            let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
            assert_eq!(value.get("ok"), Some(&serde_json::Value::Bool(false)));
            assert_eq!(
                value.get("error").and_then(|v| v.as_str()),
                Some(error.to_string().as_str())
            );
            assert_eq!(value.as_object().unwrap().len(), 2);
        }
    }

    #[test]
    fn text_rendering_is_one_prefixed_line() {
        assert_eq!(
            Error::Usage("bad".into()).render(false),
            "claude-monitor: usage: bad\n"
        );
    }
}
