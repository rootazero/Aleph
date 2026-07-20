# Intermediate Message Delivery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver LLM-generated intermediate progress messages to users immediately during multi-step agent loop execution, instead of buffering everything until completion.

**Architecture:** Add `on_intermediate_text()` to `LoopCallback` trait, distinguish intermediate vs. final text in the agent loop based on `has_tool_calls()`, add `is_intermediate` field to `ResponseChunk` event, and have `ReplyEmitter` send intermediate chunks immediately. Also add prompt guidance to `BASE_BEHAVIOR` and SOUL.md templates.

**Tech Stack:** Rust, serde, tokio, async-trait

**Spec:** `docs/superpowers/specs/2026-03-20-intermediate-message-delivery-design.md`

---

### Task 1: Add `is_intermediate` field to `ResponseChunk` event

**Files:**
- Modify: `src/gateway/event_emitter/types.rs:93-99`

This is the data type change that everything else depends on. All existing code that constructs `ResponseChunk` must be updated to include the new field.

- [ ] **Step 1: Add `is_intermediate` field to `ResponseChunk`**

In `src/gateway/event_emitter/types.rs`, find the `ResponseChunk` variant (line 93-99):

```rust
    /// Response text chunk (streaming output)
    ResponseChunk {
        run_id: String,
        seq: u64,
        content: String,
        chunk_index: u32,
        is_final: bool,
    },
```

Replace with:

```rust
    /// Response text chunk (streaming output)
    ResponseChunk {
        run_id: String,
        seq: u64,
        content: String,
        chunk_index: u32,
        is_final: bool,
        /// When true, this chunk should be sent to the user immediately as a
        /// standalone message (intermediate progress update during multi-step
        /// execution). When false, the chunk is buffered per existing behavior.
        #[serde(default)]
        is_intermediate: bool,
    },
```

- [ ] **Step 2: Fix ALL existing `ResponseChunk` construction sites (10 total)**

Add `is_intermediate: false` to every existing `ResponseChunk` construction. Here is the complete list:

**File 1: `src/gateway/execution_engine/run_loop.rs:270`** — `StreamCallback::on_text()`
```rust
            is_final: false,
            is_intermediate: false,  // ADD THIS
```

**File 2: `src/gateway/execution_engine/engine.rs:569`** — `finalize_fast_path_error()`
```rust
                is_final: true,
                is_intermediate: false,  // ADD THIS
```

**File 3: `src/gateway/event_emitter/mod.rs:100`** — `emit_response_chunk()` helper
```rust
                is_final,
                is_intermediate: false,  // ADD THIS
```

**File 4: `src/gateway/event_emitter/impls.rs:155`** — `GatewayEventEmitter` instant-mode final reconstruction
```rust
                    is_final: true,
                    is_intermediate: false,  // ADD THIS
```

**File 5: `src/gateway/event_emitter/tests.rs:245,255,271`** — 3 constructions in tests
Add `is_intermediate: false` to each of the 3 `ResponseChunk` constructions in the test file.

**File 6: `src/gateway/execution_engine/simple.rs:219,231`** — 2 constructions in simple engine
Add `is_intermediate: false` to each of the 2 `ResponseChunk` constructions.

**File 7: `src/gateway/execution_engine/slash_command.rs:171,232`** — 2 constructions in slash command handler
Add `is_intermediate: false` to each of the 2 `ResponseChunk` constructions.

Verify completeness: `grep -rn "ResponseChunk {" src/ | grep -v "matches!\|\.\.\|ref " | wc -l` should equal 10 construction sites.

- [ ] **Step 3: Update `GatewayEventEmitter` instant-mode to pass through intermediate chunks**

In `src/gateway/event_emitter/impls.rs:131-143`, the instant-mode logic buffers all non-final `ResponseChunk` events. After this change, `is_intermediate: true` chunks must NOT be buffered — they should be emitted immediately (same as typewriter mode).

Find (line 131-143):

```rust
            if let StreamEvent::ResponseChunk {
                ref content,
                is_final,
                ref run_id,
                ..
            } = event
            {
                if !is_final {
                    // Buffer the chunk content, don't emit yet
                    self.instant_buffer.lock().await.push_str(content);
                    return Ok(());
                }
```

Replace with:

```rust
            if let StreamEvent::ResponseChunk {
                ref content,
                is_final,
                is_intermediate,
                ref run_id,
                ..
            } = event
            {
                // Intermediate chunks bypass buffering — emit immediately
                if is_intermediate {
                    // Fall through to default broadcast below
                } else if !is_final {
                    // Buffer the chunk content, don't emit yet
                    self.instant_buffer.lock().await.push_str(content);
                    return Ok(());
                }
```

The `else` block (lines 145-168, `is_final` handling) remains unchanged.

- [ ] **Step 4: Compile check**

Run: `cargo check -p alephcore`

Expected: compiles successfully with no errors.

- [ ] **Step 5: Run existing tests**

Run: `cargo test -p alephcore --lib -- event_emitter`

Expected: all existing tests pass (serde default handles backward compat).

- [ ] **Step 6: Commit**

```bash
git add src/gateway/event_emitter/types.rs src/gateway/event_emitter/mod.rs src/gateway/event_emitter/impls.rs src/gateway/event_emitter/tests.rs src/gateway/execution_engine/run_loop.rs src/gateway/execution_engine/engine.rs src/gateway/execution_engine/simple.rs src/gateway/execution_engine/slash_command.rs
git commit -m "gateway: add is_intermediate field to ResponseChunk event"
```

---

### Task 2: Add `on_intermediate_text` to `LoopCallback` and change dispatch logic

**Files:**
- Modify: `src/agent_loop/loop_core.rs:102-107` (trait definition)
- Modify: `src/agent_loop/loop_core.rs:241-244` (text dispatch in loop)

- [ ] **Step 1: Write the failing test**

In `src/agent_loop/loop_core.rs`, find the `TrackingCallback` in the test module (around line 578) and add an `intermediate_texts` field:

```rust
    #[derive(Default)]
    struct TrackingCallback {
        texts: Vec<String>,
        intermediate_texts: Vec<String>,  // ADD THIS
        tool_starts: Vec<String>,
        tool_dones: Vec<String>,
        safety_blocks: Vec<String>,
    }
```

And implement the new callback method (after the existing `on_text` impl, around line 588):

```rust
        fn on_intermediate_text(&mut self, text: &str) {
            self.intermediate_texts.push(text.to_string());
        }
```

Then add a new test at the end of the test module:

```rust
    // =========================================================================
    // L14: Intermediate text callback for tool-accompanied responses
    // =========================================================================

    #[tokio::test]
    async fn test_intermediate_text_with_tool_calls() {
        let provider = MockProvider::new(vec![
            // Turn 1: text + tool call → should be intermediate
            ProviderResponse {
                text: Some("Let me search for that...".to_string()),
                tool_calls: vec![NativeToolCall {
                    id: "call_1".to_string(),
                    name: "echo".to_string(),
                    arguments: json!({ "message": "search" }),
                }],
                thinking: None,
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
            // Turn 2: text only → should be final
            ProviderResponse {
                text: Some("Here are the results.".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);

        let agent = make_loop(provider);
        let mut cb = TrackingCallback::default();
        let result = agent.run("find something", &mut cb).await.unwrap();

        assert_eq!(result.iterations, 2);
        // Intermediate text goes to intermediate_texts, not texts
        assert_eq!(cb.intermediate_texts, vec!["Let me search for that..."]);
        // Final text goes to texts
        assert_eq!(cb.texts, vec!["Here are the results."]);
        // final_text should be the last text produced
        assert_eq!(result.final_text.as_deref(), Some("Here are the results."));
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p alephcore --lib -- test_intermediate_text_with_tool_calls`

Expected: FAIL — `intermediate_texts` is empty, `texts` contains both strings (because `on_intermediate_text` is never called yet).

- [ ] **Step 3: Add `on_intermediate_text` to LoopCallback trait**

In `src/agent_loop/loop_core.rs`, find the `LoopCallback` trait (line 102-107):

```rust
pub trait LoopCallback: Send {
    fn on_text(&mut self, _text: &str) {}
    fn on_tool_start(&mut self, _name: &str, _input: &Value) {}
    fn on_tool_done(&mut self, _name: &str, _result: &ToolResult) {}
    fn on_safety_block(&mut self, _error: &SafetyError) {}
}
```

Replace with:

```rust
pub trait LoopCallback: Send {
    fn on_text(&mut self, _text: &str) {}
    /// Called when the LLM produces text alongside tool calls (intermediate progress).
    /// This text should be delivered to the user immediately, not buffered.
    fn on_intermediate_text(&mut self, _text: &str) {}
    fn on_tool_start(&mut self, _name: &str, _input: &Value) {}
    fn on_tool_done(&mut self, _name: &str, _result: &ToolResult) {}
    fn on_safety_block(&mut self, _error: &SafetyError) {}
}
```

- [ ] **Step 4: Change text dispatch logic in the loop**

In `src/agent_loop/loop_core.rs`, find the text processing block (line 241-244):

```rust
            // Process text output
            if let Some(text) = &response.text {
                callback.on_text(text);
                final_text = Some(text.clone());
            }
```

Replace with:

```rust
            // Process text output
            if let Some(text) = &response.text {
                if response.has_tool_calls() {
                    // Intermediate: LLM said something AND requested tools → still working
                    callback.on_intermediate_text(text);
                } else {
                    // Final: LLM said something with no tool calls → done
                    callback.on_text(text);
                }
                final_text = Some(text.clone());
            }
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p alephcore --lib -- test_intermediate_text_with_tool_calls`

Expected: PASS

- [ ] **Step 6: Run ALL existing agent loop tests to verify no regressions**

Run: `cargo test -p alephcore --lib -- agent_loop::loop_core::tests`

Expected: all tests pass. Note that existing tests like `test_tool_call_then_response` and `test_assistant_message_completeness` have intermediate iterations where LLM returned text with tool_calls. In those tests, the text now goes to `intermediate_texts` instead of `texts`. Verify these tests still pass — they assert on `final_text` and `iterations`, not on `cb.texts` for intermediate steps. The one test that checks `cb.texts` is `test_simple_text_response` which has no tool calls (EndTurn), so it's unaffected.

- [ ] **Step 7: Commit**

```bash
git add src/agent_loop/loop_core.rs
git commit -m "agent_loop: add on_intermediate_text callback, dispatch based on has_tool_calls"
```

---

### Task 3: Implement `on_intermediate_text` in `StreamCallback`

**Files:**
- Modify: `src/gateway/execution_engine/run_loop.rs:262-283`

- [ ] **Step 1: Add `on_intermediate_text` implementation to `StreamCallback`**

In `src/gateway/execution_engine/run_loop.rs`, find the `LoopCallback` impl for `StreamCallback` (starts around line 262). After the `on_text` method (ends around line 283), add:

```rust
    fn on_intermediate_text(&mut self, text: &str) {
        self.seq += 1;
        let chunk_index = self.chunk_index;
        self.chunk_index += 1;

        let event = StreamEvent::ResponseChunk {
            run_id: self.run_id.clone(),
            seq: self.seq,
            content: text.to_string(),
            chunk_index,
            is_final: false,
            is_intermediate: true,
        };

        // Fire-and-forget emit (LoopCallback is sync, emitter is async)
        let emitter = self.emitter.clone();
        tokio::spawn(async move {
            let _ = emitter.emit(event).await;
        });
    }
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p alephcore`

Expected: compiles successfully.

- [ ] **Step 3: Commit**

```bash
git add src/gateway/execution_engine/run_loop.rs
git commit -m "execution_engine: implement on_intermediate_text in StreamCallback"
```

---

### Task 4: Update `ReplyEmitter` to send intermediate messages immediately

**Files:**
- Modify: `src/gateway/reply_emitter.rs:362-381`

- [ ] **Step 1: Update `ResponseChunk` handling in `emit()`**

In `src/gateway/reply_emitter.rs`, find the `ResponseChunk` match arm in `emit()` (line 362-381):

```rust
            StreamEvent::ResponseChunk {
                content, is_final, ..
            } => {
                // Both modes: accumulate text into buffer
                if !content.is_empty() {
                    self.buffer.lock().await.push_str(&content);
                }

                // Instant mode: flush on final chunk
                if !self.config.stream_enabled && is_final {
                    let mut buffer = self.buffer.lock().await;
                    if !buffer.is_empty() {
                        let text = std::mem::take(&mut *buffer);
                        drop(buffer);
                        self.send_to_channel(&text).await;
                    }
                }
                // Typewriter mode: do nothing here, wait for RunComplete
            }
```

Replace with:

```rust
            StreamEvent::ResponseChunk {
                content, is_final, is_intermediate, ..
            } => {
                if is_intermediate {
                    // Intermediate message: send immediately as a standalone message
                    if !content.is_empty() {
                        self.send_to_channel(&content).await;
                    }
                    // Do NOT buffer — this is a separate message from the final response
                } else {
                    // Existing behavior: accumulate text into buffer
                    if !content.is_empty() {
                        self.buffer.lock().await.push_str(&content);
                    }

                    // Instant mode: flush on final chunk
                    if !self.config.stream_enabled && is_final {
                        let mut buffer = self.buffer.lock().await;
                        if !buffer.is_empty() {
                            let text = std::mem::take(&mut *buffer);
                            drop(buffer);
                            self.send_to_channel(&text).await;
                        }
                    }
                    // Typewriter mode: do nothing here, wait for RunComplete
                }
            }
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p alephcore`

Expected: compiles successfully.

- [ ] **Step 3: Run existing reply_emitter tests**

Run: `cargo test -p alephcore --lib -- reply_emitter`

Expected: all existing tests pass (they don't construct `ResponseChunk` events directly).

- [ ] **Step 4: Commit**

```bash
git add src/gateway/reply_emitter.rs
git commit -m "reply_emitter: send intermediate ResponseChunks immediately instead of buffering"
```

---

### Task 5: Add intermediate message guidance to `BASE_BEHAVIOR`

**Files:**
- Modify: `src/agent_loop/prompt_builder.rs:27-40`

- [ ] **Step 1: Add the behavioral rule**

In `src/agent_loop/prompt_builder.rs`, find the `BASE_BEHAVIOR` constant. After the line (line 38):

```
- Provide concise summaries of actions taken and results obtained.\n\
```

Insert the following new line:

```
- **KEEP THE USER INFORMED.** When you need to execute multiple steps, briefly tell the user what you're about to do before each tool call. Use natural, conversational language — do NOT expose raw tool names, parameters, or JSON. For example: \"Let me check your calendar...\" or \"I'll search for that file now.\" This helps the user understand your progress and know you're still working.\n\
```

The insertion point is between the "concise summaries" line and the "For web browsing" line.

- [ ] **Step 2: Compile check**

Run: `cargo check -p alephcore`

Expected: compiles successfully.

- [ ] **Step 3: Commit**

```bash
git add src/agent_loop/prompt_builder.rs
git commit -m "prompt: add KEEP THE USER INFORMED rule to BASE_BEHAVIOR"
```

---

### Task 6: Add "Communication" section to `default_soul()` template

**Files:**
- Modify: `src/config/agent_resolver.rs:388-425`

- [ ] **Step 1: Add Communication section to `default_soul()` template**

In `src/config/agent_resolver.rs`, find `default_soul()` (line 388). In the format string, insert a new section between "## Vibe" and "## Continuity". Find:

```
## Vibe

Be the assistant you'd actually want to talk to. Concise when needed, thorough when it matters. Not a corporate drone. Not a sycophant. Just... good.

## Continuity
```

Replace with:

```
## Vibe

Be the assistant you'd actually want to talk to. Concise when needed, thorough when it matters. Not a corporate drone. Not a sycophant. Just... good.

## Communication

**Show your work.** When tackling multi-step tasks, narrate your progress naturally — what you're doing, what you found, what's next. Don't go silent for long stretches. The user should feel like they're working _with_ you, not waiting _for_ you.

## Continuity
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p alephcore`

Expected: compiles successfully.

- [ ] **Step 3: Commit**

```bash
git add src/config/agent_resolver.rs
git commit -m "config: add Communication section to default_soul() template"
```

---

### Task 7: Update existing agent SOUL.md files

**Files:**
- Modify: `~/.aleph/agents/main/SOUL.md`
- Modify: `~/.aleph/agents/codev/SOUL.md`
- Modify: `~/.aleph/agents/coding/SOUL.md`
- Modify: `~/.aleph/agents/cowork/SOUL.md`
- Modify: `~/.aleph/agents/agent-73f36847/SOUL.md`

All 5 files have the same template structure. Insert the "Communication" section between "## Vibe" and "## Continuity" in each file.

- [ ] **Step 1: Update all 5 agent SOUL.md files**

In each file, find:

```markdown
## Vibe

Be the assistant you'd actually want to talk to. Concise when needed, thorough when it matters. Not a corporate drone. Not a sycophant. Just... good.

## Continuity
```

Replace with:

```markdown
## Vibe

Be the assistant you'd actually want to talk to. Concise when needed, thorough when it matters. Not a corporate drone. Not a sycophant. Just... good.

## Communication

**Show your work.** When tackling multi-step tasks, narrate your progress naturally — what you're doing, what you found, what's next. Don't go silent for long stretches. The user should feel like they're working _with_ you, not waiting _for_ you.

## Continuity
```

The files to modify:
1. `~/.aleph/agents/main/SOUL.md`
2. `~/.aleph/agents/codev/SOUL.md`
3. `~/.aleph/agents/coding/SOUL.md`
4. `~/.aleph/agents/cowork/SOUL.md`
5. `~/.aleph/agents/agent-73f36847/SOUL.md`

- [ ] **Step 2: Verify the changes**

Run: `grep -l "Communication" ~/.aleph/agents/*/SOUL.md | wc -l`

Expected: `5`

- [ ] **Step 3: No commit needed**

These are user data files outside the git repo. No commit required.

---

### Task 8: Full integration verification

- [ ] **Step 1: Run all core tests**

Run: `cargo test -p alephcore --lib`

Expected: all tests pass, including the new `test_intermediate_text_with_tool_calls`.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | head -30`

Expected: no new warnings.

- [ ] **Step 3: Final commit (if any clippy fixes needed)**

If clippy found issues, fix them and commit:

```bash
git commit -am "fix: address clippy warnings from intermediate message changes"
```
