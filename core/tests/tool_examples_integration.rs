//! Integration tests for tool examples() method

use aethecore::builtin_tools::bash_exec::BashExecTool;
use aethecore::builtin_tools::search::SearchTool;
use aethecore::tools::AetherTool;

#[test]
fn test_bash_tool_has_examples() {
    let tool = BashExecTool::new();
    let def = tool.definition();

    assert_eq!(def.name, "bash");
    assert!(def.llm_context.is_some());

    let context = def.llm_context.unwrap();
    assert!(context.contains("## Usage Examples"));
    assert!(context.contains("bash(cmd='ls -la /tmp')"));
    assert!(context.contains("bash(cmd='pwd && ls -l', working_dir='/home/user')"));
}

#[test]
fn test_search_tool_has_examples() {
    let tool = SearchTool::new();
    let def = tool.definition();

    assert_eq!(def.name, "search");
    assert!(def.llm_context.is_some());

    let context = def.llm_context.unwrap();
    assert!(context.contains("## Usage Examples"));
    assert!(context.contains("search(query='latest Rust async trends', limit=5)"));
    assert!(context.contains("search(query='Claude AI capabilities 2025')"));
}
