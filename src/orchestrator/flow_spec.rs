//! Declarative flow configuration. See Phase 5 design §5.

use serde::{Deserialize, Serialize};

use crate::agents::ContextMode;
use crate::session::events::MessageContent;

pub type FlowId = String;
pub type AgentId = String;
pub type ProviderId = String;

/// Gateway-agnostic input envelope for a Flow dispatch.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum FlowInput {
    /// Single user prompt. Seeded as one `UserMessage` session event.
    Prompt(String),
    /// Pre-assembled user messages, seeded one event per entry.
    Messages(Vec<MessageContent>),
    /// Multi-turn history plus a new user prompt. History turns are replayed
    /// as the corresponding `UserMessage` / `AssistantMessage` events before
    /// the prompt is emitted as a fresh `UserMessage`.
    History {
        turns: Vec<FlowHistoryTurn>,
        prompt: String,
    },
    /// Multimodal user messages (one per entry). Each `MessageContent` can
    /// carry `blocks` referencing images, files, or other non-text payloads;
    /// the harness delegates interpretation to the LLM provider.
    Multimodal(Vec<MessageContent>),
    /// Resume an interrupted run. Carries no input: the session event log
    /// already holds the full trajectory (including the original
    /// `UserMessage`). `seed_session` treats this as a no-op so replay is
    /// not corrupted by a duplicate user message.
    Resume,
}

/// One role-tagged turn in a replayed history. Used only by
/// [`FlowInput::History`] for seeding the session log.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum FlowHistoryTurn {
    User(MessageContent),
    Assistant(MessageContent),
}

impl FlowInput {
    /// True when this run's replayed history already carries at least one
    /// `<session_context>` compaction-summary block (injected upstream by
    /// `memory::session_compactor::prepare_history` when raw history exceeds
    /// the fresh tail). Threaded into the prompt builder so
    /// `SessionContextGuideLayer` surfaces its "summaries are lossy — search,
    /// don't guess" guide only when summaries are actually present. `prepare_history`
    /// runs before the runner, so the flag is timely for this same run.
    #[must_use]
    pub fn carries_session_summaries(&self) -> bool {
        match self {
            FlowInput::History { turns, .. } => turns.iter().any(|turn| match turn {
                FlowHistoryTurn::User(mc) => mc.text.contains("<session_context"),
                FlowHistoryTurn::Assistant(_) => false,
            }),
            _ => false,
        }
    }
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
    #[serde(default = "default_flow_priority")]
    pub priority: u8,
    #[serde(default)]
    pub overrides: FlowOverrides,
}

const fn default_flow_priority() -> u8 {
    128
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
    /// Spawn a new child session linked to a parent.
    ///
    /// The parent is resolved in this order:
    /// 1. `parent_session` field from the runtime `FlowRequest` (if present)
    /// 2. `parent_session_key` from this config field (TOML static value)
    ///
    /// This means a TOML config author should treat `parent_session_key` as a
    /// fallback default, not the primary binding — the runtime always wins.
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
