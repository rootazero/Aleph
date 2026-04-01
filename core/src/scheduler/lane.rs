use serde::{Deserialize, Serialize};

/// Execution lane for scheduling and resource isolation.
///
/// Lanes provide isolation and resource management for different types of runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    /// Main agent lane (user-initiated)
    Main,
    /// Sub-agent lane (delegated tasks)
    #[default]
    Subagent,
    /// Cron job lane (scheduled tasks)
    Cron,
    /// Nested sub-agent lane (sub-agents spawning sub-agents)
    Nested,
}

impl Lane {
    /// Get the default maximum concurrent runs for this lane
    pub fn default_max_concurrent(&self) -> usize {
        match self {
            Lane::Main => 2,
            Lane::Subagent => 8,
            Lane::Cron => 2,
            Lane::Nested => 4,
        }
    }

    /// Get the default priority for this lane
    ///
    /// Higher values indicate higher priority.
    pub fn default_priority(&self) -> u8 {
        match self {
            Lane::Main => 10,
            Lane::Nested => 8,
            Lane::Subagent => 5,
            Lane::Cron => 0,
        }
    }
}
