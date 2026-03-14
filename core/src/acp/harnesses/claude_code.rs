//! Claude Code harness adapter — oneshot mode.
//!
//! Claude Code CLI does not support ACP protocol. Instead, we use:
//! `claude --print --output-format json -p <prompt>`
//!
//! This spawns a fresh process per prompt and returns structured JSON output.

use async_trait::async_trait;
use tokio::process::Command;
use tracing::{debug, error};

use crate::acp::harness::{AcpHarness, HarnessMode};
use crate::acp::session::HarnessConfig;
use crate::error::{AlephError, Result};

const DEFAULT_EXECUTABLE: &str = "claude";

/// ACP harness for Claude Code CLI (oneshot mode).
///
/// Each prompt spawns: `claude --print --output-format json -p "<prompt>"`
/// The response is a JSON object with a `result` field containing the text.
pub struct ClaudeCodeHarness {
    executable: String,
}

impl ClaudeCodeHarness {
    pub fn new(executable: Option<String>) -> Self {
        Self {
            executable: executable.unwrap_or_else(|| DEFAULT_EXECUTABLE.to_string()),
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
        HarnessMode::Oneshot
    }

    fn build_config(&self, cwd: Option<&str>) -> HarnessConfig {
        HarnessConfig {
            executable: self.executable.clone(),
            args: vec![
                "--print".to_string(),
                "--output-format".to_string(),
                "json".to_string(),
            ],
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
}
