//! Mid-run trajectory resume configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `[resume]` config section — boot-scan auto-resume of interrupted runs.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResumeConfig {
    /// Master switch. When false the `ResumeCoordinator` is not spawned.
    #[serde(default = "default_resume_enabled")]
    pub enabled: bool,

    /// Don't resume runs interrupted more than this many seconds ago
    /// (default: 86400 = 24h). Older candidates are marked `Abandoned`.
    #[serde(default = "default_resume_max_age_secs")]
    pub max_age_secs: u64,

    /// Abandon a run after this many consecutive crash-loops (default: 3).
    #[serde(default = "default_resume_max_attempts")]
    pub max_attempts: u32,

    /// Cap simultaneous resumes to protect the freshly-booted process and
    /// provider rate limits (default: 2). One permit pool bounds the boot
    /// scan's fan-out (`ResumeCoordinator::launch_resume`) and on-demand
    /// `agent.resume` alike; a permit spans one candidate's boundary repair
    /// and its re-trigger.
    ///
    /// Lowered from 4 with the fan-out (spec §8.1): until then the knob
    /// bounded only `retrigger`, and the scan itself was serial, so
    /// contention never occurred and the 4 carried no evidence.
    #[serde(default = "default_resume_max_concurrent")]
    pub max_concurrent: usize,
}

const fn default_resume_enabled() -> bool {
    true
}

const fn default_resume_max_age_secs() -> u64 {
    86_400
}

const fn default_resume_max_attempts() -> u32 {
    3
}

const fn default_resume_max_concurrent() -> usize {
    2
}

impl Default for ResumeConfig {
    fn default() -> Self {
        Self {
            enabled: default_resume_enabled(),
            max_age_secs: default_resume_max_age_secs(),
            max_attempts: default_resume_max_attempts(),
            max_concurrent: default_resume_max_concurrent(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let c = ResumeConfig::default();
        assert!(c.enabled);
        assert_eq!(c.max_age_secs, 86_400);
        assert_eq!(c.max_attempts, 3);
        assert_eq!(c.max_concurrent, 2);
    }

    #[test]
    fn serde_with_missing_fields_uses_defaults() {
        let parsed: ResumeConfig = toml::from_str("").unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.max_attempts, 3);
    }

    #[test]
    fn serde_round_trip() {
        let c = ResumeConfig {
            enabled: false,
            max_age_secs: 100,
            max_attempts: 9,
            max_concurrent: 1,
        };
        let toml = toml::to_string(&c).unwrap();
        let back: ResumeConfig = toml::from_str(&toml).unwrap();
        assert!(!back.enabled);
        assert_eq!(back.max_age_secs, 100);
        assert_eq!(back.max_attempts, 9);
        assert_eq!(back.max_concurrent, 1);
    }
}
