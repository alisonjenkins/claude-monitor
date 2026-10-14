//! Sanitisers for text derived from untrusted data (hook payloads, git
//! branches, plan files, notes) before it reaches a terminal or notification.
//! Text mode only: `--json` escapes control characters already, and stored or
//! compared values are never rewritten.

/// `text` without control characters (ESC, BEL, C1), line/paragraph
/// separators and bidi overrides, so session text cannot drive the terminal or
/// reorder what is shown; `\n` and `\t` stay for multi-line fields.
pub fn strip_control(text: &str) -> String {
    text.chars()
        .filter(|c| {
            matches!(c, '\n' | '\t')
                || !(c.is_control()
                    || matches!(c, '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        })
        .collect()
}

/// `text` as one line: control characters stripped, every run of whitespace
/// (newline and tab included) collapsed to a single space.
pub fn single_line(text: &str) -> String {
    strip_control(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `text` for a multi-line field: control characters stripped, tabs turned to
/// spaces and every line after the first indented, so a continuation line
/// cannot pass for a `label  value` line.
pub fn indent_continuations(text: &str, indent: usize) -> String {
    let pad = format!("\n{}", " ".repeat(indent));
    strip_control(text).replace('\t', " ").replace('\n', &pad)
}
