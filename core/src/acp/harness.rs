//! AcpHarness trait — abstraction over external CLI tools that speak ACP.

use async_trait::async_trait;
use tokio::process::Command;
use tracing::debug;

use crate::acp::session::{AcpSession, HarnessConfig};
use crate::error::Result;

/// Trait for ACP-capable CLI harnesses (Claude Code, Codex, Gemini, etc.).
///
/// Each harness knows how to build a `HarnessConfig` for its CLI tool
/// and can spawn an initialized `AcpSession`.
#[async_trait]
pub trait AcpHarness: Send + Sync {
    /// Unique identifier (e.g. "claude-code", "codex", "gemini").
    fn id(&self) -> &str;

    /// Human-readable display name (e.g. "Claude Code", "Codex", "Gemini").
    fn display_name(&self) -> &str;

    /// Build the spawn configuration for this harness.
    fn build_config(&self, cwd: Option<&str>) -> HarnessConfig;

    /// Check whether the harness executable is available on the system.
    ///
    /// Default: runs `executable --version` and returns `true` on exit 0.
    async fn is_available(&self) -> bool {
        let config = self.build_config(None);
        match Command::new(&config.executable)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
        {
            Ok(status) => {
                let available = status.success();
                debug!(
                    harness = self.id(),
                    executable = %config.executable,
                    available,
                    "ACP harness availability check"
                );
                available
            }
            Err(_) => {
                debug!(
                    harness = self.id(),
                    executable = %config.executable,
                    "ACP harness not found"
                );
                false
            }
        }
    }

    /// Spawn and initialize an ACP session for this harness.
    ///
    /// Default: builds config via `build_config`, spawns via `AcpSession::spawn`,
    /// then calls `session.initialize`.
    async fn spawn_session(&self, cwd: Option<&str>) -> Result<AcpSession> {
        let config = self.build_config(cwd);
        let timeout = config.timeout;
        let mut session = AcpSession::spawn(self.id(), &config).await?;
        session.initialize(timeout).await?;
        Ok(session)
    }
}
