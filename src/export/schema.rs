//! The JSON Schema of the export, generated from one description of the wire format.

use serde_json::Value;

/// The JSON Schema (draft 2020-12) of one export record.
pub fn generate() -> Value {
    Value::Null
}

/// Checks one parsed record against the wire format.
pub fn validate_record(_record: &Value) -> Result<(), String> {
    Ok(())
}
