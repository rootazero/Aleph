//! Heartbeat management tools — create, list, update, delete, and toggle monitoring tasks.
//!
//! Exposes the heartbeat service CRUD API as LLM-callable tools, letting users manage
//! periodic monitoring tasks through natural language (R8 LLM Sovereignty).
//!
//! Also provides `heartbeat_report`, the L2 output tool used during heartbeat execution
//! for the agent to report its findings.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::Result;
use crate::tasks::heartbeat::service::ops::HeartbeatTaskUpdates;
use crate::tasks::heartbeat::{
    config::{HeartbeatTask, HeartbeatTaskView, ProbeConfig, TriggerCondition},
    SharedHeartbeatService,
};
use crate::tasks::shared::active_hours::ActiveHoursSchedule;
use crate::tasks::shared::alert::FailureAlertConfig;
use crate::tasks::shared::clock::SystemClock;
use crate::tools::AlephTool;

// =============================================================================
// Heartbeat List Tool
// =============================================================================

/// Arguments for `heartbeat_list` — no parameters required
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct HeartbeatListArgs {}

/// Output from `heartbeat_list`
#[derive(Debug, Clone, Serialize)]
pub struct HeartbeatListOutput {
    /// Human-readable status message
    pub message: String,
    /// All heartbeat tasks
    pub tasks: Vec<HeartbeatTaskView>,
}

/// Tool for listing all heartbeat monitoring tasks.
#[derive(Clone)]
pub struct HeartbeatListTool {
    service: SharedHeartbeatService,
}

impl HeartbeatListTool {
    pub const fn new(service: SharedHeartbeatService) -> Self {
        Self { service }
    }
}

#[async_trait]
impl AlephTool for HeartbeatListTool {
    const NAME: &'static str = "heartbeat_list";
    const DESCRIPTION: &'static str =
        "List all heartbeat monitoring tasks with their status, schedule, and last probe results. \
         Use this to see what monitoring checks are configured.";

    type Args = HeartbeatListArgs;
    type Output = HeartbeatListOutput;

    async fn call(&self, _args: Self::Args) -> Result<Self::Output> {
        let service = self.service.lock().await;
        let tasks = service.list_tasks().await;
        let count = tasks.len();
        Ok(HeartbeatListOutput {
            message: format!("{count} heartbeat monitoring tasks"),
            tasks,
        })
    }
}

// =============================================================================
// Heartbeat Create Tool
// =============================================================================

/// Trigger condition variants for LLM input
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TriggerConditionInput {
    /// Always trigger L2 analysis on every probe
    Always,
    /// Trigger if the probe output is non-empty
    NonEmpty,
    /// Trigger if the probe output has changed since last run
    Changed,
    /// Trigger if the numeric output is greater than the given value
    GreaterThan { threshold: f64 },
    /// Trigger if the probe output contains the given substring
    Contains { text: String },
}

impl From<TriggerConditionInput> for TriggerCondition {
    fn from(input: TriggerConditionInput) -> Self {
        match input {
            TriggerConditionInput::Always => Self::Always,
            TriggerConditionInput::NonEmpty => Self::NonEmpty,
            TriggerConditionInput::Changed => Self::Changed,
            TriggerConditionInput::GreaterThan { threshold } => Self::GreaterThan(threshold),
            TriggerConditionInput::Contains { text } => Self::Contains(text),
        }
    }
}

/// Arguments for `heartbeat_create`
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct HeartbeatCreateArgs {
    /// Human-readable name for this monitoring task
    pub name: String,
    /// Probe tool to call for L1 monitoring (e.g. "`gmail.unread_count`")
    pub probe_tool_name: String,
    /// How often to run the probe, in milliseconds (e.g. 300000 for 5 minutes)
    pub interval_ms: u64,
    /// Condition that triggers L2 agent analysis (default: always)
    #[serde(default)]
    pub probe_trigger_condition: Option<TriggerConditionInput>,
    /// Agent that handles L2 analysis when triggered (default: "main")
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Optional parameters to pass to the probe tool
    #[serde(default)]
    pub probe_tool_params: Option<serde_json::Value>,
    /// Alert when this monitor itself keeps failing (probe error, L2 error, or
    /// a delivery its configured target refused). Without it the failure only
    /// lengthens the retry backoff and nobody is told.
    #[serde(default)]
    pub failure_alert: Option<FailureAlertConfig>,
}

/// Output from `heartbeat_create`
#[derive(Debug, Clone, Serialize)]
pub struct HeartbeatCreateOutput {
    /// Human-readable status message
    pub message: String,
    /// Created task ID
    pub task_id: String,
}

/// Tool for creating a new heartbeat monitoring task.
#[derive(Clone)]
pub struct HeartbeatCreateTool {
    service: SharedHeartbeatService,
}

impl HeartbeatCreateTool {
    pub const fn new(service: SharedHeartbeatService) -> Self {
        Self { service }
    }
}

#[async_trait]
impl AlephTool for HeartbeatCreateTool {
    const NAME: &'static str = "heartbeat_create";
    const DESCRIPTION: &'static str =
        "Create a new heartbeat monitoring task. Use this when the user wants to periodically \
         check something — e.g., 'monitor my Gmail every 5 minutes', 'alert me when the server \
         CPU exceeds 80%', 'check if new papers are published daily'.";

    type Args = HeartbeatCreateArgs;
    type Output = HeartbeatCreateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Floor the interval (consistent with cron's Every guard). A sub-second
        // interval makes the probe — and any L2 agent run it triggers — fire on
        // every tick, a resource hazard.
        if args.interval_ms < 1000 {
            return Err(crate::error::AlephError::tool(format!(
                "Interval too short: interval_ms={} is below the 1000ms minimum.",
                args.interval_ms
            )));
        }

        let trigger_condition = args
            .probe_trigger_condition
            .map_or(TriggerCondition::Always, TriggerCondition::from);

        let probe = ProbeConfig {
            tool_name: args.probe_tool_name,
            tool_params: args.probe_tool_params,
            trigger_condition,
        };

        let agent_id = args.agent_id.unwrap_or_else(|| "main".to_string());
        let mut task = HeartbeatTask::new(args.name.clone(), agent_id, args.interval_ms, probe);
        task.failure_alert = args.failure_alert;

        let clock = SystemClock;
        let id = {
            let service = self.service.lock().await;
            service
                .add_task(task, &clock)
                .await
                .map_err(crate::error::AlephError::tool)?
        };

        info!(task_id = %id, name = %args.name, "Heartbeat task created via tool");

        Ok(HeartbeatCreateOutput {
            message: format!("Heartbeat task '{}' created (ID: {})", args.name, id),
            task_id: id,
        })
    }
}

// =============================================================================
// Heartbeat Update Tool
// =============================================================================

/// Arguments for `heartbeat_update`
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct HeartbeatUpdateArgs {
    /// Task ID to update, from `heartbeat_list` (required)
    pub id: String,
    /// New name (optional)
    #[serde(default)]
    pub name: Option<String>,
    /// New agent ID (optional)
    #[serde(default)]
    pub agent_id: Option<String>,
    /// New interval in milliseconds (optional)
    #[serde(default)]
    pub interval_ms: Option<u64>,
    /// Enable or disable the task (optional)
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Active hours schedule to restrict task execution windows (optional)
    #[serde(default)]
    pub active_hours: Option<ActiveHoursSchedule>,
    /// Replace the failure alerting for this monitor (optional)
    #[serde(default)]
    pub failure_alert: Option<FailureAlertConfig>,
    /// Pass `true` to remove an existing `failure_alert`. (A JSON tool call
    /// cannot distinguish `null` from "not mentioned", hence the flag.)
    #[serde(default)]
    pub clear_failure_alert: Option<bool>,
}

/// Output from `heartbeat_update`
#[derive(Debug, Clone, Serialize)]
pub struct HeartbeatUpdateOutput {
    /// Human-readable status message
    pub message: String,
    /// Updated task ID
    pub task_id: String,
}

/// Tool for updating an existing heartbeat monitoring task.
#[derive(Clone)]
pub struct HeartbeatUpdateTool {
    service: SharedHeartbeatService,
}

impl HeartbeatUpdateTool {
    pub const fn new(service: SharedHeartbeatService) -> Self {
        Self { service }
    }
}

#[async_trait]
impl AlephTool for HeartbeatUpdateTool {
    const NAME: &'static str = "heartbeat_update";
    const DESCRIPTION: &'static str = "Update an existing heartbeat monitoring task.";

    type Args = HeartbeatUpdateArgs;
    type Output = HeartbeatUpdateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Mirror the create-path floor: a sub-second interval makes the probe
        // (and any L2 agent run it triggers) fire on every tick.
        if let Some(interval_ms) = args.interval_ms {
            if interval_ms < 1000 {
                return Err(crate::error::AlephError::tool(format!(
                    "Interval too short: interval_ms={interval_ms} is below the 1000ms minimum."
                )));
            }
        }

        let failure_alert = if args.clear_failure_alert == Some(true) {
            Some(None)
        } else {
            args.failure_alert.map(Some)
        };

        let updates = HeartbeatTaskUpdates {
            name: args.name,
            agent_id: args.agent_id,
            interval_ms: args.interval_ms,
            enabled: args.enabled,
            active_hours: args.active_hours.map(Some),
            failure_alert,
            ..Default::default()
        };

        let clock = SystemClock;
        {
            let service = self.service.lock().await;
            service
                .update_task(&args.id, updates, &clock)
                .await
                .map_err(|e| {
                    crate::error::AlephError::tool(format!("Failed to update heartbeat task: {e}"))
                })?;
        }

        info!(task_id = %args.id, "Heartbeat task updated via tool");

        Ok(HeartbeatUpdateOutput {
            message: format!("Heartbeat task {} updated", args.id),
            task_id: args.id,
        })
    }
}

// =============================================================================
// Heartbeat Delete Tool
// =============================================================================

/// Arguments for `heartbeat_delete`
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct HeartbeatDeleteArgs {
    /// Task ID to delete, from `heartbeat_list` (required)
    pub id: String,
}

/// Output from `heartbeat_delete`
#[derive(Debug, Clone, Serialize)]
pub struct HeartbeatDeleteOutput {
    /// Human-readable status message
    pub message: String,
    /// Deleted task ID
    pub task_id: String,
}

/// Tool for deleting a heartbeat monitoring task.
#[derive(Clone)]
pub struct HeartbeatDeleteTool {
    service: SharedHeartbeatService,
}

impl HeartbeatDeleteTool {
    pub const fn new(service: SharedHeartbeatService) -> Self {
        Self { service }
    }
}

#[async_trait]
impl AlephTool for HeartbeatDeleteTool {
    const NAME: &'static str = "heartbeat_delete";
    const DESCRIPTION: &'static str = "Delete a heartbeat monitoring task permanently.";

    type Args = HeartbeatDeleteArgs;
    type Output = HeartbeatDeleteOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let service = self.service.lock().await;
        service.delete_task(&args.id).await.map_err(|e| {
            crate::error::AlephError::tool(format!("Failed to delete heartbeat task: {e}"))
        })?;

        info!(task_id = %args.id, "Heartbeat task deleted via tool");

        Ok(HeartbeatDeleteOutput {
            message: format!("Heartbeat task {} deleted", args.id),
            task_id: args.id,
        })
    }
}

// =============================================================================
// Heartbeat Toggle Tool
// =============================================================================

/// Arguments for `heartbeat_toggle`
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct HeartbeatToggleArgs {
    /// Task ID to toggle, from `heartbeat_list` (required)
    pub id: String,
}

/// Output from `heartbeat_toggle`
#[derive(Debug, Clone, Serialize)]
pub struct HeartbeatToggleOutput {
    /// Human-readable status message
    pub message: String,
    /// Task ID
    pub task_id: String,
    /// New enabled state
    pub enabled: bool,
}

/// Tool for toggling a heartbeat monitoring task's enabled state.
#[derive(Clone)]
pub struct HeartbeatToggleTool {
    service: SharedHeartbeatService,
}

impl HeartbeatToggleTool {
    pub const fn new(service: SharedHeartbeatService) -> Self {
        Self { service }
    }
}

#[async_trait]
impl AlephTool for HeartbeatToggleTool {
    const NAME: &'static str = "heartbeat_toggle";
    const DESCRIPTION: &'static str = "Enable or disable a heartbeat monitoring task.";

    type Args = HeartbeatToggleArgs;
    type Output = HeartbeatToggleOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let service = self.service.lock().await;
        let clock = SystemClock;
        let enabled = service.toggle_task(&args.id, &clock).await.map_err(|e| {
            crate::error::AlephError::tool(format!("Failed to toggle heartbeat task: {e}"))
        })?;

        let state_str = if enabled { "enabled" } else { "disabled" };
        Ok(HeartbeatToggleOutput {
            message: format!("Heartbeat task {} {}", args.id, state_str),
            task_id: args.id,
            enabled,
        })
    }
}

// =============================================================================
// Heartbeat Report Tool (L2 output tool)
// =============================================================================

/// Action for `heartbeat_report`
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HeartbeatReportAction {
    /// No action needed — findings are routine or below threshold
    Silent,
    /// Notify the user with the given message
    Notify,
}

/// Arguments for `heartbeat_report`
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct HeartbeatReportArgs {
    /// Action to take: "silent" (no notification) or "notify" (send message to user)
    pub action: HeartbeatReportAction,
    /// Message to send to the user (required when action = "notify")
    #[serde(default)]
    pub message: Option<String>,
}

/// Output from `heartbeat_report`
#[derive(Debug, Clone, Serialize)]
pub struct HeartbeatReportOutput {
    /// The action taken
    pub action: String,
    /// Message content (if notify)
    pub message: String,
    /// Whether the report was acknowledged
    pub acknowledged: bool,
}

/// Tool for L2 heartbeat execution — agent reports its findings via this tool.
///
/// This is the designated "output gate" for heartbeat L2 agents. The agent
/// calls this at the end of analysis to declare either "nothing to report"
/// (silent) or "notify the user" with a message.
#[derive(Clone, Default)]
pub struct HeartbeatReportTool;

#[async_trait]
impl AlephTool for HeartbeatReportTool {
    const NAME: &'static str = "heartbeat_report";
    const DESCRIPTION: &'static str =
        "Report the results of a heartbeat monitoring analysis. Call this at the end of your analysis \
         to declare the outcome: use action='silent' if nothing notable was found, or action='notify' \
         with a message to alert the user about findings that require attention.";

    type Args = HeartbeatReportArgs;
    type Output = HeartbeatReportOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let (action_str, message) = match args.action {
            HeartbeatReportAction::Silent => ("silent".to_string(), String::new()),
            HeartbeatReportAction::Notify => {
                let msg = args.message.unwrap_or_default();
                // The executor (classify_l2_outcome) treats a blank notify
                // message as Silent and sends nothing. Returning acknowledged
                // here would tell the agent it alerted the user when no
                // notification was ever delivered — reject so it can retry.
                if msg.trim().is_empty() {
                    return Err(crate::error::AlephError::tool(
                        "heartbeat_report(action=\"notify\") requires a non-empty message.",
                    ));
                }
                ("notify".to_string(), msg)
            }
        };

        Ok(HeartbeatReportOutput {
            action: action_str,
            message,
            acknowledged: true,
        })
    }
}
