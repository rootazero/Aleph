//! JSON Schema generation for Aleph configuration.

use crate::config::Config;
use schemars::{schema::RootSchema, schema_for};

/// Generate JSON Schema for the main Config struct.
pub fn generate_config_schema() -> RootSchema {
    schema_for!(Config)
}

/// Generate JSON Schema as a serde_json::Value.
pub fn generate_config_schema_json() -> serde_json::Value {
    let schema = generate_config_schema();
    serde_json::to_value(schema).expect("Schema serialization should not fail")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_generation() {
        let schema = generate_config_schema();
        assert!(schema.schema.metadata.is_some());

        let json = generate_config_schema_json();
        assert!(json.is_object());
        assert!(json.get("$schema").is_some());
    }

    #[test]
    fn test_schema_has_definitions() {
        let schema = generate_config_schema();
        // Should have definitions for nested types
        assert!(!schema.definitions.is_empty());
    }
}
