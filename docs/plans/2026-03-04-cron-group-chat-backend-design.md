# Cron & Group Chat Backend Implementation Design

> Date: 2026-03-04
> Status: Approved
> Scope: Wire CronService + implement GroupChatExecutor + replace all Gateway stubs

## Context

Both `cron.*` and `group_chat.*` have complete domain models and Gateway handler stubs, but lack:
1. **Cron**: JobExecutor implementation + Gateway handler wiring to real CronService
2. **Group Chat**: LLM execution layer (GroupChatExecutor) + Gateway handler wiring to real Orchestrator

## Approach: Direct AiProvider Calls

Both systems use `AiProvider::process()` for LLM calls. No agent loop needed — cron jobs are simple prompt→response, group chat is coordinator→persona sequential calls.

## Cron Backend

### What Exists

- `CronService` (1342 lines): Full SQLite persistence, scheduling, job CRUD, history, timeout handling
- `CronConfig`: `enabled`, `db_path`, `check_interval_secs`, `max_concurrent_jobs`, `job_timeout_secs`, `history_retention_days`
- `CronJob`: Complete model with scheduling (Cron/Every/At), chaining, templates, delivery
- Gateway stubs: 9 handlers (`list`, `get`, `create`, `update`, `delete`, `status`, `run`, `runs`, `toggle`) returning fake data

### What's Missing

1. **CronConfig not in FullConfig** — needs `[cron]` section in aleph.toml
2. **JobExecutor never set** — `set_executor()` exists but never called
3. **Gateway stubs** — all 9 return hardcoded fake data

### Implementation

#### JobExecutor

```rust
let provider_registry = Arc::clone(&provider_registry);
let executor: JobExecutor = Arc::new(move |_job_id, _agent_id, prompt| {
    let registry = Arc::clone(&provider_registry);
    Box::pin(async move {
        let provider = registry.get_default()
            .ok_or_else(|| "No AI provider configured".to_string())?;
        provider.process(&prompt, None).await
            .map_err(|e| format!("LLM error: {e}"))
    })
});
```

#### Gateway Handler Wiring

Replace 9 stubs. Context: `Arc<Mutex<CronService>>`.

| RPC Method | Handler | CronService Method |
|------------|---------|-------------------|
| `cron.list` | `handle_list` | `list_jobs()` |
| `cron.get` | `handle_get` | `get_job(job_id)` |
| `cron.create` | `handle_create` | `add_job(job)` |
| `cron.update` | `handle_update` | `update_job(job)` |
| `cron.delete` | `handle_delete` | `delete_job(job_id)` |
| `cron.status` | `handle_status` | service metadata (job count, running status) |
| `cron.run` | `handle_run` | `get_job(id)` + call executor directly |
| `cron.runs` | `handle_runs` | `get_job_runs(job_id, limit)` |
| `cron.toggle` | `handle_toggle` | `enable_job(id)` / `disable_job(id)` |

Note: `cron.run` (manual trigger) = get job + call executor, since CronService has no `trigger_job()` method.

#### Config Integration

Add to `FullConfig`:
```rust
#[serde(default)]
pub cron: CronConfig,
```

## Group Chat Backend

### What Exists

- `GroupChatOrchestrator`: Session lifecycle (create/get/end/list)
- `coordinator.rs`: Pure functions — `build_coordinator_prompt()`, `parse_coordinator_plan()`, `build_fallback_plan()`, `build_persona_prompt()`
- `protocol.rs`: Full type system (Persona, Speaker, GroupChatRequest, CoordinatorPlan, GroupChatMessage, GroupChatError)
- `GroupChatSession`: History management with `add_turn()`, `build_history_text()`
- `GroupChatConfig`: Already in FullConfig (`max_personas_per_session`, `max_rounds`, `coordinator_visible`, `default_coordinator_model`)
- Gateway stubs: 6 placeholder handlers returning RUNTIME_REQUIRED error

### What's Missing

1. **GroupChatExecutor** — LLM execution layer: coordinator → persona loop
2. **Gateway stubs** — all 6 return error

### GroupChatExecutor Design

New file: `core/src/group_chat/executor.rs`

```rust
pub struct GroupChatExecutor {
    provider: Arc<dyn AiProvider>,
}

impl GroupChatExecutor {
    pub fn new(provider: Arc<dyn AiProvider>) -> Self;

    /// Execute one round of group chat
    pub async fn execute_round(
        &self,
        session: &mut GroupChatSession,
        user_message: &str,
    ) -> Result<Vec<GroupChatMessage>, GroupChatError> {
        // 1. Build coordinator prompt from session state
        let history = session.build_history_text();
        let coord_prompt = build_coordinator_prompt(
            &session.participants, user_message, &history, &session.topic
        );

        // 2. Call LLM for coordinator plan
        let coord_response = self.provider.process(&coord_prompt, None).await
            .map_err(|e| GroupChatError::ProviderUnavailable(e.to_string()))?;
        let plan = parse_coordinator_plan(&coord_response)
            .unwrap_or_else(|_| build_fallback_plan(&session.participants));

        // 3. Execute each persona in order
        let round = session.current_round + 1;
        let mut messages = Vec::new();
        let mut prior_discussion = String::new();

        for respondent in &plan.respondents {
            let persona = session.participants.iter()
                .find(|p| p.id == respondent.persona_id)
                .ok_or_else(|| GroupChatError::PersonaNotFound(respondent.persona_id.clone()))?;

            let persona_prompt = build_persona_prompt(
                persona, user_message, &prior_discussion, &respondent.guidance
            );

            let response = self.provider.process(&persona_prompt, Some(&persona.system_prompt)).await
                .map_err(|e| GroupChatError::PersonaInvocationFailed {
                    persona_id: persona.id.clone(),
                    reason: e.to_string(),
                })?;

            // Record turn
            session.add_turn(round, Speaker::Persona {
                id: persona.id.clone(),
                name: persona.name.clone(),
            }, response.clone());

            // Build message
            let msg = GroupChatMessage {
                session_id: session.id.clone(),
                speaker: Speaker::Persona { id: persona.id.clone(), name: persona.name.clone() },
                content: response.clone(),
                round,
                sequence: messages.len() as u32 + 1,
                is_final: false,
            };
            messages.push(msg);

            // Accumulate for next persona's context
            prior_discussion.push_str(&format!("[{}]: {}\n\n", persona.name, response));
        }

        Ok(messages)
    }
}
```

### Gateway Handler Wiring

Replace 6 placeholders. Context: `Arc<Mutex<GroupChatOrchestrator>>` + `Arc<GroupChatExecutor>`.

| RPC Method | Handler | Implementation |
|------------|---------|---------------|
| `group_chat.start` | `handle_start` | `orchestrator.create_session()` + `executor.execute_round()` with initial message |
| `group_chat.continue` | `handle_continue` | `orchestrator.get_session_mut()` + `orchestrator.check_round_limit()` + `executor.execute_round()` |
| `group_chat.mention` | `handle_mention` | Same as continue but with targeted personas |
| `group_chat.end` | `handle_end` | `orchestrator.end_session()` |
| `group_chat.list` | `handle_list` | `orchestrator.list_active_sessions()` |
| `group_chat.history` | `handle_history` | `orchestrator.get_session()` → `session.history` |

## File Changes

### New Files

```
core/src/group_chat/executor.rs          # GroupChatExecutor
```

### Modified Files

```
core/src/config/types/mod.rs             # pub use cron config (if not already)
core/src/config/structs.rs               # Add CronConfig to FullConfig
core/src/group_chat/mod.rs               # pub mod executor
core/src/gateway/handlers/cron.rs        # Replace 9 stubs with real implementations
core/src/gateway/handlers/group_chat.rs  # Replace 6 placeholders with real implementations
core/src/bin/aleph/commands/start/builder/handlers.rs  # register_cron_handlers + register_group_chat_handlers
core/src/bin/aleph/commands/start/mod.rs # Create CronService, GroupChatOrchestrator, GroupChatExecutor at startup
```

## Tasks

1. Add CronConfig to FullConfig
2. Implement GroupChatExecutor
3. Replace cron Gateway stubs (9 handlers)
4. Replace group_chat Gateway stubs (6 handlers)
5. Wire CronService + GroupChatOrchestrator at startup
6. Register all handlers
