//! `TeamDisbandTool` — mark a team as disbanded.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::teams::events::EventLogStore;
use crate::teams::messages::MessageStore;
use crate::teams::sessions::SessionStore;
use crate::teams::TeamStore;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for disbanding a team.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TeamDisbandArgs {
    /// ID of the team to disband
    pub team_id: String,
}

/// Output from `team_disband`.
#[derive(Debug, Clone, Serialize)]
pub struct TeamDisbandOutput {
    pub team_id: String,
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that marks a team as disbanded.
///
/// A disbanded team is no longer active. Its records are preserved for
/// history. Use `team_delete` (if available) to permanently remove the record.
#[derive(Clone)]
pub struct TeamDisbandTool {
    store: Arc<dyn TeamStore>,
    msg_store: Option<Arc<dyn MessageStore>>,
    session_store: Option<Arc<dyn SessionStore>>,
    event_store: Option<Arc<dyn EventLogStore>>,
    current_agent_id: String,
}

impl TeamDisbandTool {
    pub fn new(store: Arc<dyn TeamStore>, current_agent_id: String) -> Self {
        Self {
            store,
            msg_store: None,
            session_store: None,
            event_store: None,
            current_agent_id,
        }
    }

    /// The agent acting in THIS call — the identity of the running turn, not
    /// the one this tool was constructed with. See [`acting_agent_id`].
    fn actor(&self) -> String {
        crate::builtin_tools::acting_agent::acting_agent_id(&self.current_agent_id)
    }

    /// Set optional cleanup stores for post-disband resource cleanup.
    #[must_use]
    pub fn with_cleanup_stores(
        mut self,
        msg_store: Option<Arc<dyn MessageStore>>,
        session_store: Option<Arc<dyn SessionStore>>,
        event_store: Option<Arc<dyn EventLogStore>>,
    ) -> Self {
        self.msg_store = msg_store;
        self.session_store = session_store;
        self.event_store = event_store;
        self
    }
}

#[async_trait]
impl AlephTool for TeamDisbandTool {
    const NAME: &'static str = "team_disband";
    const DESCRIPTION: &'static str =
        "Mark a team as disbanded. The team's history is preserved but it becomes \
        inactive. Members and tasks are retained for reference. \
        This action cannot be undone.";

    type Args = TeamDisbandArgs;
    type Output = TeamDisbandOutput;

    fn requires_confirmation(&self) -> bool {
        true
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // BT-D-R4-23: gate before any store mutation — the previous shape
        // relied on `requires_confirmation` (a UX safeguard) as the only
        // gate, which is bypassable. Verify the caller is the team's
        // leader or a member.
        super::require_team_auth(&*self.store, &args.team_id, &self.actor()).await?;
        self.store.disband_team(&args.team_id).await?;

        info!(team_id = %args.team_id, "team_disband: team disbanded");

        // Victory-claim trigger: a disband is the team's "we're done" moment —
        // poke any watcher paired to `team:<id>` in the governance graph
        // (best-effort; a no-op when the team was never explicitly paired).
        // No one-shot claim guards this one (a disband happens once), so the
        // "was anything poked" answer has no caller here.
        let _ = crate::loop_graph::service::notify_team_settled(&args.team_id).await;

        // Post-disband cleanup: expire messages, cancel sessions, prune events.
        // Failures are logged but do not fail the disband operation.
        if let Some(ref ms) = self.msg_store {
            match ms.expire_all_for_team(&args.team_id).await {
                Ok(n) => info!(team_id = %args.team_id, expired = n, "expired pending messages"),
                Err(e) => warn!(team_id = %args.team_id, err = %e, "failed to expire messages"),
            }
        }
        if let Some(ref ss) = self.session_store {
            match ss.cancel_all_for_team(&args.team_id).await {
                Ok(n) => info!(team_id = %args.team_id, cancelled = n, "cancelled active sessions"),
                Err(e) => warn!(team_id = %args.team_id, err = %e, "failed to cancel sessions"),
            }
        }
        if let Some(ref es) = self.event_store {
            match es
                .prune_events(&args.team_id, chrono::Duration::hours(24))
                .await
            {
                Ok(n) => info!(team_id = %args.team_id, pruned = n, "pruned old events"),
                Err(e) => warn!(team_id = %args.team_id, err = %e, "failed to prune events"),
            }
        }

        Ok(TeamDisbandOutput {
            team_id: args.team_id.clone(),
            message: format!("Team '{}' has been disbanded.", args.team_id),
        })
    }
}
