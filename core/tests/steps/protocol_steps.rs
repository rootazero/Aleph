//! Step definitions for Protocol Integration features

use std::sync::Arc;

use cucumber::{given, when, then};

use crate::world::{AlephWorld, ProtocolContext};
use alephcore::providers::protocols::{ConfigurableProtocol, ProtocolDefinition, ProtocolRegistry};
use alephcore::ProviderConfig;

// =========================================================================
// Given Steps - YAML Protocol Definitions
// =========================================================================

#[given(expr = "a YAML protocol definition extending {string} with custom auth header")]
async fn given_yaml_extends_with_auth(w: &mut AlephWorld, base: String) {
    let ctx = w.protocol.get_or_insert_with(ProtocolContext::default);
    ProtocolContext::init_registry();
    ctx.set_yaml(&format!(r#"
name: test-minimal
extends: {}
base_url: https://api.test.com
differences:
  auth:
    header: X-API-Key
    prefix: ""
"#, base));
}

#[given("a YAML custom protocol definition with request template")]
async fn given_yaml_custom_with_template(w: &mut AlephWorld) {
    let ctx = w.protocol.get_or_insert_with(ProtocolContext::default);
    ProtocolContext::init_registry();
    ctx.set_yaml(r#"
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
"#);
}

#[given("a temporary protocol file with v1 configuration")]
async fn given_temp_protocol_v1(w: &mut AlephWorld) {
    let ctx = w.protocol.get_or_insert_with(ProtocolContext::default);
    ProtocolContext::init_registry();
    let temp_dir = ctx.create_temp_dir();
    let yaml_v1 = r#"
name: reload-test
extends: openai
base_url: https://api.v1.com
differences:
  auth:
    header: X-API-Key
    prefix: "v1-"
"#;
    let file_path = temp_dir.path().join("reload-test.yaml");
    std::fs::write(&file_path, yaml_v1).expect("Failed to write v1 YAML");
    ctx.protocol_name = Some("reload-test".to_string());
}

#[given(expr = "protocols extending {string} {string} and {string}")]
async fn given_multiple_protocols(w: &mut AlephWorld, p1: String, p2: String, p3: String) {
    let ctx = w.protocol.get_or_insert_with(ProtocolContext::default);
    ProtocolContext::init_registry();

    ctx.protocol_defs = vec![
        serde_yaml::from_str(&format!(r#"
name: proto-1
extends: {}
base_url: https://api.proto1.com
"#, p1)).unwrap(),
        serde_yaml::from_str(&format!(r#"
name: proto-2
extends: {}
base_url: https://api.proto2.com
"#, p2)).unwrap(),
        serde_yaml::from_str(&format!(r#"
name: proto-3
extends: {}
base_url: https://api.proto3.com
"#, p3)).unwrap(),
    ];
    ctx.protocol_names = vec!["proto-1".to_string(), "proto-2".to_string(), "proto-3".to_string()];
}

#[given("a temporary directory with protocol files")]
async fn given_temp_dir_with_protocols(w: &mut AlephWorld) {
    let ctx = w.protocol.get_or_insert_with(ProtocolContext::default);
    ProtocolContext::init_registry();
    let temp_dir = ctx.create_temp_dir();

    let protocols = vec![
        ("dir-proto-1.yaml", r#"
name: dir-proto-1
extends: openai
base_url: https://api.dir1.com
"#),
        ("dir-proto-2.yaml", r#"
name: dir-proto-2
extends: anthropic
base_url: https://api.dir2.com
"#),
        ("dir-proto-3.yml", r#"
name: dir-proto-3
extends: gemini
base_url: https://api.dir3.com
"#),
    ];

    for (filename, yaml) in protocols {
        let file_path = temp_dir.path().join(filename);
        std::fs::write(&file_path, yaml).expect("Failed to write protocol file");
    }
    ctx.protocol_names = vec![
        "dir-proto-1".to_string(),
        "dir-proto-2".to_string(),
        "dir-proto-3".to_string(),
    ];
}

#[given("invalid YAML content")]
async fn given_invalid_yaml(w: &mut AlephWorld) {
    let ctx = w.protocol.get_or_insert_with(ProtocolContext::default);
    ctx.set_yaml("invalid: yaml: [[[");
}

#[given("a YAML protocol without extends or custom")]
async fn given_yaml_incomplete(w: &mut AlephWorld) {
    let ctx = w.protocol.get_or_insert_with(ProtocolContext::default);
    ProtocolContext::init_registry();
    ctx.set_yaml(r#"
name: incomplete
# Missing extends or custom
"#);
}

#[given(expr = "a YAML protocol extending non-existent base {string}")]
async fn given_yaml_invalid_extends(w: &mut AlephWorld, base: String) {
    let ctx = w.protocol.get_or_insert_with(ProtocolContext::default);
    ProtocolContext::init_registry();
    ctx.set_yaml(&format!(r#"
name: invalid-extends
extends: {}
"#, base));
}

#[given("a provider config with non-existent protocol")]
async fn given_nonexistent_protocol_config(w: &mut AlephWorld) {
    let ctx = w.protocol.get_or_insert_with(ProtocolContext::default);
    ProtocolContext::init_registry();
    let mut config = ProviderConfig::test_config("model");
    config.protocol = Some("non-existent".to_string());
    config.api_key = Some("key".to_string());
    ctx.provider_config = Some(config);
}

#[given("a YAML protocol with empty auth prefix")]
async fn given_yaml_no_prefix(w: &mut AlephWorld) {
    let ctx = w.protocol.get_or_insert_with(ProtocolContext::default);
    ProtocolContext::init_registry();
    ctx.set_yaml(r#"
name: no-prefix
extends: openai
base_url: https://api.test.com
differences:
  auth:
    header: X-API-Key
    prefix: ""
"#);
}

#[given("a YAML protocol with Bearer auth prefix")]
async fn given_yaml_bearer_prefix(w: &mut AlephWorld) {
    let ctx = w.protocol.get_or_insert_with(ProtocolContext::default);
    ProtocolContext::init_registry();
    ctx.set_yaml(r#"
name: with-prefix
extends: openai
base_url: https://api.test.com
differences:
  auth:
    header: Authorization
    prefix: "Bearer "
"#);
}

#[given("a YAML custom protocol with complex template")]
async fn given_yaml_complex_template(w: &mut AlephWorld) {
    let ctx = w.protocol.get_or_insert_with(ProtocolContext::default);
    ProtocolContext::init_registry();
    ctx.set_yaml(r#"
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
"#);
}

#[given("the protocol registry is initialized")]
async fn given_registry_initialized(w: &mut AlephWorld) {
    let ctx = w.protocol.get_or_insert_with(ProtocolContext::default);
    ProtocolContext::init_registry();
    // Ensure cleanup list doesn't have stale entries
    ctx.protocol_names.clear();
}

#[given(expr = "a YAML protocol with default base_url {string}")]
async fn given_yaml_with_base_url(w: &mut AlephWorld, base_url: String) {
    let ctx = w.protocol.get_or_insert_with(ProtocolContext::default);
    ProtocolContext::init_registry();
    ctx.set_yaml(&format!(r#"
name: url-override
extends: openai
base_url: {}
"#, base_url));
}

// =========================================================================
// When Steps - Parsing
// =========================================================================

#[when("I parse the protocol definition")]
async fn when_parse_protocol(w: &mut AlephWorld) {
    let ctx = w.protocol.as_mut().expect("Protocol context not initialized");
    ctx.last_result = Some(ctx.parse_yaml());
}

#[when("I try to parse the protocol definition")]
async fn when_try_parse_protocol(w: &mut AlephWorld) {
    let ctx = w.protocol.as_mut().expect("Protocol context not initialized");
    ctx.last_result = Some(ctx.parse_yaml());
}

// =========================================================================
// When Steps - Protocol Creation
// =========================================================================

#[when("I create a ConfigurableProtocol from the definition")]
async fn when_create_protocol(w: &mut AlephWorld) {
    let ctx = w.protocol.as_mut().expect("Protocol context not initialized");
    ctx.last_result = Some(ctx.create_protocol());
}

#[when("I try to create a ConfigurableProtocol")]
async fn when_try_create_protocol(w: &mut AlephWorld) {
    let ctx = w.protocol.as_mut().expect("Protocol context not initialized");
    ctx.last_result = Some(ctx.create_protocol());
}

// =========================================================================
// When Steps - Registry
// =========================================================================

#[when("I register the protocol in the registry")]
async fn when_register_protocol(w: &mut AlephWorld) {
    let ctx = w.protocol.as_mut().expect("Protocol context not initialized");
    ctx.last_result = Some(ctx.register_protocol());
}

#[when("I register all protocols in the registry")]
async fn when_register_all_protocols(w: &mut AlephWorld) {
    let ctx = w.protocol.as_mut().expect("Protocol context not initialized");

    for def in ctx.protocol_defs.clone() {
        let protocol = ConfigurableProtocol::new(def.clone(), reqwest::Client::new())
            .expect("Failed to create protocol");
        ProtocolRegistry::global()
            .register(def.name.clone(), Arc::new(protocol))
            .expect("Failed to register protocol");
    }
}

#[when(expr = "I register a custom protocol {string}")]
async fn when_register_custom_protocol(w: &mut AlephWorld, name: String) {
    let ctx = w.protocol.as_mut().expect("Protocol context not initialized");
    let yaml = format!(r#"
name: {}
extends: openai
base_url: https://api.{}.com
"#, name, name);
    let def: ProtocolDefinition = serde_yaml::from_str(&yaml).unwrap();
    let protocol = ConfigurableProtocol::new(def.clone(), reqwest::Client::new())
        .expect("Failed to create protocol");
    ProtocolRegistry::global()
        .register(def.name.clone(), Arc::new(protocol))
        .expect("Failed to register protocol");
    ctx.protocol_names.push(name);
}

#[when(expr = "I unregister protocol {string}")]
async fn when_unregister_protocol(w: &mut AlephWorld, name: String) {
    let ctx = w.protocol.as_mut().expect("Protocol context not initialized");
    ctx.unregister_protocol(&name);
    ctx.protocol_names.retain(|n| n != &name);
}

// =========================================================================
// When Steps - Provider Creation
// =========================================================================

#[when(expr = "I create a provider using protocol {string}")]
async fn when_create_provider(w: &mut AlephWorld, protocol_name: String) {
    let ctx = w.protocol.as_mut().expect("Protocol context not initialized");
    ctx.last_result = Some(ctx.create_provider(&protocol_name));
}

#[when("I create providers for all protocols")]
async fn when_create_all_providers(w: &mut AlephWorld) {
    let ctx = w.protocol.as_mut().expect("Protocol context not initialized");
    for name in ctx.protocol_names.clone() {
        let result = ctx.create_provider(&name);
        if result.is_err() {
            ctx.last_result = Some(result);
            return;
        }
    }
    ctx.last_result = Some(Ok(()));
}

#[when("I try to create a provider")]
async fn when_try_create_provider(w: &mut AlephWorld) {
    let ctx = w.protocol.as_mut().expect("Protocol context not initialized");
    let config = ctx.provider_config.clone().expect("Provider config not set");
    let protocol_name = config.protocol.clone().unwrap_or_else(|| "test".to_string());
    ctx.last_result = Some(ctx.create_provider(&protocol_name));
}

#[when(expr = "I create a provider with base_url {string}")]
async fn when_create_provider_with_base_url(w: &mut AlephWorld, base_url: String) {
    let ctx = w.protocol.as_mut().expect("Protocol context not initialized");
    let protocol_name = ctx.protocol_name.clone().expect("Protocol name not set");
    let mut config = ProviderConfig::test_config("model");
    config.protocol = Some(protocol_name.clone());
    config.api_key = Some("key".to_string());
    config.base_url = Some(base_url);
    ctx.provider_config = Some(config);
    ctx.last_result = Some(ctx.create_provider(&protocol_name));
}

// =========================================================================
// When Steps - Request Building
// =========================================================================

#[when("I build a request with the protocol")]
async fn when_build_request(w: &mut AlephWorld) {
    let ctx = w.protocol.as_mut().expect("Protocol context not initialized");
    ctx.request_result = Some(ctx.build_request());
}

#[when("I try to build a request")]
async fn when_try_build_request(w: &mut AlephWorld) {
    let ctx = w.protocol.as_mut().expect("Protocol context not initialized");
    ctx.request_result = Some(ctx.build_request());
}

// =========================================================================
// When Steps - File Loading
// =========================================================================

#[when("I load the protocol from file")]
async fn when_load_from_file(w: &mut AlephWorld) {
    use alephcore::providers::protocols::ProtocolLoader;

    let ctx = w.protocol.as_mut().expect("Protocol context not initialized");
    let temp_dir = ctx.temp_dir.as_ref().expect("Temp dir not created");
    let file_path = temp_dir.path().join("reload-test.yaml");

    let result = ProtocolLoader::load_from_file(&file_path).await;

    ctx.last_result = Some(result.map_err(|e| e.to_string()));
}

#[when("I update the protocol file with v2 configuration")]
async fn when_update_protocol_v2(w: &mut AlephWorld) {
    let ctx = w.protocol.as_mut().expect("Protocol context not initialized");
    let temp_dir = ctx.temp_dir.as_ref().expect("Temp dir not created");
    let yaml_v2 = r#"
name: reload-test
extends: openai
base_url: https://api.v2.com
differences:
  auth:
    header: Authorization
    prefix: "Bearer "
"#;
    let file_path = temp_dir.path().join("reload-test.yaml");
    std::fs::write(&file_path, yaml_v2).expect("Failed to write v2 YAML");
}

#[when("I reload the protocol from file")]
async fn when_reload_from_file(w: &mut AlephWorld) {
    when_load_from_file(w).await;
}

#[when("I create a provider with v2 configuration")]
async fn when_create_provider_v2(w: &mut AlephWorld) {
    let ctx = w.protocol.as_mut().expect("Protocol context not initialized");
    let mut config = ProviderConfig::test_config("model-v2");
    config.protocol = Some("reload-test".to_string());
    config.api_key = Some("key-v2".to_string());
    config.base_url = Some("https://api.v2.com".to_string());
    ctx.provider_config = Some(config);
    ctx.last_result = Some(ctx.create_provider("reload-test"));
}

#[when("I load protocols from the directory")]
async fn when_load_from_dir(w: &mut AlephWorld) {
    use alephcore::providers::protocols::ProtocolLoader;

    let ctx = w.protocol.as_mut().expect("Protocol context not initialized");
    let temp_dir = ctx.temp_dir.as_ref().expect("Temp dir not created");

    let result = ProtocolLoader::load_from_dir(temp_dir.path()).await;

    ctx.last_result = Some(result.map_err(|e| e.to_string()));
}

// =========================================================================
// When Steps - Combined Operations
// =========================================================================

#[when("I create and register the protocol")]
async fn when_create_and_register(w: &mut AlephWorld) {
    let ctx = w.protocol.as_mut().expect("Protocol context not initialized");
    if let Err(e) = ctx.parse_yaml() {
        ctx.last_result = Some(Err(e));
        return;
    }
    if let Err(e) = ctx.create_protocol() {
        ctx.last_result = Some(Err(e));
        return;
    }
    ctx.last_result = Some(ctx.register_protocol());
}

#[when(expr = "I configure provider with max_tokens {int} and temperature {float}")]
async fn when_configure_provider(w: &mut AlephWorld, max_tokens: u32, temperature: f64) {
    let ctx = w.protocol.as_mut().expect("Protocol context not initialized");
    ctx.configure_provider("test-model-v2", Some(max_tokens), Some(temperature as f32));
}

// =========================================================================
// Then Steps - Protocol Definition
// =========================================================================

#[then(expr = "the protocol name should be {string}")]
async fn then_protocol_name(w: &mut AlephWorld, expected: String) {
    let ctx = w.protocol.as_ref().expect("Protocol context not initialized");
    let def = ctx.protocol_def.as_ref().expect("Protocol definition not parsed");
    assert_eq!(def.name, expected, "Protocol name mismatch");
}

#[then(expr = "the protocol should extend {string}")]
async fn then_protocol_extends(w: &mut AlephWorld, expected: String) {
    let ctx = w.protocol.as_ref().expect("Protocol context not initialized");
    let def = ctx.protocol_def.as_ref().expect("Protocol definition not parsed");
    assert_eq!(def.extends, Some(expected), "Protocol extends mismatch");
}

#[then("the protocol should have custom config")]
async fn then_protocol_has_custom(w: &mut AlephWorld) {
    let ctx = w.protocol.as_ref().expect("Protocol context not initialized");
    let def = ctx.protocol_def.as_ref().expect("Protocol definition not parsed");
    assert!(def.custom.is_some(), "Protocol should have custom config");
}

// =========================================================================
// Then Steps - Protocol Creation Results
// =========================================================================

#[then("the protocol should be created successfully")]
async fn then_protocol_created(w: &mut AlephWorld) {
    let ctx = w.protocol.as_ref().expect("Protocol context not initialized");
    match &ctx.last_result {
        Some(Ok(())) => {}
        Some(Err(e)) => panic!("Protocol creation failed: {}", e),
        None => panic!("No creation result"),
    }
}

#[then("protocol creation should fail")]
async fn then_protocol_creation_fail(w: &mut AlephWorld) {
    let ctx = w.protocol.as_ref().expect("Protocol context not initialized");
    match &ctx.last_result {
        Some(Err(_)) => {}
        Some(Ok(())) => panic!("Expected protocol creation to fail"),
        None => panic!("No creation result"),
    }
}

// =========================================================================
// Then Steps - Registry
// =========================================================================

#[then(expr = "the protocol should be retrievable as {string}")]
async fn then_protocol_retrievable(w: &mut AlephWorld, name: String) {
    let ctx = w.protocol.as_ref().expect("Protocol context not initialized");
    assert!(ctx.is_protocol_registered(&name), "Protocol {} not in registry", name);
}

#[then(expr = "the protocol should be registered as {string}")]
async fn then_protocol_registered(w: &mut AlephWorld, name: String) {
    let ctx = w.protocol.as_ref().expect("Protocol context not initialized");
    assert!(ctx.is_protocol_registered(&name), "Protocol {} not in registry", name);
}

#[then("all protocols should be retrievable")]
async fn then_all_protocols_retrievable(w: &mut AlephWorld) {
    let ctx = w.protocol.as_ref().expect("Protocol context not initialized");
    for name in &ctx.protocol_names {
        assert!(ctx.is_protocol_registered(name), "Protocol {} not in registry", name);
    }
}

#[then(expr = "the registry should contain {string}")]
async fn then_registry_contains(w: &mut AlephWorld, name: String) {
    let protocols = ProtocolRegistry::global().list_protocols();
    assert!(protocols.contains(&name), "Registry should contain {}", name);
}

#[then(expr = "the registry should not contain {string}")]
async fn then_registry_not_contains(w: &mut AlephWorld, name: String) {
    let protocols = ProtocolRegistry::global().list_protocols();
    assert!(!protocols.contains(&name), "Registry should not contain {}", name);
}

#[then(expr = "protocol {string} should be registered")]
async fn then_specific_protocol_registered(w: &mut AlephWorld, name: String) {
    let ctx = w.protocol.as_ref().expect("Protocol context not initialized");
    assert!(ctx.is_protocol_registered(&name), "Protocol {} not in registry", name);
}

// =========================================================================
// Then Steps - Provider
// =========================================================================

#[then("the provider should be created successfully")]
async fn then_provider_created(w: &mut AlephWorld) {
    let ctx = w.protocol.as_ref().expect("Protocol context not initialized");
    match &ctx.last_result {
        Some(Ok(())) => {}
        Some(Err(e)) => panic!("Provider creation failed: {}", e),
        None => panic!("No creation result"),
    }
}

#[then("all providers should be created successfully")]
async fn then_all_providers_created(w: &mut AlephWorld) {
    let ctx = w.protocol.as_ref().expect("Protocol context not initialized");
    match &ctx.last_result {
        Some(Ok(())) => {}
        Some(Err(e)) => panic!("Provider creation failed: {}", e),
        None => panic!("No creation result"),
    }
}

#[then("provider creation should fail")]
async fn then_provider_creation_fail(w: &mut AlephWorld) {
    let ctx = w.protocol.as_ref().expect("Protocol context not initialized");
    match &ctx.last_result {
        Some(Err(_)) => {}
        Some(Ok(())) => panic!("Expected provider creation to fail"),
        None => panic!("No creation result"),
    }
}

#[then(expr = "the provider name should be {string}")]
async fn then_provider_name(w: &mut AlephWorld, expected: String) {
    let ctx = w.protocol.as_ref().expect("Protocol context not initialized");
    let provider = ctx.provider.as_ref().expect("Provider not created");
    assert_eq!(provider.name(), expected, "Provider name mismatch");
}

// =========================================================================
// Then Steps - Request Building
// =========================================================================

#[then("the request should be built successfully")]
async fn then_request_built(w: &mut AlephWorld) {
    let ctx = w.protocol.as_ref().expect("Protocol context not initialized");
    match &ctx.request_result {
        Some(Ok(())) => {}
        Some(Err(e)) => panic!("Request build failed: {}", e),
        None => panic!("No request build result"),
    }
}

#[then("building the request should fail")]
async fn then_request_build_fail(w: &mut AlephWorld) {
    let ctx = w.protocol.as_ref().expect("Protocol context not initialized");
    match &ctx.request_result {
        Some(Err(_)) => {}
        Some(Ok(())) => panic!("Expected request build to fail"),
        None => panic!("No request build result"),
    }
}

// =========================================================================
// Then Steps - Parsing Results
// =========================================================================

#[then("parsing should fail")]
async fn then_parsing_fail(w: &mut AlephWorld) {
    let ctx = w.protocol.as_ref().expect("Protocol context not initialized");
    match &ctx.last_result {
        Some(Err(_)) => {}
        Some(Ok(())) => panic!("Expected parsing to fail"),
        None => panic!("No parsing result"),
    }
}
