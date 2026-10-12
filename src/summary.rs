//! Session summaries: the verbatim "task" and "left off" text taken from the
//! session itself, and the on-demand model-written summary kept apart from it
//! (FR-017, FR-017a, FR-020).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

/// Most bytes read from the end of a transcript when looking for the latest
/// assistant message; a transcript can run to many megabytes and a hook has a
/// 50 ms budget.
pub const TRANSCRIPT_TAIL_BYTES: u64 = 256 * 1024;

/// The latest assistant message in the last `max_bytes` of the transcript at
/// `path`, text only and untouched. `None` when there is none in that window or
/// the file cannot be read.
pub fn transcript_tail_message(path: &Path, max_bytes: u64) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    // A FIFO or device would block the hook.
    if !meta.is_file() {
        return None;
    }
    let start = meta.len().saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.take(max_bytes).read_to_end(&mut buf).ok()?;

    // A window that starts mid-file starts mid-line; drop that partial line.
    let window = if start > 0 {
        let newline = buf.iter().position(|b| *b == b'\n')?;
        buf.get(newline.saturating_add(1)..)?
    } else {
        buf.as_slice()
    };
    window.split(|b| *b == b'\n').rev().find_map(assistant_text)
}

fn assistant_text(line: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(line).ok()?;
    if value.get("isSidechain").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let message = value.get("message")?;
    let is_assistant = value.get("type").and_then(Value::as_str) == Some("assistant")
        || message.get("role").and_then(Value::as_str) == Some("assistant");
    if !is_assistant {
        return None;
    }
    let text = match message.get("content")? {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    (!text.trim().is_empty()).then_some(text)
}

/// The "left off" text for a `Stop`: the payload's message when it has one,
/// else the transcript tail.
pub fn left_off(stop_message: Option<&str>, transcript_path: Option<&Path>) -> Option<String> {
    match stop_message {
        Some(message) => Some(message.to_owned()),
        None => {
            transcript_path.and_then(|path| transcript_tail_message(path, TRANSCRIPT_TAIL_BYTES))
        }
    }
}
