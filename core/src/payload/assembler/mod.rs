/// PromptAssembler - Assembles final prompts from AgentPayload
///
/// This module formats context data (memory, search, MCP) into LLM prompts
/// using different formats (Markdown, XML, JSON).
///
/// # Module Structure
///
/// - `core`: Core PromptAssembler struct and main public methods
/// - `capability`: Capability instruction formatting
/// - `context`: Context formatting (memory, search, MCP)
/// - `tools`: Tool list formatting
/// - `intent`: Intent-based prompt building
/// - `formatters`: Individual content formatters

mod capability;
mod context;
mod core;
mod formatters;
mod intent;
mod tools;

// Re-export the main struct
pub use self::core::PromptAssembler;

// The PromptAssembler methods for intent-based building delegate to the intent module
impl PromptAssembler {
    /// Build prompt with agent mode injection based on `IntentResult`.
    ///
    /// When the result is `Execute` or `DirectTool`, injects the Agent Mode
    /// Prompt.
    pub fn build_prompt_with_intent_result(
        &self,
        base_prompt: &str,
        capabilities: &[crate::capability::CapabilityDeclaration],
        context: Option<&super::AgentContext>,
        result: Option<&crate::intent::types::IntentResult>,
    ) -> String {
        intent::build_prompt_with_intent_result(
            &self.context_format,
            base_prompt,
            capabilities,
            context,
            result,
        )
    }

    /// Build prompt using the new `IntentResult` enum.
    ///
    /// This is the `IntentResult`-based prompt builder.
    pub fn build_prompt_for_intent(
        &self,
        result: &crate::intent::types::IntentResult,
        tools: &[crate::prompt::ToolInfo],
        context: Option<&super::AgentContext>,
        config: Option<&crate::prompt::PromptConfig>,
    ) -> String {
        intent::build_prompt_for_intent(
            &self.context_format,
            result,
            tools,
            context,
            config,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::CapabilityDeclaration;
    use crate::memory::ContextAnchor as MemoryContextAnchor;
    use crate::memory::MemoryEntry;
    use crate::payload::{AgentContext, ContextAnchor, ContextFormat, Intent, PayloadBuilder};
    use crate::search::SearchResult;
    use crate::utils::text_format::{escape_markdown, format_timestamp, truncate_text};

    #[test]
    fn test_assemble_system_prompt_no_context() {
        let assembler = PromptAssembler::new(ContextFormat::Markdown);

        let anchor = ContextAnchor::new(None);

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

        let anchor = ContextAnchor::new(None);

        let memory_anchor =
            MemoryContextAnchor::with_timestamp("Window".to_string(), 1000);

        let memories = vec![MemoryEntry {
            id: "test-id".to_string(),
            context: memory_anchor,
            user_input: "Previous question".to_string(),
            ai_output: "Previous answer".to_string(),
            embedding: Some(vec![0.1; 512]),
            namespace: "owner".to_string(),
            workspace: "default".to_string(),
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
        assert!(prompt.contains("**Related Conversation History**"));
        assert!(prompt.contains("Previous question"));
        assert!(prompt.contains("Previous answer"));
    }

    #[test]
    fn test_format_memory_markdown() {
        let memory_anchor = MemoryContextAnchor::with_timestamp(
            "Window".to_string(),
            1609459200, // 2021-01-01 00:00:00 UTC
        );

        let memories = vec![MemoryEntry {
            id: "test-id".to_string(),
            context: memory_anchor,
            user_input: "Test input".to_string(),
            ai_output: "Test output".to_string(),
            embedding: Some(vec![0.1; 512]),
            namespace: "owner".to_string(),
            workspace: "default".to_string(),
            similarity_score: Some(0.85),
        }];

        let formatted = formatters::format_memory_markdown(&memories);

        assert!(formatted.contains("**Related Conversation History**"));
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

        let anchor = ContextAnchor::new(None);

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

        let anchor = ContextAnchor::new(None);

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

        let formatted = formatters::format_search_results_markdown(&results);

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

        let anchor = ContextAnchor::new(None);

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

        let anchor = ContextAnchor::new(None);

        let memory_anchor =
            MemoryContextAnchor::with_timestamp("Window".to_string(), 1000);

        let memories = vec![MemoryEntry {
            id: "test-id".to_string(),
            context: memory_anchor,
            user_input: "Previous question".to_string(),
            ai_output: "Previous answer".to_string(),
            embedding: Some(vec![0.1; 512]),
            namespace: "owner".to_string(),
            workspace: "default".to_string(),
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
        assert!(prompt.contains("**Related Conversation History**"));
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

        let prompt =
            assembler.build_capability_aware_prompt("You are helpful.", &capabilities, None);

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
            CapabilityDeclaration::mcp(),
        ];

        let prompt = assembler.build_capability_aware_prompt("Base prompt.", &capabilities, None);

        // Should contain both capabilities
        assert!(prompt.contains("Web Search"));
        assert!(prompt.contains("Tool Execution"));
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
            MemoryContextAnchor::with_timestamp("Window".to_string(), 1000);

        let memories = vec![MemoryEntry {
            id: "test-id".to_string(),
            context: memory_anchor,
            user_input: "Previous question".to_string(),
            ai_output: "Previous answer".to_string(),
            embedding: None,
            namespace: "owner".to_string(),
            workspace: "default".to_string(),
            similarity_score: Some(0.9),
        }];

        let context = AgentContext {
            memory_facts: None,
            memory_snippets: Some(memories),
            search_results: None,
            mcp_resources: None,
            mcp_tool_result: None,
            webfetch_content: None,
            workflow_state: None,
            attachments: None,
            skill_instructions: None,
            available_skills: None,
        };

        let prompt =
            assembler.build_capability_aware_prompt("Base prompt.", &capabilities, Some(&context));

        // Should contain base prompt, capabilities, AND memory context
        assert!(prompt.contains("Base prompt."));
        assert!(prompt.contains("## CRITICAL: Proactive Search Decision System"));
        assert!(prompt.contains("### Context Information"));
        assert!(prompt.contains("**Related Conversation History**"));
        assert!(prompt.contains("Previous question"));
    }

    #[test]
    fn test_build_prompt_with_intent_result_execute() {
        use crate::intent::types::{DetectionLayer, ExecuteMetadata, IntentResult};

        let assembler = PromptAssembler::new(ContextFormat::Markdown);

        let result = IntentResult::Execute {
            confidence: 0.9,
            metadata: ExecuteMetadata {
                layer: DetectionLayer::L2,
                keyword_tag: None,
                detected_path: Some("/Downloads".to_string()),
                detected_url: None,
                context_hint: None,
            },
        };

        let prompt = assembler.build_prompt_with_intent_result("Base prompt.", &[], None, Some(&result));

        // Should contain base prompt
        assert!(prompt.contains("Base prompt."));

        // Should contain agent mode prompt
        assert!(prompt.contains("# Role"));
        assert!(prompt.contains("task executor"));
    }

    #[test]
    fn test_build_prompt_with_intent_result_converse() {
        use crate::intent::types::IntentResult;

        let assembler = PromptAssembler::new(ContextFormat::Markdown);

        let result = IntentResult::Converse { confidence: 0.8 };

        let prompt = assembler.build_prompt_with_intent_result("Base prompt.", &[], None, Some(&result));

        // Should contain base prompt
        assert!(prompt.contains("Base prompt."));

        // Should NOT contain agent mode prompt
        assert!(!prompt.contains("Agent Execution Mode"));
    }

    #[test]
    fn test_build_prompt_with_intent_result_none() {
        let assembler = PromptAssembler::new(ContextFormat::Markdown);

        let prompt = assembler.build_prompt_with_intent_result("Base prompt.", &[], None, None);

        // Should contain only base prompt
        assert_eq!(prompt, "Base prompt.");
    }
}
