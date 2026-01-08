/// PromptAssembler - Assembles final prompts from AgentPayload
///
/// This module formats context data (memory, search, MCP) into LLM prompts
/// using different formats (Markdown, XML, JSON).
use super::{AgentContext, AgentPayload, ContextFormat};
use crate::capability::CapabilityDeclaration;
use crate::memory::MemoryEntry;
use crate::search::SearchResult;

/// Prompt assembler that converts AgentPayload to LLM message format
///
/// Supports different context injection formats (Markdown, XML, JSON)
pub struct PromptAssembler {
    context_format: ContextFormat,
}

impl PromptAssembler {
    /// Create a new prompt assembler
    ///
    /// # Arguments
    ///
    /// * `format` - Context injection format to use
    pub fn new(format: ContextFormat) -> Self {
        Self {
            context_format: format,
        }
    }

    /// Build a capability-aware system prompt for AI-first intent detection.
    ///
    /// This method creates a system prompt that:
    /// 1. Includes the base prompt
    /// 2. Describes available capabilities to the AI
    /// 3. Instructs AI how to request capability invocation via JSON
    /// 4. Optionally includes existing context (memory)
    ///
    /// # Arguments
    ///
    /// * `base_prompt` - Base system prompt from routing rule or provider
    /// * `capabilities` - List of available capabilities
    /// * `context` - Optional existing context (memory snippets, etc.)
    ///
    /// # Returns
    ///
    /// Complete system prompt with capability instructions
    pub fn build_capability_aware_prompt(
        &self,
        base_prompt: &str,
        capabilities: &[CapabilityDeclaration],
        context: Option<&AgentContext>,
    ) -> String {
        let mut prompt = base_prompt.to_string();

        // Add capability instructions if any capabilities are available
        let available_caps: Vec<_> = capabilities.iter().filter(|c| c.available).collect();
        if !available_caps.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(&self.format_capability_instructions(&available_caps));
        }

        // Add existing context if provided
        if let Some(ctx) = context {
            if let Some(formatted_ctx) = self.format_context(ctx) {
                prompt.push_str("\n\n");
                prompt.push_str(&formatted_ctx);
            }
        }

        prompt
    }

    /// Format capability instructions for the AI.
    fn format_capability_instructions(&self, capabilities: &[&CapabilityDeclaration]) -> String {
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
                    let required_str = if param.required { "required" } else { "optional" };
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

            lines.push(String::new());
        }

        lines.push("### Decision Framework (MUST FOLLOW):".to_string());
        lines.push(String::new());
        lines.push("**Step 1: Evaluate the question type**".to_string());
        lines.push("- Does it involve time-sensitive information? → SEARCH".to_string());
        lines.push("- Does it ask about specific real-world entities/events? → SEARCH".to_string());
        lines.push("- Would outdated information harm the user? → SEARCH".to_string());
        lines.push("- Is it purely about concepts, code, or creative tasks? → RESPOND DIRECTLY".to_string());
        lines.push(String::new());
        lines.push("**Step 2: When in doubt, SEARCH**".to_string());
        lines.push("- It's better to search and provide accurate info than to guess and be wrong".to_string());
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
        lines.push("User: \"帮我查一下北京到上海的高铁\" → SEARCH (user explicitly wants to look up)".to_string());
        lines.push("```json".to_string());
        lines.push(r#"{"__capability_request__": true, "capability": "search", "parameters": {"query": "北京到上海高铁时刻表票价"}, "query": "帮我查一下北京到上海的高铁"}"#.to_string());
        lines.push("```".to_string());
        lines.push(String::new());
        lines.push("User: \"What is the current Bitcoin price?\" → SEARCH (real-time price)".to_string());
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

    /// Assemble complete system prompt
    ///
    /// Format: {base_prompt}\n\n{formatted_context}
    ///
    /// # Arguments
    ///
    /// * `base_prompt` - Base system prompt from routing rule or provider
    /// * `payload` - Agent payload containing context data
    ///
    /// # Returns
    ///
    /// Complete system prompt with context data appended
    pub fn assemble_system_prompt(&self, base_prompt: &str, payload: &AgentPayload) -> String {
        let mut prompt = base_prompt.to_string();

        // Append formatted context if available
        if let Some(formatted_ctx) = self.format_context(&payload.context) {
            prompt.push_str("\n\n");
            prompt.push_str(&formatted_ctx);
        }

        prompt
    }

    /// Format context data (memory, search, MCP) without base prompt
    ///
    /// Use this when you need only the context part, not the full system prompt.
    /// Useful for prepend mode where base prompt should be excluded.
    ///
    /// Selects formatting strategy based on context_format
    pub fn format_context(&self, context: &AgentContext) -> Option<String> {
        match self.context_format {
            ContextFormat::Markdown => self.format_markdown(context),
            ContextFormat::Xml => self.format_xml(context),
            ContextFormat::Json => self.format_json(context),
        }
    }

    /// Markdown formatting (MVP implementation)
    fn format_markdown(&self, context: &AgentContext) -> Option<String> {
        let mut sections = Vec::new();

        // Memory section
        if let Some(memories) = &context.memory_snippets {
            if !memories.is_empty() {
                let memory_section = self.format_memory_markdown(memories);
                sections.push(memory_section);
            }
        }

        // Search section
        if let Some(results) = &context.search_results {
            if !results.is_empty() {
                let search_section = self.format_search_results_markdown(results);
                sections.push(search_section);
            }
        }

        // Video transcript section
        if let Some(transcript) = &context.video_transcript {
            let video_section = transcript.format_for_context();
            sections.push(video_section);
        }

        // MCP section (reserved for future)
        if let Some(_resources) = &context.mcp_resources {
            // TODO: Implement MCP formatting
        }

        // Skills instructions section
        if let Some(instructions) = &context.skill_instructions {
            if !instructions.is_empty() {
                let skill_section = format!("## Skill Instructions\n\n{}", instructions);
                sections.push(skill_section);
            }
        }

        if sections.is_empty() {
            None
        } else {
            Some(format!(
                "### Context Information\n\n{}",
                sections.join("\n\n")
            ))
        }
    }

    /// Format memory entries as Markdown
    fn format_memory_markdown(&self, memories: &[MemoryEntry]) -> String {
        let mut lines = vec!["**Relevant History**:".to_string()];

        for (i, entry) in memories.iter().enumerate() {
            lines.push(format!(
                "\n{}. **Conversation at {}**",
                i + 1,
                format_timestamp(entry.context.timestamp)
            ));
            lines.push(format!("   App: {}", entry.context.app_bundle_id));
            lines.push(format!("   Window: {}", entry.context.window_title));
            lines.push(format!(
                "   User: {}",
                truncate_text(&entry.user_input, 200)
            ));
            lines.push(format!("   AI: {}", truncate_text(&entry.ai_output, 200)));

            // Show similarity score if available
            if let Some(score) = entry.similarity_score {
                lines.push(format!("   Relevance: {:.0}%", score * 100.0));
            }
        }

        lines.join("\n")
    }

    /// Format search results as Markdown
    ///
    /// Creates a numbered list of search results with:
    /// - Title as clickable Markdown link
    /// - Snippet/excerpt text
    /// - Optional published date
    /// - Optional relevance score
    ///
    /// Also includes instructions to help AI understand that these results
    /// were fetched by its own search capability, not provided by the user.
    fn format_search_results_markdown(&self, results: &[SearchResult]) -> String {
        let mut lines = vec![
            "**Web Search Results** (Retrieved by your search capability):".to_string(),
            String::new(),
            "_CRITICAL: These results were just fetched by YOUR search capability in real-time. You HAVE successfully accessed the internet. Do NOT say \"I cannot access the internet\" or ask the user for more search results. Answer directly based on this data._".to_string(),
            String::new(),
        ];

        for (i, result) in results.iter().enumerate() {
            // Main entry with title as link
            lines.push(format!(
                "\n{}. [{}]({})",
                i + 1,
                escape_markdown(&result.title),
                result.url
            ));

            // Snippet/excerpt (truncate if too long)
            if !result.snippet.is_empty() {
                let snippet = truncate_text(&result.snippet, 300);
                lines.push(format!("   {}", snippet));
            }

            // Optional metadata
            let mut metadata = Vec::new();

            // Published date
            if let Some(timestamp) = result.published_date {
                let date = format_timestamp(timestamp);
                metadata.push(format!("Published: {}", date));
            }

            // Relevance score
            if let Some(score) = result.relevance_score {
                metadata.push(format!("Relevance: {:.0}%", score * 100.0));
            }

            // Source type
            if let Some(ref source_type) = result.source_type {
                metadata.push(format!("Type: {}", source_type));
            }

            // Provider attribution
            if let Some(ref provider) = result.provider {
                metadata.push(format!("Source: {}", provider));
            }

            if !metadata.is_empty() {
                lines.push(format!("   _{}_", metadata.join(" | ")));
            }
        }

        lines.join("\n")
    }

    /// XML formatting (reserved for future)
    fn format_xml(&self, _context: &AgentContext) -> Option<String> {
        // TODO: Implement XML formatting
        None
    }

    /// JSON formatting (reserved for future)
    fn format_json(&self, _context: &AgentContext) -> Option<String> {
        // TODO: Implement JSON formatting
        None
    }
}

/// Format Unix timestamp as human-readable string
fn format_timestamp(timestamp: i64) -> String {
    use chrono::{DateTime, Utc};

    DateTime::<Utc>::from_timestamp(timestamp, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "Unknown".to_string())
}

/// Truncate text to max length (character-safe, not byte-based)
fn truncate_text(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        text.to_string()
    } else {
        // Find the byte index of the max_chars-th character boundary
        let truncate_at = text
            .char_indices()
            .nth(max_chars)
            .map(|(idx, _)| idx)
            .unwrap_or(text.len());
        format!("{}...", &text[..truncate_at])
    }
}

/// Escape Markdown special characters
///
/// Escapes characters that have special meaning in Markdown to prevent
/// formatting issues when displaying user-provided text.
fn escape_markdown(text: &str) -> String {
    text.replace('[', "\\[")
        .replace(']', "\\]")
        .replace('(', "\\(")
        .replace(')', "\\)")
        .replace('*', "\\*")
        .replace('_', "\\_")
        .replace('`', "\\`")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::ContextAnchor as MemoryContextAnchor;
    use crate::payload::{ContextAnchor, Intent, PayloadBuilder};

    #[test]
    fn test_assemble_system_prompt_no_context() {
        let assembler = PromptAssembler::new(ContextFormat::Markdown);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config("openai".to_string(), vec![], ContextFormat::Markdown)
            .user_input("Test".to_string())
            .build()
            .unwrap();

        let prompt = assembler.assemble_system_prompt("You are helpful.", &payload);
        assert_eq!(prompt, "You are helpful.");
    }

    #[test]
    fn test_assemble_system_prompt_with_memory() {
        let assembler = PromptAssembler::new(ContextFormat::Markdown);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        let memory_anchor =
            MemoryContextAnchor::with_timestamp("com.app".to_string(), "Window".to_string(), 1000);

        let memories = vec![MemoryEntry {
            id: "test-id".to_string(),
            context: memory_anchor,
            user_input: "Previous question".to_string(),
            ai_output: "Previous answer".to_string(),
            embedding: Some(vec![0.1; 512]),
            similarity_score: Some(0.9),
        }];

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config("openai".to_string(), vec![], ContextFormat::Markdown)
            .user_input("Test".to_string())
            .memory(memories)
            .build()
            .unwrap();

        let prompt = assembler.assemble_system_prompt("You are helpful.", &payload);

        assert!(prompt.starts_with("You are helpful."));
        assert!(prompt.contains("### Context Information"));
        assert!(prompt.contains("**Relevant History**"));
        assert!(prompt.contains("Previous question"));
        assert!(prompt.contains("Previous answer"));
    }

    #[test]
    fn test_format_memory_markdown() {
        let assembler = PromptAssembler::new(ContextFormat::Markdown);

        let memory_anchor = MemoryContextAnchor::with_timestamp(
            "com.app".to_string(),
            "Window".to_string(),
            1609459200, // 2021-01-01 00:00:00 UTC
        );

        let memories = vec![MemoryEntry {
            id: "test-id".to_string(),
            context: memory_anchor,
            user_input: "Test input".to_string(),
            ai_output: "Test output".to_string(),
            embedding: Some(vec![0.1; 512]),
            similarity_score: Some(0.85),
        }];

        let formatted = assembler.format_memory_markdown(&memories);

        assert!(formatted.contains("**Relevant History**"));
        assert!(formatted.contains("Test input"));
        assert!(formatted.contains("Test output"));
        assert!(formatted.contains("85%")); // Relevance score
    }

    #[test]
    fn test_truncate_text() {
        assert_eq!(truncate_text("Short", 10), "Short");
        assert_eq!(
            truncate_text("This is a very long text", 10),
            "This is a ..."
        );
    }

    #[test]
    fn test_truncate_text_utf8() {
        // Test with Chinese characters (3 bytes each in UTF-8)
        let chinese_text = "美军披露马杜罗被抓全过程";
        assert_eq!(truncate_text(chinese_text, 5), "美军披露马...");
        assert_eq!(truncate_text(chinese_text, 12), chinese_text);

        // Test with mixed content
        let mixed = "Hello 世界 Test 测试";
        assert_eq!(truncate_text(mixed, 8), "Hello 世界...");

        // Test edge case: truncate at exactly 300 chars with Chinese
        let long_chinese = "中".repeat(150);
        let truncated = truncate_text(&long_chinese, 100);
        assert!(truncated.ends_with("..."));
        // Should have 100 Chinese chars + "..."
        assert_eq!(truncated.chars().count(), 103); // 100 + 3 for "..."
    }

    #[test]
    fn test_format_timestamp() {
        let timestamp = 1609459200; // 2021-01-01 00:00:00 UTC
        let formatted = format_timestamp(timestamp);
        assert!(formatted.contains("2021-01-01"));
    }

    #[test]
    fn test_xml_format_reserved() {
        let assembler = PromptAssembler::new(ContextFormat::Xml);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config("openai".to_string(), vec![], ContextFormat::Xml)
            .user_input("Test".to_string())
            .build()
            .unwrap();

        let prompt = assembler.assemble_system_prompt("You are helpful.", &payload);

        // XML format not implemented, should return base prompt only
        assert_eq!(prompt, "You are helpful.");
    }

    #[test]
    fn test_json_format_reserved() {
        let assembler = PromptAssembler::new(ContextFormat::Json);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config("openai".to_string(), vec![], ContextFormat::Json)
            .user_input("Test".to_string())
            .build()
            .unwrap();

        let prompt = assembler.assemble_system_prompt("You are helpful.", &payload);

        // JSON format not implemented, should return base prompt only
        assert_eq!(prompt, "You are helpful.");
    }

    #[test]
    fn test_format_search_results_markdown() {
        let assembler = PromptAssembler::new(ContextFormat::Markdown);

        let results = vec![
            SearchResult {
                title: "Rust Programming Language".to_string(),
                url: "https://www.rust-lang.org".to_string(),
                snippet: "A language empowering everyone to build reliable and efficient software."
                    .to_string(),
                published_date: Some(1609459200), // 2021-01-01
                relevance_score: Some(0.95),
                source_type: Some("web".to_string()),
                full_content: None,
                provider: Some("tavily".to_string()),
            },
            SearchResult {
                title: "Getting Started with Rust".to_string(),
                url: "https://doc.rust-lang.org/book/".to_string(),
                snippet: "Learn Rust with The Rust Programming Language book".to_string(),
                published_date: None,
                relevance_score: None,
                source_type: None,
                full_content: None,
                provider: Some("brave".to_string()),
            },
        ];

        let formatted = assembler.format_search_results_markdown(&results);

        // Check header and instruction
        assert!(formatted.contains("**Web Search Results** (Retrieved by your search capability)"));
        assert!(formatted.contains("YOUR search capability in real-time"));

        // Check first result
        assert!(formatted.contains("1. [Rust Programming Language](https://www.rust-lang.org)"));
        assert!(formatted.contains("A language empowering everyone"));
        assert!(formatted.contains("Relevance: 95%"));
        assert!(formatted.contains("Published: 2021-01-01"));
        assert!(formatted.contains("Type: web"));
        assert!(formatted.contains("Source: tavily"));

        // Check second result
        assert!(
            formatted.contains("2. [Getting Started with Rust](https://doc.rust-lang.org/book/)")
        );
        assert!(formatted.contains("Learn Rust with The Rust Programming Language book"));
        assert!(formatted.contains("Source: brave"));
    }

    #[test]
    fn test_assemble_system_prompt_with_search_results() {
        let assembler = PromptAssembler::new(ContextFormat::Markdown);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        let results = vec![SearchResult {
            title: "Test Result".to_string(),
            url: "https://example.com".to_string(),
            snippet: "Test snippet".to_string(),
            published_date: None,
            relevance_score: Some(0.9),
            source_type: None,
            full_content: None,
            provider: Some("test".to_string()),
        }];

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config("openai".to_string(), vec![], ContextFormat::Markdown)
            .user_input("Test query".to_string())
            .search_results(results)
            .build()
            .unwrap();

        let prompt = assembler.assemble_system_prompt("You are helpful.", &payload);

        // Should contain base prompt
        assert!(prompt.starts_with("You are helpful."));

        // Should contain context section
        assert!(prompt.contains("### Context Information"));

        // Should contain search results with capability instruction
        assert!(prompt.contains("**Web Search Results** (Retrieved by your search capability)"));
        assert!(prompt.contains("[Test Result](https://example.com)"));
        assert!(prompt.contains("Test snippet"));
        assert!(prompt.contains("Relevance: 90%"));
    }

    #[test]
    fn test_escape_markdown() {
        assert_eq!(escape_markdown("Normal text"), "Normal text");
        assert_eq!(
            escape_markdown("Text with [brackets]"),
            "Text with \\[brackets\\]"
        );
        assert_eq!(
            escape_markdown("Link: [Title](url)"),
            "Link: \\[Title\\]\\(url\\)"
        );
        assert_eq!(escape_markdown("Bold *text*"), "Bold \\*text\\*");
        assert_eq!(escape_markdown("Code `snippet`"), "Code \\`snippet\\`");
        assert_eq!(escape_markdown("Under_score"), "Under\\_score");
    }

    #[test]
    fn test_assemble_with_memory_and_search() {
        let assembler = PromptAssembler::new(ContextFormat::Markdown);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        let memory_anchor =
            MemoryContextAnchor::with_timestamp("com.app".to_string(), "Window".to_string(), 1000);

        let memories = vec![MemoryEntry {
            id: "test-id".to_string(),
            context: memory_anchor,
            user_input: "Previous question".to_string(),
            ai_output: "Previous answer".to_string(),
            embedding: Some(vec![0.1; 512]),
            similarity_score: Some(0.9),
        }];

        let search_results = vec![SearchResult {
            title: "Search Result".to_string(),
            url: "https://example.com".to_string(),
            snippet: "Relevant information".to_string(),
            published_date: None,
            relevance_score: Some(0.85),
            source_type: None,
            full_content: None,
            provider: Some("test".to_string()),
        }];

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config("openai".to_string(), vec![], ContextFormat::Markdown)
            .user_input("Current question".to_string())
            .memory(memories)
            .search_results(search_results)
            .build()
            .unwrap();

        let prompt = assembler.assemble_system_prompt("You are helpful.", &payload);

        // Should contain both memory and search sections
        assert!(prompt.contains("**Relevant History**"));
        assert!(prompt.contains("Previous question"));
        assert!(prompt.contains("**Web Search Results** (Retrieved by your search capability)"));
        assert!(prompt.contains("Search Result"));
        assert!(prompt.contains("Relevant information"));
    }

    #[test]
    fn test_build_capability_aware_prompt_no_capabilities() {
        let assembler = PromptAssembler::new(ContextFormat::Markdown);

        let prompt = assembler.build_capability_aware_prompt("You are helpful.", &[], None);

        // Should only contain base prompt when no capabilities
        assert_eq!(prompt, "You are helpful.");
    }

    #[test]
    fn test_build_capability_aware_prompt_with_search() {
        let assembler = PromptAssembler::new(ContextFormat::Markdown);

        let capabilities = vec![CapabilityDeclaration::search()];

        let prompt = assembler.build_capability_aware_prompt("You are helpful.", &capabilities, None);

        // Should contain base prompt
        assert!(prompt.starts_with("You are helpful."));

        // Should contain capability instructions
        assert!(prompt.contains("## CRITICAL: Proactive Search Decision System"));
        assert!(prompt.contains("__capability_request__"));
        assert!(prompt.contains("Web Search"));
        assert!(prompt.contains("search"));

        // Should contain self-assessment instructions
        assert!(prompt.contains("Self-Assessment Before Every Response"));

        // Should contain decision framework
        assert!(prompt.contains("Decision Framework"));
    }

    #[test]
    fn test_build_capability_aware_prompt_with_multiple_capabilities() {
        let assembler = PromptAssembler::new(ContextFormat::Markdown);

        let capabilities = vec![
            CapabilityDeclaration::search(),
            CapabilityDeclaration::video(),
        ];

        let prompt = assembler.build_capability_aware_prompt("Base prompt.", &capabilities, None);

        // Should contain both capabilities
        assert!(prompt.contains("Web Search"));
        assert!(prompt.contains("Video Transcript"));
        assert!(prompt.contains("YouTube"));
    }

    #[test]
    fn test_build_capability_aware_prompt_filters_unavailable() {
        let assembler = PromptAssembler::new(ContextFormat::Markdown);

        let mut mcp = CapabilityDeclaration::mcp();
        mcp.available = false; // MCP is not available

        let capabilities = vec![CapabilityDeclaration::search(), mcp];

        let prompt = assembler.build_capability_aware_prompt("Base prompt.", &capabilities, None);

        // Should contain search but not MCP
        assert!(prompt.contains("Web Search"));
        assert!(!prompt.contains("MCP Tools"));
    }

    #[test]
    fn test_build_capability_aware_prompt_with_context() {
        let assembler = PromptAssembler::new(ContextFormat::Markdown);

        let capabilities = vec![CapabilityDeclaration::search()];

        // Create a context with memory
        let memory_anchor =
            MemoryContextAnchor::with_timestamp("com.app".to_string(), "Window".to_string(), 1000);

        let memories = vec![MemoryEntry {
            id: "test-id".to_string(),
            context: memory_anchor,
            user_input: "Previous question".to_string(),
            ai_output: "Previous answer".to_string(),
            embedding: None,
            similarity_score: Some(0.9),
        }];

        let context = AgentContext {
            memory_snippets: Some(memories),
            search_results: None,
            mcp_resources: None,
            video_transcript: None,
            workflow_state: None,
            attachments: None,
            skill_instructions: None,
        };

        let prompt = assembler.build_capability_aware_prompt("Base prompt.", &capabilities, Some(&context));

        // Should contain base prompt, capabilities, AND memory context
        assert!(prompt.contains("Base prompt."));
        assert!(prompt.contains("## CRITICAL: Proactive Search Decision System"));
        assert!(prompt.contains("### Context Information"));
        assert!(prompt.contains("**Relevant History**"));
        assert!(prompt.contains("Previous question"));
    }
}
