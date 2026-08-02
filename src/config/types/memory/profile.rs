use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// CUT: `profile_inject_interval_turns` had no mechanism behind it — nothing
// counts turns for profile injection, the `<UserProfile>` block is part of the
// per-session frozen curated snapshot. `max_body_bytes` was a duplicate budget:
// the live render already truncates the body at `[memory.curated]
// user_char_limit` (`capture_curated` → `render_user_block`), so two knobs
// claimed the same job and only one of them was ever read.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UserProfileConfig {
    /// Master switch for USER.md synthesis. `false` means no synthesizer is
    /// constructed at all, which is the point of the knob: the session-end
    /// merge is an LLM call that fires whenever a synthesizer exists.
    #[serde(default = "super::defaults::default_profile_enabled")]
    pub enabled: bool,
    #[serde(default = "super::defaults::default_profile_min_interval_minutes")]
    pub profile_min_interval_minutes: u32,
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
            max_bullets_per_section: 20,
            bootstrap_on_first_session_end: true,
        }
    }
}
