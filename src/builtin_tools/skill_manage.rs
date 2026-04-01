//! skill_manage — LLM Tool for toggling and configuring skills.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::domain::skill::{PromptScope, SkillId};
use crate::error::{AlephError, Result};
use crate::skill::{SkillConfigUpdate, SkillSystem};
use crate::tools::AlephTool;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SkillManageArgs {
    /// The skill ID to configure (e.g. 'web-search', 'code-review')
    pub skill_id: String,
    /// Set enabled/disabled. true = enabled, false = disabled.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Set prompt scope: "system", "tool", "standalone", "disabled"
    #[serde(default)]
    pub scope: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SkillManageOutput {
    pub success: bool,
    pub message: String,
}

#[derive(Clone)]
pub struct SkillManageTool {
    system: SkillSystem,
}

impl SkillManageTool {
    pub fn new(system: SkillSystem) -> Self {
        Self { system }
    }
}

#[async_trait]
impl AlephTool for SkillManageTool {
    const NAME: &'static str = "skill_manage";
    const DESCRIPTION: &'static str = "Toggle or configure a skill. Set enabled/disabled state or change the prompt injection scope (system/tool/standalone/disabled).";

    type Args = SkillManageArgs;
    type Output = SkillManageOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "skill_manage(skill_id='web-search', enabled=false) — disable web-search".to_string(),
            "skill_manage(skill_id='code-review', scope='tool') — change to tool scope".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let skill_id = SkillId::new(&args.skill_id);

        if args.enabled.is_none() && args.scope.is_none() {
            return Ok(SkillManageOutput {
                success: false,
                message: "No changes requested. Provide 'enabled' or 'scope' to update."
                    .to_string(),
            });
        }

        if let Some(enabled) = args.enabled {
            self.system
                .update_config(&skill_id, SkillConfigUpdate::SetEnabled(enabled))
                .await
                .map_err(|e| AlephError::tool(format!("Failed to set enabled: {}", e)))?;
        }

        if let Some(scope_str) = &args.scope {
            let scope = match scope_str.as_str() {
                "system" => PromptScope::System,
                "tool" => PromptScope::Tool,
                "standalone" => PromptScope::Standalone,
                "disabled" => PromptScope::Disabled,
                other => {
                    return Err(AlephError::tool(format!("Invalid scope: {}", other)));
                }
            };
            self.system
                .update_config(&skill_id, SkillConfigUpdate::SetScope(scope))
                .await
                .map_err(|e| AlephError::tool(format!("Failed to set scope: {}", e)))?;
        }

        Ok(SkillManageOutput {
            success: true,
            message: format!("Skill {} configuration updated", args.skill_id),
        })
    }
}
