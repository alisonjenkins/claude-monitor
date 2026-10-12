use super::{AgentKind, ParseError, ParsedHook};

/// Claude Code: hook payloads on stdin, `claude --resume <id>`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClaudeKind;

impl AgentKind for ClaudeKind {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn parse_hook(&self, _raw: &serde_json::Value) -> Result<Vec<ParsedHook>, ParseError> {
        Ok(Vec::new())
    }
}
