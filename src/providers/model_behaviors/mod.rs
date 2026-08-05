//! Model behavior loading — per-LLM-family behavioral directives.
//!
//! Built-in `.md` files are compiled into the binary. User overrides
//! at `~/.aleph/model_behaviors/{name}.md` take full precedence.

const BUILTIN_ANTHROPIC: &str = include_str!("anthropic.md");
const BUILTIN_OPENAI: &str = include_str!("openai.md");
const BUILTIN_GEMINI: &str = include_str!("gemini.md");
const BUILTIN_OLLAMA: &str = include_str!("ollama.md");
const BUILTIN_STRICT: &str = include_str!("strict.md");

/// Validate a behavior name to prevent path traversal.
///
/// Only allows alphanumeric characters, hyphens, and underscores.
fn is_valid_behavior_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Load model behavior content by name (async).
///
/// Checks user override at `~/.aleph/model_behaviors/{name}.md` first,
/// falls back to built-in content. Rejects invalid names to prevent
/// path traversal.
pub async fn load_model_behavior(name: &str) -> Option<String> {
    if !is_valid_behavior_name(name) {
        return None;
    }

    // 1. Check user override (async I/O, silently skip if the dir is
    //    unresolvable). `ALEPH_HOME`-aware: an override the user dropped into
    //    the configured home must be the one that wins, or the "user override"
    //    layer is silently inert under a relocated home.
    if let Ok(home) = crate::utils::paths::get_config_dir() {
        let user_path = home.join("model_behaviors").join(format!("{name}.md"));
        if let Ok(content) = tokio::fs::read_to_string(&user_path).await {
            return Some(content);
        }
    }

    // 2. Fallback to built-in
    builtin_behavior(name)
}

/// Get built-in behavior content (no user override check).
fn builtin_behavior(name: &str) -> Option<String> {
    match name {
        "anthropic" => Some(BUILTIN_ANTHROPIC.to_string()),
        "openai" => Some(BUILTIN_OPENAI.to_string()),
        "gemini" => Some(BUILTIN_GEMINI.to_string()),
        "ollama" => Some(BUILTIN_OLLAMA.to_string()),
        "strict" => Some(BUILTIN_STRICT.to_string()),
        _ => None,
    }
}

/// Map protocol name to default behavior name.
#[must_use]
pub fn protocol_to_behavior(protocol: &str) -> Option<&'static str> {
    match protocol {
        "anthropic" => Some("anthropic"),
        "openai" => Some("openai"),
        "openai-responses" => Some("openai"),
        "gemini" => Some("gemini"),
        "ollama" => Some("ollama"),
        _ => None,
    }
}

/// Self-identify a weak/open-weight vendor from its endpoint and model id so
/// it can be governed with the tight `"strict"` profile even when it shares a
/// wire protocol with a strong model (e.g. Kimi/Minimax over the anthropic
/// protocol). Matching is on machine-stable identifiers only — base-URL host
/// substrings and model-id substrings, lowercased (P8: never natural language).
///
/// `base_url` is the more reliable signal (one provider → one endpoint;
/// Minimax's legacy `abab*` model ids do not contain the vendor name), so it
/// is checked first. Returns the behavior name (`"strict"`) or `None` for
/// unrecognized / strong vendors.
#[must_use]
pub fn vendor_identity(base_url: Option<&str>, model_id: &str) -> Option<&'static str> {
    const URL_SIGNALS: &[&str] = &[
        "moonshot.cn",
        "moonshot.ai",
        "minimaxi.com",
        "minimax.io",
        "api.deepseek.com",
        "dashscope.aliyuncs.com",
        "open.bigmodel.cn",
    ];
    const MODEL_SIGNALS: &[&str] = &[
        "moonshot", "kimi", "minimax", "abab", "deepseek", "qwen", "qwq", "glm", "chatglm",
    ];
    if let Some(url) = base_url {
        let url = url.to_ascii_lowercase();
        if URL_SIGNALS.iter().any(|s| url.contains(s)) {
            return Some("strict");
        }
    }
    let model = model_id.to_ascii_lowercase();
    if MODEL_SIGNALS.iter().any(|s| model.contains(s)) {
        return Some("strict");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_anthropic_loads() {
        let content = builtin_behavior("anthropic").unwrap();
        assert!(!content.is_empty());
    }

    #[test]
    fn test_builtin_openai_loads() {
        let content = builtin_behavior("openai").unwrap();
        assert!(content.contains("Execution Discipline — OpenAI Family"));
        assert!(content.contains("Act, don't ask"));
    }

    #[test]
    fn test_builtin_gemini_loads() {
        let content = builtin_behavior("gemini").unwrap();
        assert!(content.contains("Google Model Operational Directives"));
    }

    #[test]
    fn test_builtin_ollama_loads() {
        let content = builtin_behavior("ollama").unwrap();
        assert!(content.contains("Local / Open-Weight Model Guide"));
    }

    #[test]
    fn test_builtin_unknown_returns_none() {
        assert!(builtin_behavior("unknown-model").is_none());
    }

    #[tokio::test]
    async fn test_load_falls_back_to_builtin() {
        let content = load_model_behavior("openai").await.unwrap();
        assert!(content.contains("Execution Discipline — OpenAI Family"));
    }

    #[tokio::test]
    async fn test_load_unknown_returns_none() {
        assert!(load_model_behavior("nonexistent").await.is_none());
    }

    #[test]
    fn test_protocol_to_behavior_openai() {
        assert_eq!(protocol_to_behavior("openai"), Some("openai"));
    }

    #[test]
    fn test_protocol_to_behavior_openai_responses() {
        assert_eq!(protocol_to_behavior("openai-responses"), Some("openai"));
    }

    #[test]
    fn test_protocol_to_behavior_anthropic() {
        assert_eq!(protocol_to_behavior("anthropic"), Some("anthropic"));
    }

    #[test]
    fn test_protocol_to_behavior_gemini() {
        assert_eq!(protocol_to_behavior("gemini"), Some("gemini"));
    }

    #[test]
    fn test_protocol_to_behavior_ollama() {
        assert_eq!(protocol_to_behavior("ollama"), Some("ollama"));
    }

    #[test]
    fn test_protocol_to_behavior_unknown() {
        assert_eq!(protocol_to_behavior("some-custom-protocol"), None);
    }

    // Path traversal validation tests
    #[test]
    fn test_valid_behavior_names() {
        assert!(is_valid_behavior_name("openai"));
        assert!(is_valid_behavior_name("my-custom"));
        assert!(is_valid_behavior_name("my_custom_123"));
    }

    #[test]
    fn test_invalid_behavior_names() {
        assert!(!is_valid_behavior_name(""));
        assert!(!is_valid_behavior_name("../etc/passwd"));
        assert!(!is_valid_behavior_name("../../secret"));
        assert!(!is_valid_behavior_name("foo/bar"));
        assert!(!is_valid_behavior_name("foo.bar"));
        assert!(!is_valid_behavior_name("a".repeat(65).as_str()));
    }

    #[tokio::test]
    async fn test_load_rejects_path_traversal() {
        assert!(load_model_behavior("../../../etc/passwd").await.is_none());
        assert!(load_model_behavior("foo/bar").await.is_none());
    }

    #[test]
    fn vendor_identity_matches_kimi_by_url_and_model() {
        assert_eq!(
            vendor_identity(
                Some("https://api.moonshot.cn/anthropic"),
                "kimi-k2-0905-preview"
            ),
            Some("strict")
        );
        assert_eq!(
            vendor_identity(None, "kimi-k2-turbo-preview"),
            Some("strict")
        );
        assert_eq!(
            vendor_identity(Some("https://api.moonshot.ai/v1"), "moonshot-v1-8k"),
            Some("strict")
        );
    }

    #[test]
    fn vendor_identity_matches_minimax_including_abab_by_url() {
        assert_eq!(
            vendor_identity(Some("https://api.minimaxi.com/anthropic"), "MiniMax-M2.5"),
            Some("strict")
        );
        assert_eq!(
            vendor_identity(Some("https://api.minimaxi.com/anthropic"), "abab6.5s-chat"),
            Some("strict")
        );
        assert_eq!(vendor_identity(None, "abab6.5s-chat"), Some("strict"));
    }

    #[test]
    fn vendor_identity_matches_other_oss_families() {
        assert_eq!(
            vendor_identity(Some("https://api.deepseek.com"), "deepseek-chat"),
            Some("strict")
        );
        assert_eq!(vendor_identity(None, "qwen-max"), Some("strict"));
        assert_eq!(
            vendor_identity(Some("https://dashscope.aliyuncs.com"), "qwq-32b"),
            Some("strict")
        );
        assert_eq!(vendor_identity(None, "glm-4-plus"), Some("strict"));
    }

    #[test]
    fn vendor_identity_ignores_strong_models() {
        assert_eq!(
            vendor_identity(Some("https://api.openai.com/v1"), "gpt-4o"),
            None
        );
        assert_eq!(
            vendor_identity(Some("https://api.anthropic.com"), "claude-sonnet-4-6"),
            None
        );
        assert_eq!(vendor_identity(None, "gemini-2.5-pro"), None);
    }

    #[test]
    fn builtin_strict_loads() {
        let content = builtin_behavior("strict").unwrap();
        assert!(content.contains("Strict Operating Mode"));
        assert!(content.contains("One tool call at a time"));
    }
}
