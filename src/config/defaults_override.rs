//! Defaults override types for ~/.aleph/defaults.toml
//!
//! These types represent user overrides for built-in default values used during
//! serde deserialization. Because serde calls `fn default_*()` functions while
//! parsing config.toml, this file must be loaded and the slot below
//! initialized BEFORE config.toml is parsed.
//!
//! All fields are Option<T> so users only need to specify the defaults they
//! want to change. Missing fields fall back to the hard-coded defaults.

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use serde::Deserialize;
use std::path::Path;
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
// Process-global singleton
// =============================================================================

/// `IndistinguishableDefault`, derived from the one reader
/// ([`get_defaults_override`]): an uninstalled handle answers the empty
/// override, which is byte-for-byte what a machine with no
/// `~/.aleph/defaults.toml` answers. Every `fn default_*()` serde calls while
/// parsing `config.toml` then returns its compiled value, and the operator's
/// `defaults.toml` is silently inert. The failure has a narrow window and no
/// symptom: this handle must be installed BEFORE config parsing, so "installed
/// too late" and "never installed" read the same to every consumer.
static DEFAULTS_OVERRIDE: CapabilitySlot<DefaultsOverride> = CapabilitySlot::new(
    "config/defaults-override",
    MissingSemantics::IndistinguishableDefault {
        reads_as: "an empty override -- every serde `default_*()` returns its \
                   compiled value, as if ~/.aleph/defaults.toml did not exist",
    },
);

/// The fallback [`get_defaults_override`] hands back when nothing was
/// installed — deliberately OUTSIDE the slot.
///
/// Latching it through [`CapabilitySlot::install`] would stamp `Installed` for
/// a boot that never happened, which is the forged stamp the whole round exists
/// to make impossible. `get_or_init` on the slot is not available for the same
/// reason, and that absence is the type doing its job rather than a gap in it.
///
/// A `const`-constructed plain `static` rather than a second `OnceLock`: it adds
/// no census candidate (the container-type rule selects `OnceLock`/`OnceCell`,
/// not arbitrary data), and the exhaustive struct literal means adding a field
/// to `DefaultsOverride` is a compile error here rather than a silently
/// widened default.
///
/// ⚠️ **This is the one BEHAVIOUR change in the batch, and it is a fix.** The
/// old accessor was `get_or_init(DefaultsOverride::default)`, which **latched**
/// the empty default into the cell on first read. `Config::load` calls
/// [`init_defaults_override`] only when `get_config_dir()` succeeds (`load.rs`,
/// inside `if let Some(ref dir) = config_dir`) but calls
/// [`get_defaults_override`] unconditionally further down — so a load that
/// could not resolve a config dir latched the empty override, and the next
/// load that *did* find one hit "already initialized; ignoring re-init" and
/// **silently discarded the operator's `defaults.toml`**. Reading through a
/// separate static cannot latch, so that later install now succeeds.
///
/// Blast radius checked: all five `get_defaults_override()` callers `.clone()`
/// or read a field immediately, so no long-lived `&'static` holds the
/// pre-install address across an install.
static EMPTY_DEFAULTS_OVERRIDE: DefaultsOverride = DefaultsOverride {
    memory: None,
    provider: None,
    generation: None,
};

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape, and why the
/// `#[allow(dead_code)]` expires with Task 11 rather than outliving it.
#[allow(dead_code)]
pub(crate) fn defaults_override_slot() -> &'static dyn SlotStatus {
    &DEFAULTS_OVERRIDE
}

/// Initialize the global defaults override. Called once during startup.
///
/// If already initialized (e.g., in tests), a warning is logged and the new
/// value is ignored. Callers that legitimately need to refresh the override
/// (e.g. a test running after a previous one) must restart the process or
/// otherwise reset the global state.
pub fn init_defaults_override(overrides: DefaultsOverride) {
    if !DEFAULTS_OVERRIDE.install(overrides) {
        warn!(
            "DEFAULTS_OVERRIDE already initialized; ignoring re-init. \
             defaults.toml from this load is silently inactive — restart the \
             process to pick up changes."
        );
    }
}

/// Get a reference to the global defaults override.
///
/// Returns a default (empty) override if not yet initialized.
pub fn get_defaults_override() -> &'static DefaultsOverride {
    DEFAULTS_OVERRIDE.get().unwrap_or(&EMPTY_DEFAULTS_OVERRIDE)
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
        assert!(parsed.generation_timeout_seconds().is_none());
    }

    #[test]
    fn test_memory_defaults_parse() {
        let toml_str = r#"
[memory]
similarity_threshold = 0.75
"#;
        let parsed: DefaultsOverride = toml::from_str(toml_str).unwrap();

        let mem = parsed.memory.as_ref().unwrap();
        assert_eq!(mem.similarity_threshold, Some(0.75));

        // Accessors
        assert_eq!(parsed.memory_similarity_threshold(), Some(0.75));
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

[provider]
timeout_seconds = 120
# generation section is not present at all
"#;
        let parsed: DefaultsOverride = toml::from_str(toml_str).unwrap();

        // Memory: similarity_threshold is set
        let mem = parsed.memory.as_ref().unwrap();
        assert_eq!(mem.similarity_threshold, Some(0.8));

        // Provider: timeout_seconds is set
        assert_eq!(parsed.provider_timeout_seconds(), Some(120));

        // Generation: entire section is absent
        assert!(parsed.generation.is_none());
        assert!(parsed.generation_timeout_seconds().is_none());
    }

    #[test]
    fn test_load_nonexistent_defaults_file() {
        let result = load_defaults_override(Path::new("/tmp/does-not-exist-aleph-defaults.toml"));
        assert!(result.memory.is_none());
        assert!(result.provider.is_none());
        assert!(result.generation.is_none());
    }

    /// The `reads_as` sentence reaches an operator, so the fallback it names
    /// is asserted — and asserting it also pins [`EMPTY_DEFAULTS_OVERRIDE`]'s
    /// literal against someone filling a value into it.
    #[test]
    fn the_accessor_exposes_this_handle_to_the_roster() {
        let slot = defaults_override_slot();
        assert_eq!(slot.id(), "config/defaults-override");
        let MissingSemantics::IndistinguishableDefault { reads_as } = slot.missing() else {
            panic!(
                "expected IndistinguishableDefault, got {:?}",
                slot.missing()
            );
        };
        assert!(
            reads_as.contains("empty override"),
            "must name what get_defaults_override() really hands back; got {reads_as:?}"
        );
        assert!(EMPTY_DEFAULTS_OVERRIDE.memory.is_none());
        assert!(EMPTY_DEFAULTS_OVERRIDE.provider.is_none());
        assert!(EMPTY_DEFAULTS_OVERRIDE.generation.is_none());
        assert!(EMPTY_DEFAULTS_OVERRIDE.provider_timeout_seconds().is_none());
        assert!(EMPTY_DEFAULTS_OVERRIDE
            .memory_similarity_threshold()
            .is_none());
        assert!(EMPTY_DEFAULTS_OVERRIDE
            .generation_timeout_seconds()
            .is_none());
    }
}
