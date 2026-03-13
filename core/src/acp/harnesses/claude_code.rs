//! Claude Code ACP harness adapter.

use async_trait::async_trait;

use crate::acp::harness::AcpHarness;
use crate::acp::session::HarnessConfig;

const DEFAULT_EXECUTABLE: &str = "claude";

/// ACP harness for Claude Code CLI.
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

    fn build_config(&self, cwd: Option<&str>) -> HarnessConfig {
        HarnessConfig {
            executable: self.executable.clone(),
            args: vec!["--acp".to_string()],
            cwd: cwd.map(String::from),
            ..Default::default()
        }
    }
}
