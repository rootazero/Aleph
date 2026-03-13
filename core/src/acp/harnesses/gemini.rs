//! Gemini ACP harness adapter.

use async_trait::async_trait;

use crate::acp::harness::AcpHarness;
use crate::acp::session::HarnessConfig;

const DEFAULT_EXECUTABLE: &str = "gemini";

/// ACP harness for Gemini CLI.
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

    fn build_config(&self, cwd: Option<&str>) -> HarnessConfig {
        HarnessConfig {
            executable: self.executable.clone(),
            args: vec!["--acp".to_string()],
            cwd: cwd.map(String::from),
            ..Default::default()
        }
    }
}
