# Cortex Retirement — Design

**Date:** 2026-04-12
**Branch:** main
**Scope:** Delete `src/memory/cortex/` (5530 lines)

## 1. Context

The module header at `src/memory/cortex/mod.rs` claims:

> **This module is deprecated.** Its capabilities have been absorbed by the POE module:
> - Meta-cognition (anchors, critic, reflector) → `crate::poe::meta_cognition`
> - Crystallization (distillation, clustering, dreaming) → `crate::poe::crystallization`

This is **false**. Verification:

- `ls src/poe/` → directory does not exist.
- `git log --all | grep -i "poe\|crystalliz"` → zero commits.
- No code anywhere imports from `crate::poe`.

POE was never built. The deprecation notice is aspirational, not factual.

## 2. Usage Audit

`grep -rn "memory::cortex\|cortex::\|CortexIntegration\|CortexDreamingService\|ClusteringService\|DistillationService\|PatternExtractor\|BehavioralAnchor\|AnchorStore" src/`:

- All matches inside `src/memory/cortex/` (self-references and sub-module tests)
- One re-export in `src/memory/mod.rs:74`
- One false positive in `src/engine/reflex_layer.rs` — it defines local types `SearchPatternExtractor` and `ReplacePatternExtractor` that happen to share the "PatternExtractor" substring; these are unrelated to `cortex::PatternExtractor`.

**Zero consumers outside `src/memory/cortex/` and the re-export.**

## 3. What is Deleted

The entire `src/memory/cortex/` directory:

| File | Lines |
|------|-------|
| `clustering.rs` | 342 |
| `distillation.rs` | 265 |
| `dreaming.rs` | 325 |
| `integration.rs` | 239 |
| `mod.rs` | 33 |
| `pattern_extractor.rs` | 363 |
| `types.rs` | 345 |
| `meta_cognition/anchor_store.rs` | 535 |
| `meta_cognition/conflict_detector.rs` | 245 |
| `meta_cognition/critic.rs` | 678 |
| `meta_cognition/injection.rs` | 580 |
| `meta_cognition/integration_tests.rs` | 502 |
| `meta_cognition/mod.rs` | 27 |
| `meta_cognition/reactive.rs` | 682 |
| `meta_cognition/schema.rs` | 105 |
| `meta_cognition/types.rs` | 264 |
| **Total** | **5530** |

Plus:

- `pub mod cortex;` declaration in `src/memory/mod.rs`
- All `pub use cortex::{...}` re-exports in `src/memory/mod.rs`
- Any database schema init that creates cortex tables (e.g., `initialize_schema` calls for `behavioral_anchors`, `experiences`, etc.). Identify via grep; delete the calls. Tables on disk remain harmless dead weight — no migration needed.
- Any config entries for cortex under `[memory.cortex]` or similar in `src/config/types/memory.rs`
- Any reference doc lines mentioning cortex/POE in `docs/reference/MEMORY_SYSTEM.md`, `docs/reference/memory/*.md`

## 4. What is NOT Deleted

- `src/engine/reflex_layer.rs` — unrelated despite shared "PatternExtractor" substring
- `src/poe/` — never existed; no action needed
- Any LLM-facing behavior that used cortex indirectly — grep returns zero such consumers, so there is nothing to preserve

## 5. Principles Applied

- **R8 LLM Sovereignty.** Cortex's pattern extraction, clustering, meta-cognitive critic, and behavioral anchors are deterministic code substituting for LLM judgment. If these capabilities are ever needed, they belong in prompt templates, not in 5000+ lines of Rust heuristics.
- **R6 KISS / YAGNI.** No caller. No roadmap to wire one. Delete.
- **R3 Core Minimalism.** 5530 lines removed from the core crate.

## 6. Non-Goals

- Building POE. If future work needs memory evolution, design it from first principles under current architectural constraints.
- Preserving experience-replay semantics. The design doc for whatever replaces this should decide from scratch whether that concept is useful.
- Migrating cortex database tables. Tables on disk are harmless; removing them is a separate cleanup.

## 7. Verification

After deletion:

- `cargo check -p alephcore` — clean.
- `cargo test -p alephcore --lib --no-fail-fast` — 8693 pass, 0 fail (current baseline after Phase 1). No new failures.
- `grep -rn "memory::cortex\|cortex::\|CortexIntegration" src/` — zero hits.
- `grep -rn "memory::cortex\|cortex::" docs/reference/` — zero hits (after doc sweep).

## 8. Risks

| Risk | Mitigation |
|------|------------|
| A consumer exists that the grep missed (e.g., via a dyn trait object or generic) | Compile-time: `cargo check` will fail if any consumer exists. The error list is the escape hatch. |
| Database tables created by cortex's `initialize_schema` break future migrations | Tables are additive; they persist as dead weight until a future cleanup explicitly drops them. |
| Some test file imports cortex transitively | Covered by `cargo test` run. |

## 9. Rollback

Single commit; `git revert` restores. The deletion is mechanical — revert is zero-risk.

## 10. Execution

Single atomic commit: `memory: retire cortex module (dead, POE never built)`.
