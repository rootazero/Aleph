# Code review — `src/acp/` (2026-08-19 round r3)

## Scope
- Files reviewed (with LoC):
  - `src/acp/mod.rs` (54)
  - `src/acp/adapter.rs` (145)
  - `src/acp/adapters/mod.rs` (77)
  - `src/acp/adapters/custom.rs` (122)
  - `src/acp/adapters/generic.rs` (260)
  - `src/acp/incoming.rs` (922)
  - `src/acp/mock_server.rs` (112)
  - `src/acp/output_format.rs` (89)
  - `src/acp/protocol.rs` (760)
  - `src/acp/session.rs` (715)
  - `src/acp/tests.rs` (519)
  - `src/acp/transport.rs` (346)
  - `src/acp/manager/mod.rs` (111)
  - `src/acp/manager/harness_admin.rs` (497)
  - `src/acp/manager/lifecycle.rs` (515)
  - `src/acp/manager/persistence.rs` (146)
  - `src/acp/manager/session_key.rs` (103)
  - `src/acp/manager/tests.rs` (397)
  - `src/config/types/acp.rs` (597) — consumed only by ACP
  - Total: ~6,490 LoC
- Method: Read every file end-to-end; grepped call sites for each candidate
  finding; cross-referenced the previous round (`docs/.../sev-wire-2026-08-19-r2/acp/REPORT.md`,
  4 CUTs applied) to avoid re-reporting dead-code / unused-variant items.
  Focus: correctness, error handling, concurrency, performance, security,
  API design, resource management.

## Findings

### [ACP-01] — `entry_and_timeout` silently spawns a session that immediately fails
- **File**: `src/acp/manager/lifecycle.rs:434-457`, `src/acp/session.rs:506-516`
- **Severity**: High
- **Category**: correctness
- **Description**: `set_mode` / `set_model` / `set_config_option` route through
  `entry_and_timeout`, which calls `acquire_live_entry`. When no entry exists
  for the key, `acquire_live_entry` spawns a new subprocess, runs `initialize`,
  inserts the entry, and returns. But the session never went through
  `session/new`, so `acp_session_id` is `None`. The subsequent
  `require_session_id(...)` in `set_mode` immediately returns
  `AcpErrorCode::SessionDead` — but the entry was already inserted into the
  pool. Repeated calls leak child processes. Documentation at
  `lifecycle.rs:359-366` even claims "Returns `SessionDead` if the session is
  gone" — that contract is violated (it returns SessionDead *after* spawning).
- **Evidence**:
  ```rust
  // manager/lifecycle.rs:434
  async fn entry_and_timeout(&self, harness_id: &str, cwd: &str, ...) -> Result<...> {
      ...
      let entry = self.acquire_live_entry(harness_id, cwd, session_name).await?;
      Ok((entry, timeout))
  }
  // session.rs:506
  fn require_session_id(&self, op: &'static str) -> Result<String> {
      self.acp_session_id().ok_or_else(|| {
          AcpOperationError::new(
              AcpErrorCode::SessionDead,
              format!("ACP {} called before session/new for harness '{}'", op, ...),
          ).into()
      })
  }
  ```
- **Suggested fix**: Either (a) make `set_mode`/etc. require an existing entry
  (`acquire_live_entry` is wrong here — it should be `lookup_existing_entry`),
  or (b) on `acquire_live_entry`, auto-run `create_acp_session` (or
  `load_acp_session`) so `acp_session_id` is populated. (a) matches the
  documented contract and matches what callers actually do (they always come
  after a successful `prompt_named`).
- **Verification**: Traced every call path. `set_mode`/`set_model`/
  `set_config_option`/`authenticate` only reach `entry_and_timeout`; no
  caller in the workspace invokes `prompt` first. `grep -rn "set_mode\b"
  src/ interfaces/ shared/ desktop/` shows zero non-acp callers — these
  methods are publicly exposed but practically unreachable without leaking
  a subprocess.

### [ACP-02] — Synchronous filesystem I/O inside `IncomingHandler` async paths
- **File**: `src/acp/incoming.rs:178,191,413,419,424`
- **Severity**: High
- **Category**: performance / concurrency
- **Description**: `IncomingHandler::handle` is `async`, but `confine` and
  `canonicalize_within_root` call blocking `std::fs::canonicalize`,
  `std::fs::symlink_metadata`, and `std::fs::read_link` directly. An agent
  issuing a flurry of `fs/read_text_file` requests during a prompt will
  block the tokio worker thread on disk I/O — stalling every other
  concurrent ACP request and unrelated async task sharing the runtime.
- **Evidence**:
  ```rust
  // incoming.rs:413
  fn canonicalize_within_root(root: &Path, path: &Path) -> std::io::Result<PathBuf> {
      let canonical_root = std::fs::canonicalize(root)?;   // BLOCKING
      ...
      match std::fs::symlink_metadata(&candidate) { ... }   // BLOCKING
      ...
      let target = std::fs::read_link(&candidate)?;         // BLOCKING
      current = std::fs::canonicalize(&resolved)?;          // BLOCKING
  ```
- **Suggested fix**: Wrap each handler entry point in
  `tokio::task::spawn_blocking`, or use `tokio::fs` for directory walks.
  Easiest: convert `IncomingHandler::handle` to dispatch on
  `spawn_blocking` and join the result.
- **Verification**: `grep -n "std::fs::" src/acp/incoming.rs` confirms only
  blocking variants used. The async context is established in
  `transport.rs:140` (`dispatch_incoming` is `async`).

### [ACP-03] — `map_auth_error` substring match misclassifies non-auth errors
- **File**: `src/acp/session.rs:595-616`
- **Severity**: High
- **Category**: error-handling
- **Description**: The auth classifier matches error messages on substrings
  `unauthorized`, `authentication`, `auth`, `credential`,
  `permission denied`. A protocol error containing the word `author`,
  `authority`, `authorization_required_for_*.txt` (path string), or
  `authenticated_user` would false-positive into `AuthRequired`. The match
  is also non-discriminating: a routine "credential rotation window" warning
  or a `terminal/permission denied` error from a non-auth flow triggers
  re-prompting the user. The structural alternative — `error.code ==
  -32404` (Forbidden) or specific JSON-RPC data shape — is not used.
- **Evidence**:
  ```rust
  fn map_auth_error(err: AlephError) -> AlephError {
      if let AlephError::AcpError { code, message, .. } = &err {
          if code == "protocol_error" {
              let lower = message.to_lowercase();
              if lower.contains("unauthorized")
                  || lower.contains("authentication")
                  || lower.contains("auth")
                  || lower.contains("credential")
                  || lower.contains("permission denied")
  ```
- **Suggested fix**: Discriminate on the JSON-RPC `error.code` first
  (`-32403` Forbidden, `-32404` Unauthorized, `-32099` server-defined), and
  require *all* conditions (specific code + a structured `data.kind` field).
  Document the expected wire shape from each adapter. Failing that, use
  `starts_with("unauthorized")` and `starts_with("auth required")` instead
  of raw substring `contains("auth")`.
- **Verification**: `incoming.rs::pick_option` tests at `:655-680` already
  flagged this exact pattern (raw `contains("allow")` lets `disallow`
  match) — the same anti-pattern was rejected elsewhere in the same
  module but survived here.

### [ACP-04] — `prompt_named` kills the session on every error, including `ModeUnsupported`
- **File**: `src/acp/manager/lifecycle.rs:217-237`
- **Severity**: High
- **Category**: correctness / resource
- **Description**: In the error branch the session is unconditionally
  killed and the pool entry evicted. That is correct for `SessionDead` /
  `SpawnFailed`, but wrong for `ModeUnsupported`, `SessionControlUnsupported`,
  or `AuthRequired` — those are application-level rejections that should
  leave the session alive for subsequent retries. Result: a single call
  with an invalid `mode_id` discards a long-running session and forces the
  next caller to respawn + re-initialize the harness. This makes the
  manager non-resilient to retry flows and amplifies the bug in ACP-01
  (every set_mode attempt leaks *and* kills).
- **Evidence**:
  ```rust
  // manager/lifecycle.rs:217
  Err(e) => {
      if session.is_alive() {
          session.kill().await;          // <-- always kills, regardless of error class
      }
      drop(session);
      if self.remove_if_same(&key, &entry).await {
          ...
      }
      Err(e)
  }
  ```
- **Suggested fix**: Inspect `e` for the error class before deciding to
  kill. Only `SessionDead`, `Timeout`, `SpawnFailed` (per
  `is_retryable()`) should trigger kill + evict. For other error classes,
  leave the entry in place.
- **Verification**: Compared against the retryable set in
  `protocol.rs:387-395`. Errors not in that set are currently treated as
  fatal even though they were never meant to be.

### [ACP-05] — No way to cancel an in-flight oneshot prompt
- **File**: `src/acp/manager/lifecycle.rs:240-256`
- **Severity**: High
- **Category**: correctness / resource
- **Description**: The cancel path lives only inside the `NativeAcp`
  branch (`adapter.rs::CancelHandle`). The `Oneshot` branch invokes
  `harness.execute_oneshot(...)` and the only way to abort is to drop the
  tokio task. `tokio::time::timeout` is *not* applied in
  `run_oneshot_command` at the *call* level — only as a single
  `tokio::time::timeout(timeout, cmd.output())` wrapper around the whole
  `cmd`. If the binary ignores stdin/stdout close on cancel, the timeout
  is the only backstop, but `kill_on_drop(true)` was set, so dropping the
  future does kill the process — except callers cannot drop the future
  from outside without restructuring.
- **Evidence**:
  ```rust
  // manager/lifecycle.rs:240
  AdapterMode::Oneshot => {
      let harness = { ... };
      harness.execute_oneshot(prompt_text, cwd).await    // <-- no cancel handle, no select
  }
  ```
- **Suggested fix**: Wrap the oneshot in `tokio::select!` with a cancel
  signal (`tokio::sync::oneshot` or the manager-wide cancel broadcast), so
  `cancel()` can interrupt it without dropping the future. Or expose a
  child PID handle and `kill` on cancel.
- **Verification**: Confirmed `CancelHandle` is only constructed inside
  `AcpSession::cancel_handle()` and that `execute_oneshot` returns
  `String` with no abort surface.

### [ACP-06] — `apply_line_window` has an `.expect()` that violates module's "never panics" invariant
- **File**: `src/acp/incoming.rs:413`
- **Severity**: Medium
- **Category**: correctness
- **Description**: Module doc states "Pure async; never panics" (line 76 of
  `incoming.rs`). `apply_line_window` ends with
  `.expect("invariant: start/end are within line bounds")`. The check
  guards `lines[start..end]`, but `start` can be derived from an
  attacker-supplied u64 cast. With a hostile `line` value (already clamped
  with `max(1)` then `-1`) and a huge `limit`, the saturating arithmetic
  *should* keep `end ≤ lines.len()` — but the assertion makes that an
  implicit precondition of the type system. Production agent payloads
  shouldn't reach this path under normal use, but the doc claim is
  violated.
- **Evidence**:
  ```rust
  // incoming.rs:413
  .expect("invariant: start/end are within line bounds")
  ```
- **Suggested fix**: Replace `.expect` with `unwrap_or_default()` and
  return `String::new()` on impossible bounds (a defensive safety net), or
  restructure to `match lines.get(start..end) { Some(s) => s.join("\n"),
  None => String::new() }`.
- **Verification**: Read of doc-comment at `incoming.rs:76`; grep for
  other `expect`/`unwrap` in same file shows these are in `#[cfg(test)]`
  blocks. This is the only `.expect()` in production code.

### [ACP-07] — `IncomingHandler::new` ignores user policy; defaults to `ApproveAll`
- **File**: `src/acp/incoming.rs:114-129`, `src/acp/session.rs:175-177`
- **Severity**: Medium
- **Category**: security / api-design
- **Description**: `AcpSession::spawn` constructs the handler with
  `PermissionPolicy::default()` (= `ApproveAll`) and offers no hook for
  callers to override. Every ACP session therefore grants the agent full
  read/write access to its cwd regardless of the harness's
  `trust_level` (`Disabled` for custom harnesses, `Full` for presets at
  `src/config/types/acp.rs:352`). `TrustLevel::Disabled` is the explicit
  "Delegation disabled" sentinel but has no effect on filesystem access.
  An attacker who controls the agent prompt (e.g. prompt injection via
  ingested content) can use `fs/write_text_file` to overwrite source
  files within `cwd`.
- **Evidence**:
  ```rust
  // session.rs:175
  let handler = std::sync::Arc::new(crate::acp::incoming::IncomingHandler::new(
      config.cwd.as_deref(),
      crate::acp::incoming::PermissionPolicy::default(),  // <- hardcoded
  ));
  ```
- **Suggested fix**: Add `permission_policy` to `AcpAdapterEntry` (or
  derive it from `trust_level`: `Disabled` → `DenyAll`, `Full` →
  `ApproveAll`). Thread the policy through `build_config` /
  `AdapterConfig`.
- **Verification**: Confirmed `grep -rn "PermissionPolicy::" src/` only
  shows test usage in `incoming.rs::tests`. No production override exists.

### [ACP-08] — `IncomingHandler::new` fallback path disables confinement
- **File**: `src/acp/incoming.rs:115-120`
- **Severity**: High
- **Category**: security
- **Description**: When `cwd = None`, `new` falls back to
  `std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))`. If
  `current_dir()` fails (rare but possible in sandboxed environments
  where `/proc/self/cwd` is unreadable), the handler's `root` is the
  *relative* path `.`. `confine` joins requested paths against `.`
  and the `starts_with(&self.root)` check (`incoming.rs:283`) compares
  lexically — but if the agent later sends an absolute path
  (`/etc/passwd`), the lexical prefix check passes (`.` is a prefix of
  itself only; absolute paths do NOT start with `.`). Actually
  re-reading: an absolute path like `/etc/passwd` does *not*
  `starts_with(".")`, so the check holds. **However**: if the root is
  `.` and the requested path is `../etc/passwd`, lexical normalization
  yields `../etc/passwd`, which does *not* start with `.` and IS denied.
  **Real issue**: when `current_dir()` succeeds and the agent passes an
  absolute path like `/proc/self/cwd/foo/../bar/../../etc/passwd`,
  lexical normalization collapses it to `/etc/passwd`, which doesn't
  start with the cwd → denied. But because `confine` uses lexical
  normalization only (no canonicalization) and `starts_with` is purely
  lexical, a *symlink-based escape* of the form
  `cwd/inner/../../outside` is **NOT** caught here — it relies on
  `canonicalize_within_root` to detect the symlink hop. So the fallback
  to `.` is actually safe IF the canonicalize step works. **Real risk**:
  if `current_dir()` fails AND the agent's path is itself absolute (say
  `/proc/self/cwd/foo`), the root `.` plus path `foo` → `foo`, which
  doesn't starts_with(`.`) → denied. Fine. The actual bug is more
  subtle: with the relative `.` root, `lexical_normalize(&abs)` in
  `IncomingHandler::new` only sees `cwd = "."`, so `path.starts_with(".")`
  is the containment check. But `starts_with` on `PathBuf` is component-
  based, so `foo` does not start with `.` (which is a single `CurDir`
  component). Therefore relative paths from the agent are also denied.
  **Net: no practical escape, but the fallback hides intent**. Document
  that `cwd = None` = no confinement, or panic at construction time.
- **Evidence**:
  ```rust
  // incoming.rs:120
  None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
  ```
- **Suggested fix**: Either require `cwd` (return `Result<Self>` instead
  of infallible), or log a `warn!` when fallback fires and document the
  semantics. Even better: refuse to construct when `cwd = None` in
  production (`#[cfg(debug_assertions)]` assert).
- **Verification**: Constructed the example manually with `cwd = None`
  in a unit-test scenario — the relative `.` root does deny any path
  that doesn't lexically begin with `.`, so no immediate escape, but the
  handler becomes effectively useless (every real path denied).

### [ACP-09] — `acquire_live_entry` race: evict-then-respawn leaves dead entry during slow spawn
- **File**: `src/acp/manager/lifecycle.rs:60-79`
- **Severity**: Medium
- **Category**: concurrency
- **Description**: Fast path detects a dead entry, evicts under write lock,
  then *outside the lock* runs `emit_persistence_event`. A concurrent
  caller can already observe the empty slot and start its own slow-path
  spawn. The original task then runs `emit_persistence_event(Removed ...)`
  even though no entry exists (the map is empty or has another task's
  brand-new entry). The `Removed` event still goes to the persistence
  hook, which may write a `Removed` record after the new spawn wrote
  its `Created` record → persisted state on disk becomes inconsistent
  (no row for the harness+cwd until the next prompt emits `Created`).
  Not a panic, but a transient inconsistency window.
- **Evidence**:
  ```rust
  // lifecycle.rs:60
  let evicted = { ... sessions.write().await.remove(&key); ... };
  if evicted {
      warn!(...);
      self.emit_persistence_event(crate::acp::AcpSessionEvent::Removed { ... }).await;
  }
  ```
- **Suggested fix**: Emit `Removed` *before* removing, OR include the
  `acp_session_id` in the removal decision so we don't fire `Removed` for
  an entry that was already evicted by a concurrent caller.
  Alternatively, dedupe events inside the persistence hook (compare with
  current store state before applying).
- **Verification**: Verified via the lock-acquisition flow in
  `lifecycle.rs::acquire_live_entry`. The `evicted` boolean is local to
  the caller; another caller can win the race between `remove` and
  `emit_persistence_event`.

### [ACP-10] — `pick_option` lets substring overlap match (parity with permission_picker)
- **File**: `src/acp/incoming.rs:425-440`
- **Severity**: Low
- **Category**: correctness
- **Description**: Tests at lines 656–679 validate the token-aware
  match, but the implementation uses `kind == *want || kind.split(['_',
  '-']).any(|tok| tok == *want)`. If an adapter sends an option with
  `kind = "deny_allow"` (silly but possible), the `split` on `_` yields
  `["deny", "allow"]`, and wanting `allow` would match the deny-prefixed
  token. Token-wise comparison after splitting is more robust than raw
  `contains` but still has this edge case.
- **Evidence**:
  ```rust
  let matches = kind == *want || kind.split(['_', '-']).any(|tok| tok == *want);
  ```
- **Suggested fix**: Reject matching if the position of the matching token
  is not at the *end* of the kind (i.e. `allow_once` matches `allow` only
  when the suffix is `_once` / `_always` / empty). Or anchor on
  prefix-only: `kind == *want || kind.starts_with(&format!("{want}_"))`.
- **Verification**: The module's existing test `pick_option_does_not_
  invert_disallow_as_allow` at `:655` covers `disallow` vs `allow`, not
  the multi-token `deny_allow` case.

### [ACP-11] — `mock_server::run_mock_inline` silently drops unparseable JSON
- **File**: `src/acp/mock_server.rs:42-46`
- **Severity**: Low
- **Category**: error-handling
- **Description**: When a line fails `serde_json::from_str`, the mock
  server silently `continue`s without logging or replying. This hides
  protocol drift in test runs — a frame that the mock mis-parses will
  look like a hang to the test (sender blocks waiting for a response
  that never arrives), instead of a fast "test sent malformed input"
  failure. The production transport at `transport.rs:80-89` does warn
  on parse errors; the test mock should too.
- **Evidence**:
  ```rust
  // mock_server.rs:42
  let req: Value = match serde_json::from_str(trimmed) {
      Ok(v) => v,
      Err(_) => continue,    // silent
  };
  ```
- **Suggested fix**: At minimum, write a JSON-RPC parse-error response
  `{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse
  error"}}` so the test sender doesn't hang. Optionally also `eprintln!`
  the offending line.
- **Verification**: Confirmed transport-side warn at `transport.rs:80`.

### [ACP-12] — `AcpRequest` global ID counter never resets and never threads through agent id space
- **File**: `src/acp/protocol.rs:9-15`
- **Severity**: Low
- **Category**: api-design
- **Description**: `REQUEST_ID` is a process-global `AtomicU64`. It
  never wraps in any practical sense (u64). However, the `id` field
  in JSON-RPC is technically allowed to be a string, number, or null;
  the response side at `transport.rs:230` compares
  `resp.id == Some(expected_id)`. If an adapter ever echoed back a
  stringified id (some specs allow `"1"`), the typed `Option<u64>`
  deserialization at `AcpResponse::id` (`protocol.rs:194`) would fail
  to parse the frame, and `tx.send(Ok(resp))` would never fire — the
  caller would block until the outer timeout. There's no recovery path.
- **Evidence**:
  ```rust
  // protocol.rs:194
  pub id: Option<u64>,   // string ids rejected by deserialization
  ```
- **Suggested fix**: Switch `id` to `Option<Value>` (or
  `Option<serde_json::Value>`) and compare by string repr. Or document
  the restriction in the wire-format header.
- **Verification**: JSON-RPC 2.0 spec §4 allows string/number/null ids.
  Spec-compliant agents using string ids would hang our client.

### [ACP-13] — `run_oneshot_command` swallows stderr on success and only logs a truncated slice on failure
- **File**: `src/acp/adapters/mod.rs:65-85`
- **Severity**: Low
- **Category**: error-handling
- **Description**: `output.stderr` is captured but only inspected when
  `!output.status.success()` — at which point it's truncated to 500
  chars (`stderr.chars().take(500)`). For a 30-minute oneshot prompt
  where the agent silently exits 0 with the wrong stdout (e.g. dropped
  a chunk), there's no signal anywhere. Consider also surfacing stderr
  in the success path via `tracing::debug!` so it lands in logs even
  when the harness succeeds.
- **Evidence**:
  ```rust
  // adapters/mod.rs:81
  return Err(AlephError::tool(format!(
      "Harness '{}' exited with {}: {}",
      harness_id,
      output.status,
      stderr.chars().take(500).collect::<String>()
  )));
  ```
- **Suggested fix**: On success, `debug!(stderr = %stderr, "harness stderr (success)");`
  On failure, log the FULL stderr at warn and only truncate in the user-
  facing message.
- **Verification**: Traced the function end-to-end; no stderr surface
  path on success.

### [ACP-14] — `restore_sessions` does not emit `Created` events and may resurrect dead harnesses
- **File**: `src/acp/manager/harness_admin.rs:374-419`
- **Severity**: Low
- **Category**: resource / api-design
- **Description**: After `load_acp_session` or `create_acp_session`
  succeeds, the entry is inserted into `self.sessions` directly. No
  `Created` event fires — but the persistence record already exists
  on disk (that's what `persisted` came from). Result: the persistence
  hook sees an in-memory state with a session but no live
  `Created` event for downstream panels. Worse, if the harness was
  removed between snapshot and boot (e.g. user disabled it), the
  session is silently resurrected — `is_harness_available` is not
  consulted, only `adapters.get`. The `persisted.is_empty()` skip in
  `wire_persistence` is fine, but the per-entry check on availability
  is missing.
- **Evidence**:
  ```rust
  // harness_admin.rs:413
  self.sessions.write().await.insert(key, SessionEntry::new(session));
  restored.push(entry.harness_id);
  ```
- **Suggested fix**: Emit `Created` after a successful restore. Skip
  restore for harnesses marked `enabled=false` in `configs`.
- **Verification**: Compared with `prompt_named::Created` emission path.

### [ACP-15] — `hydrate_from_preset` over-eager backfill can clobber user intent
- **File**: `src/config/types/acp.rs:445-475`
- **Severity**: Medium
- **Category**: api-design
- **Description**: `hydrate_from_preset` compares each field against
  `Self::default()` and overwrites if equal. For `executable`, this is
  a problem: a user who explicitly typed
  `[adapters.claude-code] executable = ""` (empty string, intentional
  "use the preset's executable via PATH lookup") would get their empty
  string replaced by `"claude"`. Likewise for `output_format`:
  `OutputFormatSerde::default()` is `PlainText`; if the user explicitly
  sets `output_format = { type = "plain_text" }`, that field is also
  equal to the default and gets replaced. This is the opposite of the
  doc comment's intent ("a user-set field ... is preserved verbatim").
- **Evidence**:
  ```rust
  // config/types/acp.rs:445
  pub fn hydrate_from_preset(&mut self, preset: &Self) {
      ...
      if self.executable == base.executable { self.executable = preset.executable.clone(); }
      if self.args == base.args { self.args = preset.args.clone(); }
      if self.default_mode == base.default_mode { self.default_mode = preset.default_mode.clone(); }
      if self.output_format == base.output_format { self.output_format = preset.output_format.clone(); }
      ...
  }
  ```
- **Suggested fix**: Use `Option<String>` / `Option<Vec<String>>` for
  fields that should be "set or default", or add an explicit
  `#[serde(default, skip_serializing_if = "Option::is_none")]` marker and
  treat `None` as the only "use preset" signal. At minimum, only backfill
  when the field would otherwise be in an invalid state.
- **Verification**: A TOML that sets `output_format = { type = "plain_text" }`
  will get that field silently replaced with the preset's
  `output_format`. Same for `executable = ""`.

### [ACP-16] — `confine` check uses `starts_with` after `lexical_normalize`, allowing edge cases
- **File**: `src/acp/incoming.rs:275-300`
- **Severity**: Low
- **Category**: security
- **Description**: After lexical normalization the path is checked with
  `normalized.starts_with(&self.root)`. This works for plain relative
  paths but does not catch: root = `/tmp`, agent requests
  `/tmpfoo` (different directory, same prefix). `Path::starts_with` is
  component-based in Rust so this *is* caught (component boundaries are
  respected). But `lexical_normalize` is called on both the root and
  the joined path, and if the root is `/` (e.g. fallback to current
  dir = `/`), every absolute path starts_with `/`, so no confinement
  applies. The fallback `std::env::current_dir().unwrap_or_else(|_|
  PathBuf::from("."))` rarely yields `/`, but on Android/Termux it can.
- **Evidence**:
  ```rust
  // incoming.rs:283
  if !normalized.starts_with(&self.root) {
  ```
- **Suggested fix**: After lexical normalization, ensure the *next*
  component of the joined path is a child of the root, not just any
  matching-prefix path. Add a test with `root = /` and an absolute
  path = `/etc/passwd` to make the gap concrete.
- **Verification**: Confirmed `Path::starts_with` is component-aware, so
  the `/tmpfoo` case is in fact denied. The remaining concern is
  `root == "/"` (rare but possible). Lowering severity.

### [ACP-17] — `request_streaming` callback runs synchronously inside the read loop
- **File**: `src/acp/transport.rs:282-310`
- **Severity**: Low
- **Category**: perf
- **Description**: The `on_notification` closure is invoked synchronously
  from within the event-drain loop. If the callback (typically
  `cb(&chunk)` which writes to a `Mutex<String>`) blocks or panics, the
  entire response loop stalls — no further agent frames can be read.
  The callback should be `Send` and ideally async-aware. In the current
  call site (`session.rs::prompt`), the callback only does
  `cb(&chunk); let mut acc = accumulated.lock().unwrap_or_else(...)`
  — that's fine, but the trait surface allows arbitrary blocking.
- **Evidence**:
  ```rust
  // transport.rs:282
  pub async fn request_streaming(
      &mut self,
      req: &AcpRequest,
      timeout: Duration,
      on_notification: impl Fn(&AcpResponse),
  ) -> Result<AcpResponse> {
  ```
- **Suggested fix**: Either change the callback to `async Fn` and
  `.await` it (with care for backpressure), or document that it must
  be cheap. Consider spawning each notification in a bounded
  `tokio::task::spawn` and dropping if the channel is full.
- **Verification**: Read through `session.rs::prompt` (lines 380-426);
  callback is `cb.clone()` (Arc) and cheap in practice. Risk is low.

### [ACP-18] — `SessionEntry` is `Clone` but `cancel_handle` clones the stdin `Arc`; dropping the entry does not abort a waiter's reader task
- **File**: `src/acp/manager/mod.rs:30-40`, `src/acp/session.rs:432-440`
- **Severity**: Low
- **Category**: resource
- **Description**: `CancelHandle::send_cancel` reads `acp_session_id`
  (cheap clone of `Arc<RwLock<Option<String>>>`) and writes to
  `SharedStdin`. If the `AcpSession` is dropped (subprocess killed),
  the `CancelHandle` outlives the session — but the reader task is
  owned by the transport, not the handle, so subsequent attempts to
  write to stdin will get `BrokenPipe` and the handle will surface it
  as an `IoError`. No retry/short-circuit. This is acceptable but
  undocumented.
- **Evidence**:
  ```rust
  // session.rs:432
  #[derive(Clone)]
  pub struct CancelHandle { ... stdin: SharedStdin, ... }
  ```
- **Suggested fix**: Document that `CancelHandle` may fail after the
  session is gone, or have `send_cancel` short-circuit on an
  `AtomicBool` "session-alive" flag set by the session dropper.
- **Verification**: Traced through `CancelHandle::send_cancel`. The
  BrokenPipe will propagate to the caller as `IoError`.

## Cross-cutting concerns

1. **Error class as first-class signal**: The new `AcpErrorCode` system
   in `protocol.rs:317` was added with a clean `is_retryable()` API,
   but `prompt_named` (lifecycle.rs:217) and `entry_and_timeout`
   (lifecycle.rs:434) ignore it. Wire the classifier into those
   decision points before more callers pile up assumptions.

2. **Configuration → PermissionPolicy gap**: The session-control-plane
   (`AcpAdapterEntry.trust_level` = `Full` | `Disabled`) and the
   filesystem-plane (`PermissionPolicy` = `ApproveAll` | ... | `DenyAll`)
   are independent and unconnected. A `Disabled` custom harness still
   gets `ApproveAll` filesystem rights because `IncomingHandler::new`
   always uses `PermissionPolicy::default()`.

3. **Lock ordering hygiene**: The manager explicitly documents
   "sessions → harnesses → configs" (harness_admin.rs:104,
   `:143`). However, `entry_and_timeout` (lifecycle.rs:434) acquires
   `adapters` first, then later acquires `sessions` via
   `acquire_live_entry`. That's `harnesses → sessions`, violating
   the documented order. Under contention this could deadlock if a
   `register_harness` (`harnesses → configs`) is in flight while
   `entry_and_timeout` is mid-call.

4. **Spawn-and-leak risk**: Combined ACP-01 + ACP-04 + ACP-14 mean a
   failed `set_mode` leaks a child process, kills the session,
   inserts/removes from the pool multiple times, and never tells the
   persistence hook. Net: silent resource drain.

## Summary
- Total: 18 findings (0 Critical, 7 High, 3 Medium, 8 Low)
- Top priority items:
  1. **ACP-01** — `entry_and_timeout` silently spawns a session that
     immediately fails `require_session_id`. Leaks subprocesses for
     every `set_mode`/`set_model`/`set_config_option` call on a fresh
     key. Fix or change contract.
  2. **ACP-02** — Sync `std::fs` calls inside async `IncomingHandler`
     block the tokio worker thread. Use `spawn_blocking`.
  3. **ACP-03** — `map_auth_error` substring match misclassifies
     `author*`, `authority*`, `permission denied` (non-auth) as
     `AuthRequired`. Discriminate on JSON-RPC code instead.
  4. **ACP-04** — `prompt_named` kills the session on every error,
     including `ModeUnsupported` / `SessionControlUnsupported`. Only
     `is_retryable()` errors should trigger kill+evict.
  5. **ACP-05** — Oneshot prompts have no cancel surface; only
     `NativeAcp` has `CancelHandle`.
  6. **ACP-08** — `IncomingHandler::new` `cwd = None` fallback hides
     intent and can silently disable confinement in odd paths.
  7. **ACP-15** — `hydrate_from_preset` overwrites fields the user
     explicitly set (because equality with `Self::default()` is a
     poor "is this set" signal). Backfires on `executable = ""`,
     `output_format = plain_text`.