# Findings — EE1 (execution engine — execute core, tool dispatch)

Files reviewed: 19 (12940 LOC)

Counts: P0= 1 P1= 2 P2= 1 P3= 0

### EE1-01 [P0] Terminal trace bypasses secret masking through the tool timeline

- **Location**: `src/gateway/execution_engine/unattended_redacting_sink.rs:113` (`mask_trace_event`, `SessionCompleted` arm)
- **Failure mode**: An unattended run encounters a tool error containing a recognizable credential, such as `AKIAIOSFODNN7EXAMPLE` → the individual `ToolCallCompleted.result.error` is masked, but the same original error survives in `SessionCompleted.tool_timeline[*].error` and is persisted in `task_traces`. The terminal arm masks only `final_text`, not the nested timeline, violating this sink's stated every-text-leaf contract. The live-variant test fixtures all use an empty terminal timeline and therefore do not exercise this path. This finding concerns persisted terminal traces, not an assertion that `SessionCompleted` is published as a live step event.
- **Evidence**:
  > `tool_timeline: _,`
  > `final_text`
  > `.is_some_and(|t| mask_in_place(masker, t)),`

  Cross-module contract checked: `src/harness/agent/act.rs:1255-1276` copies the original tool error into the timeline after emitting the separately redacted event; `src/harness/agent.rs:330-337` stores that error unchanged. `src/harness/trace.rs:79` defines the terminal timeline as `Vec<ToolInvocation>`, whose `error: Option<String>` is defined in `src/orchestrator/dispatch.rs:438`. `src/gateway/trace_protocol.rs:88-119` serializes the timeline into the protocol trace; `src/gateway/execution_engine/callback.rs:27-31` persists it. `src/gateway/execution_engine/run_trace_sinks.rs:130-133` installs this redactor above that persistence path.
- **Suggested fix**: Mask the terminal timeline's string fields, including every optional error, before forwarding the event; likewise explicitly handle text-bearing termination-reason fields rather than treating structured payloads as text-free. Preserve the existing trace shape, tool/result correlation and attended-run behavior. Add a terminal fixture with a planted secret in a nonempty timeline and assert that the persisted/protocol trace contains no original credential.
- **Confidence**: 0.99

### EE1-02 [P1] A queued run cannot consume cancellation until it obtains a concurrency permit

- **Location**: `src/gateway/execution_engine/gate.rs:357` (`ExecutionEngine::admit_run`)
- **Failure mode**: Existing runs exhaust the concurrency cap and another session's run parks in `concurrency.acquire()` → `/stop` can enqueue a cancellation signal, but the parked run retains its session claim and stays `Running` until an unrelated run releases a permit. It cannot cancel promptly even though it is registered as cancellable. The request deadline does not rescue this wait: `execute()` creates its timed execution select only after `admit_run` returns. With the capacity-one cancellation channel already full, a second stop request can itself block in `send().await` for the remainder of the same admission wait. The defect is cancellation latency/resource retention while queued; it does not require assuming that a cancelled run necessarily dispatches tools afterward.
- **Evidence**:
  > `slot._permit = Some(match self.concurrency.try_acquire(&agent_id) {`
  > `Some(p) => p,`
  > `None => self.concurrency.acquire(&agent_id).await,`

  Within the batch, `src/gateway/execution_engine/execute.rs:468` creates the receiver, `src/gateway/execution_engine/execute.rs:495` awaits admission, and its cancellation/deadline select is only at `src/gateway/execution_engine/execute.rs:883-912`. Cross-module contract checked: `src/gateway/execution_engine/engine.rs:520-534` clones the registered sender and awaits `cancel_tx.send(())`; it does not independently cancel the admission wait.
- **Suggested fix**: Make admission's permit wait select on the same run cancellation signal, releasing the session claim and removing/settling the registered run on cancellation. Keep the cancellation typed as `ExecutionError::Cancelled` and send the correctly scoped pre-acceptance receipt on that exit. Preserve agent-first/global-second permit acquisition and the current RAII permit lifetime; do not introduce a second cancellation authority.
- **Confidence**: 0.99

### EE1-03 [P1] Goal wake clears an unbound barrier before its fallible continuation claim

- **Location**: `src/gateway/execution_engine/goal_wait.rs:379` (`GoalWakeService::wake`)
- **Failure mode**: The wake service reads a goal waiting on task A, then awaits the task-store check; while it waits, the goal is changed to wait on task B → the A wake clears B's barrier because `clear_wait_barrier` accepts only a session id. For a same-goal barrier update, the later identity-bound claim still succeeds and resumes before B settles; for a replacement goal, the later claim rejects the old identity only after the replacement barrier has already been erased. There is also a lost-wake failure boundary: if clearing succeeds but the subsequent claim fails/refuses, the active goal has neither its barrier nor a pending continuation. Periodic/boot barrier scans no longer select it, so an unattended pursuit can remain stalled until another run happens to enter `post_run`.
- **Evidence**:
  > `match crate::goal::global().map(|s| s.clear_wait_barrier(&goal.session_id, now)) {`
  > `Some(Ok(true)) => {}`
  > `self.claim_and_spawn(`

  The real await window is `src/gateway/execution_engine/goal_wait.rs:354`; the later claim and its failure exits are `src/gateway/execution_engine/goal_wait.rs:420-448`. The periodic selector at `src/gateway/execution_engine/goal_wait.rs:329-338` requires a task barrier or an elapsed unclaimed timer. Cross-module contract checked: `src/goal/store.rs:1247-1255` checks only the current row's active/barrier predicates before writing `without_wait`, not the expected goal or expected barrier. `src/goal/store.rs:472-499` binds the subsequent claim to goal identity, but neither rolls back that earlier write nor validates the originally observed wait.
- **Suggested fix**: Move expected-goal/expected-barrier comparison, barrier consumption and continuation claim into one store operation/transaction. A superseded snapshot must change nothing; a failed/refused claim must leave a recoverable barrier. Preserve the existing once-only claim, creator-origin validation, budget checks and fire-time marker semantics; no new scheduler is needed.
- **Confidence**: 0.99

### EE1-04 [P2] Side-session seed failure bypasses registered-run and task cleanup

- **Location**: `src/gateway/execution_engine/execute.rs:547` (`ExecutionEngine::execute`, side-session seed error return)
- **Failure mode**: `/btw` reaches a side session with an uninterpretable `btw_seed_cursor`, or another seed read/append failure occurs → the function emits an error and returns after admission and task-start persistence, but before the normal terminal-state/cleanup sequence. `RunSlot` releases the actual session claim and concurrency permit, while the `ActiveRun` entry remains `Running` with its full request and cancellation sender, and the persisted task remains started/running. Repeating the sticky cursor failure accumulates entries without the normal five-second removal. Subsequent status/cancel/steering lookups can encounter these nonexecuting runs even though their actual session claims were released.
- **Evidence**:
  > `if let Err(e) = self.seed_side_session(main, &request).await {`
  > `self.emit_pre_admission_run_error(emitter.as_ref(), &run_id, main, &request, &e)`
  > `return Err(e);`

  Within the batch, registration is at `src/gateway/execution_engine/gate.rs:329-345`, task-start persistence at `src/gateway/execution_engine/execute.rs:500-502`, and `RunSlot::drop` at `src/gateway/execution_engine/gate.rs:69-72` only releases the session registry claim. This return bypasses terminal run-state assignment at `src/gateway/execution_engine/execute.rs:936-941`, terminal task persistence at `src/gateway/execution_engine/execute.rs:1095-1096`, and delayed entry removal at `src/gateway/execution_engine/execute.rs:1160-1163`. The nearby seed-error comment explicitly identifies the sticky malformed-cursor trigger.
- **Suggested fix**: Settle the registered run and persisted task on this early error path, removing or scheduling removal of the `ActiveRun` just as on ordinary failure. Prefer routing through the existing terminal cleanup rather than adding another run-state authority. Preserve the main-session-scoped sanitized error receipt and the current RAII release of the claim/permit.
- **Confidence**: 0.99

## Clean files

- `src/gateway/execution_engine/steering.rs`
- `src/gateway/execution_engine/event_drain.rs`
- `src/gateway/execution_engine/helpers.rs`
- `src/gateway/execution_engine/turn_permissions.rs`
- `src/gateway/execution_engine/goal_continuation.rs`
- `src/gateway/execution_engine/simple.rs`
- `src/gateway/execution_engine/run_trace_sinks.rs`
- `src/gateway/execution_engine/concurrency.rs`
- `src/gateway/execution_engine/btw_promote.rs`
- `src/gateway/execution_engine/turn_mode.rs`
- `src/gateway/execution_engine/callback.rs`
- `src/gateway/execution_engine/turn_model.rs`
- `src/gateway/execution_engine/turn_thinking.rs`
- `src/gateway/execution_engine/topic.rs`
- `src/gateway/execution_engine/tool_refresh.rs`
