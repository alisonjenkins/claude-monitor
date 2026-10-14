//! Sanitisers for text derived from untrusted data (hook payloads, git
//! branches, plan files, notes) before it reaches a terminal or notification.
//! Text mode only: `--json` escapes control characters already, and stored or
//! compared values are never rewritten.

/// `text` without control characters (ESC, BEL, C1), line/paragraph
/// separators and bidi overrides, so session text cannot drive the terminal or
/// reorder what is shown; `\n` and `\t` stay for multi-line fields.
pub fn strip_control(text: &str) -> String {
    text.chars()
        .filter(|&c| matches!(c, '\n' | '\t') || !is_terminal_unsafe(c))
        .collect()
}

/// True for a character that can drive a terminal or reorder what it shows:
/// control characters (including `\n` and `\t`), line/paragraph separators and
/// bidi overrides.
pub fn is_terminal_unsafe(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
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

#[cfg(test)]
mod tests {
    use super::*;

    const HOSTILE: &str = "a\u{1b}[2J\tb\n\u{85}c\u{2028}d\u{202e}e\u{7f}";

    #[test]
    fn strip_control_keeps_newline_and_tab_only() {
        assert_eq!(strip_control(HOSTILE), "a[2J\tb\ncde");
    }

    #[test]
    fn single_line_collapses_all_whitespace() {
        assert_eq!(single_line(HOSTILE), "a[2J b cde");
        assert_eq!(single_line("  x \n\t y  "), "x y");
    }

    #[test]
    fn indent_continuations_pads_later_lines_and_turns_tabs_to_spaces() {
        assert_eq!(indent_continuations(HOSTILE, 2), "a[2J b\n  cde");
    }
}
