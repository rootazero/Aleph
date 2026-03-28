//! Model behavior loading — per-LLM-family behavioral directives.
//!
//! Built-in `.md` files are compiled into the binary. User overrides
//! at `~/.aleph/model_behaviors/{name}.md` take full precedence.

const BUILTIN_ANTHROPIC: &str = include_str!("anthropic.md");
const BUILTIN_OPENAI: &str = include_str!("openai.md");
const BUILTIN_GEMINI: &str = include_str!("gemini.md");
const BUILTIN_OLLAMA: &str = include_str!("ollama.md");

/// Load model behavior content by name.
///
/// Checks user override at `~/.aleph/model_behaviors/{name}.md` first,
/// falls back to built-in content.
pub fn load_model_behavior(name: &str) -> Option<String> {
    // 1. Check user override (silently skip if home_dir unavailable)
    if let Some(home) = dirs::home_dir() {
        let user_path = home.join(".aleph/model_behaviors").join(format!("{name}.md"));
        if let Ok(content) = std::fs::read_to_string(&user_path) {
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

    #[test]
    fn test_load_falls_back_to_builtin() {
        let content = load_model_behavior("openai").unwrap();
        assert!(content.contains("Execution Directives"));
    }

    #[test]
    fn test_load_unknown_returns_none() {
        assert!(load_model_behavior("nonexistent").is_none());
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
}
