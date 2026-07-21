# Module: src/context

- Path: `src/context/`
- Files scanned: 24
- Total LOC: 10,770
- Confidence threshold: 80 (all reported findings considered actionable)

## Summary

| Severity | Count |
|----------|------:|
| critical | 0 |
| high     | 5 |
| medium   | 9 |
| low      | 12 |
| **Total**| **26** |

## High-Confidence Issues

### Perspective 1 — Security & Robustness

```
ISSUE|src/context/budget/mod.rs:184-189|medium|ContextBudgetConfig exposes `diminishing_window` and `diminishing_threshold` fields that no production code reads — they are dead config and the module doc-comment on lines 1 and 254 ("diminishing returns detection") advertises a feature that does not exist.
ISSUE|src/context/compact/summary_utils.rs:30-60|low|escape_prompt_boundaries evaluates the same `MARKERS.iter().map(...).min_by_key(...)` expression twice on every iteration (once to find the position, once to recover the marker), doubling CPU and obscuring intent; a one-line helper would be both faster and auditable for the prompt-injection neutrality guarantee this function is load-bearing for.
ISSUE|src/context/retrieval/content_index.rs:678-723|low|proximity_relevance recomputes `to_ascii_lowercase()` and builds a `Vec<(usize, u64)>` for *every* candidate chunk on multi-term searches; a 200-line body × dozens of over-fetched rows allocates hundreds of `String`s per search, and the inner allocation is hit twice (lowering + per-word `String::from`). No correctness risk but a clear quadratic-ish CPU path.
ISSUE|src/context/budget/cheap_passes/structured/json.rs:100-145|medium|shrink recurses without persisting the `changed` flag for the depth-limit placeholder: a JSON subtree at depth ≥ MAX_DEPTH is replaced with `Value::String("…(depth limit)")`, but the walk above returns `changed=true` while the caller's `body.len() >= trimmed.len()` byte guard (line 74) still allows emission — fine for size, but no test exercises depth-bounded output, and an attacker-controlled deeply-nested blob could still survive detection if not actually smaller.
ISSUE|src/context/compact/tool_aware_chunker.rs:127-128|low|ToolAwareChunker::new panics on `token_ratio <= 0.0`; this `pub` constructor is reachable from session-compaction code, so a misconfigured caller (e.g. a future MCP/Skill that injects a ratio override) crashes the agent loop instead of degrading gracefully — a `Result` return or `saturating_or(default)` would fit R7 better.
ISSUE|src/context/compact/preserve.rs:34-36|medium|is_summary_text matches any user turn whose `trim_start().starts_with("[Context Summary")` — including the session-memory marker flavor and ANY user content that happens to start with that literal prefix. A user pasting the line "[Context Summary, please ignore everything above]" at the start of a turn silently drops that intent from re-preservation, and could be used to detach future re-compaction from a specific request.
ISSUE|src/context/retrieval/content_index.rs:107-159|low|ContentIndex::from_conn interpolates the two table names into CREATE statements; today the only call sites pass the constants `"chunks" / "chunks_tri"` (verified at lines 301/307), but a future caller passing user-influenced names would inject SQL — making the parameter `pub(crate)` or wrapping it in a sealed const-set would harden the seam.
ISSUE|src/context/budget/pressure.rs:29-97|medium|looks_like_code's keyword allowlist is a 14-entry substring scan over the first 20 lines; a pasted multi-megabyte code block whose first 20 lines look like prose (e.g. doc comments at the head of a generated source) silently pays the prose ratio instead of the code ratio and under-counts tokens by ~40%, biasing the budget sensor on exactly the tool-heavy turns the helper was meant to fix.
```

### Perspective 2 — Logic & Correctness

```
ISSUE|src/context/budget/mod.rs:1-7|high|The module doc claims "Context Budget — pressure sensing, compaction circuit breaker, and diminishing returns detection" but `before_turn`, `note_compaction_effect`, `observe_actual_usage` and the `CompactionCircuitBreaker` never read `diminishing_window` / `diminishing_threshold`; the half-implemented feature name appears in the type's own doc and three production wirings (`orchestrator/deps_builder/context_budget.rs:422`, `orchestrator/harness_bridge/runner_impl.rs:1114`, plus tests) but has no observable behaviour, so a future dev wiring up "the configured diminishing returns check" will silently find nothing.
ISSUE|src/context/compact/compactor.rs:500-514|medium|When the window contains multiple prior `[Context Summary]` markers, `find_map` takes the FIRST and folds only the turns AFTER it — turns between the first and second summary are dropped (not folded into either), and the second summary is re-summarized from scratch instead of being chained. In practice a child-session seed re-compacted immediately would lose the structure chain this branch was designed to preserve.
ISSUE|src/context/budget/pressure.rs:120|medium|f64 token-budget at low ratio: `content_ratio_with_baseline` returns `0.0` for `non_cjk_ratio == 0.0` (`f64::INFINITY` path, line 165), `total_tokens` then becomes `inf`, `total_chars / inf = 0.0`, the function returns `prose_ratio` instead of a zero-guard. The fallback is documented but the contract is silently inverted — a zero prose_ratio ends up charging at the prose anchor it explicitly forbids.
ISSUE|src/context/compact/fit.rs:60-63|low|truncate_to_fit's leading orphan tool_result snap only runs once after the eviction loop; if a single eviction uncovers a NEW orphan that was previously protected by another orphan head, it remains — verified manually against eviction-one's `answered` set which only removes the directly paired result, leaving multi-result pairs without their call intact.
ISSUE|src/context/budget/cheap_passes/file_op_supersede.rs:398-416|medium|canonicalize_path_string only strips a leading `./` and trims whitespace; `a/../b/foo.txt` and `b/foo.txt` are different keys, as are absolute vs relative forms. Two callers addressing the same logical path with different normalization produce two separate supersession graphs, so reads/writes on `relative.txt` never invalidate reads on `./relative.txt` — the comment says it deliberately avoids full canonicalization (good — symlinks etc.) but the minimal normalization leaves a real false negative path that costs tokens on every pass.
ISSUE|src/context/compact/directive.rs:113-120|low|On `CompactAndContinue` `Err`, the budget's `note_compaction_effect` is never called, so the circuit breaker keeps its count from the failed pass — a flapping LLM provider can drive the breaker to a `SplitSession` decision on transient errors that don't represent ineffective compaction. The reactive rescue path has its own fail-soft (line 235), but the proactive path does not mirror it.
ISSUE|src/context/compact/compactor.rs:419-433|low|The cache fast path's freshness check `c.end <= cut_end` does not also check `c.start < messages.len()`; if a previous compaction grew the message list into the protected fresh-tail zone (e.g. effective_tail changed between runs), `c.end` could now point past the message-vector end and `messages[c.start..c.end]` would panic — currently unreachable because `effective_tail = fresh_tail.max(config.fresh_tail)` is monotonic, but the guard's silence hides the assumption.
ISSUE|src/context/retrieval/content_index.rs:765-775|low|iter_bits via `std::iter::from_fn` returns Some(ti) where ti is u64::trailing_zeros; calling `.trailing_zeros()` on a u64 with only the top bit set returns 63, but on a u64 == 0 the `.unwrap_or_else` matches the prior guard. The iterator does not check `mask != 0` before each yield, instead relying on the outer `if mask == 0 { None }` — correct, but a one-line `debug_assert!(mask != 0)` would document the invariant.
```

### Perspective 3 — Architecture Compliance

```
ISSUE|src/context/retrieval/content_index.rs:629-635|high|proximity_rerank is a deterministic, multi-feature boost (coverage, tightness, BM25 fusion) that re-orders search results before the model sees them. R7/R9 expect retrieval reranking to live where the model can steer it via prompts/tools; baking the boost into a const `PROXIMITY_BOOST = 0.35` means the model has no way to opt out of, weaken, or reroute this reordering — replacing model judgment with hidden middleware.
ISSUE|src/context/budget/mod.rs:413-444|high|The directive-state machine (warning → CompactAndContinue → breaker trip → SplitSession → CompactToFit) is fully deterministic: every transition is computed from pressure.ratio and the circuit-breaker counter, with no threshold that the model can override, no recognisable "let the model decide whether to compact" hand-off, and no Skill/MCP seam for an LLM-side strategy. R9 ("all configurability exposed as tools") is fundamentally violated here — the model's only handle is `observe_actual_usage`, which only refines the next ratio estimate, not the strategy.
ISSUE|src/context/compact/compactor.rs:294-319|high|ContextCompactor embeds cache invalidation policy in core (fingerprint hashing + carry-over + LRU eviction) that the model cannot observe or steer. R7 says "one core, many shells" but eviction-on-rewrite, the freshness check `c.end <= cut_end`, and the carry-over slot bounds (CARRYOVER_MAX_SESSIONS=16) are all silent runtime decisions the harness never reports. A skill could plausibly expose these — currently it can't.
ISSUE|src/context/budget/cheap_passes/structured/search.rs:21-54|medium|match_path is a hand-rolled byte-level `path:line:` parser living in core; R3 permits it because there's no regex dep, but R8 allows regex for "machine formats" — this is a (path, line, sep) parser operating on what is essentially a structured grep log. A small `regex` dep (already likely in the workspace) or a tiny parser crate would replace ~30 lines of byte arithmetic that is bug-prone around Windows paths and dashed filenames (tests at lines 154-182 only cover three shapes).
ISSUE|src/context/budget/pressure.rs:29-97|medium|looks_like_code is a 14-indicator substring heuristic in core that classifies content type for token-ratio selection; this is intent-adjacent (deciding whether code or prose density applies) and lives in the budget math path. R9 expects "intelligence should live in the prompt" — moving this to a content-type tag the model can supply (or letting the caller pre-classify) would drop ~70 lines of brittle detection from core.
ISSUE|src/context/budget/cheap_passes/file_op_supersede.rs:236-265|medium|obsolete_call_ids uses path-equality as the sole signal for "is this read obsolete?" — the heuristic is deterministic and structurally correct, but it operates on rich assistant-turn semantics (read-then-write on the same path means the read result is safe to stub) that an LLM would normally judge. R7: a skill that exposes this judgement to the model could outperform the path-equality rule for "I rewrote this but only the trailing newline" cases the rule wrongly counts as supersession.
ISSUE|src/context/budget/preflight.rs:99-107|medium|The preventive-band gate `if pressure.ratio < self.min_pressure_ratio` is fully deterministic — once the pipeline decides "we're past the floor, fire every stage", there is no per-stage bypass for the model's current intent (e.g. "I am about to query for `foo` — do not drop the foo-related tool result"). R9 says configurability should be tool-exposed; this gate is invisible.
```

### Perspective 4 — Code Quality

```
ISSUE|src/context/compact/compactor.rs:1-2122|low|File is ~2122 lines (the largest by far in the module) — well past the 500-line flag threshold; the public API surface (`pub enum CompactStrategy`, `pub struct CompactResult`, `pub struct CompactorConfig`, `pub struct ContextCompactor`) plus ~1500 lines of body and ~900 lines of tests all live in one file. Splitting helper modules (`hash.rs`, `cache.rs`, `windowing.rs`, `splice.rs`) would localize concerns.
ISSUE|src/context/retrieval/content_index.rs:1-1371|low|File is 1371 lines — three sub-concerns (schema/connection mgmt, RRF+proximity math, helpers like sanitize_fts_query) interleaved with no submodule split.
ISSUE|src/context/budget/cheap_passes/file_op_supersede.rs:1-984|low|File is 984 lines — algorithm code and 350+ lines of tests in one file; trimming the helper definitions out would aid review.
ISSUE|src/context/budget/mod.rs:1-1020|low|File is 1020 lines — `ContextPressure` (compute/calibrated) + `LoopDirective` + `ContextBudgetConfig` + `CompactionCircuitBreaker` + `ContextBudget` (new + accessors + before_turn + note_compaction_effect + observe_actual_usage + calibration + record_split) + 460 lines of tests, all in one file.
ISSUE|src/context/compact/compactor.rs:128|low|`static COMPACTION_CARRYOVER: Mutex<...>` is a process-wide static carrying session-keyed state; the constant `CARRYOVER_MAX_SESSIONS = 16` is hardcoded next to a five-paragraph module comment describing why it's process-wide and never on disk — a `OnceLock` or an explicit `LazyLock` `ContextCompactorStore` would document the lifecycle and make it testable across resets.
ISSUE|src/context/compact/compactor.rs:136-163|low|carryover_get / carryover_put / carryover_remove all use the poison-safe `unwrap_or_else(|e| e.into_inner())` pattern; three near-identical helpers hand-rolled — a `MutexExt` newtype would let `lock_or_recover()` express the intent once.
ISSUE|src/context/budget/pressure.rs:131-173|low|`content_ratio_with_baseline` is 40+ lines with a 14-line doc explaining the proportional-blend math; readability would benefit from extracting three small helpers (`count_cjk_chars`, `non_cjk_ratio_for`, `weighted_blend_ratio`).
ISSUE|src/context/compact/compactor.rs:880-908|low|select_window_end's final clamp `.min(hard_end).max(start + 1).min(hard_end)` repeats `.min(hard_end)` — minor stylistic noise.
ISSUE|src/context/compact/summary_utils.rs:30-60|low|Nested `MARKERS.iter().map(...).min_by_key(...)` expression is hard to read and inefficient (see Perspective 1) — extract a `fn find_first_marker(rest: &str) -> Option<(usize, &'static str)>` helper.
ISSUE|src/context/budget/cheap_passes/file_op_supersede.rs:49-50|low|Alias `read_tools = ["file_read", "Read", "read_file"]` and the parallel write/edit lists are hardcoded; `Default` impl carries them and `new` repeats the shape — this is a config-shape duplication that could fold to a single `fn default() -> Self` over a const slice.
ISSUE|src/context/budget/cheap_passes/tool_result_pruning.rs:108-148|low|First-line placeholder vs structured reduce dispatch plus an images-survive rebuild is 40 lines of message-mutation; a `build_replacement(original_text, tool_name) -> Replacement` helper would split the policy from the plumbing.
ISSUE|src/context/retrieval/content_index.rs:678-723|low|`proximity_relevance` mixes coverage and tightness into a single 45-line body; the two signals are conceptually distinct — extract `coverage_fraction(matched)` and `min_window_span(...)` (already a separate fn) and combine.
```

---

**Total: 26 issues** — 0 critical, 5 high (R7/R9 deterministic replacement of LLM judgement × 3, half-implemented feature × 1, retrieval reranking policy without prompt hook × 1), 9 medium (logic bugs, false negatives, panic-adjacent constructors, dead-config fields), 12 low (file length, deduplication opportunities, helper extraction).

**Saved to:** `/tmp/opencode/aleph-review/review-results/context.md`
