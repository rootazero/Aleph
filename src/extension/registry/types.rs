//! Plugin registration type definitions
//!
//! This module defines the registration types used by the plugin API:
//! Tool, Hook, Service, Command, Skill, Agent.
//!
//! Plus diagnostics support for plugin health reporting.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::PathBuf;

/// JSON Schema type alias for tool parameter definitions
pub type JsonSchema = JsonValue;

/// The one derivation of `<plugin>:<component>` in the extension subsystem.
///
/// Both the registry's storage key (`PluginRegistry::register_skill` /
/// `register_agent`) and the name the model is shown
/// (`SkillRegistration::qualified_name`) must be the *same* string, or a
/// component the model can see becomes a component nobody can address.
///
/// # Why this is a function and not two `format!`s
///
/// Until 2026-08-19 there were two answers. The registry keyed on
/// `plugin_id`; `qualified_name()` read a separate `plugin_name` field that
/// **no production code path ever assigned** — every `SkillRegistration` /
/// `AgentRegistration` construction site in `manifest/parsers.rs`,
/// `manifest/adapters/*` and `registrar/api.rs` sets `plugin_id` and leaves
/// `plugin_name` at its `Option::None` default, so the only assignments in the
/// tree were test fixtures. Two consequences, both silent:
///
/// * `qualified_name()` returned a bare name forever, so the namespacing
///   `PLUGIN_SYSTEM.md` documents never happened on the surface the model
///   reads; and
/// * `tool_catalog_init.rs` filtered plugin commands on
///   `plugin_name.is_some()`, so **no plugin command was ever registered into
///   the dispatch registry** — `commands/` is one of the five Claude Code
///   component kinds, and it reached no surface at all.
///
/// The field is gone; this function is the single source. An empty
/// `plugin_id` means "not from a plugin" and yields the bare name.
#[must_use]
pub fn namespaced_component_key(plugin_id: &str, name: &str) -> String {
    if plugin_id.is_empty() {
        name.to_string()
    } else {
        format!("{plugin_id}:{name}")
    }
}

// ============================================================================
// P0 Core Registration Types
// ============================================================================

/// Tool registration for plugins to expose callable tools to the agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRegistration {
    /// Unique tool name (must be unique across all plugins)
    pub name: String,
    /// Human-readable description of what the tool does
    pub description: String,
    /// JSON Schema defining the tool's input parameters
    pub parameters: JsonSchema,
    /// Handler function name within the plugin
    pub handler: String,
    /// ID of the plugin that registered this tool
    pub plugin_id: String,
}

/// Hook registration for plugins to intercept system events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookRegistration {
    /// The event that triggers this hook
    pub event: crate::extension::types::HookEvent,
    /// Execution priority (lower = earlier, default 0)
    pub priority: i32,
    /// Handler function name within the plugin. Used for WASM-runtime
    /// dispatch when `actions` is empty; display-only otherwise.
    pub handler: String,
    /// Optional human-readable name for the hook
    pub name: Option<String>,
    /// Optional description of what the hook does
    pub description: Option<String>,
    /// ID of the plugin that registered this hook
    pub plugin_id: String,
    /// Execution kind (`interceptor` | `observer`). `None` picks the
    /// per-event default (blocking-capable events → interceptor). Lets a
    /// runtime plugin register a hook that can actually block / rewrite —
    /// previously every registry hook was hard-wired to Observer, so a
    /// plugin's block/deny output was silently ignored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<crate::extension::types::HookKind>,
    /// Optional regex matched against `tool_name` for tool-based events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    /// Concrete actions to execute when the event fires. When non-empty
    /// these are dispatched directly (shell command / prompt / http…, with
    /// the consent gate applying to command + http actions). When empty,
    /// dispatch falls back to invoking the WASM export named by `handler`.
    /// Plugin-shipped `hooks.json` shell hooks flow through here — they were
    /// previously mangled into a fake WASM handler string ("cmd1; cmd2")
    /// that could never execute.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<crate::extension::types::HookAction>,
    /// Plugin root directory for `${PLUGIN_ROOT}` substitution and the
    /// default working directory of command actions. `None` for runtime
    /// (WASM) registrations, which execute no filesystem-relative commands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_root: Option<std::path::PathBuf>,
    /// Per-hook timeout in seconds (applies to command/http actions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

// ============================================================================
// P2 Useful Registration Types
// ============================================================================

/// Background service registration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceRegistration {
    /// Unique service identifier
    pub id: String,
    /// Display name for the service
    pub name: String,
    /// Handler function to start the service
    pub start_handler: String,
    /// Handler function to stop the service
    pub stop_handler: String,
    /// ID of the plugin that registered this service
    pub plugin_id: String,
    /// Start automatically when the plugin is loaded
    #[serde(default = "default_service_auto_start")]
    pub auto_start: bool,
}

const fn default_service_auto_start() -> bool {
    true
}

// ============================================================================
// P3 Optional Registration Types
// ============================================================================

// ============================================================================
// Diagnostics
// ============================================================================

/// Severity level for plugin diagnostics
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticLevel {
    /// Warning - plugin may have issues but can continue
    Warn,
    /// Error - plugin has critical issues
    Error,
}

/// Diagnostic message from a plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDiagnostic {
    /// Severity level
    pub level: DiagnosticLevel,
    /// Human-readable diagnostic message
    pub message: String,
    /// ID of the plugin that generated this diagnostic
    pub plugin_id: Option<String>,
    /// Source location or component that generated the diagnostic
    pub source: Option<String>,
}

// ============================================================================
// Skill & Agent Registration Types (added for capability-driven architecture)
// ============================================================================

/// Skill registration for plugins to expose prompt-based skills.
///
/// This is the unified type for both plugin-registered skills (via `CapabilityApi`)
/// and filesystem-discovered skills (via legacy loader). `ExtensionSkill` is a type
/// alias for this struct.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillRegistration {
    /// Unique skill name
    pub name: String,

    /// Skill type (command vs skill)
    #[serde(default)]
    pub skill_type: crate::extension::types::SkillType,

    /// Human-readable description of the skill
    pub description: String,

    /// Skill prompt content (Markdown)
    pub content: String,

    /// Whether to disable automatic model invocation
    #[serde(default)]
    pub disable_model_invocation: bool,

    /// Prompt injection scope
    #[serde(default)]
    pub scope: crate::extension::types::PromptScope,

    /// Bound tool name (for Tool scope)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_tool: Option<String>,

    /// Trigger phrases that activate this skill
    #[serde(default)]
    pub triggers: Vec<String>,

    /// Optional category for grouping
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,

    /// Source path on disk (empty for runtime-registered skills)
    #[serde(default)]
    pub source_path: PathBuf,

    /// Discovery source
    #[serde(default)]
    pub source: crate::discovery::DiscoverySource,

    /// ID of the plugin that registered this skill (plugin API path)
    #[serde(default)]
    pub plugin_id: String,
}

impl SkillRegistration {
    /// Get the fully qualified name (plugin:skill or just skill)
    #[must_use]
    pub fn qualified_name(&self) -> String {
        namespaced_component_key(&self.plugin_id, &self.name)
    }

    /// Check if this skill can be auto-invoked by the model
    #[must_use]
    pub fn is_auto_invocable(&self) -> bool {
        !self.disable_model_invocation
            && self.skill_type == crate::extension::types::SkillType::Skill
    }

    /// Substitute $ARGUMENTS placeholder
    #[must_use]
    pub fn with_arguments(&self, arguments: &str) -> String {
        self.content.replace("$ARGUMENTS", arguments)
    }

    /// Get the base directory for this skill (for file references)
    #[must_use]
    pub fn base_dir(&self) -> PathBuf {
        self.source_path
            .parent()
            .map_or_else(|| PathBuf::from("."), |p| p.to_path_buf())
    }
}

/// Agent registration for plugins to expose agent definitions.
///
/// This is the unified type for both plugin-registered agents (via `CapabilityApi`)
/// and filesystem-discovered agents (via legacy loader). `ExtensionAgent` is a type
/// alias for this struct.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentRegistration {
    /// Unique agent name
    pub name: String,

    /// Agent mode
    #[serde(default)]
    pub mode: crate::extension::types::AgentMode,

    /// Human-readable description of the agent
    #[serde(default)]
    pub description: Option<String>,

    /// Whether to hide from UI
    #[serde(default)]
    pub hidden: bool,

    /// UI color (hex format)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,

    /// Optional model override
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Temperature
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    /// Top P
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,

    /// Maximum iteration steps
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<u32>,

    /// Tool permissions
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<HashMap<String, bool>>,

    /// Permission rules
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<HashMap<String, crate::extension::types::PermissionRule>>,

    /// Provider-specific options
    #[serde(default)]
    pub options: HashMap<String, serde_json::Value>,

    /// Agent system prompt content (markdown body)
    pub content: String,

    /// Source path on disk (empty for runtime-registered agents)
    #[serde(default)]
    pub source_path: PathBuf,

    /// Discovery source
    #[serde(default)]
    pub source: crate::discovery::DiscoverySource,

    /// ID of the plugin that registered this agent (plugin API path)
    #[serde(default)]
    pub plugin_id: String,
}

impl AgentRegistration {
    /// Get the fully qualified name
    #[must_use]
    pub fn qualified_name(&self) -> String {
        namespaced_component_key(&self.plugin_id, &self.name)
    }

    /// Check if agent is a primary agent
    #[must_use]
    pub const fn is_primary(&self) -> bool {
        matches!(
            self.mode,
            crate::extension::types::AgentMode::Primary | crate::extension::types::AgentMode::All
        )
    }

    /// Check if agent can be used as a sub-agent
    #[must_use]
    pub const fn is_subagent(&self) -> bool {
        matches!(
            self.mode,
            crate::extension::types::AgentMode::Subagent | crate::extension::types::AgentMode::All
        )
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_registration() {
        let tool = ToolRegistration {
            name: "my_tool".to_string(),
            description: "A test tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            handler: "handle_my_tool".to_string(),
            plugin_id: "test-plugin".to_string(),
        };
        assert_eq!(tool.name, "my_tool");
        assert_eq!(tool.plugin_id, "test-plugin");
    }

    #[test]
    fn test_hook_event_serialization() {
        use crate::extension::types::HookEvent;

        let event = HookEvent::BeforeToolCall;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, "\"before_tool_call\"");

        let deserialized: HookEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, HookEvent::BeforeToolCall);
    }

    #[test]
    fn test_subagent_hook_events_wire_contract() {
        use crate::extension::types::HookEvent;

        // Canonical wire form is snake_case (plugin JSON-RPC surface).
        assert_eq!(
            serde_json::to_string(&HookEvent::SubagentStart).unwrap(),
            "\"subagent_start\""
        );
        assert_eq!(
            serde_json::to_string(&HookEvent::SubagentStop).unwrap(),
            "\"subagent_stop\""
        );

        // PascalCase aliases stay accepted for codex-style hooks.json configs.
        let start: HookEvent = serde_json::from_str("\"SubagentStart\"").unwrap();
        assert_eq!(start, HookEvent::SubagentStart);
        let stop: HookEvent = serde_json::from_str("\"SubagentStop\"").unwrap();
        assert_eq!(stop, HookEvent::SubagentStop);
    }

    #[test]
    fn test_all_hook_events_serialize() {
        use crate::extension::types::HookEvent;

        let events = [
            HookEvent::BeforeAgentStart,
            HookEvent::AgentEnd,
            HookEvent::BeforeToolCall,
            HookEvent::AfterToolCall,
            HookEvent::AfterToolCallFailure,
            HookEvent::ToolResultPersist,
            HookEvent::MessageReceived,
            HookEvent::MessageSending,
            HookEvent::MessageSent,
            HookEvent::SessionStart,
            HookEvent::SessionEnd,
            HookEvent::BeforeCompaction,
            HookEvent::AfterCompaction,
            HookEvent::PreApiRequest,
            HookEvent::PostApiRequest,
            HookEvent::GatewayStart,
            HookEvent::GatewayStop,
            HookEvent::Notification,
            HookEvent::PermissionRequest,
            HookEvent::UserPromptSubmit,
            HookEvent::SubagentStart,
            HookEvent::SubagentStop,
        ];

        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            let roundtrip: HookEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(roundtrip, event);
        }
    }

    #[test]
    fn test_after_tool_call_failure_event() {
        use crate::extension::types::HookEvent;

        let event = HookEvent::AfterToolCallFailure;
        let json = serde_json::to_string(&event).unwrap();
        let roundtrip: HookEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip, event);

        // Test alias
        let aliased: HookEvent = serde_json::from_str("\"PostToolUseFailure\"").unwrap();
        assert_eq!(aliased, HookEvent::AfterToolCallFailure);
    }

    #[test]
    fn test_hook_event_pascal_case_aliases() {
        use crate::extension::types::HookEvent;

        // Verify PascalCase aliases work for backward compat
        let event: HookEvent = serde_json::from_str("\"PreToolUse\"").unwrap();
        assert_eq!(event, HookEvent::BeforeToolCall);

        let event: HookEvent = serde_json::from_str("\"PostToolUse\"").unwrap();
        assert_eq!(event, HookEvent::AfterToolCall);

        let event: HookEvent = serde_json::from_str("\"PreCompact\"").unwrap();
        assert_eq!(event, HookEvent::BeforeCompaction);

        let event: HookEvent = serde_json::from_str("\"SessionStart\"").unwrap();
        assert_eq!(event, HookEvent::SessionStart);

        let event: HookEvent = serde_json::from_str("\"PreApiRequest\"").unwrap();
        assert_eq!(event, HookEvent::PreApiRequest);

        let event: HookEvent = serde_json::from_str("\"PostApiRequest\"").unwrap();
        assert_eq!(event, HookEvent::PostApiRequest);

        // snake_case form (the canonical serialization) also round-trips.
        let event: HookEvent = serde_json::from_str("\"post_api_request\"").unwrap();
        assert_eq!(event, HookEvent::PostApiRequest);
    }

    #[test]
    fn test_hook_registration() {
        use crate::extension::types::HookEvent;

        let hook = HookRegistration {
            event: HookEvent::MessageReceived,
            priority: 10,
            handler: "on_message".to_string(),
            name: Some("Message Logger".to_string()),
            description: Some("Logs all incoming messages".to_string()),
            plugin_id: "logger-plugin".to_string(),
            kind: None,
            matcher: None,
            actions: Vec::new(),
            plugin_root: None,
            timeout_secs: None,
        };
        assert_eq!(hook.priority, 10);
        assert_eq!(hook.name, Some("Message Logger".to_string()));

        // Wire-format lock: the new optional fields default when absent, so
        // pre-existing runtime (WASM) registration JSON keeps deserializing.
        let legacy_json = r#"{
            "event": "message_received",
            "priority": 0,
            "handler": "on_message",
            "name": null,
            "description": null,
            "plugin_id": "p"
        }"#;
        let legacy: HookRegistration = serde_json::from_str(legacy_json).unwrap();
        assert!(legacy.kind.is_none());
        assert!(legacy.actions.is_empty());
        assert!(legacy.plugin_root.is_none());
    }

    #[test]
    fn test_service_registration() {
        let service = ServiceRegistration {
            id: "background-worker".to_string(),
            name: "Background Worker".to_string(),
            start_handler: "start_worker".to_string(),
            stop_handler: "stop_worker".to_string(),
            plugin_id: "worker-plugin".to_string(),
            auto_start: true,
        };
        assert_ne!(service.start_handler, service.stop_handler);
    }

    #[test]
    fn test_diagnostic_level_serialization() {
        let warn = DiagnosticLevel::Warn;
        let error = DiagnosticLevel::Error;

        assert_eq!(serde_json::to_string(&warn).unwrap(), "\"warn\"");
        assert_eq!(serde_json::to_string(&error).unwrap(), "\"error\"");
    }

    #[test]
    fn test_plugin_diagnostic() {
        let diagnostic = PluginDiagnostic {
            level: DiagnosticLevel::Error,
            message: "Failed to connect to database".to_string(),
            plugin_id: Some("db-plugin".to_string()),
            source: Some("connection_pool".to_string()),
        };
        assert_eq!(diagnostic.level, DiagnosticLevel::Error);
        assert!(diagnostic.message.contains("database"));
    }

    #[test]
    fn test_skill_registration() {
        let skill = SkillRegistration {
            name: "web-search".to_string(),
            description: "Search the web".to_string(),
            content: "You are a web search assistant.".to_string(),
            triggers: vec!["search".to_string(), "find".to_string()],
            category: Some("research".to_string()),
            plugin_id: "search-plugin".to_string(),
            ..Default::default()
        };
        assert_eq!(skill.name, "web-search");
        assert_eq!(skill.triggers.len(), 2);
        assert_eq!(skill.category, Some("research".to_string()));
    }

    #[test]
    fn test_agent_registration() {
        let agent = AgentRegistration {
            name: "coder".to_string(),
            description: Some("A coding assistant agent".to_string()),
            content: "You are a coding expert.".to_string(),
            model: Some("claude-sonnet-4".to_string()),
            plugin_id: "coder-plugin".to_string(),
            ..Default::default()
        };
        assert_eq!(agent.name, "coder");
        assert_eq!(agent.model, Some("claude-sonnet-4".to_string()));
    }

    #[test]
    fn test_skill_registration_serialization() {
        let skill = SkillRegistration {
            name: "test-skill".to_string(),
            description: "A test skill".to_string(),
            content: "prompt content".to_string(),
            triggers: vec!["trigger1".to_string()],
            category: None,
            plugin_id: "test-plugin".to_string(),
            ..Default::default()
        };
        let json = serde_json::to_string(&skill).unwrap();
        let roundtrip: SkillRegistration = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip.name, "test-skill");
        assert!(roundtrip.category.is_none());
    }

    #[test]
    fn test_agent_registration_serialization() {
        let agent = AgentRegistration {
            name: "test-agent".to_string(),
            description: Some("A test agent".to_string()),
            content: "system prompt".to_string(),
            model: None,
            plugin_id: "test-plugin".to_string(),
            ..Default::default()
        };
        let json = serde_json::to_string(&agent).unwrap();
        let roundtrip: AgentRegistration = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip.name, "test-agent");
        assert!(roundtrip.model.is_none());
    }

    #[test]
    fn namespaced_key_is_bare_without_a_plugin() {
        assert_eq!(namespaced_component_key("", "review"), "review");
        assert_eq!(namespaced_component_key("acme", "review"), "acme:review");
    }

    /// Source-level census: `<plugin>:<component>` may be spelled in exactly
    /// one place.
    ///
    /// This has to be a source scan, not a runtime assertion — at runtime a
    /// second hand-rolled `format!` that happens to agree today is
    /// indistinguishable from a call to the single source, right up to the
    /// day someone changes the separator on one side. That divergence is what
    /// actually shipped: the registry keyed `plugin_id:name` while
    /// `qualified_name()` read a never-assigned `plugin_name`, so the model
    /// was shown a name that addressed nothing.
    ///
    /// Scope this guard honestly declares: it recognises a *line* that
    /// mentions `plugin_id` and also carries a `:`-joining format literal. A
    /// derivation split across two statements, or one that goes through a
    /// local variable on another line, is outside what it can see.
    #[test]
    fn joining_a_plugin_id_to_a_component_name_has_exactly_one_author() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/extension");
        let mut offenders: Vec<String> = Vec::new();
        let mut checked_files = 0usize;

        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                // This file *is* the single source; its own derivation and the
                // prose describing it are allowed to name the shape.
                if path.ends_with("registry/types.rs") {
                    continue;
                }
                let Ok(src) = std::fs::read_to_string(&path) else {
                    continue;
                };
                checked_files += 1;
                for (lineno, line) in src.lines().enumerate() {
                    let code = line.trim_start();
                    // Comments are documentation, not a second author.
                    if code.starts_with("//") {
                        continue;
                    }
                    if !code.contains("plugin_id") {
                        continue;
                    }
                    let joins = code.contains("\"{}:{}\"") || code.contains("}:{");
                    if joins {
                        offenders.push(format!(
                            "{}:{} — {}",
                            path.display(),
                            lineno + 1,
                            code.trim()
                        ));
                    }
                }
            }
        }

        assert!(
            checked_files > 10,
            "census scanned only {checked_files} files — it is not looking where it thinks it is"
        );
        assert!(
            offenders.is_empty(),
            "hand-rolled plugin namespacing outside `namespaced_component_key`:\n  {}",
            offenders.join("\n  ")
        );
    }
}
