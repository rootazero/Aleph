//! Gemini ACP harness adapter — native ACP and oneshot modes.

use async_trait::async_trait;
use tracing::{debug, error};

use crate::acp::harness::{AcpHarness, HarnessMode};
use crate::acp::session::HarnessConfig;
use crate::error::Result;

const DEFAULT_EXECUTABLE: &str = "gemini";

/// ACP harness for Gemini CLI.
///
/// Uses native ACP protocol: `gemini --acp` starts a persistent NDJSON stdio session.
/// Protocol: initialize → session/new → session/prompt (streaming agent_message_chunk).
pub struct GeminiHarness {
    executable: String,
    default_mode: HarnessMode,
}

impl GeminiHarness {
    pub fn new(executable: Option<String>, default_mode: HarnessMode) -> Self {
        Self {
            executable: executable.unwrap_or_else(|| DEFAULT_EXECUTABLE.to_string()),
            default_mode,
        }
    }
}

#[async_trait]
impl AcpHarness for GeminiHarness {
    fn id(&self) -> &str {
        "gemini"
    }

    fn display_name(&self) -> &str {
        "Gemini"
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
            args: vec!["--acp".to_string()],
            cwd: cwd.map(String::from),
            ..Default::default()
        }
    }

    async fn execute_oneshot(&self, prompt: &str, cwd: &str) -> Result<String> {
        use tokio::process::Command;

        let mut cmd = Command::new(&self.executable);
        cmd.args(["-p", prompt])
            .current_dir(cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        debug!(harness = "gemini", "Spawning oneshot Gemini process");

        let output = cmd.output().await.map_err(|e| {
            crate::error::AlephError::tool(format!(
                "Failed to execute Gemini CLI: {}. Is 'gemini' installed and in PATH?",
                e
            ))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(harness = "gemini", stderr = %stderr, "Gemini CLI failed");
            return Err(crate::error::AlephError::tool(format!(
                "Gemini CLI exited with {}: {}",
                output.status,
                stderr.chars().take(500).collect::<String>()
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}
