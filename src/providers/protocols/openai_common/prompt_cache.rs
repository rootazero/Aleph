//! `OpenAI` prompt-cache routing helpers shared by the Chat and Responses adapters.

use std::borrow::Cow;

/// `OpenAI` rejects a `prompt_cache_key` longer than 64 characters.
pub const PROMPT_CACHE_KEY_MAX_CHARS: usize = 64;

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
}
