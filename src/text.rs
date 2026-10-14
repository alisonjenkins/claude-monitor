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

/// `word` as one bash word, safe to print on a single line and to paste: bare
/// when it holds only characters a shell never treats specially, in single
/// quotes (verbatim, spaces kept) when it is printable, and in `$'...'` with
/// escapes when it holds anything [`is_terminal_unsafe`].
pub fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c));
    if plain {
        word.to_owned()
    } else if word.chars().any(is_terminal_unsafe) {
        ansi_c_quoted(word)
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// `text` exactly as written when it is clean, else its `$'...'` form, for a
/// path shown to the user rather than copied.
pub fn inert(text: &str) -> String {
    if text.chars().any(is_terminal_unsafe) {
        ansi_c_quoted(text)
    } else {
        text.to_owned()
    }
}

/// Bash `$'...'`: backslash, quote, newline, tab and every unsafe character
/// escaped. Non-ASCII ones become their UTF-8 bytes as `\xNN`, which needs no
/// UTF-8 locale to read back.
fn ansi_c_quoted(text: &str) -> String {
    let mut out = String::from("$'");
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if is_terminal_unsafe(c) => {
                let mut buf = [0u8; 4];
                for byte in c.encode_utf8(&mut buf).bytes() {
                    out.push_str(&format!("\\x{byte:02x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('\'');
    out
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
    fn shell_word_quotes_by_what_the_word_holds() {
        let cases = [
            ("plain-1.2/x", "plain-1.2/x"),
            ("", "''"),
            ("a  b", "'a  b'"),
            ("it's", "'it'\\''s'"),
            ("a\tb", "$'a\\tb'"),
            ("a\nb", "$'a\\nb'"),
            ("a\u{1b}[2J", "$'a\\x1b[2J'"),
            ("a\u{7f}", "$'a\\x7f'"),
            ("a\u{85}", "$'a\\xc2\\x85'"),
            ("a\u{2028}", "$'a\\xe2\\x80\\xa8'"),
            ("a\u{202e}", "$'a\\xe2\\x80\\xae'"),
            ("a'\\\n", "$'a\\'\\\\\\n'"),
        ];
        for (word, expected) in cases {
            assert_eq!(shell_word(word), expected, "{word:?}");
        }
    }

    #[test]
    fn inert_keeps_clean_text_and_escapes_the_rest() {
        assert_eq!(inert("/work/a  b"), "/work/a  b");
        assert_eq!(inert("/work/a\tb"), "$'/work/a\\tb'");
    }

    #[test]
    fn indent_continuations_pads_later_lines_and_turns_tabs_to_spaces() {
        assert_eq!(indent_continuations(HOSTILE, 2), "a[2J b\n  cde");
    }
}
