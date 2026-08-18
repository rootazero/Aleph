//! Custom ACP adapter — user-defined CLI tools as ACP adapters.
//!
//! Built from `AcpAdapterEntry` config, allowing arbitrary executables
//! to participate as ACP harnesses via configuration alone.

use std::time::Duration;

use async_trait::async_trait;

use crate::acp::adapter::{AcpAdapter, AdapterMode};
use crate::acp::session::AdapterConfig;
use crate::config::types::acp::AcpAdapterEntry;
use crate::error::Result;

/// ACP harness built from user-provided `AcpAdapterEntry` configuration.
///
/// This enables arbitrary CLI tools to act as ACP harnesses without
/// writing a dedicated Rust adapter — just configure and go.
pub struct CustomAcpAdapter {
    harness_id: String,
    config: AcpAdapterEntry,
}

impl CustomAcpAdapter {
    /// Create a new custom harness from config.
    #[must_use]
    pub const fn new(harness_id: String, config: AcpAdapterEntry) -> Self {
        Self { harness_id, config }
    }

    /// Resolve the executable: explicit config value or fall back to harness ID.
    fn executable(&self) -> &str {
        self.config
            .executable
            .as_deref()
            .unwrap_or(&self.harness_id)
    }
}

#[async_trait]
impl AcpAdapter for CustomAcpAdapter {
    fn id(&self) -> &str {
        &self.harness_id
    }

    fn display_name(&self) -> &str {
        &self.config.display_name
    }

    fn mode(&self) -> AdapterMode {
        AdapterMode::from_serde(&self.config.default_mode)
    }

    fn build_config(&self, cwd: Option<&str>) -> AdapterConfig {
        let env: Vec<(String, String)> = self
            .config
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        // Map trust_level -> PermissionPolicy. `Full` trusts the harness
        // fully; `Disabled` denies all filesystem access. See ACP-07.
        let permission_policy = match self.config.trust_level {
            crate::config::types::acp::TrustLevel::Full => {
                crate::acp::incoming::PermissionPolicy::ApproveAll
            }
            crate::config::types::acp::TrustLevel::Disabled => {
                crate::acp::incoming::PermissionPolicy::DenyAll
            }
        };

        AdapterConfig {
            executable: self.executable().to_string(),
            args: self.config.args.clone(),
            cwd: cwd.map(String::from).or_else(|| self.config.cwd.clone()),
            env,
            timeout: Duration::from_secs(self.config.timeout_seconds.max(1)),
            permission_policy,
        }
    }

    async fn execute_oneshot(&self, prompt: &str, cwd: &str) -> Result<String> {
        let env: Vec<(String, String)> = self
            .config
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        super::run_oneshot_command(
            &self.harness_id,
            self.executable(),
            &self.config.args,
            &env,
            &crate::acp::output_format::OutputFormat::from(&self.config.output_format),
            prompt,
            cwd,
            Duration::from_secs(self.config.timeout_seconds.max(1)),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_custom_harness_timeout_clamped_to_minimum() {
        let entry = AcpAdapterEntry {
            display_name: "Test".to_string(),
            executable: Some("test".to_string()),
            timeout_seconds: 0,
            ..Default::default()
        };
        let adapter = CustomAcpAdapter::new("test".to_string(), entry);
        let config = adapter.build_config(None);
        assert_eq!(config.timeout, std::time::Duration::from_secs(1));
    }

    #[test]
    fn test_custom_harness_timeout_respects_valid_value() {
        let entry = AcpAdapterEntry {
            display_name: "Test".to_string(),
            executable: Some("test".to_string()),
            timeout_seconds: 60,
            ..Default::default()
        };
        let adapter = CustomAcpAdapter::new("test".to_string(), entry);
        let config = adapter.build_config(None);
        assert_eq!(config.timeout, std::time::Duration::from_secs(60));
    }
}
