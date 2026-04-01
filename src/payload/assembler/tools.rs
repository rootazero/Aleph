/// Tool formatting for system prompts
///
/// This module handles formatting of the Available Tools section
/// that lists all tool types (Builtin, Native, MCP, Skills, Custom).

/// Format the Available Tools section.
///
/// This renders the unified tool list with priority information.
pub fn format_available_tools(tools_block: &str) -> String {
    let mut lines = vec![
        "## Available Tools".to_string(),
        String::new(),
        "The following tools are available for you to use. When invoking a tool, use the Tool Execution capability:".to_string(),
        "```json".to_string(),
        r#"{"__capability_request__": true, "capability": "mcp", "parameters": {"tool": "tool_name", "args": {...}}, "query": "original user request"}"#.to_string(),
        "```".to_string(),
        String::new(),
        "### Tool Priority (Higher = Preferred)".to_string(),
        "- **[Builtin - Preferred]**: System commands, highest reliability".to_string(),
        "- **[Native - Preferred]**: Built-in tools, optimized for local execution".to_string(),
        "- **[Custom]**: User-defined commands".to_string(),
        "- **[MCP:xxx]**: External MCP server tools".to_string(),
        "- **[Skill:xxx]**: Agent skills".to_string(),
        String::new(),
        "### Tool List".to_string(),
        String::new(),
    ];

    lines.push(tools_block.to_string());

    lines.push(String::new());
    lines.push("**Examples**:".to_string());
    lines.push("- User: \"分析这个网页 https://example.com\"".to_string());
    lines.push("  ```json".to_string());
    lines.push(r#"  {"__capability_request__": true, "capability": "mcp", "parameters": {"tool": "web_fetch", "args": {"url": "https://example.com", "prompt": "分析网页内容"}}, "query": "分析这个网页 https://example.com"}"#.to_string());
    lines.push("  ```".to_string());
    lines.push("- User: \"截屏\"".to_string());
    lines.push("  ```json".to_string());
    lines.push(r#"  {"__capability_request__": true, "capability": "mcp", "parameters": {"tool": "screen_capture", "args": {}}, "query": "截屏"}"#.to_string());
    lines.push("  ```".to_string());
    lines.push(String::new());
    lines.push("**CRITICAL**: When a tool can help answer the user's request, USE IT. Do not claim you cannot access web pages, files, or perform system operations when the appropriate tool is available.".to_string());

    lines.join("\n")
}
