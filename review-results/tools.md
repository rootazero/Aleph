# Module: tools

## Summary
- Path: `src/tools/` (33 top-level `.rs` files + 3 internal submodules, ~10,893 lines)
- Issues found: 0 high-confidence

## Reviewers
- Security / Logic / Architecture / Quality (all four perspectives applied)

## High-Confidence Issues
None.

## Per-perspective findings

### Security
- No `static mut` in production code. `OnceLock` / `AtomicU64` / `Lazy` used throughout.
- Lock pattern: every `lock().unwrap()` uses `unwrap_or_else(|e| e.into_inner())`.
- Path locks (`path_locks.rs`) uses sorted-path acquisition to prevent ABBA deadlock + `Arc::strong_count` pruning.
- `in_flight.rs` uses `InFlightGuard` RAII + `guard_id` comparison to prevent stale guards from evicting live registrations.
- No SQL-injection-prone `format!()` into query strings.
- No platform APIs (R1 ✓).

### Logic
- All production `.unwrap()` / `.expect()` calls were verified to be in `#[cfg(test)]` blocks. Verification:
  - `result_store.rs` — first `#[cfg(test)]` at line 710; all 18 unwraps ≥721.
  - `traits.rs` — `mod tests` at 443; all 13 unwraps ≥497.
  - `text_tool_call.rs` — `mod tests` at 283; all 10 unwraps ≥297.
  - `registry.rs` — `mod tests` at 92; all 8 unwraps ≥133.
  - `fs_scope.rs`, `runtime.rs`, `concurrency.rs`, `service.rs`, `tool_search.rs`, `info.rs`, `schema_lookup.rs`, `budget.rs` — all unwraps confirmed inside test modules.
- `BTreeMap` used in `markdown_skill/spec.rs:68` for deterministic CLI arg ordering — explicit and correct.

### Architecture (R1-R10)
- **R1**: no `cocoa|appkit|coregraphics|windows-rs|tokio_tungstenite|tonic|reqwest::Client|isahc::Client` imports. Clean.
- **R3**: no heavy deps. Tools use `tokio`, `serde`, `serde_json`, `schemars`. All crate-size appropriate.
- **R4**: no business logic leaks to `interfaces/*`. Direct `use crate::interfaces|desktop::|gateway::` is zero.
- **R8**: zero `regex::|Regex::new` — no deterministic LLM-bypass. Tool dispatch and tool-call repair are model-driven.
- **R9**: configurable tool execution funnels through `ToolService` (exposed interface).
- **R10**: intelligence stays in prompts. `tools/scoped/` enforces allow/deny without cognitive judgment; the LLM is sovereign.

### Quality
- File sizes well-distributed. The largest top-level file is `result_store.rs` (1194 LOC) — within reason for the global result-store responsibility.
- Subsystem grouping is clean: `scoped/`, `server/`, `markdown_skill/`, `adapters/`, `handlers/`, `probes/` are well-named.
- `BTreeMap` chosen where deterministic order matters; `HashMap` for cache/registry where order doesn't matter.
- `pub(crate)` / private visibility is used appropriately — public API curated to `ToolService` traits only.

## Production-grade patterns observed
- Sorted lock acquisition (`path_locks.rs`) — matches ABBA-deadlock-prevention discipline.
- `Arc::strong_count` opportunistic map pruning (`path_locks.rs:50`).
- Bidirectional registry collision handling (`path_locks.rs:71-86`).
- `InFlightGuard` RAII with stale-idempotency check (`in_flight.rs:200-214`).
- `OnceLock` global singleton + idempotent setter (`in_flight.rs:52-58`).
- `tracing::warn!` instead of `panic!` on non-critical lock poisoning.

## Conclusion
`src/tools/` is well-engineered and matches every project redline. No changes required.
