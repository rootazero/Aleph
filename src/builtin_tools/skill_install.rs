//! `skill_install` — LLM Tool for installing skill dependencies.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::domain::skill::SkillId;
use crate::error::Result;
use crate::skill::{InstallResult, SkillSystem};
use crate::tools::AlephTool;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SkillInstallArgs {
    /// The skill ID to install dependencies for (e.g. 'web-search', 'code-review')
    pub skill_id: String,
    /// Optional specific install recipe ID. If omitted, the best one is auto-selected.
    #[serde(default)]
    pub spec_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SkillInstallOutput {
    pub success: bool,
    pub message: String,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone)]
pub struct SkillInstallTool {
    system: SkillSystem,
}

impl SkillInstallTool {
    #[must_use]
    pub const fn new(system: SkillSystem) -> Self {
        Self { system }
    }
}

impl From<InstallResult> for SkillInstallOutput {
    fn from(r: InstallResult) -> Self {
        Self {
            success: r.success,
            message: r.message,
            stdout: r.stdout,
            stderr: r.stderr,
        }
    }
}

#[async_trait]
impl AlephTool for SkillInstallTool {
    const NAME: &'static str = "skill_install";
    const DESCRIPTION: &'static str = "Install missing dependencies for a skill. Specify skill_id; optionally spec_id for a specific installer (e.g. brew vs npm). Auto-selects best installer if spec_id is omitted.";

    type Args = SkillInstallArgs;
    type Output = SkillInstallOutput;

    fn requires_confirmation(&self) -> bool {
        true // Installing software requires user confirmation
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let skill_id = SkillId::new(&args.skill_id);
        let result = self
            .system
            .install_dependency(&skill_id, args.spec_id.as_deref())
            .await;

        // Successful installs reshape the skill's runtime (new binaries,
        // env, eligibility) — record as a patch event so the curator /
        // status surface reflects the install activity.
        if result.success {
            self.system.record_patch(&skill_id).await;
        }

        Ok(SkillInstallOutput::from(result))
    }
}
