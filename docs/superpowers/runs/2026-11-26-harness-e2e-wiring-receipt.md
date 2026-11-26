# Harness E2E wiring — execution receipt / 执行回执

- Plan / 计划: `docs/superpowers/plans/2026-11-26-harness-e2e-wiring.md`
- Spec / 设计: `docs/superpowers/specs/2026-11-26-harness-e2e-wiring-design.md`
- Branch / 分支: `harness-e2e-wiring` (worktree `.worktrees/harness-e2e-wiring`)
- Baseline build / 基线构建: HEAD `27f825f97` (product code identical to main `961f38dc1`); `cargo build -p alephcore --bin aleph-server` passed in 4m15s on macOS arm64.

## Environment notes / 环境说明

- The Python drivers import `websockets`, which the system `python3` (Homebrew 3.15) does not carry. Runs used a scratch venv created with `uv venv` plus `uv pip install 'websockets>=15'` (17.4), placed first on `PATH`. No fixture was changed for this.
  Python 驱动依赖 `websockets`；基线用 uv 建临时 venv 并放在 `PATH` 前面，未改装置。
- The worktree shares cargo's target dir (`.cargo/config.toml` pins `/Volumes/TBU4/Workspace/Aleph/target`). Any fixture that hardcodes `$REPO/target` misses the binary from a worktree.
  worktree 与主检出共享 target 目录。

## Baseline / 基线

| Fixture | Stage | First run | Final | Evidence / 证据 |
|---|---|---|---|---|
| `qa/busy_input` | burst-drain | rc=1 (stale fixture) | rc=0 | Fixture repair: `BIN` hardcoded `$REPO/target`; now resolved through `cargo metadata` as `qa/session_order/run.sh` does. 装置修复 |
| `qa/busy_input` | interrupt | rc=1 (same) | rc=0 | `PASS: interrupt reached the engine — the live run … was cancelled by a channel arrival (1 cancellation(s) for 3 arrival(s), spaced 1.2s)` |
| `qa/busy_input` | queue | rc=1 (same) | rc=0 | `PASS: 1 run(s), zero cancellations under queue` |
| `qa/busy_input` | steer | rc=0 (weak assertion) | **rc=1 → D1** | The old verdict asserted only zero cancellations, which a queue also shows. With the Task 1 Step 3 assertions added, the verdict reads `FAIL: steered text never reached a provider request: ['bravo: burst message 2', 'charlie: burst message 3', 'delta: burst message 4']` |
| `qa/resume_boundary` | knobs | rc=1 (stale fixture) | rc=0 | Fixture repair: `stampSessionMeta` assumed a sqlite `sessions` table in `data/sessions.db`. The session manager now follows `session_store_backend = "file"`, so the row is `data/sessions/<key>/metadata.json` (`identity_meta`). The stamp now writes whichever store holds the row. 10/10 assertions. 装置修复 |
| `qa/resume_boundary` | holes | rc=0 | rc=0 | `PASS [after] the finished run is billed exactly once — the heal pass added no tokens` |
| `qa/resume_boundary` | unanswered | rc=0 | rc=0 | `PASS STILL exactly one lost-input SystemMessage on that session` |
| `qa/session_order` | (all) | rc=0 | rc=0 | `PASS the same driven conversation reads identically on both backends` |
| `qa/run_halt` | crash | rc=0 | rc=0 | `[PASS] a failed run reports failed, not the enum's Default` |
| `qa/run_halt` | cap | rc=0 | rc=0 | `[PASS] a capped run is not reported as a clean finish — terminate_reason='budget_exhausted_partial_result'` |

### F3 steer assertion (Task 1 Step 3) / 本步插入断言

`qa/busy_input/drive_channel_busy.py` steer verdict now requires:

1. exactly one run id in `RunStarted`/`RunFinished`, before and after the wait; and
2. every burst text appears in `body.messages` of some logged provider request whose `run_turn` is greater than 1, i.e. after the run had started. It reads the mock's request log, which `qa/busy_input/run.sh` now passes as the mock's fifth argument and as `--request-log`.

### F5/F6 plan mode (Task 2) / 计划模式

`qa/plan_handoff` revived against `ExecTier::Plan` + `scratchpad{request_approval}` (was STALE, drove the removed `plan_phase` design). Run: `for s in handoff deny floor; do KEEP=1 ./qa/plan_handoff/run.sh $s; done`.

| Stage | Result | Claims that passed / 通过的断言 |
|---|---|---|
| handoff | rc=1 → **D2 only** | control run lists `file_write`; while planning `file_write` stays listed and is refused with `GateRule::PlanMode`'s sentence; `set_plan` runs; `request_approval` appears in `clarification.pending` (question id `plan_approval`); `clarification.resolve reply=approved` accepted; model told `Plan approved`; the same run's next `file_write` writes and the file is on disk; `chat.history` `session.exec_tier` is no longer `plan` |
| deny | rc=1 → **D2 only** | `reply=rejected` accepted; model told `Plan rejected`; `file_write` still listed and still refused; session tier still `plan` |
| floor | rc=1 → **D2 only** | with config `bash = "allow"`, `file_write = "allow"`: `bash` still refused by plan mode; `file_ops list` admitted; `file_ops delete` refused; session tier still `plan` |

Deviation from the plan: the plan asked to identify the refusing gate by id `plan_mode`. The id does not appear in model-facing text, so the fixture matches the opening of `GateRule::PlanMode::reason` (`This conversation is PLANNING`, `src/tools/scoped/gate_chain.rs`). 偏离：门的 id 不出现在模型可见文本中，改为匹配该规则的原因句。

### F6 approval order (Task 3) / 审批顺序

`cargo test -p alephcore --lib tools::scoped::tests::approval_order_matches_the_documented_five_stages` → PASS. Pinned today's order: (1) per-tool `deny` refuses with `GateRule::PolicyDeny`, no card, even at `exec_tier = full`; (2) `full` tier still asks for a tool that declares `requires_confirmation`, card rule `DECLARED_FLOOR_RULE` (`tool_declared`) — no YOLO below the floor, per design §8; (3) an `ApprovedForSession` grant suppresses the second card; (4) the same session unattended is refused (`RefusedBy::NobodyAsked`) before the grant is consulted; (5) attended, no grant → exactly one card.

### F1 reopen (Task 4) / 重开线程

`KEEP=1 ./qa/thread_continuity/run.sh reopen` → rc=0, 31 claims. Phase `before`: run 1 pins `qa-mock-model-b` through the `select_model` tool; `sessions.patch` sets `session_mode=code`, `exec_tier=auto`, `think_level=high`; run 2's `RunStarted` envelope reads `{exec_tier: auto, session_mode: code, think_level: high, model: qa-mock-model-b}` and its provider request carries `model=qa-mock-model-b`, `thinking={type: enabled, budget_tokens: 20000}` (control: run 1, before the patch, sent `thinking=None` and the default model). Server stopped with SIGTERM and restarted on the same `ALEPH_HOME`. Phase `after`: `chat.history` `result.session` fields `mode / exec_tier / think_level / memory_mode / model_pin / model_pin_provider / model / input_tokens(30) / output_tokens(36) / total_tokens(66)` and `session.usage` `tokens / input_tokens / output_tokens / messages(7) / cost_usd` all equal; `memory_mode` stays `null`; run 3 again runs under `auto / high / qa-mock-model-b` with the same thinking field.

Deviations from the plan / 偏离: (1) the model pin is set through the `select_model` tool, not `sessions.patch` — the latter refuses `model_pin` by design (`NOT_PATCHABLE`, `src/gateway/handlers/session/db_handlers/modify.rs`), because the pin has two stores that only `select_model` writes together. (2) `qa/busy_input/mock_anthropic.py` gained a `scripted` plan now rather than in Task 8: the tool-spec list is the whole conversation, entries are consumed only by requests that carry a tool surface (side-channel calls answer plain text), and a list entry emits several `tool_use` blocks in one turn. The existing plans now go through the same `_respond`; `./qa/busy_input/run.sh burst-drain` re-run rc=0 after the change.

### F2 checklist (Task 5) / 执行清单

`KEEP=1 ./qa/thread_continuity/run.sh checklist` → rc=0, 15 claims. Phase `before`: the mock sets an objective, a two-item plan, starts and completes item 0, then tries to finish. `ScratchpadGoalVerifier` vetoes the stop: the next provider request in the SAME run (one run on the server) carries "still has 1 incomplete step(s)" naming `QA_ITEM_B`; the mock then completes item 1 and the run ends. Each list-changing call (`set_plan`, `start_item`, both `complete_item`) reached the attached client as a live `stream.tool_end` frame whose output carries the snapshot; the run's `stream.run_complete` `summary.plan` and `chat.history` `result.plan` both show both items `completed`. Phase `after` (SIGTERM + restart, same `ALEPH_HOME`): `chat.history` returns the identical plan; a run that calls `scratchpad{action: clear}` leaves `result.plan = null`, and its `summary.plan` is an EMPTY snapshot (`items: []`).

Observation / 观察: `clear` itself returns no snapshot. Clients drop the stale list through two documented routes: the Panel hides on the `clear` action (`interfaces/webchat/src/platform/wide/views/chat/plan.rs` `scratchpad_plan_update`), and the TUI waits for the empty `RunSummary.plan` at run end (`interfaces/tui/src/tui/app/mod.rs` `maybe_apply_plan_from_tool`). So the TUI shows the old list until that run ends. This is by design, not a defect.

Deviation from the plan / 偏离: the script also calls `set_objective` first. The verifier is dormant without an objective, so without that call the veto claim would be vacuous.

### F8 two clients (Task 6) / 双终端同线程

`./qa/thread_continuity/run.sh two_clients` → rc=1, 10 of 11 claims pass. The only failure is D3. Client A opens session `agent:main:main:s1`. Then A and B, on separate WebSocket connections, send `"same text"` to that session at the same moment (`asyncio.gather`). Both `chat.send` calls return a `run_id`, and neither client receives an error frame. The transcript holds exactly two `"same text"` user messages, followed by an assistant reply. One provider request shows the model both of them. `session_events` sequence numbers have no holes.

Deviation from the plan / 偏离: the plan expected each message to get its OWN assistant reply. Under the default busy-input mode `Steer` (`src/gateway/execution_engine/gate.rs` `BusyInputMode::Steer`), a message that arrives while the session's run is going is folded into that run, so the two messages share one reply. The claim was rewritten to what must hold under either mode: nothing is lost, a reply follows, and the model is shown both. The client that lost the race varies between runs (A in one run, B in others), and the result is the same either way.

### F7 fork and isolated sub-agents (Task 8) / 子代理

`KEEP=1 ./qa/thread_continuity/run.sh fork` → rc=0, 18 claims. One session, six runs: runs 1–2 carry `QA_PARENT_MARKER`; run 3's single assistant turn calls `subagent{context:"fork"}` twice (`QA_FORK_TASK_1`, `QA_FORK_TASK_2`); run 4 is plain; run 5 forks once more (`QA_FORK_TASK_3`); run 6 calls `subagent{context:"isolated"}` (`QA_ISO_TASK`). Child requests are attributed by content (a tool-bearing request whose last user message states the task), not by position.

- Siblings from one turn: identical `system` and identical `messages` minus the task (4 messages = runs 1–2), the prefix carries the parent marker, ends on an assistant message, and has no `tool_use` without its `tool_result`.
- Across turns: run 5's fork gets the same `system`, and run 3's fork prefix is a strict prefix of it (4 → 10 messages; run 3's completed tool round and run 4 appended).
- Isolated: the checker's request is a single task message; the parent marker appears nowhere in it (system or messages).

Limit / 局限: the mock logs the parsed request body, so "identical" means identical JSON re-serialized in the order the server sent it, not raw HTTP bytes. Whether the provider actually reports a cache hit is not measured (no real provider). Per the approved design, a fork child still does not hit the PARENT's cache (`src/agents/subagent_spawner/fork.rs` module doc: parent `AssemblyPath::Cached` vs child `AssemblyPath::Basic`); that is the separate round the user chose.

### F4 human gate (Task 7) / 人工介入

New fixture `qa/human_gate` (stages `ask`, `text`, `unattended`, `stale`; ports 18851/18852; reuses the scripted mock with `ask.tools.json` = one `ask_user` call carrying two questions `QA_Q1`/`QA_Q2`). Results:

- `ask` → rc=0, 7 claims. A `chat.send` run parks; `clarification.pending` shows one request with both questions in order; `clarification.resolve` with `answers: ["A1-RED", "A2-LARGE"]` + `expected_request_id` returns `resolved:true`; the run completes and the next provider request's `ask_user` tool result carries both answers, A1 before A2; nothing stays pending.
- `stale` → rc=0, 9 claims. Same as `ask`, then a second `clarification.resolve` on the answered request returns `{resolved:false, stale:true}` and `A-LATE` never appears in any provider request.
- `unattended` → rc=0, 5 claims. `cron.create` (one-shot `at` +24h) then `cron.run`: the cron run's `ask_user` comes back as an error carrying `no human is reachable on this run` (`src/clarification/ask.rs` `HEADLESS_DENIAL`) and the run completes in 1.2 s; `clarification.pending` stays empty throughout.
- `text` → rc=1, 10 of 11 claims. A paired webhook sender starts a run; the channel send of the question fails (the fixture's callback sink is unreachable on purpose) and the ask falls back to the event bus and parks. The user answers in plain text: after `A1-RED` the run makes no new provider request (it is waiting for question 2); after `A2-LARGE` the run completes and the tool result carries both answers in order. The one failure: the parked request never appears in `clarification.pending` for the operator → D1 (below).

Fixture notes / 装置说明: `run.sh` turns `[cron] enabled = true` because the `cron.*` RPCs are registered only when the cron service starts (`src/bin/aleph-server/commands/start/mod.rs` cron init), and sets `check_interval_secs = 1` because `cron.run` only queues the job for the next tick (the default 60 s tick made the first `unattended` run time out). `text` pairs the sender first (`channel.pairing.list` / `channel.pairing.approve`), as `qa/busy_input` does.

Deviation from the plan / 偏离: `text` observes "still waiting after the first reply" through the provider request count, not through `clarification.pending` `answered == 1`, because D1 hides the channel request from `clarification.pending`.

### D2 fix (Task 9) / 修复

User decision / 用户决定: refuse only the removed field, not every unknown field. `AgentRunParams` (`src/gateway/handlers/agent.rs`) now derives with `#[serde(remote = "Self")]`, and a hand-written `Deserialize` reads the raw keys, refuses any name in `REFUSED_REMOVED_FIELDS` (today only `plan_phase`, with the hint to send `exec_tier: "plan"`), and then runs the derived mapping. One check covers every path that parses this struct (`chat.send`, `agent.run`, and both `server_init.rs` handlers); none of the 28 struct literals changed. Wire result: `-32602 Invalid params: \`plan_phase\` is not accepted: it was removed; send \`exec_tier: "plan"\` for a read-only planning turn`. Other unknown fields are still ignored.

## Defect ledger / 缺陷台账

| ID | Scenario | Symptom / 症状 | Root cause / 根因 | Failing test | Fix commit |
|---|---|---|---|---|---|
| D1 | `qa/busy_input/run.sh steer` (F3) | The server logs `mid-loop steering: injected user message into running loop` three times, yet none of the three texts reaches any of the run's 7 later provider requests (run turns 3–9). The run never sees the steer. | Two different `SessionKey`s are used for one per-peer DM. The webhook router builds `DirectMessage{channel:"webhook", dm_scope:PerPeer}`; the harness runner receives the key as a string (`agent:main:dm:qa-sender`) and rebuilds it with `SessionKey::from_key_string` (`src/orchestrator/harness_bridge/runner_impl.rs:181`), which yields `channel:""`. Steering emits with the structured key (`src/gateway/execution_engine/steering.rs` `try_inject_steering`; its comment says "no translation needed"), so the event lands in a separate `session_events` stream the loop never reads. The pending-steer count (`read_steering_events`) reads that orphan stream too. Same split seen for `AssistantRunMeta` (`stamp_run_meta`, `src/gateway/execution_engine/helpers.rs:98`) and the `journal_hook_stop` receipt. The string form does not survive the round trip either: `agent:main:dm:x` parses to `{channel:"", PerPeer}`, which formats as `agent:main:peer:x`, so the harness's session-preference key (`runner_impl.rs:184`) differs from the gateway's. Scope: every DM from a named channel under the default `DmScope::PerPeer`. `Main` keys are not affected. **Also F4 (Task 7):** the channel run parks its `ask_user` request under `agent:main:dm:qa-sender`; `clarification.pending` (`src/gateway/handlers/clarification.rs` `handle_pending`) re-parses that string with `SessionKey::from_key_string` and looks up `agent:main:peer:qa-sender`, a directory with a transcript but no `metadata.json`, so a scoped operator caller never sees the question (the real row is `data/sessions/agent:main:dm:qa-sender/metadata.json`, owner `u-owner`). The user can still answer in the channel itself. | `qa/busy_input/run.sh steer`; `qa/human_gate/run.sh text` claim "shows up in clarification.pending" (real machine); unit test to come in Task 9 | — |
| D2 | `qa/plan_handoff/run.sh {handoff,deny,floor}` (F5) | A `chat.send` carrying the removed `plan_phase: "planning"` field is accepted and starts an ordinary run (`result.run_id` returned). An old client would believe it is planning while every tool runs: fail-open. | `chat.send` deserializes into `AgentRunParams` (`src/gateway/handlers/agent.rs:37`, shared with `agent.run` via `src/bin/aleph-server/server_init.rs:368`), which has no `#[serde(deny_unknown_fields)]`, so unknown fields are dropped silently. Fix options (decide in Task 9, wire-contract change): reject unknown fields on `AgentRunParams` (broad: every client sending an extra field breaks) vs. reject only removed fields (narrow, but an enumerated list). | `gateway::handlers::agent::tests::a_run_request_carrying_the_removed_plan_phase_is_refused`; `qa/plan_handoff` last claim (all three stages green after the fix) | see D2 fix note |
| D3 | `qa/thread_continuity/run.sh two_clients` (F3/F8) | When a `chat.send` is folded into a running run (busy-input `Steer`), the client still gets `result.run_id`, but no stream frame ever names that id: no `run_accepted`, no `run_complete`, no `run_error`. The client does see the merged run's `run_complete`, under the OTHER id. Verified on the wire in 4 runs. Expected impact, not verified in a browser: the Leptos Panel binds the returned id (`interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs:398`) and settles it only on a terminal frame for that id (`interfaces/webchat/src/platform/wide/views/chat/events.rs:945`) or on reconnect repair, so the composer would stay on Stop. A single client steering its own run takes the same path. | The fold returns `GateOutcome::HandledInline` (`src/gateway/execution_engine/gate.rs:303`), and `execute()` returns `Ok(())` (`src/gateway/execution_engine/execute.rs:514`) without emitting anything for the reserved run id. The busy queue logs `Chat run completed successfully` for it (`src/gateway/busy_queue/spawn.rs`), but no frame goes out. Fix options (wire contract, ask the user at Task 9): emit a terminal frame for the folded id that names the run it joined, or have `chat.send` report the target run in its reply. | `two_clients`: "client X receives a terminal frame for the run id its chat.send returned" | — |
