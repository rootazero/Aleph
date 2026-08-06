//! Shared agent-ID validation and ID-generation helpers.
//!
//! Two call sites today — `agent_create` (LLM-driven) and `AgentManager`
//! (TOML persistence). Both need the same grammar; this is the single source
//! so the LLM-side error message and the TOML-side `invalid_config` agree
//! byte-for-byte.
//!
//! Grammar: `[a-z0-9][a-z0-9_-]{0,63}` (1–64 chars). Tool-id grammar mirrors
//! `AgentManager::validate_id` minus the 32-char cap (the runtime registry
//! allows longer ids so sub-agent descriptors and user-created agents have
//! room). When `AgentManager` rejects a longer id the LLM gets a clean
//! `protected`/already-exists error on the second round-trip; the asymmetry
//! here is intentional — runtime ids are user-facing, TOML ids are admin.

/// Validate an agent ID against the runtime grammar.
///
/// Returns `Err(msg)` with a one-line, model-friendly reason when the ID is
/// empty, too long, or contains characters outside `[a-z0-9_-]`.
pub fn validate_agent_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("Agent ID cannot be empty".to_string());
    }
    if id.len() > 64 {
        return Err(format!(
            "Agent ID too long ({} chars, max 64)",
            id.len()
        ));
    }
    let first = id.chars().next().expect("non-empty checked above");
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(format!(
            "Agent ID must start with a lowercase letter or digit, got '{first}'"
        ));
    }
    for ch in id.chars().skip(1) {
        if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '_' && ch != '-' {
            return Err(format!(
                "Agent ID contains invalid character '{ch}'. Allowed: a-z, 0-9, _, -"
            ));
        }
    }
    Ok(())
}

/// Generate a valid ASCII agent ID from a display name.
///
/// Slugifies ASCII alphanumerics into `[a-z0-9-]` separators; falls back to
/// a deterministic 8-char hash when the slug is empty or too short, so
/// non-ASCII names (`"交易助手"` → `agent-a1b2c3d4`) stay reproducible across
/// runs.
#[must_use]
pub fn generate_agent_id_from_name(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else if c == ' ' || c == '-' || c == '_' {
                '-'
            } else {
                '\0'
            }
        })
        .filter(|&c| c != '\0')
        .collect();

    let slug: String = slug
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    if slug.len() >= 2
        && slug.len() <= 64
        && slug
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        return slug;
    }

    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    format!("agent-{:08x}", hasher.finish() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_agent_id_accepts_valid() {
        assert!(validate_agent_id("main").is_ok());
        assert!(validate_agent_id("trader").is_ok());
        assert!(validate_agent_id("my-agent").is_ok());
        assert!(validate_agent_id("agent_01").is_ok());
        assert!(validate_agent_id("0agent").is_ok());
        assert!(validate_agent_id("a").is_ok());
    }

    #[test]
    fn validate_agent_id_rejects_invalid() {
        assert!(validate_agent_id("").is_err());
        assert!(validate_agent_id("Agent").is_err()); // uppercase
        assert!(validate_agent_id("-start").is_err()); // starts with dash
        assert!(validate_agent_id("_start").is_err()); // starts with underscore
        assert!(validate_agent_id("has space").is_err());
        assert!(validate_agent_id("has.dot").is_err());
        let long = "a".repeat(65);
        assert!(validate_agent_id(&long).is_err()); // too long
    }

    #[test]
    fn validate_agent_id_accepts_max_length() {
        let exact = "a".repeat(64);
        assert!(validate_agent_id(&exact).is_ok());
    }

    #[test]
    fn generate_id_ascii_name() {
        assert_eq!(
            generate_agent_id_from_name("Trading Assistant"),
            "trading-assistant"
        );
        assert_eq!(
            generate_agent_id_from_name("code-reviewer"),
            "code-reviewer"
        );
        assert_eq!(generate_agent_id_from_name("my_agent"), "my-agent");
    }

    #[test]
    fn generate_id_non_ascii_name() {
        let id = generate_agent_id_from_name("交易助手");
        assert!(id.starts_with("agent-"), "Got: {id}");
        assert!(validate_agent_id(&id).is_ok(), "Generated id invalid: {id}");
        assert_eq!(id, generate_agent_id_from_name("交易助手"));
    }

    #[test]
    fn generate_id_mixed_name() {
        // ASCII + non-ASCII: "ai" is the slug; 2 chars, valid.
        assert_eq!(generate_agent_id_from_name("AI助手"), "ai");
    }

    #[test]
    fn generate_id_single_char_falls_back() {
        let id = generate_agent_id_from_name("A");
        assert!(id.starts_with("agent-"), "Single char fallback: {id}");
    }
}