//! Snapshot Manager — builds a point-in-time snapshot of eligible skills with prompt XML.
//!
//! A `SkillSnapshot` captures which skills are eligible, which are not (and why),
//! and the pre-rendered prompt XML for system prompt injection. Each snapshot is
//! versioned; version increments indicate cache invalidation.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::domain::skill::{SkillId, SkillManifest};
use crate::skill::eligibility::{EligibilityResult, EligibilityService, IneligibilityReason};
use crate::skill::prompt::build_skills_prompt_xml;
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
    /// When this snapshot was built.
    pub built_at: DateTime<Utc>,
}

impl SkillSnapshot {
    /// Create an empty snapshot with version 0.
    pub fn empty() -> Self {
        Self {
            version: 0,
            prompt_xml: String::new(),
            eligible: Vec::new(),
            ineligible: HashMap::new(),
            built_at: Utc::now(),
        }
    }

    /// Build a snapshot by evaluating all skills in the registry.
    ///
    /// Iterates every skill, evaluates eligibility, and collects:
    /// - eligible skill IDs
    /// - ineligible skill IDs with reasons
    /// - prompt XML for eligible + model-visible skills
    pub fn build(registry: &SkillRegistry, eligibility: &EligibilityService, version: u64) -> Self {
        let mut eligible = Vec::new();
        let mut ineligible: HashMap<SkillId, Vec<IneligibilityReason>> = HashMap::new();
        let mut model_visible: Vec<&SkillManifest> = Vec::new();

        // Collect and sort by skill ID for deterministic ordering
        let mut entries: Vec<_> = registry.iter().collect();
        entries.sort_by_key(|(id, _)| id.as_str().to_string());

        for (id, manifest) in entries {
            match eligibility.evaluate(manifest) {
                EligibilityResult::Eligible => {
                    eligible.push(id.clone());
                    if manifest.is_model_visible() {
                        model_visible.push(manifest);
                    }
                }
                EligibilityResult::Ineligible(reasons) => {
                    ineligible.insert(id.clone(), reasons);
                }
            }
        }

        let prompt_xml = build_skills_prompt_xml(&model_visible);

        Self {
            version,
            prompt_xml,
            eligible,
            ineligible,
            built_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::{
        EligibilitySpec, InvocationPolicy, PromptScope, SkillContent, SkillManifest, SkillSource,
    };

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

        let snap = SkillSnapshot::build(&registry, &eligibility, 1);

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

        let snap1 = SkillSnapshot::build(&registry, &eligibility, 1);
        let snap2 = SkillSnapshot::build(&registry, &eligibility, 2);
        let snap3 = SkillSnapshot::build(&registry, &eligibility, 5);

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

        let snap = SkillSnapshot::build(&registry, &eligibility, 1);

        // All three are eligible (no eligibility constraints)
        // But only the visible one should appear in prompt_xml
        assert_eq!(snap.eligible.len(), 3);
        assert!(snap.prompt_xml.contains("visible:skill"));
        assert!(!snap.prompt_xml.contains("hidden:skill"));
        assert!(!snap.prompt_xml.contains("disabled:skill"));
    }
}
