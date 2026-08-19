//! A2A delegation + remote-agent management tools.
//!
//! Wires Aleph's A2A **outbound** path into the LLM tool surface. Before this,
//! `A2ASubAgent` and the whole `A2AClient` / `SmartRouter` / `CardRegistry`
//! stack were constructed at startup and immediately dropped — an Aleph agent
//! had no way to delegate a task to a *remote* A2A agent. These two tools close
//! that gap, honouring R8 ("Everything is a Tool").
//!
//! - `a2a_delegate` — hand a task to a remote A2A agent (auto-routed or pinned).
//! - `a2a_agents`   — list / add / remove remote A2A agents at runtime.

use arc_swap::ArcSwapOption;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{notify_tool_result, notify_tool_start};
use crate::a2a::adapter::client::A2AClient;
use crate::a2a::domain::TrustLevel;
use crate::a2a::port::{AgentHealth, AgentResolver, RegisteredAgent};
use crate::a2a::service::CardRegistry;
use crate::a2a::sub_agent::A2ASubAgent;
use crate::error::{AlephError, Result};
use crate::security::ssrf::{validate_url_async, SsrfPolicy};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Late-bound dependency handle
// =============================================================================

/// Late-bound dependencies shared by the A2A builtin tools.
///
/// The A2A client subsystem initialises *after* the builtin tool registry is
/// built, so the tools hold an [`A2AToolHandle`] that the startup sequence
/// fills in once `A2ASubAgent` + `CardRegistry` exist.
pub struct A2AToolDeps {
    /// Outbound delegation engine (smart routing + client pool).
    pub sub_agent: Arc<A2ASubAgent>,
    /// Registry of known remote agents (backs the `a2a_agents` tool).
    pub card_registry: Arc<CardRegistry>,
}

/// Shared, swappable handle to [`A2AToolDeps`].
///
/// Created empty before the builtin registry is built; populated during A2A
/// subsystem initialisation. `load()` returns `None` until then.
pub type A2AToolHandle = Arc<ArcSwapOption<A2AToolDeps>>;

/// Create an empty A2A tool handle.
#[must_use]
pub fn new_a2a_tool_handle() -> A2AToolHandle {
    Arc::new(ArcSwapOption::empty())
}

/// Resolve the handle or return a uniform "not ready" tool error.
fn load_deps(handle: &A2AToolHandle) -> Result<Arc<A2AToolDeps>> {
    handle.load_full().ok_or_else(|| {
        AlephError::tool(
            "A2A subsystem is not available. Enable it with `[a2a] enabled = true` \
             in the Aleph config and restart.",
        )
    })
}

// =============================================================================
// A2ADelegateTool — delegate a task to a remote A2A agent
// =============================================================================

/// Arguments for the `a2a_delegate` tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct A2ADelegateArgs {
    /// The task to delegate. Make it fully self-contained — the remote agent
    /// has no access to this conversation.
    pub prompt: String,
    /// Optional: pin the delegation to a specific remote agent by name or id
    /// (see `a2a_agents` with action `list`). When omitted, the best match is
    /// chosen automatically from the prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

/// Output of the `a2a_delegate` tool.
#[derive(Debug, Clone, Serialize)]
pub struct A2ADelegateOutput {
    /// Name of the remote agent that handled the task (absent when none matched).
    pub agent: Option<String>,
    /// Whether the delegation succeeded.
    pub success: bool,
    /// The remote agent's response summary, or the failure reason.
    pub result: String,
}

/// Delegate a task to a remote agent over the A2A protocol.
#[derive(Clone)]
pub struct A2ADelegateTool {
    handle: A2AToolHandle,
}

impl A2ADelegateTool {
    pub const fn new(handle: A2AToolHandle) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl AlephTool for A2ADelegateTool {
    const NAME: &'static str = "a2a_delegate";
    const DESCRIPTION: &'static str = "Delegate a task to a remote agent over the A2A \
        (Agent-to-Agent) protocol. Use this to hand specialised work to an external agent \
        registered via `a2a_agents`. The remote agent runs independently and returns a \
        result; it cannot see this conversation, so write a self-contained prompt.";

    type Args = A2ADelegateArgs;
    type Output = A2ADelegateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        notify_tool_start(
            Self::NAME,
            &crate::utils::text_format::truncate_text(&args.prompt, 80),
        );
        let deps = load_deps(&self.handle)?;

        // W16 — thread the delegating turn's real identity into the delegation
        // memory row (task-local turn context; `None` outside a scoped turn
        // falls back to the legacy "default" attribution).
        let outcome = deps
            .sub_agent
            .execute_delegation(
                &args.prompt,
                args.agent.as_deref(),
                crate::tools::turn_context::current_agent_id(),
                crate::tools::turn_context::current_session_key(),
            )
            .await?;

        let success = outcome.result.success;
        let result_text = if success {
            outcome.result.summary.clone()
        } else {
            outcome
                .result
                .error
                .clone()
                .unwrap_or_else(|| "A2A delegation failed".to_string())
        };
        notify_tool_result(
            Self::NAME,
            outcome.agent.as_deref().unwrap_or("no agent matched"),
            success,
        );

        Ok(A2ADelegateOutput {
            agent: outcome.agent,
            success,
            result: result_text,
        })
    }
}

// =============================================================================
// A2AAgentsTool — list / add / remove remote A2A agents
// =============================================================================

/// Action for the `a2a_agents` tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum A2AAgentsAction {
    /// List every registered remote A2A agent.
    List,
    /// Register a new remote agent — fetches and stores its Agent Card.
    Add,
    /// Remove a registered remote agent by id or name.
    Remove,
}

/// Arguments for the `a2a_agents` tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct A2AAgentsArgs {
    /// What to do: `list`, `add`, or `remove`.
    pub action: A2AAgentsAction,
    /// Base URL of the remote agent (required for `add`), e.g. `https://host:8080`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Agent id or name (required for `remove`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Optional bearer token for authenticating to the remote agent (`add` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// Compact view of one registered remote agent.
#[derive(Debug, Clone, Serialize)]
pub struct A2AAgentSummary {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub base_url: String,
    pub trust_level: String,
    pub health: String,
    pub skills: Vec<String>,
}

/// Output of the `a2a_agents` tool.
#[derive(Debug, Clone, Serialize)]
pub struct A2AAgentsOutput {
    /// The action performed.
    pub action: String,
    /// Human-readable result summary.
    pub message: String,
    /// Current registered agents (always returned so the model sees the result).
    pub agents: Vec<A2AAgentSummary>,
}

/// Manage the set of remote A2A agents Aleph can delegate to.
#[derive(Clone)]
pub struct A2AAgentsTool {
    handle: A2AToolHandle,
}

impl A2AAgentsTool {
    pub const fn new(handle: A2AToolHandle) -> Self {
        Self { handle }
    }
}

/// Map a [`RegisteredAgent`] to its compact summary.
fn summarize_agent(agent: &RegisteredAgent) -> A2AAgentSummary {
    A2AAgentSummary {
        id: agent.card.id.clone(),
        name: agent.card.name.clone(),
        version: agent.card.version.clone(),
        description: agent.card.description.clone(),
        base_url: agent.base_url.clone(),
        trust_level: format!("{:?}", agent.trust_level).to_lowercase(),
        health: format!("{:?}", agent.health).to_lowercase(),
        skills: agent.card.skills.iter().map(|s| s.name.clone()).collect(),
    }
}

/// List all registered agents as summaries.
async fn list_summaries(registry: &Arc<CardRegistry>) -> Result<Vec<A2AAgentSummary>> {
    let agents = registry
        .list_agents()
        .await
        .map_err(|e| AlephError::tool(format!("Failed to list A2A agents: {e}")))?;
    Ok(agents.iter().map(summarize_agent).collect())
}

#[async_trait]
impl AlephTool for A2AAgentsTool {
    const NAME: &'static str = "a2a_agents";
    const DESCRIPTION: &'static str = "Manage the set of remote A2A (Agent-to-Agent) agents \
        Aleph can delegate to. `list` shows registered agents and their skills; `add` \
        registers a new agent by URL (fetching its Agent Card); `remove` unregisters one. \
        Use `a2a_delegate` to actually send work to a registered agent.";

    type Args = A2AAgentsArgs;
    type Output = A2AAgentsOutput;

    /// Conditionally-required fields (`url` for add, `agent` for remove) make
    /// this schema unsuitable for strict mode.
    fn strict_schema(&self) -> bool {
        false
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let deps = load_deps(&self.handle)?;
        let registry = &deps.card_registry;

        match args.action {
            A2AAgentsAction::List => {
                notify_tool_start(Self::NAME, "list");
                let agents = list_summaries(registry).await?;
                let message = format!("{} remote A2A agent(s) registered", agents.len());
                notify_tool_result(Self::NAME, &message, true);
                Ok(A2AAgentsOutput {
                    action: "list".to_string(),
                    message,
                    agents,
                })
            }
            A2AAgentsAction::Add => {
                let url = args
                    .url
                    .clone()
                    .ok_or_else(|| AlephError::tool("`url` is required for action `add`"))?;
                notify_tool_start(
                    Self::NAME,
                    &format!("add {}", crate::utils::text_format::truncate_text(&url, 60)),
                );

                // BT-B-R4-05: validate the URL against the SSRF policy
                // before any outbound connection. Previously the URL was
                // passed straight to A2AClient::new / with_auth with no
                // host-policy gate, so an LLM-supplied url like
                // `http://169.254.169.254/latest/meta-data/` or
                // `http://127.0.0.1:8123/` would be happily registered and
                // called on every smart-route, with the persisted token
                // attached. validate_url_async returns the same
                // host-policy decision web_fetch uses, so the operator's
                // existing config controls both surfaces.
                let url_for_card = url::Url::parse(&url)
                    .map_err(|e| AlephError::tool(format!("`url` is not a valid URL: {e}")))?;
                let scheme = url_for_card.scheme();
                if scheme != "http" && scheme != "https" {
                    return Err(AlephError::tool(format!(
                        "`url` must be http:// or https:// (got '{scheme}://'); \
                         refusing to register a non-HTTP A2A agent"
                    )));
                }
                validate_url_async(&url, &SsrfPolicy::default())
                    .await
                    .map_err(|e| {
                        AlephError::tool(format!(
                            "`url` blocked by SSRF policy ({e}); \
                             A2A `add` rejects private/loopback hosts unless \
                             the operator widens the policy"
                        ))
                    })?;

                // Fetch the remote Agent Card so smart routing knows its skills.
                let client = match args.token.clone() {
                    Some(token) => A2AClient::with_auth(&url, token),
                    None => A2AClient::new(&url),
                };
                let card = client.fetch_agent_card().await.map_err(|e| {
                    AlephError::tool(format!(
                        "Could not reach A2A agent at {url}: {e}. \
                         Check the URL and that the remote agent is running."
                    ))
                })?;

                let trust = TrustLevel::infer_from_url(&url);
                // `upsert` (not the `AgentResolver::register` trait method) so
                // the auth token is preserved — outbound calls need it.
                registry
                    .upsert(RegisteredAgent::new(
                        card.clone(),
                        trust,
                        url.clone(),
                        chrono::Utc::now(),
                        AgentHealth::Healthy,
                        args.token.clone(),
                    ))
                    .await;

                let agents = list_summaries(registry).await?;
                let message = format!(
                    "Registered remote A2A agent '{}' ({} skill(s), trust={})",
                    card.name,
                    card.skills.len(),
                    format!("{trust:?}").to_lowercase(),
                );
                notify_tool_result(Self::NAME, &message, true);
                Ok(A2AAgentsOutput {
                    action: "add".to_string(),
                    message,
                    agents,
                })
            }
            A2AAgentsAction::Remove => {
                let needle = args.agent.clone().ok_or_else(|| {
                    AlephError::tool("`agent` (id or name) is required for action `remove`")
                })?;
                notify_tool_start(Self::NAME, &format!("remove {needle}"));

                // Resolve id-or-name to a concrete agent id.
                let all = registry
                    .list_agents()
                    .await
                    .map_err(|e| AlephError::tool(format!("Failed to list A2A agents: {e}")))?;
                let lc = needle.to_lowercase();
                let target_id = all
                    .iter()
                    .find(|a| a.card.id.to_lowercase() == lc || a.card.name.to_lowercase() == lc)
                    .map(|a| a.card.id.clone())
                    .ok_or_else(|| {
                        AlephError::tool(format!("No registered A2A agent matches '{needle}'"))
                    })?;

                registry
                    .unregister(&target_id)
                    .await
                    .map_err(|e| AlephError::tool(format!("Failed to remove agent: {e}")))?;

                let agents = list_summaries(registry).await?;
                let message = format!("Removed remote A2A agent '{needle}'");
                notify_tool_result(Self::NAME, &message, true);
                Ok(A2AAgentsOutput {
                    action: "remove".to_string(),
                    message,
                    agents,
                })
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::adapter::client::A2AClientPool;
    use crate::a2a::domain::AgentCard;
    use crate::a2a::service::SmartRouter;

    /// Build a filled handle backed by an empty in-memory `CardRegistry`,
    /// returning the registry too so tests can seed agents.
    fn build_handle() -> (A2AToolHandle, Arc<CardRegistry>) {
        let registry = Arc::new(CardRegistry::new());
        let router = Arc::new(SmartRouter::new(registry.clone()));
        let pool = Arc::new(A2AClientPool::new());
        let sub_agent = Arc::new(A2ASubAgent::new(router, pool));
        let handle = new_a2a_tool_handle();
        handle.store(Some(Arc::new(A2AToolDeps {
            sub_agent,
            card_registry: registry.clone(),
        })));
        (handle, registry)
    }

    /// A minimal `RegisteredAgent` for seeding the test registry.
    fn sample_registered(id: &str, name: &str, url: &str) -> RegisteredAgent {
        let card = AgentCard {
            id: id.to_string(),
            name: name.to_string(),
            version: "1.0".to_string(),
            description: None,
            provider: None,
            documentation_url: None,
            interfaces: vec![],
            skills: vec![],
            security: vec![],
            extensions: vec![],
            default_input_modes: vec![],
            default_output_modes: vec![],
        };
        RegisteredAgent::new(
            card,
            TrustLevel::Trusted,
            url.to_string(),
            chrono::Utc::now(),
            AgentHealth::Healthy,
            None,
        )
    }

    #[test]
    fn delegate_args_deserialize_minimal() {
        let args: A2ADelegateArgs = serde_json::from_str(r#"{"prompt":"do x"}"#).unwrap();
        assert_eq!(args.prompt, "do x");
        assert_eq!(args.agent, None);
    }

    #[test]
    fn delegate_args_deserialize_with_agent() {
        let args: A2ADelegateArgs =
            serde_json::from_str(r#"{"prompt":"do x","agent":"trader"}"#).unwrap();
        assert_eq!(args.agent.as_deref(), Some("trader"));
    }

    #[test]
    fn agents_action_serde_is_snake_case() {
        let a: A2AAgentsAction = serde_json::from_str(r#""list""#).unwrap();
        assert_eq!(a, A2AAgentsAction::List);
        let a: A2AAgentsAction = serde_json::from_str(r#""add""#).unwrap();
        assert_eq!(a, A2AAgentsAction::Add);
        let a: A2AAgentsAction = serde_json::from_str(r#""remove""#).unwrap();
        assert_eq!(a, A2AAgentsAction::Remove);
    }

    #[tokio::test]
    async fn delegate_unavailable_when_handle_empty() {
        let tool = A2ADelegateTool::new(new_a2a_tool_handle());
        let err = tool
            .call(A2ADelegateArgs {
                prompt: "hi".to_string(),
                agent: None,
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("A2A subsystem"));
    }

    #[tokio::test]
    async fn delegate_no_agents_returns_unsuccessful_output() {
        let (handle, _) = build_handle();
        let tool = A2ADelegateTool::new(handle);
        let out = tool
            .call(A2ADelegateArgs {
                prompt: "do x".to_string(),
                agent: None,
            })
            .await
            .unwrap();
        assert!(!out.success);
        assert!(out.agent.is_none());
    }

    #[tokio::test]
    async fn agents_list_is_empty_initially() {
        let (handle, _) = build_handle();
        let tool = A2AAgentsTool::new(handle);
        let out = tool
            .call(A2AAgentsArgs {
                action: A2AAgentsAction::List,
                url: None,
                agent: None,
                token: None,
            })
            .await
            .unwrap();
        assert_eq!(out.action, "list");
        assert!(out.agents.is_empty());
    }

    #[tokio::test]
    async fn agents_add_without_url_errors() {
        let (handle, _) = build_handle();
        let tool = A2AAgentsTool::new(handle);
        let err = tool
            .call(A2AAgentsArgs {
                action: A2AAgentsAction::Add,
                url: None,
                agent: None,
                token: None,
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("url"));
    }

    #[tokio::test]
    async fn agents_remove_unknown_errors() {
        let (handle, _) = build_handle();
        let tool = A2AAgentsTool::new(handle);
        let err = tool
            .call(A2AAgentsArgs {
                action: A2AAgentsAction::Remove,
                url: None,
                agent: Some("ghost".to_string()),
                token: None,
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("ghost"));
    }

    #[tokio::test]
    async fn agents_list_then_remove_roundtrip() {
        let (handle, registry) = build_handle();
        registry
            .upsert(sample_registered(
                "rev",
                "Reviewer",
                "https://r.example.com",
            ))
            .await;
        let tool = A2AAgentsTool::new(handle);

        // list shows the seeded agent
        let out = tool
            .call(A2AAgentsArgs {
                action: A2AAgentsAction::List,
                url: None,
                agent: None,
                token: None,
            })
            .await
            .unwrap();
        assert_eq!(out.agents.len(), 1);
        assert_eq!(out.agents[0].name, "Reviewer");

        // remove by name resolves to the id and unregisters
        let out = tool
            .call(A2AAgentsArgs {
                action: A2AAgentsAction::Remove,
                url: None,
                agent: Some("Reviewer".to_string()),
                token: None,
            })
            .await
            .unwrap();
        assert!(out.agents.is_empty());
    }
}
