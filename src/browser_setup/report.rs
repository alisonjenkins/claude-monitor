//! What `setup-browser` did to each target, for people and for `--json`.

use std::path::PathBuf;

use serde_json::{json, Value};

use super::discover::Target;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Imported {
        ca: bool,
        client: bool,
        auto_select: bool,
        backup: Option<PathBuf>,
    },
    /// `--dry-run`: the same flags, nothing written.
    WouldImport {
        ca: bool,
        client: bool,
        auto_select: bool,
    },
    UpToDate,
    /// The profile has not created its certificate database yet.
    NoDatabase,
    Skipped(String),
    InUse(String),
    Failed(String),
}

impl Outcome {
    /// Everything was done or there was nothing to do.
    pub fn is_fine(&self) -> bool {
        matches!(
            self,
            Self::Imported { .. } | Self::WouldImport { .. } | Self::UpToDate | Self::NoDatabase
        )
    }

    fn key(&self) -> &'static str {
        match self {
            Self::Imported { .. } => "imported",
            Self::WouldImport { .. } => "would_import",
            Self::UpToDate => "up_to_date",
            Self::NoDatabase => "no_database",
            Self::Skipped(_) => "skipped",
            Self::InUse(_) => "refused_in_use",
            Self::Failed(_) => "failed",
        }
    }

    fn changes(ca: bool, client: bool, auto_select: bool) -> String {
        let parts: Vec<&str> = [
            (ca, "CA"),
            (client, "client certificate"),
            (auto_select, "auto-select"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect();
        parts.join(", ")
    }

    pub fn text(&self) -> String {
        match self {
            Self::Imported {
                ca,
                client,
                auto_select,
                backup,
            } => {
                let mut line = format!("imported ({})", Self::changes(*ca, *client, *auto_select));
                if let Some(backup) = backup {
                    line.push_str(&format!("; backup in {}", backup.display()));
                }
                line
            }
            Self::WouldImport {
                ca,
                client,
                auto_select,
            } => format!(
                "would import ({})",
                Self::changes(*ca, *client, *auto_select)
            ),
            Self::UpToDate => "already up to date".to_owned(),
            Self::NoDatabase => {
                "skipped: no certificate database yet; start the browser once".to_owned()
            }
            Self::Skipped(reason) => format!("skipped: {reason}"),
            Self::InUse(reason) => {
                format!("refused: in use ({reason}); close the browser or pass --force")
            }
            Self::Failed(detail) => format!("failed: {detail}"),
        }
    }

    fn detail(&self) -> Value {
        match self {
            Self::Imported {
                ca,
                client,
                auto_select,
                backup,
            } => json!({
                "ca": ca, "client": client, "auto_select": auto_select,
                "backup": backup.as_ref().map(|b| b.display().to_string()),
            }),
            Self::WouldImport {
                ca,
                client,
                auto_select,
            } => json!({ "ca": ca, "client": client, "auto_select": auto_select }),
            Self::Skipped(reason) | Self::InUse(reason) | Self::Failed(reason) => {
                json!({ "reason": reason })
            }
            Self::UpToDate | Self::NoDatabase => json!({}),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetReport {
    pub target: Target,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    pub targets: Vec<TargetReport>,
    pub dry_run: bool,
    /// Lines printed after the per-target ones.
    pub notes: Vec<String>,
    /// The Chromium policy instructions, when asked for.
    pub chromium_policy: Option<String>,
}

impl Report {
    pub fn all_fine(&self) -> bool {
        self.targets.iter().all(|t| t.outcome.is_fine())
    }

    pub fn nothing_changed(&self) -> bool {
        self.targets
            .iter()
            .all(|t| matches!(t.outcome, Outcome::UpToDate))
    }

    pub fn text(&self) -> String {
        let mut lines: Vec<String> = self
            .targets
            .iter()
            .map(|t| {
                format!(
                    "{} {} ({}): {}",
                    t.target.browser.label(),
                    t.target.name,
                    t.target.dir.display(),
                    t.outcome.text()
                )
            })
            .collect();
        if self.nothing_changed() {
            lines.push("nothing to do".to_owned());
        }
        lines.extend(self.notes.iter().cloned());
        if let Some(policy) = &self.chromium_policy {
            lines.push(policy.clone());
        }
        lines.join("\n")
    }

    pub fn json(&self) -> Value {
        let targets: Vec<Value> = self
            .targets
            .iter()
            .map(|t| {
                json!({
                    "browser": t.target.browser.label(),
                    "name": t.target.name,
                    "dir": t.target.dir.display().to_string(),
                    "status": t.outcome.key(),
                    "detail": t.outcome.detail(),
                })
            })
            .collect();
        json!({
            "dry_run": self.dry_run,
            "ok": self.all_fine(),
            "targets": targets,
            "notes": self.notes,
            "chromium_policy": self.chromium_policy,
        })
    }
}
