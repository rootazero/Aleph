# Stage J-pre — Cache Observability Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Establish the trace-side cache-token observability that Stage J's "≥2 weeks of cache miss data" precondition requires. Add the missing `cache_creation_tokens` field; fix the streaming protocol's silent-drop of `message_start` usage; wire a `MeteringProvider` decorator that emits a per-call `ProviderUsage` trace event labelled with `agent_id` so root vs subagent cache-hit ratios become measurable.

**Architecture:** Pure decorator pattern + schema-only trace extension. The decorator (`MeteringProvider`) wraps `Arc<dyn AiProvider>`, intercepts `process()`, and pushes `LoopTraceEvent::ProviderUsage` into the existing `TraceSink`. Two wrap sites: root-provider construction in `orchestrator_init.rs` and the per-spawn provider in `subagent_spawner.rs::spawn`. **R10 redline preserved**: `src/harness/agent.rs` 0 diff vs `f891cc71b` (Stage I closure); `trace.rs` gains one schema-only enum variant + `From` arm (~14 lines, same pattern as Stage H/I).

**Tech Stack:** Rust, `tokio`, `serde`, `aleph_protocol`. No new dependencies.

**Out-of-scope (explicit):**
- Stage J fork branch (`AgentDef::inherit_parent_prompt`, prompt-prefix byte-equal mode) — deferred until 2026-05-23 when ≥2 weeks of trace data are accumulated
- Fixing `total_tokens: 0` placeholders in `harness/agent.rs` TurnMetrics (would violate R10 redline; the new `ProviderUsage` event sidecars the missing data)
- Cost dashboard / aggregation UI
- Non-Anthropic provider cache-token extraction (Anthropic is the only protocol that exposes cache fields; others stay `None` until they extend their APIs)

**R10 verification (pre-flight & per-task)**:
```bash
git diff f891cc71b -- src/harness/agent.rs | wc -l   # must stay 0
ls src/harness/*.rs | wc -l                           # must stay 10
```

**Pre-conditions confirmed during reconnaissance:**
- `TokenUsage` at `src/providers/adapter.rs:268-274` has `input_tokens`, `output_tokens`, `cache_read_tokens`, `thinking_tokens`. Missing `cache_creation_tokens`.
- `AnthropicUsage` at `src/providers/anthropic/types.rs:192-199` has 3 fields. Missing `cache_creation_input_tokens`.
- Streaming protocol at `src/providers/protocols/anthropic.rs:822-832` (`message_delta`) extracts `cache_read_input_tokens` only.
- Streaming protocol at `src/providers/protocols/anthropic.rs:860-863` (`message_start`) is **completely unhandled** — `input_tokens` + `cache_creation_input_tokens` both silently dropped on streaming path.
- `AiProvider` trait surface: `process<'a>(&'a self, req: RequestPayload<'a>) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>>`, `fn name(&self) -> &str`, `fn color(&self) -> &str`.
- `TraceSink` trait: `on_trace(&self, event: &LoopTraceEvent)`, `flush(&self)`, `on_init_seam(...)` (default no-op).
- `subagent_spawner.rs:271` resolves the per-spawn `Arc<dyn AiProvider>` — wrap site for the subagent label.
- `src/bin/aleph-server/commands/start/orchestrator_init.rs:47` has `default_provider: Arc<dyn alephcore::providers::AiProvider>` — root wrap site is wherever this struct is constructed (implementer to locate; ~50 lines around).
- `LoopTraceEvent` enum at `src/harness/trace.rs:12-64` and `AgentTraceEvent` at `shared/protocol/src/events.rs:232-302` are the schema-mirror pair.

---

## File Structure

| Path | Action | Responsibility |
|---|---|---|
| `src/providers/adapter.rs` | Modify (+1 field, +tests) | `TokenUsage` schema |
| `src/providers/anthropic/types.rs` | Modify (+1 field, +tests) | Non-streaming usage parse |
| `src/providers/anthropic/parser.rs` *or wherever maps to TokenUsage* | Modify | Map cache_creation_input_tokens → cache_creation_tokens |
| `src/providers/protocols/anthropic.rs` | Modify (+message_start handler, +cache_creation in delta) | Streaming usage extraction |
| `src/harness/trace.rs` | Modify (+1 variant, +1 From arm, +2 unit tests) | New `ProviderUsage` event |
| `shared/protocol/src/events.rs` | Modify (+1 variant, +1 kind() arm) | Cross-process schema mirror |
| `src/harness/loop_callback.rs` | Modify (exhaustive match update) | Compile after enum expansion |
| `src/harness/tests/stability.rs` | Modify (exhaustive match update) | Compile after enum expansion |
| `shared/protocol/src/trace_presentation.rs` | Modify (exhaustive match update) | Compile after enum expansion |
| `src/providers/metering.rs` | **CREATE** (~80 LOC) | `MeteringProvider` decorator + unit test |
| `src/providers/mod.rs` | Modify (+pub mod metering, +re-export) | Module wiring |
| `src/agents/subagent_spawner.rs` | Modify (~5 LOC) | Wrap per-spawn provider with subagent_id label |
| `src/bin/aleph-server/commands/start/orchestrator_init.rs` | Modify (~5 LOC) | Wrap root provider with "root" label |
| `tests/cache_observability_smoke.rs` | **CREATE** (~120 LOC) | End-to-end smoke: subagent + root each emit `ProviderUsage` |
| `docs/reference/MULTI_AGENT_SYSTEM.md` | Modify (+section ~60 LOC) | Document the cache observability pipeline |
| `docs/superpowers/specs/2026-05-08-subagent-uplift-roadmap-design.md` | Modify (top + Stage J entry) | Roadmap status |

---

## Task 1: Add `cache_creation_tokens` field to `TokenUsage`

**Files:**
- Modify: `src/providers/adapter.rs:268-274` (struct), `:276-302` (test module)

- [ ] **Step 1: Write the failing test**

In the existing `mod tests` block of `src/providers/adapter.rs`, add:

```rust
    #[test]
    fn token_usage_carries_cache_creation_tokens() {
        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: Some(80),
            cache_creation_tokens: Some(20),
            thinking_tokens: None,
        };
        assert_eq!(usage.cache_creation_tokens, Some(20));
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p alephcore --lib providers::adapter::tests::token_usage_carries_cache_creation_tokens 2>&1 | tail -20
```
Expected: compile FAIL with "no field `cache_creation_tokens` on type `TokenUsage`".

- [ ] **Step 3: Add the field**

Modify `src/providers/adapter.rs:268-274`:

```rust
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: Option<u32>,
    /// Anthropic `cache_creation_input_tokens`: tokens written *into* the
    /// prompt cache on this call. Cost-relevant for Stage J fork-branch
    /// decision-making.
    pub cache_creation_tokens: Option<u32>,
    pub thinking_tokens: Option<u32>,
}
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test -p alephcore --lib providers::adapter::tests:: 2>&1 | tail -5
```
Expected: PASS for `token_usage_carries_cache_creation_tokens`.

- [ ] **Step 5: Update existing struct literals**

Run:
```bash
grep -rn "TokenUsage {" /Volumes/TBU4/Workspace/Aleph/src 2>/dev/null
```

Add `cache_creation_tokens: None,` to every literal that compiler errors point at. Expect ~5–8 sites (delta.rs tests, openai_chat.rs, gemini.rs, openai_responses.rs, anthropic protocol). Run:
```bash
cargo build -p alephcore 2>&1 | tail -10
```
Expected: clean build.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
providers: add cache_creation_tokens to TokenUsage (Stage J-pre)

Adds the missing field so trace events can carry Anthropic's
cache_creation_input_tokens. All non-Anthropic providers leave it
None; Anthropic mapping lands in subsequent task.
EOF
)"
```

---

## Task 2: Extend non-streaming `AnthropicUsage` + map cache_creation through to `TokenUsage`

**Files:**
- Modify: `src/providers/anthropic/types.rs:190-199` (struct), `:280-330` (existing tests block)
- Modify: wherever `AnthropicUsage` → `TokenUsage` mapping happens (search: `cache_read_tokens: usage.cache_read_input_tokens` or `into() for AnthropicUsage`)

- [ ] **Step 1: Write the failing test**

Append to `src/providers/anthropic/types.rs` test module:

```rust
    #[test]
    fn anthropic_usage_parses_cache_creation_input_tokens() {
        let json = serde_json::json!({
            "input_tokens": 200,
            "output_tokens": 100,
            "cache_read_input_tokens": 150,
            "cache_creation_input_tokens": 50
        });
        let usage: AnthropicUsage = serde_json::from_value(json).unwrap();
        assert_eq!(usage.cache_creation_input_tokens, Some(50));
    }
```

- [ ] **Step 2: Run test (expect fail — missing field)**

```bash
cargo test -p alephcore --lib anthropic::types::tests::anthropic_usage_parses_cache_creation_input_tokens 2>&1 | tail -10
```
Expected: compile FAIL.

- [ ] **Step 3: Add the field**

Modify `src/providers/anthropic/types.rs:192-199`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicUsage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u32>,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u32>,
}
```

- [ ] **Step 4: Find the `AnthropicUsage → TokenUsage` mapping**

```bash
grep -rn "cache_read_input_tokens\|AnthropicUsage" /Volumes/TBU4/Workspace/Aleph/src/providers/anthropic 2>/dev/null
```

Identify the conversion point (likely a `From<AnthropicUsage>` impl or a parse function in `src/providers/anthropic/parser.rs` or `src/providers/anthropic/mod.rs`). Add `cache_creation_tokens: usage.cache_creation_input_tokens` to that mapping.

- [ ] **Step 5: Run tests + build**

```bash
cargo test -p alephcore --lib anthropic:: 2>&1 | tail -10
cargo build -p alephcore 2>&1 | tail -5
```
Expected: all pass; build clean.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "providers/anthropic: parse cache_creation_input_tokens (non-streaming, Stage J-pre)"
```

---

## Task 3: Fix streaming protocol — extract `message_start` usage + `cache_creation` in `message_delta`

**Files:**
- Modify: `src/providers/protocols/anthropic.rs:807-863` (message_delta + message_start handlers)
- Modify: existing test module in same file (likely `mod tests` near bottom)

- [ ] **Step 1: Write the failing tests**

Append to the test module of `src/providers/protocols/anthropic.rs` (locate the existing `mod tests` — there is one around line 870):

```rust
    #[test]
    fn message_start_emits_input_tokens_and_cache_creation() {
        let event = serde_json::json!({
            "type": "message_start",
            "message": {
                "id": "msg_01",
                "type": "message",
                "role": "assistant",
                "content": [],
                "usage": {
                    "input_tokens": 250,
                    "output_tokens": 0,
                    "cache_creation_input_tokens": 30,
                    "cache_read_input_tokens": 100
                }
            }
        });
        let mut deltas = std::collections::VecDeque::new();
        AnthropicProtocol::dispatch_stream_event(&event, &mut deltas, &mut Vec::new());
        let usage_delta = deltas.iter().find_map(|d| match d {
            Ok(ProviderDelta::Usage(u)) => Some(u.clone()),
            _ => None,
        }).expect("message_start should emit ProviderDelta::Usage");
        assert_eq!(usage_delta.input_tokens, 250);
        assert_eq!(usage_delta.cache_creation_tokens, Some(30));
        assert_eq!(usage_delta.cache_read_tokens, Some(100));
    }

    #[test]
    fn message_delta_emits_cache_creation_tokens() {
        let event = serde_json::json!({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn"},
            "usage": {
                "output_tokens": 75,
                "cache_creation_input_tokens": 5
            }
        });
        let mut deltas = std::collections::VecDeque::new();
        AnthropicProtocol::dispatch_stream_event(&event, &mut deltas, &mut Vec::new());
        let usage_delta = deltas.iter().find_map(|d| match d {
            Ok(ProviderDelta::Usage(u)) => Some(u.clone()),
            _ => None,
        }).expect("message_delta should emit ProviderDelta::Usage");
        assert_eq!(usage_delta.cache_creation_tokens, Some(5));
    }
```

> **Implementer note:** the dispatcher function name may differ. Search for the function that the existing streaming-test infrastructure exercises (look for `message_delta` string-match dispatch). If the existing tests use a different harness, adapt the test invocation to match. Keep the assertions identical.

- [ ] **Step 2: Run tests (expect fail)**

```bash
cargo test -p alephcore --lib providers::protocols::anthropic 2>&1 | tail -20
```
Expected: 2 new tests FAIL (message_start because it's unhandled; message_delta because cache_creation isn't extracted).

- [ ] **Step 3: Implement message_start usage extraction**

Replace the catch-all at `src/providers/protocols/anthropic.rs:860-863`:

```rust
        // ── message_start ──────────────────────────────────────────────────────
        "message_start" => {
            if let Some(usage) = v.get("message").and_then(|m| m.get("usage")) {
                let input = usage.get("input_tokens").and_then(|t| t.as_u64()).and_then(|t| t.try_into().ok()).unwrap_or(0);
                let output = usage.get("output_tokens").and_then(|t| t.as_u64()).and_then(|t| t.try_into().ok()).unwrap_or(0);
                let cache_read = usage.get("cache_read_input_tokens").and_then(|t| t.as_u64()).and_then(|t| t.try_into().ok());
                let cache_creation = usage.get("cache_creation_input_tokens").and_then(|t| t.as_u64()).and_then(|t| t.try_into().ok());
                out.push_back(Ok(ProviderDelta::Usage(TokenUsage {
                    input_tokens: input,
                    output_tokens: output,
                    cache_read_tokens: cache_read,
                    cache_creation_tokens: cache_creation,
                    thinking_tokens: None,
                })));
            }
        }

        // ── ping / other ───────────────────────────────────────────────────────
        _ => {
            // ignore
        }
```

- [ ] **Step 4: Add cache_creation extraction to message_delta**

Modify `src/providers/protocols/anthropic.rs:822-832` (inside the `message_delta` arm):

```rust
                let cache_read = usage
                    .get("cache_read_input_tokens")
                    .and_then(|t| t.as_u64())
                    .and_then(|t| t.try_into().ok());
                let cache_creation = usage
                    .get("cache_creation_input_tokens")
                    .and_then(|t| t.as_u64())
                    .and_then(|t| t.try_into().ok());
                out.push_back(Ok(ProviderDelta::Usage(TokenUsage {
                    input_tokens: 0,
                    output_tokens: output,
                    cache_read_tokens: cache_read,
                    cache_creation_tokens: cache_creation,
                    thinking_tokens: None,
                })));
```

- [ ] **Step 5: Run tests + build**

```bash
cargo test -p alephcore --lib providers::protocols::anthropic 2>&1 | tail -10
cargo build -p alephcore 2>&1 | tail -5
```
Expected: 2 new tests PASS; existing tests stay green; build clean.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "providers/anthropic: stream protocol extracts message_start usage + cache_creation (Stage J-pre)"
```

---

## Task 4: Add `LoopTraceEvent::ProviderUsage` schema-only variant

**Files:**
- Modify: `src/harness/trace.rs:12-64` (enum + From bridge), test module at `:296-322`
- Modify: `shared/protocol/src/events.rs:232-302` (mirror enum + kind() match)
- Modify: `src/harness/loop_callback.rs`, `src/harness/tests/stability.rs`, `shared/protocol/src/trace_presentation.rs` (any `match LoopTraceEvent` or `match AgentTraceEvent` exhaustive sites)

- [ ] **Step 1: Write the failing test**

Append to the test module at `src/harness/trace.rs:296-322`:

```rust
    #[test]
    fn provider_usage_serializes_with_agent_id_and_token_split() {
        let event = LoopTraceEvent::ProviderUsage {
            agent_id: "subagent-foo".into(),
            input_tokens: 250,
            output_tokens: 75,
            cache_read_tokens: Some(100),
            cache_creation_tokens: Some(30),
            thinking_tokens: None,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains(r#""type":"provider_usage""#));
        assert!(json.contains(r#""agent_id":"subagent-foo""#));
        assert!(json.contains(r#""cache_creation_tokens":30"#));
        assert!(json.contains(r#""cache_read_tokens":100"#));
    }
```

- [ ] **Step 2: Run test (expect fail)**

```bash
cargo test -p alephcore --lib harness::trace::tests::provider_usage 2>&1 | tail -10
```
Expected: compile FAIL.

- [ ] **Step 3: Add the variant to `LoopTraceEvent`**

In `src/harness/trace.rs`, add as the last variant of `LoopTraceEvent` (after `McpScopeCleaned`):

```rust
    /// Per-call provider usage (Stage J-pre cache observability).
    /// `agent_id` is "root" for the top-level harness or the subagent_id
    /// when emitted from within a spawned subagent.
    ProviderUsage {
        agent_id: String,
        input_tokens: u32,
        output_tokens: u32,
        cache_read_tokens: Option<u32>,
        cache_creation_tokens: Option<u32>,
        thinking_tokens: Option<u32>,
    },
```

- [ ] **Step 4: Mirror in `AgentTraceEvent`**

In `shared/protocol/src/events.rs:232-302`, add the matching variant to `AgentTraceEvent` and a `kind()` arm `Self::ProviderUsage { .. } => "provider_usage"`.

- [ ] **Step 5: Bridge `From<LoopTraceEvent> for AgentTraceEvent`**

In `src/harness/trace.rs:133-235`, add a new arm before the closing `}`:

```rust
            LoopTraceEvent::ProviderUsage {
                agent_id,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                thinking_tokens,
            } => aleph_protocol::AgentTraceEvent::ProviderUsage {
                agent_id,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                thinking_tokens,
            },
```

- [ ] **Step 6: Update any exhaustive matches that the compiler now flags**

```bash
cargo build -p alephcore 2>&1 | tail -30
```
Expect "non-exhaustive patterns" errors in `loop_callback.rs`, `tests/stability.rs`, `shared/protocol/src/trace_presentation.rs`. For each, add a no-op or trivial arm — these are schema-mirror sites, not authority/cognition (R10-safe). Example arm pattern:
```rust
LoopTraceEvent::ProviderUsage { .. } => { /* observability passthrough */ }
```

- [ ] **Step 7: Run tests + build**

```bash
cargo test -p alephcore --lib harness::trace 2>&1 | tail -10
cargo build -p alephcore 2>&1 | tail -5
git diff f891cc71b -- src/harness/agent.rs | wc -l   # MUST be 0
ls src/harness/*.rs | wc -l                            # MUST be 10
```
Expected: trace tests PASS, build clean, R10 verifier outputs 0 + 10.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "harness/trace + protocol: add ProviderUsage schema variant (Stage J-pre)"
```

---

## Task 5: Implement `MeteringProvider` decorator

**Files:**
- Create: `src/providers/metering.rs` (~80 LOC)
- Modify: `src/providers/mod.rs` (`pub mod metering;` + re-export)

- [ ] **Step 1: Write the failing unit test**

Create `src/providers/metering.rs` with:

```rust
//! `MeteringProvider` — decorator that emits `LoopTraceEvent::ProviderUsage`
//! after each `process()` call (Stage J-pre cache observability pipeline).
//!
//! Decorator-only: no harness diff. Composes with any `AiProvider` (anthropic,
//! mock, failover, etc.). Non-Anthropic providers will populate `cache_*` as
//! `None` until their protocols extend.
//!
//! See: docs/superpowers/plans/2026-05-09-subagent-uplift-stage-j-pre-plan.md

use crate::error::Result;
use crate::harness::trace::LoopTraceEvent;
use crate::harness::TraceSink;
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::AiProvider;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub struct MeteringProvider {
    inner: Arc<dyn AiProvider>,
    sink: Option<Arc<dyn TraceSink>>,
    agent_id: String,
}

impl MeteringProvider {
    pub fn new(
        inner: Arc<dyn AiProvider>,
        sink: Option<Arc<dyn TraceSink>>,
        agent_id: impl Into<String>,
    ) -> Self {
        Self {
            inner,
            sink,
            agent_id: agent_id.into(),
        }
    }
}

impl AiProvider for MeteringProvider {
    fn process<'a>(
        &'a self,
        req: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        let fut = self.inner.process(req);
        let sink = self.sink.clone();
        let agent_id = self.agent_id.clone();
        Box::pin(async move {
            let resp = fut.await?;
            if let (Some(sink), Some(usage)) = (sink, resp.usage.as_ref()) {
                sink.on_trace(&LoopTraceEvent::ProviderUsage {
                    agent_id,
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    cache_read_tokens: usage.cache_read_tokens,
                    cache_creation_tokens: usage.cache_creation_tokens,
                    thinking_tokens: usage.thinking_tokens,
                });
            }
            Ok(resp)
        })
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn color(&self) -> &str {
        self.inner.color()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::adapter::TokenUsage;
    use std::sync::Mutex;

    struct FakeProvider {
        usage: TokenUsage,
    }
    impl AiProvider for FakeProvider {
        fn process<'a>(
            &'a self,
            _req: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
            let usage = self.usage.clone();
            Box::pin(async move {
                Ok(ProviderResponse {
                    usage: Some(usage),
                    ..Default::default()
                })
            })
        }
        fn name(&self) -> &str { "fake" }
        fn color(&self) -> &str { "#000" }
    }

    struct CapturingSink(Mutex<Vec<LoopTraceEvent>>);
    impl TraceSink for CapturingSink {
        fn on_trace(&self, event: &LoopTraceEvent) {
            self.0.lock().unwrap().push(event.clone());
        }
        fn flush(&self) {}
    }

    #[tokio::test]
    async fn emits_provider_usage_with_agent_id_and_full_token_split() {
        let inner = Arc::new(FakeProvider {
            usage: TokenUsage {
                input_tokens: 200,
                output_tokens: 50,
                cache_read_tokens: Some(150),
                cache_creation_tokens: Some(20),
                thinking_tokens: None,
            },
        });
        let sink = Arc::new(CapturingSink(Mutex::new(Vec::new())));
        let metering = MeteringProvider::new(
            inner,
            Some(sink.clone() as Arc<dyn TraceSink>),
            "subagent-test",
        );

        let msgs = [crate::providers::message::UnifiedMessage::user("hi")];
        let req = RequestPayload::new(&msgs);
        let _ = metering.process(req).await.expect("process");

        let events = sink.0.lock().unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            LoopTraceEvent::ProviderUsage {
                agent_id,
                input_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                ..
            } => {
                assert_eq!(agent_id, "subagent-test");
                assert_eq!(*input_tokens, 200);
                assert_eq!(*cache_read_tokens, Some(150));
                assert_eq!(*cache_creation_tokens, Some(20));
            }
            other => panic!("expected ProviderUsage, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn no_event_when_response_lacks_usage() {
        struct EmptyProvider;
        impl AiProvider for EmptyProvider {
            fn process<'a>(
                &'a self,
                _req: RequestPayload<'a>,
            ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
                Box::pin(async { Ok(ProviderResponse::default()) })
            }
            fn name(&self) -> &str { "empty" }
            fn color(&self) -> &str { "#000" }
        }
        let sink = Arc::new(CapturingSink(Mutex::new(Vec::new())));
        let metering = MeteringProvider::new(
            Arc::new(EmptyProvider),
            Some(sink.clone() as Arc<dyn TraceSink>),
            "x",
        );
        let msgs = [crate::providers::message::UnifiedMessage::user("hi")];
        let _ = metering.process(RequestPayload::new(&msgs)).await.unwrap();
        assert!(sink.0.lock().unwrap().is_empty());
    }
}
```

- [ ] **Step 2: Wire the module**

Add to `src/providers/mod.rs`:
```rust
pub mod metering;
pub use metering::MeteringProvider;
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p alephcore --lib providers::metering 2>&1 | tail -15
```
Expected: 2/2 PASS.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "providers: MeteringProvider decorator emits ProviderUsage trace (Stage J-pre)"
```

---

## Task 6: Wire `MeteringProvider` in `subagent_spawner`

**Files:**
- Modify: `src/agents/subagent_spawner.rs:271` area (where `llm: Arc<dyn AiProvider>` is resolved)

- [ ] **Step 1: Write the failing test**

Add a new integration test inside the existing test module of `src/agents/subagent_spawner.rs` (search for `#[cfg(test)] mod tests` near the bottom). The mock provider already exists in tests; reuse it. New test:

```rust
    #[tokio::test]
    async fn subagent_spawn_emits_provider_usage_with_agent_id() {
        // Reuse SingleCallProvider or AlwaysToolCallProvider from existing tests;
        // attach a CapturingSink to base.trace_sink and assert that after spawn
        // resolves, the captured events include LoopTraceEvent::ProviderUsage
        // with agent_id == req.agent_def.id (NOT "root").
        //
        // If existing test mocks don't return Some(usage), extend one of them
        // (or define a new tiny mock inline) to return ProviderResponse with
        // usage: Some(TokenUsage { input_tokens: 10, .. }).
    }
```

> **Implementer note:** the exact mock setup mirrors `tests/cancellation_chain.rs:178+212` SpawnerBase pattern from Stage D. If the existing test mocks all return `ProviderResponse::text_only(...)` (which has `usage: None`), define a small inline `UsageProvider` that returns a `ProviderResponse` with `usage: Some(...)`.

- [ ] **Step 2: Run test (expect fail)**

```bash
cargo test -p alephcore --lib agents::subagent_spawner::tests::subagent_spawn_emits_provider_usage 2>&1 | tail -10
```
Expected: FAIL — no `ProviderUsage` events captured (because spawner doesn't wrap yet).

- [ ] **Step 3: Wrap the resolved provider**

Modify `src/agents/subagent_spawner.rs` around line 271 (where `let llm: Arc<dyn AiProvider> = match resolved_model { ... };` ends). After `llm` is resolved, before it's passed into `HarnessDeps`:

```rust
let llm: Arc<dyn AiProvider> = Arc::new(crate::providers::MeteringProvider::new(
    llm,
    base.trace_sink.clone(),
    req.agent_def.id.clone(),
));
```

> **Implementer note:** confirm the variable name (`llm` vs other) and the `agent_def.id` field name by reading nearby context. The exact field that produces a stable subagent identifier is `req.agent_def.id` — verify it matches the existing struct shape.

- [ ] **Step 4: Run tests + build + R10 check**

```bash
cargo test -p alephcore --lib agents::subagent_spawner 2>&1 | tail -10
cargo build -p alephcore 2>&1 | tail -5
git diff f891cc71b -- src/harness/agent.rs | wc -l   # MUST be 0
```
Expected: all tests pass, build clean, R10 0.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "agents/spawner: wrap subagent provider with MeteringProvider (Stage J-pre)"
```

---

## Task 7: Wire `MeteringProvider` at root provider construction

**Files:**
- Modify: `src/bin/aleph-server/commands/start/orchestrator_init.rs` — locate `default_provider` construction (search for `default_provider:` field assignment, around line 47 area)

- [ ] **Step 1: Locate the construction site**

```bash
grep -n "default_provider" /Volumes/TBU4/Workspace/Aleph/src/bin/aleph-server/commands/start/orchestrator_init.rs
```

The first match (line 47) is the field declaration. Search for where `default_provider:` is assigned in a struct literal — that is the wrap site. There may be multiple branches (anthropic / mock / fallback); wrap each.

- [ ] **Step 2: Wrap the provider**

For each construction site, replace:
```rust
default_provider: provider_arc,
```
with:
```rust
default_provider: Arc::new(alephcore::providers::MeteringProvider::new(
    provider_arc,
    Some(trace_sink.clone()),
    "root",
)),
```

> **Implementer note:** the local variable holding the trace_sink may be named differently (`gateway_trace_sink`, `noop_sink`, etc.). Use whatever the existing code already passes into the orchestrator's `trace_sink` field — that is the same sink the harness uses, which is what we want for the events to land in the same stream.

- [ ] **Step 3: Build**

```bash
cargo build -p alephcore --bin aleph-server 2>&1 | tail -10
```
Expected: clean build.

- [ ] **Step 4: Run a smoke check**

If a server-startup integration test exists (search `tests/orchestrator` or similar), run it:
```bash
cargo test --workspace orchestrator 2>&1 | tail -10
```
Expected: existing tests stay green (the wrap is transparent unless usage data is emitted).

- [ ] **Step 5: R10 check + commit**

```bash
git diff f891cc71b -- src/harness/agent.rs | wc -l   # MUST be 0
git add -A
git commit -m "aleph-server/orchestrator: wrap root provider with MeteringProvider (Stage J-pre)"
```

---

## Task 8: End-to-end smoke integration test

**Files:**
- Create: `tests/cache_observability_smoke.rs`

- [ ] **Step 1: Write the test**

```rust
//! Stage J-pre — cache observability smoke.
//!
//! Asserts that:
//! 1. A `MeteringProvider` wrapping a provider that returns Some(TokenUsage)
//!    causes a `LoopTraceEvent::ProviderUsage` to land on the trace sink.
//! 2. The event carries the configured agent_id and the full token split.
//!
//! This is the intentional cheap smoke — real-LLM cache_read/creation
//! verification is in the manual checklist on the PR description.

use alephcore::error::Result;
use alephcore::harness::trace::LoopTraceEvent;
use alephcore::harness::TraceSink;
use alephcore::providers::adapter::{ProviderResponse, RequestPayload, TokenUsage};
use alephcore::providers::message::UnifiedMessage;
use alephcore::providers::{AiProvider, MeteringProvider};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

struct CannedUsageProvider {
    usage: TokenUsage,
}

impl AiProvider for CannedUsageProvider {
    fn process<'a>(
        &'a self,
        _req: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        let usage = self.usage.clone();
        Box::pin(async move {
            Ok(ProviderResponse {
                usage: Some(usage),
                ..Default::default()
            })
        })
    }
    fn name(&self) -> &str { "canned-usage" }
    fn color(&self) -> &str { "#000" }
}

struct CapturingSink(Mutex<Vec<LoopTraceEvent>>);
impl TraceSink for CapturingSink {
    fn on_trace(&self, event: &LoopTraceEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
    fn flush(&self) {}
}

#[tokio::test]
async fn root_label_emits_provider_usage_event() {
    let inner = Arc::new(CannedUsageProvider {
        usage: TokenUsage {
            input_tokens: 1000,
            output_tokens: 200,
            cache_read_tokens: Some(800),
            cache_creation_tokens: Some(50),
            thinking_tokens: None,
        },
    });
    let sink = Arc::new(CapturingSink(Mutex::new(Vec::new())));
    let metering = MeteringProvider::new(
        inner,
        Some(sink.clone() as Arc<dyn TraceSink>),
        "root",
    );

    let msgs = [UnifiedMessage::user("hi")];
    let _ = metering.process(RequestPayload::new(&msgs)).await.expect("process");

    let events = sink.0.lock().unwrap();
    assert_eq!(events.len(), 1);
    match &events[0] {
        LoopTraceEvent::ProviderUsage { agent_id, cache_read_tokens, cache_creation_tokens, .. } => {
            assert_eq!(agent_id, "root");
            assert_eq!(*cache_read_tokens, Some(800));
            assert_eq!(*cache_creation_tokens, Some(50));
        }
        other => panic!("expected ProviderUsage, got {other:?}"),
    }
}

#[tokio::test]
async fn subagent_label_distinguishes_from_root() {
    let inner = Arc::new(CannedUsageProvider {
        usage: TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            thinking_tokens: None,
        },
    });
    let sink = Arc::new(CapturingSink(Mutex::new(Vec::new())));
    let metering = MeteringProvider::new(
        inner,
        Some(sink.clone() as Arc<dyn TraceSink>),
        "subagent-research",
    );
    let msgs = [UnifiedMessage::user("hi")];
    let _ = metering.process(RequestPayload::new(&msgs)).await.unwrap();

    let events = sink.0.lock().unwrap();
    let agent_ids: Vec<_> = events.iter().filter_map(|e| match e {
        LoopTraceEvent::ProviderUsage { agent_id, .. } => Some(agent_id.clone()),
        _ => None,
    }).collect();
    assert_eq!(agent_ids, vec!["subagent-research".to_string()]);
}

#[tokio::test]
async fn no_sink_means_no_panic_no_event() {
    let inner = Arc::new(CannedUsageProvider {
        usage: TokenUsage {
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            thinking_tokens: None,
        },
    });
    let metering = MeteringProvider::new(inner, None, "root");
    let msgs = [UnifiedMessage::user("hi")];
    let resp = metering.process(RequestPayload::new(&msgs)).await.expect("process");
    assert!(resp.usage.is_some());
}
```

- [ ] **Step 2: Run the test**

```bash
cargo test --test cache_observability_smoke 2>&1 | tail -15
```
Expected: 3/3 PASS.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "tests: cache_observability_smoke (Stage J-pre)"
```

---

## Task 9: Documentation + roadmap closure

**Files:**
- Modify: `docs/reference/MULTI_AGENT_SYSTEM.md` (append new section)
- Modify: `docs/superpowers/specs/2026-05-08-subagent-uplift-roadmap-design.md` (top status line + Stage J entry annotation)

- [ ] **Step 1: Append section to `MULTI_AGENT_SYSTEM.md`**

Add a new section near the end:

```markdown
## Cache Observability Pipeline (Stage J-pre)

The `MeteringProvider` decorator (`src/providers/metering.rs`) wraps every
LLM-facing `Arc<dyn AiProvider>` and emits a `LoopTraceEvent::ProviderUsage`
event after each `process()` call. The event carries:

- `agent_id` — `"root"` for the top-level harness, or the subagent's
  `agent_def.id` when emitted from within a spawned subagent
- `input_tokens` / `output_tokens` — total tokens charged
- `cache_read_tokens` / `cache_creation_tokens` — Anthropic prompt-cache
  fields (other providers leave these `None` until they extend their
  protocols)
- `thinking_tokens` — Gemini extended-thinking tokens (where applicable)

The decorator is wrapped at exactly two sites:

- `src/bin/aleph-server/commands/start/orchestrator_init.rs` — root
  provider, label `"root"`
- `src/agents/subagent_spawner.rs` — per-spawn, label `req.agent_def.id`

This gives every consumer of the trace stream (gateway, log sink, future
cost dashboard) the data needed to compute root vs subagent cache-hit
ratios. Stage J's "fork branch" decision is gated on collecting ≥2 weeks
of this data starting from the J-pre ship date — see roadmap § 1.2 Stage J.

R10 redline preserved: the decorator does not touch `src/harness/agent.rs`;
the `LoopTraceEvent::ProviderUsage` variant is schema-only (mirrors into
`AgentTraceEvent`). The harness loop remains unchanged.
```

- [ ] **Step 2: Update roadmap**

In `docs/superpowers/specs/2026-05-08-subagent-uplift-roadmap-design.md`:

- Below the existing "P3 Stage I Shipped" line at the top of the file, add:
  ```markdown
  ✅ Stage J-pre Shipped: <commit-hash> on 2026-05-09 — cache observability pipeline; reassess Stage J fork branch on 2026-05-23 (≥2 weeks of trace data)
  ```
- In the Stage J section (line 594), under `**Status**`, append:
  ```markdown
  · J-pre (cache observability) shipped 2026-05-09; fork-branch decision deferred to 2026-05-23 review
  ```

- [ ] **Step 3: Final R10 + R-baseline check**

```bash
git diff f891cc71b -- src/harness/agent.rs | wc -l   # MUST be 0
ls src/harness/*.rs | wc -l                            # MUST be 10
cargo build -p alephcore 2>&1 | tail -5
cargo test -p alephcore --lib 2>&1 | tail -5
cargo test --test cache_observability_smoke 2>&1 | tail -5
```
Expected: 0, 10, build clean, lib tests stay green relative to baseline (pre-existing failures untouched), smoke 3/3.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
docs: Stage J-pre shipped (cache observability pipeline)

Wires LoopTraceEvent::ProviderUsage via MeteringProvider decorator at
root + subagent provider construction sites. R10 redline preserved
(harness/agent.rs 0 diff vs Stage I closure, file count 10).

Stage J fork-branch decision deferred to 2026-05-23 review based on
≥2 weeks of trace data starting from this ship date.
EOF
)"
```

---

## Self-Review Checklist (run after writing the plan, before dispatch)

- [x] **Spec coverage** — every requirement in the user-approved design (Q1 decorator approach, Q2 RecordingMockProvider note, schema-only trace, two wrap sites, smoke test) maps to a task above.
- [x] **No placeholders** — every step shows the actual file path and code; "Implementer note" callouts flag the few things only confirmable at edit time (mock-mock pattern reuse, exact construction site of `default_provider`).
- [x] **Type consistency** — `cache_creation_tokens` (TokenUsage) ↔ `cache_creation_input_tokens` (Anthropic raw) is named the same way the existing `cache_read_tokens` ↔ `cache_read_input_tokens` pair is.
- [x] **R10 verifier present** — every task that could conceivably touch harness has the `git diff f891cc71b` check + `ls` count check.
- [x] **Single-PR / atomic commits** — 9 commits, one per task, each compiles + passes tests independently.

---

## Execution Handoff

Plan complete. Two execution options:

**1. Subagent-Driven (recommended)** — fresh implementer subagent per task; spec-compliance review then code-quality review between tasks; fast iteration.

**2. Inline Execution** — execute tasks in this session using `superpowers:executing-plans`; batch execution with checkpoints.

Default: Subagent-Driven via `superpowers:subagent-driven-development`.
