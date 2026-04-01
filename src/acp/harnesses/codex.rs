//! Codex harness adapter — oneshot and native ACP modes.
//!
//! Oneshot mode: `codex exec "<prompt>"`
//! Native ACP mode: `codex --acp` (persistent stdio session)
//!
//! Default mode is Oneshot; NativeAcp can be selected via config.

use async_trait::async_trait;
use tokio::process::Command;
use tracing::{debug, error};

use crate::acp::harness::{AcpHarness, HarnessMode};
use crate::acp::session::{AcpSession, HarnessConfig};
use crate::error::{AlephError, Result};

const DEFAULT_EXECUTABLE: &str = "codex";

/// ACP harness for Codex CLI (oneshot and native ACP modes).
///
/// Oneshot: spawns `codex exec "<prompt>"` per request.
/// NativeAcp: spawns `codex --acp` as a persistent stdio session.
pub struct CodexHarness {
    executable: String,
    default_mode: HarnessMode,
}

impl CodexHarness {
    pub fn new(executable: Option<String>, default_mode: HarnessMode) -> Self {
        Self {
            executable: executable.unwrap_or_else(|| DEFAULT_EXECUTABLE.to_string()),
            default_mode,
        }
    }
}

#[async_trait]
impl AcpHarness for CodexHarness {
    fn id(&self) -> &str {
        "codex"
    }

    fn display_name(&self) -> &str {
        "Codex"
    }

    fn mode(&self) -> HarnessMode {
        self.default_mode
    }

    fn supported_modes(&self) -> Vec<HarnessMode> {
        vec![HarnessMode::Oneshot, HarnessMode::NativeAcp]
    }

    fn build_config(&self, cwd: Option<&str>) -> HarnessConfig {
        HarnessConfig {
            executable: self.executable.clone(),
            args: vec!["exec".to_string()],
            cwd: cwd.map(String::from),
            ..Default::default()
        }
    }

    async fn execute_oneshot(&self, prompt: &str, cwd: &str) -> Result<String> {
        let mut cmd = Command::new(&self.executable);
        cmd.args(["exec", prompt])
            .current_dir(cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        debug!(harness = "codex", "Spawning oneshot Codex process");

        let output = cmd.output().await.map_err(|e| {
            AlephError::tool(format!(
                "Failed to execute Codex CLI: {}. Is 'codex' installed and in PATH?",
                e
            ))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(harness = "codex", stderr = %stderr, "Codex CLI failed");
            return Err(AlephError::tool(format!(
                "Codex CLI exited with {}: {}",
                output.status,
                stderr.chars().take(500).collect::<String>()
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.trim().to_string())
    }

    async fn spawn_session(&self, cwd: Option<&str>) -> Result<AcpSession> {
        let config = HarnessConfig {
            executable: self.executable.clone(),
            args: vec!["--acp".to_string()],
            cwd: cwd.map(String::from),
            ..Default::default()
        };
        let timeout = config.timeout;
        let mut session = AcpSession::spawn(self.id(), &config).await?;
        session.initialize(timeout).await?;
        Ok(session)
    }
}
