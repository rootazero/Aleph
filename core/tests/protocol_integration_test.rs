// core/tests/protocol_integration_test.rs

//! Integration tests for the configurable protocol system
//!
//! These tests verify the entire protocol system works end-to-end:
//! - YAML parsing (ProtocolDefinition deserialization)
//! - ConfigurableProtocol creation
//! - ProtocolRegistry registration
//! - Provider factory integration (create_provider)
//! - Protocol methods work (build_request, parse_response)

use alephcore::ProviderConfig;
use alephcore::providers::adapter::{ProtocolAdapter, RequestPayload};
use alephcore::providers::protocols::{
    ConfigurableProtocol, ProtocolDefinition, ProtocolRegistry,
};
use alephcore::providers::create_provider;
use std::sync::Arc;
use tempfile::TempDir;

/// Initialize the protocol registry with built-in protocols
fn init_registry() {
    let registry = ProtocolRegistry::global();
    if registry.list_protocols().is_empty() {
        registry.register_builtin();
    }
}

#[tokio::test]
async fn test_end_to_end_minimal_protocol() {
    init_registry();

    // 1. Load protocol from YAML
    let yaml = r#"
name: test-minimal
extends: openai
base_url: https://api.test.com
differences:
  auth:
    header: X-API-Key
    prefix: ""
"#;

    let def: ProtocolDefinition = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(def.name, "test-minimal");
    assert_eq!(def.extends, Some("openai".to_string()));

    // 2. Create ConfigurableProtocol
    let protocol = ConfigurableProtocol::new(def.clone(), reqwest::Client::new()).unwrap();
    assert_eq!(protocol.name(), "test-minimal");

    // 3. Register in ProtocolRegistry
    ProtocolRegistry::global()
        .register(def.name.clone(), Arc::new(protocol))
        .unwrap();

    // 4. Verify it's in the registry
    assert!(ProtocolRegistry::global().get("test-minimal").is_some());

    // 5. Create provider using the protocol
    let mut config = ProviderConfig::test_config("test-model");
    config.protocol = Some("test-minimal".to_string());
    config.api_key = Some("test-key".to_string());
    config.base_url = Some("https://api.test.com".to_string());

    let provider = create_provider("test-minimal", config);
    assert!(provider.is_ok());

    let provider = provider.unwrap();
    assert_eq!(provider.name(), "test-minimal");

    // 6. Verify protocol methods work (build_request)
    let protocol = ProtocolRegistry::global()
        .get("test-minimal")
        .expect("Protocol should be registered");

    let payload = RequestPayload::new("Hello, world!");
    let mut test_config = ProviderConfig::test_config("test-model");
    test_config.protocol = Some("test-minimal".to_string());
    test_config.api_key = Some("test-key".to_string());
    test_config.base_url = Some("https://api.test.com".to_string());

    let request = protocol.build_request(&payload, &test_config, false);
    assert!(request.is_ok(), "Should build request successfully");

    // Clean up
    ProtocolRegistry::global().unregister("test-minimal");
}

#[tokio::test]
async fn test_end_to_end_custom_protocol() {
    init_registry();

    // 1. Load custom protocol from YAML
    let yaml = r#"
name: test-custom
base_url: https://api.custom.com
custom:
  auth:
    type: header
    header: Authorization
    prefix: "Bearer "
  endpoints:
    chat: /v1/chat
  request_template: '{"model": "{{config.model}}", "messages": [{"role": "user", "content": "{{input}}"}]}'
  response_mapping:
    content: "$.choices[0].message.content"
"#;

    let def: ProtocolDefinition = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(def.name, "test-custom");
    assert!(def.custom.is_some());

    // 2. Create ConfigurableProtocol
    let protocol = ConfigurableProtocol::new(def.clone(), reqwest::Client::new()).unwrap();
    assert_eq!(protocol.name(), "test-custom");

    // 3. Register in ProtocolRegistry
    ProtocolRegistry::global()
        .register(def.name.clone(), Arc::new(protocol))
        .unwrap();

    // 4. Verify it's in the registry
    assert!(ProtocolRegistry::global().get("test-custom").is_some());

    // 5. Create provider using the protocol
    let mut config = ProviderConfig::test_config("custom-model");
    config.protocol = Some("test-custom".to_string());
    config.api_key = Some("custom-key".to_string());
    config.base_url = Some("https://api.custom.com".to_string());

    let provider = create_provider("test-custom", config);
    assert!(provider.is_ok());

    // 6. Verify protocol methods work
    let protocol = ProtocolRegistry::global()
        .get("test-custom")
        .expect("Protocol should be registered");

    let payload = RequestPayload::new("Test input");
    let mut test_config = ProviderConfig::test_config("custom-model");
    test_config.protocol = Some("test-custom".to_string());
    test_config.api_key = Some("custom-key".to_string());
    test_config.base_url = Some("https://api.custom.com".to_string());

    let request = protocol.build_request(&payload, &test_config, false);
    assert!(request.is_ok(), "Should build custom request successfully");

    // Clean up
    ProtocolRegistry::global().unregister("test-custom");
}

#[tokio::test]
async fn test_protocol_hot_reload_simulation() {
    use alephcore::providers::protocols::ProtocolLoader;

    init_registry();

    // Create a temporary directory
    let temp_dir = TempDir::new().unwrap();

    // 1. Load v1 protocol
    let file_path = temp_dir.path().join("reload-test.yaml");
    let yaml_v1 = r#"
name: reload-test
extends: openai
base_url: https://api.v1.com
differences:
  auth:
    header: X-API-Key
    prefix: "v1-"
"#;

    tokio::fs::write(&file_path, yaml_v1)
        .await
        .expect("Failed to write v1 YAML");

    ProtocolLoader::load_from_file(&file_path)
        .await
        .expect("Should load v1 protocol");

    // Verify v1 is loaded
    let protocol_v1 = ProtocolRegistry::global()
        .get("reload-test")
        .expect("v1 protocol should be registered");
    assert_eq!(protocol_v1.name(), "reload-test");

    // 2. Create provider with v1
    let mut config_v1 = ProviderConfig::test_config("model-v1");
    config_v1.protocol = Some("reload-test".to_string());
    config_v1.api_key = Some("key-v1".to_string());
    config_v1.base_url = Some("https://api.v1.com".to_string());

    let provider_v1 = create_provider("reload-test", config_v1);
    assert!(provider_v1.is_ok());

    // 3. Reload v2 protocol (simulate hot reload)
    let yaml_v2 = r#"
name: reload-test
extends: openai
base_url: https://api.v2.com
differences:
  auth:
    header: Authorization
    prefix: "Bearer "
"#;

    tokio::fs::write(&file_path, yaml_v2)
        .await
        .expect("Failed to write v2 YAML");

    ProtocolLoader::load_from_file(&file_path)
        .await
        .expect("Should reload v2 protocol");

    // 4. Verify v2 is now loaded
    let protocol_v2 = ProtocolRegistry::global()
        .get("reload-test")
        .expect("v2 protocol should be registered");
    assert_eq!(protocol_v2.name(), "reload-test");

    // 5. Create provider with v2
    let mut config_v2 = ProviderConfig::test_config("model-v2");
    config_v2.protocol = Some("reload-test".to_string());
    config_v2.api_key = Some("key-v2".to_string());
    config_v2.base_url = Some("https://api.v2.com".to_string());

    let provider_v2 = create_provider("reload-test", config_v2);
    assert!(provider_v2.is_ok());

    // Clean up
    ProtocolRegistry::global().unregister("reload-test");
}

#[tokio::test]
async fn test_multiple_protocols_coexist() {
    init_registry();

    // Load multiple protocols
    let protocols = vec![
        (
            "proto-1",
            r#"
name: proto-1
extends: openai
base_url: https://api.proto1.com
"#,
        ),
        (
            "proto-2",
            r#"
name: proto-2
extends: anthropic
base_url: https://api.proto2.com
"#,
        ),
        (
            "proto-3",
            r#"
name: proto-3
extends: gemini
base_url: https://api.proto3.com
"#,
        ),
    ];

    for (name, yaml) in &protocols {
        let def: ProtocolDefinition = serde_yaml::from_str(yaml).unwrap();
        let protocol = ConfigurableProtocol::new(def.clone(), reqwest::Client::new()).unwrap();
        ProtocolRegistry::global()
            .register(name.to_string(), Arc::new(protocol))
            .unwrap();
    }

    // Verify all are registered
    for (name, _) in &protocols {
        assert!(
            ProtocolRegistry::global().get(name).is_some(),
            "Protocol {} should be registered",
            name
        );
    }

    // Create providers for each
    for (name, _) in &protocols {
        let mut config = ProviderConfig::test_config(&format!("{}-model", name));
        config.protocol = Some(name.to_string());
        config.api_key = Some(format!("{}-key", name));
        config.base_url = Some(format!("https://api.{}.com", name));

        let provider = create_provider(name, config);
        assert!(provider.is_ok(), "Should create provider for {}", name);
    }

    // Clean up
    for (name, _) in &protocols {
        ProtocolRegistry::global().unregister(name);
    }
}

#[tokio::test]
async fn test_protocol_load_from_directory() {
    use alephcore::providers::protocols::ProtocolLoader;

    init_registry();

    // Create a temporary directory with multiple protocol files
    let temp_dir = TempDir::new().unwrap();

    let protocols = vec![
        (
            "dir-proto-1.yaml",
            r#"
name: dir-proto-1
extends: openai
base_url: https://api.dir1.com
"#,
        ),
        (
            "dir-proto-2.yaml",
            r#"
name: dir-proto-2
extends: anthropic
base_url: https://api.dir2.com
"#,
        ),
        (
            "dir-proto-3.yml",
            r#"
name: dir-proto-3
extends: gemini
base_url: https://api.dir3.com
"#,
        ),
    ];

    for (filename, yaml) in &protocols {
        let file_path = temp_dir.path().join(filename);
        tokio::fs::write(&file_path, yaml)
            .await
            .expect("Failed to write protocol file");
    }

    // Load all protocols from directory
    ProtocolLoader::load_from_dir(temp_dir.path())
        .await
        .expect("Should load protocols from directory");

    // Verify all are registered
    assert!(ProtocolRegistry::global().get("dir-proto-1").is_some());
    assert!(ProtocolRegistry::global().get("dir-proto-2").is_some());
    assert!(ProtocolRegistry::global().get("dir-proto-3").is_some());

    // Clean up
    ProtocolRegistry::global().unregister("dir-proto-1");
    ProtocolRegistry::global().unregister("dir-proto-2");
    ProtocolRegistry::global().unregister("dir-proto-3");
}

#[tokio::test]
async fn test_invalid_protocol_handling() {
    init_registry();

    // Test 1: Invalid YAML
    let invalid_yaml = "invalid: yaml: [[[";
    let result: Result<ProtocolDefinition, _> = serde_yaml::from_str(invalid_yaml);
    assert!(result.is_err(), "Should fail to parse invalid YAML");

    // Test 2: Missing required fields (fails when trying to build request, not at creation)
    let incomplete_yaml = r#"
name: incomplete
# Missing extends or custom
"#;
    let def: ProtocolDefinition = serde_yaml::from_str(incomplete_yaml).unwrap();
    let protocol = ConfigurableProtocol::new(def, reqwest::Client::new()).unwrap();

    // Should fail when trying to build a request
    let mut config = ProviderConfig::test_config("model");
    config.api_key = Some("key".to_string());
    let payload = RequestPayload::new("test");
    let result = protocol.build_request(&payload, &config, false);
    assert!(result.is_err(), "Should fail to build request without extends or custom");

    // Test 3: Invalid extends reference
    let invalid_extends = r#"
name: invalid-extends
extends: non-existent-protocol
"#;
    let def: ProtocolDefinition = serde_yaml::from_str(invalid_extends).unwrap();
    let result = ConfigurableProtocol::new(def, reqwest::Client::new());
    assert!(result.is_err(), "Should fail with non-existent base protocol");

    // Test 4: Create provider with non-existent protocol
    let mut config = ProviderConfig::test_config("model");
    config.protocol = Some("non-existent".to_string());
    config.api_key = Some("key".to_string());
    let result = create_provider("test", config);
    assert!(result.is_err(), "Should fail with non-existent protocol");
}

#[tokio::test]
async fn test_protocol_with_auth_variations() {
    init_registry();

    // Test 1: No prefix
    let no_prefix_yaml = r#"
name: no-prefix
extends: openai
base_url: https://api.test.com
differences:
  auth:
    header: X-API-Key
    prefix: ""
"#;
    let def: ProtocolDefinition = serde_yaml::from_str(no_prefix_yaml).unwrap();
    let protocol = ConfigurableProtocol::new(def.clone(), reqwest::Client::new()).unwrap();
    ProtocolRegistry::global()
        .register(def.name.clone(), Arc::new(protocol))
        .unwrap();

    let mut config = ProviderConfig::test_config("model");
    config.protocol = Some("no-prefix".to_string());
    config.api_key = Some("key123".to_string());
    config.base_url = Some("https://api.test.com".to_string());

    let proto = ProtocolRegistry::global().get("no-prefix").unwrap();
    let payload = RequestPayload::new("test");
    let request = proto.build_request(&payload, &config, false);
    assert!(request.is_ok());

    // Test 2: With prefix
    let with_prefix_yaml = r#"
name: with-prefix
extends: openai
base_url: https://api.test.com
differences:
  auth:
    header: Authorization
    prefix: "Bearer "
"#;
    let def: ProtocolDefinition = serde_yaml::from_str(with_prefix_yaml).unwrap();
    let protocol = ConfigurableProtocol::new(def.clone(), reqwest::Client::new()).unwrap();
    ProtocolRegistry::global()
        .register(def.name.clone(), Arc::new(protocol))
        .unwrap();

    let mut config = ProviderConfig::test_config("model");
    config.protocol = Some("with-prefix".to_string());
    config.api_key = Some("key456".to_string());
    config.base_url = Some("https://api.test.com".to_string());

    let proto = ProtocolRegistry::global().get("with-prefix").unwrap();
    let payload = RequestPayload::new("test");
    let request = proto.build_request(&payload, &config, false);
    assert!(request.is_ok());

    // Clean up
    ProtocolRegistry::global().unregister("no-prefix");
    ProtocolRegistry::global().unregister("with-prefix");
}

#[tokio::test]
async fn test_custom_protocol_template_rendering() {
    init_registry();

    // Test custom protocol with complex template
    let yaml = r#"
name: template-test
base_url: https://api.template.com
custom:
  auth:
    type: header
    header: X-Custom-Auth
    prefix: "Custom "
  endpoints:
    chat: /v1/completions
  request_template: '{"model": "{{config.model}}", "prompt": "{{input}}", "max_tokens": {{config.max_tokens}}, "temperature": {{config.temperature}}}'
  response_mapping:
    content: "$.result.text"
"#;

    let def: ProtocolDefinition = serde_yaml::from_str(yaml).unwrap();
    let protocol = ConfigurableProtocol::new(def.clone(), reqwest::Client::new()).unwrap();
    ProtocolRegistry::global()
        .register(def.name.clone(), Arc::new(protocol))
        .unwrap();

    let mut config = ProviderConfig::test_config("test-model-v2");
    config.protocol = Some("template-test".to_string());
    config.api_key = Some("template-key".to_string());
    config.base_url = Some("https://api.template.com".to_string());
    config.max_tokens = Some(1024);
    config.temperature = Some(0.7);

    let proto = ProtocolRegistry::global().get("template-test").unwrap();
    let payload = RequestPayload::new("Translate this text");
    let request = proto.build_request(&payload, &config, false);
    assert!(
        request.is_ok(),
        "Should render template with all config values"
    );

    // Clean up
    ProtocolRegistry::global().unregister("template-test");
}

#[tokio::test]
async fn test_registry_list_protocols() {
    init_registry();

    // Get initial count (should have built-in protocols)
    let initial_protocols = ProtocolRegistry::global().list_protocols();
    assert!(
        initial_protocols.contains(&"openai".to_string()),
        "Should contain built-in openai"
    );
    assert!(
        initial_protocols.contains(&"anthropic".to_string()),
        "Should contain built-in anthropic"
    );
    assert!(
        initial_protocols.contains(&"gemini".to_string()),
        "Should contain built-in gemini"
    );

    // Add custom protocols
    let yaml = r#"
name: list-test
extends: openai
base_url: https://api.list.com
"#;
    let def: ProtocolDefinition = serde_yaml::from_str(yaml).unwrap();
    let protocol = ConfigurableProtocol::new(def.clone(), reqwest::Client::new()).unwrap();
    ProtocolRegistry::global()
        .register(def.name.clone(), Arc::new(protocol))
        .unwrap();

    // List should now include custom protocol
    let protocols = ProtocolRegistry::global().list_protocols();
    assert!(
        protocols.contains(&"list-test".to_string()),
        "Should contain custom protocol"
    );

    // Clean up
    ProtocolRegistry::global().unregister("list-test");

    // Verify it's removed
    let protocols = ProtocolRegistry::global().list_protocols();
    assert!(
        !protocols.contains(&"list-test".to_string()),
        "Should not contain unregistered protocol"
    );
}

#[tokio::test]
async fn test_protocol_override_base_url() {
    init_registry();

    // Test that provider config base_url takes precedence
    let yaml = r#"
name: url-override
extends: openai
base_url: https://api.default.com
"#;
    let def: ProtocolDefinition = serde_yaml::from_str(yaml).unwrap();
    let protocol = ConfigurableProtocol::new(def.clone(), reqwest::Client::new()).unwrap();
    ProtocolRegistry::global()
        .register(def.name.clone(), Arc::new(protocol))
        .unwrap();

    // Create provider with different base_url in config
    let mut config = ProviderConfig::test_config("model");
    config.protocol = Some("url-override".to_string());
    config.api_key = Some("key".to_string());
    config.base_url = Some("https://api.override.com".to_string()); // Different from protocol's base_url

    let provider = create_provider("url-override", config);
    assert!(
        provider.is_ok(),
        "Should allow provider config to override base_url"
    );

    // Clean up
    ProtocolRegistry::global().unregister("url-override");
}
