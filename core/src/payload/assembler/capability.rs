/// Capability formatting for system prompts
///
/// This module handles formatting of capability instructions that guide
/// the AI on how to use various capabilities (search, video, MCP, etc.)
use crate::capability::CapabilityDeclaration;

/// Format capability instructions for the AI.
pub fn format_capability_instructions(capabilities: &[&CapabilityDeclaration]) -> String {
    let mut lines = vec![
        "## CRITICAL: Proactive Search Decision System".to_string(),
        String::new(),
        "**YOU MUST PROACTIVELY DECIDE WHETHER TO SEARCH FOR EVERY QUESTION.**".to_string(),
        String::new(),
        "You have the ability to search the internet in real-time. Before answering ANY question, you MUST first evaluate: \"Does answering this question accurately require up-to-date information from the internet?\"".to_string(),
        String::new(),
        "### MANDATORY: Self-Assessment Before Every Response".to_string(),
        String::new(),
        "Ask yourself these questions:".to_string(),
        "1. Is this about current events, recent news, or things that change over time?".to_string(),
        "2. Would my training data (which has a knowledge cutoff) be outdated for this?".to_string(),
        "3. Is the user asking about specific facts that I should verify rather than guess?".to_string(),
        "4. Does the user explicitly or implicitly want the latest/current information?".to_string(),
        String::new(),
        "**If ANY of the above is YES → USE SEARCH IMMEDIATELY.**".to_string(),
        String::new(),
        "### When to Search (MUST search for these):".to_string(),
        String::new(),
        "- **Time-sensitive**: weather, stock prices, exchange rates, sports scores, election results".to_string(),
        "- **Current events**: news, recent developments, \"what happened\", \"latest updates\"".to_string(),
        "- **Specific entities**: company news, person updates, product releases, policy changes".to_string(),
        "- **Factual verification**: statistics, data, facts that may have changed since training".to_string(),
        "- **User intent keywords**: 搜索, 查一下, 找找, search, look up, find out, what's new".to_string(),
        "- **Recency indicators**: 今天, 最近, 现在, 最新, today, now, latest, recent, current".to_string(),
        String::new(),
        "### How to Request Search".to_string(),
        String::new(),
        "When search is needed, respond with ONLY this JSON (no other text):".to_string(),
        "```json".to_string(),
        r#"{"__capability_request__": true, "capability": "search", "parameters": {"query": "optimized search terms"}, "query": "original user question"}"#.to_string(),
        "```".to_string(),
        String::new(),
        "**CRITICAL RULES:**".to_string(),
        "- DO NOT guess or make up information when search would help".to_string(),
        "- DO NOT say \"I don't have access to real-time data\" - you DO have search capability".to_string(),
        "- DO NOT ask user for permission to search - just search proactively".to_string(),
        "- DO NOT respond with natural language if search is needed - return JSON immediately".to_string(),
        "- ONLY respond directly for: translations, code help, creative writing, timeless knowledge".to_string(),
        String::new(),
        "### Available Capabilities:".to_string(),
        String::new(),
    ];

    for cap in capabilities {
        lines.push(format!("#### {} (`{}`)", cap.name, cap.id));
        lines.push(format!("- **Description**: {}", cap.description));

        if !cap.parameters.is_empty() {
            lines.push("- **Parameters**:".to_string());
            for param in &cap.parameters {
                let required_str = if param.required {
                    "required"
                } else {
                    "optional"
                };
                lines.push(format!(
                    "  - `{}` ({}): {} [{}]",
                    param.name, param.param_type, param.description, required_str
                ));
            }
        }

        if !cap.examples.is_empty() {
            lines.push("- **Use when user asks**:".to_string());
            for example in &cap.examples {
                lines.push(format!("  - \"{}\"", example));
            }
        }

        // Note: Tool list is now rendered separately via format_available_tools()
        // The Tool capability just describes HOW to call tools
        // The actual tool list with 5 categories is in the "Available Tools" section

        lines.push(String::new());
    }

    lines.push("### Decision Framework (MUST FOLLOW):".to_string());
    lines.push(String::new());
    lines.push("**Step 1: Evaluate the question type**".to_string());
    lines.push("- Does it involve time-sensitive information? → SEARCH".to_string());
    lines.push("- Does it ask about specific real-world entities/events? → SEARCH".to_string());
    lines.push("- Would outdated information harm the user? → SEARCH".to_string());
    lines.push(
        "- Is it purely about concepts, code, or creative tasks? → RESPOND DIRECTLY"
            .to_string(),
    );
    lines.push(String::new());
    lines.push("**Step 2: When in doubt, SEARCH**".to_string());
    lines.push(
        "- It's better to search and provide accurate info than to guess and be wrong"
            .to_string(),
    );
    lines.push("- Users expect you to use your search capability proactively".to_string());
    lines.push(String::new());
    lines.push("**Step 3: Multi-turn awareness**".to_string());
    lines.push("- If previous conversation involved a search-worthy topic and user provides follow-up details, combine context and SEARCH".to_string());
    lines.push(String::new());
    lines.push("**Examples requiring SEARCH:**".to_string());
    lines.push(String::new());
    lines.push("User: \"今天中国有什么大新闻\" → SEARCH (current events)".to_string());
    lines.push("```json".to_string());
    lines.push(r#"{"__capability_request__": true, "capability": "search", "parameters": {"query": "中国今日新闻 头条"}, "query": "今天中国有什么大新闻"}"#.to_string());
    lines.push("```".to_string());
    lines.push(String::new());
    lines.push("User: \"苹果公司最近怎么样\" → SEARCH (company news)".to_string());
    lines.push("```json".to_string());
    lines.push(r#"{"__capability_request__": true, "capability": "search", "parameters": {"query": "Apple company news 2024"}, "query": "苹果公司最近怎么样"}"#.to_string());
    lines.push("```".to_string());
    lines.push(String::new());
    lines.push(
        "User: \"帮我查一下北京到上海的高铁\" → SEARCH (user explicitly wants to look up)"
            .to_string(),
    );
    lines.push("```json".to_string());
    lines.push(r#"{"__capability_request__": true, "capability": "search", "parameters": {"query": "北京到上海高铁时刻表票价"}, "query": "帮我查一下北京到上海的高铁"}"#.to_string());
    lines.push("```".to_string());
    lines.push(String::new());
    lines.push(
        "User: \"What is the current Bitcoin price?\" → SEARCH (real-time price)".to_string(),
    );
    lines.push("```json".to_string());
    lines.push(r#"{"__capability_request__": true, "capability": "search", "parameters": {"query": "Bitcoin BTC price USD"}, "query": "What is the current Bitcoin price?"}"#.to_string());
    lines.push("```".to_string());
    lines.push(String::new());
    lines.push("**Examples NOT requiring search (respond directly):**".to_string());
    lines.push("- \"帮我翻译这段话\" → Translation task, no search needed".to_string());
    lines.push("- \"写一首关于春天的诗\" → Creative writing, no search needed".to_string());
    lines.push("- \"解释一下什么是递归\" → Timeless concept, no search needed".to_string());
    lines.push("- \"帮我改一下这段代码\" → Code task, no search needed".to_string());

    lines.join("\n")
}
