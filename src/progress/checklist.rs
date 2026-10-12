//! Parser for plain Markdown checklists.

use super::Progress;

/// `None` when the text holds no checkbox lines.
pub fn parse(_text: &str, _path: Option<&str>) -> Option<Progress> {
    None
}
