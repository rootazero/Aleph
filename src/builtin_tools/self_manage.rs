//! `SelfManageTool` — enter self-management mode via LLM intent recognition
//!
//! When the LLM detects that the user wants to configure, modify, fix, or
//! manage Aleph settings, it calls this tool to get the full self-management
//! manual (workspace map + operation protocol + domain routing).
//!
//! The tool reads the `/self` SKILL.md from the bundled skills directory,
//! providing the same content as the `/self` slash command but triggered
//! by natural language intent.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::{notify_tool_result, notify_tool_start};
use crate::error::Result;
use crate::tools::AlephTool;

/// Minimal self-management guidance returned when the /self SKILL.md is
/// missing or unreadable — the agent still gets a usable next step rather
/// than a dead end.
const FALLBACK_MANUAL: &str = concat!(
    "Self-management skill not found. Basic guidance:\n\n",
    "1. Configuration file: ~/.aleph/config.toml (hot-reload)\n",
    "2. API keys: use vault_store tool (never write to config)\n",
    "3. Detailed guides: call read_config_guide(topic) for domain-specific help\n",
    "4. Topics: overview, providers, generation, channels, agents, skills, mcp, general, cron\n",
);

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SelfManageArgs {
    /// Brief description of what the user wants to configure or manage
    #[schemars(
        description = "What the user wants to configure, modify, or fix. Examples: 'add video provider', 'change default LLM', 'configure Telegram bot', 'install a plugin'"
    )]
    pub intent: String,
}

#[derive(Debug, Serialize)]
pub struct SelfManageOutput {
    pub success: bool,
    pub manual: String,
    pub hint: String,
}

#[derive(Clone)]
pub struct SelfManageTool {
    skill_search_dirs: Vec<PathBuf>,
}

impl SelfManageTool {
    #[must_use]
    pub const fn new(skill_search_dirs: Vec<PathBuf>) -> Self {
        Self { skill_search_dirs }
    }

    /// Search for the /self SKILL.md in known directories
    fn find_skill_file(&self) -> Option<PathBuf> {
        for dir in &self.skill_search_dirs {
            let path = dir.join("self").join("SKILL.md");
            if path.exists() {
                return Some(path);
            }
        }
        None
    }
}

impl Default for SelfManageTool {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
        let aleph_home = home.join(".aleph");
        Self::new(vec![aleph_home.join("skills")])
    }
}

#[async_trait]
impl AlephTool for SelfManageTool {
    const NAME: &'static str = "self_manage";
    const DESCRIPTION: &'static str = "Enter Aleph self-management mode. Call when the user wants to configure, modify, fix, or manage Aleph settings — including providers (LLM/generation), channels (Telegram/Discord), agents, skills, plugins, MCP servers, cron jobs, or any ~/.aleph/ configuration. Returns the full self-management manual with workspace structure, operation protocol, and step-by-step guides.";

    type Args = SelfManageArgs;
    type Output = SelfManageOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        notify_tool_start(Self::NAME, &args.intent);

        // Read the /self SKILL.md content
        let manual = match self.find_skill_file() {
            Some(path) => {
                match tokio::fs::read_to_string(&path).await {
                    Ok(content) => {
                        // Strip YAML frontmatter (between --- delimiters)
                        if let Some(rest) = content.strip_prefix("---") {
                            if let Some(pos) = rest.find("---") {
                                rest[pos + 3..].trim_start().to_string()
                            } else {
                                content
                            }
                        } else {
                            content
                        }
                    }
                    Err(e) => {
                        // A read failure (transient I/O, permissions) means the
                        // full manual is unavailable — exactly like a missing
                        // SKILL.md. Degrade to the same fallback guidance instead
                        // of hard-failing the tool, so the agent still gets a
                        // usable next step rather than a dead end.
                        notify_tool_result(
                            Self::NAME,
                            &format!("using fallback guidance (SKILL.md read failed: {e})"),
                            true,
                        );
                        return Ok(SelfManageOutput {
                            success: true,
                            manual: FALLBACK_MANUAL.to_string(),
                            hint: format!(
                                "Now use read_config_guide(topic) for details about: {}",
                                args.intent
                            ),
                        });
                    }
                }
            }
            None => {
                // Fallback: return minimal guidance if SKILL.md not found
                notify_tool_result(
                    Self::NAME,
                    "using fallback guidance (SKILL.md not found)",
                    true,
                );
                return Ok(SelfManageOutput {
                    success: true,
                    manual: FALLBACK_MANUAL.to_string(),
                    hint: format!(
                        "Now use read_config_guide(topic) for details about: {}",
                        args.intent
                    ),
                });
            }
        };

        notify_tool_result(Self::NAME, "self-management mode activated", true);
        Ok(SelfManageOutput {
            success: true,
            manual,
            hint: format!(
                "Self-management mode active. For '{}', call read_config_guide(topic) with the relevant topic to get detailed structure and steps.",
                args.intent
            ),
        })
    }
}
