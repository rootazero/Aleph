//! Snapshot Manager — builds a point-in-time snapshot of eligible skills with prompt XML.
//!
//! A `SkillSnapshot` captures which skills are eligible, which are not (and why),
//! and the pre-rendered prompt XML for system prompt injection. Each snapshot is
//! versioned; version increments indicate cache invalidation.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};

use crate::domain::skill::{SkillId, SkillManifest};
use crate::skill::config::SkillEntryConfig;
use crate::skill::eligibility::{EligibilityResult, EligibilityService, IneligibilityReason};
use crate::skill::prompt::{build_skills_prompt_xml, SkillPromptBudget};
use crate::skill::registry::SkillRegistry;

/// A point-in-time snapshot of skill eligibility and the pre-rendered prompt XML.
#[derive(Debug, Clone)]
pub struct SkillSnapshot {
    /// Monotonically increasing version counter for cache invalidation.
    pub version: u64,
    /// Pre-rendered XML fragment for system prompt injection.
    pub prompt_xml: String,
    /// Skill IDs that passed eligibility evaluation.
    pub eligible: Vec<SkillId>,
    /// Skill IDs that failed eligibility, mapped to their reasons.
    pub ineligible: HashMap<SkillId, Vec<IneligibilityReason>>,
    /// Full manifests for eligible + model-visible skills (for scope-aware filtering).
    pub eligible_manifests: Vec<SkillManifest>,
    /// When this snapshot was built.
    pub built_at: DateTime<Utc>,
    /// Budget the **live** prompt render applies. The authoritative
    /// `<available_skills>` index is rendered by `SkillInstructionsLayer`
    /// (it alone knows the active tool set for `Tool`-scope filtering); this
    /// field carries the user's `[prompt_budget]` config to that layer via
    /// `PromptConfig`. The `prompt_xml` field above is a convenience preview
    /// rendered with the **default** budget and is not the injected text.
    pub prompt_budget: SkillPromptBudget,
}

impl SkillSnapshot {
    /// Create an empty snapshot with version 0.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            version: 0,
            prompt_xml: String::new(),
            eligible: Vec::new(),
            ineligible: HashMap::new(),
            eligible_manifests: Vec::new(),
            built_at: Utc::now(),
            prompt_budget: SkillPromptBudget::default(),
        }
    }

    /// Build a snapshot by evaluating all skills in the registry.
    ///
    /// `config` is the Aleph main configuration serialized as a `serde_json::Value`
    /// and is forwarded to `EligibilityService::evaluate` for `required_config` checks.
    /// Pass `&serde_json::json!({})` when no config context is available.
    ///
    /// `entries` holds the user's per-skill overrides from `skills.toml`
    /// (enable/disable, scope override). These are applied **before** prompt
    /// injection so that a skill the user disabled never reaches the model and a
    /// scope override actually changes how the skill is surfaced. Pass an empty
    /// map when no user config is available — the result is then identical to a
    /// manifest-only evaluation.
    ///
    /// `archived` holds skill ids whose `.usage.json` lifecycle state is
    /// `archived`. Archived skills remain *eligible* (status surfaces and
    /// explicit `skill_read` still work) but are excluded from the injected
    /// prompt index so dormant skills stop consuming prompt budget.
    ///
    /// Iterates every skill, evaluates eligibility, and collects:
    /// - eligible skill IDs
    /// - ineligible skill IDs with reasons
    /// - prompt XML for eligible + model-visible skills
    #[must_use]
    pub fn build(
        registry: &SkillRegistry,
        eligibility: &EligibilityService,
        version: u64,
        config: &serde_json::Value,
        entries: &HashMap<String, SkillEntryConfig>,
        archived: &HashSet<String>,
    ) -> Self {
        let mut eligible = Vec::new();
        let mut ineligible: HashMap<SkillId, Vec<IneligibilityReason>> = HashMap::new();
        let mut eligible_manifests: Vec<SkillManifest> = Vec::new();

        // Collect and sort by skill ID for deterministic ordering
        let mut sorted: Vec<_> = registry.iter().collect();
        sorted.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));

        for (id, manifest) in sorted {
            let entry = entries.get(id.as_str());

            // A config-level disable overrides everything — the skill must not
            // appear as eligible, nor leak into the injected prompt index.
            if entry.and_then(|e| e.enabled) == Some(false) {
                ineligible.insert(id.clone(), vec![IneligibilityReason::Disabled]);
                continue;
            }

            match eligibility.evaluate(manifest, config) {
                EligibilityResult::Eligible => {
                    eligible.push(id.clone());
                    // Archived skills stay eligible but never reach the
                    // injected prompt index.
                    if archived.contains(id.as_str()) {
                        continue;
                    }
                    // Apply the user's scope override (if any) on a clone so the
                    // downstream prompt layer, which reads `manifest.scope()`,
                    // honours it. Without an override this is a plain clone.
                    let effective = match entry.and_then(|e| e.scope_override) {
                        Some(scope) => {
                            let mut m = manifest.clone();
                            m.set_scope(scope);
                            m
                        }
                        None => manifest.clone(),
                    };
                    if effective.is_model_visible() {
                        eligible_manifests.push(effective);
                    }
                }
                EligibilityResult::Ineligible(reasons) => {
                    ineligible.insert(id.clone(), reasons);
                }
            }
        }

        let model_visible: Vec<&SkillManifest> = eligible_manifests.iter().collect();
        let prompt_xml = build_skills_prompt_xml(&model_visible);

        Self {
            version,
            prompt_xml,
            eligible,
            ineligible,
            eligible_manifests,
            built_at: Utc::now(),
            prompt_budget: SkillPromptBudget::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::{
        EligibilitySpec, InvocationPolicy, PromptScope, SkillContent, SkillManifest, SkillSource,
    };

    /// Helper: an empty user-override map (no enable/disable, no scope override).
    fn no_overrides() -> HashMap<String, SkillEntryConfig> {
        HashMap::new()
    }

    /// Helper: an empty archived-skill set.
    fn no_archived() -> HashSet<String> {
        HashSet::new()
    }

    /// Helper: create a simple eligible manifest.
    fn make_manifest(name: &str, source: SkillSource) -> SkillManifest {
        SkillManifest::new(
            name,
            name,
            format!("{} description", name),
            SkillContent::new("content"),
            source,
        )
    }

    #[test]
    fn empty_snapshot() {
        let snap = SkillSnapshot::empty();
        assert_eq!(snap.version, 0);
        assert!(snap.prompt_xml.is_empty());
        assert!(snap.eligible.is_empty());
        assert!(snap.ineligible.is_empty());
    }

    #[test]
    fn build_from_registry() {
        let mut registry = SkillRegistry::new();
        let eligibility = EligibilityService::new();

        // Add an eligible skill
        let m1 = make_manifest("git:commit", SkillSource::Bundled);
        registry.register(m1);

        // Add an explicitly disabled skill
        let mut m2 = make_manifest("docker:build", SkillSource::Bundled);
        m2.set_eligibility(EligibilitySpec {
            enabled: Some(false),
            ..Default::default()
        });
        registry.register(m2);

        let snap = SkillSnapshot::build(
            &registry,
            &eligibility,
            1,
            &serde_json::json!({}),
            &no_overrides(),
            &no_archived(),
        );

        assert_eq!(snap.version, 1);
        assert_eq!(snap.eligible.len(), 1);
        assert_eq!(snap.ineligible.len(), 1);
        assert!(snap.eligible.contains(&SkillId::new("git:commit")));
        assert!(snap.ineligible.contains_key(&SkillId::new("docker:build")));
        assert!(!snap.prompt_xml.is_empty());
        assert!(snap.prompt_xml.contains("git:commit"));
    }

    #[test]
    fn version_increments() {
        let registry = SkillRegistry::new();
        let eligibility = EligibilityService::new();

        let cfg = serde_json::json!({});
        let ov = no_overrides();
        let snap1 = SkillSnapshot::build(&registry, &eligibility, 1, &cfg, &ov, &no_archived());
        let snap2 = SkillSnapshot::build(&registry, &eligibility, 2, &cfg, &ov, &no_archived());
        let snap3 = SkillSnapshot::build(&registry, &eligibility, 5, &cfg, &ov, &no_archived());

        assert_eq!(snap1.version, 1);
        assert_eq!(snap2.version, 2);
        assert_eq!(snap3.version, 5);
    }

    #[test]
    fn model_invisible_excluded_from_prompt() {
        let mut registry = SkillRegistry::new();
        let eligibility = EligibilityService::new();

        // Model-visible skill
        let m1 = make_manifest("visible:skill", SkillSource::Bundled);
        registry.register(m1);

        // Model-invisible skill (disable_model_invocation = true)
        let mut m2 = make_manifest("hidden:skill", SkillSource::Bundled);
        m2.set_invocation(InvocationPolicy {
            disable_model_invocation: true,
            ..Default::default()
        });
        registry.register(m2);

        // Disabled scope skill
        let mut m3 = make_manifest("disabled:skill", SkillSource::Bundled);
        m3.set_scope(PromptScope::Disabled);
        registry.register(m3);

        let snap = SkillSnapshot::build(
            &registry,
            &eligibility,
            1,
            &serde_json::json!({}),
            &no_overrides(),
            &no_archived(),
        );

        // All three are eligible (no eligibility constraints)
        // But only the visible one should appear in prompt_xml
        assert_eq!(snap.eligible.len(), 3);
        assert!(snap.prompt_xml.contains("visible:skill"));
        assert!(!snap.prompt_xml.contains("hidden:skill"));
        assert!(!snap.prompt_xml.contains("disabled:skill"));
    }

    #[test]
    fn eligible_manifests_populated() {
        let mut registry = SkillRegistry::new();
        let eligibility = EligibilityService::new();

        let m1 = make_manifest("visible:skill", SkillSource::Bundled);
        registry.register(m1);

        let mut m2 = make_manifest("disabled:skill", SkillSource::Bundled);
        m2.set_scope(PromptScope::Disabled);
        registry.register(m2);

        let mut m3 = make_manifest("hidden:skill", SkillSource::Bundled);
        m3.set_invocation(InvocationPolicy {
            disable_model_invocation: true,
            ..Default::default()
        });
        registry.register(m3);

        let snap = SkillSnapshot::build(
            &registry,
            &eligibility,
            1,
            &serde_json::json!({}),
            &no_overrides(),
            &no_archived(),
        );
        assert_eq!(snap.eligible.len(), 3);
        assert_eq!(snap.eligible_manifests.len(), 1);
        assert_eq!(snap.eligible_manifests[0].name(), "visible:skill");
    }

    #[test]
    fn config_disable_removes_from_eligible_and_prompt() {
        let mut registry = SkillRegistry::new();
        let eligibility = EligibilityService::new();
        // Manifest itself is fully eligible (no constraints).
        registry.register(make_manifest("git:commit", SkillSource::Bundled));

        // User disables it via skills.toml.
        let mut entries = HashMap::new();
        entries.insert(
            "git:commit".to_string(),
            SkillEntryConfig {
                enabled: Some(false),
                scope_override: None,
            },
        );

        let snap = SkillSnapshot::build(
            &registry,
            &eligibility,
            1,
            &serde_json::json!({}),
            &entries,
            &no_archived(),
        );

        assert!(
            snap.eligible.is_empty(),
            "config-disabled skill must not be eligible"
        );
        assert!(snap
            .ineligible
            .get(&SkillId::new("git:commit"))
            .is_some_and(|r| r.contains(&IneligibilityReason::Disabled)));
        assert!(
            !snap.prompt_xml.contains("git:commit"),
            "disabled skill must not leak into the injected prompt"
        );
        assert!(snap.eligible_manifests.is_empty());
    }

    #[test]
    fn config_scope_override_applied_to_prompt() {
        let mut registry = SkillRegistry::new();
        let eligibility = EligibilityService::new();
        // Default scope is System (model-visible). Override it to Disabled so it
        // drops out of the prompt index even though it stays eligible.
        registry.register(make_manifest("git:commit", SkillSource::Bundled));

        let mut entries = HashMap::new();
        entries.insert(
            "git:commit".to_string(),
            SkillEntryConfig {
                enabled: None,
                scope_override: Some(PromptScope::Disabled),
            },
        );

        let snap = SkillSnapshot::build(
            &registry,
            &eligibility,
            1,
            &serde_json::json!({}),
            &entries,
            &no_archived(),
        );

        // Still eligible (eligibility is independent of prompt scope)...
        assert!(snap.eligible.contains(&SkillId::new("git:commit")));
        // ...but the scope override made it model-invisible, so it is absent
        // from both eligible_manifests and the rendered prompt.
        assert!(snap.eligible_manifests.is_empty());
        assert!(!snap.prompt_xml.contains("git:commit"));
    }

    #[test]
    fn archived_skills_excluded_from_prompt_but_stay_eligible() {
        let mut registry = SkillRegistry::new();
        let eligibility = EligibilityService::new();
        registry.register(make_manifest("alive:skill", SkillSource::Global));
        registry.register(make_manifest("dormant:skill", SkillSource::Global));

        let mut archived = HashSet::new();
        archived.insert("dormant:skill".to_string());

        let snap = SkillSnapshot::build(
            &registry,
            &eligibility,
            1,
            &serde_json::json!({}),
            &no_overrides(),
            &archived,
        );

        // Both remain eligible — archive is a prompt-budget decision, not
        // an eligibility one. skill_read on the dormant skill still works.
        assert_eq!(snap.eligible.len(), 2);
        assert!(snap.prompt_xml.contains("alive:skill"));
        assert!(
            !snap.prompt_xml.contains("dormant:skill"),
            "archived skill must not occupy prompt budget"
        );
        assert_eq!(snap.eligible_manifests.len(), 1);
    }
}
