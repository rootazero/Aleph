//! [memory] TOML section for Aleph plugin manifests.
//!
//! Third-party plugins add this optional section to declare which memory
//! hooks they implement, along with priority and scheduling hints.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryHook {
    OnRetrieve,
    OnCapture,
    Produce,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryManifestSection {
    /// Hooks the plugin implements. Other hooks use the trait's no-op defaults.
    pub hooks: Vec<MemoryHook>,

    /// Priority for `on_capture` chain ordering (lower = earlier).
    #[serde(default = "default_priority")]
    pub priority: i32,

    /// Only relevant when `produce` is in `hooks`. Seconds between
    /// invocations. If None, defaults to scheduler's tick interval.
    #[serde(default)]
    pub produce_interval_seconds: Option<u64>,
}

const fn default_priority() -> i32 {
    100
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_manifest() {
        let toml = r#"
hooks = ["on_retrieve", "produce"]
priority = 50
produce_interval_seconds = 300
"#;
        let m: MemoryManifestSection = toml::from_str(toml).unwrap();
        assert_eq!(m.hooks, vec![MemoryHook::OnRetrieve, MemoryHook::Produce]);
        assert_eq!(m.priority, 50);
        assert_eq!(m.produce_interval_seconds, Some(300));
    }

    #[test]
    fn defaults_apply_when_absent() {
        let toml = r#"hooks = ["on_capture"]"#;
        let m: MemoryManifestSection = toml::from_str(toml).unwrap();
        assert_eq!(m.priority, 100);
        assert!(m.produce_interval_seconds.is_none());
    }

    #[test]
    fn memory_hook_json_snake_case_round_trip() {
        for h in [
            MemoryHook::OnRetrieve,
            MemoryHook::OnCapture,
            MemoryHook::Produce,
        ] {
            let s = serde_json::to_string(&h).unwrap();
            let back: MemoryHook = serde_json::from_str(&s).unwrap();
            assert_eq!(back, h);
        }
    }

    #[test]
    fn toml_rejects_unknown_hook() {
        let toml = r#"hooks = ["on_retrieve", "not_a_real_hook"]"#;
        let parsed: Result<MemoryManifestSection, _> = toml::from_str(toml);
        assert!(parsed.is_err());
    }
}
