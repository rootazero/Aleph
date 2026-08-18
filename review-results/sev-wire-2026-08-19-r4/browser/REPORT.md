# Code review — `src/browser/` (2026-08-19 round r4)

## Scope
- Files reviewed: `mod.rs`, `error.rs`, `types.rs`, `backend.rs`, `manager.rs`, `chrome_mcp.rs`, `chrome_mcp_backend.rs`, `playwright_cli.rs`, `playwright_cli_backend.rs`, `playwright_launch.rs`, `tab_registry.rs`, `network_policy.rs`, `secret_guard.rs`, `post_nav.rs`, `wait_probe.rs`, `discovery.rs`, `profile.rs`, `testkit.rs`
- LoC total: 8,459
- Cross-checked callers: `builtin_tools/browser_tools/*` (`select`, `hover`, `upload`, `profile_tool`, `exec`, `type_text`, `navigate`, `console`, `cookies`, `evaluate`, `fill_form`, `dialog`, `network`, `emulate`, `tabs`, `screenshot`, `mod.rs`), `executor/builtin_registry/{config,builder/constructor/mod,definitions}.rs`, `bin/aleph-server/commands/start/builder/agent_init/mod.rs`, `builtin_tools/pdf_generate/browser_engine.rs`, `tools/probes/browser.rs`, `diagnostics/checks/browser_runtime.rs`
- Method: read-first sweep, focus on P0-P2 per `skills/code-review/SKILL.md` (correctness, error handling, resource/cleanup, security, concurrency). Did not re-audit adjacent rounds' scope. Cross-referenced the `chrome-devtools-mcp`/`playwright-cli` wire-shape notes in `chrome_mcp_backend.rs`/`playwright_cli.rs` to anchor findings.

## Findings

### BROWSER-R4-01 — `ChromeMcpDriver::ensure_chrome_running` leaks the launched Chrome process; no Drop shutdown hook
- **File**: src/browser/chrome_mcp.rs:299-340
- **Severity**: Medium
- **Category**: resource
- **Description**: When Aleph is the one launching Chrome (no user Chrome running), the function spawns a Chrome process with `--remote-debugging-port=0` and drops the `Child` handle. `tokio::process::Child::kill_on_drop` defaults to `false`, so the launched Chrome is reparented to PID 1 and outlives the daemon. The code comment admits this: *"A future shutdown hook (e.g. `Drop` on `ChromeMcpDriver`) can reap it."* — but no `Drop` impl exists. Under error-recovery paths (a session is destroyed, then `ensure_session` re-runs and finds no Chrome, so launches a fresh one), Aleph-launched Chrome processes accumulate. They hold `--remote-debugging-port` sockets, files in the temp dirs, etc.
- **Evidence**:
  ```rust
  // chrome_mcp.rs:322-339
  let mut child = cmd
      .stdin(Stdio::null())
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .no_window()
      .spawn()
      .map_err(|e| BrowserError::LaunchFailed(format!("Failed to launch Chrome: {e}")))?;
  // … blind-sleep + try_wait …
  // Drop the handle intentionally: `tokio::process::Child::kill_on_drop`
  // defaults to `false`, so this does NOT terminate the launched Chrome
  // process — it remains alive to serve the MCP server's `--autoConnect`.
  // A future shutdown hook (e.g. `Drop` on `ChromeMcpDriver`) can reap it.
  drop(child);
  Ok(())
  ```
- **Suggested fix**: Track launched PIDs in a `Vec<Child>` field on `ChromeMcpDriver`, with an `is_alive()` check before each spawn and a `Drop` impl that walks the list and calls `child.kill().await`. Cheaper: hand the Chrome `Child` to a background "chrome-sentinel" task that lives for the lifetime of the manager.
- **Verification**: Searched chrome_mcp.rs for `impl Drop` and `kill_on_drop` — only matches were the comments. No resource recovery code exists.

### BROWSER-R4-02 — `chrome_mcp_backend::wait_for` text arm releases the per-profile lock before the MCP call, creating an intra-window race with concurrent `select_page`
- **File**: src/browser/chrome_mcp_backend.rs:267-292
- **Severity**: Medium
- **Category**: concurrency
- **Description**: For `WaitCondition::Text`, the per-profile lock is held only across `select_page`. The lock is released *before* the `wait_for` MCP call is issued. The accompanying doc-comment acknowledges this ("A concurrent op that re-selects inside the one round-trip window is the residual cost"). The race window is the time between `select_page` returning and the MCP server beginning to evaluate the `wait_for` request — but the MCP `wait_for` tool is server-side, so its target page is whatever the server has selected at request-arrival *and* whenever the server later consults it. If a concurrent op re-selects, the wait may observe the wrong page. Documented tradeoff, not a hidden bug, but it is a real race surface.
- **Evidence**:
  ```rust
  // chrome_mcp_backend.rs:283-292
  {
      let _guard = self.profile_guard().await;
      self.select_page(tab_id).await?;
  }
  let outcome = self.call("wait_for", wait_for_args(text, timeout_ms)).await;
  match outcome {
      Ok(_) => Ok(true),
      Err(e) => classify_wait_error(e, tab_id),
  }
  ```
- **Suggested fix**: Either accept the tradeoff (already documented) or add a `wait_for_with_page_pin` MCP protocol call shape that pins by `pageId`. The cleanest defence at the driver layer is to issue `select_page` *immediately before* dispatch and rely on the MCP server's request ordering. Without a contract guarantee from the server, this is best flagged for visibility.
- **Verification**: Read the design comment in the function body and surrounding doc. No concurrent-call test exercises this race.

### BROWSER-R4-03 — `PlaywrightCliDriver::classify_failure` matches the substring `"timeout"` to identify timeouts, producing false positives on any error text containing the word
- **File**: src/browser/playwright_cli.rs:331-362
- **Severity**: Medium
- **Category**: correctness
- **Description**: The classifier lowercases the combined stdout+stderr and checks `s.contains("timeout")`. Any failure message containing the substring "timeout" for a non-timeout reason (e.g. an MCP error like "the timeout parameter was rejected", a debug log like "previous request hit a timeout", or a stale HTTP cache "HTTP 504 timeout from upstream") is misclassified as `BrowserError::Timeout`. The downstream caller treats `Timeout` as a retryable time-budget error; a misclassification causes the tool layer to retry against a backend in a broken or rejected state.
- **Evidence**:
  ```rust
  // playwright_cli.rs:344-355
  let s = format!("{stdout}\n{detail}").to_lowercase();
  if s.contains("please run open first")
      || s.contains("is not open")
      || s.contains("no session")
      || s.contains("browser not open")
  {
      BrowserError::NoSession(session_key.to_string())
  } else if s.contains("timeout") {
      BrowserError::Timeout(timeout_ms)
  } else if s.contains("element not found")
      || s.contains("no element")
      || s.contains("does not match any elements")
  {
      BrowserError::ActionFailed(format!("element not found ({})", detail.trim()))
  ```
- **Suggested fix**: Anchor the `timeout` pattern (e.g. `\baction timeout\b`, `\bnavigation timeout\b`, `\btimeout\s*\d+\s*m?s\b`) or, better, surface a typed signal from the CLI driver via the existing structured-error contract (the CLI exits 0 with `### Error` text — adding a stable `error-kind: timeout` marker would resolve this). Same issue applies to `chrome_mcp_backend::classify_wait_error` (see BROWSER-R4-04).
- **Verification**: Tests cover the *common* failure shapes but no test for the substring-overlap failure. The classifier is invoked on every CLI failure path in production.

### BROWSER-R4-04 — `ChromeMcpBackend::classify_wait_error` uses four substrings to decide whether an MCP error means "wait timed out"
- **File**: src/browser/chrome_mcp_backend.rs:573-586
- **Severity**: Medium
- **Category**: correctness
- **Description**: The function checks for `"timeout"`, `"timed out"`, `"did not appear"`, `"exceeded"` (lowercased) to fold a `ChromeMcpError` into `Ok(false)`. Any non-timeout error containing one of these words is reported as "text did not appear" — a confident lie. Conversely, a real timeout message using slightly different wording (e.g. "the wait window elapsed", "deadline reached") becomes a generic `Err` to the caller, leaking transport noise as a tool error. Heuristic text matching on the failure path is exactly the wrong tool for the distinction the doc tries to draw.
- **Evidence**:
  ```rust
  // chrome_mcp_backend.rs:573-581
  BrowserError::ChromeMcpError(ref msg) => {
      let lower = msg.to_lowercase();
      if lower.contains("timeout")
          || lower.contains("timed out")
          || lower.contains("did not appear")
          || lower.contains("exceeded")
      {
          Ok(false)
      } else {
          Err(err)
      }
  }
  ```
- **Suggested fix**: Use the structured MCP error code if available. If only text is available, anchor the patterns (e.g. `\bwait[_ ]timeout\b`) and accept that any unrecognised error propagates as `Err` rather than being mis-folded.
- **Verification**: Test `wait_error_folds_only_the_tools_own_timeout` only covers the happy path; `wait_error_never_folds_a_transport_failure` covers a different vector. No test for substring false positives on non-timeout errors.

### BROWSER-R4-05 — `chrome_launch_args` does not set a fallback `--user-data-dir` when the profile leaves it unset, so Aleph's bootstrapped Chrome shares the user's everyday profile
- **File**: src/browser/chrome_mcp.rs:24-37
- **Severity**: Medium
- **Category**: security
- **Description**: When Aleph is the one launching Chrome (no user Chrome running), `chrome_launch_args` only appends `--user-data-dir=<path>` when `cfg.user_data_dir` is `Some`. Otherwise Chrome falls back to its platform default (`~/.config/google-chrome` on Linux, `%LOCALAPPDATA%\Google\Chrome\User Data` on Windows), which is the user's daily Chrome profile. The auto-injected `user` profile (ExistingSession driver) does NOT configure `user_data_dir`, so if Aleph bootstraps Chrome for it, the launched Chrome silently reads/writes the user's persistent cookies, history, logins, and saved passwords — and is invisible in the user's taskbar/dock.
- **Evidence**:
  ```rust
  // chrome_mcp.rs:24-37
  fn chrome_launch_args(profile_cfg: Option<&ProfileConfig>) -> Vec<String> {
      let mut args = vec![
          "--remote-debugging-port=0".to_string(),
          "--no-first-run".to_string(),
          "--no-default-browser-check".to_string(),
      ];
      if let Some(cfg) = profile_cfg {
          if let Some(proxy) = &cfg.proxy {
              args.push(format!("--proxy-server={proxy}"));
          }
          if let Some(dir) = &cfg.user_data_dir {
              args.push(format!("--user-data-dir={dir}"));
          }
          args.extend(cfg.extra_args.iter().cloned());
      }
      args
  }
  ```
- **Suggested fix**: When `user_data_dir` is `None`, default to a per-profile Aleph-private path under the Aleph data dir, e.g. `${ALEPH_DATA}/browser/chrome-mcp/${profile}/user-data-dir`, and create it before spawn. The `ExistingSession` semantics ("attach to user's Chrome") then cleanly fail when no Chrome is running rather than launching a clone that reads the user's profile.
- **Verification**: The `test_chrome_mcp_driver_new` and `chrome_launch_args_*` tests only verify argv contents; no test exercises the bootstrap-launch path with `user_data_dir == None`. The `is_chrome_running` check that triggers the bootstrap is in `ensure_chrome_running` at line 313.

### BROWSER-R4-06 — `ChromeMcpDriver` per-profile lock maps grow without bound across the daemon's lifetime
- **File**: src/browser/chrome_mcp.rs:61-67, 110-117, 153-160
- **Severity**: Low
- **Category**: resource
- **Description**: `profile_locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>` and `session_create_locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>` populate on first sight of a profile name and never remove entries. For long-lived daemons serving dynamic profile names, every name seen — including ones from short-lived test sessions, customer-id-based names, or names passed in error logs — leaves an entry forever. Each entry is roughly one `Arc` (8 bytes for the pointer + heap allocation) + one `tokio::sync::Mutex` (≈96 bytes) = ~100 bytes. 100k distinct names over a daemon's lifetime ≈ 10 MB process bloat.
- **Evidence**:
  ```rust
  // chrome_mcp.rs:110-117
  pub(crate) fn profile_lock(&self, profile_name: &str) -> Arc<AsyncMutex<()>> {
      let mut map = self.profile_locks.lock().unwrap_or_else(|e| e.into_inner());
      map.entry(profile_name.to_string())
          .or_insert_with(|| Arc::new(AsyncMutex::new(())))
          .clone()
  }
  // chrome_mcp.rs:153-160 (session_create_lock — same shape)
  ```
- **Suggested fix**: Remove the entry in `destroy_session` (after the session is gone, no one needs the lock); add an LRU cap (e.g. 1024) as a safety net. The same shape applies in `PlaywrightCliDriver::per_session_locks` at playwright_cli.rs:54-65, 159-167 — flag for the same fix.
- **Verification**: Searched chrome_mcp.rs and playwright_cli.rs for `remove` / `clear` on the per-profile lock maps. No matches.

### BROWSER-R4-07 — `ProfileManager::reap_idle_tabs` constructs a fresh `BrowserBackend` per candidate per sweep — needless allocation churn and re-entry through the lazy-launch policy
- **File**: src/browser/manager.rs:381-422
- **Severity**: Low
- **Category**: perf
- **Description**: Each sweep of each candidate profile triggers `get_backend(profile)`, which constructs a new `Arc<dyn BrowserBackend>` (Playwright or Chrome-MCP backed). For a long-lived daemon with dozens of managed profiles on a 5-second sweep, that's dozens of backend constructions per second. The construction is cheap but not free — it clones the shared drivers, allocates a new Arc, and runs through `SessionLaunch::from_profile` plus JSON validation. There is no real reason: a single `BackendCache` keyed on `(profile_name, ssrf_guard_ptr)` would amortize the cost and additionally make the lazy-launch policy (`LaunchPolicy::OpenIfNeeded`) work the same way every call.
- **Evidence**:
  ```rust
  // manager.rs:393-401
  let backend = match self.get_backend(&profile) {
      Ok(b) => b,
      Err(e) => {
          tracing::warn!(profile = %profile, error = %e, "reap_idle_tabs: failed to get backend");
          continue;
      }
  };
  ```
- **Suggested fix**: Add an internal `Mutex<HashMap<String, Arc<dyn BrowserBackend>>>` (or LRU) invalidated on `apply_policy`. The lock order (`manager.profiles.read()` then `cache.lock()`) is consistent with existing patterns.
- **Verification**: `get_backend` is `O(1)` allocation; the issue is at scale.

### BROWSER-R4-08 — `LIVE_MANAGER` is a `std::sync::Mutex<Option<Weak<ProfileManager>>>` mutated from `spawn_idle_reaper`; a second `ProfileManager` invocation silently overwrites the first
- **File**: src/browser/manager.rs:30-43, 198-228
- **Severity**: Medium
- **Category**: correctness
- **Description**: `apply_policy_live` and `spawn_idle_reaper` race on `LIVE_MANAGER.lock()`. Both hold the lock only briefly (the lock pattern is safe). But: there is no protection against two `ProfileManager` instances each calling `spawn_idle_reaper`. The second instance's `Arc::downgrade` overwrites the first's static entry, and `apply_policy_live` will hot-apply to whichever manager most recently published. In production this is fine (one manager per daemon), but in tests it can cause surprising cross-test pollution — a test running after another creates a hot-apply target inside an unrelated daemon lifetime. The `idle_reaper_started` `AtomicBool` only protects re-entry on the same instance.
- **Evidence**:
  ```rust
  // manager.rs:30-31
  static LIVE_MANAGER: Mutex<Option<Weak<ProfileManager>>> = Mutex::new(None);
  // manager.rs:212-216
  if self.idle_reaper_started.swap(true, Ordering::AcqRel) {
      return;
  }
  *LIVE_MANAGER.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::downgrade(self));
  ```
- **Suggested fix**: Compare-exchange on `Weak::ptr_eq` (or just `swap_weak`) so the static holds the strongest-roots reference rather than last-write-wins. Or scope the handle to a daemon-instance id rather than a single global.
- **Verification**: `test_live_apply_reaches_a_published_manager_and_downgrades_otherwise` demonstrates the canonical path; no test exercises two coexisting managers.

### BROWSER-R4-09 — `ChromeMcpDriver::call_tool` distinguishes transport failure from tool-level error by substring-matching on `e.to_string()`
- **File**: src/browser/chrome_mcp.rs:124-150
- **Severity**: Medium
- **Category**: error-handling
- **Description**: The contract this function advertises says it separates "nothing ever looked" (`ChromeMcpTransport`) from "the tool said no" (`ChromeMcpError`). The first branch delegates to `mcp::external::is_tool_error(&err_str)`. The second branch falls through to substring matches on the lower-cased error string: `"broken pipe"`, `"connection reset"`, `"process exited"`, `"channel closed"`. If `mcp::McpClient::call_tool` ever changes its error wording (e.g. "broken-pipe" with a hyphen, "connection closed", "IO error: broken pipe"), the four-anchor classifier becomes more or less permissive, silently corrupting the `ChromeMcpTransport` vs `ChromeMcpError` split. The four anchors are also too lax — `"process exited"` matches both a legitimate subprocess crash and a routine event-loop tick.
- **Evidence**:
  ```rust
  // chrome_mcp.rs:135-145
  let is_broken_pipe = err_str.contains("broken pipe")
      || err_str.contains("connection reset")
      || err_str.contains("process exited")
      || err_str.contains("channel closed");
  if is_broken_pipe {
      tracing::warn!(
          "Chrome MCP transport error for profile '{profile_name}': {err_str}"
      );
      self.destroy_session_if_same(profile_name, &session).await;
  }
  return Err(BrowserError::ChromeMcpTransport(err_str));
  ```
- **Suggested fix**: Push the typed-error distinction into `mcp::external`: `enum McpClientError { Transport(TransportError), Tool(String), Timeout }` with structured variants. Then `is_tool_error` and `is_broken_pipe` collapse into a single match. The downstream `wait_for` classifier (BROWSER-R4-04) benefits from the same typed signal.
- **Verification**: No test pins the exact error wording; the contract is asserted in the doc only.

### BROWSER-R4-10 — `PlaywrightCliDriver::provision_binary` runs `ensure_capability` with no upper timeout; a stalled installer pins the first browser call indefinitely
- **File**: src/browser/playwright_cli.rs:106-128
- **Severity**: Medium
- **Category**: resource
- **Description**: The first browser-tool call on a fresh daemon runs `provision_binary`, which loads the `CapabilityLedger` and invokes `ensure_capability("playwright-cli", &ledger)`. The ledger provisions `playwright-cli` + Chromium + OS-specific browsers from the network. There is no `tokio::time::timeout` wrapper. The call only fails when `ensure_capability` itself returns (success or error). On a CI runner behind a captive portal, an offline host, or a slow mirror, `ensure_capability` could conceivably take minutes before returning — and a single browser-tool call will block on it. The cached `binary_path` is checked first, so the slow path runs at most once per daemon.
- **Evidence**:
  ```rust
  // playwright_cli.rs:107-128
  #[cfg(not(test))]
  async fn provision_binary(&self) -> Result<PathBuf, BrowserError> {
      let runtimes_dir = crate::runtimes::get_runtimes_dir()
          .map_err(|e| BrowserError::PlaywrightCliError(format!("runtimes dir: {e}")))?;
      let ledger_path = runtimes_dir.join("ledger.json");
      let ledger = tokio::task::spawn_blocking(move || CapabilityLedger::load_or_create(ledger_path))
          .await
          .map_err(...)?;
      let ledger = Arc::new(tokio::sync::RwLock::new(ledger));
      let resolved = ensure_capability("playwright-cli", &ledger)
          .await
          .map_err(|e| BrowserError::PlaywrightCliError(format!("ensure playwright-cli: {e}")))?;
      *self.binary_path.write().unwrap_or_else(|e| e.into_inner()) = Some(resolved.clone());
      Ok(resolved)
  }
  ```
- **Suggested fix**: `tokio::time::timeout(Duration::from_secs(300), ensure_capability(...)).await` with a clear "playwright-cli install timed out" error. Document the budget.
- **Verification**: Searched for any `timeout` wrapping `ensure_capability`. None found. The action_timeout (10s default) is unrelated — it only bounds the per-CLI-call once the binary is provisioned.

### BROWSER-R4-11 — `ChromeMcpBackend::screenshot` fall-back path treats text content as base64 PNG; an MCP error returned as text surfaces as "base64 decode: …"
- **File**: src/browser/chrome_mcp_backend.rs:368-395
- **Severity**: Low
- **Category**: error-handling
- **Description**: When the MCP response carries no `image` content type but does have text, the code calls `extract_text(&result)` and tries `base64::engine::general_purpose::STANDARD.decode(&text)`. If the text is actually an error message (e.g. `"Error: page not loaded"`, `"Permission denied for screenshot"`), the base64 decoder fails and the caller sees `"base64 decode: Invalid byte …"`, not the underlying reason. The graceful-degradation test (`test_extract_text_no_text_returns_empty_not_json`) only covers the empty-image case, not the textual-error case.
- **Evidence**:
  ```rust
  // chrome_mcp_backend.rs:382-394
  let text = Self::extract_text(&result);
  if text.is_empty() {
      return Err(BrowserError::ScreenshotFailed(
          "Chrome MCP returned no image data".into(),
      ));
  }
  let png_bytes = base64::engine::general_purpose::STANDARD
      .decode(&text)
      .map_err(|e| BrowserError::ScreenshotFailed(format!("base64 decode: {e}")))?;
  ```
- **Suggested fix**: Detect text-shaped errors ("Error:", "FAILED:", "Access denied") before base64-decoding and surface them as `BrowserError::ActionFailed` with the underlying message. Cheap; restores the diagnostic.
- **Verification**: Test `test_extract_text_no_text_returns_empty_not_json` covers the empty case only. No test exercises the text-error fallback.

### BROWSER-R4-12 — `ProfileManager::new` auto-injects both `default` and `user` profiles; there is no way to disable either via config
- **File**: src/browser/manager.rs:118-148
- **Severity**: Low
- **Category**: correctness
- **Description**: `ProfileManager::new` always inserts `default` (Managed) and `user` (ExistingSession) unless already present. Removing these from the TOML config still leaves them in the manager. An operator who wants Aleph without an ExistingSession attachment to their Chrome has no opt-out — the daemon will silently spawn a `user` profile pointing at the user's Chrome. The behaviour is documented as deliberate (a turnkey default), but the lack of opt-out makes a multi-tenant or kiosk deployment load this profile too. `test_explicit_user_profile_not_overridden` only checks that explicit config survives, not that omission removes the profile.
- **Evidence**:
  ```rust
  // manager.rs:135-148
  // Auto-inject "default" profile with Managed driver if not already present.
  if !profiles.contains_key("default") { profiles.insert("default", …); }
  // Auto-inject "user" profile with ExistingSession driver if not already present.
  if !profiles.contains_key("user") { profiles.insert("user", …); }
  ```
- **Suggested fix**: Add a config-level `disabled_profiles: Vec<String>` (or feature flag) in `BrowserSystemConfig` and skip auto-injection if the name is in the disabled list. Keep the default behaviour for the empty case.
- **Verification**: Read the entirety of `ProfileManager::new` (manager.rs:91-187). No skip-list consulted.

### BROWSER-R4-13 — `tab_idle_timeout_secs` is unbounded: an operator setting it to `u64::MAX` prevents any tab from being reaped via the idle path
- **File**: src/browser/manager.rs:381-422, profile.rs:118-128
- **Severity**: Low
- **Category**: correctness
- **Description**: `ProfileConfig::tab_idle_timeout_secs` is a `u64` with `#[serde(default = "default_tab_idle_timeout")]` (600s). The reaper does `Duration::from_secs(idle_secs)` and `select_victims` uses `now.saturating_duration_since(*last_used) >= idle_timeout`. There is no validation that `idle_secs` falls in a sane range. The LRU-overflow branch (`idx < over_cap`) still works, so an overflow is caught; the *idle* branch never fires. This means a long-idle-timeout profile combined with `max_tabs_per_profile = 0`... wait, `max_tabs.max(1)` guards against zero, so the cap is always at least 1. The bug surface is smaller than it looks — but `u64::MAX` is still legal and unvalidated.
- **Evidence**:
  ```rust
  // manager.rs:404-411
  let (max_tabs, idle_secs) = match self.get_config(&profile) {
      Some(c) => (c.max_tabs_per_profile, c.tab_idle_timeout_secs),
      None => continue,
  };
  …
  let victims = self.tab_registry.select_victims(
      &profile, &live_ids, max_tabs,
      Duration::from_secs(idle_secs),
  );
  ```
- **Suggested fix**: Clamp `idle_secs` to `[1, 24*3600]` in `select_victims` or validate at config load. The `idle_timeout_secs` (profile-level) has the same shape.
- **Verification**: `tab_registry.rs:117-145` is the only consumer; no validation in the helper.

### BROWSER-R4-14 — `ChromeMcpBackend::emulate` for `NetworkCondition::Online` as the sole option produces an empty `emulate` args object, whose MCP validity is unguarded
- **File**: src/browser/chrome_mcp_backend.rs:425-470
- **Severity**: Low
- **Category**: correctness
- **Description**: When `opts.network_condition = Some(Online)`, the chrome-devtools-mcp contract expresses `Online` by *omitting* `networkConditions` (per the trait doc). With no other options set, `args` ends up empty (`{}`) and the call goes through `select_and_call(tab_id, "emulate", json!({}))`. The MCP server's `emulate` schema is not pinned by tests — it may reject an empty object as invalid, or it may no-op silently, or it may reset other emulations the caller previously set. Either way, the expected "online" outcome is guessed, not verified.
- **Evidence**:
  ```rust
  // chrome_mcp_backend.rs:444-450
  if let Some(nc) = opts.network_condition {
      if let Some(v) = nc.as_mcp() {
          args.insert("networkConditions".into(), json!(v));
      }
  }
  ```
- **Suggested fix**: Early-return `Ok(())` when the only effective option is `Online` (no call at all — it is the default). Or call `emulate` with a sentinel comment field the schema tolerates. Add a real-API test that asserts the call shape.
- **Verification**: Tests `network_condition_backend_mappings` and `test_emulate_rejects_*` cover the *shape* but not the Online-only no-op path.

### BROWSER-R4-15 — `EmulateOptions::extra_http_headers` accepts arbitrary model-supplied key/value pairs with no size or charset validation
- **File**: src/browser/chrome_mcp_backend.rs:444-465, types.rs:191-194
- **Severity**: Low
- **Category**: security
- **Description**: `EmulateOptions.extra_http_headers: BTreeMap<String, String>` flows into `McpClient::call_tool` unmolested. No upper bound on header count, no character whitelist on names (HTTP header names per RFC 7230 are tokens — letters, digits, and a small set of punctuation), no length cap on values. A misconfigured model request could set `Authorization: sk-ant-api03-…` — though `check_input_secret` covers form-input text, header values pass through `redact_content` egress but the *emulation* itself is not gated. Worst case: a header injection into downstream protocols (CRLF in a value name could in theory split headers, depending on MCP implementation).
- **Evidence**:
  ```rust
  // chrome_mcp_backend.rs:457-462
  if let Some(headers) = &opts.extra_http_headers {
      let encoded = serde_json::to_string(headers).map_err(...)?;
      args.insert("extraHttpHeaders".into(), json!(encoded));
  }
  ```
- **Suggested fix**: Validate at the validate() level (already exists at types.rs:203-225): reject header names that are not valid HTTP tokens, cap header count at 32, cap value bytes at 4096, reject control characters in values.
- **Verification**: `extra_http_headers` is documented at types.rs:178-181 but unwritten in `validate()`. Same gap exists in `user_agent`.

### BROWSER-R4-16 — `wait_probe::poll_wait_for` sleep for `WaitCondition::Time` ignores `timeout_ms` entirely; a caller passing `timeout_ms < ms` gets a longer wait than expected
- **File**: src/browser/wait_probe.rs:97-110
- **Severity**: Low
- **Category**: correctness
- **Description**: The `Time` arm does `tokio::time::sleep(Duration::from_millis(*ms)).await;` and returns `Ok(true)` without consulting `timeout_ms`. The doc comment claims `ms` is pre-clamped by the tool layer but no test or code enforces that contract. A caller passing `timeout_ms = 0, ms = 100` waits 100 ms and reports success; a sane `wait_for("page never changes", timeout_ms = 60_000)` with `Time(120_000)` silently doubles the budget.
- **Evidence**:
  ```rust
  // wait_probe.rs:108-111
  if let WaitCondition::Time(ms) = condition {
      tokio::time::sleep(Duration::from_millis(*ms)).await;
      return Ok(true);
  }
  ```
- **Suggested fix**: `tokio::time::sleep(Duration::from_millis(ms.min(timeout_ms))).await;` or `assert!(ms <= timeout_ms, "Time condition must be <= timeout_ms")` at the boundary.
- **Verification**: Test `time_condition_never_touches_the_page` only verifies that `evaluate` is not called. No timeout-relationship test.

### BROWSER-R4-17 — `PlaywrightCliBackend::navigate` post-nav audit silently degrades to `Ok(())` when both the `page_meta` path and the listing path fail
- **File**: src/browser/playwright_cli_backend.rs:222-265
- **Severity**: Low
- **Category**: security
- **Description**: When the CLI doesn't emit a `### Page` header (older versions) and a subsequent `tab-list` also fails, `audit_landed_tab` logs the skip and returns `Ok(())`. The navigation stands. A redirect to a blocked private host then sits on the tab until the read-time guard happens to fire (or doesn't). The pre-nav check catches the *initial* URL but not the *landed* URL after an HTTP redirect chain. The defense-in-depth comment acknowledges this; flag for operator visibility.
- **Evidence**:
  ```rust
  // playwright_cli_backend.rs:236-264
  let landed = out.page_meta.map(|m| m.url).filter(|u| !u.is_empty());
  let offender = addressable.then(|| tab_id.to_string());
  match (offender, landed) {
      (Some(id), Some(landed)) => {
          super::post_nav::audit_landed_url(self, &self.ssrf_guard, &landed, Some(&id)).await
      }
      (offender, _) => {
          super::post_nav::audit_landed_tab(self, &self.ssrf_guard, offender.as_deref()).await
      }
  }
  ```
- **Suggested fix**: When `audit_landed_tab` skips due to a `list_tabs` failure, also issue `playwright-cli url` (a single URL retrieval command, if supported) to capture the landed URL via a second path. If both fail, quarantine the tab with a generic warning rather than leaving it on an unvetted origin.
- **Verification**: `audit_landed_tab`'s failure path is logged with a warning; the navigation completes successfully. The defense-in-depth gap is the residual risk.

### BROWSER-R4-18 — `wait_probe::poll_wait_for` text probe is a JS-evaluated subset-match: pages that legitimately contain the needle inside a hidden DOM subtree will report "found" even when the user-visible body doesn't show it
- **File**: src/browser/wait_probe.rs:23-46
- **Severity**: Low
- **Category**: correctness
- **Description**: `WaitCondition::Text("foo")` builds `document.body.innerText.includes("foo")`. `innerText` is a CSS-aware rendering (skipping `display: none` and unrendered children for block layout). For SPAs that hide the needle inside `aria-hidden`, `visibility: hidden`, or a collapsed `<details>` element, `innerText` still reports the text — so the wait fires before the user can actually see it. The opposite failure (waits that never fire on text rendered via shadow DOM) is also real. `openclaw` parity note in the doc acknowledges the design choice; flag for visibility only.
- **Evidence**:
  ```rust
  // wait_probe.rs:25-30
  WaitCondition::Text(text) => {
      let needle = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into());
      format!(
          "() => (!!document.body && document.body.innerText.includes({needle})) \
           ? {WAIT_PROBE_FOUND:?} : 'absent'"
      )
  }
  ```
- **Suggested fix**: Use `document.body.innerText` for the common case (current behaviour) but document the failure modes in the tool layer's doc string so the model knows "text contains" is approximate. Optionally, accept an explicit `WaitCondition::TextVisible` for fully-rendered-text checks.
- **Verification**: No test probes the visibility/SPA edge cases.

### BROWSER-R4-19 — `ChromeMcpDriver::ensure_chrome_running` runs `pgrep`/`tasklist` via `spawn_blocking` while holding the global `chrome_launch_lock`; another profile's session-create waits for the IO
- **File**: src/browser/chrome_mcp.rs:313-318, 343-372
- **Severity**: Low
- **Category**: perf
- **Description**: The function acquires `chrome_launch_lock` and then `await`s the `is_chrome_running` (`pgrep`/`tasklist` subprocess). On a slow VM, `pgrep` can take seconds; under contention with periodic reaper sweeps or concurrent ensure_session calls, two callers (on different profiles) serialize behind the long pgrep. Lock is global, not per-profile — a per-profile check would let two profiles launch in parallel. The cost is usually negligible but the design is coarser than needed.
- **Evidence**:
  ```rust
  // chrome_mcp.rs:313-318
  async fn ensure_chrome_running(&self, profile_name: &str) -> Result<(), BrowserError> {
      let _guard = self.chrome_launch_lock.lock().await;

      if Self::is_chrome_running().await {
          return Err(...);
      }
      …
  }
  ```
- **Suggested fix**: Inline the `pgrep` check inside `is_chrome_running` so the lock window is sub-millisecond; or split into a quick `try_is_chrome_running()` (synchronous) for the common cached case. Negligible unless contention is observed.
- **Verification**: Lock pattern read end-to-end (chrome_mcp.rs:292-340). The lock is global.

### BROWSER-R4-20 — `ChromeMcpBackend::handle_dialog` accepts the action `"ok"` / `"confirm"` / `"reject"` aliases; the lowercased `"accept"` is what the MCP server expects, but aliases are silently remapped
- **File**: src/browser/chrome_mcp_backend.rs:381-398
- **Severity**: Low
- **Category**: api-design
- **Description**: The function accepts `"accept"`, `"ok"`, `"confirm"` and silently maps all three to `"accept"` for the MCP server. Same for `"dismiss"`/`"cancel"`/`"reject"`. This is reasonable behaviour for the tool layer but the contract is not pinned by tests — a typo from a future refactor (`"acccept"`) would reach the MCP server unchanged. The same alias fan-in exists in `playwright_cli_backend::handle_dialog` (playwright_cli_backend.rs:497-520).
- **Evidence**:
  ```rust
  // chrome_mcp_backend.rs:381-396
  let action_norm = match action.to_ascii_lowercase().as_str() {
      "accept" | "ok" | "confirm" => "accept",
      "dismiss" | "cancel" | "reject" => "dismiss",
      other => {
          return Err(BrowserError::ActionFailed(format!(
              "unknown dialog action '{other}' — expected 'accept' or 'dismiss'"
          )));
      }
  };
  ```
- **Suggested fix**: Acceptable as-is; flag for visibility only. If tightening, add a test that asserts `"OK"`, `"accept"`, `"Confirm"` all map to `"accept"`.
- **Verification**: No test exercises the alias fan-in.

### BROWSER-R4-21 — `ProfileManager::session_active` reads `tab_registry.has_tabs` as the Managed-side approximation; a managed session whose browser was killed externally stays "active" until the next reaper sweep clears it
- **File**: src/browser/manager.rs:286-294
- **Severity**: Low
- **Category**: correctness
- **Description**: For `BrowserDriver::Managed`, `session_active` returns `self.tab_registry.has_tabs(name)`. The registry is reconciled only by `reap_idle_tabs` (every 5+ seconds) and by `reap_idle` (idle profile sweep). If a managed browser was killed by the user externally (or by OOM), `session_active` still reports `true` until the reaper reads `list_tabs` and clears the registry. A tool call that triggers `get_backend` for a dead-but-flagged-active profile will spawn a new session (lazy open). This is the documented trade-off but worth surfacing for operator visibility.
- **Evidence**:
  ```rust
  // manager.rs:286-294
  pub fn session_active(&self, name: &str) -> bool {
      match self.get_driver(name) {
          Some(BrowserDriver::ExistingSession) => self.chrome_mcp_driver.has_session(name),
          Some(BrowserDriver::Managed) => self.tab_registry.has_tabs(name),
          None => false,
      }
  }
  ```
- **Suggested fix**: Acceptable as-is; flag for visibility only. If tightening, expose `session_active` with a "best-effort" annotation in the doc.
- **Verification**: The function is used by `list_profiles` to produce a status snapshot. Reaper reconciles the registry every sweep.

### BROWSER-R4-22 — `ProfileManager::get_backend` dispatches the `Managed` driver with `Arc::new(PlaywrightCliBackend::new(...))` on every call, but does NOT clone the SsrfGuard `Arc`'s inner policy uniformly across backends
- **File**: src/browser/manager.rs:256-275
- **Severity**: Low
- **Category**: correctness
- **Description**: `ProfileManager::get_backend` reads `self.ssrf_guard.load_full()` once and threads the resulting `Arc<BrowserSsrfGuard>` into the new backend. `Arc::load_full` returns `Arc<T>`; that's correctly passed to both backends. Verified consistent. No actual bug — but the `ArcSwap::load_full` semantics merit a comment: the `Arc` is captured at backend-construction time and lives for the backend's lifetime. If `apply_policy_live` swaps the policy mid-call, the in-flight backend keeps the old policy. For a one-shot tool call this is fine; for the reaper sweep that constructs many backends, each gets whichever `Arc` is current at construction — a benign race.
- **Evidence**:
  ```rust
  // manager.rs:259-275
  BrowserDriver::Managed => {
      let headless = cfg.headless.unwrap_or(self.config.playwright_cli.headless);
      Ok(Arc::new(PlaywrightCliBackend::new(
          self.playwright_cli_driver.clone(),
          profile_name.to_string(),
          self.ssrf_guard.load_full(),
          SessionLaunch::from_profile(&cfg, headless),
      )))
  }
  BrowserDriver::ExistingSession => Ok(Arc::new(ChromeMcpBackend::new(
      self.chrome_mcp_driver.clone(),
      profile_name.to_string(),
      self.ssrf_guard.load_full(),
  ))),
  ```
- **Suggested fix**: None required. Marked for visibility.
- **Verification**: Both backends are constructed with the same `Arc<BrowserSsrfGuard>` from `load_full`. The hot-swap semantics are documented at manager.rs:71-74.

### BROWSER-R4-23 — `playwright_launch::launch_config_json` sets `allowUnrestrictedFileAccess: true` globally; this disables a safety net the CLI author intended
- **File**: src/browser/playwright_launch.rs:130-180
- **Severity**: Low
- **Category**: security
- **Description**: The config object includes `allowUnrestrictedFileAccess: true` deliberately, to bypass `playwright-cli`'s own file-write containment (`outputDir ∪ cwd`). The docstring acknowledges the trade-off: callers must verify paths via `file_ops` themselves. The four callers (`screenshot --filename`, `state-save` / `state-load`, `pdf --filename`, `upload`) are individually gated, but a fifth caller added later will silently inherit the relaxed CLI behaviour. Tag the config with a comment that this is intentional.
- **Evidence**:
  ```rust
  // playwright_launch.rs:159-176
  json!({
      "browser": Value::Object(browser),
      "outputDir": output_dir.to_string_lossy(),
      "allowUnrestrictedFileAccess": true,
  })
  ```
- **Suggested fix**: Tag with `// SAFETY: see file_ops gates; do not extend without re-gating.` on the JSON line. Optional: introduce a `ClosureToggles` audit step that asserts any new CLI verb is registered through a file-ops gate.
- **Verification**: The 4-verb inventory is in the doc above `launch_config_json`. No test pins the count.

## Cross-cutting concerns

1. **Lazy launch policy is the spine, but only one call site honors it explicitly.** `LaunchPolicy::OpenIfNeeded(&self.launch)` is only passed in `run_launching` (playwright_cli_backend.rs:75-83). Every other backend action uses `LaunchPolicy::Refuse`. This is the right shape for the reaper and the idler, but a future contributor adding a new verb to `playwright_cli_backend` must remember to use `run_launching` and not `run` — the type system can't catch it. A `#[must_launch = "explain"]` attribute or a `BackendAction::Observation | BrowserAction` enum would make this an compile-time decision.

2. **Wire-protocol contracts are pinned only for the failures that bit.** `chrome_mcp_backend::fill_form_args` pins `elements` (not `fields`); `wait_for_args` pins the array shape; `select_script_args` pins the single-uid args. But other shapes (`emulate` arg keys, `drag` arg keys, `list_pages` response shape) are unchecked. The pattern is "pin the shape, then write a test that asserts it". Apply that everywhere a backend talks to the wire.

3. **Per-profile locking is duplicated in three drivers.** `ChromeMcpDriver::profile_locks` + `session_create_locks` and `PlaywrightCliDriver::per_session_locks` each maintain a `Mutex<HashMap<String, Arc<AsyncMutex<()>>>>`. They are independent maps (no cross-driver contention) and serve different scopes, but the per-profile lock *purpose* is "serialize same-profile operations" — for the reaper sweep, this serializes against arbitrary tool calls. The reaper doesn't acquire the lock before constructing victims; correctness depends on `close_tab` being idempotent at the wire level. Verify with an integration test.

4. **Error-string matching appears in three places.** BROWSER-R4-03 (`playwright_cli::classify_failure`), BROWSER-R4-04 (`chrome_mcp_backend::classify_wait_error`), BROWSER-R4-09 (`chrome_mcp::call_tool` transport detection). All three are heuristic string matching on `Display` output of an external process or library. The right fix is structured-error variants from the `mcp` and `playwright_cli` layers; the current shape is a known sharp edge and new contributors will be tempted to "fix" the substrings, making the contracts even more fragile.

5. **Two-layer backend construction has a subtle policy-fence.** `ProfileManager::get_backend` captures `self.ssrf_guard.load_full()` (an `Arc`) into the backend; the backend holds the Arc for its lifetime. `apply_policy_live` swaps the guard's `ArcSwap`, but in-flight backends keep the old Arc until they're dropped. For one-shot tool calls this is fine; the reaper constructs fresh backends per sweep, which guarantees the new policy. A long-lived test or a tool that holds a backend across many operations would lag the policy — not flagged for fix, but documenting this would help operators.

6. **Auto-injection of `default`/`user` profiles is on by default.** The `user` profile points at the user's Chrome with `ExistingSession` mode. There is no opt-out (BROWSER-R4-12). Tenants with sensitive profile layouts need a config-level disable; today they must set up a config that effectively replaces the auto-injection mechanism.

7. **`redact_content` zero-copy claim is solid.** Verified end-to-end: when `redact_secrets_in_content = true` and no secret is present, `redact_secrets` returns `Cow::Borrowed`. When a secret is present, it returns `Cow::Owned`. Caller `Cow::as_ref()` / `&*` deref into `&str` cleanly. The `Cow::Borrowed(_)` match in `network_policy.rs::tests` confirms the zero-copy path is exercised.

8. **Lock ordering is consistent within each module.** Across the surface:
   - `ProfileManager`: profiles.RwLock → tab_registry.Mutex (consistent)
   - `ChromeMcpDriver`: profile_locks.Mutex OR session_create_locks.Mutex (independent, no shared resource)
   - `PlaywrightCliDriver`: per_session_locks.Mutex (per-session, so independent)
   - `chrome_mcp.sessions.RwLock`: held briefly; writers serialize via `chrome_launch_lock` or `session_create_lock`
   - No nested lock-acquisition pattern I've found that would deadlock.

## Summary
- **Total: 23 findings** (0 Critical, 0 High, 7 Medium, 16 Low)
- **Top priority items (must-fix)**:
  1. **BROWSER-R4-01** — Launched Chrome processes are leaked (no Drop shutdown). Process accumulation under session-recovery paths.
  2. **BROWSER-R4-03** + **BROWSER-R4-04** + **BROWSER-R4-09** — Three `Display`-based error classifiers (CLI timeout, MCP wait timeout, MCP transport) all use substring matching. Misclassifications produce wrong caller-visible errors and break the contract the doc-comments promise. Push for typed errors at the `mcp`/`playwright_cli` layer.
  3. **BROWSER-R4-05** — `chrome_launch_args` leaves `--user-data-dir` unset when the profile doesn't configure one, causing Aleph's bootstrapped Chrome to silently use the user's daily profile.
  4. **BROWSER-R4-08** — `LIVE_MANAGER` last-write-wins across multiple `ProfileManager` instances; cross-test pollution.
  5. **BROWSER-R4-10** — `provision_binary` has no upper bound on the network installer.

## What was NOT covered
- **Loom concurrency**: I did not model or run the loom tests for the lock-ordering claims.
- **Wire-protocol conformance beyond what the pin-tests assert**: I did not exhaustively diff every chrome-devtools-mcp / playwright-cli arg shape against an external server reference. The pin-tests (in `fill_form_args`, `wait_for_args`, `select_script_args`) anchor the shapes that bit; others are unverified.
- **`pdf_generate/browser_engine.rs`**: Cross-checked the import surface only; not deep-audited. That tool layer probably warrants its own round if it diverges from these drivers.
- **Performance benchmarks**: No `cargo bench` ran. Findings flagged "perf" (R4-06, R4-07) are static, not measured.
- **Cross-module contracts**: I did not verify that `file_ops` actually gates every caller that the `allowUnrestrictedFileAccess` docstring lists (`pdf`, `upload`, etc.). One grep pass through `builtin_tools/browser_tools/{pdf,upload,screenshot,state}.rs` would confirm; not done here.
- **Test-only modules**: `testkit.rs` is heavily exercised by `mod.rs` (`pub(crate) mod testkit;`); I noted its `FakeBackend` discipline (one test pins method coverage) but did not audit every test in `a2a/swe-bench` style.
- **`chromium` browser binary injection / `--load-extension` / pre-installed profile tampering**: I noted `--user-data-dir` not being defaulted (R4-05) but did not audit the full launch-arg matrix against a threat model.
- **`playwright-cli` upstream wire format**: I anchored on the docstring-cited "0.1.8" behavior; if a newer version changed the `### Result` / `### Error` shape, several findings (R4-13, R4-17) need updating.
