//! Status reporting — rich, serializable view of skill status for Panel UI, CLI, and LLM Tools.

use serde::{Deserialize, Serialize};

use crate::domain::skill::{InstallKind, PromptScope, SkillId, SkillManifest, SkillSource};
use crate::domain::Entity;
use crate::skill::config::SkillEntryConfig;
use crate::skill::eligibility::{EligibilityResult, IneligibilityReason};
use crate::skill::installer::filter_install_specs_for_current_os;
use crate::skill::usage::UsageStats;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallOption {
    pub id: String,
    pub kind: InstallKind,
    pub label: String,
    pub bins: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MissingRequirements {
    pub bins: Vec<String>,
    pub env: Vec<String>,
    pub config: Vec<String>,
    /// OSes the skill declares support for but the current platform isn't in.
    /// `non-empty` means `eligible: false` purely due to OS mismatch (no
    /// install path can fix it — distinct from `bins` / `env` / `config`,
    /// which the user can resolve).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub os: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillStatusFilter {
    All,
    Ready,
    NeedsSetup,
    Disabled,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillStatusEntry {
    pub id: SkillId,
    pub name: String,
    pub description: String,
    pub emoji: Option<String>,
    pub source: SkillSource,
    /// Human-readable source label for UI display (e.g. "Bundled", "Global", "Plugin")
    pub source_label: String,
    pub homepage: Option<String>,
    pub eligible: bool,
    pub disabled: bool,
    pub missing: MissingRequirements,
    pub install_options: Vec<InstallOption>,
    pub primary_env: Option<String>,
    pub api_key_set: bool,
    pub scope: PromptScope,
    pub user_invocable: bool,
    /// Per-skill activity telemetry from `.usage.json`. `None` when the
    /// sidecar has no row for this skill (e.g. brand-new install that's
    /// never been read, or a bundled skill that's never been touched).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageStats>,
}

impl SkillStatusEntry {
    #[must_use]
    pub fn build(
        manifest: &SkillManifest,
        eligibility: &EligibilityResult,
        entry_config: Option<&SkillEntryConfig>,
        api_key_set: bool,
        usage: Option<UsageStats>,
    ) -> Self {
        let disabled = entry_config.and_then(|c| c.enabled).is_some_and(|e| !e);

        let scope = entry_config
            .and_then(|c| c.scope_override.clone())
            .unwrap_or_else(|| manifest.scope().clone());

        let mut missing = MissingRequirements::default();
        let eligible = match eligibility {
            EligibilityResult::Eligible => true,
            EligibilityResult::Ineligible(reasons) => {
                for reason in reasons {
                    match reason {
                        IneligibilityReason::MissingBinary(bin) => missing.bins.push(bin.clone()),
                        IneligibilityReason::MissingAnyBinary(bins) => {
                            missing.bins.extend(bins.iter().cloned())
                        }
                        IneligibilityReason::MissingEnv(env) => missing.env.push(env.clone()),
                        IneligibilityReason::MissingConfig(cfg) => missing.config.push(cfg.clone()),
                        IneligibilityReason::OsNotSupported(os) => {
                            missing.os.push(os.as_str().to_string());
                        }
                        IneligibilityReason::Disabled => {
                            // Surfaced by the `disabled` field (set just below
                            // when `entry_config.enabled == Some(false)`); the
                            // Disabled reason covers the manifest-level
                            // `eligibility.enabled: false` case which has no
                            // entry_config counterpart.
                        }
                    }
                }
                false
            }
        };

        if let Some(env_name) = manifest.primary_env() {
            if !api_key_set && !missing.env.iter().any(|e| e == env_name) {
                missing.env.push(env_name.to_string());
            }
        }

        let install_options = filter_install_specs_for_current_os(manifest.install_specs())
            .into_iter()
            .map(|spec| InstallOption {
                id: spec.id.clone(),
                kind: spec.kind.clone(),
                label: format!("Install {} ({})", spec.package, spec.kind.as_str()),
                bins: spec.bins.clone(),
            })
            .collect();

        let source_label = match manifest.source() {
            SkillSource::Bundled => "Bundled".to_string(),
            SkillSource::Global => "Global".to_string(),
            SkillSource::Workspace => "Workspace".to_string(),
            SkillSource::Plugin(id) => format!("Plugin: {}", id.as_str()),
        };

        Self {
            id: manifest.id().clone(),
            name: manifest.name().to_string(),
            description: manifest.description().to_string(),
            emoji: manifest.emoji().map(|s| s.to_string()),
            source: manifest.source().clone(),
            source_label,
            homepage: manifest.homepage().map(|s| s.to_string()),
            eligible,
            disabled,
            missing,
            install_options,
            primary_env: manifest.primary_env().map(|s| s.to_string()),
            api_key_set,
            scope,
            user_invocable: manifest.is_user_invocable(),
            usage,
        }
    }

    #[must_use]
    pub const fn matches_filter(&self, filter: SkillStatusFilter) -> bool {
        match filter {
            SkillStatusFilter::All => true,
            SkillStatusFilter::Ready => self.eligible && !self.disabled,
            SkillStatusFilter::NeedsSetup => !self.eligible && !self.disabled,
            SkillStatusFilter::Disabled => self.disabled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::{SkillContent, SkillManifest, SkillSource};

    fn make_manifest(name: &str) -> SkillManifest {
        SkillManifest::new(
            name,
            name,
            format!("{} desc", name),
            SkillContent::new("c"),
            SkillSource::Bundled,
        )
    }

    #[test]
    fn build_eligible_entry() {
        let m = make_manifest("test:skill");
        let e = SkillStatusEntry::build(&m, &EligibilityResult::Eligible, None, false, None);
        assert!(e.eligible);
        assert!(!e.disabled);
        assert!(e.missing.bins.is_empty());
        assert!(e.usage.is_none());
    }

    #[test]
    fn build_ineligible_entry() {
        let m = make_manifest("test:skill");
        let reasons = vec![IneligibilityReason::MissingBinary("docker".into())];
        let e = SkillStatusEntry::build(
            &m,
            &EligibilityResult::Ineligible(reasons),
            None,
            false,
            None,
        );
        assert!(!e.eligible);
        assert_eq!(e.missing.bins, vec!["docker"]);
    }

    #[test]
    fn disabled_by_config() {
        let m = make_manifest("test:skill");
        let cfg = SkillEntryConfig {
            enabled: Some(false),
            scope_override: None,
        };
        let e = SkillStatusEntry::build(&m, &EligibilityResult::Eligible, Some(&cfg), false, None);
        assert!(e.disabled);
    }

    #[test]
    fn missing_api_key_added_to_env() {
        let mut m = make_manifest("test:skill");
        m.set_primary_env("OPENAI_API_KEY".to_string());
        let e = SkillStatusEntry::build(&m, &EligibilityResult::Eligible, None, false, None);
        assert!(e.missing.env.contains(&"OPENAI_API_KEY".to_string()));
    }

    #[test]
    fn api_key_set_not_missing() {
        let mut m = make_manifest("test:skill");
        m.set_primary_env("OPENAI_API_KEY".to_string());
        let e = SkillStatusEntry::build(&m, &EligibilityResult::Eligible, None, true, None);
        assert!(!e.missing.env.contains(&"OPENAI_API_KEY".to_string()));
    }

    #[test]
    fn filter_matching() {
        let m = make_manifest("test:skill");
        let ready = SkillStatusEntry::build(&m, &EligibilityResult::Eligible, None, false, None);
        let needs = SkillStatusEntry::build(
            &m,
            &EligibilityResult::Ineligible(vec![IneligibilityReason::MissingBinary("x".into())]),
            None,
            false,
            None,
        );
        assert!(ready.matches_filter(SkillStatusFilter::Ready));
        assert!(!ready.matches_filter(SkillStatusFilter::NeedsSetup));
        assert!(needs.matches_filter(SkillStatusFilter::NeedsSetup));
    }

    #[test]
    fn usage_is_attached_when_provided() {
        let m = make_manifest("test:skill");
        let stats = UsageStats {
            use_count: 5,
            view_count: 2,
            ..Default::default()
        };
        let e = SkillStatusEntry::build(&m, &EligibilityResult::Eligible, None, false, Some(stats));
        let u = e.usage.expect("usage should pass through");
        assert_eq!(u.use_count, 5);
        assert_eq!(u.view_count, 2);
    }
}
