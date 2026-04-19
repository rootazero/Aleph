//! Declarative flow configuration. See Phase 5 design §5.

use serde::{Deserialize, Serialize};

use crate::agents::ContextMode;
use crate::session::events::MessageContent;

pub type FlowId = String;
pub type AgentId = String;
pub type ProviderId = String;

/// Gateway-agnostic input envelope for a Flow dispatch.
#[derive(Debug, Clone)]
pub enum FlowInput {
    Prompt(String),
    Messages(Vec<MessageContent>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowSpec {
    pub id: FlowId,
    pub description: String,
    pub agent: AgentId,
    pub brain: BrainRef,
    pub sandbox_kind: SandboxKind,
    pub session_strategy: SessionStrategy,
    #[serde(default)]
    pub overrides: FlowOverrides,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum BrainRef {
    Default,
    Preferred {
        provider: ProviderId,
    },
    Strict {
        provider: ProviderId,
        #[serde(default)]
        model: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SandboxKind {
    None,
    Workspace,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum SessionStrategy {
    Reuse,
    Fresh,
    Child {
        #[serde(default)]
        parent_session_key: Option<String>,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowOverrides {
    #[serde(default)]
    pub max_iterations: Option<u32>,
    #[serde(default)]
    pub context_mode: Option<ContextMode>,
    #[serde(default)]
    pub extra_system_prompt: Option<String>,
}
