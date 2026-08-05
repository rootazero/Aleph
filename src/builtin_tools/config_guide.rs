//! `ReadConfigGuideTool` — progressive disclosure of configuration knowledge

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
        description = "Configuration domain: overview (all domains + file paths), providers (LLM provider config + vault), mcp (MCP server config), skills (skill install + format), agents (agent workspace + SOUL.md), general (general/memory/policies), generation (image/speech/video providers), channels (Telegram/Discord config), cron (scheduled tasks), multi_channel (one core serving many ends: service connection + channels + device pairing), cluster (center/node cluster: enroll, node_invoke, node_file, approval)"
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
    MultiChannel,
    Cluster,
}

impl GuideTopic {
    /// Every topic the tool accepts, in schema order.
    ///
    /// Exists so that surfaces which *advertise* the topic list to the model
    /// (notably `self_manage`'s fallback manual) enumerate it instead of
    /// hand-spelling a copy — a hand-spelled copy had already drifted, omitting
    /// `multi_channel` and `cluster`, which reads to the model as "those
    /// domains have no guide".
    pub const ALL: &'static [Self] = &[
        Self::Overview,
        Self::Providers,
        Self::Mcp,
        Self::Skills,
        Self::Agents,
        Self::General,
        Self::Generation,
        Self::Channels,
        Self::Cron,
        Self::MultiChannel,
        Self::Cluster,
    ];

    /// Wire id — the exact string the `topic` argument accepts. Kept in
    /// lock-step with the `serde(rename_all = "snake_case")` above by
    /// [`tests::ids_match_the_serialized_form`].
    #[must_use]
    pub const fn id(&self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Providers => "providers",
            Self::Mcp => "mcp",
            Self::Skills => "skills",
            Self::Agents => "agents",
            Self::General => "general",
            Self::Generation => "generation",
            Self::Channels => "channels",
            Self::Cron => "cron",
            Self::MultiChannel => "multi_channel",
            Self::Cluster => "cluster",
        }
    }

    /// Comma-separated list of every accepted topic id.
    #[must_use]
    pub fn all_ids() -> String {
        Self::ALL
            .iter()
            .map(Self::id)
            .collect::<Vec<_>>()
            .join(", ")
    }

    const fn filename(&self) -> &'static str {
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
            Self::MultiChannel => "multi_channel.md",
            Self::Cluster => "cluster.md",
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
    #[must_use]
    pub const fn new(guides_dir: PathBuf) -> Self {
        Self { guides_dir }
    }

    /// Default guides directory: `<config_dir>/guides/`.
    ///
    /// ⚠️ Must match the *writer*: `start::mod` deploys the embedded guides to
    /// `utils::paths::get_config_dir()/guides`. Resolving the read side
    /// through `dirs::home_dir().join(".aleph")` instead — as this did — meant
    /// that under `ALEPH_HOME` the tool read a directory nothing ever wrote,
    /// and progressive disclosure (the spine of self-management) returned
    /// "guide not found" on a fully provisioned install.
    #[must_use]
    pub fn default_dir() -> PathBuf {
        crate::utils::paths::get_config_dir()
            .unwrap_or_else(|_| PathBuf::from(".aleph"))
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

        notify_tool_result(Self::NAME, &format!("loaded {topic_name} guide"), true);
        Ok(ReadConfigGuideOutput {
            success: true,
            topic: topic_name,
            content,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `id()` must be the serialized form, or a surface that advertises the
    /// list would be naming topics the argument rejects.
    #[test]
    fn ids_match_the_serialized_form() {
        for topic in GuideTopic::ALL {
            let wire = serde_json::to_value(topic).unwrap();
            assert_eq!(
                wire.as_str(),
                Some(topic.id()),
                "id() drifted from the serde representation for {topic:?}"
            );
            // And the wire id round-trips back into the enum, which is what
            // the model actually sends.
            let parsed: GuideTopic = serde_json::from_value(wire).unwrap();
            assert_eq!(parsed.filename(), topic.filename());
        }
    }

    /// Every declared topic must have a deployed guide file behind it —
    /// advertising a topic whose file was never embedded is a promise the
    /// tool cannot keep.
    #[test]
    fn every_topic_has_an_embedded_guide_file() {
        let embedded: Vec<&str> = crate::config::guides::GUIDE_FILES
            .iter()
            .map(|(name, _)| *name)
            .collect();
        for topic in GuideTopic::ALL {
            assert!(
                embedded.contains(&topic.filename()),
                "{} is offered as a topic but no guide file is embedded for it",
                topic.id()
            );
        }
    }

    #[test]
    fn new_topics_map_to_files() {
        assert_eq!(GuideTopic::MultiChannel.filename(), "multi_channel.md");
        assert_eq!(GuideTopic::Cluster.filename(), "cluster.md");
    }

    #[test]
    fn new_topics_deserialize_snake_case() {
        let m: GuideTopic = serde_json::from_str("\"multi_channel\"").unwrap();
        assert!(matches!(m, GuideTopic::MultiChannel));
        let c: GuideTopic = serde_json::from_str("\"cluster\"").unwrap();
        assert!(matches!(c, GuideTopic::Cluster));
    }
}
