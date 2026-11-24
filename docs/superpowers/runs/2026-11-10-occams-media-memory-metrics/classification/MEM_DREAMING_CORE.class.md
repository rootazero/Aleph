# Classification & Routing Matrix: MEM_DREAMING_CORE

## 1. Executive Summary

This document provides the Jev-tier classification, taxonomy breakdown, and routing decisions for **MEM_DREAMING_CORE** (`src/memory/dreaming/*.rs` and `src/memory/dreaming/evolution/*.rs`, 17 files total).

Under the Occam's Razor doctrine (`/rust-occams-razor`), our mission is to eliminate accidental complexity, redundant allocations, dead parameters, and unnecessary indirection while strictly preserving subsystem invariants:
1. **Convergence & Monotonicity**: Note consolidation, decay, drift detection, and SkillOpt evolution gates must guarantee convergence and never corrupt persistent notes.
2. **LLM Sovereignty (R7)**: No deterministic rules replacing semantic LLM decisions; structural gates operate solely on identifiers, digests, and arithmetic scores.
3. **Data Integrity & Provenance**: Single persistent source of truth (`dream_events.jsonl`, `dream_reports`) without loss of cross-cycle rehydration or auditability.

Across the 17 files in scope:
- **11 FIX-NOW** candidates identified: direct zero-allocation refactorings, dead parameter removal, in-place string manipulation, and set-operation mathematical reductions.
- **3 FIX-CAREFULLY** candidates: cross-function signature alignments (`run_l2_validation` slice borrowing and event log buffer reuse).
- **2 DEFER** candidates: public API / trait boundaries requiring broader migration.
- **5 REJECT** candidates: optimal safety patterns, data modeling invariants, and test scaffolding.

---

## 2. Invariant Safety Pre-Flight

- **A3 Persistent Rehydration**: `MutationGate`, `StrategySelector`, and `ProjectCycle` derive cross-cycle state directly from `EventLog` (`dream_events.jsonl`). Any refactoring must preserve exact history sliding windows (`DREAM_HISTORY_WINDOW = 5`, `PERSONALITY_WINDOW = 10`).
- **R7 Structural vs. Semantic Separation**: Structural gates (`MutationGate`, `EditBudget`, `evidence::recall_support`, `score::memory_health_score`) compare mathematical bounds and digests. No semantic text classification or heuristics may be introduced.
- **Fail-Closed Durability**: Vacuous cycles (interrupted before first stage) must append no records to avoid polluting the churn window. Forced and scheduled paths must maintain synchronized status reporting (`DreamStatus`).

---

## 3. Classification & Routing Matrix

| Location | Taxonomy | Smell Description | Route | Remediation Plan |
|---|---|---|---|---|
| [`src/memory/dreaming/distill_action.rs:139-146`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/distill_action.rs#L139-L146) | REDUNDANT-ALLOCATION | Converts `Severity` enum by serializing through `serde_json::to_value` (allocating JSON AST) to call `.as_str().map(str::to_string)`. | `FIX-NOW` | Replace with direct `match s { Severity::Low => "low", Severity::Med => "med", Severity::High => "high", Severity::Critical => "critical" }`. |
| [`src/memory/dreaming/distill_action.rs:158-190`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/distill_action.rs#L158-L190) | REDUNDANT-ALLOCATION | `DistillActionRecord::from_action` takes borrowed `&DistillAction` and clones strings (`title`, `paths`). | `DEFER` | Public API helper; borrowing string slices would require adding lifetimes to `DistillActionRecord`. |
| [`src/memory/dreaming/event_log.rs:147`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/event_log.rs#L147) | REDUNDANT-ALLOCATION | `vec![0u8; window as usize]` reallocated in every tail expansion step in `read_last_measured`. | `FIX-CAREFULLY` | Allocate outside loop and resize/clear on geometric expansion. |
| [`src/memory/dreaming/event_log.rs:195-198`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/event_log.rs#L195-L198) | REDUNDANT-ALLOCATION | `next_cycle` deserializes full `DreamEvent` including nested reports just to read `cycle: u32`. | `DEFER` | Low execution frequency (once per cycle start); full deserialization validates trailing line integrity. |
| [`src/memory/dreaming/lifecycle_tests.rs:1-98`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/lifecycle_tests.rs#L1-L98) | INTENTIONAL-PATTERN | Test scaffolding for runtime shutdown drain and lifecycle admission gate. | `REJECT` | Valid test harness. No simplification required. |
| [`src/memory/dreaming/mod.rs:684-686`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/mod.rs#L684-L686) | REDUNDANT-ALLOCATION | `last_date.format("%Y-%m-%d").to_string() != today` allocates heap `String` on each tick check. | `FIX-NOW` | Compare formatted date without heap allocation or format into small stack buffer. |
| [`src/memory/dreaming/mod.rs:912-918, 1142-1148`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/mod.rs#L912-L918) | ACCIDENTAL-COMPLEXITY | Duplicate local struct `RunGuard<'a>(&'a AtomicBool)` and its `Drop` implementation in both `run_now` and `check_and_run`. | `FIX-NOW` | Deduplicate into a single private struct `DreamRunGuard<'a>` within `mod.rs`. |
| [`src/memory/dreaming/mod.rs:1259, 1180, 941`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/mod.rs#L1259) | DEAD-CODE / REDUNDANT-ALLOCATION | `_run_date: String` parameter in `run_dream` is unused; callers execute `run_date.clone()` just to supply it. | `FIX-NOW` | Remove `_run_date` parameter from `run_dream` and delete caller `.clone()`. |
| [`src/memory/dreaming/mod.rs:1542-1546`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/mod.rs#L1542-L1546) | REDUNDANT-ALLOCATION | Clones `path` and `content_hash` for every note in `post_index` into `Vec<(String, String)>` for L2 validation. | `FIX-CAREFULLY` | Refactor `run_l2_validation` to accept `(&str, &str)` slices/iterators, eliminating all clones. |
| [`src/memory/dreaming/mod.rs:1774-1790`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/mod.rs#L1774-L1790) | CONTROL-FLOW | 4 separate `.iter().filter(...)` passes over `notes` with identical `n.category == "skill"` condition. | `FIX-NOW` | Consolidate skill counts into a single O(N) iteration pass. |
| [`src/memory/dreaming/mutation_gate.rs:199-201`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/mutation_gate.rs#L199-L201) | REDUNDANT-ALLOCATION | `self.merge_history.iter().collect::<Vec<&HashSet<_>>>()` allocates heap vector on every merge check. | `FIX-NOW` | Index directly over `VecDeque` or use contiguous slices without allocating intermediate `Vec`. |
| [`src/memory/dreaming/mutation_gate.rs:230-233`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/mutation_gate.rs#L230-L233) | REDUNDANT-ALLOCATION | `self.synthesis_history.iter().collect::<Vec<&HashMap<_>>>()` allocates heap vector just to call `split_first`. | `FIX-NOW` | Use `self.synthesis_history.front()` and `.iter().skip(1)` without heap allocation. |
| [`src/memory/dreaming/mutation_gate.rs:260-264`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/mutation_gate.rs#L260-L264) | PANIC-RISK | Unchecked `+` addition in `fold` for `(total_produced, total_recalled)`. | `FIX-NOW` | Replace with `saturating_add` or `u64` arithmetic. |
| [`src/memory/dreaming/project_cycle.rs:343-347`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/project_cycle.rs#L343-L347) | REDUNDANT-ALLOCATION | Clones `(path, content_hash)` for all project corpus notes into `Vec<(String, String)>`. | `FIX-CAREFULLY` | Share zero-allocation `(&str, &str)` borrowed L2 validator with `mod.rs`. |
| [`src/memory/dreaming/project_cycle.rs:423, 455`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/project_cycle.rs#L423) | CONTROL-FLOW | Duplicate `Box::new(DreamCycleOutcome { status, report, decision })` construction across early and late return arms. | `FIX-NOW` | Consolidate outcome packaging into single return path. |
| [`src/memory/dreaming/report.rs:1-169`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/report.rs#L1-L169) | INTENTIONAL-PATTERN | Data transfer objects (`DreamReport`, `CycleDecision`, `DreamReportStatus`). | `REJECT` | Explicit serde attributes, zero redundant complexity. |
| [`src/memory/dreaming/selector.rs:46-49, 59`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/selector.rs#L46-L49) | ACCIDENTAL-COMPLEXITY | `struct CycleRecord { validation_passed: bool }` wraps a single boolean in `VecDeque<CycleRecord>`. | `FIX-NOW` | Replace `VecDeque<CycleRecord>` with `VecDeque<bool>` and prune wrapper struct. |
| [`src/memory/dreaming/selector.rs:125-134`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/selector.rs#L125-L134) | CONTROL-FLOW | Match block with empty fall-through arm on `GateDecision::Allow`. | `FIX-NOW` | Replace with idiomatic `if let GateDecision::Conserve { reason, .. } = gate`. |
| [`src/memory/dreaming/signals.rs:68-127`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/signals.rs#L68-L127) | REDUNDANT-ALLOCATION | `let mut signals = Vec::new();` with 6 pushes causes dynamic vector reallocation. | `FIX-NOW` | Preallocate `Vec::with_capacity(6)`. |
| [`src/memory/dreaming/skill_gate.rs:48-110`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/skill_gate.rs#L48-L110) | CONTROL-FLOW / ACCIDENTAL-COMPLEXITY | Destructures and reconstructs entire `DistillAction` enum variants (`New`, `Supersede`, `Strengthen`) on validation pass. | `FIX-NOW` | Match `mut action` by reference/mut, update `severity` in-place, and return `Allow(action)`. |
| [`src/memory/dreaming/skill_gate.rs:118-123`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/skill_gate.rs#L118-L123) | REDUNDANT-ALLOCATION | Overlong `reason` collects characters into a new `String` instead of truncating in place. | `FIX-NOW` | Truncate in-place using character boundary index. |
| [`src/memory/dreaming/strategy.rs:36-44`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/strategy.rs#L36-L44) | ACCIDENTAL-COMPLEXITY | Missing `as_str()` helper for `DreamStrategy`; callers must invoke `write!` or `.to_string()`. | `FIX-NOW` | Add `pub const fn as_str(&self) -> &'static str`. |
| [`src/memory/dreaming/validation.rs:105-106`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/validation.rs#L105-L106) | REDUNDANT-ALLOCATION | `let valid_categories: HashSet<&str> = CATEGORY_DIRS.iter().copied().collect();` allocated for EVERY validated note. | `FIX-NOW` | Replace with zero-allocation `CATEGORY_DIRS.contains(&cat)`. |
| [`src/memory/dreaming/validation.rs:129-140`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/validation.rs#L129-L140) | REDUNDANT-ALLOCATION | `extract_yaml_value` allocates two `String`s (`format!("{key}:")` and `.to_string()`). | `FIX-NOW` | Return `Option<&str>` without string formatting or allocation. |
| [`src/memory/dreaming/validation.rs:71, 78, 95`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/validation.rs#L71) | REDUNDANT-ALLOCATION | `let tier = "L1".to_string();` repeatedly cloned with `tier.clone()`. | `FIX-NOW` | Use string literal `"L1".to_string()` directly or static slice. |
| [`src/memory/dreaming/validation.rs:148, 200`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/validation.rs#L148) | API-RIGIDITY | `check_duplicate_hashes` & `run_l2_validation` require owned `&[(String, String)]`. | `FIX-CAREFULLY` | Generalize to accept `(&str, &str)` pairs. |
| [`src/memory/dreaming/evolution/budget.rs:1-124`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/evolution/budget.rs#L1-L124) | INTENTIONAL-PATTERN | Pure arithmetic learning rate controller (`EditBudget`). | `REJECT` | Optimal design; no simplification needed. |
| [`src/memory/dreaming/evolution/evidence.rs:85-86`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/evolution/evidence.rs#L85-L86) | REDUNDANT-ALLOCATION | `Sha256::digest(format!("supersede|{old_note_path}|{title}|{rule}"))` allocates temporary format string up to 4KB+. | `FIX-NOW` | Stream byte slices directly into hasher: `hasher.update(...)`. |
| [`src/memory/dreaming/evolution/gate.rs:1-91`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/evolution/gate.rs#L1-L91) | INTENTIONAL-PATTERN | SkillOpt arithmetic decision gate (`evaluate_gate`). | `REJECT` | Zero-allocation, pure math. |
| [`src/memory/dreaming/evolution/mod.rs:1-78`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/evolution/mod.rs#L1-L78) | INTENTIONAL-PATTERN | Module facade and clean re-exports. | `REJECT` | Optimal design. |
| [`src/memory/dreaming/evolution/score.rs:49-67`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/evolution/score.rs#L49-L67) | ACCIDENTAL-COMPLEXITY | Runs `union(&af).count()` and `difference(&kf).count()` after calculating `overlap`. | `FIX-NOW` | Derive `union = kf.len() + af.len() - overlap` and `novel = af.len() - overlap` in O(1) math. |
| [`src/memory/dreaming/evolution/score.rs:83-93`](file:///home/zou/data/workspace/Aleph/src/memory/dreaming/evolution/score.rs#L83-L93) | REDUNDANT-ALLOCATION | `share_leading_token` allocates two heap `String`s using `.to_lowercase()`. | `FIX-NOW` | Use `eq_ignore_ascii_case` on token slices without heap allocation. |

---

## 4. Deep-Dive Remediation Guides

### Category A: REDUNDANT-ALLOCATION (Hot-Path Allocation Removal)

1. **`validation.rs:105-106` (`CATEGORY_DIRS` HashSet creation)**
   - *Current*:
     ```rust
     let valid_categories: HashSet<&str> = CATEGORY_DIRS.iter().copied().collect();
     if !valid_categories.contains(cat.as_str()) { ... }
     ```
   - *Fix*:
     ```rust
     if !CATEGORY_DIRS.contains(&cat) { ... }
     ```
   - *Benefit*: Eliminates 1 `HashSet` allocation per note validated in L1 (up to 200 allocations per dream cycle).

2. **`evolution/evidence.rs:85-86` (Hashing format string allocation)**
   - *Current*:
     ```rust
     let digest = Sha256::digest(format!("supersede|{old_note_path}|{title}|{rule}"));
     ```
   - *Fix*:
     ```rust
     let mut hasher = Sha256::new();
     hasher.update(b"supersede|");
     hasher.update(old_note_path.as_bytes());
     hasher.update(b"|");
     hasher.update(title.as_bytes());
     hasher.update(b"|");
     hasher.update(rule.as_bytes());
     let digest = hasher.finalize();
     ```
   - *Benefit*: Eliminates multi-kilobyte string formatting buffer during skill supersede evidence checks.

3. **`evolution/score.rs:83-93` (Leading token lowercase allocation)**
   - *Current*: Allocates two `String`s via `.to_lowercase()` to compare the first word of note titles.
   - *Fix*: Extract `&str` slices and compare using `.eq_ignore_ascii_case()`.
   - *Benefit*: Completely zero-allocation title affinity check.

4. **`validation.rs:129-140` (`extract_yaml_value`)**
   - *Current*: Creates formatted prefix `format!("{key}:")` and returns owned `String`.
   - *Fix*: Match line prefix with byte search or two-step slice strip and return `Option<&str>`.
   - *Benefit*: Zero-allocation frontmatter parsing.

5. **`distill_action.rs:139-146` (`severity_str`)**
   - *Current*: Calls `serde_json::to_value(s)` (JSON tree serialization) followed by `as_str().map(str::to_string)`.
   - *Fix*: Match on `s` producing a static `&'static str` (`"low"`, `"med"`, `"high"`, `"critical"`).
   - *Benefit*: Instantaneous const-table translation without serde overhead.

6. **`signals.rs:68` (`SignalSnapshot::from_metrics`)**
   - *Current*: `Vec::new()` followed by 6 `.push()` invocations.
   - *Fix*: `Vec::with_capacity(6)`.
   - *Benefit*: Eliminates dynamic buffer resizing.

---

### Category B: ACCIDENTAL-COMPLEXITY & CONTROL-FLOW

1. **`evolution/score.rs:49-67` (Set operation reduction)**
   - Jaccard similarity and novelty difference can be derived directly from `kf.len()`, `af.len()`, and `overlap`:
     $$\text{union} = |K| + |A| - \text{overlap}$$
     $$\text{novel\_in\_absorbed} = |A| - \text{overlap}$$
   - Saves 2 full hash table iterations per merge candidate evaluation.

2. **`skill_gate.rs:48-110` (`validate_skill_action` destructuring/reconstructing)**
   - Instead of breaking apart the enum and rebuilding every field, pattern match by reference on `mut action` and modify only `severity` if low confidence. Return `SkillGateDecision::Allow(action)`.

3. **`selector.rs:46-49` (`CycleRecord` single-field struct)**
   - Replace `VecDeque<CycleRecord>` with `VecDeque<bool>` for personality adaptation tracking.

4. **`mod.rs:1259` (`_run_date` dead parameter)**
   - `_run_date: String` is prefixed with an underscore and never referenced in `run_dream`. Remove parameter and eliminate `.clone()` in `check_and_run`.

5. **`mod.rs:912, 1142` (`RunGuard` duplication)**
   - Unify the duplicate local struct definitions into a single crate-private or module-level RAII guard.

---

### Category C: FIX-CAREFULLY & CROSS-CUTTING SIGNATURES

1. **`validation::run_l2_validation` Borrowed Signature**
   - Both `mod.rs:1542` and `project_cycle.rs:343` clone every `(path, content_hash)` in the corpus just to satisfy `&[(String, String)]`.
   - Updating `check_duplicate_hashes` to accept `impl IntoIterator<Item = (&str, &str)>` or `&[(&str, &str)]` removes heap allocations across the entire note index during validation.

---

## 5. Verification Checklist for Deep Reasoning Tier

1. **Compilation & Unit Tests**: Ensure all 17 files pass existing tests (`cargo test memory::dreaming`).
2. **Behavioral Invariant Preservation**:
   - `test_roundtrip`: Confirm serialized JSON forms of `DistillActionRecord`, `DreamEvent`, `DreamReport`, and `CycleDecision` remain identical.
   - `score_merge_candidate`: Verify numerical identity of the Jaccard union and novelty calculations.
   - `action_fingerprint`: Verify 16-hex sha256 output matches previous values for test fixtures.
   - `validate_skill_action`: Verify downgrade logic from High/Critical to Med for confidence < 0.5 remains identical.
