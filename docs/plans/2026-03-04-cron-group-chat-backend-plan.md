# Cron & Group Chat Backend Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Wire CronService to Gateway handlers with a real JobExecutor, implement GroupChatExecutor for LLM-driven multi-persona conversations, and replace all stub/placeholder handlers.

**Architecture:** Both systems use direct `AiProvider::process()` calls (no agent loop). CronService already has full SQLite persistence and scheduling — we only add the executor and wire handlers. Group Chat needs a new `GroupChatExecutor` that implements the coordinator→persona LLM loop, plus handler wiring.

**Tech Stack:** Rust, Tokio, rusqlite (CronService), serde_json, `AiProvider` trait, `register_handler!` macro

---

### Task 1: Add CronConfig to FullConfig

**Files:**
- Modify: `core/src/config/structs.rs`

**Context:** `CronConfig` exists in `core/src/cron/config.rs` but is NOT in `Config` (the main app config struct at `core/src/config/structs.rs:16`). `GroupChatConfig` IS already there (line 87). We need to add `CronConfig` so it can be loaded from `aleph.toml` and used at startup.

**Step 1: Add CronConfig import and field to Config**

In `core/src/config/structs.rs`, add after the `use` block at the top:

```rust
use crate::cron::CronConfig;
```

Then add to the `Config` struct (after the `group_chat` field around line 87):

```rust
    /// Cron job scheduling configuration
    #[serde(default)]
    pub cron: CronConfig,
```

**Step 2: Run tests to verify no breakage**

Run: `cargo check -p alephcore`
Expected: PASS (CronConfig implements Default via serde default)

**Step 3: Commit**

```bash
git add core/src/config/structs.rs
git commit -m "config: add CronConfig to app Config struct"
```

---

### Task 2: Implement GroupChatExecutor

**Files:**
- Create: `core/src/group_chat/executor.rs`
- Modify: `core/src/group_chat/mod.rs`

**Context:** The executor takes an `Arc<dyn AiProvider>` and implements a coordinator→persona LLM loop. Pure functions in `coordinator.rs` build prompts and parse plans. `GroupChatSession` has `add_turn(round, speaker, content)` and `build_history_text()`. `AiProvider::process(input, system_prompt) -> Pin<Box<dyn Future<Output = Result<String>> + Send>>`.

**Step 1: Create executor.rs**

Create `core/src/group_chat/executor.rs`:

```rust
//! GroupChat Executor — LLM execution layer for multi-persona conversations.
//!
//! Implements the coordinator → persona loop using `AiProvider::process()`.

use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

use super::coordinator::{
    build_coordinator_prompt, build_fallback_plan, build_persona_prompt, parse_coordinator_plan,
};
use super::protocol::{GroupChatError, GroupChatMessage, Speaker};
use super::session::GroupChatSession;

/// Executes LLM calls for group chat rounds.
///
/// The executor does NOT own sessions — it receives a mutable session reference
/// from the orchestrator and performs the coordinator → persona LLM loop.
pub struct GroupChatExecutor {
    provider: Arc<dyn AiProvider>,
}

impl GroupChatExecutor {
    /// Create a new executor with the given AI provider.
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self { provider }
    }

    /// Execute one round of group chat discussion.
    ///
    /// Flow:
    /// 1. Build coordinator prompt from session state + user message
    /// 2. Call LLM to get coordinator plan (which personas respond, in what order)
    /// 3. For each persona in the plan, build persona prompt and call LLM
    /// 4. Record each response as a turn in the session
    ///
    /// Returns the list of messages generated in this round.
    pub async fn execute_round(
        &self,
        session: &mut GroupChatSession,
        user_message: &str,
    ) -> Result<Vec<GroupChatMessage>, GroupChatError> {
        let round = session.current_round + 1;

        // Record user message as a turn
        session.add_turn(round, Speaker::System, format!("User: {}", user_message));

        // 1. Build coordinator prompt
        let history = session.build_history_text();
        let coord_prompt =
            build_coordinator_prompt(&session.participants, user_message, &history, &session.topic);

        // 2. Call LLM for coordinator plan
        let coord_response = self
            .provider
            .process(&coord_prompt, None)
            .await
            .map_err(|e| GroupChatError::ProviderUnavailable(e.to_string()))?;

        let plan = parse_coordinator_plan(&coord_response)
            .unwrap_or_else(|_| build_fallback_plan(&session.participants));

        // 3. Execute each persona in order
        let mut messages = Vec::new();
        let mut prior_discussion = String::new();

        for respondent in &plan.respondents {
            let persona = session
                .participants
                .iter()
                .find(|p| p.id == respondent.persona_id)
                .ok_or_else(|| GroupChatError::PersonaNotFound(respondent.persona_id.clone()))?;

            let persona_prompt = build_persona_prompt(
                persona,
                user_message,
                &prior_discussion,
                &respondent.guidance,
            );

            let response = self
                .provider
                .process(&persona_prompt, Some(&persona.system_prompt))
                .await
                .map_err(|e| GroupChatError::PersonaInvocationFailed {
                    persona_id: persona.id.clone(),
                    reason: e.to_string(),
                })?;

            // Record turn in session
            let speaker = Speaker::Persona {
                id: persona.id.clone(),
                name: persona.name.clone(),
            };
            session.add_turn(round, speaker.clone(), response.clone());

            // Build message for return
            let msg = GroupChatMessage {
                session_id: session.id.clone(),
                speaker,
                content: response.clone(),
                round,
                sequence: messages.len() as u32,
                is_final: false,
            };
            messages.push(msg);

            // Accumulate prior discussion for next persona
            prior_discussion.push_str(&format!("[{}]: {}\n\n", persona.name, response));
        }

        // Mark last message as final
        if let Some(last) = messages.last_mut() {
            last.is_final = true;
        }

        Ok(messages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::group_chat::protocol::Persona;
    use std::pin::Pin;
    use std::future::Future;

    /// Mock provider that returns deterministic responses based on input content.
    struct MockProvider;

    impl AiProvider for MockProvider {
        fn process(
            &self,
            input: &str,
            _system_prompt: Option<&str>,
        ) -> Pin<Box<dyn Future<Output = anyhow::Result<String>> + Send + '_>> {
            let input = input.to_string();
            Box::pin(async move {
                // If input contains "Coordinator" — return a valid coordinator plan
                if input.contains("Coordinator of a multi-persona group chat") {
                    Ok(r#"{"respondents":[{"persona_id":"alice","order":0,"guidance":"Share your thoughts"}],"need_summary":false}"#.to_string())
                } else {
                    // Persona response
                    Ok("I think we should use async channels for this.".to_string())
                }
            })
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn color(&self) -> &str {
            "gray"
        }
    }

    fn make_session() -> GroupChatSession {
        let participants = vec![
            Persona {
                id: "alice".to_string(),
                name: "Alice".to_string(),
                system_prompt: "You are Alice, a Rust expert.".to_string(),
                provider: None,
                model: None,
                thinking_level: None,
            },
        ];

        GroupChatSession::new(
            "test-session".to_string(),
            Some("Rust patterns".to_string()),
            participants,
            "test".to_string(),
            "test:1".to_string(),
        )
    }

    #[tokio::test]
    async fn test_execute_round_basic() {
        let executor = GroupChatExecutor::new(Arc::new(MockProvider));
        let mut session = make_session();

        let messages = executor
            .execute_round(&mut session, "What patterns should we use?")
            .await
            .unwrap();

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].round, 1);
        assert_eq!(messages[0].sequence, 0);
        assert!(messages[0].is_final);
        assert_eq!(messages[0].speaker.name(), "Alice");
        assert!(!messages[0].content.is_empty());
        assert_eq!(session.current_round, 1);
    }

    #[tokio::test]
    async fn test_execute_round_records_history() {
        let executor = GroupChatExecutor::new(Arc::new(MockProvider));
        let mut session = make_session();

        let _ = executor
            .execute_round(&mut session, "Hello")
            .await
            .unwrap();

        // Should have 2 turns: user message (System) + Alice's response
        assert_eq!(session.history.len(), 2);

        let history_text = session.build_history_text();
        assert!(history_text.contains("[System]"));
        assert!(history_text.contains("[Alice]"));
    }
}
```

**Step 2: Add pub mod to group_chat/mod.rs**

In `core/src/group_chat/mod.rs`, add after `pub mod session;`:

```rust
pub mod executor;
```

And add to the re-exports:

```rust
pub use executor::GroupChatExecutor;
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib group_chat::executor`
Expected: 2 tests PASS

**Step 4: Commit**

```bash
git add core/src/group_chat/executor.rs core/src/group_chat/mod.rs
git commit -m "group_chat: add GroupChatExecutor with coordinator-persona LLM loop"
```

---

### Task 3: Replace cron Gateway handler stubs

**Files:**
- Modify: `core/src/gateway/handlers/cron.rs`

**Context:** Currently all 9 handlers are stateless stubs returning fake data. We need to change them to accept `Arc<tokio::sync::Mutex<CronService>>` as context and delegate to real `CronService` methods. The handlers will be re-registered at startup to inject the context.

Handler signatures change from `pub async fn handle_xxx(request: JsonRpcRequest) -> JsonRpcResponse` to `pub async fn handle_xxx(request: JsonRpcRequest, cron: Arc<tokio::sync::Mutex<CronService>>) -> JsonRpcResponse`.

The old stateless versions remain as the `HandlerRegistry::new()` default (fallback when CronService isn't initialized). We keep them but rename to `handle_xxx_stub`.

**Step 1: Rewrite cron.rs**

Replace the entire file `core/src/gateway/handlers/cron.rs` with the new implementation that has both stub and real handlers. The stubs are used as placeholders in `HandlerRegistry::new()`, and the real handlers are wired at startup.

```rust
//! Cron job RPC handlers.
//!
//! Each handler has two variants:
//! - Stub: registered in HandlerRegistry::new() as fallback (no CronService)
//! - Real: wired at startup with Arc<Mutex<CronService>> context

use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::cron::{CronJob, CronService, ScheduleKind};

/// Shared CronService type alias
pub type SharedCronService = Arc<Mutex<CronService>>;

// =============================================================================
// Real handlers (wired at startup with CronService context)
// =============================================================================

pub async fn handle_list(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let service = cron.lock().await;
    match service.list_jobs().await {
        Ok(jobs) => {
            let jobs_json: Vec<Value> = jobs.iter().map(|j| job_to_json(j)).collect();
            JsonRpcResponse::success(request.id, json!({ "jobs": jobs_json }))
        }
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("{e}")),
    }
}

pub async fn handle_get(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let job_id = match extract_str(&request, "job_id") {
        Some(id) => id,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id"),
    };
    let service = cron.lock().await;
    match service.get_job(&job_id).await {
        Ok(job) => JsonRpcResponse::success(request.id, json!({ "job": job_to_json(&job) })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("{e}")),
    }
}

pub async fn handle_create(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params"),
    };

    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("unnamed");
    let schedule = match params.get("schedule").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing schedule"),
    };
    let agent_id = params.get("agent_id").and_then(|v| v.as_str()).unwrap_or("main");
    let prompt = params.get("prompt").and_then(|v| v.as_str()).unwrap_or("");

    let job = CronJob::new(name, schedule, agent_id, prompt);
    let service = cron.lock().await;
    match service.add_job(job).await {
        Ok(job_id) => JsonRpcResponse::success(request.id, json!({ "job_id": job_id })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("{e}")),
    }
}

pub async fn handle_update(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params"),
    };

    let job_id = match params.get("job_id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id"),
    };

    let service = cron.lock().await;
    let mut job = match service.get_job(&job_id).await {
        Ok(j) => j,
        Err(e) => return JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("{e}")),
    };

    // Apply partial updates
    if let Some(name) = params.get("name").and_then(|v| v.as_str()) {
        job.name = name.to_string();
    }
    if let Some(schedule) = params.get("schedule").and_then(|v| v.as_str()) {
        job.schedule = schedule.to_string();
    }
    if let Some(prompt) = params.get("prompt").and_then(|v| v.as_str()) {
        job.prompt = prompt.to_string();
    }
    if let Some(enabled) = params.get("enabled").and_then(|v| v.as_bool()) {
        job.enabled = enabled;
    }

    match service.update_job(job).await {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "updated": true })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("{e}")),
    }
}

pub async fn handle_delete(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let job_id = match extract_str(&request, "job_id") {
        Some(id) => id,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id"),
    };
    let service = cron.lock().await;
    match service.delete_job(&job_id).await {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "deleted": job_id })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("{e}")),
    }
}

pub async fn handle_status(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let service = cron.lock().await;
    let job_count = service.list_jobs().await.map(|j| j.len()).unwrap_or(0);
    JsonRpcResponse::success(
        request.id,
        json!({
            "running": true,
            "job_count": job_count,
        }),
    )
}

pub async fn handle_run(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let job_id = match extract_str(&request, "job_id") {
        Some(id) => id,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id"),
    };
    let service = cron.lock().await;
    let job = match service.get_job(&job_id).await {
        Ok(j) => j,
        Err(e) => return JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("{e}")),
    };
    // Execute via the executor if available
    match service.executor_ref() {
        Some(executor) => {
            let executor = executor.clone();
            // Drop the lock before awaiting
            drop(service);
            match executor(job.id.clone(), job.agent_id.clone(), job.prompt.clone()).await {
                Ok(result) => JsonRpcResponse::success(
                    request.id,
                    json!({ "job_id": job_id, "status": "completed", "result": result }),
                ),
                Err(e) => JsonRpcResponse::success(
                    request.id,
                    json!({ "job_id": job_id, "status": "failed", "error": e }),
                ),
            }
        }
        None => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "No executor configured — cannot run job",
        ),
    }
}

pub async fn handle_runs(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params"),
    };
    let job_id = match params.get("job_id").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id"),
    };
    let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;

    let service = cron.lock().await;
    match service.get_job_runs(job_id, limit).await {
        Ok(runs) => {
            let runs_json: Vec<Value> = runs.iter().map(|r| run_to_json(r)).collect();
            JsonRpcResponse::success(request.id, json!({ "job_id": job_id, "runs": runs_json }))
        }
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("{e}")),
    }
}

pub async fn handle_toggle(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params"),
    };
    let job_id = match params.get("job_id").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id"),
    };
    let enabled = params.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);

    let service = cron.lock().await;
    let result = if enabled {
        service.enable_job(job_id).await
    } else {
        service.disable_job(job_id).await
    };

    match result {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "job_id": job_id, "enabled": enabled })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("{e}")),
    }
}

// =============================================================================
// Stub handlers (fallback when CronService is not initialized)
// =============================================================================

pub async fn handle_list_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::success(request.id, json!({ "jobs": [] }))
}

pub async fn handle_get_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    let job_id = match extract_str(&request, "job_id") {
        Some(id) => id,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id"),
    };
    JsonRpcResponse::success(request.id, json!({ "job": { "id": job_id, "name": "", "schedule": "", "enabled": false } }))
}

pub async fn handle_create_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params"),
    };
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("unnamed");
    let id = uuid::Uuid::new_v4().to_string();
    JsonRpcResponse::success(request.id, json!({ "job": { "id": id, "name": name } }))
}

pub async fn handle_update_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    let job_id = match extract_str(&request, "job_id") {
        Some(id) => id,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id"),
    };
    JsonRpcResponse::success(request.id, json!({ "job": { "id": job_id, "updated": true } }))
}

pub async fn handle_delete_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    let job_id = match extract_str(&request, "job_id") {
        Some(id) => id,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id"),
    };
    JsonRpcResponse::success(request.id, json!({ "deleted": job_id }))
}

pub async fn handle_status_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::success(request.id, json!({ "running": false, "job_count": 0 }))
}

pub async fn handle_run_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    let job_id = match extract_str(&request, "job_id") {
        Some(id) => id,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id"),
    };
    JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("CronService not initialized — cannot run job {}", job_id))
}

pub async fn handle_runs_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    let job_id = match extract_str(&request, "job_id") {
        Some(id) => id,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id"),
    };
    JsonRpcResponse::success(request.id, json!({ "job_id": job_id, "runs": [] }))
}

pub async fn handle_toggle_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params"),
    };
    let job_id = match params.get("job_id").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id"),
    };
    let enabled = params.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
    JsonRpcResponse::success(request.id, json!({ "job_id": job_id, "enabled": enabled }))
}

// =============================================================================
// Helpers
// =============================================================================

fn extract_str(request: &JsonRpcRequest, key: &str) -> Option<String> {
    request
        .params
        .as_ref()
        .and_then(|v| v.as_object())
        .and_then(|map| map.get(key))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn job_to_json(job: &CronJob) -> Value {
    json!({
        "id": job.id,
        "name": job.name,
        "schedule": job.schedule,
        "agent_id": job.agent_id,
        "prompt": job.prompt,
        "enabled": job.enabled,
        "schedule_kind": format!("{:?}", job.schedule_kind),
        "created_at": job.created_at,
        "updated_at": job.updated_at,
        "next_run_at": job.next_run_at,
        "last_run_at": job.last_run_at,
        "consecutive_failures": job.consecutive_failures,
        "priority": job.priority,
    })
}

fn run_to_json(run: &crate::cron::JobRun) -> Value {
    json!({
        "id": run.id,
        "job_id": run.job_id,
        "status": format!("{:?}", run.status),
        "started_at": run.started_at,
        "ended_at": run.ended_at,
        "duration_ms": run.duration_ms,
        "error": run.error,
        "response": run.response,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_handle_list_stub() {
        let request = JsonRpcRequest::with_id("cron.list", None, json!(1));
        let response = handle_list_stub(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_get_stub() {
        let request = JsonRpcRequest::new(
            "cron.get",
            Some(json!({ "job_id": "daily-backup" })),
            Some(json!(1)),
        );
        let response = handle_get_stub(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_get_stub_missing_job_id() {
        let request = JsonRpcRequest::with_id("cron.get", None, json!(1));
        let response = handle_get_stub(request).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_create_stub() {
        let request = JsonRpcRequest::new(
            "cron.create",
            Some(json!({ "name": "daily-backup", "schedule": "0 0 * * *" })),
            Some(json!(1)),
        );
        let response = handle_create_stub(request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_create_stub_missing_params() {
        let request = JsonRpcRequest::with_id("cron.create", None, json!(1));
        let response = handle_create_stub(request).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_run_stub_returns_error() {
        let request = JsonRpcRequest::new(
            "cron.run",
            Some(json!({ "job_id": "test" })),
            Some(json!(1)),
        );
        let response = handle_run_stub(request).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_toggle_stub() {
        let request = JsonRpcRequest::new(
            "cron.toggle",
            Some(json!({ "job_id": "daily-backup", "enabled": false })),
            Some(json!(1)),
        );
        let response = handle_toggle_stub(request).await;
        assert!(response.is_success());
    }
}
```

**Step 2: Add `executor_ref()` method to CronService**

In `core/src/cron/mod.rs`, add this method to `CronService` impl block:

```rust
    /// Get a reference to the executor (for manual job triggering via RPC)
    pub fn executor_ref(&self) -> Option<&JobExecutor> {
        self.executor.as_ref()
    }
```

**Step 3: Update HandlerRegistry to use stub handlers**

In `core/src/gateway/handlers/mod.rs`, update the cron registration section (around lines 212-221) to use the `_stub` suffix:

```rust
        // Cron handlers (stubs — real handlers wired at startup with CronService)
        registry.register("cron.list", cron::handle_list_stub);
        registry.register("cron.get", cron::handle_get_stub);
        registry.register("cron.create", cron::handle_create_stub);
        registry.register("cron.update", cron::handle_update_stub);
        registry.register("cron.delete", cron::handle_delete_stub);
        registry.register("cron.status", cron::handle_status_stub);
        registry.register("cron.run", cron::handle_run_stub);
        registry.register("cron.runs", cron::handle_runs_stub);
        registry.register("cron.toggle", cron::handle_toggle_stub);
```

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib gateway::handlers::cron`
Expected: PASS (stub tests still pass)

**Step 5: Commit**

```bash
git add core/src/gateway/handlers/cron.rs core/src/gateway/handlers/mod.rs core/src/cron/mod.rs
git commit -m "gateway: replace cron handler stubs with real CronService-backed handlers"
```

---

### Task 4: Replace group_chat Gateway handler placeholders

**Files:**
- Modify: `core/src/gateway/handlers/group_chat.rs`

**Context:** Currently 6 placeholder handlers return INTERNAL_ERROR. We need real handlers that accept `Arc<Mutex<GroupChatOrchestrator>>` and `Arc<GroupChatExecutor>` as context. The placeholders remain in `HandlerRegistry::new()` as fallback.

**Step 1: Rewrite group_chat.rs**

Replace the entire file:

```rust
//! Group Chat RPC handlers.
//!
//! Placeholder handlers are registered in HandlerRegistry::new().
//! Real handlers are wired at startup with orchestrator + executor context.

use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::group_chat::{
    GroupChatExecutor, GroupChatOrchestrator, PersonaSource,
};

/// Shared orchestrator type alias
pub type SharedOrchestrator = Arc<Mutex<GroupChatOrchestrator>>;

// =============================================================================
// Real handlers (wired at startup)
// =============================================================================

pub async fn handle_start(
    request: JsonRpcRequest,
    orch: SharedOrchestrator,
    executor: Arc<GroupChatExecutor>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params"),
    };

    // Parse personas
    let personas_val = match params.get("personas") {
        Some(v) => v,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing personas"),
    };
    let personas: Vec<PersonaSource> = match serde_json::from_value(personas_val.clone()) {
        Ok(p) => p,
        Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, format!("Invalid personas: {e}")),
    };

    let topic = params.get("topic").and_then(|v| v.as_str()).map(|s| s.to_string());
    let initial_message = params.get("initial_message").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let source_channel = params.get("source_channel").and_then(|v| v.as_str()).unwrap_or("rpc").to_string();
    let source_session_key = params.get("source_session_key").and_then(|v| v.as_str()).unwrap_or("rpc:direct").to_string();

    let mut orch = orch.lock().await;
    let session_id = match orch.create_session(personas, topic, source_channel, source_session_key) {
        Ok(id) => id,
        Err(e) => return JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("{e}")),
    };

    // Execute initial round if message provided
    if !initial_message.is_empty() {
        let session = match orch.get_session_mut(&session_id) {
            Some(s) => s,
            None => return JsonRpcResponse::error(request.id, INTERNAL_ERROR, "Session created but not found"),
        };
        match executor.execute_round(session, &initial_message).await {
            Ok(messages) => {
                let msgs: Vec<Value> = messages.iter().map(|m| message_to_json(m)).collect();
                return JsonRpcResponse::success(request.id, json!({
                    "session_id": session_id,
                    "messages": msgs,
                }));
            }
            Err(e) => return JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("{e}")),
        }
    }

    JsonRpcResponse::success(request.id, json!({ "session_id": session_id }))
}

pub async fn handle_continue(
    request: JsonRpcRequest,
    orch: SharedOrchestrator,
    executor: Arc<GroupChatExecutor>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params"),
    };

    let session_id = match params.get("session_id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_id"),
    };
    let message = match params.get("message").and_then(|v| v.as_str()) {
        Some(m) => m.to_string(),
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing message"),
    };

    let mut orch = orch.lock().await;

    // Check round limit
    if let Err(e) = orch.check_round_limit(&session_id) {
        return JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("{e}"));
    }

    let session = match orch.get_session_mut(&session_id) {
        Some(s) => s,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, format!("Session not found: {session_id}")),
    };

    match executor.execute_round(session, &message).await {
        Ok(messages) => {
            let msgs: Vec<Value> = messages.iter().map(|m| message_to_json(m)).collect();
            JsonRpcResponse::success(request.id, json!({
                "session_id": session_id,
                "messages": msgs,
            }))
        }
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("{e}")),
    }
}

pub async fn handle_mention(
    request: JsonRpcRequest,
    orch: SharedOrchestrator,
    executor: Arc<GroupChatExecutor>,
) -> JsonRpcResponse {
    // Mention is essentially continue — the coordinator decides who responds
    // based on the message content. Targets are informational.
    handle_continue(request, orch, executor).await
}

pub async fn handle_end(request: JsonRpcRequest, orch: SharedOrchestrator) -> JsonRpcResponse {
    let session_id = match extract_str(&request, "session_id") {
        Some(id) => id,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_id"),
    };

    let mut orch = orch.lock().await;
    match orch.end_session(&session_id) {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "ended": session_id })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("{e}")),
    }
}

pub async fn handle_list(request: JsonRpcRequest, orch: SharedOrchestrator) -> JsonRpcResponse {
    let orch = orch.lock().await;
    let sessions: Vec<Value> = orch
        .list_active_sessions()
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "topic": s.topic,
                "participants": s.participants.iter().map(|p| json!({ "id": p.id, "name": p.name })).collect::<Vec<_>>(),
                "current_round": s.current_round,
                "status": s.status.as_str(),
                "created_at": s.created_at,
            })
        })
        .collect();

    JsonRpcResponse::success(request.id, json!({ "sessions": sessions }))
}

pub async fn handle_history(request: JsonRpcRequest, orch: SharedOrchestrator) -> JsonRpcResponse {
    let session_id = match extract_str(&request, "session_id") {
        Some(id) => id,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_id"),
    };

    let orch = orch.lock().await;
    let session = match orch.get_session(&session_id) {
        Some(s) => s,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, format!("Session not found: {session_id}")),
    };

    let turns: Vec<Value> = session
        .history
        .iter()
        .map(|t| {
            json!({
                "round": t.round,
                "speaker": t.speaker.name(),
                "content": t.content,
                "timestamp": t.timestamp,
            })
        })
        .collect();

    JsonRpcResponse::success(
        request.id,
        json!({
            "session_id": session_id,
            "history": turns,
            "current_round": session.current_round,
        }),
    )
}

// =============================================================================
// Placeholder handlers (registered in HandlerRegistry::new())
// =============================================================================

const RUNTIME_REQUIRED: &str = "requires GroupChatOrchestrator runtime - wire Gateway first";

pub async fn handle_start_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("group_chat.start {}", RUNTIME_REQUIRED))
}

pub async fn handle_continue_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("group_chat.continue {}", RUNTIME_REQUIRED))
}

pub async fn handle_mention_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("group_chat.mention {}", RUNTIME_REQUIRED))
}

pub async fn handle_end_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("group_chat.end {}", RUNTIME_REQUIRED))
}

pub async fn handle_list_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("group_chat.list {}", RUNTIME_REQUIRED))
}

pub async fn handle_history_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("group_chat.history {}", RUNTIME_REQUIRED))
}

// =============================================================================
// Helpers
// =============================================================================

fn extract_str(request: &JsonRpcRequest, key: &str) -> Option<String> {
    request
        .params
        .as_ref()
        .and_then(|v| v.as_object())
        .and_then(|map| map.get(key))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn message_to_json(msg: &crate::group_chat::GroupChatMessage) -> Value {
    json!({
        "session_id": msg.session_id,
        "speaker": msg.speaker.name(),
        "content": msg.content,
        "round": msg.round,
        "sequence": msg.sequence,
        "is_final": msg.is_final,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use crate::gateway::protocol::JsonRpcRequest;

    #[test]
    fn test_group_chat_handlers_registered() {
        let registry = crate::gateway::handlers::HandlerRegistry::new();
        assert!(registry.has_method("group_chat.start"));
        assert!(registry.has_method("group_chat.continue"));
        assert!(registry.has_method("group_chat.mention"));
        assert!(registry.has_method("group_chat.end"));
        assert!(registry.has_method("group_chat.list"));
        assert!(registry.has_method("group_chat.history"));
    }

    #[tokio::test]
    async fn test_start_placeholder_returns_error() {
        let registry = crate::gateway::handlers::HandlerRegistry::new();
        let req = JsonRpcRequest::with_id("group_chat.start", Some(json!({})), json!(1));
        let resp = registry.handle(&req).await;
        assert!(resp.is_error());
    }
}
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --lib gateway::handlers::group_chat`
Expected: 2 tests PASS

**Step 3: Commit**

```bash
git add core/src/gateway/handlers/group_chat.rs
git commit -m "gateway: replace group_chat placeholder handlers with real orchestrator-backed handlers"
```

---

### Task 5: Wire CronService + GroupChatOrchestrator at startup

**Files:**
- Modify: `core/src/bin/aleph/commands/start/builder/handlers.rs`
- Modify: `core/src/bin/aleph/commands/start/mod.rs`

**Context:** At startup in `start_server()`, we need to:
1. Create `CronService` from config, set executor, wrap in `Arc<Mutex<>>`
2. Create `GroupChatOrchestrator` from config, wrap in `Arc<Mutex<>>`
3. Create `GroupChatExecutor` with a provider, wrap in `Arc<>`
4. Register both sets of handlers via `register_handler!` macro

**Step 1: Add register functions to handlers.rs**

In `core/src/bin/aleph/commands/start/builder/handlers.rs`, add imports:

```rust
use alephcore::gateway::handlers::cron as cron_handlers;
use alephcore::gateway::handlers::cron::SharedCronService;
use alephcore::gateway::handlers::group_chat as group_chat_handlers;
use alephcore::gateway::handlers::group_chat::SharedOrchestrator;
use alephcore::group_chat::GroupChatExecutor;
```

Then add two new registration functions at the bottom of the file:

```rust
// ─── register_cron_handlers ─────────────────────────────────────────────────

pub(in crate::commands::start) fn register_cron_handlers(
    server: &mut GatewayServer,
    cron_service: &SharedCronService,
    daemon: bool,
) {
    register_handler!(server, "cron.list", cron_handlers::handle_list, cron_service);
    register_handler!(server, "cron.get", cron_handlers::handle_get, cron_service);
    register_handler!(server, "cron.create", cron_handlers::handle_create, cron_service);
    register_handler!(server, "cron.update", cron_handlers::handle_update, cron_service);
    register_handler!(server, "cron.delete", cron_handlers::handle_delete, cron_service);
    register_handler!(server, "cron.status", cron_handlers::handle_status, cron_service);
    register_handler!(server, "cron.run", cron_handlers::handle_run, cron_service);
    register_handler!(server, "cron.runs", cron_handlers::handle_runs, cron_service);
    register_handler!(server, "cron.toggle", cron_handlers::handle_toggle, cron_service);

    if !daemon {
        println!("Cron methods:");
        println!("  - cron.list   : List all cron jobs");
        println!("  - cron.get    : Get cron job details");
        println!("  - cron.create : Create a new cron job");
        println!("  - cron.update : Update an existing cron job");
        println!("  - cron.delete : Delete a cron job");
        println!("  - cron.status : Get cron service status");
        println!("  - cron.run    : Manually trigger a cron job");
        println!("  - cron.runs   : Get cron job execution history");
        println!("  - cron.toggle : Enable or disable a cron job");
        println!();
    }
}

// ─── register_group_chat_handlers ───────────────────────────────────────────

pub(in crate::commands::start) fn register_group_chat_handlers(
    server: &mut GatewayServer,
    orch: &SharedOrchestrator,
    executor: &Arc<GroupChatExecutor>,
    daemon: bool,
) {
    // start, continue, mention need both orchestrator + executor (2 context args)
    register_handler!(server, "group_chat.start", group_chat_handlers::handle_start, orch, executor);
    register_handler!(server, "group_chat.continue", group_chat_handlers::handle_continue, orch, executor);
    register_handler!(server, "group_chat.mention", group_chat_handlers::handle_mention, orch, executor);

    // end, list, history only need orchestrator (1 context arg)
    register_handler!(server, "group_chat.end", group_chat_handlers::handle_end, orch);
    register_handler!(server, "group_chat.list", group_chat_handlers::handle_list, orch);
    register_handler!(server, "group_chat.history", group_chat_handlers::handle_history, orch);

    if !daemon {
        println!("Group Chat methods:");
        println!("  - group_chat.start    : Start a new group chat session");
        println!("  - group_chat.continue : Continue discussion with new message");
        println!("  - group_chat.mention  : Mention specific personas");
        println!("  - group_chat.end      : End a group chat session");
        println!("  - group_chat.list     : List active sessions");
        println!("  - group_chat.history  : Get session conversation history");
        println!();
    }
}
```

**Step 2: Wire services at startup in mod.rs**

In `core/src/bin/aleph/commands/start/mod.rs`, add imports near the top (after existing imports):

```rust
use alephcore::cron::{CronConfig, CronService};
use alephcore::group_chat::{GroupChatExecutor, GroupChatOrchestrator};
```

Then in `start_server()`, add the following initialization code **after** the identity resolver registration (after line ~1158) and **before** the channel registry initialization (before line ~1218):

```rust
    // Initialize CronService
    {
        let app_cfg = app_config_for_channels.read().await;
        let cron_config = app_cfg.cron.clone();
        drop(app_cfg);

        match CronService::new(cron_config) {
            Ok(mut cron_service) => {
                // Wire JobExecutor using the provider registry (if available)
                if can_create_provider_from_env() {
                    if let Ok(provider_reg) = create_provider_registry_from_env() {
                        let provider = provider_reg.default_provider();
                        let executor: alephcore::cron::JobExecutor = Arc::new(move |_job_id, _agent_id, prompt| {
                            let provider = provider.clone();
                            Box::pin(async move {
                                provider.process(&prompt, None).await.map_err(|e| format!("{e}"))
                            })
                        });
                        cron_service.set_executor(executor);
                    }
                }

                let shared_cron: alephcore::gateway::handlers::cron::SharedCronService =
                    Arc::new(tokio::sync::Mutex::new(cron_service));
                register_cron_handlers(&mut server, &shared_cron, args.daemon);
            }
            Err(e) => {
                if !args.daemon {
                    eprintln!("Warning: Failed to initialize CronService: {}. Cron stubs remain.", e);
                }
            }
        }
    }

    // Initialize GroupChat Orchestrator + Executor
    {
        let app_cfg = app_config_for_channels.read().await;
        let gc_config = app_cfg.group_chat.clone();
        let persona_configs = app_cfg.personas.clone();
        drop(app_cfg);

        let orchestrator = GroupChatOrchestrator::new(gc_config, &persona_configs);
        let shared_orch: alephcore::gateway::handlers::group_chat::SharedOrchestrator =
            Arc::new(tokio::sync::Mutex::new(orchestrator));

        // Create executor with default provider (if available)
        let gc_executor = if can_create_provider_from_env() {
            create_provider_registry_from_env()
                .ok()
                .map(|reg| Arc::new(GroupChatExecutor::new(reg.default_provider())))
        } else {
            None
        };

        if let Some(executor) = gc_executor {
            register_group_chat_handlers(&mut server, &shared_orch, &executor, args.daemon);
        } else if !args.daemon {
            println!("Group Chat: Disabled (requires ANTHROPIC_API_KEY or OPENAI_API_KEY)");
            println!();
        }
    }
```

**Step 3: Run cargo check**

Run: `cargo check -p aleph-server`
Expected: PASS (or whatever the server binary package is named)

Note: The server binary crate name may be `aleph-server` or just the binary in `core/`. Check `Cargo.toml` for the exact name. If the crate is `alephcore`, use `cargo check -p alephcore`. If the binary is separate, find its package name.

**Step 4: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All existing tests PASS

**Step 5: Commit**

```bash
git add core/src/bin/aleph/commands/start/builder/handlers.rs core/src/bin/aleph/commands/start/mod.rs
git commit -m "gateway: wire CronService and GroupChatOrchestrator at startup"
```

---

### Task 6: Add lib.rs re-exports for GroupChatExecutor

**Files:**
- Modify: `core/src/lib.rs` (if needed)

**Context:** Check if `GroupChatExecutor` needs to be added to `lib.rs` re-exports. The existing `pub use crate::group_chat::...` block should include it.

**Step 1: Verify and add re-export**

In `core/src/lib.rs`, find the `group_chat` re-export block and add `GroupChatExecutor`:

```rust
pub use crate::group_chat::{
    GroupChatOrchestrator, GroupChatSession, PersonaRegistry,
    GroupChatCommandParser, GroupChatRenderer,
    GroupChatError, GroupChatMessage, GroupChatRequest, GroupChatStatus,
    Persona, PersonaSource, Speaker, RenderedContent, ContentFormat,
    CoordinatorPlan, RespondentPlan,
    GroupChatExecutor,  // ADD THIS
};
```

**Step 2: Run cargo check**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 3: Commit**

```bash
git add core/src/lib.rs
git commit -m "lib: re-export GroupChatExecutor"
```

---

### Task 7: Final verification

**Step 1: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests PASS (including new executor tests and updated cron/group_chat handler tests)

**Step 2: Run cargo check on server binary**

Run: `cargo check` (workspace root)
Expected: PASS (may fail on Tauri crate — use `cargo check -p alephcore` if so)

**Step 3: Verify handler count**

The cron handlers (9) should now be wired to real CronService.
The group_chat handlers (6) should now be wired to real Orchestrator + Executor.
Total: 15 handlers upgraded from stubs to real implementations.
