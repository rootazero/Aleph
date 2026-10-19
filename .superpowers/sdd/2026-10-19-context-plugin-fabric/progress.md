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

## Task state

- Task 1: `c65f50d3a`; canonical payload verification `fe4da3fb1`. Context batch: 41 fabric tests passed, one old digest-boundary assertion failed; assertion corrected to reject a noncanonical digest, pending the next shared test run.
- Task 2: `a97dc0f52`, final independent review APPROVED. Accepted: all 26 registry tests plus 8 extension capability tests passed. Multiple-session grants, per-session owners, bounded audit and descriptor-bound consent implemented.
- Task 3: candidate implementer `0e22ddae-5e48-406` works on isolated branch `codex/context-fabric-runtime`, worktree `D:/Workspace/Aleph/.worktrees/context-fabric-runtime`, from `b30a9c778`. No cargo there. Parent targeted registry/fold verification is complete; candidate review and integrated facade verification are pending.
- Task 4: `b30a9c778`, independent review APPROVED. Accepted: folds 9, manual compaction 23, ledger 5, decompression 9, diagnostics 4 and session events 31 tests passed.
- Task 5: pending. R10 baseline is demonstrably 5570 lines against CEILING 5545 both at `79a2494f6` and after Task 4: zero new budgeted lines. Fix the existing 25-line overrun by genuine scaffolding extraction when integrating the facade; do not raise the ceiling.
- Task 6: pending
- Task 7: pending
