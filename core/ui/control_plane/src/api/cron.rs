// API layer for Cron Job RPC methods
// Provides type-safe interfaces for managing scheduled cron jobs via Gateway

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::context::DashboardState;

// ============================================================================
// DTOs
// ============================================================================

/// Cron job info returned by the backend
#[derive(Debug, Clone, Deserialize)]
pub struct CronJobInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub schedule: String,
    #[serde(default)]
    pub schedule_kind: String,
    #[serde(default)]
    pub agent_id: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub next_run_at: Option<i64>,
    #[serde(default)]
    pub last_run_at: Option<i64>,
}

/// Request payload for creating a cron job
#[derive(Debug, Clone, Serialize)]
pub struct CreateCronJob {
    pub name: String,
    pub schedule: String,
    pub schedule_kind: String,
    pub agent_id: String,
    pub prompt: String,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// Request payload for updating a cron job (all fields optional except job_id)
#[derive(Debug, Clone, Serialize)]
pub struct UpdateCronJob {
    pub job_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

/// Job execution run info returned by the backend
#[derive(Debug, Clone, Deserialize)]
pub struct JobRunInfo {
    pub id: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub started_at: i64,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub error: Option<String>,
}

// ============================================================================
// CronApi
// ============================================================================

pub struct CronApi;

impl CronApi {
    /// List all cron jobs
    pub async fn list(state: &DashboardState) -> Result<Vec<CronJobInfo>, String> {
        let result = state.rpc_call("cron.list", Value::Null).await?;

        result
            .get("jobs")
            .ok_or_else(|| "Invalid response: missing jobs".to_string())
            .and_then(|jobs| {
                serde_json::from_value(jobs.clone())
                    .map_err(|e| format!("Failed to parse cron jobs: {}", e))
            })
    }

    /// Create a new cron job
    pub async fn create(
        state: &DashboardState,
        job: CreateCronJob,
    ) -> Result<CronJobInfo, String> {
        let params =
            serde_json::to_value(&job).map_err(|e| format!("Failed to serialize job: {}", e))?;

        let result = state.rpc_call("cron.create", params).await?;

        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse created cron job: {}", e))
    }

    /// Update an existing cron job
    pub async fn update(
        state: &DashboardState,
        patch: UpdateCronJob,
    ) -> Result<CronJobInfo, String> {
        let params = serde_json::to_value(&patch)
            .map_err(|e| format!("Failed to serialize patch: {}", e))?;

        let result = state.rpc_call("cron.update", params).await?;

        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse updated cron job: {}", e))
    }

    /// Delete a cron job by ID
    pub async fn delete(state: &DashboardState, job_id: &str) -> Result<(), String> {
        let params = serde_json::json!({ "job_id": job_id });
        state.rpc_call("cron.delete", params).await?;
        Ok(())
    }

    /// Get execution history for a cron job
    pub async fn runs(
        state: &DashboardState,
        job_id: &str,
        limit: i32,
    ) -> Result<Vec<JobRunInfo>, String> {
        let params = serde_json::json!({
            "job_id": job_id,
            "limit": limit,
        });

        let result = state.rpc_call("cron.runs", params).await?;

        result
            .get("runs")
            .ok_or_else(|| "Invalid response: missing runs".to_string())
            .and_then(|runs| {
                serde_json::from_value(runs.clone())
                    .map_err(|e| format!("Failed to parse job runs: {}", e))
            })
    }

    /// Toggle a cron job enabled/disabled
    pub async fn toggle(
        state: &DashboardState,
        job_id: &str,
        enabled: bool,
    ) -> Result<CronJobInfo, String> {
        let params = serde_json::json!({
            "job_id": job_id,
            "enabled": enabled,
        });

        let result = state.rpc_call("cron.toggle", params).await?;

        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse toggled cron job: {}", e))
    }

    /// Trigger an immediate run of a cron job
    pub async fn run_now(state: &DashboardState, job_id: &str) -> Result<Value, String> {
        let params = serde_json::json!({ "job_id": job_id });
        state.rpc_call("cron.run", params).await
    }
}
