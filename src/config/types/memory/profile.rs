use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UserProfileConfig {
    #[serde(default = "super::defaults::default_profile_enabled")]
    pub enabled: bool,
    #[serde(default = "super::defaults::default_profile_min_interval_minutes")]
    pub profile_min_interval_minutes: u32,
    #[serde(default = "super::defaults::default_profile_inject_interval_turns")]
    pub profile_inject_interval_turns: u32,
    #[serde(default = "super::defaults::default_profile_max_body_bytes")]
    pub max_body_bytes: usize,
    #[serde(default = "super::defaults::default_profile_max_bullets")]
    pub max_bullets_per_section: usize,
    #[serde(default = "super::defaults::default_profile_bootstrap_on_first")]
    pub bootstrap_on_first_session_end: bool,
}

impl Default for UserProfileConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            profile_min_interval_minutes: 30,
            profile_inject_interval_turns: 10,
            max_body_bytes: 2048,
            max_bullets_per_section: 20,
            bootstrap_on_first_session_end: true,
        }
    }
}
