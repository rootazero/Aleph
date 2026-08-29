//! `TeamSetProtocolTool` — set or clear a team's operating protocol.
//!
//! The protocol is a free-form operating agreement (role definitions, hand-off
//! rules, quality gates) authored by the leader. Once set, the dispatcher's
//! handoff-context builder injects it verbatim into every member's launch
//! context, so the whole team works under one shared agreement. This tool is
//! the in-conversation editor for that agreement (R8: configuration is a tool);
//! `team_create` can seed it at creation time.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::teams::TeamStore;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for setting a team's operating protocol.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TeamSetProtocolArgs {
    /// ID of the team whose protocol to set.
    pub team_id: String,
    /// The protocol text. Omit (or pass an empty/whitespace-only string) to
    /// clear the protocol — members will then run with no shared agreement.
    #[serde(default)]
    pub protocol: Option<String>,
}

/// Output from `team_set_protocol`.
#[derive(Debug, Clone, Serialize)]
pub struct TeamSetProtocolOutput {
    pub team_id: String,
    /// `true` when a non-empty protocol is now in effect, `false` when cleared.
    pub protocol_set: bool,
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that sets (or clears) a team's operating protocol.
#[derive(Clone)]
pub struct TeamSetProtocolTool {
    store: Arc<dyn TeamStore>,
}

impl TeamSetProtocolTool {
    pub fn new(store: Arc<dyn TeamStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl AlephTool for TeamSetProtocolTool {
    const NAME: &'static str = "team_set_protocol";
    const DESCRIPTION: &'static str =
        "Set or update a team's operating protocol — the shared agreement (role definitions, \
        hand-off rules, quality standards) injected verbatim into every member's context when \
        they are launched for a task. Use this to align the whole team on how to collaborate, \
        or to amend the rules mid-flight. Pass an empty/omitted `protocol` to clear it. \
        Leader-driven; takes effect on the next task hand-off.";

    type Args = TeamSetProtocolArgs;
    type Output = TeamSetProtocolOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // BT-D-R4-23: gate before any store mutation — the protocol is
        // injected verbatim into every member's launch context, so an
        // attacker setting it on a team they don't belong to would
        // hijack the next member launch. The shared team-auth helper
        // also matches the NotFound-shaped refusal the rest of the
        // team tools use, so probing for foreign team ids returns the
        // same shape as a genuinely-missing team.
        super::require_team_auth(&*self.store, &args.team_id, &self.actor()).await?;

        // Determine the resulting state before the write so the message is
        // accurate (the store normalizes whitespace-only input to "cleared").
        let will_be_set = args
            .protocol
            .as_deref()
            .is_some_and(|p| !p.trim().is_empty());

        self.store
            .set_protocol(&args.team_id, args.protocol.clone())
            .await?;

        info!(
            team_id = %args.team_id,
            protocol_set = will_be_set,
            "team_set_protocol: updated"
        );

        let message = if will_be_set {
            format!("Protocol set for team '{}'.", args.team_id)
        } else {
            format!("Protocol cleared for team '{}'.", args.team_id)
        };

        Ok(TeamSetProtocolOutput {
            team_id: args.team_id,
            protocol_set: will_be_set,
            message,
        })
    }
}
