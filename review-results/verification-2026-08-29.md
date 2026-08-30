# Module: src/verification (review 2026-08-29)

## Summary
- Files: 9 (8 in `src/verification/` + `tests/{mod,stop_hook_verifier,tool_loop_verifier,turn_verifier}.rs`)
- LOC: ~2,904 production + tests (per `wc -l`)
- R1 platform-API isolation: OK (only `tokio` / `async-trait` / `serde`, no `objc`/`windows`/`x11`)
- R7/R10 verifier purity: OK (permanent `JudgeVerifier` prohibition in `mod.rs:21-43`,
  every impl is structural: shell exit code, args-hash repetition, scratchpad
  unchecked-box count, mutation/evidence tool-name set — zero LLM calls, zero
  regex beyond enum-string containment, zero content inspection)
- Wiring: all five verifiers (`StopHookVerifier`, `ExtensionStopHookVerifier`,
  `ToolLoopVerifier`, `ScratchpadGoalVerifier`, `MutationEvidenceVerifier`)
  are registered in `src/bin/aleph-server/commands/start/orchestrator_init.rs:241-262`
  via `VerifierChainBuilder`. No dead `pub fn`s, no `impl`s without consumers.

## High-Confidence Issues

### [medium] `truncate_chars` misnamed: does byte truncation despite char-named signature
- **Location**: `src/verification/extension_stop_gate.rs:104-106`
- **Description**: Private helper delegates to `crate::utils::text_format::truncate_bytes`
  (byte-budget, char-boundary safe) but is named `truncate_chars`. The byte-vs-char
  distinction is exactly the bug the 2026-07-20 review's *"byte-cap env truncate"*
  commit (7f2298813) was created to prevent. Relabelling without behaviour change
  is safe and locks in the post-fix semantics for future readers.
- **Risk / trigger**: A maintainer copying the wrapper into a sibling module could
  misread it as "chars" and re-introduce the cap×3-for-UTF-8 regression that 7f2298813
  fixed in `StopHookVerifier`.
- **Suggested fix**: Rename the wrapper to `truncate_bytes` and call it directly,
  OR inline the call. The call site is private to this module — no API surface impact.

### [medium] `execute_shell_hook` does not reap the child on timeout/cancel — possible zombie
- **Location**: `src/verification/stop_hooks.rs:425-450` (the two timeout/cancel
  arms of the outer `tokio::select!`)
- **Description**: When `tokio::time::sleep(hook.timeout)` or `cancel.cancelled()`
  fires, we drop the inner future (which had `child.wait()` in flight), release
  the &mut borrow, call `child.kill().await`, and return `Error`. We never
  reap: `tokio::process::Child::drop` sends SIGKILL on `kill_on_drop` but does
  **not** call `waitpid`. The process becomes a zombie that persists until the
  parent process reaps it (typically on shutdown). In the normal (non-timeout)
  path we DO `child.wait().await`, which is why this leak only materialises on
  the slow/stalled/cancelled path.
- **Risk / trigger**: A misbehaving hook that hangs beyond `timeout`, or any
  hook cancelled during a long run — leaks one zombie per occurrence. Zombies
  don't consume CPU/memory but they occupy a PID slot; under pathological
  volume (a hook churning every few seconds for an hour) this can hit
  `pid_max` and cause `spawn` to fail with `EAGAIN` — a confusing, indirect
  failure mode.
- **Suggested fix**: After `child.kill().await` in both arms, add
  `let _ = child.wait().await;` to reap the process. The wait result is
  discarded (the verdict is "Error" either way); the side effect (zombie
  reaping) is what matters.

### [medium] Test gap: `MutationEvidenceVerifier` strict `end_turn` check has no negative test
- **Location**: `src/verification/mutation_evidence_verifier.rs:69`
- **Description**: The verifier short-circuits to `Continue` for any
  `stop_reason` value other than `Some(STOP_REASON_END_TURN)` — which is the
  documented design ("only nudge when the model is stopping, not when the loop
  is forced to terminate by `max_loops`/`user_stopped`/etc."). However, every
  existing test in the module exercises either `Some("end_turn")` or `None`.
  Nothing locks down the strict-equality semantics for, say,
  `Some("max_loops")` or `Some("user_stopped")`.
- **Risk / trigger**: A future refactor that loosens the equality to `.is_some()`
  (matching the other stop-only verifiers) would silently start nudging on
  forced terminations. The nudge is non-blocking so harm is small, but the
  intent ("only end_turn is a model-initiated stop") would be lost.
- **Suggested fix**: Add one unit test asserting that
  `MutationEvidenceVerifier::default().verify(ctx, …)` returns `Continue`
  when `stop_reason = Some("max_loops")` even with mutating tool calls in
  the window. Place alongside the existing `stays_silent_mid_turn_and_without_mutations`.

## Per-perspective findings (lower confidence)

### Security & Robustness
- All `Mutex::lock()` calls use `unwrap_or_else(|e| e.into_inner())` consistently
  (`extension_stop_gate.rs:118/124/133`, `mutation_evidence_verifier.rs:107`).
- Lock acquisitions in `ExtensionStopHookVerifier::verify_with_executor`:
  `veto_count` → drop → `record_veto` → drop → `clear` are sequential, not
  nested, so no deadlock risk and no upgrade path.
- `HashMap<String, u32>` in `ExtensionStopHookVerifier.vetoes` is bounded by
  "concurrently wedged sessions" — entries are removed on `Allow`/`Halt` and
  on ceiling breach. Maximum occupancy is the number of sessions currently
  being vetoed. Comment at lines 110-112 documents the invariant.
- `HashSet<String>` in `MutationEvidenceVerifier.nudged` is wholesale-cleared at
  `NUDGED_SESSIONS_CAP = 1024`. The `contains`-check happens BEFORE
  cap-clear+insert, so a session that retries after a wholesale clear still
  gets nudged once. Idempotent and bounded.
- `execute_shell_hook` short-circuits on `is_shell_safe(&hook.command)` before
  spawn — this is the security boundary that protects per-goal gates against
  shell-metacharacter injection from LLM-generated config. The check uses
  char-iteration against a static `SAFE` allowlist; newlines/CRs/redirects/
  subshells are absent. No unicode-bypass possible since any non-ASCII char
  fails the allowlist check.
- `LAST_MESSAGE_ENV_CAP` (4096 bytes) is uniformly byte-truncated through
  `crate::utils::text_format::truncate_bytes` after the 2026-08-25 byte-cap
  fix. No `&s[..n]` panic surface anywhere.
- No `static mut`, no `unwrap` in production paths (`serde_json::to_string(ctx)
  .unwrap_or_else(|_| "{}".to_string())` at `stop_hooks.rs:296` is the only one,
  and it has a fallback that emits `{}`).

### Logic & Correctness
- The verifier chain contract (`first non-`Continue` wins`) is verified by
  `tests/turn_verifier.rs::first_veto_short_circuits_subsequent_verifiers`.
- Tier-1 / Tier-2 / Halt thresholds in `tool_loop_verifier.rs` are sourced from
  `TurnVerifyContext.robustness_profile` at `verify` time. Tests cover the
  news fan-out (high distinctness → Continue), 3-file thrash (low distinctness
  silent → Veto), narrated exploration (narration rescues Tier-2), repeated
  loops (Tier-1 Veto at threshold, Halt at full window). No profile-driven
  pipeline left untested for the conservative profile; `clamped_enforces_window_invariants`
  locks the safety bounds on out-of-range profiles.
- `STOP_HOOK_ACTIVE` semantics: the flag is computed from `prior_vetoes` BEFORE
  `record_veto`, so the hook sees a snapshot from the moment we entered
  `verify_with_executor`. Test `blocking_hook_vetoes_and_sets_active_flag` proves
  the round-trip (first attempt sees `false` and vetoes, second sees `true`
  and the flag-aware hook self-limits).
- `veto_count` ceiling: tests `consecutive_veto_ceiling_unwedges_the_loop` walks
  through exactly `MAX_CONSECUTIVE_STOP_VETOES` (5) successful vetoes, then
  verifies ceiling + reset + post-reset veto re-honoring. Comment at lines
  191-194 explains why the count must be reset (otherwise the next run would
  see a permanently disabled gate).
- `STOP_REASON_END_TURN` is the single source of truth for the producer
  (harness agent loop) and the only consumer that gates on the exact value
  (`MutationEvidenceVerifier`). Documented in `turn_verifier.rs:90` so a
  spelling change in the producer breaks the build rather than the verify.
- `ScratchpadGoalVerifier::veto_reason` — the round-8 fix from
  `9f3a9a96 scratchpad: make "item N" mean one thing on every surface` is in
  place: the veto enumerates `snapshot.items.iter().enumerate().filter(…)`
  (absolute indices) rather than the filtered `incomplete()` sublist. Test
  `the_veto_prints_absolute_item_indices_not_positions_in_the_pending_list`
  locks this down — a regression here would re-introduce the "model completes
  an already-done step because the index it printed was wrong" failure mode.
- `StopHookVerifier` Halt-vs-Block priority: `if let Some(halt) = result.halt_reason()
  { return Halt } else if let Some(block) = result.blocking_reason() { return Veto }
  else { Continue }`. Documented in the trait as "Halt outranks Block —
  claude-code's `preventContinuation` semantics". NOT directly tested in
  `tests/stop_hook_verifier.rs` (the in-process `InProcessHook` only supports
  Allow/Block). Minor test gap — see "low" items below.
- Rerunning the round-8 critical TOCTOU-style concerns: I did NOT find any new
  check-then-act splits on shared mutable state inside this module. The
  `record_veto`/`clear`/`veto_count` callers may briefly observe stale cross-
  call state, but each individual call's read-and-increment is atomic under
  the mutex, and each decides based on its own post-increment observation —
  no read/write race that affects the verdict.

### Architecture (R1-R10)
- **R1**: clean (only tokio / async-trait / serde; no AppKit, Vision, CoreGraphics,
  windows-rs, x11).
- **R3**: `Cargo.toml` (not in scope of this file review) has `async-trait`,
  `tokio`, `serde_json`. No regex, no heavy deps.
- **R4**: Each verifier is pure mechanism — input is `TurnVerifyContext`,
  output is a variant of `VerifierVerdict`. No business logic in any shell hook
  consumer of `last_message`; the prompt decides.
- **R7 / R8 / R10**: explicitly enforced by `mod.rs:21-43` and reiterated in
  each per-verifier module doc. The "JudgeVerifier" / "ComputationalVerifier"
  prohibition is permanent per the redline. No LLM invocation paths in this
  module. The verifier chain is a "structural watchdog layer" — exactly what
  R10's Future-Proof Test calls for.
- **R9**: All tuning is exposed via `ModelRobustnessProfile::for_behavior(name)`,
  driven from the orchestrator. No internal-only thresholds.

### Code Quality
- All `pub` re-exports in `mod.rs:46-54` resolve — `cargo doc` would not warn
  about missing docs on `pub` items (Rust 2018+ doesn't require docs).
- Per-verifier module docs uniformly mention R7/R10 and explain the structural
  nature of their checks. `tool_loop_verifier.rs:31-37` calls out the
  expensive false-positive cost (vetoes disrupt the model) and justifies each
  tier's conservatism — exactly the institutional memory the previous
  review flagged as exemplary.
- `extension_stop_gate.rs:165-170` documents the observer-vs-interceptor
  separation and the historical bug ("the gate used to dispatch interceptors
  only"). The fix prevents the re-introduction of "observer hook registered
  but never runs".
- `execute_shell_hook` `tokio::join!` for stdin write + stdout read + stderr
  read is concurrency-correct: a hook that prints to stdout before reading
  stdin can no longer deadlock against a large `context_json` (comment at
  lines 348-354 explains the prior failure mode). `read_capped` drains past
  the cap so a hook printing >64 KB cannot hang `wait()` into the timeout.
  This is the exact pattern flagged in the prior review's "byte-cap env
  truncate" / "cancel pass-through" cleanup.

### Minor test gaps (low)
- `StopHookVerifier` does not directly test the Halt mapping (priority
  semantics: Halt wins over Block when both fire in the same aggregate).
  The aggregate-result priority is testable with two in-process hooks —
  one Block, one Halt — and one VerifierChainBuilder round.
- `MutationEvidenceVerifier` does not test non-`end_turn` stop_reasons — see
  the high-confidence finding above.
- `ExtensionStopHookVerifier::verify_with_executor` does not test the
  `action_failed = true` path against the public `verify` trait method
  (currently only the unit test exercises it via the private helper).

## Conclusion
`src/verification/` remains exemplary — the structural-watchdog design
holds, the wiring is intact, the lock conventions follow `crate::sync_primitives`
+ `unwrap_or_else(|e| e.into_inner())`, and UTF-8 byte truncation is correctly
delegated to `crate::utils::text_format::truncate_bytes` after the 2026-08-25
fix batch. The three issues filed above are small, targeted hardening:

1. relabel a misleading helper (code clarity / future-proofing),
2. reap a zombie in the slow-hook cancel path (resource hygiene),
3. lock down the strict-equality semantics of `MutationEvidenceVerifier` with
   one extra unit test (defensive against silent design drift).

Nothing in this module touches platform APIs, calls an LLM, or runs regex
beyond mechanical format parsing. R7/R8/R10 hold.
