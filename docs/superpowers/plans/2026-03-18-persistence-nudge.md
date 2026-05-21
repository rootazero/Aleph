# Persistence Nudge Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent the agent loop from prematurely terminating when tool errors occur, by adding a persistence nudge that challenges the LLM to reconsider before giving up.

**Architecture:** When the LLM returns EndTurn with no tool calls but `consecutive_errors > 0`, inject a nudge message asking it to reconsider. The nudge fires at most once per loop run. Also strengthen BASE_BEHAVIOR prompt and make `retryable` field meaningful.

**Tech Stack:** Rust, existing agent_loop module

---

## File Map

- Modify: `src/agent_loop/loop_core.rs` — add nudge logic at EndTurn check, make retryable meaningful
- Modify: `src/agent_loop/prompt_builder.rs` — strengthen BASE_BEHAVIOR persistence instructions
- Test: `src/agent_loop/loop_core.rs` (inline tests)

---

### Task 1: Strengthen BASE_BEHAVIOR prompt

**Files:**
- Modify: `src/agent_loop/prompt_builder.rs:27-34`

- [ ] **Step 1: Update BASE_BEHAVIOR constant**

Replace the current BASE_BEHAVIOR with stronger persistence instructions. Add explicit rules about never giving up after tool errors, trying alternative approaches, and only stopping when the task is verifiably complete.

```rust
const BASE_BEHAVIOR: &str = "\
- ALWAYS use available tools to gather information and take actions. Do NOT answer from memory or guess when a tool can provide the answer.\n\
- When the user asks you to do something and a matching tool exists, call it immediately rather than describing what you would do.\n\
- Continue working until the user's request is fully resolved. Chain multiple tool calls if needed.\n\
- **PERSISTENCE IS YOUR CORE TRAIT.** When a tool call fails:\n\
  1. Analyze the error carefully.\n\
  2. Retry with corrected parameters or a different approach.\n\
  3. If that fails, try a completely different strategy to achieve the same goal.\n\
  4. NEVER give up after just 1-2 attempts. You have up to 100 iterations — use them.\n\
  5. Only stop if you have genuinely exhausted all possible approaches AND explained what you tried.\n\
- **NEVER end your turn with an apology about failures without having tried at least 3 different approaches.** The user trusts you to be tenacious.\n\
- Provide concise summaries of actions taken and results obtained.\n\
- For web browsing: ALWAYS use browser_open/browser_snapshot/browser_click etc. to open URLs and interact with web pages. Do NOT use the desktop tool to launch a browser application. The browser_open tool runs a headless browser by default (fast, no visible window). Only use profile=\"user\" when the user explicitly asks to open a real/visible browser (e.g. mentions \"devtool\", \"Chrome\", \"open browser window\"). If a browser tool fails, wait briefly and retry — browser operations are inherently flaky and retrying usually works.\n\
- NEVER use the system Python directly. Aleph has a shared virtual environment at `~/.aleph/.venv/`. Use it for all global tools, packages, and quick scripts: `source ~/.aleph/.venv/bin/activate && uv pip install <packages>`. If the venv does not exist, create it first: `uv venv ~/.aleph/.venv`. For standalone Python projects, create `.venv` inside the project directory under the workspace.";
```

- [ ] **Step 2: Run tests to verify prompt still builds correctly**

Run: `cargo test -p alephcore --lib prompt_builder -- --nocapture`
Expected: All prompt_builder tests PASS

- [ ] **Step 3: Commit**

```bash
git add src/agent_loop/prompt_builder.rs
git commit -m "agent_loop: strengthen BASE_BEHAVIOR persistence instructions"
```

---

### Task 2: Make retryable field meaningful in loop

**Files:**
- Modify: `src/agent_loop/loop_core.rs:294-304`

- [ ] **Step 1: Write failing test — retryable errors don't increment consecutive_errors**

Add test `test_retryable_errors_dont_count` that verifies: when a tool returns `ToolResult::Error { retryable: true }`, `consecutive_errors` does not increment toward the MAX_CONSECUTIVE_ERRORS limit.

```rust
#[tokio::test]
async fn test_retryable_errors_dont_count_toward_limit() {
    // Setup: 12 retryable errors followed by EndTurn
    // The loop should NOT hit the 10-consecutive-errors limit
    // because retryable errors don't count
    let mut responses: Vec<ProviderResponse> = (0..12)
        .map(|i| ProviderResponse {
            text: None,
            tool_calls: vec![NativeToolCall {
                id: format!("call_{}", i),
                name: "fail_retryable".to_string(),
                arguments: json!({}),
            }],
            thinking: None,
            stop_reason: StopReason::ToolUse,
            usage: None,
        })
        .collect();
    responses.push(ProviderResponse::text_only("Gave up.".to_string()));

    // ... (uses RetryableFailTool that returns retryable: true)
    // Assert: hit_limit is false (did not hit consecutive error limit)
    // Assert: iterations == 13 (12 retries + final text)
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib test_retryable_errors_dont_count -- --nocapture`
Expected: FAIL (currently retryable errors DO count)

- [ ] **Step 3: Modify loop to skip consecutive_errors increment for retryable errors**

In `loop_core.rs` line 294, change the Error arm:

```rust
ToolResult::Error { error, retryable } => {
    if !retryable {
        consecutive_errors += 1;
    }
    // retryable errors don't count toward the hard limit
    if consecutive_errors >= MAX_CONSECUTIVE_ERRORS { ... }
    (error.clone(), true, false)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib test_retryable_errors_dont_count -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/loop_core.rs
git commit -m "agent_loop: retryable errors skip consecutive-error counter"
```

---

### Task 3: Add persistence nudge mechanism

**Files:**
- Modify: `src/agent_loop/loop_core.rs:164-216`

- [ ] **Step 1: Write failing test — nudge fires when EndTurn follows errors**

Add test `test_persistence_nudge_on_premature_stop` that verifies: when the LLM returns EndTurn after tool errors without having recovered, a nudge message is injected and the loop continues for one more LLM call.

```rust
#[tokio::test]
async fn test_persistence_nudge_on_premature_stop() {
    let captured = Arc::new(Mutex::new(Vec::<Vec<UnifiedMessage>>::new()));
    let provider = CapturingMockProvider::new(
        vec![
            // Turn 1: call a tool that fails
            ProviderResponse {
                text: None,
                tool_calls: vec![NativeToolCall {
                    id: "call_1".to_string(),
                    name: "fail".to_string(),
                    arguments: json!({}),
                }],
                thinking: None,
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
            // Turn 2: LLM tries to give up (text, no tools, EndTurn)
            ProviderResponse {
                text: Some("Sorry, the tool failed.".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
            // Turn 3: After nudge, LLM retries with the tool
            ProviderResponse {
                text: None,
                tool_calls: vec![NativeToolCall {
                    id: "call_2".to_string(),
                    name: "echo".to_string(),
                    arguments: json!({ "message": "retried" }),
                }],
                thinking: None,
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
            // Turn 4: Final success
            ProviderResponse {
                text: Some("Done after retry.".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ],
        captured.clone(),
    );

    let mut registry = LoopToolRegistry::new();
    registry.register(Box::new(FailTool));
    registry.register(Box::new(EchoTool));
    let agent = AgentLoop::new(
        provider,
        registry,
        PromptBuilder::new(),
        SafetyGuard::default_guard(),
        LoopConfig {
            max_iterations: 10,
            token_budget: 100_000,
            timeout_secs: 60,
        },
    );

    let mut cb = TrackingCallback::default();
    let result = agent.run("do something", &mut cb).await.unwrap();

    // The loop continued past the first EndTurn thanks to the nudge
    assert_eq!(result.iterations, 4);
    assert_eq!(result.final_text.as_deref(), Some("Done after retry."));
    assert!(!result.hit_limit);

    // Verify the nudge message was injected
    let caps = captured.lock().unwrap();
    // The 3rd call's messages should contain a user nudge message
    let third_call_msgs = &caps[2];
    let has_nudge = third_call_msgs.iter().any(|m| {
        if let UnifiedMessage::User { content } = m {
            content.iter().any(|b| {
                if let ContentBlock::Text { text } = b {
                    text.contains("not yet fully resolved")
                } else {
                    false
                }
            })
        } else {
            false
        }
    });
    assert!(has_nudge, "Expected a persistence nudge message");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib test_persistence_nudge -- --nocapture`
Expected: FAIL (nudge doesn't exist yet)

- [ ] **Step 3: Implement the persistence nudge**

Add state variable `nudge_sent: bool = false` alongside other loop state. At the EndTurn check (line 208):

```rust
// If no tool calls and EndTurn → check if we should nudge
if !response.has_tool_calls() && response.stop_reason == StopReason::EndTurn {
    if consecutive_errors > 0 && !nudge_sent {
        // Persistence nudge: challenge the LLM to try harder
        nudge_sent = true;
        consecutive_errors = 0; // reset for the retry phase
        messages.push(UnifiedMessage::user(
            "[SYSTEM] Your task is not yet fully resolved — the last tool calls failed \
             and you stopped without a successful outcome. Do NOT apologize or explain \
             the failure. Instead: try a different approach, use different parameters, \
             or attempt an alternative strategy to complete the user's original request. \
             If you have genuinely exhausted ALL possible approaches, explain each \
             approach you tried and why it failed."
        ));
        continue; // skip the break, give the LLM another chance
    }
    break;
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib test_persistence_nudge -- --nocapture`
Expected: PASS

- [ ] **Step 5: Write test — nudge only fires once**

```rust
#[tokio::test]
async fn test_persistence_nudge_fires_only_once() {
    // Setup: fail → EndTurn (nudge fires) → fail again → EndTurn (no nudge, loop stops)
    // Assert: iterations == 4 (not infinite loop)
}
```

- [ ] **Step 6: Run all tests**

Run: `cargo test -p alephcore --lib loop_core -- --nocapture`
Expected: ALL PASS

- [ ] **Step 7: Commit**

```bash
git add src/agent_loop/loop_core.rs
git commit -m "agent_loop: add persistence nudge when LLM gives up after errors"
```

---

### Task 4: Final verification

- [ ] **Step 1: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests pass (except known pre-existing failures in markdown_skill)

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | head -30`
Expected: No new warnings

- [ ] **Step 3: Final commit (if any fixups needed)**
