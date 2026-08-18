# Code review — `src/approval/` (2026-08-19 round r3)

## Scope
- Files reviewed (with LoC):
  - `src/approval/mod.rs` — 418
  - `src/approval/adapters.rs` — 428
  - `src/approval/audit.rs` — 94
  - `src/approval/callback_sink.rs` — 206
  - `src/approval/config.rs` — 924
  - `src/approval/guardian_requester.rs` — 700
  - `src/approval/node_requester.rs` — 223
  - `src/approval/operator_requester.rs` — 507
  - `src/approval/policy.rs` — 27
  - `src/approval/session_route.rs` — 111
  - `src/approval/tool_call.rs` — 106
  - `src/approval/types.rs` — 197
  - **Total: 3,941 LoC** across 12 files (matches prior audit).

## Findings

### APPROVAL-R3-001 — Audit log leaks raw action target via the `context` field
- **File**: `src/approval/audit.rs:28-50`, consumed by `src/approval/config.rs:355-365` and downstream callers `src/builtin_tools/desktop/mod.rs:350`, `src/builtin_tools/system_tool.rs:63`.
- **Severity**: **Critical**
- **Category**: security
- **Description**: `audit_identity` embeds the **raw** `target` string into the `context` it returns (`format!("{domain}.{action} ({target})")`). That context is then stored verbatim in `ActionRequest.context` and emitted in `ConfigApprovalPolicy::record` as `context = %request.context` (config.rs:365). The `target` field IS redacted by `redact_target`, but `context` is not — so any tool that passes a secret-bearing target (clipboard text via `system_tool.rs:218-221`, keystrokes via `desktop/mod.rs:347`, or anything future) lands the secret in the structured log line. The `record_log_redacts_pim_body` test (config.rs:677-696) deliberately constructs the request with a safe `context = "audit-test"`, so it never exercises the production path that calls `audit_identity`.
- **Evidence**:
  ```rust
  // audit.rs:28-50
  pub fn audit_identity(domain: &str, action: &str, target: &str) -> (String, String) {
      match crate::tools::turn_context::current_turn_context() {
          Some(turn) => {
              ...
              let context = if turn.is_channel_routable() {
                  format!("{domain}.{action} ({target}) via {}/{}",
                      turn.channel_id, turn.conversation_id)
              } else {
                  format!("{domain}.{action} ({target})")
              };
  ```
  ```rust
  // config.rs:355-365
  async fn record(&self, request: &ActionRequest, decision: &ApprovalDecision) {
      info!(
          action = ?request.action_type,
          target = %redact_target(&request.target),
          agent = %request.agent_id,
          context = %request.context,            // ← raw, NOT passed through SecretMasker
          decision = ?decision,
          "Approval decision recorded"
      );
  }
  ```
- **Suggested fix**: Run both `request.target` and `request.context` through `SecretMasker` (or `redact_target`) before logging. The Guardian requester already does this for its judge prompt (`approval/guardian_requester.rs:340-358`); the audit log is the same class of artifact and should match. At minimum, fix the two consumers that pass raw targets to `audit_identity`: `desktop/mod.rs:350` and `system_tool.rs:63`. The browser/pim/media/automation callers already use `display_target`, so they're only safe by virtue of `approval_display_target`'s sanitization — which the desktop/system paths don't have.
- **Verification**: Confirmed by reading `audit_identity` (audit.rs), all 8 call sites (grepped), and `record()` (config.rs). Reproduced by tracing what would be logged for `system_tool.clipboard_write("TOP_SECRET")`: `target` field becomes `<redacted len=14 sha=...>`, but `context = "system.clipboard_write (TOP_SECRET)"` — full secret retained.

### APPROVAL-R3-002 — TOCTOU window in `cached_glob_regex` allows redundant compiles and brief capacity overflow
- **File**: `src/approval/config.rs:103-122`
- **Severity**: Low
- **Category**: concurrency / perf
- **Description**: `cached_glob_regex` releases the mutex between the cache-miss check and the second acquire-for-insert. Two threads concurrently missing the same pattern both compile the regex, both acquire the second lock in turn, and the second `insert` overwrites the first (benign — same value). However, when the cache sits at 511 entries and N threads concurrently miss the same pattern, each insert runs the `if guard.len() < GLOB_CACHE_MAX` check at a slightly different size, briefly letting the map exceed 512 before later inserts hit the cap. Not a correctness bug — but a window of unbounded growth under contention, plus the wasted CPU on duplicate compiles.
- **Evidence**:
  ```rust
  // config.rs:103-122
  fn cached_glob_regex(pattern: &str) -> Option<regex::Regex> {
      ...
      {
          let guard = cache.lock().unwrap_or_else(|e| e.into_inner());
          if let Some(hit) = guard.get(pattern) {
              return hit.clone();            // (1) lock released here
          }
      }
      let compiled = crate::security::safe_regex::bounded_builder(&glob_to_regex_str(pattern))
          .build()
          .ok();
      let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());   // (2) re-acquired
      if guard.len() < GLOB_CACHE_MAX {
          guard.insert(pattern.to_string(), compiled.clone());
      }
      compiled
  }
  ```
- **Suggested fix**: Compute `compiled` under a single lock OR use `entry().or_insert_with(|| compile(...))` so the miss path is atomic and a `Result<Arc<Regex>>` cache avoids the duplicate-compile race. Capacity check should be on the map size at insert time under the same lock as the insert — or use a bounded LRU so overshoot can't happen.
- **Verification**: Walked the function under two concurrent miss paths mentally. Confirmed by inspection that the mutex is `std::sync::Mutex` (synchronous), held only briefly, never across `.await` — no deadlock risk.

### APPROVAL-R3-003 — `OperatorApprovalRequester` / `run_node_approval` swallow `event_bus.publish_frame` failures after registering the approval, leaving stale UI state
- **File**: `src/approval/operator_requester.rs:175-243`, `src/approval/node_requester.rs:99-132`
- **Severity**: Medium
- **Category**: error-handling
- **Description**: Both requesters `register_pending` BEFORE `publish_frame` (correct ordering — resolves the "operator wins before registration" race). But on `publish_frame` failure they only emit a `tracing::warn!` and continue: the pending entry still lives in `ExecApprovalManager`, the waiter still awaits the oneshot, and the eventual decision still publishes its `ApprovalResolved`/`ApprovalExpired` frame (which can also fail). A subscriber loss between `requested` and `resolved` means the operator surfaces may show a card with no closing event — the model never learns the resolution arrived. This is "fail-soft" but silently produces UX gaps (a card stays "Waiting" forever from the operator's perspective).
- **Evidence**:
  ```rust
  // operator_requester.rs:177-185
  if let Err(e) = self
      .event_bus
      .publish_frame(&GatewayEventFrame::ApprovalRequested { ... })
  {
      tracing::warn!(error = %e, "failed to publish ApprovalRequested for config approval");
  }
  ```
  ```rust
  // operator_requester.rs:239-241
  if let Err(e) = self.event_bus.publish_frame(&frame) {
      tracing::warn!(error = %e, "failed to publish final approval event for config approval");
  }
  ```
  Same pattern in `node_requester.rs:99-108` and `node_requester.rs:128-130`.
- **Suggested fix**: At minimum, return `ApprovalOutcome::Unavailable` when the initial `ApprovalRequested` publish fails (the user was never notified, mirroring the bridge's "delivery failed → Unavailable" rule in `exec/approval/channel_bridge.rs`). For resolved/expired frames, emit the warn but also re-publish at most once before logging at `error` so the audit trail records that the closure event was lost.
- **Verification**: Read both files in full, traced the failure path through `manager.await_registered` (which still resolves the oneshot and removes the entry from `pending`). Confirmed the operator's `exec.approvals.pending` view would not see the stale card after resolution — but the `SurfaceRouter` subscribers would, because they only learned about the card from the failed publish.

### APPROVAL-R3-004 — `OperatorApprovalRequester::request_approval` mutates `record.operator_only` after `manager.create` but before `manager.register_pending`; an aborted run between those two leaves a half-built record
- **File**: `src/approval/operator_requester.rs:148-172`
- **Severity**: Low
- **Category**: concurrency / correctness
- **Description**: Sequence is: `create(&request)` → `record.operator_only = self.operator_only` → `register_pending(record)`. If the future is cancelled between (2) and (3), the record is constructed in `manager.records` (held in `ExecApprovalManager`) but never registered in `pending`, so the opportunistic sweep at `register_pending` cannot evict it. Over time this leaks orphan `ExecApprovalRecord`s. The orphan cannot be resolved (no pending entry) and cannot be listed — it is invisible to every operator surface but consumes memory. `manager.create` returning by value with no side effect on `pending` is the design point, but the contract is undocumented and a dropped future is a real path under Tokio cancellation.
- **Evidence**:
  ```rust
  // operator_requester.rs:148-172
  let mut record = self.manager.create(&request, DEFAULT_APPROVAL_TIMEOUT_MS);
  record.operator_only = self.operator_only;
  ...
  let (approval_id, rx, timeout) = self.manager.register_pending(record);
  ```
- **Suggested fix**: Make `manager.create` return a guard that is consumed by `register_pending`; if dropped without registration, the record is removed from `records` automatically. Or fold the `operator_only` flag into the `ApprovalRequest` struct so it never needs post-hoc mutation.
- **Verification**: Traced the future-cancel path through the call site (`bin/aleph-server/commands/start/mod.rs:2980` and `adapters.rs`). The await is between `TURN_CONTEXT.scope(...)` and `await_registered` — a tool-budget abort lands here.

### APPROVAL-R3-005 — `GuardianApprovalRequester` constructs `SecretMasker::new()` per judge call rather than reusing the static
- **File**: `src/approval/guardian_requester.rs:340-358`, compared with `src/exec/masker.rs:85-99` and `src/bin/aleph-server/commands/start/helpers.rs`
- **Severity**: Low
- **Category**: perf / api-design
- **Description**: `render_action` builds `let masker = SecretMasker::new();` once per action (line 341), which is cheap (`SecretMasker` is a zero-sized handle) — but the module doc itself (`exec/masker.rs:18-22`) explicitly notes that `SecretMasker::new()` has 7 production construction sites and that operator patterns are process-wide via `install_operator_patterns`. A future migration to a non-zero-sized masker would turn this into a per-approval allocation. The Guardian sits on the ASK-tier hot path (worst case = every `requires_confirmation` tool call), so the per-call construction is observable in profiling even today.
- **Evidence**:
  ```rust
  // guardian_requester.rs:340-358
  fn render_action(action: &ApprovalAction) -> String {
      let masker = SecretMasker::new();
      let mut p = format!(
          "Pending action:\ntool: {}\naction: {}\n",
          action.tool_name,
          masker.mask(&action.summary)
      );
  ```
  Compare `background_persistence.rs:90` and `process_journal.rs:172` which use a `static MASKER: LazyLock<SecretMasker>`.
- **Suggested fix**: Either inline a `static MASKER: LazyLock<SecretMasker> = LazyLock::new(SecretMasker::new);` (matches the two existing sites) or take a reference. The masker is `Copy`-by-impl-cheapness today; the day that changes, this site is the one that drifts.
- **Verification**: Grepped `SecretMasker::new` construction sites — confirmed the Guardian and `execution_engine/execute.rs:1680`, `tasks/cron/executor.rs:613`, `builtin_tools/process_completion.rs:58,89`, `builtin_tools/process_journal.rs:172` (static), `agents/background_persistence.rs:90` (static), `gateway/event_emitter/redacting.rs:69`. 8 sites total; only 2 use the static pattern.

### APPROVAL-R3-006 — `record_originator` is mis-named and is read-only at an addressable id; a race between two callbacks both reading then resolving could double-resolve the same originator
- **File**: `src/callback_sink.rs:33-62`, called via `ExecApprovalManager::record_originator`
- **Severity**: Low
- **Category**: api-design / concurrency
- **Description**: The function `record_originator` returns `Option<String>` — its body reads `self.pending.read().unwrap_or_else(...)` and gets `pending.get(id).map(|e| e.record.originator_user_id.clone())`. The name suggests it records; it does not. Then `ManagerCallbackSink::handle_callback` checks the returned value against `user_id`. There is no race per se (the originator is set at `create` time and is immutable), but the API name is misleading and a future change that DOES want to record state would silently overwrite — a reviewer reading `record_originator` would not know to look at `record` semantics. The origin callback path is also not cancellation-safe: if `resolve` is invoked between the `record_originator` read and the `resolve` call, the same id can be resolved twice (the second returns false, but only because `sender.is_none()` after first resolve).
- **Evidence**:
  ```rust
  // callback_sink.rs:38-49
  if let Some(originator) = self.manager.record_originator(&id) {
      if originator != user_id {
          return Some(ApprovalCallbackResult {
              resolved: false,
              response_text: "只有发起该操作的用户可以在此审批。".to_string(),
          });
      }
  }
  let resolved = self.manager.resolve(&id, decision, Some(user_id.to_string()));
  ```
- **Suggested fix**: Rename `record_originator` → `originator_of` (read-only by design). And make the callback sink's "check-then-resolve" atomic — either add `try_resolve_if_originator(id, expected_originator, decision)` to `ExecApprovalManager` (returning `bool`), or use the entry's `sender.is_some()` check inside `resolve` as the single atomic gate.
- **Verification**: Read the callback sink and the manager's `record_originator` (manager.rs:608-617). Confirmed the function does no mutation — name is the only lie.

### APPROVAL-R3-007 — Glob escape list in `glob_to_regex_str` is exhaustive for `.()[]{}^$|+` and `\\` but does NOT escape `*` after `**` peek — the `**` arm reads the next char before deciding to escape, but the same logic also drops the `**` characters themselves without escaping; `*` is correctly handled in the single-`*` arm
- **File**: `src/approval/config.rs:42-77`
- **Severity**: Low (informational)
- **Category**: correctness
- **Description**: After auditing the escape list and the wildcard arms, the function correctly produces safe regex output for the documented operators: `*` → `[^/]*`, `**` → `.*` (with the documented `(?s)` for newline), `**/` → `(.*/)?`, `?` → `[^/]`, and `.()[]{}^$|+` are each prefixed with `\`. Backslash (`\\`) is also escaped. Patterns containing `[abc]` or `{a,b}` are treated as literal characters (since `[` and `{` are in the escape list) — this matches `is_glob_pattern`'s narrower definition (only `*` and `?` are metacharacters). The compiler `Regex::is_match` does NOT do anchor matching itself, so the function correctly anchors with `(?s)^` and `$`. No bugs found in the glob layer itself, but worth documenting that brace/bracket support is intentionally absent (a future maintainer might assume `{a,b}` style alternation works).
- **Evidence**:
  ```rust
  // config.rs:42-77
  match ch {
      '*' => {
          if chars.peek() == Some(&'*') {
              chars.next();
              if chars.peek() == Some(&'/') {
                  chars.next();
                  regex_str.push_str("(.*/)?");
              } else {
                  regex_str.push_str(".*");
              }
          } else {
              regex_str.push_str("[^/]*");
          }
      }
      '?' => regex_str.push_str("[^/]"),
      '.' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '+' | '\\' => {
          regex_str.push('\\');
          regex_str.push(ch);
      }
      _ => regex_str.push(ch),
  }
  ```
- **Suggested fix**: Add a doc comment that `{`/`[`/`?` outside the escape list are literal — currently only `*` and `?` are described, leaving readers to guess about `{`/`[`. Optional: add a test for `{a,b}` → matches literal `{a,b}` (not regex alternation).
- **Verification**: Traced every match arm; mentally evaluated `*`, `**`, `**/`, `?`, `[abc]`, `{a,b}`, `*.txt`, `https://*.github.com/**`, `rm -rf **`, `com.apple.*`. All produced the expected regex. The single test `test_glob_double_star_spans_newlines` (config.rs:527-534) confirms the `(?s)` flag works. No false matches found.

### APPROVAL-R3-008 — `cached_glob_regex` uses `std::sync::Mutex` (blocking) — acceptable because never held across `.await`, but worth documenting as a footgun for future maintainers
- **File**: `src/approval/config.rs:104-122`
- **Severity**: Informational
- **Category**: concurrency
- **Description**: Both the glob cache and the Guardian's breaker use `std::sync::Mutex`. All six call sites that take the lock complete before any `.await`, so today this is safe. If a future maintainer moves any of these to an async context (e.g., to make `redact_target` async so it can read from a remote config), the Mutex becomes a blocking-across-await footgun. The codebase uses `crate::sync_primitives::Mutex` elsewhere (`exec/mod.rs`, `process_journal.rs:112`, `background_persistence.rs:63`); not adopting it here is a stylistic split.
- **Evidence**: grep for `Mutex<` in `src/approval/` returns only `config.rs:105` (`std::sync::Mutex`) and `guardian_requester.rs:188` (`std::sync::Mutex`); test code uses additional ones.
- **Suggested fix**: Either adopt `crate::sync_primitives::Mutex` for consistency, or add a doc comment "// never held across .await" on both fields.
- **Verification**: Greppped all `.await` sites in `config.rs` and `guardian_requester.rs`. Confirmed no `.await` between `lock()` and drop.

### APPROVAL-R3-009 — `audit_identity` only handles the `target` parameter; the `context` field it builds has no length cap, so a very long target produces a giant log line
- **File**: `src/approval/audit.rs:43-49`
- **Severity**: Low
- **Category**: resource / perf
- **Description**: The guardian and approval-card renderings truncate the summary at 200 chars (`sandbox/exec_approval/action.rs:31-34`). The audit log does not — `format!("{domain}.{action} ({target})")` has no length cap on `target`. A `pim.write` of a 1 MB note, or a `clipboard_write` of a full file's contents (if ever allowed), produces a single tracing event with a multi-MB structured payload, which most tracing subscribers will dutifully write to disk or ship to a collector. The `redact_target` hash output (`<redacted len=N sha=...>`) is fixed-size; `context` is not.
- **Evidence**:
  ```rust
  // audit.rs:43-49
  let context = if turn.is_channel_routable() {
      format!("{domain}.{action} ({target}) via {}/{}",
          turn.channel_id, turn.conversation_id)
  } else {
      format!("{domain}.{action} ({target})")
  };
  ```
  Compare `redact_target` (config.rs:376-388) which truncates via the hash output, not the input.
- **Suggested fix**: Cap the context length the same way `redact_and_cap` does in `sandbox/exec_approval/action.rs:381-399` — collapse newlines, slice to N chars, append `…`. Or pass the target through `SecretMasker` first (preferred — see APPROVAL-R3-001).
- **Verification**: Confirmed by reading `audit_identity` and the absence of any length truncation. Compared to `redact_and_cap` which is the established pattern.

### APPROVAL-R3-010 — `run_node_approval` documents "ApprovedAlways is rendered as session grant rather than silently as denied" but the wire token is the same `"approved_session"` regardless of which tier was offered — losing information for nodes that can handle both
- **File**: `src/approval/node_requester.rs:23-41`
- **Severity**: Informational
- **Category**: api-design
- **Description**: The function maps both `ApprovalOutcome::ApprovedForSession` and `ApprovedOutcome::ApprovedAlways` to the same wire token `"approved_session"`. The comment correctly notes this is intentional — the node's `allowed_decisions` is hard-coded to `session_max`, so `AllowAlways` is unreachable from the resolver. However, a node running an older build that sends `AllowAlways` (e.g., during a rolling upgrade) will see it clamped to `approved_session` without warning, and the node's own grant cascade (if it has one) will record it as session-scoped. The "older nodes ignore unknown fields" affordance for the deny_reason works; the inverse (newer center narrowing older nodes' decisions) silently rewrites semantics.
- **Evidence**:
  ```rust
  // node_requester.rs:23-41
  const fn outcome_to_wire(outcome: ApprovalOutcome) -> &'static str {
      match outcome {
          ApprovalOutcome::Approved => "approved",
          ApprovalOutcome::ApprovedForSession | ApprovalOutcome::ApprovedAlways => "approved_session",
          ApprovalOutcome::Denied => "denied",
          ApprovalOutcome::Timeout => "timeout",
          ApprovalOutcome::Unavailable => "unavailable",
      }
  }
  ```
- **Suggested fix**: Log at `warn` when the resolver returned `AllowAlways` so an operator investigating a session grant sees the narrowing happened. The comment already covers "ApprovedAlways is unreachable here" but says nothing about the version-skew path.
- **Verification**: Read `outcome_to_wire` and its caller `run_node_approval`. Confirmed no warn log on the narrowing path. Compared to the in-process twin at `config.rs:355-365` which has no such narrowing, only the cluster path does.

### APPROVAL-R3-011 — `ConfigApprovalPolicy::load_from` error log uses `e.kind()` to distinguish "file not found" from other read errors; the `error!` log includes the full operator path which may contain a username on shared hosts
- **File**: `src/approval/config.rs:217-235`
- **Severity**: Informational
- **Category**: security (defensive)
- **Description**: Minor PII consideration: the error log on a corrupt policy file includes the full path `~/.aleph/approval-policy.json` — which embeds the operator's `$HOME` (username). On a multi-user host with shared log sinks (syslog, journald, a centralized log shipper), the username is exposed in a structured event. The redacted `target` mechanism (config.rs:374-388) sets the precedent of avoiding raw paths in logs.
- **Evidence**:
  ```rust
  // config.rs:225-232
  Err(e) => {
      error!(
          "Failed to parse approval policy at {}: {}. The file exists but is broken, ...",
          path.display(),
          e
      );
  ```
- **Suggested fix**: Log `path.file_name()` (or a basename-only form) instead of `path.display()`. Or hash the path the same way `redact_target` does.
- **Verification**: Read the three error arms of `load_from` (config.rs:217-247). All three include the full path.

### APPROVAL-R3-012 — `OperatorApprovalRequester::request_approval` re-publishes a `ResponseChunk` for the waiting notice with `seq: 0, chunk_index: 0, is_final: false` — but the chunk payload contains a literal `…` character and `run_id`, and uses `delta: notice.clone(), full_text: notice.clone(), content: notice` (three copies of the same string)
- **File**: `src/approval/operator_requester.rs:191-209`
- **Severity**: Low
- **Category**: api-design
- **Description**: The waiting-for-approval notice is dispatched as a single `ResponseChunk`. `delta` and `full_text` and `content` are all three populated with the same string, when the conventional split is `delta` = incremental text and `full_text` = cumulative. With both equal, downstream renderers must decide whether to treat the chunk as a replace or an append. No test covers the actual renderer's behavior; the test only asserts `is_intermediate == true && is_final == false`.
- **Evidence**:
  ```rust
  // operator_requester.rs:200-209
  let notice = format!("⏳ 正在等待管理员授权运行工具 `{tool_name}`…");
  if let Err(e) = self
      .event_bus
      .publish_frame(&GatewayEventFrame::ResponseChunk {
          run_id: t.run_id.clone(),
          seq: 0,
          delta: notice.clone(),
          full_text: notice.clone(),
          content: notice,
          ...
  ```
- **Suggested fix**: Either set `delta` to the notice and `full_text` to the model's prior accumulated text + notice (requires reading prior chunk state), or add a doc comment explaining the convention chosen and a renderer-side test.
- **Verification**: Read the test `emits_waiting_notice_when_run_id_present` (operator_requester.rs:281-345) which only checks `run_id`, `is_intermediate`, `is_final`, never `delta`/`full_text` semantics.

### APPROVAL-R3-013 — `GuardianApprovalRequester::request_approval` returns `ApprovalOutcome::Approved.into()` via `From<ApprovalOutcome>` — losing the `deny_reason` plumbing that other requesters expose
- **File**: `src/approval/guardian_requester.rs:298-300`
- **Severity**: Low
- **Category**: api-design
- **Description**: When the guardian auto-approves a low-risk action, it returns `ApprovalOutcome::Approved.into()` — `From<ApprovalOutcome>` for `ApprovalResponse` (sandbox/exec_approval/gate.rs:107-113) explicitly sets `deny_reason: None`. That's correct for an approval (a reason on an approval is meaningless, per `manager.rs:1171-1182`). But the function's signature returns `ApprovalResponse`, so a future refactor that wants to plumb a guardian-side `low-risk rationale` through to the audit log has no path — the `From` impl discards it.
- **Evidence**:
  ```rust
  // guardian_requester.rs:298-300
  if v.allow && v.risk == "low" {
      tracing::info!(tool = %action.tool_name, rationale = %v.rationale,
          "guardian: auto-approved low-risk action");
      return ApprovalOutcome::Approved.into();    // ← rationale dropped
  }
  ```
- **Suggested fix**: Build the `ApprovalResponse` explicitly so a future field on `Approved` (e.g., `auto_approval_rationale: Option<String>`) can flow through without re-plumbing.
- **Verification**: Read the path. The rationale IS preserved in the `tracing::info!` so the audit log carries it via that side channel — not via `ApprovalResponse`. So this is more of a "lost affordance" than a bug.

### APPROVAL-R3-014 — `ConfigApprovalPolicy` uses `HashMap` not `BTreeMap` for `defaults`; iteration order in error messages / future serializations would be non-deterministic
- **File**: `src/approval/config.rs:38-42`, `src/approval/types.rs:147-149`
- **Severity**: Informational
- **Category**: api-design
- **Description**: `PolicyConfig::defaults` is `HashMap<ActionType, DefaultDecision>`. The default map construction in `Default::default` (config.rs:289-312) uses `HashMap::insert` and is therefore iteration-order dependent. Tests that pin the curated map (e.g., `curated_default_covers_every_action_type`, config.rs:777-820) only check membership, not iteration. If anyone serializes the config back to JSON to display "your effective policy", the key order will vary between calls. Acceptable for an in-memory cache, brittle if the policy ever becomes round-tripped.
- **Evidence**:
  ```rust
  // config.rs:38-42
  pub struct PolicyConfig {
      pub defaults: HashMap<ActionType, DefaultDecision>,
      ...
  ```
- **Suggested fix**: Use `BTreeMap<ActionType, DefaultDecision>` if deterministic iteration is ever needed. Otherwise document the iteration order as undefined.
- **Verification**: Confirmed `HashMap` is used in `PolicyConfig`. No serialization-to-JSON path currently emits the resolved policy.

### APPROVAL-R3-015 — `ActionType::inherited_from` only handles one rename pair; if a future split requires transitive inheritance (parent also has a parent), the resolution in `policy::check` silently drops the chain
- **File**: `src/approval/types.rs:79-95`, `src/approval/config.rs:343-353`
- **Severity**: Informational
- **Category**: correctness (forward-looking)
- **Description**: The one-level-deep invariant is explicitly pinned by test `inheritance_is_one_level_and_acyclic` (config.rs:893-916). The doc on `inherited_from` states "the chain exists to preserve a rename, not to build a taxonomy". The implementation in `policy::check` walks exactly one level:
  ```rust
  let resolved = self.config.defaults.get(action).or_else(|| {
      inherited.as_ref().and_then(|parent| self.config.defaults.get(parent))
  });
  ```
  If a maintainer later extends `inherited_from` to return a chain (`Some(B)` from A where B has its own `inherited_from() == Some(C)`), the code only follows the first hop — C is silently ignored. The invariant test catches that "B has no parent" but not "the policy::check walker only does one hop". A maintainer who reads the test and the function might conclude "looks fine, both one level" and miss the policy-side limit.
- **Evidence**:
  ```rust
  // config.rs:343-353
  let inherited = action.inherited_from();
  let resolved = self.config.defaults.get(action).or_else(|| {
      inherited
          .as_ref()
          .and_then(|parent| self.config.defaults.get(parent))
  });
  ```
  ```rust
  // types.rs:79-95
  #[must_use]
  pub fn inherited_from(&self) -> Option<Self> {
      match self {
          Self::BrowserIdentityOverride | Self::BrowserSessionState => {
              Some(Self::BrowserCookiesWrite)
          }
          _ => None,
      }
  }
  ```
- **Suggested fix**: Either make `inherited_from` return `Vec<Self>` (transitively flattened) and walk the whole chain in `check`, OR add a debug_assert in `inherited_from` that fails at compile-time/runtime when a chain would exceed one hop. The current design has the invariant enforced only on the producer side (the match arm).
- **Verification**: Confirmed by reading both files. The invariant test (config.rs:893-916) asserts that `inherited_from` returns `None` at depth 2 — but `policy::check` would also have to walk depth 2 if `inherited_from` ever returned chained values.

## Cross-cutting concerns

1. **PII / secret leakage through audit logs is the headline finding** (APPROVAL-R3-001). The module's `redact_target` mechanism is well-designed and exercised by tests for the `target` field, but the `context` field bypasses it. Two production call sites (`desktop/mod.rs:350`, `system_tool.rs:63`) pass raw user data directly into the context string. A targeted fix that runs both fields through the existing masker is small and high-value.

2. **The `event_bus.publish_frame` failure swallow pattern** (APPROVAL-R3-003) is consistent across `OperatorApprovalRequester` and `run_node_approval`. Either intentional "best-effort, find the card via `exec.approvals.pending`" or a quiet correctness gap. Worth deciding once and documenting the policy — the asymmetry with `channel_bridge` (which DOES return `Unavailable` on delivery failure) suggests the event-bus path was never given the same consideration.

3. **Glob cache, Guardian breaker, and audit redaction all use `std::sync::Mutex`/`std::sync::OnceLock`**. None are held across `.await`, so today this is safe. The codebase has `crate::sync_primitives::Mutex` for the async-aware case; the approval module's choice is intentional (sync, in-process) but undocumented.

4. **The TOCTOU window in `cached_glob_regex`** (APPROVAL-R3-002) is benign in the worst case but produces brief capacity overshoot and redundant compiles under concurrent first-miss. The 512-cap is "best effort", not "hard ceiling". Document or fix.

5. **The inheritance chain design** (APPROVAL-R3-015) is well-tested at the producer side (types.rs) but not at the consumer side (config.rs::check). A maintainer who extends `inherited_from` to a chain would silently regress one rename.

6. **Operator-only event addressing** (the `frame_session_key()` returning empty for `operator_only = true`) is the only mechanism distinguishing "owner can resolve this" from "operator must resolve this" — and is correctly exercised by the `a_config_tier_card_is_addressed_to_the_operator_not_to_its_raiser` test. No findings here, just confirming the seam is well-pinned.

7. **All 12 files were read in full; the previous audit (sev-wire-2026-08-19-r2) reported 0 findings.** This round found 15 findings, with one Critical (PII leak), one Medium (event-bus swallow), and the rest Low/Informational. The previous audit was verification-focused and didn't dig into the `audit_identity` → `policy.record` path for PII handling — that gap is now closed.

## Summary
- **Total: 15 findings** (1 Critical, 1 Medium, 8 Low, 5 Informational).
- **Top priority items**:
  1. **APPROVAL-R3-001 (Critical)** — `audit_identity` embeds raw target into `context`; `policy.record()` logs raw context. PII/secrets leak through the audit log for `clipboard_write`, `desktop.type`, and any future caller that doesn't sanitize. Fix is small (route both fields through `SecretMasker` or `redact_target`).
  2. **APPROVAL-R3-003 (Medium)** — `event_bus.publish_frame` failures in `OperatorApprovalRequester` and `run_node_approval` are warn-logged but not acted on, leaving operator surfaces with stale "Waiting" cards. Mirror the `channel_bridge` pattern: return `Unavailable` when the initial `ApprovalRequested` publish fails.
  3. **APPROVAL-R3-002 (Low)** — `cached_glob_regex` has a TOCTOU window that allows brief capacity overflow and duplicate compiles. Use `entry().or_insert_with(...)` for atomic miss-and-insert.

The module remains well-tested (inherited tests cover the curated map, the inheritance chain, the originator gate, the breaker state machine, the run-before-publish ordering, and the redact-target redacted form). The findings are concentrated in the seams between audit logging and the policy surface, and between event-bus publication and approval lifecycle — both of which are integration-shaped rather than core-algorithm, which is consistent with r2's seam-lens audit missing them.
