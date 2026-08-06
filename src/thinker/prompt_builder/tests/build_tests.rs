//! Tests that call build_* entry points
//
// (The routing-experience/memory system-prompt injection tests were removed
// 2026-07-03: per-query recall no longer rides the system prompt — it is
// delivered as a transient trailing user message via
// `HarnessDeps::recall_context`.)

use super::super::*;

// ========== Integration tests: public API via Pipeline ==========

// (`test_thinking_guidance_*` removed with `ThinkingGuidanceLayer` —
// `PromptConfig.thinking_transparency` had no production writer, so the layer
// could never fire outside its own tests.)

#[test]
fn phase3_with_resolved_context_basic_path_emits_operational_guidelines() {
    // Phase 3 wiring: `PromptBuilder::with_resolved_context(...)` must
    // thread a `ResolvedContext` into the `Basic` assembly path so the
    // Phase 2 widened layers fire on the harness route (which calls
    // `build_system_prompt`).
    //
    // The harness-bridge default is `Background` paradigm + permissive
    // security — under those settings `OperationalGuidelinesLayer` must
    // emit its `## System Operational Awareness` block.
    use crate::thinker::context::ContextAggregator;
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::security_context::SecurityContext;

    let interaction = InteractionManifest::new(InteractionParadigm::Background);
    let security = SecurityContext::permissive();
    let resolved = ContextAggregator::resolve(&interaction, &security);

    let builder = PromptBuilder::new(PromptConfig::default()).with_resolved_context(resolved);
    let prompt = builder.build_system_prompt(&[]);

    assert!(
        prompt.contains("## System Operational Awareness"),
        "OperationalGuidelinesLayer must emit on Basic path when resolved_context is attached"
    );
    assert!(
        prompt.contains("Never autonomously restart"),
        "operational guidelines must retain the never-restart safety rail"
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
        !prompt.contains("<environment_context>"),
        "RuntimeContextLayer should stay silent without runtime_context attached"
    );
}

#[test]
fn phase3_with_behavior_name_openai_emits_delta_on_basic_path() {
    // `ProviderGuidanceLayer` no longer hardcodes a shared tool-use /
    // persistence baseline (§1.1 prune-the-prompt); it emits only the per-family
    // `.md` delta threaded via `with_model_behavior_delta`. With a delta and a
    // behavior name, it surfaces on the `Basic` assembly path.
    let builder = PromptBuilder::new(PromptConfig::default())
        .with_behavior_name("openai")
        .with_model_behavior_delta(Some("## OpenAI Family\nAct, don't ask".to_string()));
    let prompt = builder.build_system_prompt(&[]);
    assert!(
        prompt.contains("Act, don't ask"),
        "OpenAI family delta must surface on the Basic path"
    );
    // The old hardcoded baseline is gone.
    assert!(!prompt.contains("## Tool-Use Enforcement"));
}

#[test]
fn phase3_with_behavior_name_anthropic_stays_silent_on_basic_path() {
    let builder = PromptBuilder::new(PromptConfig::default()).with_behavior_name("anthropic");
    let prompt = builder.build_system_prompt(&[]);
    assert!(
        !prompt.contains("## Tool-Use Enforcement"),
        "Anthropic protocol must not emit tool-use enforcement (Claude is well-behaved)"
    );
    assert!(
        !prompt.contains("## Execution Discipline — OpenAI Family"),
        "Anthropic protocol must not emit OpenAI-family execution discipline"
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
    // must emit its single-line `<environment_context>` summary even on
    // the Basic path. Mirrors `harness_bridge::build_system_prompt`
    // populating the field with `RuntimeContext::collect(...)`.
    use crate::thinker::context::ContextAggregator;
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::runtime_context::RuntimeContext;
    use crate::thinker::security_context::SecurityContext;

    let interaction = InteractionManifest::new(InteractionParadigm::Background);
    let security = SecurityContext::permissive();
    let mut resolved = ContextAggregator::resolve(&interaction, &security);
    resolved.runtime_context = Some(RuntimeContext {
        os: "linux".to_string(),
        arch: "aarch64".to_string(),
        shell: "fish".to_string(),
        working_dir: std::path::PathBuf::from("/srv/aleph"),
        repo_root: None,
        current_model: "test-provider".to_string(),
        hostname: "ci-runner".to_string(),
        current_time: "2026-05-20 12:00:00".to_string(),
        timezone: "UTC".to_string(),
    });

    let builder = PromptBuilder::new(PromptConfig::default()).with_resolved_context(resolved);
    let prompt = builder.build_system_prompt(&[]);

    assert!(
        prompt.contains("<environment_context>"),
        "RuntimeContextLayer must emit on Basic path when runtime_context is populated"
    );
    // Per-run / per-hour facts ride the Dynamic runtime line…
    assert!(prompt.contains("cwd=/srv/aleph"));
    assert!(prompt.contains("model=test-provider"));
    assert!(prompt.contains("(UTC)"));
    // …while the process-invariant ones are stated ONCE, by the Stable
    // `## Environment` section, in its Markdown-bullet shape.
    assert!(prompt.contains("- **OS**: linux (aarch64)"), "{prompt}");
    assert!(prompt.contains("- **Shell**: fish"), "{prompt}");
    assert!(prompt.contains("- **Host**: ci-runner"), "{prompt}");
    // Neither half may restate the other's facts (R9).
    assert!(!prompt.contains("os=linux"), "{prompt}");
    assert!(!prompt.contains("arch=aarch64"), "{prompt}");
    assert!(!prompt.contains("shell=fish"), "{prompt}");
    assert!(!prompt.contains("host=ci-runner"), "{prompt}");
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
    let resolved = ContextAggregator::resolve(&interaction, &security);

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
fn phase5_messaging_paradigm_security_context_announces_approval_required() {
    // Phase 5 F2: harness_bridge now derives SecurityContext from the
    // InteractionManifest paradigm via `SecurityContext::for_paradigm`.
    // Messaging paradigm must surface the Standard-sandbox + approval-
    // required posture in the SecurityLayer output so the LLM is told
    // to be cautious about elevated operations on public-channel bots.
    use crate::thinker::context::ContextAggregator;
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::security_context::SecurityContext;

    let interaction = InteractionManifest::new(InteractionParadigm::Messaging);
    let security = SecurityContext::for_paradigm(InteractionParadigm::Messaging);
    let resolved = ContextAggregator::resolve(&interaction, &security);

    let builder = PromptBuilder::new(PromptConfig::default()).with_resolved_context(resolved);
    let prompt = builder.build_system_prompt(&[]);

    assert!(
        prompt.contains("Security Level: Standard"),
        "Messaging paradigm must surface Standard sandbox baseline"
    );
    assert!(
        prompt.contains("Elevated Operations: Require user approval"),
        "Messaging paradigm must surface approval-required posture"
    );
}

#[test]
fn phase5_cli_paradigm_security_context_stays_permissive() {
    // Phase 5 F2 negative test: CLI paradigm must preserve the existing
    // permissive baseline — the LLM sees "Security Level: None" and is
    // not told about elevated-operation restrictions.
    use crate::thinker::context::ContextAggregator;
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::security_context::SecurityContext;

    let interaction = InteractionManifest::new(InteractionParadigm::CLI);
    let security = SecurityContext::for_paradigm(InteractionParadigm::CLI);
    let resolved = ContextAggregator::resolve(&interaction, &security);

    let builder = PromptBuilder::new(PromptConfig::default()).with_resolved_context(resolved);
    let prompt = builder.build_system_prompt(&[]);

    assert!(prompt.contains("Security Level: None"));
    assert!(
        !prompt.contains("Elevated Operations: Require user approval"),
        "CLI paradigm must not announce approval requirement (permissive posture)"
    );
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
    let mut ctx = ContextAggregator::resolve(&interaction, &security);

    ctx.runtime_context = Some(crate::thinker::runtime_context::RuntimeContext {
        os: "linux".to_string(),
        arch: "x86_64".to_string(),
        shell: "bash".to_string(),
        working_dir: std::path::PathBuf::from("/home/user"),
        repo_root: None,
        current_model: "gpt-4".to_string(),
        hostname: "server-01".to_string(),
        current_time: "2026-03-30 02:30:00".to_string(),
        timezone: "UTC".to_string(),
    });

    let prompt = builder.with_resolved_context(ctx).build_system_prompt(&[]);

    // Runtime context should be present. `os=` moved to the Stable
    // `## Environment` bullet (`- **OS**: linux (x86_64)`); the Dynamic line owns
    // the per-run facts.
    assert!(prompt.contains("<environment_context>"));
    assert!(prompt.contains("<cwd>/home/user</cwd>"));
    assert!(prompt.contains("<model>gpt-4</model>"));
    assert!(prompt.contains("- **OS**: linux (x86_64)"));

    // Runtime context is a dynamic layer (priority 1710) so it appears
    // after stable layers like environment (priority 300).
    let runtime_pos = prompt.find("<environment_context>").unwrap();
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
    let ctx = ContextAggregator::resolve(&interaction, &security);

    // runtime_context should be None by default
    assert!(ctx.runtime_context.is_none());

    let prompt = builder.with_resolved_context(ctx).build_system_prompt(&[]);

    // Runtime context section should NOT be present
    assert!(!prompt.contains("<environment_context>"));
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
    let mut resolved = ContextAggregator::resolve(&interaction, &security);

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
        timezone: "Asia/Shanghai".to_string(),
    });

    // Cached is the production path where every enhancement layer fires
    // (CitationStandardsLayer is Soul+Cached, not Basic); join the stable +
    // dynamic parts to inspect the full assembled prompt.
    let parts = builder
        .with_resolved_context(resolved)
        .build_system_prompt_cached_with_mode(&[], crate::thinker::prompt_mode::PromptMode::Full);
    let prompt = parts
        .iter()
        .map(|p| p.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    // 1. RuntimeContext should be present
    assert!(
        prompt.contains("<environment_context>"),
        "Missing RuntimeContext section"
    );
    // OS is stated once, by the Stable `## Environment` bullet — not by the
    // Dynamic runtime line, which owns only per-run / per-hour facts.
    assert!(
        prompt.contains("- **OS**: macOS 15.3"),
        "Missing OS info: {prompt}"
    );
    assert!(!prompt.contains("os=macOS 15.3"), "OS stated twice");
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
        prompt.contains("Never autonomously restart"),
        "operational guidelines must retain the never-restart safety rail"
    );

    // 4. Citation standards should be present (always injected)
    assert!(
        prompt.contains("Citation Standards"),
        "Missing citation standards"
    );
    assert!(
        prompt.contains("never fabricate a source"),
        "Missing citation requirement"
    );

    // Standard sections should still be present
    assert!(
        prompt.contains("You are an AI assistant"),
        "Missing role section"
    );

    // Verify ordering: Environment -> Protocol -> Guidelines -> Citations -> RuntimeContext(dynamic)
    let env_pos = prompt.find("## Environment").unwrap();
    let protocol_pos = prompt.find("Response Protocol Tokens").unwrap();
    let guidelines_pos = prompt.find("System Operational Awareness").unwrap();
    let citation_pos = prompt.find("Citation Standards").unwrap();
    let runtime_pos = prompt.find("<environment_context>").unwrap();

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
fn with_strategy_appends_welded_strategy_block() {
    let agent = crate::agents::AgentDef::new("explore", crate::agents::AgentMode::SubAgent);
    let body = "Objective: ship the parser.\nGuardrails:\n- no network calls";

    let builder = PromptBuilder::new(PromptConfig::default()).with_agent(agent.clone());
    let without = builder.build_system_prompt(&[]);

    let builder_s = PromptBuilder::new(PromptConfig::default())
        .with_agent(agent)
        .with_strategy(body.to_string());
    let with = builder_s.build_system_prompt(&[]);

    // The welded block is present and wrapped exactly like StrategyLayer.
    assert!(with.contains("<strategy>\n"));
    assert!(with.contains("</strategy>\n"));
    assert!(with.contains("ship the parser."));
    // The strategy block is appended; the body without it is a strict prefix.
    assert!(with.starts_with(&without));
    assert_eq!(
        &with[without.len()..],
        &format!("<strategy>\n{body}\n</strategy>\n\n")
    );
}

#[test]
fn without_strategy_is_byte_identical() {
    let agent = crate::agents::AgentDef::new("explore", crate::agents::AgentMode::SubAgent);
    let a = PromptBuilder::new(PromptConfig::default())
        .with_agent(agent.clone())
        .build_system_prompt(&[]);
    let b = PromptBuilder::new(PromptConfig::default())
        .with_agent(agent)
        .build_system_prompt(&[]);
    assert_eq!(a, b);
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
    let mut resolved = ContextAggregator::resolve(&interaction, &security);

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
        timezone: "UTC".to_string(),
    });

    // Cached is the production path where every enhancement layer fires
    // (CitationStandardsLayer is Soul+Cached, not Basic); join the stable +
    // dynamic parts to inspect the full assembled prompt.
    let parts = builder
        .with_resolved_context(resolved)
        .build_system_prompt_cached_with_mode(&[], crate::thinker::prompt_mode::PromptMode::Full);
    let prompt = parts
        .iter()
        .map(|p| p.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    // 1. RuntimeContext SHOULD be present (always injected when provided)
    assert!(
        prompt.contains("<environment_context>"),
        "RuntimeContext should be present in WebRich mode"
    );
    assert!(
        prompt.contains("- **OS**: linux"),
        "Missing OS info in WebRich mode: {prompt}"
    );
    assert!(!prompt.contains("os=linux"), "OS stated twice");
    assert!(
        prompt.contains("<model>gpt-4</model>"),
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
        prompt.contains("never fabricate a source"),
        "Citation requirement should be present in WebRich mode"
    );

    // Standard sections should be present
    assert!(
        prompt.contains("You are an AI assistant"),
        "Missing role section"
    );
}
