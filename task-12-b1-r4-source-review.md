# Task 12-B1 R4 Source Review

## SPEC: PASS

The reviewed test setup matches the stated R4 contract and preserves strict assertions:

- The attached-queue cohort test uses the real `register_pair_for_test` publication, applies the actual publication, and asserts the initial snapshot plus both real member changes and cursor state.
- The overflow test performs real fill mutations and applies each publication before the real two-member cohort. It asserts invalidation followed by one replacement snapshot, exact cursor, exact membership presence, and absence of partial change events.
- The overtaken-publication test creates two real registry publications, applies the first, then replays it. It asserts the truthful latest resync shape, including invalidation, replacement snapshot, cursor `2`, and both entries.
- The gated invalidation test performs causal setup through the real applier after each mutation. It retains strict pre-gate `replacement_count == 0`, post-enqueue `replacement_count == 1` / `applied_replacement_count == 0`, `applied_invalidation_count == 1`, cleared current snapshot, `None` cursor receipt, and empty ID receipt. After release it requires exact applied counts, exact replacement cursor, exact post-mutation ID set, exclusion of the initial ID, inclusion of both post-mutation IDs, and equality with the applied snapshot.

No assertion weakening, fabricated counter increment, production-worker disablement, or receipt spoofing is present in the reviewed setup. The gate is exercised against the real default applier and production invalidation path.

## QUALITY: PASS

The repair is causality-preserving: setup publications are consumed before the explicit gate, preventing incidental overtaken resync from contaminating the intended assertion. The replacement payload is derived by production `invalidate_all`; the expected cursor and membership are independently obtained from the registry state after release and compared by exact equality. The tests therefore distinguish source enqueue state, invalidation consumption, replacement consumption, and receipt contents.

No remaining deterministic setup fault was identified in the reviewed ranges. The five reported R4 gates passing is consistent with the source-level review.

## Limits

This is a targeted read-only review of the requested test ranges only. It does not claim runtime correctness beyond the exercised tests, does not re-audit production implementation, and does not assess unrelated files or the unavailable task brief.