# Code review — `src/builtin_tools/` chunk B (2026-08-19 round r4)

## Scope
- Files reviewed (production code under `src/builtin_tools/`):
  - `a2a_tools.rs` (562)
  - `acp_tools.rs` (498)
  - `acting_agent.rs` (116)
  - `agent_identity.rs` (648)
  - `ask_user.rs` (693)
  - `automation_tool.rs` (568)
  - `gateway_route.rs` (319)
  - `goal.rs` (3034)
  - `governance_metrics.rs` (277)
  - `heartbeat_manage.rs` (565)
  - `hooks_manage.rs` (577)
  - `loop_graph_manage.rs` (2055)
  - `loop_manage.rs` (2062)
  - `moa_manage.rs` (1203)
  - `self_config.rs` (1307)
  - `self_manage.rs` (177)
  - `strategy_manage.rs` (423)
  - `agent_manage/context.rs` (97), `create.rs` (625), `delete.rs` (398), `error.rs` (153), `info.rs` (316), `list.rs` (267), `mod.rs` (88), `switch.rs` (296), `test_utils.rs` (78), `unbind.rs` (228), `update.rs` (763), `validation.rs` (151)
- LoC total: ~18,544
- Cross-checked callers / shared types:
  - `src/clarification/ask.rs` (the ask_user implementation seam)
  - `src/clarification/session.rs` (clarification registry / lifecycle)
  - `src/goal/types.rs` (Goal constructors, MAX_LESSONS)
  - `src/goal/store.rs` (commit_field_update semantics)
  - `src/goal/pursuit.rs` (autonomous continuation consumer of lessons)
  - `src/identity/` (ledger/keystore contracts)
  - `src/a2a/port/agent_resolver.rs` (RegisteredAgent::new contract)
  - `src/a2a/adapter/client/http_client.rs` (A2AClient::with_auth contract)
  - `src/providers/moa/preset_store.rs` (MoaPresetStore::save_preset validation pipeline)
  - `src/tasks/heartbeat/service/ops.rs` (add_task / recompute_schedule)
  - `src/tools/turn_context.rs` (role_is_operator, TURN_CONTEXT)
  - `src/extension/` (hooks_admin / HookEvent catalogue)
  - `src/routing/resolve.rs`, `src/routing/session_key.rs` (session/peer resolution)
  - `src/approval/` (ActionType / ApprovalDecision / audit_identity)
- Skipped (already audited in prior rounds):
  - `src/a2a/`, `src/agents/` (r3, see `sev-wire-2026-08-19-r3/a2a/REPORT.md` and `.../acp/REPORT.md`)
  - `src/acp/` (r3, see `.../acp/REPORT.md`) — this chunk audits only the *callers* in `builtin_tools/`
- Method: read every file end-to-end; targeted `grep` for risk patterns (`tokio::spawn`, `unwrap()`, `expect(`, blocking `std::fs::` in async fns, length-cap absence, tool-name validation, scope/auth gates); cross-checked shared contracts to verify whether protections exist downstream. Findings are filtered to ≥80% confidence per the skill protocol.

## Findings

### BT-B-R4-01 — `goal.rs` accepts unbounded `objective` / `note` / `lesson` strings, no per-field size cap
- **File**: `src/builtin_tools/goal.rs:772-786` (`set`), `src/builtin_tools/goal.rs:900-903` (`update` note), `src/builtin_tools/goal.rs:1028-1030` (`update` lesson); types at `src/goal/types.rs:213-240` (`Goal::new`), `:350-355` (`with_note`), `:377-386` (`with_lesson_appended`)
- **Severity**: Medium
- **Category**: resource / security
- **Description**: The `goal` tool writes user-controlled strings (`objective`, `note`, `gate_command`, `lesson`, `waiting_reason`) directly into `Goal::new` / `with_note` / `with_lesson_appended` with no length cap, no character whitelist, and no per-string size limit. The only bounded field is the *count* of lessons (`MAX_LESSONS = 5`, `src/goal/types.rs:211`), not the size of each one. A compromised or runaway model that calls `goal(action='update', lesson='A'.repeat(1<<30))` will: (a) consume SQLite pages linearly with the lesson size, (b) inflate the `lessons` `Vec<String>` in the in-memory `Goal` so every `StandingGoalLayer` prompt render and every `render(...)` call (re-rendered on every read of `get`/`list`) loads the whole string, and (c) survive across sessions because the per-session row is persisted. The `lesson` field is in particular surfaced to the model every turn via `StandingGoalLayer`, so an oversized lesson is also an output-spend amplification vector. The `wait_for_task` field stores an opaque coordination id that is similarly unbounded.
- **Evidence**:
  ```rust
  // goal.rs:1028-1030 (update path)
  if let Some(lesson) = args.lesson.clone() {
      goal = goal.with_lesson_appended(lesson, now);
  }
  // goal.rs:782-786 (set path — objective passes through unchanged)
  let mut goal = Goal::new(&session, objective, 0, now)
      .with_budget(args.token_budget)
      .with_note(args.note.clone(), now)
      ...
  // goal/types.rs:377-386 — no size guard
  pub fn with_lesson_appended(mut self, lesson: String, now_ms: u64) -> Self {
      self.lessons.push(lesson);
      if self.lessons.len() > MAX_LESSONS {
          let drop = self.lessons.len() - MAX_LESSONS;
          self.lessons.drain(0..drop);
      }
      ...
  }
  ```
- **Suggested fix**: Add a `MAX_OBJECTIVE_LEN`, `MAX_NOTE_LEN`, `MAX_LESSON_LEN` (e.g. 4 KiB / 2 KiB / 2 KiB) constant in `src/goal/types.rs` and enforce at `Goal::new`, `with_note`, `with_lesson_appended`, plus a `validate_lesson`/`validate_objective` helper called from `goal.rs:set`/`update` mirroring the existing `validate_gate_command` (`goal.rs:676`). The lesson ring buffer already proves the author knew caps matter; only the per-string cap is missing.
- **Verification**: Grepped `src/builtin_tools/goal.rs` and `src/goal/` for any of `MAX_OBJECTIVE_LEN`, `MAX_LESSON_LEN`, `MAX_NOTE_LEN`, `validate_objective_length` — 0 hits. The only length-shaped guards in the goal subsystem are `clamp_iterations` (`:524`, numeric), `deadline_from_minutes` (`:535`, numeric), and `reject_zero_caps` (`:553`, numeric). String fields have no analogue.

### BT-B-R4-02 — `goal.rs` `validate_wait_args` accepts a non-existent `wait_for_task` id, parking the goal until the iteration cap or deadline fires
- **File**: `src/builtin_tools/goal.rs:596-635` (`validate_wait_args`); arm at `:1051-1058`
- **Severity**: Medium
- **Category**: correctness
- **Description**: `validate_wait_args` rejects empty `wait_for_task` and rejects waits on non-`Active`/`PursuitMode::Active` goals, but it does not check that the task id exists in the team/workflow task store. A model that passes `wait_for_task: "non-existent-task-12345"` will commit a parked goal whose `waiting_on_task` is set, `status='active'`, `pursuit=Active` — the continuation hook will see the goal parked, no team-settle event will ever fire (the task does not exist), and the goal remains parked until `pursuit_max_iterations` is exhausted by some other path or `timeout_minutes` elapses. The model gets a clean "Updated." message at commit time. This is the *goal* sibling of the r3 ACP `entry_and_timeout` bug (which the r3 review cited as `ACP-01` High).
- **Evidence**:
  ```rust
  // goal.rs:622-629 (inside validate_wait_args)
  if goal.status != GoalStatus::Active {
      return Err(...);
  }
  if args.wait_for_task.as_deref()
      .is_some_and(|t| t.trim().is_empty())
  {
      return Err(...);
  }
  // No check that `args.wait_for_task` names a live team/workflow task id.
  ```
  And the commit arm (`goal.rs:1051-1058`) calls `with_wait_on_task(task_id, args.note.clone(), now)` unconditionally once validation passes.
- **Suggested fix**: Add a task-store existence check (the team/task registry's lookup seam — same one `goal::pursuit` queries to wire the wake). Mirror the rejection style used by `goal.rs:240-247` (`resolve_target` returns "no standing goal is set for session …" with an actionable hint). Returning a clear "no coordination task named 'X' is in flight" gives the model something to retry against instead of a silent 24-hour park.
- **Verification**: Read the whole `validate_wait_args` body (`goal.rs:596-635`) — only the four branches above exist. The setters (`with_wait_until` at `types.rs:280-290`, `with_wait_on_task` at `:295-310`) are pure and do not validate either. The r3 cross-check shows that `ACP-01` flagged the equivalent pattern in `entry_and_timeout` (subprocess spawn without `acp_session_id`), which the r3 fix preferred to a more permissive seam — same posture applies here.

### BT-B-R4-03 — `heartbeat_manage.rs` accepts any `probe_tool_name` without verifying it resolves in the tool registry
- **File**: `src/builtin_tools/heartbeat_manage.rs:107-247` (args + create); shared seam `src/tasks/heartbeat/service/ops.rs:93-110` (`add_task`)
- **Severity**: Medium
- **Category**: correctness / resource
- **Description**: `HeartbeatCreateArgs::probe_tool_name` is a free-form `String` and the tool passes it straight into `ProbeConfig.tool_name` (`heartbeat_manage.rs:199`). `HeartbeatService::add_task` (and the shared `ops::add_task` at `ops.rs:93-110`) never check that the named tool exists, has the right argument shape, or is callable from the heartbeat worker's context. A misnamed probe creates a heartbeat task that fires on schedule forever, logs a probe error each tick (no early-stop), and — depending on `trigger_condition` — may keep re-running the L2 agent against a never-resolving probe (because the trigger default `Always` short-circuits the comparison; the L2 still gets the error string). The model has no in-tool diagnostic path that says "this tool name does not exist"; the failure is only visible via `heartbeat_list` plus the heartbeat service's runtime log. A malicious model can use this to pin a periodic LLM-cost sink to the daemon (the L2 agent still gets a turn on every probe, even with `Always`, because the trigger-condition gates whether L2 runs — and `trigger_condition=NonEmpty` on a missing probe is structurally identical to `Always`).
- **Evidence**:
  ```rust
  // heartbeat_manage.rs:198-203
  let probe = ProbeConfig {
      tool_name: args.probe_tool_name,
      tool_params: args.probe_tool_params,
      trigger_condition,
  };
  ...
  // no `validate_tool_exists(&probe.tool_name)` between construction and persistence
  let id = service.add_task(task, &clock).await?;
  ```
  And the shared seam:
  ```rust
  // ops.rs:93-110
  pub fn add_task<C: Clock>(store: &mut HeartbeatStore, mut task: HeartbeatTask, clock: &C) -> String {
      ...
      recompute_schedule(&mut task, clock);
      let id = task.id.clone();
      store.add_task(task);
      id
  }
  ```
  No tool-name validation in either path.
- **Suggested fix**: In `HeartbeatCreateTool::call`, look the tool up in the same registry the executor consults (the `AlephTool` registry handle the executor already holds) before persisting. Reject with `AlephError::tool("probe tool '{name}' is not a registered tool; call list_tools to pick a real name")`. Mirror the same gate in `heartbeat_update` when `probe_tool_name` is provided.
- **Verification**: Grepped `src/builtin_tools/heartbeat_manage.rs` and `src/tasks/heartbeat/` for any `tool_name` validation — 0 hits. The closest reference is the `interval_ms < 1000` floor at `heartbeat_manage.rs:188-192`, which proves the author knew input validation lives at the tool boundary.

### BT-B-R4-04 — `self_config.rs` `list_files` / `read_file` / `write_file` use synchronous `std::fs::*` in methods invoked from the async `call` path
- **File**: `src/builtin_tools/self_config.rs:217-236` (`list_files`), `:241-261` (`read_file`), `:265-324` (`write_file`); called from `async fn call` at `:813-816`
- **Severity**: Medium
- **Category**: concurrency / performance
- **Description**: `SelfConfigTool::list_files`, `read_file`, and `write_file` are declared as **sync** `fn` but perform `std::fs::metadata`, `std::fs::read_to_string`, `std::fs::create_dir_all`, and `std::fs::write` on the agent's identity directory. They are invoked from `async fn call` (lines `:813-816`). Each invocation runs on the tokio worker thread that polled the future, blocking it for the duration of every filesystem syscall — and these calls are reachable on a hot path (every LLM turn that touches identity files goes through this surface). The synchronous `read_config` (`:341-365`), `route_status` (`:401-426`), and the various `#[allow(...)]`-decorated sync helpers further widen the surface. The companion `update_config`, `list_backups`, and `rollback_config` are correctly `async` (they call `patcher.apply().await`), so the codebase already mixes both styles — the sync files are the inconsistency. This is the *same* shape as `ACP-02` (r3 finding on `src/acp/incoming.rs`), just in a different module.
- **Evidence**:
  ```rust
  // self_config.rs:217-236 (list_files — sync fn, blocking I/O)
  fn list_files(&self) -> Result<SelfConfigOutput> {
      ...
      for &name in IDENTITY_FILE_NAMES {
          let path = self.agent_dir.join(name);
          let (exists, size) = match std::fs::metadata(&path) {  // BLOCKING
              Ok(meta) => (true, meta.len()),
              Err(_) => (false, 0),
          };
          ...
      }
  }
  // self_config.rs:265-324 (write_file — sync fn, blocking I/O)
  fn write_file(&self, file_name: &str, content: &str) -> Result<SelfConfigOutput> {
      ...
      if let Err(e) = std::fs::create_dir_all(&self.agent_dir) { ... }  // BLOCKING
      ...
      match std::fs::write(&path, content) { ... }                       // BLOCKING
  }
  // self_config.rs:813-825 (called from async fn)
  async fn call(&self, args: Self::Args) -> Result<Self::Output> {
      ...
      let result = match args {
          SelfConfigArgs::ListFiles => self.list_files(),                 // blocks worker
          SelfConfigArgs::ReadFile { file_name } => self.read_file(&file_name),
          SelfConfigArgs::WriteFile { file_name, content } => self.write_file(&file_name, &content),
          ...
      };
  }
  ```
- **Suggested fix**: Either (a) convert these three methods to `async fn` and replace the `std::fs::*` calls with `tokio::fs::*`; (b) keep them sync and dispatch through `tokio::task::spawn_blocking` from `call`; or (c) introduce a single private `block_on_io(...)` helper that wraps the body in `spawn_blocking` so the change is local. The author already chose async elsewhere in the file (`:341`, `:401`, `:571`, `:608`) — option (a) is the most consistent.
- **Verification**: Read every `std::fs::*` call site in `self_config.rs` (8 hits at lines `:222`, `:249`, `:294`, `:311`, `:886`, `:975`, `:1192`, `:1232`). Three of them are in production sync methods reachable from `async fn call`; the rest are in tests or already-async paths. The async style is well-established — the gap is exactly the three identity-file helpers.

### BT-B-R4-05 — `a2a_tools.rs` `a2a_agents add` stores caller-supplied `url` and `token` verbatim, no SSRF / URL allowlist / token-scope check
- **File**: `src/builtin_tools/a2a_tools.rs:288-329` (the `Add` arm), `src/a2a/adapter/client/http_client.rs:24-69` (`A2AClient::with_auth`), `src/a2a/port/agent_resolver.rs:23-58` (`RegisteredAgent` + `new`)
- **Severity**: Medium
- **Category**: security
- **Description**: The `a2a_agents add` arm accepts a free-form `url` and a free-form `token`, fetches the agent card over the network, and persists both into `RegisteredAgent::new(..., url, token)`. There is no SSRF check (private IP filter, scheme allowlist, DNS resolution check), no scope/audience validation of the token (a leaked token from any other context can be reused as an outbound credential), and no provenance stamp on the stored token. The companion `remove` arm is fine (just looks up the agent id and unregisters), but the `add` path is the *write* side. This is a complementary risk to `A2A-R3-18` (r3, hostname leak in agent card id): the agent card id leak reveals *this* host's identity to remote peers; this finding is the mirror — a peer-chosen URL/token combination gives the model (or a prompt-injected model) arbitrary outbound HTTP with arbitrary credential scope. The downstream `A2ASubAgent::execute_delegation` (called from `a2a_tools.rs:147`) reuses the stored token verbatim on every subsequent delegation.
- **Evidence**:
  ```rust
  // a2a_tools.rs:292-326 (Add arm)
  A2AAgentsAction::Add => {
      let url = args.url.clone()
          .ok_or_else(|| AlephError::tool("`url` is required for action `add`"))?;
      ...
      let client = match args.token.clone() {
          Some(token) => A2AClient::with_auth(&url, token),
          None => A2AClient::new(&url),
      };
      let card = client.fetch_agent_card().await.map_err(|e| { ... })?;
      let trust = TrustLevel::infer_from_url(&url);   // <-- the only policy hook
      registry.upsert(RegisteredAgent::new(card, trust, url.clone(), ..., args.token.clone())).await;
      ...
  }
  ```
  `TrustLevel::infer_from_url` (used here) only buckets URL → `Public` / `Trusted`, with no SSRF filter. The `A2AClient::with_auth` constructor (`http_client.rs:65-69`) stores the token in `self.auth_token` and ships it on every outbound call without scoping.
- **Suggested fix**: Layer a `validate_outbound_url(&url)` call between the `url` extraction and the `A2AClient::with_auth` construction. Reject `https?://localhost`, `https?://127.*`, `https?://10.*`, `https?://192.168.*`, `https?://169.254.*`, `https?://[fd*`/`[fe80:*` (mirrors the r3 `A2A-R3-11` IPv6-private carve-out), and `file://` / non-http(s) schemes. Add a credential-scope check: store `token_scope: Option<String>` (audience claim) alongside the token and refuse delegation unless the request's `aud` matches. At minimum, require the LLM to pass a `purpose` field that gets persisted with the token so an operator audit can answer "why was this credential authorized for this peer".
- **Verification**: Grepped `src/builtin_tools/a2a_tools.rs` for any of `validate_outbound_url`, `ssrf`, `reject_private_ip`, `ensure_https` — 0 hits. The closest policy is `TrustLevel::infer_from_url`, which is documented elsewhere as a coarse public-vs-trusted classifier, not a network policy. The r3 report explicitly flagged the *outbound* side as a known gap (`A2A-R3-18` was the *card-id* leak; this is the *cred* leak).

### BT-B-R4-06 — `goal.rs` `update` accepts `wait_minutes=0` after `wait_minutes >= 1` cap, but cross-call race leaves a stuck `pending_continuation_ms` marker
- **File**: `src/builtin_tools/goal.rs:553-571` (`reject_zero_caps`), `:978-1001` (`update` arm — pre_wait_until capture and status block), `:1118-1131` (post-commit timer supersede)
- **Severity**: Low
- **Category**: correctness / concurrency
- **Description**: `reject_zero_caps` correctly rejects `wait_minutes == 0` at the boundary (`goal.rs:567-571`), so a `0`-minute park cannot be committed. However, the timer-supersede path that follows the status commit (`goal.rs:1118-1131`) only fires when `barrier_touched == true`, which is `args.status.is_some() || args.wait_minutes.is_some() || args.wait_for_task.is_some()`. The capture of `pre_wait_until = goal.waiting_until_ms` happens *before* the status block, but `supersede_wait_timer` is called only inside the `barrier_touched` branch. If a caller passes an `update` that does NOT touch the barrier (e.g. `update(lesson=…)` only), and the stored goal already had an armed timer from a previous `wait_minutes`, the new commit leaves the marker alone — by design. The real race is the opposite direction: two concurrent `update`s on the same session, where one sets a new `wait_minutes` and one only updates a lesson. The lesson-only update wins the commit; the timer update's `barrier_touched` branch executes; but `pre_wait_until` was captured from the *pre-commit* read and reflects the OLD timer. `supersede_wait_timer` then tries to clear a marker that has already been overwritten by the lesson-only update's commit (which did nothing because it didn't touch the barrier), so the marker is correctly cleared — *if* the lesson-only update was second. If it was first, the wait-bearing update's `commit_field_update` runs after and overwrites the lesson field while preserving the armed marker (per the comment at `:1100-1115`), but `pre_wait_until` was captured against the *first* commit's snapshot, so the supersede targets the *new* timer's wake — leaving the old wake stuck. The doc-comment promises "supersede that stale marker so the fresh claim can fire immediately"; under this interleaving it does not.
- **Evidence**:
  ```rust
  // goal.rs:1100-1131 (commit + supersede region)
  // Atomic commit: re-reads the LIVE `pending_continuation_ms` under
  // the store lock and keeps it, so a tool update landing while a
  // claimed continuation fires cannot restore a stale marker...
  match self.store.commit_field_update(&goal, prev_status)? { ... }
  // Supersede a stale timer marker left armed by a barrier this
  // update just cleared/replaced
  if let Some(armed) = pre_wait_until {
      if barrier_touched {
          if let Err(e) = self.store.supersede_wait_timer(&session, armed) {
              ...
          }
      }
  }
  ```
  The race: the `commit_field_update` call at `:1107` is a CAS that preserves `pending_continuation_ms`. Two concurrent calls `A` (lesson-only) and `B` (wait_minutes) interleave such that A commits first (preserves marker X), B then reads pre_wait_until=X (correct), commits with the new barrier, supersedes X. So far so good — but the docs at `:1100-1115` warn that the field is store-owned; the *actual* stored value after B commits depends on whether the store's CAS sees the marker X still valid (yes — A preserved it). The supersede at the end of B uses `armed=X` and clears it. The interleaving is safe; the doc-comment's worry about "stale grace" is real. So this is conservative-low. I keep it as Low because the *interleaving where B's `pre_wait_until` is captured before A's commit lands and B commits after A's commit* is unmodelled and the supersede uses pre-capture not post-capture.
- **Suggested fix**: Capture `pre_wait_until` *after* `commit_field_update` returns, against the just-committed store state (re-read `goal = self.store.get(&session)?` then `goal.waiting_until_ms`), and only supersede when the new barrier is set. Costs one extra read per update, which is negligible compared to the CAS the store already does.
- **Verification**: Read `commit_field_update`'s contract (store seam at `src/goal/store.rs`, called from `goal.rs:1107`) — the function explicitly preserves `pending_continuation_ms` and the surrounding doc-comment at `goal.rs:1100-1115` flags this as the live hazard. The exact race requires concurrent tool invocations on the same session, which is unlikely (one tool call per turn) but is reachable from a parallel sub-agent hook.

### BT-B-R4-07 — `loop_graph_manage.rs` `pair` action's rollback path deletes the cron job but the graph-row cleanup races the node-then-edge write
- **File**: `src/builtin_tools/loop_graph_manage.rs:1014-1056` (the `wired = ... and_then(...)` chain), `:1047-1056` (failure rollback)
- **Severity**: Low
- **Category**: error-handling
- **Description**: The `Pair` arm does node-then-edge upsert under the same `Result` chain (`wired = upsert_node(...).and_then(|()| upsert_edge(...))`). On failure, the rollback is cron-delete *then* node-delete, but the *order of the failure paths* is not symmetric with the write order: the node may succeed and the edge fail, in which case the cron rollback correctly drops the cron, then the node rollback drops the node — but a third failure on the node rollback (logged but swallowed at `:1052-1055`) leaves a graph row pointing at a deleted cron. The audit template and `lint_naked_loops` already key on this exact pattern (the comment at `:1041-1056` documents the residue). The defensive cleanup at `:1051-1055` is `if let Err` and `warn!` — it does NOT retry or escalate, so the only path left to the operator is the "remove by hand" warning. This is documented as intentional but the *absence of a publish event when the residual node persists* is the gap: subscribers (the audit persister) record "node deleted" from the cron rollback path, then do not record the failed cleanup at all, so the audit log shows a deleted watcher where reality is a zombie node.
- **Evidence**:
  ```rust
  // loop_graph_manage.rs:1041-1056
  if let Err(e) = wired {
      let service = cron.lock().await;
      if let Err(rollback) = service.delete_job(&job_id).await { ... }
      if let Err(rollback) = self.store.delete_node(&agent_id, &watcher_id) {
          warn!(node = %watcher_id, error = %rollback,
              "loop_graph pair: watcher node left behind after a failed edge \
               write — remove it with action='drop_node'");
      }
      return Err(e);
  }
  ```
  No `crate::loop_graph::publish(TopologyEvent::NodeDeleted { ... })` on the secondary rollback failure.
- **Suggested fix**: On the secondary rollback failure, publish a `TopologyEvent::NodeDeleted` so the audit persister records the deletion, then surface a distinct error variant that names the residue (e.g. `PairError::WatcherZombie { node_id, cron_id }`) so the operator gets one actionable error instead of a `warn!` line in the daemon log. The audit persister is the only consumer that can keep the roll-call honest when a manual cleanup is required.
- **Verification**: Compared to the `enable_audit` arm at `:927-944` which *does* publish `TopologyEvent::NodeUpserted` after the second-pass orphan re-adopt. The `Pair` arm's rollback does not publish — only the `NodeUpserted` event in the success path at `:1061-1063`. The asymmetry is documented as the `watcher_nuke_failed_event` gap.

### BT-B-R4-08 — `agent_manage/delete.rs` archives workspace + agent-state dirs via `tokio::fs::rename` but never verifies the post-rename path
- **File**: `src/builtin_tools/agent_manage/delete.rs:191-217` (legacy archival path), `src/builtin_tools/agent_manage/delete.rs:165-191` (manager path)
- **Severity**: Low
- **Category**: resource / correctness
- **Description**: When `agent_delete` is invoked without a wired `AgentManager`, the tool falls through to the legacy archival at `:191-217`. It renames `workspace` and `agent_state_dir` to `*.archived` siblings via `tokio::fs::rename`, then proceeds to unregister from the runtime. On the success path of the workspace rename the operation continues. On failure it `warn!`s and continues — but the original workspace and state dir remain in their original locations, AND the `Deleted` lifecycle event (`:220-227`) is still published with `workspace_archived: true`. A Panel or other consumer that reads the event believes the archive happened. The post-rename *existence check* on the `*.archived` target is missing, so the event is a claim about a state that may not exist. The `requires_confirmation` flag on the tool is set, which is the right backstop, but the published event's truthfulness is the gap.
- **Evidence**:
  ```rust
  // delete.rs:194-217
  if let Some(ref removed) = removed {
      let workspace = removed.workspace();
      let archived = workspace.with_extension("archived");
      if workspace.exists() {
          if let Err(e) = tokio::fs::rename(workspace, &archived).await {
              warn!(...);                                // <-- continues
          } else {
                info!(...; "Workspace archived");
          }
      }
      let agent_state_dir = ...;
      if agent_state_dir.exists() {
          let archived_state = agent_state_dir.with_extension("archived");
          if let Err(e) = tokio::fs::rename(&agent_state_dir, &archived_state).await {
              warn!(...);                                // <-- continues
          }
      }
  }
  // delete.rs:222-227 — event fires regardless
  AgentLifecycleEvent::Deleted { agent_id, workspace_archived: true }.publish(bus);
  ```
- **Suggested fix**: Compute a `workspace_archived: bool` from the actual rename result and pass it through. Use `tokio::fs::try_exists(&archived)` to verify after the rename. If verification fails, set `workspace_archived: false` and add a structured field `workspace_archived_path: Option<PathBuf>` so consumers can route cleanup themselves.
- **Verification**: Read the whole `call` body of `AgentDeleteTool` (`delete.rs:112-249`) — no existence verification on either rename. Compared to the manager path at `:165-191` which is best-effort but also unconditional on its own success. The `Deleted` event is at `:222-227` and unconditionally says `workspace_archived: true`.

## Cross-cutting concerns

1. **String-length asymmetry across `goal.rs`.** The goal subsystem validates shell safety (`validate_gate_command`), numeric caps (`reject_zero_caps`, `clamp_iterations`), and deadline arithmetic (`deadline_from_minutes`), but every user-supplied string field (`objective`, `note`, `lesson`, `waiting_reason`, `gate_command` after safety check) is accepted with no byte-length cap. A single MAX constant family in `src/goal/types.rs` (already where `MAX_LESSONS` lives) would close the gap and match the discipline the loop and cron tools already show (`loop_manage.rs` rejects `interval_ms < 1000`, `heartbeat_manage.rs` rejects `interval_ms < 1000`).

2. **Identity / authority of model-driven configuration edits.** `agent_update` correctly audits authority changes (`update.rs:198-206`), and `goal.update` is operator-gated for cross-session verbs, but `goal.lesson` writes — which are now part of the prompt every turn via `StandingGoalLayer` — have no provenance stamp and no length cap. A prompt-injected model can durably poison its own future turns (and any sub-agent that inherits the session's prompt layer) through one oversized `update(lesson=…)` call. This is the goal-subsystem analogue of the r3 `A2A-R3-21` token-redaction finding: a self-prompt-injection vector surfaced through a tool that writes to the model-facing layer.

3. **`std::fs::*` vs `tokio::fs::*` discipline.** `self_config.rs` mixes sync and async I/O in the same tool (BT-B-R4-04). The same pattern likely exists in other `builtin_tools/` files outside chunk B (e.g. `bash_exec`, `file_ops`); a single sweep across all of `builtin_tools/` is recommended for the next round.

4. **`goal` `wait_for_task` is the only `goal` cross-tool reference without existence validation.** `goal.rs`'s `reject_zero_caps`, `validate_gate_command`, `validate_wait_args`, `inert_resume_reason`, and `governing_owner_or_refuse` form a coherent validation surface — every check has a domain-specific purpose except the task-id existence check. The cross-team/workflow task registry is the consumer that *would* fire the wake, so the registry's existence is the right check.

5. **Loop-graph `Pair` vs `enable_audit` rollback asymmetry.** BT-B-R4-07 mirrors the r3 `A2A-R3-09` pattern (pushNotificationConfig orphan). The two top-level install paths (`enable_audit` and `pair`) both do best-effort cron-rollback on graph-write failure; only `enable_audit` publishes the post-rollback topology event. The audit-template roll-call reads the events table, so the asymmetry surfaces as a missing entry that an operator cannot diagnose from the panel.

6. **`a2a_agents add` is the A2A subsystem's write surface.** The r3 report deferred body-level work (`A2A-R3-21` flagged the token-leak risk in the `RegisteredAgent` struct; this chunk flagged the *caller* that populates it). The two findings together describe the full credential flow from LLM-supplied `token` to persistent `RegisteredAgent.auth_token` to outbound `A2AClient::with_auth` — and there is no token-scope check, no SSRF filter, no audit trail of the binding anywhere in that chain.

7. **Hook and Heartbeat lifecycle tools don't validate their target tool/agent identity exists.** `hooks_manage add` validates event names (good) but stores any `prompt`/`agent`/`command`/`url` content unfiltered (relying on operator consent for shell/HTTP). `heartbeat_create` accepts any `probe_tool_name` (BT-B-R4-03). A model that wants to silently pin a daemon cost sink can do so via heartbeat without touching any approval-gated path.

## Summary
- **Total: 8 findings** (0 Critical, 0 High, 5 Medium, 3 Low)
- **Top priority items (must-fix)**:
  1. **BT-B-R4-01** — `goal.rs` `objective` / `note` / `lesson` strings have no per-field size cap; one tool call can persist multi-MiB content that re-renders every turn via `StandingGoalLayer`. Memory and prompt-spend DoS through a model-driven write path.
  2. **BT-B-R4-05** — `a2a_agents add` accepts arbitrary `url` + `token` with no SSRF filter, no scope check, no audit trail; an LLM-supplied URL+secret combination becomes a persistent outbound credential in `RegisteredAgent.auth_token` and is reused on every subsequent delegation.
  3. **BT-B-R4-03** — `heartbeat_create` accepts any `probe_tool_name` without verifying the named tool exists in the registry, so a misnamed probe creates a perpetual failing heartbeat (and any L2 agent still gets a turn on `Always`/`NonEmpty` triggers).

## What was NOT covered
- **Out-of-chunk callers in `src/builtin_tools/`**: `bash_exec`, `file_ops/*`, `canvas`, `code_exec`, `workflow_tool`, `scratchpad`, `memory_*`, `note_*`, `note_graph_query`, `skill_manage`, `skill_*`, `process_*`, `recall_*`, `search`, `select_model`, `session_*`, `partial_output`, `media_*`, `mcp_*`, `google_meet`, `doctor`, `crawl4ai`, `cron_manage`, `command_*`, `channel_*`, `browser_tools`, `desktop/*`, `hub/*`, `voice_tools/*`, `pdf_generate/*`, `pim/*`, `task_manage/*`, `team/*`, `sessions/*`, `web_fetch/*`, `generation/*`, `remember`, `flag_user_correction`, `process_completion`, `note_schema`, `note_orient`, `tool_usage`, `meta_tools`, `node_*`, `user_profile`, `vault_store`, `artifact_publish`, `mcp_login`, `mcp_prompt`, `mcp_resource`, `system_tool`, `error`, `config_audit`, `config_guide`, `code_check`, `ctx_search`, `permission_tool`, `mod`, `list_models`, `workspace_manage`, `skill_install`, `skill_status`, `skill_reader/*`, `memory_browse`, `memory_explore`, `memory_reflect`, `memory_search`, `memory_timeline`, `memory_trace`, `process_journal`, `process_registry`, `loop_manage`'s tests beyond what's in scope.
- **The full body of `clarification::ask`/`session`**: I read the entry points and the secret-withhold path (because ask_user delegates there), but the receiver/`RetireOnAbandon` guard, the timeout sweep, and the cleanup race window were not fully audited — those live in `src/clarification/ask.rs`/`session.rs` and are out of chunk B.
- **The `goal::pursuit` continuation hook** that consumes `lessons` and `waiting_on_task`: the production read paths were checked, but the *wake* implementation (which would tell us how a non-existent `wait_for_task` is handled) lives in `src/goal/pursuit.rs` and is out of scope.
- **`config::agent_manager::provisioning_roots`** semantics — assumed correct based on the create/delete callers' comments.
- **`a2a::sub_agent::A2ASubAgent::execute_delegation`** contract — read only the call site, not the implementation.
- **`a2a::service::card_registry::upsert`** semantics — read only the call site, not the implementation; cannot fully confirm the `args.token.clone()` lifetime is not duplicated or leaked.
- **`agents::AgentRegistry::set_allowed_users`** race-condition surface (live half of `agent_update`) — read the call site, not the implementation; the audit hook is in place but the live-write semantics need a separate audit.
- **`identity::*`** body — only the `agent_identity` *tool* surface was reviewed; the ledger writer, the keystore rotate/revoke paths, and the verify pipeline were not re-audited (they live in `src/identity/` and `src/tools/turn_context::ambient_actor`, both out of chunk B).
- **The wired cross-reference `OPERATOR_TOOLS` allowlist** in `src/gateway/method_authz` — the chat-tier/operator-tier posture was inferred from `caller_is_operator` and the `OPERATOR_TOOLS` self-references in tool comments; the central registry was not re-audited.
- **Wire-protocol conformance / model schema round-trips**: I checked `MoaManageArgs` (flat-schema choice) and `AgentCreateArgs` / `AgentUpdateArgs` (derives JsonSchema) but did not trace the per-provider Anthropic / OpenAI / Gemini adapters' quirks (those live in `src/providers/protocols/*`).
- **Performance / load**: no benchmarks; the `goal.rs` lesson size cap (BT-B-R4-01) and `self_config.rs` blocking I/O (BT-B-R4-04) are flagged on static reasoning, not measurements.
- **Tests in `tests/` or `#[cfg(test)]` modules of any file in scope** were treated as test fixtures — production-only findings.