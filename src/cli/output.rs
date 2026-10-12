//! What a command returns on success, rendered for humans or for `--json`.

#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    Text(String),
    Json(serde_json::Value),
}

impl Output {
    /// Nothing to print (for example `hook`, which never writes stdout).
    pub fn empty() -> Self {
        Self::Text(String::new())
    }

    /// The text to print, newline-terminated. `Json` outputs print compact
    /// in `--json` mode and pretty otherwise; `Text` is wrapped as
    /// `{"ok":true,"output":"..."}` in `--json` mode so stdout always parses.
    /// Empty text prints nothing in either mode.
    pub fn render(&self, json: bool) -> String {
        match self {
            Self::Text(text) if text.is_empty() => String::new(),
            Self::Text(text) if json => {
                let value = serde_json::json!({ "ok": true, "output": text });
                format!("{value}\n")
            }
            Self::Text(text) if text.ends_with('\n') => text.clone(),
            Self::Text(text) => format!("{text}\n"),
            Self::Json(value) if json => format!("{value}\n"),
            Self::Json(value) => {
                let pretty = serde_json::to_string_pretty(value).unwrap_or_default();
                format!("{pretty}\n")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn text_prints_with_one_trailing_newline() {
        assert_eq!(Output::Text("hi".into()).render(false), "hi\n");
        assert_eq!(Output::Text("hi\n".into()).render(false), "hi\n");
    }

    #[test]
    fn text_in_json_mode_is_wrapped_and_parses() {
        let rendered = Output::Text("hi".into()).render(true);
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value, serde_json::json!({"ok": true, "output": "hi"}));
    }

    #[test]
    fn json_is_compact_in_json_mode_and_pretty_otherwise() {
        let out = Output::Json(serde_json::json!({"sessions": []}));
        assert_eq!(out.render(true), "{\"sessions\":[]}\n");
        assert_eq!(out.render(false), "{\n  \"sessions\": []\n}\n");
    }

    #[test]
    fn empty_prints_nothing_in_both_modes() {
        assert_eq!(Output::empty().render(true), "");
        assert_eq!(Output::empty().render(false), "");
    }
}
