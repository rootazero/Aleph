# Agent Loop Tier 2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add 413 reactive recovery, fallback model, stop hooks, and tool use summary to the agent loop.

**Architecture:** Extend `retry.rs` error classification with two new verdicts (CompactAndRetry, Fallback). Integrate recovery/fallback logic into the Think step of `loop_core.rs`. Add two new modules (`stop_hooks.rs`, `tool_summary.rs`) for extensibility and observability.

**Tech Stack:** Rust, tokio, anyhow, serde_json, tokio::process

**Spec:** `docs/superpowers/specs/2026-04-01-agent-loop-tier2-design.md`

---

## File Structure

| File | Action | Responsibility |
|------|--------|---------------|
| `src/agent_loop/retry.rs` | Modify | +CompactAndRetry, +Fallback verdicts, +classify_exhausted_error(), +parse_token_gap() |
| `src/agent_loop/loop_core.rs` | Modify | +413 recovery loop, +fallback switching, +stop hook checks, +summary fire-and-forget, +new fields |
| `src/agent_loop/stop_hooks.rs` | Create | Stop hook execution, exit code protocol |
| `src/agent_loop/tool_summary.rs` | Create | Tool use summary generation via AiProvider |
| `src/agent_loop/mod.rs` | Modify | +module declarations, +re-exports |
| `src/agent_loop/factory.rs` | Modify | +optional fallback/summary provider injection |

---

## Batch 1: 413 Recovery + Fallback Model

### Task 1: Extend RetryVerdict with CompactAndRetry and parse_token_gap

**Files:**
- Modify: `src/agent_loop/retry.rs:16-49`

- [ ] **Step 1: Write tests for parse_token_gap**

Add to the `#[cfg(test)] mod tests` block at the end of `retry.rs`:

```rust
#[test]
fn test_parse_token_gap_standard_format() {
    let err = anyhow::anyhow!("prompt is too long: 137500 tokens > 135000 maximum");
    assert_eq!(parse_token_gap(&err), Some(2500));
}

#[test]
fn test_parse_token_gap_no_match() {
    let err = anyhow::anyhow!("HTTP 413 Payload Too Large");
    assert_eq!(parse_token_gap(&err), None);
}

#[test]
fn test_parse_token_gap_large_gap() {
    let err = anyhow::anyhow!("prompt is too long: 200000 tokens > 128000 maximum");
    assert_eq!(parse_token_gap(&err), Some(72000));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib -- retry::tests::test_parse_token_gap`
Expected: FAIL — `parse_token_gap` not found

- [ ] **Step 3: Implement parse_token_gap**

Add after `classify_error()` (after line 50):

```rust
/// Extract token gap from "prompt is too long: X tokens > Y maximum" error messages.
///
/// Returns `Some(actual - limit)` if the pattern matches, `None` otherwise.
pub fn parse_token_gap(err: &anyhow::Error) -> Option<usize> {
    let msg = err.to_string();
    // Pattern: "prompt is too long: 137500 tokens > 135000 maximum"
    let lower = msg.to_lowercase();
    if !lower.contains("prompt is too long") && !lower.contains("prompt_too_long") {
        return None;
    }
    // Extract numbers: find "N tokens > M"
    let re_pattern = regex_lite::Regex::new(r"(\d+)\s*tokens?\s*>\s*(\d+)").ok()?;
    let caps = re_pattern.captures(&msg)?;
    let actual: usize = caps.get(1)?.as_str().parse().ok()?;
    let limit: usize = caps.get(2)?.as_str().parse().ok()?;
    Some(actual.saturating_sub(limit))
}
```

- [ ] **Step 4: Check if regex-lite is available, add if needed**

Run: `grep 'regex-lite' /Volumes/TBU/Workspace/Aleph/Cargo.toml`

If not present, add to `[dependencies]` in `Cargo.toml`:
```toml
regex-lite = "0.1"
```

If you prefer to avoid the dependency, replace with manual string parsing:

```rust
pub fn parse_token_gap(err: &anyhow::Error) -> Option<usize> {
    let msg = err.to_string();
    let lower = msg.to_lowercase();
    if !lower.contains("prompt is too long") && !lower.contains("prompt_too_long") {
        return None;
    }
    // Find "N tokens > M" pattern manually
    let tokens_idx = lower.find("tokens")?;
    let before_tokens = &msg[..tokens_idx].trim_end();
    let actual_str = before_tokens.rsplit_once(|c: char| !c.is_ascii_digit())?.1;
    let actual: usize = actual_str.parse().ok()?;

    let after_tokens = &lower[tokens_idx..];
    let gt_idx = after_tokens.find('>')?;
    let after_gt = after_tokens[gt_idx + 1..].trim_start();
    let limit_str: String = after_gt.chars().take_while(|c| c.is_ascii_digit()).collect();
    let limit: usize = limit_str.parse().ok()?;

    Some(actual.saturating_sub(limit))
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib -- retry::tests::test_parse_token_gap`
Expected: 3 tests PASS

- [ ] **Step 6: Write tests for classify_error with 413**

```rust
#[test]
fn test_classify_413_status() {
    let err = anyhow::anyhow!("HTTP 413 Payload Too Large");
    assert!(matches!(
        classify_error(&err),
        RetryVerdict::CompactAndRetry { token_gap: None }
    ));
}

#[test]
fn test_classify_prompt_too_long_with_gap() {
    let err = anyhow::anyhow!("prompt is too long: 137500 tokens > 135000 maximum");
    assert!(matches!(
        classify_error(&err),
        RetryVerdict::CompactAndRetry { token_gap: Some(2500) }
    ));
}

#[test]
fn test_classify_prompt_too_long_no_numbers() {
    let err = anyhow::anyhow!("Error: prompt_too_long");
    assert!(matches!(
        classify_error(&err),
        RetryVerdict::CompactAndRetry { token_gap: None }
    ));
}
```

- [ ] **Step 7: Extend RetryVerdict and classify_error**

Update the `RetryVerdict` enum (line 16-22):

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum RetryVerdict {
    Retry { delay: Duration },
    Fatal,
    /// The request is too large — compact messages and retry.
    CompactAndRetry { token_gap: Option<usize> },
}
```

Add 413 detection at the TOP of `classify_error()`, before the 429 check (line 28):

```rust
pub fn classify_error(err: &anyhow::Error) -> RetryVerdict {
    let msg = err.to_string().to_lowercase();

    // Prompt too long / 413 → compact and retry (not a transient retry)
    if msg.contains("413")
        || msg.contains("prompt is too long")
        || msg.contains("prompt_too_long")
        || msg.contains("request_too_large")
    {
        return RetryVerdict::CompactAndRetry {
            token_gap: parse_token_gap(err),
        };
    }

    // Rate-limit / overloaded → 500 ms base
    // ... rest unchanged ...
}
```

- [ ] **Step 8: Update retry_async to not retry CompactAndRetry**

In `retry_async()` (line 82), the existing check is:
```rust
if retries_left == 0 || verdict == RetryVerdict::Fatal {
    return Err(err);
}
```

Change to:
```rust
match verdict {
    RetryVerdict::Fatal | RetryVerdict::CompactAndRetry { .. } => {
        return Err(err);
    }
    RetryVerdict::Retry { .. } if retries_left == 0 => {
        return Err(err);
    }
    _ => {}
}
```

This ensures CompactAndRetry errors are immediately returned to the caller (loop_core) instead of being retried with backoff.

- [ ] **Step 9: Run all retry tests**

Run: `cargo test -p alephcore --lib -- retry::tests`
Expected: All tests PASS (existing + new)

- [ ] **Step 10: Commit**

```
feat(retry): add CompactAndRetry verdict and parse_token_gap for 413 recovery
```

---

### Task 2: Add Fallback verdict and classify_exhausted_error

**Files:**
- Modify: `src/agent_loop/retry.rs`

- [ ] **Step 1: Write tests for Fallback verdict and classify_exhausted_error**

```rust
#[test]
fn test_classify_exhausted_overloaded() {
    let err = anyhow::anyhow!("HTTP 529 overloaded");
    assert!(matches!(
        classify_exhausted_error(&err),
        RetryVerdict::Fallback { .. }
    ));
}

#[test]
fn test_classify_exhausted_network() {
    let err = anyhow::anyhow!("connection reset by peer");
    assert!(matches!(
        classify_exhausted_error(&err),
        RetryVerdict::Fallback { .. }
    ));
}

#[test]
fn test_classify_exhausted_404() {
    let err = anyhow::anyhow!("HTTP 404 model not found");
    assert!(matches!(
        classify_exhausted_error(&err),
        RetryVerdict::Fallback { .. }
    ));
}

#[test]
fn test_classify_exhausted_401() {
    let err = anyhow::anyhow!("HTTP 401 Unauthorized");
    assert!(matches!(
        classify_exhausted_error(&err),
        RetryVerdict::Fallback { .. }
    ));
}

#[test]
fn test_classify_exhausted_400_stays_fatal() {
    let err = anyhow::anyhow!("HTTP 400 Bad Request: invalid parameter");
    assert_eq!(classify_exhausted_error(&err), RetryVerdict::Fatal);
}

#[test]
fn test_classify_exhausted_413_stays_compact() {
    let err = anyhow::anyhow!("HTTP 413 prompt is too long: 137500 tokens > 135000 maximum");
    assert!(matches!(
        classify_exhausted_error(&err),
        RetryVerdict::CompactAndRetry { .. }
    ));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib -- retry::tests::test_classify_exhausted`
Expected: FAIL — `classify_exhausted_error` not found

- [ ] **Step 3: Add Fallback variant to RetryVerdict**

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum RetryVerdict {
    Retry { delay: Duration },
    Fatal,
    CompactAndRetry { token_gap: Option<usize> },
    /// Primary model unavailable — switch to fallback if available.
    Fallback { reason: String },
}
```

- [ ] **Step 4: Implement classify_exhausted_error**

Add after `parse_token_gap()`:

```rust
/// Classify an error AFTER all retries are exhausted.
///
/// Called by the main loop when `retry_async` returns its final error.
/// Reclassifies transient errors as `Fallback` since retries didn't help.
/// 413 errors pass through as `CompactAndRetry` (handled separately).
/// 400 errors remain `Fatal` (request itself is broken).
pub fn classify_exhausted_error(err: &anyhow::Error) -> RetryVerdict {
    let base = classify_error(err);

    // 413 → still CompactAndRetry (main loop handles compression)
    if matches!(base, RetryVerdict::CompactAndRetry { .. }) {
        return base;
    }

    let msg = err.to_string().to_lowercase();

    // 400 → Fatal (request is malformed, fallback won't help)
    if msg.contains("400") && msg.contains("bad request") {
        return RetryVerdict::Fatal;
    }

    // 404 model not found → Fallback
    if msg.contains("404") || msg.contains("not found") {
        return RetryVerdict::Fallback {
            reason: "model not found".into(),
        };
    }

    // 401/403 auth errors → Fallback (but caller should notify user)
    if msg.contains("401") || msg.contains("403") || msg.contains("unauthorized") || msg.contains("forbidden") {
        return RetryVerdict::Fallback {
            reason: "authentication failed — check your API key".into(),
        };
    }

    // Transient errors (overloaded, network) that exhausted retries → Fallback
    if matches!(base, RetryVerdict::Retry { .. }) {
        return RetryVerdict::Fallback {
            reason: format!("primary model unavailable: {}", err),
        };
    }

    // Everything else stays Fatal
    RetryVerdict::Fatal
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib -- retry::tests`
Expected: All tests PASS

- [ ] **Step 6: Commit**

```
feat(retry): add Fallback verdict and classify_exhausted_error for model switching
```

---

### Task 3: Emergency truncation for 413 recovery

> **Dependency note:** Task 4 (Think step integration) depends on BOTH Task 3 AND Task 5.
> Implement Task 5 (fallback fields) BEFORE Task 4.
> Execution order: 1 → 2 → 3 → **5 → 4** → 6

**Files:**
- Modify: `src/agent_loop/loop_core.rs`

- [ ] **Step 1: Write tests for emergency_truncate**

Add to the `#[cfg(test)] mod tests` block in `loop_core.rs`:

```rust
#[test]
fn test_emergency_truncate_drops_oldest_groups() {
    // Build a conversation: 3 rounds (user→assistant pairs) + 1 final user
    let mut messages = vec![
        UnifiedMessage::user("round 1 question"),
        UnifiedMessage::assistant("round 1 answer"),
        UnifiedMessage::user("round 2 question"),
        UnifiedMessage::assistant("round 2 answer"),
        UnifiedMessage::user("round 3 question"),
        UnifiedMessage::assistant("round 3 answer"),
        UnifiedMessage::user("current question"),
    ];

    // Drop 20% (no token_gap → fallback ratio), fresh_tail=2 (protect last 2 rounds)
    emergency_truncate(&mut messages, None, 2);

    // Should have truncation marker + preserved tail
    assert!(messages[0].text_content().unwrap_or_default().contains("truncated"));
    // Final user message preserved
    assert_eq!(
        messages.last().unwrap().text_content().unwrap_or_default(),
        "current question"
    );
    // Original 7 messages should be fewer now
    assert!(messages.len() < 7);
}

#[test]
fn test_emergency_truncate_with_known_gap() {
    let mut messages = vec![];
    // Create 10 rounds of ~100 chars each
    for i in 0..10 {
        messages.push(UnifiedMessage::user(&format!("question {i} {}", "x".repeat(80))));
        messages.push(UnifiedMessage::assistant(&format!("answer {i} {}", "y".repeat(80))));
    }
    messages.push(UnifiedMessage::user("final"));

    let original_len = messages.len();
    // token_gap = 500, each round ~50 tokens at 3.5 ratio → need to drop ~10 rounds × safety margin
    // but fresh_tail=3 protects last 3 rounds
    emergency_truncate(&mut messages, Some(500), 3);

    assert!(messages.len() < original_len);
    assert_eq!(
        messages.last().unwrap().text_content().unwrap_or_default(),
        "final"
    );
}

#[test]
fn test_emergency_truncate_too_few_messages_is_noop() {
    let mut messages = vec![
        UnifiedMessage::user("only question"),
        UnifiedMessage::assistant("only answer"),
    ];
    let original_len = messages.len();
    emergency_truncate(&mut messages, None, 2);
    // Should not drop anything — not enough groups outside fresh tail
    assert_eq!(messages.len(), original_len);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib -- loop_core::tests::test_emergency_truncate`
Expected: FAIL — `emergency_truncate` not found

- [ ] **Step 3: Implement emergency_truncate**

Add as a private function in `loop_core.rs`, before the `AgentLoop` impl block:

```rust
/// Constants for 413 recovery
const MAX_PTL_RETRIES: usize = 3;
const PTL_SAFETY_MARGIN: f64 = 1.2;
const PTL_FALLBACK_DROP_RATIO: f64 = 0.20;
const PTL_TRUNCATION_MARKER: &str =
    "[earlier conversation truncated for recovery]";

/// Group messages by API round: each group is (user → assistant [→ tool_results]*).
///
/// Returns Vec of (start_index, end_index_exclusive) pairs.
fn group_by_round(messages: &[UnifiedMessage]) -> Vec<(usize, usize)> {
    let mut groups = Vec::new();
    let mut start = 0;

    for (i, msg) in messages.iter().enumerate() {
        // A new user message (not the first) starts a new group
        if i > 0 && msg.role_str() == "user" && !msg.is_tool_result() {
            groups.push((start, i));
            start = i;
        }
    }
    // Last group
    if start < messages.len() {
        groups.push((start, messages.len()));
    }

    groups
}

/// Emergency truncation for 413 recovery.
///
/// Drops oldest message groups to free tokens. Protects the last
/// `fresh_tail_count` groups. Inserts a marker at the truncation point.
fn emergency_truncate(
    messages: &mut Vec<UnifiedMessage>,
    token_gap: Option<usize>,
    fresh_tail_count: usize,
) {
    use super::context_budget::pressure::estimate_tokens_smart;

    let groups = group_by_round(messages);
    if groups.len() <= fresh_tail_count + 1 {
        // Not enough groups to drop — protect fresh tail
        return;
    }

    let droppable_count = groups.len().saturating_sub(fresh_tail_count);
    if droppable_count == 0 {
        return;
    }

    let groups_to_drop = if let Some(gap) = token_gap {
        // Drop oldest groups until we've freed gap × safety_margin tokens
        let target = (gap as f64 * PTL_SAFETY_MARGIN) as usize;
        let mut freed = 0usize;
        let mut count = 0usize;
        for &(start, end) in &groups[..droppable_count] {
            if freed >= target {
                break;
            }
            for msg in &messages[start..end] {
                freed += estimate_tokens_smart(&msg.to_string());
            }
            count += 1;
        }
        count.max(1) // Drop at least 1 group
    } else {
        // No gap info → drop 20% of droppable groups (at least 1)
        ((droppable_count as f64 * PTL_FALLBACK_DROP_RATIO).ceil() as usize).max(1)
    };

    let groups_to_drop = groups_to_drop.min(droppable_count);
    let drop_end = groups[groups_to_drop - 1].1;

    // Replace dropped messages with truncation marker
    messages.drain(..drop_end);
    messages.insert(0, UnifiedMessage::user(PTL_TRUNCATION_MARKER));
}
```

- [ ] **Step 4: Verify `role_str()` and `is_tool_result()` exist on UnifiedMessage**

Run: `grep -n 'fn role_str\|fn is_tool_result\|pub fn text_content' /Volumes/TBU/Workspace/Aleph/src/providers/message.rs | head -10`

If `role_str()` doesn't exist, use the `role` field directly:
```rust
if i > 0 && messages[i].role == crate::providers::message::Role::User
```

If `is_tool_result()` doesn't exist, check for tool_use_id presence or role == "tool".

Adapt the `group_by_round` function accordingly.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib -- loop_core::tests::test_emergency_truncate`
Expected: 3 tests PASS

- [ ] **Step 6: Commit**

```
feat(loop_core): add emergency_truncate for 413 reactive recovery
```

---

### Task 4: Integrate 413 recovery into the main loop Think step

**Files:**
- Modify: `src/agent_loop/loop_core.rs:404-410`

- [ ] **Step 1: Write test for 413 recovery integration**

```rust
#[tokio::test]
async fn test_413_recovery_retries_after_truncation() {
    // MockProvider that fails with 413 once, then succeeds
    struct Ptl413ThenOk {
        call_count: Mutex<usize>,
    }

    #[async_trait]
    impl LoopProvider for Ptl413ThenOk {
        async fn stream(
            &self,
            _messages: &[UnifiedMessage],
            _system_prompt: &str,
            _tools: &[ToolDefinition],
        ) -> anyhow::Result<BoxStream<'static, anyhow::Result<ProviderDelta>>> {
            let mut count = self.call_count.lock().unwrap_or_else(|e| e.into_inner());
            *count += 1;
            if *count == 1 {
                Err(anyhow::anyhow!(
                    "prompt is too long: 137500 tokens > 135000 maximum"
                ))
            } else {
                let resp = ProviderResponse::text_only("recovered".to_string());
                Ok(crate::providers::delta::response_to_delta_stream(resp))
            }
        }
    }

    let provider = Ptl413ThenOk {
        call_count: Mutex::new(0),
    };
    let config = LoopConfig {
        max_iterations: 5,
        token_budget: 200_000,
    };
    let agent = AgentLoop::new(
        provider,
        LoopToolRegistry::new(),
        PromptBuilder::new(),
        SafetyGuard::permissive(),
        config,
        CancellationToken::new(),
    );

    // Build enough history that truncation has something to drop
    let mut history = Vec::new();
    for i in 0..10 {
        history.push(UnifiedMessage::user(&format!("question {i}")));
        history.push(UnifiedMessage::assistant(&format!("answer {i}")));
    }
    history.push(UnifiedMessage::user("final question"));

    let mut cb = TrackingCallback::default();
    let result = agent
        .run_with_history_messages(history, &mut cb)
        .await
        .unwrap();

    assert_eq!(result.final_text.as_deref(), Some("recovered"));
    assert!(!result.cancelled);
}

#[tokio::test]
async fn test_413_recovery_exhausts_retries() {
    // MockProvider that always fails with 413
    struct Always413;

    #[async_trait]
    impl LoopProvider for Always413 {
        async fn stream(
            &self,
            _messages: &[UnifiedMessage],
            _system_prompt: &str,
            _tools: &[ToolDefinition],
        ) -> anyhow::Result<BoxStream<'static, anyhow::Result<ProviderDelta>>> {
            Err(anyhow::anyhow!("HTTP 413 Payload Too Large"))
        }
    }

    let agent = AgentLoop::new(
        Always413,
        LoopToolRegistry::new(),
        PromptBuilder::new(),
        SafetyGuard::permissive(),
        LoopConfig::default(),
        CancellationToken::new(),
    );

    let mut history = Vec::new();
    for i in 0..10 {
        history.push(UnifiedMessage::user(&format!("q{i}")));
        history.push(UnifiedMessage::assistant(&format!("a{i}")));
    }
    history.push(UnifiedMessage::user("final"));

    let mut cb = NoopCallback;
    let result = agent.run_with_history_messages(history, &mut cb).await;
    // Should fail after MAX_PTL_RETRIES
    assert!(result.is_err());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib -- loop_core::tests::test_413`
Expected: FAIL — current code propagates 413 immediately

- [ ] **Step 3: Replace the Think step with 413 recovery loop**

Replace lines 404-410 in `loop_core.rs`:

```rust
            // Think: stream deltas with retry and cancellation
            let delta_stream = super::retry::retry_async(
                || self.provider.stream(&messages, &system_prompt, &tool_defs),
                &self.cancel_token,
                3,
            )
            .await?;
```

With:

```rust
            // Think: stream deltas with retry, 413 recovery, and fallback
            let mut ptl_attempts: usize = 0;
            let delta_stream = loop {
                let provider: &dyn LoopProvider = match active_provider {
                    ActiveProvider::Primary => &self.provider,
                    ActiveProvider::Fallback => {
                        self.fallback_provider.as_ref().unwrap().as_ref()
                    }
                };
                match super::retry::retry_async(
                    || provider.stream(&messages, &system_prompt, &tool_defs),
                    &self.cancel_token,
                    3,
                )
                .await
                {
                    Ok(stream) => break stream,
                    Err(e) => {
                        let verdict = super::retry::classify_exhausted_error(&e);
                        match verdict {
                            super::retry::RetryVerdict::CompactAndRetry { token_gap } => {
                                ptl_attempts += 1;
                                if ptl_attempts > MAX_PTL_RETRIES {
                                    return Err(e);
                                }
                                let fresh_tail = {
                                    let ctx = self
                                        .context_budget
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner());
                                    ctx.as_ref()
                                        .map(|b| b.fresh_tail_count())
                                        .unwrap_or(6)
                                };
                                tracing::warn!(
                                    attempt = ptl_attempts,
                                    ?token_gap,
                                    "413 recovery: truncating messages"
                                );
                                emergency_truncate(
                                    &mut messages,
                                    token_gap,
                                    fresh_tail,
                                );
                                continue;
                            }
                            super::retry::RetryVerdict::Fallback { reason }
                                if self.fallback_provider.is_some()
                                    && matches!(
                                        active_provider,
                                        ActiveProvider::Primary
                                    ) =>
                            {
                                active_provider = ActiveProvider::Fallback;
                                let label = self
                                    .fallback_label
                                    .as_deref()
                                    .unwrap_or("fallback");
                                tracing::warn!(
                                    %reason,
                                    fallback = label,
                                    "Switching to fallback model"
                                );
                                callback.on_model_fallback(&reason, label);
                                continue;
                            }
                            _ => return Err(e),
                        }
                    }
                }
            };
```

- [ ] **Step 4: Add ActiveProvider enum and state variable**

Add at the top of `run_with_history_messages`, after the existing state variables (around line 303):

```rust
        #[derive(Clone, Copy)]
        enum ActiveProvider {
            Primary,
            Fallback,
        }
        let mut active_provider = ActiveProvider::Primary;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib -- loop_core::tests::test_413`
Expected: 2 tests PASS

- [ ] **Step 6: Run all loop_core tests for regressions**

Run: `cargo test -p alephcore --lib -- loop_core::tests`
Expected: All existing tests still PASS

- [ ] **Step 7: Commit**

```
feat(loop_core): integrate 413 reactive recovery with emergency truncation
```

---

### Task 5: Add fallback_provider fields and LoopCallback event

**Files:**
- Modify: `src/agent_loop/loop_core.rs:139-178`

- [ ] **Step 1: Add on_model_fallback to LoopCallback**

In the `LoopCallback` trait (around line 139):

```rust
pub trait LoopCallback: Send {
    fn on_text(&mut self, _text: &str) {}
    fn on_intermediate_text(&mut self, _text: &str) {}
    fn on_tool_start(&mut self, _name: &str, _input: &Value) {}
    fn on_tool_done(&mut self, _name: &str, _result: &ToolResult) {}
    fn on_safety_block(&mut self, _error: &SafetyError) {}
    fn on_model_fallback(&mut self, _reason: &str, _fallback_model: &str) {}  // NEW
}
```

- [ ] **Step 2: Add fallback fields to AgentLoop**

In the `AgentLoop` struct (around line 158):

```rust
pub struct AgentLoop<P: LoopProvider> {
    provider: P,
    fallback_provider: Option<Box<dyn LoopProvider>>,  // NEW
    fallback_label: Option<String>,                     // NEW
    tool_registry: LoopToolRegistry,
    // ... rest unchanged ...
}
```

- [ ] **Step 3: Update constructor and add builder method**

In `AgentLoop::new()`, add after existing field initialization:

```rust
        Self {
            provider,
            fallback_provider: None,
            fallback_label: None,
            tool_registry,
            // ... rest unchanged ...
        }
```

Add builder method after `with_delta_sink()`:

```rust
    /// Attach a fallback provider for automatic model switching.
    ///
    /// When the primary model is unavailable (overloaded, auth failure,
    /// not found), the loop automatically switches to this fallback.
    pub fn with_fallback(
        mut self,
        provider: Box<dyn LoopProvider>,
        label: impl Into<String>,
    ) -> Self {
        self.fallback_provider = Some(provider);
        self.fallback_label = Some(label.into());
        self
    }
```

- [ ] **Step 4: Write test for fallback model switching**

```rust
#[tokio::test]
async fn test_fallback_model_switches_on_overload() {
    struct OverloadedProvider;

    #[async_trait]
    impl LoopProvider for OverloadedProvider {
        async fn stream(
            &self,
            _messages: &[UnifiedMessage],
            _system_prompt: &str,
            _tools: &[ToolDefinition],
        ) -> anyhow::Result<BoxStream<'static, anyhow::Result<ProviderDelta>>> {
            Err(anyhow::anyhow!("HTTP 529 overloaded"))
        }
    }

    struct FallbackProvider;

    #[async_trait]
    impl LoopProvider for FallbackProvider {
        async fn stream(
            &self,
            _messages: &[UnifiedMessage],
            _system_prompt: &str,
            _tools: &[ToolDefinition],
        ) -> anyhow::Result<BoxStream<'static, anyhow::Result<ProviderDelta>>> {
            let resp = ProviderResponse::text_only("fallback response".to_string());
            Ok(crate::providers::delta::response_to_delta_stream(resp))
        }
    }

    let agent = AgentLoop::new(
        OverloadedProvider,
        LoopToolRegistry::new(),
        PromptBuilder::new(),
        SafetyGuard::permissive(),
        LoopConfig::default(),
        CancellationToken::new(),
    )
    .with_fallback(Box::new(FallbackProvider), "test-fallback");

    let mut cb = TrackingCallback::default();
    let result = agent.run("hello", &mut cb).await.unwrap();

    assert_eq!(result.final_text.as_deref(), Some("fallback response"));
}

#[tokio::test]
async fn test_fallback_not_available_propagates_error() {
    struct OverloadedProvider;

    #[async_trait]
    impl LoopProvider for OverloadedProvider {
        async fn stream(
            &self,
            _messages: &[UnifiedMessage],
            _system_prompt: &str,
            _tools: &[ToolDefinition],
        ) -> anyhow::Result<BoxStream<'static, anyhow::Result<ProviderDelta>>> {
            Err(anyhow::anyhow!("HTTP 529 overloaded"))
        }
    }

    // No fallback configured
    let agent = AgentLoop::new(
        OverloadedProvider,
        LoopToolRegistry::new(),
        PromptBuilder::new(),
        SafetyGuard::permissive(),
        LoopConfig::default(),
        CancellationToken::new(),
    );

    let mut cb = NoopCallback;
    let result = agent.run("hello", &mut cb).await;
    assert!(result.is_err());
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib -- loop_core::tests::test_fallback`
Expected: 2 tests PASS

- [ ] **Step 6: Update TrackingCallback in tests to track fallback events**

In the test module's `TrackingCallback`:

```rust
#[derive(Default)]
struct TrackingCallback {
    texts: Vec<String>,
    intermediate_texts: Vec<String>,
    tool_starts: Vec<String>,
    tool_dones: Vec<String>,
    safety_blocks: Vec<String>,
    fallback_events: Vec<(String, String)>,  // NEW: (reason, model)
}

impl LoopCallback for TrackingCallback {
    // ... existing methods ...
    fn on_model_fallback(&mut self, reason: &str, fallback_model: &str) {
        self.fallback_events
            .push((reason.to_string(), fallback_model.to_string()));
    }
}
```

- [ ] **Step 7: Run all loop_core tests**

Run: `cargo test -p alephcore --lib -- loop_core::tests`
Expected: All PASS

- [ ] **Step 8: Commit**

```
feat(loop_core): add fallback model switching on primary model failure
```

---

### Task 6: Update factory and mod.rs for Batch 1

**Files:**
- Modify: `src/agent_loop/factory.rs`
- Modify: `src/agent_loop/mod.rs`

- [ ] **Step 1: Update mod.rs re-exports**

Add to the `pub use` section:

```rust
pub use retry::{classify_exhausted_error, parse_token_gap, RetryVerdict};
```

- [ ] **Step 2: Run cargo check**

Run: `cargo check -p alephcore`
Expected: No errors

- [ ] **Step 3: Commit**

```
chore(agent_loop): update mod.rs re-exports for Batch 1
```

---

## Batch 2: Stop Hooks + Tool Use Summary

### Task 7: Create stop_hooks.rs with types and execution

**Files:**
- Create: `src/agent_loop/stop_hooks.rs`

- [ ] **Step 1: Write tests for StopHook execution**

Create `src/agent_loop/stop_hooks.rs` with tests first:

```rust
//! Stop hooks — external commands executed before the agent loop stops.
//!
//! Hooks receive context via stdin JSON and communicate decisions via exit code:
//! - 0: allow stop
//! - 2: block stop (stdout = reason)
//! - other: hook error (logged, does not block)

use std::time::Duration;

use serde::Serialize;
use tokio_util::sync::CancellationToken;

/// A registered stop hook.
pub struct StopHook {
    pub name: String,
    pub command: String,
    pub timeout: Duration,
}

impl StopHook {
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            timeout: Duration::from_secs(30),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// Context passed to stop hooks via stdin as JSON.
#[derive(Serialize)]
pub struct StopHookContext {
    pub final_text: Option<String>,
    pub iterations: usize,
    pub tool_calls_made: usize,
    pub stop_reason: String,
}

/// Result of a single hook execution.
#[derive(Debug)]
pub enum StopHookVerdict {
    Allow,
    Block { reason: String },
    Error { hook_name: String, message: String },
}

/// Aggregated result of all stop hooks.
#[derive(Debug)]
pub struct StopHookAggregateResult {
    pub verdicts: Vec<StopHookVerdict>,
}

impl StopHookAggregateResult {
    /// Returns the first blocking reason, if any.
    pub fn blocking_reason(&self) -> Option<&str> {
        self.verdicts.iter().find_map(|v| match v {
            StopHookVerdict::Block { reason } => Some(reason.as_str()),
            _ => None,
        })
    }

    /// Returns all error messages.
    pub fn errors(&self) -> Vec<(&str, &str)> {
        self.verdicts
            .iter()
            .filter_map(|v| match v {
                StopHookVerdict::Error { hook_name, message } => {
                    Some((hook_name.as_str(), message.as_str()))
                }
                _ => None,
            })
            .collect()
    }
}

/// Execute all stop hooks in parallel.
pub async fn execute_stop_hooks(
    hooks: &[StopHook],
    context: &StopHookContext,
    cancel: &CancellationToken,
) -> StopHookAggregateResult {
    use futures::future::join_all;

    let context_json =
        serde_json::to_string(context).unwrap_or_else(|_| "{}".to_string());

    let futures: Vec<_> = hooks
        .iter()
        .map(|hook| execute_single_hook(hook, &context_json, cancel))
        .collect();

    let verdicts = join_all(futures).await;
    StopHookAggregateResult { verdicts }
}

async fn execute_single_hook(
    hook: &StopHook,
    context_json: &str,
    cancel: &CancellationToken,
) -> StopHookVerdict {
    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;

    let result = tokio::select! {
        r = async {
            let mut child = match Command::new("sh")
                .arg("-c")
                .arg(&hook.command)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    return StopHookVerdict::Error {
                        hook_name: hook.name.clone(),
                        message: format!("failed to spawn: {e}"),
                    };
                }
            };

            // Write context to stdin
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(context_json.as_bytes()).await;
                drop(stdin);
            }

            // Wait with timeout
            match tokio::time::timeout(hook.timeout, child.wait_with_output()).await {
                Ok(Ok(output)) => {
                    let code = output.status.code().unwrap_or(-1);
                    match code {
                        0 => StopHookVerdict::Allow,
                        2 => {
                            let reason = String::from_utf8_lossy(&output.stdout)
                                .trim()
                                .to_string();
                            StopHookVerdict::Block {
                                reason: if reason.is_empty() {
                                    format!("hook '{}' blocked stop", hook.name)
                                } else {
                                    reason
                                },
                            }
                        }
                        _ => StopHookVerdict::Error {
                            hook_name: hook.name.clone(),
                            message: format!(
                                "exit code {code}: {}",
                                String::from_utf8_lossy(&output.stderr).trim()
                            ),
                        },
                    }
                }
                Ok(Err(e)) => StopHookVerdict::Error {
                    hook_name: hook.name.clone(),
                    message: format!("wait failed: {e}"),
                },
                Err(_) => StopHookVerdict::Error {
                    hook_name: hook.name.clone(),
                    message: "timed out".to_string(),
                },
            }
        } => r,
        _ = cancel.cancelled() => {
            StopHookVerdict::Error {
                hook_name: hook.name.clone(),
                message: "cancelled".to_string(),
            }
        }
    };

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_hook_allow() {
        let hook = StopHook::new("allow", "exit 0");
        let ctx = StopHookContext {
            final_text: Some("done".into()),
            iterations: 5,
            tool_calls_made: 3,
            stop_reason: "end_turn".into(),
        };
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&[hook], &ctx, &cancel).await;
        assert!(result.blocking_reason().is_none());
    }

    #[tokio::test]
    async fn test_hook_block() {
        let hook = StopHook::new("blocker", "echo 'tests not passing' && exit 2");
        let ctx = StopHookContext {
            final_text: None,
            iterations: 1,
            tool_calls_made: 0,
            stop_reason: "end_turn".into(),
        };
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&[hook], &ctx, &cancel).await;
        assert_eq!(result.blocking_reason(), Some("tests not passing"));
    }

    #[tokio::test]
    async fn test_hook_error_non_blocking() {
        let hook = StopHook::new("broken", "exit 1");
        let ctx = StopHookContext {
            final_text: None,
            iterations: 1,
            tool_calls_made: 0,
            stop_reason: "end_turn".into(),
        };
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&[hook], &ctx, &cancel).await;
        assert!(result.blocking_reason().is_none());
        assert_eq!(result.errors().len(), 1);
    }

    #[tokio::test]
    async fn test_hook_timeout() {
        let hook = StopHook::new("slow", "sleep 60")
            .with_timeout(Duration::from_millis(100));
        let ctx = StopHookContext {
            final_text: None,
            iterations: 1,
            tool_calls_made: 0,
            stop_reason: "end_turn".into(),
        };
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&[hook], &ctx, &cancel).await;
        assert!(result.blocking_reason().is_none());
        let errors = result.errors();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].1.contains("timed out"));
    }

    #[tokio::test]
    async fn test_hook_receives_context_json() {
        // Hook that reads stdin and blocks if it finds "end_turn"
        let hook = StopHook::new(
            "ctx_checker",
            r#"input=$(cat); echo "$input" | grep -q "end_turn" && echo "found end_turn" && exit 2 || exit 0"#,
        );
        let ctx = StopHookContext {
            final_text: Some("done".into()),
            iterations: 5,
            tool_calls_made: 3,
            stop_reason: "end_turn".into(),
        };
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&[hook], &ctx, &cancel).await;
        assert_eq!(result.blocking_reason(), Some("found end_turn"));
    }

    #[tokio::test]
    async fn test_multiple_hooks_first_block_wins() {
        let hooks = vec![
            StopHook::new("allow1", "exit 0"),
            StopHook::new("blocker", "echo 'blocked' && exit 2"),
            StopHook::new("allow2", "exit 0"),
        ];
        let ctx = StopHookContext {
            final_text: None,
            iterations: 1,
            tool_calls_made: 0,
            stop_reason: "end_turn".into(),
        };
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&hooks, &ctx, &cancel).await;
        assert_eq!(result.blocking_reason(), Some("blocked"));
    }

    #[test]
    fn test_aggregate_no_errors() {
        let result = StopHookAggregateResult {
            verdicts: vec![StopHookVerdict::Allow, StopHookVerdict::Allow],
        };
        assert!(result.blocking_reason().is_none());
        assert!(result.errors().is_empty());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib -- stop_hooks::tests`
Expected: All PASS

- [ ] **Step 3: Commit**

```
feat(agent_loop): add stop_hooks module with exit-code protocol
```

---

### Task 8: Integrate stop hooks into the main loop

**Files:**
- Modify: `src/agent_loop/loop_core.rs`
- Modify: `src/agent_loop/mod.rs`

- [ ] **Step 1: Add stop hook fields and callbacks to AgentLoop**

In `loop_core.rs`, add import at top:

```rust
use super::stop_hooks::{self, StopHook, StopHookContext};
```

Add to `LoopCallback` trait:

```rust
    fn on_stop_hook_block(&mut self, _reason: &str) {}
    fn on_stop_hook_error(&mut self, _hook_name: &str, _error: &str) {}
```

Add to `AgentLoop` struct:

```rust
    stop_hooks: Vec<StopHook>,
```

Initialize in `new()`:

```rust
    stop_hooks: Vec::new(),
```

Add builder method:

```rust
    /// Register stop hooks to run before the loop exits.
    pub fn with_stop_hooks(mut self, hooks: Vec<StopHook>) -> Self {
        self.stop_hooks = hooks;
        self
    }
```

- [ ] **Step 2: Extract break logic into a stop-hook-aware check**

Add a constant and helper at the top of `run_with_history_messages`:

```rust
        const MAX_STOP_HOOK_BLOCKS: usize = 3;
        let mut stop_hook_blocks: usize = 0;
```

Create a macro or inline the check before each `break` that should support hooks. Replace the primary break points (EndTurn with completion tag at line ~495, exhausted nudges at ~524, max_tokens exhausted at ~558, no-tool-calls at ~563) with:

```rust
// Instead of:  break;
// Use this pattern:
{
    if !self.stop_hooks.is_empty() && stop_hook_blocks < MAX_STOP_HOOK_BLOCKS {
        let ctx = StopHookContext {
            final_text: final_text.clone(),
            iterations,
            tool_calls_made,
            stop_reason: "end_turn".to_string(),  // adjust per break point
        };
        let hook_result = stop_hooks::execute_stop_hooks(
            &self.stop_hooks,
            &ctx,
            &self.cancel_token,
        )
        .await;
        for (name, msg) in hook_result.errors() {
            callback.on_stop_hook_error(name, msg);
        }
        if let Some(reason) = hook_result.blocking_reason() {
            stop_hook_blocks += 1;
            callback.on_stop_hook_block(reason);
            messages.push(UnifiedMessage::user(&format!(
                "[SYSTEM] Stop hook blocked exit: {reason}. Address the issue and try again."
            )));
            continue;
        }
    }
    break;
}
```

**Important:** Only apply stop hooks to these break points:
- EndTurn + completion tag (line ~495): `stop_reason: "end_turn"`
- Exhausted nudges (line ~524): `stop_reason: "nudge_exhausted"`
- Simple Q&A break (line ~485): **do NOT** apply hooks (no tools used, nothing to check)
- Max tokens (line ~558): **do NOT** apply hooks (output truncation, not a task completion)
- Consecutive errors (line ~677): **do NOT** apply hooks (error state)
- Token budget (line ~687): **do NOT** apply hooks (resource limit)

- [ ] **Step 3: Write integration test for stop hooks**

```rust
#[tokio::test]
async fn test_stop_hook_blocks_then_allows() {
    // Provider: first call returns EndTurn with completion tag,
    // second call (after hook block) also returns with completion tag
    let provider = MockProvider::new(vec![
        ProviderResponse {
            text: Some("first attempt <task-complete/>".to_string()),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
        ProviderResponse {
            text: Some("addressed hook feedback <task-complete/>".to_string()),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]);

    // Need at least one tool call to trigger completion protocol
    let mut registry = LoopToolRegistry::new();
    registry.register(Box::new(EchoTool));

    // Simulate: first iteration uses a tool, second returns with completion tag
    let provider = MockProvider::new(vec![
        // Iteration 1: tool call
        ProviderResponse {
            text: None,
            tool_calls: vec![NativeToolCall {
                id: "tc1".into(),
                name: "echo".into(),
                arguments: json!({"text": "hello"}),
            }],
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
        // Iteration 2: EndTurn with completion (hook blocks)
        ProviderResponse {
            text: Some("done <task-complete/>".to_string()),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
        // Iteration 3: after hook block, LLM tries again
        ProviderResponse {
            text: Some("fixed <task-complete/>".to_string()),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]);

    // Hook that blocks once (uses a temp file as state)
    let tmp = std::env::temp_dir().join("aleph_test_hook_state");
    let _ = std::fs::remove_file(&tmp);
    let hook_cmd = format!(
        "if [ ! -f '{}' ]; then touch '{}' && echo 'needs review' && exit 2; else exit 0; fi",
        tmp.display(),
        tmp.display()
    );

    let agent = AgentLoop::new(
        provider,
        registry,
        PromptBuilder::new(),
        SafetyGuard::permissive(),
        LoopConfig::default(),
        CancellationToken::new(),
    )
    .with_stop_hooks(vec![StopHook::new("review", hook_cmd)]);

    let mut cb = TrackingCallback::default();
    let result = agent.run("test", &mut cb).await.unwrap();

    // Should have gone through: tool call → completion (blocked) → retry → completion (allowed)
    assert!(result.final_text.is_some());
    assert!(result.iterations >= 3);

    let _ = std::fs::remove_file(&tmp);
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib -- loop_core::tests::test_stop_hook`
Expected: PASS

- [ ] **Step 5: Update mod.rs**

```rust
pub mod stop_hooks;
```

And add re-exports:

```rust
pub use stop_hooks::{StopHook, StopHookContext, StopHookAggregateResult, StopHookVerdict};
```

- [ ] **Step 6: Run all tests**

Run: `cargo test -p alephcore --lib -- agent_loop`
Expected: All PASS

- [ ] **Step 7: Commit**

```
feat(loop_core): integrate stop hooks with exit-code protocol at break points
```

---

### Task 9: Create tool_summary.rs

**Files:**
- Create: `src/agent_loop/tool_summary.rs`

- [ ] **Step 1: Write the module with tests**

Create `src/agent_loop/tool_summary.rs`:

```rust
//! Tool use summary — async LLM-generated one-line summaries of tool calls.
//!
//! Fires after tool execution and resolves during the next Think step's
//! streaming. Non-blocking, silent on failure.

use crate::providers::adapter::RequestPayload;
use crate::providers::AiProvider;

const SUMMARY_SYSTEM_PROMPT: &str = "\
You are a tool-use summarizer. Write a single short summary (~30 characters) \
of what the tools accomplished. Use past tense verb, be specific, no period.\n\
\n\
Examples:\n\
- Searched auth/ for login bugs\n\
- Read 3 config files\n\
- Created signup API endpoint\n\
- Fetched weather data for Tokyo";

const INPUT_TRUNCATE_CHARS: usize = 300;
const ASSISTANT_TEXT_TRUNCATE_CHARS: usize = 200;

/// Input for a single tool call to be summarized.
pub struct ToolSummaryInput {
    pub tool_name: String,
    pub tool_input: String,
    pub tool_output: String,
}

/// Truncate a string at the nearest char boundary at or before `max` bytes.
fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let end = s
        .char_indices()
        .take_while(|(i, _)| *i <= max)
        .last()
        .map(|(i, _)| i)
        .unwrap_or(0);
    &s[..end]
}

/// Build the user prompt from tool inputs and optional assistant context.
fn build_prompt(tools: &[ToolSummaryInput], last_assistant_text: Option<&str>) -> String {
    let mut prompt = String::new();

    if let Some(text) = last_assistant_text {
        let truncated = truncate_str(text, ASSISTANT_TEXT_TRUNCATE_CHARS);
        prompt.push_str(&format!("User intent: {truncated}\n\n"));
    }

    for (i, tool) in tools.iter().enumerate() {
        let input = truncate_str(&tool.tool_input, INPUT_TRUNCATE_CHARS);
        let output = truncate_str(&tool.tool_output, INPUT_TRUNCATE_CHARS);
        prompt.push_str(&format!(
            "Tool {}: {}\nInput: {}\nOutput: {}\n\n",
            i + 1,
            tool.tool_name,
            input,
            output,
        ));
    }

    prompt.push_str("Summary:");
    prompt
}

/// Generate a tool use summary using a lightweight LLM provider.
///
/// Returns `None` on any error (logged via tracing::warn). Never panics.
pub async fn generate_tool_summary(
    provider: &dyn AiProvider,
    tools: &[ToolSummaryInput],
    last_assistant_text: Option<&str>,
) -> Option<String> {
    if tools.is_empty() {
        return None;
    }

    let user_prompt = build_prompt(tools, last_assistant_text);

    let messages = vec![crate::providers::message::UnifiedMessage::user(&user_prompt)];

    let payload = RequestPayload {
        messages: &messages,
        system_prompt: Some(SUMMARY_SYSTEM_PROMPT),
        tools: None,
        model: None,
        ..Default::default()
    };

    match provider.process(payload).await {
        Ok(response) => {
            let text = response.text?.trim().to_string();
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "Tool summary generation failed");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_str_ascii() {
        assert_eq!(truncate_str("hello world", 5), "hello");
    }

    #[test]
    fn test_truncate_str_no_truncation_needed() {
        assert_eq!(truncate_str("short", 100), "short");
    }

    #[test]
    fn test_truncate_str_utf8_boundary() {
        // "你好世界" = 4 chars, 12 bytes (3 bytes each)
        let s = "你好世界";
        // Truncate at 7 bytes — should cut to "你好" (6 bytes)
        let result = truncate_str(s, 7);
        assert_eq!(result, "你好");
    }

    #[test]
    fn test_truncate_str_empty() {
        assert_eq!(truncate_str("", 10), "");
    }

    #[test]
    fn test_build_prompt_single_tool() {
        let tools = vec![ToolSummaryInput {
            tool_name: "search".into(),
            tool_input: r#"{"query": "rust async"}"#.into(),
            tool_output: "Found 3 results".into(),
        }];
        let prompt = build_prompt(&tools, None);
        assert!(prompt.contains("Tool 1: search"));
        assert!(prompt.contains("rust async"));
        assert!(prompt.contains("Summary:"));
    }

    #[test]
    fn test_build_prompt_with_assistant_context() {
        let tools = vec![ToolSummaryInput {
            tool_name: "read".into(),
            tool_input: "config.toml".into(),
            tool_output: "[file content]".into(),
        }];
        let prompt = build_prompt(&tools, Some("I'll check the config file"));
        assert!(prompt.contains("User intent: I'll check the config file"));
    }

    #[test]
    fn test_build_prompt_truncates_long_input() {
        let long_input = "x".repeat(1000);
        let tools = vec![ToolSummaryInput {
            tool_name: "fetch".into(),
            tool_input: long_input.clone(),
            tool_output: "ok".into(),
        }];
        let prompt = build_prompt(&tools, None);
        // Input should be truncated to INPUT_TRUNCATE_CHARS
        assert!(!prompt.contains(&long_input));
        assert!(prompt.len() < 1000);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib -- tool_summary::tests`
Expected: All PASS

- [ ] **Step 3: Commit**

```
feat(agent_loop): add tool_summary module with LLM-based summary generation
```

---

### Task 10: Integrate tool summary into the main loop

**Files:**
- Modify: `src/agent_loop/loop_core.rs`
- Modify: `src/agent_loop/mod.rs`

- [ ] **Step 1: Add summary_provider field and callback**

In `loop_core.rs`, add import:

```rust
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
```

Add to `LoopCallback`:

```rust
    fn on_tool_summary(&mut self, _summary: &str) {}
```

Add to `AgentLoop` struct:

```rust
    summary_provider: Option<Arc<dyn AiProvider>>,
```

Initialize in `new()`:

```rust
    summary_provider: None,
```

Add builder:

```rust
    /// Attach a lightweight provider for async tool use summaries.
    ///
    /// The provider should be configured with a fast, cheap model (e.g., Haiku).
    /// Summary generation is fire-and-forget — failures are silent.
    pub fn with_summary_provider(mut self, provider: Arc<dyn AiProvider>) -> Self {
        self.summary_provider = Some(provider);
        self
    }
```

- [ ] **Step 2: Add fire-and-forget summary after tool execution**

In `run_with_history_messages`, add state variable at top:

```rust
        let mut pending_summary: Option<tokio::task::JoinHandle<Option<String>>> = None;
```

After the tool batch execution block (after `tool_calls_made += outcomes.len();`, around line 639), add:

```rust
                // Fire-and-forget: generate tool summary in background
                if let Some(ref sp) = self.summary_provider {
                    let inputs: Vec<super::tool_summary::ToolSummaryInput> = outcomes
                        .iter()
                        .map(|o| super::tool_summary::ToolSummaryInput {
                            tool_name: o.tool_name.clone(),
                            tool_input: o.tool_input_text.clone().unwrap_or_default(),
                            tool_output: o.output_text.clone(),
                        })
                        .collect();
                    let sp = sp.clone();
                    let last_text = intermediate_texts.last().cloned();
                    pending_summary = Some(tokio::spawn(async move {
                        super::tool_summary::generate_tool_summary(
                            &*sp,
                            &inputs,
                            last_text.as_deref(),
                        )
                        .await
                    }));
                }
```

At the start of each iteration (after `iterations += 1;`), resolve pending summary:

```rust
            // Resolve pending tool summary from previous iteration
            if let Some(handle) = pending_summary.take() {
                if let Ok(Some(summary)) = handle.await {
                    callback.on_tool_summary(&summary);
                }
            }
```

- [ ] **Step 3: Check that ToolOutcome has tool_input_text field**

Run: `grep -n 'tool_input_text\|struct ToolOutcome' /Volumes/TBU/Workspace/Aleph/src/agent_loop/tool_orchestrator.rs | head -10`

If `tool_input_text` doesn't exist on `ToolOutcome`, add it or use the tool call's `arguments` field serialized to string. Adapt the mapping accordingly.

- [ ] **Step 4: Update mod.rs**

Add module declaration:

```rust
pub mod tool_summary;
```

Add re-exports:

```rust
pub use tool_summary::{generate_tool_summary, ToolSummaryInput};
```

- [ ] **Step 5: Run cargo check**

Run: `cargo check -p alephcore`
Expected: No errors

- [ ] **Step 6: Run all agent_loop tests**

Run: `cargo test -p alephcore --lib -- agent_loop`
Expected: All PASS

- [ ] **Step 7: Commit**

```
feat(loop_core): integrate fire-and-forget tool use summary
```

---

### Task 11: Final integration test and cleanup

**Files:**
- Modify: `src/agent_loop/loop_core.rs` (test section)

- [ ] **Step 1: Write a comprehensive integration test**

```rust
#[tokio::test]
async fn test_tier2_all_callbacks_fire() {
    // Verify that all new callback methods are callable and don't panic
    #[derive(Default)]
    struct Tier2Callback {
        fallback_fired: bool,
        stop_hook_block_fired: bool,
        stop_hook_error_fired: bool,
        summary_fired: bool,
    }

    impl LoopCallback for Tier2Callback {
        fn on_model_fallback(&mut self, reason: &str, model: &str) {
            assert!(!reason.is_empty());
            assert!(!model.is_empty());
            self.fallback_fired = true;
        }
        fn on_stop_hook_block(&mut self, reason: &str) {
            assert!(!reason.is_empty());
            self.stop_hook_block_fired = true;
        }
        fn on_stop_hook_error(&mut self, name: &str, error: &str) {
            assert!(!name.is_empty());
            assert!(!error.is_empty());
            self.stop_hook_error_fired = true;
        }
        fn on_tool_summary(&mut self, summary: &str) {
            assert!(!summary.is_empty());
            self.summary_fired = true;
        }
    }

    // Basic smoke test — just verify the callback trait compiles and works
    let mut cb = Tier2Callback::default();
    cb.on_model_fallback("test reason", "test-model");
    cb.on_stop_hook_block("test block");
    cb.on_stop_hook_error("test-hook", "test error");
    cb.on_tool_summary("Searched for bugs");

    assert!(cb.fallback_fired);
    assert!(cb.stop_hook_block_fired);
    assert!(cb.stop_hook_error_fired);
    assert!(cb.summary_fired);
}
```

- [ ] **Step 2: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All PASS

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: No warnings

- [ ] **Step 4: Commit**

```
test(agent_loop): add Tier 2 integration tests and callback verification
```

---

## Summary

| Task | Feature | Batch | Files |
|------|---------|-------|-------|
| 1 | CompactAndRetry + parse_token_gap | 1 | retry.rs |
| 2 | Fallback + classify_exhausted_error | 1 | retry.rs |
| 3 | emergency_truncate | 1 | loop_core.rs |
| **5** | **Fallback fields + LoopCallback** | **1** | **loop_core.rs (do before Task 4)** |
| 4 | 413 recovery + fallback in Think step | 1 | loop_core.rs |
| 6 | Factory + mod.rs updates | 1 | factory.rs, mod.rs |
| 7 | stop_hooks.rs module | 2 | stop_hooks.rs (new) |
| 8 | Stop hooks integration | 2 | loop_core.rs, mod.rs |
| 9 | tool_summary.rs module | 2 | tool_summary.rs (new) |
| 10 | Tool summary integration | 2 | loop_core.rs, mod.rs |
| 11 | Final integration + cleanup | 2 | loop_core.rs |
