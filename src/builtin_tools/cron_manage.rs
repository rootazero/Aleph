//! Cron management tool — create, list, delete, and control scheduled tasks.
//!
//! Exposes the cron service CRUD API as an LLM-callable tool, letting users
//! manage scheduled tasks through natural language. The LLM handles all
//! intent parsing (R8 LLM Sovereignty): converting "明天早上9点" into a
//! timestamp or cron expression is the model's job, not ours.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::Result;
use crate::tasks::cron::{
    CronJob, CronJobView, FailureAlertConfig, ScheduleKind, SharedCronService,
};
use crate::tools::AlephTool;

// =============================================================================
// Args
// =============================================================================

/// Action to perform on the cron system
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CronAction {
    /// Create a new scheduled task
    Create,
    /// List all scheduled tasks
    List,
    /// Get details of a specific task
    Get,
    /// Delete a scheduled task
    Delete,
    /// Enable a disabled task
    Enable,
    /// Disable an active task
    Disable,
    /// Toggle a task's enabled state
    Toggle,
    /// Trigger a task to run immediately (manual execution)
    Run,
    /// Modify an existing task's fields (name, prompt, schedule, agent, enabled)
    Update,
}

/// Schedule definition for creating a cron job.
///
/// The LLM should choose the appropriate variant:
/// - `at`: For one-shot tasks at a specific time (timestamp in ms since epoch)
/// - `every`: For recurring interval-based tasks (interval in ms)
/// - `cron`: For recurring tasks with cron expressions (6-field: sec min hour dom mon dow)
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScheduleInput {
    /// One-shot: fire at a specific timestamp (milliseconds since epoch).
    /// IMPORTANT: Use the current system time as reference. For example,
    /// 2026-03-30 10:00:00 Asia/Shanghai = 2026-03-30 02:00:00 UTC = 1774850400000 ms.
    /// Tip: 1 hour = 3600000 ms, 1 day = 86400000 ms. You can compute
    /// `at_ms` = `current_time_ms` + `offset_ms` for relative scheduling.
    At {
        /// Timestamp in milliseconds since epoch (UTC).
        /// Must be in the future — past timestamps will be rejected.
        at_ms: i64,
        /// Whether to auto-delete after execution (default: true)
        #[serde(default = "default_true")]
        delete_after_run: bool,
    },
    /// Interval: fire every N milliseconds
    Every {
        /// Interval in milliseconds (e.g. 3600000 for 1 hour)
        every_ms: i64,
    },
    /// Cron expression: standard 6-field (sec min hour dom mon dow)
    Cron {
        /// Cron expression (e.g. "0 0 9 * * *" for daily at 9:00)
        expr: String,
        /// Optional timezone (e.g. "Asia/Shanghai"), defaults to local
        #[serde(default)]
        timezone: Option<String>,
    },
}

const fn default_true() -> bool {
    true
}

/// Validate a schedule input before it is converted to a `ScheduleKind`.
///
/// Rejects one-shot tasks scheduled in the past and sub-second intervals
/// (which would fire on every timer tick and serve no real use case).
fn validate_schedule(schedule: &ScheduleInput) -> Result<()> {
    match schedule {
        ScheduleInput::At { at_ms, .. } => {
            let now_ms = chrono::Utc::now().timestamp_millis();
            if *at_ms <= now_ms {
                let at_human = chrono::DateTime::from_timestamp_millis(*at_ms).map_or_else(
                    || format!("{at_ms}ms"),
                    |dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
                );
                return Err(crate::error::AlephError::tool(format!(
                    "Cannot schedule a one-shot task in the past. at_ms={at_ms} resolves to \
                     {at_human}, but current time is {now_ms}ms. Provide a future timestamp."
                )));
            }
        }
        ScheduleInput::Every { every_ms } => {
            if *every_ms < 1000 {
                return Err(crate::error::AlephError::tool(format!(
                    "Interval too short: every_ms={every_ms} is below the 1000ms minimum."
                )));
            }
        }
        ScheduleInput::Cron { expr, timezone } => {
            // Validate the expression (and timezone) up front. Without this, a
            // malformed expr is accepted, persisted, and reported as created —
            // but compute_next_run_for_job collapses the parse error to None,
            // so the job has no next_run and silently never fires.
            crate::tasks::shared::schedule::compute_next_cron(
                expr,
                timezone.as_deref(),
                chrono::Utc::now(),
            )
            .map_err(|e| crate::error::AlephError::tool(format!("Invalid cron schedule: {e}")))?;
        }
    }
    Ok(())
}

impl From<ScheduleInput> for ScheduleKind {
    fn from(input: ScheduleInput) -> Self {
        match input {
            ScheduleInput::At {
                at_ms,
                delete_after_run,
            } => Self::At {
                at: at_ms,
                delete_after_run,
            },
            ScheduleInput::Every { every_ms } => Self::Every {
                every_ms,
                anchor_ms: None,
            },
            ScheduleInput::Cron { expr, timezone } => Self::Cron {
                expr,
                tz: timezone,
                stagger_ms: None,
            },
        }
    }
}

/// Arguments for the `cron_manage` tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CronManageArgs {
    /// Action to perform
    pub action: CronAction,

    // ── Create fields ──────────────────────────────────────────────
    /// Task name (required for create)
    #[serde(default)]
    pub name: Option<String>,

    /// Prompt/instruction to send to the agent when the task fires (required for create)
    #[serde(default)]
    pub prompt: Option<String>,

    /// Schedule definition (required for create)
    #[serde(default)]
    pub schedule: Option<ScheduleInput>,

    /// Agent ID to invoke (default: "main")
    #[serde(default)]
    pub agent_id: Option<String>,

    // ── Get/Delete/Enable/Disable/Toggle/Update fields ─────────────
    /// Job ID (required for get/delete/enable/disable/toggle/update)
    #[serde(default)]
    pub job_id: Option<String>,

    /// New enabled state — only used by the `update` action.
    #[serde(default)]
    pub enabled: Option<bool>,

    /// Failure alerting for this task. Set on `create`, replaced on `update`.
    ///
    /// Without it a job that keeps failing is only visible to someone who
    /// opens the task history. `target` picks where the alert goes — a
    /// `Webhook` URL, a `Gateway` channel + chat, or `Memory`.
    #[serde(default)]
    pub failure_alert: Option<FailureAlertConfig>,

    /// `update` only: pass `true` to remove an existing `failure_alert`.
    /// (`failure_alert: null` is indistinguishable from "not mentioned" in a
    /// JSON tool call, so clearing needs its own flag.)
    #[serde(default)]
    pub clear_failure_alert: Option<bool>,

    // ── Internal (injected by dispatcher, not LLM-visible) ────────
    /// Source channel ID — injected by the tool dispatcher from session context.
    /// Used to set `source_channel_id` on created jobs so results are delivered
    /// back to the originating channel.
    #[serde(default, rename = "__channel")]
    #[schemars(skip)]
    pub __channel: Option<String>,

    /// Source conversation ID — injected by the tool dispatcher from session context.
    /// For Telegram this is the `chat_id`; used so cron results can be sent to the correct chat.
    #[serde(default, rename = "__conversation_id")]
    #[schemars(skip)]
    pub __conversation_id: Option<String>,

    /// Current server time in ms since epoch — injected by dispatcher so the LLM
    /// has a reliable reference for computing At timestamps.
    #[serde(default, rename = "__current_time_ms")]
    #[schemars(skip)]
    pub __current_time_ms: Option<i64>,
}

// =============================================================================
// Output
// =============================================================================

/// Output from `cron_manage` tool
#[derive(Debug, Clone, Serialize)]
pub struct CronManageOutput {
    /// Human-readable status message
    pub message: String,
    /// Created/affected job ID (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    /// Job list (for list action)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jobs: Option<Vec<CronJobView>>,
    /// Single job detail (for get action)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<CronJobView>,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool for managing cron/scheduled tasks via natural language.
#[derive(Clone)]
pub struct CronManageTool {
    service: SharedCronService,
    /// Channel ID where this tool instance is operating (injected at construction)
    source_channel_id: Option<String>,
}

impl CronManageTool {
    pub const fn new(service: SharedCronService) -> Self {
        Self {
            service,
            source_channel_id: None,
        }
    }

    /// Create with a source channel context so scheduled jobs can push results back to the right channel
    pub const fn with_channel(service: SharedCronService, channel_id: Option<String>) -> Self {
        Self {
            service,
            source_channel_id: channel_id,
        }
    }
}

#[async_trait]
impl AlephTool for CronManageTool {
    const NAME: &'static str = "cron_manage";
    const DESCRIPTION: &'static str =
        "Manage scheduled tasks (cron jobs). Create, list, update, delete, enable, disable, \
         or manually trigger recurring or one-shot tasks. Use this when the user wants \
         to schedule something for a specific time or interval — e.g., 'remind me tomorrow at 9am', \
         'check the server every hour', 'send a report every Monday at 10am'. \
         Also use action='run' to manually trigger a job immediately (e.g., 'run the daily report now'). \
         For one-shot 'at' scheduling, compute at_ms from current system time (check __current_time_ms). \
         Tip: 1h = 3600000ms, 1d = 86400000ms. Asia/Shanghai = UTC+8.";

    type Args = CronManageArgs;
    type Output = CronManageOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let service = self.service.lock().await;

        match args.action {
            CronAction::Create => {
                let name = args.name.ok_or_else(|| {
                    crate::error::AlephError::tool("cron_manage create: 'name' is required")
                })?;
                let prompt = args.prompt.ok_or_else(|| {
                    crate::error::AlephError::tool("cron_manage create: 'prompt' is required")
                })?;
                let schedule = args.schedule.ok_or_else(|| {
                    crate::error::AlephError::tool("cron_manage create: 'schedule' is required")
                })?;

                validate_schedule(&schedule)?;

                let agent_id = args.agent_id.unwrap_or_else(|| "main".to_string());
                let schedule_kind: ScheduleKind = schedule.into();

                let mut job = CronJob::new(&name, &agent_id, &prompt, schedule_kind);
                job.failure_alert = args.failure_alert;
                // Prefer runtime channel from dispatcher (injected via __channel),
                // fall back to static channel set at construction time.
                job.source_channel_id = args.__channel.or_else(|| self.source_channel_id.clone());
                // Store conversation_id for delivery routing (e.g. Telegram chat_id)
                job.source_conversation_id = args.__conversation_id;
                // P1 data isolation: cron.* is admin-gated, so the owner is
                // the operating admin — stamp from the ambient attribution of
                // THIS creating (admin) run.
                if let Some(attr) = crate::scope::current_scope() {
                    job.owner_user_id = Some(attr.owner_user_id);
                    job.scope_id = Some(attr.scope.render());
                }
                let id = service.add_job(job).await.map_err(|e| {
                    crate::error::AlephError::tool(format!("Failed to create cron job: {e}"))
                })?;

                info!(job_id = %id, name = %name, "Cron job created via tool");

                Ok(CronManageOutput {
                    message: format!("定时任务 '{name}' 已创建 (ID: {id})"),
                    job_id: Some(id),
                    jobs: None,
                    job: None,
                })
            }

            CronAction::List => {
                let jobs = service.list_jobs().await.map_err(|e| {
                    crate::error::AlephError::tool(format!("Failed to list cron jobs: {e}"))
                })?;

                let count = jobs.len();
                Ok(CronManageOutput {
                    message: format!("共 {count} 个定时任务"),
                    job_id: None,
                    jobs: Some(jobs),
                    job: None,
                })
            }

            CronAction::Get => {
                let id = args.job_id.ok_or_else(|| {
                    crate::error::AlephError::tool("cron_manage get: 'job_id' is required")
                })?;
                let job = service.get_job(&id).await.map_err(|e| {
                    crate::error::AlephError::tool(format!("Failed to get cron job: {e}"))
                })?;

                Ok(CronManageOutput {
                    message: format!("任务 '{}' ({})", job.name, id),
                    job_id: Some(id),
                    jobs: None,
                    job: Some(job),
                })
            }

            CronAction::Delete => {
                let id = args.job_id.ok_or_else(|| {
                    crate::error::AlephError::tool("cron_manage delete: 'job_id' is required")
                })?;
                service.delete_job(&id).await.map_err(|e| {
                    crate::error::AlephError::tool(format!("Failed to delete cron job: {e}"))
                })?;

                info!(job_id = %id, "Cron job deleted via tool");

                Ok(CronManageOutput {
                    message: format!("定时任务 {id} 已删除"),
                    job_id: Some(id),
                    jobs: None,
                    job: None,
                })
            }

            CronAction::Enable => {
                let id = args.job_id.ok_or_else(|| {
                    crate::error::AlephError::tool("cron_manage enable: 'job_id' is required")
                })?;
                service.enable_job(&id).await.map_err(|e| {
                    crate::error::AlephError::tool(format!("Failed to enable cron job: {e}"))
                })?;

                Ok(CronManageOutput {
                    message: format!("定时任务 {id} 已启用"),
                    job_id: Some(id),
                    jobs: None,
                    job: None,
                })
            }

            CronAction::Disable => {
                let id = args.job_id.ok_or_else(|| {
                    crate::error::AlephError::tool("cron_manage disable: 'job_id' is required")
                })?;
                service.disable_job(&id).await.map_err(|e| {
                    crate::error::AlephError::tool(format!("Failed to disable cron job: {e}"))
                })?;

                Ok(CronManageOutput {
                    message: format!("定时任务 {id} 已禁用"),
                    job_id: Some(id),
                    jobs: None,
                    job: None,
                })
            }

            CronAction::Toggle => {
                let id = args.job_id.ok_or_else(|| {
                    crate::error::AlephError::tool("cron_manage toggle: 'job_id' is required")
                })?;
                let new_state = service.toggle_job(&id).await.map_err(|e| {
                    crate::error::AlephError::tool(format!("Failed to toggle cron job: {e}"))
                })?;

                let state_str = if new_state { "启用" } else { "禁用" };
                Ok(CronManageOutput {
                    message: format!("定时任务 {id} 已{state_str}"),
                    job_id: Some(id),
                    jobs: None,
                    job: None,
                })
            }

            CronAction::Run => {
                let id = args.job_id.ok_or_else(|| {
                    crate::error::AlephError::tool("cron_manage run: 'job_id' is required")
                })?;
                service.run_job(&id).await.map_err(|e| {
                    crate::error::AlephError::tool(format!("Failed to trigger cron job: {e}"))
                })?;

                info!(job_id = %id, "Cron job triggered manually via tool");

                Ok(CronManageOutput {
                    message: format!("定时任务 {id} 已手动触发，将在下一次检查周期内执行"),
                    job_id: Some(id),
                    jobs: None,
                    job: None,
                })
            }

            CronAction::Update => {
                let id = args.job_id.ok_or_else(|| {
                    crate::error::AlephError::tool("cron_manage update: 'job_id' is required")
                })?;

                if let Some(ref schedule) = args.schedule {
                    validate_schedule(schedule)?;
                }

                // Tri-state: an explicit `clear_failure_alert` wins, then a new
                // config, else leave the existing one alone. A tool call
                // cannot express `null`-means-clear, hence the separate flag.
                let failure_alert = if args.clear_failure_alert == Some(true) {
                    Some(None)
                } else {
                    args.failure_alert.map(Some)
                };

                let updates = crate::tasks::cron::service::ops::CronJobUpdates {
                    name: args.name,
                    agent_id: args.agent_id,
                    prompt: args.prompt,
                    enabled: args.enabled,
                    schedule_kind: args.schedule.map(ScheduleKind::from),
                    tags: None,
                    session_target: None,
                    timeout_ms: None,
                    failure_alert,
                };

                service.update_job(&id, updates).await.map_err(|e| {
                    crate::error::AlephError::tool(format!("Failed to update cron job: {e}"))
                })?;

                info!(job_id = %id, "Cron job updated via tool");

                Ok(CronManageOutput {
                    message: format!("定时任务 {id} 已更新"),
                    job_id: Some(id),
                    jobs: None,
                    job: None,
                })
            }
        }
    }
}
