//! Free functions for the builtin tool registry.
//!
//! `parse_caller_agent_id` and `resolve_plugin_handler_from_sources` are
//! standalone helpers (no `self`), grouped here so the registry's `impl`
//! blocks stay focused on method dispatch.
#![allow(unused_imports)]

use std::collections::HashMap;

use crate::tool_metadata::{ToolSource, UnifiedTool};

/// Parse `agent_id` out of a serialized session key string, returning
/// `fallback` when the key fails to parse.
///
/// `session_key_str` follows the canonical form `agent:<id>:<rest>` (see
/// [`crate::routing::session_key::SessionKey::to_key_string`]). A naive
/// `.split(':').next()` would return the literal `"agent"` namespace
/// prefix instead of the `agent_id`, silently misrouting per-agent state
/// (e.g. `RememberTool` writing to `~/.aleph/agents/agent/MEMORY.md`
/// instead of `~/.aleph/agents/<id>/MEMORY.md`). Going through
/// `SessionKey::from_key_string` keeps the parser in lock-step with the
/// canonical encoding and survives every key variant (Main, DM, Group,
/// Task, Subagent, Ephemeral) plus the legacy `peer:` form.
pub(crate) fn parse_caller_agent_id(session_key_str: &str, fallback: &str) -> String {
    crate::routing::session_key::SessionKey::from_key_string(session_key_str)
        .map_or_else(|| fallback.to_string(), |k| k.agent_id().to_string())
}

pub(crate) fn resolve_plugin_handler_from_sources(
    extension_manager: Option<&crate::extension::ExtensionManager>,
    tools: &HashMap<String, UnifiedTool>,
    tool_name: &str,
) -> Option<(String, String)> {
    if let Some(ext_mgr) = extension_manager {
        if let Some(tool) = ext_mgr.resolve_active_plugin_tool(tool_name) {
            return Some((tool.plugin_id, tool.handler));
        }
    }

    tools
        .get(tool_name)
        .and_then(|unified| match &unified.source {
            ToolSource::Plugin { plugin_id } => {
                Some((plugin_id.clone(), format!("tool_{tool_name}")))
            }
            _ => None,
        })
}
