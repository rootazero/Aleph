//! Anthropic protocol adapter
//!
//! Handles Claude Messages API format.

use crate::sync_primitives::{Arc, RwLock};
use reqwest::Client;
use std::collections::HashMap;

/// Anthropic API version header value
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// User-Agent sent on OAuth requests.
///
/// Anthropic's OAuth infrastructure validates the user-agent and intermittently
/// rejects requests whose spoofed Claude Code version is far behind the
/// shipping CLI. Keep this string reasonably recent — bump alongside the
/// `claude-cli` releases the OAuth flow is paired with. Matches the fallback
/// hermes-agent uses when it can't detect a locally installed CLI version.
const CLAUDE_CODE_USER_AGENT: &str = "claude-cli/2.1.74";

/// Mandatory first system block for Anthropic OAuth requests.
///
/// Anthropic's OAuth infrastructure (the `oauth-2025-04-20` + `claude-code`
/// beta stack) validates that the request identifies as Claude Code: the very
/// first `system` block must be exactly this string. Omitting it makes
/// otherwise well-formed OAuth requests fail with 401/403, so it is injected
/// transport-side whenever an OAuth token is detected — never surfaced to the
/// caller's persona/system-prompt layer. Mirrors openclaw
/// `anthropic-transport-stream.ts` and hermes-agent's OAuth payload builder.
const CLAUDE_CODE_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

/// Sanitize a tool name to satisfy Anthropic's regex `^[a-zA-Z][a-zA-Z0-9_-]{0,127}$`.
///
/// Replaces any disallowed character with `_`, prefixes a letter when the
/// resulting name doesn't start with one, and truncates to 128 chars. The
/// transform is deterministic so identical inputs always sanitize to the same
/// output, allowing a per-process `sanitized → original` map to round-trip.
pub(crate) fn sanitize_anthropic_tool_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let needs_prefix = out.chars().next().is_none_or(|c| !c.is_ascii_alphabetic());
    if needs_prefix {
        out = format!("t_{out}");
    }
    if out.len() > 128 {
        out.truncate(128);
    }
    out
}

/// Shared map: sanitized tool name → original tool name.
type ToolNameMap = Arc<RwLock<HashMap<String, String>>>;

/// Anthropic protocol adapter
pub struct AnthropicProtocol {
    client: Client,
    /// Sanitized → original tool-name map. Populated when building requests
    /// (so Anthropic accepts the names) and consulted while parsing the
    /// streamed response (so the tool layer receives the original names).
    name_map: ToolNameMap,
    /// Per-event idle timeout (seconds) for streaming responses.
    /// Written by `build_request` from `ProviderConfig.stream_idle_timeout_secs`
    /// (default 60); read by `stream_deltas` at stream-construction time.
    /// A value of 0 disables the idle watchdog.
    ///
    /// Uses `AtomicU64` rather than `RwLock<u64>` because the value is a
    /// single primitive: lock-free load/store is appropriate and avoids
    /// any contention between concurrent `build_request` and `stream_deltas`
    /// calls within the same protocol instance.
    stream_idle_timeout_secs: Arc<crate::sync_primitives::AtomicU64>,
}

mod adapter;
mod proto_impl;
pub mod provider_policy;
mod sse;

#[cfg(test)]
mod adapter_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_tool_name_passthrough() {
        assert_eq!(sanitize_anthropic_tool_name("read_file"), "read_file");
        assert_eq!(sanitize_anthropic_tool_name("get-data"), "get-data");
        assert_eq!(sanitize_anthropic_tool_name("Tool123"), "Tool123");
    }

    #[test]
    fn test_sanitize_tool_name_replaces_dots() {
        assert_eq!(
            sanitize_anthropic_tool_name("agents.bindings"),
            "agents_bindings"
        );
        assert_eq!(
            sanitize_anthropic_tool_name("channel.pairing.list"),
            "channel_pairing_list"
        );
    }

    #[test]
    fn test_sanitize_tool_name_replaces_other_invalid_chars() {
        assert_eq!(
            sanitize_anthropic_tool_name("chrome-devtools-mcp@latest"),
            "chrome-devtools-mcp_latest"
        );
        assert_eq!(sanitize_anthropic_tool_name("foo bar/baz"), "foo_bar_baz");
        assert_eq!(sanitize_anthropic_tool_name("查询工具"), "t_____");
    }

    #[test]
    fn test_sanitize_tool_name_prefixes_when_first_not_letter() {
        assert_eq!(sanitize_anthropic_tool_name("123tool"), "t_123tool");
        assert_eq!(sanitize_anthropic_tool_name("_tool"), "t__tool");
        assert_eq!(sanitize_anthropic_tool_name(""), "t_");
    }

    #[test]
    fn test_sanitize_tool_name_truncates_to_128() {
        let long = "a".repeat(200);
        let out = sanitize_anthropic_tool_name(&long);
        assert_eq!(out.len(), 128);
        assert!(out.chars().all(|c| c == 'a'));
    }

    #[test]
    fn test_sanitize_tool_name_is_deterministic() {
        // Same input must always produce same output (round-trip via name_map).
        assert_eq!(
            sanitize_anthropic_tool_name("foo.bar"),
            sanitize_anthropic_tool_name("foo.bar")
        );
    }
}
