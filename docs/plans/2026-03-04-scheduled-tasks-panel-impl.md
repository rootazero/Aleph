# Scheduled Tasks Panel Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a "Scheduled Tasks" view to the Dashboard mode with left-right split pane layout for managing cron jobs.

**Architecture:** Dashboard sidebar gets a new "Scheduled Tasks" entry routing to `/dashboard/cron`. The view follows the `RoutingRulesView` split-pane pattern (left: job list, right: editor + history). A new `api/cron.rs` module wraps RPC calls. Gateway handlers are expanded from 3 stubs to 8 full CRUD stubs.

**Tech Stack:** Leptos 0.7 (signals, components, `spawn_local`), Tailwind CSS utility classes, JSON-RPC over WebSocket via `DashboardState::rpc_call()`.

---

### Task 1: Expand Gateway cron RPC handler stubs

**Files:**
- Modify: `core/src/gateway/handlers/cron.rs`
- Modify: `core/src/gateway/handlers/mod.rs:213-215` (handler registration)

**Step 1: Add new stub handlers to `core/src/gateway/handlers/cron.rs`**

Replace the entire file with expanded stubs. Keep existing `handle_list`, `handle_status`, `handle_run` and add `handle_get`, `handle_create`, `handle_update`, `handle_delete`, `handle_runs`, `handle_toggle`.

```rust
//! Cron job RPC handlers.
//!
//! Handlers for cron job operations: list, get, create, update, delete, run, runs, toggle, status.

use serde_json::{json, Value};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};

/// Handle cron.list RPC request
///
/// Returns a list of all configured cron jobs.
pub async fn handle_list(request: JsonRpcRequest) -> JsonRpcResponse {
    // TODO: Integrate with actual CronService when available
    JsonRpcResponse::success(
        request.id,
        json!({
            "jobs": []
        }),
    )
}

/// Handle cron.get RPC request
///
/// Returns a single cron job by ID.
pub async fn handle_get(request: JsonRpcRequest) -> JsonRpcResponse {
    let job_id = match &request.params {
        Some(Value::Object(map)) => map.get("job_id").and_then(|v| v.as_str()),
        _ => None,
    };

    match job_id {
        Some(_id) => {
            // TODO: Integrate with actual CronService
            JsonRpcResponse::error(request.id, INVALID_PARAMS, "Job not found")
        }
        None => JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id"),
    }
}

/// Handle cron.create RPC request
///
/// Creates a new cron job.
pub async fn handle_create(request: JsonRpcRequest) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => Some(map),
        _ => None,
    };

    let params = match params {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params");
        }
    };

    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if name.is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing name");
    }

    // TODO: Integrate with actual CronService
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();

    JsonRpcResponse::success(
        request.id,
        json!({
            "id": id,
            "name": name,
            "schedule": params.get("schedule").and_then(|v| v.as_str()).unwrap_or(""),
            "schedule_kind": params.get("schedule_kind").and_then(|v| v.as_str()).unwrap_or("cron"),
            "agent_id": params.get("agent_id").and_then(|v| v.as_str()).unwrap_or("main"),
            "prompt": params.get("prompt").and_then(|v| v.as_str()).unwrap_or(""),
            "enabled": params.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
            "timezone": params.get("timezone").and_then(|v| v.as_str()),
            "tags": params.get("tags").cloned().unwrap_or(json!([])),
            "next_run_at": null,
            "last_run_at": null,
            "created_at": now,
            "updated_at": now,
        }),
    )
}

/// Handle cron.update RPC request
///
/// Updates an existing cron job.
pub async fn handle_update(request: JsonRpcRequest) -> JsonRpcResponse {
    let job_id = match &request.params {
        Some(Value::Object(map)) => map.get("job_id").and_then(|v| v.as_str()),
        _ => None,
    };

    match job_id {
        Some(_id) => {
            // TODO: Integrate with actual CronService
            JsonRpcResponse::error(request.id, INVALID_PARAMS, "Job not found")
        }
        None => JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id"),
    }
}

/// Handle cron.delete RPC request
///
/// Deletes a cron job by ID.
pub async fn handle_delete(request: JsonRpcRequest) -> JsonRpcResponse {
    let job_id = match &request.params {
        Some(Value::Object(map)) => map.get("job_id").and_then(|v| v.as_str()),
        _ => None,
    };

    match job_id {
        Some(_id) => {
            // TODO: Integrate with actual CronService
            JsonRpcResponse::success(request.id, json!({ "deleted": true }))
        }
        None => JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id"),
    }
}

/// Handle cron.status RPC request
///
/// Returns the status of the cron service.
pub async fn handle_status(request: JsonRpcRequest) -> JsonRpcResponse {
    // TODO: Integrate with actual CronService when available
    JsonRpcResponse::success(
        request.id,
        json!({
            "running": true,
            "job_count": 0,
            "last_tick": null
        }),
    )
}

/// Handle cron.run RPC request
///
/// Manually triggers a cron job by ID.
pub async fn handle_run(request: JsonRpcRequest) -> JsonRpcResponse {
    let job_id = match &request.params {
        Some(Value::Object(map)) => map.get("job_id").and_then(|v| v.as_str()),
        _ => None,
    };

    let job_id = match job_id {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id");
        }
    };

    // TODO: Integrate with actual CronService when available
    JsonRpcResponse::success(
        request.id,
        json!({
            "triggered": job_id,
            "status": "queued"
        }),
    )
}

/// Handle cron.runs RPC request
///
/// Returns execution history for a job.
pub async fn handle_runs(request: JsonRpcRequest) -> JsonRpcResponse {
    let job_id = match &request.params {
        Some(Value::Object(map)) => map.get("job_id").and_then(|v| v.as_str()),
        _ => None,
    };

    match job_id {
        Some(_id) => {
            // TODO: Integrate with actual CronService
            JsonRpcResponse::success(request.id, json!({ "runs": [] }))
        }
        None => JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id"),
    }
}

/// Handle cron.toggle RPC request
///
/// Enables or disables a cron job.
pub async fn handle_toggle(request: JsonRpcRequest) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => Some(map),
        _ => None,
    };

    let params = match params {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params");
        }
    };

    let job_id = params.get("job_id").and_then(|v| v.as_str());
    let _enabled = params.get("enabled").and_then(|v| v.as_bool());

    match job_id {
        Some(_id) => {
            // TODO: Integrate with actual CronService
            JsonRpcResponse::error(request.id, INVALID_PARAMS, "Job not found")
        }
        None => JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_handle_list() {
        let request = JsonRpcRequest::with_id("cron.list", None, json!(1));
        let response = handle_list(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_status() {
        let request = JsonRpcRequest::with_id("cron.status", None, json!(1));
        let response = handle_status(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_run() {
        let request = JsonRpcRequest::new(
            "cron.run",
            Some(json!({ "job_id": "daily-backup" })),
            Some(json!(1)),
        );
        let response = handle_run(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_run_missing_job_id() {
        let request = JsonRpcRequest::with_id("cron.run", None, json!(1));
        let response = handle_run(request).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_create() {
        let request = JsonRpcRequest::new(
            "cron.create",
            Some(json!({
                "name": "Test Job",
                "schedule": "0 0 9 * * *",
                "agent_id": "main",
                "prompt": "Do something"
            })),
            Some(json!(1)),
        );
        let response = handle_create(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_create_missing_name() {
        let request = JsonRpcRequest::new(
            "cron.create",
            Some(json!({})),
            Some(json!(1)),
        );
        let response = handle_create(request).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_delete() {
        let request = JsonRpcRequest::new(
            "cron.delete",
            Some(json!({ "job_id": "some-id" })),
            Some(json!(1)),
        );
        let response = handle_delete(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_runs() {
        let request = JsonRpcRequest::new(
            "cron.runs",
            Some(json!({ "job_id": "some-id" })),
            Some(json!(1)),
        );
        let response = handle_runs(request).await;
        assert!(response.is_success());
    }
}
```

**Step 2: Register new handlers in `core/src/gateway/handlers/mod.rs`**

Find lines 212-215 (the existing cron registration block):
```rust
        // Cron handlers
        registry.register("cron.list", cron::handle_list);
        registry.register("cron.status", cron::handle_status);
        registry.register("cron.run", cron::handle_run);
```

Replace with:
```rust
        // Cron handlers
        registry.register("cron.list", cron::handle_list);
        registry.register("cron.get", cron::handle_get);
        registry.register("cron.create", cron::handle_create);
        registry.register("cron.update", cron::handle_update);
        registry.register("cron.delete", cron::handle_delete);
        registry.register("cron.status", cron::handle_status);
        registry.register("cron.run", cron::handle_run);
        registry.register("cron.runs", cron::handle_runs);
        registry.register("cron.toggle", cron::handle_toggle);
```

**Step 3: Update handler registry test**

Find `test_cron_handlers_registered` test (around line 661) and replace:
```rust
    #[test]
    fn test_cron_handlers_registered() {
        let registry = HandlerRegistry::new();
        assert!(registry.has_method("cron.list"));
        assert!(registry.has_method("cron.get"));
        assert!(registry.has_method("cron.create"));
        assert!(registry.has_method("cron.update"));
        assert!(registry.has_method("cron.delete"));
        assert!(registry.has_method("cron.status"));
        assert!(registry.has_method("cron.run"));
        assert!(registry.has_method("cron.runs"));
        assert!(registry.has_method("cron.toggle"));
    }
```

**Step 4: Run tests to verify**

Run: `cargo test -p alephcore --lib cron`
Expected: All cron handler tests pass.

**Step 5: Commit**

```
gateway: expand cron RPC handlers with full CRUD stubs
```

---

### Task 2: Create UI API layer (`api/cron.rs`)

**Files:**
- Create: `core/ui/control_plane/src/api/cron.rs`
- Modify: `core/ui/control_plane/src/api.rs:1` (add `pub mod cron;`)

**Step 1: Create `core/ui/control_plane/src/api/cron.rs`**

```rust
//! Cron job API — RPC wrappers for scheduled task management.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::context::DashboardState;

// ============================================================================
// DTOs
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
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
// API
// ============================================================================

pub struct CronApi;

impl CronApi {
    /// List all cron jobs
    pub async fn list(state: &DashboardState) -> Result<Vec<CronJobInfo>, String> {
        let result = state.rpc_call("cron.list", Value::Null).await?;
        let jobs: Vec<CronJobInfo> = result
            .get("jobs")
            .cloned()
            .unwrap_or(Value::Array(vec![]));
        serde_json::from_value(jobs).map_err(|e| format!("Failed to parse jobs: {}", e))
    }

    /// Create a new cron job
    pub async fn create(
        state: &DashboardState,
        job: CreateCronJob,
    ) -> Result<CronJobInfo, String> {
        let params = serde_json::to_value(&job).map_err(|e| e.to_string())?;
        let result = state.rpc_call("cron.create", params).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse created job: {}", e))
    }

    /// Update an existing cron job
    pub async fn update(
        state: &DashboardState,
        patch: UpdateCronJob,
    ) -> Result<CronJobInfo, String> {
        let params = serde_json::to_value(&patch).map_err(|e| e.to_string())?;
        let result = state.rpc_call("cron.update", params).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse updated job: {}", e))
    }

    /// Delete a cron job
    pub async fn delete(state: &DashboardState, job_id: &str) -> Result<(), String> {
        let params = serde_json::json!({ "job_id": job_id });
        state.rpc_call("cron.delete", params).await?;
        Ok(())
    }

    /// Get execution history for a job
    pub async fn runs(
        state: &DashboardState,
        job_id: &str,
        limit: i32,
    ) -> Result<Vec<JobRunInfo>, String> {
        let params = serde_json::json!({ "job_id": job_id, "limit": limit });
        let result = state.rpc_call("cron.runs", params).await?;
        let runs: Value = result
            .get("runs")
            .cloned()
            .unwrap_or(Value::Array(vec![]));
        serde_json::from_value(runs).map_err(|e| format!("Failed to parse runs: {}", e))
    }

    /// Toggle job enabled/disabled
    pub async fn toggle(
        state: &DashboardState,
        job_id: &str,
        enabled: bool,
    ) -> Result<CronJobInfo, String> {
        let params = serde_json::json!({ "job_id": job_id, "enabled": enabled });
        let result = state.rpc_call("cron.toggle", params).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse toggled job: {}", e))
    }

    /// Manually trigger a job
    pub async fn run_now(state: &DashboardState, job_id: &str) -> Result<Value, String> {
        let params = serde_json::json!({ "job_id": job_id });
        state.rpc_call("cron.run", params).await
    }
}
```

**Step 2: Register the module in `core/ui/control_plane/src/api.rs`**

Add `pub mod cron;` at the top, after `pub mod chat;` (line 1):

```rust
pub mod chat;
pub mod cron;
```

**Step 3: Verify compilation**

Run: `cargo check -p control_plane` (or the appropriate crate name for the UI)
Expected: Compiles without errors.

**Step 4: Commit**

```
panel: add cron API wrapper for RPC calls
```

---

### Task 3: Create the CronView with all sub-components

**Files:**
- Create: `core/ui/control_plane/src/views/cron.rs`
- Modify: `core/ui/control_plane/src/views/mod.rs` (add `pub mod cron;`)

**Step 1: Create `core/ui/control_plane/src/views/cron.rs`**

This is the main view file containing `CronView`, `JobList`, `JobListItem`, `JobEditor`, and `RunHistory` components. Follow `RoutingRulesView` patterns exactly.

```rust
//! Scheduled Tasks View
//!
//! Dashboard view for managing cron jobs:
//! - Left pane: job list with status indicators
//! - Right pane: job editor form + execution history

use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::context::DashboardState;
use crate::api::cron::{CronApi, CronJobInfo, CreateCronJob, UpdateCronJob, JobRunInfo};

// ============================================================================
// CronView — Main container
// ============================================================================

#[component]
pub fn CronView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // State
    let jobs = RwSignal::new(Vec::<CronJobInfo>::new());
    let selected = RwSignal::new(Option::<usize>::None);
    let loading = RwSignal::new(true);
    let saving = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);

    // Load jobs on mount
    spawn_local(async move {
        match CronApi::list(&state).await {
            Ok(list) => {
                jobs.set(list);
                loading.set(false);
            }
            Err(e) => {
                error.set(Some(format!("Failed to load jobs: {}", e)));
                loading.set(false);
            }
        }
    });

    view! {
        <div class="flex flex-col h-full">
            // Header
            <div class="p-6 border-b border-border">
                <h1 class="text-2xl font-bold text-text-primary">"Scheduled Tasks"</h1>
                <p class="mt-1 text-sm text-text-secondary">
                    "Manage automated jobs that run on a schedule"
                </p>
            </div>

            // Content
            <div class="flex-1 flex overflow-hidden">
                <JobList jobs=jobs selected=selected loading=loading />
                <JobEditor jobs=jobs selected=selected saving=saving error=error />
            </div>
        </div>
    }
}

// ============================================================================
// JobList — Left pane
// ============================================================================

#[component]
fn JobList(
    jobs: RwSignal<Vec<CronJobInfo>>,
    selected: RwSignal<Option<usize>>,
    loading: RwSignal<bool>,
) -> impl IntoView {
    view! {
        <div class="w-80 border-r border-border flex flex-col">
            // Add button
            <div class="p-4 border-b border-border">
                <button
                    on:click=move |_| selected.set(Some(usize::MAX))
                    class="w-full px-4 py-2 bg-primary hover:bg-primary-hover text-white rounded-lg transition-colors"
                >
                    "+ New Job"
                </button>
            </div>

            // Jobs list
            <div class="flex-1 overflow-y-auto">
                {move || {
                    if loading.get() {
                        view! {
                            <div class="p-4 text-center text-text-secondary">
                                "Loading..."
                            </div>
                        }.into_any()
                    } else if jobs.get().is_empty() {
                        view! {
                            <div class="p-8 text-center">
                                <div class="text-text-tertiary mb-2">
                                    <svg class="w-12 h-12 mx-auto mb-3 opacity-50" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                                        <circle cx="12" cy="12" r="10" />
                                        <polyline points="12 6 12 12 16 14" />
                                    </svg>
                                </div>
                                <p class="text-sm text-text-tertiary">"No scheduled tasks yet"</p>
                                <p class="text-xs text-text-tertiary mt-1">"Click '+ New Job' to create one"</p>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="p-2 space-y-1">
                                {move || {
                                    jobs.get().iter().enumerate().map(|(idx, job)| {
                                        let job = job.clone();
                                        let is_selected = Signal::derive(move || selected.get() == Some(idx));
                                        view! {
                                            <JobListItem job=job index=idx is_selected=is_selected selected=selected />
                                        }
                                    }).collect::<Vec<_>>()
                                }}
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

// ============================================================================
// JobListItem — Individual job card in list
// ============================================================================

#[component]
fn JobListItem(
    job: CronJobInfo,
    index: usize,
    is_selected: Signal<bool>,
    selected: RwSignal<Option<usize>>,
) -> impl IntoView {
    let name = job.name.clone();
    let schedule = job.schedule.clone();
    let schedule_kind = job.schedule_kind.clone();
    let enabled = job.enabled;
    let next_run_at = job.next_run_at;

    let schedule_display = format_schedule_summary(&schedule_kind, &schedule);
    let next_run_display = next_run_at
        .map(|ts| format_relative_time(ts))
        .unwrap_or_else(|| "—".to_string());

    view! {
        <button
            on:click=move |_| selected.set(Some(index))
            class=move || {
                if is_selected.get() {
                    "w-full p-3 bg-primary-subtle border border-primary rounded-lg text-left transition-colors"
                } else {
                    "w-full p-3 bg-surface-sunken border border-border hover:border-border-strong rounded-lg text-left transition-colors"
                }
            }
        >
            <div class="flex items-center gap-2 mb-1">
                // Status dot
                <span class=move || {
                    if enabled {
                        "w-2 h-2 rounded-full bg-success flex-shrink-0"
                    } else {
                        "w-2 h-2 rounded-full bg-text-tertiary flex-shrink-0"
                    }
                }></span>
                <span class="text-sm font-medium text-text-primary truncate">
                    {name}
                </span>
            </div>
            <div class="ml-4 text-xs text-text-secondary">
                {schedule_display}
            </div>
            <div class="ml-4 text-xs text-text-tertiary mt-0.5">
                "Next: " {next_run_display}
            </div>
        </button>
    }
}

// ============================================================================
// JobEditor — Right pane (form + history)
// ============================================================================

#[component]
fn JobEditor(
    jobs: RwSignal<Vec<CronJobInfo>>,
    selected: RwSignal<Option<usize>>,
    saving: RwSignal<bool>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // Form signals
    let form_name = RwSignal::new(String::new());
    let form_schedule_kind = RwSignal::new(String::from("cron"));
    let form_schedule = RwSignal::new(String::new());
    let form_agent_id = RwSignal::new(String::from("main"));
    let form_prompt = RwSignal::new(String::new());
    let form_timezone = RwSignal::new(String::new());
    let form_tags = RwSignal::new(String::new());
    let form_enabled = RwSignal::new(true);

    // Execution history
    let runs = RwSignal::new(Vec::<JobRunInfo>::new());

    let is_new = move || selected.get() == Some(usize::MAX);
    let is_editing = move || selected.get().is_some();

    // Load job data when selection changes
    Effect::new(move || {
        if let Some(idx) = selected.get() {
            runs.set(Vec::new());
            if idx == usize::MAX {
                // Reset form for new job
                form_name.set(String::new());
                form_schedule_kind.set(String::from("cron"));
                form_schedule.set(String::new());
                form_agent_id.set(String::from("main"));
                form_prompt.set(String::new());
                form_timezone.set(String::new());
                form_tags.set(String::new());
                form_enabled.set(true);
            } else if let Some(job) = jobs.get().get(idx) {
                form_name.set(job.name.clone());
                form_schedule_kind.set(job.schedule_kind.clone());
                form_schedule.set(job.schedule.clone());
                form_agent_id.set(job.agent_id.clone());
                form_prompt.set(job.prompt.clone());
                form_timezone.set(job.timezone.clone().unwrap_or_default());
                form_tags.set(job.tags.join(", "));
                form_enabled.set(job.enabled);

                // Load run history
                let job_id = job.id.clone();
                spawn_local(async move {
                    if let Ok(history) = CronApi::runs(&state, &job_id, 10).await {
                        runs.set(history);
                    }
                });
            }
        }
    });

    // Handle save
    let on_save = move |_| {
        let name = form_name.get();
        if name.is_empty() {
            error.set(Some("Job name is required".to_string()));
            return;
        }

        saving.set(true);
        error.set(None);

        let tags: Vec<String> = form_tags.get()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let timezone = {
            let tz = form_timezone.get();
            if tz.is_empty() { None } else { Some(tz) }
        };

        if is_new() {
            let new_job = CreateCronJob {
                name,
                schedule: form_schedule.get(),
                schedule_kind: form_schedule_kind.get(),
                agent_id: form_agent_id.get(),
                prompt: form_prompt.get(),
                enabled: form_enabled.get(),
                timezone,
                tags,
            };

            spawn_local(async move {
                match CronApi::create(&state, new_job).await {
                    Ok(_created) => {
                        // Reload list
                        if let Ok(list) = CronApi::list(&state).await {
                            jobs.set(list);
                        }
                        selected.set(None);
                    }
                    Err(e) => error.set(Some(format!("Failed to create: {}", e))),
                }
                saving.set(false);
            });
        } else if let Some(idx) = selected.get() {
            if let Some(job) = jobs.get().get(idx) {
                let patch = UpdateCronJob {
                    job_id: job.id.clone(),
                    name: Some(name),
                    schedule: Some(form_schedule.get()),
                    schedule_kind: Some(form_schedule_kind.get()),
                    agent_id: Some(form_agent_id.get()),
                    prompt: Some(form_prompt.get()),
                    enabled: Some(form_enabled.get()),
                    timezone,
                    tags: Some(tags),
                };

                spawn_local(async move {
                    match CronApi::update(&state, patch).await {
                        Ok(_updated) => {
                            if let Ok(list) = CronApi::list(&state).await {
                                jobs.set(list);
                            }
                            selected.set(None);
                        }
                        Err(e) => error.set(Some(format!("Failed to update: {}", e))),
                    }
                    saving.set(false);
                });
            }
        }
    };

    // Handle delete
    let on_delete = move |_| {
        if let Some(idx) = selected.get() {
            if idx == usize::MAX { return; }
            if let Some(job) = jobs.get().get(idx) {
                let job_id = job.id.clone();
                saving.set(true);
                error.set(None);

                spawn_local(async move {
                    match CronApi::delete(&state, &job_id).await {
                        Ok(()) => {
                            if let Ok(list) = CronApi::list(&state).await {
                                jobs.set(list);
                            }
                            selected.set(None);
                        }
                        Err(e) => error.set(Some(format!("Failed to delete: {}", e))),
                    }
                    saving.set(false);
                });
            }
        }
    };

    // Handle run now
    let on_run_now = move |_| {
        if let Some(idx) = selected.get() {
            if idx == usize::MAX { return; }
            if let Some(job) = jobs.get().get(idx) {
                let job_id = job.id.clone();
                spawn_local(async move {
                    match CronApi::run_now(&state, &job_id).await {
                        Ok(_) => {
                            // Optionally reload runs
                        }
                        Err(e) => error.set(Some(format!("Failed to trigger: {}", e))),
                    }
                });
            }
        }
    };

    view! {
        <div class="flex-1 overflow-y-auto">
            {move || {
                if !is_editing() {
                    view! {
                        <div class="flex items-center justify-center h-full text-text-tertiary">
                            <div class="text-center">
                                <svg class="w-16 h-16 mx-auto mb-4 opacity-30" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                                    <circle cx="12" cy="12" r="10" />
                                    <polyline points="12 6 12 12 16 14" />
                                </svg>
                                <p>"Select a job to view details, or create a new one"</p>
                            </div>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="p-8 max-w-3xl mx-auto">
                            // Header
                            <div class="mb-6">
                                <h2 class="text-2xl font-bold text-text-primary mb-2">
                                    {move || if is_new() { "New Scheduled Task" } else { "Edit Scheduled Task" }}
                                </h2>
                            </div>

                            // Error banner
                            {move || {
                                if let Some(err) = error.get() {
                                    view! {
                                        <div class="mb-4 p-4 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">
                                            {err}
                                        </div>
                                    }.into_any()
                                } else {
                                    view! { <div></div> }.into_any()
                                }
                            }}

                            // Form
                            <div class="space-y-6">
                                // Name
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">"Name"</label>
                                    <input
                                        type="text"
                                        prop:value=move || form_name.get()
                                        on:input=move |ev| form_name.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                        placeholder="e.g., Daily Report"
                                    />
                                </div>

                                // Schedule Kind + Schedule (side by side)
                                <div class="grid grid-cols-3 gap-4">
                                    <div>
                                        <label class="block text-sm font-medium text-text-secondary mb-2">"Schedule Type"</label>
                                        <select
                                            prop:value=move || form_schedule_kind.get()
                                            on:change=move |ev| form_schedule_kind.set(event_target_value(&ev))
                                            class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                        >
                                            <option value="cron">"Cron"</option>
                                            <option value="every">"Every"</option>
                                            <option value="at">"At"</option>
                                        </select>
                                    </div>
                                    <div class="col-span-2">
                                        <label class="block text-sm font-medium text-text-secondary mb-2">"Schedule"</label>
                                        <input
                                            type="text"
                                            prop:value=move || form_schedule.get()
                                            on:input=move |ev| form_schedule.set(event_target_value(&ev))
                                            class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary font-mono focus:outline-none focus:border-primary"
                                            placeholder=move || {
                                                match form_schedule_kind.get().as_str() {
                                                    "every" => "60000 (ms)",
                                                    "at" => "1709510400 (unix timestamp)",
                                                    _ => "0 0 9 * * * (cron expression)",
                                                }
                                            }
                                        />
                                    </div>
                                </div>

                                // Agent
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">"Agent"</label>
                                    <input
                                        type="text"
                                        prop:value=move || form_agent_id.get()
                                        on:input=move |ev| form_agent_id.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                        placeholder="main"
                                    />
                                    <p class="mt-1 text-xs text-text-tertiary">"Agent ID to invoke for this task"</p>
                                </div>

                                // Prompt
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">"Prompt"</label>
                                    <textarea
                                        prop:value=move || form_prompt.get()
                                        on:input=move |ev| form_prompt.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                        rows="3"
                                        placeholder="Message to send to the agent..."
                                    ></textarea>
                                </div>

                                // Timezone + Tags (side by side)
                                <div class="grid grid-cols-2 gap-4">
                                    <div>
                                        <label class="block text-sm font-medium text-text-secondary mb-2">"Timezone"</label>
                                        <input
                                            type="text"
                                            prop:value=move || form_timezone.get()
                                            on:input=move |ev| form_timezone.set(event_target_value(&ev))
                                            class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                            placeholder="Asia/Shanghai (optional)"
                                        />
                                    </div>
                                    <div>
                                        <label class="block text-sm font-medium text-text-secondary mb-2">"Tags"</label>
                                        <input
                                            type="text"
                                            prop:value=move || form_tags.get()
                                            on:input=move |ev| form_tags.set(event_target_value(&ev))
                                            class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                            placeholder="report, daily (comma-separated)"
                                        />
                                    </div>
                                </div>

                                // Enabled toggle
                                <div class="flex items-center gap-3">
                                    <button
                                        on:click=move |_| form_enabled.update(|v| *v = !*v)
                                        class=move || {
                                            if form_enabled.get() {
                                                "relative w-11 h-6 bg-primary rounded-full transition-colors"
                                            } else {
                                                "relative w-11 h-6 bg-surface-sunken border border-border rounded-full transition-colors"
                                            }
                                        }
                                    >
                                        <span class=move || {
                                            if form_enabled.get() {
                                                "absolute top-0.5 left-5 w-5 h-5 bg-white rounded-full shadow transition-all"
                                            } else {
                                                "absolute top-0.5 left-0.5 w-5 h-5 bg-white rounded-full shadow transition-all"
                                            }
                                        }></span>
                                    </button>
                                    <label class="text-sm font-medium text-text-secondary">"Enabled"</label>
                                </div>
                            </div>

                            // Action buttons
                            <div class="mt-8 flex items-center gap-3">
                                <button
                                    on:click=on_save
                                    prop:disabled=move || saving.get()
                                    class="px-6 py-2 bg-primary hover:bg-primary-hover disabled:bg-primary/50 text-white rounded-lg transition-colors disabled:cursor-not-allowed"
                                >
                                    {move || if saving.get() { "Saving..." } else { "Save" }}
                                </button>

                                {move || {
                                    if !is_new() {
                                        view! {
                                            <button
                                                on:click=on_run_now
                                                prop:disabled=move || saving.get()
                                                class="px-6 py-2 bg-surface-sunken hover:bg-surface-raised border border-border text-text-primary rounded-lg transition-colors disabled:cursor-not-allowed"
                                            >
                                                "Run Now"
                                            </button>
                                        }.into_any()
                                    } else {
                                        view! { <span></span> }.into_any()
                                    }
                                }}

                                {move || {
                                    if !is_new() {
                                        view! {
                                            <button
                                                on:click=on_delete
                                                prop:disabled=move || saving.get()
                                                class="px-6 py-2 bg-danger hover:bg-danger/80 disabled:bg-danger/50 text-white rounded-lg transition-colors disabled:cursor-not-allowed"
                                            >
                                                "Delete"
                                            </button>
                                        }.into_any()
                                    } else {
                                        view! { <span></span> }.into_any()
                                    }
                                }}

                                <button
                                    on:click=move |_| selected.set(None)
                                    class="px-6 py-2 bg-surface-sunken hover:bg-surface-raised text-text-primary rounded-lg transition-colors"
                                >
                                    "Cancel"
                                </button>
                            </div>

                            // Execution history (only for existing jobs)
                            {move || {
                                if !is_new() {
                                    view! { <RunHistory runs=runs /> }.into_any()
                                } else {
                                    view! { <div></div> }.into_any()
                                }
                            }}
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

// ============================================================================
// RunHistory — Execution history table
// ============================================================================

#[component]
fn RunHistory(runs: RwSignal<Vec<JobRunInfo>>) -> impl IntoView {
    view! {
        <div class="mt-8 pt-8 border-t border-border">
            <h3 class="text-lg font-semibold text-text-primary mb-4">"Execution History"</h3>

            {move || {
                let run_list = runs.get();
                if run_list.is_empty() {
                    view! {
                        <p class="text-sm text-text-tertiary">"No execution records yet"</p>
                    }.into_any()
                } else {
                    view! {
                        <div class="overflow-x-auto">
                            <table class="w-full text-sm">
                                <thead>
                                    <tr class="text-left text-text-tertiary border-b border-border">
                                        <th class="pb-2 pr-4 font-medium">"Status"</th>
                                        <th class="pb-2 pr-4 font-medium">"Time"</th>
                                        <th class="pb-2 pr-4 font-medium">"Duration"</th>
                                        <th class="pb-2 font-medium">"Error"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {run_list.into_iter().map(|run| {
                                        let status_class = match run.status.as_str() {
                                            "success" => "text-success",
                                            "failed" => "text-danger",
                                            "timeout" => "text-warning",
                                            "running" => "text-primary",
                                            _ => "text-text-tertiary",
                                        };
                                        let status_icon = match run.status.as_str() {
                                            "success" => "✓",
                                            "failed" => "✗",
                                            "timeout" => "⏱",
                                            "running" => "●",
                                            _ => "—",
                                        };
                                        let time_str = format_timestamp(run.started_at);
                                        let duration_str = format_duration(run.duration_ms);
                                        let error_str = run.error.clone().unwrap_or_default();

                                        view! {
                                            <tr class="border-b border-border/50">
                                                <td class="py-2 pr-4">
                                                    <span class=status_class>{status_icon} " " {run.status.clone()}</span>
                                                </td>
                                                <td class="py-2 pr-4 text-text-secondary">{time_str}</td>
                                                <td class="py-2 pr-4 text-text-secondary">{duration_str}</td>
                                                <td class="py-2 text-danger text-xs truncate max-w-xs">{error_str}</td>
                                            </tr>
                                        }
                                    }).collect::<Vec<_>>()}
                                </tbody>
                            </table>
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

// ============================================================================
// Helper functions
// ============================================================================

fn format_schedule_summary(kind: &str, schedule: &str) -> String {
    match kind {
        "every" => {
            if let Ok(ms) = schedule.parse::<u64>() {
                if ms >= 3_600_000 {
                    format!("Every {}h", ms / 3_600_000)
                } else if ms >= 60_000 {
                    format!("Every {}min", ms / 60_000)
                } else {
                    format!("Every {}s", ms / 1_000)
                }
            } else {
                format!("Every {}", schedule)
            }
        }
        "at" => {
            if let Ok(ts) = schedule.parse::<i64>() {
                format_timestamp(ts)
            } else {
                format!("At {}", schedule)
            }
        }
        _ => schedule.to_string(),
    }
}

fn format_relative_time(ts: i64) -> String {
    let now = js_sys::Date::now() as i64 / 1000;
    let diff = ts - now;
    if diff <= 0 {
        "overdue".to_string()
    } else if diff < 60 {
        format!("{}s", diff)
    } else if diff < 3600 {
        format!("{}min", diff / 60)
    } else if diff < 86400 {
        format!("{}h", diff / 3600)
    } else {
        format!("{}d", diff / 86400)
    }
}

fn format_timestamp(ts: i64) -> String {
    if ts == 0 {
        return "—".to_string();
    }
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts as f64 * 1000.0));
    let hours = date.get_hours();
    let minutes = date.get_minutes();
    let month = date.get_month() + 1;
    let day = date.get_date();
    format!("{:02}/{:02} {:02}:{:02}", month, day, hours, minutes)
}

fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{}ms", ms)
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{:.1}min", ms as f64 / 60_000.0)
    }
}
```

**Step 2: Register module in `core/ui/control_plane/src/views/mod.rs`**

Add `pub mod cron;` after the existing modules:

```rust
pub mod home;
pub mod system_status;
pub mod agent_trace;
pub mod memory;
pub mod settings;
pub mod chat;
pub mod cron;
```

**Step 3: Verify compilation**

Run: `cargo check -p control_plane`
Expected: Compiles. Note: `js_sys` and `wasm_bindgen` are likely already dependencies since this is a WASM crate.

**Step 4: Commit**

```
panel: add CronView with job list, editor, and run history
```

---

### Task 4: Wire routing — sidebar + dashboard router

**Files:**
- Modify: `core/ui/control_plane/src/components/dashboard_sidebar.rs:15-39` (add SidebarItem)
- Modify: `core/ui/control_plane/src/app.rs:1-12` (add import)
- Modify: `core/ui/control_plane/src/app.rs:170-185` (add route)

**Step 1: Add SidebarItem to `dashboard_sidebar.rs`**

After the Memory Vault `SidebarItem` (line 38), before the closing `</nav>`, add:

```rust
                <SidebarItem href="/dashboard/cron" label="Scheduled Tasks">
                    <circle cx="12" cy="12" r="10" />
                    <polyline points="12 6 12 12 16 14" />
                </SidebarItem>
```

**Step 2: Add import in `app.rs`**

After line 11 (`use crate::views::settings::*;`), add:

```rust
use crate::views::cron::CronView;
```

**Step 3: Add route in `DashboardRouter`**

In the `DashboardRouter` component (around line 176), add a new match arm before the wildcard `_`:

```rust
            "/dashboard/cron" => view! { <CronView /> }.into_any(),
```

The full match block should read:
```rust
        match path.as_str() {
            "/dashboard" => view! { <Home /> }.into_any(),
            "/dashboard/trace" => view! { <AgentTrace /> }.into_any(),
            "/dashboard/health" => view! { <SystemStatus /> }.into_any(),
            "/dashboard/memory" => view! { <Memory /> }.into_any(),
            "/dashboard/cron" => view! { <CronView /> }.into_any(),
            _ => ().into_any(),
        }
```

**Step 4: Verify compilation**

Run: `cargo check -p control_plane`
Expected: Compiles successfully.

**Step 5: Commit**

```
panel: wire scheduled tasks into dashboard sidebar and router
```

---

### Task 5: Build and verify end-to-end

**Step 1: Build the full project**

Run: `cargo build -p alephcore`
Expected: Successful build.

Run: `cargo test -p alephcore --lib cron`
Expected: All cron handler tests pass.

**Step 2: Build WASM UI (if build system supports it)**

Run: `just panel` or the equivalent WASM build command
Expected: WASM compiles without errors.

**Step 3: Commit final state (if any fixups needed)**

```
panel: scheduled tasks panel complete
```
