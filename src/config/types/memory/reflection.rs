use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReflectionConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "super::defaults::default_reflection_min_turns")]
    pub min_turns: u32,
    #[serde(default = "super::defaults::default_reflection_min_chars")]
    pub min_user_chars: u32,
    #[serde(default = "super::defaults::default_reflection_cooldown")]
    pub cooldown_minutes: u32,
    #[serde(default)]
    pub open_loop_tracking: bool,
    #[serde(default)]
    pub open_loop_inject_prompt: bool,
}

impl Default for ReflectionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min_turns: super::defaults::default_reflection_min_turns(),
            min_user_chars: super::defaults::default_reflection_min_chars(),
            cooldown_minutes: super::defaults::default_reflection_cooldown(),
            open_loop_tracking: false,
            open_loop_inject_prompt: false,
        }
    }
}
