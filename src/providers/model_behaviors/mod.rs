//! Model behavior loading — per-LLM-family behavioral directives.
//!
//! Built-in `.md` files are compiled into the binary. User overrides
//! at `~/.aleph/model_behaviors/{name}.md` take full precedence.

const BUILTIN_ANTHROPIC: &str = include_str!("anthropic.md");
const BUILTIN_OPENAI: &str = include_str!("openai.md");
const BUILTIN_GEMINI: &str = include_str!("gemini.md");
const BUILTIN_OLLAMA: &str = include_str!("ollama.md");

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

    // 1. Check user override (async I/O, silently skip if home_dir unavailable)
    if let Some(home) = dirs::home_dir() {
        let user_path = home
            .join(".aleph/model_behaviors")
            .join(format!("{name}.md"));
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
        _ => None,
    }
}

/// Map protocol name to default behavior name.
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
        assert!(content.contains("Execution Directives"));
        assert!(content.contains("ALWAYS call tools proactively"));
    }

    #[test]
    fn test_builtin_gemini_loads() {
        let content = builtin_behavior("gemini").unwrap();
        assert!(content.contains("Execution Directives"));
    }

    #[test]
    fn test_builtin_ollama_loads() {
        let content = builtin_behavior("ollama").unwrap();
        assert!(content.contains("Tool Usage Guide"));
    }

    #[test]
    fn test_builtin_unknown_returns_none() {
        assert!(builtin_behavior("unknown-model").is_none());
    }

    #[tokio::test]
    async fn test_load_falls_back_to_builtin() {
        let content = load_model_behavior("openai").await.unwrap();
        assert!(content.contains("Execution Directives"));
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
}
