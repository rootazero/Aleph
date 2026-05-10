use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OrientationConfig {
    #[serde(default = "super::defaults::default_orientation_enabled")]
    pub enabled: bool,
    #[serde(default = "super::defaults::default_orientation_max_tokens")]
    pub max_tokens: usize,
    #[serde(default = "super::defaults::default_orientation_log_rotate_lines")]
    pub log_rotate_lines: usize,
    #[serde(default = "super::defaults::default_orientation_inject_on_agent_switch")]
    pub inject_on_agent_switch: bool,
}

impl Default for OrientationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_tokens: 4000,
            log_rotate_lines: 2000,
            inject_on_agent_switch: true,
        }
    }
}
