//! Step definitions for thinker prompt builder features

use crate::world::{AlephWorld, ThinkerContext};
use alephcore::agent_loop::{Observation, StepSummary, ToolInfo};
use alephcore::thinker::{
    Capability, ContextAggregator, DisableReason, InteractionManifest, InteractionParadigm,
    MessageRole, PromptConfig, SecurityContext,
};
use cucumber::{gherkin::Step, given, then, when};
use std::path::PathBuf;

// ═══════════════════════════════════════════════════════════════════════════
// Given Steps
// ═══════════════════════════════════════════════════════════════════════════

#[given("a default prompt builder")]
async fn given_default_prompt_builder(w: &mut AlephWorld) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.config = PromptConfig::default();
    ctx.init_builder();
}

#[given("a prompt builder with runtime capabilities:")]
async fn given_builder_with_runtime(w: &mut AlephWorld, step: &Step) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.config = PromptConfig::default();
    if let Some(docstring) = step.docstring.as_ref() {
        ctx.config.runtime_capabilities = Some(docstring.clone());
    }
    ctx.init_builder();
}

#[given("a prompt builder with tool index:")]
async fn given_builder_with_tool_index(w: &mut AlephWorld, step: &Step) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.config = PromptConfig::default();
    if let Some(docstring) = step.docstring.as_ref() {
        ctx.config.tool_index = Some(docstring.clone());
    }
    ctx.init_builder();
}

#[given("a prompt builder with skill mode enabled")]
async fn given_builder_with_skill_mode(w: &mut AlephWorld) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.config = PromptConfig::default();
    ctx.config.skill_mode = true;
    ctx.init_builder();
}

#[given("tools:")]
async fn given_tools(w: &mut AlephWorld, step: &Step) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);

    if let Some(table) = step.table.as_ref() {
        for row in table.rows.iter().skip(1) {
            // Skip header row
            if row.len() >= 3 {
                let name = row[0].clone();
                let description = row[1].clone();
                let schema = row[2].clone();
                ctx.add_tool(&name, &description, &schema);
            }
        }
    }
}

#[given(expr = "an observation with history {string}")]
async fn given_observation_with_history(w: &mut AlephWorld, history: String) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.observation = Some(Observation {
        history_summary: history,
        recent_steps: vec![],
        available_tools: vec![],
        attachments: vec![],
        current_step: 0,
        total_tokens: 0,
    });
}

#[given(expr = "a recent step with action {string} and result {string}")]
async fn given_recent_step(w: &mut AlephWorld, action: String, result: String) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    if let Some(obs) = ctx.observation.as_mut() {
        obs.recent_steps.push(StepSummary {
            step_id: obs.recent_steps.len(),
            reasoning: "Need to search".to_string(),
            action_type: action,
            action_args: r#"{"query": "rust"}"#.to_string(),
            result_summary: result.clone(),
            result_output: format!(r#"{{"results": 10, "items": []}}"#),
            success: true,
        });
        obs.current_step = obs.recent_steps.len();
        obs.total_tokens = 500;
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// When Steps
// ═══════════════════════════════════════════════════════════════════════════

#[when("I build the system prompt")]
async fn when_build_system_prompt(w: &mut AlephWorld) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.build_system_prompt();
}

#[when("I build the cached system prompt")]
async fn when_build_cached_system_prompt(w: &mut AlephWorld) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.build_cached_prompt();
}

#[when(expr = "I build messages for query {string}")]
async fn when_build_messages(w: &mut AlephWorld, query: String) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.build_messages(&query);
}

#[when("I build a second cached prompt with tools:")]
async fn when_build_second_cached_with_tools(w: &mut AlephWorld, step: &Step) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);

    // Store the first cached parts before building second
    ctx.second_cached_parts = ctx.cached_parts.clone();

    // Parse tools from table
    let mut second_tools = Vec::new();
    if let Some(table) = step.table.as_ref() {
        for row in table.rows.iter().skip(1) {
            if row.len() >= 3 {
                second_tools.push(ToolInfo {
                    name: row[0].clone(),
                    description: row[1].clone(),
                    parameters_schema: row[2].clone(),
                    category: None,
                });
            }
        }
    }

    // Build second cached prompt with new tools
    if let Some(builder) = &ctx.builder {
        let second_parts = builder.build_system_prompt_cached(&second_tools);
        // Swap: move current to second_cached, put new in cached_parts
        ctx.second_cached_parts = ctx.cached_parts.take();
        ctx.cached_parts = Some(second_parts);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Then Steps - Prompt Content
// ═══════════════════════════════════════════════════════════════════════════

#[then(expr = "the prompt should contain {string}")]
async fn then_prompt_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.thinker.as_ref().expect("Thinker context not initialized");
    assert!(
        ctx.prompt_contains(&expected),
        "Expected prompt to contain '{}', but it didn't. Prompt:\n{}",
        expected,
        ctx.system_prompt.as_deref().unwrap_or("<none>")
    );
}

#[then(expr = "the prompt should not contain {string}")]
async fn then_prompt_not_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.thinker.as_ref().expect("Thinker context not initialized");
    assert!(
        ctx.prompt_not_contains(&expected),
        "Expected prompt NOT to contain '{}', but it did",
        expected
    );
}

#[then(expr = "{string} should appear before {string}")]
async fn then_section_appears_before(w: &mut AlephWorld, first: String, second: String) {
    let ctx = w.thinker.as_ref().expect("Thinker context not initialized");
    let prompt = ctx.system_prompt.as_ref().expect("No system prompt built");

    let first_pos = prompt.find(&first).expect(&format!("'{}' not found in prompt", first));
    let second_pos = prompt.find(&second).expect(&format!("'{}' not found in prompt", second));

    assert!(
        first_pos < second_pos,
        "'{}' (at {}) should appear before '{}' (at {})",
        first, first_pos, second, second_pos
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Then Steps - Messages
// ═══════════════════════════════════════════════════════════════════════════

#[then(expr = "messages should have at least {int} entries")]
async fn then_messages_have_at_least(w: &mut AlephWorld, count: i32) {
    let ctx = w.thinker.as_ref().expect("Thinker context not initialized");
    let actual = ctx.messages_count();
    assert!(
        actual >= count as usize,
        "Expected at least {} messages, got {}",
        count,
        actual
    );
}

#[then("the first message should be from User")]
async fn then_first_message_from_user(w: &mut AlephWorld) {
    let ctx = w.thinker.as_ref().expect("Thinker context not initialized");
    let first = ctx.first_message().expect("No messages built");
    assert_eq!(
        first.role,
        MessageRole::User,
        "Expected first message to be from User, got {:?}",
        first.role
    );
}

#[then(expr = "the first message should contain {string}")]
async fn then_first_message_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.thinker.as_ref().expect("Thinker context not initialized");
    let first = ctx.first_message().expect("No messages built");
    assert!(
        first.content.contains(&expected),
        "Expected first message to contain '{}', got: {}",
        expected,
        first.content
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Then Steps - Cached Parts
// ═══════════════════════════════════════════════════════════════════════════

#[then(expr = "cached parts should have {int} entries")]
async fn then_cached_parts_count(w: &mut AlephWorld, count: i32) {
    let ctx = w.thinker.as_ref().expect("Thinker context not initialized");
    assert_eq!(
        ctx.cached_parts_count(),
        count as usize,
        "Expected {} cached parts, got {}",
        count,
        ctx.cached_parts_count()
    );
}

#[then("the first cached part should be marked for caching")]
async fn then_first_part_cached(w: &mut AlephWorld) {
    let ctx = w.thinker.as_ref().expect("Thinker context not initialized");
    assert!(
        ctx.first_part_is_cached(),
        "Expected first cached part to have cache=true"
    );
}

#[then("the second cached part should not be marked for caching")]
async fn then_second_part_not_cached(w: &mut AlephWorld) {
    let ctx = w.thinker.as_ref().expect("Thinker context not initialized");
    assert!(
        ctx.second_part_not_cached(),
        "Expected second cached part to have cache=false"
    );
}

#[then("the first part headers should be identical")]
async fn then_headers_identical(w: &mut AlephWorld) {
    let ctx = w.thinker.as_ref().expect("Thinker context not initialized");

    let first_header = ctx.cached_parts
        .as_ref()
        .and_then(|v: &Vec<_>| v.first())
        .map(|p| &p.content)
        .expect("No first cached parts");

    let second_header = ctx.second_cached_parts
        .as_ref()
        .and_then(|v: &Vec<_>| v.first())
        .map(|p| &p.content)
        .expect("No second cached parts");

    assert_eq!(
        first_header, second_header,
        "Headers should be identical"
    );
}

#[then("the dynamic parts should be different")]
async fn then_dynamic_parts_different(w: &mut AlephWorld) {
    let ctx = w.thinker.as_ref().expect("Thinker context not initialized");

    let first_dynamic = ctx.cached_parts
        .as_ref()
        .and_then(|v: &Vec<_>| v.get(1))
        .map(|p| &p.content)
        .expect("No first cached parts");

    let second_dynamic = ctx.second_cached_parts
        .as_ref()
        .and_then(|v: &Vec<_>| v.get(1))
        .map(|p| &p.content)
        .expect("No second cached parts");

    assert_ne!(
        first_dynamic, second_dynamic,
        "Dynamic parts should be different"
    );
}

#[then(expr = "the header should contain {string}")]
async fn then_header_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.thinker.as_ref().expect("Thinker context not initialized");
    let header = ctx.get_header().expect("No header found");
    assert!(
        header.contains(&expected),
        "Expected header to contain '{}', but it didn't.\nHeader:\n{}",
        expected,
        header
    );
}

#[then(expr = "the dynamic part should contain {string}")]
async fn then_dynamic_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.thinker.as_ref().expect("Thinker context not initialized");
    let dynamic = ctx.get_dynamic().expect("No dynamic part found");
    assert!(
        dynamic.contains(&expected),
        "Expected dynamic part to contain '{}', but it didn't.\nDynamic:\n{}",
        expected,
        dynamic
    );
}

#[then(expr = "both prompts should contain {string}")]
async fn then_both_prompts_contain(w: &mut AlephWorld, expected: String) {
    let ctx = w.thinker.as_ref().expect("Thinker context not initialized");

    let full_prompt = ctx.system_prompt.as_ref().expect("No full prompt");
    let combined = ctx.get_combined_cached().expect("No cached parts");

    assert!(
        full_prompt.contains(&expected),
        "Expected full prompt to contain '{}', but it didn't",
        expected
    );
    assert!(
        combined.contains(&expected),
        "Expected combined cached prompt to contain '{}', but it didn't",
        expected
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Context Aggregation - Given Steps
// ═══════════════════════════════════════════════════════════════════════════

#[given("a web rich interaction manifest")]
async fn given_web_manifest(w: &mut AlephWorld) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.interaction = Some(InteractionManifest::new(InteractionParadigm::WebRich));
}

#[given("a CLI interaction manifest")]
async fn given_cli_manifest(w: &mut AlephWorld) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.interaction = Some(InteractionManifest::new(InteractionParadigm::CLI));
}

#[given("a messaging interaction manifest with inline buttons")]
async fn given_messaging_with_buttons(w: &mut AlephWorld) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    let mut manifest = InteractionManifest::new(InteractionParadigm::Messaging);
    manifest.add_capability(Capability::InlineButtons);
    ctx.interaction = Some(manifest);
}

#[given("a background interaction manifest")]
async fn given_background_manifest(w: &mut AlephWorld) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.interaction = Some(InteractionManifest::new(InteractionParadigm::Background));
}

#[given("a standard sandbox security context")]
async fn given_standard_security(w: &mut AlephWorld) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.security = Some(SecurityContext::standard_sandbox(PathBuf::from("/workspace")));
}

#[given("a strict readonly security context")]
async fn given_strict_security(w: &mut AlephWorld) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.security = Some(SecurityContext::strict_readonly(PathBuf::from("/workspace")));
}

#[given("a permissive security context")]
async fn given_permissive_security(w: &mut AlephWorld) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.security = Some(SecurityContext::permissive());
}

#[given(expr = "tools {string}")]
async fn given_tools_list(w: &mut AlephWorld, tools_str: String) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.tools.clear();
    for name in tools_str.split(',') {
        let name = name.trim();
        ctx.add_tool(name, &format!("{} tool", name), "{}");
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Context Aggregation - When Steps
// ═══════════════════════════════════════════════════════════════════════════

#[when("I aggregate the context")]
async fn when_aggregate(w: &mut AlephWorld) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    let interaction = ctx.interaction.as_ref().expect("No interaction manifest");
    let security = ctx.security.as_ref().expect("No security context");
    let tools = &ctx.tools;

    ctx.resolved = Some(ContextAggregator::resolve(interaction, security, tools));
}

#[when("I build the system prompt with context")]
async fn when_build_prompt_with_context(w: &mut AlephWorld) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);

    // First aggregate context
    let interaction = ctx.interaction.as_ref().expect("No interaction manifest");
    let security = ctx.security.as_ref().expect("No security context");
    let tools = &ctx.tools;
    let resolved = ContextAggregator::resolve(interaction, security, tools);

    // Build prompt with context
    ctx.init_builder();
    if let Some(builder) = &ctx.builder {
        ctx.system_prompt = Some(builder.build_system_prompt_with_context(&resolved));
    }
    ctx.resolved = Some(resolved);
}

// ═══════════════════════════════════════════════════════════════════════════
// Context Aggregation - Then Steps
// ═══════════════════════════════════════════════════════════════════════════

#[then(expr = "the environment contract paradigm should be {string}")]
async fn then_paradigm(w: &mut AlephWorld, expected: String) {
    let ctx = w.thinker.as_ref().expect("No thinker context");
    let resolved = ctx.resolved.as_ref().expect("No resolved context");
    let paradigm = format!("{:?}", resolved.environment_contract.paradigm);
    assert!(
        paradigm.contains(&expected),
        "Expected paradigm to contain '{}', got '{}'",
        expected,
        paradigm
    );
}

#[then(expr = "{string} should be available")]
async fn then_tool_available(w: &mut AlephWorld, tool_name: String) {
    let ctx = w.thinker.as_ref().expect("No thinker context");
    let resolved = ctx.resolved.as_ref().expect("No resolved context");
    assert!(
        resolved
            .available_tools
            .iter()
            .any(|t| t.name == tool_name),
        "Expected tool '{}' to be available, but it wasn't. Available: {:?}",
        tool_name,
        resolved
            .available_tools
            .iter()
            .map(|t| &t.name)
            .collect::<Vec<_>>()
    );
}

#[then(expr = "{string} should require approval")]
async fn then_requires_approval(w: &mut AlephWorld, tool_name: String) {
    let ctx = w.thinker.as_ref().expect("No thinker context");
    let resolved = ctx.resolved.as_ref().expect("No resolved context");
    assert!(
        resolved.disabled_tools.iter().any(|d| d.name == tool_name
            && matches!(d.reason, DisableReason::RequiresApproval { .. })),
        "Expected tool '{}' to require approval, but it didn't. Disabled tools: {:?}",
        tool_name,
        resolved
            .disabled_tools
            .iter()
            .map(|d| (&d.name, &d.reason))
            .collect::<Vec<_>>()
    );
}

#[then(expr = "{string} should be blocked by policy")]
async fn then_blocked(w: &mut AlephWorld, tool_name: String) {
    let ctx = w.thinker.as_ref().expect("No thinker context");
    let resolved = ctx.resolved.as_ref().expect("No resolved context");
    assert!(
        resolved.disabled_tools.iter().any(|d| d.name == tool_name
            && matches!(d.reason, DisableReason::BlockedByPolicy { .. })),
        "Expected tool '{}' to be blocked by policy, but it wasn't. Disabled tools: {:?}",
        tool_name,
        resolved
            .disabled_tools
            .iter()
            .map(|d| (&d.name, &d.reason))
            .collect::<Vec<_>>()
    );
}

#[then(expr = "{string} should be unsupported by channel")]
async fn then_unsupported_by_channel(w: &mut AlephWorld, tool_name: String) {
    let ctx = w.thinker.as_ref().expect("No thinker context");
    let resolved = ctx.resolved.as_ref().expect("No resolved context");
    assert!(
        resolved.disabled_tools.iter().any(|d| d.name == tool_name
            && matches!(d.reason, DisableReason::UnsupportedByChannel)),
        "Expected tool '{}' to be unsupported by channel, but it wasn't. Disabled tools: {:?}",
        tool_name,
        resolved
            .disabled_tools
            .iter()
            .map(|d| (&d.name, &d.reason))
            .collect::<Vec<_>>()
    );
}

#[then(expr = "the environment contract should have {string} capability")]
async fn then_has_capability(w: &mut AlephWorld, cap_name: String) {
    let ctx = w.thinker.as_ref().expect("No thinker context");
    let resolved = ctx.resolved.as_ref().expect("No resolved context");

    let has_cap = resolved
        .environment_contract
        .active_capabilities
        .iter()
        .any(|c| {
            let (name, _) = c.prompt_hint();
            name == cap_name
        });

    assert!(
        has_cap,
        "Expected environment contract to have '{}' capability, but it didn't. Capabilities: {:?}",
        cap_name,
        resolved
            .environment_contract
            .active_capabilities
            .iter()
            .map(|c| c.prompt_hint().0)
            .collect::<Vec<_>>()
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Embodiment Engine - Given Steps
// ═══════════════════════════════════════════════════════════════════════════

#[given("a soul file with content:")]
async fn given_soul_file_content(w: &mut AlephWorld, step: &Step) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    if let Some(docstring) = step.docstring.as_ref() {
        ctx.soul_content = Some(docstring.clone());
    }
}

#[given(expr = "a global soul with identity {string}")]
async fn given_global_soul(w: &mut AlephWorld, identity: String) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.init_identity_resolver();
    // Create a soul that will be returned by global path
    ctx.soul = Some(alephcore::thinker::soul::SoulManifest {
        identity,
        ..Default::default()
    });
}

#[given(expr = "a session override soul with identity {string}")]
async fn given_session_override(w: &mut AlephWorld, identity: String) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    if ctx.identity_resolver.is_none() {
        ctx.init_identity_resolver();
    }
    let override_soul = alephcore::thinker::soul::SoulManifest {
        identity,
        ..Default::default()
    };
    if let Some(resolver) = ctx.identity_resolver.as_mut() {
        resolver.set_session_override(override_soul);
    }
}

#[given("no soul files configured")]
async fn given_no_soul_files(w: &mut AlephWorld) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.init_identity_resolver();
}

#[given(expr = "a soul with identity {string}")]
async fn given_soul_with_identity(w: &mut AlephWorld, identity: String) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    let mut soul = ctx.soul.take().unwrap_or_default();
    soul.identity = identity;
    ctx.soul = Some(soul);
}

#[given(expr = "a soul with directive {string}")]
async fn given_soul_with_directive(w: &mut AlephWorld, directive: String) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    let mut soul = ctx.soul.take().unwrap_or_default();
    soul.directives.push(directive);
    ctx.soul = Some(soul);
}

#[given("an empty soul")]
async fn given_empty_soul(w: &mut AlephWorld) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.soul = Some(alephcore::thinker::soul::SoulManifest::default());
}

// ═══════════════════════════════════════════════════════════════════════════
// Embodiment Engine - When Steps
// ═══════════════════════════════════════════════════════════════════════════

#[when("I parse the soul file")]
async fn when_parse_soul_file(w: &mut AlephWorld) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.parse_soul_content();
}

#[when("I resolve identity")]
async fn when_resolve_identity(w: &mut AlephWorld) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    if let Some(resolver) = &ctx.identity_resolver {
        // If we have a soul set (simulating global), use it
        // Otherwise, resolve from resolver
        if ctx.soul.is_none() {
            ctx.soul = Some(resolver.resolve());
        }
    }
}

#[when("I clear the session override")]
async fn when_clear_session_override(w: &mut AlephWorld) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    if let Some(resolver) = ctx.identity_resolver.as_mut() {
        resolver.clear_session_override();
    }
    // Clear the cached soul to force re-resolution
    ctx.soul = None;
}

#[when("I build the system prompt with soul")]
async fn when_build_prompt_with_soul(w: &mut AlephWorld) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.init_builder();
    ctx.build_system_prompt_with_soul();
}

// ═══════════════════════════════════════════════════════════════════════════
// Embodiment Engine - Then Steps
// ═══════════════════════════════════════════════════════════════════════════

#[then(expr = "the soul identity should contain {string}")]
async fn then_soul_identity_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.thinker.as_ref().expect("No thinker context");
    let soul = ctx.soul.as_ref().expect("No soul parsed");
    assert!(
        soul.identity.contains(&expected),
        "Expected identity to contain '{}', got '{}'",
        expected,
        soul.identity
    );
}

#[then(expr = "the soul should have {int} directives")]
async fn then_soul_directives_count(w: &mut AlephWorld, count: i32) {
    let ctx = w.thinker.as_ref().expect("No thinker context");
    let soul = ctx.soul.as_ref().expect("No soul parsed");
    assert_eq!(
        soul.directives.len(),
        count as usize,
        "Expected {} directives, got {}",
        count,
        soul.directives.len()
    );
}

#[then(expr = "the soul relationship should be {string}")]
async fn then_soul_relationship(w: &mut AlephWorld, expected: String) {
    let ctx = w.thinker.as_ref().expect("No thinker context");
    let soul = ctx.soul.as_ref().expect("No soul parsed");
    let relationship = format!("{:?}", soul.relationship);
    assert!(
        relationship.contains(&expected),
        "Expected relationship to contain '{}', got '{}'",
        expected,
        relationship
    );
}

#[then(expr = "the soul should have {int} expertise areas")]
async fn then_soul_expertise_count(w: &mut AlephWorld, count: i32) {
    let ctx = w.thinker.as_ref().expect("No thinker context");
    let soul = ctx.soul.as_ref().expect("No soul parsed");
    assert_eq!(
        soul.expertise.len(),
        count as usize,
        "Expected {} expertise areas, got {}",
        count,
        soul.expertise.len()
    );
}

#[then(expr = "the soul should have expertise {string}")]
async fn then_soul_has_expertise(w: &mut AlephWorld, expected: String) {
    let ctx = w.thinker.as_ref().expect("No thinker context");
    let soul = ctx.soul.as_ref().expect("No soul parsed");
    assert!(
        soul.expertise.iter().any(|e| e.contains(&expected)),
        "Expected soul to have expertise '{}', got {:?}",
        expected,
        soul.expertise
    );
}

#[then(expr = "the soul should have {int} anti-patterns")]
async fn then_soul_anti_patterns_count(w: &mut AlephWorld, count: i32) {
    let ctx = w.thinker.as_ref().expect("No thinker context");
    let soul = ctx.soul.as_ref().expect("No soul parsed");
    assert_eq!(
        soul.anti_patterns.len(),
        count as usize,
        "Expected {} anti-patterns, got {}",
        count,
        soul.anti_patterns.len()
    );
}

#[then(expr = "the soul anti-patterns should contain {string}")]
async fn then_soul_anti_patterns_contain(w: &mut AlephWorld, expected: String) {
    let ctx = w.thinker.as_ref().expect("No thinker context");
    let soul = ctx.soul.as_ref().expect("No soul parsed");
    assert!(
        soul.anti_patterns.iter().any(|a| a.contains(&expected)),
        "Expected anti-patterns to contain '{}', got {:?}",
        expected,
        soul.anti_patterns
    );
}

#[then(expr = "the effective identity should be {string}")]
async fn then_effective_identity(w: &mut AlephWorld, expected: String) {
    let ctx = w.thinker.as_ref().expect("No thinker context");
    if let Some(resolver) = &ctx.identity_resolver {
        let resolved = resolver.resolve();
        assert_eq!(
            resolved.identity, expected,
            "Expected identity '{}', got '{}'",
            expected, resolved.identity
        );
    } else {
        let soul = ctx.soul.as_ref().expect("No soul");
        assert_eq!(
            soul.identity, expected,
            "Expected identity '{}', got '{}'",
            expected, soul.identity
        );
    }
}

#[then("the soul should be empty")]
async fn then_soul_empty(w: &mut AlephWorld) {
    let ctx = w.thinker.as_ref().expect("No thinker context");
    let soul = ctx.soul.as_ref().expect("No soul");
    assert!(soul.is_empty(), "Expected soul to be empty, but it wasn't");
}

// ═══════════════════════════════════════════════════════════════════════════
// CoT Transparency - Given Steps
// ═══════════════════════════════════════════════════════════════════════════

#[given(expr = "reasoning text {string}")]
async fn given_reasoning_text(w: &mut AlephWorld, text: String) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.reasoning_text = Some(text);
}

#[given("reasoning text:")]
async fn given_reasoning_text_docstring(w: &mut AlephWorld, step: &Step) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    if let Some(docstring) = step.docstring.as_ref() {
        ctx.reasoning_text = Some(docstring.clone());
    }
}

#[given(expr = "a valid LLM response with reasoning {string}")]
async fn given_llm_response_with_reasoning(w: &mut AlephWorld, reasoning: String) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.reasoning_text = Some(reasoning);
}

#[given("a valid LLM response with no reasoning")]
async fn given_llm_response_no_reasoning(w: &mut AlephWorld) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.reasoning_text = None;
}

#[given("a prompt builder with thinking transparency enabled")]
async fn given_builder_with_thinking_transparency(w: &mut AlephWorld) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.config = PromptConfig::default();
    ctx.config.thinking_transparency = true;
    ctx.init_builder();
}

// ═══════════════════════════════════════════════════════════════════════════
// CoT Transparency - When Steps
// ═══════════════════════════════════════════════════════════════════════════

#[when("I parse the structured thinking")]
async fn when_parse_structured_thinking(w: &mut AlephWorld) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    ctx.parse_structured_thinking();
}

#[when("I parse the decision")]
async fn when_parse_decision(w: &mut AlephWorld) {
    let ctx = w.thinker.get_or_insert_with(ThinkerContext::new);
    // Parse structured thinking from reasoning text
    ctx.parse_structured_thinking();
}

// ═══════════════════════════════════════════════════════════════════════════
// CoT Transparency - Then Steps
// ═══════════════════════════════════════════════════════════════════════════

#[then(expr = "the first step should be type {string}")]
async fn then_first_step_type(w: &mut AlephWorld, expected_type: String) {
    let ctx = w.thinker.as_ref().expect("No thinker context");
    let thinking = ctx.structured_thinking.as_ref().expect("No structured thinking");
    let first_step = thinking.steps.first().expect("No steps");
    let actual_type = format!("{:?}", first_step.step_type);
    assert!(
        actual_type.contains(&expected_type),
        "Expected first step type '{}', got '{}'",
        expected_type,
        actual_type
    );
}

#[then(expr = "step {int} should be type {string}")]
async fn then_step_n_type(w: &mut AlephWorld, step_num: i32, expected_type: String) {
    let ctx = w.thinker.as_ref().expect("No thinker context");
    let thinking = ctx.structured_thinking.as_ref().expect("No structured thinking");
    let step = thinking.steps.get((step_num - 1) as usize)
        .expect(&format!("No step {}", step_num));
    let actual_type = format!("{:?}", step.step_type);
    assert!(
        actual_type.contains(&expected_type),
        "Expected step {} type '{}', got '{}'",
        step_num,
        expected_type,
        actual_type
    );
}

#[then(expr = "the confidence should be {string}")]
async fn then_confidence(w: &mut AlephWorld, expected: String) {
    let ctx = w.thinker.as_ref().expect("No thinker context");
    let thinking = ctx.structured_thinking.as_ref().expect("No structured thinking");
    let actual = format!("{:?}", thinking.confidence);
    assert!(
        actual.contains(&expected),
        "Expected confidence '{}', got '{}'",
        expected,
        actual
    );
}

#[then(expr = "the alternatives should contain {string}")]
async fn then_alternatives_contain(w: &mut AlephWorld, expected: String) {
    let ctx = w.thinker.as_ref().expect("No thinker context");
    let thinking = ctx.structured_thinking.as_ref().expect("No structured thinking");
    assert!(
        thinking.alternatives.iter().any(|a| a.contains(&expected)),
        "Expected alternatives to contain '{}', got {:?}",
        expected,
        thinking.alternatives
    );
}

#[then(expr = "the uncertainties should contain {string}")]
async fn then_uncertainties_contain(w: &mut AlephWorld, expected: String) {
    let ctx = w.thinker.as_ref().expect("No thinker context");
    let thinking = ctx.structured_thinking.as_ref().expect("No structured thinking");
    assert!(
        thinking.uncertainties.iter().any(|u| u.contains(&expected)),
        "Expected uncertainties to contain '{}', got {:?}",
        expected,
        thinking.uncertainties
    );
}

#[then(expr = "the structured thinking should have {int} steps")]
async fn then_structured_thinking_step_count(w: &mut AlephWorld, count: i32) {
    let ctx = w.thinker.as_ref().expect("No thinker context");
    let thinking = ctx.structured_thinking.as_ref().expect("No structured thinking");
    assert_eq!(
        thinking.steps.len(),
        count as usize,
        "Expected {} steps, got {}",
        count,
        thinking.steps.len()
    );
}

#[then("the decision should have structured thinking")]
async fn then_decision_has_structured(w: &mut AlephWorld) {
    let ctx = w.thinker.as_ref().expect("No thinker context");
    assert!(
        ctx.structured_thinking.is_some(),
        "Expected decision to have structured thinking"
    );
}

#[then("the decision should not have structured thinking")]
async fn then_decision_no_structured(w: &mut AlephWorld) {
    let ctx = w.thinker.as_ref().expect("No thinker context");
    assert!(
        ctx.structured_thinking.is_none() || ctx.reasoning_text.is_none(),
        "Expected decision to not have structured thinking"
    );
}

#[then(expr = "the structured thinking should have at least {int} step")]
async fn then_structured_thinking_at_least_steps(w: &mut AlephWorld, count: i32) {
    let ctx = w.thinker.as_ref().expect("No thinker context");
    let thinking = ctx.structured_thinking.as_ref().expect("No structured thinking");
    assert!(
        thinking.steps.len() >= count as usize,
        "Expected at least {} steps, got {}",
        count,
        thinking.steps.len()
    );
}
