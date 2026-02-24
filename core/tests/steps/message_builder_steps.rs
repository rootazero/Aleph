//! Step definitions for message builder features

use crate::world::{AlephWorld, MessageBuilderContext};
use alephcore::agent_loop::message_builder::{Message, MessageBuilderConfig, ToolCall};
use alephcore::components::{ExecutionSession, ToolCallStatus};
use cucumber::{gherkin::Step, given, then, when};
use serde_json::json;

// ═══════════════════════════════════════════════════════════════════════════
// Given Steps - Builder Configuration
// ═══════════════════════════════════════════════════════════════════════════

#[given("a default message builder")]
async fn given_default_message_builder(w: &mut AlephWorld) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    ctx.config = MessageBuilderConfig::default();
    ctx.config.inject_reminders = false; // Default to no reminders for simple tests
    ctx.init_builder();
}

#[given(expr = "a message builder with reminder threshold {int}")]
async fn given_builder_with_reminder_threshold(w: &mut AlephWorld, threshold: i32) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    ctx.config = MessageBuilderConfig::default()
        .with_inject_reminders(true)
        .with_reminder_threshold(threshold as u32);
    ctx.init_builder();
}

#[given(expr = "a message builder with max messages {int}")]
async fn given_builder_with_max_messages(w: &mut AlephWorld, max: i32) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    ctx.config = MessageBuilderConfig::default()
        .with_max_messages(max as usize)
        .with_inject_reminders(false);
    ctx.init_builder();
}

#[given(expr = "a message builder with inject reminders enabled and threshold {int}")]
async fn given_builder_with_reminders_and_threshold(w: &mut AlephWorld, threshold: i32) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    ctx.config = MessageBuilderConfig::default()
        .with_inject_reminders(true)
        .with_reminder_threshold(threshold as u32);
    ctx.init_builder();
}

#[given("a message builder with compactor")]
async fn given_builder_with_compactor(w: &mut AlephWorld) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    ctx.config = MessageBuilderConfig::default().with_inject_reminders(false);
    ctx.setup_compactor();
    ctx.init_builder_with_compactor();
}

#[given("a message builder with overflow detector")]
async fn given_builder_with_overflow_detector(w: &mut AlephWorld) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    ctx.config = MessageBuilderConfig::default().with_inject_reminders(false);
    ctx.setup_testing_overflow_detector();
    ctx.init_builder_with_overflow_detector();
}

#[given("a message builder with overflow detector and reminders enabled")]
async fn given_builder_with_overflow_detector_and_reminders(w: &mut AlephWorld) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    ctx.config = MessageBuilderConfig::default()
        .with_inject_reminders(true)
        .with_reminder_threshold(1);
    ctx.setup_testing_overflow_detector();
    ctx.init_builder_with_overflow_detector();
}

#[given("a message builder with compactor and overflow detector")]
async fn given_builder_with_compactor_and_overflow_detector(w: &mut AlephWorld) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    ctx.config = MessageBuilderConfig::default();
    ctx.setup_compactor();
    ctx.setup_testing_overflow_detector();
    ctx.init_builder_with_all();
}

#[given(expr = "a message builder with {word} compactor and {word} overflow detector via with_all")]
async fn given_builder_with_optional_components(
    w: &mut AlephWorld,
    compactor_state: String,
    detector_state: String,
) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    ctx.config = MessageBuilderConfig::default();

    if compactor_state == "enabled" {
        ctx.setup_compactor();
    }
    if detector_state == "enabled" {
        ctx.setup_testing_overflow_detector();
    }
    ctx.init_builder_with_all();
}

#[given(expr = "a message builder with max iterations {int} and reminders enabled")]
async fn given_builder_with_max_iterations(w: &mut AlephWorld, max: i32) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    ctx.config = MessageBuilderConfig::default()
        .with_inject_reminders(true)
        .with_reminder_threshold(0)
        .with_max_iterations(max as u32);
    ctx.init_builder();
}

#[given("a message builder with default max iterations")]
async fn given_builder_with_default_max_iterations(w: &mut AlephWorld) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    ctx.config = MessageBuilderConfig::default();
    // Verify default is 50
    assert_eq!(ctx.config.max_iterations, 50);
    ctx.init_builder();
}

// ═══════════════════════════════════════════════════════════════════════════
// Given Steps - Session Parts
// ═══════════════════════════════════════════════════════════════════════════

#[given(expr = "a user input part with text {string}")]
async fn given_user_input_part(w: &mut AlephWorld, text: String) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    ctx.add_user_input(&text, None, 1000);
}

#[given(expr = "a user input part with text {string} and context {string}")]
async fn given_user_input_part_with_context(w: &mut AlephWorld, text: String, context: String) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    ctx.add_user_input(&text, Some(&context), 1000);
}

#[given(expr = "a user input part with text {string} at timestamp {int}")]
async fn given_user_input_part_at_timestamp(w: &mut AlephWorld, text: String, timestamp: i64) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    ctx.add_user_input(&text, None, timestamp);
}

#[given("a tool call part:")]
async fn given_tool_call_part(w: &mut AlephWorld, step: &Step) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);

    if let Some(table) = step.table.as_ref() {
        let mut id = String::new();
        let mut tool_name = String::new();
        let mut input = json!({});
        let mut status = ToolCallStatus::Completed;
        let mut output: Option<&str> = None;
        let mut error: Option<&str> = None;

        for row in &table.rows {
            if row.len() >= 2 {
                match row[0].as_str() {
                    "id" => id = row[1].clone(),
                    "tool_name" => tool_name = row[1].clone(),
                    "input" => input = serde_json::from_str(&row[1]).unwrap_or(json!({})),
                    "status" => {
                        status = match row[1].as_str() {
                            "Completed" => ToolCallStatus::Completed,
                            "Failed" => ToolCallStatus::Failed,
                            "Running" => ToolCallStatus::Running,
                            "Pending" => ToolCallStatus::Pending,
                            "Aborted" => ToolCallStatus::Aborted,
                            _ => ToolCallStatus::Completed,
                        }
                    }
                    "output" => output = Some(Box::leak(row[1].clone().into_boxed_str())),
                    "error" => error = Some(Box::leak(row[1].clone().into_boxed_str())),
                    _ => {}
                }
            }
        }

        ctx.add_tool_call(&id, &tool_name, input, status, output, error);
    }
}

#[given(expr = "an AI response part with content {string} at timestamp {int}")]
async fn given_ai_response_part_at_timestamp(w: &mut AlephWorld, content: String, timestamp: i64) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    ctx.add_ai_response(&content, None, timestamp);
}

#[given(expr = "an AI response part with empty content and reasoning {string}")]
async fn given_ai_response_with_reasoning_only(w: &mut AlephWorld, reasoning: String) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    ctx.add_ai_response("", Some(&reasoning), 1000);
}

#[given(expr = "a summary part with content {string}")]
async fn given_summary_part(w: &mut AlephWorld, content: String) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    ctx.add_summary(&content, 10, 5000);
}

#[given(expr = "a summary part with content {string} compacted at {int}")]
async fn given_summary_part_with_compacted_at(w: &mut AlephWorld, content: String, compacted_at: i64) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    ctx.add_summary(&content, 5, compacted_at);
}

#[given(expr = "a compaction marker at timestamp {int}")]
async fn given_compaction_marker(w: &mut AlephWorld, timestamp: i64) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    ctx.add_compaction_marker(timestamp, true);
}

// ═══════════════════════════════════════════════════════════════════════════
// Given Steps - Messages (for inject_reminders tests)
// ═══════════════════════════════════════════════════════════════════════════

#[given("messages:")]
async fn given_messages(w: &mut AlephWorld, step: &Step) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);

    if let Some(table) = step.table.as_ref() {
        for row in table.rows.iter().skip(1) {
            // Skip header row
            if row.len() >= 2 {
                let role = &row[0];
                let content = &row[1];
                let msg = match role.as_str() {
                    "user" => Message::user(content),
                    "assistant" => Message::assistant(content),
                    "tool" => Message::tool_result("", content),
                    _ => Message::user(content),
                };
                ctx.messages.push(msg);
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Given Steps - Session State
// ═══════════════════════════════════════════════════════════════════════════

#[given(expr = "session iteration count is {int}")]
async fn given_session_iteration_count(w: &mut AlephWorld, count: i32) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    ctx.session.iteration_count = count as u32;
}

#[given(expr = "session with model {string} and total tokens {int}")]
async fn given_session_with_model_and_tokens(w: &mut AlephWorld, model: String, tokens: i64) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    ctx.session = ExecutionSession::new().with_model(&model);
    ctx.session.total_tokens = tokens as u64;
}

// ═══════════════════════════════════════════════════════════════════════════
// Given Steps - Message Factory Tests
// ═══════════════════════════════════════════════════════════════════════════

#[given(expr = "I create a user message with content {string}")]
async fn given_create_user_message(w: &mut AlephWorld, content: String) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    ctx.messages = vec![Message::user(&content)];
}

#[given(expr = "I create an assistant message with content {string}")]
async fn given_create_assistant_message(w: &mut AlephWorld, content: String) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    ctx.messages = vec![Message::assistant(&content)];
}

#[given(expr = "I create a tool result message with id {string} and content {string}")]
async fn given_create_tool_result_message(w: &mut AlephWorld, id: String, content: String) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    ctx.messages = vec![Message::tool_result(&id, &content)];
}

#[given(expr = "I create an assistant message with tool call id {string} name {string} arguments {string}")]
async fn given_create_assistant_with_tool_call(
    w: &mut AlephWorld,
    id: String,
    name: String,
    arguments: String,
) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    let tc = ToolCall::new(&id, &name, &arguments);
    ctx.messages = vec![Message::assistant_with_tool_call(tc)];
}

#[given(expr = "a tool call with id {string} name {string} arguments {string}")]
async fn given_tool_call(w: &mut AlephWorld, id: String, name: String, arguments: String) {
    let ctx = w.message_builder.get_or_insert_with(MessageBuilderContext::new);
    let tc = ToolCall::new(&id, &name, &arguments);
    ctx.serialize_tool_call(&tc);
}

// ═══════════════════════════════════════════════════════════════════════════
// When Steps
// ═══════════════════════════════════════════════════════════════════════════

#[when("I convert parts to messages")]
async fn when_convert_parts_to_messages(w: &mut AlephWorld) {
    let ctx = w.message_builder.as_mut().expect("MessageBuilder context not initialized");
    ctx.parts_to_messages();
}

#[when("I build messages from session")]
async fn when_build_messages_from_session(w: &mut AlephWorld) {
    let ctx = w.message_builder.as_mut().expect("MessageBuilder context not initialized");
    ctx.build_messages();
}

#[when("I inject reminders")]
async fn when_inject_reminders(w: &mut AlephWorld) {
    let ctx = w.message_builder.as_mut().expect("MessageBuilder context not initialized");
    ctx.inject_reminders();
}

#[when("I build from session")]
async fn when_build_from_session(w: &mut AlephWorld) {
    let ctx = w.message_builder.as_mut().expect("MessageBuilder context not initialized");
    ctx.build_from_session();
}

#[when("I serialize the tool call to JSON")]
async fn when_serialize_tool_call(w: &mut AlephWorld) {
    // Already serialized in given step, nothing to do
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    assert!(ctx.serialized_json.is_some(), "Tool call should be serialized");
}

#[when("I deserialize the JSON to a tool call")]
async fn when_deserialize_tool_call(w: &mut AlephWorld) {
    let ctx = w.message_builder.as_mut().expect("MessageBuilder context not initialized");
    ctx.deserialize_tool_call();
}

#[when("I serialize the message to JSON")]
async fn when_serialize_message(w: &mut AlephWorld) {
    let ctx = w.message_builder.as_mut().expect("MessageBuilder context not initialized");
    let msg = ctx.messages.first().expect("No message to serialize").clone();
    ctx.serialize_message(&msg);
}

#[when("I deserialize the JSON to a message")]
async fn when_deserialize_message(w: &mut AlephWorld) {
    let ctx = w.message_builder.as_mut().expect("MessageBuilder context not initialized");
    ctx.deserialize_message();
}

// ═══════════════════════════════════════════════════════════════════════════
// Then Steps - Message Count
// ═══════════════════════════════════════════════════════════════════════════

#[then(expr = "there should be {int} message(s)")]
async fn then_message_count(w: &mut AlephWorld, count: i32) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    assert_eq!(
        ctx.message_count(),
        count as usize,
        "Expected {} messages, got {}",
        count,
        ctx.message_count()
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Then Steps - Message Properties by Index
// ═══════════════════════════════════════════════════════════════════════════

#[then(expr = "message {int} should have role {string}")]
async fn then_message_has_role(w: &mut AlephWorld, index: i32, role: String) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    let msg = ctx.get_message(index as usize).unwrap_or_else(|| panic!("No message at index {}", index));
    assert_eq!(msg.role, role, "Message {} role mismatch", index);
}

#[then(expr = "message {int} should have content {string}")]
async fn then_message_has_content(w: &mut AlephWorld, index: i32, content: String) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    let msg = ctx.get_message(index as usize).unwrap_or_else(|| panic!("No message at index {}", index));
    assert_eq!(msg.content, content, "Message {} content mismatch", index);
}

#[then(expr = "message {int} should contain {string}")]
async fn then_message_contains(w: &mut AlephWorld, index: i32, text: String) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    let msg = ctx.get_message(index as usize).unwrap_or_else(|| panic!("No message at index {}", index));
    assert!(
        msg.content.contains(&text),
        "Message {} should contain '{}', but was: {}",
        index,
        text,
        msg.content
    );
}

#[then(expr = "message {int} should not contain {string}")]
async fn then_message_not_contains(w: &mut AlephWorld, index: i32, text: String) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    let msg = ctx.get_message(index as usize).unwrap_or_else(|| panic!("No message at index {}", index));
    assert!(
        !msg.content.contains(&text),
        "Message {} should NOT contain '{}', but was: {}",
        index,
        text,
        msg.content
    );
}

#[then(expr = "message {int} should not have tool_call_id")]
async fn then_message_no_tool_call_id(w: &mut AlephWorld, index: i32) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    let msg = ctx.get_message(index as usize).unwrap_or_else(|| panic!("No message at index {}", index));
    assert!(msg.tool_call_id.is_none(), "Message {} should not have tool_call_id", index);
}

#[then(expr = "message {int} should not have tool_calls")]
async fn then_message_no_tool_calls(w: &mut AlephWorld, index: i32) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    let msg = ctx.get_message(index as usize).unwrap_or_else(|| panic!("No message at index {}", index));
    assert!(msg.tool_calls.is_none(), "Message {} should not have tool_calls", index);
}

#[then(expr = "message {int} should have tool_call_id {string}")]
async fn then_message_has_tool_call_id(w: &mut AlephWorld, index: i32, id: String) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    let msg = ctx.get_message(index as usize).unwrap_or_else(|| panic!("No message at index {}", index));
    assert_eq!(
        msg.tool_call_id,
        Some(id.clone()),
        "Message {} tool_call_id mismatch",
        index
    );
}

#[then(expr = "message {int} should have a tool call with id {string} and name {string}")]
async fn then_message_has_tool_call(w: &mut AlephWorld, index: i32, id: String, name: String) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    let msg = ctx.get_message(index as usize).unwrap_or_else(|| panic!("No message at index {}", index));
    let tool_calls = msg.tool_calls.as_ref().expect("Message should have tool_calls");
    assert!(!tool_calls.is_empty(), "Tool calls should not be empty");
    assert_eq!(tool_calls[0].id, id, "Tool call id mismatch");
    assert_eq!(tool_calls[0].name, name, "Tool call name mismatch");
}

// ═══════════════════════════════════════════════════════════════════════════
// Then Steps - Message Search
// ═══════════════════════════════════════════════════════════════════════════

#[then(expr = "some message should contain {string}")]
async fn then_some_message_contains(w: &mut AlephWorld, text: String) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    assert!(
        ctx.any_message_contains(&text),
        "Expected some message to contain '{}', but none did",
        text
    );
}

#[then(expr = "no message should contain {string}")]
async fn then_no_message_contains(w: &mut AlephWorld, text: String) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    assert!(
        ctx.no_message_contains(&text),
        "Expected no message to contain '{}', but one did",
        text
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Then Steps - Created Message (for factory tests)
// ═══════════════════════════════════════════════════════════════════════════

#[then(expr = "the created message should have role {string}")]
async fn then_created_message_has_role(w: &mut AlephWorld, role: String) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    let msg = ctx.messages.first().expect("No message created");
    assert_eq!(msg.role, role, "Created message role mismatch");
}

#[then(expr = "the created message should have content {string}")]
async fn then_created_message_has_content(w: &mut AlephWorld, content: String) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    let msg = ctx.messages.first().expect("No message created");
    assert_eq!(msg.content, content, "Created message content mismatch");
}

#[then("the created message should have empty content")]
async fn then_created_message_has_empty_content(w: &mut AlephWorld) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    let msg = ctx.messages.first().expect("No message created");
    assert!(msg.content.is_empty(), "Created message should have empty content");
}

#[then("the created message should not have tool_call_id")]
async fn then_created_message_no_tool_call_id(w: &mut AlephWorld) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    let msg = ctx.messages.first().expect("No message created");
    assert!(msg.tool_call_id.is_none(), "Created message should not have tool_call_id");
}

#[then("the created message should not have tool_calls")]
async fn then_created_message_no_tool_calls(w: &mut AlephWorld) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    let msg = ctx.messages.first().expect("No message created");
    assert!(msg.tool_calls.is_none(), "Created message should not have tool_calls");
}

#[then(expr = "the created message should have tool_call_id {string}")]
async fn then_created_message_has_tool_call_id(w: &mut AlephWorld, id: String) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    let msg = ctx.messages.first().expect("No message created");
    assert_eq!(
        msg.tool_call_id,
        Some(id),
        "Created message tool_call_id mismatch"
    );
}

#[then(expr = "the created message should have a tool call with name {string}")]
async fn then_created_message_has_tool_call_with_name(w: &mut AlephWorld, name: String) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    let msg = ctx.messages.first().expect("No message created");
    let tool_calls = msg.tool_calls.as_ref().expect("Message should have tool_calls");
    assert!(!tool_calls.is_empty(), "Tool calls should not be empty");
    assert_eq!(tool_calls[0].name, name, "Tool call name mismatch");
}

// ═══════════════════════════════════════════════════════════════════════════
// Then Steps - Builder State
// ═══════════════════════════════════════════════════════════════════════════

#[then("the builder should have a compactor")]
async fn then_builder_has_compactor(w: &mut AlephWorld) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    assert!(ctx.compactor.is_some(), "Builder should have a compactor");
}

#[then("the builder should not have a compactor")]
async fn then_builder_no_compactor(w: &mut AlephWorld) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    assert!(ctx.compactor.is_none(), "Builder should NOT have a compactor");
}

#[then("the builder should have an overflow detector")]
async fn then_builder_has_overflow_detector(w: &mut AlephWorld) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    assert!(
        ctx.overflow_detector.is_some(),
        "Builder should have an overflow detector"
    );
}

#[then("the builder should not have an overflow detector")]
async fn then_builder_no_overflow_detector(w: &mut AlephWorld) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    assert!(
        ctx.overflow_detector.is_none(),
        "Builder should NOT have an overflow detector"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Then Steps - Serialization
// ═══════════════════════════════════════════════════════════════════════════

#[then(expr = "the JSON should contain {string}")]
async fn then_json_contains(w: &mut AlephWorld, text: String) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    let json = ctx.serialized_json.as_ref().expect("No JSON serialized");
    assert!(
        json.contains(&text),
        "JSON should contain '{}', but was: {}",
        text,
        json
    );
}

#[then(expr = "the JSON should not contain {string}")]
async fn then_json_not_contains(w: &mut AlephWorld, text: String) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    let json = ctx.serialized_json.as_ref().expect("No JSON serialized");
    assert!(
        !json.contains(&text),
        "JSON should NOT contain '{}', but was: {}",
        text,
        json
    );
}

#[then(expr = "the deserialized tool call should have id {string}")]
async fn then_deserialized_tool_call_has_id(w: &mut AlephWorld, id: String) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    let tc = ctx.deserialized_tool_call.as_ref().expect("No tool call deserialized");
    assert_eq!(tc.id, id, "Deserialized tool call id mismatch");
}

#[then(expr = "the deserialized tool call should have name {string}")]
async fn then_deserialized_tool_call_has_name(w: &mut AlephWorld, name: String) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    let tc = ctx.deserialized_tool_call.as_ref().expect("No tool call deserialized");
    assert_eq!(tc.name, name, "Deserialized tool call name mismatch");
}

#[then(expr = "the deserialized message should have role {string}")]
async fn then_deserialized_message_has_role(w: &mut AlephWorld, role: String) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    let msg = ctx.deserialized_message.as_ref().expect("No message deserialized");
    assert_eq!(msg.role, role, "Deserialized message role mismatch");
}

#[then(expr = "the deserialized message should have content {string}")]
async fn then_deserialized_message_has_content(w: &mut AlephWorld, content: String) {
    let ctx = w.message_builder.as_ref().expect("MessageBuilder context not initialized");
    let msg = ctx.deserialized_message.as_ref().expect("No message deserialized");
    assert_eq!(msg.content, content, "Deserialized message content mismatch");
}
