# Memory Governance Surfacing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface two invisible memory subsystems to the panel via read-only JSON-RPC + Settings ▸ Memory views: dream insights (daily digest / synthesis / run history) and the correction→distillation lifecycle.

**Architecture:** Two new read-only handlers reuse existing store read APIs (only `recent_daily_insights` is net-new). Both register as db-only handlers via the `register_handler!` macro in `register_memory_handlers`. The panel gains two read-only components in Settings ▸ Memory (the existing memory diagnostics hub), consuming the RPCs through the established `DashboardState::rpc_call` client. No write paths are touched — correction/distillation stays LLM/tool-driven (R7/R8/R10).

**Tech Stack:** Rust (tokio, serde, rusqlite, async-trait), JSON-RPC gateway, Leptos 0.8/WASM panel (`aleph-panel` crate), i18n via `leptos-i18n` (`locales/en.json` + `locales/zh.json`).

## Global Constraints

- Branch: `memory-governance-surfacing`, off main `751b9c7d3`. All work in the worktree, never touch main directly.
- `cargo check` ALLOWED (#3 special dispensation). Backend: `cargo check -p alephcore --lib`. Frontend: `cargo check -p aleph-panel --target wasm32-unknown-unknown`. Minimize invocations — one per task at the verify step.
- Pre-existing unrelated `tests/worktree_isolation.rs` E0063 breaks `cargo test -p alephcore` without `--lib`; ALWAYS use `--lib` filter for backend tests.
- RPC naming: snake_case — `dreaming.list_insights`, `memory.list_corrections`.
- Default agent id: `crate::routing::DEFAULT_AGENT_ID`. Default limits: insights=30, corrections=50.
- Read-only: handlers call NO write/delete store APIs.
- Error responses use `INTERNAL_ERROR` code; never leak internal paths in messages (use the existing `format!("... failed: {err}")` style that the siblings use).
- Commit message format: `<scope>: <description>` in English.
- Lock handling: `.lock().unwrap_or_else(|e| e.into_inner())` (existing sqlite style) — but new code reuses existing methods that already do this; do not add new lock sites.

---

### Task 1: `recent_daily_insights` store method

**Files:**
- Modify: `src/memory/store/mod.rs` (DreamStore trait, after `get_daily_insight` at line 48)
- Modify: `src/memory/store/sqlite/sessions.rs` (impl block for `DreamStore`, after `get_daily_insight` ~line 113)
- Test: `src/memory/store/sqlite/sessions.rs` (add `#[cfg(test)] mod` if none exists, else extend)

**Interfaces:**
- Consumes: `DailyInsight { date: String, content: String, source_memory_count: u32, created_at: i64 }` (from `src/memory/dreaming/mod.rs`); `daily_insights` table `(date PK, content, source_memory_count, created_at)`.
- Produces: `DreamStore::recent_daily_insights(&self, limit: usize) -> Result<Vec<DailyInsight>, AlephError>` — ordered by `date DESC`, capped at `limit`.

- [ ] **Step 1: Write the failing test**

Add to `src/memory/store/sqlite/sessions.rs` (create the test module if absent):

```rust
#[cfg(test)]
mod daily_insight_tests {
    use super::*;
    use crate::memory::dreaming::DailyInsight;
    use crate::memory::store::DreamStore;
    use crate::memory::store::sqlite::SqliteMemoryBackend;

    #[tokio::test]
    async fn recent_daily_insights_orders_desc_and_limits() {
        let backend = SqliteMemoryBackend::in_memory().expect("in-memory backend");
        for date in ["2026-06-18", "2026-06-19", "2026-06-20"] {
            backend
                .upsert_daily_insight(DailyInsight::new(
                    date.to_string(),
                    format!("digest for {date}"),
                    3,
                ))
                .await
                .unwrap();
        }

        let recent = backend.recent_daily_insights(2).await.unwrap();
        assert_eq!(recent.len(), 2, "limit honored");
        assert_eq!(recent[0].date, "2026-06-20", "newest first");
        assert_eq!(recent[1].date, "2026-06-19");
    }

    #[tokio::test]
    async fn recent_daily_insights_empty_ok() {
        let backend = SqliteMemoryBackend::in_memory().expect("in-memory backend");
        let recent = backend.recent_daily_insights(10).await.unwrap();
        assert!(recent.is_empty());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib daily_insight_tests`
Expected: FAIL — `no method named recent_daily_insights`.

- [ ] **Step 3: Add the trait method**

In `src/memory/store/mod.rs`, inside `pub trait DreamStore`, after the `get_daily_insight` declaration (line 48):

```rust
    /// List the most recent daily insights, ordered by date descending,
    /// capped at `limit`. Used by the `dreaming.list_insights` RPC.
    async fn recent_daily_insights(&self, limit: usize) -> Result<Vec<DailyInsight>, AlephError>;
```

- [ ] **Step 4: Implement on SqliteMemoryBackend**

In `src/memory/store/sqlite/sessions.rs`, inside the `impl DreamStore for SqliteMemoryBackend` block, after `get_daily_insight` (~line 113):

```rust
    async fn recent_daily_insights(&self, limit: usize) -> Result<Vec<DailyInsight>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                "SELECT date, content, source_memory_count, created_at \
                 FROM daily_insights ORDER BY date DESC LIMIT ?1",
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare recent_daily_insights: {e}")))?;

        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok(DailyInsight {
                    date: row.get(0)?,
                    content: row.get(1)?,
                    source_memory_count: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })
            .map_err(|e| AlephError::config(format!("recent_daily_insights query: {e}")))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| AlephError::config(format!("recent_daily_insights row: {e}")))?);
        }
        Ok(out)
    }
```

> NOTE: If any other `impl DreamStore` exists (e.g. an in-memory/mock test store), add a minimal impl there too so the workspace compiles. Search: `grep -rn "impl DreamStore" src/`. The known production impl is in `sessions.rs`. If a mock exists, mirror the get_daily_insight stub style.

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p alephcore --lib daily_insight_tests`
Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
git add src/memory/store/mod.rs src/memory/store/sqlite/sessions.rs
git commit -m "memory: add recent_daily_insights to DreamStore"
```

---

### Task 2: `dreaming.list_insights` handler

**Files:**
- Modify: `src/gateway/handlers/dreaming.rs` (add handler + tests)
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/memory.rs` (register in `register_memory_handlers`)
- Test: `src/gateway/handlers/dreaming.rs` (extend `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `DreamStore::recent_daily_insights(limit)` (Task 1); `NoteStore::list_notes(&self, agent_id) -> Vec<NoteIndexEntry>` where `NoteIndexEntry { path, filename, agent_id, category, tags: Vec<String>, link_count, created_at, updated_at, content_hash }`; `SqliteMemoryBackend::recent_dream_reports(limit) -> Vec<PersistedDreamReport>` where `PersistedDreamReport { id, pipeline_type, started_at, finished_at, duration_ms: i64, synthesis_count: u32, errors: Option<String>, namespace }`; `MemoryBackend = Arc<SqliteMemoryBackend>`.
- Produces: `pub async fn handle_list_insights(request: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse`. Response: `{ "daily": [{date,content,source_memory_count,created_at}], "synthesis": [{path,title,tags,updated_at}], "runs": [{id,pipeline_type,started_at,finished_at,duration_ms,synthesis_count,errors}] }`.

- [ ] **Step 1: Write the failing test**

Extend the `#[cfg(test)] mod tests` in `src/gateway/handlers/dreaming.rs`:

```rust
    use crate::memory::dreaming::DailyInsight;
    use crate::memory::store::DreamStore;
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;

    #[tokio::test]
    async fn list_insights_returns_daily_and_runs() {
        let backend = SqliteMemoryBackend::in_memory().expect("in-memory backend");
        backend
            .upsert_daily_insight(DailyInsight::new(
                "2026-06-20".to_string(),
                "today digest".to_string(),
                4,
            ))
            .await
            .unwrap();
        let db: crate::memory::store::MemoryBackend = Arc::new(backend);

        let req = JsonRpcRequest::with_id("dreaming.list_insights", None, json!(1));
        let resp = handle_list_insights(req, db).await;

        assert!(resp.is_success(), "expected success: {:?}", resp.error);
        let v = resp.result.expect("result payload");
        let daily = v.get("daily").and_then(|d| d.as_array()).expect("daily array");
        assert_eq!(daily.len(), 1);
        assert_eq!(daily[0]["date"], "2026-06-20");
        assert_eq!(daily[0]["source_memory_count"], 4);
        assert!(v.get("synthesis").unwrap().is_array());
        assert!(v.get("runs").unwrap().is_array());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib handlers::dreaming::tests::list_insights`
Expected: FAIL — `cannot find function handle_list_insights`.

- [ ] **Step 3: Implement the handler**

In `src/gateway/handlers/dreaming.rs`, add (keep the existing `use serde_json::json;` and protocol import; add `MemoryBackend` import at top):

```rust
use crate::memory::store::MemoryBackend;
```

Then the handler:

```rust
/// Read-only listing of dream insights: recent daily digests, synthesis
/// notes, and dream-run history. Surfaced to the panel's Settings ▸ Memory
/// governance view. Pure I/O over existing store read APIs.
pub async fn handle_list_insights(request: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    use crate::memory::notes::store::NoteStore;
    use crate::memory::store::DreamStore;

    #[derive(serde::Deserialize, Default)]
    struct Params {
        agent_id: Option<String>,
        limit: Option<usize>,
    }
    let params: Params = request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();

    let agent_id = params
        .agent_id
        .as_deref()
        .unwrap_or(crate::routing::DEFAULT_AGENT_ID);
    let limit = params.limit.filter(|n| *n > 0).unwrap_or(30);

    // 1. Recent daily digests.
    let daily = match db.recent_daily_insights(limit).await {
        Ok(rows) => rows
            .into_iter()
            .map(|d| {
                json!({
                    "date": d.date,
                    "content": d.content,
                    "source_memory_count": d.source_memory_count,
                    "created_at": d.created_at,
                })
            })
            .collect::<Vec<_>>(),
        Err(err) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("dreaming.list_insights daily failed: {err}"),
            );
        }
    };

    // 2. Weekly synthesis notes (category == "synthesis").
    let synthesis = match db.list_notes(agent_id).await {
        Ok(notes) => notes
            .into_iter()
            .filter(|n| n.category == "synthesis")
            .take(limit)
            .map(|n| {
                json!({
                    "path": n.path,
                    "title": n.filename,
                    "tags": n.tags,
                    "updated_at": n.updated_at,
                })
            })
            .collect::<Vec<_>>(),
        Err(err) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("dreaming.list_insights synthesis failed: {err}"),
            );
        }
    };

    // 3. Dream-run audit trail.
    let runs = match db.recent_dream_reports(limit) {
        Ok(reports) => reports
            .into_iter()
            .map(|r| {
                json!({
                    "id": r.id,
                    "pipeline_type": r.pipeline_type,
                    "started_at": r.started_at,
                    "finished_at": r.finished_at,
                    "duration_ms": r.duration_ms,
                    "synthesis_count": r.synthesis_count,
                    "errors": r.errors,
                })
            })
            .collect::<Vec<_>>(),
        Err(err) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("dreaming.list_insights runs failed: {err}"),
            );
        }
    };

    JsonRpcResponse::success(
        request.id,
        json!({ "daily": daily, "synthesis": synthesis, "runs": runs }),
    )
}
```

- [ ] **Step 4: Register the handler**

In `src/bin/aleph-server/commands/start/builder/handlers/memory.rs`, inside `register_memory_handlers`, next to the `insights.tools` registration (after it), add:

```rust
    // Read-only dream insights listing (daily digests + synthesis + run history).
    register_handler!(
        server,
        "dreaming.list_insights",
        alephcore::gateway::handlers::dreaming::handle_list_insights,
        memory_db
    );
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p alephcore --lib handlers::dreaming::tests::list_insights`
Expected: PASS.

- [ ] **Step 6: Verify the binary compiles (registration site)**

Run: `cargo check --bin aleph-server`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add src/gateway/handlers/dreaming.rs src/bin/aleph-server/commands/start/builder/handlers/memory.rs
git commit -m "gateway: add dreaming.list_insights read-only handler"
```

---

### Task 3: `memory.list_corrections` handler

**Files:**
- Modify: `src/gateway/handlers/memory.rs` (add handler + tests)
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/memory.rs` (register)
- Test: `src/gateway/handlers/memory.rs` (`#[cfg(test)] mod`)

**Interfaces:**
- Consumes: `RawMemoryStore::get_raw_by_path_prefix(path_prefix: &str, agent_id: &str, limit: usize) -> Vec<RawMemory>` where `RawMemory { id, content, source: RawMemorySource, agent_id, session_id, path, layer, attachment_text, is_processed: bool, created_at }`; `RawMemorySource::Correction { severity: String, suggested_rule: Option<String> }`; correction path prefix is `"aleph://correction/"`. Seed in tests via `RawMemory::new(content, RawMemorySource::Correction{..}).with_agent(id).with_path("aleph://correction/<uuid>")` + `db.insert_raw_memory(&raw).await`.
- Produces: `pub async fn handle_list_corrections(request: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse`. Response: `{ "corrections": [{id, content, severity, suggested_rule, status, created_at}] }`, `status` ∈ `{"pending","distilled"}` (= `is_processed`).

- [ ] **Step 1: Write the failing test**

Add to `src/gateway/handlers/memory.rs` test module (create if absent):

```rust
#[cfg(test)]
mod list_corrections_tests {
    use super::*;
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;
    use serde_json::json;

    async fn seed(db: &SqliteMemoryBackend, id_suffix: &str, processed: bool, sev: &str) {
        let mut raw = RawMemory::new(
            format!("correction {id_suffix}"),
            RawMemorySource::Correction {
                severity: sev.to_string(),
                suggested_rule: Some(format!("rule {id_suffix}")),
            },
        )
        .with_agent("main")
        .with_path(format!("aleph://correction/{id_suffix}"));
        raw.is_processed = processed;
        db.insert_raw_memory(&raw).await.unwrap();
    }

    #[tokio::test]
    async fn maps_status_severity_and_rule() {
        let backend = SqliteMemoryBackend::in_memory().unwrap();
        seed(&backend, "c1", false, "high").await;
        seed(&backend, "c2", true, "low").await;
        let db: crate::memory::store::MemoryBackend = Arc::new(backend);

        let req = JsonRpcRequest::with_id(
            "memory.list_corrections",
            Some(json!({ "agent_id": "main" })),
            json!(1),
        );
        let resp = handle_list_corrections(req, db).await;
        assert!(resp.is_success(), "{:?}", resp.error);
        let items = resp.result.unwrap()["corrections"].as_array().unwrap().clone();
        assert_eq!(items.len(), 2);
        // Each entry carries status mapped from is_processed.
        let statuses: Vec<&str> = items.iter().map(|i| i["status"].as_str().unwrap()).collect();
        assert!(statuses.contains(&"pending"));
        assert!(statuses.contains(&"distilled"));
        let c1 = items.iter().find(|i| i["status"] == "pending").unwrap();
        assert_eq!(c1["severity"], "high");
        assert_eq!(c1["suggested_rule"], "rule c1");
    }

    #[tokio::test]
    async fn include_distilled_false_filters_processed() {
        let backend = SqliteMemoryBackend::in_memory().unwrap();
        seed(&backend, "c1", false, "high").await;
        seed(&backend, "c2", true, "low").await;
        let db: crate::memory::store::MemoryBackend = Arc::new(backend);

        let req = JsonRpcRequest::with_id(
            "memory.list_corrections",
            Some(json!({ "agent_id": "main", "include_distilled": false })),
            json!(1),
        );
        let resp = handle_list_corrections(req, db).await;
        let items = resp.result.unwrap()["corrections"].as_array().unwrap().clone();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["status"], "pending");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib list_corrections_tests`
Expected: FAIL — `cannot find function handle_list_corrections`.

- [ ] **Step 3: Implement the handler**

In `src/gateway/handlers/memory.rs`, add (the file already imports `JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR`, `json`, `MemoryBackend`):

```rust
/// Read-only listing of user corrections (raw `flag_user_correction` rows)
/// and their distillation status. Surfaces the correction→feedback lifecycle
/// to the panel; performs NO mutation (R7/R8: distillation stays LLM-driven).
pub async fn handle_list_corrections(request: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    use crate::memory::store::raw_memory::{RawMemorySource, RawMemoryStore};

    #[derive(serde::Deserialize, Default)]
    struct Params {
        agent_id: Option<String>,
        limit: Option<usize>,
        include_distilled: Option<bool>,
    }
    let params: Params = request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();

    let agent_id = params
        .agent_id
        .as_deref()
        .unwrap_or(crate::routing::DEFAULT_AGENT_ID);
    let limit = params.limit.filter(|n| *n > 0).unwrap_or(50);
    let include_distilled = params.include_distilled.unwrap_or(true);

    match db
        .get_raw_by_path_prefix("aleph://correction/", agent_id, limit)
        .await
    {
        Ok(rows) => {
            let corrections: Vec<_> = rows
                .into_iter()
                .filter(|r| include_distilled || !r.is_processed)
                .map(|r| {
                    let (severity, suggested_rule) = match &r.source {
                        RawMemorySource::Correction {
                            severity,
                            suggested_rule,
                        } => (severity.clone(), suggested_rule.clone()),
                        _ => ("low".to_string(), None),
                    };
                    json!({
                        "id": r.id,
                        "content": r.content,
                        "severity": severity,
                        "suggested_rule": suggested_rule,
                        "status": if r.is_processed { "distilled" } else { "pending" },
                        "created_at": r.created_at,
                    })
                })
                .collect();
            JsonRpcResponse::success(request.id, json!({ "corrections": corrections }))
        }
        Err(err) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("memory.list_corrections failed: {err}"),
        ),
    }
}
```

> NOTE: If `src/gateway/handlers/memory.rs` does not already `use serde_json::json;` at module scope, add it. Confirm the existing imports at the top (lines 5-11) — `json` and the protocol items are already imported per the codebase.

- [ ] **Step 4: Register the handler**

In `src/bin/aleph-server/commands/start/builder/handlers/memory.rs`, after the `memory.listFacts` registration, add:

```rust
    // Read-only corrections governance: raw correction rows + distillation status.
    register_handler!(
        server,
        "memory.list_corrections",
        memory_handlers::handle_list_corrections,
        memory_db
    );
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p alephcore --lib list_corrections_tests`
Expected: PASS (2 tests).

- [ ] **Step 6: Verify the binary compiles**

Run: `cargo check --bin aleph-server`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add src/gateway/handlers/memory.rs src/bin/aleph-server/commands/start/builder/handlers/memory.rs
git commit -m "gateway: add memory.list_corrections read-only handler"
```

---

### Task 4: Frontend API client (both RPCs)

**Files:**
- Modify: `interfaces/webchat/src/api/memory_config.rs` (add response DTOs + two `rpc_call` wrappers)

**Interfaces:**
- Consumes: `DashboardState::rpc_call(method: &str, params: serde_json::Value) -> Result<serde_json::Value, String>` (already used by sibling clients, e.g. `MemoryConfigApi::get`).
- Produces:
  - DTOs `DreamInsightsResponse { daily: Vec<DailyInsightDto>, synthesis: Vec<SynthesisNoteDto>, runs: Vec<DreamRunDto> }`, `DailyInsightDto`, `SynthesisNoteDto`, `DreamRunDto`, `CorrectionsResponse { corrections: Vec<CorrectionDto> }`, `CorrectionDto`.
  - `DreamInsightsApi::list(state, agent_id, limit) -> Result<DreamInsightsResponse, String>`.
  - `CorrectionsApi::list(state, agent_id, include_distilled) -> Result<CorrectionsResponse, String>`.

- [ ] **Step 1: Add response DTOs**

Append near the other `#[derive(Deserialize)]` response structs in `interfaces/webchat/src/api/memory_config.rs` (after `TracedResult`, ~line 274):

```rust
/// Response from `dreaming.list_insights` RPC.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct DreamInsightsResponse {
    #[serde(default)]
    pub daily: Vec<DailyInsightDto>,
    #[serde(default)]
    pub synthesis: Vec<SynthesisNoteDto>,
    #[serde(default)]
    pub runs: Vec<DreamRunDto>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DailyInsightDto {
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub source_memory_count: u32,
    #[serde(default)]
    pub created_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SynthesisNoteDto {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DreamRunDto {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub pipeline_type: String,
    #[serde(default)]
    pub started_at: i64,
    #[serde(default)]
    pub finished_at: i64,
    #[serde(default)]
    pub duration_ms: i64,
    #[serde(default)]
    pub synthesis_count: u32,
    #[serde(default)]
    pub errors: Option<String>,
}

/// Response from `memory.list_corrections` RPC.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct CorrectionsResponse {
    #[serde(default)]
    pub corrections: Vec<CorrectionDto>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CorrectionDto {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub severity: String,
    #[serde(default)]
    pub suggested_rule: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub created_at: i64,
}
```

- [ ] **Step 2: Add the API wrappers**

Add two impl blocks near the other `*Api` structs (e.g. after `MemoryConfigApi`, ~line 449):

```rust
pub struct DreamInsightsApi;

impl DreamInsightsApi {
    pub async fn list(
        state: &DashboardState,
        agent_id: Option<String>,
        limit: Option<usize>,
    ) -> Result<DreamInsightsResponse, String> {
        let mut params = serde_json::Map::new();
        if let Some(a) = agent_id {
            params.insert("agent_id".into(), serde_json::Value::String(a));
        }
        if let Some(l) = limit {
            params.insert("limit".into(), serde_json::json!(l));
        }
        let result = state
            .rpc_call("dreaming.list_insights", serde_json::Value::Object(params))
            .await?;
        serde_json::from_value(result).map_err(|e| format!("parse dreaming.list_insights: {e}"))
    }
}

pub struct CorrectionsApi;

impl CorrectionsApi {
    pub async fn list(
        state: &DashboardState,
        agent_id: Option<String>,
        include_distilled: bool,
    ) -> Result<CorrectionsResponse, String> {
        let mut params = serde_json::Map::new();
        if let Some(a) = agent_id {
            params.insert("agent_id".into(), serde_json::Value::String(a));
        }
        params.insert("include_distilled".into(), serde_json::json!(include_distilled));
        let result = state
            .rpc_call("memory.list_corrections", serde_json::Value::Object(params))
            .await?;
        serde_json::from_value(result).map_err(|e| format!("parse memory.list_corrections: {e}"))
    }
}
```

- [ ] **Step 3: Verify the panel compiles**

Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown`
Expected: clean (DTOs/APIs unused-but-pub is fine; if a dead-code warning fires, it clears once Tasks 5/6 consume them).

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/api/memory_config.rs
git commit -m "panel: add dream-insights and corrections API clients"
```

---

### Task 5: Dream Insights panel component

**Files:**
- Modify: `interfaces/webchat/src/views/settings/memory.rs` (add `DreamInsightsPanel` component + mount in `MemoryView` view, after `<RetrievalDebugPanel />`)
- Modify: `interfaces/webchat/locales/en.json` (add keys under `settings.memory`)
- Modify: `interfaces/webchat/locales/zh.json` (add same keys)

**Interfaces:**
- Consumes: `DreamInsightsApi::list`, `DreamInsightsResponse` (Task 4); `expect_context::<DashboardState>()`; `use_i18n()`, `t!`, `t_string!` macros (already imported in this file — mirror `RetrievalDebugPanel`).
- Produces: `fn DreamInsightsPanel() -> impl IntoView` (collapsible, lazy-loads on expand).

- [ ] **Step 1: Add i18n keys (en.json)**

In `interfaces/webchat/locales/en.json`, inside the `settings.memory` object (near the existing `retrieval_debug` key ~line 765), add:

```json
      "dream_insights": "Dream Insights",
      "dream_daily": "Daily Digests",
      "dream_synthesis": "Synthesis Notes",
      "dream_runs": "Recent Dream Runs",
      "dream_no_insights": "No insights yet — the dream daemon has not produced any.",
      "dream_source_count": "sources",
      "corrections": "Corrections",
      "corrections_pending": "pending",
      "corrections_distilled": "distilled",
      "corrections_suggested_rule": "Suggested rule",
      "corrections_none": "No corrections recorded yet.",
      "corrections_show_distilled": "Show distilled"
```

- [ ] **Step 2: Add i18n keys (zh.json)**

In `interfaces/webchat/locales/zh.json`, inside the matching `settings.memory` object, add:

```json
      "dream_insights": "做梦洞察",
      "dream_daily": "每日摘要",
      "dream_synthesis": "综合笔记",
      "dream_runs": "最近做梦运行",
      "dream_no_insights": "暂无洞察 —— 做梦守护进程尚未产出。",
      "dream_source_count": "来源",
      "corrections": "纠正记录",
      "corrections_pending": "待蒸馏",
      "corrections_distilled": "已蒸馏",
      "corrections_suggested_rule": "建议规则",
      "corrections_none": "暂无纠正记录。",
      "corrections_show_distilled": "显示已蒸馏"
```

- [ ] **Step 3: Add the component**

In `interfaces/webchat/src/views/settings/memory.rs`, add (mirror `RetrievalDebugPanel`'s collapsible + spawn_local pattern; import the API types at the top alongside the existing `RetrieveWithTraceResponse` import):

```rust
#[component]
fn DreamInsightsPanel() -> impl IntoView {
    use crate::api::memory_config::{DreamInsightsApi, DreamInsightsResponse};
    let i18n = use_i18n();
    let expanded = RwSignal::new(false);
    let loading = RwSignal::new(false);
    let data = RwSignal::new(Option::<DreamInsightsResponse>::None);
    let error = RwSignal::new(Option::<String>::None);

    let load = move || {
        let state = expect_context::<DashboardState>();
        spawn_local(async move {
            loading.set(true);
            error.set(None);
            match DreamInsightsApi::list(&state, None, Some(30)).await {
                Ok(resp) => data.set(Some(resp)),
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    };

    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <button
                on:click=move |_| {
                    let next = !expanded.get();
                    expanded.set(next);
                    if next && data.get().is_none() {
                        load();
                    }
                }
                class="flex items-center w-full text-left"
            >
                <span class="text-lg font-semibold">
                    {move || {
                        let prefix = if expanded.get() { "- " } else { "+ " };
                        format!("{}{}", prefix, t_string!(i18n, settings.memory.dream_insights))
                    }}
                </span>
            </button>

            {move || {
                if !expanded.get() {
                    return view! { <div></div> }.into_any();
                }
                view! {
                    <div class="mt-4 space-y-4">
                        {move || if loading.get() {
                            view! { <div class="text-text-tertiary">{t!(i18n, common.loading)}</div> }.into_any()
                        } else { view! { <div></div> }.into_any() }}

                        {move || error.get().map(|e| view! {
                            <div class="p-3 bg-danger-subtle text-danger rounded text-sm">{e}</div>
                        })}

                        {move || data.get().map(|resp| {
                            let runs = resp.runs.clone();
                            let daily = resp.daily.clone();
                            let synthesis = resp.synthesis.clone();
                            let is_empty = runs.is_empty() && daily.is_empty() && synthesis.is_empty();
                            view! {
                                {move || if is_empty {
                                    view! { <div class="text-text-tertiary text-sm">{t!(i18n, settings.memory.dream_no_insights)}</div> }.into_any()
                                } else { view! { <div></div> }.into_any() }}

                                // Recent runs
                                <div>
                                    <h3 class="text-sm font-semibold mb-2">{t!(i18n, settings.memory.dream_runs)}</h3>
                                    <div class="space-y-1">
                                        {runs.into_iter().map(|r| {
                                            let err = r.errors.clone();
                                            view! {
                                                <div class="p-2 bg-surface-sunken rounded border border-border text-sm flex justify-between">
                                                    <span>{r.pipeline_type}</span>
                                                    <span class="text-text-tertiary">{format!("{}ms · {} synth", r.duration_ms, r.synthesis_count)}</span>
                                                    {err.map(|e| view! { <span class="text-danger">{e}</span> })}
                                                </div>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                </div>

                                // Daily digests
                                <div>
                                    <h3 class="text-sm font-semibold mb-2">{t!(i18n, settings.memory.dream_daily)}</h3>
                                    <div class="space-y-2">
                                        {daily.into_iter().map(|d| {
                                            view! {
                                                <div class="p-3 bg-surface-sunken rounded border border-border">
                                                    <div class="flex justify-between mb-1">
                                                        <span class="text-xs font-mono text-text-tertiary">{d.date}</span>
                                                        <span class="text-xs">{format!("{} {}", d.source_memory_count, t_string!(i18n, settings.memory.dream_source_count))}</span>
                                                    </div>
                                                    <p class="text-sm">{d.content}</p>
                                                </div>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                </div>

                                // Synthesis notes
                                <div>
                                    <h3 class="text-sm font-semibold mb-2">{t!(i18n, settings.memory.dream_synthesis)}</h3>
                                    <div class="space-y-2">
                                        {synthesis.into_iter().map(|s| {
                                            view! {
                                                <div class="p-3 bg-surface-sunken rounded border border-border">
                                                    <div class="flex justify-between">
                                                        <span class="text-sm font-medium">{s.title}</span>
                                                        <span class="text-xs font-mono text-text-tertiary">{s.path}</span>
                                                    </div>
                                                </div>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                </div>
                            }
                        })}
                    </div>
                }.into_any()
            }}
        </div>
    }
}
```

- [ ] **Step 4: Mount the component**

In `MemoryView`'s `view!` (the success branch around line 87-90), add `<DreamInsightsPanel />` immediately after `<RetrievalDebugPanel />`:

```rust
                                <RetrievalDebugPanel />
                                <DreamInsightsPanel />
```

- [ ] **Step 5: Verify the panel compiles**

Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown`
Expected: clean. (If `common.loading` key path differs, use the same loading key `RetrievalDebugPanel`'s file already uses — confirm via `grep -n "common.loading\|loading" interfaces/webchat/src/views/settings/memory.rs`.)

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/views/settings/memory.rs interfaces/webchat/locales/en.json interfaces/webchat/locales/zh.json
git commit -m "panel: add Dream Insights settings view"
```

---

### Task 6: Corrections panel component

**Files:**
- Modify: `interfaces/webchat/src/views/settings/memory.rs` (add `CorrectionsPanel` component + mount after `<DreamInsightsPanel />`)

> i18n keys for corrections were added in Task 5 (Steps 1-2). They are already present.

**Interfaces:**
- Consumes: `CorrectionsApi::list`, `CorrectionsResponse` (Task 4); same Leptos/i18n context as Task 5.
- Produces: `fn CorrectionsPanel() -> impl IntoView` (collapsible, lazy-loads on expand, has a "show distilled" toggle that reloads).

- [ ] **Step 1: Add the component**

In `interfaces/webchat/src/views/settings/memory.rs`:

```rust
#[component]
fn CorrectionsPanel() -> impl IntoView {
    use crate::api::memory_config::{CorrectionsApi, CorrectionsResponse};
    let i18n = use_i18n();
    let expanded = RwSignal::new(false);
    let loading = RwSignal::new(false);
    let show_distilled = RwSignal::new(true);
    let data = RwSignal::new(Option::<CorrectionsResponse>::None);
    let error = RwSignal::new(Option::<String>::None);

    let load = move || {
        let state = expect_context::<DashboardState>();
        let include = show_distilled.get();
        spawn_local(async move {
            loading.set(true);
            error.set(None);
            match CorrectionsApi::list(&state, None, include).await {
                Ok(resp) => data.set(Some(resp)),
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    };

    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <button
                on:click=move |_| {
                    let next = !expanded.get();
                    expanded.set(next);
                    if next && data.get().is_none() {
                        load();
                    }
                }
                class="flex items-center w-full text-left"
            >
                <span class="text-lg font-semibold">
                    {move || {
                        let prefix = if expanded.get() { "- " } else { "+ " };
                        format!("{}{}", prefix, t_string!(i18n, settings.memory.corrections))
                    }}
                </span>
            </button>

            {move || {
                if !expanded.get() {
                    return view! { <div></div> }.into_any();
                }
                view! {
                    <div class="mt-4 space-y-3">
                        <label class="flex items-center gap-2 text-sm">
                            <input
                                type="checkbox"
                                prop:checked=move || show_distilled.get()
                                on:change=move |ev| {
                                    show_distilled.set(event_target_checked(&ev));
                                    load();
                                }
                            />
                            <span>{t!(i18n, settings.memory.corrections_show_distilled)}</span>
                        </label>

                        {move || if loading.get() {
                            view! { <div class="text-text-tertiary">{t!(i18n, common.loading)}</div> }.into_any()
                        } else { view! { <div></div> }.into_any() }}

                        {move || error.get().map(|e| view! {
                            <div class="p-3 bg-danger-subtle text-danger rounded text-sm">{e}</div>
                        })}

                        {move || data.get().map(|resp| {
                            let items = resp.corrections.clone();
                            if items.is_empty() {
                                return view! { <div class="text-text-tertiary text-sm">{t!(i18n, settings.memory.corrections_none)}</div> }.into_any();
                            }
                            view! {
                                <div class="space-y-2">
                                    {items.into_iter().map(|c| {
                                        let is_pending = c.status == "pending";
                                        let badge = if is_pending {
                                            t_string!(i18n, settings.memory.corrections_pending).to_string()
                                        } else {
                                            t_string!(i18n, settings.memory.corrections_distilled).to_string()
                                        };
                                        let badge_class = if is_pending {
                                            "text-xs px-2 py-0.5 rounded bg-warning-subtle text-warning"
                                        } else {
                                            "text-xs px-2 py-0.5 rounded bg-success-subtle text-success"
                                        };
                                        let rule = c.suggested_rule.clone();
                                        view! {
                                            <div class="p-3 bg-surface-sunken rounded border border-border">
                                                <div class="flex justify-between items-center mb-1">
                                                    <span class=badge_class>{badge}</span>
                                                    <span class="text-xs text-text-tertiary">{c.severity}</span>
                                                </div>
                                                <p class="text-sm">{c.content}</p>
                                                {rule.map(|r| view! {
                                                    <p class="text-xs text-text-tertiary mt-1">
                                                        {format!("{}: {}", t_string!(i18n, settings.memory.corrections_suggested_rule), r)}
                                                    </p>
                                                })}
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            }.into_any()
                        })}
                    </div>
                }.into_any()
            }}
        </div>
    }
}
```

- [ ] **Step 2: Mount the component**

After `<DreamInsightsPanel />` in `MemoryView`:

```rust
                                <DreamInsightsPanel />
                                <CorrectionsPanel />
```

- [ ] **Step 3: Verify the panel compiles**

Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown`
Expected: clean. If `event_target_checked` is not in scope, confirm import (it's a Leptos helper; `event_target_value` is already used in this file — `event_target_checked` lives in the same module `leptos::prelude`). If `bg-warning-subtle`/`bg-success-subtle` classes are absent in the Tailwind config, fall back to the classes used elsewhere — `grep -n "warning\|success" interfaces/webchat/src/views/settings/memory.rs` for an existing badge style.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/settings/memory.rs
git commit -m "panel: add Corrections governance settings view"
```

---

### Task 7: Sync FEATURE_LOCATOR.md

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md` (§2.5③ + dreaming anchors)

**Interfaces:**
- Consumes: nothing (docs only). Reflects the real RPCs added in Tasks 2-3.

- [ ] **Step 1: Update §2.5③ (Correction & Lesson Sedimentation)**

In `docs/reference/FEATURE_LOCATOR.md`, in the §2.5③ block (~lines 141-147), add a bullet after the **召回** line documenting the new governance visibility:

```markdown
- **治理可见性 (2026-06-20 连线)**：raw correction → distillation 生命周期现经 `memory.list_corrections`（只读，`src/gateway/handlers/memory.rs::handle_list_corrections`）暴露给 panel（Settings ▸ Memory「Corrections」区，`interfaces/webchat/src/views/settings/memory.rs::CorrectionsPanel`）。**纯只读**——写入/蒸馏仍 LLM/工具驱动（守上文设计边界）。
```

- [ ] **Step 2: Add dreaming.list_insights anchor**

Find the row/section referencing `dreaming.run_now` or the dreaming subsystem (search `grep -n "dreaming\|做梦\|DREAM" docs/reference/FEATURE_LOCATOR.md`). Add a sibling note (place it in the most relevant existing section; if none, append to §2.5 as a new line):

```markdown
- **做梦洞察可见性 (2026-06-20 连线)**：每日摘要 / synthesis 笔记 / 做梦运行历史现经 `dreaming.list_insights`（只读，`src/gateway/handlers/dreaming.rs::handle_list_insights`，复用 `DreamStore::recent_daily_insights` + `NoteStore::list_notes` filter synthesis + `recent_dream_reports`）暴露给 panel（Settings ▸ Memory「Dream Insights」区）。
```

- [ ] **Step 3: Verify no stale claims**

Re-read the edited sections. Confirm: §2.5③ status line still accurate (pipeline still end-to-end live); new bullets reference real file paths created in Tasks 2-3/5-6. No `cargo` needed (docs only).

- [ ] **Step 4: Commit**

```bash
git add docs/reference/FEATURE_LOCATOR.md
git commit -m "docs: record corrections + dream-insights surfacing in FEATURE_LOCATOR"
```

---

## Self-Review

**1. Spec coverage:**
- Spec §4 (dream insights RPC) → Task 1 (recent_daily_insights) + Task 2 (handler). ✅
- Spec §4.2 (dream panel) → Task 5. ✅
- Spec §5 (corrections RPC, read-only) → Task 3. ✅
- Spec §5.2 (corrections panel) → Task 6. ✅
- Spec §6 (registration) → Tasks 2/3 step "Register". Frontend API → Task 4. ✅
- Spec §6 (FEATURE_LOCATOR sync) → Task 7. ✅
- Spec §7 (test strategy) → real sqlite/in_memory tests in Tasks 1-3; wasm check in 4-6. ✅
- Spec §8 (YAGNI: no CRUD, no pagination, no cross-agent) → handlers are read-only, limit-only, agent-scoped. ✅

**2. Placeholder scan:** All code steps contain full code. The few `grep`/fallback NOTEs are explicit verification hints, not deferred work. ✅

**3. Type consistency:**
- `recent_daily_insights(limit: usize) -> Result<Vec<DailyInsight>>` defined Task 1, consumed Task 2. ✅
- `handle_list_insights(request, db: MemoryBackend)` / `handle_list_corrections(request, db: MemoryBackend)` — registered with matching db-only `register_handler!`. ✅
- Frontend DTO field names (`source_memory_count`, `pipeline_type`, `suggested_rule`, `status`) match the backend `json!` keys exactly. ✅
- `DreamInsightsApi::list` / `CorrectionsApi::list` defined Task 4, consumed Tasks 5/6. ✅

---

## Execution Handoff

This plan is ready for **Subagent-Driven Development** (per the #3 SDD pattern used for blocks ① and ②). Worktree `memory-governance-surfacing` to be created at execution start.
