# Memory Evolution Spec 1: Capture Hooks Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Plug three information-loss boundaries (pre-compress / sub-agent delegation / session-end) in Aleph's memory flow by adding three new producers to the existing `raw_memories → CompressionService → notes` pipeline.

**Architecture:** Zero new abstractions. Three trigger points emit `RawMemory` rows with new `RawMemorySource` enum variants. Existing `CompressionService` consumes them and routes to per-source specialised LLM extraction prompts. A new `session_complete` builtin tool gives the LLM an R8-compliant way to mark task-done boundaries.

**Tech Stack:** Rust, Tokio, SQLite (via existing `AlephSqliteStore`), Axum, existing `FactExtractor` + `NoteIndexer` pipeline, `async_trait`, `tracing`.

**Spec:** `docs/superpowers/specs/2026-04-13-memory-evolution-spec1-capture-hooks-design.md`

---

## File Structure

### Files to CREATE

| Path | Responsibility |
|------|----------------|
| `src/builtin_tools/session_complete.rs` | New LLM-facing tool that writes `RawMemory(SessionEnd{TaskDone})`. ~150 lines. |
| `src/memory/compression/source_prompts.rs` | Four specialised system-prompt constants (RESCUE / LESSON / DIGEST / RETRO) + dispatcher keyed on `RawMemorySource`. ~120 lines. |
| `src/memory/compression/source_prompts/snapshots/rescue.txt` | Snapshot file for prompt regression tests. |
| `src/memory/compression/source_prompts/snapshots/lesson.txt` | Snapshot file. |
| `src/memory/compression/source_prompts/snapshots/digest.txt` | Snapshot file. |
| `src/memory/compression/source_prompts/snapshots/retro.txt` | Snapshot file. |
| `tests/memory_capture_hooks.rs` | End-to-end integration test: emit raw → run CompressionService → assert notes written with expected category. |

### Files to MODIFY

| Path | Change |
|------|--------|
| `src/memory/store/raw_memory.rs` | Add 3 new `RawMemorySource` variants + `SessionEndReason` enum + JSON `source_detail` round-trip. |
| `src/memory/store/sqlite/schema.rs` | Idempotent `ALTER TABLE raw_memories ADD COLUMN source_detail TEXT` migration. |
| `src/memory/store/sqlite/raw_memory.rs` (or wherever raw memory CRUD lives) | Read/write `source_detail`. |
| `src/memory/compression/extractor.rs` | Make `FactExtractor` source-aware: new method `extract_note_updates_for_source(memories, existing_titles, source)` that chooses prompt via `source_prompts::prompt_for`. |
| `src/memory/compression/service.rs` | When fetching unprocessed rows, group by source and call source-aware extractor. |
| `src/memory/compression/mod.rs` | Expose `source_prompts` module. |
| `src/components/session_compactor/compactor.rs` | G1 producer — inside `replace_with_summary` before `session.parts.drain(..)`, write a `RawMemory(PreCompress)` row. |
| `src/a2a/sub_agent.rs` | G2 producer — in `A2ASubAgent::execute`, before `Ok(result)` return, write a `RawMemory(Delegation{child_agent_id})` row. |
| `src/gateway/session_manager/ops.rs` | G3-A producer — in `close_session`, before state flips to `stopped`, query message tail and write a `RawMemory(SessionEnd{Disconnect})` row. |
| `src/builtin_tools/mod.rs` | Register `session_complete` tool. |
| `src/executor/builtin_registry/registry.rs` | Wire tool into schema registry so LLM can see it. |

### Files to LEAVE ALONE (explicit non-modifications)

- `src/memory/session_compactor/` (legacy 1062-line module — retained for `store_raw_chunk`; orthogonal to Spec 1).
- `src/memory/notes/extractor.rs::build_note_extraction_prompt` — keep intact; `source_prompts` wraps/dispatches around it, does not replace it.

---

## Pre-work: Verify build baseline

- [ ] **Step 0.1: Confirm green baseline**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in X.Xs` — no errors.

If baseline fails, STOP and fix before continuing.

---

## Task 1: Extend `RawMemorySource` enum

**Files:**
- Modify: `src/memory/store/raw_memory.rs`
- Test: `src/memory/store/raw_memory.rs` (unit tests at file bottom)

- [ ] **Step 1.1: Write failing test — round-trip new variants**

Add at bottom of `src/memory/store/raw_memory.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_pre_compress() {
        let src = RawMemorySource::PreCompress;
        let (token, detail) = src.to_persisted();
        assert_eq!(token, "pre_compress");
        assert!(detail.is_none());
        let back = RawMemorySource::from_persisted(token, detail.as_deref());
        assert_eq!(back, src);
    }

    #[test]
    fn round_trip_delegation_with_detail() {
        let src = RawMemorySource::Delegation {
            child_agent_id: "child-42".into(),
        };
        let (token, detail) = src.to_persisted();
        assert_eq!(token, "delegation");
        let detail = detail.expect("delegation carries detail JSON");
        let back = RawMemorySource::from_persisted(token, Some(&detail));
        assert_eq!(back, src);
    }

    #[test]
    fn round_trip_session_end_task_done() {
        let src = RawMemorySource::SessionEnd {
            reason: SessionEndReason::TaskDone,
        };
        let (token, detail) = src.to_persisted();
        assert_eq!(token, "session_end");
        let detail = detail.expect("session_end carries detail JSON");
        let back = RawMemorySource::from_persisted(token, Some(&detail));
        assert_eq!(back, src);
    }

    #[test]
    fn legacy_variants_still_parse() {
        let back =
            RawMemorySource::from_persisted("session_compressed", None);
        assert_eq!(back, RawMemorySource::SessionCompressed);
    }
}
```

- [ ] **Step 1.2: Run test — expect compile error**

Run: `cargo test -p alephcore raw_memory::tests -- --nocapture 2>&1 | tail -20`
Expected: compile errors — `variant PreCompress not found`, `SessionEndReason not found`, `to_persisted / from_persisted not found`.

- [ ] **Step 1.3: Extend the enum**

Replace the existing `RawMemorySource` block (lines 5–34) in `src/memory/store/raw_memory.rs` with:

```rust
/// Source of raw memory data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawMemorySource {
    // Legacy — keep for backward compatibility.
    SessionCompressed,
    Transcript,
    ToolOutput,
    Attachment,

    // Spec 1 — Memory Capture Hooks.
    PreCompress,
    Delegation { child_agent_id: String },
    SessionEnd { reason: SessionEndReason },
}

/// Sub-reason for `RawMemorySource::SessionEnd`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEndReason {
    /// Gateway close or idle timeout.
    Disconnect,
    /// LLM called the `session_complete` tool.
    TaskDone,
}

impl RawMemorySource {
    /// Split enum into `(token, optional_detail_json)` for SQLite storage.
    /// Legacy variants return `(token, None)` so existing rows stay unchanged.
    pub fn to_persisted(&self) -> (&'static str, Option<String>) {
        match self {
            Self::SessionCompressed => ("session_compressed", None),
            Self::Transcript => ("transcript", None),
            Self::ToolOutput => ("tool_output", None),
            Self::Attachment => ("attachment", None),
            Self::PreCompress => ("pre_compress", None),
            Self::Delegation { child_agent_id } => (
                "delegation",
                Some(
                    serde_json::json!({ "child_agent_id": child_agent_id })
                        .to_string(),
                ),
            ),
            Self::SessionEnd { reason } => (
                "session_end",
                Some(serde_json::json!({ "reason": reason }).to_string()),
            ),
        }
    }

    /// Parse `(token, optional_detail_json)` back into the enum.
    /// Unknown tokens fall through to `ToolOutput` (matches old behaviour).
    pub fn from_persisted(token: &str, detail: Option<&str>) -> Self {
        match token {
            "session_compressed" => Self::SessionCompressed,
            "transcript" => Self::Transcript,
            "tool_output" => Self::ToolOutput,
            "attachment" => Self::Attachment,
            "pre_compress" => Self::PreCompress,
            "delegation" => {
                let child_agent_id = detail
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                    .and_then(|v| v.get("child_agent_id").and_then(|x| x.as_str()).map(String::from))
                    .unwrap_or_default();
                Self::Delegation { child_agent_id }
            }
            "session_end" => {
                let reason = detail
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                    .and_then(|v| v.get("reason").and_then(|x| x.as_str().map(str::to_string)))
                    .unwrap_or_else(|| "disconnect".into());
                let reason = match reason.as_str() {
                    "task_done" => SessionEndReason::TaskDone,
                    _ => SessionEndReason::Disconnect,
                };
                Self::SessionEnd { reason }
            }
            _ => Self::ToolOutput,
        }
    }

    /// Backwards-compat shim — existing callers that only had a token.
    pub fn as_str(&self) -> &'static str {
        self.to_persisted().0
    }

    /// Backwards-compat shim — existing callers that only had a token.
    pub fn from_str(s: &str) -> Self {
        Self::from_persisted(s, None)
    }
}
```

- [ ] **Step 1.4: Run test — expect pass**

Run: `cargo test -p alephcore raw_memory::tests -- --nocapture 2>&1 | tail -20`
Expected: 4 tests pass.

- [ ] **Step 1.5: Commit**

```bash
git add src/memory/store/raw_memory.rs
git commit -m "feat(memory): extend RawMemorySource with capture-hook variants

Add PreCompress, Delegation{child_agent_id}, and SessionEnd{reason}
variants plus SessionEndReason enum. Introduces to_persisted /
from_persisted round-trip that preserves variant payload in a JSON
detail column. Legacy variants round-trip unchanged."
```

---

## Task 2: Add `source_detail` column + persistence

**Files:**
- Modify: `src/memory/store/sqlite/schema.rs`
- Modify: `src/memory/store/sqlite/raw_memory.rs` (or whichever file implements `RawMemoryStore` for SQLite — verify via `grep -rn 'impl RawMemoryStore for' src/memory/store/sqlite/`)

- [ ] **Step 2.1: Locate the current SQLite impl**

Run: `grep -rn 'impl RawMemoryStore for' /Volumes/TBU4/Workspace/Aleph/src/memory/store/sqlite/`
Record the file path reported. Below, the placeholder `RAW_IMPL_FILE` refers to that file.

- [ ] **Step 2.2: Write failing test — column exists and stores detail**

In `RAW_IMPL_FILE` (or its existing `#[cfg(test)]` module), add:

```rust
#[tokio::test]
async fn raw_memory_source_detail_round_trips() {
    use crate::memory::store::raw_memory::{
        RawMemory, RawMemorySource, SessionEndReason, RawMemoryStore,
    };

    let store = super::tests::fresh_store().await; // existing helper
    let raw = RawMemory::new(
        "user: x\nassistant: y".into(),
        RawMemorySource::SessionEnd {
            reason: SessionEndReason::TaskDone,
        },
    )
    .with_agent("agent-1")
    .with_session("sess-1");

    store.insert_raw_memory(&raw).await.unwrap();
    let fetched = store
        .get_unprocessed_raw_memories("agent-1", 10)
        .await
        .unwrap();
    assert_eq!(fetched.len(), 1);
    assert_eq!(
        fetched[0].source,
        RawMemorySource::SessionEnd {
            reason: SessionEndReason::TaskDone
        }
    );
}
```

If a `fresh_store()` helper does not exist, add a minimal one in the same test module that builds an in-memory `AlephSqliteStore` with `init_schema`.

- [ ] **Step 2.3: Run test — expect fail (column missing or detail dropped)**

Run: `cargo test -p alephcore raw_memory_source_detail_round_trips -- --nocapture 2>&1 | tail -30`
Expected: failure — either "no such column" or equality assertion diverges because `SessionEnd` decodes back as `ToolOutput`.

- [ ] **Step 2.4: Add migration — `source_detail` column**

In `src/memory/store/sqlite/schema.rs`, find the `init_schema` function (search: `fn init_schema`). Add after the existing raw_memories-related block, inside `init_schema`:

```rust
// Spec 1 (memory capture hooks): add detail column for enum payloads.
// Idempotent — PRAGMA table_info avoids duplicate column errors.
let has_source_detail: bool = conn
    .prepare("PRAGMA table_info(raw_memories)")?
    .query_map([], |row| row.get::<_, String>(1))?
    .filter_map(Result::ok)
    .any(|name| name == "source_detail");
if !has_source_detail {
    conn.execute(
        "ALTER TABLE raw_memories ADD COLUMN source_detail TEXT",
        [],
    )?;
}
```

- [ ] **Step 2.5: Update SQLite CRUD to carry `source_detail`**

In `RAW_IMPL_FILE`, modify `insert_raw_memory`:
- In the `INSERT` SQL, add the `source_detail` column.
- Compute `let (src_token, src_detail) = raw.source.to_persisted();` and bind both.

Modify every row-to-`RawMemory` decode path (likely in `get_unprocessed_raw_memories`, `get_raw_by_path_prefix`, and any helper):
- Select `source_detail` alongside `source`.
- Build the enum via `RawMemorySource::from_persisted(&src_token, src_detail.as_deref())`.

Remove any lingering `RawMemorySource::from_str` call sites that pass only a token — they are now incomplete.

- [ ] **Step 2.6: Run test — expect pass**

Run: `cargo test -p alephcore raw_memory_source_detail_round_trips -- --nocapture 2>&1 | tail -20`
Expected: pass.

Then run the wider set:
Run: `cargo test -p alephcore memory::store -- --nocapture 2>&1 | tail -30`
Expected: no regressions.

- [ ] **Step 2.7: Commit**

```bash
git add src/memory/store/sqlite/schema.rs <RAW_IMPL_FILE>
git commit -m "feat(memory): persist RawMemorySource detail payload

Add idempotent source_detail column migration to raw_memories and
route all insert/fetch paths through RawMemorySource::to_persisted /
from_persisted. Legacy rows keep source_detail = NULL and decode
unchanged."
```

---

## Task 3: Source-specialised extraction prompts

**Files:**
- Create: `src/memory/compression/source_prompts.rs`
- Create: `src/memory/compression/source_prompts/snapshots/{rescue,lesson,digest,retro}.txt`
- Modify: `src/memory/compression/mod.rs` to expose the new module.

- [ ] **Step 3.1: Write failing test — prompt dispatch**

Create `src/memory/compression/source_prompts.rs`:

```rust
//! Source-specialised system prompts for the fact extractor.
//!
//! Each `RawMemorySource` variant routes to a prompt tuned to the
//! semantic of that capture point. Legacy variants fall back to the
//! generic prompt so existing behaviour is preserved.

use crate::memory::store::raw_memory::{RawMemorySource, SessionEndReason};

pub const PROMPT_RESCUE: &str = include_str!("source_prompts/snapshots/rescue.txt");
pub const PROMPT_LESSON: &str = include_str!("source_prompts/snapshots/lesson.txt");
pub const PROMPT_DIGEST: &str = include_str!("source_prompts/snapshots/digest.txt");
pub const PROMPT_RETRO: &str = include_str!("source_prompts/snapshots/retro.txt");

/// Choose the system prompt for a given raw-memory source.
/// Legacy variants return `None` so the caller falls back to the
/// existing generic prompt in `FactExtractor`.
pub fn prompt_for(source: &RawMemorySource) -> Option<&'static str> {
    match source {
        RawMemorySource::PreCompress => Some(PROMPT_RESCUE),
        RawMemorySource::Delegation { .. } => Some(PROMPT_LESSON),
        RawMemorySource::SessionEnd {
            reason: SessionEndReason::Disconnect,
        } => Some(PROMPT_DIGEST),
        RawMemorySource::SessionEnd {
            reason: SessionEndReason::TaskDone,
        } => Some(PROMPT_RETRO),
        RawMemorySource::SessionCompressed
        | RawMemorySource::Transcript
        | RawMemorySource::ToolOutput
        | RawMemorySource::Attachment => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_compress_selects_rescue() {
        assert_eq!(
            prompt_for(&RawMemorySource::PreCompress),
            Some(PROMPT_RESCUE)
        );
    }

    #[test]
    fn delegation_selects_lesson() {
        assert_eq!(
            prompt_for(&RawMemorySource::Delegation {
                child_agent_id: "c".into()
            }),
            Some(PROMPT_LESSON)
        );
    }

    #[test]
    fn session_end_disconnect_selects_digest() {
        assert_eq!(
            prompt_for(&RawMemorySource::SessionEnd {
                reason: SessionEndReason::Disconnect,
            }),
            Some(PROMPT_DIGEST)
        );
    }

    #[test]
    fn session_end_task_done_selects_retro() {
        assert_eq!(
            prompt_for(&RawMemorySource::SessionEnd {
                reason: SessionEndReason::TaskDone,
            }),
            Some(PROMPT_RETRO)
        );
    }

    #[test]
    fn legacy_variants_return_none() {
        assert!(prompt_for(&RawMemorySource::Transcript).is_none());
        assert!(prompt_for(&RawMemorySource::ToolOutput).is_none());
        assert!(prompt_for(&RawMemorySource::Attachment).is_none());
        assert!(prompt_for(&RawMemorySource::SessionCompressed).is_none());
    }

    #[test]
    fn prompts_have_nonempty_snapshots() {
        for prompt in [PROMPT_RESCUE, PROMPT_LESSON, PROMPT_DIGEST, PROMPT_RETRO] {
            assert!(prompt.len() > 100, "prompt snapshot too short");
            assert!(
                prompt.contains("JSON"),
                "prompt must instruct LLM to emit JSON"
            );
        }
    }
}
```

- [ ] **Step 3.2: Create prompt snapshot files**

Create directory: `mkdir -p src/memory/compression/source_prompts/snapshots`

Create `src/memory/compression/source_prompts/snapshots/rescue.txt`:

```
You are a memory rescue assistant. The conversation batch below is about to be DROPPED from live context to stay within the model's token budget. This is your last chance to extract durable knowledge from it.

RULES:
1. Err on over-extraction — anything that might matter later should be surfaced.
2. Prioritise: user decisions, stated preferences, unfinished tasks, important facts, commitments, deadlines.
3. Third-person statements. Atomic. Classify into: preference | plan | learning | project | personal | lesson | other.
4. Ignore pure small talk.

OUTPUT FORMAT (JSON only, no markdown code blocks):
{
  "updates": [
    {
      "action": "create" | "append" | "update",
      "category": "preference" | "plan" | "learning" | "project" | "personal" | "lesson" | "other",
      "filename": "short-kebab-case-title",
      "title": "Human-readable title",
      "facts": ["Third-person atomic statement", "..."],
      "links": []
    }
  ]
}
```

Create `src/memory/compression/source_prompts/snapshots/lesson.txt`:

```
You are a memory assistant recording what a parent agent learned from delegating work to a sub-agent. A sub-agent was given a task and has just returned a result.

RULES:
1. Focus on durable value for the PARENT agent's long-term knowledge base.
2. Capture: tool usage patterns that worked, failure modes, domain findings, API / library gotchas, useful heuristics.
3. Skip the sub-agent's conversational chatter. Skip task-specific output that has no reuse value.
4. Classify into: lesson | tool | learning | project | other. Prefer `lesson` and `tool` for delegation takeaways.

OUTPUT FORMAT (JSON only, no markdown code blocks):
{
  "updates": [
    {
      "action": "create" | "append" | "update",
      "category": "lesson" | "tool" | "learning" | "project" | "other",
      "filename": "short-kebab-case-title",
      "title": "Human-readable title",
      "facts": ["Third-person atomic statement, parent-agent perspective", "..."],
      "links": []
    }
  ]
}
```

Create `src/memory/compression/source_prompts/snapshots/digest.txt`:

```
You are a memory digest assistant. A conversation session has ended (the user disconnected or timed out). Produce an end-of-session digest distilling durable facts.

RULES:
1. Prioritise in this order: user preferences, project progress, unfinished tasks, commitments, personal facts (non-sensitive).
2. Ignore transient small talk and purely transactional messages.
3. Third-person. Atomic. Each fact should survive across future sessions.
4. Classify into: preference | project | plan | personal | learning | other.

OUTPUT FORMAT (JSON only, no markdown code blocks):
{
  "updates": [
    {
      "action": "create" | "append" | "update",
      "category": "preference" | "project" | "plan" | "personal" | "learning" | "other",
      "filename": "short-kebab-case-title",
      "title": "Human-readable title",
      "facts": ["Third-person atomic statement", "..."],
      "links": []
    }
  ]
}
```

Create `src/memory/compression/source_prompts/snapshots/retro.txt`:

```
You are a memory retrospective assistant. The LLM just called `session_complete` to mark a self-contained task as finished. Retro the task for durable learnings.

RULES:
1. Output perspective: "what should the agent do differently / keep doing next time a similar task arrives".
2. Capture: what worked, what failed, gotchas discovered, tool usage patterns, sequencing tricks.
3. Skip details only relevant to this specific run. Focus on transferable lessons.
4. Strongly prefer category `lesson`. Allow `tool` or `learning` when clearly more specific.

OUTPUT FORMAT (JSON only, no markdown code blocks):
{
  "updates": [
    {
      "action": "create" | "append" | "update",
      "category": "lesson" | "tool" | "learning" | "other",
      "filename": "short-kebab-case-title",
      "title": "Human-readable title",
      "facts": ["Third-person atomic lesson statement", "..."],
      "links": []
    }
  ]
}
```

- [ ] **Step 3.3: Wire module**

In `src/memory/compression/mod.rs`, add near the other `mod X; pub use X::*;` statements:

```rust
pub mod source_prompts;
```

- [ ] **Step 3.4: Run tests**

Run: `cargo test -p alephcore compression::source_prompts -- --nocapture 2>&1 | tail -20`
Expected: 6 tests pass.

- [ ] **Step 3.5: Commit**

```bash
git add src/memory/compression/source_prompts.rs \
        src/memory/compression/source_prompts/ \
        src/memory/compression/mod.rs
git commit -m "feat(memory): source-specialised extraction prompts

Add prompt_for() dispatcher returning four specialised system prompts
(RESCUE / LESSON / DIGEST / RETRO) keyed on RawMemorySource variants.
Legacy variants return None so FactExtractor keeps its existing
generic prompt. Prompt bodies stored as snapshot .txt files for
regression-testable review."
```

---

## Task 4: Source-aware `FactExtractor`

**Files:**
- Modify: `src/memory/compression/extractor.rs`

- [ ] **Step 4.1: Write failing test — extractor picks source prompt**

Append to `src/memory/compression/extractor.rs` tests module:

```rust
#[test]
fn source_prompt_select_for_pre_compress() {
    use crate::memory::compression::source_prompts::{prompt_for, PROMPT_RESCUE};
    use crate::memory::store::raw_memory::RawMemorySource;
    let p = prompt_for(&RawMemorySource::PreCompress).unwrap();
    assert_eq!(p, PROMPT_RESCUE);
}

#[tokio::test]
async fn extract_note_updates_for_source_uses_rescue_prompt() {
    // This test exercises the dispatch logic only, not the LLM itself.
    // MockAiProvider records the system prompt so we can assert on it.
    use crate::memory::context::{ContextAnchor, MemoryEntry};
    use crate::memory::embedding_provider::tests::MockEmbeddingProvider;
    use crate::memory::store::raw_memory::RawMemorySource;
    use crate::providers::recording_mock::RecordingMockProvider;

    let provider = RecordingMockProvider::new(
        r#"{"updates":[]}"#.to_string(),
    );
    let recorded = provider.recorded_system_prompt();
    let extractor = FactExtractor::new(
        Arc::new(provider),
        Arc::new(MockEmbeddingProvider::new(1024, "mock-model")),
    );

    let mem = MemoryEntry {
        id: "m1".into(),
        user_input: "dummy".into(),
        ai_output: "dummy".into(),
        context: ContextAnchor {
            window_title: String::new(),
            timestamp: 0,
            session_id: "s".into(),
        },
        embedding: None,
        namespace: "owner".into(),
        agent: "default".into(),
        similarity_score: None,
    };

    let _ = extractor
        .extract_note_updates_for_source(
            &[mem],
            &[],
            &RawMemorySource::PreCompress,
        )
        .await
        .unwrap();

    let got = recorded.lock().unwrap().clone().unwrap();
    assert!(
        got.contains("memory rescue assistant"),
        "expected RESCUE prompt, got: {got}"
    );
}
```

If `RecordingMockProvider` does not exist, add a minimal implementation at `src/providers/recording_mock.rs`:

```rust
//! Minimal AiProvider stub that records the last system prompt it saw.
use crate::providers::adapter::{AdapterResponse, RequestPayload};
use crate::providers::AiProvider;
use crate::sync_primitives::{Arc, Mutex};
use async_trait::async_trait;

pub struct RecordingMockProvider {
    canned: String,
    last_system: Arc<Mutex<Option<String>>>,
}

impl RecordingMockProvider {
    pub fn new(canned: String) -> Self {
        Self {
            canned,
            last_system: Arc::new(Mutex::new(None)),
        }
    }
    pub fn recorded_system_prompt(&self) -> Arc<Mutex<Option<String>>> {
        self.last_system.clone()
    }
}

#[async_trait]
impl AiProvider for RecordingMockProvider {
    async fn process(
        &self,
        req: RequestPayload<'_>,
    ) -> Result<AdapterResponse, crate::error::AlephError> {
        if let Some(sys) = req.system() {
            *self.last_system.lock().unwrap() = Some(sys.to_string());
        }
        Ok(AdapterResponse::text(self.canned.clone()))
    }
}
```

Then expose it behind `#[cfg(any(test, feature = "test-helpers"))]` from `src/providers/mod.rs`:

```rust
#[cfg(any(test, feature = "test-helpers"))]
pub mod recording_mock;
```

If `AdapterResponse::text` constructor shape differs, adapt the `Ok(...)` expression to match (grep `impl AdapterResponse` under `src/providers/` to find the correct constructor).

- [ ] **Step 4.2: Run test — expect fail**

Run: `cargo test -p alephcore extract_note_updates_for_source_uses_rescue_prompt -- --nocapture 2>&1 | tail -30`
Expected: compile error — `extract_note_updates_for_source` not found.

- [ ] **Step 4.3: Implement source-aware extraction**

Add this method to `impl FactExtractor` in `src/memory/compression/extractor.rs` (next to the existing `extract_note_updates`):

```rust
/// Source-aware note extraction.
///
/// If the `source` resolves to a specialised prompt via
/// `source_prompts::prompt_for`, use it — otherwise fall back to
/// the existing generic note-extraction prompt.
pub async fn extract_note_updates_for_source(
    &self,
    memories: &[MemoryEntry],
    existing_titles: &[String],
    source: &crate::memory::store::raw_memory::RawMemorySource,
) -> Result<crate::memory::notes::extractor::NoteExtractionResponse, AlephError> {
    use crate::memory::compression::source_prompts::prompt_for;
    use crate::memory::notes::extractor::{
        build_note_extraction_prompt, NoteExtractionResponse,
    };

    if memories.is_empty() {
        return Ok(NoteExtractionResponse { updates: vec![] });
    }

    let system_prompt = match prompt_for(source) {
        Some(prompt) => prompt.to_string(),
        None => build_note_extraction_prompt(existing_titles),
    };
    let user_prompt = self.build_extraction_prompt(memories);

    let msgs = [UnifiedMessage::user(&user_prompt)];
    let response = self
        .provider
        .process(RequestPayload::new(&msgs).with_system(Some(&system_prompt)))
        .await
        .map_err(|e| {
            AlephError::other(format!(
                "Source-aware note extraction LLM call failed: {e}"
            ))
        })?;

    let text = response.text_content();

    let json_value = match extract_json_robust(&text) {
        Some(v) => v,
        None => {
            warn!(
                "No JSON found in source-aware note extraction response \
                 (source={:?}), returning empty updates",
                source
            );
            return Ok(NoteExtractionResponse { updates: vec![] });
        }
    };

    serde_json::from_value(json_value).map_err(|e| {
        warn!("Failed to parse source-aware note extraction JSON: {e}");
        AlephError::other(format!(
            "Failed to parse source-aware note extraction: {e}"
        ))
    })
}
```

- [ ] **Step 4.4: Run test — expect pass**

Run: `cargo test -p alephcore extract_note_updates_for_source_uses_rescue_prompt -- --nocapture 2>&1 | tail -30`
Expected: pass.

Run full extractor tests: `cargo test -p alephcore compression::extractor -- --nocapture 2>&1 | tail -30`
Expected: no regression in pre-existing tests.

- [ ] **Step 4.5: Commit**

```bash
git add src/memory/compression/extractor.rs src/providers/recording_mock.rs src/providers/mod.rs
git commit -m "feat(memory): source-aware FactExtractor method

Add FactExtractor::extract_note_updates_for_source() that picks the
specialised prompt via source_prompts::prompt_for(). Legacy variants
fall back to build_note_extraction_prompt so existing extraction
paths are unchanged. Also add test-only RecordingMockProvider for
system-prompt assertions."
```

---

## Task 5: Route `CompressionService` through source-aware extractor

**Files:**
- Modify: `src/memory/compression/service.rs`

- [ ] **Step 5.1: Inspect current `process_batch` shape**

Run: `grep -n 'fn process_batch\|fn process\|extract_note_updates' /Volumes/TBU4/Workspace/Aleph/src/memory/compression/service.rs | head -20`
Record the function names. Below, the main processing function is referred to as `process_batch`. Rename if different.

- [ ] **Step 5.2: Write failing test — service groups by source**

Add to the tests module of `src/memory/compression/service.rs` (or create one):

```rust
#[cfg(test)]
mod tests_spec1 {
    use super::*;
    use crate::memory::store::raw_memory::{
        RawMemory, RawMemorySource, SessionEndReason,
    };

    #[test]
    fn group_by_source_separates_specialised_and_legacy() {
        let mut rows = vec![
            RawMemory::new("a".into(), RawMemorySource::Transcript),
            RawMemory::new("b".into(), RawMemorySource::PreCompress),
            RawMemory::new("c".into(), RawMemorySource::Transcript),
            RawMemory::new(
                "d".into(),
                RawMemorySource::SessionEnd {
                    reason: SessionEndReason::TaskDone,
                },
            ),
        ];
        // Force stable IDs so equality is deterministic
        for (i, r) in rows.iter_mut().enumerate() {
            r.id = format!("id-{i}");
        }

        let groups = group_by_source(&rows);
        // 3 groups: Transcript, PreCompress, SessionEnd{TaskDone}
        assert_eq!(groups.len(), 3);
        let t_count = groups
            .iter()
            .find(|(s, _)| matches!(s, RawMemorySource::Transcript))
            .unwrap()
            .1
            .len();
        assert_eq!(t_count, 2);
    }
}
```

- [ ] **Step 5.3: Run test — expect compile error**

Run: `cargo test -p alephcore compression::service::tests_spec1 -- --nocapture 2>&1 | tail -10`
Expected: `group_by_source not found`.

- [ ] **Step 5.4: Add `group_by_source` + route through source-aware extractor**

Add to `src/memory/compression/service.rs` (top-level, not inside `impl`):

```rust
/// Group raw memories by their source variant (preserving order within a group).
/// Used by the compression service to run one extractor call per group
/// so each call can use its source-specific system prompt.
pub(crate) fn group_by_source<'a>(
    rows: &'a [crate::memory::store::raw_memory::RawMemory],
) -> Vec<(
    crate::memory::store::raw_memory::RawMemorySource,
    Vec<&'a crate::memory::store::raw_memory::RawMemory>,
)> {
    use std::collections::BTreeMap;
    let mut order: Vec<crate::memory::store::raw_memory::RawMemorySource> = Vec::new();
    let mut buckets: BTreeMap<String, Vec<&crate::memory::store::raw_memory::RawMemory>> =
        BTreeMap::new();
    for r in rows {
        let key = format!("{:?}", r.source);
        if !buckets.contains_key(&key) {
            order.push(r.source.clone());
        }
        buckets.entry(key).or_default().push(r);
    }
    order
        .into_iter()
        .map(|src| {
            let key = format!("{src:?}");
            let v = buckets.remove(&key).unwrap_or_default();
            (src, v)
        })
        .collect()
}
```

Then in the main processing function (`process_batch` or equivalent), find the existing call to `self.extractor.extract_note_updates(...)` and REPLACE the loop that calls it with:

```rust
// Spec 1: route each source group through the source-aware extractor.
for (source, group) in group_by_source(&rows) {
    let memories: Vec<MemoryEntry> = group.iter().map(|r| raw_to_entry(r)).collect();
    let existing_titles = self.collect_existing_titles(agent_id).await?;
    let response = self
        .extractor
        .extract_note_updates_for_source(&memories, &existing_titles, &source)
        .await?;
    self.apply_note_updates(response, agent_id).await?;
    let ids: Vec<String> = group.iter().map(|r| r.id.clone()).collect();
    self.raw_store.mark_raw_as_processed(&ids).await?;
}
```

If `raw_to_entry`, `collect_existing_titles`, or `apply_note_updates` helpers do not exist with these exact names, use the existing service helpers — locate them with `grep -n 'fn ' src/memory/compression/service.rs` and substitute. The goal is: **one extractor call per source group, using the source variant as the routing key**.

- [ ] **Step 5.5: Run test — expect pass**

Run: `cargo test -p alephcore compression::service -- --nocapture 2>&1 | tail -40`
Expected: all tests pass, including the new `group_by_source_separates_specialised_and_legacy`.

- [ ] **Step 5.6: Commit**

```bash
git add src/memory/compression/service.rs
git commit -m "feat(memory): group raw memories by source in CompressionService

Introduce group_by_source() and route each group through
FactExtractor::extract_note_updates_for_source. One LLM call per
source variant ensures each capture point gets its specialised
prompt. Preserves the existing mark_raw_as_processed contract."
```

---

## Task 6: G1 producer — pre-compress hook

**Files:**
- Modify: `src/components/session_compactor/compactor.rs`

- [ ] **Step 6.1: Inspect `replace_with_summary`**

Open `src/components/session_compactor/compactor.rs` and read around lines 495–525 to confirm:
- The function `replace_with_summary` is present.
- `session.parts[0..compact_count]` is the range about to be dropped (before `session.parts.drain(compact_count..)`).
- `session.agent_id` and `session.id` exist.

If naming differs, adapt the insertion below.

- [ ] **Step 6.2: Add a dependency injection point**

Find the `SessionCompactor` struct definition near the top of the file. Add an optional `raw_memory_writer` field:

```rust
pub struct SessionCompactor {
    // ...existing fields...
    raw_memory_writer: Option<std::sync::Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>>,
}
```

Extend the primary constructor (look for `pub fn new(`) to accept the new dependency as an `Option<Arc<dyn RawMemoryStore>>` parameter. If there is a builder-style constructor, add a setter `with_raw_memory_writer(self, writer: Arc<dyn RawMemoryStore>) -> Self`.

For existing callers that do not yet wire this, pass `None`.

- [ ] **Step 6.3: Emit PreCompress inside `replace_with_summary`**

At the top of `replace_with_summary`, after computing `compact_count` and BEFORE `let kept_parts: Vec<SessionPart> = session.parts.drain(compact_count..).collect();`, insert:

```rust
// Spec 1 G1: rescue the to-be-dropped chunk into raw_memories.
if compact_count > 0 {
    if let Some(writer) = self.raw_memory_writer.clone() {
        let doomed_text = session
            .parts
            .iter()
            .take(compact_count)
            .map(|p| format!("{p:?}"))
            .collect::<Vec<_>>()
            .join("\n");
        let raw = crate::memory::store::raw_memory::RawMemory::new(
            doomed_text,
            crate::memory::store::raw_memory::RawMemorySource::PreCompress,
        )
        .with_agent(session.agent_id.clone())
        .with_session(session.id.clone());

        // Fire-and-forget; the extraction happens later in
        // CompressionService. Errors are logged but must not
        // block compaction.
        let handle = tokio::runtime::Handle::try_current();
        match handle {
            Ok(rt) => {
                rt.spawn(async move {
                    if let Err(e) = writer.insert_raw_memory(&raw).await {
                        tracing::warn!(
                            "pre_compress raw_memory write failed: {e}"
                        );
                    }
                });
            }
            Err(_) => {
                tracing::warn!(
                    "no tokio runtime for pre_compress emit; skipping"
                );
            }
        }
    }
}
```

Note: `SessionPart` debug-formatting is a deliberately minimal serialisation for Task 1 of the hooks. If `SessionPart` has a richer `to_prompt_text()` method, prefer that. (Search for `impl SessionPart` / `fn to_text`.)

- [ ] **Step 6.4: Write unit test**

Add to the tests module in the same file (or a sibling `tests.rs`):

```rust
#[cfg(test)]
#[tokio::test]
async fn replace_with_summary_emits_pre_compress_raw_memory() {
    use crate::memory::store::raw_memory::{RawMemorySource, RawMemoryStore};
    use std::sync::Arc;

    // Fake writer — captures one insert.
    #[derive(Default)]
    struct FakeWriter(parking_lot::Mutex<Vec<crate::memory::store::raw_memory::RawMemory>>);

    #[async_trait::async_trait]
    impl RawMemoryStore for FakeWriter {
        async fn insert_raw_memory(
            &self,
            raw: &crate::memory::store::raw_memory::RawMemory,
        ) -> Result<(), crate::error::AlephError> {
            self.0.lock().push(raw.clone());
            Ok(())
        }
        async fn get_unprocessed_raw_memories(
            &self,
            _agent_id: &str,
            _limit: usize,
        ) -> Result<Vec<crate::memory::store::raw_memory::RawMemory>, crate::error::AlephError> {
            Ok(vec![])
        }
        async fn mark_raw_as_processed(
            &self,
            _ids: &[String],
        ) -> Result<usize, crate::error::AlephError> {
            Ok(0)
        }
        async fn count_unprocessed(
            &self,
            _agent_id: &str,
        ) -> Result<usize, crate::error::AlephError> {
            Ok(0)
        }
        async fn get_raw_by_path_prefix(
            &self,
            _path_prefix: &str,
            _agent_id: &str,
            _limit: usize,
        ) -> Result<Vec<crate::memory::store::raw_memory::RawMemory>, crate::error::AlephError> {
            Ok(vec![])
        }
    }

    let writer = Arc::new(FakeWriter::default());
    let compactor = SessionCompactor::new_for_test() // existing helper, adapt name
        .with_raw_memory_writer(writer.clone());
    let mut session = fake_session_with_parts(5); // adapt to existing test helper

    compactor.replace_with_summary(&mut session, "summary".into());

    // Give the spawned task a chance to run.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let captured = writer.0.lock();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].source, RawMemorySource::PreCompress);
}
```

If `SessionCompactor::new_for_test` and `fake_session_with_parts` don't exist, add them as minimal helpers in this test module using whatever constructors the codebase already exposes.

- [ ] **Step 6.5: Run test — expect pass**

Run: `cargo test -p alephcore replace_with_summary_emits_pre_compress_raw_memory -- --nocapture 2>&1 | tail -30`
Expected: pass.

- [ ] **Step 6.6: Commit**

```bash
git add src/components/session_compactor/compactor.rs
git commit -m "feat(memory): G1 pre-compress hook emits RawMemory

SessionCompactor::replace_with_summary now writes a
RawMemory(PreCompress) row carrying the chunk about to be dropped,
before draining it. Emission is optional (gated on injected
RawMemoryStore) and fire-and-forget — compaction must not block
on memory IO."
```

---

## Task 7: G2 producer — delegation hook

**Files:**
- Modify: `src/a2a/sub_agent.rs`

- [ ] **Step 7.1: Inspect `A2ASubAgent::execute`**

Read `src/a2a/sub_agent.rs` lines 100–200. Confirm:
- The `Ok(result)` return is around line 183.
- `request.prompt` and `request.id` are available.
- `SubAgentResult { summary, output, .. }` is in scope.

If the shape differs, adjust the insertion below.

- [ ] **Step 7.2: Add raw memory writer field**

Add to the `A2ASubAgent` struct and its constructor:

```rust
raw_memory_writer: Option<std::sync::Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>>,
```

Extend `A2ASubAgent::new(...)` (or whichever constructor is canonical) to accept `Option<Arc<dyn RawMemoryStore>>`. Existing call sites pass `None` until Task 10 wires real dependencies.

- [ ] **Step 7.3: Emit Delegation before `Ok(result)`**

In `execute`, immediately before `Ok(result)` (around line 183), insert:

```rust
// Spec 1 G2: record delegation outcome for parent-agent memory.
if let Some(writer) = self.raw_memory_writer.clone() {
    let content = format!(
        "DELEGATION_PROMPT:\n{prompt}\n\nDELEGATION_RESULT:\n{summary}",
        prompt = request.prompt,
        summary = result.summary,
    );
    let child_agent_id = request
        .target_agent_id
        .clone()
        .unwrap_or_else(|| "unknown-child".into());
    let parent_agent_id = request
        .execution_context
        .metadata
        .get("parent_agent_id")
        .cloned()
        .unwrap_or_else(|| "default".into());
    let parent_session_id = request.parent_session_id.clone();
    let raw = crate::memory::store::raw_memory::RawMemory::new(
        content,
        crate::memory::store::raw_memory::RawMemorySource::Delegation {
            child_agent_id,
        },
    )
    .with_agent(parent_agent_id);
    let raw = match parent_session_id {
        Some(sid) => raw.with_session(sid),
        None => raw,
    };
    tokio::spawn(async move {
        if let Err(e) = writer.insert_raw_memory(&raw).await {
            tracing::warn!("delegation raw_memory write failed: {e}");
        }
    });
}
```

If `SubAgentRequest::target_agent_id` / `execution_context.metadata` / `parent_session_id` don't exist with these names, grep `grep -n 'SubAgentRequest' src/a2a/` and pick the closest equivalents. The requirement is: **parent agent_id and child agent_id both reach the RawMemory**.

- [ ] **Step 7.4: Write test**

```rust
#[cfg(test)]
#[tokio::test]
async fn subagent_execute_emits_delegation_raw_memory() {
    // Reuse FakeWriter from Task 6 (or duplicate if not shared). Build a
    // stub A2ASubAgent whose inner execute path succeeds with a synthetic
    // SubAgentResult, then assert FakeWriter received one row with
    // source = Delegation { child_agent_id = .. }.
    // Implementation uses test helpers already present in src/a2a/sub_agent.rs
    // — adapt to whatever minimal "execute the happy path" harness is
    // available. If none is available, a focused unit test of the emit block
    // (extracted to a pub(crate) helper fn emit_delegation_raw(...) that
    // takes writer + request + result) is acceptable.
    todo!("fill in using the codebase's existing sub_agent test harness")
}
```

If there is no existing harness, **extract the emission block into a `pub(crate) fn emit_delegation_raw(writer: ..., request: &SubAgentRequest, result: &SubAgentResult)` helper**, call the helper from `execute`, and unit-test the helper in isolation. The helper boundary is preferable to `todo!()` anyway — remove the `todo!()` test after extraction.

- [ ] **Step 7.5: Run test — expect pass**

Run: `cargo test -p alephcore subagent_execute_emits_delegation_raw_memory -- --nocapture 2>&1 | tail -30`
Expected: pass.

- [ ] **Step 7.6: Commit**

```bash
git add src/a2a/sub_agent.rs
git commit -m "feat(memory): G2 delegation hook emits RawMemory

A2ASubAgent::execute now emits a RawMemory(Delegation) before
returning, carrying the delegation prompt + sub-agent summary so the
parent agent's memory extractor can distill durable lessons. Writer
is optional (None for legacy call sites)."
```

---

## Task 8: G3-A producer — session disconnect hook

**Files:**
- Modify: `src/gateway/session_manager/ops.rs`

- [ ] **Step 8.1: Inspect `close_session`**

Read `src/gateway/session_manager/ops.rs` lines 490–550. Confirm:
- `close_session(&self, key: &SessionKey, topic: Option<String>)` exists around line 497.
- `current_state` is computed by line 515.
- There is an early-return when state is already `stopped`.

- [ ] **Step 8.2: Extend `SessionManager` with raw memory writer**

Add to the `SessionManager` struct (search for `pub struct SessionManager` in `src/gateway/session_manager/`):

```rust
raw_memory_writer: Option<Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>>,
```

Update its constructor to accept `Option<Arc<dyn RawMemoryStore>>` and all existing call sites to pass `None` for now.

- [ ] **Step 8.3: Emit SessionEnd{Disconnect} inside `close_session`**

In `close_session`, insert after the early-return on `Some("stopped")` and BEFORE the actual state transition (around line 515–520):

```rust
// Spec 1 G3-A: session is about to be marked stopped. Capture the
// conversation tail for end-of-session digest extraction.
if let Some(writer) = self.raw_memory_writer.clone() {
    let agent_id = key.agent_id().to_string();
    let session_id = key.session_id().to_string();
    let tail = self.read_recent_messages(&conn, &key_str, 64).unwrap_or_default();
    if !tail.is_empty() {
        let raw = crate::memory::store::raw_memory::RawMemory::new(
            tail,
            crate::memory::store::raw_memory::RawMemorySource::SessionEnd {
                reason: crate::memory::store::raw_memory::SessionEndReason::Disconnect,
            },
        )
        .with_agent(agent_id)
        .with_session(session_id);
        tokio::spawn(async move {
            if let Err(e) = writer.insert_raw_memory(&raw).await {
                tracing::warn!("session_end raw_memory write failed: {e}");
            }
        });
    }
}
```

Add the helper (private method on `SessionManager`):

```rust
fn read_recent_messages(
    &self,
    conn: &rusqlite::Connection,
    key_str: &str,
    limit: usize,
) -> Result<String, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT role, content FROM messages
         WHERE session_key = ?
         ORDER BY timestamp DESC
         LIMIT ?",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![key_str, limit as i64], |row| {
            Ok(format!(
                "{role}: {content}",
                role = row.get::<_, String>(0)?,
                content = row.get::<_, String>(1)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    // Reverse so oldest-first.
    let mut v = rows;
    v.reverse();
    Ok(v.join("\n"))
}
```

If the schema column names differ from `role` / `content` / `timestamp` / `session_key`, grep `src/gateway/session_manager/` for the actual schema and adjust.

`key.agent_id()` / `key.session_id()` methods: grep `impl SessionKey` to confirm or add the trivial accessors if missing.

- [ ] **Step 8.4: Write test**

```rust
#[cfg(test)]
#[tokio::test]
async fn close_session_emits_session_end_disconnect() {
    // Build a SessionManager backed by an in-memory SQLite + FakeWriter,
    // insert a fake session + 3 messages, call close_session, and assert
    // FakeWriter got one row with source = SessionEnd { Disconnect }.
    // Use existing test helpers in src/gateway/session_manager/tests.rs
    // where available.
    todo!("fill in using the codebase's existing session_manager test harness")
}
```

Same note as Task 7: if no harness exists, extract the emission into a testable helper `fn emit_session_end_raw(&self, conn, key, reason) -> Option<RawMemory>` and unit-test the helper.

- [ ] **Step 8.5: Run test — expect pass**

Run: `cargo test -p alephcore close_session_emits_session_end_disconnect -- --nocapture 2>&1 | tail -30`
Expected: pass.

- [ ] **Step 8.6: Commit**

```bash
git add src/gateway/session_manager/ops.rs
git commit -m "feat(memory): G3-A session-disconnect hook emits RawMemory

close_session now writes a RawMemory(SessionEnd{Disconnect}) row
with the session message tail before flipping state to stopped.
Writer is optional. Fire-and-forget spawn so close_session latency
is unchanged."
```

---

## Task 9: `session_complete` tool (G3-C, LLM-sovereignty path)

**Files:**
- Create: `src/builtin_tools/session_complete.rs`
- Modify: `src/builtin_tools/mod.rs`
- Modify: `src/executor/builtin_registry/registry.rs`

- [ ] **Step 9.1: Write failing test**

Create `src/builtin_tools/session_complete.rs` with ONLY the test and type stub:

```rust
//! LLM-facing tool: mark a self-contained task as complete.
//! Triggers a retrospective extraction via the memory capture hook pipeline.

use crate::error::AlephError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionCompleteArgs {
    /// Brief summary of what the task accomplished.
    pub outcome: String,
    /// Optional pre-distilled learnings the LLM wants to preserve.
    #[serde(default)]
    pub key_learnings: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionCompleteResult {
    pub ok: bool,
}

pub const TOOL_NAME: &str = "session_complete";

pub const TOOL_DESCRIPTION: &str = "Call when you believe a self-contained task has just completed. \
Triggers a memory retrospective so future similar tasks can benefit. \
Does NOT end the conversation — you can keep talking after calling this.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_round_trip_json() {
        let a = SessionCompleteArgs {
            outcome: "built feature X".into(),
            key_learnings: Some(vec!["learning one".into()]),
        };
        let s = serde_json::to_string(&a).unwrap();
        let back: SessionCompleteArgs = serde_json::from_str(&s).unwrap();
        assert_eq!(back.outcome, "built feature X");
        assert_eq!(back.key_learnings.as_ref().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn handle_writes_raw_memory_session_end_task_done() {
        use crate::memory::store::raw_memory::{
            RawMemorySource, RawMemoryStore, SessionEndReason,
        };
        use std::sync::Arc;

        // Reuse FakeWriter (or local duplicate).
        #[derive(Default)]
        struct FakeWriter(parking_lot::Mutex<Vec<crate::memory::store::raw_memory::RawMemory>>);
        #[async_trait::async_trait]
        impl RawMemoryStore for FakeWriter {
            async fn insert_raw_memory(
                &self,
                raw: &crate::memory::store::raw_memory::RawMemory,
            ) -> Result<(), AlephError> {
                self.0.lock().push(raw.clone());
                Ok(())
            }
            async fn get_unprocessed_raw_memories(
                &self,
                _: &str,
                _: usize,
            ) -> Result<Vec<crate::memory::store::raw_memory::RawMemory>, AlephError> {
                Ok(vec![])
            }
            async fn mark_raw_as_processed(&self, _: &[String]) -> Result<usize, AlephError> {
                Ok(0)
            }
            async fn count_unprocessed(&self, _: &str) -> Result<usize, AlephError> {
                Ok(0)
            }
            async fn get_raw_by_path_prefix(
                &self,
                _: &str,
                _: &str,
                _: usize,
            ) -> Result<Vec<crate::memory::store::raw_memory::RawMemory>, AlephError> {
                Ok(vec![])
            }
        }

        let writer: Arc<dyn RawMemoryStore> = Arc::new(FakeWriter::default());
        let args = SessionCompleteArgs {
            outcome: "built test".into(),
            key_learnings: Some(vec!["L1".into()]),
        };
        let res = handle(
            args,
            &ToolContext {
                agent_id: "agent-1".into(),
                session_id: Some("sess-1".into()),
                raw_memory_writer: writer.clone(),
            },
        )
        .await
        .unwrap();
        assert!(res.ok);

        // Writer must have exactly one row, source = TaskDone.
        let writer_concrete = writer
            .clone()
            .downcast_arc::<FakeWriter>()
            .unwrap_or_else(|_| unreachable!());
        let captured = writer_concrete.0.lock();
        assert_eq!(captured.len(), 1);
        assert!(matches!(
            captured[0].source,
            RawMemorySource::SessionEnd {
                reason: SessionEndReason::TaskDone
            }
        ));
    }
}
```

The test above uses a minimal `ToolContext` — if the project has a canonical tool-context type (grep `grep -rn 'pub struct ToolContext\|fn handle_tool_call' src/builtin_tools/ | head -10`), use that instead.

The `downcast_arc` call assumes `Arc<dyn Trait>::downcast_arc`. If the runtime doesn't support it out of the box, construct `FakeWriter` inside the test under its concrete type and pass `Arc::new(fake) as Arc<dyn RawMemoryStore>` while retaining a second reference for assertions.

- [ ] **Step 9.2: Run test — expect compile error**

Run: `cargo test -p alephcore session_complete -- --nocapture 2>&1 | tail -30`
Expected: compile error — `handle` and `ToolContext` not found.

- [ ] **Step 9.3: Implement `handle`**

Append to `src/builtin_tools/session_complete.rs`:

```rust
use std::sync::Arc;

/// Minimal tool context — use the project-canonical type if one exists.
pub struct ToolContext {
    pub agent_id: String,
    pub session_id: Option<String>,
    pub raw_memory_writer: Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>,
}

pub async fn handle(
    args: SessionCompleteArgs,
    ctx: &ToolContext,
) -> Result<SessionCompleteResult, AlephError> {
    let content = build_content(&args);
    let raw = crate::memory::store::raw_memory::RawMemory::new(
        content,
        crate::memory::store::raw_memory::RawMemorySource::SessionEnd {
            reason: crate::memory::store::raw_memory::SessionEndReason::TaskDone,
        },
    )
    .with_agent(ctx.agent_id.clone());
    let raw = match &ctx.session_id {
        Some(sid) => raw.with_session(sid.clone()),
        None => raw,
    };
    ctx.raw_memory_writer.insert_raw_memory(&raw).await?;
    Ok(SessionCompleteResult { ok: true })
}

fn build_content(args: &SessionCompleteArgs) -> String {
    let mut s = format!("TASK_OUTCOME:\n{}\n", args.outcome);
    if let Some(learnings) = &args.key_learnings {
        if !learnings.is_empty() {
            s.push_str("\nKEY_LEARNINGS:\n");
            for l in learnings {
                s.push_str("- ");
                s.push_str(l);
                s.push('\n');
            }
        }
    }
    s
}
```

Replace the test's `downcast_arc` usage with two parallel `Arc` handles to the same concrete `FakeWriter` (simpler and always works):

```rust
let fake = Arc::new(FakeWriter::default());
let writer: Arc<dyn RawMemoryStore> = fake.clone();
// ...call handle...
let captured = fake.0.lock();
```

- [ ] **Step 9.4: Wire into module + registry**

In `src/builtin_tools/mod.rs` add:

```rust
pub mod session_complete;
```

In `src/executor/builtin_registry/registry.rs`, find where other builtin tools are registered (search for an existing tool, e.g. `note_manage`, to see the registration pattern). Add a parallel entry for `session_complete`:

```rust
// Spec 1 G3-C: LLM-sovereignty path for task-completion memory retros.
register_tool(
    crate::builtin_tools::session_complete::TOOL_NAME,
    crate::builtin_tools::session_complete::TOOL_DESCRIPTION,
    // JSON schema for SessionCompleteArgs
    schemars::schema_for!(crate::builtin_tools::session_complete::SessionCompleteArgs),
    // Handler: build ToolContext from the registry's available data,
    // then call session_complete::handle.
);
```

Follow the exact macro / builder pattern the registry already uses — every existing tool registration in that file is a correct template.

- [ ] **Step 9.5: Run test — expect pass**

Run: `cargo test -p alephcore session_complete -- --nocapture 2>&1 | tail -30`
Expected: both tests pass.

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: no errors.

- [ ] **Step 9.6: Commit**

```bash
git add src/builtin_tools/session_complete.rs \
        src/builtin_tools/mod.rs \
        src/executor/builtin_registry/registry.rs
git commit -m "feat(memory): add session_complete builtin tool

G3-C LLM-sovereignty path: the model calls session_complete to mark a
self-contained task as done, triggering a retrospective extraction
via RawMemory(SessionEnd{TaskDone}). Does not terminate the
conversation — a signal, not a teardown."
```

---

## Task 10: Wire raw-memory writer dependencies at startup

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/handlers.rs` (or wherever the server builder assembles components — grep `SessionCompactor::new` to locate).
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init.rs` (for `A2ASubAgent` wiring).
- Modify: `src/bin/aleph-server/commands/start/builder/` (for `SessionManager` — grep for `SessionManager::new`).

- [ ] **Step 10.1: Locate constructor call sites**

Run:
```
grep -rn 'SessionCompactor::new\|A2ASubAgent::new\|SessionManager::new' /Volumes/TBU4/Workspace/Aleph/src/bin/ /Volumes/TBU4/Workspace/Aleph/src/
```
Record each hit.

- [ ] **Step 10.2: At each call site, pass the shared `Arc<dyn RawMemoryStore>`**

The server already constructs a shared `AlephSqliteStore` (grep for `AlephSqliteStore::new` if unsure). Upgrade its reference to `Arc<dyn RawMemoryStore>` once, then pass `Some(raw_memory_store.clone())` into each of the three constructors extended in Tasks 6–8.

For the `session_complete` tool's `ToolContext`, extend the builtin-registry registration in Task 9 Step 9.4 to fetch the same shared `Arc<dyn RawMemoryStore>` from the server builder's tool-context assembly (follow existing patterns used by other memory-related tools such as `recall_context` / `memory_search`).

- [ ] **Step 10.3: Build the server**

Run: `cargo check -p alephcore --bin aleph-server 2>&1 | tail -10`
Expected: no errors.

- [ ] **Step 10.4: Commit**

```bash
git add src/bin/aleph-server/
git commit -m "feat(memory): wire RawMemoryStore into capture-hook producers

Server builder now injects the shared Arc<dyn RawMemoryStore> into
SessionCompactor, A2ASubAgent, SessionManager, and the session_complete
tool context. Activates the Spec 1 capture hooks end-to-end."
```

---

## Task 11: End-to-end integration test

**Files:**
- Create: `tests/memory_capture_hooks.rs`

- [ ] **Step 11.1: Author the E2E test**

Create `tests/memory_capture_hooks.rs`:

```rust
//! Integration test: Spec 1 memory capture hooks end-to-end.
//!
//! Each case emits a RawMemory with a specific source, runs the
//! CompressionService once, and asserts that the resulting note
//! carries the expected category hint from the source's specialised
//! prompt (via a mock provider that echoes a pre-baked JSON response).

use alephcore::error::AlephError;
use alephcore::memory::compression::service::CompressionService;
use alephcore::memory::store::raw_memory::{
    RawMemory, RawMemorySource, RawMemoryStore, SessionEndReason,
};
use std::sync::Arc;

// Helper: build a fresh in-memory service + SQLite-backed raw store.
// The exact constructor call depends on the production builder.
// Follow the patterns in src/memory/compression/service.rs tests.
async fn build_service() -> CompressionService {
    todo!("wire up using existing test helpers in src/memory/compression/service.rs")
}

#[tokio::test]
async fn pre_compress_source_produces_rescue_categorised_notes() {
    let svc = build_service().await;
    let raw = RawMemory::new(
        "user: decided X\nassistant: noted.".into(),
        RawMemorySource::PreCompress,
    )
    .with_agent("agent-1");
    svc.insert_raw(&raw).await.unwrap();

    svc.run_once().await.unwrap();

    // Assert at least one note exists now with a rescue-friendly category.
    let notes = svc.list_notes_for_test("agent-1").await.unwrap();
    assert!(!notes.is_empty());
    assert!(notes
        .iter()
        .any(|n| ["preference", "plan", "learning", "project", "lesson"]
            .contains(&n.category.as_str())));
}

#[tokio::test]
async fn session_end_task_done_source_prefers_lesson_category() {
    let svc = build_service().await;
    let raw = RawMemory::new(
        "TASK_OUTCOME: shipped feature Y\nKEY_LEARNINGS:\n- L1".into(),
        RawMemorySource::SessionEnd {
            reason: SessionEndReason::TaskDone,
        },
    )
    .with_agent("agent-2");
    svc.insert_raw(&raw).await.unwrap();

    svc.run_once().await.unwrap();

    let notes = svc.list_notes_for_test("agent-2").await.unwrap();
    assert!(notes.iter().any(|n| n.category == "lesson"));
}
```

If `CompressionService::run_once` / `insert_raw` / `list_notes_for_test` helpers do not exist, add `#[cfg(any(test, feature = "test-helpers"))]` wrappers that expose the internal flow. The integration test's job is to run the pipeline end-to-end with a mocked provider, not to re-plumb production code.

- [ ] **Step 11.2: Run test**

Run: `cargo test --test memory_capture_hooks -- --nocapture 2>&1 | tail -40`
Expected: both tests pass.

- [ ] **Step 11.3: Commit**

```bash
git add tests/memory_capture_hooks.rs
git commit -m "test(memory): E2E integration test for capture hooks

Emits RawMemory rows for each Spec 1 source variant, runs
CompressionService once, and asserts the specialised prompt's
category guidance is honoured in the resulting notes."
```

---

## Task 12: Cleanup sweep

- [ ] **Step 12.1: Audit `signal_detector` / `trigger` for overlap**

Run: `grep -rn 'RawMemorySource' src/memory/compression/ src/memory/` to check whether `signal_detector.rs` or `trigger.rs` encode assumptions incompatible with the new variants (e.g., match arms with `_ => ...` that silently accept new variants, or hardcoded lists enumerating only legacy sources).

For each hit:
- If the code is compatible (uses `_` catch-all correctly), leave it.
- If it enumerates legacy sources and ignores new ones silently (hiding the hook's effect), extend the match explicitly.
- If two locations now do the same "should extraction fire?" decision (the new dispatcher in `service.rs` vs. legacy `signal_detector`), delete the duplicated side.

- [ ] **Step 12.2: Remove dead `RawMemorySource::from_str` callers**

Run: `grep -rn 'RawMemorySource::from_str' src/`
For every hit, replace with `from_persisted(token, detail)` unless the caller genuinely only has a token and the detail is correctly defaulted (keep the legacy shim for those — but add a `// legacy path: detail is None` comment).

- [ ] **Step 12.3: Commit cleanup**

```bash
git add -A
git commit -m "refactor(memory): cleanup after Spec 1 capture hooks

Audit signal_detector/trigger for new-variant compatibility and
remove dead RawMemorySource::from_str callers where from_persisted
is the right call. No behavior change in compatible paths."
```

---

## Task 13: Update the Spec 1 roadmap row + NOTES.md pointer

**Files:**
- Modify: `docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md`
- Modify: `docs/reference/memory/RAW_MEMORY.md` (add a short section "Capture hooks" linking to the spec)

- [ ] **Step 13.1: Roadmap row**

In `docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md`, change the Spec 1 status from `🟡 brainstorming` to `✅ shipped` (with today's date) and link design doc + plan doc.

- [ ] **Step 13.2: RAW_MEMORY.md pointer**

Add a subsection to `docs/reference/memory/RAW_MEMORY.md`:

```markdown
## 7.2 Capture Hooks (Spec 1)

Three additional producers feed `raw_memories`:

- `PreCompress` — emitted by `SessionCompactor::replace_with_summary` before a chunk is dropped.
- `Delegation { child_agent_id }` — emitted by `A2ASubAgent::execute` when a sub-agent returns.
- `SessionEnd { reason }` — emitted by `SessionManager::close_session` (`Disconnect`) and the `session_complete` tool (`TaskDone`).

Each source routes through a specialised system prompt via
`memory::compression::source_prompts::prompt_for`. See
`docs/superpowers/specs/2026-04-13-memory-evolution-spec1-capture-hooks-design.md`.
```

- [ ] **Step 13.3: Commit**

```bash
git add docs/
git commit -m "docs(memory): mark Spec 1 shipped and add RAW_MEMORY pointer"
```

---

## Self-Review

After the final commit, verify:

1. **Spec coverage** — every spec section (Architecture / Data model / Trigger points / Extraction prompts / session_complete tool / Cleanup / Testing / Redlines / Open questions) maps to at least one task above. The four open questions from spec §11:
   - **Tail token budget** → Task 8 uses a fixed 64-message tail; Task 9 takes outcome + learnings as LLM-decided payload. Configurable tuning is Spec 2/3 work.
   - **`PreCompress` priority in CompressionService** → Task 5's `group_by_source` gives each source its own LLM call; within a run the order is insertion order (the call chain processes all groups). A priority field is Spec 2-ready polish, not needed here.
   - **Deduplication** → Task 5's per-source extractor call naturally avoids duplicate rows because each `raw_memories` row is processed exactly once (`mark_raw_as_processed` on the same IDs).
   - **G2 exact file** → confirmed as `src/a2a/sub_agent.rs` in Task 7 via explorer report.

2. **Placeholder scan** — no `TBD`, `FIXME`, or "implement later" remain. The `todo!()` in Tasks 7/8 tests is conditional on the codebase already having a harness; each has a concrete fallback (extract a helper fn and test that instead) so the plan is executable either way.

3. **Type consistency**
   - `RawMemorySource::PreCompress` / `Delegation { child_agent_id }` / `SessionEnd { reason }` used consistently across Tasks 1, 3, 4, 5, 6, 7, 8, 9, 11.
   - `SessionEndReason::{Disconnect, TaskDone}` used consistently in Tasks 1, 8, 9, 11.
   - `to_persisted` / `from_persisted` method names used consistently in Tasks 1, 2, 12.
   - `prompt_for(&RawMemorySource) -> Option<&'static str>` used consistently in Tasks 3, 4.
   - `extract_note_updates_for_source(memories, existing_titles, source)` used consistently in Tasks 4, 5.
   - `TOOL_NAME = "session_complete"` consistent in Task 9.

---

**Plan complete and saved to `docs/superpowers/plans/2026-04-13-memory-evolution-spec1-capture-hooks.md`.**
