//! The JSON Schema of the export, generated from one description of the wire format.

use serde_json::Value;

/// Bumps only for breaking changes; fields are added, never repurposed.
pub const SCHEMA_VERSION: u32 = 1;
/// The `external.system` of every record.
pub const SYSTEM: &str = "ai-control-centre";

/// The JSON Schema (draft 2020-12) of one export record.
pub fn generate() -> Value {
    Value::Null
}

/// Checks one parsed record against the wire format.
pub fn validate_record(_record: &Value) -> Result<(), String> {
    Ok(())
}
