//! `OpenAI` prompt-cache routing helpers shared by the Chat and Responses adapters.

use crate::providers::adapter::RequestPayload;
use sha2::{Digest, Sha256};
use std::borrow::Cow;

/// `OpenAI` rejects a `prompt_cache_key` longer than 64 characters.
pub(crate) const PROMPT_CACHE_KEY_MAX_CHARS: usize = 64;

/// Derive the request's `prompt_cache_key`: content-addressed from the static
/// prefix, session-id fallback.
///
/// `OpenAI` prompt caching is implicit prefix caching plus a routing hint —
/// requests sharing a `prompt_cache_key` land on the same backend, where the
/// shared byte prefix (system prompt + tool schemas) actually hits. Keying by
/// session id alone (the previous behaviour) made every daemon/cron-triggered
/// run cache-cold: each fire mints a fresh session id even though its static
/// prefix is byte-identical to the previous fire's. Hashing the static prefix
/// instead lets scheduled runs, subagents on the same toolset, and parallel
/// sessions of one agent share a warm routing bucket (hermes-agent
/// `_content_cache_key` parity).
///
/// The key is a routing hint only — never a correctness boundary: two
/// requests that share system prompt + toolset are *supposed* to share a
/// bucket. Domain separators (`\0` between fields, `\x01` between tools)
/// keep system/tool bytes from colliding across field boundaries; tools are
/// hashed sorted by name so registry iteration order cannot churn the key.
///
/// Only the *stable* parts of a split system prompt are hashed — the dynamic
/// suffix varies per turn and would otherwise re-key (and cold-route) every
/// request. Returns the clamped session id when the request carries neither
/// system prompt nor tools, and `None` when there is nothing to key on.
pub fn derive_prompt_cache_key(payload: &RequestPayload<'_>) -> Option<String> {
    let mut hasher = Sha256::new();
    let mut has_static = false;

    match payload.system_blocks {
        Some(parts) => {
            for part in parts.iter().filter(|p| p.cache) {
                hasher.update(part.content.as_bytes());
                has_static |= !part.content.is_empty();
            }
        }
        None => {
            if let Some(sp) = payload.system_prompt.filter(|s| !s.is_empty()) {
                hasher.update(sp.as_bytes());
                has_static = true;
            }
        }
    }
    hasher.update([0u8]);

    if let Some(tools) = payload.tools.filter(|t| !t.is_empty()) {
        let mut sorted: Vec<_> = tools.iter().collect();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        for tool in sorted {
            hasher.update(tool.name.as_bytes());
            hasher.update([0u8]);
            hasher.update(tool.description.as_bytes());
            hasher.update([0u8]);
            hasher.update(tool.parameters.to_string().as_bytes());
            hasher.update([1u8]);
        }
        has_static = true;
    }

    if has_static {
        let digest = hasher.finalize();
        // 12 bytes → 24 hex chars; `pck_` + 24 = 28 chars, well inside the
        // 64-char wire limit. Collision odds are irrelevant for a routing
        // hint (a collision just shares a backend bucket).
        use std::fmt::Write as _;
        let mut key = String::with_capacity(28);
        key.push_str("pck_");
        for byte in &digest[..12] {
            let _ = write!(key, "{byte:02x}");
        }
        Some(key)
    } else {
        payload
            .metadata
            .as_ref()
            .and_then(|m| m.get("session_id"))
            .filter(|s| !s.is_empty())
            .map(|s| clamp_prompt_cache_key(s).into_owned())
    }
}

/// Clamp a `prompt_cache_key` to `OpenAI`'s 64-character limit on a char boundary.
///
/// Aleph routes prompt-cache affinity by session id, but composite session keys
/// can exceed 64 chars — `OpenAI` then rejects the request (or silently ignores
/// the key). Mirrors pi's `clampOpenAIPromptCacheKey`, but clamps by `char`
/// rather than UTF-16 code unit so a multibyte key is never split mid-codepoint.
/// Returns `Cow::Borrowed` (no allocation) when already within bounds.
#[must_use]
pub fn clamp_prompt_cache_key(key: &str) -> Cow<'_, str> {
    // `char_indices` finds the byte offset of the 65th char without counting the
    // whole string when it is long; fall through to borrow when short enough.
    match key.char_indices().nth(PROMPT_CACHE_KEY_MAX_CHARS) {
        Some((byte_idx, _)) => Cow::Owned(key[..byte_idx].to_string()),
        None => Cow::Borrowed(key),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_key_borrowed_unchanged() {
        let k = "sess-abc";
        assert!(matches!(
            clamp_prompt_cache_key(k),
            Cow::Borrowed("sess-abc")
        ));
    }

    #[test]
    fn exact_limit_borrowed() {
        let k = "a".repeat(PROMPT_CACHE_KEY_MAX_CHARS);
        let out = clamp_prompt_cache_key(&k);
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out.chars().count(), PROMPT_CACHE_KEY_MAX_CHARS);
    }

    #[test]
    fn over_limit_truncated_to_64_chars() {
        let k = "x".repeat(100);
        let out = clamp_prompt_cache_key(&k);
        assert!(matches!(out, Cow::Owned(_)));
        assert_eq!(out.chars().count(), PROMPT_CACHE_KEY_MAX_CHARS);
    }

    #[test]
    fn multibyte_not_split_mid_codepoint() {
        // 70 multibyte chars → must clamp to 64 whole chars, still valid UTF-8.
        let k = "🦀".repeat(70);
        let out = clamp_prompt_cache_key(&k);
        assert_eq!(out.chars().count(), PROMPT_CACHE_KEY_MAX_CHARS);
        // Round-trips as valid UTF-8 (no panic / no partial codepoint).
        assert!(out.chars().all(|c| c == '🦀'));
    }

    // ── derive_prompt_cache_key ────────────────────────────────────────────

    use crate::providers::message::UnifiedMessage;
    use crate::tool_metadata::{ToolCategory, ToolDefinition};

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition::new(
            name,
            format!("{name} description"),
            serde_json::json!({"type": "object", "properties": {}}),
            ToolCategory::Builtin,
        )
    }

    fn payload_with_session<'a>(
        messages: &'a [UnifiedMessage],
        session_id: &str,
    ) -> RequestPayload<'a> {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("session_id".to_string(), session_id.to_string());
        RequestPayload::new(messages).with_metadata(Some(metadata))
    }

    #[test]
    fn same_static_prefix_same_key_across_sessions() {
        // The daemon/cron fix: two runs with different session ids but an
        // identical static prefix must share a warm routing bucket.
        let msgs = [UnifiedMessage::user("hi")];
        let tools = [tool("alpha"), tool("beta")];
        let a = payload_with_session(&msgs, "cron_1_170001")
            .with_system(Some("You are Aleph."))
            .with_tools(Some(&tools));
        let b = payload_with_session(&msgs, "cron_1_170099")
            .with_system(Some("You are Aleph."))
            .with_tools(Some(&tools));
        let ka = derive_prompt_cache_key(&a).expect("key");
        let kb = derive_prompt_cache_key(&b).expect("key");
        assert_eq!(ka, kb);
        assert!(ka.starts_with("pck_"));
        assert!(ka.chars().count() <= PROMPT_CACHE_KEY_MAX_CHARS);
    }

    #[test]
    fn tool_order_does_not_change_key() {
        let msgs = [UnifiedMessage::user("hi")];
        let ab = [tool("alpha"), tool("beta")];
        let ba = [tool("beta"), tool("alpha")];
        let a = RequestPayload::new(&msgs)
            .with_system(Some("sys"))
            .with_tools(Some(&ab));
        let b = RequestPayload::new(&msgs)
            .with_system(Some("sys"))
            .with_tools(Some(&ba));
        assert_eq!(derive_prompt_cache_key(&a), derive_prompt_cache_key(&b));
    }

    #[test]
    fn different_system_prompt_different_key() {
        let msgs = [UnifiedMessage::user("hi")];
        let a = RequestPayload::new(&msgs).with_system(Some("persona A"));
        let b = RequestPayload::new(&msgs).with_system(Some("persona B"));
        assert_ne!(derive_prompt_cache_key(&a), derive_prompt_cache_key(&b));
    }

    #[test]
    fn split_system_hashes_only_stable_parts() {
        // The dynamic suffix varies per turn; hashing it would re-key (and
        // cold-route) every request. Two payloads differing only in their
        // dynamic part must share a key.
        use crate::thinker::prompt_builder::SystemPromptPart;
        let msgs = [UnifiedMessage::user("hi")];
        let parts_a = [
            SystemPromptPart {
                content: "stable persona".into(),
                cache: true,
            },
            SystemPromptPart {
                content: "time=2026-07-17T10".into(),
                cache: false,
            },
        ];
        let parts_b = [
            SystemPromptPart {
                content: "stable persona".into(),
                cache: true,
            },
            SystemPromptPart {
                content: "time=2026-07-17T11".into(),
                cache: false,
            },
        ];
        let a = RequestPayload::new(&msgs).with_system_blocks(Some(&parts_a));
        let b = RequestPayload::new(&msgs).with_system_blocks(Some(&parts_b));
        let ka = derive_prompt_cache_key(&a).expect("key");
        let kb = derive_prompt_cache_key(&b).expect("key");
        assert_eq!(ka, kb);
    }

    #[test]
    fn no_static_content_falls_back_to_session_id() {
        let msgs = [UnifiedMessage::user("hi")];
        let payload = payload_with_session(&msgs, "sess-42");
        assert_eq!(
            derive_prompt_cache_key(&payload).as_deref(),
            Some("sess-42")
        );
    }

    #[test]
    fn nothing_to_key_on_returns_none() {
        let msgs = [UnifiedMessage::user("hi")];
        let payload = RequestPayload::new(&msgs);
        assert_eq!(derive_prompt_cache_key(&payload), None);
    }
}
