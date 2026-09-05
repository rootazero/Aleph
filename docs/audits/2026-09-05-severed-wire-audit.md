# Severed Wire Audit — 2026-09-05

Static code review across 5 modules (no cargo check during review; fixes applied in worktree).

Reviewed: src/metrics, src/pii, src/orchestrator, src/providers, src/resilience
Reviewers: 6 parallel subagents (general-purpose)

## Summary

| Module | Critical | Important | Minor | Health |
|--------|---------:|----------:|------:|:-------|
| metrics | 0 | 3 | 7 | yellow |
| pii | 2 | 8 | 16 | yellow |
| resilience | 4 | 8 | 11 | yellow |
| orchestrator | 4 | 8 | 14 | yellow |
| providers (top) | 5 | 7 | 10 | yellow |
| providers (subdirs) | 1 | 11 | 24 | yellow |
| **Total** | **16** | **45** | **82** | — |

## Critical Findings (must fix)

### src/pii
- `src/pii/engine.rs:11-15` — `BUILTIN_RULE_COUNT = 7` duplicates literal list in `src/pii/rules/mod.rs:46-54`; operator-facing partial-load warning mis-counts after refactors.
- `src/pii/rules/id_card.rs:136-145` — IdCardRule lacks `is_decimal_context` / `is_hex_bounded` guards; JSON floats/version literals false-positive on 18-digit pattern.

### src/resilience
- `src/resilience/database/state_database/mod.rs:34-43` — `unsafe` transmute of `sqlite3_vec_init` signature erases rust-bindgen contract; signature drift breaks stack.
- `src/resilience/database/traces.rs:293,309; tasks.rs:319; memory_events.rs:334,360,401` — `as u64` on SQLite i64 silently truncates negative sums to `u64::MAX`.
- `src/resilience/database/state_database/mod.rs:215-222` — poisoned-mutex recovery reuses a half-broken connection; can commit inconsistent data.
- `src/resilience/database/tasks.rs:163-234` — `update_task_status` leaks side `execute` side-effects on the not-found path before transaction commit/rollback decision.

### src/orchestrator
- `src/orchestrator/dispatch.rs:1072-1148` — `tokio::spawn` discards `JoinHandle`; harness panics leave `FlowHandle` forever pending.
- `src/orchestrator/dispatch.rs:953-963` — `parent_session_key` is computed but never persisted; child sessions lose their parent link.
- `src/orchestrator/loader.rs:38-67` — `load_catalog` vs `reload_flows` race: two callers can wipe each other's user-flow sets.
- `src/orchestrator/dispatch.rs:601-616,608-616` — `SessionLockGuard::drop` acquires std `Mutex` during stack unwinding; can deadlock under panic.

### src/providers (top-level)
- `src/providers/mod.rs:155-179` — Ollama and Mock bypass the full safety pipeline (PII, metering, PostApiRequest hook).
- `src/providers/registry.rs:36-66` — `ProviderRegistry` uses `&mut self + HashMap`; concurrent `register` panic/corruption.
- `src/providers/http_provider.rs:18-32` — `is_stale_encrypted_reasoning_error` substring match strips ALL thinking signatures from history on a single stale blob.
- `src/providers/load_stats.rs:106-130` — `RateWindow::roll` reset is two separate `Ordering::Relaxed` stores; a minute of RPM/TPM can silently zero out.
- `src/providers/mod.rs:177-179` — Mock provider hardcodes `"Mock response"` text and ignores request payload.

### src/providers (subdirs)
- `src/providers/protocols/openai_chat/proto_impl.rs:45-52` — Only OpenAI Chat adapter returns the unvalidated `raw_base_url` on `validate_provider_base_url` rejection; siblings log+continue.

## Important Findings (should fix — selection)

- `src/pii/allowlist.rs:69-81` — allowlist dispatch is hardcoded by rule name; custom rules have no allowlist escape hatch.
- `src/pii/engine.rs:215-228` — duplicate custom rule names silently diverge between rules and `custom_rule_actions`.
- `src/pii/rules/api_key.rs:42-66` — unbounded `{20,}` / `{36,}` quantifiers; pathological input → huge `PiiMatch.matched_text`.
- `src/resilience/database/state_database/mod.rs:475-525` — `store_sticker_description` is a Telegram-specific method on the core DB surface.
- `src/resilience/database/memory_events.rs:194-226` — query-shape scattered; adding a column requires touching many functions.
- `src/resilience/database/migration.rs:281-294` — `LIKE '%UNIQUE%task_id%step_index%'` order-sensitive constraint detection.
- `src/orchestrator/runner_impl.rs:1410-1411` — `CALIBRATION_CARRYOVER` is a process-global single-slot Mutex; concurrent runs on different models race.
- `src/orchestrator/harness_bridge/error.rs:31-34` — message-based transient classification is fragile.
- `src/providers/health.rs:48-77` — 400-class errors never trip the circuit breaker.
- `src/providers/http_provider.rs:828-868` — `PostApiRequest` hook skipped on aborted streams → metering misses aborted turns.
- `src/providers/route_witness.rs:393-396` — global `WITNESSES` write-locked on hot path; serializes concurrent turns.
- `src/providers/route_handle.rs:113-126` — `global_route_handle` bypasses `CapabilitySlot` census; exemption is comment-only.
- `src/providers/retry.rs` — half-empty module kept "to avoid a second answer".
- `src/providers/protocols/anthropic.rs:19` — `CLAUDE_CODE_USER_AGENT` hardcoded version.
- `src/providers/protocols/anthropic/adapter.rs:428` — `name_map` write lock held across O(tools) work per request.
- `src/providers/moa/provider.rs:495-500` — `debug_assert_eq!` on cache invariant disabled in release.
- `src/providers/moa/provider.rs` — no `tracing` imports; MoA turn invisible to log monitoring.
- `src/providers/model_catalog/alias.rs:296-330` — substring matching for `canonical_provider_id`; "anthropic" matches "anthropomorphic-key".
- `src/providers/model_behaviors/mod.rs:87-110` — substring matching for `vendor_identity`.

## Minor Findings (selection)

Skipped for size; full per-file notes retained by the review subagents.
