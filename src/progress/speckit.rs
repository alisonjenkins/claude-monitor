//! Parser for spec-kit `tasks.md` files.

use super::Progress;

/// `None` when the text holds no `T###` task lines.
pub fn parse(_text: &str, _path: Option<&str>) -> Option<Progress> {
    None
}
