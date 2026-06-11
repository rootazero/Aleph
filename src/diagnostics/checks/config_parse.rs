//! `core/config-parse` — `config.toml` must parse if present.
//!
//! A malformed `config.toml` makes the daemon fall back to defaults (or
//! refuse to start), silently dropping the operator's settings. Doctor
//! surfaces the parse error early. No auto-repair: editing config is the
//! `self_config` / `self_manage` domain (LLM-driven), so doctor only points
//! there rather than guessing a fix.

use std::path::PathBuf;

use async_trait::async_trait;

use crate::config::Config;
use crate::diagnostics::check::{HealthCheck, Posture};
use crate::diagnostics::finding::{Finding, Severity};

const ID: &str = "core/config-parse";

pub struct ConfigParseCheck {
    config_path: PathBuf,
}

impl ConfigParseCheck {
    #[must_use]
    pub const fn new(config_path: PathBuf) -> Self {
        Self { config_path }
    }
}

#[async_trait]
impl HealthCheck for ConfigParseCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        "Config file"
    }

    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        let display = self.config_path.display().to_string();

        if !self.config_path.exists() {
            return vec![Finding::ok(
                ID,
                "Using default config",
                format!("{display} not present; built-in defaults are in effect."),
            )];
        }

        match Config::load_from_file(&self.config_path) {
            Ok(_) => vec![Finding::ok(
                ID,
                "Config parses",
                format!("{display} parsed successfully."),
            )],
            Err(e) => vec![Finding::problem(
                ID,
                Severity::Error,
                "Config does not parse",
                format!("{display} failed to load: {e}"),
            )
            .with_fix_hint(
                "Fix the TOML syntax, or ask Aleph to repair it via self-management (it owns config edits).",
            )],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn ok_when_file_absent() {
        let tmp = tempdir().unwrap();
        let check = ConfigParseCheck::new(tmp.path().join("config.toml"));
        let findings = check.run(Posture::Inspect).await;
        assert!(!findings[0].is_problem());
    }

    #[tokio::test]
    async fn errors_on_malformed_toml() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, "this is = not valid toml [[[").unwrap();
        let check = ConfigParseCheck::new(path);
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings[0].severity, Severity::Error);
    }
}
