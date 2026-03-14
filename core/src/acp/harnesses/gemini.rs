//! Gemini ACP harness adapter — native ACP over stdio.

use async_trait::async_trait;

use crate::acp::harness::{AcpHarness, HarnessMode};
use crate::acp::session::HarnessConfig;

const DEFAULT_EXECUTABLE: &str = "gemini";

/// ACP harness for Gemini CLI.
///
/// Uses native ACP protocol: `gemini --acp` starts a persistent NDJSON stdio session.
/// Protocol: initialize → session/new → session/prompt (streaming agent_message_chunk).
pub struct GeminiHarness {
    executable: String,
}

impl GeminiHarness {
    pub fn new(executable: Option<String>) -> Self {
        Self {
            executable: executable.unwrap_or_else(|| DEFAULT_EXECUTABLE.to_string()),
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
        HarnessMode::NativeAcp
    }

    fn build_config(&self, cwd: Option<&str>) -> HarnessConfig {
        HarnessConfig {
            executable: self.executable.clone(),
            args: vec!["--acp".to_string()],
            cwd: cwd.map(String::from),
            ..Default::default()
        }
    }
}
