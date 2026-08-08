//! `skill_status` — LLM Tool for querying skill system status.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::skill::{SkillState, SkillStatusEntry, SkillStatusFilter, SkillSystem};
use crate::tools::AlephTool;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SkillStatusArgs {
    /// Filter skills by status: "all", "ready", "`needs_setup`", "disabled", "stale"
    #[serde(default = "default_filter")]
    pub filter: String,
}

fn default_filter() -> String {
    "all".to_string()
}

/// The tool's filter vocabulary.
///
/// `"stale"` is a *usage* predicate, not a readiness one, so it is resolved
/// here rather than added to [`SkillStatusFilter`]: that enum answers "can
/// this skill run", is shared with the Panel and the CLI, and neither has a
/// usage column to filter on. Keeping the two axes apart is why
/// `matches_filter` stays a total function over readiness alone.
#[derive(Debug, Clone, Copy)]
enum StatusFilter {
    Readiness(SkillStatusFilter),
    Stale,
}

impl StatusFilter {
    fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "all" => Self::Readiness(SkillStatusFilter::All),
            "ready" => Self::Readiness(SkillStatusFilter::Ready),
            "needs_setup" => Self::Readiness(SkillStatusFilter::NeedsSetup),
            "disabled" => Self::Readiness(SkillStatusFilter::Disabled),
            "stale" => Self::Stale,
            _ => return None,
        })
    }

    fn matches(self, entry: &SkillStatusEntry) -> bool {
        match self {
            Self::Readiness(f) => entry.matches_filter(f),
            Self::Stale => entry
                .usage
                .as_ref()
                .is_some_and(|u| u.state == SkillState::Stale),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SkillStatusOutput {
    pub total: usize,
    pub filtered: usize,
    pub skills: Vec<SkillStatusEntry>,
}

#[derive(Clone)]
pub struct SkillStatusTool {
    system: SkillSystem,
}

impl SkillStatusTool {
    #[must_use]
    pub const fn new(system: SkillSystem) -> Self {
        Self { system }
    }
}

#[async_trait]
impl AlephTool for SkillStatusTool {
    const NAME: &'static str = "skill_status";
    const DESCRIPTION: &'static str = "Query skill system status. filter: all, ready (eligible and enabled), needs_setup (missing dependencies or API keys), disabled (user turned off), or stale (nightly maintenance saw no activity for long enough to flag it). Each entry carries usage — use_count, view_count, last_used_at, state — reported nowhere else; a stale skill is an archive candidate for skill_manage, so confirm it is still wanted first.";

    type Args = SkillStatusArgs;
    type Output = SkillStatusOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let Some(filter) = StatusFilter::parse(args.filter.as_str()) else {
            return Err(crate::error::AlephError::tool(format!(
                "Invalid filter '{}'. Expected one of: all, ready, needs_setup, disabled, stale.",
                args.filter
            )));
        };

        let all_entries = self.system.full_status().await;
        let total = all_entries.len();
        let filtered: Vec<SkillStatusEntry> = all_entries
            .into_iter()
            .filter(|e| filter.matches(e))
            .collect();
        let filtered_count = filtered.len();

        Ok(SkillStatusOutput {
            total,
            filtered: filtered_count,
            skills: filtered,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn skill_status_uses_shared_initialized_system() {
        // A SkillStatusTool built from the shared singleton must not panic and
        // must return consistent total/filtered counts.
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let system = crate::skill::shared_skill_system().clone();
        let _ = system.init(crate::skill::default_skill_dirs()).await;
        let tool = SkillStatusTool::new(system);
        let out = tool
            .call(SkillStatusArgs {
                filter: "all".to_string(),
            })
            .await
            .unwrap();
        // total >= filtered is a tautology; the assertion guards against a panic
        // and verifies the tool can call through to a real (possibly empty) system.
        assert!(out.total >= out.filtered);
    }

    fn entry_with_state(name: &str, state: SkillState) -> SkillStatusEntry {
        use crate::domain::skill::{SkillContent, SkillManifest, SkillSource};
        use crate::skill::{EligibilityResult, UsageStats};

        let manifest = SkillManifest::new(
            name,
            name,
            format!("{name} desc"),
            SkillContent::new("c"),
            SkillSource::Bundled,
        );
        let usage = UsageStats {
            use_count: 3,
            state,
            ..Default::default()
        };
        SkillStatusEntry::build(
            &manifest,
            &EligibilityResult::Eligible,
            None,
            false,
            Some(usage),
        )
    }

    /// `stale` is the lifecycle marker nightly maintenance writes; before it
    /// was a filter value the model had no way to ask for the skills that
    /// telemetry had already flagged.
    #[test]
    fn stale_filter_selects_only_stale_entries() {
        let stale = entry_with_state("dormant", SkillState::Stale);
        let active = entry_with_state("busy", SkillState::Active);
        let filter = StatusFilter::parse("stale").expect("stale is a valid filter");
        assert!(filter.matches(&stale));
        assert!(!filter.matches(&active));
        // ...and it is orthogonal to readiness: both entries are `ready`.
        let ready = StatusFilter::parse("ready").unwrap();
        assert!(ready.matches(&stale) && ready.matches(&active));
    }

    #[test]
    fn unknown_filter_is_rejected() {
        assert!(StatusFilter::parse("archived").is_none());
        assert!(StatusFilter::parse("").is_none());
    }
}
