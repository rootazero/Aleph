//! Generic ACP harness — configuration-driven preset adapter.
//!
//! Covers ~90% of preset agents (Claude Code, Codex, Gemini, `OpenCode`, etc.)
//! that differ only in executable name, CLI args, and output format.

use std::time::Duration;

use async_trait::async_trait;

use crate::acp::adapter::{AcpAdapter, AdapterMode};
use crate::acp::output_format::OutputFormat;
use crate::acp::session::{AcpSession, AdapterConfig};
use crate::config::types::acp::AcpAdapterEntry;
use crate::error::Result;

/// Generic ACP harness built from configuration.
///
/// Covers agents that follow the standard pattern:
/// - Executable name from config
/// - Fixed args per mode (oneshot vs `native_acp`)
/// - Standard output parsing (plain text or JSON field extraction)
pub struct GenericAcpAdapter {
    id: String,
    display_name: String,
    executable: String,
    default_mode: AdapterMode,
    supported_modes: Vec<AdapterMode>,
    oneshot_args: Vec<String>,
    native_acp_args: Vec<String>,
    output_format: OutputFormat,
    timeout: Duration,
    trust_level: crate::config::types::acp::TrustLevel,
}

impl GenericAcpAdapter {
    /// Create from an `AcpAdapterEntry` config.
    #[must_use]
    pub fn from_entry(entry: &AcpAdapterEntry) -> Self {
        let id = entry.preset.clone().unwrap_or_default();
        let display_name = entry.display_name.clone();
        let executable = entry.executable.clone().unwrap_or_else(|| id.clone());
        let default_mode = AdapterMode::from_serde(&entry.default_mode);

        // All generic presets support both modes
        let supported_modes = vec![AdapterMode::Oneshot, AdapterMode::NativeAcp];

        Self {
            id,
            display_name,
            executable,
            default_mode,
            supported_modes,
            oneshot_args: entry.args.clone(),
            native_acp_args: vec!["--acp".to_string()],
            output_format: OutputFormat::from(&entry.output_format),
            timeout: Duration::from_secs(entry.timeout_seconds.max(1)),
            trust_level: entry.trust_level.clone(),
        }
    }

    /// Create from individual fields (for programmatic construction).
    pub fn new(
        id: impl Into<String>,
        display_name: impl Into<String>,
        executable: impl Into<String>,
        default_mode: AdapterMode,
        oneshot_args: Vec<String>,
        native_acp_args: Vec<String>,
        output_format: OutputFormat,
        trust_level: crate::config::types::acp::TrustLevel,
    ) -> Self {
        let id = id.into();
        let supported_modes = vec![AdapterMode::Oneshot, AdapterMode::NativeAcp];
        Self {
            id,
            display_name: display_name.into(),
            executable: executable.into(),
            default_mode,
            supported_modes,
            oneshot_args,
            native_acp_args,
            output_format,
            timeout: Duration::from_secs(300),
            trust_level,
        }
    }

    /// Resolve effective args based on mode.
    fn resolve_args(&self, mode: AdapterMode) -> Vec<String> {
        match mode {
            AdapterMode::Oneshot => self.oneshot_args.clone(),
            AdapterMode::NativeAcp => self.native_acp_args.clone(),
        }
    }
}

#[async_trait]
impl AcpAdapter for GenericAcpAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.display_name
    }

    fn mode(&self) -> AdapterMode {
        self.default_mode
    }

    fn supported_modes(&self) -> Vec<AdapterMode> {
        self.supported_modes.clone()
    }

    fn build_config(&self, cwd: Option<&str>) -> AdapterConfig {
        let permission_policy = match self.trust_level {
            crate::config::types::acp::TrustLevel::Full => {
                crate::acp::incoming::PermissionPolicy::ApproveAll
            }
            crate::config::types::acp::TrustLevel::Disabled => {
                crate::acp::incoming::PermissionPolicy::DenyAll
            }
        };
        AdapterConfig {
            executable: self.executable.clone(),
            args: self.resolve_args(self.default_mode),
            cwd: cwd.map(String::from),
            timeout: self.timeout.max(Duration::from_secs(1)),
            permission_policy,
            ..Default::default()
        }
    }

    async fn execute_oneshot(&self, prompt: &str, cwd: &str) -> Result<String> {
        super::run_oneshot_command(
            &self.id,
            &self.executable,
            &self.oneshot_args,
            &[],
            &self.output_format,
            prompt,
            cwd,
            self.timeout,
        )
        .await
    }

    async fn spawn_session(&self, cwd: Option<&str>) -> Result<AcpSession> {
        let timeout = self.timeout.max(Duration::from_secs(1));
        let permission_policy = match self.trust_level {
            crate::config::types::acp::TrustLevel::Full => {
                crate::acp::incoming::PermissionPolicy::ApproveAll
            }
            crate::config::types::acp::TrustLevel::Disabled => {
                crate::acp::incoming::PermissionPolicy::DenyAll
            }
        };
        let config = AdapterConfig {
            executable: self.executable.clone(),
            args: self.native_acp_args.clone(),
            cwd: cwd.map(String::from),
            timeout,
            permission_policy,
            ..Default::default()
        };
        let mut session = AcpSession::spawn(self.id(), &config).await?;
        session.initialize(timeout).await?;
        Ok(session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::acp::OutputFormatSerde;

    #[test]
    fn test_output_format_plain_text() {
        let fmt = OutputFormat::PlainText;
        assert_eq!(fmt.parse("  hello world  "), "hello world");
        assert_eq!(fmt.parse("no trim needed"), "no trim needed");
    }

    #[test]
    fn test_output_format_json_field() {
        let fmt = OutputFormat::JsonField {
            field: "result".to_string(),
        };
        let json = r#"{"result": "success", "other": 123}"#;
        assert_eq!(fmt.parse(json), "success");
    }

    #[test]
    fn test_output_format_json_field_missing() {
        let fmt = OutputFormat::JsonField {
            field: "missing".to_string(),
        };
        let json = r#"{"result": "success"}"#;
        // Falls back to full JSON string
        assert_eq!(fmt.parse(json), r#"{"result":"success"}"#);
    }

    #[test]
    fn test_output_format_invalid_json() {
        let fmt = OutputFormat::JsonField {
            field: "result".to_string(),
        };
        let text = "not json at all";
        // Falls back to raw text
        assert_eq!(fmt.parse(text), "not json at all");
    }

    #[test]
    fn test_generic_harness_build_config() {
        let harness = GenericAcpAdapter::new(
            "test",
            "Test",
            "test-exe",
            AdapterMode::Oneshot,
            vec!["exec".to_string()],
            vec!["--acp".to_string()],
            OutputFormat::PlainText,
            crate::config::types::acp::TrustLevel::Disabled,
        );

        let config = harness.build_config(Some("/tmp"));
        assert_eq!(config.executable, "test-exe");
        assert_eq!(config.args, vec!["exec"]);
        assert_eq!(config.cwd, Some("/tmp".to_string()));
    }

    #[test]
    fn test_generic_harness_build_config_native_acp() {
        let harness = GenericAcpAdapter::new(
            "test",
            "Test",
            "test-exe",
            AdapterMode::NativeAcp,
            vec!["exec".to_string()],
            vec!["--acp".to_string()],
            OutputFormat::PlainText,
            crate::config::types::acp::TrustLevel::Disabled,
        );

        let config = harness.build_config(None);
        assert_eq!(config.args, vec!["--acp"]);
    }

    #[test]
    fn test_generic_harness_from_entry() {
        let entry = AcpAdapterEntry {
            display_name: "My Tool".to_string(),
            executable: Some("my-tool".to_string()),
            args: vec!["run".to_string()],
            default_mode: crate::config::types::acp::AdapterModeSerde::Oneshot,
            output_format: OutputFormatSerde::PlainText,
            preset: Some("my-tool".to_string()),
            ..Default::default()
        };

        let harness = GenericAcpAdapter::from_entry(&entry);
        assert_eq!(harness.id(), "my-tool");
        assert_eq!(harness.display_name(), "My Tool");
        assert_eq!(harness.executable, "my-tool");
        assert_eq!(harness.oneshot_args, vec!["run"]);
        assert_eq!(harness.native_acp_args, vec!["--acp"]);
    }

    #[test]
    fn test_generic_harness_timeout_clamped_to_minimum() {
        let harness = GenericAcpAdapter::new(
            "test",
            "Test",
            "test-exe",
            AdapterMode::Oneshot,
            vec!["exec".to_string()],
            vec!["--acp".to_string()],
            OutputFormat::PlainText,
            crate::config::types::acp::TrustLevel::Disabled,
        );

        let config = harness.build_config(None);
        assert_eq!(config.timeout, std::time::Duration::from_secs(300));
    }
}
