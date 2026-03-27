//! skill_status — LLM Tool for querying skill system status.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::skill::{SkillStatusEntry, SkillStatusFilter, SkillSystem};
use crate::tools::AlephTool;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SkillStatusArgs {
    /// Filter skills by status: "all", "ready", "needs_setup", "disabled"
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
    pub fn new(system: SkillSystem) -> Self {
        Self { system }
    }
}

#[async_trait]
impl AlephTool for SkillStatusTool {
    const NAME: &'static str = "skill_status";
    const DESCRIPTION: &'static str = "Query skill system status. Returns skills filtered by readiness: all, ready (eligible and enabled), needs_setup (missing dependencies or API keys), or disabled (user turned off).";

    type Args = SkillStatusArgs;
    type Output = SkillStatusOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "skill_status(filter='all') — list all skills with their status".to_string(),
            "skill_status(filter='needs_setup') — find skills missing dependencies".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let filter = match args.filter.as_str() {
            "ready" => SkillStatusFilter::Ready,
            "needs_setup" => SkillStatusFilter::NeedsSetup,
            "disabled" => SkillStatusFilter::Disabled,
            _ => SkillStatusFilter::All,
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
