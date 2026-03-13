//! Codex ACP harness adapter.

use async_trait::async_trait;

use crate::acp::harness::AcpHarness;
use crate::acp::session::HarnessConfig;

const DEFAULT_EXECUTABLE: &str = "codex";

/// ACP harness for Codex CLI.
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

    fn build_config(&self, cwd: Option<&str>) -> HarnessConfig {
        HarnessConfig {
            executable: self.executable.clone(),
            args: vec!["--acp".to_string()],
            cwd: cwd.map(String::from),
            ..Default::default()
        }
    }
}
