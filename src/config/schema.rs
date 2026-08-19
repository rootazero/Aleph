//! JSON Schema generation for Aleph configuration.

use crate::config::Config;
use schemars::{schema_for, Schema};

/// Generate JSON Schema for the main Config struct.
///
/// schemars 1.x emits a draft 2020-12 schema: `Schema` is a transparent wrapper
/// over `serde_json::Value`, and nested types live under `$defs` (not `definitions`).
pub(crate) fn generate_config_schema() -> Schema {
    schema_for!(Config)
}

/// Generate JSON Schema as a `serde_json::Value`.
///
/// The schema is derived from the same `Config` type the validator consumes,
/// so a serialization failure here means the type itself is unsound: panicking
/// is the only honest response. A "silently accept everything" sentinel (`{}`)
/// would disable the Panel's client-side edit validation without a trace.
pub fn generate_config_schema_json() -> serde_json::Value {
    let schema = generate_config_schema();
    serde_json::to_value(&schema).unwrap_or_else(|e| {
        // Unreachable invariant: `Schema` is a transparent wrapper over
        // `serde_json::Value` with string-only keys, so `to_value` cannot
        // fail. It can only error on non-string map keys, which the
        // generated schema never contains.
        unreachable!(
            "Config schema serialization failed — the schema is generated from the \
             same `Config` struct the validator consumes, so this is a type-system \
             soundness failure, not a recoverable error: {e}"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_generation() {
        let json = generate_config_schema_json();
        assert!(json.is_object());
        assert!(json.get("$schema").is_some());
    }

    #[test]
    fn test_schema_has_definitions() {
        let json = generate_config_schema_json();
        // schemars 1.x (draft 2020-12) places nested types under `$defs`.
        let defs = json.get("$defs").and_then(|d| d.as_object());
        assert!(defs.is_some_and(|d| !d.is_empty()));
    }
}
