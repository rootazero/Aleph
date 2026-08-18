# Code review — `src/builtin_tools/` chunk A (2026-08-19 round r4)

## Scope
- Files reviewed (production `src/builtin_tools/`, ~20,713 LoC total):
  - Top-level files (core execution / shell / config / system tools):
    `bash_exec.rs`, `code_check.rs`, `code_exec.rs`, `command_canonicalize.rs`, `command_ledger.rs`,
    `config_audit.rs`, `config_guide.rs`, `doctor.rs`, `error.rs`, `meta_tools.rs`, `mod.rs`,
    `permission_tool.rs`, `search.rs`, `system_tool.rs`, `tool_usage.rs`
  - Subdirectories:
    - `file_ops/` (all 16: `apply_patch.rs`, `batch.rs`, `edit.rs`, `edit_match.rs`, `image_read.rs`,
      `mod.rs`, `ops.rs`, `path_utils.rs`, `read.rs`, `read_cache.rs`, `search.rs`, `stats.rs`,
      `text.rs`, `tool.rs`, `types.rs`, `write.rs`)
    - `generation/` (all 5: `audio_generate.rs`, `image_generate.rs`, `mod.rs`, `speech_generate.rs`,
      `video_generate.rs`)
    - `hub/` (all 7: `catalog_search.rs`, `catalog_sync.rs`, `fetch_docs.rs`, `install_run.rs`,
      `install_verify.rs`, `mod.rs`, `resolve_spec.rs`)
- Cross-checked callers:
  - `executor/builtin_registry/` (where the tools are dispatched)
  - `tools::AlephTool` trait used by all tools
  - `sandbox::Sandbox` consumers (`bash_exec`, `code_exec`, `code_check`)
  - `sandbox::deny_globs::glob_to_anchored_regex` (the OS-floor translator shared by `file_ops/path_utils`)
  - `vault::SharedTokenManager` (secret storage in `hub/install_run`)
  - `event::GlobalBus` (process-completion announce used by `bash_exec::spawn_background`)
- Method: read-first sweep across all files, focused on correctness, error handling, resource
  management, security, and concurrency. Cross-referenced prior reports under
  `review-results/sev-wire-2026-08-19-r3/` only for format consistency (not re-audit content).

## Findings

### BT-A-R4-01 — `FileReadTool`'s `ReadCache` is unbounded: long sessions slowly leak memory across every distinct file window
- **File**: `src/builtin_tools/file_ops/read_cache.rs:18-93`
- **Severity**: Medium
- **Category**: resource
- **Description**: `ReadCache` keeps a `HashMap<ReadKey, ReadFingerprint>` that is never evicted.
  Every `FileReadTool` clone shares the same backing `Arc<Mutex<…>>`, so the map is shared across
  the lifetime of a session. Each distinct `(canonical_path, offset, limit)` triple inserts a
  row that lives until the `FileReadTool` is dropped (i.e. until the session ends). A long-lived
  agent that reads tens of thousands of distinct windows over the course of a day accumulates
  the same number of cache entries — the fingerprint is 32 bytes (mtime + size + u32) plus the
  String key, so a million-row cache runs ~80 MB. This is the only unbounded structure under
  `file_ops/`; every other tool enforces an explicit size cap (`stats`, `search`, `list`,
  `command_ledger`, `process_registry`).
- **Evidence**:
  ```rust
  // read_cache.rs:18-22
  pub(super) struct ReadCache {
      inner: Arc<Mutex<HashMap<ReadKey, ReadFingerprint>>>,
  }
  // read_cache.rs:61-92 (observe) — every successful observe either inserts or updates;
  // there is no removal path other than `map.remove(&key)` on a missing fingerprint.
  ```
  Compare with `command_ledger.rs:60-69` which has explicit LRU eviction at MAX_COMMANDS_PER_SESSION.
- **Suggested fix**: Add an LRU or simple size cap (e.g. 10 000 entries). On insert, drop the
  least-recently-touched key when the cap is exceeded. Or use a fixed-size `lru::LruCache` (the
  `lru` crate is already a dependency of Aleph per `Cargo.toml`). Mirror the discipline
  `command_ledger` enforces.
- **Verification**: Grep for `ReadCache`, `map.remove`, `evict`, `cap` — the only removal path
  (`map.remove(&key)`) fires on a missing fingerprint (line 76), not on growth. No `clear()`,
  no eviction policy.

### BT-A-R4-02 — `hub_install_run` silently drops JSON arrays / objects / nulls from `config_values`, leaving the install misconfigured without warning
- **File**: `src/builtin_tools/hub/install_run.rs:180-184, 233-249`
- **Severity**: Medium
- **Category**: error-handling / correctness
- **Description**: `value_to_string` returns `None` for every `Value` variant except
  `String`/`Number`/`Bool`. The caller `proceed()` then `continue`s past the `None` arm
  silently, so a config field that the model sent as a JSON array, object, or null is dropped
  without any log line or error. The install then runs with that field missing from
  `plain_values` / `secret_refs`. The model gets back `Installed { ok: true, … }` (gated only by
  the post-install `verify` step), and the missing field surfaces as a runtime failure deep
  inside the new MCP server / skill — far from the install call, in a place that points at the
  extension rather than the install command. The denial is silent.
- **Evidence**:
  ```rust
  // install_run.rs:180-184
  fn value_to_string(v: &Value) -> Option<String> {
      match v {
          Value::String(s) => Some(s.clone()),
          Value::Number(n) => Some(n.to_string()),
          Value::Bool(b) => Some(b.to_string()),
          _ => None,
      }
  }
  // install_run.rs:233-249 — `let Some(val) = value_to_string(raw) else { continue; };`
  ```
- **Suggested fix**: Either (a) reject any `config_values` entry whose `Value` is not a string
  with `AlephError::tool(format!("config field '{name}' must be a string, got {kind}"))` so the
  model sees the mistake on the install call, or (b) at minimum `tracing::warn!` the drop with
  the field name + actual `Value` kind so the operator can spot silent misconfiguration in
  logs.
- **Verification**: The function is called only from `proceed()`; `grep -n "value_to_string"`
  in `hub/` returns the definition plus that single call site. No tests cover the array/object
  drop path.

### BT-A-R4-03 — `system_tool::clipboard_read` is not gated by the approval policy that gates `clipboard_write`
- **File**: `src/builtin_tools/system_tool.rs:198-222, 230-321`
- **Severity**: Medium
- **Category**: security
- **Description**: The state-changing action gate at `system_tool.rs:202-222` adds
  `ActionType::DesktopLaunchApp` for `launch_app`/`quit_app`/`restart_app`/`open_path` and
  `ActionType::DesktopType` for `clipboard_write`, but `clipboard_read` falls through the
  `_ => None` arm entirely. An LLM-injected prompt that asks the agent to run
  `{"action":"clipboard_read"}` will read whatever the user has on their clipboard (passwords
  from a password manager, 2FA codes copied by another app, private conversation snippets)
  without any user-visible approval. `clipboard_write` is gated for the obvious reason —
  destructive side-effect on the user's clipboard — but read is the symmetric disclosure risk
  and is silently treated as benign. The rest of the `system_tool` gating is deliberately
  symmetric (see `DesktopTool`'s pair of gates), so this is a single-source asymmetry that
  reads like an oversight rather than a design choice.
- **Evidence**:
  ```rust
  // system_tool.rs:202-222
  let gated = match args.action.as_str() {
      "launch_app" | "quit_app" | "restart_app" => Some((ActionType::DesktopLaunchApp, …)),
      "open_path" | "open" => Some((ActionType::DesktopLaunchApp, …)),
      "clipboard_write" => Some((ActionType::DesktopType, …)),
      _ => None,   // <-- "clipboard_read" / "send_notification" / "list_*" / "system_info" / "user_idle_time"
  };
  ```
  Compare with the symmetric gating the sibling `DesktopTool` is documented to apply (per
  the file's `with_approval_policy` doc comment).
- **Suggested fix**: Add a `ActionType::DesktopReadClipboard` variant (or reuse
  `ActionType::DesktopType` / a new sensitive-read variant) and route `clipboard_read` through
  the same gate. If the policy `Allow`s by default (current behaviour for other gated actions),
  this is byte-identical to today's behaviour for honest callers and adds a single approval
  dialog for prompt-injected ones.
- **Verification**: Grep for `clipboard_read` in `system_tool.rs` returns the read handler at
  line ~308 with no gate check on the path leading to it. The same `permission_tool.rs` has a
  per-permission gate for every action — clipboard read is the asymmetric exception.

### BT-A-R4-04 — `expand_input_path` does string-level `.replace("$HOME", …)` that mangles paths containing `$HOME`-like substrings (`$HOMEBREW`, `$HOMEDIR`)
- **File**: `src/builtin_tools/file_ops/path_utils.rs:603-625`
- **Severity**: Medium
- **Category**: correctness / security
- **Description**: The path resolver's env-var expansion replaces every literal occurrence of
  `$HOME` and `$USER` in the *user-supplied path string* via `String::replace`, not a proper
  tokenizer. A path string the user constructs as `/opt/$HOMEBREW/bin/foo` (a perfectly normal
  homebrew install location) becomes `/opt//home/aliceBREW/bin/foo`, which then fails
  `canonicalize()` and the read is refused with an opaque error. The same substitution also
  fires on a path that contains the substring in a comment-style position, e.g. `$(HOME)`.
  The doc comment on the function claims the choice is a security feature ("arbitrary env var
  expansion could allow path injection") — but the security argument is about *which* env vars
  to expand, not *how* to do it; a proper `ShellQuotedWord`-style expander would honour the
  same allowlist without the substring-mangling hazard.
- **Evidence**:
  ```rust
  // path_utils.rs:603-625 (excerpted)
  if path_str.contains('$') {
      let mut result = path_str.to_string();
      if let Some(home) = dirs::home_dir() {
          result = result.replace("$HOME", &home.to_string_lossy());
      }
      if let Ok(user) = std::env::var("USER") {
          result = result.replace("$USER", &user);
      }
      …
  }
  ```
- **Suggested fix**: Anchor the substitution to a `$(VAR)` form (`result = regex_replace(r"\$\{?HOME\b\}?", home)`) or scan token-by-token. The 1-line fix is to gate the substitution on a word boundary: only substitute `$HOME` when it is followed by `/` or end-of-string, and `$USER` likewise.
- **Verification**: A 1-line check: `path_str.replace("$HOME", …)` with `$HOMEBREW` produces
  `…aliceBREW…`. The path then fails canonicalization and surfaces as `File not found`, which
  is the same observable as a real missing file — no error names the expansion.

### BT-A-R4-05 — `mod.rs` exposes `notify_tool_start` / `notify_tool_result` / `notify_tool_streaming_chunk` as no-op stubs with ~85 live callers — execution progress events go nowhere
- **File**: `src/builtin_tools/mod.rs:285-300`; call sites in `tool_usage.rs:106,163`,
  `web_fetch/mod.rs:114-272`, `sessions/{send,list}_tool.rs`, `ctx_search.rs`, `recall_events.rs`,
  `a2a_tools.rs`, `remember.rs`, `acp_tools.rs`, `session_search.rs`,
  `generation/{image,video,audio}_generate.rs`, `vault_store.rs`, `select_model.rs`,
  `moa_manage.rs` and ~10 others.
- **Severity**: Low
- **Category**: perf / observability
- **Description**: The file-level doc comment acknowledges these are kept as no-op stubs "so the
  ~85 callers … stay compile-clean". Tool progress no longer flows through these hooks — the
  doc comment says progress is supposed to flow through the gateway event bus via
  `GatewayEventEmitter`. But the doc comment also says: "rip them out when the call sites are
  updated to publish via the event bus directly." That update never happened; 85+ call sites
  still exist, each one a slot that contributes nothing but a wasted function call and the
  symbol's continued presence in the public surface. There is no `tracing::trace!` even though
  some sites would benefit from one.
- **Evidence**:
  ```rust
  // mod.rs:285-300 (excerpted)
  /// Notify that a tool has started execution (legacy no-op).
  pub fn notify_tool_start(_tool_name: &str, _args_summary: &str) {}
  /// Notify that a tool has completed execution (legacy no-op).
  pub fn notify_tool_result(_tool_name: &str, _result_summary: &str, _success: bool) {}
  ```
  The doc comment is clear about the intended cleanup, but the cleanup has not happened.
- **Suggested fix**: Either (a) replace the stubs with real `tracing::info!(tool = …, result = …,
  success = …)` calls so the callsites produce something (even if not the eventual event-bus
  publish), or (b) delete the stubs and migrate the 85+ call sites to a single
  `gateway_event::emit_tool_progress(...)` call. The current state — function bodies exist, are
  called, and do nothing — is the worst of both worlds.
- **Verification**: `grep -c "notify_tool_" src/builtin_tools/**/*.rs` — 85+ hits. The
  function definitions are 3 in total; the call sites never produce a log line. The README and
  AGENTS.md have no entry for a migration timeline.

### BT-A-R4-06 — `FileWriteTool::execute_write` short-circuits on byte-equality with a TOCTOU window between `metadata()` and `read()`
- **File**: `src/builtin_tools/file_ops/ops.rs:243-269` (`is_byte_equal_existing`) called by `execute_write` at `ops.rs:174-181`
- **Severity**: Low
- **Category**: correctness
- **Description**: `is_byte_equal_existing` does a two-syscall check: `fs::metadata(canonical)`
  to compare length, then `fs::read(canonical)` to compare bytes. Both calls happen under the
  `path_locks::lock_path` guard (so no other *Aleph writer* races with us), but they happen
  *separately*. Between `metadata` returning `len == wanted.len()` and the `read()` the file
  could be written by any external process (the user's editor, a git hook, another agent on a
  shared volume). A small change between the two reads yields a false `unchanged: true` return:
  the file is now `len == wanted.len()` bytes but the bytes have been replaced with something
  else that happens to match. A real `read → write` cycle would be the correct outcome, not a
  no-op. The same race applies to the `clone()` lock (per-process), so this is an OS-level gap
  the per-process lock can't close.
- **Evidence**:
  ```rust
  // ops.rs:243-269 (excerpted)
  if is_byte_equal_existing(&canonical, content.as_bytes()).await) {
      info!(path = %canonical.display(), bytes, "Wrote file (no-op, mtime preserved)");
      return Ok(WriteOutcome { …, unchanged: true });
  }
  // …
  async fn is_byte_equal_existing(canonical: &Path, wanted: &[u8]) -> bool {
      let meta = match fs::metadata(canonical) { … };
      if meta.len() as usize != wanted.len() { return false; }
      match fs::read(canonical) { Ok(existing) => existing == wanted, … }
  }
  ```
- **Suggested fix**: Open the file once with `fs::File::open(canonical)`, then read the exact
  `len` bytes and compare in one shot — or, better, take a SHA-256 of the existing file (via a
  one-shot `read_to_end` hashed in memory) and compare against the wanted content's precomputed
  hash. The current two-syscall check is a measurable race that matters in collaborative
  workspaces.
- **Verification**: The function is private to `file_ops/ops.rs`; the only caller is
  `execute_write`. The race is documented but not asserted — no test mutates the file
  externally between `metadata` and `read`.

### BT-A-R4-07 — `code_exec` and `bash_exec` inject `ALEPH_SESSION_ID` and `ALEPH_TOOL_NAME` via `serde_json::to_string(&session_id)` which can panic for non-`SessionKey` session ids
- **File**: `src/builtin_tools/code_exec.rs:351-358`; `src/builtin_tools/bash_exec.rs` (mirrors same pattern)
- **Severity**: Low
- **Category**: error-handling
- **Description**: Both code-execution paths inject `ALEPH_SESSION_ID` via
  `serde_json::to_string(&session_id).unwrap_or_else(|_| format!("{session_id:?}"))`. The
  `unwrap_or_else` is correctly fallible, but the `Debug` fallback path will produce
  `SessionKey("…", …)`-style strings (the type's `Debug` impl) that are not stable, not
  parseable, and look different from the documented `serde_json::to_string` form. A future
  consumer that does `python3 -c "import json,os; print(json.loads(os.environ['ALEPH_SESSION_ID']))"`
  will fail with `JSONDecodeError` for any session whose `Serialize` impl ever changes, and the
  fallback message gives no clue what went wrong.
- **Evidence**:
  ```rust
  // code_exec.rs:351-358 (excerpted)
  env.insert(
      "ALEPH_SESSION_ID".to_string(),
      serde_json::to_string(&session_id).unwrap_or_else(|_| format!("{session_id:?}")),
  );
  ```
- **Suggested fix**: Same `to_string()` + fall-back, but log the fallback as a `tracing::error!`
  so the operator sees when the canonical form is unavailable. Optionally, also write the env
  var under a second name (`ALEPH_SESSION_ID_DEBUG`) so a child script that needs the
  type-only value can find it. Today both forms collapse into one slot.
- **Verification**: `grep -n "ALEPH_SESSION_ID" src/builtin_tools/` returns the two injection
  sites. The `Debug`-fallback branch is never tested. `session_key::SessionKey::Serialize` is
  stable today, so the fallback is dead code in practice, but the dead branch hides bugs in
  any future `SessionKey` shape change.

### BT-A-R4-08 — `hub_install_run::proceed` stores secrets keyed by `(entry.kind, entry.id, field_name)` without clearing on uninstall — secrets persist after the extension is removed
- **File**: `src/builtin_tools/hub/install_run.rs:240-249`
- **Severity**: Medium
- **Category**: resource / security
- **Description**: When `hub_install_run` stores a `sensitive` config field in the vault, the key
  is derived from `(entry.kind, entry.id, name)` via `field_key` (see `hub/secrets::field_key`).
  The vault has no companion path that purges these keys when the extension is uninstalled
  (or when an entry id is overwritten in the catalog with a different schema). A user who
  installs an MCP server, supplies an API key, and later uninstalls it has the API key sit
  in the encrypted vault forever — encrypted at rest, but never rotated, never reachable by
  the user to inspect, and never reclaimed when the binding `(kind, id, name)` is reused for
  a different extension whose operator expects a fresh slate.
- **Evidence**:
  ```rust
  // install_run.rs:240-249
  if secret_names.contains(name.as_str()) {
      let key = field_key(entry.kind, &entry.id, name);
      self.vault.store_secret(&key, &val).map_err(...)?;
      secret_refs.insert(name.clone(), key);
  }
  ```
- **Suggested fix**: Add a `hub_uninstall` (or equivalent) path that purges vault rows keyed by
  the uninstalled `(kind, id, *)` prefix. The encrypted-at-rest claim is real but the user-
  facing risk is "secret the user thought they deleted by uninstalling the extension never
  goes away". Same fix should be wired through any `mcp_remove` / `plugin_remove` /
  `skill_remove` admin tool.
- **Verification**: `grep -rn "field_key\|store_secret" src/hub/` — store sites exist (in
  `install_run`), but no `forget_secret` / `delete_secret` call site follows an uninstall. The
  vault itself has a delete path; the wiring to it is what's missing.

### BT-A-R4-09 — `bash_exec::spawn_background` race: `tokio::spawn` is created BEFORE `register_running` returns, so a fast-failing `id_tx.send` is awaited but the abort path can leak `live_for_task`
- **File**: `src/builtin_tools/bash_exec.rs:432-470`
- **Severity**: Low
- **Category**: concurrency / resource
- **Description**: The spawn-and-register pattern has `id_tx` / `id_rx` as a gate: the detached
  task parks on `id_rx.await` until the foreground sends the assigned id. On the
  `RegisterOutcome::TooManyRunning` arm, the foreground drops `id_tx` and calls `join.abort()`
  — both correctly cancel the task. However, the `live_for_task` `Arc<LiveTail>` constructed at
  line 462 is dropped only when the detached task ends. The `LiveTail::new()` allocates an
  internal ring buffer (likely per-driver). Under a thundering herd of background-job
  submissions that all hit the per-session cap (8 running), each request allocates a
  `LiveTail`, sends the task into `id_rx.await`, the task then runs the cancellation path —
  the ring buffer is dropped when the task ends, which is async. The comment block claims
  "Dropping id_tx makes the gated task exit on its id_rx.await error without ever touching
  the sandbox; abort it too so the detached task is reaped promptly rather than lingering
  until the channel drop is observed." But the `LiveTail` is held by the task and lives until
  the task ends — which depends on Tokio's cancellation timing. A flurry of refused
  background submissions (8+ in flight) holds up to N `LiveTail` instances pending cancellation.
- **Evidence**:
  ```rust
  // bash_exec.rs:432-470 (excerpted)
  let live = Arc::new(LiveTail::new());
  let live_for_task = live.clone();
  let (id_tx, id_rx) = tokio::sync::oneshot::channel::<u64>();
  let join = tokio::spawn(async move { /* id_rx.await → execute → reg.complete */ });
  match registry.register_running(preview, caller, join.abort_handle()) {
      RegisterOutcome::Registered(id) => { registry.attach_live(id, live); let _ = id_tx.send(id); … }
      RegisterOutcome::TooManyRunning { limit } => {
          drop(id_tx); join.abort(); /* … error_output(…) */
      }
  }
  ```
- **Suggested fix**: Construct `LiveTail` inside the gated task body (after `id_rx.await`) so
  the no-go path never allocates it. Today it's allocated at the call site and only the
  detached task can drop it.
- **Verification**: The flow is single-file, single-function, fully commented. The
  `RegisterOutcome::TooManyRunning` path is tested but the test only asserts the user-facing
  error message; it does not assert the absence of an in-flight `LiveTail`.

### BT-A-R4-10 — `config_audit` covers SSRF / sandbox / privacy but never inspects gateway authentication, ACLs, or `deny_read_globs` integrity — leaves the operator thinking the audit is comprehensive
- **File**: `src/builtin_tools/config_audit.rs:91-103`
- **Severity**: Low
- **Category**: security / correctness
- **Description**: `audit_config` runs `audit_ssrf`, `audit_sandbox`, `audit_privacy` and
  nothing else. The file's doc comment explicitly notes: "gateway authentication is NOT on
  the top-level `Config` … To keep this tool surgical and avoid invasive plumbing, the audit
  covers only what is reachable from `Config`: SSRF, sandbox, shell security, and
  privacy/PII filtering." That scope is reasonable, but `config_audit` is presented to the
  operator as "is my setup secure?" (per its `DESCRIPTION` const). The model's natural
  reading is "audit passed ⇒ no security issues" — and the missing coverage includes
  high-impact areas: gateway auth (token validation, bearer parsing), approval policy
  strictness, `[sandbox] deny_read_globs` parse correctness, and the credential vault's
  encryption posture. The tool surfaces a green `No posture issues found` for any of those
  misconfigurations.
- **Evidence**:
  ```rust
  // config_audit.rs:91-103
  #[must_use]
  pub fn audit_config(cfg: &Config) -> Vec<Finding> {
      let mut out = Vec::new();
      audit_ssrf(cfg, &mut out);
      audit_sandbox(cfg, &mut out);
      audit_privacy(cfg, &mut out);
      out
  }
  ```
- **Suggested fix**: Either widen the audit to cover the missing areas, or add an
  `info`-severity finding under each covered area that says "this audit intentionally
  excludes X, Y, Z — run `<other-tool>` for full coverage" so the green verdict is honest about
  what it checked.
- **Verification**: `grep -n "audit_" src/builtin_tools/config_audit.rs` shows three audit
  functions; the coverage map is small. The DESCRIPTION is broader than the implementation.

### BT-A-R4-11 — `search.rs` includes the Tavily API key in the JSON request body sent over HTTPS — fine for Tavily's protocol, but the body shape is logged at `debug!` level so the key is reachable through any log capture
- **File**: `src/builtin_tools/search.rs:160-185`
- **Severity**: Low
- **Category**: security
- **Description**: The legacy Tavily path builds the request body with the API key inline:
  `serde_json::json!({"api_key": api_key, "query": args.query, …})`. `debug!("Sending request
  to Tavily API")` is emitted before the call, but `reqwest`'s default logging at INFO level
  will not include the body. However, the configured `tracing` layer at DEBUG would log the
  full `request_body` if a future call adds `tracing::debug!(body = %request_body, …)`. The
  risk is small (Tavily's protocol expects this), but the registry path (multi-provider) puts
  the key in the `Authorization` header which is the standard pattern. The legacy fallback
  picks the less-safe shape silently; if a future debug log captures the body, the legacy key
  leaks with no rotation hint.
- **Evidence**:
  ```rust
  // search.rs:160-185 (excerpted)
  let request_body = serde_json::json!({
      "api_key": api_key,
      "query": args.query,
      "max_results": limit,
      "include_answer": false
  });
  debug!("Sending request to Tavily API");
  ```
- **Suggested fix**: Wrap the debug log so the body is printed with `api_key` redacted (or use
  `tracing::debug!(body = ?redact(request_body), …)`). The same redaction helper should be
  used for the registry path so a future audit has one canonical redaction.
- **Verification**: The legacy path is only reached when no `SearchRegistry` is wired — the
  primary path puts the key in `Authorization`. The legacy path is documented as fallback,
  not deprecated, so it ships.

### BT-A-R4-12 — `permission_tool::check` and `request` actions accept `permission` as a free string and silently drop unknown values — operator cannot tell why an action refused
- **File**: `src/builtin_tools/permission_tool.rs:75-86`
- **Severity**: Low
- **Category**: api-design / error-handling
- **Description**: `parse_permission` does `serde_json::from_value(Value::String(s))` and
  returns `None` on parse failure. The dispatch in `call()` then matches `Some(p) => p, None
  => friendly error`. The error message correctly enumerates the supported values
  ("Values: screen_recording, camera, microphone, speech_recognition, accessibility,
  notifications" for `check`/`request`). For `guide`/`open_settings`, the error message
  extends the list to include the manual-grant kinds. But the underlying parse doesn't
  distinguish "string the user sent is a known variant" from "string is unknown". An
  operator who sends `{"action":"check","permission":"screenRecording"}` (camelCase) gets the
  same "Values: ..." error as someone sending `{"action":"check","permission":"lol"}`. The
  hint could say which kind was unparseable for the action.
- **Evidence**:
  ```rust
  // permission_tool.rs:75-86
  fn parse_permission(s: &str) -> Option<aleph_desktop::permission_types::PermissionKind> {
      serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
  }
  ```
  The error messages at lines 117-127, 165-174, 202-211, 230-239 list the values but never
  echo back the caller's input.
- **Suggested fix**: Echo the caller's input in the error: `"check requires a valid 'permission' parameter; got '{input}', expected one of: …"`. Helps the operator notice typos immediately.
- **Verification**: Grep for `parse_permission` returns the single definition. The four error
  paths (one per gated action) are separate copy-pastes; refactoring them into one helper
  would also let the fix land in one place.

### BT-A-R4-13 — `command_canonicalize` only accepts `bash`/`sh`/`zsh`/`dash`/`ash` as outer wrappers, but the noop-flag list treats `-l`/`--login`/`-i`/`-e`/`-u`/`-x`/`+e` as passthrough — any other flag (`-O`, `-C`, `--noprofile`, `--rcfile`) silently falls through to `passthrough` without a warning
- **File**: `src/builtin_tools/command_canonicalize.rs:39-44, 81-94`
- **Severity**: Low
- **Category**: security / correctness
- **Description**: The unwrap decision tree walks flag tokens, accepts `-c`/`-lc`/`-cl`/`-ec`/
  `-ce` as the script-bearing flag, accepts the listed `NOOP_FLAGS` as consumable, and
  returns `passthrough` (which preserves the wrapper verbatim) on any unknown flag. That is
  safe — a wrapper that doesn't fit is left for bash to parse. The cost is invisible to the
  operator: an LLM emitting `bash --noprofile --rcfile /tmp/inject.sh -c 'cargo test'` gets
  passthrough (the wrapper survives), but `bash` is then invoked with `--rcfile /tmp/inject.sh`
  — bash will source the rcfile before running the script. This is *not* an injection risk
  today (the operator controls the LLM's input), but the `passthrough` choice means a
  permission-gating prompt shown to the human would have the literal wrapper, while the
  effective invocation is `bash --rcfile … -c 'cargo test'`. The rcfile side-effect is silent.
- **Evidence**:
  ```rust
  // command_canonicalize.rs:81-94 (excerpted)
  if let Some(remainder) = match_command_flag(flag, after) {
      return match try_extract_safe_script(remainder) { … };
  }
  if NOOP_FLAGS.contains(&flag) {
      cursor = after;
      continue;
  }
  // Unrecognized token — bail to passthrough.
  return passthrough(cmd);
  ```
- **Suggested fix**: Either (a) refuse to unwrap when a flag is not in the known set (force
  passthrough without the unwrap — which is what happens today — but add a `tracing::warn!`
  on the unknown-flag path so the operator sees it), or (b) extend the `NOOP_FLAGS` list with
  a doc-comment block enumerating the chosen set and the rejected set. The function is small
  enough that the fix is local.
- **Verification**: Grep for `NOOP_FLAGS` returns the const definition; tests cover the
  passthrough on `bash --version` but no test asserts the missing-flag warning.

### BT-A-R4-14 — `meta_tools::levenshtein_distance` allocates `matrix[a_len+1][b_len+1]` per call — every fuzzy "did you mean" suggestion pays O(N×M) allocation on every dispatch
- **File**: `src/builtin_tools/meta_tools.rs:38-90`
- **Severity**: Low
- **Category**: perf
- **Description**: The classic 2-row Levenshtein DP allocates a full `(a_len+1) × (b_len+1)`
  matrix per call. The function is the single source of truth for "did you mean" suggestions
  on tool-name repair and slash-command repair. A busy agent turn that issues 3 tool calls
  each triggering a tool-name repair pays 3 allocations of up to ~500 × 500 cells (the
  upper-bound guard at line 38 caps at 500 chars, so worst case ~250 KB per call). The
  function is `pub(crate)` and called per-repair; for tight tool-name repair loops it adds up.
  A standard 2-row rolling DP would use O(min(a,b)) cells and one allocation per call.
- **Evidence**:
  ```rust
  // meta_tools.rs:38-90 (excerpted)
  let mut matrix = vec![vec![0usize; b_len + 1]; a_len + 1];
  ```
- **Suggested fix**: Use 2 rows + a `swap` (the standard space optimization), or `tinyvec` /
  stack allocation when `(a_len, b_len)` is below a threshold. The function is small, the fix
  is local, the algorithm is well-documented.
- **Verification**: `grep -n "matrix" src/builtin_tools/meta_tools.rs` returns the single
  full-matrix allocation. Tests cover small inputs (5 chars × 5 chars); no benchmark covers
  the 500 × 500 worst case.

### BT-A-R4-15 — `apply_patch` body parser accumulates all ops and lines in memory before any write — the 4 MiB envelope cap is checked at byte-length but a billion-byte-length encoded pattern with low entropy could blow the cap's protection
- **File**: `src/builtin_tools/file_ops/apply_patch.rs:88-115`
- **Severity**: Low
- **Category**: resource
- **Description**: `apply_patch` parses the entire envelope into a `Vec<PatchOp>` plus per-op
  `Vec<Line>` before any write, bounded by `MAX_PATCH_BYTES` (4 MiB) and `MAX_PATCH_OPS`
  (500). Both caps are checked on raw byte-length, but a 4 MiB envelope of single-byte ops
  like `*** Add File: x\n+x\n` would parse to ~2M tiny `PatchOp`s, which the cap refuses
  (op count > 500). A 4 MiB envelope of `*** Update File: x\n`-prefixed blocks with 1MB of
  context lines each would parse to 4 ops but 1 MB of per-op context — the `MAX_PATCH_BYTES`
  check passes and the per-op `Vec<Line>` exceeds reasonable working set limits. The
  planner then holds ~4 MiB across planning + execution. Today's op cap catches the worst
  case (a million add-ops), but a long-context envelope slips through.
- **Evidence**:
  ```rust
  // apply_patch.rs:88-115 (excerpted)
  if args.patch.len() > MAX_PATCH_BYTES { … }
  let op_headers = …;
  if op_headers > MAX_PATCH_OPS { … }
  let ops = parse_patch(&args.patch).map_err(ToolError::InvalidArgs)?;
  ```
- **Suggested fix**: Also cap the per-op line count (e.g. `MAX_LINES_PER_OP = 1000`) and the
  per-line byte length (e.g. `MAX_LINE_BYTES = 64 KiB`). Both are cheap to enforce in the
  parser and prevent a single envelope from monopolising the planner.
- **Verification**: The two caps are tested. The per-op line count is not. A search for
  `parse_patch` returns the function definition; the inner loop has no per-op check.

## Cross-cutting concerns

1. **`path_utils::check_and_resolve_path` is a single chokepoint for the whole file layer.** The
   path traversal, symlink, denylist, FsScope-rebase, `/proc` block, and pattern-guards all
   route through one function. The unit-test `the_two_path_resolvers_stay_split` pins this by
   name. Any reviewer touching `expand_input_path` (BT-A-R4-04) needs to remember this
   chokepoint role — a fix there improves the whole layer.

2. **Background-job resource lifecycle is split across `bash_exec::spawn_background` and
   `process_registry`.** BT-A-R4-09 (LiveTail pending in cancelled tasks) is a small leak
   today but a future change to `process_registry` could turn it into a bigger one. The two
   should be reviewed together when either is touched.

3. **`notify_tool_*` no-op stubs (BT-A-R4-05) are the largest dead-code surface in the
   codebase.** The doc comment admits the situation and asks for cleanup. ~85 call sites
   means a one-PR cleanup is non-trivial; flagging it so it isn't forgotten.

4. **The approval-gate coverage is asymmetric across tools.** `system_tool` (BT-A-R4-03),
   `permission_tool` (BT-A-R4-12), `desktop_tool` (out of scope here), and the `hub_install_run`
   trust gate all gate different subsets of dangerous actions. A single chokepoint audit —
   "every tool action that can read or write user state must route through an
   `ApprovalGate::check`" — would catch the systemic gaps (clipboard read is the most
   obvious one).

5. **`command_ledger` enforces LRU + TTL; `process_registry` enforces LRU + TTL;
   `ReadCache` enforces neither (BT-A-R4-01).** The pattern is established; the outlier is
   the file read cache. Either add the cap or document why `ReadCache` is intentionally
   unbounded.

6. **`hub_install_run` secrets never expire (BT-A-R4-08).** Same lifecycle discipline as the
   `command_ledger` / `process_registry` TTL would cover this — uninstall should purge
   vault rows keyed by `(kind, id, *)`.

7. **`expand_input_path`'s string-level `.replace()` (BT-A-R4-04) is a recurring
   anti-pattern.** The same shape appears in `expand_denied_entry` (windows env-token
   expansion) — different surface, same kind of bug. Future readers should look for `result
   = result.replace(…)` on user-supplied path strings.

## Summary
- **Total: 15 findings** (0 Critical, 0 High, 4 Medium, 11 Low)
- **Top priority items (must-fix)**:
  1. **BT-A-R4-01** — `FileReadTool`'s `ReadCache` is unbounded; long sessions slowly leak
     memory across every distinct `(path, offset, limit)` triple. The rest of the file_ops
     layer enforces size caps; this is the outlier.
  2. **BT-A-R4-02** — `hub_install_run::proceed` silently drops JSON arrays / objects / nulls
     from `config_values`. Misconfiguration surfaces far from the install call as a runtime
     failure inside the new MCP server / skill.
  3. **BT-A-R4-03** — `system_tool::clipboard_read` is not gated by the approval policy that
     gates `clipboard_write`. Asymmetric read risk on the user's clipboard.
  4. **BT-A-R4-04** — `expand_input_path` does string-level `.replace("$HOME", …)` that mangles
     paths containing `$HOME`-like substrings (`$HOMEBREW` etc).

## What was NOT covered
- **Loom concurrency tests** — not run; the conclusions on locking are static-analysis only.
- **Generation-provider implementations** — only the tool adapters in `generation/*` were
  read; the underlying provider implementations in `src/generation/` were not.
- **Hub sub-modules** — `src/hub/` (cache, install, trust, verify, reconcile, secrets,
  origin, catalog_client) were read at the boundary touched by `hub/install_run.rs` only;
  the deeper `verify::verify_install` logic and `trust::scan_for_injection` are not
  independently audited here.
- **`executor/builtin_registry/`** — the registry wiring every tool is dispatched through
  is out of scope (mentioned in `r3/a2a/REPORT.md` as a known cross-module contract gap).
- **`sandbox::Sandbox::execute`** — the underlying sandbox trait that `bash_exec`,
  `code_exec`, `code_check` all route through is not audited here.
- **Benchmark data** — no `cargo bench` was run. The `ReadCache` leak estimate (BT-A-R4-01)
  is back-of-envelope, not measured.
- **Cross-module wire-protocol conformance** — the tool-level JSON shapes are read but not
  diffed against a separate schema registry.
- **`src/builtin_tools/skill_manage.rs`, `goal.rs`, `loop_manage.rs`** and other
  sibling-tool files — outside this chunk's scope (covered by other chunks per the
  AGENTS.md worktree layout).
- **`hub/install_run.rs::proceed` vault key derivation** — the `field_key` function in
  `hub/secrets` is referenced but its full implementation was not read.