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

// A `[memory]` override section (`similarity_threshold`) used to live here.
// It fed `default_similarity_threshold()`, whose config field was cut as a
// never-wired no-op; a `defaults.toml` still carrying the section parses
// fine and is ignored (no `deny_unknown_fields`).

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
/// ⚠️ **This is the one BEHAVIOUR change in this file's batch, and it is a
/// fix.** The round makes exactly one other, in [`crate::spend`]'s
/// `global_ledger` — the same `get_or_init` latch on a different handle, and
/// that file says so too. (Both sentences used to claim to be the only one;
/// read at the level of the round, which is how a later reader reads them,
/// they contradict.) The old accessor was
/// `get_or_init(DefaultsOverride::default)`, which **latched**
/// the empty default into the cell on first read. `Config::load` calls
/// [`init_defaults_override`] only when `get_config_dir()` succeeds (`load.rs`,
/// inside `if let Some(ref dir) = config_dir`) but calls
/// [`get_defaults_override`] unconditionally further down — so a load that
/// could not resolve a config dir latched the empty override, and the next
/// load that *did* find one hit "already initialized; ignoring re-init" and
/// **silently discarded the operator's `defaults.toml`**. Reading through a
/// separate static cannot latch, so that later install now succeeds.
///
/// Blast radius checked: every `get_defaults_override()` caller `.clone()`s
/// or reads a field immediately, so no long-lived `&'static` holds the
/// pre-install address across an install.
static EMPTY_DEFAULTS_OVERRIDE: DefaultsOverride = DefaultsOverride {
    provider: None,
    generation: None,
};

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn defaults_override_slot() -> &'static dyn SlotStatus {
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
        assert!(parsed.provider.is_none());
        assert!(parsed.generation.is_none());
        // Accessors should all return None
        assert!(parsed.provider_timeout_seconds().is_none());
        assert!(parsed.generation_timeout_seconds().is_none());
    }

    /// A `defaults.toml` written for the removed `[memory]` override section
    /// must keep parsing: the section is ignored, not rejected, so an operator
    /// with a stale file gets compiled defaults instead of a broken boot.
    #[test]
    fn stale_memory_override_section_is_ignored_not_rejected() {
        let toml_str = r#"
[memory]
similarity_threshold = 0.75
"#;
        let parsed: DefaultsOverride = toml::from_str(toml_str).unwrap();
        assert!(parsed.provider.is_none());
        assert!(parsed.generation.is_none());
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
[provider]
timeout_seconds = 120
# generation section is not present at all
"#;
        let parsed: DefaultsOverride = toml::from_str(toml_str).unwrap();

        // Provider: timeout_seconds is set
        assert_eq!(parsed.provider_timeout_seconds(), Some(120));

        // Generation: entire section is absent
        assert!(parsed.generation.is_none());
        assert!(parsed.generation_timeout_seconds().is_none());
    }

    #[test]
    fn test_load_nonexistent_defaults_file() {
        let result = load_defaults_override(Path::new("/tmp/does-not-exist-aleph-defaults.toml"));
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
        assert!(EMPTY_DEFAULTS_OVERRIDE.provider.is_none());
        assert!(EMPTY_DEFAULTS_OVERRIDE.generation.is_none());
        assert!(EMPTY_DEFAULTS_OVERRIDE.provider_timeout_seconds().is_none());
        assert!(EMPTY_DEFAULTS_OVERRIDE
            .generation_timeout_seconds()
            .is_none());
    }
}
