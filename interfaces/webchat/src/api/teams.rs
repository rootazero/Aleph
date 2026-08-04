//! Panel API for team management (teams.* RPC calls)

use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// -- Types --

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct MemberPreview {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub emoji: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TeamSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub leader_id: String,
    pub status: String,
    /// Present in list responses (`TeamSummary`), absent in get responses (Team).
    #[serde(default)]
    pub member_count: usize,
    /// Present in list responses (`TeamSummary`), absent in get responses (Team).
    #[serde(default)]
    pub task_count: usize,
    pub created_at: i64,
    pub disbanded_at: Option<i64>,
    #[serde(default)]
    pub members_preview: Vec<MemberPreview>,
    #[serde(default)]
    pub last_message: Option<String>,
    /// Unix-epoch-seconds timestamp of the most recent group message. Enriched
    /// by `agents.teams`; `None` when the team has no transcript yet. Used to
    /// sort the sidebar group-chat list newest-first.
    #[serde(default)]
    pub last_message_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TeamMember {
    pub team_id: String,
    pub agent_id: String,
    pub role: String,
    pub joined_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TeamDetail {
    pub team: TeamSummary,
    pub members: Vec<TeamMember>,
    /// Server returns serialized `CoordTask` records — reuse the kanban DTO
    /// rather than a second, drift-prone shape.
    pub tasks: Vec<CoordTaskDto>,
}

// -- API --

pub struct TeamsApi;

impl TeamsApi {
    pub async fn list(state: &DashboardState) -> Result<Vec<TeamSummary>, String> {
        let result = state.rpc_call("teams.list", Value::Null).await?;
        // Server returns {"teams": [...]}, extract the inner array
        let teams_value = result.get("teams").cloned().unwrap_or(Value::Array(vec![]));
        serde_json::from_value(teams_value).map_err(|e| e.to_string())
    }

    pub async fn get(state: &DashboardState, team_id: &str) -> Result<TeamDetail, String> {
        let result = state
            .rpc_call("teams.get", json!({"team_id": team_id}))
            .await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn disband(state: &DashboardState, team_id: &str) -> Result<(), String> {
        state
            .rpc_call("teams.disband", json!({"team_id": team_id}))
            .await?;
        Ok(())
    }

    pub async fn rename(state: &DashboardState, team_id: &str, name: &str) -> Result<(), String> {
        state
            .rpc_call("teams.rename", json!({ "team_id": team_id, "name": name }))
            .await?;
        Ok(())
    }

    pub async fn delete(state: &DashboardState, team_id: &str) -> Result<(), String> {
        state
            .rpc_call("teams.delete", json!({"team_id": team_id}))
            .await?;
        Ok(())
    }

    pub async fn agent_teams(
        state: &DashboardState,
        agent_id: &str,
    ) -> Result<Vec<TeamSummary>, String> {
        let result = state
            .rpc_call("agents.teams", json!({"agent_id": agent_id}))
            .await?;
        // Server returns {"teams": [...]}, extract the inner array
        let teams_value = result.get("teams").cloned().unwrap_or(Value::Array(vec![]));
        serde_json::from_value(teams_value).map_err(|e| e.to_string())
    }

    /// Discover available team templates (builtin + user-dir overrides).
    pub async fn list_templates(state: &DashboardState) -> Result<Vec<TemplateMeta>, String> {
        let result = state.rpc_call("teams.list_templates", Value::Null).await?;
        let value = result
            .get("templates")
            .cloned()
            .unwrap_or(Value::Array(vec![]));
        serde_json::from_value(value).map_err(|e| e.to_string())
    }

    /// Ad-hoc team assembly: create a persistent team with an explicit leader_id
    /// + members. Returns the new team_id. Wraps the `teams.create` RPC.
    pub async fn create(
        state: &DashboardState,
        name: &str,
        description: &str,
        leader_id: &str,
        members: &[(String, String)], // (agent_id, role)
        auto_name: bool,
    ) -> Result<String, String> {
        let members_json: Vec<Value> = members
            .iter()
            .map(|(id, role)| json!({ "agent_id": id, "role": role }))
            .collect();
        let result = state
            .rpc_call(
                "teams.create",
                json!({
                    "name": name,
                    "description": description,
                    "leader_id": leader_id,
                    "members": members_json,
                    "auto_name": auto_name,
                }),
            )
            .await?;
        result
            .get("team_id")
            .and_then(|v| v.as_str().map(String::from))
            .ok_or_else(|| "teams.create did not return team_id".to_string())
    }

    /// Materialize a team from a template via the `team_from_template` builtin
    /// tool. Returns the freshly-created team id so the caller can refresh and
    /// open the team detail.
    pub async fn create_from_template(
        state: &DashboardState,
        template: &str,
        team_name: &str,
        goal: Option<&str>,
    ) -> Result<String, String> {
        let mut args = json!({
            "template": template,
            "team_name": team_name,
        });
        if let Some(g) = goal.filter(|s| !s.is_empty()) {
            args["goal"] = Value::String(g.to_string());
        }
        let result = state
            .rpc_call(
                "tools.invoke",
                json!({
                    "tool_name": "team_from_template",
                    "arguments": args,
                }),
            )
            .await?;
        // `tools.invoke` returns the tool's raw JSON output; for
        // `team_from_template` that's TeamFromTemplateOutput { team_id, ... }.
        result
            .get("team_id")
            .and_then(|v| v.as_str().map(String::from))
            .ok_or_else(|| "team_from_template did not return team_id".to_string())
    }
}

/// Light view of a team template returned by `teams.list_templates`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TemplateMeta {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub default_goal: Option<String>,
    pub leader_id: String,
    pub leader_role: String,
    pub member_count: usize,
    pub task_count: usize,
}

// =============================================================================
// CoordTask DTO + kanban API methods
// =============================================================================

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CoordTaskDto {
    pub id: String,
    #[serde(default)]
    pub team_id: Option<String>,
    pub subject: String,
    #[serde(default)]
    pub description: String,
    /// "pending" | "blocked" | "`in_progress`" | "completed" | "failed" | "cancelled"
    pub status: String,
    #[serde(default)]
    pub owner: Option<String>,
    /// "low" | "normal" | "high" | "critical"
    #[serde(default = "default_priority")]
    pub priority: String,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub created_at: u64,
    #[serde(default)]
    pub started_at: Option<u64>,
    #[serde(default)]
    pub completed_at: Option<u64>,
}

fn default_priority() -> String {
    "normal".to_string()
}

#[derive(Debug, Clone, Default)]
pub struct TaskFilter {
    pub status: Option<String>,
    pub owner: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TaskPatch {
    pub status: Option<String>,
    pub owner: Option<String>,
    pub result: Option<String>,
}

/// Fields for creating a new kanban task. `subject` is required; the rest are
/// optional and omitted from the request when blank.
#[derive(Debug, Clone, Default)]
pub struct NewTaskInput {
    pub subject: String,
    pub description: String,
    pub owner: Option<String>,
    /// "low" | "normal" | "high" | "critical"
    pub priority: Option<String>,
}

impl TeamsApi {
    pub async fn list_tasks(
        state: &DashboardState,
        team_id: &str,
        filter: TaskFilter,
    ) -> Result<Vec<CoordTaskDto>, String> {
        let mut params = serde_json::Map::new();
        params.insert("team_id".to_string(), Value::String(team_id.to_string()));
        if let Some(s) = filter.status {
            params.insert("status".to_string(), Value::String(s));
        }
        if let Some(o) = filter.owner {
            params.insert("owner".to_string(), Value::String(o));
        }
        let result = state
            .rpc_call("teams.list_tasks", Value::Object(params))
            .await?;
        let tasks_value = result.get("tasks").cloned().unwrap_or(Value::Array(vec![]));
        serde_json::from_value(tasks_value).map_err(|e| e.to_string())
    }

    pub async fn update_task(
        state: &DashboardState,
        task_id: &str,
        patch: TaskPatch,
    ) -> Result<CoordTaskDto, String> {
        let mut params = serde_json::Map::new();
        params.insert("task_id".to_string(), Value::String(task_id.to_string()));
        if let Some(s) = patch.status {
            params.insert("status".to_string(), Value::String(s));
        }
        if let Some(o) = patch.owner {
            params.insert("owner".to_string(), Value::String(o));
        }
        if let Some(r) = patch.result {
            params.insert("result".to_string(), Value::String(r));
        }
        let result = state
            .rpc_call("teams.update_task", Value::Object(params))
            .await?;
        let task_value = result.get("task").cloned().unwrap_or(Value::Null);
        serde_json::from_value(task_value).map_err(|e| e.to_string())
    }

    pub async fn create_task(
        state: &DashboardState,
        team_id: &str,
        input: NewTaskInput,
    ) -> Result<CoordTaskDto, String> {
        let mut params = serde_json::Map::new();
        params.insert("team_id".to_string(), Value::String(team_id.to_string()));
        params.insert("subject".to_string(), Value::String(input.subject));
        if !input.description.is_empty() {
            params.insert("description".to_string(), Value::String(input.description));
        }
        if let Some(o) = input.owner.filter(|s| !s.is_empty()) {
            params.insert("owner".to_string(), Value::String(o));
        }
        if let Some(p) = input.priority.filter(|s| !s.is_empty()) {
            params.insert("priority".to_string(), Value::String(p));
        }
        let result = state
            .rpc_call("teams.create_task", Value::Object(params))
            .await?;
        let task_value = result.get("task").cloned().unwrap_or(Value::Null);
        serde_json::from_value(task_value).map_err(|e| e.to_string())
    }

    // --- Per-task drawer subviews (runs / comments / events) -----------------

    pub async fn list_task_runs(
        state: &DashboardState,
        task_id: &str,
    ) -> Result<Vec<TaskRunDto>, String> {
        let result = state
            .rpc_call("teams.list_task_runs", json!({ "task_id": task_id }))
            .await?;
        let runs_value = result.get("runs").cloned().unwrap_or(Value::Array(vec![]));
        serde_json::from_value(runs_value).map_err(|e| e.to_string())
    }

    pub async fn list_task_comments(
        state: &DashboardState,
        task_id: &str,
    ) -> Result<Vec<TaskCommentDto>, String> {
        let result = state
            .rpc_call("teams.list_task_comments", json!({ "task_id": task_id }))
            .await?;
        let v = result
            .get("comments")
            .cloned()
            .unwrap_or(Value::Array(vec![]));
        serde_json::from_value(v).map_err(|e| e.to_string())
    }

    pub async fn add_task_comment(
        state: &DashboardState,
        task_id: &str,
        author: &str,
        body: &str,
    ) -> Result<TaskCommentDto, String> {
        let result = state
            .rpc_call(
                "teams.add_task_comment",
                json!({ "task_id": task_id, "author": author, "body": body }),
            )
            .await?;
        let v = result.get("comment").cloned().unwrap_or(Value::Null);
        serde_json::from_value(v).map_err(|e| e.to_string())
    }

    pub async fn list_task_events(
        state: &DashboardState,
        task_id: &str,
    ) -> Result<Vec<TaskEventDto>, String> {
        let result = state
            .rpc_call("teams.list_task_events", json!({ "task_id": task_id }))
            .await?;
        let v = result
            .get("events")
            .cloned()
            .unwrap_or(Value::Array(vec![]));
        serde_json::from_value(v).map_err(|e| e.to_string())
    }

    // --- R3: task-level control + unified trace + exit journal -------------

    /// teams.task.pause — suspend a task; dispatcher will not claim it.
    pub async fn task_pause(state: &DashboardState, task_id: &str) -> Result<(), String> {
        state
            .rpc_call("teams.task.pause", json!({ "task_id": task_id }))
            .await
            .map(|_| ())
    }

    /// teams.task.resume — return a paused task to pending.
    pub async fn task_resume(state: &DashboardState, task_id: &str) -> Result<(), String> {
        state
            .rpc_call("teams.task.resume", json!({ "task_id": task_id }))
            .await
            .map(|_| ())
    }

    /// teams.task.retry — hard-retry from any non-pending state.
    pub async fn task_retry(state: &DashboardState, task_id: &str) -> Result<(), String> {
        state
            .rpc_call("teams.task.retry", json!({ "task_id": task_id }))
            .await
            .map(|_| ())
    }

    /// teams.task.skip — mark a task as not required.
    pub async fn task_skip(state: &DashboardState, task_id: &str) -> Result<(), String> {
        state
            .rpc_call("teams.task.skip", json!({ "task_id": task_id }))
            .await
            .map(|_| ())
    }

    /// teams.workflow.approve_step — approve a waiting-review task; the
    /// backend stamps the latest run Approved and transitions the task to
    /// Completed (downstream dependents unblock). `reviewer_kind` defaults
    /// to "user" on the server, so the panel sends only the task id.
    pub async fn task_approve(state: &DashboardState, task_id: &str) -> Result<(), String> {
        state
            .rpc_call("teams.workflow.approve_step", json!({ "task_id": task_id }))
            .await
            .map(|_| ())
    }

    /// teams.workflow.reject_step — reject a waiting-review task; the backend
    /// stamps the latest run Rejected and transitions the task to Failed
    /// (dependents stay blocked). An optional `reason` is recorded as the
    /// task result and a review comment.
    pub async fn task_reject(
        state: &DashboardState,
        task_id: &str,
        reason: Option<&str>,
    ) -> Result<(), String> {
        let mut params = serde_json::Map::new();
        params.insert("task_id".to_string(), Value::String(task_id.to_string()));
        if let Some(r) = reason.filter(|s| !s.trim().is_empty()) {
            params.insert("comment".to_string(), Value::String(r.to_string()));
        }
        state
            .rpc_call("teams.workflow.reject_step", Value::Object(params))
            .await
            .map(|_| ())
    }

    /// teams.task.trace — unified audit timeline (task + runs + comments
    ///   + events + artifacts + journal) in one round-trip. Returns the
    ///     raw JSON envelope; callers cherry-pick the fields they render.
    pub async fn task_trace(state: &DashboardState, task_id: &str) -> Result<Value, String> {
        state
            .rpc_call("teams.task.trace", json!({ "task_id": task_id }))
            .await
    }

    /// teams.task.journal.list — all journals for a team, newest first.
    pub async fn task_journal_list(
        state: &DashboardState,
        team_id: &str,
    ) -> Result<Vec<TaskExitJournalDto>, String> {
        let result = state
            .rpc_call("teams.task.journal.list", json!({ "team_id": team_id }))
            .await?;
        let v = result
            .get("journals")
            .cloned()
            .unwrap_or(Value::Array(vec![]));
        serde_json::from_value(v).map_err(|e| e.to_string())
    }

    /// teams.usage — per-team provider token aggregation (F5).
    /// `since`/`until` are optional epoch seconds; both omitted ⇒ all-time.
    pub async fn usage(
        state: &DashboardState,
        team_id: &str,
        since: Option<i64>,
        until: Option<i64>,
    ) -> Result<TeamUsageDto, String> {
        let mut params = serde_json::Map::new();
        params.insert("team_id".into(), json!(team_id));
        if let Some(s) = since {
            params.insert("since".into(), json!(s));
        }
        if let Some(u) = until {
            params.insert("until".into(), json!(u));
        }
        let result = state.rpc_call("teams.usage", Value::Object(params)).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }
}

/// Mirror of server-side `teams.usage` response.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct TeamUsageDto {
    pub team_id: String,
    #[serde(default)]
    pub since: Option<i64>,
    #[serde(default)]
    pub until: Option<i64>,
    #[serde(default)]
    pub member_count: u64,
    #[serde(default)]
    pub total: UsageTotals,
    #[serde(default)]
    pub per_agent: Vec<AgentUsageRow>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct UsageTotals {
    #[serde(default)]
    pub call_count: u64,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_creation_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
    /// Cache read tokens as a fraction of total input (0.0–1.0); computed
    /// server-side. `None` when the backend omitted it.
    #[serde(default)]
    pub cache_hit_ratio: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct AgentUsageRow {
    pub agent_id: String,
    #[serde(default)]
    pub call_count: u64,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_creation_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
}

// --- DTOs for the 3 new drawer sections ---------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TaskRunDto {
    pub id: String,
    pub task_id: String,
    pub agent_id: String,
    pub started_at: u64,
    #[serde(default)]
    pub ended_at: Option<u64>,
    /// "running" | "completed" | "failed" | "timeout"
    pub status: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TaskCommentDto {
    pub id: String,
    pub task_id: String,
    pub author: String,
    pub body: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TaskEventDto {
    pub id: String,
    pub team_id: String,
    /// Snake-case (e.g. "`task_assigned`", "`task_completed`").
    pub event_type: String,
    pub agent_id: String,
    #[serde(default)]
    pub payload: Value,
    /// RFC3339 timestamp.
    pub timestamp: String,
}

/// R3 — `TaskExitJournal` mirror DTO.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TaskExitJournalDto {
    pub task_id: String,
    pub agent_id: String,
    pub summary: String,
    #[serde(default)]
    pub decisions: Vec<String>,
    #[serde(default)]
    pub artifacts_ref: Vec<String>,
    #[serde(default)]
    pub next_steps: Vec<String>,
    #[serde(default)]
    pub confidence: Option<u8>,
    pub created_at: u64,
}
