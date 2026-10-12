//! Parser for an agent todo-list snapshot.

use super::Progress;

/// `None` for absent or unrecognised payloads.
pub fn parse(_json: &str) -> Option<Progress> {
    None
}
