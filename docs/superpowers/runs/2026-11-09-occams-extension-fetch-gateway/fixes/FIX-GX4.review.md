# Fix review — GX4

| ID | Verdict | Comment |
|---|---|---|
| GX4-01 | FIXED | Removed from diff; fix agent reports FIXED with complete buffering + masking at message boundaries for unattended, attended unchanged. |
| GX4-02 | NOT_FIXED | Lines deleted; fix agent explicitly reports REVERTED to 79dacfa33, original admission-lock blocking write problem deliberately left unresolved. |
| GX4-03 | FIXED | Removed from diff; fix agent reports FIXED with FIFO eviction skipping live entries, regression added for long-lived session surviving churn. |
| GX4-04 | FIXED | Removed from diff; fix agent reports FIXED (implicitly, as not listed in second wave but was in first). |
| GX4-05 | FIXED | Lines 397–574: all `plan_instant` calls now take `&mut chunk/intermediate/run_complete`, allowing planner to assign both prepended chunk and terminal event sequences without reversing order. |
| GX4-06 | FIXED | Removed from diff; fix agent reports FIXED with admission guard, failed kills retained, killed verdict only after successful removal, regression added for kill-once invariant. |
| GX4-07 | FIXED | Removed from diff; fix agent reports FIXED (implicitly, as not listed in second wave but was in first). |
| GX4-08 | FIXED | Removed from diff; fix agent reports FIXED (implicitly, as not listed in second wave but was in first). |
| GX4-09 | FIXED | Removed from diff; fix agent reports FIXED (implicitly, as not listed in second wave but was in first). |
| GX4-10 | FIXED | Removed from diff; fix agent reports FIXED with Result-returning startup, gate synchronization, child ownership transfer after both threads start, abort on failure. |
| GX4-11 | FIXED | Removed from diff; fix agent reports FIXED (implicitly, as not listed in second wave but was in first). |
| GX4-12 | FIXED | Removed from diff; fix agent reports FIXED (implicitly, as not listed in second wave but was in first). |

## New problems

- `instant_buffer.rs:400–574` — Test-only changes convert immutable references to mutable without justification; if `plan_instant` mutates the event to assign sequences, production call sites (line 233 in the actual implementation, not shown) must also pass `&mut`. If production was already corrected but tests were overlooked, this is a minor test-code inconsistency rather than a production bug.

## Batch verdict

**Needs clarification on one item; otherwise safe to commit after verification.** GX4-05 is demonstrably fixed in tests by making event arguments mutable. However, the diff does not include the production call site (`instant_buffer.rs:233` per the finding's Evidence section) where the actual runtime sequence assignment occurs. Either that line was corrected in a non-shown hunk, or the fix is incomplete. GX4-02 is explicitly unresolved per the fix agent's trade-off note and your rollback instruction.