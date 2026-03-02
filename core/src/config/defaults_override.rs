//! Defaults override types for ~/.aleph/defaults.toml
//!
//! These types represent user overrides for built-in default values used during
//! serde deserialization. Because serde calls `fn default_*()` functions while
//! parsing config.toml, this file must be loaded and the OnceLock initialized
//! BEFORE config.toml is parsed.
//!
//! All fields are Option<T> so users only need to specify the defaults they
//! want to change. Missing fields fall back to the hard-coded defaults.

use serde::Deserialize;
use std::path::Path;
use std::sync::OnceLock;
use tracing::warn;

// =============================================================================
// Override types
// =============================================================================

/// Memory system defaults
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MemoryDefaultsOverride {
    /// Override the default similarity threshold for memory retrieval
    #[serde(default)]
    pub similarity_threshold: Option<f32>,
    /// Override the default retention days for memory items
    #[serde(default)]
    pub retention_days: Option<u32>,
    /// Override the default max context items returned from memory
    #[serde(default)]
    pub max_context_items: Option<u32>,
}

/// Provider defaults
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProviderDefaultsOverride {
    /// Override the default timeout in seconds for provider requests
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
}

/// Generation defaults
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GenerationDefaultsOverride {
    /// Override the default timeout in seconds for generation requests
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
}

/// Root struct for ~/.aleph/defaults.toml
///
/// Contains user overrides for default values that are used during serde
/// deserialization of config.toml. Must be loaded before config parsing.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DefaultsOverride {
    /// Memory system default overrides
    #[serde(default)]
    pub memory: Option<MemoryDefaultsOverride>,
    /// Provider default overrides
    #[serde(default)]
    pub provider: Option<ProviderDefaultsOverride>,
    /// Generation default overrides
    #[serde(default)]
    pub generation: Option<GenerationDefaultsOverride>,
}

// =============================================================================
// OnceLock global singleton
// =============================================================================

static DEFAULTS_OVERRIDE: OnceLock<DefaultsOverride> = OnceLock::new();

/// Initialize the global defaults override. Called once during startup.
///
/// If already initialized (e.g., in tests), the new value is silently ignored.
pub fn init_defaults_override(overrides: DefaultsOverride) {
    let _ = DEFAULTS_OVERRIDE.set(overrides);
}

/// Get a reference to the global defaults override.
///
/// Returns a default (empty) override if not yet initialized.
pub fn get_defaults_override() -> &'static DefaultsOverride {
    DEFAULTS_OVERRIDE.get_or_init(DefaultsOverride::default)
}

// =============================================================================
// Loading
// =============================================================================

/// Load defaults override from a TOML file.
///
/// Returns `DefaultsOverride::default()` if the file does not exist or cannot
/// be parsed. Logs warnings on parse errors.
pub fn load_defaults_override(path: &Path) -> DefaultsOverride {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return DefaultsOverride::default();
        }
        Err(e) => {
            warn!(
                "Failed to read defaults override file {}: {}",
                path.display(),
                e
            );
            return DefaultsOverride::default();
        }
    };

    match toml::from_str(&content) {
        Ok(parsed) => parsed,
        Err(e) => {
            warn!(
                "Failed to parse defaults override file {}: {}",
                path.display(),
                e
            );
            DefaultsOverride::default()
        }
    }
}

// =============================================================================
// Accessor helpers
// =============================================================================

impl DefaultsOverride {
    /// Get the provider timeout override, if set.
    pub fn provider_timeout_seconds(&self) -> Option<u64> {
        self.provider.as_ref()?.timeout_seconds
    }

    /// Get the memory similarity threshold override, if set.
    pub fn memory_similarity_threshold(&self) -> Option<f32> {
        self.memory.as_ref()?.similarity_threshold
    }

    /// Get the memory retention days override, if set.
    pub fn memory_retention_days(&self) -> Option<u32> {
        self.memory.as_ref()?.retention_days
    }

    /// Get the generation timeout override, if set.
    pub fn generation_timeout_seconds(&self) -> Option<u64> {
        self.generation.as_ref()?.timeout_seconds
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_defaults_override() {
        let parsed: DefaultsOverride = toml::from_str("").unwrap();
        assert!(parsed.memory.is_none());
        assert!(parsed.provider.is_none());
        assert!(parsed.generation.is_none());
        // Accessors should all return None
        assert!(parsed.provider_timeout_seconds().is_none());
        assert!(parsed.memory_similarity_threshold().is_none());
        assert!(parsed.memory_retention_days().is_none());
        assert!(parsed.generation_timeout_seconds().is_none());
    }

    #[test]
    fn test_memory_defaults_parse() {
        let toml_str = r#"
[memory]
similarity_threshold = 0.75
retention_days = 90
max_context_items = 20
"#;
        let parsed: DefaultsOverride = toml::from_str(toml_str).unwrap();

        let mem = parsed.memory.as_ref().unwrap();
        assert_eq!(mem.similarity_threshold, Some(0.75));
        assert_eq!(mem.retention_days, Some(90));
        assert_eq!(mem.max_context_items, Some(20));

        // Accessors
        assert_eq!(parsed.memory_similarity_threshold(), Some(0.75));
        assert_eq!(parsed.memory_retention_days(), Some(90));
    }

    #[test]
    fn test_provider_defaults_parse() {
        let toml_str = r#"
[provider]
timeout_seconds = 600
"#;
        let parsed: DefaultsOverride = toml::from_str(toml_str).unwrap();

        let prov = parsed.provider.as_ref().unwrap();
        assert_eq!(prov.timeout_seconds, Some(600));

        // Accessor
        assert_eq!(parsed.provider_timeout_seconds(), Some(600));
    }

    #[test]
    fn test_partial_override() {
        let toml_str = r#"
[memory]
similarity_threshold = 0.8
# retention_days and max_context_items are not set

[provider]
timeout_seconds = 120
# generation section is not present at all
"#;
        let parsed: DefaultsOverride = toml::from_str(toml_str).unwrap();

        // Memory: only similarity_threshold is set
        let mem = parsed.memory.as_ref().unwrap();
        assert_eq!(mem.similarity_threshold, Some(0.8));
        assert!(mem.retention_days.is_none());
        assert!(mem.max_context_items.is_none());

        // Provider: timeout_seconds is set
        assert_eq!(parsed.provider_timeout_seconds(), Some(120));

        // Generation: entire section is absent
        assert!(parsed.generation.is_none());
        assert!(parsed.generation_timeout_seconds().is_none());
    }

    #[test]
    fn test_load_nonexistent_defaults_file() {
        let result =
            load_defaults_override(Path::new("/tmp/does-not-exist-aleph-defaults.toml"));
        assert!(result.memory.is_none());
        assert!(result.provider.is_none());
        assert!(result.generation.is_none());
    }
}
