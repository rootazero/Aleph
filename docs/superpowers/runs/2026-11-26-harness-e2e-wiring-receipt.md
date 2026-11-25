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

## Defect ledger / 缺陷台账

| ID | Scenario | Symptom / 症状 | Root cause / 根因 | Failing test | Fix commit |
|---|---|---|---|---|---|
| D1 | `qa/busy_input/run.sh steer` (F3) | The server logs `mid-loop steering: injected user message into running loop` three times, yet none of the three texts reaches any of the run's 7 later provider requests (run turns 3–9). The run never sees the steer. | Two different `SessionKey`s are used for one per-peer DM. The webhook router builds `DirectMessage{channel:"webhook", dm_scope:PerPeer}`; the harness runner receives the key as a string (`agent:main:dm:qa-sender`) and rebuilds it with `SessionKey::from_key_string` (`src/orchestrator/harness_bridge/runner_impl.rs:181`), which yields `channel:""`. Steering emits with the structured key (`src/gateway/execution_engine/steering.rs` `try_inject_steering`; its comment says "no translation needed"), so the event lands in a separate `session_events` stream the loop never reads. The pending-steer count (`read_steering_events`) reads that orphan stream too. Same split seen for `AssistantRunMeta` (`stamp_run_meta`, `src/gateway/execution_engine/helpers.rs:98`) and the `journal_hook_stop` receipt. The string form does not survive the round trip either: `agent:main:dm:x` parses to `{channel:"", PerPeer}`, which formats as `agent:main:peer:x`, so the harness's session-preference key (`runner_impl.rs:184`) differs from the gateway's. Scope: every DM from a named channel under the default `DmScope::PerPeer`. `Main` keys are not affected. | `qa/busy_input/run.sh steer` (real machine); unit test to come in Task 9 | — |
| D2 | `qa/plan_handoff/run.sh {handoff,deny,floor}` (F5) | A `chat.send` carrying the removed `plan_phase: "planning"` field is accepted and starts an ordinary run (`result.run_id` returned). An old client would believe it is planning while every tool runs: fail-open. | `chat.send` deserializes into `AgentRunParams` (`src/gateway/handlers/agent.rs:37`, shared with `agent.run` via `src/bin/aleph-server/server_init.rs:368`), which has no `#[serde(deny_unknown_fields)]`, so unknown fields are dropped silently. Fix options (decide in Task 9, wire-contract change): reject unknown fields on `AgentRunParams` (broad: every client sending an extra field breaks) vs. reject only removed fields (narrow, but an enumerated list). | `qa/plan_handoff` last claim (real machine); unit test to come in Task 9 | — |
