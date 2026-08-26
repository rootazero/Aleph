# Logic Review Report — src/context
**Mode**: --strict
**Files audited**: 24 files
**Date**: 2026-08-26

(Full report captured from subagent 336f6690-63b1-48a)

## Summary
| Critical | Warning | Suggested Test |
|----------|---------|----------------|
| 3 (F1, F2, F3) | 12 (F4–F15) | 2 (F16, F17) |

## Critical findings
- **F1** — `before_turn` returns `CompactToFit` on zero budget; calibration divergence causes infinite trip (`src/context/budget/mod.rs::ContextPressure::compute` 76–119, `ContextBudget::before_turn` 363–454)
- **F2** — `fit::compact_to_fit` calibrated-space floor guard can leave ratio `>= critical` on a `< critical` raw total (`src/context/compact/fit.rs::compact_to_fit` 156–172)
- **F3** — `preflight::run` mutates `*messages` only on commit; "discarded pass leaves messages untouched" can spuriously rewrite when `min_savings_ratio == 0` (default) (`src/context/budget/preflight.rs::PreflightPipeline::run` 215–274)

## Warning findings (12)
F4–F15 covering: borrow aliasing in file_op_supersede (F4), directive fallback (F5), rescue streaming contract (F6), preventive floor (F7), compactor transient_tail docs gap (F8), manual compaction panic recovery (F9), ContentIndex unbounded LIMIT (F10), observe_actual_usage non-obvious correctness (F11), ContentIndex reopen invariant undocumented (F12), truncate_to_fit floor eviction (F13), pre-scope table whitelist (F14), call_llm transient string-match (F15).

## Suggested tests (2)
- F16: trailing transient message survives LLM summary path
- F17: zero-message preflight zero-alloc

## Wiring gaps
- `ContentIndex::clear` — orphan public method with claim of "denial circuit-breaker" use site NOT FOUND. NEEDS VERIFICATION.
- `ContentIndex::search_sessions`, `len_sessions`, `list_sessions` — orphan public methods
- `ContextBudget::seed_calibration` — public surface for future caller, no production caller

## Notes
- All `Mutex/RwLock/Arc/atomic` imports come from `crate::sync_primitives`. Verified.
- No lock held across `.await` for sync mutexes. Verified.
- No `usize as u32` truncations found.
- Cheap-pass order (file_op → pruning → image) verified correct against docs.
- The system prompt invariant is defended by design (system prompt is NOT in `messages`).
---

# Logic Review Report — src/config (core)
**Mode**: --strict
**Files audited**: 35 files
**Date**: 2026-08-26

## Summary
| Critical | Warning | Suggested Test |
|----------|---------|----------------|
| 3 | 17 | 4 |

## Critical findings
- **C1** — `[security.ssrf] strip_auth_on_cross_origin` silently dropped by `apply_security_ssrf_overrides` (`src/config/load.rs:323-392`, `src/security/ssrf/policy.rs:18-39`). Bridge handles only 5 of 6 fields.
- **C2** — `merge_builtin_rules` is a silent no-op despite being called from loader (`src/config/load.rs:200, 310-316`). Body is `debug!` + return. Misleading name.
- **C3** — `classify_verified` re-exported but no production caller (`src/config/mod.rs:24`, `src/config/live_apply.rs:170-180`). WIRING GAP.

## Warning findings (17, abbreviated)
- W1 `guard_against_section_loss` substring search false-positives on comments (`save.rs:67-79`)
- W2 `merge_sections` partial mutation before Err (`save.rs:174-240`)
- W3 `save_incremental_to_file` no conflict detection — 35+ direct callers race (`save.rs:393-469`)
- W4 patcher `validate_candidate` runs on no-op patches (`patcher.rs:264-275, 384-400`)
- W5 `apply_security_ssrf_overrides` re-parses raw TOML silently (`load.rs:325-329`)
- W6 `validate_default_provider` accepts disabled provider (`validate.rs:75-90`)
- W7 `validate_search_config` ignores disabled backends (`validate.rs:254-371`)
- W8 `Config::save()` only internally called but `pub` (`save.rs:391-401`)
- W9 `merge_builtin_rules` doc comment misleading (`load.rs:310-316`)
- W10 `Config::load` order asymmetric between file vs default paths (`load.rs:188-200` vs `295-301`)
- W11 `merge_sections` `let _ = current_table` drop undocumented (`save.rs:208`)
- W12 `live_target_for` substring collision dormant but untested (`reload_impact.rs:174-184`)
- W13 `apply_security_ssrf_overrides` doesn't list skipped fields (`load.rs:323-392`)
- W14 atomic write leaves orphaned `.tmp` on fsync failure (`save.rs:97-120`)
- W15 `apply_security_ssrf_overrides` called before `validate` (`load.rs:182, 223`) — order correct but implicit
- W16 `merge_sections` accepts nonexistent sections — correctly rejected, no action
- W17 `merge_builtin_rules` could be deleted (dead) (`load.rs:310-316`)

## Suggested tests (4)
- T1: ssrf bridge covers every SsrfPolicy field
- T2: guard ignores commented markers
- T3: live_target_for rejects substring collisions
- T4: patcher commit_patch rolls back in-memory on save failure

## Wiring gaps (most important)
- `live_apply::classify_verified` — re-exported but no caller found outside `src/config/` (WIRING GAP)
- `ConfigPatcher::rollback` — only exercised in tests, not wired to RPC

## Save atomicity
- main save path is atomic on POSIX (temp + rename + fsync)
- BUT: fsync failure leaves orphan `.tmp` (W14)
- AND: `save_incremental` has no conflict detection (W3)

---

# Logic Review Report — src/cluster + src/command
**Mode**: --strict
**Files audited**: 9 files (7 cluster + 2 command)
**Date**: 2026-08-26

## Summary
| Module | Critical | Warning | Suggested Test |
|--------|----------|---------|----------------|
| src/cluster | 0 | 8 | 7 |
| src/command | 0 | 4 | 4 |
| **Total** | **0** | **12** | **11** |

## cluster Findings (8 Warnings, no Critical)

| # | Finding | Location | Severity |
|---|---------|----------|----------|
| CW1 | Empty `device_name` passes through to `admit_node` → ghost row | `src/gateway/server/handler.rs:2003-2013` → `src/cluster/enrollment.rs:289` | low |
| CW2 | Short `presented_id` causes fingerprint collision + lost offline row (path 4) | `src/cluster/enrollment.rs:217-247` | low |
| CW3 | `as usize` on `u64` size silently lossy on 32-bit platforms | `src/cluster/node_file_cmd.rs:232` | latent (zero today) |
| CW4 | `static COUNTER` in `request_approval` doesn't reset on reconnect | `src/cluster/node_approval.rs:79-87` | low |
| CW5 | `deregister_node` followed by rapid reconnect can birth a session already revoked | `src/cluster/enrollment.rs:387-413` | low |
| CW6 | `match_id` doc says "≥4 chars" but code uses bytes (CJK) | `src/cluster/registry.rs:310-313` | low |
| CW7 | `now_unix()` silently returns 0 for clock skew before UNIX_EPOCH | `src/cluster/registry.rs:611-614` | very low |
| CW8 | `request_approval` `Ok(_)` non-success maps to Denied without warn | `src/cluster/node_approval.rs:124-125` | very low |

Also a doc-stale warning: `NodeRegistry::resolve_id` doc comment claims `gateway/handlers/cluster.rs:269` calls it but doesn't.

## command Findings (4 Warnings, no Critical)

| # | Finding | Location | Severity |
|---|---------|----------|----------|
| CCW1 | Parser doesn't validate `arguments` field separately | `src/command/parser.rs:95-127` | low |
| CCW2 | Duplicate clone of `command_name`/`tool_id` could combine | `src/command/parser.rs:108-122` | cosmetic |
| CCW3 | `Builtin` variant conflates `Builtin` and `Plugin` sources | `src/command/parser.rs:131-163` | low |
| CCW4 | `Skill::allowed_tools` silently emptied when `routing_capabilities` empty | `src/command/parser.rs:140-147` | low |

## Wiring gaps — VERIFIED NONE
Every public export has at least one caller. SSOT property (single `deregister_node`, `enroll_node_device` shared by RPC + tool) verified by grep.

## Lock audit
- All `Mutex/RwLock/Arc` imports from `crate::sync_primitives` ✓
- No `.lock()` held across `.await` for std::sync::Mutex ✓
- Static atomics use std::sync::atomic (documented exception) ✓
