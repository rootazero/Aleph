//! `TeamDigestTool` — generate a summary of recent team activity.

use async_trait::async_trait;
use chrono::{Duration, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::builtin_tools::acting_agent::acting_agent_id;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::events::{EventLogStore, TeamEvent};
use crate::teams::TeamStore;
use crate::tools::AlephTool;
// UTF-8 safe truncation by character count, no marker — digest lines are
// reassembled downstream, so a stray ellipsis would leak into the payload.
use crate::utils::text_format::truncate_chars as truncate_str;

// =============================================================================
// Args / Output
// =============================================================================

const fn default_24() -> u64 {
    24
}

/// Arguments for generating a team activity digest.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TeamDigestArgs {
    /// ID of the team to generate a digest for
    pub team_id: String,
    /// Number of hours to look back (default: 24)
    #[serde(default = "default_24")]
    pub hours: u64,
}

/// Output from `team_digest`.
#[derive(Debug, Serialize)]
pub struct TeamDigestOutput {
    pub team_id: String,
    pub period_start: String,
    pub period_end: String,
    pub event_count: usize,
    /// Formatted events for LLM to process
    pub events_summary: String,
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that generates a summary of recent team activity for a specified period.
#[derive(Clone)]
pub struct TeamDigestTool {
    team_store: Arc<dyn TeamStore>,
    event_store: Arc<dyn EventLogStore>,
    current_agent_id: String,
}

impl TeamDigestTool {
    /// The agent acting in THIS call — the identity of the running turn, not
    /// the one this tool was constructed with. See [`acting_agent_id`].
    fn actor(&self) -> String {
        acting_agent_id(&self.current_agent_id)
    }

    pub fn new(
        team_store: Arc<dyn TeamStore>,
        event_store: Arc<dyn EventLogStore>,
        current_agent_id: String,
    ) -> Self {
        Self {
            team_store,
            event_store,
            current_agent_id,
        }
    }
}

/// Format a single event into a readable line.
fn format_event(event: &TeamEvent) -> String {
    let ts = event.timestamp.format("%Y-%m-%d %H:%M:%S UTC");
    let event_type = event.event_type.as_str();
    let agent = &event.agent_id;

    // Extract a brief summary from the payload
    let brief = if event.payload.is_null()
        || event.payload == serde_json::Value::Object(Default::default())
    {
        String::new()
    } else if let Some(obj) = event.payload.as_object() {
        // Pick the most informative fields
        let parts: Vec<String> = obj
            .iter()
            .take(3)
            .map(|(k, v)| {
                let val_str = match v {
                    serde_json::Value::String(s) => {
                        let truncated = truncate_str(s, 80);
                        if truncated.len() < s.len() {
                            format!("{truncated}...")
                        } else {
                            s.clone()
                        }
                    }
                    other => {
                        let s = other.to_string();
                        let truncated = truncate_str(&s, 80);
                        if truncated.len() < s.len() {
                            format!("{truncated}...")
                        } else {
                            s
                        }
                    }
                };
                format!("{k}={val_str}")
            })
            .collect();
        format!(" ({})", parts.join(", "))
    } else {
        let s = event.payload.to_string();
        let truncated = truncate_str(&s, 100);
        if truncated.len() < s.len() {
            format!(" ({truncated}...)")
        } else {
            format!(" ({s})")
        }
    };

    format!("[{ts}] {event_type} by {agent}{brief}")
}

#[async_trait]
impl AlephTool for TeamDigestTool {
    const NAME: &'static str = "team_digest";
    const DESCRIPTION: &'static str =
        "Generate a summary of recent team activity for the specified time period. \
        Returns raw event data for you to synthesize into a digest.";

    type Args = TeamDigestArgs;
    type Output = TeamDigestOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Verify team exists
        let team = self
            .team_store
            .get_team(&args.team_id)
            .await?
            .ok_or_else(|| AlephError::other(format!("Team '{}' not found", args.team_id)))?;

        let now = Utc::now();
        // `hours` arrives from the LLM (untrusted); chrono::Duration::hours
        // panics on out-of-range values, so clamp to a generous upper bound
        // (~100 years) before converting.
        let hours = args.hours.min(24 * 366 * 100);
        let since = now - Duration::hours(hours as i64);

        let events = self
            .event_store
            .get_events(&args.team_id, Some(since), Some(now))
            .await?;

        let event_count = events.len();

        let events_summary = if events.is_empty() {
            "No events recorded in this period.".to_string()
        } else {
            events
                .iter()
                .map(format_event)
                .collect::<Vec<_>>()
                .join("\n")
        };

        let is_leader = team.leader_id == self.actor();

        debug!(
            team_id = %args.team_id,
            hours = args.hours,
            event_count,
            is_leader,
            "team_digest: generated"
        );

        let message = if is_leader {
            format!(
                "Digest generated for team '{}' ({} events in last {} hours). You are the team leader.",
                team.name, event_count, args.hours
            )
        } else {
            format!(
                "Digest generated for team '{}' ({} events in last {} hours).",
                team.name, event_count, args.hours
            )
        };

        Ok(TeamDigestOutput {
            team_id: args.team_id,
            period_start: since.to_rfc3339(),
            period_end: now.to_rfc3339(),
            event_count,
            events_summary,
            message,
        })
    }
}
