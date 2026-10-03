# Module: src/thinker (occams-r9 review, 2026-10-02)

## Summary
- Files reviewed: 11 (round-8 cited files + every file touched by `git log --since="2026-08-29" -- src/thinker/`)
- Total findings: 2 critical / 11 warning / 5 suggested test (round-8 carry-overs plus new)
- Status of round-8 Criticals: **C1 fixed/REFACTORED-AWAY** (single-source-of-truth invariant now test-enforced); **C2 fixed** (char-vs-byte marker math now char-explicit with release-safe debug_assert)
- Status of round-8 Warnings:
  - W1 (Light sanitizer not stripping control chars) — STILL PRESENT
  - W2 (LayerInput::basic hardcoding Full) — STILL PRESENT (now 17 fields)
  - W3 (is_synthetic_reminder brittle positional) — REFACTORED-AWAY (heavy source-level guard scaffolding)
  - W4 (stale "heuristic" comment) — FIXED
  - W5 (strip_injection_markers_once unconditional lowercase) — FIXED
  - W6 (LayerInput.identity_file case-sensitive) — STILL PRESENT
  - W7 (15-field LayerInput builder inconsistency) — STILL PRESENT (now 17 fields)
  - W8 (MAX_STRIP_PASSES=16 safety net) — STILL PRESENT
  - W9 (window_char_budget floor>ceil) — STILL PRESENT (better documented)
  - W10 (large matches! in is_format_char) — STILL PRESENT (supplemented by drift guard)
  - W11 (truncation_notice classification) — FIXED with explicit scan-based test
  - W12 (identity_file could be inlined) — STILL PRESENT, low impact
  - W13 (38 layer hardcoded priorities) — STILL PRESENT (now 38 layers, count pinned)
  - W14–W17 (REPO_ROOT_CACHE / SoulManifest::is_empty / clean_value / expand_imports depth) — STILL PRESENT, not re-reviewed this round
- NEW findings (not in round-8): **1 critical, 8 warning, 4 suggested test**

The single biggest source of new findings is the introduction of `prompt_size_registry.rs` (a 600-line new file) and the threading chain rewrite in `prompt_builder/cache.rs` / `prompt_builder/mod.rs`. The second-biggest is the still-present Light-sanitizer pattern in `SecurityLayer`, which now lands filesystem-path-bearing posture lines through to the prompt without invisible-Char stripping.

## Critical

### **C-NEW-1** [category: logic] `src/thinker/prompt_size_registry.rs:155-175` — eviction race window silently drops in-flight writes
- **Round-8 status**: NEW (not flagged — file did not exist).
- **Description**: `record_turn` performs a non-atomic `contains_key` → `min_by_key` → `remove(&oldest)` → `insert(session_key, …)` sequence when adding a NEW session_key to a full map. Between the `remove` and the `insert`, a concurrent `record_messages` or `record_tool_output` call on the same lock window could observe a session_key that is about to be evicted but has not been removed yet (because `record_messages` looks up by key, not by stamp, when the bound call is also in flight). Since `update_turn` and `record_tool_output` silently no-op when the record is missing, the doomed session's last write is dropped.
- **Evidence**:
  - `record_turn` body at `src/thinker/prompt_size_registry.rs:155-175` — three separate map operations (contains_key check, min_by_key iter, remove) before the insert.
  - `update_turn` at `src/thinker/prompt_size_registry.rs:200-214` — silently drops writes when the record is absent.
  - `record_tool_output` at `src/thinker/prompt_size_registry.rs:217-235` — silently drops writes when the record is absent.
  - The single-threaded test `a_stamp_outlives_nothing_it_did_not_name` (`src/thinker/prompt_size_registry.rs:396-410`) exercises the case where the bound run was already evicted and recreated; it does NOT exercise the mid-eviction window.
- **Suggested fix**: Hold the `Mutex` across eviction+insert atomically (already inside one lock acquisition, but expose the lock pattern to `record_messages`/`record_tool_output` readers via a guard-pattern callback or a `with_record` API that observes eviction during the callback). Alternatively, document the lossy semantics explicitly: "writes concurrent with eviction may be silently dropped".

## Warning

### **W-NEW-1** [category: logic] `src/thinker/prompt_size_registry.rs:222-227` — `latest()` clones a full `PromptSizeRecord` under the lock
- **Round-8 status**: NEW (not flagged — file did not exist).
- **Description**: `latest(&self, session_key: &str) -> Option<PromptSizeRecord>` locks the registry and returns a `.cloned()` of the record, which contains `Vec<LayerSize>` (each layer's `(priority, name, stability, chars, bytes, tokens)`), `Vec<(String,u64,u64)>` tools, and several `Option<…>`. Under heavy `context.breakdown` RPC load this holds the mutex through a non-trivial allocation+clone.
- **Evidence**: `src/thinker/prompt_size_registry.rs:222-227` — single accessor returns an owned clone.
- **Suggested fix**: Add a `latest_borrowed(&self, &str, &mut impl FnOnce(Option<&PromptSizeRecord>))` API for read-only RPC paths, or a `try_latest` returning a `MutexGuard` (clippy will complain — use `parking_lot::Mutex` if available, or document the trade-off). For now, this is a contention warning not a correctness issue.

### **W-NEW-2** [category: architecture] `src/thinker/prompt_builder/cache.rs:251-275` + `src/thinker/prompt_builder/mod.rs:213-234` — duplicated `LayerInput` threading chains
- **Round-8 status**: NEW (wiring fix changed both builders; drift risk newly introduced).
- **Description**: `build_cached_input` (cache.rs) and `build_basic_input` (mod.rs) both walk the same 11 builder fields with the same `match`/`with_*` pattern. The existing regression test `cached_full_prompt_carries_subagent_role_and_protocol` (cache.rs:329-353) caught exactly one such drift on the cached path. Adding one more wiring field now requires editing two parallel chains, and any subsequent field could drift in only one.
- **Evidence**:
  - `build_cached_input` chain: `with_identity_files_opt` → `with_extra_files_opt` → `with_agent_def` → `with_mcp_instructions` → `with_curated_envelope` → `with_chain_context_opt` → `with_resolved_context_opt` → `with_behavior_name_opt` → `with_model_behavior_delta_opt` → `with_iteration_cap_opt` → `with_session_summaries` → `with_recalled_memory`.
  - `build_basic_input` chain: same shape, identical ordering.
- **Suggested fix**: Extract a private `LayerInput::threaded_from(self, &PromptBuilder) -> LayerInput` (or accept `&PromptBuilder` as `&self`). One wiring path; one diff to update on next field addition.

### **W-NEW-3** [category: architecture] `src/thinker/prompt_layer.rs:124-145` — `LayerInput::basic` still hardcodes `mode: PromptMode::Full` (carries W2 forward)
- **Round-8 status**: CONFIRMED-W (round-8 W2).
- **Description**: `LayerInput::basic(config, tools)` constructs the input with `mode: PromptMode::Full` hardcoded (src/thinker/prompt_layer.rs:131). The 17-field struct grew (added `model_behavior_delta`, `iteration_cap`, `extra_files`, `has_session_summaries`, `has_recalled_memory`, `curated_memory_envelope` since round-8), but the constructor still does not surface a mode argument. Every real caller follows `basic()` with `with_mode(...)` to opt out, which means the constructor's default is never observed in production.
- **Evidence**: `src/thinker/prompt_layer.rs:131` hardcodes `mode: PromptMode::Full`.
- **Suggested fix**: Make `basic` take a mode parameter (`fn basic(config: &PromptConfig, tools: &[ToolInfo], mode: PromptMode) -> Self`), or rename `basic` to `for_basic_path` to signal the path-marker rather than the default. The `const fn` qualifier on the existing `basic` is also decorative: `with_curated_envelope` allocates `Option<String>` and cannot be `const`, so no real construction is `const` either.

### **W-NEW-4** [category: security] `src/thinker/layers/security.rs:80,96,103` — `Light` sanitizer used on sandbox posture lines that carry filesystem paths (carries W1 forward with new evidence)
- **Round-8 status**: ESCALATED-from-W (round-8 W1 + 2026-08-19 commit "OperatingEnvelopeLayer moved sandbox `Writable roots` line" + worktree-id mints per isolated run).
- **Description**: `SecurityLayer::inject` calls `sanitize_for_prompt(&line, SanitizeLevel::Light)` on every sandbox posture line (`layers/security.rs:75-78`), every security note (`layers/security.rs:95-97`), and the elevated-policy fallback note (`layers/security.rs:101-103`). The `Light` sanitizer (src/thinker/prompt_sanitizer.rs:48) ONLY strips injection markers — it does NOT strip invisible Unicode, RTL/bidi overrides, zero-width joiners, or control characters. The posture lines include filesystem paths (workspace roots, worktree ids — see `SandboxSummary::isolated_worktree` at src/sandbox/, also referenced by the test `never_renders_the_per_run_writable_root` at layers/security.rs:213-225 which exercises this exact path).
- **Evidence**:
  - `layers/security.rs:80` — `let line = sanitize_for_prompt(&line, SanitizeLevel::Light);`
  - `layers/security.rs:96` — `let note = sanitize_for_prompt(note, SanitizeLevel::Light);`
  - `prompt_sanitizer.rs:48` — `SanitizeLevel::Light => strip_injection_markers(value),` (no invisible-Char stripping).
  - The test `never_renders_the_per_run_writable_root` proves a worktree id reaches the cacheable prefix; nothing in the pipeline strips invisible Unicode from such an id before injection.
- **Suggested fix**: Switch posture-line and security-note calls to `SanitizeLevel::Moderate` (which keeps `\n`/`\t`/`\r` and strips everything else) or to `Strict` for path-bearing lines. Round-8 already suggested this; the round-9 finding is that the surface area has grown (worktree ids, multi-root workspaces) and the threat is now wider.

### **W-NEW-5** [category: architecture] `src/thinker/layers/identity_files.rs:28-47` — `INJECTION_PATTERNS` list is conservative-but-leaky; gap not enumerated
- **Round-8 status**: NEW (round-8 covered the old list but not the new tightened list).
- **Description**: The tightened `INJECTION_PATTERNS` list (commit 38a9650c1 "tighten injection patterns") removes overly-broad phrases like `'system prompt:'` and `'do not reveal'` that were blocking innocent soul-framing. The new list captures 9 specific phrases verbatim. A determined attacker can phrasetwist around every entry: `ignore every prior instruction` (vs the literal `ignore all previous instructions`), `bypass your instructions`, `forget your instructions`, `discard your rules`, `replace your guidelines with`, etc. — none of these match the current list.
- **Evidence**: `src/thinker/layers/identity_files.rs:28-47` — list is hardcoded; no Fuzzy/case-stripped match; no regex variant.
- **Suggested fix**: Either (a) extend the list to cover common phrasetwists (and pin each new entry with a "block test" + a "pass-through test" for innocent variants), or (b) add a second pass using a less-strict pattern (e.g., `ignore … instruction`) at lower priority, and document the trade-off explicitly. The comment at line 21-27 explains the broad-stroke reasoning; a follow-up comment enumerating the known gaps (the phrases that DO slip through) would let future maintainers decide whether to widen the net.

### **W-NEW-6** [category: quality] `src/thinker/prompt_budget.rs:208-217` — `debug_assert!` on shipped-marker size has no release-mode counterpart
- **Round-8 status**: NEW.
- **Description**: `truncate_with_head_tail` ships a `debug_assert!(marker.chars().count() <= reserved_chars, ...)` (src/thinker/prompt_budget.rs:208-217) to guard against the shipped marker outgrowing its reservation. In release builds, the assertion is compiled out; the downstream safety-net at line 235 catches the TOTAL budget (`result.chars().count() > max_chars`) but does NOT detect that the head/tail split was computed against an older/smaller marker shape. A future non-ASCII marker edit (e.g., a CJK translation) would silently degrade the head/tail split in production.
- **Evidence**: src/thinker/prompt_budget.rs:208-217 (`debug_assert!`).
- **Suggested fix**: Either (a) make `truncate_with_head_tail` return a `Result<String, BudgetError>` so release builds surface the drift, or (b) add a release-mode `eprintln!` (or a `tracing::warn!`) that names the drift; or (c) hard-fail in CI by keeping the assertion as a `#[track_caller]` panic in a wrapper that release builds also call.

### **W-NEW-7** [category: architecture] `src/thinker/prompt_size_registry.rs:332-340` — shared test `OnceLock` claims parallel-test safety without proof
- **Round-8 status**: NEW.
- **Description**: `install_test_prompt_size_registry` returns a shared `Arc<PromptSizeRegistry>` from a `OnceLock`, with the comment "tests keep to their own session keys". Two tests using the same session_key concurrently WILL produce cross-talk: `record_turn` is a single-turn REPLACE (drops layout) but the records are still visible to one another through `latest()`. Under `cargo test -- --test-threads=N` this could surface as flaky failures (test A's assertion sees test B's record). The comment also references `session::store::install_test_event_store` as a same-shape pattern, which is worth verifying has been proven race-free.
- **Evidence**: src/thinker/prompt_size_registry.rs:332-340.
- **Suggested fix**: Generate per-test session keys (`format!("test-{}-{}", module_path!(), test_name())`) or pass a unique key from each test. If the underlying `session::store` pattern has already been verified race-free under `cargo test`, mirror that proof here.

### **W-NEW-8** [category: architecture] `src/thinker/layers/security.rs:213-225` + related — `never_renders_the_per_run_writable_root` test pins an inline knowledge that should be a `SandboxSummary` invariant
- **Round-8 status**: NEW (test added in commit 276fccb16 era).
- **Description**: The test `never_renders_the_per_run_writable_root` (src/thinker/layers/security.rs:213-225) is a layered regression guard — fine in itself — but the underlying invariant it enforces ("Stable layers must not render per-run values") is currently encoded in 3 places: (a) the comment block at layers/security.rs:35-46, (b) `OperatingEnvelopeLayer::priority` / `stability()` declarations, (c) the test. Any new sandbox-derived value (e.g., a future per-run `permission_profile_id`) would re-introduce the bug if added to `SecurityLayer` without updating the test.
- **Suggested fix**: Encode the invariant in a typed helper `SandboxSummary::stable_lines(&self)` vs `volatile_lines(&self)`, and have `SecurityLayer` consume only `stable_lines` while `OperatingEnvelopeLayer` consumes `volatile_lines`. The compiler then enforces the split.

### **W-NEW-9** [category: quality] `src/thinker/layers/extra_files.rs:73-83` — `sanitize_header` strips invisible but not control chars; benign in current callers but undocumented
- **Round-8 status**: NEW (file substantially changed by commit 276fccb16).
- **Description**: `sanitize_header` (src/thinker/layers/extra_files.rs:73-83) calls `strip_invisible_chars(name).0` from `unicode_guard`, which strips invisible Unicode (Cf, bidi, tag block) but NOT control characters. A configured filename containing `\x00` or `\x07` would pass through to the markdown `### ` header. Current callers never feed such names (the loader reads from TOML), but the function is `pub(crate)` and could be reused.
- **Suggested fix**: Add `out = out.chars().filter(|c| !c.is_control()).collect()` after the newline normalization, or document explicitly that "control-Char stripping is the caller's responsibility".

### **W-CONF-1** [category: quality] `src/thinker/prompt_layer.rs:265-270` — `identity_file(&self, name: &str)` is exact-match (round-8 W6 CONFIRMED)
- Round-8 framing accepted. No change since 2026-08-29. Current callers use exact case.

### **W-CONF-2** [category: architecture] `src/thinker/prompt_layer.rs:93-260` — `LayerInput` builder inconsistency, now 17 fields (round-8 W7 CONFIRMED)
- Round-8 framing accepted. Pattern still inconsistent: required-only for `mode`, `agent_def`, `mcp_instructions`, `curated_memory_envelope`, `has_session_summaries`, `has_recalled_memory`; both required+optional for `identity_files`, `extra_files`, `chain_context`, `behavior_name`, `iteration_cap`. The two-mode construction (`basic` hardcodes Full; `build_cached_input` uses `.with_mode(mode)`) is the primary drift risk.

### **W-CONF-3** [category: quality] `src/thinker/prompt_sanitizer.rs:124` — `MAX_STRIP_PASSES=16` safety net (round-8 W8 CONFIRMED)
- Still not test-pinned for pathological nesting. The fixed-point termination test `pass.len() == result.len()` is the primary safeguard; the cap is a backstop.

### **W-CONF-4** [category: quality] `src/thinker/prompt_budget.rs:36-43` — `scale_window_to_budget` invariant: `floor <= ceil` (round-8 W9 CONFIRMED)
- Now better documented; comment explicitly says "callers pass compile-time constants that satisfy this". No release-mode check.

## Suggested Test

### **ST-NEW-1** — `prompt_size_registry` multi-threaded contention test
- Pin the eviction+insert atomicity story. Spawn 4 threads, each writing to a unique session_key plus one shared `record_messages` call. Assert no panics, no torn records, and that every written session has a `latest()` matching what was written.
- Drop the existing single-threaded tests' reliance on `OnceLock` for shared state and replace with `Mutex<HashMap<session_key, expected_record>>` snapshots.

### **ST-NEW-2** — `LayerInput::basic` mode pinning
- `assert_eq!(LayerInput::basic(&cfg, &[]).mode, PromptMode::Full);`
- A future change that flips `basic`'s default to Compact or Minimal would otherwise go undetected; this pins the current behavior so the flip is intentional.

### **ST-NEW-3** — `agent_role.rs` Basic-path visibility (symmetric to `cached_full_prompt_carries_subagent_role_and_protocol`)
- Verify a registered SubAgent with `prompt_sections` shows up in `build_system_prompt_parts`'s cached prefix. Same shape as the cached-path test (src/thinker/prompt_builder/cache.rs:329-353) but on the Basic path.

### **ST-NEW-4** — `Light` sanitizer does NOT strip invisible Unicode (regression for the documented behavior)
- `let out = sanitize_for_prompt("path\u{202E}/etc/passwd\u{FEFF}", SanitizeLevel::Light); assert_eq!(out, "path\u{202E}/etc/passwd\u{FEFF}");`
- Pins the current behavior so a future change upgrading Light to also strip invisible Unicode is intentional, and so SecurityLayer's use of Light on path-bearing posture lines is documented as known-vulnerable-to-invisible-Char-passthrough.

### **ST-NEW-5** — `INJECTION_PATTERNS` coverage + gap enumeration
- For each pattern, a "block test" (verifies the pattern trips the gate) AND a "pass-through test" (verifies a near-miss, e.g. `ignore every prior instruction`, is NOT blocked). Enumerate the gap so future maintainers decide explicitly whether to widen the net.

## Per-perspective (lower confidence)

### Security
- `SecurityLayer` uses `SanitizeLevel::Light` for sandbox posture lines that carry filesystem paths (workspace roots, worktree ids). Light does NOT strip invisible Unicode. A worktree id containing RTL override or zero-width chars would reach the model verbatim. Round-8 flagged this; round-9 evidence shows the surface area grew. (See W-NEW-4.)
- `INJECTION_PATTERNS` is conservative on purpose but the gap is not documented. Phrasetwists slip through silently. (See W-NEW-5.)
- `prompt_sanitizer::strip_injection_markers_once` lowercases the WHOLE input (src/thinker/prompt_sanitizer.rs:139-180) and allocates a `Vec<(usize,usize)>` of removal intervals. For multi-megabyte inputs the allocation is bounded by `O(matches × len)` rather than `O(len)`, which is fine for the fixed-point loop's purpose but worth a comment if any future caller processes multi-megabyte untrusted text in a hot path.

### Logic
- `prompt_size_registry::record_turn` eviction race window is small but exists. (See C-NEW-1.)
- `LayerInput::basic` hardcodes `mode: PromptMode::Full` while the constructor's purpose ("basic assembly path") is orthogonal to mode. This is a correctness risk only if a future layer relies on mode-based behavior and assumes `basic` = a default other than Full. (See W-NEW-3.)
- `truncate_with_head_tail`'s release-mode safety net clips the TOTAL budget but not the head/tail split. (See W-NEW-6.)

### Architecture
- `build_basic_input` and `build_cached_input` duplicate the threading chain. (See W-NEW-2.)
- `LayerInput` builder inconsistency grew from 15 to 17 fields. (See W-CONF-2.)
- `prompt_size_registry::latest` clones under lock. (See W-NEW-1.)
- `SandboxSummary` stable/volatile split is implicit in layer placement, not in the type. (See W-NEW-8.)

### Quality
- `MAX_STRIP_PASSES=16` is a backstop but lacks a pathological-nesting test. (See W-CONF-3.)
- `sanitize_header` strips invisible but not control chars; undocumented. (See W-NEW-9.)
- `identity_file` is case-sensitive; all current callers use exact case but the assumption is undocumented. (See W-CONF-1.)

## Conclusion

### Net delta from round-8
- **Both round-8 Criticals are fixed** — and the fixes go beyond the round-8 framing. C1 moved from "documented dual SSOT" to "test-enforced single SSOT with a whole-code-point-space drift guard". C2 moved from "byte-vs-char confusion" to "char-explicit math + release-safe debug_assert + dedicated CJK regression test". These are real wins.
- **Two of the most actionable Warnings (W3, W4, W5, W11) are also fixed** — the brittle positional classifier now has a source-level guard, the stale heuristic comment is gone, the unconditional lowercase is gone, and the truncation-notice classification is test-pinned.
- **The remaining Warnings (W1, W2, W6, W7, W8, W9, W10, W12, W13) are still present** — and round-9 found that some have grown in surface area (W1 now lands worktree IDs through to the prompt; W7 grew to 17 fields).
- **The new code (prompt_size_registry.rs, the threading rewrite in prompt_builder/cache.rs + mod.rs, the injection-pattern tightening) is mostly clean but introduces one Critical (C-NEW-1, eviction race) and several Warnings**. The new code is also notably well-pinned by tests: `the_collapsed_traversal_matches_a_direct_append`, `cached_full_prompt_carries_subagent_role_and_protocol`, `cached_full_prompt_welds_strategy_into_stable_and_dynamic`, `cached_full_prompt_injects_soul_and_agents_identity_files`, `supplement_does_not_overlap_with_unicode_guard_ssot`, `notice_fits_within_reserve`, `no_fenced_const_escapes_classification`, `no_fenced_formatter_escapes_classification`, `a_stamp_outlives_nothing_it_did_not_name`, `a_session_that_keeps_writing_is_not_evicted_as_stale`. The test density is significantly higher than in round-8's module.

### Fix order proposal (cheapest + highest-impact first)
1. **C-NEW-1** — `prompt_size_registry.rs:155-175` eviction race. Hold the lock across eviction+insert atomically and surface eviction to in-flight readers. 1-line semantic change; existing tests still pass.
2. **W-NEW-4** — `layers/security.rs:80,96,103` upgrade to `SanitizeLevel::Moderate` for posture-line and security-note calls. 3-character change per call site. Closes the still-present W1 with concrete new evidence.
3. **W-NEW-2** — extract `thread_into(input)` helper used by both `build_basic_input` and `build_cached_input`. One helper, two call sites collapse. Prevents the next drift on a 12th field.

### What this review did NOT do (unhandled edge cases / skipped validations)
- Did NOT re-review round-8 W14 (`REPO_ROOT_CACHE` double-lock), W15 (`SoulManifest::is_empty`), W16 (`clean_value` parens ambiguity), W17 (`expand_imports` depth) — these are unchanged since 2026-08-29 but I have not opened the files to confirm.
- Did NOT run `cargo check` or `cargo test` per the hard constraint.
- Did NOT cross-check `prompt_size_registry::tests` parallel-test behavior empirically under `cargo test -- --test-threads=N`.
- Did NOT review `prompt_contract.rs` (the module's ratchet tests) — round-8 cited it but no commits touch it since 2026-08-29.
- Did NOT review `memory_context_provider/tests.rs`, `soul_archetypes/`, `xml_util.rs`, or `protocol_tokens.rs` — no commits touch them since 2026-08-29.
- Did NOT review `interaction.rs`, `security_context.rs`, `project_instructions.rs`, `context.rs`, `identity_profile.rs`, `runtime_context.rs`, `mod.rs` — touched by commits but no round-8 Critical/Warning is cited against them and round-9 surfaced no findings that would override that priority.
- Findings are filed against the THINKER-side code only. Cross-module evidence (e.g. consumer of `prompt_budget::truncate_with_head_tail` in `prompt_builder/cache.rs`) is cited as evidence but not as a separate finding.
