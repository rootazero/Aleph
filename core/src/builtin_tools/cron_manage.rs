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

use crate::cron::{CronJob, CronJobView, ScheduleKind, SharedCronService};
use crate::error::Result;
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
    /// One-shot: fire at a specific timestamp (milliseconds since epoch)
    At {
        /// Timestamp in milliseconds since epoch (UTC)
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

fn default_true() -> bool {
    true
}

impl From<ScheduleInput> for ScheduleKind {
    fn from(input: ScheduleInput) -> Self {
        match input {
            ScheduleInput::At {
                at_ms,
                delete_after_run,
            } => ScheduleKind::At {
                at: at_ms,
                delete_after_run,
            },
            ScheduleInput::Every { every_ms } => ScheduleKind::Every {
                every_ms,
                anchor_ms: None,
            },
            ScheduleInput::Cron { expr, timezone } => ScheduleKind::Cron {
                expr,
                tz: timezone,
                stagger_ms: None,
            },
        }
    }
}

/// Arguments for the cron_manage tool
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

    // ── Get/Delete/Enable/Disable/Toggle fields ────────────────────
    /// Job ID (required for get/delete/enable/disable/toggle)
    #[serde(default)]
    pub job_id: Option<String>,
}

// =============================================================================
// Output
// =============================================================================

/// Output from cron_manage tool
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
    pub fn new(service: SharedCronService) -> Self {
        Self {
            service,
            source_channel_id: None,
        }
    }

    /// Create with a source channel context so scheduled jobs can push results back to the right channel
    pub fn with_channel(service: SharedCronService, channel_id: Option<String>) -> Self {
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
        "Manage scheduled tasks (cron jobs). Create, list, delete, enable, or disable \
         recurring or one-shot tasks. Use this when the user wants to schedule something \
         for a specific time or interval — e.g., 'remind me tomorrow at 9am', \
         'check the server every hour', 'send a report every Monday at 10am'.";

    type Args = CronManageArgs;
    type Output = CronManageOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"cron_manage(action="create", name="晨间汇报", prompt="生成今日待办清单", schedule={"type":"cron","expr":"0 0 9 * * *","timezone":"Asia/Shanghai"})"#.to_string(),
            r#"cron_manage(action="create", name="发送合同邮件", prompt="给王总发合同回复邮件", schedule={"type":"at","at_ms":1711944000000})"#.to_string(),
            r#"cron_manage(action="list")"#.to_string(),
            r#"cron_manage(action="delete", job_id="abc-123")"#.to_string(),
        ])
    }

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

                let agent_id = args.agent_id.unwrap_or_else(|| "main".to_string());
                let schedule_kind: ScheduleKind = schedule.into();

                let mut job = CronJob::new(&name, &agent_id, &prompt, schedule_kind);
                job.source_channel_id = self.source_channel_id.clone();
                let id = service.add_job(job).await.map_err(|e| {
                    crate::error::AlephError::tool(format!("Failed to create cron job: {}", e))
                })?;

                info!(job_id = %id, name = %name, "Cron job created via tool");

                Ok(CronManageOutput {
                    message: format!("定时任务 '{}' 已创建 (ID: {})", name, id),
                    job_id: Some(id),
                    jobs: None,
                    job: None,
                })
            }

            CronAction::List => {
                let jobs = service.list_jobs().await.map_err(|e| {
                    crate::error::AlephError::tool(format!("Failed to list cron jobs: {}", e))
                })?;

                let count = jobs.len();
                Ok(CronManageOutput {
                    message: format!("共 {} 个定时任务", count),
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
                    crate::error::AlephError::tool(format!("Failed to get cron job: {}", e))
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
                    crate::error::AlephError::tool(format!("Failed to delete cron job: {}", e))
                })?;

                info!(job_id = %id, "Cron job deleted via tool");

                Ok(CronManageOutput {
                    message: format!("定时任务 {} 已删除", id),
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
                    crate::error::AlephError::tool(format!("Failed to enable cron job: {}", e))
                })?;

                Ok(CronManageOutput {
                    message: format!("定时任务 {} 已启用", id),
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
                    crate::error::AlephError::tool(format!("Failed to disable cron job: {}", e))
                })?;

                Ok(CronManageOutput {
                    message: format!("定时任务 {} 已禁用", id),
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
                    crate::error::AlephError::tool(format!("Failed to toggle cron job: {}", e))
                })?;

                let state_str = if new_state { "启用" } else { "禁用" };
                Ok(CronManageOutput {
                    message: format!("定时任务 {} 已{}", id, state_str),
                    job_id: Some(id),
                    jobs: None,
                    job: None,
                })
            }
        }
    }
}
