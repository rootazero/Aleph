//! `TeamUsageTool` — per-team provider token aggregation.
//!
//! Sister to the `teams.usage` RPC handler: both walk the same `SQLite`
//! `task_traces` table for `ProviderUsage` events filtered to the agents
//! belonging to a given team. The tool surface (R8) lets the LLM ask
//! "how much have we spent in this team this week?" via natural language;
//! the RPC surface lets the panel render the same numbers in a dashboard.
//!
//! Cost is intentionally not computed here. Tokens are factual; cost is a
//! rate-card concern that lives in the provider config / UI overlay.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::builtin_tools::acting_agent::acting_agent_id;
use crate::error::{AlephError, Result};
use crate::resilience::{AgentUsageTotal, StateDatabase};
use crate::sync_primitives::Arc;
use crate::teams::TeamStore;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TeamUsageArgs {
    /// The team to aggregate over.
    pub team_id: String,
    /// Inclusive lower bound on trace `timestamp` (epoch seconds). Omitted =
    /// no lower bound.
    #[serde(default)]
    pub since: Option<i64>,
    /// Inclusive upper bound on trace `timestamp` (epoch seconds). Omitted =
    /// no upper bound.
    #[serde(default)]
    pub until: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct UsageTotal {
    pub call_count: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub reasoning_tokens: u64,
    /// Share of prompt tokens served from the provider's prompt cache, in
    /// `[0.0, 1.0]`. `None` when no usage was observed at all; `Some(0.0)`
    /// when input occurred but no cache reads were reported.
    ///
    /// Computed as `cache_read / (input + cache_read)`. `input_tokens` and
    /// `cache_read_tokens` are disjoint for every provider — each adapter
    /// normalises its protocol's usage before the numbers are stored — so the
    /// same formula is comparable across providers and across agents.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cache_hit_ratio: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TeamUsageOutput {
    pub team_id: String,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub member_count: usize,
    pub total: UsageTotal,
    pub per_agent: Vec<AgentUsageTotal>,
    /// Rolled-up spend from MoA advisor calls (synthetic `moa:<idx>:...`
    /// agent ids), which never appear in `per_agent` since advisors aren't
    /// team members. `None` when no advisor usage exists in the window
    /// (round-2 B6).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub moa_advisors: Option<AgentUsageTotal>,
}

// =============================================================================
// Tool
// =============================================================================

#[derive(Clone)]
pub struct TeamUsageTool {
    team_store: Arc<dyn TeamStore>,
    state_db: Arc<StateDatabase>,
    /// Injected by `ExecutionEngine` before each call (for audit logging only).
    pub current_agent_id: String,
}

impl TeamUsageTool {
    /// The agent acting in THIS call — the identity of the running turn, not
    /// the one this tool was constructed with. See [`acting_agent_id`].
    fn actor(&self) -> String {
        acting_agent_id(&self.current_agent_id)
    }

    pub fn new(
        team_store: Arc<dyn TeamStore>,
        state_db: Arc<StateDatabase>,
        current_agent_id: impl Into<String>,
    ) -> Self {
        Self {
            team_store,
            state_db,
            current_agent_id: current_agent_id.into(),
        }
    }
}

#[async_trait]
impl AlephTool for TeamUsageTool {
    const NAME: &'static str = "team_usage";
    const DESCRIPTION: &'static str = "Aggregate LLM provider token usage for a team over an \
        optional time window. Returns the team total plus a per-agent breakdown (call_count, \
        input/output/cache_read/cache_creation/reasoning tokens). Cost is NOT computed — \
        tokens are factual, cost depends on per-model rate cards that live elsewhere. Use \
        when the user asks 'how much have we spent', 'which agent is the heaviest user', or \
        wants to compare team activity across a window. `since` and `until` are epoch \
        seconds (the same units stored in task_traces.timestamp); omit them for all-time.";

    type Args = TeamUsageArgs;
    type Output = TeamUsageOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(
            team_id = %args.team_id,
            since = ?args.since,
            until = ?args.until,
            caller = %self.actor(),
            "team_usage: dispatch"
        );

        // Validate team exists so the caller gets a clear NotFound instead of
        // a silent zero-row response.
        let _team = self
            .team_store
            .get_team(&args.team_id)
            .await
            .map_err(|e| AlephError::other(format!("team_usage: load team failed: {e}")))?
            .ok_or_else(|| AlephError::NotFound(format!("team `{}` not found", args.team_id)))?;

        let members = self
            .team_store
            .get_members(&args.team_id)
            .await
            .map_err(|e| AlephError::other(format!("team_usage: load members failed: {e}")))?;
        let agent_ids: Vec<String> = members.iter().map(|m| m.agent_id.clone()).collect();

        let per_agent = self
            .state_db
            .aggregate_usage_by_agents(&agent_ids, args.since, args.until)
            .await
            .map_err(|e| AlephError::other(format!("team_usage: aggregation failed: {e}")))?;

        let mut total = UsageTotal::default();
        for row in &per_agent {
            total.call_count = total.call_count.saturating_add(row.call_count);
            total.input_tokens = total.input_tokens.saturating_add(row.input_tokens);
            total.output_tokens = total.output_tokens.saturating_add(row.output_tokens);
            total.cache_read_tokens = total
                .cache_read_tokens
                .saturating_add(row.cache_read_tokens);
            total.cache_creation_tokens = total
                .cache_creation_tokens
                .saturating_add(row.cache_creation_tokens);
            total.reasoning_tokens = total.reasoning_tokens.saturating_add(row.reasoning_tokens);
        }
        // Derive the ratio after the sum so the rollup denominator matches the
        // per-agent rows. Reusing the per-row method keeps the disjoint
        // `input + cache_read` denominator identical at both layers.
        total.cache_hit_ratio = AgentUsageTotal {
            agent_id: String::new(),
            call_count: total.call_count,
            input_tokens: total.input_tokens,
            output_tokens: total.output_tokens,
            cache_read_tokens: total.cache_read_tokens,
            cache_creation_tokens: total.cache_creation_tokens,
            reasoning_tokens: total.reasoning_tokens,
        }
        .cache_hit_ratio();

        // MoA advisor spend (round-2 B6) — advisors are metered under
        // synthetic agent ids that never appear in `agent_ids`, so they're
        // invisible to the per-agent aggregation above. Roll them into one
        // dedicated bucket instead. Supplementary field: failure degrades to
        // "no advisor usage" rather than failing the whole tool call.
        let moa_advisors = self
            .state_db
            .aggregate_moa_advisor_usage(args.since, args.until)
            .await
            .ok()
            .flatten();

        Ok(TeamUsageOutput {
            team_id: args.team_id,
            since: args.since,
            until: args.until,
            member_count: agent_ids.len(),
            total,
            per_agent,
            moa_advisors,
        })
    }
}
