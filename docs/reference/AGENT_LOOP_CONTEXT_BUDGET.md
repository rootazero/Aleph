# Agent Loop Context Budget

> Context management architecture for the Aleph agent system.

## Overview

The context budget system manages token usage across agent turns using a three-tier architecture:

```
┌─────────────────────────────────────────────────────────────┐
│                    TIER 3: LLM Compactor                    │
│      ContextCompactor (LLM summary + fingerprint cache)     │
├─────────────────────────────────────────────────────────────┤
│                 TIER 2: Pre-flight cheap passes             │
│  file_op_supersede → tool_result_pruning → image_stripping  │
│        → structured/ (content-type-aware reduction)         │
├─────────────────────────────────────────────────────────────┤
│                    TIER 1: Inline (ingress)                 │
│  compress → hygiene (ANSI strip → structured/ → distill)    │
│      → persist original + inline signal / marker / truncate  │
└─────────────────────────────────────────────────────────────┘
```

`structured/` appears in both Tier 1 and Tier 2: the same content-type router
serves the ingress cleaner and the stale-result pruning pass, which is why it
lives in `src/tool_output/structured/` rather than under `context/budget/`.

## Tier 1: Inline Result Limiting

Applied at tool-execution time, before anything reaches the history.
Each tool declares an optional per-result size limit
(`ToolDefinition.max_result_tokens`, falling back to the global default).

When a result exceeds it, `tool_output::hygiene::clean_result_value` first
cleans the tool's **structured value** field-wise — ANSI strip, then
content-type reduction (`structured/`), then error/path distillation. This has
to happen before the value is flattened: `Value::to_string()` escapes every
newline and collapses the result onto one line, which blinds both
content-aware cleaners (see the ⚠️ note in the root `CLAUDE.md`).

`apply_result_budget` then persists the **untouched original** to the
`ToolResultStore` (on-disk + FTS-indexed via `ContentIndex`) and puts the
reduced signal inline above a `[Full output persisted]` marker, so the model
sees the failing test / compile error immediately and can still `ctx_search`
the dropped detail. Output the router could not classify keeps the older
marker-only shape — with no idea what the signal is, a head/tail slice would
just be a guess.

Two producers cap themselves so they never reach this cascade: `file_read`
sizes its window against `text.rs::read_window_tokens()`, and `file_ops`
`list`/`search`/`stats` cap their entry lists (aggregates stay exact). See
FEATURE_LOCATOR §3.14 and §3.4.
See [AGENT_LOOP_TOOL_EXECUTION.md](./AGENT_LOOP_TOOL_EXECUTION.md) for the
execution pipeline (grouping/cascade semantics live there, not here).

## Tier 2: Pre-flight Pipeline

Runs at the start of every Think turn — BEFORE the budget pressure check
and BEFORE the LLM compactor — to proactively shrink context with cheap,
LLM-free transforms.

### Wiring

`src/harness/agent/think.rs::run_turn` step 2a invokes
`HarnessDeps.preflight_pipeline.run(&mut messages, &pressure, fresh_tail)`
on every turn when `[context_budget]` is configured. The pipeline is
assembled in `src/orchestrator/harness_bridge/runner_impl.rs` and is `Some`
whenever `context_compactor` is `Some` — same opt-in as the compactor
itself. All stages share a single config-derived pressure gate
(`PreflightPipeline::with_min_pressure_ratio(cfg.preventive_floor())`), act
only outside the fresh tail, and a final token guard ensures no stage can
ever *grow* the context.

### PreflightStage Trait

`src/context/budget/preflight.rs`:

```rust
#[async_trait]
pub trait PreflightStage: Send + Sync {
    fn name(&self) -> &'static str;
    async fn prepare(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        pressure: &ContextPressure,
        fresh_tail_count: usize,
    ) -> usize; // returns tokens freed
}
```

### Cheap-Pass Stages (live)

Located in `src/context/budget/cheap_passes/`:

- **FileOpSupersedeStage** (`file_op_supersede.rs`) — a later *successful*
  write to a path supersedes earlier reads of the same path (pairing by
  `tool_call_id`, `is_error == false` required); the stale read is replaced
  by a stub naming the superseding tool.
- **ToolResultPruningStage** (`tool_result_pruning.rs`) — replaces stale
  oversized `ToolResult` text with a one-line placeholder, **preserving
  `ContentBlock::Image` blocks** (screenshots must not silently vanish;
  image lifecycle belongs to the stripping stage below). Skips when the
  placeholder wouldn't save tokens.
- **HistoricalImageStrippingStage** (`image_stripping.rs`) — drops
  `ContentBlock::Image` blocks from every message preceding the newest
  image-bearing turn (outside the fresh tail), ~1500 tokens per image.
- **structured/** (`src/tool_output/structured/`) — content-type-aware
  tool-result reduction (log / search / diff / json); tries each candidate
  shape in order and falls back to first-line truncation when none produces a
  meaningful shrink. Shared with the Tier 1 ingress cleaner. `Some(Reduction)`
  guarantees the body is smaller **in bytes** (central
  `is_meaningful_shrink`) — line counts do not bound context, since one kept
  200 KB minified line is a 94 % line reduction and a 1 % token reduction. See
  FEATURE_LOCATOR §2.7 and §3.14.

## Tier 3: LLM Compactor

`src/context/compact/compactor.rs::ContextCompactor::compact()` is invoked
by `harness/agent/think.rs` when the budget directive requires it (directive
dispatch lives in `src/context/compact/directive.rs`; the harness only
consumes the outcome). Falls back to deterministic truncation on provider
failure — and when the compaction window carries a prior running summary,
both fallback paths carry the summary **body** forward verbatim and
first-line-truncate only the raw gap behind it (gutting the summary to its
marker line would persist silent history loss through the fingerprint
cache). The 5-section summary template (Primary Request / Key Decisions /
Files & Code / Current State / Pending) is hermes-compatible.

### Execution-list carry (`plan_carry`)

Every compaction drain site funnels through
`compactor.rs::splice_preserved`, which replaces the drained range with
`[preserved user turns…, summary, execution list?]`. The trailing element is
`src/context/compact/plan_carry.rs::plan_carry_message` — the model's own
todo/plan checklist, re-injected below the summary because it is live state
the model acts on next turn, not history.

Why it is needed: the `scratchpad` tool echoes the updated checklist in every
mutating result, and that echo is the model's only structural record of what
it already finished inside a run. It is a tool-result message, so compaction
summarizes it into prose. `<execution_plan>` does not cover the gap — that
block is resolved **once per run** into the frozen system prompt, so after a
mid-run compaction it still shows the turn-0 snapshot.

Port of hermes-agent's `TodoStore.format_for_injection()`, with one deliberate
divergence: hermes filters to *active* items (its flat text list made the model
re-do finished work), while Aleph carries the **full** list. Aleph's steps are
index-addressed by `start_item` / `complete_item`, so dropping the finished
ones would corrupt every index the model is about to use; the explicit
`[x]` / `[~]` / `[ ]` glyphs already do the disambiguation.

Properties: pure (no I/O, no session key — everything comes from the messages
being drained); `None` for a window with no plan or a fully-finished one, so
calm runs pay nothing; and it recognises its own `[Execution list preserved
across context compaction]` marker, so a plan survives repeated compaction
passes after the originating tool result is long gone. Zero lines in
`src/harness/`. See FEATURE_LOCATOR §3.13.

### Fingerprint cache (cross-run carry-over)

The harness rebuilds the message list from the session log every turn, so an
in-place compaction is discarded by the next rebuild. The compactor therefore
keeps a fingerprint cache (`CompactionCache { start, end, hash, summary }`,
openteams compression-cache parity): when the previously covered range still
hashes identically in the rebuilt list, the cached summary is reapplied with
**zero API cost** (`CompactStrategy::CacheReuse`). Once the un-summarized gap
behind the summary grows past 8 messages / ~4 K estimated tokens, one LLM
merge folds the gap into the prior summary (fed explicitly as prior state —
no re-summarizing, no paraphrase decay) and the cache cover widens
monotonically. Any change inside the covered prefix (e.g. a preflight pass
pruning differently) misses the hash and falls back to a full recompaction.

Contract details (locked by tests, see FEATURE_LOCATOR §2.14/§2.15):

- **Coordinates**: `store_cache` records `[window_start, window_end)` — the
  exact hashed range. (Recording `cut_end` was a bug: any 48 K-budget window
  clamp made every turn miss.)
- **Cross-run carry-over** (`COMPACTION_CARRYOVER`, 2026-07-17): the
  compactor instance is per-run, but the cache is seeded from / written
  through to a process-wide per-session slot (bounded, insertion-order
  eviction, purged on hash miss) — the session-keyed twin of
  `CALIBRATION_CARRYOVER`. Without it every run boundary re-paid the
  side-channel summarization call and the re-worded summary re-keyed the
  provider's message-prefix cache. Hash validation makes stale carry-over
  fail-safe: rewritten history between runs simply misses.
- **Watchdog interplay**: `compact()` notifies the process-wide
  `CacheMonitor` (so post-compaction provider-cache misses aren't flagged)
  for every strategy EXCEPT `Skipped` and `CacheReuse` — a byte-identical
  reapply leaves the provider-visible prefix unchanged, and a miss there is
  precisely the stable-prefix bug the watchdog exists to catch.

## Token-Estimate Calibration (server-observed feedback)

Every tier above reacts to a single number: `ContextPressure::ratio`, derived
from a heuristic char-per-token estimate (`pressure.rs::detect_content_ratio`,
1.5 CJK / 2.5 code / 3.5 prose). A fixed ratio is necessarily wrong for any
given conversation's real token mix, so the budget **calibrates the estimate
against the provider's reported prompt size** after each turn.

### Mechanism

`src/context/budget/mod.rs`:

```rust
impl ContextBudget {
    /// Feed back the provider's ground-truth prompt size for the request just sent.
    pub fn observe_actual_usage(&mut self, observed_prompt_tokens: usize);
}
```

- `before_turn()` / `note_compaction_effect()` scale their `ContextPressure`
  snapshot by `self.calibration` (via `ContextPressure::calibrated`). Within a
  run, until the first observation `calibration` is `None` → factor `1.0`.
- **Cross-run carry-over** (`CALIBRATION_CARRYOVER`,
  `src/orchestrator/harness_bridge/runner_impl.rs`): the EWMA factor a run
  converged to is stored keyed by model id and seeded into the next run on
  the *same* model (`ContextBudget::seed_calibration`) — the first
  `before_turn` of a follow-up run (the one carrying the full accumulated
  history, where drift is largest) no longer starts uncalibrated. Breaker /
  diminishing-returns / split counters remain strictly per-run; a model
  switch misses the slot and starts uncalibrated as before.
- After each LLM turn, `harness/agent/think.rs` calls
  `observe_actual_usage(usage.prompt_tokens_total())`. The saved `last_pressure`
  is the calibrated estimate of *that exact prompt*, so `observed / estimated`
  (with the previous factor backed out) is the residual error. It is clamped to
  `[0.25, 4.0]` (rejecting transient noise — mid-flight resends, degenerate
  usage reports) and EWMA-smoothed (`α = 0.3`) into the running multiplier.
- `TokenUsage::prompt_tokens_total()` (`src/providers/adapter.rs`) folds the
  cached + cache-creation portions back in — unconditionally summing all three
  counters — so a warm cache hit (tiny `input_tokens`) doesn't look like the
  prompt shrank. There is no Anthropic-vs-OpenAI convention detection: every
  adapter normalises its provider's usage into **disjoint** counters before
  they are recorded (Anthropic reports them that way natively; the OpenAI and
  Gemini paths subtract the cached portion out of the inclusive prompt total).
  Guessing the convention from token magnitudes was deleted as a bug — it
  misclassified every Anthropic turn whose cached prefix was smaller than its
  fresh input, and the same heuristic survived in the usage rollup until
  §2.18, where it over-reported the hit rate by up to 2x in exactly the
  degraded regime.
- `compact_to_fit` (`src/context/compact/fit.rs`) divides its floor target by
  `budget.calibration()` to convert back into raw-estimate space — the
  eviction loop measures raw, so a calibrated-space target would stall the
  floor when the factor exceeds 1.

### Effect

The estimate converges to *this conversation's* true tokenizer ratio within a
few turns, adapting to content mix, the provider's tokenizer, and cache
behaviour the static ratio cannot capture. This is purely an accuracy
improvement to the number that already drives compaction — it adds no new
decision category and makes no LLM call (R7/R10-safe). Compared to codex's
one-shot `ServerObserved` prefill snapshot, the EWMA multiplier is continuous:
it also corrects the estimate of the *growing tail* the provider hasn't yet
counted.

## Provider Prompt Cache Interplay

The context layer's output is the prefix the provider caches; the two are
co-designed (FEATURE_LOCATOR §2.15):

- **Anthropic**: `cache_control` breakpoints (system stable tail + last-3
  sliding messages, ≤4 total) are injected at request-build time in
  `src/providers/protocols/anthropic/adapter/cache.rs`; persisted history
  never carries markers. TTL tiers via `cache_retention = off|short|long`.
- **OpenAI**: content-addressed `prompt_cache_key`
  (`openai_common/prompt_cache.rs::derive_prompt_cache_key`, static-prefix
  hash with session-id fallback) + `prompt_cache_retention: "24h"` on
  official endpoints when retention is `long`.
- **Watchdog**: `CacheMonitor` (per-agent, armed only after observed cache
  activity) warns on ≥3 consecutive misses — the live alarm for a broken
  stable prefix.
- **Contract test**: `providers/protocols/anthropic/adapter_tests/prefix_stability.rs`
  pins "turn N+1 is a strict prefix extension of turn N" on raw request
  bodies.

## Budget Structure

```rust
pub struct Budget {
    pub max_tokens: usize,
    pub warning_threshold: f32,
    pub critical_threshold: f32,
}
```

### Per-model threshold overrides (`[[context_budget.model_thresholds]]`)

`token_budget` is already model-aware — it is sized for the *smallest* context
window on the resolved failover chain (`derive_chain_min_budget`,
`src/orchestrator/deps_builder.rs`). The conservative consequence: a single
**narrow fallback sibling** caps the compaction budget for an otherwise-wide
primary, so the primary compacts earlier than its real window requires. When the
chain-min winner is *not* the primary and the budget falls below
`CHAIN_MIN_UNDERCUT_WARN_FRACTION` (60%) of the primary's own usable window,
`build_context_budget_config` logs a one-line startup advisory naming the
offending sibling and the fix (reorder/trim `[fallback_provider].chain`, or pin
an explicit `token_budget`). It is observability only — the safe smaller budget
still stands. The **trigger fractions** can also vary per
model: a narrow 200k-window model often wants to compact earlier than a 1M model
(less absolute headroom above the warning line). Declare overrides keyed off the
same chain-min model that sizes the budget:

```toml
[context_budget]
enabled = true
warning_threshold = 0.70   # global defaults
critical_threshold = 0.85

[[context_budget.model_thresholds]]
model = "kimi"             # case-insensitive substring of the model id OR provider key
warning_threshold = 0.60   # compact this narrow model sooner
critical_threshold = 0.78
```

Matching is first-wins in declaration order; the matcher is a case-insensitive
substring tested against the resolved model id and then the provider key. Each
field falls back independently to the top-level threshold, then the built-in
`0.70 / 0.85` — so an absent or non-matching override is byte-identical to the
prior behaviour. Resolved thresholds pass the same `0 < warning < critical ≤ 1.0`
defensive gate as the global config (`build_context_budget_config`), so a
mis-configured override disables the budget rather than silently degrading it.

## Related Documents

- [AGENT_LOOP_TOOL_EXECUTION.md](./AGENT_LOOP_TOOL_EXECUTION.md) - Tool execution context and pipeline
- [AGENT_LOOP_RECOVERY.md](./AGENT_LOOP_RECOVERY.md) - Truncation recovery mechanisms
