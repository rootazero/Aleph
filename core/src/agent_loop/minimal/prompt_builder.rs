//! MinimalPromptBuilder — assembles system prompt from sections.

use crate::thinker::soul::SoulManifest;

// =============================================================================
// ToolInfo
// =============================================================================

/// Lightweight tool info for prompt building.
#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
}

// =============================================================================
// MinimalPromptBuilder
// =============================================================================

const SECTION_SEPARATOR: &str = "\n\n---\n\n";

const DEFAULT_IDENTITY: &str = "You are a helpful personal AI assistant.";

const BASE_BEHAVIOR: &str = "\
- Use available tools to gather information and take actions when needed.\n\
- Continue working until the user's request is fully resolved.\n\
- When a tool call fails, analyze the error and retry with corrected parameters if possible.\n\
- Provide concise summaries of actions taken and results obtained.";

/// Builds the system prompt by assembling sections.
pub struct MinimalPromptBuilder {
    soul_identity: Option<String>,
    soul_tone: Option<String>,
    soul_directives: Vec<String>,
    capability_rules: Option<String>,
    custom_instructions: Option<String>,
}

impl MinimalPromptBuilder {
    /// Build a prompt builder pre-configured from a SoulManifest.
    pub fn from_soul(soul: &SoulManifest) -> Self {
        let mut builder = Self::new();

        // Identity
        if !soul.identity.is_empty() {
            builder = builder.with_soul_identity(&soul.identity);
        }

        // Voice tone
        if !soul.voice.tone.is_empty() {
            builder = builder.with_soul_tone(&soul.voice.tone);
        }

        // Directives (both positive directives and anti-patterns)
        for directive in &soul.directives {
            builder = builder.with_soul_directive(directive);
        }
        for anti in &soul.anti_patterns {
            builder = builder.with_soul_directive(&format!("NEVER: {anti}"));
        }

        // Expertise as directives
        if !soul.expertise.is_empty() {
            let expertise_str =
                format!("Your areas of expertise: {}", soul.expertise.join(", "));
            builder = builder.with_soul_directive(&expertise_str);
        }

        // Addendum as custom instructions
        if let Some(addendum) = &soul.addendum {
            builder = builder.with_custom_instructions(addendum);
        }

        builder
    }

    /// Create an empty builder.
    pub fn new() -> Self {
        Self {
            soul_identity: None,
            soul_tone: None,
            soul_directives: Vec::new(),
            capability_rules: None,
            custom_instructions: None,
        }
    }

    /// Set the soul identity (who the assistant is).
    pub fn with_soul_identity(mut self, identity: &str) -> Self {
        self.soul_identity = Some(identity.to_string());
        self
    }

    /// Set the communication tone/style.
    pub fn with_soul_tone(mut self, tone: &str) -> Self {
        self.soul_tone = Some(tone.to_string());
        self
    }

    /// Add a directive (accumulated as a bullet list).
    pub fn with_soul_directive(mut self, directive: &str) -> Self {
        self.soul_directives.push(directive.to_string());
        self
    }

    /// Set capability/tool-usage rules.
    pub fn with_capability_rules(mut self, rules: &str) -> Self {
        self.capability_rules = Some(rules.to_string());
        self
    }

    /// Set custom instructions from the user.
    pub fn with_custom_instructions(mut self, instructions: &str) -> Self {
        self.custom_instructions = Some(instructions.to_string());
        self
    }

    /// Assemble the full system prompt from all configured sections.
    ///
    /// Sections are separated by `\n\n---\n\n` and each has a `# Header`.
    /// Only non-empty/configured sections are included.
    pub fn build(&self, tools: &[ToolInfo], memory_context: Option<&str>) -> String {
        let mut sections: Vec<String> = Vec::new();

        // 1. Identity
        let identity = self
            .soul_identity
            .as_deref()
            .unwrap_or(DEFAULT_IDENTITY);
        sections.push(format!("# Identity\n\n{}", identity));

        // 2. Communication Style
        if let Some(tone) = &self.soul_tone {
            sections.push(format!("# Communication Style\n\n{}", tone));
        }

        // 3. Directives
        if !self.soul_directives.is_empty() {
            let bullets: String = self
                .soul_directives
                .iter()
                .map(|d| format!("- {}", d))
                .collect::<Vec<_>>()
                .join("\n");
            sections.push(format!("# Directives\n\n{}", bullets));
        }

        // 4. Tool Usage Rules
        if let Some(rules) = &self.capability_rules {
            sections.push(format!("# Tool Usage Rules\n\n{}", rules));
        }

        // 5. Available Tools
        if !tools.is_empty() {
            let tool_list: String = tools
                .iter()
                .map(|t| format!("- **{}**: {}", t.name, t.description))
                .collect::<Vec<_>>()
                .join("\n");
            sections.push(format!("# Available Tools\n\n{}", tool_list));
        }

        // 6. Context from Memory
        if let Some(ctx) = memory_context {
            sections.push(format!("# Context from Memory\n\n{}", ctx));
        }

        // 7. Additional Instructions
        if let Some(instructions) = &self.custom_instructions {
            sections.push(format!("# Additional Instructions\n\n{}", instructions));
        }

        // 8. Behavior
        sections.push(format!("# Behavior\n\n{}", BASE_BEHAVIOR));

        sections.join(SECTION_SEPARATOR)
    }
}

impl Default for MinimalPromptBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::soul::{SoulManifest, SoulVoice};

    #[test]
    fn test_build_includes_soul() {
        let prompt = MinimalPromptBuilder::new()
            .with_soul_identity("I am Aleph, your personal AI.")
            .with_soul_tone("Speak concisely and warmly.")
            .build(&[], None);

        assert!(prompt.contains("I am Aleph, your personal AI."));
        assert!(prompt.contains("Speak concisely and warmly."));
        assert!(prompt.contains("# Identity"));
        assert!(prompt.contains("# Communication Style"));
    }

    #[test]
    fn test_build_includes_tool_rules() {
        let prompt = MinimalPromptBuilder::new()
            .with_capability_rules("Always confirm before destructive actions.")
            .build(&[], None);

        assert!(prompt.contains("# Tool Usage Rules"));
        assert!(prompt.contains("Always confirm before destructive actions."));
    }

    #[test]
    fn test_build_includes_memory_context() {
        let prompt = MinimalPromptBuilder::new()
            .build(&[], Some("User prefers dark mode and short replies."));

        assert!(prompt.contains("# Context from Memory"));
        assert!(prompt.contains("User prefers dark mode and short replies."));
    }

    #[test]
    fn test_build_includes_tool_descriptions() {
        let tools = vec![
            ToolInfo {
                name: "web_search".to_string(),
                description: "Search the web for information.".to_string(),
            },
            ToolInfo {
                name: "file_read".to_string(),
                description: "Read a file from disk.".to_string(),
            },
        ];

        let prompt = MinimalPromptBuilder::new().build(&tools, None);

        assert!(prompt.contains("# Available Tools"));
        assert!(prompt.contains("- **web_search**: Search the web for information."));
        assert!(prompt.contains("- **file_read**: Read a file from disk."));
    }

    #[test]
    fn test_build_empty_is_valid() {
        let prompt = MinimalPromptBuilder::new().build(&[], None);

        assert!(!prompt.is_empty());
        assert!(prompt.contains("assistant"));
        assert!(prompt.contains("# Identity"));
        assert!(prompt.contains("# Behavior"));
    }

    #[test]
    fn test_from_soul_identity() {
        let soul = SoulManifest {
            identity: "I am Aleph, a personal AI companion.".to_string(),
            voice: SoulVoice {
                tone: "warm and concise".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };

        let prompt = MinimalPromptBuilder::from_soul(&soul).build(&[], None);

        assert!(prompt.contains("I am Aleph, a personal AI companion."));
        assert!(prompt.contains("warm and concise"));
    }

    #[test]
    fn test_from_soul_directives() {
        let soul = SoulManifest {
            directives: vec![
                "Always explain reasoning".to_string(),
                "Be precise".to_string(),
            ],
            anti_patterns: vec!["Making things up".to_string()],
            ..Default::default()
        };

        let prompt = MinimalPromptBuilder::from_soul(&soul).build(&[], None);

        assert!(prompt.contains("Always explain reasoning"));
        assert!(prompt.contains("Be precise"));
        assert!(prompt.contains("NEVER: Making things up"));
    }

    #[test]
    fn test_from_soul_addendum() {
        let soul = SoulManifest {
            addendum: Some("Remember the user prefers dark mode.".to_string()),
            ..Default::default()
        };

        let prompt = MinimalPromptBuilder::from_soul(&soul).build(&[], None);

        assert!(prompt.contains("# Additional Instructions"));
        assert!(prompt.contains("Remember the user prefers dark mode."));
    }

    #[test]
    fn test_from_soul_empty() {
        let soul = SoulManifest::default();
        let prompt = MinimalPromptBuilder::from_soul(&soul).build(&[], None);

        // Should still produce a valid prompt with defaults
        assert!(!prompt.is_empty());
        assert!(prompt.contains("# Identity"));
        assert!(prompt.contains("# Behavior"));
    }

    #[test]
    fn test_custom_instructions() {
        let prompt = MinimalPromptBuilder::new()
            .with_custom_instructions("Reply only in haiku format.")
            .build(&[], None);

        assert!(prompt.contains("# Additional Instructions"));
        assert!(prompt.contains("Reply only in haiku format."));
    }
}
