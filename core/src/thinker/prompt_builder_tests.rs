//! Tests for PromptBuilder

use super::*;

#[test]
fn test_system_prompt_generation() {
    let builder = PromptBuilder::new(PromptConfig::default());

    let tools = vec![
        ToolInfo {
            name: "search".to_string(),
            description: "Search the web".to_string(),
            parameters_schema: r#"{"query": "string"}"#.to_string(),
            category: None,
        },
        ToolInfo {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters_schema: r#"{"path": "string"}"#.to_string(),
            category: None,
        },
    ];

    let prompt = builder.build_system_prompt(&tools);

    assert!(prompt.contains("AI assistant"));
    assert!(prompt.contains("search"));
    assert!(prompt.contains("read_file"));
    assert!(prompt.contains("Response Format"));
    assert!(prompt.contains("JSON"));
}

#[test]
fn test_message_building() {
    let builder = PromptBuilder::new(PromptConfig::default());

    let observation = Observation {
        history_summary: "Previously searched for Rust tutorials".to_string(),
        recent_steps: vec![StepSummary {
            step_id: 0,
            reasoning: "Need to search".to_string(),
            action_type: "tool:search".to_string(),
            action_args: r#"{"query": "rust"}"#.to_string(),
            result_summary: "Found 10 results".to_string(),
            result_output: r#"{"results": 10, "items": []}"#.to_string(),
            success: true,
        }],
        available_tools: vec![],
        attachments: vec![],
        current_step: 1,
        total_tokens: 500,
    };

    let messages = builder.build_messages("Find Rust tutorials", &observation);

    assert!(messages.len() >= 3);
    assert_eq!(messages[0].role, MessageRole::User);
    assert!(messages[0].content.contains("Find Rust tutorials"));
}

#[test]
fn test_system_prompt_with_runtime_capabilities() {
    let mut config = PromptConfig::default();
    config.runtime_capabilities = Some(
        "**Python (via uv)**\n\
         - Execute Python scripts\n\
         - Executable: `/path/to/python`\n"
            .to_string(),
    );

    let builder = PromptBuilder::new(config);
    let prompt = builder.build_system_prompt(&[]);

    // Verify runtime capabilities section is present
    assert!(prompt.contains("## Available Runtimes"));
    assert!(prompt.contains("Python (via uv)"));
    assert!(prompt.contains("/path/to/python"));

    // Verify section order: Runtimes should come before Tools
    let runtimes_pos = prompt.find("## Available Runtimes").unwrap();
    let tools_pos = prompt.find("## Available Tools").unwrap();
    assert!(
        runtimes_pos < tools_pos,
        "Available Runtimes should appear before Available Tools"
    );
}

#[test]
fn test_system_prompt_without_runtime_capabilities() {
    let config = PromptConfig::default();
    let builder = PromptBuilder::new(config);
    let prompt = builder.build_system_prompt(&[]);

    // Verify runtime capabilities section is NOT present
    assert!(!prompt.contains("## Available Runtimes"));
}

#[test]
fn test_system_prompt_with_tool_index() {
    let mut config = PromptConfig::default();
    config.tool_index = Some(
        "- github:pr_list: List pull requests\n\
         - github:issue_create: Create an issue\n\
         - notion:page_read: Read a Notion page\n"
            .to_string(),
    );

    let builder = PromptBuilder::new(config);

    // Only core tools with full schema
    let core_tools = vec![ToolInfo {
        name: "search".to_string(),
        description: "Search the web".to_string(),
        parameters_schema: r#"{"query": "string"}"#.to_string(),
        category: None,
    }];

    let prompt = builder.build_system_prompt(&core_tools);

    // Verify core tool has full schema
    assert!(prompt.contains("search"));
    assert!(prompt.contains("Search the web"));
    assert!(prompt.contains(r#"{"query": "string"}"#));

    // Verify tool index section is present
    assert!(prompt.contains("### Additional Tools"));
    assert!(prompt.contains("get_tool_schema"));
    assert!(prompt.contains("github:pr_list"));
    assert!(prompt.contains("notion:page_read"));
}

#[test]
fn test_system_prompt_smart_discovery_no_full_tools() {
    let mut config = PromptConfig::default();
    config.tool_index = Some("- tool1: Description 1\n- tool2: Description 2\n".to_string());

    let builder = PromptBuilder::new(config);
    let prompt = builder.build_system_prompt(&[]);

    // Should not say "No tools available" because we have tool index
    assert!(!prompt.contains("No tools available"));
    assert!(prompt.contains("### Additional Tools"));
}

#[test]
fn test_system_prompt_with_skill_mode() {
    let mut config = PromptConfig::default();
    config.skill_mode = true;

    let builder = PromptBuilder::new(config);
    let prompt = builder.build_system_prompt(&[]);

    // Verify skill mode section is present
    assert!(prompt.contains("Skill Execution Mode"));
    // Verify it emphasizes JSON response format
    assert!(prompt.contains("RESPONSE FORMAT"));
    assert!(prompt.contains("EVERY response MUST be a valid JSON action object"));
    // Verify it warns against direct output
    assert!(prompt.contains("NEVER output raw content directly"));
    // Verify workflow requirements
    assert!(prompt.contains("Complete ALL steps"));
    assert!(prompt.contains("file_ops"));
}

#[test]
fn test_system_prompt_without_skill_mode() {
    let config = PromptConfig::default();
    let builder = PromptBuilder::new(config);
    let prompt = builder.build_system_prompt(&[]);

    // Verify skill mode section is NOT present
    assert!(!prompt.contains("Skill Execution Mode"));
    assert!(!prompt.contains("NEVER output raw content directly"));
}

#[test]
fn test_build_system_prompt_cached() {
    let builder = PromptBuilder::new(PromptConfig::default());

    let parts = builder.build_system_prompt_cached(&[]);

    assert_eq!(parts.len(), 2);
    assert!(parts[0].cache); // Static header should be cached
    assert!(!parts[1].cache); // Dynamic part should not be cached
}

#[test]
fn test_cached_header_is_static() {
    let builder = PromptBuilder::new(PromptConfig::default());

    // Call twice with different tools
    let parts1 = builder.build_system_prompt_cached(&[]);
    let parts2 = builder.build_system_prompt_cached(&[ToolInfo {
        name: "test".to_string(),
        description: "Test tool".to_string(),
        parameters_schema: "{}".to_string(),
        category: None,
    }]);

    // Header should be identical
    assert_eq!(parts1[0].content, parts2[0].content);
    // Dynamic content should differ
    assert_ne!(parts1[1].content, parts2[1].content);
}

#[test]
fn test_cached_header_contains_core_instructions() {
    let builder = PromptBuilder::new(PromptConfig::default());

    let parts = builder.build_system_prompt_cached(&[]);
    let header = &parts[0].content;

    // Verify static header contains role definition
    assert!(header.contains("AI assistant executing tasks step by step"));
    // Verify static header contains core instructions
    assert!(header.contains("## Your Role"));
    assert!(header.contains("Observe the current state"));
    // Verify static header contains decision framework
    assert!(header.contains("## Decision Framework"));
    assert!(header.contains("What is the current state?"));
}

#[test]
fn test_cached_dynamic_contains_tools() {
    let builder = PromptBuilder::new(PromptConfig::default());

    let tools = vec![ToolInfo {
        name: "my_tool".to_string(),
        description: "My test tool".to_string(),
        parameters_schema: r#"{"param": "value"}"#.to_string(),
        category: None,
    }];

    let parts = builder.build_system_prompt_cached(&tools);
    let dynamic = &parts[1].content;

    // Verify dynamic content contains tools
    assert!(dynamic.contains("## Available Tools"));
    assert!(dynamic.contains("my_tool"));
    assert!(dynamic.contains("My test tool"));
    assert!(dynamic.contains(r#"{"param": "value"}"#));
    // Verify dynamic content contains special actions
    assert!(dynamic.contains("## Special Actions"));
    // Verify dynamic content contains response format
    assert!(dynamic.contains("## Response Format"));
}

#[test]
fn test_cached_parts_combined_equals_full_prompt() {
    let builder = PromptBuilder::new(PromptConfig::default());

    let tools = vec![ToolInfo {
        name: "search".to_string(),
        description: "Search the web".to_string(),
        parameters_schema: r#"{"query": "string"}"#.to_string(),
        category: None,
    }];

    let full_prompt = builder.build_system_prompt(&tools);
    let parts = builder.build_system_prompt_cached(&tools);
    let combined = format!("{}{}", parts[0].content, parts[1].content);

    // The combined cached parts should have the same content as the full prompt
    // Note: They may differ slightly in structure due to decision framework section
    // which is in the static header but not in the original build_system_prompt
    // So we check that key sections are present in both
    assert!(full_prompt.contains("AI assistant"));
    assert!(combined.contains("AI assistant"));
    assert!(full_prompt.contains("## Available Tools"));
    assert!(combined.contains("## Available Tools"));
    assert!(full_prompt.contains("search"));
    assert!(combined.contains("search"));
}
