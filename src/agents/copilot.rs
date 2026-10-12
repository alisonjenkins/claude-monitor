use serde_json::Value;

use super::{AgentKind, ParseError, ParsedHook};

/// GitHub Copilot CLI: hook payloads on stdin.
#[derive(Debug, Clone, Copy, Default)]
pub struct CopilotKind;

impl AgentKind for CopilotKind {
    fn id(&self) -> &'static str {
        "copilot"
    }

    fn parse_hook(&self, _raw: &Value) -> Result<Vec<ParsedHook>, ParseError> {
        Ok(Vec::new())
    }
}
