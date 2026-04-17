# Dream Daemon Evolution Upgrade Design

> **Date**: 2026-04-17
> **Status**: Approved
> **Scope**: Memory evolution — upgrade Dream Daemon from fixed-order pipeline to signal-driven evolution engine
> **Reference**: Evolver GEP Protocol (comparative study)

## 1. Background

### Problem

Aleph's Dream Daemon currently runs a fixed-order pipeline (daily: 6 stages, weekly: 7 stages) that performs knowledge note consolidation, drift detection, synthesis, and maintenance. While functional, it lacks:

- **Signal-driven decision making** — no awareness of what needs attention most
- **Strategy selection** — all cycles run the same stages regardless of context
- **Safety mechanisms** — no detection of evolution loops, oscillation, or wasted effort
- **Validation** — no verification that evolution cycles actually improved knowledge quality
- **Audit trail** — no immutable record of what each cycle did and why

### Comparative Study: Evolver

Evolver (GEP Protocol) is a JavaScript-based self-evolution engine for AI agents. Key innovations relevant to Aleph:

| Evolver Concept | Aleph Mapping | Rationale |
|-----------------|---------------|-----------|
| Gene (strategy template) | DreamStrategy enum | Strategies are few and stable; Rust enum > external config |
| Capsule (success case) | Skill-category knowledge note | Already exists in memory layer; on-demand recall avoids context explosion |
| Signal extraction | Signal Collector | Four signal sources specific to personal AI assistant |
| Selector | Strategy Selector | Deterministic rules, no LLM needed for strategy choice |
| Mutation gating | Extended gate.rs | Cycle detection, oscillation, wasted distillation |
| ValidationReport | DreamValidationReport | Four-layer validation (format → consistency → semantic → retrospective) |
| events.jsonl | dream_events.jsonl | Append-only audit trail per agent |
| Personality state | Selector statistics window | Lightweight, no separate module |

### What We Don't Import

- **Self-code-modification** — Aleph is compiled Rust, not interpreted JS
- **Network hub/proxy/worker pool** — personal assistant, not distributed system
- **Git commit/rollback per cycle** — note evolution uses .bak file recovery, not git
- **Complex personality model** — simplified to sliding-window statistics in Selector

## 2. Architecture

### Evolution Loop (replaces fixed pipeline)

```
Conversation → Note Extractor → Notes / Skill-Notes (cold storage)
                                       ↓
                              Signal Collector (4 signal sources)
                                       ↓
                              Strategy Selector (signals → best DreamStrategy)
                                       ↓
                              Mutation Gate (loop/oscillation/waste detection)
                                       ↓ ALLOW
                              Dream Pipeline (strategy-determined stages)
                                       ↓
                              Validation Layer (4-tier verification)
                                       ↓
                              Solidify (append DreamEvent to event log)
                                       ↓
                              Recall Feedback (next-cycle retrospective)
```

### Key Principle: Cold-Hot Separation

- **Hot skills** (formal `src/skill/` system) — injected into LLM context, count-controlled
- **Cold skill-notes** (memory `notes/skill/` category) — distilled from conversations, retrieved on-demand via memory search

This prevents context window explosion from unbounded skill growth. Evolver's Capsule model validates this pattern: capsules are loaded only when signal-matched, not always-present.

## 3. Six New Mechanisms

### 3.1 Signal Collector

**New file**: `src/memory/dreaming/signals.rs`

Aggregates signals from four data sources into a unified signal list before each Dream cycle.

#### Signal Sources

| Source | Data Location | Signal Type | Examples |
|--------|--------------|-------------|----------|
| Conversation quality | Session metadata, user feedback | `quality` | correction_count, retry_rate, abandonment |
| Memory recall | `recall_signals.rs` data | `recall` | note_hit_rate, never_recalled_count |
| Note health | DreamReport metrics | `health` | duplication_rate, contradiction_rate, staleness_rate |
| Skill usage | Skill-note recall frequency | `skill_usage` | skill_recall_rate, skill_total_count |

#### Data Structure

```rust
pub struct DreamSignal {
    pub signal_type: SignalType,  // Quality, Recall, Health, SkillUsage
    pub name: String,             // e.g. "high_contradiction_rate"
    pub score: f64,               // 0.0 - 1.0
    pub source: String,           // data source identifier
    pub collected_at: i64,        // unix timestamp
}

pub struct SignalSnapshot {
    pub signals: Vec<DreamSignal>,
    pub window_start: i64,        // 24h window start
    pub window_end: i64,
}
```

#### Collection Rules

- **Window**: Last 24 hours of activity
- **Timing**: Collected at the start of each Dream cycle
- **Aggregation**: Raw metrics → normalized scores (0.0-1.0)
- **Persistence**: Included in DreamEvent, not stored separately

### 3.2 DreamStrategy

**New file**: `src/memory/dreaming/strategy.rs`

Three strategies, each defining which pipeline stages to execute.

```rust
pub enum DreamStrategy {
    /// Default mode: merge duplicates, fix formats, maintain index
    Consolidate,
    /// Growth mode: cross-category synthesis, skill-note distillation
    Synthesize,
    /// Defensive mode: deterministic-only ops, skip all LLM stages
    Conserve,
}
```

#### Strategy → Stage Mapping

| Strategy | Stages (in order) |
|----------|-------------------|
| Consolidate | NoteLint → NoteConsolidate → NoteDrift → IndexRefresher → NoteDecay |
| Synthesize | NoteLint → NoteConsolidate → NoteSynthesis → SkillDistill* → DailyDigest |

*`SkillDistill` is a **new stage** that extracts reusable skill-notes from synthesis output. It generalizes patterns found during synthesis into `skill`-category knowledge notes. Implementation: LLM prompt that identifies actionable patterns from synthesis results and writes them as skill-notes.
| Conserve | NoteLint → IndexRefresher |

**Replaces**: `DreamPipeline::daily()` and `DreamPipeline::weekly()` with `DreamPipeline::from_strategy(strategy)`.

### 3.3 Strategy Selector

**New file**: `src/memory/dreaming/selector.rs`

Deterministic selection (no LLM call). Input: `SignalSnapshot`. Output: `SelectionDecision`.

#### Selection Logic

```
1. Check Mutation Gate → if triggered, return Conserve
2. Compute composite scores:
   - growth_pressure = note_growth_rate × (1 - skill_recall_rate)
   - stability = 1 - (contradiction_rate + duplication_rate) / 2
3. If growth_pressure > SYNTHESIZE_THRESHOLD (0.6) AND stability > 0.5:
   → Synthesize
4. Else:
   → Consolidate
```

#### Personality Adaptation (sliding window)

Maintain statistics over last 10 Dream cycles:
- `recent_strategy_distribution: HashMap<DreamStrategy, u32>`
- `recent_validation_pass_rate: f64`
- `recent_skill_recall_hit_rate: f64`

These adjust thresholds:
- High validation pass rate (>0.8 over 10 cycles) → lower SYNTHESIZE_THRESHOLD by 0.1 (more aggressive)
- Low pass rate (<0.5) → raise threshold by 0.1 (more conservative)
- Clamped to [0.4, 0.8] range

#### Output

```rust
pub struct SelectionDecision {
    pub strategy: DreamStrategy,
    pub signals_used: Vec<DreamSignal>,
    pub rationale: String,
    pub personality_adjustment: f64,  // threshold delta applied
}
```

### 3.4 Mutation Gate

**Extends**: `src/memory/dreaming/gate.rs`

Three new detection mechanisms added to existing gate logic.

#### 3.4.1 Merge Cycle Detection

Track `content_hash` of notes involved in consolidation. If the same pair of notes appears in merge decisions across 3+ consecutive cycles (hash alternation), flag as merge cycle.

**Implementation**: Maintain a `recent_merges: VecDeque<HashSet<(String, String)>>` (last 5 cycles). If intersection of any 3 consecutive sets is non-empty → trigger.

#### 3.4.2 Synthesis Oscillation Detection

Compare key conclusions of the two most recent weekly synthesis notes. Simple heuristic: extract assertion sentences, check for negation patterns ("should use X" vs "should not use X", "prefer A" vs "avoid A").

**Implementation**: Regex-based negation pair detection on synthesis note content. Not LLM-powered — keeps gate deterministic.

#### 3.4.3 Wasted Distillation Detection

Track skill-notes produced in last N cycles. If recall hit rate < 10% across the last 5 cycles' skill-note output → flag as wasted distillation.

**Implementation**: Query recall_signals for skill-note paths produced in recent cycles. Count hits vs total.

#### Gate Output

```rust
pub enum GateDecision {
    Allow,
    Conserve { reason: String, cooldown_remaining: u32 },
    Skip { reason: String },
}
```

**Cooldown**: When `Conserve` is triggered, minimum 3 cycles before re-evaluation. Cooldown counter decremented each cycle.

### 3.5 Validation Layer

**New file**: `src/memory/dreaming/validation.rs`

Four-tier validation executed after Dream Pipeline completes.

#### Tier L1: Format Validation (deterministic, zero-cost)

- YAML frontmatter parses correctly
- All wikilinks resolve to existing notes
- Category is in `CATEGORY_DIRS`
- No empty-content notes

#### Tier L2: Consistency Validation (deterministic)

- Index file matches filesystem state
- No duplicate `content_hash` across notes
- All merge operations resulted in valid notes
- No orphaned .bak files from incomplete operations

#### Tier L3: Semantic Validation (LLM, Synthesize strategy only)

- Synthesis conclusions are supported by source notes
- No hallucinated facts in synthesis output
- Skill-note descriptions match their content

**Cost control**: Only runs in Synthesize mode. Uses short prompt (<500 tokens). Skipped if budget exhausted.

#### Tier L4: Retrospective Validation (delayed, next cycle)

- Check recall hit rate for notes/skill-notes produced in previous cycle
- Compare previous cycle's DreamReport predictions vs actual outcomes

#### Output

```rust
pub struct DreamValidationReport {
    pub l1_format: ValidationTier,
    pub l2_consistency: ValidationTier,
    pub l3_semantic: Option<ValidationTier>,  // None if not applicable
    pub l4_retrospective: Option<ValidationTier>,  // None if first cycle
    pub overall_ok: bool,
}

pub struct ValidationTier {
    pub passed: bool,
    pub checks_run: u32,
    pub checks_passed: u32,
    pub issues: Vec<ValidationIssue>,
}
```

**Failure behavior**: L1/L2 failure → mark cycle as Failed, restore .bak files. L3/L4 failure → mark as Warning, log but don't rollback.

### 3.6 Solidify — Immutable Event Log

**New file**: `src/memory/dreaming/event_log.rs`

Append-only event log per agent. One `DreamEvent` per cycle.

```rust
pub struct DreamEvent {
    pub id: String,                          // "dream_{timestamp}_{seq}"
    pub cycle: u32,                          // monotonic counter
    pub strategy: DreamStrategy,
    pub selection: SelectionDecision,
    pub gate_decision: GateDecision,
    pub report: DreamReport,
    pub validation: DreamValidationReport,
    pub duration_ms: u64,
    pub created_at: i64,
}
```

**Storage**: `{memory_dir}/{agent_id}/dream_events.jsonl` — one JSON line per event, append-only.

**Read operations**: Load last N events for Selector personality window and gate cycle detection. No full-file scan needed — read from end.

## 4. Code Changes

### New Files (~6)

| File | Purpose | Est. Lines |
|------|---------|------------|
| `src/memory/dreaming/signals.rs` | Signal collection from 4 sources | 200-300 |
| `src/memory/dreaming/strategy.rs` | DreamStrategy enum + stage mapping | 80-120 |
| `src/memory/dreaming/selector.rs` | Deterministic strategy selection + personality | 200-300 |
| `src/memory/dreaming/validation.rs` | 4-tier validation layer | 250-350 |
| `src/memory/dreaming/event_log.rs` | Append-only DreamEvent log | 150-200 |
| `src/memory/dreaming/stages/skill_distill.rs` | New stage: extract skill-notes from synthesis output | 150-250 |
| `tests/dream_evolution.rs` | Integration tests | 300-400 |

### Refactored Files (~4)

| File | Current Lines | Change |
|------|--------------|--------|
| `src/memory/dreaming/mod.rs` | 627 | Refactor main loop: Signal → Select → Gate → Pipeline → Validate → Solidify |
| `src/memory/dreaming/gate.rs` | 320 | Add 3 new detection mechanisms (merge cycle, oscillation, waste) |
| `src/memory/dreaming/stages/mod.rs` | 45 | `DreamPipeline::from_strategy()` replaces `::daily()` / `::weekly()` |
| `src/memory/dreaming/report.rs` | 61 | Extend DreamReport with validation results |

### Cleanup

- Remove `DreamPipeline::daily()` and `DreamPipeline::weekly()` static methods
- Remove hardcoded stage ordering logic
- Replace any `is_weekly` boolean checks with strategy-based dispatch

## 5. Non-Goals

- No self-code-modification (Rust compiled binary)
- No network hub/proxy/worker pool (personal assistant)
- No changes to formal Skill system (`src/skill/`)
- No git operations per Dream cycle
- No external dependency additions to core

## 6. Testing Strategy

- **Unit tests**: Each new module (signals, selector, gate, validation, event_log) independently testable with mock data
- **Integration test**: Full cycle with test notes → signal collection → strategy selection → pipeline execution → validation → event logging
- **Regression**: Existing Dream Daemon tests must pass — new loop wraps existing stages, doesn't replace their internals
