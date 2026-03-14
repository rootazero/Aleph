//! Codex harness adapter — oneshot mode.
//!
//! Codex CLI does not support ACP protocol. Instead, we use:
//! `codex exec "<prompt>"`
//!
//! This spawns a fresh process per prompt and returns plain text output.

use async_trait::async_trait;
use tokio::process::Command;
use tracing::{debug, error};

use crate::acp::harness::{AcpHarness, HarnessMode};
use crate::acp::session::HarnessConfig;
use crate::error::{AlephError, Result};

const DEFAULT_EXECUTABLE: &str = "codex";

/// ACP harness for Codex CLI (oneshot mode).
///
/// Each prompt spawns: `codex exec "<prompt>"`
/// The response is plain text on stdout.
pub struct CodexHarness {
    executable: String,
}

impl CodexHarness {
    pub fn new(executable: Option<String>) -> Self {
        Self {
            executable: executable.unwrap_or_else(|| DEFAULT_EXECUTABLE.to_string()),
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
        HarnessMode::Oneshot
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
}
