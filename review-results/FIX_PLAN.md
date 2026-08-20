# Rust-Doctor Fix Plan (2026-08-22)

Source: REVIEW.md in each module dir (logging, loop_graph, looping).
Total findings: 52 (logging 18, loop_graph 22, looping 12).
Strategy: fix high-severity correctness issues + low-cost polish; skip high-cost
refactors that require API changes or DB/schema redesigns.

| ID | Sev | Module | Decision | Notes |
|----|-----|--------|----------|-------|
| logging H1 | high | logging | FIX | set_log_level → Result; propagate to RPC handler |
| logging H2 | high | logging | FIX | Add TEST_LOCK guard + restore on panic |
| logging M1 | med  | logging | FIX | `Box<dyn Error + Send + Sync>` |
| logging M2 | med  | logging | FIX | init_log_level: process all RUST_LOG directives, take last simple level |
| logging M3 | med  | logging | FIX | EnvGuard drop in test |
| logging M4 | med  | logging | FIX | CAS loop in set_log_level |
| logging M5 | med  | logging | FIX | return Result<(), LoggingError> (paired with H1) |
| logging M6 | med  | logging | SKIP | Requires aleph-logging refactor (out of scope) |
| logging M7 | med  | logging | SKIP | Renaming variant — purely cosmetic |
| logging L1 | low  | logging | FIX | Acquire/Release ordering |
| logging L2 | low  | logging | FIX | Once-warn guard for from_u8 invalid |
| logging L3 | low  | logging | FIX | `#[repr(u8)]` (matches atomic backing) |
| logging L4 | low  | logging | FIX | `.trim()` in parse |
| logging L5 | low  | logging | SKIP | Rename `file_appender.rs` → `log_directory.rs` is a 2-touch refactor; defer |
| logging L6 | low  | logging | SKIP | Crate-root re-export churn — public API; defer |
| logging L7 | low  | logging | SKIP | `pub(crate)` vs private — no functional difference |
| logging L8 | low  | logging | FIX | Delete the misleading test (doesn't exercise get_log_directory) |
| logging L9 | low  | logging | FIX | Add table-driven edge-case parse tests |
| loop_graph H1 | high | loop_graph | SKIP | gc write-lock window — requires schema redesign |
| loop_graph H2 | high | loop_graph | FIX | log lifecycle of persister; document supervisor absence |
| loop_graph H3 | high | loop_graph | FIX | gc returns `Result<GcReport, (GcReport, Error)>` to honor doc |
| loop_graph M1 | med  | loop_graph | FIX | upsert_node refuses kind change (extends existing strictness) |
| loop_graph M2 | med  | loop_graph | SKIP | Per-turn full-graph read — would need cache wiring |
| loop_graph M3 | med  | loop_graph | FIX | Drop unused `_nodes` parameter |
| loop_graph M4 | med  | loop_graph | FIX | `governance_chain_anchored` empty-graph semantics |
| loop_graph M5 | med  | loop_graph | SKIP | typed LintFinding enum is a multi-module refactor |
| loop_graph M6 | med  | loop_graph | FIX | render_session_topology uses raw-column reads |
| loop_graph M7 | med  | loop_graph | SKIP | multi-owner semantics change requires design discussion |
| loop_graph M8 | med  | loop_graph | SKIP | lock-semantics comment fix only (no behavior change) |
| loop_graph L (selected) | low | loop_graph | FIX | BUS_CAPACITY as `pub const`, Lagged metric, recent_events paging, one_line `\r`, doc pin |
| looping H1 | high | looping | FIX | stop_all routes through transition (or inlines refund_iteration) |
| looping H2 | high | looping | FIX | try_claim_tick: now_ms==0 falls through to regular claim path |
| looping M1 | med  | looping | FIX | pause_all_owned_by: snapshot+single-lock atomic batch |
| looping M2 | med  | looping | FIX | Surface "budget unenforced" in human_summary when baseline absent |
| looping M3 | med  | looping | SKIP | tick_prompt quota — saturating_sub safety is enough; ordering is documented |
| looping M4 | med  | looping | SKIP | tick_prompt over-conservative — doc already calls it out |
| looping M5 | med  | looping | SKIP | eviction policy — bounded but low priority; add doc note |

## Skipped rationale summary

- Anything that requires **changing public API surface** or **design discussions** (multi-owner semantics, gc schema, eviction policy, aleph-logging refactor, module renames) is deferred.
- Anything that is **cosmetic** without changing correctness (variant renames, repr tags, visibility) is deferred unless trivial.
- Anything that requires **large new tests or refactors** (typed LintFinding enum, render caching, concurrent stress tests) is deferred.

## Commit cadence

1. `logging: review round 3 (H1+H2 + medium correctness + low polish)`
2. `loop_graph: review round 3 (H2+H3 + selected medium + low polish)`
3. `looping: review round 3 (H1+H2 + atomic pause + budget unenforced surfacing)`

Each commit scoped to one module's REVIEW.md findings.