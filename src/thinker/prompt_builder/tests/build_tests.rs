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
fn phase3_with_resolved_context_basic_path_emits_operational_guidelines() {
    // Phase 3 wiring: `PromptBuilder::with_resolved_context(...)` must
    // thread a `ResolvedContext` into the `Basic` assembly path so the
    // Phase 2 widened layers fire on the harness route (which calls
    // `build_system_prompt`, not `build_system_prompt_with_context`).
    //
    // The harness-bridge default is `Background` paradigm + permissive
    // security — under those settings `OperationalGuidelinesLayer` must
    // emit its `## System Operational Awareness` block.
    use crate::thinker::context::ContextAggregator;
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::security_context::SecurityContext;

    let interaction = InteractionManifest::new(InteractionParadigm::Background);
    let security = SecurityContext::permissive();
    let resolved = ContextAggregator::resolve(&interaction, &security, &[]);

    let builder = PromptBuilder::new(PromptConfig::default()).with_resolved_context(resolved);
    let prompt = builder.build_system_prompt(&[]);

    assert!(
        prompt.contains("## System Operational Awareness"),
        "OperationalGuidelinesLayer must emit on Basic path when resolved_context is attached"
    );
    assert!(
        prompt.contains("Diagnostic Capabilities"),
        "Diagnostic Capabilities sub-section missing from operational guidelines"
    );
    // SecurityLayer always renders a "Security Level: …" note (sandbox
    // baseline) even under permissive — that's the documented envelope
    // surface for the LLM. Verify the header + the permissive note both
    // arrive on Basic.
    assert!(
        prompt.contains("## Security & Constraints"),
        "SecurityLayer should emit the section header when context is attached"
    );
    assert!(
        prompt.contains("Security Level: None"),
        "SecurityLayer must surface the sandbox baseline note under permissive"
    );
    // ProtocolTokensLayer guards on the `SilentReply` capability —
    // Background paradigm includes it by default, so the harness path
    // gets the protocol token block automatically.
    assert!(
        prompt.contains("ALEPH_SILENT_COMPLETE"),
        "ProtocolTokensLayer must emit when Background paradigm enables SilentReply"
    );
    // RuntimeContextLayer requires `runtime_context` to be populated on
    // the ResolvedContext — left as None here, so layer is silent.
    assert!(
        !prompt.contains("## Runtime Environment"),
        "RuntimeContextLayer should stay silent without runtime_context attached"
    );
}

#[test]
fn phase3_with_provider_protocol_openai_emits_guidance_on_basic_path() {
    // `PromptBuilder::with_provider_protocol(...)` must thread the
    // wire-protocol family into the `Basic` assembly path so
    // `ProviderGuidanceLayer` selects the right per-family block. The
    // harness bridge sources the protocol from
    // `AiProvider::model_behavior_override()` falling back to
    // `AiProvider::protocol()`.
    let builder = PromptBuilder::new(PromptConfig::default()).with_provider_protocol("openai");
    let prompt = builder.build_system_prompt(&[]);
    assert!(
        prompt.contains("## Tool-Use Enforcement"),
        "OpenAI protocol must surface tool-use enforcement on Basic path"
    );
    assert!(
        prompt.contains("## Execution Discipline"),
        "OpenAI protocol must surface execution discipline on Basic path"
    );
}

#[test]
fn phase3_with_provider_protocol_anthropic_stays_silent_on_basic_path() {
    let builder = PromptBuilder::new(PromptConfig::default()).with_provider_protocol("anthropic");
    let prompt = builder.build_system_prompt(&[]);
    assert!(
        !prompt.contains("## Tool-Use Enforcement"),
        "Anthropic protocol must not emit tool-use enforcement (Claude is well-behaved)"
    );
    assert!(
        !prompt.contains("## Execution Discipline"),
        "Anthropic protocol must not emit execution discipline"
    );
    assert!(
        !prompt.contains("## Google Model Operational Directives"),
        "Anthropic protocol must not emit Google directives"
    );
}

#[test]
fn phase4_with_runtime_context_populated_emits_runtime_environment_on_basic_path() {
    // Phase 4 F1: when a `ResolvedContext` carries a populated
    // `runtime_context`, `RuntimeContextLayer` (priority 1720, Dynamic)
    // must emit its single-line `## Runtime Environment` summary even on
    // the Basic path. Mirrors `harness_bridge::build_system_prompt`
    // populating the field with `RuntimeContext::collect(...)`.
    use crate::thinker::context::ContextAggregator;
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::runtime_context::RuntimeContext;
    use crate::thinker::security_context::SecurityContext;

    let interaction = InteractionManifest::new(InteractionParadigm::Background);
    let security = SecurityContext::permissive();
    let mut resolved = ContextAggregator::resolve(&interaction, &security, &[]);
    resolved.runtime_context = Some(RuntimeContext {
        os: "linux".to_string(),
        arch: "aarch64".to_string(),
        shell: "fish".to_string(),
        working_dir: std::path::PathBuf::from("/srv/aleph"),
        repo_root: None,
        current_model: "test-provider".to_string(),
        hostname: "ci-runner".to_string(),
        current_time: "2026-05-20 12:00:00".to_string(),
        current_time_ms: 1779789600000,
        timezone: "UTC".to_string(),
    });

    let builder = PromptBuilder::new(PromptConfig::default()).with_resolved_context(resolved);
    let prompt = builder.build_system_prompt(&[]);

    assert!(
        prompt.contains("## Runtime Environment"),
        "RuntimeContextLayer must emit on Basic path when runtime_context is populated"
    );
    assert!(prompt.contains("arch=aarch64"));
    assert!(prompt.contains("shell=fish"));
    assert!(prompt.contains("model=test-provider"));
    assert!(prompt.contains("host=ci-runner"));
    assert!(prompt.contains("(UTC)"));
}

#[test]
fn phase4_with_iteration_cap_emits_session_budget_block() {
    // Phase 4 F2: harness_bridge resolves the per-run iteration cap
    // once via `resolve_max_iterations` and threads it into the prompt
    // via `PromptBuilder::with_iteration_cap(...)`. `SessionBudgetLayer`
    // must surface it as a `## Session Budget` block on the Basic path.
    let builder = PromptBuilder::new(PromptConfig::default()).with_iteration_cap(64);
    let prompt = builder.build_system_prompt(&[]);
    assert!(prompt.contains("## Session Budget"));
    assert!(prompt.contains("Iteration cap**: 64"));
    assert!(prompt.contains("decisive action"));
}

#[test]
fn phase4_channel_aware_resolved_context_messaging_paradigm() {
    // Phase 4 F4: when the gateway threads a channel-specific
    // InteractionManifest (e.g., Messaging paradigm for Telegram) into
    // `FlowRequest.interaction_manifest`, the harness bridge constructs
    // the `ResolvedContext` from that manifest instead of the
    // `Background` default. Messaging paradigm doesn't include
    // SilentReply (Background does), so the protocol tokens block must
    // NOT emit — the test pins the contract by paradigm choice.
    use crate::thinker::context::ContextAggregator;
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::security_context::SecurityContext;

    let interaction = InteractionManifest::new(InteractionParadigm::Messaging);
    let security = SecurityContext::permissive();
    let resolved = ContextAggregator::resolve(&interaction, &security, &[]);

    let builder = PromptBuilder::new(PromptConfig::default()).with_resolved_context(resolved);
    let prompt = builder.build_system_prompt(&[]);

    // OperationalGuidelinesLayer gates on `Background | CLI` only — the
    // Messaging paradigm must keep that section silent.
    assert!(
        !prompt.contains("## System Operational Awareness"),
        "Messaging paradigm must not emit operational guidelines (gates on Background/CLI)"
    );
    // ProtocolTokensLayer gates on `SilentReply` — Messaging paradigm
    // doesn't enable it by default.
    assert!(
        !prompt.contains("ALEPH_SILENT_COMPLETE"),
        "Messaging paradigm must not emit silent-complete protocol tokens"
    );
    // SecurityLayer fires whenever any disabled_tools / security_notes
    // arrive — the permissive sandbox baseline note still shows up.
    assert!(prompt.contains("Security Level: None"));
}

#[test]
fn phase4_without_iteration_cap_session_budget_stays_silent() {
    let builder = PromptBuilder::new(PromptConfig::default());
    let prompt = builder.build_system_prompt(&[]);
    assert!(
        !prompt.contains("## Session Budget"),
        "SessionBudgetLayer must not emit when no iteration cap was attached"
    );
}

#[test]
fn phase3_basic_path_without_resolved_context_stays_silent() {
    // Symmetric to the above: without `with_resolved_context`, the
    // widened layers must still graceful-noop on Basic so we don't
    // accidentally render half-headers.
    let builder = PromptBuilder::new(PromptConfig::default());
    let prompt = builder.build_system_prompt(&[]);

    assert!(
        !prompt.contains("## System Operational Awareness"),
        "OperationalGuidelinesLayer must not emit when no resolved_context attached"
    );
    assert!(
        !prompt.contains("## Security & Constraints"),
        "SecurityLayer must not emit when no resolved_context attached"
    );
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
        current_time: "2026-03-30 02:30:00".to_string(),
        current_time_ms: 1774852200000,
        timezone: "UTC".to_string(),
    });

    let prompt = builder.build_system_prompt_with_context(&ctx);

    // Runtime context should be present
    assert!(prompt.contains("## Runtime Environment"));
    assert!(prompt.contains("os=linux"));
    assert!(prompt.contains("model=gpt-4"));

    // Runtime context is a dynamic layer (priority 1710) so it appears
    // after stable layers like environment (priority 300).
    let runtime_pos = prompt.find("## Runtime Environment").unwrap();
    let env_pos = prompt.find("## Environment").unwrap();
    assert!(
        runtime_pos > env_pos,
        "Runtime context (dynamic) should appear after environment (stable)"
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
        current_time: "2026-03-30 14:30:00".to_string(),
        current_time_ms: 1774852200000,
        timezone: "Asia/Shanghai".to_string(),
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

    // Verify ordering: Environment -> Protocol -> Guidelines -> Citations -> RuntimeContext(dynamic)
    let env_pos = prompt.find("## Environment").unwrap();
    let protocol_pos = prompt.find("Response Protocol Tokens").unwrap();
    let guidelines_pos = prompt.find("System Operational Awareness").unwrap();
    let citation_pos = prompt.find("Citation Standards").unwrap();
    let runtime_pos = prompt.find("## Runtime Environment").unwrap();

    assert!(
        env_pos < protocol_pos,
        "Environment should appear before Protocol tokens"
    );
    assert!(
        protocol_pos < guidelines_pos,
        "Protocol tokens should appear before Operational guidelines"
    );
    assert!(
        citation_pos < runtime_pos,
        "RuntimeContext (dynamic) should appear after stable layers"
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
        current_time: "2026-03-30 02:30:00".to_string(),
        current_time_ms: 1774852200000,
        timezone: "UTC".to_string(),
    });

    let prompt = builder.build_system_prompt_with_context(&resolved);

    // 1. RuntimeContext SHOULD be present (always injected when provided)
    assert!(
        prompt.contains("## Runtime Environment"),
        "RuntimeContext should be present in WebRich mode"
    );
    assert!(
        prompt.contains("os=linux"),
        "Missing OS info in WebRich mode"
    );
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
