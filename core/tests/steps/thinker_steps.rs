//! Step definitions for thinker prompt builder features

use crate::world::{AlephWorld, ThinkerContext};
use alephcore::agent_loop::{Observation, StepSummary, ToolInfo};
use alephcore::thinker::{MessageRole, PromptConfig};
use cucumber::{gherkin::Step, given, then, when};

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
