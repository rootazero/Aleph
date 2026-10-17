//! Hook configuration types
//!
//! Types for shell-based hooks and plugin hooks. A plugin's MCP server
//! declarations are `crate::mcp::McpManagerConfig` (parsed by
//! `extension::mcp_config`), not a type in this module.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// =============================================================================
// Hook Types
// =============================================================================

// Selected legacy names only: Core keeps its distinct runtime context/result/config.
pub use aleph_slot_api::hooks::{
    HookAction, HookEvent, HookKind, HookPriority, MatchSubject, SESSION_SOURCES_FIRED,
    SESSION_SOURCE_STARTUP,
};

/// Prompt injection scope
///
/// Determines when and how a prompt is injected into the conversation context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PromptScope {
    /// System-level: Always injected when plugin is active
    #[default]
    System,

    /// Tool-bound: Injected when specific tool is available
    Tool,

    /// Standalone: User must explicitly invoke (command)
    Standalone,

    /// Disabled: Not injected
    Disabled,
}

impl PromptScope {
    /// Parse from string with fallback to System
    #[must_use]
    pub fn from_str_or_default(s: &str) -> Self {
        s.parse().unwrap_or(Self::System)
    }
}

impl std::str::FromStr for PromptScope {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "system" => Ok(Self::System),
            "tool" => Ok(Self::Tool),
            "standalone" => Ok(Self::Standalone),
            "disabled" => Ok(Self::Disabled),
            _ => Err(format!("Unknown prompt scope: {s}")),
        }
    }
}

/// Hook configuration - defines when and how a hook executes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfig {
    /// Event to hook
    pub event: HookEvent,

    /// Hook execution kind (V2)
    #[serde(default)]
    pub kind: HookKind,

    /// Hook priority (V2)
    #[serde(default)]
    pub priority: HookPriority,

    /// Regex pattern to match (for tool-based events)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,

    /// Actions to execute
    pub actions: Vec<HookAction>,

    /// Plugin name (for logging)
    #[serde(default)]
    pub plugin_name: String,

    /// Plugin root: what the hook's path variables (`${CLAUDE_PLUGIN_ROOT}` …) name
    #[serde(skip)]
    pub plugin_root: PathBuf,

    /// Handler function name (for runtime plugins)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handler: Option<String>,

    /// Per-hook timeout in seconds. Overrides the executor's default
    /// timeout when set. Applies to Command and Http actions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,

    /// The event name exactly as the hook's author wrote it (`PreToolUse`
    /// or `before_tool_call`) — what the stdin payload's `hook_event_name`
    /// echoes back (user ruling 2026-09-20 U-b). Filled by the two file
    /// parsers that read the key as written (`~/.aleph` / project
    /// `hooks.json`, a plugin's `hooks.json`). `None` for the runtime/WASM
    /// registration API and `aleph.plugin.toml` `[[hooks]]`: both dispatch
    /// to a WASM handler, which has always read the canonical serde name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_event: Option<String>,

    /// Who may see this hook fire: stamped by the producer that knows where
    /// the hook came from (`hooks::load_user_hooks` for `~/.aleph/hooks.json`
    /// and project files, `extension/mod.rs::sync_hooks_from_registry` for
    /// plugin-shipped hooks, from the owning plugin's registry row). The
    /// executor compares it with `extension::visibility::visible_to`. There is
    /// deliberately no `Default`: a hook constructed without saying where it
    /// belongs would fire everywhere.
    pub scope_key: crate::extension::visibility::ScopeKey,
}

impl HookConfig {
    /// The `hook_event_name` this hook's payload carries: the declared
    /// spelling when there is one, else the canonical serde name. THE one
    /// derivation — the executor's dispatch loops read it per hook.
    #[must_use]
    pub fn event_name(&self) -> String {
        self.declared_event
            .clone()
            .unwrap_or_else(|| self.event.canonical_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_name_is_the_declared_spelling_else_the_canonical_name() {
        let mut hook = HookConfig {
            event: HookEvent::BeforeToolCall,
            kind: HookKind::Interceptor,
            priority: HookPriority::Normal,
            matcher: None,
            actions: Vec::new(),
            plugin_name: "t".into(),
            plugin_root: PathBuf::new(),
            handler: None,
            timeout_secs: None,
            declared_event: None,
            scope_key: crate::extension::visibility::ScopeKey::Global,
        };
        assert_eq!(hook.event_name(), "before_tool_call");
        hook.declared_event = Some("PreToolUse".into());
        assert_eq!(hook.event_name(), "PreToolUse");
        // An absent spelling is not written out.
        hook.declared_event = None;
        let v = serde_json::to_value(&hook).unwrap();
        assert!(v.get("declared_event").is_none(), "{v}");
    }
}
