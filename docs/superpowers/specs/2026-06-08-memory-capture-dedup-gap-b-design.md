# Memory Capture-Conflict Dedup (Gap B) — Design

> **Status:** Approved (brainstorming complete, 2026-06-08)
> **Scope:** Single implementation plan. Activate the dormant write-time
> semantic dedup and upgrade its binary decision into a three-tier
> ADD / MERGE / NOOP. No deterministic LLM-replacement; the LLM keeps op
> sovereignty (R7).

## 1. Purpose

mem0's value at capture time — "decide ADD / UPDATE / MERGE / NOOP for a new
memory instead of blindly appending" — is **already ~85% implemented** in
Aleph. The compound ingestor's LLM emits `create / append / update / contradict
/ supersede` ops guided by the prompt and `[P<n>]` related-page context, and
`dedup_redirect_creates` (`ingestor.rs:511`) is a mem0-style write-time semantic
dedup that redirects a near-duplicate `Create` into an `Append`.

Two real gaps remain:

1. **The dedup is dormant.** `default_dedup_enabled()` returns `false`
   (`config/types/memory/defaults.rs:270`), so most deployments do
   byte-identical ingest and never benefit from the built dedup.
2. **The decision is binary** (ADD / MERGE). There is no NOOP tier: a `Create`
   that is *near-identical* to an existing note is redirected to `Append`
   (bumping `updated_at` and re-touching the note) rather than recognised as a
   no-op.

This spec activates the dedup and adds the NOOP tier. It deliberately does **not**
build a deterministic conflict-decision engine — that would replace LLM
reasoning Aleph keeps in the model (R7 LLM sovereignty, R10 dumb-loop).

## 2. Architecture

The embedding-cosine dedup is a **safety/perf enabling layer**, not a reasoning
layer — it only collapses near-identical `Create`s; all genuine
create-vs-append-vs-update-vs-contradict judgement stays with the LLM. We:

1. flip the production default so the dedup runs, and
2. turn its binary threshold into a three-tier decision by adding a second,
   higher NOOP threshold.

```
LLM plan (Create/Append/Update/Contradict/Supersede)  ← LLM sovereignty (unchanged)
        │
        ▼
dedup_redirect_creates  (embedding cosine vs related notes — enabling layer)
   sim < dedup_threshold (0.92)        → ADD   : Create passes through
   0.92 ≤ sim < noop_threshold (0.985) → MERGE : Create → Append   (existing)
   sim ≥ noop_threshold (0.985)        → NOOP  : drop the Create    (NEW)
        │
        ▼
apply (create / append[fact-dedup] / update / ...)
```

## 3. Activation

`config/types/memory/defaults.rs`:

```rust
pub fn default_dedup_enabled() -> bool {
    true   // was false
}
```

Update the doc-comment to reflect the new default. The value flows
`app_config.memory.compound_ingest.dedup_enabled` → `RelatedBudget.dedup_enabled`
(`agent_init/mod.rs:848`) → `dedup_redirect_creates` (`ingestor.rs:532`). No
other wiring changes for activation. Operators can still set
`dedup_enabled = false` in TOML to restore byte-identical ingest. The cosine
threshold (`0.92`) is unchanged — deliberately conservative.

## 4. Harden: three-tier decision

### 4.1 New config

`config/types/memory/defaults.rs`:

```rust
/// Cosine-similarity threshold at or above which a freshly-planned `Create`
/// is treated as a NOOP and dropped (the note already exists, essentially
/// verbatim). Must be >= `dedup_similarity_threshold`. `0.985` is deliberately
/// very high: only near-identical title+summary+facts collapse to a no-op,
/// so a Create carrying genuinely new facts (which would sit below this) is
/// still merged via Append rather than dropped.
pub fn default_dedup_noop_threshold() -> f32 {
    0.985
}
```

Add the field to the `compound_ingest` config struct (next to
`dedup_similarity_threshold`) with `#[serde(default = "default_dedup_noop_threshold")]`.

### 4.2 `RelatedBudget`

`retrieve.rs`:

```rust
pub struct RelatedBudget {
    // ... existing fields ...
    /// Cosine threshold at/above which a near-identical `Create` is dropped
    /// as a NOOP (must be >= dedup_similarity_threshold). Clamped at decision
    /// time. Only consulted when `dedup_enabled`.
    pub dedup_noop_threshold: f32,
}
```

`Default` impl gains `dedup_noop_threshold: 0.985`.

### 4.3 Wiring

`agent_init/mod.rs:844` `RelatedBudget { ... }` gains
`dedup_noop_threshold: cfg.dedup_noop_threshold`.

### 4.4 Decision logic (`dedup_redirect_creates`, `ingestor.rs`)

Today (lines 577-604) the loop records a single `redirect: HashMap<usize,String>`
when `sim >= threshold`. Change to a three-way classification:

- Compute `dedup_threshold = dedup_similarity_threshold.clamp(0,1)` and
  `noop_threshold = dedup_noop_threshold.clamp(0,1).max(dedup_threshold)`
  (the `.max` guarantees `noop >= dedup` even on misconfiguration).
- For each `Create`'s best match `(path, sim)`:
  - `sim >= noop_threshold` → insert `op_i` into `drop: HashSet<usize>`.
  - else `sim >= dedup_threshold` → insert `(op_i → path)` into `redirect`.
  - else → leave the `Create` untouched.
- Early-return guard becomes `if redirect.is_empty() && drop.is_empty() { return ops; }`.
- The rewrite switches `.map()` → `.filter_map()`:
  - `i` in `drop` → `None` (Create dropped; log `info!` "ingest dedup: dropping near-identical Create as NOOP").
  - `i` in `redirect` → `Some(Append { ... })` (existing behaviour).
  - else → `Some(op)`.

### 4.5 NOOP data-loss tradeoff (accepted)

Dropping a `Create` at `sim >= 0.985` could, in principle, discard a Create that
carries one genuinely new fact. This is accepted and mitigated:

- The threshold is **very high (0.985)**: only `title+summary+facts` that are
  ~98.5% cosine-identical to an existing note collapse to a no-op. A Create with
  a materially new fact embeds below this and is **merged via Append**
  (loss-free) instead.
- This mirrors mem0's own NOOP tradeoff. Below the NOOP tier, MERGE/Append is
  loss-free because the apply layer already drops exact-duplicate facts
  (`apply.rs:145`).
- Operators who want zero drop risk can set `dedup_noop_threshold = 1.0` (no
  Create reaches it → NOOP disabled) or `dedup_enabled = false`.

## 5. Error handling / edge cases (P7)

- Thresholds clamped to `[0,1]`; `noop` floored to `>= dedup` so a misconfig
  can never make NOOP fire below MERGE.
- A dropped `Create`'s outgoing `link`/`relations` to its own fresh path are
  dropped with it. A separate op linking *to* the dropped path degrades
  gracefully to an unresolved link (same as the existing Append-redirect case,
  which also doesn't rewrite third-party references).
- Dedup remains fully gated on `dedup_enabled` and on related pages having
  stored embeddings — no embeddings → graceful pass-through (unchanged).

## 6. Testing

In `ingestor.rs` `#[cfg(test)]` (the module already has dedup tests with
`dedup_enabled: true`):

1. **ADD:** a `Create` whose best match cosine `< 0.92` survives as `Create`.
2. **MERGE:** a `Create` in `[0.92, 0.985)` is rewritten to `Append` onto the
   matched note (existing behaviour preserved).
3. **NOOP:** a `Create` at `>= 0.985` is dropped (absent from the returned ops).
4. **noop floor:** with `dedup_noop_threshold` misconfigured below
   `dedup_similarity_threshold`, NOOP never fires below MERGE (the `.max` floor
   holds).

In `config/types/memory` tests (or a defaults test):

5. **Activation default:** `default_dedup_enabled() == true` and
   `default_dedup_noop_threshold() == 0.985`.

(Per project protocol, tests are written but not run with `cargo`; grep
caller-verification guards substitute for the compiler.)

## 7. Explicitly NOT in scope (R7 / YAGNI)

- **No deterministic `Create → Update` (replacement).** Cosine-driven content
  replacement risks destroying good content; the LLM emits `Update` when it
  judges replacement is correct. Out of scope.
- **No unified `ADD/UPDATE/MERGE/NOOP` decision enum threaded through the
  pipeline.** The LLM already classifies via op choice; a deterministic
  classifier is 越俎代庖 (R7/R10). The three-tier logic stays *inside* the
  embedding dedup as an enabling layer, not a new pipeline stage.
- **No extension of the governance gate to non-Create ops** (separate concern).
- **No full-note structural MERGE at capture time** (that remains the offline
  Dream `NoteConsolidateStage`'s job).

## 8. Backward compatibility

- Activation is a **behaviour change** (dedup now on by default) but is the
  documented intent; operators can disable via TOML. The dedup itself is
  conservative (0.92) and already tested.
- New config field is `#[serde(default = "default_dedup_noop_threshold")]` →
  existing TOML without the key parses unchanged.
- `RelatedBudget` gains a field; all `RelatedBudget::default()` construction
  sites (many in tests) inherit `0.985` via the `Default` impl — only the
  explicit struct-literal sites (`agent_init` and any test building the struct
  field-by-field) need the new field. The grep guard enumerates them.
