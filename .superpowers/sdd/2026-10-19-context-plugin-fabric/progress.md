# SDD ledger — plan: docs/superpowers/plans/2026-10-19-context-plugin-fabric.md

## Preflight scan

### Shared files and interfaces

| Tasks | Shared surface | Finding | Ruling |
|---|---|---|---|
| 1 / 2 | `src/extension/capability.rs`, `ContextCapabilityDescriptor` | Task 1 defines the descriptor; Task 2 adds registry/grant ownership. | Task 1 owns wire types; Task 2 consumes them and may add only registration/permission mapping. |
| 1 / 3 | `src/context/fabric.rs`, phase and policy types | Task 3 assembles the facade from Task 1 types. | Task 1 lands stable serde names first; Task 3 cannot invent parallel DTOs. |
| 1 / 6 | descriptor and external adapter contract | Task 6 consumes the protocol for fixture transport. | Task 6 must use Task 1 JSON shape and adapter only; no transport-specific descriptor copy. |
| 2 / 3 | registry owner resolution and fallback | Task 3 needs Task 2 grant/owner validation. | Task 3 calls registry APIs; it does not duplicate owner conflict logic. |
| 2 / 5 | extension lifecycle/disposal and grants | Task 5 emits lifecycle events and Task 2 owns plugin registration. | Task 5 uses existing disposal/registry APIs and does not create a second plugin registry. |
| 2 / 6 | adapter grants, bundle owner, side-effect permissions | Task 6 exercises Task 2 semantics. | Task 6 is blocked until Task 2 tests prove grant denial and atomic bundle behavior. |
| 3 / 5 | `ContextFabric`, `HarnessDeps`, fallback/cancellation | Task 5 injects the Task 3 facade into both harness assembly paths. | Task 3 stabilizes the facade first; Task 5 owns the harness seam and both assembly sites atomically. |
| 3 / 6 | Admission output contract and external adapter | Task 6 supplies the external implementation tested by Task 3 fallback. | Task 3 owns contract enforcement/fallback; Task 6 owns adapter behavior; failure paths must be tested at both boundaries. |
| 3 / 4 | compaction/fabric and recovery | Task 4 adds owner-aware recovery consumed by Task 3 facade. | Task 4 owns fold/recovery wire and routing; Task 3 exposes it through the facade without moving recovery into `builtin_tools` dependency direction. |
| 4 / 5 | lifecycle `fold_recorded` and owner provenance | Task 4 emits fold provenance; Task 5 emits lifecycle notification. | Event persistence remains Task 4; lifecycle notification remains Task 5; no second durable event. |
| 5 / 6 | extension lifecycle and adapter shutdown | Task 6 registers external adapter disposal through Task 5 lifecycle seams. | Task 6 uses the existing disposer; Task 5 remains owner of event ordering and cancellation. |
| 6 / 7 | descriptor and final docs | Task 7 documents final symbols after Task 6. | Task 7 is last and uses actual final code symbols, not planned names that changed. |

### Task self-consistency

| Task | Internal scan | Ruling |
|---|---|---|
| 1 | Creates `fabric.rs`, mounts it, tests the same wire types; preserves native static stage name. | Consistent. |
| 2 | Registry/grant tests cover every produced registration rule; disabling cleanup matches bundle API. | Consistent. |
| 3 | Facade, preflight registration, Admission fallback and tests target the same policy boundary; existing `pub(crate)` setters remain in-crate. | Consistent. |
| 4 | Event field, fold record, manual emission and decompression routing form one owner-aware path; legacy `None` behavior is explicit. | Consistent. |
| 5 | Harness changes are limited to the two duplicated assembly sites plus lifecycle hooks; regression test targets the documented missing-spawner failure. | Consistent, subject to R10 file-count check before edits. |
| 6 | Adapter fixture consumes descriptor/grant and cache tests prove derived-state rebuild; side-effect permissions are tested at the adapter boundary. | Consistent. |
| 7 | Docs are last and derive facts from final symbols; prompt-size check is conditional on descriptor/tool text changes. | Consistent. |

## Rulings

- Ruling: Keep `PreflightStage::name(&self) -> &'static str` unchanged; dynamic plugin identity belongs to `ContextCapabilityDescriptor.id` and registry keys. This preserves native ABI and avoids forcing a Rust trait change across all builtin stages; the cost if wrong is an adapter layer that may need later simplification.
- Ruling: Represent legacy fold ownership as `owner = None` meaning builtin. This preserves old sessions and lets Core route mixed histories; the cost if wrong is a durable event migration later.
- Ruling: Treat Task 3's facade injection change and Task 6's cache change as serialized even where both touch compaction code. This lowers parallelism but avoids silently crossing the fallback/cache contract.
- Ruling: Treat `src/harness/` edits as allowed only after the facade exists and the 12-file ratchet is re-counted. This preserves R10; the cost if wrong is one extra integration checkpoint.

## Current continuation checkpoint / 当前续作检查点

- Task3 facade integrated in parent at `bda8c05cc`; native fallback, rollback, protected window and measured results use the accepted runtime seam. Earlier candidate agents are no longer running.
- Task5a code and static re-review complete; slot `ai_reasoning` 27 and `ai_compat` 12 tests passed, Core execution still pending. `ProviderResponse` is a local DTO with no serde derive; the optional request-bound replay policy is not a plugin wire field.
- Task5b root/child/deps/Think/directive/fit/rescue wiring is on disk. Independent reviewer `4d4704f7-9ed2-4da` found proactive fresh_tail=0; parent fixed it to budget.fresh_tail_count() before await (fallback six). Remaining reviewed boundaries had no new Important findings. Harness measured 5528 lines across 12 files against ceiling5545; repeat after final format.
- Task6a/6b actual-mounted WASM discovery, identity binding, explicit grant, resolver, blocking permits and lifecycle disposal are on disk. `ContextHandler::invoke_authenticated` is wired; context_plugin canonical-confirmation grant/revoke and plugin_manage context_status are reachable. RegistryToolAdapter's separate confirmation list includes context_plugin. Static reviewer `80456ee7-01bc-468` is running.
- Main added actual-session task-local Admission at ScopedToolService pre-ingress and manual compaction facade routing, preserving exact-read/no-budget behavior and original result storage. Owner commits atomically with the existing FoldRecorded transaction.
- Lifecycle worker `4e929c50-1c3d-4a1` owns only wire Notify/runtime before-after events/new lifecycle tests, no cargo/commit. Parent will wire TurnEnd/FoldRecorded/RestoreRequested. Do not substitute per-run boundaries for durable SessionStart/SessionShutdown.
- First integrated check used an old trait/test AST: E0407 invoke_authenticated, missing degraded fields, text_only(&str), Vec<UnifiedMessage> equality. All identified sites repaired. Stale own process tree PID6196 stopped; this is NOT a successful check. Build slot now free; next batch must use fresh output.
- Shared target remains parent target; jobs4/debug0; check AvailablePhysicalMemory>=4194304KiB before every cargo; do not kill unrelated workspace builds.
- New reference CONTEXT_PLUGINS.md and updates FEATURE_LOCATOR/PLUGIN_SYSTEM/AGENT_LOOP_CONTEXT_BUDGET are on disk. Final tests and status markers pending. No performance reduction percentages claimed.

## Task state

- Task1 `c65f50d3a` + `fe4da3fb1`; canonical digest-boundary test correction in `535195c18`, pending next batch.
- Task2 `a97dc0f52`, independent review APPROVED; registry 26 + capability 8 tests passed.
- Task3 production integrated `bda8c05cc`; post-integration focused regression batch pending.
- Task4 `b30a9c778`, independent review APPROVED; folds9/manual23/ledger5/decompress9/diagnostics4/session-events31 passed.
- Task5a/b and actual ingress/manual integration implemented, Core batch and lifecycle completion pending.
- Task6 actual WASM mount/grant/resolver/tool implemented, static review and executable fixture verification pending. Historical foreign-owner restoration remains explicitly degraded Core recovery; no raw history sent to a newly selected owner. No new plugin disk cache/network/write/cross-session authorization.
- Task7 reference updates drafted; final static check, meaningful focused tests, clippy/fmt/R10 and final commits remain.
