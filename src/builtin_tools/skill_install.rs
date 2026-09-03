//! `skill_install` — LLM Tool for installing skill dependencies.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::domain::skill::SkillId;
use crate::error::{AlephError, Result};
use crate::skill::SkillSystem;
use crate::skill::installer::SkillInstallError;
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
        // `SkillId::new` only enforces its non-empty invariant under
        // `debug_assert!`; in release the assertion is a no-op and an empty
        // `skill_id` flows through to `install_dependency` as an empty
        // registry key. Validate the JSON-RPC boundary here so the
        // invariant the doc comment promises is actually upheld.
        let raw = args.skill_id.trim();
        if raw.is_empty() {
            return Err(AlephError::tool(
                "'skill_id' is required for 'skill_install'",
            ));
        }
        let skill_id = SkillId::new(raw);
        let result = self
            .system
            .install_dependency(&skill_id, args.spec_id.as_deref())
            .await;

        // Successful installs reshape the skill's runtime (new binaries,
        // env, eligibility) — record as a patch event so the curator /
        // status surface reflects the install activity.
        if result.is_ok() {
            self.system.record_patch(&skill_id).await;
        }

        // Convert the typed Result into the existing wire-format output
        // shape. Success populates stdout/exit-code; failure surfaces the
        // error message in the `message` field so the model can read it.
        let output = match result {
            Ok(s) => SkillInstallOutput {
                success: true,
                message: "Successfully installed".to_string(),
                stdout: s.stdout,
                stderr: s.stderr,
            },
            Err(e) => match e {
                SkillInstallError::SkillNotFound(id) => SkillInstallOutput {
                    success: false,
                    message: format!("Skill not found: {id}"),
                    stdout: String::new(),
                    stderr: String::new(),
                },
                SkillInstallError::NoMatchingSpec(id) => SkillInstallOutput {
                    success: false,
                    message: format!("No matching install spec found for skill {id}"),
                    stdout: String::new(),
                    stderr: String::new(),
                },
                SkillInstallError::ExecutionFailed {
                    message,
                    stderr,
                    ..
                } => SkillInstallOutput {
                    success: false,
                    message: format!("Execution failed: {message}"),
                    stdout: String::new(),
                    stderr,
                },
                SkillInstallError::Io(io) => SkillInstallOutput {
                    success: false,
                    message: format!("Failed to execute install command: {io}"),
                    stdout: String::new(),
                    stderr: String::new(),
                },
            },
        };

        Ok(output)
    }
}
