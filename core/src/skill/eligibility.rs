//! Eligibility service — evaluates whether a skill is eligible to run
//! on the current machine based on OS, binaries, environment variables, etc.

use crate::domain::skill::{EligibilitySpec, Os, SkillManifest};

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/// Result of evaluating a skill's eligibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EligibilityResult {
    /// The skill is eligible to run.
    Eligible,
    /// The skill is ineligible for one or more reasons.
    Ineligible(Vec<IneligibilityReason>),
}

impl EligibilityResult {
    /// Convenience: is the skill eligible?
    pub fn is_eligible(&self) -> bool {
        matches!(self, Self::Eligible)
    }
}

/// Reason why a skill is not eligible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IneligibilityReason {
    /// The skill has been explicitly disabled.
    Disabled,
    /// The current OS is not in the skill's allowed OS list.
    OsNotSupported(Os),
    /// A required binary is missing from PATH.
    MissingBinary(String),
    /// None of the "any" binaries are present on PATH.
    MissingAnyBinary(Vec<String>),
    /// A required environment variable is not set.
    MissingEnv(String),
    /// A required config key is missing.
    MissingConfig(String),
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

/// Stateless service that evaluates skill eligibility.
#[derive(Debug, Default)]
pub struct EligibilityService;

impl EligibilityService {
    /// Create a new eligibility service.
    pub fn new() -> Self {
        Self
    }

    /// Evaluate a single skill's eligibility.
    ///
    /// Check order:
    /// 1. `always` flag — if true, skip all other checks
    /// 2. `enabled` override — if explicitly `false`, immediately ineligible
    /// 3. OS check
    /// 4. required_bins
    /// 5. any_bins
    /// 6. required_env
    pub fn evaluate(&self, manifest: &SkillManifest) -> EligibilityResult {
        let spec = manifest.eligibility();
        self.evaluate_spec(spec)
    }

    /// Evaluate an eligibility spec directly.
    fn evaluate_spec(&self, spec: &EligibilitySpec) -> EligibilityResult {
        // 1. always flag
        if spec.always {
            return EligibilityResult::Eligible;
        }

        // 2. enabled override
        if spec.enabled == Some(false) {
            return EligibilityResult::Ineligible(vec![IneligibilityReason::Disabled]);
        }

        let mut reasons = Vec::new();

        // 3. OS check
        if let Some(ref os_list) = spec.os {
            let current = current_os();
            if !os_list.contains(&current) {
                reasons.push(IneligibilityReason::OsNotSupported(current));
            }
        }

        // 4. required_bins
        for bin in &spec.required_bins {
            if which::which(bin).is_err() {
                reasons.push(IneligibilityReason::MissingBinary(bin.clone()));
            }
        }

        // 5. any_bins
        if !spec.any_bins.is_empty() {
            let any_found = spec.any_bins.iter().any(|b| which::which(b).is_ok());
            if !any_found {
                reasons.push(IneligibilityReason::MissingAnyBinary(
                    spec.any_bins.clone(),
                ));
            }
        }

        // 6. required_env
        for var in &spec.required_env {
            if std::env::var(var).is_err() {
                reasons.push(IneligibilityReason::MissingEnv(var.clone()));
            }
        }

        // 7. required_config — config system not yet wired, skip checks for now
        if !spec.required_config.is_empty() {
            tracing::debug!(
                count = spec.required_config.len(),
                "required_config checks not yet implemented, skipping"
            );
        }

        if reasons.is_empty() {
            EligibilityResult::Eligible
        } else {
            EligibilityResult::Ineligible(reasons)
        }
    }

    /// Evaluate all skills in an iterator.
    pub fn evaluate_all<'a>(
        &self,
        skills: impl IntoIterator<Item = &'a SkillManifest>,
    ) -> Vec<(&'a SkillManifest, EligibilityResult)> {
        skills
            .into_iter()
            .map(|m| {
                let result = self.evaluate(m);
                (m, result)
            })
            .collect()
    }
}

/// Detect the current operating system.
pub fn current_os() -> Os {
    #[cfg(target_os = "macos")]
    {
        Os::Darwin
    }
    #[cfg(target_os = "linux")]
    {
        Os::Linux
    }
    #[cfg(target_os = "windows")]
    {
        Os::Windows
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        // Fallback — treat unknown as Linux
        Os::Linux
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::{EligibilitySpec, SkillContent, SkillId, SkillManifest, SkillSource};

    /// Helper: create a manifest with a given eligibility spec.
    fn manifest_with_eligibility(spec: EligibilitySpec) -> SkillManifest {
        let mut m = SkillManifest::new(
            SkillId::new("test:skill"),
            "Test Skill",
            "A test skill",
            SkillContent::new("content"),
            SkillSource::Bundled,
        );
        m.set_eligibility(spec);
        m
    }

    #[test]
    fn default_eligibility_is_eligible() {
        let svc = EligibilityService::new();
        let m = manifest_with_eligibility(EligibilitySpec::default());
        let result = svc.evaluate(&m);
        assert!(result.is_eligible());
    }

    #[test]
    fn always_flag_bypasses_checks() {
        let svc = EligibilityService::new();
        // Even with impossible requirements, always=true should pass
        let spec = EligibilitySpec {
            always: true,
            required_bins: vec!["nonexistent-binary-abc123".to_string()],
            required_env: vec!["NONEXISTENT_ENV_VAR_XYZ".to_string()],
            ..Default::default()
        };
        let m = manifest_with_eligibility(spec);
        let result = svc.evaluate(&m);
        assert!(result.is_eligible());
    }

    #[test]
    fn explicit_disabled() {
        let svc = EligibilityService::new();
        let spec = EligibilitySpec {
            enabled: Some(false),
            ..Default::default()
        };
        let m = manifest_with_eligibility(spec);
        let result = svc.evaluate(&m);
        assert!(!result.is_eligible());
        match result {
            EligibilityResult::Ineligible(reasons) => {
                assert_eq!(reasons.len(), 1);
                assert_eq!(reasons[0], IneligibilityReason::Disabled);
            }
            _ => panic!("expected Ineligible"),
        }
    }

    #[test]
    fn missing_binary() {
        let svc = EligibilityService::new();
        let spec = EligibilitySpec {
            required_bins: vec!["nonexistent-binary-abc123".to_string()],
            ..Default::default()
        };
        let m = manifest_with_eligibility(spec);
        let result = svc.evaluate(&m);
        assert!(!result.is_eligible());
        match result {
            EligibilityResult::Ineligible(reasons) => {
                assert!(reasons.iter().any(|r| matches!(r, IneligibilityReason::MissingBinary(b) if b == "nonexistent-binary-abc123")));
            }
            _ => panic!("expected Ineligible"),
        }
    }

    #[test]
    fn any_bins_all_missing() {
        let svc = EligibilityService::new();
        let spec = EligibilitySpec {
            any_bins: vec![
                "nonexistent-binary-abc123".to_string(),
                "nonexistent-binary-def456".to_string(),
            ],
            ..Default::default()
        };
        let m = manifest_with_eligibility(spec);
        let result = svc.evaluate(&m);
        assert!(!result.is_eligible());
        match result {
            EligibilityResult::Ineligible(reasons) => {
                assert!(reasons
                    .iter()
                    .any(|r| matches!(r, IneligibilityReason::MissingAnyBinary(..))));
            }
            _ => panic!("expected Ineligible"),
        }
    }

    #[test]
    fn missing_env() {
        let svc = EligibilityService::new();
        let spec = EligibilitySpec {
            required_env: vec!["ALEPH_NONEXISTENT_ENV_VAR_12345".to_string()],
            ..Default::default()
        };
        let m = manifest_with_eligibility(spec);
        let result = svc.evaluate(&m);
        assert!(!result.is_eligible());
        match result {
            EligibilityResult::Ineligible(reasons) => {
                assert!(reasons.iter().any(|r| matches!(r, IneligibilityReason::MissingEnv(v) if v == "ALEPH_NONEXISTENT_ENV_VAR_12345")));
            }
            _ => panic!("expected Ineligible"),
        }
    }

    #[test]
    fn current_os_eligible() {
        let svc = EligibilityService::new();
        let current = current_os();
        let spec = EligibilitySpec {
            os: Some(vec![current.clone()]),
            ..Default::default()
        };
        let m = manifest_with_eligibility(spec);
        let result = svc.evaluate(&m);
        assert!(result.is_eligible(), "skill should be eligible on current OS: {:?}", current);
    }
}
