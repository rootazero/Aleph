# Classification & Routing Matrix: Batch MEM_COMPRESS_SESSION

## Executive Summary

Batch `MEM_COMPRESS_SESSION` covers 26 files (8,556 total lines) spanning memory compression, session compaction, reflection, resume snapshots, and search summarization:
- **Compression Orchestration**: [`src/memory/compression/`](file:///home/zou/data/workspace/Aleph/src/memory/compression/) (`mod.rs`, `scheduler.rs`, `service.rs`, `service_lifecycle_tests.rs`, `source_prompts.rs`)
- **Session Compactor**: [`src/memory/session_compactor/`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/) (`mod.rs`, `constructor.rs`, `context_window.rs`, `fallback.rs`, `helpers.rs`, `post_turn_compress.rs`, `prepare_history.rs`, `summary_engine.rs`, `summary_source.rs`, `chunker_tests.rs`, `tests.rs`)
- **Session Reflection**: [`src/memory/session_reflection/mod.rs`](file:///home/zou/data/workspace/Aleph/src/memory/session_reflection/mod.rs)
- **Session Resume**: [`src/memory/session_resume/`](file:///home/zou/data/workspace/Aleph/src/memory/session_resume/) (`mod.rs`, `snapshot.rs`, `reader.rs`, `writer.rs`)
- **Session Search Summary**: [`src/memory/session_search_summary/`](file:///home/zou/data/workspace/Aleph/src/memory/session_search_summary/) (`mod.rs`, `filter.rs`, `lookup.rs`, `end_hook.rs`, `synthesizer.rs`)

### Invariant Safety Pre-Flight
1. **R7/R8/R9/R10 Compliance**: The codebase strongly embodies LLM sovereignty: deterministic keyword-based signal detectors have been retired in favor of content-blind cadence and tool-driven flags (`flag_user_correction`). Prompts mandate verbatim identifier preservation and absolute dates.
2. **Data & Storage Integrity**: SQLite migrations, Markdown-backed `RawMemoryStore` invariants, partition isolation (`snapshot_partition`, `session_write_id`), and atomic rename writes (`write_atomic`) are rigorously respected.
3. **Concurrency & Locking**: Per-agent and per-(session, agent) serialization locks (`ingest_locks`, `compress_locks`, single-flight `InFlightTable`) prevent duplicate LLM invocations and database races.

---

## Classification & Routing Matrix

| ID | Location | Taxonomy | Smell Description | Proposed Action | Route | Risk |
|---|---|---|---|---|---|---|
| **MCS-01** | [`src/memory/session_compactor/post_turn_compress.rs#L103-L123`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/post_turn_compress.rs#L103-L123) | `CONTROL-FLOW / INDIRECTION` | Allocates `pairs: Vec<(String, String)>` cloning every message content before checking if `raw_messages.len() <= fresh_tail_count`. If tail check fails, all allocations are discarded. | Early return `if raw_messages.len() <= self.config.fresh_tail_count { return Ok(Default::default()); }` before converting any messages. | `FIX-NOW` | Low |
| **MCS-02** | [`src/memory/session_compactor/post_turn_compress.rs#L175-L179`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/post_turn_compress.rs#L175-L179), [`#L276-L280`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/post_turn_compress.rs#L276-L280) | `REDUNDANT-ALLOCATION` | Identical string formatting (`raw_text` vs `raw_content`) computed twice in the same loop via temporary `Vec<String>` and `.join("\n\n")`. | Format chunk text once into a single `raw_content` string and reuse across pre-compress writer and indexer. | `FIX-NOW` | Low |
| **MCS-03** | [`src/memory/session_compactor/summary_source.rs#L137-L144`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/summary_source.rs#L137-L144), [`src/memory/session_search_summary/end_hook.rs#L78-L87`](file:///home/zou/data/workspace/Aleph/src/memory/session_search_summary/end_hook.rs#L78-L87) | `DEAD-CODE / ACCIDENTAL-COMPLEXITY` | Shape 1 ("同一事实的两份表述"): `extract_depth` implemented in 3 places. `end_hook.rs` explicitly inlines an exact duplicate because `summary_source::extract_depth` was private, while `summary_source.rs` has a fragile `split('/')` version. | Expose canonical `extract_depth` from `session_compactor` and reuse across `summary_source.rs` and `end_hook.rs`. | `FIX-NOW` | Low |
| **MCS-04** | [`src/memory/session_compactor/fallback.rs#L39-L44`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/fallback.rs#L39-L44) | `CORRECTNESS / INVARIANT-TIGHTENING` | `deterministic_truncate` uses `.take_while(|(i, _)| *i <= max_chars).last().map_or(0, |(i, c)| i + c.len_utf8())`, which overshoots `max_chars` by up to `c.len_utf8()` (multibyte boundary overshoot). | Use `result.floor_char_boundary(max_chars)` or clamp boundary to ensure `<= max_chars`. | `FIX-NOW` | Low |
| **MCS-05** | [`src/memory/session_compactor/helpers.rs#L154-L156`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/helpers.rs#L154-L156), [`#L258-L260`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/helpers.rs#L258-L260), [`src/memory/session_compactor/post_turn_compress.rs#L228-L230`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/post_turn_compress.rs#L228-L230) | `REDUNDANT-ALLOCATION` | `let store: Arc<dyn RawMemoryStore> = self.database.clone();` just to pass `&store` to `insert_with_runtime_capture_filter` (accompanied by `// rust-doctor-disable-next-line excessive-clone`). | Pass `&self.database` directly; no `Arc::clone` required. | `FIX-NOW` | Low |
| **MCS-06** | [`src/memory/compression/service.rs#L474`](file:///home/zou/data/workspace/Aleph/src/memory/compression/service.rs#L474), [`#L832-L837`](file:///home/zou/data/workspace/Aleph/src/memory/compression/service.rs#L832-L837) | `REDUNDANT-ALLOCATION` | `ingest_group_key` allocates a new `String` for every raw memory in the batch during grouping check. | Return `&'static str` (`to_persisted().0` is already static; `SessionEndReason` variants map to static literals `"session_end/Disconnect"` / `"session_end/TaskDone"`). | `FIX-NOW` | Low |
| **MCS-07** | [`src/memory/compression/service.rs#L393-L397`](file:///home/zou/data/workspace/Aleph/src/memory/compression/service.rs#L393-L397) | `REDUNDANT-ALLOCATION` | `let digest: String = session_end_raws.iter().map(|r| r.content.clone()).collect::<Vec<_>>().join("\n");` allocates intermediate `Vec<String>` and clones all contents. | Build `digest` string with pre-allocated capacity and iterate with `push_str`, avoiding intermediate `Vec` and clones. | `FIX-NOW` | Low |
| **MCS-08** | [`src/memory/compression/service.rs#L308`](file:///home/zou/data/workspace/Aleph/src/memory/compression/service.rs#L308), [`#L386`](file:///home/zou/data/workspace/Aleph/src/memory/compression/service.rs#L386) | `REDUNDANT-ALLOCATION` | `let Some(ing) = self.compound_ingestor.clone()` and `let Some(ps) = self.profile_synthesizer.clone()` perform unnecessary `Arc` clones before validation checks. | Borrow via `if let Some(ing) = &self.compound_ingestor`; only clone when spawning background task. | `FIX-NOW` | Low |
| **MCS-09** | [`src/memory/compression/service.rs#L336-L342`](file:///home/zou/data/workspace/Aleph/src/memory/compression/service.rs#L336-L342) | `REDUNDANT-ALLOCATION` | `raw_memories.iter().cloned().partition(|r| ...)` clones the entire batch of `RawMemory` instances (including IDs, strings, tags) into two owned vectors. | Compute `latest_timestamp` and partition `raw_memories.into_iter()` directly without cloning all payloads. | `FIX-CAREFULLY` | Medium |
| **MCS-10** | [`src/memory/session_compactor/prepare_history.rs#L73-L78`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/prepare_history.rs#L73-L78) | `CONTROL-FLOW / INDIRECTION` | On summary fetch error, `agent.get_history(session_key, None).await` is called a second time despite `raw_messages` already sitting in local scope from line 41. | Reuse `raw_messages` directly on error fallback. | `FIX-NOW` | Low |
| **MCS-11** | [`src/memory/session_compactor/prepare_history.rs#L88-L96`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/prepare_history.rs#L88-L96), [`#L189-L197`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/prepare_history.rs#L189-L197) | `REDUNDANT-ALLOCATION` | Unconditional allocation of 8+ `String`s and `Vec<(&'static str, String)>` for hook statistics when `hooks` is `None` or contains 0 hooks. | Check `if hooks.map_or(0, |h| h.hook_count()) > 0` before assembling the stats vectors. | `FIX-NOW` | Low |
| **MCS-12** | [`src/memory/session_compactor/summary_source.rs#L85-L90`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/summary_source.rs#L85-L90) | `REDUNDANT-ALLOCATION` | Collects `UnifiedMessage::transcript_text` into temporary `Vec<String>` and joins with `\n` solely to run `estimate_tokens(&window_text)`. | Sum token estimates directly per message slice without allocating temporary vector and joined string. | `FIX-NOW` | Low |
| **MCS-13** | [`src/memory/session_reflection/mod.rs#L368-L381`](file:///home/zou/data/workspace/Aleph/src/memory/session_reflection/mod.rs#L368-L381) | `REDUNDANT-ALLOCATION` | `output.to_ascii_lowercase()` copies up to 32k characters of LLM output just to find the ASCII substring `"open_loops:"`. | Perform case-insensitive ASCII subslice match without allocating a lowercase string copy. | `FIX-NOW` | Low |
| **MCS-14** | [`src/memory/session_reflection/mod.rs#L386-L392`](file:///home/zou/data/workspace/Aleph/src/memory/session_reflection/mod.rs#L386-L392) | `REDUNDANT-ALLOCATION` | `strip_header` returns owned `String` which is then borrowed by `normalize_section(&strip_header(...))` which allocates another `String`. | Return `&str` from `strip_header` zero-alloc. | `FIX-NOW` | Low |
| **MCS-15** | [`src/memory/session_reflection/mod.rs#L433-L435`](file:///home/zou/data/workspace/Aleph/src/memory/session_reflection/mod.rs#L433-L435) | `REDUNDANT-ALLOCATION` | Double consecutive `replace` allocations on `resolved_id` in `open_loops_path`. | Combine sanitization into a single pass or use `sanitize_session_id`. | `FIX-NOW` | Low |
| **MCS-16** | [`src/memory/session_reflection/mod.rs#L338-L342`](file:///home/zou/data/workspace/Aleph/src/memory/session_reflection/mod.rs#L338-L342), [`src/memory/session_compactor/summary_engine.rs#L123`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/summary_engine.rs#L123), [`src/memory/session_compactor/fallback.rs#L35`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/fallback.rs#L35) | `REDUNDANT-ALLOCATION` | Repeated `format!("[{role}]: {content}")` inside loops allocating new strings for each conversation message. | Use `write!(prompt, ...)` or direct `push_str` sequence. | `FIX-NOW` | Low |
| **MCS-17** | [`src/memory/session_resume/writer.rs#L179-L183`](file:///home/zou/data/workspace/Aleph/src/memory/session_resume/writer.rs#L179-L183) | `REDUNDANT-ALLOCATION` | Directory cleanup sweep parses full `SessionSnapshot` including large `summary` strings just to extract `(agent_id, scope_id)`. | Deserialize into an owner-only projection struct (`struct SnapshotOwner { agent_id: String, scope_id: Option<String> }`). | `FIX-NOW` | Low |
| **MCS-18** | [`src/memory/session_search_summary/end_hook.rs#L64-L69`](file:///home/zou/data/workspace/Aleph/src/memory/session_search_summary/end_hook.rs#L64-L69) | `REDUNDANT-ALLOCATION` | `best.map(|src| RawMemory::new(src.content.clone(), ...))` unnecessarily clones `content` when `src` is already an owned `RawMemory` from `into_iter()`. | Move `src.content` directly: `RawMemory::new(src.content, ...)`. | `FIX-NOW` | Low |
| **MCS-19** | [`src/memory/session_compactor/constructor.rs#L12`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/constructor.rs#L12), [`#L50-L60`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/constructor.rs#L50-L60) | `DEAD-CODE / ACCIDENTAL-COMPLEXITY` | `SessionCompactor.raw_memory_writer` is always set to `Some(self.database.clone())` and only read at one site (`post_turn_compress.rs:174`). | Simplify internal calls to read `&self.database`; keep `with_raw_memory_writer` as API wrapper. | `FIX-CAREFULLY` | Low |
| **MCS-20** | [`src/memory/session_compactor/context_window.rs#L21-L30`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/context_window.rs#L21-L30) | `ACCIDENTAL-COMPLEXITY` | `partition_fresh_tail_pairs` restricted to `&[(String, String)]` when it only calculates `messages.len().saturating_sub(fresh_tail_count)`. | Make generic `&[T]` or inline `saturating_sub`. | `FIX-NOW` | Low |
| **MCS-21** | [`src/memory/session_compactor/summary_engine.rs#L144-L166`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/summary_engine.rs#L144-L166) | `REDUNDANT-ALLOCATION` | `summary_to_fact` creates an intermediate `MemoryFact` that is immediately discarded after cloning its `content` and `path` into `RawMemory`. | Add direct `summary_to_raw` constructor moving strings; keep `summary_to_fact` for backwards compatibility. | `FIX-CAREFULLY` | Low |
| **MCS-22** | [`src/memory/session_search_summary/end_hook.rs#L116-L150`](file:///home/zou/data/workspace/Aleph/src/memory/session_search_summary/end_hook.rs#L116-L150) | `DEAD-CODE / ACCIDENTAL-COMPLEXITY` | `MockSummaryLlm` and `InMemorySessionStore` test mocks in `end_hook.rs` duplicate identical implementations in `synthesizer::test_support`. | Import test support structs from `synthesizer::test_support`. | `DEFER` | Low |

---

## Detailed Findings & Remediation Analysis

### 1. Compression Subsystem (`src/memory/compression/`)

#### MCS-06: `ingest_group_key` Allocations in Batch Partitioning Loop
- **File**: [`src/memory/compression/service.rs#L474`](file:///home/zou/data/workspace/Aleph/src/memory/compression/service.rs#L474), [`#L832-L837`](file:///home/zou/data/workspace/Aleph/src/memory/compression/service.rs#L832-L837)
- **Problem**: In `compress_to_notes`, each raw memory is partitioned into groups using `ingest_group_key(&r.source)`. The function allocates a `String` on every call:
  ```rust
  fn ingest_group_key(source: &crate::memory::store::raw_memory::RawMemorySource) -> String {
      use crate::memory::store::raw_memory::RawMemorySource;
      match source {
          RawMemorySource::SessionEnd { reason } => format!("session_end/{reason:?}"),
          other => other.to_persisted().0.to_string(),
      }
  }
  ```
  In line 474:
  ```rust
  for r in ingestable {
      let key = ingest_group_key(&r.source);
      match groups.iter_mut().find(|(k, _)| *k == key) {
  ```
  This creates and drops a heap-allocated string for every single memory item.
- **Remediation**: `to_persisted().0` is already `&'static str`. The `SessionEnd` reasons are `Disconnect` and `TaskDone`. Returning `&'static str` eliminates all allocations:
  ```rust
  fn ingest_group_key(source: &RawMemorySource) -> &'static str {
      match source {
          RawMemorySource::SessionEnd { reason: SessionEndReason::Disconnect } => "session_end/Disconnect",
          RawMemorySource::SessionEnd { reason: SessionEndReason::TaskDone } => "session_end/TaskDone",
          other => other.to_persisted().0,
      }
  }
  ```

#### MCS-07: Digest Allocation in SessionEnd Profile Trigger
- **File**: [`src/memory/compression/service.rs#L393-L397`](file:///home/zou/data/workspace/Aleph/src/memory/compression/service.rs#L393-L397)
- **Problem**:
  ```rust
  let digest: String = session_end_raws
      .iter()
      .map(|r| r.content.clone())
      .collect::<Vec<_>>()
      .join("\n");
  ```
  Clones every content string, creates a `Vec<String>`, and then joins them into another `String`.
- **Remediation**:
  ```rust
  let mut digest = String::new();
  for (i, r) in session_end_raws.iter().enumerate() {
      if i > 0 { digest.push('\n'); }
      digest.push_str(&r.content);
  }
  ```

---

### 2. Session Compactor Subsystem (`src/memory/session_compactor/`)

#### MCS-01: Premature Conversions Before Split Guard
- **File**: [`src/memory/session_compactor/post_turn_compress.rs#L103-L123`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/post_turn_compress.rs#L103-L123)
- **Problem**:
  ```rust
  let raw_messages = agent.get_history(session_key, None).await;
  let pairs: Vec<(String, String)> = raw_messages.iter().map(...).collect();
  let split = super::context_window::partition_fresh_tail_pairs(&pairs, self.config.fresh_tail_count);
  if split == 0 {
      return Ok(CompressResult::default());
  }
  ```
  `raw_messages` may have length <= `fresh_tail_count`. When it does, `pairs` cloned all message strings for nothing.
- **Remediation**: Check `raw_messages.len()` before allocating `pairs`:
  ```rust
  let raw_messages = agent.get_history(session_key, None).await;
  if raw_messages.len() <= self.config.fresh_tail_count {
      return Ok(CompressResult::default());
  }
  ```

#### MCS-02: Duplicate Chunk Formatting in post_turn_compress
- **File**: [`src/memory/session_compactor/post_turn_compress.rs#L175-L179`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/post_turn_compress.rs#L175-L179), [`#L276-L280`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/post_turn_compress.rs#L276-L280)
- **Problem**: Both `raw_text` and `raw_content` do:
  ```rust
  chunk.iter().map(|(role, text)| format!("[{role}]: {text}")).collect::<Vec<_>>().join("\n\n");
  ```
  This is executed twice per semantic chunk.
- **Remediation**: Compute `raw_content` once at the start of the chunk loop and pass to both pre-compress tasks and `store_raw_chunk`/`indexer`.

#### MCS-03: Shape 1 Triplication of `extract_depth`
- **File**: [`src/memory/session_compactor/mod.rs:154`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/mod.rs#L154), [`src/memory/session_compactor/summary_source.rs:137`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/summary_source.rs#L137), [`src/memory/session_search_summary/end_hook.rs:78`](file:///home/zou/data/workspace/Aleph/src/memory/session_search_summary/end_hook.rs#L78)
- **Problem**: Three independent implementations of depth parsing. `end_hook.rs` notes it copy-pasted `extract_depth` because `summary_source`'s helper was private. Moreover, `summary_source`'s implementation splits on `'/'` and checks `.strip_prefix('d')`, which mistakenly matches session IDs like `"d123"`!
- **Remediation**: Make `pub fn extract_depth(path: &str) -> u32` in `session_compactor::mod` and use it in both `summary_source.rs` and `end_hook.rs`.

#### MCS-04: Multibyte Boundary Overshoot in `deterministic_truncate`
- **File**: [`src/memory/session_compactor/fallback.rs#L39-L44`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/fallback.rs#L39-L44)
- **Problem**:
  ```rust
  let boundary = result
      .char_indices()
      .take_while(|(i, _)| *i <= max_chars)
      .last()
      .map_or(0, |(i, c)| i + c.len_utf8());
  ```
  If `i == max_chars`, `i + c.len_utf8()` exceeds `max_chars`. For multibyte characters, `boundary` can overshoot `max_chars` by up to 4 bytes.
- **Remediation**:
  Use `result.floor_char_boundary(max_chars)`.

#### MCS-05: Unnecessary `Arc::clone` for Reference Trait Arguments
- **File**: [`src/memory/session_compactor/helpers.rs#L154-L156`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/helpers.rs#L154-L156), [`#L258-L260`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/helpers.rs#L258-L260), [`src/memory/session_compactor/post_turn_compress.rs#L228-L230`](file:///home/zou/data/workspace/Aleph/src/memory/session_compactor/post_turn_compress.rs#L228-L230)
- **Problem**: Code explicitly disables rust-doctor with `// rust-doctor-disable-next-line excessive-clone`:
  ```rust
  let store: Arc<dyn RawMemoryStore> = self.database.clone();
  insert_with_runtime_capture_filter(&store, ...);
  ```
- **Remediation**: `&self.database` is already `&Arc<dyn RawMemoryStore>`. Remove the clone and the lint suppression.

---

### 3. Session Reflection Subsystem (`src/memory/session_reflection/`)

#### MCS-13: Full Lowercase Transcript Copy for Marker Search
- **File**: [`src/memory/session_reflection/mod.rs#L368-L381`](file:///home/zou/data/workspace/Aleph/src/memory/session_reflection/mod.rs#L368-L381)
- **Problem**:
  ```rust
  let lower = output.to_ascii_lowercase();
  let marker = OPEN_LOOPS_HEADER.to_ascii_lowercase();
  let (lessons_raw, loops_raw) = match lower.find(&marker) { ... }
  ```
  `output` contains full transcript reflections up to 32k chars. Allocating `output.to_ascii_lowercase()` copies the entire string just to find `"open_loops:"`.
- **Remediation**: Perform a case-insensitive window search over ASCII bytes:
  ```rust
  let marker = OPEN_LOOPS_HEADER.as_bytes();
  let idx = output.as_bytes().windows(marker.len()).position(|w| w.eq_ignore_ascii_case(marker));
  ```
  Zero allocations.

#### MCS-14: Double String Allocation in `strip_header` / `normalize_section`
- **File**: [`src/memory/session_reflection/mod.rs#L386-L392`](file:///home/zou/data/workspace/Aleph/src/memory/session_reflection/mod.rs#L386-L392)
- **Problem**: `strip_header(section: &str, header: &str) -> String` creates a new `String` which is then passed to `normalize_section(&strip_header(...))` which creates another `String`.
- **Remediation**: `strip_header` can return `&str`, slicing the original input zero-alloc.

---

### 4. Session Resume Subsystem (`src/memory/session_resume/`)

#### MCS-17: Snapshot Sweeper Reads and Deserializes Entire Payloads
- **File**: [`src/memory/session_resume/writer.rs#L179-L183`](file:///home/zou/data/workspace/Aleph/src/memory/session_resume/writer.rs#L179-L183)
- **Problem**:
  `cleanup_old_snapshots` reads and deserializes the full `SessionSnapshot` including potentially large `summary` fields solely to group by `(agent_id, scope_id)`.
- **Remediation**: Define an internal projection struct:
  ```rust
  #[derive(serde::Deserialize)]
  struct SnapshotOwner {
      #[serde(default)]
      agent_id: String,
      #[serde(default)]
      scope_id: Option<String>,
  }
  ```
  Serde skips the `summary` string without allocating heap memory for it.

---

### 5. Session Search Summary Subsystem (`src/memory/session_search_summary/`)

#### MCS-18: Unnecessary Content Clone in Highest-Depth Reuse
- **File**: [`src/memory/session_search_summary/end_hook.rs#L64-L69`](file:///home/zou/data/workspace/Aleph/src/memory/session_search_summary/end_hook.rs#L64-L69)
- **Problem**:
  ```rust
  Ok(best.map(|src| {
      RawMemory::new(src.content.clone(), RawMemorySource::SessionCompressed)
          ...
  }))
  ```
  `best` is an `Option<RawMemory>` from `into_iter()`. `src` is owned; `src.content.clone()` is redundant.
- **Remediation**:
  `RawMemory::new(src.content, RawMemorySource::SessionCompressed)` (move instead of clone).

---

## Architectural Redline Check

- **R1 (Brain-Limb Separation)**: All 26 files operate strictly within memory core models and store traits. No direct OS / platform UI calls.
- **R7 (LLM Sovereignty)**: No deterministic keyword detection reintroduced; content-blind cadence strictly preserved.
- **R8 (Tools as Capabilities)**: All operations expose clear functional interfaces; state persisted to Markdown/SQLite stores.
- **R10 (Thin Harness, Lean Loops)**: Multiple opportunities to prune redundant allocations, simplify repeated prompt formatting, and eliminate duplicate helper functions.

---

## Route Summary & Recommendations

| Category | FIX-NOW | FIX-CAREFULLY | DEFER | REJECT | Total |
|---|:---:|:---:|:---:|:---:|:---:|
| **DEAD-CODE / ACCIDENTAL-COMPLEXITY** | 1 | 1 | 1 | 0 | 3 |
| **REDUNDANT-ALLOCATION** | 11 | 2 | 0 | 0 | 13 |
| **CONTROL-FLOW / INDIRECTION** | 2 | 0 | 0 | 0 | 2 |
| **CORRECTNESS / INVARIANT-TIGHTENING** | 1 | 0 | 0 | 0 | 1 |
| **PANIC-RISK / DEFENSIVE-CHECK** | 0 | 0 | 0 | 0 | 0 |
| **Total** | **15** | **3** | **1** | **0** | **19** |

- **Recommended Immediate Actions (`FIX-NOW`)**:
  - Consolidate triplicated `extract_depth` (MCS-03)
  - Prevent overshoot in `deterministic_truncate` (MCS-04)
  - Fix early return before `pairs` allocation in `post_turn_compress` (MCS-01)
  - Deduplicate chunk formatting in `post_turn_compress` (MCS-02)
  - Remove unnecessary `Arc::clone` in helpers and post_turn_compress (MCS-05)
  - Change `ingest_group_key` to return `&'static str` (MCS-06)
  - Eliminate temporary `Vec<String>` and content clones in session_end digest and summary_source (MCS-07, MCS-12)
  - Optimize case-insensitive search and `strip_header` in session_reflection (MCS-13, MCS-14)
  - Optimize `cleanup_old_snapshots` deserialization in writer.rs (MCS-17)
  - Move `src.content` without clone in end_hook (MCS-18)
