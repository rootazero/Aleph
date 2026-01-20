//! Conversational mode prompt - for dialogue and Q&A.
//!
//! This prompt is used when `ExecutionIntentDecider` determines the user wants
//! a conversation or explanation, not task execution. No tools are provided
//! in this mode.

/// Conversational prompt configuration
#[derive(Debug, Clone)]
pub struct ConversationalPrompt {
    /// Custom persona (optional)
    persona: Option<String>,
    /// Language preference
    language: Option<String>,
}

impl ConversationalPrompt {
    /// Create a new conversational prompt
    pub fn new() -> Self {
        Self {
            persona: None,
            language: None,
        }
    }

    /// Set a custom persona
    pub fn with_persona(mut self, persona: impl Into<String>) -> Self {
        self.persona = Some(persona.into());
        self
    }

    /// Set language preference
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = Some(language.into());
        self
    }

    /// Generate the system prompt
    pub fn generate(&self) -> String {
        let persona = self
            .persona
            .as_deref()
            .unwrap_or(DEFAULT_CONVERSATIONAL_PERSONA);

        let language_hint = self
            .language
            .as_ref()
            .map(|lang| format!("\n- Respond in {}", lang))
            .unwrap_or_default();

        format!(
            r#"# Role
{persona}

# Guidelines
- Be concise and direct
- Use examples when helpful
- Ask for clarification if the question is ambiguous{language_hint}"#
        )
    }
}

impl Default for ConversationalPrompt {
    fn default() -> Self {
        Self::new()
    }
}

const DEFAULT_CONVERSATIONAL_PERSONA: &str =
    "You are a helpful assistant. Answer questions, explain concepts, and have conversations.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conversational_prompt_basic() {
        let prompt = ConversationalPrompt::new();
        let text = prompt.generate();

        assert!(text.contains("# Role"));
        assert!(text.contains("helpful assistant"));
        assert!(text.contains("# Guidelines"));
    }

    #[test]
    fn test_conversational_prompt_no_tool_mentions() {
        let prompt = ConversationalPrompt::new();
        let text = prompt.generate();

        // Should NOT mention tools or execution
        assert!(!text.contains("tool"));
        assert!(!text.contains("execute"));
        assert!(!text.contains("file_ops"));
    }

    #[test]
    fn test_conversational_prompt_custom_persona() {
        let prompt =
            ConversationalPrompt::new().with_persona("You are a friendly coding tutor.");
        let text = prompt.generate();

        assert!(text.contains("coding tutor"));
    }

    #[test]
    fn test_conversational_prompt_with_language() {
        let prompt = ConversationalPrompt::new().with_language("Chinese");
        let text = prompt.generate();

        assert!(text.contains("Respond in Chinese"));
    }

    #[test]
    fn test_prompt_is_minimal() {
        let prompt = ConversationalPrompt::new();
        let text = prompt.generate();

        // Should be very short
        let estimated_tokens = text.len() / 4;
        assert!(
            estimated_tokens < 100,
            "Prompt too long: ~{} tokens",
            estimated_tokens
        );
    }
}
