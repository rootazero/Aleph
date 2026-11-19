//! `browser/ignored-fields` — profile settings the running engine cannot honour.
//!
//! A profile may carry a setting its engine has no way to apply — today
//! `browser = "brave"` on a profile that runs obscura. The setting parses, is
//! stored, and is then never read: the operator believes it did something,
//! which is the silent no-op shape (判据 §11). Spec §6.3 says obscura
//! 「被忽略并在 doctor 记一条」 — the browser subsystem computes that list in
//! `BrowserSystemConfig::ignored_fields`, and this check is the doctor half
//! that renders it. The list existed and was rendered by nobody: the promise
//! in its own doc named a check that did not exist.
//!
//! The check REPORTS and never REFUSES: a profile carrying a leftover key
//! still starts. `BrowserSystemConfig::validate` owns the refusals.

use std::path::PathBuf;

use async_trait::async_trait;

use crate::config::Config;
use crate::diagnostics::check::{HealthCheck, Posture, Presence};
use crate::diagnostics::finding::{Finding, Severity};

const ID: &str = "browser/ignored-fields";

/// Reports the browser profile settings the running engine ignores.
///
/// The config file is the same one the daemon loads (`Config::effective_path`),
/// so the finding describes the settings that are actually in effect rather
/// than a file nobody reads.
pub struct BrowserIgnoredFieldsCheck {
    config_path: PathBuf,
}

impl BrowserIgnoredFieldsCheck {
    #[must_use]
    pub const fn new(config_path: PathBuf) -> Self {
        Self { config_path }
    }
}

#[async_trait]
impl HealthCheck for BrowserIgnoredFieldsCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        "Browser profile settings"
    }

    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        let display = self.config_path.display().to_string();

        match Presence::of(ID, "Config file state", &self.config_path) {
            Err(f) => return vec![f],
            Ok(Presence::Absent) => {
                return vec![Finding::ok(
                    ID,
                    "No browser profile settings to check",
                    format!(
                        "{display} not present; the built-in defaults are in effect, and no \
                         default is ignored by an engine."
                    ),
                )]
            }
            Ok(Presence::Present) => {}
        }

        let config = match Config::load_from_file(&self.config_path) {
            Ok(config) => config,
            Err(_) => {
                // The file is there but could not be turned into a config.
                // `core/config-parse` owns that finding; HERE the browser
                // settings are unknown, and unknown must never render as
                // "nothing is ignored" (判据 §8).
                return vec![Finding::problem(
                    ID,
                    Severity::Warning,
                    "Browser profile settings unknown",
                    format!(
                        "{display} could not be read as a config, so whether a profile setting \
                         is ignored could not be determined."
                    ),
                )
                .with_fix_hint(
                    "Fix the config file — `aleph doctor` reports why it does not parse under \
                     `core/config-parse` — then re-run the doctor.",
                )];
            }
        };

        let ignored = config.general.browser.ignored_fields();
        if ignored.is_empty() {
            return vec![Finding::ok(
                ID,
                "Every browser profile setting is in effect",
                format!("No profile in {display} carries a setting its engine ignores."),
            )];
        }

        ignored
            .into_iter()
            .map(|(profile, key, explanation)| {
                Finding::problem(
                    ID,
                    Severity::Warning,
                    format!("Profile \"{profile}\" ignores \"{key}\""),
                    explanation,
                )
                .with_fix_hint(format!(
                    "Edit the profile \"{profile}\" in {display}, or delete the key."
                ))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    async fn run_on(contents: &str) -> Vec<Finding> {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, contents).unwrap();
        let check = BrowserIgnoredFieldsCheck::new(path);
        check.run(Posture::Inspect).await
    }

    #[tokio::test]
    async fn ok_when_the_config_file_is_absent() {
        let tmp = tempdir().unwrap();
        let check = BrowserIgnoredFieldsCheck::new(tmp.path().join("config.toml"));
        let findings = check.run(Posture::Inspect).await;
        assert!(
            !findings[0].is_problem(),
            "a missing config means the defaults, and the defaults ignore nothing: {}",
            findings[0].detail
        );
    }

    /// The class this check exists for: the operator names a Chromium-family
    /// browser on a profile that runs obscura, and nothing happens.
    #[tokio::test]
    async fn a_browser_key_on_an_obscura_profile_is_reported() {
        let findings =
            run_on("[general.browser.profiles.work]\nengine = \"obscura\"\nbrowser = \"brave\"\n")
                .await;
        let problem = findings
            .iter()
            .find(|f| f.is_problem())
            .expect("the ignored key must be reported");
        assert_eq!(problem.severity, Severity::Warning);
        assert!(
            problem.title.contains("work") && problem.title.contains("browser"),
            "the finding must name the profile and the key, got: {}",
            problem.title
        );
        assert!(
            problem.fix_hint.is_some(),
            "an operator needs somewhere to go"
        );
    }

    /// The same key on a profile that runs chromium is honoured, so reporting
    /// it would be a false alarm.
    #[tokio::test]
    async fn the_same_key_on_a_chromium_profile_is_not_reported() {
        let findings =
            run_on("[general.browser.profiles.work]\nengine = \"chromium\"\nbrowser = \"brave\"\n")
                .await;
        assert!(
            findings.iter().all(|f| !f.is_problem()),
            "a honoured key must not be reported: {:?}",
            findings
        );
    }

    /// A file that is there but does not parse means the settings are
    /// unknown — not "nothing is ignored".
    #[tokio::test]
    async fn an_unreadable_config_is_unknown_rather_than_clean() {
        let findings = run_on("this is = not valid toml [[[").await;
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Warning);
        assert!(
            findings[0].title.contains("unknown"),
            "the finding must say the settings are unknown, got: {}",
            findings[0].title
        );
    }
}
