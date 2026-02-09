#[cfg(test)]
mod tests {
    use crate::exec::sandbox::presets::PresetRegistry;
    use crate::exec::sandbox::parameter_binding::{CapabilityOverrides, ParameterBinding, ValidationRule, MappingType};
    use crate::exec::sandbox::capability_resolver::apply_overrides;
    use crate::exec::sandbox::capabilities::NetworkCapability;
    use crate::skill_evolution::sandbox_integration::resolve_tool_capabilities;
    use crate::skill_evolution::tool_generator::{GeneratedToolDefinition, GenerationMetadata};
    use std::collections::HashMap;

    #[test]
    fn test_end_to_end_capability_resolution() {
        // Create a tool definition with capabilities
        let tool_def = create_test_tool_definition();

        // Create test parameters (empty for this test)
        let params = serde_json::json!({});

        // Resolve capabilities
        let result = resolve_tool_capabilities(&tool_def, &params);

        // Should succeed
        assert!(result.is_ok());
        let caps = result.unwrap();

        // Should have temp workspace from preset
        assert!(!caps.filesystem.is_empty());
    }

    #[test]
    fn test_preset_immutability_enforcement() {
        let registry = PresetRegistry::default();
        let preset = registry.get("file_processor").unwrap();

        // Try to override network (immutable)
        let mut overrides = CapabilityOverrides::default();
        overrides.network = Some(NetworkCapability::AllowAll);

        let result = apply_overrides(
            preset.capabilities.clone(),
            &overrides,
            &preset.immutable_fields,
        );

        // Should fail
        assert!(result.is_err());
    }

    fn create_test_tool_definition() -> GeneratedToolDefinition {
        use crate::exec::sandbox::parameter_binding::RequiredCapabilities;

        let bindings = HashMap::new();

        GeneratedToolDefinition {
            name: "test_tool".to_string(),
            description: "Test tool".to_string(),
            input_schema: serde_json::json!({}),
            runtime: "python".to_string(),
            entrypoint: "entrypoint.py".to_string(),
            self_tested: false,
            requires_confirmation: true,
            required_capabilities: Some(RequiredCapabilities {
                base_preset: "file_processor".to_string(),
                description: "Test capabilities".to_string(),
                overrides: Default::default(),
                parameter_bindings: bindings,
            }),
            generated: GenerationMetadata {
                pattern_id: "test".to_string(),
                confidence: 0.9,
                generated_at: 0,
                generator_version: "1.0".to_string(),
            },
        }
    }
}
