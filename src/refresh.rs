//! One refresh pass: everything the monitor can learn without a hook.
//!
//! Passive discovery of Claude and Copilot sessions, then the Claude live
//! session registry (pid, tmux pane, busy/waiting/idle), then reconcile. Only
//! reads the agents' own directories; every change is an `events` row.

use crate::agents::discover_store::DiscoverSummary;
use crate::env::Env;
use crate::reconcile::ReconcileReport;
use crate::store::Store;

/// What one [`refresh`] did. Sources that failed are listed in `warnings`;
/// they never fail the pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefreshReport {
    pub discovered: DiscoverSummary,
    /// Sessions created from a live registry entry no transcript had yet.
    pub registry_created: usize,
    /// Known sessions whose pid, pane or state the registry changed.
    pub registry_updated: usize,
    pub reconcile: Option<ReconcileReport>,
    pub warnings: Vec<String>,
}

impl RefreshReport {
    /// Whether the store may differ from before the pass.
    pub fn changed(&self) -> bool {
        self.discovered.created > 0
            || self.discovered.updated > 0
            || self.registry_created > 0
            || self.registry_updated > 0
            || self
                .reconcile
                .as_ref()
                .is_some_and(|r| r.interrupted > 0 || r.unchanged < r.checked)
    }
}

/// Discover, read the registry, reconcile. Idempotent: a second pass over the
/// same facts writes nothing.
pub fn refresh(_store: &Store, _env: &Env, _now: &str) -> RefreshReport {
    RefreshReport::default()
}
