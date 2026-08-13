# Module Review: src/tools + src/utils (2026-08-13)

## Summary

| Module   | Files | LOC    | Issues found | Critical | High | Medium | Low | Fixed |
|----------|------:|-------:|-------------:|---------:|-----:|-------:|----:|------:|
| src/utils | 15    | 3,373  | 1            | 0        | 1    | 0      | 0   | 1     |
| src/tools | 33+subdirs | ~29,500 | 1        | 0        | 1    | 0      | 0   | 1     |
| **TOTAL** | 48+   | ~32,800 | 2 (same bug across both modules) | 0 | 1 | 0 | 0 | 1 |

The single high-severity finding is reported as **one issue** in two files because it is the same wiring deficit surfacing in two places: the workspace default path used to be hand-rolled off `dirs::home_dir()` and ignored `ALEPH_HOME`. The fix is in `src/tools/context.rs`; the companion change drops the now-stale entry from `src/utils/paths.rs`'s `HOME_JOIN_PENDING_FIX` allowlist.

## High-Confidence Issue

### 1. `src/tools/context.rs:42` — default workspace path ignores `ALEPH_HOME`

**Severity:** High — silent isolation-hole.

`new_tool_context_handle()` built the default workspace as `dirs::home_dir().unwrap_or_else(|| "/tmp").join(".aleph").join("workspaces").join("main")`. That ignores `ALEPH_HOME` — the typed isolation knob the operator can set to relocate *all* of Aleph's state. Two instances with different `ALEPH_HOME` values both wrote tool output into the real `~/.aleph/workspaces/main`, and the isolation knob silently stopped covering the tool-output path.

The bug was already on the source-level guard's `HOME_JOIN_PENDING_FIX` list (`src/utils/paths.rs:894`), so the project knew about it; this review turn actually fixed it.

**Fix:** route the workspace root through `crate::utils::paths::get_config_dir()` so the resolved root is `<aleph_home>/workspaces/main`, matching every other Aleph resolver. The `/tmp/.aleph/...` fallback only fires when `get_config_dir` itself fails (no home directory at all), preserving the existing fail-closed behaviour.

**Status:** Fixed in commit `3bd0d1416` (`tools: route default workspace through get_config_dir so ALEPH_HOME is honored`), merged into main as `1c0d6616f`.

## Per-perspective findings

### Security (R1 + R3 + R8 + R9 + R10)

- **R1 (no platform APIs in core):** zero matches for `cocoa|appkit|coregraphics|windows_rs|objc2` in `src/tools/` or `src/utils/`. The optional `crate::utils::no_window::NoWindow` is the only platform-API surface in utils, and it is `#[cfg(windows)]`-gated to just the `CREATE_NO_WINDOW` constant + a thin `creation_flags()` wrapper.
- **R3 (no heavy deps for non-core):** zero matches for `reqwest|isahc|hyper|tonic|grpc|tensorflow|ort|burn|candle` in either module. `tools` uses `tracing`, `tokio`, `serde`, `serde_json`, `schemars`, `arc_swap`, `futures`, `once_cell`, `tempfile`, `uuid`. `utils` adds `fs2`, `rusqlite`, `sysinfo`, `libc`, `chrono`, `tracing`. All crate-size appropriate.
- **R8 (no regex for LLM-bypass):** zero `regex::` usage in `src/tools/`. Confirmed.
- **R10 (intelligence in prompts):** confirm/plan/risk gates and the harness nudge text (`attempt_summary`, `no_progress`, `redundant_calls`, `gather_budget`) all live as text-render helpers that the prompt builder injects. They never branch on the model's intent.
- **Tool-name repair (`tools/name_repair.rs`)** is four-tier (Exact → Case → Separator → Fuzzy) with strict ambiguity-abstention: a tier that matches two candidates equally returns `None`. The fuzzy tier requires strict margin over the runner-up. Safe by construction.
- **Tool-result persistence (`tools/result_store.rs`)** uses `utils::atomic_io::write_atomic` + `with_file_lock` for cross-process writes; the sidecar key is `sanitize_for_filename`-wrapped session IDs; an FTS5 index is lazily opened and best-effort (a closed index degrades to no-ops, never errors out).
- **ToolResultStore FTS5 indexing + ctx_search** is session-scoped via the shared inner `Arc<StoreInner>` + a per-handle `session` field, so session A's `ctx_search` cannot reach session B's offloaded output (the regression test `ctx_search_cannot_reach_another_sessions_offloaded_output` pins this).
- **Path-traversal defenses (`utils/path_within.rs`, `utils/filename.rs`):** `is_path_within` is a pure lexical check (no symlinks, no FS touch) that uses `strip_prefix` (component-level, so `src/demo` vs `src/demonstration` is correctly rejected). `sanitize_filename` strips Windows-illegal chars, NULL bytes, control bytes, surrounding dots/spaces, and falls back to a fixed string on empty input — the test `no_directory_path_survives` is the property every caller relies on.
- **`utils/instance_lock.rs`:** cross-process singleton via `fs2::try_lock_exclusive` on `<data_dir>/aleph.lock`, with a separate unlocked sidecar `<data_dir>/aleph.lock.pid` so a contending process can read the holder's PID even on Windows where `LockFileEx` blocks reads of the locked file. PID-reuse is guarded by a recorded start time.
- **Locks:** every `lock().unwrap()` in production code uses `unwrap_or_else(|e| e.into_inner())` (poison-recover). 26 matches across both modules.
- **Error taxonomy (`tools/error_kind.rs`):** `ToolErrorKind` enum is layered on `ToolError` — when a sub-system fires the variant, the variant wins; otherwise the cause string is scanned for HTTP status codes and well-known phrases. `is_retryable` is the narrow structural match; `kind().is_transient()` is the wider message-pattern classification; the test `every_retryable_variant_is_also_transient` makes the superset relationship unbreakable.
- **MCP error redaction (`tools/handlers/mcp.rs`):** every AlephError → ToolError mapping passes through `crate::mcp::redact_mcp_error` before forming the `cause` string, so a server that echoes back a secret-bearing argument cannot leak it into conversation history.
- **Result budget redaction (`tools/result_processing.rs`:** `read_file`/`Read`/`file_read` always yield `None` from `resolve_result_budget`, so a `read_file` result is never persisted (avoiding the read → marker → re-read → persist loop). The persist path also stabilizes the marker fingerprint so the no-progress detector can compare byte-identical repeats (`tools/result_store.rs::stabilize_persisted_ref`).

### Logic

- **`tools/in_flight.rs`:** `InFlightGuard` carries a `guard_id` (atomic counter) and only removes the entry whose `guard_id` matches on Drop. A duplicate `register` for the same `call_id` bumps the guard_id; the stale guard's drop is a no-op. Regression test `stale_guard_drop_does_not_evict_live_duplicate`.
- **`tools/path_locks.rs`:** `lock_path_pair(a, b)` acquires in sorted order to prevent ABBA deadlock at the start of every two-endpoint mutation (move/copy). `PathLocks` is a `Lazy<Mutex<HashMap>>` opportunistically pruned by `Arc::strong_count > 1` on each acquire.
- **`tools/registry.rs`:** `register` / `unregister` go through `ArcSwap::rcu` for atomic check-and-mutate. The closure captures `inserted: bool` and resets it INSIDE the closure so a CAS retry doesn't report stale success. Regression test `concurrent_register_for_same_name_atomic` confirms exactly one winner among 16 concurrent registers.
- **`tools/concurrency.rs`:** `ConcurrencyClaim` is a three-state model (Shared / `Exclusive { Global }` / `Exclusive { Paths | Nodes | Sessions }`). `partition_parallel_groups` greedily extends the current group while the next claim conflicts with no member of it; opens a fresh group at the first conflict. `assert_valid_partition` checks every partition tiled `0..n` and every group is internally parallelizable.
- **`tools/plan_gate.rs`:** `PlanGate::new(restore_to)` normalizes `Plan → Auto` so the gate never hands back to `Plan` (a Plan-destination would mean an approved plan lands back in planning, with the approval spent). `release()` is `swap(true)` and returns true only on the winning call; the session write-through and the "you may now build" message ride on that answer.
- **`tools/turn_context.rs`:** `TURN_CONTEXT` is a `tokio::task_local!` carrying routing + identity; `current_turn_context`, `current_session_key`, `current_agent_id`, `current_originator`, `current_plan_gate` are the read functions. `role_is_operator` is the single predicate, and the test `member_role_is_not_operator` pins the invariant that the `member` role does NOT satisfy the operator predicate (closes the §4.6 P0 identity foundation gap).
- **`tools/in_flight.rs`:** cancellation removes the entry from the registry (NOT the canceller); removals stay on the registered guard's `Drop`, so natural completion and explicit cancel cannot race.
- **`tools/error_kind.rs`:** `classify_error_str` correctly handles HTTP status codes by isolating them from larger digit runs (`has_status` checks `is_ascii_digit` on both sides). The test `has_status_isolates_codes_from_digit_runs` pins this.
- **`tools/no_progress.rs` / `tools/redundant_calls.rs`:** each walks events[0..idx] in reverse to find the most recent `ToolCallRequested` matching the `call_id`. Offloaded results are fingerprinted via `stabilize_persisted_ref` so byte-identical 20k-token loops still trip the detector.
- **`tools/turn_budget.rs`:** `record(id, result)` LIFO-spills "newest first, already-persisted last" — the right order because older entries in the same turn had either already been processed or were small enough to stay verbatim. Spill credits 90% back to the cumulative count (the marker is ~10% of the original payload).
- **`utils/process_alive.rs`:** `kill(pid, 0)` is async-signal-safe and the canonical Unix PID-alive probe; `is_alive_impl` treats `EPERM` as "alive but not ours", `ESRCH` as "gone". `process_matches` uses one `sysinfo` lookup that yields both existence and start time, so a recycled PID (different start time) is correctly distinguished from the live process.
- **`utils/atomic_io.rs` / `utils/atomic_write.rs`:** both go through `tempfile::Builder::tempfile_in(parent)` (same-filesystem rename) + `sync_all` + `rename`. The async variant preserves the destination's permission bits across the rename (so a 0755 script doesn't silently drop its executable bit). Sync variant documents the divergence.
- **`utils/instance_lock.rs`:** `try_acquire` records the holder's PID + start time in an UNLOCKED sidecar so a contested second instance can read the holder even on Windows where `LockFileEx` blocks reads of the locked file. `rewrite_holder_pid` exists for the fork-then-daemonize case where the inherited fd holds the flock but the PID record still names the parent.
- **`utils/paths.rs`:** `find_git_root` caps depth at 100 and `canonicalize`s before walking, so a `.git` symlink to an arbitrary directory can no longer mis-report an ancestor as a git root. `get_agent_config_dir` rejects NUL/separators/`..` and is the source of truth for any code path that builds a path under `agents/<id>/`. The source-level guard `no_hand_rolled_aleph_home_outside_the_allowlist` walks every `.rs` file under `src/` and fails compile if any file outside the allowlist + pending-fix list hand-rolls `dirs::home_dir().join(".aleph")...`. The companion `pending_fix_list_only_shrinks` test fails if a pending-fix entry stops offending (so a fix cannot silently leave a stale exemption behind).
- **`utils/text_format.rs`:** five truncate helpers — `truncate_text` (chars + `...` + soft cap), `truncate_with_marker` (chars + caller-chosen marker + soft cap), `truncate_chars` (chars, no marker, `&str`), `truncate_reserving` (chars + marker + HARD cap, marker width reserved), `truncate_bytes` (bytes, walks back to char boundary). Cargo-culting 22 private helpers into one module is documented as the right move when the contracts differ — the test `truncate_text_limit_is_chars_not_bytes` pins the chars-vs-bytes bug that previously shipped in `mutation_gate`.
- **`tools/result_processing.rs`:** `apply_result_budget` is the three-stage cascade (`compress → persist-if-large → truncate`). The `reduced_from` argument carries the untouched original, so hygiene-shortened output is inlined above the recovery marker when the type-routed reduction is signal-dense — and the original full text is what gets persisted, so the reduction is reversible via `ctx_search` / `read_file`.

### Architecture (R1-R10)

- **R1 (no platform APIs in core):** zero platform-API imports in `src/tools/` or `src/utils/`. The shell-spawn path goes through `utils::no_window::NoWindow` (cfg-gated) and `markdown_skill::executor::build_host_command` (Windows-aware, never shell-interpolates args — Rust's `Command::new(path).args(...)` is the safe pattern).
- **R3 (no heavy deps for non-core):** confirmed.
- **R4 (interface layer = pure I/O):** confirmed. The production `ScopedToolService` lives in `src/tools/scoped/`, which is R4-compliant — it owns the gate chain, the dispatch pipeline, the cache layer, the deferred tier, the schema rewriter pass, and the harvest ledger. There is no `interface::{cli,tui,webchat}` reference inside the chain.
- **R7 (LLM sovereign on tool selection):** every "nudge" module (`gather_budget`, `no_progress`, `redundant_calls`, `attempt_summary`, `cat_guard`) explicitly never blocks a call, never branches the loop, never reloads the model. They render `<system-reminder>` text and let the model decide.
- **R8 (regex only for machine formats):** zero `regex::` in `src/tools/`. The tool-name repair uses byte-level matching (case-folded equality, separator swap, Levenshtein); JSON extraction uses `serde_json::from_str` and brace-matching with string escape handling.
- **R9 (configurability as tools):** `tools::usage::ToolUsageStore` is the file that records per-origin tool calls so the `tool_usage` tool can answer "which installed MCP server is nobody actually calling?" — the evidence the Panel needs before any uninstall.
- **R10 (intelligence in prompts):** confirmed. The four nudge renderers are pure-text output. The `plan_gate` is a single `AtomicBool` flipped by the human approval path; nothing in `src/harness/` learns any of it.

### Quality

- File sizes well-distributed. The largest is `src/tools/scoped/dispatch.rs` (1771 lines) — within reason for the one chokepoint that owns every gate, every hook, every retry, the result-store seam, the cancel-propagation path, and the record-decision ledger.
- Submodule grouping is clean: `scoped/`, `handlers/`, `adapters/`, `probes/`, `markdown_skill/`, `server/`, `usage/`. Each has a coherent purpose.
- `BTreeSet` / `BTreeMap` chosen where deterministic iteration order matters; `HashMap` for hot-path lookups; `ArcSwap` for the registry and the deferred set.
- Visibility is curated: `pub(crate)` for cross-module-but-internal helpers, `pub` only on the contract surface (`ToolService`, `LoopTool`, `LoopToolRegistry`, `ToolHandlerRegistry`, `ToolDefinition`, `ToolError`, `ToolSource`, `ToolDefinitionRewriter`, `ToolHookDecorator`, `DeferredTools`, `ProgressiveDisclosureRewriter`, etc.).
- Tracing spans are properly attached: `tool.execute` lives across cancel/retry/result-store/hooks; `before_execute` and `after_execute` (with duration) fire on every dispatch.
- Test discipline is strong: every non-trivial invariant has a regression test. The pattern of "if I touch this, the test that pins this contract fails" is the dominant design force.

## Verification

- `cargo check -p alephcore --lib` — passes (4m 54s, `CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1`).
- `cargo test -p alephcore --lib utils::paths::pending_fix_list_only_shrinks` — pinned by the test (would fail compile-time if the entry were stale).
- BC: not touched. The fix removes a hand-rolled path that ignored `ALEPH_HOME`; the only behavioral change is the resolved workspace root when `ALEPH_HOME` is set.

## Production-grade patterns observed

- **Path containment by component-wise `strip_prefix`** — `utils/path_within.rs` survives prefix-confusion attack (`/skills/demo` vs `/skills/demonstration`) by construction.
- **Sorted-acquire pair locks** — `tools/path_locks.rs::lock_path_pair` prevents ABBA at the start of every move/copy.
- **Atomic swap-once counter** — `tools/plan_gate.rs::release` returns whether THIS call did the release, so the session write-through and the "you may now build" message are emitted once, not once per approval the model asks for after the first.
- **Lazy FTS5 with no-op degradation** — `tools/result_store.rs::index` opens the index once via `OnceLock` and returns `None` permanently on failure; ctx_search and the persist path both degrade to no-ops rather than erroring out at the user.
- **Race-free session narrowing** — `ToolResultStore::new` installs the process-wide store; `for_session` clones the shared `inner` and stamps a session-id on the handle. Two concurrent sessions can never read each other's output, and the `ctx_search_cannot_reach_another_sessions_offloaded_output` test pins this.
- **Three-state concurrency claim** — `tools/concurrency.rs` partitions the tool space into read-only / unbounded-mutator / bounded-mutator at the registry level, then the harness scheduler mechanically checks resource overlap. No LLM-judgment inside the scheduler.
- **The 12-step redact→record→deny enforcement of the `confirm_with_memory` path** — `tools/scoped/dispatch.rs::confirm_with_memory` is the single seam that consults the global grant store, the per-session denial ledger, the unattended-run auto-deny, the `BeforeToolCall` hook's `Ask`, and the user-facing `ApprovalRequester`, all keyed on `(tool_name, canonical_args)` so the grant covers exactly the call the user read and approved.
- **RCU-based concurrent mutation** — `tools/registry.rs::register` and `tools/scoped/deferred.rs::undefer` use `ArcSwap::rcu` for atomic check-and-mutate, with the closure's captured flag reset INSIDE the closure so a CAS retry can't report stale success.

## Conclusion

Both modules are well-engineered and match every project redline. One high-severity home-isolation bug was found and fixed (the same wiring deficit was tracked on `paths.rs`'s `HOME_JOIN_PENDING_FIX` list, so the project already knew it; this review turn closed it). The remaining 16 entries on that pending-fix list are out of scope for this batch and out of scope for `src/tools`/`src/utils` — they are in `src/approval`, `src/builtin_tools/`, `src/executor/`, `src/gateway/`, `src/sandbox/`, and `src/acp/`, which are not in this round's module list.
