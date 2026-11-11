# Classification & Routing Matrix: MEM_ROOT_EXT

- **Batch ID**: `MEM_ROOT_EXT`
- **Subsystem**: `src/memory` (Root interfaces, embedding manager/resolver, extensions registry/scheduler, flush registry & scrubber)
- **Files Inspected (29 files, 8,766 lines)**:
  - `src/memory/content_scanner.rs`
  - `src/memory/embedding_manager.rs`
  - `src/memory/embedding_provider.rs`
  - `src/memory/embedding_resolver.rs`
  - `src/memory/embedding_signature.rs`
  - `src/memory/explain.rs`
  - `src/memory/insights.rs`
  - `src/memory/mod.rs`
  - `src/memory/namespace.rs`
  - `src/memory/project_scope.rs`
  - `src/memory/proptest_enums.rs`
  - `src/memory/reembed.rs`
  - `src/memory/session_memory_mode.rs`
  - `src/memory/streaming_scrubber.rs`
  - `src/memory/tool_signal_sink.rs`
  - `src/memory/extensions/capture_lifecycle_tests.rs`
  - `src/memory/extensions/delegation_lifecycle_tests.rs`
  - `src/memory/extensions/first_party.rs`
  - `src/memory/extensions/insert_helper.rs`
  - `src/memory/extensions/manifest.rs`
  - `src/memory/extensions/mcp_adapter.rs`
  - `src/memory/extensions/mod.rs`
  - `src/memory/extensions/registry.rs`
  - `src/memory/extensions/scheduler.rs`
  - `src/memory/extensions/scheduler_lifecycle_tests.rs`
  - `src/memory/extensions/traits.rs`
  - `src/memory/extensions/types.rs`
  - `src/memory/flush/mod.rs`
  - `src/memory/flush/registry.rs`

---

## 1. Executive Summary

This review applies the **Occam's Razor doctrine** to `MEM_ROOT_EXT`: eliminating accidental complexity, dead imports, redundant heap allocations (repeated `format!`, redundant string/vec cloning, unneeded `.to_string()`), over-nested control flow, and token budget desynchronization, while strictly preserving:
1. **Security & Content Integrity**: Content scanning against prompt injection, invisible Unicode, and data exfiltration patterns;
2. **Vector Space Invariants**: Embedding provider contracts, dimension matching, and provider signature drift detection;
3. **Multi-Partition Scoping Boundaries**: Sibling partition isolation (`proj-*`, `u-*`, `p-*`), user profile floor rules, and personal-scope ownership;
4. **Streaming Framing Scrubber**: Zero leakage of raw `<memory>` tags or chain-of-thought blocks across chunk boundaries;
5. **Extension Pipeline Semantics**: Timeout bounding, fail-closed capture filters, and ordered MCP hook dispatch;
6. **Flush Concurrency & Atomicity**: Multi-session in-flight flush tracking and full partition backlog draining.

---

## 2. Classification & Routing Matrix

| ID | Location | Taxonomy Category | Smell / Anti-Pattern Description | Route Tag | Proposed Action |
|---|---|---|---|---|---|
| **MEM-SCAN-01** | [`src/memory/content_scanner.rs:103`](file:///home/zou/data/workspace/Aleph/src/memory/content_scanner.rs#L103) | `REDUNDANT-ALLOCATION` | `normalize_homoglyphs(content)` is called unconditionally, allocating a new `String` for every fact scanned even if `content.is_ascii()` (homoglyphs only exist in non-ASCII scripts). | `FIX-NOW` | Fast-path `if content.is_ascii() { content } else { ... }` or borrow directly to avoid heap allocation on clean ASCII text. |
| **MEM-SCAN-02** | [`src/memory/content_scanner.rs:107-163`](file:///home/zou/data/workspace/Aleph/src/memory/content_scanner.rs#L107-L163) | `CONTROL-FLOW` / `INDIRECTION` | 8 distinct threat regexes evaluated sequentially. Could theoretically compile into a `RegexSet`. | `DEFER` | Individual checks are fast on short text, provide distinct error codes/reasons, and keep threat rules isolated. |
| **MEM-EMGR-01** | [`src/memory/embedding_manager.rs:58-60`](file:///home/zou/data/workspace/Aleph/src/memory/embedding_manager.rs#L58-L60) | `REDUNDANT-ALLOCATION` | In `init()`, clones `decision.effective.map(|c| c.id.clone())` while simultaneously doing `decision.effective.cloned()`. | `FIX-NOW` | Clone the `Option<EmbeddingProviderConfig>` once and derive `effective_id` from the cloned config's `id`. |
| **MEM-EMGR-02** | [`src/memory/embedding_manager.rs:131-132`](file:///home/zou/data/workspace/Aleph/src/memory/embedding_manager.rs#L131-L132) | `CONTROL-FLOW` / `REDUNDANT-ALLOCATION` | In `switch_provider()`, read lock is acquired to clone `active_provider_id`, dropped, then write lock acquired. TOCTOU lock gap and extra string clone. | `FIX-NOW` | Acquire write lock once and use `std::mem::replace(&mut self.settings.write().await.active_provider_id, new_id.to_string())` to capture `old_id` without cloning. |
| **MEM-EMGR-03** | [`src/memory/embedding_manager.rs:225-227`](file:///home/zou/data/workspace/Aleph/src/memory/embedding_manager.rs#L225-L227) | `REDUNDANT-ALLOCATION` | In `flush_pending()`, requeuing does `let mut combined = drained; combined.append(&mut *q); *q = combined;`, allocating and copying the entire queue vector. | `FIX-NOW` | Use `q.splice(0..0, drained);` to prepend in-place without allocating a temporary vector. |
| **MEM-EPROV-01** | [`src/memory/embedding_provider.rs:185-198`](file:///home/zou/data/workspace/Aleph/src/memory/embedding_provider.rs#L185-L198) | `REDUNDANT-ALLOCATION` | `truncate_input` calls `text.chars().count()` up to 4 times, and creates an owned `String` via `text.chars().take(...).collect()`, allocating heap memory for what is strictly a string prefix slice. | `FIX-CAREFULLY` | Check byte length first; find char boundary via `char_indices().nth(...)` and subslice `&text[..boundary]`. Update test asserting on `Cow::Owned`. |
| **MEM-EPROV-02** | [`src/memory/embedding_provider.rs:216-222`](file:///home/zou/data/workspace/Aleph/src/memory/embedding_provider.rs#L216-L222) | `REDUNDANT-ALLOCATION` | In `call_api()`, repeatedly normalizes `api_base` and formats `format!("{normalized}/embeddings")` on every single batch request. | `FIX-NOW` | Format URL directly without intermediate string `normalized`, or cache endpoint URL in `RemoteEmbeddingProvider`. |
| **MEM-EPROV-03** | [`src/memory/embedding_provider.rs:228-230`](file:///home/zou/data/workspace/Aleph/src/memory/embedding_provider.rs#L228-L230) | `REDUNDANT-ALLOCATION` | Two consecutive `Vec` allocations in `call_api()`: `truncated: Vec<Cow<str>>` followed immediately by `payload: Vec<&str>`. | `FIX-NOW` | Flatten into a single `Vec<&str>` mapped directly from `texts`. |
| **MEM-EPROV-04** | [`src/memory/embedding_provider.rs:266`](file:///home/zou/data/workspace/Aleph/src/memory/embedding_provider.rs#L266) | `REDUNDANT-ALLOCATION` | `response.headers()...unwrap_or("").to_string()` allocates an owned `String` merely to check `!content_type.contains("json")`. | `FIX-NOW` | Borrow `content_type` as `&str` (`.unwrap_or("")`). |
| **MEM-EPROV-05** | [`src/memory/embedding_provider.rs:351`](file:///home/zou/data/workspace/Aleph/src/memory/embedding_provider.rs#L351) | `REDUNDANT-ALLOCATION` | `let chunks: Vec<&[&str]> = texts.chunks(batch_size).collect();` allocates a temporary vector solely to call `.chunks(MAX_CONCURRENT_BATCHES)`. | `FIX-NOW` | Chunk directly without intermediate allocation. |
| **MEM-ERES-01** | [`src/memory/embedding_resolver.rs:108`](file:///home/zou/data/workspace/Aleph/src/memory/embedding_resolver.rs#L108) | `REDUNDANT-ALLOCATION` | `let requested = settings.active_provider_id.clone();` allocates an owned `String` into `EmbeddingDecision.requested_id`, which is never even read by callers. | `FIX-NOW` | Change `requested_id` to borrow `&'a str` (or `Cow<'a, str>`), making `resolve()` 100% zero-allocation. |
| **MEM-ESIG-01** | [`src/memory/embedding_signature.rs:20-57`](file:///home/zou/data/workspace/Aleph/src/memory/embedding_signature.rs#L20-L57) | `ACCIDENTAL-COMPLEXITY` | Clean, minimal 107-line module. | `REJECT` | Code is idiomatic and correctly implements signature checking. |
| **MEM-EXPL-01** | [`src/memory/explain.rs:22-51`](file:///home/zou/data/workspace/Aleph/src/memory/explain.rs#L22-L51) | `ACCIDENTAL-COMPLEXITY` | Public read-model serialization structs (`FactExplanation`, `ExplainedEvent`). | `REJECT` | Pure DTOs, no extraneous logic. |
| **MEM-INS-01** | [`src/memory/insights.rs:308`](file:///home/zou/data/workspace/Aleph/src/memory/insights.rs#L308) | `REDUNDANT-ALLOCATION` | `samples.get(*tool).cloned().unwrap_or_default()` clones the entire `Vec<String>` of evidence samples per tool, even though `samples` is a local map that is dropped right after. | `FIX-NOW` | Use `samples.remove(*tool).unwrap_or_default()` to move the vector without any cloning. |
| **MEM-INS-02** | [`src/memory/insights.rs:289-291`](file:///home/zou/data/workspace/Aleph/src/memory/insights.rs#L289-L291) | `REDUNDANT-ALLOCATION` | `let body: String = r.content.chars().take(FAILURE_SAMPLE_MAX_CHARS).collect();` allocates an owned `String` before checking whether `bucket.iter().any(...)` already contains it. | `FIX-NOW` | Truncate via char boundary slice `&str`, checking containment before allocating. |
| **MEM-INS-03** | [`src/memory/insights.rs:147, 243-244`](file:///home/zou/data/workspace/Aleph/src/memory/insights.rs#L147) | `REDUNDANT-ALLOCATION` | `fetch_tool_invocation_rows` takes `agent_ids: &[String]`, forcing callers with `&str` to allocate `let one = [agent_id.to_string()];`. | `FIX-CAREFULLY` | Accept `&[impl AsRef<str>]` or `&[&str]` to eliminate owned string allocation. |
| **MEM-MOD-01** | [`src/memory/mod.rs:1-99`](file:///home/zou/data/workspace/Aleph/src/memory/mod.rs#L1-L99) | `ACCIDENTAL-COMPLEXITY` | Module root declarations and re-exports. | `REJECT` | Canonical public entry points. |
| **MEM-NS-01** | [`src/memory/namespace.rs:44-48`](file:///home/zou/data/workspace/Aleph/src/memory/namespace.rs#L44-L48) | `REDUNDANT-ALLOCATION` | `to_namespace_value(&self) -> String` returns an owned `"owner".to_string()`. | `FIX-NOW` | Expose `pub const fn as_str(&self) -> &'static str` or return `&'static str`. |
| **MEM-NS-02** | [`src/memory/namespace.rs:33`](file:///home/zou/data/workspace/Aleph/src/memory/namespace.rs#L33) | `ACCIDENTAL-COMPLEXITY` | `NamespaceScope` derives `Clone`, `PartialEq`, `Eq` but omits `Copy`. | `FIX-NOW` | Add `#[derive(Copy)]`. |
| **MEM-PROJ-01** | [`src/memory/project_scope.rs:103-106`](file:///home/zou/data/workspace/Aleph/src/memory/project_scope.rs#L103-L106) | `REDUNDANT-ALLOCATION` | In `project_namespace`, `for byte in &digest[..4] { hex.push_str(&format!("{byte:02x}")); }` allocates 4 temporary `String`s in a loop, then formats `format!("proj-{hex}")` (5 allocations total). | `FIX-NOW` | Replace loop with direct single format: `format!("proj-{:02x}{:02x}{:02x}{:02x}", digest[0], digest[1], digest[2], digest[3])` (1 allocation). |
| **MEM-PROJ-02** | [`src/memory/project_scope.rs:419`](file:///home/zou/data/workspace/Aleph/src/memory/project_scope.rs#L419) | `REDUNDANT-ALLOCATION` | `agent_id.strip_suffix(&format!("{NS_SEP}{OWNER_USER_ID}"))` allocates an owned `String` on every call just to strip suffix `"__u-owner"`. | `FIX-NOW` | Replace with zero-allocation: `agent_id.strip_suffix(OWNER_USER_ID)?.strip_suffix(NS_SEP)`. |
| **MEM-PROJ-03** | [`src/memory/project_scope.rs:286-294`](file:///home/zou/data/workspace/Aleph/src/memory/project_scope.rs#L286-L294) | `REDUNDANT-ALLOCATION` | `list_scoped_agent_ids` formats `{base}{NS_SEP}` prefix and collects filtered corpora list. | `FIX-NOW` | Stream iterator directly without unnecessary intermediate vectors. |
| **MEM-PROP-01** | [`src/memory/proptest_enums.rs:17-34`](file:///home/zou/data/workspace/Aleph/src/memory/proptest_enums.rs#L17-L34) | `CORRECTNESS` / `INVARIANT-TIGHTENING` | `arb_note_type()` covers 15 variants of `NoteType` but completely omits `NoteType::Feedback` (leaving `Feedback` uncovered by property roundtrip tests). | `FIX-NOW` | Add `Just(NoteType::Feedback)` to `prop_oneof!` in `arb_note_type()`. |
| **MEM-REEMB-01** | [`src/memory/reembed.rs:250-252`](file:///home/zou/data/workspace/Aleph/src/memory/reembed.rs#L250-L252) | `REDUNDANT-ALLOCATION` | Chained `.join(agent_id).join(&note.category).join(...)` creates 3 intermediate `PathBuf` heap allocations for every note in every re-embed batch. | `FIX-NOW` | Preallocate `PathBuf` and `.push()` path components. |
| **MEM-REEMB-02** | [`src/memory/reembed.rs:275`](file:///home/zou/data/workspace/Aleph/src/memory/reembed.rs#L275) | `REDUNDANT-ALLOCATION` | `let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();` allocates a `Vec<&str>` on every chunk. | `FIX-NOW` | Avoid allocating `text_refs` or reuse pre-sized buffer. |
| **MEM-SMODE-01** | [`src/memory/session_memory_mode.rs:67`](file:///home/zou/data/workspace/Aleph/src/memory/session_memory_mode.rs#L67) | `REDUNDANT-ALLOCATION` | `id.trim().to_ascii_lowercase().as_str()` allocates an owned `String` on every session memory mode resolution. | `FIX-NOW` | Match on `id.trim()` using `eq_ignore_ascii_case` to achieve zero-allocation parsing. |
| **MEM-SCRUB-01** | [`src/memory/streaming_scrubber.rs:156-160, 222`](file:///home/zou/data/workspace/Aleph/src/memory/streaming_scrubber.rs#L156-L160) | `REDUNDANT-ALLOCATION` | In `feed()`, `std::mem::take(&mut self.buf)` + `work.push_str(text)` + `out.push_str(...)` allocates 2 heap `String`s for every single streaming chunk even when no tags or partial tags are present. | `FIX-CAREFULLY` | Fast-path: when `self.buf.is_empty()` and not inside a span, check `text` directly without copying to `work` buffer. |
| **MEM-SCRUB-02** | [`src/memory/streaming_scrubber.rs:87, 125`](file:///home/zou/data/workspace/Aleph/src/memory/streaming_scrubber.rs#L87) | `REDUNDANT-ALLOCATION` | `pairs: Vec<(String, String)>` allocates 14 heap strings for the static default tags. | `DEFER` | Scrubber instance is reused across a turn; micro-overhead compared to stream processing. |
| **MEM-SINK-01** | [`src/memory/tool_signal_sink.rs:106`](file:///home/zou/data/workspace/Aleph/src/memory/tool_signal_sink.rs#L106) | `REDUNDANT-ALLOCATION` | `let trimmed: String = err.chars().take(200).collect();` allocates an intermediate `String` when `err` fails, which is immediately formatted into another `String`. | `FIX-NOW` | Slice `err` directly at char boundary (`floor_char_boundary(200)` or `char_indices().nth(200)`) directly into the `format!` call. |
| **MEM-EXT-CAPT-01** | [`src/memory/extensions/capture_lifecycle_tests.rs`](file:///home/zou/data/workspace/Aleph/src/memory/extensions/capture_lifecycle_tests.rs) | `ACCIDENTAL-COMPLEXITY` | Full lifecycle tests for capture filters and concurrency barriers. | `REJECT` | Test harness code; preserves regression coverage. |
| **MEM-EXT-DELEG-01** | [`src/memory/extensions/delegation_lifecycle_tests.rs:9`](file:///home/zou/data/workspace/Aleph/src/memory/extensions/delegation_lifecycle_tests.rs#L9) | `DEAD-CODE` / `ACCIDENTAL-COMPLEXITY` | Stale unused import `use std::future::Future;`. | `FIX-NOW` | Remove unused import. |
| **MEM-EXT-FP-01** | [`src/memory/extensions/first_party.rs:41-44`](file:///home/zou/data/workspace/Aleph/src/memory/extensions/first_party.rs#L41-L44) | `CORRECTNESS` / `INVARIANT-TIGHTENING` | In `EnvelopeRelevanceFloorExtension::on_retrieve()`, `slot.items.retain(...)` drops low-relevance items, but `slot.tokens_used` is never updated, leaving stale token counts that over-report context window usage. | `FIX-NOW` | Recalculate `slot.tokens_used = slot.items.iter().map(|item| item.tokens).sum();` after filtering. |
| **MEM-EXT-INS-01** | [`src/memory/extensions/insert_helper.rs:51-53`](file:///home/zou/data/workspace/Aleph/src/memory/extensions/insert_helper.rs#L51-L53) | `REDUNDANT-ALLOCATION` | Clones `store`, `registry`, and `ctx` before moving into async operation. | `DEFER` | Required for tokio async task boundary across lifetimes. |
| **MEM-EXT-MAN-01** | [`src/memory/extensions/manifest.rs:8`](file:///home/zou/data/workspace/Aleph/src/memory/extensions/manifest.rs#L8) | `ACCIDENTAL-COMPLEXITY` | `MemoryHook` unit enum derives `Clone` but omits `Copy`. | `FIX-NOW` | Add `#[derive(Copy)]`. |
| **MEM-EXT-MCP-01** | [`src/memory/extensions/mcp_adapter.rs:179`](file:///home/zou/data/workspace/Aleph/src/memory/extensions/mcp_adapter.rs#L179) | `REDUNDANT-ALLOCATION` | In `produce()`, `resp.get("raw_memories").and_then(...).cloned()` clones the entire `Vec<Value>` from an owned `resp` object. | `FIX-NOW` | Take the value directly from `mut resp` with `resp.get_mut("raw_memories").and_then(...).map(std::mem::take)`. |
| **MEM-EXT-MCP-02** | [`src/memory/extensions/mcp_adapter.rs:224`](file:///home/zou/data/workspace/Aleph/src/memory/extensions/mcp_adapter.rs#L224) | `REDUNDANT-ALLOCATION` | `resp.get("text").and_then(...).unwrap_or("").to_string()` allocates an empty `String` on the missing/default branch. | `FIX-NOW` | Use `.map(ToString::to_string).unwrap_or_default()`. |
| **MEM-EXT-MOD-01** | [`src/memory/extensions/mod.rs:1-28`](file:///home/zou/data/workspace/Aleph/src/memory/extensions/mod.rs#L1-L28) | `ACCIDENTAL-COMPLEXITY` | Submodule declarations and re-exports. | `REJECT` | Clean module root. |
| **MEM-EXT-REG-01** | [`src/memory/extensions/registry.rs:206, 237, 303, 333, 416`](file:///home/zou/data/workspace/Aleph/src/memory/extensions/registry.rs#L206) | `REDUNDANT-ALLOCATION` | Repeated `let name = ext.name().to_string();` inside loops across 5 dispatch methods (`dispatch_on_retrieve`, `dispatch_on_capture`, `dispatch_on_session_switch`, `dispatch_on_pre_compress`, `dispatch_on_delegation`), allocating an owned string on every extension hook call solely for fallback log formatting. | `FIX-NOW` | Use `ext.name()` as `&str` directly in tracing and format macros without heap allocation. |
| **MEM-EXT-REG-02** | [`src/memory/extensions/registry.rs:100, 122`](file:///home/zou/data/workspace/Aleph/src/memory/extensions/registry.rs#L100) | `REDUNDANT-ALLOCATION` | In `register` and `register_mcp`, `let name = ext.name().to_string();` allocates unconditionally before checking if the extension already exists in the registry. | `FIX-NOW` | Compare with `ext.name()` as `&str`; only allocate in the `Err` formatting path. |
| **MEM-EXT-SCHED-01** | [`src/memory/extensions/scheduler.rs:96`](file:///home/zou/data/workspace/Aleph/src/memory/extensions/scheduler.rs#L96) | `REDUNDANT-ALLOCATION` | `agent_id: DEFAULT_AGENT_ID.to_string()` allocates a new string on every 10-second tick in `run_once`. | `FIX-NOW` | Pass borrowed string or reuse string constant. |
| **MEM-EXT-SCHED-02** | [`src/memory/extensions/scheduler_lifecycle_tests.rs`](file:///home/zou/data/workspace/Aleph/src/memory/extensions/scheduler_lifecycle_tests.rs) | `ACCIDENTAL-COMPLEXITY` | Scheduler lifecycle integration test with tokio task barriers. | `REJECT` | Test scaffolding. |
| **MEM-EXT-TR-01** | [`src/memory/extensions/traits.rs:40`](file:///home/zou/data/workspace/Aleph/src/memory/extensions/traits.rs#L40) | `ACCIDENTAL-COMPLEXITY` | `Ok(vec![])` macro in trait default method. | `FIX-NOW` | Replace with `Ok(Vec::new())`. |
| **MEM-EXT-TYP-01** | [`src/memory/extensions/types.rs:1-200`](file:///home/zou/data/workspace/Aleph/src/memory/extensions/types.rs#L1-L200) | `ACCIDENTAL-COMPLEXITY` | Extension hook DTOs. | `REJECT` | Clean DTOs. |
| **MEM-FLUSH-MOD-01** | [`src/memory/flush/mod.rs:121`](file:///home/zou/data/workspace/Aleph/src/memory/flush/mod.rs#L121) | `REDUNDANT-ALLOCATION` | `let prefix = format!("{agent}{}", crate::memory::project_scope::NS_SEP);` allocates via `format!`. | `FIX-NOW` | Use `format!("{agent}__")` or `let mut prefix = agent.to_string(); prefix.push_str(NS_SEP);`. |
| **MEM-FLUSH-REG-01** | [`src/memory/flush/registry.rs:76`](file:///home/zou/data/workspace/Aleph/src/memory/flush/registry.rs#L76) | `REDUNDANT-ALLOCATION` | In `FlushRegistry::begin`, `map.entry(agent.to_string())` allocates an owned `String` even when `map.contains_key(agent)`. | `FIX-NOW` | Check `if let Some(entry) = map.get_mut(agent)` to increment `in_flight` without allocating a `String`. |

---

## 3. Summary of Routes

- **FIX-NOW**: 22 items
  - `MEM-SCAN-01`: Fast-path ASCII in content scanner homoglyph normalization
  - `MEM-EMGR-01`: Eliminate double config/id cloning in `EmbeddingManager::init`
  - `MEM-EMGR-02`: Eliminate read-then-write lock gap and clone in `switch_provider`
  - `MEM-EMGR-03`: In-place `splice` for queue requeuing in `flush_pending`
  - `MEM-EPROV-02`: Eliminate repeated URL format in embedding `call_api`
  - `MEM-EPROV-03`: Single vector allocation for embedding API payload
  - `MEM-EPROV-04`: Zero-alloc content-type header check
  - `MEM-EPROV-05`: Avoid intermediate `chunks` vector allocation in `embed_batch`
  - `MEM-ERES-01`: Zero-alloc `requested_id` borrow in `EmbeddingDecision`
  - `MEM-INS-01`: `remove` instead of `clone` on evidence samples map
  - `MEM-INS-02`: Zero-alloc prefix check before failure sample allocation
  - `MEM-NS-01`: `&'static str` accessor for `NamespaceScope`
  - `MEM-NS-02`: Add `Copy` derive to `NamespaceScope`
  - `MEM-PROJ-01`: Single `format!` replacing 4-iteration `format!` loop in `project_namespace`
  - `MEM-PROJ-02`: Zero-alloc `strip_suffix` in `owner_adoption_base`
  - `MEM-PROJ-03`: Stream iterator in `list_scoped_agent_ids`
  - `MEM-PROP-01`: Add missing `NoteType::Feedback` to proptest strategy `arb_note_type`
  - `MEM-REEMB-01`: Eliminate intermediate `PathBuf` allocations in `reembed_agent_notes`
  - `MEM-REEMB-02`: Avoid redundant vector allocation in reembed batch
  - `MEM-SMODE-01`: Zero-alloc `eq_ignore_ascii_case` in `MemoryMode::from_id`
  - `MEM-SINK-01`: Zero-alloc char boundary slice in tool failure formatting
  - `MEM-EXT-DELEG-01`: Remove unused import `use std::future::Future;`
  - `MEM-EXT-FP-01`: Recalculate `slot.tokens_used` after relevance filtering
  - `MEM-EXT-MAN-01`: Add `Copy` derive to `MemoryHook`
  - `MEM-EXT-MCP-01`: Direct take of `raw_memories` array without cloning `Vec<Value>`
  - `MEM-EXT-MCP-02`: Zero-alloc default branch in `on_pre_compress`
  - `MEM-EXT-REG-01`: Eliminate repeated `ext.name().to_string()` across 5 dispatch loops
  - `MEM-EXT-REG-02`: Delay `name.to_string()` allocation until error branch in `register`
  - `MEM-EXT-SCHED-01`: Reuse static agent string in scheduler tick
  - `MEM-EXT-TR-01`: Replace `vec![]` with `Vec::new()` in trait default method
  - `MEM-FLUSH-MOD-01`: Simplify partition prefix formatting
  - `MEM-FLUSH-REG-01`: Avoid string allocation when agent already in flush map

- **FIX-CAREFULLY**: 3 items
  - `MEM-EPROV-01`: `truncate_input` zero-alloc slicing (requires updating test asserting on `Cow::Owned`)
  - `MEM-INS-03`: `fetch_tool_invocation_rows` parameter generalization to `&[impl AsRef<str>]`
  - `MEM-SCRUB-01`: Fast-path zero-copy streaming scrubber chunk handling

- **DEFER**: 3 items
  - `MEM-SCAN-02`: `RegexSet` consolidation for content scanner
  - `MEM-SCRUB-02`: Static tag pair Cow representation
  - `MEM-EXT-INS-01`: Closure clone in `insert_with_runtime_capture_filter`

- **REJECT**: 6 items
  - `MEM-ESIG-01`: Embedding signature module is already optimal
  - `MEM-EXPL-01`: Explain read model DTOs are clean
  - `MEM-MOD-01`: Root module re-exports are canonical
  - `MEM-EXT-CAPT-01`: Capture lifecycle integration tests
  - `MEM-EXT-SCHED-02`: Scheduler lifecycle integration tests
  - `MEM-EXT-MOD-01`: Extensions module declarations
  - `MEM-EXT-TYP-01`: Extension hook types

---

## 4. Invariant Safety Pre-Flight

1. **Security & Prompt Injection Guarantees**:
   All scanning patterns in `content_scanner.rs` and redaction logic in `tool_signal_sink.rs` remain completely uncompromised. Any fast-pathing on `is_ascii()` preserves exact homoglyph normalization semantics because Unicode homoglyphs (Cyrillic, fullwidth forms) are strictly non-ASCII.
2. **Multi-Tenant & Session Scope Isolation**:
   `project_scope.rs` partitions (`proj-*`, `u-*`, `p-*`) and `session_read_ids`/`session_write_id` rules remain unchanged. Suffix stripping in `owner_adoption_base` using `strip_suffix(OWNER_USER_ID)?.strip_suffix(NS_SEP)` is mathematically identical to `strip_suffix("__u-owner")` while removing heap allocation.
3. **Context Comptroller Token Budget Accounting**:
   Recalculating `slot.tokens_used` in `EnvelopeRelevanceFloorExtension` fixes a subtle desynchronization where filtered notes still counted against the prompt token budget.
4. **Concurrency & Lock Inversion**:
   In `EmbeddingManager::switch_provider`, capturing `old_id` within the write lock using `std::mem::replace` eliminates a read-then-write lock transition, preventing lock gap races during provider switching.
