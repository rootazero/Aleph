//! Rig Agent Manager - core entry point

use super::config::RigAgentConfig;

/// Manages the rig Agent lifecycle
pub struct RigAgentManager {
    config: RigAgentConfig,
}

impl RigAgentManager {
    /// Create a new RigAgentManager
    pub fn new(config: RigAgentConfig) -> Self {
        Self { config }
    }

    /// Get the current configuration
    pub fn config(&self) -> &RigAgentConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manager_creation() {
        let config = RigAgentConfig::default();
        let manager = RigAgentManager::new(config);
        assert_eq!(manager.config().provider, "openai");
    }
}
