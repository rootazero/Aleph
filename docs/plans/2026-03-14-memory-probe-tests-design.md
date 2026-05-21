# Memory Probe Tests Design

> Date: 2026-03-14
> Approach: Mixed (logic layer + LanceDB integration), organized by memory lifecycle phases

## Overview

Production-grade probe tests for the memory optimization features: RRF fusion, cross-encoder rerank, query expansion, retrieval trace, tiered decay, access reinforcement, tier promotion, and reflection system.

**Structure:** 6 Phases, ~41 scenarios, following the project's existing probe test conventions (Harness + Phase modules + `p{n}_{nn}_{scenario}` naming).

## File Structure

```
tests/
├── memory_probe.rs                    # Entry: declare submodules
└── memory_probe/
    ├── harness.rs                     # MemoryProbeHarness (~300 LOC)
    ├── mock_embedding.rs              # MockEmbeddingProvider (~80 LOC)
    ├── extraction_classification.rs   # P1: write/extract/classify (8 scenarios, sync)
    ├── fusion_scoring.rs              # P2: fusion/scoring (8 scenarios, sync)
    ├── rerank.rs                      # P3: cross-encoder (6 scenarios, sync)
    ├── retrieval_trace.rs             # P4: trace observability (5 scenarios, sync)
    ├── decay_promotion.rs             # P5: decay/reinforcement/promotion (8 scenarios, sync)
    └── end_to_end.rs                  # P6: full lifecycle (6 scenarios, async+LanceDB)
```

## Infrastructure

### MemoryProbeHarness

Wraps production components with test isolation:

- `LanceMemoryBackend` (TempDir isolated)
- `ScoringPipeline` (trace-enabled)
- `TieredDecayConfig` + `PromotionCriteria`
- `MockEmbeddingProvider` (deterministic vectors)
- Mock clock (`AtomicI64`) for time control

**Core methods:**

| Method | Purpose |
|--------|---------|
| `new()` | Create TempDir + LanceDB + defaults |
| `insert_fact(content, type, tier)` | Insert test fact, return id |
| `insert_facts_batch(facts)` | Batch insert |
| `vector_search(query, limit)` | Vector search |
| `text_search(query, limit)` | BM25 search |
| `hybrid_search(query, limit)` | Hybrid search with configured fusion |
| `run_pipeline(candidates, query)` | Scoring pipeline |
| `run_pipeline_traced(candidates, query)` | Pipeline with trace |
| `evaluate_decay(fact_id)` | Single fact decay evaluation |
| `check_promotion(fact_id)` | Check promotion eligibility |
| `advance_time(days)` | Move mock clock forward |

### MockEmbeddingProvider

Deterministic pseudo-vectors from content hash. No external API calls.
- Similar text → high cosine similarity (keyword-level hashing)
- Configurable dimensions (768/1024/1536)

---

## P1: Extraction & Classification (8 scenarios, sync)

Tests reflection parser, mapper, gate logic. Pure logic, no LanceDB.

| # | Scenario | Validates |
|---|----------|-----------|
| p1_01 | `reflection_parses_all_four_sections` | 4-section markdown → correct counts per section |
| p1_02 | `reflection_skips_placeholders_and_empty_lines` | Filters (none), empty, (none captured) |
| p1_03 | `lesson_parses_symptom_cause_fix_format` | "symptom: cause → fix" parsing |
| p1_04 | `lesson_fallback_unstructured_text` | No `:` or `→` → symptom=full text |
| p1_05 | `mapper_invariant_to_core_tier` | Invariant → Core, Preference, confidence=0.85 |
| p1_06 | `mapper_derived_to_short_term_tier` | Derived → ShortTerm, Other, confidence=0.70 |
| p1_07 | `mapper_lesson_to_long_term_tier` | Lesson → LongTerm, Lesson type, confidence=0.80, Cases |
| p1_08 | `reflection_gate_all_conditions` | 6 boundary combos of should_reflect() |

---

## P2: Fusion & Scoring (8 scenarios, sync)

Tests RRF/Weighted fusion math and query expansion behavior.

| # | Scenario | Validates |
|---|----------|-----------|
| p2_01 | `rrf_both_sources_overlap_ranks_highest` | Docs in both sources rank top |
| p2_02 | `rrf_single_source_degrades_gracefully` | Empty text → vec-only results preserved |
| p2_03 | `rrf_bm25_bonus_breaks_tie` | bonus=0.15 flips ranking for text-present doc |
| p2_04 | `weighted_fusion_exact_formula` | 0.7×1.0 + 0.3×0.5 = 0.85 |
| p2_05 | `rrf_normalization_max_is_one` | fused[0].score == 1.0 always |
| p2_06 | `query_expansion_chinese_injects_synonyms` | "喜欢" → bm25 contains "偏好" |
| p2_07 | `query_expansion_english_unchanged` | English → no expansion |
| p2_08 | `query_expansion_no_known_keywords_unchanged` | Unknown Chinese → no expansion |

---

## P3: Cross-Encoder Rerank (6 scenarios, sync)

Tests blend_scores math, config defaults, provider factory.

| # | Scenario | Validates |
|---|----------|-----------|
| p3_01 | `blend_scores_applies_weight_formula` | 0.6×rerank + 0.4×orig exact calc |
| p3_02 | `blend_scores_missing_rerank_uses_zero` | Missing doc → rerank_score=0.0, no panic |
| p3_03 | `blend_scores_preserves_order_when_weight_zero` | weight=0.0 → original order |
| p3_04 | `blend_scores_full_rerank_weight_ignores_original` | weight=1.0 → pure rerank order |
| p3_05 | `rerank_config_defaults_are_sane` | enabled=false, weight=0.6, timeout=5000 |
| p3_06 | `build_provider_returns_correct_type` | 5 providers → correct provider_id() |

---

## P4: Retrieval Trace (5 scenarios, sync)

Tests trace recording completeness and non-interference.

| # | Scenario | Validates |
|---|----------|-----------|
| p4_01 | `trace_records_all_pipeline_stages` | 7 stages recorded with name/duration/counts |
| p4_02 | `trace_stage_names_match_pipeline_order` | Exact 7-stage order verified |
| p4_03 | `trace_scores_descend_per_stage` | Rank 1 = highest score within each stage |
| p4_04 | `trace_total_duration_sums_all_stages` | total_duration_ms() == Σ stage durations |
| p4_05 | `trace_none_produces_no_side_effects` | run_traced(None) == run() identical output |

---

## P5: Decay & Promotion (8 scenarios, sync)

Tests tiered decay math, access reinforcement boundaries, promotion criteria.

| # | Scenario | Validates |
|---|----------|-----------|
| p5_01 | `tiered_decay_short_term_fastest` | 7d: ShortTerm < LongTerm < Core |
| p5_02 | `tiered_decay_half_life_inflection` | ShortTerm at 7d ≈ 0.5 (±0.05) |
| p5_03 | `protected_type_never_decays` | Personal 365d later → 1.0 |
| p5_04 | `access_reinforcement_extends_half_life` | 3 recent accesses → eff_hl > base |
| p5_05 | `access_reinforcement_stale_access_fades` | 50 accesses 90d ago → eff_hl ≈ base |
| p5_06 | `access_reinforcement_capped_at_max_multiplier` | 1000 accesses → eff_hl = base × 3.0 |
| p5_07 | `promotion_short_to_long_all_criteria` | 5 boundary combos (all met, access low, age low, strength low, Core ceiling) |
| p5_08 | `promotion_long_to_core_threshold` | access=15/age=60d/strength=0.8 → Core; access=5 → None |

---

## P6: End-to-End (6 scenarios, async + LanceDB)

Full lifecycle integration through Harness with real LanceDB.

| # | Scenario | Validates |
|---|----------|-----------|
| p6_01 | `fact_survives_full_write_retrieve_cycle` | Insert 3 facts → hybrid search → non-empty, scores > 0 |
| p6_02 | `rrf_fusion_outperforms_single_source` | hybrid top-1 ranks in top-3 of both vector-only and text-only |
| p6_03 | `tiered_decay_reshuffles_ranking_over_time` | 14d later: Core fact outranks ShortTerm fact |
| p6_04 | `access_reinforcement_keeps_popular_fact_alive` | 3 accesses → higher strength than unaccessed peer |
| p6_05 | `promotion_upgrades_tier_after_criteria_met` | ShortTerm + criteria met → LongTerm; re-check → None |
| p6_06 | `reflection_to_storage_round_trip` | Parse markdown → map → insert → search → find invariant with correct tier/type/confidence |

---

## Summary

| Phase | Focus | Scenarios | Test Type |
|-------|-------|-----------|-----------|
| P1 | Extraction & Classification | 8 | sync |
| P2 | Fusion & Scoring | 8 | sync |
| P3 | Cross-Encoder Rerank | 6 | sync |
| P4 | Retrieval Trace | 5 | sync |
| P5 | Decay & Promotion | 8 | sync |
| P6 | End-to-End | 6 | async + LanceDB |
| **Total** | | **41** | |

## Architecture Compliance

- **R8 (LLM Sovereignty):** Tests don't mock LLM judgment — reflection parser tests use pre-generated markdown (LLM output simulation), not regex-based intent detection
- **P6 (Simplicity):** MockEmbeddingProvider uses deterministic hashing, not a full model
- **P7 (Defensive):** P6 tests verify graceful behavior on empty results, decay edge cases, missing rerank scores
