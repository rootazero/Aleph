//! ReadConfigGuideTool — progressive disclosure of configuration knowledge

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::{notify_tool_result, notify_tool_start};
use crate::error::Result;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReadConfigGuideArgs {
    /// Topic to get configuration guide for
    #[schemars(
        description = "Configuration domain: overview (all domains + file paths), providers (LLM provider config + vault), mcp (MCP server config), skills (skill install + format), agents (agent workspace + SOUL.md), general (general/memory/policies), generation (image/speech/video providers), channels (Telegram/Discord config), cron (scheduled tasks)"
    )]
    pub topic: GuideTopic,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GuideTopic {
    Overview,
    Providers,
    Mcp,
    Skills,
    Agents,
    General,
    Generation,
    Channels,
    Cron,
}

impl GuideTopic {
    fn filename(&self) -> &'static str {
        match self {
            Self::Overview => "overview.md",
            Self::Providers => "providers.md",
            Self::Mcp => "mcp.md",
            Self::Skills => "skills.md",
            Self::Agents => "agents.md",
            Self::General => "general.md",
            Self::Generation => "generation.md",
            Self::Channels => "channels.md",
            Self::Cron => "cron.md",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ReadConfigGuideOutput {
    pub success: bool,
    pub topic: String,
    pub content: String,
}

#[derive(Clone)]
pub struct ReadConfigGuideTool {
    guides_dir: PathBuf,
}

impl ReadConfigGuideTool {
    pub fn new(guides_dir: PathBuf) -> Self {
        Self { guides_dir }
    }

    /// Default guides directory: ~/.aleph/guides/
    pub fn default_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".aleph")
            .join("guides")
    }
}

impl Default for ReadConfigGuideTool {
    fn default() -> Self {
        Self::new(Self::default_dir())
    }
}

#[async_trait]
impl AlephTool for ReadConfigGuideTool {
    const NAME: &'static str = "read_config_guide";
    const DESCRIPTION: &'static str = "Get Aleph configuration manual. Call when user needs to modify config, install plugins/skills, configure API keys, manage agents, or other self-management operations. Returns structure, steps, and caveats for the domain.";

    type Args = ReadConfigGuideArgs;
    type Output = ReadConfigGuideOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let topic_name = format!("{:?}", args.topic).to_lowercase();
        notify_tool_start(Self::NAME, &topic_name);

        let file_path = self.guides_dir.join(args.topic.filename());

        let content = match tokio::fs::read_to_string(&file_path).await {
            Ok(c) => c,
            Err(e) => {
                let msg = format!(
                    "Guide '{}' not found at {}: {}",
                    topic_name,
                    file_path.display(),
                    e
                );
                notify_tool_result(Self::NAME, &msg, false);
                return Ok(ReadConfigGuideOutput {
                    success: false,
                    topic: topic_name,
                    content: msg,
                });
            }
        };

        notify_tool_result(Self::NAME, &format!("loaded {} guide", topic_name), true);
        Ok(ReadConfigGuideOutput {
            success: true,
            topic: topic_name,
            content,
        })
    }
}
