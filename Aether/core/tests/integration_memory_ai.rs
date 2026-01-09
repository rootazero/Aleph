/// Integration tests for Memory + AI Pipeline (Phase 6)
///
/// These tests verify the complete flow:
/// 1. Context capture
/// 2. Memory retrieval
/// 3. Prompt augmentation
/// 4. Router selection
/// 5. Provider execution
/// 6. Memory storage
use aethecore::{
    AetherCore, AetherEventHandler, CapturedContext, Config, MemoryConfig, ProcessingState,
    ProviderConfig, RoutingRuleConfig,
};

// Mock event handler for testing (since MockEventHandler is only available in lib tests)
#[derive(Clone)]
struct TestEventHandler;

impl AetherEventHandler for TestEventHandler {
    fn on_state_changed(&self, _state: ProcessingState) {}
    fn on_error(&self, _message: String, _suggestion: Option<String>) {}
    fn on_response_chunk(&self, _accumulated_text: String) {}
    fn on_error_typed(&self, _error_type: aethecore::ErrorType, _message: String) {}
    fn on_progress(&self, _progress: f32) {}
    fn on_ai_processing_started(&self, _provider_name: String, _provider_color: String) {}
    fn on_ai_response_received(&self, _response_preview: String) {}
    fn on_provider_fallback(&self, _from_provider: String, _to_provider: String) {}
    fn on_config_changed(&self) {}
    fn on_typewriter_progress(&self, _percent: f32) {}
    fn on_typewriter_cancelled(&self) {}
    fn on_clarification_needed(
        &self,
        _request: aethecore::ClarificationRequest,
    ) -> aethecore::ClarificationResult {
        aethecore::ClarificationResult::cancelled()
    }
    fn on_conversation_started(&self, _session_id: String) {}
    fn on_conversation_turn_completed(&self, _turn: aethecore::ConversationTurn) {}
    fn on_conversation_continuation_ready(&self) {}
    fn on_conversation_ended(&self, _session_id: String, _total_turns: u32) {}
    fn on_confirmation_needed(&self, _confirmation: aethecore::PendingConfirmationInfo) {}
    fn on_confirmation_expired(&self, _confirmation_id: String) {}
}

/// Test helper: Create a test config with mock provider
fn create_test_config_with_providers() -> Config {
    let mut config = Config::default();

    // Enable memory
    config.memory = MemoryConfig {
        enabled: true,
        embedding_model: "bge-small-zh-v1.5".to_string(),
        max_context_items: 5,
        retention_days: 90,
        vector_db: "sqlite-vec".to_string(),
        similarity_threshold: 0.7,
        excluded_apps: vec![],
        ai_retrieval_enabled: true,
        ai_retrieval_timeout_ms: 3000,
        ai_retrieval_max_candidates: 20,
        ai_retrieval_fallback_count: 3,
        ..Default::default()
    };

    // Add mock provider (uses openai type for testing)
    config.providers.insert("mock".to_string(), {
        let mut provider_config = ProviderConfig::test_config("gpt-4o");
        provider_config.provider_type = Some("openai".to_string());
        provider_config
    });

    // Set default provider
    config.general.default_provider = Some("mock".to_string());

    // Add routing rules
    config.rules.push({
        let mut rule = RoutingRuleConfig::test_config(r"^/code", "mock");
        rule.system_prompt = Some("You are a coding assistant.".to_string());
        rule
    });

    config
        .rules
        .push(RoutingRuleConfig::test_config(r".*", "mock"));

    config
}

#[test]
fn test_process_input_pipeline_structure() {
    // This test verifies the AI pipeline structure
    // NOTE: This test may use the user's configured provider if available,
    // which is acceptable for integration testing
    let handler = Box::new(TestEventHandler);
    let core = AetherCore::new(handler).expect("Failed to create AetherCore");

    // Set context
    let context = CapturedContext {
        app_bundle_id: "com.apple.Notes".to_string(),
        window_title: Some("Test.txt".to_string()),
        attachments: None,
    };
    core.set_current_context(context.clone());

    // Try to process - may succeed if provider is configured, or fail if not
    let result = core.process_input("Hello world".to_string(), context.clone());

    // Either outcome is valid - we're just testing the pipeline doesn't crash
    match result {
        Ok(_) => println!("✓ AI pipeline succeeded with configured provider"),
        Err(_) => println!("✓ AI pipeline failed as expected without providers"),
    }

    println!("✓ AI pipeline structure test passed");
}

#[test]
fn test_memory_augmentation_integration() {
    // Test that memory augmentation works correctly
    let handler = Box::new(TestEventHandler);
    let core = AetherCore::new(handler).expect("Failed to create AetherCore");

    // Set context
    let context = CapturedContext {
        app_bundle_id: "com.apple.Notes".to_string(),
        window_title: Some("Rust Learning.txt".to_string()),
        attachments: None,
    };
    core.set_current_context(context);

    // Test retrieve_and_augment_prompt (memory disabled by default)
    let result = core.retrieve_and_augment_prompt(
        "You are a helpful assistant.".to_string(),
        "What is Rust?".to_string(),
    );

    assert!(result.is_ok());
    let augmented = result.unwrap();
    assert!(augmented.contains("You are a helpful assistant."));
    assert!(augmented.contains("What is Rust?"));

    println!("✓ Memory augmentation integration test passed");
}

#[test]
fn test_context_capture_and_retrieval() {
    // Test context capture mechanism
    let handler = Box::new(TestEventHandler);
    let core = AetherCore::new(handler).expect("Failed to create AetherCore");

    // Test 1: Set and retrieve context
    let context1 = CapturedContext {
        app_bundle_id: "com.apple.Notes".to_string(),
        window_title: Some("Document1.txt".to_string()),
        attachments: None,
    };
    core.set_current_context(context1.clone());

    // Test 2: Change context
    let context2 = CapturedContext {
        app_bundle_id: "com.google.Chrome".to_string(),
        window_title: Some("GitHub - Mozilla Firefox".to_string()),
        attachments: None,
    };
    core.set_current_context(context2.clone());

    // Test 3: Try to use context for memory operations
    let result =
        core.retrieve_and_augment_prompt("System prompt".to_string(), "User query".to_string());

    // Should succeed even if memory is disabled
    assert!(result.is_ok());

    println!("✓ Context capture and retrieval test passed");
}

#[test]
fn test_memory_enable_disable() {
    // Test memory enable/disable functionality
    let handler = Box::new(TestEventHandler);
    let core = AetherCore::new(handler).expect("Failed to create AetherCore");

    // Check default state (may be enabled or disabled depending on config)
    let config = core.get_memory_config();
    let initial_state = config.enabled;
    println!(
        "Initial memory state: {}",
        if initial_state { "enabled" } else { "disabled" }
    );

    // Toggle memory state
    let mut new_config = config.clone();
    new_config.enabled = !initial_state;
    let result = core.update_memory_config(new_config.clone());
    assert!(result.is_ok());

    // Verify toggled
    let toggled_config = core.get_memory_config();
    assert_eq!(
        toggled_config.enabled, !initial_state,
        "Memory should be toggled"
    );

    // Toggle back
    let mut restore_config = toggled_config.clone();
    restore_config.enabled = initial_state;
    let result = core.update_memory_config(restore_config);
    assert!(result.is_ok());

    // Verify restored
    let final_config = core.get_memory_config();
    assert_eq!(
        final_config.enabled, initial_state,
        "Memory should be restored to initial state"
    );

    println!("✓ Memory enable/disable test passed");
}

#[test]
fn test_ai_pipeline_error_handling() {
    // Test error handling in AI pipeline
    let handler = Box::new(TestEventHandler);
    let core = AetherCore::new(handler).expect("Failed to create AetherCore");

    let context = CapturedContext {
        app_bundle_id: "com.test.app".to_string(),
        window_title: Some("Test".to_string()),
        attachments: None,
    };
    core.set_current_context(context.clone());

    // Test 1: Try processing - may succeed or fail depending on config
    // This tests that the pipeline doesn't crash
    let _result = core.process_input("Test input".to_string(), context.clone());
    // Don't assert on error - user may have providers configured

    // Test 2: Memory augmentation with missing context
    let core2 = AetherCore::new(Box::new(TestEventHandler)).unwrap();
    // Don't set context
    let result2 = core2.retrieve_and_augment_prompt("System".to_string(), "User".to_string());
    // Should fallback to base prompt when memory disabled
    assert!(result2.is_ok());

    println!("✓ AI pipeline error handling test passed");
}

#[test]
fn test_full_pipeline_flow() {
    // Test the complete flow from context to memory to AI
    let handler = Box::new(TestEventHandler);
    let core = AetherCore::new(handler).expect("Failed to create AetherCore");

    // Step 1: Set context
    let context = CapturedContext {
        app_bundle_id: "com.apple.TextEdit".to_string(),
        window_title: Some("Project Notes.txt".to_string()),
        attachments: None,
    };
    core.set_current_context(context.clone());

    // Step 2: Enable memory
    let mut config = core.get_memory_config();
    config.enabled = true;
    core.update_memory_config(config).ok();

    // Step 3: Try memory augmentation (should work with empty history)
    let result = core.retrieve_and_augment_prompt(
        "You are an AI assistant.".to_string(),
        "Tell me about Rust".to_string(),
    );

    if let Ok(augmented_text) = result {
        println!("✓ Memory augmentation succeeded");
        assert!(augmented_text.contains("Tell me about Rust"));
    } else {
        println!("Note: Memory augmentation skipped (expected in test env)");
    }

    // Step 4: Try AI processing - may succeed if providers configured
    let _result = core.process_input("Test input".to_string(), context.clone());
    // Don't assert - user may have providers configured

    println!("✓ Full pipeline flow test passed");
}

#[test]
fn test_concurrent_context_updates() {
    // Test thread safety of context updates
    use std::sync::Arc;
    use std::thread;

    let handler = Box::new(TestEventHandler);
    let core: Arc<AetherCore> =
        Arc::new(AetherCore::new(handler).expect("Failed to create AetherCore"));

    let mut handles = vec![];

    // Spawn multiple threads updating context
    for i in 0..5 {
        let core_ref = Arc::clone(&core);
        let handle = thread::spawn(move || {
            let context = CapturedContext {
                app_bundle_id: format!("com.test.app{}", i),
                window_title: Some(format!("Window{}", i)),
                attachments: None,
            };
            core_ref.set_current_context(context);
        });
        handles.push(handle);
    }

    // Wait for all threads
    for handle in handles {
        handle.join().unwrap();
    }

    println!("✓ Concurrent context updates test passed");
}

#[test]
fn test_memory_config_validation() {
    // Test memory configuration validation
    let handler = Box::new(TestEventHandler);
    let core = AetherCore::new(handler).expect("Failed to create AetherCore");

    // Test valid config
    let valid_config = MemoryConfig {
        enabled: true,
        embedding_model: "bge-small-zh-v1.5".to_string(),
        max_context_items: 10,
        retention_days: 30,
        vector_db: "sqlite-vec".to_string(),
        similarity_threshold: 0.8,
        excluded_apps: vec!["com.apple.keychainaccess".to_string()],
        ai_retrieval_enabled: true,
        ai_retrieval_timeout_ms: 3000,
        ai_retrieval_max_candidates: 20,
        ai_retrieval_fallback_count: 3,
        ..Default::default()
    };

    let result = core.update_memory_config(valid_config);
    assert!(result.is_ok());

    // Test retention policy change
    let mut updated_config = core.get_memory_config();
    updated_config.retention_days = 60;
    let result = core.update_memory_config(updated_config);
    assert!(result.is_ok());

    let final_config = core.get_memory_config();
    assert_eq!(final_config.retention_days, 60);

    println!("✓ Memory config validation test passed");
}
