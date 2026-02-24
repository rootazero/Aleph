//! Status reporting — provides a serializable view of skill eligibility
//! for dashboards and diagnostic commands.

use serde::ser::{Serialize, SerializeStruct, Serializer};

use crate::domain::skill::{SkillId, SkillManifest, SkillSource};
use crate::domain::Entity;
use crate::skill::eligibility::EligibilityResult;

/// A report combining a skill's identity with its evaluated eligibility result.
#[derive(Debug, Clone)]
pub struct SkillStatusReport {
    /// The skill's unique identifier.
    pub id: SkillId,
    /// Human-readable skill name.
    pub name: String,
    /// Short description of the skill.
    pub description: String,
    /// Where the skill came from.
    pub source: SkillSource,
    /// The eligibility evaluation result.
    pub result: EligibilityResult,
}

impl SkillStatusReport {
    /// Build a status report from a manifest and its eligibility result.
    pub fn from_manifest(manifest: &SkillManifest, result: EligibilityResult) -> Self {
        Self {
            id: manifest.id().clone(),
            name: manifest.name().to_string(),
            description: manifest.description().to_string(),
            source: manifest.source().clone(),
            result,
        }
    }

    /// Whether the skill is eligible.
    pub fn is_eligible(&self) -> bool {
        self.result.is_eligible()
    }
}

impl Serialize for SkillStatusReport {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let eligible = self.is_eligible();

        // 5 fields normally, +1 if ineligible (for "reasons" array)
        let field_count = if eligible { 5 } else { 6 };
        let mut state = serializer.serialize_struct("SkillStatusReport", field_count)?;

        state.serialize_field("id", self.id.as_str())?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("description", &self.description)?;
        state.serialize_field("source", &format!("{:?}", self.source))?;
        state.serialize_field("eligible", &eligible)?;

        if !eligible {
            if let EligibilityResult::Ineligible(reasons) = &self.result {
                let reason_strings: Vec<String> =
                    reasons.iter().map(|r| format!("{:?}", r)).collect();
                state.serialize_field("reasons", &reason_strings)?;
            }
        }

        state.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::{SkillContent, SkillManifest, SkillSource};
    use crate::skill::eligibility::IneligibilityReason;

    fn make_manifest(name: &str) -> SkillManifest {
        SkillManifest::new(
            name,
            name,
            format!("{} description", name),
            SkillContent::new("content"),
            SkillSource::Bundled,
        )
    }

    #[test]
    fn status_eligible() {
        let manifest = make_manifest("git:commit");
        let report = SkillStatusReport::from_manifest(&manifest, EligibilityResult::Eligible);

        assert!(report.is_eligible());
        assert_eq!(report.id.as_str(), "git:commit");
        assert_eq!(report.name, "git:commit");
    }

    #[test]
    fn status_ineligible() {
        let manifest = make_manifest("docker:build");
        let reasons = vec![
            IneligibilityReason::MissingBinary("docker".to_string()),
            IneligibilityReason::OsNotSupported(crate::domain::skill::Os::Windows),
        ];
        let report = SkillStatusReport::from_manifest(
            &manifest,
            EligibilityResult::Ineligible(reasons),
        );

        assert!(!report.is_eligible());
        assert_eq!(report.id.as_str(), "docker:build");
    }

    #[test]
    fn serialization_eligible() {
        let manifest = make_manifest("git:commit");
        let report = SkillStatusReport::from_manifest(&manifest, EligibilityResult::Eligible);

        let value = serde_json::to_value(&report).expect("serialization should succeed");

        assert_eq!(value["id"], "git:commit");
        assert_eq!(value["name"], "git:commit");
        assert_eq!(value["eligible"], true);
        // No "reasons" field for eligible skills
        assert!(value.get("reasons").is_none());
    }

    #[test]
    fn serialization_ineligible() {
        let manifest = make_manifest("docker:build");
        let reasons = vec![
            IneligibilityReason::MissingBinary("docker".to_string()),
            IneligibilityReason::Disabled,
        ];
        let report = SkillStatusReport::from_manifest(
            &manifest,
            EligibilityResult::Ineligible(reasons),
        );

        let value = serde_json::to_value(&report).expect("serialization should succeed");

        assert_eq!(value["id"], "docker:build");
        assert_eq!(value["eligible"], false);

        let reasons_arr = value["reasons"].as_array().expect("reasons should be an array");
        assert_eq!(reasons_arr.len(), 2);
        // Verify reasons contain debug-formatted strings
        assert!(reasons_arr[0].as_str().unwrap().contains("MissingBinary"));
        assert!(reasons_arr[1].as_str().unwrap().contains("Disabled"));
    }
}
