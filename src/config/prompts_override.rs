//! Prompt-template overrides for `~/.aleph/prompts.toml`.
//!
//! One section, one consumer. The file used to declare five sections —
//! `[planner]`, `[bootstrap]`, `[memory]`, `[agent]`, `[scratchpad]` — with
//! seven accessor methods, of which exactly one (`scratchpad_template`) was ever
//! called. A user who wrote `[agent] system_prefix = "…"` got a clean parse, no
//! warning, and no effect; the four dead sections were removed on 2026-07-26.
//! Serde ignores unknown keys, so older files that still carry them keep
//! parsing — with the same (zero) effect they always had.
//!
//! Before adding a section back, wire its consumer in the same commit. A
//! configuration surface that reads correctly and does nothing is worse than no
//! surface at all: it looks like it works.

use serde::Deserialize;
use std::path::Path;
use tracing::warn;

/// Override for the scratchpad markdown template.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ScratchpadPrompts {
    /// Override the scratchpad markdown template.
    /// Consumer: `memory::scratchpad::template::get_template`.
    #[serde(default)]
    pub template: Option<String>,
}

/// Root struct for `~/.aleph/prompts.toml`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PromptsOverride {
    /// Scratchpad template override.
    #[serde(default)]
    pub scratchpad: Option<ScratchpadPrompts>,
}

impl PromptsOverride {
    /// Get the scratchpad template override, if set.
    pub fn scratchpad_template(&self) -> Option<&str> {
        self.scratchpad.as_ref().and_then(|s| s.template.as_deref())
    }
}

// =============================================================================
// Loading
// =============================================================================

/// Load prompts override from a TOML file.
///
/// Returns `PromptsOverride::default()` if the file does not exist or cannot be parsed.
/// Logs warnings on parse errors.
pub fn load_prompts_override(path: &Path) -> PromptsOverride {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return PromptsOverride::default();
        }
        Err(e) => {
            warn!(
                "Failed to read prompts override file {}: {}",
                path.display(),
                e
            );
            return PromptsOverride::default();
        }
    };

    match toml::from_str(&content) {
        Ok(parsed) => parsed,
        Err(e) => {
            warn!(
                "Failed to parse prompts override file {}: {}",
                path.display(),
                e
            );
            PromptsOverride::default()
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_prompts_override() {
        let parsed: PromptsOverride = toml::from_str("").unwrap();
        assert!(parsed.scratchpad.is_none());
        assert!(parsed.scratchpad_template().is_none());
    }

    #[test]
    fn test_scratchpad_template_parse() {
        let toml_str = r#"
[scratchpad]
template = """
# My Custom Scratchpad

## Current Task
[empty]

## Notes
"""
"#;
        let parsed: PromptsOverride = toml::from_str(toml_str).unwrap();

        let template = parsed.scratchpad_template().unwrap();
        assert!(template.contains("# My Custom Scratchpad"));
        assert!(template.contains("## Current Task"));
    }

    #[test]
    fn retired_sections_still_parse_and_are_ignored() {
        // Back-compat: an existing prompts.toml carrying the four removed
        // sections must not become a parse error. They were inert before the
        // removal and stay inert after it.
        let toml_str = r##"
[planner]
system_prompt = "You are a custom planner."

[bootstrap]
prompt = "Welcome!"

[memory]
compression_prompt = "Compress this"
extraction_prompt = "Extract this"

[agent]
system_prefix = "prefix"
observation_prompt = "observe"

[scratchpad]
template = "# Still honored"
"##;
        let parsed: PromptsOverride = toml::from_str(toml_str).unwrap();
        assert_eq!(parsed.scratchpad_template(), Some("# Still honored"));
    }

    #[test]
    fn test_load_nonexistent_prompts_file() {
        let result = load_prompts_override(Path::new("/tmp/does-not-exist-aleph-prompts.toml"));
        assert!(result.scratchpad.is_none());
    }
}
