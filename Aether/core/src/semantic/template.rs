//! PromptTemplate - Dynamic prompt template system with variable substitution
//!
//! Supports:
//! - Variable placeholders: {{variable_name}}
//! - Context sections: Memory, Search, History, AppContext, TimeContext
//! - Default values for optional variables

use crate::memory::MemoryEntry;
use crate::payload::AgentContext;
use crate::search::SearchResult;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Prompt template with variable substitution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTemplate {
    /// Template ID
    pub id: String,

    /// Template name (for display)
    pub name: Option<String>,

    /// Template text with placeholders
    pub template: String,

    /// Variable definitions
    #[serde(default)]
    pub variables: Vec<TemplateVariable>,

    /// Context sections to include
    #[serde(default)]
    pub sections: Vec<ContextSection>,
}

impl PromptTemplate {
    /// Create a new simple template
    pub fn new(id: impl Into<String>, template: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: None,
            template: template.into(),
            variables: Vec::new(),
            sections: Vec::new(),
        }
    }

    /// Create a template with variables
    pub fn with_vars(
        id: impl Into<String>,
        template: impl Into<String>,
        variables: Vec<TemplateVariable>,
    ) -> Self {
        Self {
            id: id.into(),
            name: None,
            template: template.into(),
            variables,
            sections: Vec::new(),
        }
    }

    /// Set template name
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Add context sections
    pub fn with_sections(mut self, sections: Vec<ContextSection>) -> Self {
        self.sections = sections;
        self
    }

    /// Render template with variables and context
    pub fn render(
        &self,
        vars: &HashMap<String, String>,
        context: Option<&AgentContext>,
    ) -> String {
        let mut result = self.template.clone();

        // Substitute variables
        for var in &self.variables {
            let placeholder = format!("{{{{{}}}}}", var.name);
            let value = vars
                .get(&var.name)
                .or(var.default.as_ref())
                .map(|s| s.as_str())
                .unwrap_or("");

            result = result.replace(&placeholder, value);
        }

        // Also substitute any ad-hoc variables not in the definition
        for (key, value) in vars {
            let placeholder = format!("{{{{{}}}}}", key);
            if result.contains(&placeholder) {
                result = result.replace(&placeholder, value);
            }
        }

        // Append context sections
        if let Some(ctx) = context {
            let sections_text = self.render_sections(ctx);
            if !sections_text.is_empty() {
                result.push_str("\n\n");
                result.push_str(&sections_text);
            }
        }

        result
    }

    /// Render context sections
    fn render_sections(&self, context: &AgentContext) -> String {
        let mut sections = Vec::new();

        for section in &self.sections {
            if let Some(text) = self.render_section(section, context) {
                sections.push(text);
            }
        }

        sections.join("\n\n")
    }

    /// Render a single context section
    fn render_section(&self, section: &ContextSection, context: &AgentContext) -> Option<String> {
        match section {
            ContextSection::Memory { max_items } => {
                context.memory_snippets.as_ref().and_then(|memories| {
                    if memories.is_empty() {
                        return None;
                    }
                    let limited: Vec<_> = memories.iter().take(*max_items).collect();
                    Some(format_memory_section(&limited))
                })
            }

            ContextSection::Search {
                max_results,
                include_snippets,
            } => {
                context.search_results.as_ref().and_then(|results| {
                    if results.is_empty() {
                        return None;
                    }
                    let limited: Vec<_> = results.iter().take(*max_results).collect();
                    Some(format_search_section(&limited, *include_snippets))
                })
            }

            ContextSection::History { max_turns: _, format: _ } => {
                // Note: History is typically in ConversationContext, not AgentContext
                // This is a placeholder for future integration
                None
            }

            ContextSection::AppContext => {
                // App context would be passed separately
                None
            }

            ContextSection::TimeContext => {
                // Time context would be passed separately
                None
            }

            ContextSection::Custom { name, content } => {
                if content.is_empty() {
                    None
                } else {
                    Some(format!("**{}**:\n{}", name, content))
                }
            }

            ContextSection::VideoTranscript { max_length: _ } => {
                context.video_transcript.as_ref().map(|transcript| {
                    transcript.format_for_context()
                })
            }
        }
    }

    /// Get required variable names
    pub fn required_variables(&self) -> Vec<&str> {
        self.variables
            .iter()
            .filter(|v| v.required && v.default.is_none())
            .map(|v| v.name.as_str())
            .collect()
    }

    /// Check if all required variables are provided
    pub fn validate_vars(&self, vars: &HashMap<String, String>) -> Result<(), Vec<String>> {
        let missing: Vec<String> = self
            .required_variables()
            .into_iter()
            .filter(|name| !vars.contains_key(*name))
            .map(|s| s.to_string())
            .collect();

        if missing.is_empty() {
            Ok(())
        } else {
            Err(missing)
        }
    }
}

impl Default for PromptTemplate {
    fn default() -> Self {
        Self::new("default", "You are a helpful AI assistant.")
    }
}

/// Template variable definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateVariable {
    /// Variable name (used in {{name}} placeholders)
    pub name: String,

    /// Default value if not provided
    pub default: Option<String>,

    /// Whether this variable is required
    #[serde(default)]
    pub required: bool,

    /// Description (for documentation)
    pub description: Option<String>,
}

impl TemplateVariable {
    /// Create a new required variable
    pub fn required(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            default: None,
            required: true,
            description: None,
        }
    }

    /// Create a new optional variable with default
    pub fn optional(name: impl Into<String>, default: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            default: Some(default.into()),
            required: false,
            description: None,
        }
    }

    /// Add description
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }
}

/// Context section types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContextSection {
    /// Memory context (RAG results)
    Memory {
        /// Maximum number of memory entries to include
        max_items: usize,
    },

    /// Search results
    Search {
        /// Maximum number of search results
        max_results: usize,
        /// Whether to include snippets
        #[serde(default = "default_true")]
        include_snippets: bool,
    },

    /// Conversation history
    History {
        /// Maximum number of turns
        max_turns: usize,
        /// Format: "brief" or "full"
        #[serde(default = "default_brief")]
        format: String,
    },

    /// Application context
    AppContext,

    /// Time context
    TimeContext,

    /// Video transcript
    VideoTranscript {
        /// Maximum length in characters
        max_length: usize,
    },

    /// Custom section
    Custom {
        name: String,
        content: String,
    },
}

fn default_true() -> bool {
    true
}

fn default_brief() -> String {
    "brief".to_string()
}

/// Format memory entries as a section
fn format_memory_section(memories: &[&MemoryEntry]) -> String {
    let mut lines = vec!["**Relevant History**:".to_string()];

    for (i, entry) in memories.iter().enumerate() {
        lines.push(format!(
            "\n{}. **Conversation at {}**",
            i + 1,
            format_timestamp(entry.context.timestamp)
        ));
        lines.push(format!("   App: {}", entry.context.app_bundle_id));
        lines.push(format!("   Window: {}", entry.context.window_title));
        lines.push(format!("   User: {}", truncate(&entry.user_input, 200)));
        lines.push(format!("   AI: {}", truncate(&entry.ai_output, 200)));

        if let Some(score) = entry.similarity_score {
            lines.push(format!("   Relevance: {:.0}%", score * 100.0));
        }
    }

    lines.join("\n")
}

/// Format search results as a section
fn format_search_section(results: &[&SearchResult], include_snippets: bool) -> String {
    let mut lines = vec!["**Web Search Results**:".to_string()];

    for (i, result) in results.iter().enumerate() {
        lines.push(format!(
            "\n{}. [{}]({})",
            i + 1,
            escape_markdown(&result.title),
            result.url
        ));

        if include_snippets && !result.snippet.is_empty() {
            lines.push(format!("   {}", truncate(&result.snippet, 300)));
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

/// Format Unix timestamp as human-readable string
fn format_timestamp(timestamp: i64) -> String {
    use chrono::{DateTime, Utc};

    DateTime::<Utc>::from_timestamp(timestamp, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "Unknown".to_string())
}

/// Truncate text to max characters
fn truncate(text: &str, max_chars: usize) -> String {
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

/// Escape Markdown special characters
fn escape_markdown(text: &str) -> String {
    text.replace('[', "\\[")
        .replace(']', "\\]")
        .replace('(', "\\(")
        .replace(')', "\\)")
        .replace('*', "\\*")
        .replace('_', "\\_")
        .replace('`', "\\`")
}

/// Template registry for managing multiple templates
#[derive(Debug, Clone, Default)]
pub struct TemplateRegistry {
    templates: HashMap<String, PromptTemplate>,
}

impl TemplateRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Create with default templates
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();

        // Default assistant template
        registry.register(PromptTemplate::new(
            "default",
            "You are a helpful AI assistant.",
        ));

        // Search template
        registry.register(
            PromptTemplate::with_vars(
                "search",
                "You are a search assistant. Answer based on the search results provided.\n\nUser query: {{query}}",
                vec![TemplateVariable::required("query")],
            )
            .with_sections(vec![ContextSection::Search {
                max_results: 5,
                include_snippets: true,
            }]),
        );

        // Translation template
        registry.register(
            PromptTemplate::with_vars(
                "translate",
                "You are a professional translator. Translate the following text to {{target_language}}.\n\nText to translate: {{text}}",
                vec![
                    TemplateVariable::required("text"),
                    TemplateVariable::optional("target_language", "English"),
                ],
            ),
        );

        // Code assistant template
        registry.register(
            PromptTemplate::new(
                "code",
                "You are a senior software engineer. Provide clear, efficient, and well-documented code solutions.",
            )
            .with_sections(vec![ContextSection::Memory { max_items: 3 }]),
        );

        registry
    }

    /// Register a template
    pub fn register(&mut self, template: PromptTemplate) {
        self.templates.insert(template.id.clone(), template);
    }

    /// Get a template by ID
    pub fn get(&self, id: &str) -> Option<&PromptTemplate> {
        self.templates.get(id)
    }

    /// Get default template
    pub fn default_template(&self) -> &PromptTemplate {
        self.templates.get("default").expect("Default template must exist")
    }

    /// List all template IDs
    pub fn list(&self) -> Vec<&str> {
        self.templates.keys().map(|s| s.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_template() {
        let template = PromptTemplate::new("test", "Hello, {{name}}!");
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "World".to_string());

        let result = template.render(&vars, None);
        assert_eq!(result, "Hello, World!");
    }

    #[test]
    fn test_template_with_default() {
        let template = PromptTemplate::with_vars(
            "test",
            "Language: {{lang}}",
            vec![TemplateVariable::optional("lang", "English")],
        );

        // Without providing value - should use default
        let result = template.render(&HashMap::new(), None);
        assert_eq!(result, "Language: English");

        // With provided value
        let mut vars = HashMap::new();
        vars.insert("lang".to_string(), "Chinese".to_string());
        let result = template.render(&vars, None);
        assert_eq!(result, "Language: Chinese");
    }

    #[test]
    fn test_template_validation() {
        let template = PromptTemplate::with_vars(
            "test",
            "{{required_var}} {{optional_var}}",
            vec![
                TemplateVariable::required("required_var"),
                TemplateVariable::optional("optional_var", "default"),
            ],
        );

        // Missing required
        let result = template.validate_vars(&HashMap::new());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains(&"required_var".to_string()));

        // With required
        let mut vars = HashMap::new();
        vars.insert("required_var".to_string(), "value".to_string());
        let result = template.validate_vars(&vars);
        assert!(result.is_ok());
    }

    #[test]
    fn test_template_registry() {
        let registry = TemplateRegistry::with_defaults();

        assert!(registry.get("default").is_some());
        assert!(registry.get("search").is_some());
        assert!(registry.get("translate").is_some());
        assert!(registry.get("code").is_some());
    }

    #[test]
    fn test_ad_hoc_variables() {
        let template = PromptTemplate::new("test", "User: {{user}}, Action: {{action}}");
        let mut vars = HashMap::new();
        vars.insert("user".to_string(), "Alice".to_string());
        vars.insert("action".to_string(), "search".to_string());

        let result = template.render(&vars, None);
        assert_eq!(result, "User: Alice, Action: search");
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("Short", 10), "Short");
        assert_eq!(truncate("This is long text", 10), "This is lo...");

        // Chinese characters
        let chinese = "美军披露马杜罗被抓全过程";
        let truncated = truncate(chinese, 5);
        assert_eq!(truncated, "美军披露马...");
    }

    #[test]
    fn test_escape_markdown() {
        assert_eq!(escape_markdown("Normal"), "Normal");
        assert_eq!(escape_markdown("[link](url)"), "\\[link\\]\\(url\\)");
        assert_eq!(escape_markdown("*bold*"), "\\*bold\\*");
    }
}
