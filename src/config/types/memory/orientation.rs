use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// CUT: `log_rotate_lines` and `inject_on_agent_switch` used to live here and
// had zero readers anywhere in the tree. Log rotation is real but hardcoded at
// `memory::notes::orientation::log_md::LOG_ROTATE_LINES`, and `LogMdWriter`
// takes no config, so the knob could not move it; agent-switch re-injection was
// never implemented at all. A config surface must not advertise a switch that
// cannot work (R10) — re-adding either means threading it to a consumer first.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OrientationConfig {
    /// Auto-inject the wiki orientation envelope into prompts. `false` builds
    /// the `MemoryContextProvider` without an orientation handle, so nothing is
    /// assembled; the wiki itself keeps serving the dream daemon and the
    /// on-demand `note_orient` tool.
    #[serde(default = "super::defaults::default_orientation_enabled")]
    pub enabled: bool,
    /// Token budget for one orientation snapshot. Spent by the injected
    /// envelope *and* used as `note_orient`'s default when the model omits
    /// `max_tokens` — that argument's schema doc promises exactly this value.
    #[serde(default = "super::defaults::default_orientation_max_tokens")]
    pub max_tokens: usize,
}

impl Default for OrientationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_tokens: 4000,
        }
    }
}
