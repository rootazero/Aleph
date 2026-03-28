//! Claude Code harness adapter — oneshot and native ACP modes.
//!
//! Oneshot mode: `claude --print --output-format json -p <prompt>`
//! Native ACP mode: `claude --acp` (persistent stdio session)
//!
//! Default mode is Oneshot; NativeAcp can be selected via config.

use async_trait::async_trait;
use tokio::process::Command;
use tracing::{debug, error};

use crate::acp::harness::{AcpHarness, HarnessMode};
use crate::acp::session::{AcpSession, HarnessConfig};
use crate::error::{AlephError, Result};

const DEFAULT_EXECUTABLE: &str = "claude";

/// ACP harness for Claude Code CLI (oneshot and native ACP modes).
///
/// Oneshot: spawns `claude --print --output-format json -p "<prompt>"` per request.
/// NativeAcp: spawns `claude --acp` as a persistent stdio session.
pub struct ClaudeCodeHarness {
    executable: String,
    default_mode: HarnessMode,
}

impl ClaudeCodeHarness {
    pub fn new(executable: Option<String>, default_mode: HarnessMode) -> Self {
        Self {
            executable: executable.unwrap_or_else(|| DEFAULT_EXECUTABLE.to_string()),
            default_mode,
        }
    }
}

#[async_trait]
impl AcpHarness for ClaudeCodeHarness {
    fn id(&self) -> &str {
        "claude-code"
    }

    fn display_name(&self) -> &str {
        "Claude Code"
    }

    fn mode(&self) -> HarnessMode {
        self.default_mode
    }

    fn supported_modes(&self) -> Vec<HarnessMode> {
        vec![HarnessMode::Oneshot, HarnessMode::NativeAcp]
    }

    fn build_config(&self, cwd: Option<&str>) -> HarnessConfig {
        // Args depend on mode: --acp for NativeAcp, --print for Oneshot
        let args = match self.default_mode {
            HarnessMode::NativeAcp => vec!["--acp".to_string()],
            HarnessMode::Oneshot => vec![
                "--print".to_string(),
                "--output-format".to_string(),
                "json".to_string(),
            ],
        };
        HarnessConfig {
            executable: self.executable.clone(),
            args,
            cwd: cwd.map(String::from),
            ..Default::default()
        }
    }

    async fn execute_oneshot(&self, prompt: &str, cwd: &str) -> Result<String> {
        let mut cmd = Command::new(&self.executable);
        cmd.args(["--print", "--output-format", "json", "-p", prompt])
            .current_dir(cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        debug!(harness = "claude-code", "Spawning oneshot Claude Code process");

        let output = cmd.output().await.map_err(|e| {
            AlephError::tool(format!(
                "Failed to execute Claude Code CLI: {}. Is 'claude' installed and in PATH?",
                e
            ))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(harness = "claude-code", stderr = %stderr, "Claude Code CLI failed");
            return Err(AlephError::tool(format!(
                "Claude Code CLI exited with {}: {}",
                output.status,
                stderr.chars().take(500).collect::<String>()
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Try to parse as JSON and extract the result text
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
            // Claude --output-format json returns: {"type":"result","result":"<text>","...}
            if let Some(result) = json.get("result").and_then(|r| r.as_str()) {
                return Ok(result.to_string());
            }
            // Fallback: return the full JSON as string
            return Ok(json.to_string());
        }

        // Not JSON — return raw text
        Ok(stdout.to_string())
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
