//! hooks.json parser
//!
//! Parses Claude Code compatible hooks.json files and maps events to Aether's EventBus.

use std::path::Path;

use regex::Regex;

use crate::event::EventType;
use crate::plugins::error::{PluginError, PluginResult};
use crate::plugins::types::{HookEvent, PluginHooksConfig};

/// Hook loader for parsing hooks.json files
#[derive(Debug, Default)]
pub struct HookLoader;

impl HookLoader {
    /// Create a new hook loader
    pub fn new() -> Self {
        Self
    }

    /// Load hooks from a hooks.json file
    pub fn load(&self, path: &Path) -> PluginResult<PluginHooksConfig> {
        let content = std::fs::read_to_string(path)?;
        let config: PluginHooksConfig =
            serde_json::from_str(&content).map_err(|e| PluginError::HooksParseError {
                path: path.to_path_buf(),
                source: e,
            })?;

        // Validate matchers
        for (event, matchers) in &config.hooks {
            for matcher in matchers {
                if let Some(ref pattern) = matcher.matcher {
                    if Regex::new(pattern).is_err() {
                        tracing::warn!(
                            "Invalid regex pattern '{}' for event {:?} in {:?}",
                            pattern,
                            event,
                            path
                        );
                    }
                }
            }
        }

        Ok(config)
    }
}

/// Map Claude Code hook events to Aether EventTypes
impl HookEvent {
    /// Convert to Aether EventType(s)
    ///
    /// Some Claude Code events may map to multiple Aether events.
    pub fn to_aether_events(&self) -> Vec<EventType> {
        match self {
            HookEvent::PreToolUse => vec![EventType::ToolCallRequested],
            HookEvent::PostToolUse => vec![EventType::ToolCallCompleted],
            HookEvent::PostToolUseFailure => vec![EventType::ToolCallFailed],
            HookEvent::SessionStart => vec![EventType::SessionCreated],
            HookEvent::SessionEnd => vec![EventType::LoopStop],
            HookEvent::UserPromptSubmit => vec![EventType::InputReceived],
            HookEvent::SubagentStart => vec![EventType::SubAgentStarted],
            HookEvent::SubagentStop => vec![EventType::SubAgentCompleted],
            HookEvent::Stop => vec![EventType::LoopStop],
            HookEvent::PreCompact => vec![EventType::SessionCompacted], // Closest match
            // These events don't have direct mappings yet
            HookEvent::PermissionRequest => vec![],
            HookEvent::Notification => vec![],
            HookEvent::Setup => vec![],
        }
    }

    /// Get the event name for logging
    pub fn name(&self) -> &'static str {
        match self {
            HookEvent::PreToolUse => "PreToolUse",
            HookEvent::PostToolUse => "PostToolUse",
            HookEvent::PostToolUseFailure => "PostToolUseFailure",
            HookEvent::PermissionRequest => "PermissionRequest",
            HookEvent::UserPromptSubmit => "UserPromptSubmit",
            HookEvent::Notification => "Notification",
            HookEvent::Stop => "Stop",
            HookEvent::SubagentStart => "SubagentStart",
            HookEvent::SubagentStop => "SubagentStop",
            HookEvent::Setup => "Setup",
            HookEvent::SessionStart => "SessionStart",
            HookEvent::SessionEnd => "SessionEnd",
            HookEvent::PreCompact => "PreCompact",
        }
    }
}

/// Check if a value matches a hook matcher pattern
pub fn matches_pattern(pattern: &Option<String>, value: &str) -> bool {
    match pattern {
        None => true, // No pattern means match everything
        Some(pat) => {
            if let Ok(re) = Regex::new(pat) {
                re.is_match(value)
            } else {
                // Invalid regex, don't match
                false
            }
        }
    }
}

/// Substitute variables in hook commands
///
/// Supported variables:
/// - ${CLAUDE_PLUGIN_ROOT} - Plugin root directory
/// - $ARGUMENTS - Event context/arguments
/// - $FILE - File path (if applicable)
pub fn substitute_variables(
    input: &str,
    plugin_root: &Path,
    arguments: Option<&str>,
    file: Option<&str>,
) -> String {
    let mut result = input.to_string();

    // Substitute plugin root
    result = result.replace(
        "${CLAUDE_PLUGIN_ROOT}",
        &plugin_root.to_string_lossy(),
    );

    // Substitute arguments
    if let Some(args) = arguments {
        result = result.replace("$ARGUMENTS", args);
    }

    // Substitute file
    if let Some(f) = file {
        result = result.replace("$FILE", f);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::types::HookAction;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_hooks_json() {
        let temp = TempDir::new().unwrap();
        let hooks_path = temp.path().join("hooks.json");

        let content = r#"{
            "hooks": {
                "PostToolUse": [
                    {
                        "matcher": "Write|Edit",
                        "hooks": [
                            {
                                "type": "command",
                                "command": "echo 'File changed'"
                            }
                        ]
                    }
                ],
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "type": "prompt",
                                "prompt": "Session started"
                            }
                        ]
                    }
                ]
            }
        }"#;

        fs::write(&hooks_path, content).unwrap();

        let loader = HookLoader::new();
        let config = loader.load(&hooks_path).unwrap();

        assert!(config.hooks.contains_key(&HookEvent::PostToolUse));
        assert!(config.hooks.contains_key(&HookEvent::SessionStart));

        let post_tool_hooks = &config.hooks[&HookEvent::PostToolUse];
        assert_eq!(post_tool_hooks.len(), 1);
        assert_eq!(
            post_tool_hooks[0].matcher,
            Some("Write|Edit".to_string())
        );
    }

    #[test]
    fn test_hook_event_mapping() {
        assert_eq!(
            HookEvent::PreToolUse.to_aether_events(),
            vec![EventType::ToolCallRequested]
        );
        assert_eq!(
            HookEvent::PostToolUse.to_aether_events(),
            vec![EventType::ToolCallCompleted]
        );
        assert_eq!(
            HookEvent::SessionStart.to_aether_events(),
            vec![EventType::SessionCreated]
        );
    }

    #[test]
    fn test_matches_pattern() {
        // No pattern matches everything
        assert!(matches_pattern(&None, "Write"));
        assert!(matches_pattern(&None, "anything"));

        // Regex patterns
        let pattern = Some("Write|Edit".to_string());
        assert!(matches_pattern(&pattern, "Write"));
        assert!(matches_pattern(&pattern, "Edit"));
        assert!(!matches_pattern(&pattern, "Read"));

        // Invalid regex
        let invalid = Some("[invalid".to_string());
        assert!(!matches_pattern(&invalid, "anything"));
    }

    #[test]
    fn test_substitute_variables() {
        let plugin_root = Path::new("/home/user/.aether/plugins/my-plugin");

        let result = substitute_variables(
            "${CLAUDE_PLUGIN_ROOT}/scripts/format.sh $FILE $ARGUMENTS",
            plugin_root,
            Some("--check"),
            Some("/tmp/test.rs"),
        );

        assert_eq!(
            result,
            "/home/user/.aether/plugins/my-plugin/scripts/format.sh /tmp/test.rs --check"
        );
    }

    #[test]
    fn test_parse_hook_actions() {
        let json = r#"{
            "hooks": {
                "PostToolUse": [
                    {
                        "hooks": [
                            {"type": "command", "command": "echo test"},
                            {"type": "prompt", "prompt": "Analyze this"},
                            {"type": "agent", "agent": "reviewer"}
                        ]
                    }
                ]
            }
        }"#;

        let config: PluginHooksConfig = serde_json::from_str(json).unwrap();
        let hooks = &config.hooks[&HookEvent::PostToolUse][0].hooks;

        assert_eq!(hooks.len(), 3);
        assert!(matches!(hooks[0], HookAction::Command { .. }));
        assert!(matches!(hooks[1], HookAction::Prompt { .. }));
        assert!(matches!(hooks[2], HookAction::Agent { .. }));
    }
}
