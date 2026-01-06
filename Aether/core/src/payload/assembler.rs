/// PromptAssembler - Assembles final prompts from AgentPayload
///
/// This module formats context data (memory, search, MCP) into LLM prompts
/// using different formats (Markdown, XML, JSON).
use super::{AgentContext, AgentPayload, ContextFormat};
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
    fn format_search_results_markdown(&self, results: &[SearchResult]) -> String {
        let mut lines = vec!["**Web Search Results**:".to_string()];

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
            embedding: Some(vec![0.1; 384]),
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
            embedding: Some(vec![0.1; 384]),
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

        // Check header
        assert!(formatted.contains("**Web Search Results**"));

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

        // Should contain search results
        assert!(prompt.contains("**Web Search Results**"));
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
            embedding: Some(vec![0.1; 384]),
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
        assert!(prompt.contains("**Web Search Results**"));
        assert!(prompt.contains("Search Result"));
        assert!(prompt.contains("Relevant information"));
    }
}
