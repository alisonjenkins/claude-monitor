//! Session summaries: the verbatim "task" and "left off" text taken from the
//! session itself, and the on-demand model-written summary kept apart from it
//! (FR-017, FR-017a, FR-020).

use std::path::Path;

/// Most bytes read from the end of a transcript when looking for the latest
/// assistant message.
pub const TRANSCRIPT_TAIL_BYTES: u64 = 256 * 1024;

/// The latest assistant message in the last `max_bytes` of the transcript at
/// `path`, text only and untouched. `None` when there is none in that window or
/// the file cannot be read.
pub fn transcript_tail_message(_path: &Path, _max_bytes: u64) -> Option<String> {
    None
}

/// The "left off" text for a `Stop`: the payload's message when it has one,
/// else the transcript tail.
pub fn left_off(stop_message: Option<&str>, _transcript_path: Option<&Path>) -> Option<String> {
    stop_message.map(str::to_owned)
}
