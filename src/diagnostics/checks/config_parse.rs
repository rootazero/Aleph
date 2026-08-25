//! `core/config-parse` — `config.toml` must parse **and be in effect** if present.
//!
//! A malformed `config.toml` makes the daemon fall back to defaults (or
//! refuse to start), silently dropping the operator's settings. Doctor
//! surfaces the parse error early. No auto-repair: editing config is the
//! `self_config` / `self_manage` domain (LLM-driven), so doctor only points
//! there rather than guessing a fix.
//!
//! "Parses" alone is close to a truism relative to the question the operator
//! is asking. `Config` does not `deny_unknown_fields`, so `[browser]` written
//! where `[general.browser]` was meant parses, saves, and is read by nothing —
//! and this check used to certify that file healthy. It now also reports the
//! key paths that reached no code, from the same scan
//! `Config::load_from_file` warns about (see `config::dead_keys`);
//! a Warning rather than an Error, because tolerating unknown keys is the
//! recorded stance and an old file must still boot.

use std::path::PathBuf;

use async_trait::async_trait;

use crate::config::Config;
use crate::diagnostics::check::{HealthCheck, Posture, Presence};
use crate::diagnostics::finding::{Finding, Severity};

const ID: &str = "core/config-parse";

/// How many dead paths the finding spells out before summarising the rest.
/// A config with a hundred of them is one mistake repeated, and a hundred-item
/// line is unreadable in the CLI renderer, the `--json` bundle and the LLM
/// tool result alike.
const MAX_LISTED_PATHS: usize = 10;

fn render_paths(dead: &[String]) -> String {
    if dead.len() <= MAX_LISTED_PATHS {
        return dead.join(", ");
    }
    format!(
        "{}, and {} more",
        dead[..MAX_LISTED_PATHS].join(", "),
        dead.len() - MAX_LISTED_PATHS
    )
}

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

        // "Not present; defaults are in effect" is the RIGHT SYMPTOM WITH
        // THE WRONG CAUSE when the file is sitting right there and cannot be
        // read: the daemon also falls back to defaults in that case, so the
        // operator is sent looking for a file that exists.
        match Presence::of(ID, "Config file state", &self.config_path) {
            Err(f) => return vec![f],
            Ok(Presence::Absent) => {
                return vec![Finding::ok(
                    ID,
                    "Using default config",
                    format!("{display} not present; built-in defaults are in effect."),
                )]
            }
            Ok(Presence::Present) => {}
        }

        match Config::load_from_file_reporting_dead_keys(&self.config_path) {
            Ok((_, dead)) if dead.is_empty() => vec![Finding::ok(
                ID,
                "Config parses",
                // Not "every key is in effect": channel table interiors are
                // opaque JSON, so this scan cannot see inside them. Claim
                // exactly what was measured.
                format!("{display} parsed successfully; no key in it was discarded."),
            )],
            Ok((_, dead)) => vec![Finding::problem(
                ID,
                Severity::Warning,
                "Config has keys nothing reads",
                format!(
                    "{display} parsed, but nothing reads these key paths, so they set nothing: \
                     {list}.",
                    list = render_paths(&dead),
                ),
            )
            .with_fix_hint(
                "Move each key under the section that owns it (a common one: browser settings \
                 live under [general.browser], not [browser]), or delete it. Aleph keeps loading \
                 either way — unknown keys are tolerated so an older config still boots.",
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

    async fn run_on(contents: &str) -> Finding {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, contents).unwrap();
        let check = ConfigParseCheck::new(path);
        check.run(Posture::Inspect).await.remove(0)
    }

    #[tokio::test]
    async fn a_config_whose_keys_are_all_read_is_ok() {
        let finding = run_on("[general.browser.profiles.work]\nheadless = true\n").await;
        assert!(
            !finding.is_problem(),
            "clean config must stay OK, got: {} / {}",
            finding.title,
            finding.detail
        );
    }

    /// The class this check exists for: `[browser]` is what an operator writes
    /// and `[general.browser]` is what the code reads, so the file parses and
    /// the whole block does nothing.
    #[tokio::test]
    async fn a_misplaced_section_is_a_warning_naming_the_dead_path() {
        let finding = run_on("[browser.profiles.work]\nheadless = true\n").await;
        assert_eq!(finding.severity, Severity::Warning);
        assert!(
            finding.detail.contains("browser"),
            "the finding must name the dead path, got: {}",
            finding.detail
        );
        assert!(
            finding.fix_hint.is_some(),
            "an operator needs somewhere to go"
        );
    }

    /// A parse failure still outranks a dead key: the file is not in effect at
    /// all, so reporting which of its keys are inert would be beside the point.
    #[tokio::test]
    async fn malformed_toml_still_reports_as_an_error() {
        let finding = run_on("[browser]\nheadless = \n").await;
        assert_eq!(finding.severity, Severity::Error);
    }

    /// `[gateway]` is read by `GatewayConfig` out of this same file, and
    /// `[security.ssrf]` by the raw-TOML bridge in `config::load`. Both are
    /// invisible to `Config`'s schema, so an incomplete allowlist would make
    /// this check cry wolf on every configured deployment — which is worse
    /// than the silence it replaces.
    #[tokio::test]
    async fn foreign_owned_sections_do_not_trip_the_warning() {
        let finding = run_on(
            r#"
[gateway]
host = "127.0.0.1"
port = 18790

[gateway.auth]
mode = "token"

[security.ssrf]
allowed_hosts = ["api.example.com"]
"#,
        )
        .await;
        assert!(
            !finding.is_problem(),
            "live config owned by another parser must not be reported, got: {} / {}",
            finding.title,
            finding.detail
        );
    }

    /// Retired knobs are inert but deliberately still parse, so an old file
    /// must not be reported as broken.
    #[tokio::test]
    async fn a_retired_key_does_not_trip_the_warning() {
        let finding = run_on("[desktop.presence]\nenabled = true\n").await;
        assert!(
            !finding.is_problem(),
            "retired keys are tolerated on purpose, got: {}",
            finding.detail
        );
    }

    #[test]
    fn a_long_dead_key_list_is_summarised_rather_than_dumped() {
        let few: Vec<String> = (0..MAX_LISTED_PATHS).map(|i| format!("k{i}")).collect();
        assert_eq!(render_paths(&few), few.join(", "));

        let many: Vec<String> = (0..MAX_LISTED_PATHS + 3).map(|i| format!("k{i}")).collect();
        let rendered = render_paths(&many);
        assert!(rendered.ends_with(", and 3 more"), "got: {rendered}");
        assert!(
            !rendered.contains(&format!("k{}", MAX_LISTED_PATHS)),
            "the summarised tail must not still be listed: {rendered}"
        );
    }

    /// The right symptom with the wrong cause. A config file that exists but
    /// cannot be read ALSO makes the daemon fall back to defaults — so
    /// "not present; built-in defaults are in effect" describes what the
    /// operator is experiencing and sends them looking for a missing file
    /// that is sitting right there.
    #[tokio::test]
    async fn an_unreadable_config_file_is_not_reported_as_not_present() {
        let findings = ConfigParseCheck::new(PathBuf::from("aleph\u{0}config.toml"))
            .run(Posture::Inspect)
            .await;
        assert_eq!(findings.len(), 1);
        assert!(findings[0].is_problem(), "{:?}", findings[0]);
        assert_ne!(findings[0].title, "Using default config");
    }
}
