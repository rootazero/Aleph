# Review Summary

**Date**: 2026-07-19
**Modules reviewed**: 16 (all under `src/`)
**Threshold**: confidence >= 80 (all reported findings considered actionable)

## Module Totals

| Module | Files | Lines | Critical | High | Medium | Low | Total |
|--------|------:|------:|---------:|-----:|-------:|----:|------:|
| core | 2 | 146 | 0 | 0 | 0 | 0 | 0 |
| init_unified | 3 | 591 | 0 | 0 | 1 | 2 | 3 |
| domain | 2 | 940 | 0 | 0 | 0 | 6 | 6 |
| guardrails | 9 | 1269 | 0 | 0 | 1 | 1 | 2 |
| discovery | 4 | 1298 | 0 | 2 | 9 | 4 | 15 |
| components | 20 | 1686 | 0 | 5 | 6 | 8 | 19 |
| group_chat | 8 | 2715 | 0 | 0 | 4 | 11 | 15 |
| exec | 17 | 3515 | 0 | 1 | 5 | 12 | 18 |
| tool_metadata | 27 | 5990 | 0 | 0 | 3 | 14 | 17 |
| executor | 21 | 8352 | 0 | 4 | 7 | 13 | 24 |
| context | 24 | 10033 | 0 | 7 | 8 | 12 | 27 |
| config | 98 | 23819 | 0 | 1 | 9 | 25 | 35 |
| generation | 79 | 20386 | 0 | 0 | 5 | 8 | 13 |
| harness | 26 | 16600 | 0 | 0 | 1 | 5 | 6 |
| extension | 70 | 21608 | 0 | 0 | 3 | 12 | 15 |
| gateway (messaging/lanes) | 13 | ~190k | 1 | 8 | 11 | 3 | 23 |
| gateway (sessions/routing) | ~40 | ~120k | 0 | 0 | 3 | 17 | 20 |
| gateway (handlers/interfaces) | ~470 | ~156k | 0 | 0 | 17 | 100+ | 130+ |
| gateway (misc top-level) | ~50 | ~250k | 0 | 0 | 1 | 9 | 10 |
| **TOTAL** | — | — | **1** | **28** | **83** | **260+** | **~380** |

## Top Priorities (Critical + High)

1. **gateway/channel_approval.rs:147** — critical — approval bypass (operator auth not enforced)
2. **gateway/channel_registry.rs:570** — high — RecvError::Lagged kills channel forever
3. **gateway/channel_registry.rs:455** — high — channel write-lock held across I/O
4. **gateway/channel_registry.rs:220** — high — re-registration silently drops old channel
5. **gateway/delivery_queue.rs:352** — high — non-atomic claim causes duplicate sends
6. **gateway/delivery_queue.rs:667,709** — high — duplicate delivery windows
7. **gateway/coalescer.rs:113** — high — group-chat sender attribution lost
8. **gateway/channel_policy.rs:110** — high — snapshot timeout ignored
9. **context/budget/mod.rs:300** — high — R10 violation (diminishing-returns heuristic)
10. **context/compact/tool_aware_chunker.rs:50** — high — usize overflow on token_ratio
11. **context/compact/compactor.rs:528,1016** — high — non-truncating fallback for long lines
12. **context/compact/summary_utils.rs:160** — high — prompt injection via transcript
13. **context/compact/session_split.rs:117** — high — non-atomic session split
14. **context/retrieval/content_index.rs:382** — high — non-transactional FTS cleanup
15. **executor/builtin_registry/builder/constructor/mod.rs:249** — high — R1 violation (Core instantiates platform code)
16. **executor/builtin_registry/builder/constructor/mod.rs:849** — high — caller identity frozen at registry construction
17. **executor/builtin_registry/registry/inherent.rs:38** — high — caller identity race
18. **executor/tool_registry.rs:68** — high — shared ToolContextHandle causes workspace crosstalk
19. **config/backup.rs:46** — high — backup dir falls back to "."
20. **exec/approval/types.rs:30** — high — ApprovalRequest naming collision
21. **context/compact/compactor.rs:1016** — see #11
22. **context/budget/mod.rs:300** — see #9
23. **discovery/paths.rs:90** — high — find_git_root duplicated with utils/paths.rs
24. **discovery/scanner.rs:368** — high — project vs global plugins share priority 10
25. **components/*** (5 high) — dead code from removed EventHandler chain

## Architecture Compliance Snapshot

- **R1** (no platform APIs in core): 1 violation in `executor/builtin_registry/builder/constructor/mod.rs:249` (instantiates `aleph_desktop_macos::MacOSPlatform::new` etc.)
- **R3** (no heavy deps for non-core): clean
- **R4** (no business logic in interfaces): clean
- **R8** (regex not for LLM reasoning): 2 violations — `tool_metadata/risk.rs` (intent classification), `context/budget/mod.rs` (diminishing-returns heuristic), `gateway/i18n.rs:format_execution_error` (over-broad keyword match)
- **R10** (intelligence in prompts): see context/budget/mod.rs:300

## Categories Summary

- **Dead code**: ~25 findings (components, discovery, gateway/run_event_bus, rate_limiter, etc.)
- **DRY violations**: ~30 findings
- **`lock().unwrap()` poisoned mutex hazards**: 25+ in gateway (security/store, surface/r5_router, execution_engine/unattended_redacting_sink)
- **`.ok()` silent error suppression**: pervasive in gateway/handlers (~80 instances)
- **`unwrap_or(usize::MAX)` / clock-skew**: ~15 sites in gateway handlers
- **Function length >50 lines**: ~15 findings (already acknowledged in harness/CLAUDE.md)
- **`pub` where `pub(crate)` suffices**: ~8 findings
- **Visibility & consistency**: ~30 minor findings