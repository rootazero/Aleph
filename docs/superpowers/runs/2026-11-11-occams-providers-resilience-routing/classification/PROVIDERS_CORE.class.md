# Occam's Razor Classification Report: Batch `PROVIDERS_CORE`

- **Batch**: `PROVIDERS_CORE`
- **Subsystem**: `src/providers`
- **Review Doctrine**: `/rust-occams-razor`
- **Model Tier**: Jev Tier (Classification, Filtering, Routing)
- **Base Worktree**: `/home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing`
- **Date**: 2026-11-11

---

## 1. Executive Summary

This report covers 30 core files across `src/providers` totaling 12,800 lines of Rust. The files encompass provider adapters, HTTP provider implementation, delta streaming collectors, provider registry, retry logic, route witness diagnostics, capability gating, token metering, and session model routing.

### Candidate Counts by Taxonomy

| Taxonomy Category | Count | Description |
|-------------------|-------|-------------|
| `REDUNDANT-ALLOCATION` | 11 | Unnecessary `.to_string()`, `.clone()`, intermediate `Vec` allocations, and repeated lowercasing |
| `CONTROL-FLOW` / `INDIRECTION` | 5 | Over-nested `if let`, redundant lookups before checks, and non-short-circuiting predicates |
| `DEAD-CODE` / `ACCIDENTAL-COMPLEXITY` | 4 | Duplicated preamble/setup closures, non-unstable sorting on unique keys, and redundant helpers |
| `PANIC-RISK` / `DEFENSIVE-CHECK` | 1 | Token arithmetic handling in metering breakdown conversion |
| `CORRECTNESS` / `INVARIANT-TIGHTENING` | 2 | Premature clone before validation/check, and missing model hint propagation |
| **Total Findings** | **23** | |

### Candidate Counts by Routing Action

| Action | Count | Summary |
|--------|-------|---------|
| `FIX-NOW` | 13 | High-confidence, zero-risk simplifications (e.g. entry API, stack needle, avoid intermediate vectors) |
| `FIX-CAREFULLY` | 5 | Invariant-preserving refactorings requiring cross-call verification (e.g. `outbound_messages`, chars vector) |
| `DEFER` | 2 | Broad architectural considerations (e.g. cross-module `to_lowercase` caching, Ollama safety parity) |
| `REJECT` | 3 | Intentional design patterns / false positives (e.g. leaf census overrides, R7 fail-open recovery, dynamic trait objects) |
| **Total** | **23** | |

---

## 2. Detailed Findings by File

### 1. `src/providers/registry.rs`

#### Finding PC-01: `name.clone()` before `DashMap::entry` in `ProviderRegistry::register`
- **File & Line**: [`src/providers/registry.rs:94-105`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/providers/registry.rs#L94-L105)
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Action**: `FIX-NOW`
- **Issue**:
  ```rust
  pub fn register(&self, name: String, provider: Arc<dyn AiProvider>) -> Result<()> {
      use dashmap::mapref::entry::Entry;
      match self.providers.entry(name.clone()) {
          Entry::Vacant(slot) => {
              slot.insert(provider);
              Ok(())
          }
          Entry::Occupied(_) => Err(AlephError::invalid_config(format!(
              "Provider '{name}' is already registered"
          ))),
      }
  }
  ```
  `name` is passed by value (`name: String`). It is cloned unconditionally upfront on every registration (`name.clone()`) merely to reference it in the error message if `Occupied`. However, `Entry::Occupied(slot)` provides access to `slot.key()`, which borrows the registered string without cloning upfront!
- **Occam's Fix**:
  ```rust
  pub fn register(&self, name: String, provider: Arc<dyn AiProvider>) -> Result<()> {
      use dashmap::mapref::entry::Entry;
      match self.providers.entry(name) {
          Entry::Vacant(slot) => {
              slot.insert(provider);
              Ok(())
          }
          Entry::Occupied(slot) => Err(AlephError::invalid_config(format!(
              "Provider '{}' is already registered",
              slot.key()
          ))),
      }
  }
  ```

#### Finding PC-02: `names.sort()` on `Vec<String>` in `ProviderRegistry::names`
- **File & Line**: [`src/providers/registry.rs:163-167`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/providers/registry.rs#L163-L167)
- **Taxonomy**: `ACCIDENTAL-COMPLEXITY`
- **Action**: `FIX-NOW`
- **Issue**:
  `names.sort()` performs a stable sort. Since all provider names in the registry are unique (enforced by `register`), stability guarantees are unused and allocate temporary scratch space. `names.sort_unstable()` is faster and zero-overhead.

---

### 2. `src/providers/llm_retry.rs`

#### Finding PC-03: `code.to_string()` heap allocation in `has_status_code`
- **File & Line**: [`src/providers/llm_retry.rs:51-53`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/providers/llm_retry.rs#L51-L53)
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Action**: `FIX-NOW`
- **Issue**:
  ```rust
  pub fn has_status_code(msg: &str, code: u16) -> bool {
      let needle = code.to_string();
      let bytes = msg.as_bytes();
  ```
  `has_status_code` is invoked repeatedly across error classification routines (for codes 400, 401, 403, 404, 413, 429, 500, 502, 503, 504, 529). Formatting a 3-digit `u16` allocates a 24-byte `String` on the heap on every single call.
- **Occam's Fix**:
  Format `code` into a small stack buffer `[u8; 5]` without heap allocation:
  ```rust
  pub fn has_status_code(msg: &str, code: u16) -> bool {
      let mut buf = [0u8; 5];
      let mut n = code;
      let mut len = 0;
      while n > 0 {
          buf[len] = b'0' + (n % 10) as u8;
          n /= 10;
          len += 1;
      }
      buf[..len].reverse();
      let needle = match std::str::from_utf8(&buf[..len]) {
          Ok(s) => s,
          Err(_) => return false,
      };
      // ... search needle in msg ...
  ```

#### Finding PC-04: Intermediate `String` collection in `parse_token_gap_str`
- **File & Line**: [`src/providers/llm_retry.rs:98-105`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/providers/llm_retry.rs#L98-L105)
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Action**: `FIX-NOW`
- **Issue**:
  ```rust
  let limit_str: String = after_gt
      .chars()
      .take_while(|c| c.is_ascii_digit())
      .collect();
  if limit_str.is_empty() {
      return None;
  }
  let limit: usize = limit_str.parse().ok()?;
  ```
  Collecting chars into a `String` just to call `.parse::<usize>()` allocates unnecessary heap memory. A subslice `&after_gt[..digit_count]` can be parsed directly without allocation.

#### Finding PC-05: Intermediate `String` collection in `extract_retry_after_str`
- **File & Line**: [`src/providers/llm_retry.rs:227-232`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/providers/llm_retry.rs#L227-L232)
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Action**: `FIX-NOW`
- **Issue**:
  ```rust
  let num_str: String = msg
      .get(start..)?
      .chars()
      .take_while(|c| c.is_ascii_digit())
      .collect();
  let secs: u64 = num_str.parse().ok()?;
  ```
  Identical pattern to PC-04: allocating an intermediate `String` to parse a numeric delay. Subslice parsing `msg.get(start..)?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?` is zero-allocation.

#### Finding PC-06: Repeated `raw.to_lowercase()` across classification cascading
- **File & Line**: [`src/providers/llm_retry.rs:128, 310, 364, 433`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/providers/llm_retry.rs#L128)
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Action**: `FIX-CAREFULLY`
- **Issue**:
  `classify_exhausted` calls `classify(raw)` (which allocates `msg = raw.to_lowercase()`), then immediately calls `let msg = raw.to_lowercase();` again, and if `is_rate_limit_text(&msg)` is true, delegates to `classify_rate_limit(raw)` which calls `raw.to_lowercase()` a third time.
- **Occam's Fix**:
  Provide internal helper `classify_lower(raw: &str, msg_lower: &str)` and `classify_rate_limit_lower(raw: &str, msg_lower: &str)` so the lowercased string is allocated at most once per error chain.

---

### 3. `src/providers/route_witness.rs`

#### Finding PC-07: Inverted condition order in `BoundedWitnesses::record` causing unconditional hash lookups
- **File & Line**: [`src/providers/route_witness.rs:328`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/providers/route_witness.rs#L328)
- **Taxonomy**: `CONTROL-FLOW` / `ACCIDENTAL-COMPLEXITY`
- **Action**: `FIX-NOW`
- **Issue**:
  ```rust
  if !self.map.contains_key(&key) && self.map.len() >= MAX_TRACKED_SESSIONS {
  ```
  Because `!self.map.contains_key(&key)` is on the left side of `&&`, it computes the hash and traverses the bucket table on **every single record**, even when `self.map.len()` is well below `MAX_TRACKED_SESSIONS` (which is true in 99.9% of calls).
- **Occam's Fix**:
  Invert to:
  ```rust
  if self.map.len() >= MAX_TRACKED_SESSIONS && !self.map.contains_key(&key) {
  ```
  When `len < 256`, the `contains_key` hash calculation is completely skipped.

#### Finding PC-08: Redundant hash lookups in `BoundedWitnesses::record`
- **File & Line**: [`src/providers/route_witness.rs:342-357`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/providers/route_witness.rs#L342-L357)
- **Taxonomy**: `CONTROL-FLOW` / `ACCIDENTAL-COMPLEXITY`
- **Action**: `FIX-NOW`
- **Issue**:
  ```rust
  match self.map.get_mut(&key) {
      Some((t, witness)) => { ... }
      None => {
          ...
          self.map.insert(key, (tick, RouteWitness { first, served }));
      }
  }
  ```
  Using `self.map.get_mut(&key)` followed by `self.map.insert(key, ...)` performs two consecutive hash operations for newly recorded sessions.
- **Occam's Fix**:
  Use `self.map.entry(key)` with `Entry::Occupied` and `Entry::Vacant` for single-lookup insertion.

---

### 4. `src/providers/session_model_handle.rs`

#### Finding PC-09: Redundant map lookup in `update_session_model`
- **File & Line**: [`src/providers/session_model_handle.rs:204-219`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/providers/session_model_handle.rs#L204-L219)
- **Taxonomy**: `CONTROL-FLOW` / `ACCIDENTAL-COMPLEXITY`
- **Action**: `FIX-NOW`
- **Issue**:
  ```rust
  if let Some(existing) = guard.get(session_key) {
      if existing.generation == u64::MAX {
          ...
          return ...;
      }
  }
  let generation = guard.get(session_key).map_or(0, |state| state.generation) + 1;
  ```
  `guard.get(session_key)` is called twice under the write lock: once for the terminal generation check, and once to compute `generation`.
- **Occam's Fix**:
  Collapse to:
  ```rust
  let generation = match guard.get(session_key) {
      Some(existing) if existing.generation == u64::MAX => {
          warn!(session_key, "refusing session model update because generation reached u64::MAX");
          return SessionModelUpdate {
              pref: existing.pref.clone(),
              generation: u64::MAX,
              accepted: false,
              durable: false,
          };
      }
      Some(existing) => existing.generation + 1,
      None => 1,
  };
  ```

#### Finding PC-10: Eager clone in `update_session_model_durable` before terminal check
- **File & Line**: [`src/providers/session_model_handle.rs:257-268`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/providers/session_model_handle.rs#L257-L268)
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Action**: `FIX-NOW`
- **Issue**:
  `let previous = guard.get(session_key).cloned();` clones the entire `SessionModelState` (including `String`s) before checking if `state.generation == u64::MAX`. If it is terminal, the clone is immediately discarded. Check generation by reference before cloning.

---

### 5. `src/providers/catalog.rs`

#### Finding PC-11: Unnecessary `out = Vec::new()` intermediate vector in `presets_for_modality`
- **File & Line**: [`src/providers/catalog.rs:98-126`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/providers/catalog.rs#L98-L126)
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Action**: `FIX-NOW`
- **Issue**:
  ```rust
  pub fn presets_for_modality(modality: Modality) -> Vec<CatalogEntry> {
      let mut out = Vec::new();
      let mut chat: Vec<CatalogEntry> = ...;
      chat.sort_unstable_by_key(|e| e.name);
      out.append(&mut chat);

      let mut gen: Vec<CatalogEntry> = ...;
      gen.sort_unstable_by_key(|e| e.name);
      out.append(&mut gen);
      out
  }
  ```
  `all_presets()` in the same file directly does `chat.extend(gen); chat`. Here, allocating empty `out`, appending `chat`, then appending `gen` allocates an extra `Vec` header and performs redundant vector drains.
- **Occam's Fix**:
  Directly do `chat.extend(gen); chat` matching `all_presets()`.

---

### 6. `src/providers/health.rs`

#### Finding PC-12: `message.to_lowercase()` heap allocation in 404 error classification
- **File & Line**: [`src/providers/health.rs:110-112`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/providers/health.rs#L110-L112)
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Action**: `FIX-NOW`
- **Issue**:
  ```rust
  if crate::providers::llm_retry::has_status_code(message, 404)
      && message.to_lowercase().contains("model")
  ```
  Allocates a brand-new `String` to search for the ASCII word `"model"`.
- **Occam's Fix**:
  Use `message.to_ascii_lowercase().contains("model")` or a case-insensitive word/subslice match without full unicode case folding.

---

### 7. `src/providers/probe.rs`

#### Finding PC-13: Duplicated latency calculation in `probe_provider`
- **File & Line**: [`src/providers/probe.rs:135, 140`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/providers/probe.rs#L135)
- **Taxonomy**: `ACCIDENTAL-COMPLEXITY`
- **Action**: `FIX-NOW`
- **Issue**:
  `latency_ms: Some(u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX))` is duplicated verbatim in both `Ok(_)` and `Err(e)` branches.
- **Occam's Fix**:
  Calculate `let latency_ms = Some(u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX));` once immediately following the `.await`.

---

### 8. `src/providers/http_provider.rs`

#### Finding PC-14: Redundant message vector cloning in `outbound_messages` -> `apply_outbound_safety`
- **File & Line**: [`src/providers/http_provider.rs:344-369`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/providers/http_provider.rs#L344-L369)
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Action**: `FIX-CAREFULLY`
- **Issue**:
  In `outbound_messages`:
  ```rust
  let outbound = transform_messages(
      payload.messages,
      &self.reasoning_replay(payload.model.as_deref()),
  );
  self.apply_outbound_safety(&outbound, platform)
  ```
  `transform_messages` creates and returns an owned `Vec<UnifiedMessage>`. But `apply_outbound_safety` accepts `messages: &[UnifiedMessage]` and immediately does:
  ```rust
  let mut filtered_messages: Vec<UnifiedMessage> = messages.to_vec();
  ```
  This performs a second full deep-clone of all messages and their content blocks!
- **Occam's Fix**:
  Refactor `apply_outbound_safety` to take `mut messages: Vec<UnifiedMessage>`. `outbound_messages` passes `outbound` by value, completely eliminating the second vector allocation and all content block clones. For `stream_raw`, pass `payload.messages.to_vec()`.

#### Finding PC-15: Duplicated `RequestPayload` reconstruction between `execute_once` and `stream_raw`
- **File & Line**: [`src/providers/http_provider.rs:476-490, 720-734`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/providers/http_provider.rs#L476-L490)
- **Taxonomy**: `ACCIDENTAL-COMPLEXITY`
- **Action**: `FIX-NOW`
- **Issue**:
  The 15-line reconstruction of `RequestPayload` with overridden `messages` is copied byte-for-byte in both `execute_once` and `stream_raw`.
- **Occam's Fix**:
  Provide a helper `payload.with_messages(messages: &'a [UnifiedMessage]) -> RequestPayload<'a>`.

---

### 9. `src/providers/delta.rs`

#### Finding PC-16: Intermediate `Vec<&str>` allocation in `parse_json_tool_call`
- **File & Line**: [`src/providers/delta.rs:290-296`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/providers/delta.rs#L290-L296)
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Action**: `FIX-NOW`
- **Issue**:
  `trimmed.lines().skip(1).take_while(|l| !l.trim_start().starts_with("```")).collect::<Vec<_>>().join("\n")`
  Collects line slices into a temporary `Vec<&str>` just to join with newline.
- **Occam's Fix**:
  Use `itertools::join` or string buffer fold to avoid intermediate `Vec` allocation.

#### Finding PC-17: Massive `Vec<char>` allocation in `repair_json_emission_defects`
- **File & Line**: [`src/providers/delta.rs:372`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/providers/delta.rs#L372)
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Action**: `FIX-CAREFULLY`
- **Issue**:
  `let chars: Vec<char> = raw.chars().collect();`
  Allocates a `Vec<char>` of 4 bytes per character for the entire tool argument payload. For large tool outputs (e.g. file contents or data tables of several megabytes), this consumes multiple megabytes of heap memory just to check for trailing commas or unescaped control chars.
- **Occam's Fix**:
  Scan `raw` using `char_indices` / iterator or index over UTF-8 bytes where ASCII checks apply.

#### Finding PC-18: `sort_by_key` on unique numeric keys in `IndexIdTracker::ids_in_order`
- **File & Line**: [`src/providers/delta.rs:514`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/providers/delta.rs#L514)
- **Taxonomy**: `ACCIDENTAL-COMPLEXITY`
- **Action**: `FIX-NOW`
- **Issue**:
  `entries.sort_by_key(|(index, _)| **index);` uses stable sort for unique stream indices. `sort_unstable_by_key` is faster and allocates no scratch buffer.

---

### 10. `src/providers/ollama.rs`

#### Finding PC-19: Repeated `.collect::<Vec<_>>().join("\n")` in `convert_messages`
- **File & Line**: [`src/providers/ollama.rs:236-240, 257-263, 284-288`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/providers/ollama.rs#L236-L240)
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Action**: `FIX-NOW`
- **Issue**:
  In `convert_messages`, user text, assistant text, and tool text extraction each run `.filter_map(...).collect::<Vec<_>>().join("\n")`, creating three intermediate vectors per turn.
- **Occam's Fix**:
  Join iterators directly using fold or `itertools::join`.

---

### 11. `src/providers/metering.rs`

#### Finding PC-20: Duplicated preparation preamble between `process` and `execute_streaming_dyn`
- **File & Line**: [`src/providers/metering.rs:342-356, 384-399`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/providers/metering.rs#L342-L356)
- **Taxonomy**: `ACCIDENTAL-COMPLEXITY`
- **Action**: `FIX-NOW`
- **Issue**:
  The 15-line resolution of `scope`, `prefix_hash`, `sink`, `agent_id`, `principal`, `provider_name`, `pricing_provider`, and `pricing_model` is copied verbatim across both arms.
- **Occam's Fix**:
  Extract a private helper `fn prepare_metering(&self, req: &RequestPayload<'_>) -> MeteringCallContext`.

#### Finding PC-21: Token arithmetic overflow defense in `TokenBreakdown`
- **File & Line**: [`src/providers/metering.rs:274-275`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/providers/metering.rs#L274-L275)
- **Taxonomy**: `PANIC-RISK` / `DEFENSIVE-CHECK`
- **Action**: `FIX-CAREFULLY`
- **Issue**:
  Converting `TokenUsage` (`u32` fields) to `TokenBreakdown` and computing `breakdown.total()` must guarantee saturating addition so huge/malformed provider token usage numbers cannot trigger debug-assertion panics in arithmetic.

---

### 12. `src/providers/capability_gate.rs`

#### Finding PC-22: `m.text_content()` string allocation in character estimation
- **File & Line**: [`src/providers/capability_gate.rs:62-65`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/providers/capability_gate.rs#L62-L65)
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Action**: `FIX-CAREFULLY`
- **Issue**:
  ```rust
  let chars: usize = messages
      .iter()
      .map(|m| m.text_content().chars().count())
      .sum();
  ```
  `m.text_content()` allocates a heap `String` for every message in the conversation history solely to count its characters.
- **Occam's Fix**:
  Count characters directly across text blocks:
  `m.content_blocks().filter_map(ContentBlock::as_text).map(|t| t.chars().count()).sum()`
  without allocating intermediate `String`s.

---

### 13. `src/providers/session_model_handle/selectable.rs`

#### Finding PC-23: Over-nested `if` statement
- **File & Line**: [`src/providers/session_model_handle/selectable.rs:91-95`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/providers/session_model_handle/selectable.rs#L91-L95)
- **Taxonomy**: `CONTROL-FLOW`
- **Action**: `FIX-NOW`
- **Issue**:
  ```rust
  if base.is_empty() {
      if let Some(preset) = &preset {
          base.push(preset.default_model.clone());
      }
  }
  ```
- **Occam's Fix**:
  Flatten: `if base.is_empty() && let Some(preset) = &preset` (or `if let (true, Some(preset)) = (base.is_empty(), &preset)` / `if base.is_empty() { if let ... }` simplified with helper).

---

## 3. Rejected Candidates (False Positives & Intentional Invariants)

| Candidate | Location | Reason for Rejection |
|-----------|----------|----------------------|
| **R-01: Dynamic Dispatch (`Arc<dyn AiProvider>`)** | `src/providers/{default_handle, mod, model_override_provider, session_model_provider}.rs` | **Intentional Design Contract**. Dynamic dispatch enables the runtime decorator stack (`Failover` -> `Metering` -> `ThinkLevel` -> `ModelOverride` -> `HttpProvider`) without monomorphizing the harness across combinatorial decorator permutations. Complies with R10 thin-harness. |
| **R-02: Dropped candidate restoration in Capability Gate** | `src/providers/capability_gate.rs:136-143` | **Intentional Invariant (R7 LLM Sovereignty)**. Fail-open behavior: if filtering drops every candidate, restoring all original candidates prevents infrastructure tables from hard-bricking a request. Collecting `dropped` is essential for fail-open restoration. |
| **R-03: `OllamaProvider` leaf designation in `forwarding_census.rs`** | `src/providers/forwarding_census.rs:22-27` | **Intentional Architecture Boundary**. Ollama is a standalone terminal leaf provider that implements native chat API without wrapping another provider. Exemption from delegation in the forwarding census is verified by design. |

---

## 4. Smell Routing Summary

### `FIX-NOW` (13 Items)

1. `src/providers/registry.rs:96` — Remove `name.clone()` before `DashMap::entry`; use `slot.key()` in `Entry::Occupied`.
2. `src/providers/registry.rs:165` — Use `names.sort_unstable()` on unique registry keys.
3. `src/providers/llm_retry.rs:52` — Replace `code.to_string()` in `has_status_code` with zero-allocation stack buffer `[u8; 5]`.
4. `src/providers/llm_retry.rs:98-105` — Subslice parsing in `parse_token_gap_str` without intermediate `String`.
5. `src/providers/llm_retry.rs:227-232` — Subslice parsing in `extract_retry_after_str` without intermediate `String`.
6. `src/providers/route_witness.rs:328` — Swap order to `self.map.len() >= MAX_TRACKED_SESSIONS && !self.map.contains_key(&key)` to eliminate hash checks under cap.
7. `src/providers/route_witness.rs:342-357` — Collapse `get_mut` followed by `insert` into `self.map.entry(key)`.
8. `src/providers/session_model_handle.rs:204-219` — Consolidate duplicate `guard.get(session_key)` lookups in `update_session_model`.
9. `src/providers/session_model_handle.rs:257-268` — Avoid cloning `previous` upfront before `u64::MAX` check in `update_session_model_durable`.
10. `src/providers/catalog.rs:111-125` — Eliminate intermediate `out = Vec::new()` allocation in `presets_for_modality`.
11. `src/providers/health.rs:111` — Replace `message.to_lowercase().contains("model")` with `to_ascii_lowercase` or case-insensitive word match.
12. `src/providers/probe.rs:135` — Deduplicate `latency_ms` calculation in `probe_provider`.
13. `src/providers/delta.rs:514` — Use `sort_unstable_by_key` in `IndexIdTracker::ids_in_order`.

### `FIX-CAREFULLY` (5 Items)

1. `src/providers/http_provider.rs:369` — Avoid cloning message vector in `apply_outbound_safety` by taking `messages: Vec<UnifiedMessage>`.
2. `src/providers/delta.rs:372` — Replace `let chars: Vec<char> = raw.chars().collect()` in `repair_json_emission_defects` with string slice iterator.
3. `src/providers/llm_retry.rs:128, 433` — Cache lowercased error message across cascading classification helpers.
4. `src/providers/capability_gate.rs:62-65` — Avoid `m.text_content()` allocations in character count summation by iterating blocks directly.
5. `src/providers/metering.rs:274` — Confirm saturating addition on `TokenBreakdown` conversions.

### `DEFER` (2 Items)

1. `src/providers/metering.rs:342-416` — Extract `prepare_metering` context helper across `process` and `execute_streaming_dyn`.
2. `src/providers/ollama.rs:194` — Unify Ollama provider safety pipeline with `HttpProvider` safety filters (architectural feature task).

---

## 5. Invariant & Safety Checklist

- [x] **R1 (Brain & Limb separation)**: No platform APIs introduced in core providers.
- [x] **R7 (LLM Sovereignty)**: Capability gating remains strictly structural (image block, tools flag, context window size); no prompt semantics classification.
- [x] **R8 (Tool Calling Integrity)**: Nonce format for synthetic IDs (`ollama_call_{i}_{nonce}`, `json_{name}_{nonce}`) preserved verbatim to avoid tool-use collision bugs.
- [x] **R10 (Thin Harness Ratchet)**: Zero changes to `src/harness/`; all optimizations localized to `src/providers/`.
- [x] **Public API & Protocols**: Wire formats (SSE streaming, Anthropic/OpenAI/Gemini payloads) remain 100% byte-compatible.
- [x] **Concurrency & Mutex Safety**: Lock ordering and DashMap/RwLock shard boundaries remain unchanged.
