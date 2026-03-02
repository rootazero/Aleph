//! Tests that call build_* entry points

use super::super::*;
use crate::thinker::soul::SoulManifest;

// ========== Integration tests: public API via Pipeline ==========

#[test]
fn test_build_system_prompt_with_soul() {
    let builder = PromptBuilder::new(PromptConfig::default());

    let soul = SoulManifest {
        identity: "I am Aleph.".to_string(),
        directives: vec!["Help users".to_string()],
        ..Default::default()
    };

    let prompt = builder.build_system_prompt_with_soul(&[], &soul, None);

    // Soul should appear first
    let identity_pos = prompt.find("# Identity").unwrap();
    let role_pos = prompt.find("Your Role").unwrap();
    assert!(
        identity_pos < role_pos,
        "Identity should appear before Role"
    );

    // Standard sections should still be present
    assert!(prompt.contains("Response Format"));
    assert!(prompt.contains("JSON"));
}

#[test]
fn test_thinking_guidance_disabled_by_default() {
    let builder = PromptBuilder::new(PromptConfig::default());
    let prompt = builder.build_system_prompt(&[]);

    // Default is off, so no thinking transparency section
    assert!(!prompt.contains("Thinking Transparency"));
    assert!(!prompt.contains("Reasoning Flow"));
}

#[test]
fn test_thinking_guidance_enabled() {
    let config = PromptConfig {
        thinking_transparency: true,
        ..Default::default()
    };
    let builder = PromptBuilder::new(config);
    let prompt = builder.build_system_prompt(&[]);

    // Should contain thinking transparency section
    assert!(prompt.contains("## Thinking Transparency"));
    assert!(prompt.contains("### Reasoning Flow"));

    // Should contain the four phases
    assert!(prompt.contains("**Observation**"));
    assert!(prompt.contains("**Analysis**"));
    assert!(prompt.contains("**Planning**"));
    assert!(prompt.contains("**Decision**"));

    // Should contain uncertainty guidance
    assert!(prompt.contains("Expressing Uncertainty"));
    assert!(prompt.contains("High confidence"));
    assert!(prompt.contains("Low confidence"));

    // Should contain alternatives guidance
    assert!(prompt.contains("Acknowledging Alternatives"));
}

#[test]
fn test_thinking_guidance_with_soul() {
    let config = PromptConfig {
        thinking_transparency: true,
        ..Default::default()
    };
    let builder = PromptBuilder::new(config);

    let soul = SoulManifest {
        identity: "Test assistant.".to_string(),
        ..Default::default()
    };

    let prompt = builder.build_system_prompt_with_soul(&[], &soul, None);

    // Both soul and thinking guidance should be present
    assert!(prompt.contains("# Identity"));
    assert!(prompt.contains("## Thinking Transparency"));
}

#[test]
fn test_build_system_prompt_with_context_includes_runtime_context() {
    use crate::thinker::context::ContextAggregator;
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::security_context::SecurityContext;

    let builder = PromptBuilder::new(PromptConfig::default());

    // Build a ResolvedContext with runtime_context set
    let interaction = InteractionManifest::new(InteractionParadigm::WebRich);
    let security = SecurityContext::permissive();
    let mut ctx = ContextAggregator::resolve(&interaction, &security, &[]);

    ctx.runtime_context = Some(crate::thinker::runtime_context::RuntimeContext {
        os: "linux".to_string(),
        arch: "x86_64".to_string(),
        shell: "bash".to_string(),
        working_dir: std::path::PathBuf::from("/home/user"),
        repo_root: None,
        current_model: "gpt-4".to_string(),
        hostname: "server-01".to_string(),
    });

    let prompt = builder.build_system_prompt_with_context(&ctx);

    // Runtime context should be present
    assert!(prompt.contains("## Runtime Environment"));
    assert!(prompt.contains("os=linux"));
    assert!(prompt.contains("model=gpt-4"));

    // Runtime context should appear before environment contract
    let runtime_pos = prompt.find("## Runtime Environment").unwrap();
    let env_pos = prompt.find("## Environment").unwrap();
    assert!(
        runtime_pos < env_pos,
        "Runtime context should appear before environment contract"
    );
}

#[test]
fn test_build_system_prompt_with_context_no_runtime_context() {
    use crate::thinker::context::ContextAggregator;
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::security_context::SecurityContext;

    let builder = PromptBuilder::new(PromptConfig::default());

    let interaction = InteractionManifest::new(InteractionParadigm::WebRich);
    let security = SecurityContext::permissive();
    let ctx = ContextAggregator::resolve(&interaction, &security, &[]);

    // runtime_context should be None by default
    assert!(ctx.runtime_context.is_none());

    let prompt = builder.build_system_prompt_with_context(&ctx);

    // Runtime context section should NOT be present
    assert!(!prompt.contains("## Runtime Environment"));
}

#[test]
fn test_full_prompt_with_all_enhancements_background_mode() {
    use crate::thinker::context::ContextAggregator;
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::runtime_context::RuntimeContext;
    use crate::thinker::security_context::SecurityContext;

    let builder = PromptBuilder::new(PromptConfig::default());

    // Build a Background-mode context (should trigger all 4 enhancements)
    let interaction = InteractionManifest::new(InteractionParadigm::Background);
    let security = SecurityContext::permissive();
    let mut resolved = ContextAggregator::resolve(&interaction, &security, &[]);

    // Add RuntimeContext
    resolved.runtime_context = Some(RuntimeContext {
        os: "macOS 15.3".to_string(),
        arch: "aarch64".to_string(),
        shell: "zsh".to_string(),
        working_dir: std::path::PathBuf::from("/workspace"),
        repo_root: Some(std::path::PathBuf::from("/workspace")),
        current_model: "claude-opus-4-6".to_string(),
        hostname: "test-host".to_string(),
    });

    let prompt = builder.build_system_prompt_with_context(&resolved);

    // 1. RuntimeContext should be present
    assert!(
        prompt.contains("## Runtime Environment"),
        "Missing RuntimeContext section"
    );
    assert!(prompt.contains("os=macOS 15.3"), "Missing OS info");
    assert!(
        prompt.contains("model=claude-opus-4-6"),
        "Missing model info"
    );

    // 2. Protocol tokens should be present (Background has SilentReply)
    assert!(
        prompt.contains("ALEPH_HEARTBEAT_OK"),
        "Missing protocol tokens: ALEPH_HEARTBEAT_OK"
    );
    assert!(
        prompt.contains("ALEPH_SILENT_COMPLETE"),
        "Missing protocol tokens: ALEPH_SILENT_COMPLETE"
    );

    // 3. Operational guidelines should be present (Background mode)
    assert!(
        prompt.contains("System Operational Awareness"),
        "Missing operational guidelines"
    );
    assert!(
        prompt.contains("Diagnostic Capabilities"),
        "Missing diagnostic capabilities in operational guidelines"
    );

    // 4. Citation standards should be present (always injected)
    assert!(
        prompt.contains("Citation Standards"),
        "Missing citation standards"
    );
    assert!(
        prompt.contains("citation is mandatory"),
        "Missing citation requirement"
    );

    // Standard sections should still be present
    assert!(prompt.contains("Your Role"), "Missing role section");
    assert!(
        prompt.contains("Response Format"),
        "Missing response format section"
    );

    // Verify ordering: RuntimeContext -> Environment -> Protocol -> Guidelines -> Citations
    let runtime_pos = prompt.find("## Runtime Environment").unwrap();
    let env_pos = prompt.find("## Environment").unwrap();
    let protocol_pos = prompt.find("Response Protocol Tokens").unwrap();
    let guidelines_pos = prompt.find("System Operational Awareness").unwrap();
    let citation_pos = prompt.find("Citation Standards").unwrap();

    assert!(
        runtime_pos < env_pos,
        "RuntimeContext should appear before Environment contract"
    );
    assert!(
        env_pos < protocol_pos,
        "Environment should appear before Protocol tokens"
    );
    assert!(
        protocol_pos < guidelines_pos,
        "Protocol tokens should appear before Operational guidelines"
    );
    assert!(
        guidelines_pos < citation_pos,
        "Operational guidelines should appear before Citation standards"
    );
}

#[test]
fn test_interactive_prompt_minimal_token_overhead() {
    use crate::thinker::context::ContextAggregator;
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::runtime_context::RuntimeContext;
    use crate::thinker::security_context::SecurityContext;

    let builder = PromptBuilder::new(PromptConfig::default());

    // Build a WebRich-mode context (interactive, not background)
    let interaction = InteractionManifest::new(InteractionParadigm::WebRich);
    let security = SecurityContext::permissive();
    let mut resolved = ContextAggregator::resolve(&interaction, &security, &[]);

    // Add RuntimeContext (should still be included for interactive)
    resolved.runtime_context = Some(RuntimeContext {
        os: "linux".to_string(),
        arch: "x86_64".to_string(),
        shell: "bash".to_string(),
        working_dir: std::path::PathBuf::from("/home/user"),
        repo_root: None,
        current_model: "gpt-4".to_string(),
        hostname: "web-server".to_string(),
    });

    let prompt = builder.build_system_prompt_with_context(&resolved);

    // 1. RuntimeContext SHOULD be present (always injected when provided)
    assert!(
        prompt.contains("## Runtime Environment"),
        "RuntimeContext should be present in WebRich mode"
    );
    assert!(prompt.contains("os=linux"), "Missing OS info in WebRich mode");
    assert!(
        prompt.contains("model=gpt-4"),
        "Missing model info in WebRich mode"
    );

    // 2. Protocol tokens should NOT be present (WebRich has no SilentReply)
    assert!(
        !prompt.contains("ALEPH_HEARTBEAT_OK"),
        "Protocol tokens should NOT be present in WebRich mode"
    );
    assert!(
        !prompt.contains("Response Protocol Tokens"),
        "Protocol tokens section should NOT be present in WebRich mode"
    );

    // 3. Operational guidelines should NOT be present (WebRich is not Background/CLI)
    assert!(
        !prompt.contains("System Operational Awareness"),
        "Operational guidelines should NOT be present in WebRich mode"
    );

    // 4. Citation standards SHOULD be present (always injected)
    assert!(
        prompt.contains("Citation Standards"),
        "Citation standards should be present in WebRich mode"
    );
    assert!(
        prompt.contains("citation is mandatory"),
        "Citation requirement should be present in WebRich mode"
    );

    // Standard sections should be present
    assert!(prompt.contains("Your Role"), "Missing role section");
    assert!(
        prompt.contains("Response Format"),
        "Missing response format section"
    );
}

#[test]
fn test_build_system_prompt_with_hooks() {
    use crate::thinker::prompt_hooks::PromptHook;

    struct AppendHook;
    impl PromptHook for AppendHook {
        fn after_prompt_build(&self, prompt: &mut String) -> crate::error::Result<()> {
            prompt.push_str("\n## Custom Section\n");
            Ok(())
        }
    }

    let builder = PromptBuilder::new(PromptConfig::default());
    let soul = SoulManifest::default();
    let hooks: Vec<Box<dyn PromptHook>> = vec![Box::new(AppendHook)];
    let prompt = builder.build_system_prompt_with_hooks(&[], &soul, None, &hooks);
    assert!(prompt.contains("## Custom Section"));
}
