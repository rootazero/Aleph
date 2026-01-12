//! Integration test for L3 Task Planning with real LLM
//!
//! Run with: cargo test --test test_l3_planning_real -- --ignored --nocapture

use aethecore::ProviderConfig;
use aethecore::dispatcher::{ToolSource, UnifiedTool};
use aethecore::providers::openai::OpenAiProvider;
use aethecore::AiProvider;
use aethecore::routing::{L3TaskPlanner, PlanningResult, QuickHeuristics, ToolSafetyLevel};
use std::sync::Arc;
use std::time::Instant;

/// Create a test provider with the provided credentials
fn create_test_provider() -> Arc<dyn AiProvider> {
    let config = ProviderConfig {
        provider_type: Some("openai".to_string()),
        api_key: Some("sk-GvmRS8IKdQsZ1eun7fF1C31839Ea43E88829B26e371152F5".to_string()),
        model: "gpt-5.2".to_string(),
        base_url: Some("https://ai.t8star.cn/v1".to_string()),
        color: "#10a37f".to_string(),
        timeout_seconds: 30,
        enabled: true,
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        frequency_penalty: None,
        presence_penalty: None,
        stop_sequences: None,
        thinking_level: None,
        media_resolution: None,
        repeat_penalty: None,
        system_prompt_mode: None,
    };

    Arc::new(
        OpenAiProvider::new("test-provider".to_string(), config)
            .expect("Failed to create provider"),
    )
}

/// Create mock tools for testing
fn create_test_tools() -> Vec<UnifiedTool> {
    vec![
        UnifiedTool::new(
            "native:search",
            "search",
            "Search the web for information",
            ToolSource::Native,
        )
        .with_display_name("Search")
        .with_parameters_schema(serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search query"}
            },
            "required": ["query"]
        }))
        .with_safety_level(ToolSafetyLevel::ReadOnly),

        UnifiedTool::new(
            "native:translate",
            "translate",
            "Translate text between languages",
            ToolSource::Native,
        )
        .with_display_name("Translate")
        .with_parameters_schema(serde_json::json!({
            "type": "object",
            "properties": {
                "text": {"type": "string", "description": "Text to translate"},
                "target_language": {"type": "string", "description": "Target language code"}
            },
            "required": ["text", "target_language"]
        }))
        .with_safety_level(ToolSafetyLevel::ReadOnly),

        UnifiedTool::new(
            "native:summarize",
            "summarize",
            "Summarize text content",
            ToolSource::Native,
        )
        .with_display_name("Summarize")
        .with_parameters_schema(serde_json::json!({
            "type": "object",
            "properties": {
                "text": {"type": "string", "description": "Text to summarize"},
                "max_length": {"type": "integer", "description": "Maximum summary length"}
            },
            "required": ["text"]
        }))
        .with_safety_level(ToolSafetyLevel::ReadOnly),
    ]
}

#[tokio::test]
#[ignore] // Run with: cargo test --test test_l3_planning_real -- --ignored --nocapture
async fn test_multi_step_planning_chinese() {
    println!("\n=== Testing Multi-Step Planning (Chinese) ===\n");

    let provider = create_test_provider();
    let planner = L3TaskPlanner::new(provider);
    let tools = create_test_tools();

    let input = "搜索最新的AI新闻，然后总结主要内容";
    println!("Input: {}", input);

    let start = Instant::now();
    let result = planner
        .analyze_and_plan(input, &tools, None)
        .await;
    let elapsed = start.elapsed();

    println!("Time: {:?}", elapsed);

    match result {
        Ok(PlanningResult::Plan(plan)) => {
            println!("\n✅ Generated Plan:");
            println!("  Description: {}", plan.description);
            println!("  Steps: {}", plan.steps.len());
            for (i, step) in plan.steps.iter().enumerate() {
                println!("    {}. {} - {}", i + 1, step.tool_name, step.description);
                println!("       Params: {:?}", step.parameters);
            }
        }
        Ok(PlanningResult::SingleTool { tool_name, confidence, .. }) => {
            println!("\n⚠️ Single Tool Detected:");
            println!("  Tool: {}", tool_name);
            println!("  Confidence: {:.2}", confidence);
        }
        Ok(PlanningResult::GeneralChat { reason }) => {
            println!("\n⚠️ General Chat: {}", reason);
        }
        Err(e) => {
            println!("\n❌ Error: {:?}", e);
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_multi_step_planning_english() {
    println!("\n=== Testing Multi-Step Planning (English) ===\n");

    let provider = create_test_provider();
    let planner = L3TaskPlanner::new(provider);
    let tools = create_test_tools();

    let input = "Search for recent AI breakthroughs and then translate the summary to Chinese";
    println!("Input: {}", input);

    let start = Instant::now();
    let result = planner
        .analyze_and_plan(input, &tools, None)
        .await;
    let elapsed = start.elapsed();

    println!("Time: {:?}", elapsed);

    match result {
        Ok(PlanningResult::Plan(plan)) => {
            println!("\n✅ Generated Plan:");
            println!("  Description: {}", plan.description);
            println!("  Steps: {}", plan.steps.len());
            for (i, step) in plan.steps.iter().enumerate() {
                println!("    {}. {} - {}", i + 1, step.tool_name, step.description);
                println!("       Params: {:?}", step.parameters);
            }
        }
        Ok(PlanningResult::SingleTool { tool_name, confidence, .. }) => {
            println!("\n⚠️ Single Tool Detected:");
            println!("  Tool: {}", tool_name);
            println!("  Confidence: {:.2}", confidence);
        }
        Ok(PlanningResult::GeneralChat { reason }) => {
            println!("\n⚠️ General Chat: {}", reason);
        }
        Err(e) => {
            println!("\n❌ Error: {:?}", e);
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_single_step_detection() {
    println!("\n=== Testing Single Step Detection ===\n");

    let provider = create_test_provider();
    let planner = L3TaskPlanner::new(provider);
    let tools = create_test_tools();

    let input = "搜索今天的天气";
    println!("Input: {}", input);

    let start = Instant::now();
    let result = planner
        .analyze_and_plan(input, &tools, None)
        .await;
    let elapsed = start.elapsed();

    println!("Time: {:?}", elapsed);

    match result {
        Ok(PlanningResult::Plan(plan)) => {
            println!("\n⚠️ Unexpected Plan (should be single tool):");
            println!("  Steps: {}", plan.steps.len());
        }
        Ok(PlanningResult::SingleTool { tool_name, confidence, .. }) => {
            println!("\n✅ Single Tool Detected (as expected):");
            println!("  Tool: {}", tool_name);
            println!("  Confidence: {:.2}", confidence);
        }
        Ok(PlanningResult::GeneralChat { reason }) => {
            println!("\n⚠️ General Chat: {}", reason);
        }
        Err(e) => {
            println!("\n❌ Error: {:?}", e);
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_heuristics_performance() {
    println!("\n=== Testing Heuristics Performance ===\n");

    let test_inputs = vec![
        "搜索最新AI新闻，然后翻译成英文",
        "先搜索天气，再总结",
        "Search for news and summarize it",
        "Find information then translate",
        "帮我查询一下",
        "What's the weather?",
    ];

    for input in test_inputs {
        let start = Instant::now();
        let result = QuickHeuristics::analyze(input);
        let elapsed = start.elapsed();

        println!("Input: \"{}\"", input);
        println!("  Is Multi-Step: {}", result.is_likely_multi_step);
        println!("  Action Count: {}", result.action_count);
        println!("  Has Connector: {}", result.has_connector);
        println!("  Time: {:?}", elapsed);

        // Verify heuristics runs under 10ms
        assert!(
            elapsed.as_millis() < 10,
            "Heuristics should complete in <10ms"
        );
        println!();
    }
}
