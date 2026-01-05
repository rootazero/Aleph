/// Prompt Assembler - Central component for structured prompt construction
///
/// This component implements the "Assembler" pattern from the structured context protocol.
/// It takes user input, parses intent, applies middleware transformations, and builds
/// the final payload ready for AI provider consumption.
use super::{AgentIntent, AgentPayload, AppContext};
use crate::config::RoutingRuleConfig;
use crate::error::{AetherError, Result};
use regex::Regex;
use std::collections::HashMap;
use tracing::{debug, info};

/// Prompt template library
///
/// This would typically load from a database or configuration file.
/// For now, we maintain an in-memory HashMap.
pub struct TemplateLibrary {
    templates: HashMap<String, String>,
}

impl Default for TemplateLibrary {
    fn default() -> Self {
        let mut templates = HashMap::new();

        // Built-in templates
        templates.insert(
            "default".to_string(),
            "You are a helpful AI assistant.".to_string(),
        );
        templates.insert(
            "trans_en".to_string(),
            "You are a professional translator. Translate the user input to English. Maintain the original tone and style.".to_string(),
        );
        templates.insert(
            "trans_zh".to_string(),
            "你是一位专业的翻译。将用户输入翻译成中文。保持原文的语气和风格。".to_string(),
        );
        templates.insert(
            "code_expert".to_string(),
            "You are a senior software engineer with expertise in multiple programming languages. Provide concise, production-ready code.".to_string(),
        );
        templates.insert(
            "search_assistant".to_string(),
            "You are a research assistant. Answer the user's question based on the provided search context. Cite your sources.".to_string(),
        );

        Self { templates }
    }
}

impl TemplateLibrary {
    /// Get template by ID
    pub fn get(&self, template_id: &str) -> Option<&str> {
        self.templates.get(template_id).map(|s| s.as_str())
    }

    /// Add custom template
    pub fn insert(&mut self, template_id: String, prompt: String) {
        self.templates.insert(template_id, prompt);
    }

    /// List all template IDs
    pub fn list_templates(&self) -> Vec<String> {
        self.templates.keys().cloned().collect()
    }
}

/// Main prompt assembler
///
/// This component orchestrates the transformation from raw user input
/// into a structured AgentPayload ready for AI processing.
pub struct PromptAssembler {
    /// Template library for system prompts
    templates: TemplateLibrary,

    /// Custom command mappings (e.g., "/en" -> trans_en template)
    /// This is the "user-defined shortcuts" layer
    custom_commands: HashMap<String, CustomCommand>,
}

/// Custom command definition
///
/// Users can define shortcuts like "/polite" that map to specific
/// prompt templates and configurations.
#[derive(Debug, Clone)]
pub struct CustomCommand {
    /// Regex pattern to match (e.g., "^/en")
    pub pattern: Regex,

    /// Template ID to use
    pub template_id: String,

    /// Whether to strip the matched prefix
    pub strip_prefix: bool,

    /// Optional temperature override
    pub temperature: Option<f32>,
}

impl PromptAssembler {
    /// Create new assembler with default templates
    pub fn new() -> Self {
        Self {
            templates: TemplateLibrary::default(),
            custom_commands: HashMap::new(),
        }
    }

    /// Add custom command from routing rule configuration
    ///
    /// This integrates with the existing routing system while building
    /// the new structured protocol.
    pub fn add_custom_command_from_rule(&mut self, rule: &RoutingRuleConfig) -> Result<()> {
        let pattern = Regex::new(&rule.regex).map_err(|e| {
            AetherError::invalid_config(format!("Invalid regex pattern '{}': {}", rule.regex, e))
        })?;

        // If rule has a system prompt, add it to templates
        let template_id = if let Some(ref system_prompt) = rule.system_prompt {
            // Generate template ID from pattern (sanitize for HashMap key)
            let template_id = format!(
                "custom_{}",
                rule.regex.replace(
                    ['/', '^', '$', '.', '*', '+', '?', '(', ')', '[', ']', '{', '}', '|', '\\'],
                    "_"
                )
            );

            self.templates
                .insert(template_id.clone(), system_prompt.clone());
            template_id
        } else {
            // No custom prompt, use default
            "default".to_string()
        };

        let command = CustomCommand {
            pattern,
            template_id,
            strip_prefix: rule
                .strip_prefix
                .unwrap_or_else(|| rule.regex.starts_with("^/")),
            temperature: None, // TODO: Extract from rule config if available
        };

        debug!(
            pattern = %rule.regex,
            template_id = %command.template_id,
            "Added custom command from routing rule"
        );

        self.custom_commands.insert(rule.regex.clone(), command);

        Ok(())
    }

    /// Parse user input and create initial payload
    ///
    /// This is step 1 of the pipeline: detect intent and create structured payload.
    ///
    /// # Arguments
    ///
    /// * `input` - Raw user input (clipboard content)
    /// * `app_context` - Optional application context
    ///
    /// # Returns
    ///
    /// AgentPayload with detected intent and default configuration
    pub fn parse_intent(&self, input: &str, app_context: Option<AppContext>) -> AgentPayload {
        // Check custom commands first (user-defined takes priority)
        for (pattern_str, command) in &self.custom_commands {
            if command.pattern.is_match(input) {
                info!(
                    pattern = %pattern_str,
                    template = %command.template_id,
                    "Matched custom command"
                );

                // Strip prefix if configured
                let processed_input = if command.strip_prefix {
                    self.strip_prefix(&command.pattern, input)
                } else {
                    input.to_string()
                };

                let intent = AgentIntent::CustomTransform {
                    prompt_template_id: command.template_id.clone(),
                };

                let mut payload = AgentPayload::new(processed_input, intent);
                payload.config.system_template_id = command.template_id.clone();

                if let Some(temp) = command.temperature {
                    payload.config.temperature = temp;
                }

                if let Some(app_ctx) = app_context {
                    payload.context.app_context = Some(app_ctx);
                }

                return payload;
            }
        }

        // No custom command matched, use default chat intent
        let mut payload = AgentPayload::new(input.to_string(), AgentIntent::GeneralChat);

        if let Some(app_ctx) = app_context {
            payload.context.app_context = Some(app_ctx);
        }

        payload
    }

    /// Strip prefix from input based on regex pattern
    fn strip_prefix(&self, pattern: &Regex, input: &str) -> String {
        if let Some(mat) = pattern.find(input) {
            input[mat.end()..].trim_start().to_string()
        } else {
            input.to_string()
        }
    }

    /// Augment payload with memory context
    ///
    /// This is a middleware step that enriches the payload with retrieved memories.
    pub fn augment_with_memory(&self, payload: &mut AgentPayload, memory_snippets: Vec<String>) {
        if !memory_snippets.is_empty() {
            debug!(
                count = memory_snippets.len(),
                "Augmenting payload with memory snippets"
            );
            payload.context.memory_snippets = Some(memory_snippets);
        }
    }

    /// Augment payload with search results
    ///
    /// This is a middleware step for web search integration (future feature).
    pub fn augment_with_search(
        &self,
        payload: &mut AgentPayload,
        results: Vec<super::SearchResult>,
    ) {
        if !results.is_empty() {
            debug!(
                count = results.len(),
                "Augmenting payload with search results"
            );
            payload.context.search_results = Some(results);
        }
    }

    /// Get final system prompt for a payload
    ///
    /// This renders the complete system prompt with all context augmentations.
    pub fn get_final_system_prompt(&self, payload: &AgentPayload) -> String {
        // Get base template
        let base_prompt = self
            .templates
            .get(&payload.config.system_template_id)
            .unwrap_or("You are a helpful AI assistant.");

        // Augment with context
        payload.augment_system_prompt(base_prompt.to_string())
    }

    /// Get template library reference
    pub fn templates(&self) -> &TemplateLibrary {
        &self.templates
    }

    /// Get mutable template library reference
    pub fn templates_mut(&mut self) -> &mut TemplateLibrary {
        &mut self.templates
    }
}

impl Default for PromptAssembler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_library() {
        let lib = TemplateLibrary::default();

        assert!(lib.get("default").is_some());
        assert!(lib.get("trans_en").is_some());
        assert!(lib.get("nonexistent").is_none());
    }

    #[test]
    fn test_custom_command_from_rule() {
        let mut assembler = PromptAssembler::new();

        let rule =
            RoutingRuleConfig::command("^/en", "openai", Some("Custom translation prompt"));

        let result = assembler.add_custom_command_from_rule(&rule);
        assert!(result.is_ok());

        // Verify command was added
        assert_eq!(assembler.custom_commands.len(), 1);
    }

    #[test]
    fn test_parse_intent_custom_command() {
        let mut assembler = PromptAssembler::new();

        // Add custom /en command
        let rule = RoutingRuleConfig::command("^/en", "openai", Some("Translate to English"));

        assembler.add_custom_command_from_rule(&rule).unwrap();

        // Parse input with /en prefix
        let payload = assembler.parse_intent("/en 你好世界", None);

        // Should detect custom transform intent
        match payload.meta.intent {
            AgentIntent::CustomTransform { .. } => {}
            _ => panic!("Expected CustomTransform intent"),
        }

        // Should strip prefix
        assert_eq!(payload.user_input, "你好世界");
    }

    #[test]
    fn test_parse_intent_default() {
        let assembler = PromptAssembler::new();

        let payload = assembler.parse_intent("Hello world", None);

        // Should use general chat intent
        assert_eq!(payload.meta.intent, AgentIntent::GeneralChat);
        assert_eq!(payload.user_input, "Hello world");
    }

    #[test]
    fn test_memory_augmentation() {
        let assembler = PromptAssembler::new();

        let mut payload =
            AgentPayload::new("What did we discuss?".to_string(), AgentIntent::GeneralChat);

        let memories = vec![
            "Previous topic: Rust programming".to_string(),
            "User prefers concise answers".to_string(),
        ];

        assembler.augment_with_memory(&mut payload, memories);

        assert!(payload.context.memory_snippets.is_some());
        assert_eq!(payload.context.memory_snippets.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_final_prompt_with_context() {
        let assembler = PromptAssembler::new();

        let mut payload = AgentPayload::new(
            "Translate this".to_string(),
            AgentIntent::Translation {
                target_lang: "English".to_string(),
            },
        );

        payload.config.system_template_id = "trans_en".to_string();

        // Add memory context
        assembler.augment_with_memory(
            &mut payload,
            vec!["User prefers formal language".to_string()],
        );

        let final_prompt = assembler.get_final_system_prompt(&payload);

        assert!(final_prompt.contains("professional translator"));
        assert!(final_prompt.contains("Relevant Context from Memory"));
        assert!(final_prompt.contains("formal language"));
    }
}
