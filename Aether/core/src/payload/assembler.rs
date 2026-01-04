/// PromptAssembler - Assembles final prompts from AgentPayload
///
/// This module formats context data (memory, search, MCP) into LLM prompts
/// using different formats (Markdown, XML, JSON).
use super::{AgentContext, AgentPayload, ContextFormat};
use crate::memory::MemoryEntry;

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

    /// Format context data
    ///
    /// Selects formatting strategy based on context_format
    fn format_context(&self, context: &AgentContext) -> Option<String> {
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

        // Search section (reserved for future)
        if let Some(_results) = &context.search_results {
            // TODO: Implement search formatting
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

/// Truncate text to max length
fn truncate_text(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_string()
    } else {
        format!("{}...", &text[..max_len])
    }
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
}
