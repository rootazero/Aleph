# Memory Capture-Conflict Dedup (Gap B) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Activate the dormant mem0-style write-time semantic dedup (turn it on by default) and upgrade its binary ADD/MERGE threshold into a three-tier ADD / MERGE / NOOP decision.

**Architecture:** The embedding-cosine dedup in `dedup_redirect_creates` is an enabling/safety layer (no LLM call — R7/R10-safe). We flip the config default `dedup_enabled` to `true`, add a second higher `dedup_noop_threshold` (default 0.985), and change the redirect decision from binary (`sim ≥ 0.92 → Append`) to three-way (`<0.92` keep Create / `[0.92,0.985)` → Append / `≥0.985` drop as NOOP).

**Tech Stack:** Rust, serde (config), the existing `CompoundIngestConfig` / `RelatedBudget` / `DefaultCompoundIngestor::dedup_redirect_creates`.

---

## ⚠️ PROJECT PROTOCOL (OVERRIDES TDD "run the test" STEPS)

- **DO NOT run `cargo check` / `cargo test` / `cargo build` / `cargo fmt`.** The compiler is unavailable by mandate. Verify each task with the **grep caller-verification guard** (compiler substitute) + reading the diff. Commit directly.
- **Worktree isolation:** all work under the worktree branch off this plan's commit on main, NEVER the main checkout.
- **Append-only main / explicit-path staging:** `git add <exact paths>` only. No `reset`/`amend`/`rebase`.
- **Backward-compatible** except the deliberate `dedup_enabled` default flip (the spec's intent; operators can re-disable via TOML). New config field is `#[serde(default)]`.
- Unit tests are written (run later in CI) but verified by reading, not `cargo`.

---

## File Structure

| File | Responsibility | Task |
|------|----------------|------|
| `src/config/types/memory/defaults.rs` | flip `default_dedup_enabled`→true; add `default_dedup_noop_threshold` | 1 |
| `src/config/types/memory/ingest.rs` | `CompoundIngestConfig`: flip Default `dedup_enabled`→true; add `dedup_noop_threshold` field (serde default) + Default line | 1 |
| `src/memory/notes/ingest/retrieve.rs` | `RelatedBudget.dedup_noop_threshold` field + `Default` 0.985 | 2 |
| `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs` | wire `dedup_noop_threshold: cfg.dedup_noop_threshold` into the `RelatedBudget` literal | 2 |
| `src/memory/notes/ingest/ingestor.rs` | three-tier decision in `dedup_redirect_creates` + tests | 3 |

**Ordering:** 1 → 2 → 3. Task 3's decision logic reads `RelatedBudget.dedup_noop_threshold` (Task 2), which is wired from `cfg.dedup_noop_threshold` (Task 1). Inter-task non-compiling states are acceptable (no cargo).

---

## Task 1: Config — activate dedup + add NOOP threshold

**Files:**
- Modify: `src/config/types/memory/defaults.rs`
- Modify: `src/config/types/memory/ingest.rs`

- [ ] **Step 1: Flip `default_dedup_enabled` and add the NOOP default**

In `src/config/types/memory/defaults.rs`, change `default_dedup_enabled` (lines 268-272) to:

```rust
/// Write-time semantic dedup gate (mem0-style). Enabled by default so
/// near-duplicate notes are collapsed at ingest instead of waiting for the
/// offline dream consolidator. Operators can set `dedup_enabled = false` in
/// `[memory.compound_ingest]` to restore byte-identical ingest.
pub fn default_dedup_enabled() -> bool {
    true
}
```

Immediately AFTER `default_dedup_similarity_threshold` (ends line 280), add:

```rust
/// Cosine threshold at or above which a freshly-planned `Create` is treated as
/// a NOOP and dropped (the note already exists, essentially verbatim). Must be
/// >= `default_dedup_similarity_threshold`. `0.985` is deliberately very high:
/// only near-identical title+summary+facts collapse to a no-op, so a Create
/// carrying genuinely new facts (which embeds below this) is still merged via
/// Append rather than dropped.
pub fn default_dedup_noop_threshold() -> f32 {
    0.985
}
```

- [ ] **Step 2: Add the field + flip the Default in `CompoundIngestConfig`**

In `src/config/types/memory/ingest.rs`:

After the `dedup_similarity_threshold` field (line 29), add:
```rust
    #[serde(default = "super::defaults::default_dedup_noop_threshold")]
    pub dedup_noop_threshold: f32,
```

In `impl Default for CompoundIngestConfig` (lines 33-45), change `dedup_enabled: false,` (line 42) to `dedup_enabled: true,` and after `dedup_similarity_threshold: 0.92,` (line 43) add:
```rust
            dedup_noop_threshold: 0.985,
```

- [ ] **Step 3: Tests**

In the config tests for this module (find them via `grep -rn "mod tests" src/config/types/memory/`; if `ingest.rs` has no test module, add one, else append). Add:

```rust
#[test]
fn dedup_is_enabled_by_default() {
    assert!(super::super::defaults::default_dedup_enabled());
    assert_eq!(super::super::defaults::default_dedup_noop_threshold(), 0.985);
    let cfg = CompoundIngestConfig::default();
    assert!(cfg.dedup_enabled);
    assert_eq!(cfg.dedup_noop_threshold, 0.985);
}

#[test]
fn compound_ingest_config_parses_without_dedup_noop_key() {
    // Backward-compat: TOML lacking the new key falls back to the serde default.
    let toml = "enabled = true\n";
    let cfg: CompoundIngestConfig = toml::from_str(toml).unwrap();
    assert_eq!(cfg.dedup_noop_threshold, 0.985);
    assert!(cfg.dedup_enabled);
}
```

> Adjust the `super::super::defaults::` path to however sibling tests in this file reference the `defaults` module (read an existing test first — it may be `super::defaults::` or a `use` import). If `CompoundIngestConfig` is not in scope in the test module, add `use super::CompoundIngestConfig;`. If `toml` isn't already a dev-dependency used in this file's tests, use `serde_json` with an equivalent empty-object parse instead: `let cfg: CompoundIngestConfig = serde_json::from_str("{}").unwrap();`.

- [ ] **Step 4: Grep guard**

```bash
grep -n "pub fn default_dedup_enabled" -A3 /Volumes/TBU4/Workspace/Aleph-wt-dedup-gap-b/src/config/types/memory/defaults.rs   # returns true
grep -n "default_dedup_noop_threshold" /Volumes/TBU4/Workspace/Aleph-wt-dedup-gap-b/src/config/types/memory/defaults.rs        # defined
grep -n "dedup_noop_threshold\|dedup_enabled" /Volumes/TBU4/Workspace/Aleph-wt-dedup-gap-b/src/config/types/memory/ingest.rs    # field + serde default + Default line; dedup_enabled: true in Default
```
Expected: `default_dedup_enabled` body is `true`; `default_dedup_noop_threshold` defined returning 0.985; `ingest.rs` has the new field with serde default, `dedup_enabled: true` and `dedup_noop_threshold: 0.985` in the `Default` impl.

- [ ] **Step 5: Commit**

```bash
git -C /Volumes/TBU4/Workspace/Aleph-wt-dedup-gap-b add src/config/types/memory/defaults.rs src/config/types/memory/ingest.rs
git -C /Volumes/TBU4/Workspace/Aleph-wt-dedup-gap-b commit -m "feat(memory): activate write-time dedup by default + add dedup_noop_threshold config (Gap B)"
```

---

## Task 2: `RelatedBudget.dedup_noop_threshold` + wiring

**Files:**
- Modify: `src/memory/notes/ingest/retrieve.rs`
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs`

- [ ] **Step 1: Add the field to `RelatedBudget`**

In `src/memory/notes/ingest/retrieve.rs`, after the `dedup_similarity_threshold` field (line 22), add:
```rust
    /// Cosine threshold at/above which a near-identical `Create` is dropped as a
    /// NOOP (must be >= `dedup_similarity_threshold`; floored to it at decision
    /// time). Only consulted when `dedup_enabled`.
    pub dedup_noop_threshold: f32,
```

In `impl Default for RelatedBudget` (lines 25-35), after `dedup_similarity_threshold: 0.92,` (line 32), add:
```rust
            dedup_noop_threshold: 0.985,
```

- [ ] **Step 2: Wire config → budget in agent_init**

In `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs`, the `RelatedBudget { ... }` literal (lines 844-850) is a full field-by-field literal (no `..default()`), so it MUST gain the new field. After `dedup_similarity_threshold: cfg.dedup_similarity_threshold,` (line 849) add:
```rust
                        dedup_noop_threshold: cfg.dedup_noop_threshold,
```

- [ ] **Step 3: Grep guard — find every full `RelatedBudget` literal**

A new struct field breaks any `RelatedBudget { ... }` literal that does NOT end with `..RelatedBudget::default()` / `..Default::default()`. Run:
```bash
grep -rn "RelatedBudget {" /Volumes/TBU4/Workspace/Aleph-wt-dedup-gap-b/src/
```
Classify each hit:
- `pub struct RelatedBudget {` (definition) — N/A.
- `impl Default for RelatedBudget` block — already updated in Step 1.
- A literal ending in `, ..RelatedBudget::default() }` or `..Default::default() }` (e.g. `ingestor.rs:1647`, `ingestor.rs:1757`) — SAFE, inherits the new field, no change.
- A FULL literal listing every field (e.g. `agent_init/mod.rs:844`) — MUST include `dedup_noop_threshold`. Confirm Step 2 covered it; if grep finds any other full literal, add the field there too.

Then confirm:
```bash
grep -n "dedup_noop_threshold" /Volumes/TBU4/Workspace/Aleph-wt-dedup-gap-b/src/memory/notes/ingest/retrieve.rs   # field + Default line (2 hits)
grep -n "dedup_noop_threshold: cfg.dedup_noop_threshold" /Volumes/TBU4/Workspace/Aleph-wt-dedup-gap-b/src/bin/aleph-server/commands/start/builder/agent_init/mod.rs  # wiring (1 hit)
```
Expected: `retrieve.rs` has the field + the `Default` line; `agent_init` wires it; every other `RelatedBudget` literal either spreads Default or has the field.

- [ ] **Step 4: Commit**

```bash
git -C /Volumes/TBU4/Workspace/Aleph-wt-dedup-gap-b add src/memory/notes/ingest/retrieve.rs src/bin/aleph-server/commands/start/builder/agent_init/mod.rs
git -C /Volumes/TBU4/Workspace/Aleph-wt-dedup-gap-b commit -m "feat(memory): RelatedBudget.dedup_noop_threshold field + agent_init wiring (Gap B)"
```

---

## Task 3: Three-tier decision in `dedup_redirect_creates`

**Files:**
- Modify: `src/memory/notes/ingest/ingestor.rs` (the `dedup_redirect_creates` method, lines ~526-636)
- Test: `src/memory/notes/ingest/ingestor.rs` `#[cfg(test)]`

- [ ] **Step 1: Read the method + the existing dedup tests**

READ `dedup_redirect_creates` in full (lines ~526-636) and the two existing dedup tests around lines 1640-1800 (the ones building `budget: RelatedBudget { dedup_enabled: true, ..RelatedBudget::default() }`). Note how those tests construct the ingestor, the mock embedder (what vectors it returns for given texts), and the store with stored embeddings — you will copy that exact harness for the new tests.

- [ ] **Step 2: Replace the decision + rewrite with three-tier logic**

In `dedup_redirect_creates`, the threshold line (line ~535) currently is:
```rust
        let threshold = self.budget.dedup_similarity_threshold.clamp(0.0, 1.0);
```
Replace it with both thresholds (the `.max` guarantees `noop >= dedup` even on misconfiguration):
```rust
        let dedup_threshold = self.budget.dedup_similarity_threshold.clamp(0.0, 1.0);
        let noop_threshold = self
            .budget
            .dedup_noop_threshold
            .clamp(0.0, 1.0)
            .max(dedup_threshold);
```

Then replace the decision loop + early-return + rewrite (lines ~577-636, from the comment `// For each Create, find the best related page above threshold` through the end of the method) with:

```rust
        // For each Create, classify its best related match into three tiers:
        //   sim >= noop_threshold        → NOOP  (drop the Create)
        //   dedup_threshold <= sim < noop → MERGE (Create → Append)
        //   sim < dedup_threshold        → ADD   (keep the Create)
        // Never self-redirecting onto the Create's own path.
        use std::collections::{HashMap, HashSet};
        let mut redirect: HashMap<usize, String> = HashMap::new();
        let mut drop_noop: HashSet<usize> = HashSet::new();
        for (slot, &op_i) in create_idx.iter().enumerate() {
            let PageOp::Create { note_path, .. } = &ops[op_i] else {
                continue;
            };
            let cand = &cand_vecs[slot];
            let mut best: Option<(&str, f32)> = None;
            for (path, vec) in &related_vecs {
                if *path == note_path.as_str() {
                    continue;
                }
                let sim = cosine_similarity(cand, vec);
                if best.is_none_or(|(_, b)| sim > b) {
                    best = Some((*path, sim));
                }
            }
            if let Some((path, sim)) = best {
                if sim >= noop_threshold {
                    drop_noop.insert(op_i);
                } else if sim >= dedup_threshold {
                    redirect.insert(op_i, path.to_string());
                }
            }
        }
        if redirect.is_empty() && drop_noop.is_empty() {
            return ops;
        }

        // Rewrite: NOOP Creates are dropped; MERGE Creates become Append onto the
        // matched existing note (the existing page owns its title/summary, so
        // only the candidate's facts and links carry over); everything else
        // passes through.
        ops.into_iter()
            .enumerate()
            .filter_map(|(i, op)| {
                if drop_noop.contains(&i) {
                    if let PageOp::Create { note_path, .. } = &op {
                        info!(
                            note = %note_path,
                            "ingest dedup: dropping near-identical Create as NOOP"
                        );
                    }
                    return None;
                }
                match (redirect.remove(&i), op) {
                    (
                        Some(target),
                        PageOp::Create {
                            note_path,
                            facts,
                            links,
                            ..
                        },
                    ) => {
                        info!(
                            from = %note_path,
                            into = %target,
                            "ingest dedup: redirecting near-duplicate Create into Append"
                        );
                        Some(PageOp::Append {
                            note_path: target,
                            new_facts: facts,
                            new_links: links,
                            new_relations: vec![],
                        })
                    }
                    (_, op) => Some(op),
                }
            })
            .collect()
```

Notes:
- The original used `use std::collections::HashMap;` inside the method — replace it with the `use std::collections::{HashMap, HashSet};` shown above (do not leave a duplicate `HashMap` import).
- `cosine_similarity`, `candidate_dedup_text`, `info!`, `PageOp` are already in scope (the original method uses them).
- The `Append` literal includes `new_relations: vec![]` (the field added by the Gap A feature already on main).

- [ ] **Step 3: Tests — three tiers + floor**

Copy the harness from the existing dedup tests (Step 1). Add tests that drive a single `Create` to each tier by controlling the mock embedder's vectors so the candidate's cosine vs the seeded related note lands in the intended band. Assertions:

```rust
// ADD: best match below dedup_threshold → Create survives unchanged.
#[tokio::test]
async fn dedup_tier_add_keeps_create() {
    // <build ingestor with budget { dedup_enabled: true, ..default() } and a
    //  related note whose stored embedding is far (cosine < 0.92) from the
    //  Create candidate's embedding — copy the existing dedup test harness>
    // let out = ingestor.dedup_redirect_creates(AGENT, ops, &related).await;
    assert!(matches!(out.as_slice(), [PageOp::Create { .. }]));
}

// MERGE: best match in [0.92, 0.985) → Create becomes Append.
#[tokio::test]
async fn dedup_tier_merge_redirects_to_append() {
    // <related embedding ~0.95 cosine from the candidate>
    // let out = ...;
    assert!(matches!(out.as_slice(), [PageOp::Append { .. }]));
}

// NOOP: best match >= 0.985 → Create dropped (empty result).
#[tokio::test]
async fn dedup_tier_noop_drops_create() {
    // <related embedding ~0.99 cosine (or identical vector) from the candidate>
    // let out = ...;
    assert!(out.is_empty(), "near-identical Create must be dropped as NOOP");
}

// FLOOR: dedup_noop_threshold misconfigured below dedup_similarity_threshold →
// the .max floor keeps NOOP from firing below MERGE (a 0.95-cosine match still
// MERGES, never NOOPs).
#[tokio::test]
async fn dedup_noop_floor_never_below_merge() {
    // <budget { dedup_enabled: true, dedup_similarity_threshold: 0.92,
    //           dedup_noop_threshold: 0.50, ..default() }; related ~0.95 cosine>
    // let out = ...;
    assert!(matches!(out.as_slice(), [PageOp::Append { .. }]),
        "noop floored to >= dedup_threshold, so 0.95 MERGES not NOOPs");
}
```

> CRITICAL: the existing dedup tests already prove the MERGE path with a real mock embedder/store; copy their EXACT construction (mock embedder type, how stored embeddings are seeded via the store, the `AGENT`/agent_id, how `related: Vec<RelatedPage>` is built with matching `path`). To land a candidate in a precise cosine band, the simplest approach is a mock embedder that returns a fixed unit vector for the related note's text and a chosen vector for the candidate text whose cosine you compute by hand (e.g. identical vector → cosine 1.0 for NOOP; an orthogonal-ish vector → low cosine for ADD; a deliberately blended vector for MERGE). If the existing harness makes precise cosine control hard, prefer reusing whatever vector-injection mechanism those tests already use and pick vectors that clearly fall in each band. If you genuinely cannot control the cosine with the existing harness, report NEEDS_CONTEXT rather than writing a flaky test.

- [ ] **Step 4: Grep guard**

```bash
grep -n "noop_threshold\|drop_noop\|filter_map" /Volumes/TBU4/Workspace/Aleph-wt-dedup-gap-b/src/memory/notes/ingest/ingestor.rs   # decision present
grep -n "use std::collections::{HashMap, HashSet}" /Volumes/TBU4/Workspace/Aleph-wt-dedup-gap-b/src/memory/notes/ingest/ingestor.rs  # combined import inside method
grep -c "use std::collections::HashMap;" /Volumes/TBU4/Workspace/Aleph-wt-dedup-gap-b/src/memory/notes/ingest/ingestor.rs            # no leftover duplicate single import in this method
```
Expected: `noop_threshold` computed with `.max(dedup_threshold)`; `drop_noop` HashSet + `filter_map` rewrite present; the method's import is the combined `{HashMap, HashSet}` (no orphaned `use std::collections::HashMap;` left in the method body — note other methods in the file may legitimately have their own `HashMap` import, so this grep may show hits elsewhere; just confirm the dedup method's local import was converted).

- [ ] **Step 5: Commit**

```bash
git -C /Volumes/TBU4/Workspace/Aleph-wt-dedup-gap-b add src/memory/notes/ingest/ingestor.rs
git -C /Volumes/TBU4/Workspace/Aleph-wt-dedup-gap-b commit -m "feat(memory): three-tier ADD/MERGE/NOOP dedup decision (Gap B)"
```

---

## Final verification (whole branch, no cargo)

```bash
# Activation: default true in both the serde-default fn AND the Default impl
grep -A2 "pub fn default_dedup_enabled" /Volumes/TBU4/Workspace/Aleph-wt-dedup-gap-b/src/config/types/memory/defaults.rs    # → true
grep -n "dedup_enabled: true" /Volumes/TBU4/Workspace/Aleph-wt-dedup-gap-b/src/config/types/memory/ingest.rs                 # Default impl flipped

# NOOP threshold defined + plumbed end-to-end
grep -rn "dedup_noop_threshold" /Volumes/TBU4/Workspace/Aleph-wt-dedup-gap-b/src/ | grep -v test
#   → defaults.rs (fn), ingest.rs (field + serde + Default), retrieve.rs (field + Default),
#     agent_init (wiring), ingestor.rs (decision .max + clamp)

# Three-tier decision wired
grep -n "drop_noop\|noop_threshold" /Volumes/TBU4/Workspace/Aleph-wt-dedup-gap-b/src/memory/notes/ingest/ingestor.rs

# No RelatedBudget full literal left without the new field
grep -rn "RelatedBudget {" /Volumes/TBU4/Workspace/Aleph-wt-dedup-gap-b/src/ | grep -v "::default() }" | grep -v "struct RelatedBudget" | grep -v "impl Default"
#   → only literals that DO contain dedup_noop_threshold (agent_init) should remain.
```

Then hand off to **superpowers:finishing-a-development-branch** (merge `--no-ff` to main; worktree cleanup in a fresh session per the CLAUDE.md hazard).

---

## Self-Review (plan author, against the spec)

**Spec coverage:**
- §3 activation (flip `default_dedup_enabled` + the Default impl) → Task 1 ✓
- §4.1 new config `dedup_noop_threshold` → Task 1 ✓
- §4.2 `RelatedBudget` field → Task 2 ✓
- §4.3 agent_init wiring → Task 2 ✓
- §4.4 three-tier decision (drop HashSet + filter_map + noop floor) → Task 3 ✓
- §4.5 tradeoff (high threshold + floor) → encoded in defaults (0.985) + `.max` floor (Task 3) ✓
- §5 edge cases (clamp, floor, graceful pass-through unchanged) → Task 3 keeps the existing `dedup_enabled`/`related.is_empty()`/no-embeddings guards ✓
- §6 tests (ADD/MERGE/NOOP/floor + config default) → Tasks 1, 3 ✓
- §7 NOT in scope (no Update, no enum) → respected; nothing added ✓
- §8 backward-compat (serde default field; literal sites enumerated) → Tasks 1, 2 grep guards ✓

**Type consistency:** `dedup_noop_threshold: f32` consistent across `CompoundIngestConfig`, `RelatedBudget`, and the `cfg.dedup_noop_threshold` wiring; `dedup_threshold`/`noop_threshold` local names consistent within the Task 3 method; `default_dedup_noop_threshold() -> f32` returns `0.985` matching both `Default` impls.

**Placeholder scan:** The Task 3 test bodies intentionally describe the vector-injection setup in prose because the precise mock-embedder API must be copied from the existing dedup tests (same pattern as the prior Gap A plan). The assertions and the production code are fully specified; only the harness wiring is "copy the sibling test", which is a deliberate, bounded instruction, not a vague placeholder.
