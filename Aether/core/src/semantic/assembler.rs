//! SmartPromptAssembler - Intelligent prompt assembly with token management
//!
//! Features:
//! - Template-based prompt construction
//! - Token budget management
//! - Smart truncation strategies
//! - Multi-format support (Markdown, XML, JSON)

use super::intent::SemanticIntent;
use super::template::{PromptTemplate, TemplateRegistry};
use crate::memory::MemoryEntry;
use crate::payload::{AgentContext, ContextFormat};
use crate::search::SearchResult;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Smart prompt assembler with token management
#[derive(Debug, Clone)]
pub struct SmartPromptAssembler {
    /// Template registry
    templates: TemplateRegistry,

    /// Output format
    format: ContextFormat,

    /// Maximum tokens for context
    max_context_tokens: usize,

    /// Truncation strategy
    truncation: TruncationStrategy,
}

impl SmartPromptAssembler {
    /// Create a new assembler with default settings
    pub fn new() -> Self {
        Self {
            templates: TemplateRegistry::with_defaults(),
            format: ContextFormat::Markdown,
            max_context_tokens: 4000,
            truncation: TruncationStrategy::Smart { preserve_recent: 2 },
        }
    }

    /// Create with custom settings
    pub fn with_config(
        format: ContextFormat,
        max_context_tokens: usize,
        truncation: TruncationStrategy,
    ) -> Self {
        Self {
            templates: TemplateRegistry::with_defaults(),
            format,
            max_context_tokens,
            truncation,
        }
    }

    /// Set template registry
    pub fn with_templates(mut self, templates: TemplateRegistry) -> Self {
        self.templates = templates;
        self
    }

    /// Register a custom template
    pub fn register_template(&mut self, template: PromptTemplate) {
        self.templates.register(template);
    }

    /// Assemble a prompt from intent and context
    ///
    /// # Arguments
    ///
    /// * `template_id` - Optional template ID (uses default if None)
    /// * `intent` - The semantic intent
    /// * `context` - Agent context with memory/search/etc.
    /// * `token_limit` - Maximum tokens for the final prompt
    pub fn assemble(
        &self,
        template_id: Option<&str>,
        intent: &SemanticIntent,
        context: &AgentContext,
        token_limit: usize,
    ) -> AssembledPrompt {
        // Get template
        let template = template_id
            .and_then(|id| self.templates.get(id))
            .unwrap_or_else(|| self.templates.default_template());

        // Build variables from intent
        let vars = self.extract_variables(intent);

        // Estimate base prompt tokens (without context)
        let base_prompt = template.render(&vars, None);
        let base_tokens = estimate_tokens(&base_prompt);

        // Calculate available budget for context
        let context_budget = token_limit.saturating_sub(base_tokens);
        let effective_budget = context_budget.min(self.max_context_tokens);

        // Truncate context to fit budget
        let (truncated_context, was_truncated) =
            self.truncate_context(context, effective_budget);

        // Render final prompt with context
        let final_prompt = if was_truncated || !context_is_empty(context) {
            let prompt_with_context = template.render(&vars, Some(&truncated_context));
            prompt_with_context
        } else {
            base_prompt.clone()
        };

        let final_tokens = estimate_tokens(&final_prompt);

        AssembledPrompt {
            prompt: final_prompt,
            estimated_tokens: final_tokens,
            truncation_applied: was_truncated,
            sections_included: count_sections(&truncated_context),
            intent_type: intent.intent_type.clone(),
            template_id: template.id.clone(),
        }
    }

    /// Assemble a simple prompt without template
    pub fn assemble_simple(
        &self,
        base_prompt: &str,
        context: &AgentContext,
        token_limit: usize,
    ) -> AssembledPrompt {
        let base_tokens = estimate_tokens(base_prompt);
        let context_budget = token_limit.saturating_sub(base_tokens);

        let (truncated_context, was_truncated) =
            self.truncate_context(context, context_budget);

        let context_text = self.format_context(&truncated_context);
        let final_prompt = if let Some(ctx_text) = context_text {
            format!("{}\n\n{}", base_prompt, ctx_text)
        } else {
            base_prompt.to_string()
        };

        AssembledPrompt {
            prompt: final_prompt.clone(),
            estimated_tokens: estimate_tokens(&final_prompt),
            truncation_applied: was_truncated,
            sections_included: count_sections(&truncated_context),
            intent_type: "general".to_string(),
            template_id: "simple".to_string(),
        }
    }

    /// Extract template variables from intent
    fn extract_variables(&self, intent: &SemanticIntent) -> HashMap<String, String> {
        let mut vars = HashMap::new();

        // Add intent parameters
        for (key, value) in &intent.params {
            if let Some(s) = value.to_string_value() {
                vars.insert(key.clone(), s);
            }
        }

        // Add standard variables
        vars.insert("intent_type".to_string(), intent.intent_type.clone());

        if let Some(ref cleaned) = intent.cleaned_input {
            vars.insert("user_input".to_string(), cleaned.clone());
            vars.insert("query".to_string(), cleaned.clone());
            vars.insert("text".to_string(), cleaned.clone());
        }

        vars
    }

    /// Truncate context to fit within token budget
    fn truncate_context(
        &self,
        context: &AgentContext,
        budget: usize,
    ) -> (AgentContext, bool) {
        let mut truncated = context.clone();

        // Estimate current context tokens
        let current_tokens = self.estimate_context_tokens(context);

        if current_tokens <= budget {
            return (truncated, false);
        }

        // Apply truncation strategy
        let was_truncated = match &self.truncation {
            TruncationStrategy::OldestFirst => {
                self.truncate_oldest_first(&mut truncated, budget)
            }
            TruncationStrategy::LowestRelevanceFirst => {
                self.truncate_by_relevance(&mut truncated, budget)
            }
            TruncationStrategy::Proportional => {
                self.truncate_proportional(&mut truncated, budget)
            }
            TruncationStrategy::Smart { preserve_recent } => {
                self.truncate_smart(&mut truncated, budget, *preserve_recent)
            }
        };

        (truncated, was_truncated)
    }

    /// Truncate by removing oldest items first
    fn truncate_oldest_first(&self, context: &mut AgentContext, budget: usize) -> bool {
        let mut modified = false;

        // Truncate memory (remove oldest)
        loop {
            let current_tokens = self.estimate_context_tokens(context);
            let mem_len = context.memory_snippets.as_ref().map(|m| m.len()).unwrap_or(0);

            if current_tokens <= budget || mem_len <= 1 {
                break;
            }

            if let Some(ref mut memories) = context.memory_snippets {
                memories.remove(0);
                modified = true;
            }
        }

        // Truncate search results (remove last/oldest)
        loop {
            let current_tokens = self.estimate_context_tokens(context);
            let results_len = context.search_results.as_ref().map(|r| r.len()).unwrap_or(0);

            if current_tokens <= budget || results_len <= 1 {
                break;
            }

            if let Some(ref mut results) = context.search_results {
                results.pop();
                modified = true;
            }
        }

        modified
    }

    /// Truncate by removing lowest relevance items first
    fn truncate_by_relevance(&self, context: &mut AgentContext, budget: usize) -> bool {
        let mut modified = false;

        // Sort memory by relevance and remove lowest
        if let Some(ref mut memories) = context.memory_snippets {
            memories.sort_by(|a, b| {
                b.similarity_score
                    .partial_cmp(&a.similarity_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        loop {
            let current_tokens = self.estimate_context_tokens(context);
            let mem_len = context.memory_snippets.as_ref().map(|m| m.len()).unwrap_or(0);

            if current_tokens <= budget || mem_len <= 1 {
                break;
            }

            if let Some(ref mut memories) = context.memory_snippets {
                memories.pop();
                modified = true;
            }
        }

        // Sort search results by relevance
        if let Some(ref mut results) = context.search_results {
            results.sort_by(|a, b| {
                b.relevance_score
                    .partial_cmp(&a.relevance_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        loop {
            let current_tokens = self.estimate_context_tokens(context);
            let results_len = context.search_results.as_ref().map(|r| r.len()).unwrap_or(0);

            if current_tokens <= budget || results_len <= 1 {
                break;
            }

            if let Some(ref mut results) = context.search_results {
                results.pop();
                modified = true;
            }
        }

        modified
    }

    /// Truncate proportionally across all sections
    fn truncate_proportional(&self, context: &mut AgentContext, budget: usize) -> bool {
        let current = self.estimate_context_tokens(context);
        if current <= budget {
            return false;
        }

        let ratio = budget as f64 / current as f64;
        let mut modified = false;

        if let Some(ref mut memories) = context.memory_snippets {
            let new_len = ((memories.len() as f64 * ratio).ceil() as usize).max(1);
            if new_len < memories.len() {
                memories.truncate(new_len);
                modified = true;
            }
        }

        if let Some(ref mut results) = context.search_results {
            let new_len = ((results.len() as f64 * ratio).ceil() as usize).max(1);
            if new_len < results.len() {
                results.truncate(new_len);
                modified = true;
            }
        }

        modified
    }

    /// Smart truncation: preserve recent items, remove by relevance
    fn truncate_smart(
        &self,
        context: &mut AgentContext,
        budget: usize,
        preserve_recent: usize,
    ) -> bool {
        let mut modified = false;

        // For memory: keep most recent, then by relevance
        if let Some(ref mut memories) = context.memory_snippets {
            if memories.len() > preserve_recent {
                // Keep recent ones
                let recent: Vec<_> = memories.iter().rev().take(preserve_recent).cloned().collect();

                // Sort rest by relevance
                let mut older: Vec<_> = memories
                    .iter()
                    .rev()
                    .skip(preserve_recent)
                    .cloned()
                    .collect();
                older.sort_by(|a, b| {
                    b.similarity_score
                        .partial_cmp(&a.similarity_score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });

                // Rebuild with recent + sorted older
                *memories = older;
                memories.extend(recent);
            }
        }

        // Apply relevance-based truncation
        modified = self.truncate_by_relevance(context, budget) || modified;

        modified
    }

    /// Estimate tokens for context
    fn estimate_context_tokens(&self, context: &AgentContext) -> usize {
        let mut tokens = 0;

        if let Some(ref memories) = context.memory_snippets {
            for m in memories {
                tokens += estimate_tokens(&m.user_input);
                tokens += estimate_tokens(&m.ai_output);
                tokens += 50; // Overhead for formatting
            }
        }

        if let Some(ref results) = context.search_results {
            for r in results {
                tokens += estimate_tokens(&r.title);
                tokens += estimate_tokens(&r.snippet);
                tokens += 30; // Overhead
            }
        }

        if let Some(ref transcript) = context.video_transcript {
            tokens += estimate_tokens(&transcript.format_for_context());
        }

        tokens
    }

    /// Format context according to configured format
    fn format_context(&self, context: &AgentContext) -> Option<String> {
        match self.format {
            ContextFormat::Markdown => self.format_markdown(context),
            ContextFormat::Xml => self.format_xml(context),
            ContextFormat::Json => self.format_json(context),
        }
    }

    /// Format context as Markdown
    fn format_markdown(&self, context: &AgentContext) -> Option<String> {
        let mut sections = Vec::new();

        if let Some(ref memories) = context.memory_snippets {
            if !memories.is_empty() {
                sections.push(format_memory_markdown(memories));
            }
        }

        if let Some(ref results) = context.search_results {
            if !results.is_empty() {
                sections.push(format_search_markdown(results));
            }
        }

        if let Some(ref transcript) = context.video_transcript {
            sections.push(transcript.format_for_context());
        }

        if sections.is_empty() {
            None
        } else {
            Some(format!("### Context Information\n\n{}", sections.join("\n\n")))
        }
    }

    /// Format context as XML (placeholder)
    fn format_xml(&self, _context: &AgentContext) -> Option<String> {
        // TODO: Implement XML formatting
        None
    }

    /// Format context as JSON (placeholder)
    fn format_json(&self, _context: &AgentContext) -> Option<String> {
        // TODO: Implement JSON formatting
        None
    }
}

impl Default for SmartPromptAssembler {
    fn default() -> Self {
        Self::new()
    }
}

/// Truncation strategy for context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TruncationStrategy {
    /// Remove oldest context items first
    OldestFirst,

    /// Remove lowest relevance items first
    LowestRelevanceFirst,

    /// Reduce all sections proportionally
    Proportional,

    /// Smart: preserve recent items, then by relevance
    Smart {
        /// Number of recent items to always preserve
        preserve_recent: usize,
    },
}

impl Default for TruncationStrategy {
    fn default() -> Self {
        Self::Smart { preserve_recent: 2 }
    }
}

/// Result of prompt assembly
#[derive(Debug, Clone)]
pub struct AssembledPrompt {
    /// Final assembled prompt
    pub prompt: String,

    /// Estimated token count
    pub estimated_tokens: usize,

    /// Whether truncation was applied
    pub truncation_applied: bool,

    /// Number of context sections included
    pub sections_included: usize,

    /// Intent type used
    pub intent_type: String,

    /// Template ID used
    pub template_id: String,
}

impl AssembledPrompt {
    /// Check if prompt fits within token limit
    pub fn fits_in(&self, token_limit: usize) -> bool {
        self.estimated_tokens <= token_limit
    }
}

/// Estimate token count for text (rough approximation)
///
/// Uses a simple heuristic: ~4 characters per token for English,
/// ~1-2 characters for CJK.
pub fn estimate_tokens(text: &str) -> usize {
    let char_count = text.chars().count();
    let cjk_count = text
        .chars()
        .filter(|c| {
            let code = *c as u32;
            (0x4E00..=0x9FFF).contains(&code) || (0x3400..=0x4DBF).contains(&code)
        })
        .count();

    let english_chars = char_count - cjk_count;

    // Rough estimate: 4 chars/token for English, 1.5 chars/token for CJK
    let english_tokens = english_chars / 4;
    let cjk_tokens = (cjk_count as f64 / 1.5).ceil() as usize;

    english_tokens + cjk_tokens + 1 // +1 to avoid zero
}

/// Check if context is empty
fn context_is_empty(context: &AgentContext) -> bool {
    context.memory_snippets.as_ref().map(|v| v.is_empty()).unwrap_or(true)
        && context.search_results.as_ref().map(|v| v.is_empty()).unwrap_or(true)
        && context.video_transcript.is_none()
}

/// Count non-empty sections in context
fn count_sections(context: &AgentContext) -> usize {
    let mut count = 0;

    if context.memory_snippets.as_ref().map(|v| !v.is_empty()).unwrap_or(false) {
        count += 1;
    }

    if context.search_results.as_ref().map(|v| !v.is_empty()).unwrap_or(false) {
        count += 1;
    }

    if context.video_transcript.is_some() {
        count += 1;
    }

    count
}

/// Format memory entries as Markdown
fn format_memory_markdown(memories: &[MemoryEntry]) -> String {
    let mut lines = vec!["**Relevant History**:".to_string()];

    for (i, entry) in memories.iter().enumerate() {
        lines.push(format!(
            "\n{}. **Conversation at {}**",
            i + 1,
            format_timestamp(entry.context.timestamp)
        ));
        lines.push(format!("   App: {}", entry.context.app_bundle_id));
        lines.push(format!("   Window: {}", entry.context.window_title));
        lines.push(format!("   User: {}", truncate_text(&entry.user_input, 200)));
        lines.push(format!("   AI: {}", truncate_text(&entry.ai_output, 200)));

        if let Some(score) = entry.similarity_score {
            lines.push(format!("   Relevance: {:.0}%", score * 100.0));
        }
    }

    lines.join("\n")
}

/// Format search results as Markdown
fn format_search_markdown(results: &[SearchResult]) -> String {
    let mut lines = vec!["**Web Search Results**:".to_string()];

    for (i, result) in results.iter().enumerate() {
        lines.push(format!(
            "\n{}. [{}]({})",
            i + 1,
            result.title.replace('[', "\\[").replace(']', "\\]"),
            result.url
        ));

        if !result.snippet.is_empty() {
            lines.push(format!("   {}", truncate_text(&result.snippet, 300)));
        }

        let mut metadata = Vec::new();

        if let Some(timestamp) = result.published_date {
            metadata.push(format!("Published: {}", format_timestamp(timestamp)));
        }

        if let Some(score) = result.relevance_score {
            metadata.push(format!("Relevance: {:.0}%", score * 100.0));
        }

        if let Some(ref provider) = result.provider {
            metadata.push(format!("Source: {}", provider));
        }

        if !metadata.is_empty() {
            lines.push(format!("   _{}_", metadata.join(" | ")));
        }
    }

    lines.join("\n")
}

/// Format Unix timestamp
fn format_timestamp(timestamp: i64) -> String {
    use chrono::{DateTime, Utc};

    DateTime::<Utc>::from_timestamp(timestamp, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "Unknown".to_string())
}

/// Truncate text to max characters
fn truncate_text(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        text.to_string()
    } else {
        let truncate_at = text
            .char_indices()
            .nth(max_chars)
            .map(|(idx, _)| idx)
            .unwrap_or(text.len());
        format!("{}...", &text[..truncate_at])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::ContextAnchor;

    #[test]
    fn test_estimate_tokens() {
        // English text
        let english = "This is a test sentence for token estimation.";
        let tokens = estimate_tokens(english);
        assert!(tokens > 0 && tokens < 20);

        // Chinese text
        let chinese = "这是一个测试句子";
        let tokens = estimate_tokens(chinese);
        assert!(tokens > 0 && tokens < 15);

        // Mixed
        let mixed = "Hello 世界 Test 测试";
        let tokens = estimate_tokens(mixed);
        assert!(tokens > 0);
    }

    #[test]
    fn test_simple_assembly() {
        let assembler = SmartPromptAssembler::new();
        let context = AgentContext::default();

        let result = assembler.assemble_simple(
            "You are a helpful assistant.",
            &context,
            4000,
        );

        assert!(!result.truncation_applied);
        assert!(result.prompt.contains("helpful assistant"));
    }

    #[test]
    fn test_truncation_strategy() {
        let assembler = SmartPromptAssembler::with_config(
            ContextFormat::Markdown,
            100, // Very small budget to force truncation
            TruncationStrategy::OldestFirst,
        );

        let memories: Vec<MemoryEntry> = (0..10)
            .map(|i| MemoryEntry {
                id: format!("mem-{}", i),
                context: ContextAnchor::with_timestamp(
                    "com.app".to_string(),
                    "Window".to_string(),
                    1000 + i,
                ),
                user_input: format!("Question {}", i),
                ai_output: format!("Answer {} with some extra text to make it longer", i),
                embedding: None,
                similarity_score: Some(0.8 - (i as f32 * 0.05)),
            })
            .collect();

        let context = AgentContext {
            memory_snippets: Some(memories),
            search_results: None,
            mcp_resources: None,
            video_transcript: None,
            workflow_state: None,
            attachments: None,
        };

        let (truncated, was_truncated) = assembler.truncate_context(&context, 100);

        // Should have been truncated
        assert!(was_truncated);
        assert!(
            truncated
                .memory_snippets
                .as_ref()
                .map(|m| m.len())
                .unwrap_or(0)
                < 10
        );
    }

    #[test]
    fn test_template_based_assembly() {
        let mut assembler = SmartPromptAssembler::new();

        assembler.register_template(
            PromptTemplate::with_vars(
                "custom",
                "Query: {{query}}\nMode: {{mode}}",
                vec![
                    super::super::template::TemplateVariable::required("query"),
                    super::super::template::TemplateVariable::optional("mode", "default"),
                ],
            ),
        );

        let intent = SemanticIntent::general()
            .with_param("query", "weather".into())
            .with_cleaned_input("weather in Beijing");

        let context = AgentContext::default();
        let result = assembler.assemble(Some("custom"), &intent, &context, 4000);

        assert!(result.prompt.contains("weather"));
        assert!(result.prompt.contains("Mode: default"));
    }
}
