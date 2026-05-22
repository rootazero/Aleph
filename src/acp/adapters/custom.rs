//! Custom ACP adapter — user-defined CLI tools as ACP adapters.
//!
//! Built from `AcpAdapterEntry` config, allowing arbitrary executables
//! to participate as ACP harnesses via configuration alone.

use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;
use tracing::{debug, error};

use crate::acp::adapter::{AcpAdapter, AdapterMode};
use crate::acp::output_format::OutputFormat;
use crate::acp::session::AdapterConfig;
use crate::config::types::acp::AcpAdapterEntry;
use crate::error::{AlephError, Result};

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
    pub fn new(harness_id: String, config: AcpAdapterEntry) -> Self {
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

        AdapterConfig {
            executable: self.executable().to_string(),
            args: self.config.args.clone(),
            cwd: cwd.map(String::from).or_else(|| self.config.cwd.clone()),
            env,
            timeout: Duration::from_secs(self.config.timeout_seconds.max(1)),
        }
    }

    async fn execute_oneshot(&self, prompt: &str, cwd: &str) -> Result<String> {
        let executable = self.executable();

        let mut cmd = Command::new(executable);
        cmd.args(&self.config.args)
            .arg(prompt)
            .current_dir(cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        // Set configured environment variables
        for (key, val) in &self.config.env {
            cmd.env(key, val);
        }

        debug!(
            harness = %self.harness_id,
            executable,
            "Spawning oneshot custom harness process"
        );

        let timeout = Duration::from_secs(self.config.timeout_seconds);
        let output = tokio::time::timeout(timeout, cmd.output())
            .await
            .map_err(|_| {
                AlephError::tool(format!(
                    "Custom harness '{}' timed out after {}s",
                    self.harness_id, self.config.timeout_seconds
                ))
            })?
            .map_err(|e| {
                AlephError::tool(format!(
                    "Failed to execute custom harness '{}' (executable: '{}'): {}. \
                     Is the executable installed and in PATH?",
                    self.harness_id, executable, e
                ))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(
                harness = %self.harness_id,
                stderr = %stderr,
                "Custom harness process failed"
            );
            return Err(AlephError::tool(format!(
                "Custom harness '{}' exited with {}: {}",
                self.harness_id,
                output.status,
                stderr.chars().take(500).collect::<String>()
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(OutputFormat::from(&self.config.output_format).parse(&stdout))
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
