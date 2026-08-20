# Logging Module Review (2026-08-22)

**Module:** `src/logging/` (`mod.rs`, `error.rs`, `file_appender.rs`, `level_control.rs`)
**Reviewer:** rust-doctor subagent
**Methodology:** Static code review + graphify cross-reference

> Scope is the 4 files under `src/logging/` only. Cross-module callers
> (`src/gateway/handlers/logs.rs`, `src/gateway/handlers/daemon_control.rs`,
> `src/extension/watcher.rs`, `examples/file_logging_demo.rs`,
> `src/lib.rs:166`) were inspected for contract surface, but their
> internals were not re-reviewed.
>
> Prior reports referenced (for de-duplication only — not re-derived here):
> `docs/engineering-reports/review-results/logging.md` (legacy low-sev
> `pii_filter.rs` notes — file no longer exists) and
> `review-results/severed-wire-2026-08-17/logging/REPORT.md` (orphaned
> re-exports `sw-lo-02`/`sw-lo-03`, name-drift `sw-lo-04`). Those
> findings remain valid on the current tree; this report focuses on
> correctness, concurrency, error-handling, and contract issues that
> the prior wire-audit lens did not cover.

## Summary

- Total findings: 18
- critical: 0, high: 2, medium: 7, low: 9

| #  | severity | file:line                                | category         | one-line summary                                                          |
|----|----------|------------------------------------------|------------------|---------------------------------------------------------------------------|
| 1  | high     | `level_control.rs:118-134` + `logs.rs:74`| error-handling   | `set_log_level` swallows failures; RPC handler lies about success         |
| 2  | high     | `level_control.rs:177-189`               | testing          | `test_get_set_log_level` mutates global state without a guard             |
| 3  | medium   | `error.rs:9`                             | error-handling   | `LoggingError` is `!Send`/`!Sync` due to `Box<dyn Error>` wrapper         |
| 4  | medium   | `level_control.rs:92-96`                 | correctness      | `init_log_level` ignores all but the first comma-separated `RUST_LOG` dir |
| 5  | medium   | `file_appender.rs:15-38`                 | testing          | `test_get_log_directory` has no panic-safe `ALEPH_HOME` rollback          |
| 6  | medium   | `level_control.rs:123-124`               | concurrency      | TOCTOU between `get_log_level` and `store` in `set_log_level`             |
| 7  | medium   | `level_control.rs:118`                   | api              | `set_log_level` should return `Result<(), LoggingError>`                  |
| 8  | medium   | `file_appender.rs:8`                     | error-handling   | `e.into()` wraps `String` as `Box<dyn Error>`, loses precise context      |
| 9  | medium   | `error.rs:9`                             | api              | variant name `LogDirectory` is narrower than the wrapped errors          |
| 10 | low      | `level_control.rs:111`                   | perf             | `SeqCst` load on every `get_log_level` is overkill                        |
| 11 | low      | `level_control.rs:72`                    | error-handling   | `from_u8` emits `tracing::warn!` on every invalid read (spammable)        |
| 12 | low      | `level_control.rs:11`                    | api              | `#[repr(C)]` is misleading; `#[repr(u8)]` matches atomic backing         |
| 13 | low      | `level_control.rs:36`                    | api              | `LogLevel::parse` doesn't accept numeric or alias forms                   |
| 14 | low      | `file_appender.rs` (module)              | api              | Module name is a misnomer — no appender logic (re-confirms sw-lo-04)      |
| 15 | low      | `lib.rs:166`                             | api              | Crate-root `pub use crate::logging::LogLevel` is orphaned (sw-lo-03)      |
| 16 | low      | `level_control.rs:86`                    | api              | `init_log_level` is `pub(crate)` with no external caller                  |
| 17 | low      | `file_appender.rs:40-49`                 | testing          | `test_log_directory_creation` doesn't test `get_log_directory`             |
| 18 | low      | `level_control.rs:150-159`               | testing          | `LogLevel::parse` test coverage is thin (no whitespace / numeric)         |

---

## Findings

### [high] 1. `set_log_level` swallows failures; RPC handler returns success unconditionally

- **Location:** `src/logging/level_control.rs:118-134` and consumer `src/gateway/handlers/logs.rs:54-82` (call site at line 74)
- **Category:** error-handling (cross-module contract)
- **Description:** `set_log_level` calls `aleph_logging::set_log_level` and on error emits a `tracing::debug!` then returns `()`. The RPC handler `handle_set_level` interprets any non-erroring return as `"ok": true` and reports success to the JSON-RPC caller. If `aleph_logging` was never initialized (e.g., server started without `init_component_logging`, or `FILTER_RELOAD` failed to install), the global atomic is updated but the live `EnvFilter` is **not** — log volume does not change.
- **Impact:** The `logs.setLevel` RPC method silently lies about success. Operators who raise the level to debug for incident response may see no additional output and conclude the level was applied, then mistakenly attribute the silence to "no relevant events". Mis-diagnosis under incident pressure is the realistic worst case.
- **Suggested fix:**
  ```rust
  pub fn set_log_level(level: LogLevel) -> Result<(), LoggingError> {
      init_log_level();
      let old_level = get_log_level();
      CURRENT_LOG_LEVEL.store(level.to_u8(), Ordering::Release);
      aleph_logging::set_log_level(level.to_filter_string())
          .map_err(|e| LoggingError::LogDirectory(e.into()))?;
      tracing::info!(old_level = ?old_level, new_level = ?level, "Log level changed");
      Ok(())
  }
  ```
  Then `handle_set_level` propagates the error as a JSON-RPC error code.
  Decide a contract: either the atomic is the source of truth and
  `aleph_logging::set_log_level` failure is non-fatal (current intent,
  but should be *visible*), or both must succeed. The current code
  silently does neither.
- **Related:** graph nodes `src_logging_level_control_set_log_level` ← `src_gateway_handlers_logs_handle_set_level` (relation `calls`).

---

### [high] 2. `test_get_set_log_level` mutates a global atomic with no test isolation

- **Location:** `src/logging/level_control.rs:177-189`
- **Category:** testing (concurrency)
- **Description:** `cargo test` runs tests in parallel by default. This test mutates the global `CURRENT_LOG_LEVEL` (via `set_log_level`) and never restores it on panic or failure. Other tests in the same binary (e.g. `test_default_log_level`) read the same atomic and will see whatever value this test most recently stored, not the `LogLevel::default()` they assume.
- **Impact:** Flaky, order-dependent assertions elsewhere in `level_control` and any code that reads `get_log_level()` (e.g. via `handle_get_level` in integration tests). The hazard is real but currently invisible because the only readers of the atomic in tests are within `level_control` itself and happen to assert against the just-set value.
- **Suggested fix:** wrap the test body in a `Mutex::lock()` (or install a `scopeguard`-style guard that resets on drop), and use `set_log_level` only after the lock is held. Example:
  ```rust
  static TEST_LOCK: Mutex<()> = Mutex::new(());
  #[test]
  fn test_get_set_log_level() {
      let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
      let prev = get_log_level();          // capture prior
      set_log_level(LogLevel::Debug);
      // ... assertions ...
      set_log_level(prev);                 // restore
  }
  ```
  Alternative: serialize all tests in this module with `#[serial]` from `serial_test`.
- **Related:** the sibling test `file_appender::test_get_log_directory` uses `crate::utils::paths::ALEPH_HOME_TEST_GUARD` for the same reason (file_appender.rs:19-21); this test is missing the equivalent guard.

---

### [medium] 3. `LoggingError` is `!Send` and `!Sync` due to `Box<dyn Error>`

- **Location:** `src/logging/error.rs:9`
- **Category:** error-handling
- **Description:** `Box<dyn std::error::Error>` defaults to `?Send`/`?Sync` because the trait object does not bound `Send`/`Sync`. Therefore `LoggingError` is `!Send` and `!Sync`. This blocks the error from crossing `await` points in multi-threaded Tokio executors (the error value cannot be moved into a `Send` future), and prevents it from being held in `Arc<LoggingError>` or any `tokio::sync::Mutex<LoggingError>` (which requires `Send` for the inner guard).
- **Impact:** The only current call site is `get_log_directory() -> Result<PathBuf, LoggingError>` which is sync (good), and `handle_get_directory` which only formats the error via `Display` inside an `async fn` (also good — `Display` does not require `Send`). So today this compiles, but **any future async caller** that tries to propagate the error or store it across `.await` will fail to compile. The constraint is invisible until someone hits it.
- **Suggested fix:** either bound the trait object explicitly,
  ```rust
  pub enum LoggingError {
      #[error("failed to resolve log directory: {0}")]
      LogDirectory(#[source] Box<dyn std::error::Error + Send + Sync>),
  }
  ```
  or skip the box entirely and propagate the upstream error directly via `#[from]` on a wrapper. Note that `aleph_logging::get_log_directory` returns `Result<_, String>`, so a real fix requires either changing `aleph-logging`'s return type or wrapping the `String` into a typed local error.
- **Related:** R4 (interface layers are pure I/O, no business logic) suggests the eventual owner of the error type is a domain model — `LoggingError` is too thin to be one.

---

### [medium] 4. `init_log_level` ignores all but the first `RUST_LOG` directive

- **Location:** `src/logging/level_control.rs:86-103` (specifically lines 92-96)
- **Category:** correctness
- **Description:** `rust_log.split(',').next()` takes only the first comma-separated entry. A user setting `RUST_LOG="info,alephcore=debug,h2=warn"` (a common multi-crate pattern) sees `info` for everything; `alephcore` and `h2` directives are silently dropped. Likewise `RUST_LOG="warn,foo=debug"` drops the `foo=debug` half. Combined with the next-line `split('=').next_back()`, even if the parser were fixed to iterate all entries, only the **last** directive would naturally win — also surprising.
- **Impact:** Operators who craft careful `RUST_LOG` strings get unpredictable behavior: the first directive decides the whole-process level for `alephcore`, regardless of how many per-crate overrides they wrote. Surprises at debug time.
- **Suggested fix:** Either (a) parse all directives and apply the **last** simple level directive (drop target-prefixed ones), or (b) delegate to `tracing_subscriber::EnvFilter::try_from_default_env()` and ask it which directive applies to `alephcore`. At minimum, document the current limitation in the function's doc-comment.
- **Related:** already on the prior `logging.md` low-severity list (file no longer present in the tree), re-asserting as medium because the limitation is silently lossy and not documented.

---

### [medium] 5. `test_get_log_directory` lacks panic-safe `ALEPH_HOME` rollback

- **Location:** `src/logging/file_appender.rs:15-38`
- **Category:** testing
- **Description:** The test saves `prev`, sets `ALEPH_HOME` to a scratch path, calls `get_log_directory()`, runs assertions, then restores `ALEPH_HOME`. If either assertion panics (or `get_log_directory()` returns `Err` and `.unwrap()` panics on line 30), the restore code on lines 34-37 never runs and `ALEPH_HOME` stays pinned to the scratch dir for the rest of the test process. The mutex `ALEPH_HOME_TEST_GUARD` is held by `Drop`, but it does not restore env vars.
- **Impact:** Pollutes the env for any subsequent test that reads `ALEPH_HOME` (the same `ALEPH_HOME_TEST_GUARD` comment in `daemon_control::log_directory_is_under_home` already shows awareness of the hazard). `std::env::set_var` is `unsafe` since Rust 1.84 — there is no compiler enforcement today, but the leak is still real and easy to overlook.
- **Suggested fix:** wrap the env-mutation block in a closure with a panic guard, e.g.:
  ```rust
  struct EnvGuard(Option<OsString>);
  impl Drop for EnvGuard {
      fn drop(&mut self) {
          match self.0.take() {
              Some(v) => std::env::set_var("ALEPH_HOME", v),
              None    => std::env::remove_var("ALEPH_HOME"),
          }
      }
  }
  let _restore = EnvGuard(prev);
  ```
  Or use a crate like `temp_env` / `test-env` for the whole block.
- **Related:** `daemon_control.rs:155-160` references this exact hazard ("same hazard, same guard") but does not solve it either — copy/paste of the same brittle pattern.

---

### [medium] 6. TOCTOU between `get_log_level()` and `store()` in `set_log_level`

- **Location:** `src/logging/level_control.rs:123-124`
- **Category:** concurrency
- **Description:** Line 123 reads `old_level = get_log_level()` and line 124 unconditionally overwrites the atomic. Two threads calling `set_log_level` concurrently produce a sequence where thread A reads `old_level = Info`, thread B stores `Warn`, then thread A stores `Debug`. The audit log emitted at line 129 by thread A says "Info → Debug", which is **inaccurate** — thread B's intermediate `Warn` write is invisible to both A's read and A's eventual audit message.
- **Impact:** Audit-log misleadingness rather than a correctness bug in the level itself (the final stored value is correct). Operational investigations that rely on the "old level" line to reconstruct "what was it set to, and from what" will be wrong. Low frequency in practice (`logs.setLevel` is a manual RPC), but not zero.
- **Suggested fix:** use a `compare-exchange` loop to read-modify-write atomically:
  ```rust
  let mut current = CURRENT_LOG_LEVEL.load(Ordering::Acquire);
  loop {
      let next = level.to_u8();
      if next == current { break; }
      match CURRENT_LOG_LEVEL.compare_exchange(current, next, Ordering::AcqRel, Ordering::Acquire) {
          Ok(_) => break,
          Err(observed) => current = observed,
      }
  }
  ```
  Even better: make `set_log_level` hold a short `Mutex<()>` so the read/store/log are all under the lock. The lock contention is irrelevant compared to RPC latency.
- **Related:** R8 ("LLM handles intent; regex only for machine formats") is not violated; R10 ("zero middleware tax") is — the cost of one acquire fence is far less than the cost of a wrong audit log.

---

### [medium] 7. `set_log_level` returns `()` — should return `Result<(), LoggingError>`

- **Location:** `src/logging/level_control.rs:118`
- **Category:** api (cross-module contract)
- **Description:** Returning `()` makes the failure mode from finding #1 invisible. The RPC layer treats the call as infallible and propagates "ok:true" to the user. Other future callers (e.g. a CLI flag, a config reload, a TUI shortcut) would have to do their own silent-error swallow.
- **Impact:** Pairs with #1. Fixing #1 makes this fix obvious — return a `Result`, propagate from the handler, document the contract in the doc-comment.
- **Suggested fix:** (see #1) `pub fn set_log_level(level: LogLevel) -> Result<(), LoggingError>`. If you want a non-fatal variant, expose a separate `try_set_log_level` or change the error type to `LoggingError` with a `RuntimeFilterUnavailable` variant that callers can choose to ignore.
- **Related:** graph nodes: `src_logging_level_control_set_log_level` is consumed by `src_gateway_handlers_logs_handle_set_level` and `src_gateway_handlers_logs_test_set_level`. Both currently discard the return value.

---

### [medium] 8. `e.into()` in `file_appender.rs` wraps `String` as `Box<dyn Error>`, losing context

- **Location:** `src/logging/file_appender.rs:8`
- **Category:** error-handling
- **Description:** `aleph_logging::get_log_directory` returns `Result<PathBuf, String>`. The `String` is `into()`-converted to `Box<dyn std::error::Error>`. `String` does not implement `std::error::Error`, so `From<String>` is the implicit blanket that converts via the orphan rule — the result is a `Box<dyn Error>` whose `.source()` is `None` and whose `.to_string()` is the original string. The information is preserved, but it is no longer structured: no error code, no chainable cause, no `#[from]` integration.
- **Impact:** Any consumer that wants to programmatically distinguish "HOME not set" from "filesystem permission denied" cannot do so — the only failure is "the string". Compare with `aleph_logging::get_log_directory` itself, which is hand-rolled: its sole error path returns the literal `"Cannot determine home directory"`. There's no signal here to lose, but the wrapper pretends to be a structured error type.
- **Suggested fix:** introduce a typed error variant in `aleph-logging` (e.g. `pub enum LogDirError { HomeUnset, HomeNotAbsolute(PathBuf) }`) and propagate with `#[from]`. At minimum, add a `LoggingError::Backend(String)` variant that preserves the original `String` and drops the `Box<dyn Error>` indirection (also helps with #3).
- **Related:** R3 (core minimalism — no heavy deps for non-core features). `Box<dyn Error>` is a tiny dep but the indirection adds zero value here.

---

### [medium] 9. `LoggingError::LogDirectory` variant name is narrower than the underlying errors

- **Location:** `src/logging/error.rs:9`
- **Category:** api (naming)
- **Description:** The variant is named `LogDirectory` and the doc-comment reads "Failed to resolve the log directory path." The wrapped source, however, may include path resolution (no HOME), filesystem permission errors (when used in a future caller that calls `create_dir_all`), and any future operation that the shared crate adds. The name lies about its scope.
- **Impact:** Future contributors will add a new variant `LoggingError::Filesystem(...)` thinking `LogDirectory` covers only resolution, when in fact `Box<dyn Error>` already covers both. Either rename to `Backend` / `Resolve` / `Upstream` to reflect the broadness, or split into multiple variants as the underlying error types evolve.
- **Suggested fix:** rename to `LoggingError::Resolve(#[source] Box<dyn std::error::Error + Send + Sync>)` and reserve `LogDirectory` for path resolution specifically; or expand the `Display` to say "logging backend error" rather than "failed to resolve the log directory path".
- **Related:** AGENTS.md "Ask > Assume" rule: the current name encodes an assumption (path resolution only) that is already false on day one.

---

### [low] 10. `SeqCst` load on every `get_log_level` call is overkill

- **Location:** `src/logging/level_control.rs:111`
- **Category:** perf
- **Description:** `Ordering::SeqCst` is required only when the atomic participates in a total order with other `SeqCst` operations. There are no other `SeqCst` accesses to track. `Ordering::Acquire` on the load (paired with `Release` on the store) is sufficient and cheaper on x86 and significantly cheaper on ARM/POWER.
- **Impact:** Negligible at typical RPC rates (single-digit Hz). If `get_log_level` is invoked from a hot path (e.g. a per-request filter check that the project might add later), the cost would add up. Today: low.
- **Suggested fix:** change the load to `Ordering::Acquire` and the store to `Ordering::Release`. Keep `SeqCst` only if a future invariant requires it.

---

### [low] 11. `from_u8` emits `tracing::warn!` on every invalid read

- **Location:** `src/logging/level_control.rs:71-74`
- **Category:** error-handling
- **Description:** If the atomic ever holds an out-of-range u8 (memory corruption, debug write, ABI mismatch across hot-reload), `from_u8` emits a `warn!` event every call. There is no rate-limit, dedupe, or "warn once" guard.
- **Impact:** Log spam under fault. The "correct" behavior is to fall back to `Info` silently with a one-shot warn (or `debug_assert!`), not a per-read warn. The prior `logging.md` report flagged this as low; the current implementation moved from `debug_assert!` to a `tracing::warn!`, which is *worse* for spammability.
- **Suggested fix:** use a `static INVALID_WARNED: AtomicBool` to log exactly once, or downgrade to `debug_assert!(false, ...)`. Even better, add a `debug_assert!` *and* the warn under a once-guard.

---

### [low] 12. `#[repr(C)]` on `LogLevel` is misleading

- **Location:** `src/logging/level_control.rs:11`
- **Category:** api
- **Description:** `#[repr(C)]` on a Rust enum advertises "this can be passed to C code as a C-compatible enum". There is no FFI consumer in the tree; the only consumer is `CURRENT_LOG_LEVEL: AtomicU8`, which expects a 1-byte representation. `#[repr(C)]` on a unit-variant-only enum compiles to whatever the platform ABI chooses (typically `c_int` = 4 bytes), not 1 byte.
- **Impact:** Misleading documentation. Not a runtime bug because nothing actually stores the enum through its `repr(C)` layout — the enum is always narrowed to `u8` before storage. But readers will assume there is FFI they need to maintain, or worse, may `mem::transmute` it across an FFI boundary assuming a 4-byte payload.
- **Suggested fix:** use `#[repr(u8)]` to match the atomic backing, or drop the `repr` entirely (default Rust layout is fine for an internal enum). Add a doc-comment explaining the chosen representation.

---

### [low] 13. `LogLevel::parse` doesn't accept numeric or alias forms

- **Location:** `src/logging/level_control.rs:36-50`
- **Category:** api
- **Description:** `parse` only accepts the five canonical names plus `"warning"` as an alias for `"warn"`. `tracing-subscriber::EnvFilter` (which is what this string eventually feeds) accepts numeric levels (`"3"`), `Off`, `Error`, `Warn`, `Info`, `Debug`, `Trace`. `LogLevel::parse` rejects all but a subset. If a user passes `"ERROR"` → fine. If they pass `"  debug  "` → `None`. If they pass `"3"` → `None`.
- **Impact:** `RUST_LOG="debug"` works. `RUST_LOG="3"` does not. Operators who copy-paste from `tracing-subscriber` docs get a silent fallback to `Info` (via the `else { None }` branch on line 48). Low because documentation consistently uses canonical names.
- **Suggested fix:** add `.trim()` and `.trim_matches('"')` before the comparison; optionally accept numeric strings via `.parse::<u8>()` then `LogLevel::from_u8`.

---

### [low] 14. Module name `file_appender.rs` is a misnomer (re-confirms sw-lo-04)

- **Location:** `src/logging/file_appender.rs` (entire file, 49 lines)
- **Category:** api (naming)
- **Description:** The module is named `file_appender` but contains only `get_log_directory()` — a path resolver that delegates to `aleph-logging`. There is no appender, no rolling-file logic, no `tracing_appender` interaction. The actual file appender lives in `shared/logging/src/file_appender.rs`. The name-drift was already flagged as `sw-lo-04` and remains in the current tree.
- **Impact:** Discoverability. A reader searching for "where does the file appender live" will land here and find nothing. The doc-comment at line 1 attempts to head this off ("delegates to `aleph-logging` crate") but the module name still wins on `rg file_appender`.
- **Suggested fix:** rename `src/logging/file_appender.rs` → `src/logging/log_directory.rs` (or fold it into `mod.rs` since it's a 1-line wrapper around a 1-line wrapper). Update the `pub mod` and `pub use` lines in `mod.rs:7,11`.

---

### [low] 15. Crate-root `pub use crate::logging::LogLevel` is orphaned (re-confirms sw-lo-03)

- **Location:** `src/lib.rs:166`
- **Category:** api
- **Description:** `pub use crate::logging::LogLevel;` at the crate root has zero in-tree consumers:
  ```
  $ rg -n "alephcore::LogLevel|crate::LogLevel" src/ shared/ interfaces/ desktop/ tests/ examples/
  → no matches
  ```
  Production consumers (e.g. `src/gateway/handlers/logs.rs:10`) import `crate::logging::LogLevel` directly. Out-of-tree, `examples/file_logging_demo.rs` only uses `alephcore::logging::get_log_directory`, not the type.
- **Impact:** Library-contract question, not a bug. Keeping the re-export costs nothing; dropping it is a public-API removal that external consumers (if any) will notice. Already on sw-lo-03 list; repeating here for visibility.
- **Suggested fix:** decide once and document. Either delete `lib.rs:166`, or move all callers to `crate::LogLevel` and delete the nested re-export in `mod.rs:12`. Don't keep both.

---

### [low] 16. `init_log_level` is `pub(crate)` with no external caller

- **Location:** `src/logging/level_control.rs:86`
- **Category:** api
- **Description:** `pub(crate) fn init_log_level` is only invoked by `get_log_level` and `set_log_level` within the same module. There is no `pub(crate)` caller elsewhere in `src/`. The `pub(crate)` is dead visibility.
- **Impact:** None at runtime. Mild signal of intent — the author may have planned an external seeding API and never wired it.
- **Suggested fix:** drop `pub(crate)`. If a future caller wants to seed it early (e.g. `main` before any RPC), promote to `pub` with a clear doc-comment explaining the Once-guarantee.

---

### [low] 17. `test_log_directory_creation` doesn't test `get_log_directory`

- **Location:** `src/logging/file_appender.rs:40-49`
- **Category:** testing
- **Description:** The test creates a `tempfile::TempDir`, joins `"logs"`, asserts the directory exists, then drops. It never calls `get_log_directory` or touches `ALEPH_HOME`. The name `test_log_directory_creation` implies "test that get_log_directory creates the directory" but `get_log_directory` does *not* create directories — `shared/logging::setup_logging` does, on first `init_component_logging`.
- **Impact:** False-positive coverage signal. CI reports "we tested log directory creation" when in fact nothing about `get_log_directory` was exercised.
- **Suggested fix:** delete this test (it tests `tempfile` and `std::fs`, not our code), or rename and rewrite to call `get_log_directory()` and assert the returned path exists/is writable.

---

### [low] 18. `LogLevel::parse` test coverage is thin

- **Location:** `src/logging/level_control.rs:149-159`
- **Category:** testing
- **Description:** The test covers canonical lowercase, uppercase, the `"warning"` alias, and one negative case. It does not cover: empty string (`""`), whitespace (`"  debug"`), quoted (`"\"debug\""`), numeric (`"3"`), non-ASCII (`"INFO"` is covered but `"ｉｎｆｏ"` is not), `Option<&str>` type confusion. Many of these are caught by the `unwrap_or("info")` fallback in `init_log_level`, but the API surface itself (`LogLevel::parse`) is publicly exposed and reachable from JSON-RPC.
- **Impact:** Edge-case regressions in `parse` would slip through.
- **Suggested fix:** add a table-driven test:
  ```rust
  #[test]
  fn test_log_level_parse_edge_cases() {
      assert_eq!(LogLevel::parse(""), None);
      assert_eq!(LogLevel::parse("  "), None);
      assert_eq!(LogLevel::parse("debug "), None);   // current impl rejects — confirm intent
      assert_eq!(LogLevel::parse(" debug"), None);
      assert_eq!(LogLevel::parse("3"), None);        // numeric — confirm intent
      assert_eq!(LogLevel::parse("info=debug"), None);
  }
  ```

---

## Cross-Module Notes

1. **`shared/logging` (a.k.a. `aleph-logging`) is the actual owner of file-rotation, PII scrubbing, and retention.** `src/logging/` is a thin facade over two functions: `get_log_directory` and `set_log_level`. This is a healthy delegation: core doesn't talk to `tracing-appender` directly. R1 is satisfied (no `AppKit`, `Vision`, `CoreGraphics`, etc.). R3 is satisfied (no heavy deps pulled into core for logging — `tracing-subscriber`, `tracing-appender` live in `shared/logging`).

2. **Source of truth is duplicated.** Both `aleph_logging::get_log_directory` (`shared/logging/src/file_appender.rs:170`) and `alephcore::utils::paths::get_config_dir` (per the comment on line 159-162 of `shared/logging/src/file_appender.rs`) implement the same `ALEPH_HOME → $HOME → dirs::home_dir()` resolution. The doc-comment explicitly says "re-implemented here because this crate must stay free of an alephcore dependency". This is a deliberate duplication to break a workspace cycle — keep it, but **flag it**: any change to one resolver must be mirrored to the other, otherwise `logs.getDirectory` and the config directory can disagree (they already would, on systems where `dirs::home_dir()` differs from `$HOME`, e.g. macOS sandboxed apps).

3. **`LoggingError` vs `aleph_logging`'s `String`.** The shared crate's `get_log_directory` returns `Result<_, String>`; the core crate wraps that `String` in `Box<dyn std::error::Error>` to satisfy its own `LoggingError`. This is the only error type in core's logging surface and it's a one-liner. If the core ever needs a second failure mode (e.g. permission denied during `create_dir_all`), the current shape forces a variant addition; the shape itself is fine but slim.

4. **Crate-root re-export churn.** `lib.rs:166` re-exports `crate::logging::LogLevel`; `mod.rs:12` re-exports `LogLevel` from the submodule. `gateway/handlers/logs.rs:10` imports the nested path. The chain is duplicated but each link is harmless individually — the cost is purely navigational. Decision needed (see finding #15).

5. **No callers of `logging::error` other than `file_appender::get_log_directory`.** Confirmed:
   ```
   $ rg -n "LoggingError" src/ → src/logging/error.rs, src/logging/file_appender.rs
   ```
   External callers (`gateway/handlers/logs.rs::handle_get_directory`) use `.err()` formatting, never the type. `LoggingError` is therefore a library-contract type, not an in-tree plumbing type. The wrapper's ergonomic surface (Display, Error, `#[non_exhaustive]`) is correct for the audience.

6. **`set_log_level` is the only function in this module that crosses an async boundary** (via `tracing::info!` and `tracing::debug!` on tokio runtimes). It is itself sync, so no `Send`/`Sync` issue today. If it ever grows an async hook (e.g. notify subscribers), the `LoggingError !Send` issue (#3) will bite.

7. **Comparison with prior `severed-wire-2026-08-17` audit findings.** The earlier audit flagged four items (`sw-lo-01..04`). Status check on the current tree:
   - `sw-lo-01` (`LoggingError::Init` and `LoggingError::Cleanup` dead variants) — **resolved**: current `LoggingError` has only `LogDirectory`. Either the variants were removed or the file was rewritten.
   - `sw-lo-02` (orphaned `pub use error::LoggingError`) — **still present** at `mod.rs:10`. Only used by the in-module import at `file_appender.rs:4`. Verdict unchanged.
   - `sw-lo-03` (orphaned crate-root `pub use crate::logging::LogLevel` at `lib.rs:166`) — **still present**. Verdict unchanged.
   - `sw-lo-04` (name drift `file_appender` module) — **still present**. Re-asserted as finding #14.

8. **Test mutex usage is inconsistent.** `ALEPH_HOME_TEST_GUARD` is used by `file_appender::test_get_log_directory`, `gateway::handlers::daemon_control::log_directory_is_under_home`, and two `gateway::handlers::search_config` tests. It is *not* used by `level_control::test_get_set_log_level` (which races with the others by mutating a different global, but is still a global). Two global locks needed: one for env vars, one for the log-level atomic.

---

## Out of Scope / Skipped

- **`tracing` macro correctness** (e.g. `tracing::info!(old_level = ?old_level, ...)` field syntax) — assumed correct because the compiler will reject misuse. Not statically reviewed beyond argument count.
- **`Cargo.toml` dependency bounds** — `aleph-logging = { path = "shared/logging" }` is a path dep. Whether the path is correct and the workspace member is present was not re-verified; assumed OK because the code compiles (`rg "use aleph_logging::"` returns the expected imports).
- **`shared/logging/src/file_appender.rs::setup_logging`** — the actual file-appender code lives there, not in `src/logging/`. This review is scoped to the core facade. The shared crate's behavior (PII scrubbing, rotation, retention, `EnvFilter` reload) was not re-reviewed; refer to prior reports for that crate.
- **Behavior under `tokio::test` runtime** — `gateway::handlers::logs::test_get_level` and `test_set_level` are `#[tokio::test]` and call the sync `set_log_level`/`get_log_level` directly. Whether the tokio runtime interferes with the static `Once`/`AtomicU8` was not investigated (none expected, since both are process-globals, not task-locals).
- **`tracing-subscriber::reload::Handle` failure modes** — if `try_init()` in `shared/logging/src/file_appender.rs:117` returns an error after a partial subscriber has been installed, what happens to the filter handle is the shared crate's problem, not core's. Out of scope.
- **`DirBuilder` / `PathBuf::join` cross-platform semantics** — the path joining `home.join(".aleph").join("logs")` is platform-correct in spirit but not exercised on Windows paths in this review. The `desktop/windows` member should pick this up.
- **Log volume / cost of `tracing::warn!` on every RPC** — not relevant here because the RPC layer does not log per-call, but worth a future audit if `gateway::handlers::logs` ever logs at info+ per call.
- **Memory ordering under hot-reload / FFI** — if a future hot-reload writes the atomic from another process (e.g. via `/proc/PID/mem`), the `Relaxed` proposal in finding #10 is insufficient. Sticking with `SeqCst` is defensible for that reason alone; documenting the choice is the actual fix.
- **Real `core::logging` vs `shared/logging` boundary** — the architectural redline is "core never calls platform APIs". The current module satisfies this. Whether the module's *role* should grow (e.g. centralized audit logging) is a design question, not a static-review finding.