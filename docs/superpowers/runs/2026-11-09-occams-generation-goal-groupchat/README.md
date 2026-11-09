# Occam's Razor Static Review & Remediation Report

**Date**: 2026-11-09  
**Target Modules**: `src/generation`, `src/goal`, `src/group_chat`  
**Total Scope**: 91 files | 33,406 LOC  
**Methodology**: Parallel Static Review with Subagents (`/rust-occams-razor`), Zero Interim Cargo Checks, Unified Final Compilation with Memory Control (`CARGO_BUILD_JOBS=2`).

---

## 1. Executive Summary

This static review targeted three core asynchronous and domain logic modules of Aleph (`generation`, `goal`, `group_chat`) using Occam's Razor principles. Review batches were strictly file-disjoint and processed in parallel. All candidate smells were triaged and classified using the **Jev tier**, while complex invariant analysis, concurrency contract preservation, and surgical fixes were authored by the **deep reasoning tier**.

All identified bugs, dead code, redundant multi-megabyte allocations, and logic regressions were fixed directly and committed in isolated batches on git branch `review/occams-gen-goal-gc` before being fast-forward merged to `main`.

---

## 2. Model Tier Allocation & Workflow

### Jev Tier (Classification, Filtering, Routing)
The Jev tier executed fast, high-volume static scanning and categorization:
1. **Candidate Sweep**: Scanned all 91 files across the 7 batches for Occam's razor anti-patterns (dead code, duplicate abstraction, redundant `.clone()`, unconstrained allocations in loops, TOCTOU/symlink gaps, missing invariant guards).
2. **Taxonomy Classification**: Categorized each smell into one of 5 classes:
   - `DEAD-CODE` / `ACCIDENTAL-COMPLEXITY`
   - `REDUNDANT-ALLOCATION`
   - `CONTROL-FLOW` / `INDIRECTION`
   - `PANIC-RISK` / `DEFENSIVE-CHECK`
   - `CORRECTNESS` / `INVARIANT-TIGHTENING`
3. **Filtering & Routing**: Filtered out intentional public API re-exports, tests, and false positives; assigned route tags:
   - `FIX-NOW`: High-priority correctness/performance fixes applied immediately.
   - `FIX-CAREFULLY`: Nuanced invariant or allocation tightening applied with surgical care.
   - `DEFER`: Broad redesigns requiring cross-crate protocol shifts.
   - `REJECT`: Intentional design or test assertions.
   - *Outputs stored in `classification/*.class.md`.*

### Deep Reasoning Tier (Invariant Analysis & Code Authoring)
The deep reasoning tier handled:
1. **Semantic Invariant Verification**: Ensured public API contracts, error enum variants, wire formats (OpenAI, Anthropic, Ark, Stability, Midjourney, BFL, Suno), lock orders, cancellation tokens, and `Send + Sync + 'static` bounds were strictly preserved.
2. **Surgical Patch Authoring**: Implemented zero-allocation streaming, ownership-passing patterns, iterator optimizations, and symlink jail guards.
3. **Commit Attribution**: Authored commit messages following conventional commits with detailed descriptions of why each change is necessary and minimal.

---

## 3. Batch Partitioning & Commit History

| Batch ID | Subsystem / Files | Files / LOC | Commit SHA | Summary of Changes |
|---|---|---|---|---|
| **G1** | `src/goal/*` | 4 files / 5,195 LOC | `396dafe05` | Preserve wait barriers in `commit_field_update`, deduplicate SQLite/JSON claim queries under mutex, eliminate redundant clones. |
| **GC1** | `src/group_chat/*` | 8 files / 3,796 LOC | `bca273f5b` | Fix respondent priority ordering (sort before truncate), support tab/whitespace command parsing, single-pass mention sanitization. |
| **GEN1** | `src/generation` core & types | 11 files / 4,414 LOC | `434801327` | Fix registry canonical alias removal parity, normalize probe casing/whitespace, prevent `NaN` progress crash, preserve diagnostic context in errors. |
| **GEN2** | `src/generation/providers` image | 11 files / 5,298 LOC | `528a26a91` | Report actual requested model on 404, prioritize `request.params.aspect_ratio`, trim trailing slashes, extract `Retry-After` on 429, eliminate payload clones. |
| **GEN3** | `src/generation/providers` video & polling | 21 files / 4,555 LOC | `2a428b4af` | Eliminate Replicate builder circular coupling, remove 1-impl `SubmitPolling` trait, hoist auth headers in polling loops, eliminate multi-MB payload clones. |
| **GEN4** | `src/generation/providers` audio/TTS/STT | 29 files / 7,535 LOC | `2d168d1f4` | Fix Deepgram STT symlink jail escaping CWD, preserve audio codecs in data URL MIME parsing, guard speed against `NaN`, eliminate audio allocations. |
| **GEN5** | `src/generation/providers/openai_compat` | 7 files / 2,613 LOC | `712b346c2` | Eliminate full-base64 lowercase allocation, deduplicate 70+ LOC edit output parsing into common helper, serialize request body directly to bytes. |

---

## 4. Key Bug Fixes & Invariant Tightening

1. **Wait Barrier Loss in Goal Store (`G1-01`)**:
   - `commit_field_update` was previously overwriting `wait_barriers` with empty defaults whenever transitioning an unparked goal to waiting. Fixed by explicitly retaining existing barriers.
2. **Group Chat Priority Inversion (`GC1-08`)**:
   - `GroupChatExecutor::determine_respondents` was executing `.truncate(max)` before `.sort_by_key(|r| r.order)`. High-priority respondents registered late in participant list were dropped while low-priority respondents earlier in list were picked. Fixed by sorting before truncating.
3. **Canonical Name Parity in Registry (`GEN1-01`)**:
   - `GenerationProviderRegistry::remove` only removed primary keys; calling `remove("openai-image")` for a provider registered as `"dalle"` returned `None` despite `contains()` returning `true`. Fixed to check and clean up `canonical_index`.
4. **Symlink Escape in Deepgram STT (`GEN4-01`)**:
   - `canonical.components().any(ParentDir)` was a no-op post-`canonicalize()`. Symlinks pointing to arbitrary absolute files outside working directory could be read. Fixed by verifying `canonical.starts_with(&cwd_canonical)`.
5. **Data URL Semicolon Parameter MIME Corruption (`GEN4-02`)**:
   - When data URLs included codec parameters (e.g., `data:audio/webm;codecs=opus;base64,...`), parsing looped through semicolon segments and assigned `mime = piece.to_string()`, corrupting the MIME type to `"codecs=opus"`. Fixed by parsing MIME strictly from the primary segment before semicolons.
6. **Multi-Megabyte Payload Duplication in Polling Providers (`GEN3-03`, `GEN3-04`, `GEN5-01`)**:
   - Multi-megabyte image and video downloads were cloned solely to read `.len()`, or called `.to_lowercase()` across entire base64 strings. All transformed into zero-copy, length-first borrowing and prefix matching.

---

## 5. Verification
- Unified compilation check executed across entire codebase with concurrency bounded (`CARGO_BUILD_JOBS=2`) to respect the 20 GB host memory envelope:
  ```bash
  CARGO_BUILD_JOBS=2 cargo check --lib --all-targets
  ```
  Result: **Pass (exit code 0)** in 8m 40s with zero errors across all libraries, binaries, and integration tests.
- Final commit on `main`: `78706aa3a`.
