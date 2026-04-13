# Memory Sovereignty Cleanup — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete short-term memory tier, strength/confidence-based decay, and the `ValueEstimator` heuristic; replace tier-based core-set selection with category-based routing + a bounded token budget that overflows to the query-retrieval pool.

**Architecture:** Twelve sequential, atomic commits. Each commit keeps `cargo check -p alephcore` and `cargo test -p alephcore --lib` green. Ordering is dictated by dependency direction: delete consumers before producers, delete read sites before fields, delete fields before enums. Historical event-log rows are preserved via permissive deserialization.

**Tech Stack:** Rust (alephcore), serde/serde_json for events, sqlite-vec + rusqlite for storage, tokio for async.

**Spec:** [`docs/superpowers/specs/2026-04-12-memory-sovereignty-cleanup-design.md`](../specs/2026-04-12-memory-sovereignty-cleanup-design.md)

---

## File Structure

### Files modified

| Path | Role in this plan |
|---|---|
| `src/config/types/memory.rs` | Add `ContextComposerConfig { core_budget_tokens }`; remove any `[memory.value_estimator]` section. |
| `src/memory/context/enums.rs` | Delete `MemoryTier` enum; shrink `MemoryScope` (drop `Agent`, `Persona`). |
| `src/memory/context/fact.rs` | Remove `tier`, `scope`, `strength`, `confidence` fields from `MemoryFact`. |
| `src/memory/context/tests/enum_tests.rs` | Remove tier tests; update scope tests. |
| `src/memory/context/tests/fact_tests.rs` | Update fact construction to the shrunk shape. |
| `src/memory/proptest_enums.rs` | Drop tier arbiter; shrink scope arbiter. |
| `src/memory/composer.rs` | Rewrite `build_core_filter` / `build_retrieval_filter` OR delete the module entirely (branch at Task 10 based on live-caller grep). |
| `src/memory/notes/search_result.rs` | `to_memory_fact`: stop writing `tier`, `scope`, `strength`, `confidence`. |
| `src/memory/scoring_pipeline/mod.rs` | Remove `ImportanceWeightStage` from pipeline builder. |
| `src/memory/scoring_pipeline/stages/mod.rs` | Remove `pub mod importance_weight`. |
| `src/memory/events/commands.rs` | Delete `ApplyDecayCommand`. |
| `src/memory/events/mod.rs` | Remove `StrengthDecayed` variant; fold `FactCreated` to drop `tier`/`scope`. |
| `src/memory/events/handler.rs` | Drop `ApplyDecay` routing. |
| `src/memory/events/projector.rs` | Drop `StrengthDecayed` match arm; log + skip unknown variants on replay. |
| `src/memory/events/traveler.rs` | Drop decay-event renderer arm. |
| `src/memory/events/migration.rs` | Strip any migration code for `StrengthDecayed`. |
| `src/memory/store/types.rs` | Drop `SearchFilter::with_tier` and `with_scope_stack`; update tests. |
| `src/memory/store/sqlite/*.rs` | Scrub any `tier` / `strength` / `confidence` reads on DTO assembly. |
| `src/memory/session_compactor/summary_engine.rs` | Scrub `MemoryTier` / `.strength` references on the DTO path. |
| `src/memory/mod.rs` | Remove `pub use` re-exports for deleted types. |
| `src/memory/integration_tests/mod.rs` | Update fact construction to shrunk shape. |
| `src/memory/ripple/tests.rs` | Same. |

### Files deleted

| Path | Reason |
|---|---|
| `src/memory/scoring_pipeline/stages/importance_weight.rs` | Self-referential: scales retrieval score by a confidence derived from that same score. |
| `src/memory/value_estimator/mod.rs` | Heuristic importance scorer; LLM reads content instead. |
| `src/memory/value_estimator/estimator.rs` | Same. |
| `src/memory/value_estimator/llm_tests.rs` | Same. |
| `src/memory/value_estimator/cortex.rs` | Same. Coordinate with `memory-legacy-cleanup` spec if it also touches this. |

---

## Execution Prerequisites

Before starting Task 1, confirm baseline:

```bash
cargo check -p alephcore
cargo test -p alephcore --lib --no-fail-fast
```

Both must pass. If either is red on `main`, fix that first — this plan assumes a green baseline.

Run all grep/verification commands from the repo root (`/Volumes/TBU4/Workspace/Aleph`).

---

## Task 1: Add `ContextComposerConfig`

Additive change. No consumers yet; proves the wiring works end-to-end before anything breaks.

**Files:**
- Modify: `src/config/types/memory.rs`

- [ ] **Step 1: Write the failing test**

Add to the bottom of `src/config/types/memory.rs` inside `#[cfg(test)] mod tests`:

```rust
#[test]
fn context_composer_config_default_budget_is_2000() {
    let cfg = ContextComposerConfig::default();
    assert_eq!(cfg.core_budget_tokens, 2000);
}

#[test]
fn context_composer_config_roundtrips_toml() {
    let toml_src = "core_budget_tokens = 3000\n";
    let cfg: ContextComposerConfig = toml::from_str(toml_src).unwrap();
    assert_eq!(cfg.core_budget_tokens, 3000);
}

#[test]
fn context_composer_config_default_on_empty_toml() {
    let cfg: ContextComposerConfig = toml::from_str("").unwrap();
    assert_eq!(cfg.core_budget_tokens, 2000);
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p alephcore --lib context_composer_config -- --nocapture
```

Expected: compile error `cannot find struct ContextComposerConfig`.

- [ ] **Step 3: Implement `ContextComposerConfig`**

Add to `src/config/types/memory.rs` (near other `*Config` structs in the file):

```rust
/// Controls how `ContextComposer` assembles the always-loaded core set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextComposerConfig {
    /// Maximum bytes of `persona/` + `preference/` notes to inject into
    /// the system prompt per composition. Notes that don't fit remain
    /// reachable via query-time retrieval.
    #[serde(default = "default_core_budget_tokens")]
    pub core_budget_tokens: usize,
}

impl Default for ContextComposerConfig {
    fn default() -> Self {
        Self {
            core_budget_tokens: default_core_budget_tokens(),
        }
    }
}

fn default_core_budget_tokens() -> usize {
    2000
}
```

Then add the field to the enclosing `MemoryConfig` struct (find the struct declaration near the top of the file) — add a field and the matching default:

```rust
    #[serde(default)]
    pub context_composer: ContextComposerConfig,
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p alephcore --lib context_composer_config
cargo check -p alephcore
```

Expected: 3 new tests pass; `cargo check` is clean.

- [ ] **Step 5: Commit**

```bash
git add src/config/types/memory.rs
git commit -m "memory: add ContextComposerConfig with core_budget_tokens"
```

---

## Task 2: Event-log permissive deserialization

Adds forward-compat so historical `StrengthDecayed` rows (written by current-main code) keep deserializing after Task 5 deletes the variant. The replay path logs and skips unknown variants instead of panicking.

**Files:**
- Modify: `src/memory/events/projector.rs` (or wherever event replay reads rows — verify with grep)

- [ ] **Step 1: Locate the event replay path**

```bash
grep -rn "append_memory_event\|fold_events_to_fact\|from_str.*MemoryEvent\|serde_json::from_str.*Event" src/memory/
```

Expected output includes at least `src/memory/events/handler.rs` (writer) and `src/memory/events/projector.rs` (reader). Identify the single function that deserializes event JSON from SQLite rows; that is the target site.

- [ ] **Step 2: Write the failing test**

In the test module at the bottom of the file you identified in Step 1 (typically `src/memory/events/projector.rs`), add:

```rust
#[test]
fn projector_skips_unknown_event_variants() {
    let agent = "test-agent";
    let fact_id = "unit-test-fact";
    // Simulate an event envelope that is valid JSON but uses a variant
    // that doesn't exist in the current code.
    let orphan_row = serde_json::json!({
        "fact_id": fact_id,
        "seq": 1,
        "ts": 1_700_000_000_i64,
        "actor": { "kind": "Agent" },
        "event": { "StrengthDecayed": { "old_strength": 1.0, "new_strength": 0.9 } }
    });
    let rows = vec![orphan_row.to_string()];

    // Function under test — replace with the actual function name from
    // Step 1 once identified.
    let result = EventProjector::fold_events_to_fact(&rows, agent, fact_id);

    // Unknown variant must be skipped, not fatal. With only an unknown
    // variant present, the projection is `None` (no FactCreated ever
    // seen), not an error.
    assert!(result.is_ok(), "unknown variant must not be an error");
    assert!(result.unwrap().is_none(), "projection should be None when only unknown events seen");
}
```

Adjust signatures to match the actual function discovered in Step 1. If the actual replay path returns `Result<Option<MemoryFact>, ...>` differently, mirror that.

- [ ] **Step 3: Run test to verify it fails**

```bash
cargo test -p alephcore --lib projector_skips_unknown_event_variants -- --nocapture
```

Expected: test fails — either panics on the unknown variant, errors, or Successfully projects (if the variant already exists because Task 5 hasn't run yet). If it passes because the variant still exists, proceed anyway — the test is protecting against future deletions.

- [ ] **Step 4: Make the deserialization permissive**

In the replay path, change the deserialization so unknown variants are logged and skipped:

```rust
// Before: serde_json::from_str::<MemoryEventEnvelope>(row)?
// After:
match serde_json::from_str::<MemoryEventEnvelope>(row) {
    Ok(envelope) => { /* existing handling */ }
    Err(e) => {
        tracing::warn!(
            error = %e,
            row_snippet = row.chars().take(200).collect::<String>().as_str(),
            "skipping unrecognized memory event during replay"
        );
        continue;
    }
}
```

If the current code uses `.collect::<Result<Vec<_>, _>>()?`, refactor to a manual loop that can skip errors.

- [ ] **Step 5: Run test to verify it passes**

```bash
cargo test -p alephcore --lib projector_skips_unknown_event_variants
cargo check -p alephcore
```

Expected: pass + clean.

- [ ] **Step 6: Commit**

```bash
git add src/memory/events/projector.rs
git commit -m "memory: log+skip unknown event variants during replay"
```

---

## Task 3: Delete `importance_weight` pipeline stage

Self-referential stage: `score *= 0.7 + 0.3 * confidence`, where `confidence = score` after the note-layer migration. Pure noise.

**Files:**
- Delete: `src/memory/scoring_pipeline/stages/importance_weight.rs`
- Modify: `src/memory/scoring_pipeline/stages/mod.rs`
- Modify: `src/memory/scoring_pipeline/mod.rs`

- [ ] **Step 1: Update the pipeline builder test expectation**

In `src/memory/scoring_pipeline/mod.rs`, the existing tests assert `stage_count() == 7`. After removal this becomes 6. Update:

```rust
#[test]
fn default_pipeline_creates_six_stages() {
    let pipeline = ScoringPipeline::default();
    assert_eq!(pipeline.stage_count(), 6);
}

#[test]
fn from_config_creates_six_stages() {
    let cfg = ScoringPipelineConfig::default();
    let pipeline = ScoringPipeline::from_config(&cfg);
    assert_eq!(pipeline.stage_count(), 6);
}
```

Rename from `default_pipeline_creates_seven_stages` / `from_config_creates_seven_stages`.

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p alephcore --lib default_pipeline_creates_six_stages from_config_creates_six_stages
```

Expected: both fail — current count is 7.

- [ ] **Step 3: Remove the stage from the pipeline builder**

In `src/memory/scoring_pipeline/mod.rs`:

Delete line: `use stages::importance_weight::ImportanceWeightStage;`

In `from_config`, remove: `Box::new(ImportanceWeightStage),`

The resulting stage list:

```rust
let stages: Vec<Box<dyn ScoringStage>> = vec![
    Box::new(CosineRerankStage),
    Box::new(RecencyBoostStage),
    Box::new(LengthNormalizationStage),
    Box::new(TimeDecayStage),
    Box::new(HardMinScoreStage),
    Box::new(MmrDiversityStage),
];
```

Also update the stage-order doc comment just above `from_config` to reflect six stages.

- [ ] **Step 4: Remove the module declaration**

In `src/memory/scoring_pipeline/stages/mod.rs`, delete:

```rust
pub mod importance_weight;
```

- [ ] **Step 5: Delete the stage file**

```bash
git rm src/memory/scoring_pipeline/stages/importance_weight.rs
```

- [ ] **Step 6: Fix the end-to-end pipeline test**

The existing `test_full_pipeline_end_to_end` test in `src/memory/scoring_pipeline/mod.rs` has a comment referencing "importance weight ≈ 0.79" in its math. The filter threshold may now let `marginal fact` (score 0.30) survive after time decay only. Recompute:

```text
After time_decay (180 days at 60d half-life):
  decay = 0.5 + 0.5 * exp(-180/60) = 0.5 + 0.5 * 0.0498 ≈ 0.525
  score = 0.30 * 0.525 ≈ 0.158  → below 0.35, still filtered
```

So the assertion still holds. Update only the comment:

```rust
// low-score candidate should be filtered out
// (starts at 0.30, then time decay ≈ 0.525 → ~0.158 < 0.35 threshold)
```

- [ ] **Step 7: Run tests**

```bash
cargo test -p alephcore --lib scoring_pipeline
cargo check -p alephcore
```

Expected: all scoring pipeline tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/memory/scoring_pipeline/
git commit -m "memory: delete importance_weight scoring stage"
```

---

## Task 4: Delete `ValueEstimator`

Pure heuristic (keyword bonus + LLM scorer) that produces `MemoryFact.confidence`. No consumer remains after Task 3.

**Files:**
- Delete: `src/memory/value_estimator/` (entire directory)
- Modify: `src/memory/mod.rs`
- Modify: `src/config/types/memory.rs` (if a config section exists)

- [ ] **Step 1: Map the blast radius**

```bash
grep -rn "value_estimator\|ValueEstimator\|CortexValueEstimator\|LlmScorer" src/ Cargo.toml
```

Expected hit set: the four files inside `src/memory/value_estimator/`, the `pub mod value_estimator` line in `src/memory/mod.rs`, any `pub use` re-exports, possibly a `src/memory/cortex/` file (coordinate with memory-legacy-cleanup spec — if cortex still references value_estimator, leave cortex alone and delete only the module). List each hit before editing.

- [ ] **Step 2: Delete the directory**

```bash
git rm -r src/memory/value_estimator/
```

- [ ] **Step 3: Remove the module declaration**

In `src/memory/mod.rs`, delete the `pub mod value_estimator;` line (and any `pub use value_estimator::...` re-exports).

- [ ] **Step 4: Remove config section (if present)**

Inspect `src/config/types/memory.rs` for a `value_estimator` field on `MemoryConfig` or any `ValueEstimatorConfig` struct. If present, delete both. If absent, skip.

- [ ] **Step 5: Fix cortex references**

```bash
grep -rn "value_estimator\|ValueEstimator" src/memory/cortex/
```

If hits remain in `cortex/dreaming.rs` or `cortex/integration.rs`, delete the import lines and any call sites (cortex is deprecated anyway — see its module doc). If the cortex changes grow beyond ~20 lines, stop and coordinate with memory-legacy-cleanup spec instead of expanding scope.

- [ ] **Step 6: Verify clean**

```bash
grep -rn "value_estimator\|ValueEstimator\|CortexValueEstimator\|LlmScorer" src/
```

Expected: zero hits.

- [ ] **Step 7: Run build and tests**

```bash
cargo check -p alephcore
cargo test -p alephcore --lib --no-fail-fast
```

Expected: green.

- [ ] **Step 8: Commit**

```bash
git add -A src/memory/ src/config/
git commit -m "memory: delete value_estimator module"
```

---

## Task 5: Delete `ApplyDecayCommand` and `StrengthDecayed`

Removes the decay command, the event variant it emits, the projector arm that folds it, the handler routing, and the time-travel renderer. Task 2's permissive deserializer covers historical rows.

**Files:**
- Modify: `src/memory/events/commands.rs`
- Modify: `src/memory/events/mod.rs`
- Modify: `src/memory/events/handler.rs`
- Modify: `src/memory/events/projector.rs`
- Modify: `src/memory/events/traveler.rs`
- Modify: `src/memory/events/migration.rs` (if applicable)

- [ ] **Step 1: Map the blast radius**

```bash
grep -rn "ApplyDecayCommand\|StrengthDecayed\|strength_at_invalidation\|apply_decay" src/memory/
```

Every hit is an edit or delete site. Walk the list top-to-bottom.

- [ ] **Step 2: Delete `ApplyDecayCommand` from `commands.rs`**

In `src/memory/events/commands.rs`, remove the struct:

```rust
// DELETE
pub struct ApplyDecayCommand {
    pub fact_ids_with_strength: Vec<(String, f32, f32)>,
    pub decay_factor: f32,
    pub correlation_id: Option<String>,
}
```

- [ ] **Step 3: Delete `StrengthDecayed` from the event enum**

In `src/memory/events/mod.rs`, locate the `MemoryEvent` enum and remove the `StrengthDecayed { ... }` variant entirely. Keep every other variant (`FactCreated`, `FactContentUpdated`, etc.).

- [ ] **Step 4: Drop projector match arm**

In `src/memory/events/projector.rs`, find the `match event { ... }` inside `fold_events_to_fact` (or equivalent) and delete the `MemoryEvent::StrengthDecayed { .. } => { ... }` arm.

If the match was exhaustive, the Rust compiler now errors the match closure. If so, the match is automatically exhaustive because the variant is gone — no wildcard needed.

- [ ] **Step 5: Drop handler routing**

In `src/memory/events/handler.rs`, find any method that routes an `ApplyDecayCommand` (e.g., `handle(ApplyDecayCommand)` impl). Delete the impl block and any match arm in a dispatcher that references decay.

- [ ] **Step 6: Drop time-travel renderer**

In `src/memory/events/traveler.rs`, find any match arm on `MemoryEvent::StrengthDecayed` and delete.

- [ ] **Step 7: Drop migration code (if present)**

In `src/memory/events/migration.rs`, if there is any rename or shape-migration helper for `StrengthDecayed`, delete it.

- [ ] **Step 8: Run full test suite**

```bash
cargo check -p alephcore
cargo test -p alephcore --lib --no-fail-fast
```

Expected: green. The test from Task 2 (`projector_skips_unknown_event_variants`) should now actually exercise the skip path — good news.

- [ ] **Step 9: Commit**

```bash
git add src/memory/events/
git commit -m "memory: delete ApplyDecayCommand and StrengthDecayed event"
```

---

## Task 6: Stop writing removed fields in `NoteSearchResult::to_memory_fact`

Prepares for Task 7 (field removal) by first eliminating the write sites. Other writers are in events/commands — addressed in Task 10.

**Files:**
- Modify: `src/memory/notes/search_result.rs`

- [ ] **Step 1: Update the existing test expectation**

The current `to_memory_fact_uses_path_as_id` test asserts `fact.confidence == 0.95`. After this task, `confidence` is no longer set on the fact (field itself goes away in Task 7; for now, just stop writing it). Change the assertion block to:

```rust
// Replace confidence check with tags-as-source-ids check (already covered
// by tags_forwarded_as_source_memory_ids) + is_valid + agent.
// Delete the old: assert!((fact.confidence - 0.95).abs() < f32::EPSILON);
```

- [ ] **Step 2: Remove the field assignments**

In `src/memory/notes/search_result.rs`, update `to_memory_fact`:

```rust
pub fn to_memory_fact(&self, agent_id: &str) -> MemoryFact {
    let note_type = NoteType::from_str_or_other(&self.category);
    let mut fact = MemoryFact::new(self.content.clone(), note_type, self.tags.clone());
    fact.id = self.path.clone();
    fact.path = format!("note://{}", self.path);
    fact.agent = agent_id.to_string();
    fact.created_at = self.created_at;
    fact.updated_at = self.updated_at;
    fact.is_valid = true;
    // DELETED: fact.confidence = self.score;
    // DELETED: fact.tier = MemoryTier::LongTerm;
    // DELETED: fact.scope = MemoryScope::Global;
    // DELETED: fact.strength = 1.0;
    fact
}
```

Also update the doc comment: drop the line about defaults `tier=LongTerm, scope=Global, strength=1.0`.

- [ ] **Step 3: Remove the now-unused imports**

At the top of the file:

```rust
// Before
use crate::memory::context::{MemoryFact, MemoryScope, MemoryTier, NoteType};
// After
use crate::memory::context::{MemoryFact, NoteType};
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p alephcore --lib search_result
cargo check -p alephcore
```

Expected: 3 tests in this module pass.

- [ ] **Step 5: Commit**

```bash
git add src/memory/notes/search_result.rs
git commit -m "memory: stop writing tier/scope/strength/confidence on note-to-fact bridge"
```

---

## Task 7: Shrink `MemoryFact` (remove `strength`, `confidence`, `tier`, `scope`)

Deletes four fields in one commit. Touches every writer and reader. Any non-test consumer that still reads these fields must be updated; the compiler will catch them.

**Files:**
- Modify: `src/memory/context/fact.rs`
- Modify: `src/memory/events/commands.rs` (remove `tier`, `scope`, `confidence` from `CreateFactCommand`)
- Modify: `src/memory/events/mod.rs` (remove `tier`, `scope` from `FactCreated` variant)
- Modify: `src/memory/events/handler.rs` (stop populating removed command fields)
- Modify: `src/memory/events/projector.rs` (stop assigning removed fact fields)
- Modify: `src/memory/context/tests/fact_tests.rs`
- Modify: `src/memory/integration_tests/mod.rs`
- Modify: `src/memory/ripple/tests.rs`
- Modify: `src/memory/session_compactor/summary_engine.rs`
- Modify: `src/memory/scoring_pipeline/mod.rs` (test constructors use `.confidence = …`)
- Modify: `src/memory/store/sqlite/*.rs` (if any DTO assembly reads these fields)

- [ ] **Step 1: Map the blast radius**

```bash
grep -rn "\.tier\b\|\.scope\b\|\.strength\b\|\.confidence\b" src/ --include='*.rs' | grep -v 'similarity_score\|decay_invalidated' | tee /tmp/mf-refs.txt
wc -l /tmp/mf-refs.txt
```

Save the hit list — every entry is a code site to inspect. Comment (`//`), doc (`///`), or frontmatter (`"confidence:"` YAML string) hits can be ignored; code hits must be edited or deleted.

- [ ] **Step 2: Remove the fields from `MemoryFact`**

In `src/memory/context/fact.rs`, delete:

- The `default_strength` helper function.
- The `#[serde(default = "default_strength")] pub strength: f32,` line.
- The `pub confidence: f32,` line (plus its doc comment).
- The `pub tier: MemoryTier,` line (plus its doc comment).
- The `pub scope: MemoryScope,` line (plus its doc comment).
- Any `MemoryTier` / `MemoryScope` import line at the top if they become unused.

Update the top-of-file `use super::enums::{...}` to drop `MemoryScope, MemoryTier`.

Update the struct's `impl MemoryFact::new` constructor (and any other helper constructors) to stop initializing the deleted fields.

- [ ] **Step 3: Remove fields from `CreateFactCommand`**

In `src/memory/events/commands.rs`:

```rust
pub struct CreateFactCommand {
    pub content: String,
    pub note_type: NoteType,
    // DELETED: pub tier: MemoryTier,
    // DELETED: pub scope: MemoryScope,
    pub path: String,
    pub namespace: String,
    pub agent: String,
    // DELETED: pub confidence: f32,
    pub source: FactSource,
    pub source_memory_ids: Vec<String>,
    pub actor: EventActor,
    pub correlation_id: Option<String>,
}
```

Also update the import line at the top of `commands.rs` to drop `MemoryScope, MemoryTier`.

- [ ] **Step 4: Remove fields from `FactCreated` event**

In `src/memory/events/mod.rs`, change:

```rust
FactCreated {
    fact_id: String,
    content: String,
    note_type: NoteType,
    // DELETED: tier: MemoryTier,
    // DELETED: scope: MemoryScope,
    path: String,
    namespace: String,
    agent: String,
    // DELETED: confidence: f32,
    source: FactSource,
    source_memory_ids: Vec<String>,
},
```

Update the imports/top of `events/mod.rs` to drop `MemoryScope, MemoryTier` if unused.

- [ ] **Step 5: Update handler, projector, and migration**

In `src/memory/events/handler.rs`, find the point where `CreateFactCommand` is translated into a `FactCreated` event and drop the passthrough of removed fields.

In `src/memory/events/projector.rs`, in the `FactCreated { .. } =>` match arm, stop assigning `fact.tier = ...`, `fact.scope = ...`, `fact.confidence = ...`. The arm now only sets fields that still exist.

In `src/memory/events/migration.rs`, if there is code that renames `tier` / `scope` in old rows, delete it (permissive deserializer from Task 2 handles unknown fields).

- [ ] **Step 6: Update test constructors**

For every hit in `/tmp/mf-refs.txt` that is in a test (`#[cfg(test)]` module or `tests/` path), delete the line setting the removed field. The struct literal pattern usually looks like:

```rust
let mut fact = MemoryFact::new(...);
fact.confidence = 0.5;       // DELETE this line
fact.tier = MemoryTier::Core; // DELETE this line
```

After deletion, the call site becomes valid under the shrunk struct.

Apply this pattern to:

- `src/memory/context/tests/fact_tests.rs`
- `src/memory/integration_tests/mod.rs`
- `src/memory/ripple/tests.rs`
- `src/memory/scoring_pipeline/mod.rs` (the two tests in the bottom of file using `.confidence`)
- `src/memory/proptest_enums.rs` (if it constructs `MemoryFact`)

- [ ] **Step 7: Update session compactor**

```bash
grep -n "MemoryTier\|\.tier\b\|\.strength\b" src/memory/session_compactor/
```

For each hit, remove the reference. If it's a DTO field being set (`fact.tier = ...`), delete the line. If it's a branch on tier, collapse to the non-tier path.

- [ ] **Step 8: Verify clean**

```bash
cargo check -p alephcore
```

If errors remain, iterate: the compiler error list IS the blast-radius map. Each error is one more deletion.

- [ ] **Step 9: Run tests**

```bash
cargo test -p alephcore --lib --no-fail-fast
```

Expected: green. If a test fails because an assertion was coupled to removed fields, delete the assertion.

- [ ] **Step 10: Commit**

```bash
git add -A src/memory/
git commit -m "memory: shrink MemoryFact by removing tier/scope/strength/confidence"
```

---

## Task 8: Remove `SearchFilter::with_tier` and `with_scope_stack`

With `MemoryFact.tier` and `MemoryFact.scope` gone, the filter builder methods have nothing to filter on. Delete them.

**Files:**
- Modify: `src/memory/store/types.rs`

- [ ] **Step 1: Map callers**

```bash
grep -rn "with_tier\|with_scope_stack\|scope_stack_clause" src/ --include='*.rs'
```

Expected hits: `src/memory/store/types.rs` (definition + tests), `src/memory/composer.rs` (callers). The composer is about to change in Task 10; that's where the caller goes.

- [ ] **Step 2: Delete the two builder methods and supporting state**

In `src/memory/store/types.rs`:

- Delete the `pub fn with_tier(mut self, tier: MemoryTier) -> Self { ... }` impl block.
- Delete the `pub fn with_scope_stack(mut self, persona_id: Option<&str>, workspace: &str) -> Self { ... }` impl block.
- Delete the `tier: Option<MemoryTier>` field on `SearchFilter` if present.
- Delete the `scope_stack_clause: Option<String>` field.
- Delete any SQL assembly code in `to_lance_filter()` (or `to_sqlite_filter()`) that consumes `self.tier` or `self.scope_stack_clause`.
- Remove `MemoryTier` from imports at top of file.

- [ ] **Step 3: Delete the now-stale tests**

In the same file, remove:

- `fn search_filter_supports_tier`
- `fn search_filter_scope_stack_generates_or_clause`
- `fn search_filter_scope_stack_without_persona`
- `fn search_filter_tier_with_scope_stack`

- [ ] **Step 4: Verify composer still compiles (it does not — fix in Task 10)**

```bash
cargo check -p alephcore
```

Expected: errors in `src/memory/composer.rs` because it still calls `with_tier` and `with_scope_stack`. Task 10 fixes this. **Skip ahead: combine this task's commit with Task 10 into a single commit** rather than landing in a broken state.

Revise: **do not commit yet**. Task 8 produces a WIP change set that must land atomically with Task 10.

---

## Task 9: Decide Composer's fate

Based on the earlier grep, `ContextComposer` has no callers outside `src/memory/mod.rs` re-exports. Under P6 (KISS/YAGNI), delete the module entirely rather than rewriting it for nonexistent consumers.

**Files:**
- Verify: `src/memory/composer.rs`
- Verify: `src/memory/mod.rs`

- [ ] **Step 1: Re-verify no live consumers**

```bash
grep -rn "ContextComposer\|ComposedContext\|CompositionRequest" src/ --include='*.rs'
```

Expected hits: only `src/memory/composer.rs` and the `pub mod composer;` / `pub use composer::...` in `src/memory/mod.rs`.

- [ ] **Step 2: If consumers exist outside those two files, BRANCH**

If Step 1 surfaces a real caller (e.g., `agent_loop/` or `builtin_tools/` uses `ContextComposer::build_core_filter`), this task and Task 10 become a **rewrite** per §8 of the spec. In that case:

- Rewrite `build_core_filter` to return a `SearchFilter` that matches `category IN ('persona', 'preference') AND agent = ? AND namespace = ? AND is_valid = true`.
- Rewrite `build_retrieval_filter` to drop all tier/scope; match only `agent + namespace + is_valid`.
- Add a new `select_core_set(notes, budget)` function implementing §9 of the spec.
- Add the §8.1 / §9 tests.

For the common case (no callers), proceed to Step 3.

- [ ] **Step 3: Delete the Composer module**

```bash
git rm src/memory/composer.rs
```

In `src/memory/mod.rs`, delete:

```rust
pub mod composer;
pub use composer::{ContextComposer, ComposedContext, CompositionRequest};
```

(Adjust the exact re-export list to match what's there.)

- [ ] **Step 4: Verify**

```bash
cargo check -p alephcore
cargo test -p alephcore --lib --no-fail-fast
```

Expected: both green. Task 8's `SearchFilter` edits should now have no lingering broken callers.

- [ ] **Step 5: Commit (combined with Task 8)**

```bash
git add -A src/memory/store/types.rs src/memory/composer.rs src/memory/mod.rs
git commit -m "memory: delete dead ContextComposer and SearchFilter tier/scope methods"
```

---

## Task 10: Delete `MemoryTier` enum

After Task 7, no struct field holds a `MemoryTier`. After Task 8, no SearchFilter reads one. The enum is unreachable.

**Files:**
- Modify: `src/memory/context/enums.rs`
- Modify: `src/memory/context/tests/enum_tests.rs`
- Modify: `src/memory/proptest_enums.rs`
- Modify: `src/memory/mod.rs`
- Modify: `src/memory/context/mod.rs` (likely re-exports)

- [ ] **Step 1: Verify no references**

```bash
grep -rn "MemoryTier" src/ --include='*.rs'
```

Expected hits: definition in `enums.rs`, tests in `tests/enum_tests.rs`, arbiter in `proptest_enums.rs`, re-exports in module roots.

- [ ] **Step 2: Delete the enum and its impls**

In `src/memory/context/enums.rs`, delete:

- The `pub enum MemoryTier { Core, ShortTerm, LongTerm }` declaration.
- The entire `impl MemoryTier` block (any `as_str`, `from_str`, etc.).
- Any `impl Default for MemoryTier` block.
- Any `impl Display for MemoryTier` / `serde` trait impls.

- [ ] **Step 3: Delete tier tests**

In `src/memory/context/tests/enum_tests.rs`, delete every test referencing `MemoryTier`.

- [ ] **Step 4: Drop tier arbiter**

In `src/memory/proptest_enums.rs`, delete the arbitrary strategy for `MemoryTier` (typically `any::<MemoryTier>()` generator).

- [ ] **Step 5: Remove re-exports**

In `src/memory/context/mod.rs` and `src/memory/mod.rs`, find `pub use ...::MemoryTier` lines and delete.

- [ ] **Step 6: Verify**

```bash
grep -rn "MemoryTier" src/ --include='*.rs'
cargo check -p alephcore
cargo test -p alephcore --lib --no-fail-fast
```

Expected: zero grep hits, green check, green tests.

- [ ] **Step 7: Commit**

```bash
git add -A src/memory/
git commit -m "memory: delete MemoryTier enum"
```

---

## Task 11: Shrink `MemoryScope` to `{ Global, SessionLocal }`

`Agent` and `Persona` variants have no remaining readers after the Composer deletion and the `CreateFactCommand` / `FactCreated` shrink. `SessionLocal` is preserved for session-compactor semantics per the spec.

**Files:**
- Modify: `src/memory/context/enums.rs`
- Modify: `src/memory/context/tests/enum_tests.rs`
- Modify: `src/memory/proptest_enums.rs`

- [ ] **Step 1: Verify existing `MemoryScope` usage**

```bash
grep -rn "MemoryScope::" src/ --include='*.rs'
```

List every variant constructor. If `MemoryScope::Agent` or `MemoryScope::Persona` appears in production code (not test), investigate the site and update to use `MemoryScope::Global` or delete the branch.

- [ ] **Step 2: Shrink the enum**

In `src/memory/context/enums.rs`:

```rust
pub enum MemoryScope {
    /// Visible everywhere.
    #[default]
    Global,
    /// Visible only within the current session; used by SessionCompactor.
    SessionLocal,
}
```

Update `impl MemoryScope::as_str` / `from_str` / serde impls to only handle the two surviving variants. Any "agent" / "persona" string branch is dropped.

- [ ] **Step 3: Drop dead variants from arbiter and tests**

In `src/memory/proptest_enums.rs`, reduce the `MemoryScope` arbiter to generate only `Global` and `SessionLocal`.

In `src/memory/context/tests/enum_tests.rs`, delete every test that constructs `MemoryScope::Agent` or `MemoryScope::Persona`.

- [ ] **Step 4: Verify**

```bash
cargo check -p alephcore
cargo test -p alephcore --lib --no-fail-fast
```

Expected: green.

- [ ] **Step 5: Commit**

```bash
git add -A src/memory/
git commit -m "memory: shrink MemoryScope to Global + SessionLocal"
```

---

## Task 12: Final verification

Belt-and-braces check that nothing deleted is still referenced anywhere, and that the scoring pipeline, retrieval, and event replay all still behave.

- [ ] **Step 1: Run the spec's grep gate**

```bash
grep -rn "MemoryTier\|ShortTerm\|ApplyDecayCommand\|StrengthDecayed\|ValueEstimator\|ImportanceWeightStage\|importance_weight" src/ --include='*.rs'
```

Expected: zero hits. If anything remains, it is a loose end from an earlier task — go back and delete.

- [ ] **Step 2: Full test suite**

```bash
cargo check -p alephcore
cargo test -p alephcore --lib --no-fail-fast
cargo clippy -p alephcore -- -D warnings 2>&1 | tee /tmp/clippy.log
```

Expected: check green, tests green, clippy introduces no new warnings in touched files (pre-existing warnings elsewhere are fine).

- [ ] **Step 3: Smoke-test the scoring pipeline**

```bash
cargo test -p alephcore --lib scoring_pipeline -- --nocapture
```

Expected: `default_pipeline_creates_six_stages` and `test_full_pipeline_end_to_end` both green.

- [ ] **Step 4: Smoke-test event replay against a real DB**

```bash
# If you have a dev database at ~/.aleph/data/memory.db, skim the event log
# for historical StrengthDecayed rows.
sqlite3 ~/.aleph/data/memory.db \
  "SELECT COUNT(*) FROM memory_events WHERE event_json LIKE '%StrengthDecayed%';" 2>/dev/null || echo "no dev db; skip"

# Start the server. It must not panic during event replay.
cargo run --bin aleph-server 2>&1 | head -50
# ^C once you see the banner; the point is that startup replay does not crash.
```

Expected: server starts, any `WARN skipping unrecognized memory event` lines appear for old rows, no panics.

- [ ] **Step 5: Smoke-test memory tools against a running server**

Manually exercise `memory_search`, `recall_context`, and `memory_browse` through whatever test harness your workflow uses (REST call, CLI, IDE tool). Each should return results structurally identical to pre-change (no `tier`, `strength`, or `confidence` fields in the output JSON — downstream schema is OK if those fields simply vanish).

- [ ] **Step 6: Post-cleanup summary commit (documentation only)**

Update `docs/reference/memory/RETRIEVAL.md` §4.2 (importance_weight section) and any other doc sections that refer to deleted mechanisms. Specifically:

- `RETRIEVAL.md` §4.1 stage table: drop row #3 "importance_weight". Renumber.
- `RETRIEVAL.md` §4.2: delete entire subsection.
- `RETRIEVAL.md` §3: update `NoteSearchResult::to_memory_fact` signature and doc to match Task 6 reality (no tier/scope/strength/confidence assignments).
- `NOTES.md` §12: the `ApplyDecayCommand` bullet goes; renumber.

```bash
git add docs/reference/memory/
git commit -m "docs(memory): reflect sovereignty cleanup in retrieval and notes docs"
```

---

## Spec Coverage Check

Each spec section maps to tasks:

| Spec section | Covered by |
|---|---|
| §3 Non-Goals | (nothing to do; observed throughout) |
| §7 Disposition Summary | Tasks 3–11 |
| §8 Core-Set Category Routing | Task 9 (delete path) or Task 9 branch (rewrite path if callers exist) |
| §9 Budget Strategy iii | Task 9 branch; no work in delete path |
| §10.1 Tier/scope enum | Tasks 10 + 11 |
| §10.2 Retrieval bridge | Task 6 |
| §10.3 Scoring pipeline | Task 3 |
| §10.4 Value estimation | Task 4 |
| §10.5 Event sourcing | Tasks 2, 5, 7 |
| §10.6 Composer/comptroller | Task 9 |
| §10.7 Store and projection | Task 8 |
| §10.8 Session compactor | Task 7 Step 7 |
| §10.9 Ripple/integration tests | Task 7 Step 6 |
| §10.10 Public module surface | Task 4 Step 3 and Tasks 9, 10, 11 |
| §11 Retention list | observed — no deletions of listed items |
| §12 Config changes | Task 1 (add), Task 4 Step 4 (remove) |
| §13 Success criteria | Task 12 |
| §14 Migration notes | Task 2 |
| §15 Risks | mitigations embedded per task |
| §16 Open questions | answered: MemoryFact survives shrunk (Task 7); `persona_id` deferred via Composer-delete in Task 9; WARN rate limit not applicable under delete path |
