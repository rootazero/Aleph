//! `skill_status` — LLM Tool for querying skill system status.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::skill::{SkillStatusEntry, SkillStatusFilter, SkillSystem};
use crate::tools::AlephTool;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SkillStatusArgs {
    /// Filter skills by status: "all", "ready", "`needs_setup`", "disabled"
    #[serde(default = "default_filter")]
    pub filter: String,
}

fn default_filter() -> String {
    "all".to_string()
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
    const DESCRIPTION: &'static str = "Query skill system status. Returns skills filtered by readiness: all, ready (eligible and enabled), needs_setup (missing dependencies or API keys), or disabled (user turned off).";

    type Args = SkillStatusArgs;
    type Output = SkillStatusOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let filter = match args.filter.as_str() {
            "all" => SkillStatusFilter::All,
            "ready" => SkillStatusFilter::Ready,
            "needs_setup" => SkillStatusFilter::NeedsSetup,
            "disabled" => SkillStatusFilter::Disabled,
            other => {
                return Err(crate::error::AlephError::tool(format!(
                    "Invalid filter '{other}'. Expected one of: all, ready, needs_setup, disabled."
                )));
            }
        };

        let all_entries = self.system.full_status().await;
        let total = all_entries.len();
        let filtered: Vec<SkillStatusEntry> = all_entries
            .into_iter()
            .filter(|e| e.matches_filter(filter))
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
}
