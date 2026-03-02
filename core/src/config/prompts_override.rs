//! Prompts override types for ~/.aleph/prompts.toml
//!
//! These types represent user overrides for built-in prompt templates.
//! All fields are Option<String> so users only need to specify the prompts they want to change.
//! Missing fields are left as None and the built-in defaults are used instead.

use serde::Deserialize;
use std::path::Path;
use tracing::warn;

// =============================================================================
// Section types
// =============================================================================

/// Override for planner system prompt.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PlannerPrompts {
    /// Override the task planning system prompt
    #[serde(default)]
    pub system_prompt: Option<String>,
}

/// Override for bootstrap (first-run) prompt.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BootstrapPrompts {
    /// Override the first-contact bootstrap prompt
    #[serde(default)]
    pub prompt: Option<String>,
}

/// Override for scratchpad template.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ScratchpadPrompts {
    /// Override the scratchpad markdown template
    #[serde(default)]
    pub template: Option<String>,
}

/// Override for memory-related prompts.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MemoryPrompts {
    /// Override the memory compression prompt
    #[serde(default)]
    pub compression_prompt: Option<String>,
    /// Override the memory extraction prompt
    #[serde(default)]
    pub extraction_prompt: Option<String>,
}

/// Override for agent-level prompts.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AgentPrompts {
    /// Override the agent system prompt prefix
    #[serde(default)]
    pub system_prefix: Option<String>,
    /// Override the agent observation prompt
    #[serde(default)]
    pub observation_prompt: Option<String>,
}

// =============================================================================
// Root override struct
// =============================================================================

/// Root struct for ~/.aleph/prompts.toml
///
/// Contains user overrides for various built-in prompt templates.
/// Each section is optional — users only define the sections they want to customize.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PromptsOverride {
    /// Planner prompt overrides
    #[serde(default)]
    pub planner: Option<PlannerPrompts>,
    /// Bootstrap prompt overrides
    #[serde(default)]
    pub bootstrap: Option<BootstrapPrompts>,
    /// Scratchpad template overrides
    #[serde(default)]
    pub scratchpad: Option<ScratchpadPrompts>,
    /// Memory prompt overrides
    #[serde(default)]
    pub memory: Option<MemoryPrompts>,
    /// Agent prompt overrides
    #[serde(default)]
    pub agent: Option<AgentPrompts>,
}

// =============================================================================
// Accessor helpers
// =============================================================================

impl PromptsOverride {
    /// Get the planner system prompt override, if set.
    pub fn planner_system_prompt(&self) -> Option<&str> {
        self.planner
            .as_ref()
            .and_then(|p| p.system_prompt.as_deref())
    }

    /// Get the bootstrap prompt override, if set.
    pub fn bootstrap_prompt(&self) -> Option<&str> {
        self.bootstrap.as_ref().and_then(|b| b.prompt.as_deref())
    }

    /// Get the scratchpad template override, if set.
    pub fn scratchpad_template(&self) -> Option<&str> {
        self.scratchpad
            .as_ref()
            .and_then(|s| s.template.as_deref())
    }

    /// Get the memory compression prompt override, if set.
    pub fn memory_compression_prompt(&self) -> Option<&str> {
        self.memory
            .as_ref()
            .and_then(|m| m.compression_prompt.as_deref())
    }

    /// Get the memory extraction prompt override, if set.
    pub fn memory_extraction_prompt(&self) -> Option<&str> {
        self.memory
            .as_ref()
            .and_then(|m| m.extraction_prompt.as_deref())
    }

    /// Get the agent system prefix override, if set.
    pub fn agent_system_prefix(&self) -> Option<&str> {
        self.agent
            .as_ref()
            .and_then(|a| a.system_prefix.as_deref())
    }

    /// Get the agent observation prompt override, if set.
    pub fn agent_observation_prompt(&self) -> Option<&str> {
        self.agent
            .as_ref()
            .and_then(|a| a.observation_prompt.as_deref())
    }
}

// =============================================================================
// Loading
// =============================================================================

/// Load prompts override from a TOML file.
///
/// Returns `PromptsOverride::default()` if the file does not exist or cannot be parsed.
/// Logs warnings on parse errors.
pub fn load_prompts_override(path: &Path) -> PromptsOverride {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return PromptsOverride::default();
        }
        Err(e) => {
            warn!(
                "Failed to read prompts override file {}: {}",
                path.display(),
                e
            );
            return PromptsOverride::default();
        }
    };

    match toml::from_str(&content) {
        Ok(parsed) => parsed,
        Err(e) => {
            warn!(
                "Failed to parse prompts override file {}: {}",
                path.display(),
                e
            );
            PromptsOverride::default()
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_prompts_override() {
        let parsed: PromptsOverride = toml::from_str("").unwrap();
        assert!(parsed.planner.is_none());
        assert!(parsed.bootstrap.is_none());
        assert!(parsed.scratchpad.is_none());
        assert!(parsed.memory.is_none());
        assert!(parsed.agent.is_none());
        assert!(parsed.planner_system_prompt().is_none());
        assert!(parsed.bootstrap_prompt().is_none());
        assert!(parsed.scratchpad_template().is_none());
    }

    #[test]
    fn test_planner_prompt_parse() {
        let toml_str = r#"
[planner]
system_prompt = "You are a custom planner."
"#;
        let parsed: PromptsOverride = toml::from_str(toml_str).unwrap();

        assert_eq!(
            parsed.planner_system_prompt(),
            Some("You are a custom planner.")
        );
        // Other sections remain None
        assert!(parsed.bootstrap_prompt().is_none());
        assert!(parsed.scratchpad_template().is_none());
    }

    #[test]
    fn test_multiline_prompt() {
        let toml_str = r#"
[planner]
system_prompt = """
You are a custom planner.

## Rules
1. Be concise
2. Be accurate
"""
"#;
        let parsed: PromptsOverride = toml::from_str(toml_str).unwrap();

        let prompt = parsed.planner_system_prompt().unwrap();
        assert!(prompt.contains("You are a custom planner."));
        assert!(prompt.contains("## Rules"));
        assert!(prompt.contains("1. Be concise"));
        assert!(prompt.contains("2. Be accurate"));
    }

    #[test]
    fn test_scratchpad_template_parse() {
        let toml_str = r#"
[scratchpad]
template = """
# My Custom Scratchpad

## Current Task
[empty]

## Notes
"""
"#;
        let parsed: PromptsOverride = toml::from_str(toml_str).unwrap();

        let template = parsed.scratchpad_template().unwrap();
        assert!(template.contains("# My Custom Scratchpad"));
        assert!(template.contains("## Current Task"));
    }

    #[test]
    fn test_bootstrap_prompt_parse() {
        let toml_str = r#"
[bootstrap]
prompt = "Welcome! Let's get started."
"#;
        let parsed: PromptsOverride = toml::from_str(toml_str).unwrap();

        assert_eq!(
            parsed.bootstrap_prompt(),
            Some("Welcome! Let's get started.")
        );
    }

    #[test]
    fn test_partial_override_only_some_sections() {
        let toml_str = r#"
[planner]
system_prompt = "Custom planner"

[memory]
compression_prompt = "Compress this memory"
"#;
        let parsed: PromptsOverride = toml::from_str(toml_str).unwrap();

        // Planner is set
        assert_eq!(parsed.planner_system_prompt(), Some("Custom planner"));
        // Memory compression is set
        assert_eq!(
            parsed.memory_compression_prompt(),
            Some("Compress this memory")
        );
        // Memory extraction is NOT set (only compression was specified)
        assert!(parsed.memory_extraction_prompt().is_none());
        // Bootstrap, scratchpad, agent are NOT set
        assert!(parsed.bootstrap_prompt().is_none());
        assert!(parsed.scratchpad_template().is_none());
        assert!(parsed.agent_system_prefix().is_none());
        assert!(parsed.agent_observation_prompt().is_none());
    }

    #[test]
    fn test_load_nonexistent_prompts_file() {
        let result =
            load_prompts_override(Path::new("/tmp/does-not-exist-aleph-prompts.toml"));
        assert!(result.planner.is_none());
        assert!(result.bootstrap.is_none());
        assert!(result.scratchpad.is_none());
        assert!(result.memory.is_none());
        assert!(result.agent.is_none());
    }
}
