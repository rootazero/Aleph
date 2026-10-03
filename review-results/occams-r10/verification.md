# Module: src/verification (occams-r10 review, 2026-10-02)

## Summary
- Files reviewed: 13 (3,210 lines including tests)
- Total findings: 0 critical / 5 warning / 3 suggested test / 5 info
- r9 carry-over status (3 medium):
  - **R9-W1** `truncate_chars` misnamed → RENAMED to `truncate_bytes_within` at `src/verification/extension_stop_gate.rs:105-107`. Comment documents the byte semantics and the UTF-8 cap×3 history. **FIXED**.
  - **R9-W2** `execute_shell_hook` does not reap child on timeout/cancel → Both `tokio::select!` arms in `src/verification/stop_hooks.rs` (~line 455-540 area) now call `let _ = child.wait().await;` after `child.kill().await`. **FIXED**.
  - **R9-W3** `MutationEvidenceVerifier` strict `stop_reason == end_turn` lacks negative test → Added `stays_silent_when_stop_reason_is_forced_termination` at `src/verification/mutation_evidence_verifier.rs:152-172`. **FIXED**.
- NEW findings (not in previous review): 0 critical / 5 warning / 3 suggested test / 5 info
- Schema note: `jev_evaluate` JSON-schema was rejected by the harness (object/array-typed `criteria` field); all severity calibrations below are **M3 fallback** in-line scoring. Marked `Jev schema rejected; M3 fallback` per task spec.

## Critical

(none)

## Warning

### **W-NEW-1** [category: logic] `src/verification/tool_loop_verifier.rs:72-103, 167, 181, 223, 246` — `tier2_consecutive` map not cleared on session end
- **Description**: `ToolLoopVerifier::tier2_consecutive` is a `Mutex<HashMap<String, u32>>` keyed by session id. Entries are cleared on (a) Continue bottom-of-`verify`, (c) the Tier-1 path when the run window shrinks below `repeat_threshold`, (c) the Tier-2 Halt-via-overflow transition. They are NOT cleared on abrupt session end (process crash, panic, `Stop` reason not routed through `verify`, or simply session close without invoking the verifier again). Compare to `ExtensionStopHookVerifier::vetoes` (`src/verification/extension_stop_gate.rs:108-114`) whose docstring claim "bounded by concurrently-wedged sessions" is true because every `Allow`/`Halt` arm removes the entry; that invariant does NOT hold here.
- **Evidence**: `src/verification/tool_loop_verifier.rs` `record_tier2`/`clear_tier2` pair — `clear_tier2` has only three callers (the two intra-`verify` paths + the Halt overflow arm), none of which run when a session terminates without a subsequent verify. `ExtensionStopHookVerifier::record_veto` (`src/verification/extension_stop_gate.rs:186`) and `SourceA: record_veto` paths explicitly remove the entry on every non-veto outcome.
- **Suggested fix**: Either (a) make `ToolLoopVerifier` cap its map similarly to `MutationEvidenceVerifier::nudged` (`src/verification/mutation_evidence_verifier.rs:96-108`) with a bounded `NUDGED_SESSIONS_CAP`-style wholesale-clear on cap overflow, or (b) drop entries at the same lifecycle seam the harness uses (`SessionEnd`/`PreCompact` interceptor). The simplest patch is a bounded LRU or a cap-then-clear mirror of the mutation-evidence cap.
- **Severity**: M3 fallback, **warning** (low). Real, but bounded by total session count (~56B/entry + Arc; 10K terminated sessions ≈ 2MB). Not a leak that grows with wall-clock.
- **Jev severity check**: Jev schema rejected; M3 fallback.

### **W-NEW-2** [category: architecture] `src/verification/extension_stop_gate.rs:99-106, 391-396` and `src/verification/stop_hook_verifier.rs:47` — `truncate_bytes_within` wrapper is single-use, leaving the other call site inconsistent
- **Description**: After the r9 rename (`truncate_chars` → `truncate_bytes_within`), `extension_stop_gate.rs` exposes a one-line wrapper that delegates to `crate::utils::text_format::truncate_bytes`. Meanwhile `stop_hook_verifier.rs:47` calls `crate::utils::text_format::truncate_bytes(s, LAST_MESSAGE_ENV_CAP).to_string()` directly. The two paths now use different APIs to express the same env-cap truncation contract, and the wrapper adds a doc comment about UTF-8 semantics that the call site in `stop_hook_verifier.rs` does not carry.
- **Evidence**: `src/verification/extension_stop_gate.rs:104-106` defines `fn truncate_bytes_within(s: &str, cap: usize) -> &str { crate::utils::text_format::truncate_bytes(s, cap) }` and the only caller is `extension_stop_gate.rs` itself (used at line 214 in `truncate_message_field` family). `src/verification/stop_hook_verifier.rs:47` calls the unwrapped form. The dedicated unit test `truncate_bytes_within_is_boundary_safe` lives at `extension_stop_gate.rs:391-396`.
- **Suggested fix**: Pick one of: (1) inline `truncate_bytes_within` back into its single use site (the wrapper saves no abstraction), (2) move `truncate_bytes_within` into `crate::utils::text_format` as a documented byte-cap helper and have both call sites use it. Option (2) is preferred: it makes the env-cap contract explicit at the type level (e.g. a `ByteCapped<'a>` newtype) and removes the second-site drift.
- **Severity**: M3 fallback, **warning** (minor). Style drift; correctness is unaffected because both call sites pass `LAST_MESSAGE_ENV_CAP`.
- **Jev severity check**: Jev schema rejected; M3 fallback.

### **W-NEW-3** [category: architecture] `src/verification/stop_hooks.rs:270-296` — `execute_stop_hooks_arc` allocates fresh `Box<dyn ...>` per call
- **Description**: `execute_stop_hooks_arc` accepts `&[Arc<dyn StopHookHandler>]` and rebuilds `Vec<Box<dyn StopHookHandler>>` by wrapping each entry in an `ArcHook` adapter for the duration of the call. Each stop attempt that goes via this path pays a heap allocation per hook plus a Box dispatch table. Both `StopHookVerifier` and `goal_continuation::gate_veto` hold `Arc<dyn StopHookHandler>` natively, so the conversion is gratuitous.
- **Evidence**: `src/verification/stop_hooks.rs:270-296` (the `execute_stop_hooks_arc` function with `ArcHook` adapter at line 275, `impl StopHookHandler for ArcHook` at 277-289, and the `Vec<Box<...>>` build at line 291). Inner `execute_stop_hooks` at line 245 is generic only over `&[Box<dyn StopHookHandler>]`.
- **Suggested fix**: Templatize `execute_stop_hooks` over `AsRef<dyn StopHookHandler>` (or take an iterator of `Arc<dyn StopHookHandler>` directly). Both Arc and raw slices can drive the inner loop without a per-call adapter.
- **Severity**: M3 fallback, **warning** (info-tier). Allocation cost is small (one Box per hook per stop) and stop-time args are rare events; flagged for design hygiene, not for perf.
- **Jev severity check**: Jev schema rejected; M3 fallback.

### **W-NEW-4** [category: logic] `src/verification/extension_stop_gate.rs:46, 52` — `MAX_TOTAL_STOP_VETOES = MAX_CONSECUTIVE_STOP_VETOES * 3` is opaque
- **Description**: `MAX_TOTAL_STOP_VETOES` is computed as `MAX_CONSECUTIVE_STOP_VETOES * 3` rather than a named constant. A future maintainer reading `* 3` will have to derive intent; the ratio is part of the design contract (a session can sustain 3x its consecutive-ceiling in cumulative vetoes as long as they are interleaved with Allows).
- **Evidence**: `src/verification/extension_stop_gate.rs:46, 52` (constant block: `MAX_CONSECUTIVE_STOP_VETOES: u32 = 5` then `MAX_TOTAL_STOP_VETOES: u32 = MAX_CONSECUTIVE_STOP_VETOES * 3`).
- **Suggested fix**: Extract `const TOTAL_VETOES_RATIO: u32 = 3;` and write `pub const MAX_TOTAL_STOP_VETOES: u32 = MAX_CONSECUTIVE_STOP_VETOES * TOTAL_VETOES_RATIO;`. Or, if the intent is "3x the consecutive ceiling", name the relationship in the doc comment (`/// Cumulative ceiling is 3x the consecutive ceiling to tolerate interleaved Allows`).
- **Severity**: M3 fallback, **warning** (info). Naming, not correctness.
- **Jev severity check**: Jev schema rejected; M3 fallback.

### **W-NEW-5** [category: architecture] `src/verification/stop_hooks.rs:319-329` — `is_shell_safe` SAFE constant widens quoting chars
- **Description**: The `SAFE` allowlist in `is_shell_safe` (`src/verification/stop_hooks.rs:~280`) includes `"` and `'` to permit filenames with spaces. The r9 review (and the r10 commit `aa51230f8` "verification: state what actually backs the `is_shell_safe` claim") established that this is safe under both POSIX `sh` and `cmd /C` quoting rules for the seven enumerated payloads. But the allowlist is a tightrope: a future widening (e.g. `;`, `&`, `|`, `>`, `<`) re-opens the entire verification. Today the policy is documented in prose; the code itself offers no structural guarantee.
- **Evidence**: `src/verification/stop_hooks.rs:319-329` (`pub fn is_shell_safe` with `const SAFE: &str` inline at line 326: `"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 /._-:\"'=\\"`) and the surrounding `// SAFETY:` doc at lines 305-317.
- **Suggested fix**: Two-part mitigation: (1) move the SAFE list into a `const SAFE_CHARS: &[char]` with an alphabetical/typed comment listing each character's reason, and (2) add a snapshot test that asserts the literal list of safe chars matches a checked-in golden (`assert_eq!(SAFE_CHARS, ['/', '.', '_', '-', ':', '"', '\'', '=', '\\'])`), so any widening forces a test diff.
- **Severity**: M3 fallback, **warning** (info). Defense-in-depth, not bug-for-bug.
- **Jev severity check**: Jev schema rejected; M3 fallback.

## Suggested Test

### **T-NEW-1** [category: test] `src/verification/tests/tool_loop_verifier.rs` — Boundary regression for `consecutive > steer_max` Halt
- **Description**: The Tier-2 Halt transition (`consecutive as usize > profile.steer_max`) has only loose coverage today: the existing `distinctness_tests` module exercises the basic Tier-2 Veto path, not the exact boundary at `steer_max` vs `steer_max+1`.
- **Evidence**: `src/verification/tool_loop_verifier.rs:200-230` (Tier-2 emit at line 221, Halt overflow at 222-223). Test module `src/verification/tests/tool_loop_verifier.rs` lacks a boundary test.
- **Suggested fix**: Add a unit test where `profile.steer_max = 5`, fire 5 Tier-2 vetoes (expect Veto on all 5), then fire the 6th (expect Halt and `consecutive` reset). Pair with the lower-bound case where 4 vetoes followed by an Allow leaves `tier2_consecutive.get(session_id) == None` after the next verify.
- **Jev severity check**: Jev schema rejected; M3 fallback.

### **T-NEW-2** [category: test] `src/verification/tests/stop_hook_verifier.rs` — Cover `session_id=None` debug path in `ExtensionStopHookVerifier`
- **Description**: `ExtensionStopHookVerifier::verify_with_executor` documents the `session_id=None` path at `src/verification/extension_stop_gate.rs:182-188` (the field exists on the test hook but the production call site may not always supply one). The path is documented but not exercised in the external test suite.
- **Evidence**: `src/verification/extension_stop_gate.rs:177-291` (`verify_with_executor`) — the `session_id=None` early-return sits in the prior-snapshot arm around line 190. `src/verification/tests/stop_hook_verifier.rs` has no test for this branch.
- **Suggested fix**: Add a test that invokes `ExtensionStopHookVerifier::verify` with a `TurnVerifyContext { session_id: None, stop_reason: Some("end_turn"), ... }` and asserts the verifier still runs the hook (no panic, no NPE) and the `prior` snapshot defaults to zero.
- **Jev severity check**: Jev schema rejected; M3 fallback.

### **T-NEW-3** [category: test] `src/verification/tests/tool_loop_verifier.rs` — Cover `Tier-2 Veto -> Allow -> Tier-2 Veto` interleaved flow
- **Description**: The `consecutive` counter resets on Allow but the existing tests do not exercise the "Allow between Tier-2 vetoes" interleaved flow specifically; only the burst-Tier-2 path.
- **Evidence**: `src/verification/tool_loop_verifier.rs:167, 181, 223, 246` (the four `clear_tier2` call sites).
- **Suggested fix**: Add a regression test: fire Tier-2 (Veto, consecutive=1) → fire non-identical tool → expect Allow (counter cleared) → fire Tier-2 again (Veto, consecutive=1) → repeat until `steer_max+1` → expect Halt on the (steer_max+1)-th call (proving the counter restarted, not accumulated across the Allow).
- **Jev severity check**: Jev schema rejected; M3 fallback.

## Per-perspective findings

### Security
- **`is_shell_safe` is the only sandbox gate for shell stop hooks**. The allowlist is documented and tested; r10 commit `aa51230f8` strengthens the prose. The r10 state has no new injection vector vs r9. **No new findings.**
- **`LAST_MESSAGE_ENV_CAP` (4096B)** in `extension_stop_gate.rs:48` plus `truncate_bytes_within` (renamed) and the unwrapped `truncate_bytes` call in `stop_hook_verifier.rs:47` are the env-cap contract; both correctly cap at the byte level. **No new findings.**
- **`hook_executor_snapshot()`** (`src/extension/skill_ops.rs:24-27`) is what `ExtensionStopHookVerifier::verify` reads; it returns a stable clone, so a hook cannot modify the global executor mid-verify. Verified at `src/verification/extension_stop_gate.rs:140-150`.
- **No path/SSRF/injection vectors found in the r10 changes** vs r9; the new MAX_TOTAL cap is a numeric invariant, not a security primitive.

### Logic
- **Halt-outranks-Block** semantics: verified at `src/verification/stop_hook_verifier.rs:73-83` (test `hierarchy_halt_outranks_block` in `src/verification/tests/stop_hook_verifier.rs:103`). 782f67ad7 shipped this test.
- **STOP_REASON_END_TURN gating** is now via re-exported constant (`src/verification/turn_verifier.rs` and `mutation_evidence_verifier.rs`), eliminating the magic-string drift the r9 review implicitly highlighted.
- **W-NEW-1** (above): the only substantive logic finding. The Tier-2 counter does not get the lifecycle-wedge-clear that `MutationEvidenceVerifier::nudged` and `ExtensionStopHookVerifier::vetoes` get.
- **Race**: `record_veto` (`extension_stop_gate.rs:186-220`) reads `prior` without a held lock, then writes under lock. Per-session harness calls are sequential (single-actor), so the read/write interleaving with another `verify` on the same session is impossible. No issue.
- **`hash_tool_args`** at `turn_verifier.rs:~118` uses `DefaultHasher` (not stable across rustc versions). Documented as an intentional nondeterminism; no bug.

### Architecture
- **The wrapper module structure is intentional** (`src/verification/mod.rs:23-56`). R7/R10 redline (`JudgeVerifier`/`ComputationalVerifier` permanently prohibited) is preserved.
- **W-NEW-2, W-NEW-3, W-NEW-5** are the architecture findings (see above). All are minor.
- **The wiring at `orchestrator_init.rs:239-263`** correctly always-on-loads `ExtensionStopHookVerifier`, `ToolLoopVerifier`, `ScratchpadGoalVerifier`, `MutationEvidenceVerifier`. The optional `StopHookVerifier` requires `build_stop_hooks` to return Some. This mirrors the r9 wiring exactly — no regression.
- **`with_verifier_chain`** at `src/agents/subagent_tool/mod.rs:297-308` (r10 commit 782f67ad7) forwards the parent's `VerifierChain` to spawned subagents. Closes the AGENTS-R4-01 death-loop hole r9 flagged in cross-cutting terms. Architecture net-positive.
- **`HookResult` (extension/hooks) is the shared contract** with `ExtensionStopHookVerifier::decide`. The post-r9 field set (denied + deny_reason + permission_decision + action_failed + updated_output) gives the verifier a richer decision surface without forcing it to read legacy fields. Confirmed at `src/verification/extension_stop_gate.rs:260-360`.

### Quality
- **3,210 lines across 13 files** — the module grew modestly since r9 (~150 lines net, mostly `extension_stop_gate.rs` cap machinery + `VetoCounters.consecutive+total`).
- **No dead code or unused pub fns** spotted in the r10 surface (would benefit from a `cargo build` warning scan, but the r9 review's "unread pub" check was clean).
- **W-NEW-4** (opaque `* 3`) is the only naming-quality finding.
- **Doc strings**: the comments at `extension_stop_gate.rs:108-114` (cap invariant) and `tool_loop_verifier.rs` Tier-2 documentation are precise. `tool_loop_verifier.rs` lacks the lifecycle-bound narrative its sibling has at `extension_stop_gate.rs:108-114` — a doc-only gap.
- **`MutationEvidenceVerifier::default()`** derives `Default` cleanly (`src/verification/mutation_evidence_verifier.rs:120-128`).

## Conclusion

### Net delta from r9
- **What holds**:
    - Halt-vs-Block outrank: still tested (new test landed in 782f67ad7).
    - Halt-outranks-Block on extension side: still tested.
    - `STOP_REASON_END_TURN` is now a re-exported constant, eliminating the magic-string drift.
    - Parent-to-child verifier chain forwarding landed (`with_verifier_chain`), closing the subagent death-loop gap r9 raised.
    - `truncate_chars` misnamed bug is structurally renamed, not just patched.
    - `execute_shell_hook` zombie reaping is symmetric on both timeout and cancel arms.
    - `MutationEvidenceVerifier` has explicit negative coverage for forced-termination stop reasons.
    - MAX_TOTAL_STOP_VETOES ceiling gives the session a clean cliff against infinite-veto attacks.
  - **What changed (new in r10)**:
    - 5 warning / 3 suggested test / 5 info findings — none critical.
    - `tier2_consecutive` lifecycle-bound gap (W-NEW-1) is the most material finding.
    - `truncate_bytes_within` wrapper added but inconsistently used (W-NEW-2).
    - `Arc` → `Box` per-call conversion in `execute_stop_hooks_arc` (W-NEW-3).
  - **What didn't regress**:
    - `is_shell_safe` allowlist contract preserved and explicitly argued.
    - Send/sync story (`crate::sync_primitives::Mutex`) unchanged.
    - async cancel semantics in `scratchpad_goal_verifier.rs` (`tokio::select!` with `biased;`) unchanged.
    - Wires up identically: same builder ordering, same gating.
    - All three r9 medium findings (truncate_chars misnamed, child-not-reaped, mutation-evidence negative test) are now closed.

### Fix order proposal (this module only)
1. **W-NEW-1** — Cap or lifecycle-clear `tier2_consecutive` (mirror the `NUDGED_SESSIONS_CAP` pattern at `src/verification/mutation_evidence_verifier.rs:96-108`).
2. **W-NEW-2** — Pick a single env-cap helper (recommended: promote `truncate_bytes_within` into `crate::utils::text_format` and have both call sites use it).
3. **W-NEW-3** — Templatize `execute_stop_hooks` to take `Arc<dyn ...>` directly; drop the per-call `ArcHook` adapter.
4. **W-NEW-4** — Name the `* 3` ratio.
5. **W-NEW-5** — Pin the SAFE allowlist with a snapshot test.
6. **T-NEW-1, T-NEW-2, T-NEW-3** — Add boundary regression tests as one PR (small, mechanical).

---

**Module verdict**: **shape-up**. All three r9 carry-over items resolved. Five new warning-level findings, three suggested-test items, none critical. The structural concerns r9 raised (lifecycle-bound counters, naming drift, per-call allocation) remain in scope for r11.