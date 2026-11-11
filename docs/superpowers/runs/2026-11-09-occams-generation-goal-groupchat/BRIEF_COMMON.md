# BRIEF_COMMON — house contract for the 2026-11-09 occams review (generation / goal / group_chat)

## What this round is

A **static** review (Occam's Razor skill) of three modules, no diff involved: `src/generation`, `src/goal`, `src/group_chat`
(91 files / 33,406 LOC). Findings are fixed on `main` afterwards, in commits, **no PR**.

Two model tiers are used:
- **Jev Tier (Cheapest/Flash role: `minimax-cn/MiniMax-M3.1-Flash` / `flash_lite`)**:
  - Step 1: Candidate sweep & smell extraction (pattern-based sweep over files).
  - Step 2: Classification (categorizing findings into Dead Code, Redundant Allocation, Complexity, Control Flow, Error Handling).
  - Step 3: Triage & Filtering (discarding style opinions, filtering false positives).
  - Step 4: Routing & Prioritization (assigning FIX-NOW, FIX-CAREFULLY, DEFER, REJECT).
- **Reasoning Tier (`minimax-cn/MiniMax-M3` / `gpt-6.1-sol` / `claude-sonnet-5-5` / Pro subagents)**:
  - Step 5: Deep static review & semantic verification against non-negotiable invariants.
  - Step 6: Cross-model adjudication & refutation.
  - Step 7: Surgical, behavior-preserving fix implementation in the isolated worktree.

## Non-Negotiable Invariants (/rust-occams-razor)

1. **Public API surface**: keep all public types, functions, trait implementations, variants unchanged.
2. **Error semantics**: error variants, conversion boundaries, propagation behavior, and externally observable messages must be preserved.
3. **Ownership/lifetime behavior**: caller-dependent lifetimes, borrows, moves must remain stable.
4. **Send/Sync & Concurrency**: async cancellation safety, lock acquisition/release ordering, no locks held across `.await`.
5. **Ordering & Side Effects**: state-machine transitions, retry semantics, channel ordering, persistence contracts must remain identical.
6. **No cargo during review**: machine memory is constrained. No `cargo check` during review or between batches. Single unified `cargo check` with memory control (`CARGO_BUILD_JOBS=2`) at the end.
7. **Every finding needs a concrete failure mode**: "Trigger -> wrong outcome" with real code paths. No subjective style remarks.
