# Classification & Routing Matrix: MEM_NOTES_INGEST_INDEX

## Executive Summary

**Batch ID**: `MEM_NOTES_INGEST_INDEX`  
**Subsystem**: `src/memory/notes/{indexer, ingest, orientation, query_filer, watcher}`  
**Scope**: 27 batch files + `src/memory/notes/watcher.rs` (~10,967 lines total)  
**Doctrine**: Occam's Razor (`/rust-occams-razor`) — eliminate unnecessary complexity, redundant allocations, dead code, and over-nested control flow while preserving public API contracts, database schemas, and async safety.

The target scope comprises note ingestion (retrieval, LLM planning, transactional staging/apply, governance admission), wiki orientation document generation (`SCHEMA.md`, `index.md`, `log.md`, `overview.md`, `purpose.md`, Obsidian vault config), query answer filing (`query/` notes), and recursive vault file watching.

**Key Findings**:
1. **Redundant Allocations in Hot Paths**:
   - `retrieve.rs`: Full `content` string cloning of every seed note just to compute an 800-char preview and extract the first bullet line.
   - `plan_parse.rs`: Element-wise deserialization deeply cloning every `serde_json::Value` in the plan array even though the root `Value` is owned.
   - `index_md.rs`: `BTreeMap<String, Vec<&NoteIndexEntry>>` cloning category strings for every note index entry, redundant vector cloning during sorting, and multi-allocation summary sanitization.
   - `log_md.rs`: Four separate string allocations in `sanitize_single_line` on every log line, plus full-file string reads on every append to count lines.
   - `watcher.rs`: Allocating a heap `Vec<&str>` on every debounced filesystem event component check in `classify()`.
   - Repeated `out.push_str(&format!(...))` in prompt building (`helpers.rs`, `batch.rs`, `schema.rs`, `index_md.rs`, `log_md.rs`) instead of writing directly into pre-allocated string buffers.
2. **Duplicate Abstractions & Hashing**:
   - `schema.rs` and `query_filer/filer.rs` reinvent private SHA-256 hex hashing functions instead of using the canonical `crate::memory::notes::indexer::sha2_hash`.
   - `overview_md.rs` and `purpose_md.rs` share near-identical structural boilerplate (Shape 16: twin subsystems).
3. **Async / Blocking Boundaries**:
   - `obsidian_config.rs` calls synchronous blocking `p.exists()` inside an async function.
   - `watcher.rs` bulk reconciliation calls blocking `std::fs::read_dir` inside a spawned async task.
4. **Invariant Stability**:
   - Transactional commit/rollback order in `apply.rs` (`.tx/{id}`) is rock-solid and must remain fail-closed.
   - All tests in `indexer/tests.rs`, `ingestor/tests.rs`, and `watcher/lifecycle_tests.rs` are intentional test scaffolding (`REJECT`).

---

## Classification & Routing Matrix

| Location | Taxonomy | Smell Description | Route | Action |
| :--- | :--- | :--- | :--- | :--- |
| [`src/memory/notes/ingest/retrieve.rs#L148-L153`](file:///home/zou/data/workspace/Aleph/src/memory/notes/ingest/retrieve.rs#L148-L153) | REDUNDANT-ALLOCATION | Clones full note `content: String` for every seed note only to compute preview and extract first bullet | `FIX-NOW` | Borrow `content.as_str()` directly as `&str` without cloning note content |
| [`src/memory/notes/ingest/retrieve.rs#L86-L94`](file:///home/zou/data/workspace/Aleph/src/memory/notes/ingest/retrieve.rs#L86-L94) | REDUNDANT-ALLOCATION | `s.path.clone()` cloned twice per seed note into `seen: BTreeSet<String>` and `ranked` | `FIX-NOW` | Optimize deduplication check and replace `BTreeSet` with `HashSet` |
| [`src/memory/notes/ingest/retrieve.rs#L106-L109`](file:///home/zou/data/workspace/Aleph/src/memory/notes/ingest/retrieve.rs#L106-L109) | CONTROL-FLOW / INDIRECTION | Redundant `seen.insert(link.clone())` after `seen.contains(&link)` was already checked | `FIX-NOW` | Flatten check and avoid cloning `link` unnecessarily |
| [`src/memory/notes/ingest/retrieve.rs#L184-L201`](file:///home/zou/data/workspace/Aleph/src/memory/notes/ingest/retrieve.rs#L184-L201) | CONTROL-FLOW / INDIRECTION | Deeply nested `if let` blocks in `first_body_bullet` frontmatter stripping | `FIX-NOW` | Flatten frontmatter extraction using combinators |
| [`src/memory/notes/ingest/ref_table.rs#L101`](file:///home/zou/data/workspace/Aleph/src/memory/notes/ingest/ref_table.rs#L101) | REDUNDANT-ALLOCATION | `*value = path.clone()` discards existing `value: &mut String` heap buffer | `FIX-NOW` | Use `value.clone_from(path)` to reuse allocated capacity |
| [`src/memory/notes/ingest/ref_table.rs#L81-L83`](file:///home/zou/data/workspace/Aleph/src/memory/notes/ingest/ref_table.rs#L81-L83) | REDUNDANT-ALLOCATION | `token(idx) -> String` allocates `format!("[P{idx}]")` for every token display in prompt building | `FIX-NOW` | Allow formatting directly or passing index into formatters |
| [`src/memory/notes/ingest/apply.rs#L156-L168`](file:///home/zou/data/workspace/Aleph/src/memory/notes/ingest/apply.rs#L156-L168) | REDUNDANT-ALLOCATION | `ensure_origin_marker` clones `line.to_string()` even when matching origin regex | `FIX-NOW` | Return `Cow<'_, str>` so lines with origin markers are not re-allocated |
| [`src/memory/notes/ingest/apply.rs#L342-L346`](file:///home/zou/data/workspace/Aleph/src/memory/notes/ingest/apply.rs#L342-L346) | REDUNDANT-ALLOCATION | Eagerly stamps origin marker and allocates String for new facts before filtering duplicates | `FIX-NOW` | Check existence before allocating transformed string |
| [`src/memory/notes/ingest/apply.rs#L423-L427`](file:///home/zou/data/workspace/Aleph/src/memory/notes/ingest/apply.rs#L423-L427) | REDUNDANT-ALLOCATION | `"".to_string()` allocation when `evidence_source_ids` is empty in `Contradict` | `FIX-NOW` | Use empty string slice or `String::new()` |
| [`src/memory/notes/ingest/apply.rs#L611-L614`](file:///home/zou/data/workspace/Aleph/src/memory/notes/ingest/apply.rs#L611-L614) | REDUNDANT-ALLOCATION | `report.touched_paths.retain(|p| seen.insert(p.clone()))` uses `BTreeSet<String>` | `FIX-NOW` | Use `HashSet<String>` to eliminate tree node allocations |
| [`src/memory/notes/ingest/prompts.rs#L157-L164`](file:///home/zou/data/workspace/Aleph/src/memory/notes/ingest/prompts.rs#L157-L164) | REDUNDANT-ALLOCATION | Clones ~3.5 KB prompt static string into `String` on every batch even when no suffix is needed | `FIX-CAREFULLY` | Return `Cow<'static, str>` from `build_compound_system_prompt` |
| [`src/memory/notes/ingest/ingestor/plan_parse.rs#L142-L167`](file:///home/zou/data/workspace/Aleph/src/memory/notes/ingest/ingestor/plan_parse.rs#L142-L167) | REDUNDANT-ALLOCATION | Deep-clones each `serde_json::Value` in `ops` and `schema_proposals` during lenient parsing | `FIX-NOW` | Destructure owned `json: Value` and `into_iter()` to avoid deep clones of JSON trees |
| [`src/memory/notes/ingest/ingestor/plan_parse.rs#L27-L33`](file:///home/zou/data/workspace/Aleph/src/memory/notes/ingest/ingestor/plan_parse.rs#L27-L33) | REDUNDANT-ALLOCATION | Clones category slice `cat.to_string()` for every touched path | `FIX-NOW` | Key map or borrow category without redundant intermediate allocation |
| [`src/memory/notes/ingest/ingestor/helpers.rs#L43-L46`](file:///home/zou/data/workspace/Aleph/src/memory/notes/ingest/ingestor/helpers.rs#L43-L46) | CONTROL-FLOW / INDIRECTION | `split_once('/')` called twice on the same path for `PageOp::Contradict` | `FIX-NOW` | Split path once and reuse `(category, filename)` |
| [`src/memory/notes/ingest/ingestor/helpers.rs#L133-L169`](file:///home/zou/data/workspace/Aleph/src/memory/notes/ingest/ingestor/helpers.rs#L133-L169) | REDUNDANT-ALLOCATION | Repeated `out.push_str(&format!(...))` in `build_user_prompt` | `FIX-NOW` | Use `write!(out, ...)` with `std::fmt::Write` directly into pre-allocated string |
| [`src/memory/notes/ingest/ingestor/helpers.rs#L259-L268`](file:///home/zou/data/workspace/Aleph/src/memory/notes/ingest/ingestor/helpers.rs#L259-L268) | REDUNDANT-ALLOCATION | `chars: Vec<char>` collected on every CJK word in `keyword_query_terms` | `FIX-NOW` | Iterate adjacent char pairs without heap-collecting full word char vec |
| [`src/memory/notes/ingest/ingestor/batch.rs#L56-L307`](file:///home/zou/data/workspace/Aleph/src/memory/notes/ingest/ingestor/batch.rs#L56-L307) | REDUNDANT-ALLOCATION | `batch_ids` collected as `Vec<String>`, then cloned again via `.with_batch_sources(batch_ids.to_vec())` | `FIX-NOW` | Pass `batch_ids` by value or slice borrowing to avoid double clone |
| [`src/memory/notes/ingest/ingestor/batch.rs#L516-L533`](file:///home/zou/data/workspace/Aleph/src/memory/notes/ingest/ingestor/batch.rs#L516-L533) | REDUNDANT-ALLOCATION | Repeated `user.push_str(&format!(...))` in `enforce_link_contract` prompt assembly | `FIX-NOW` | Use `write!(user, ...)` directly |
| [`src/memory/notes/ingest/ingestor/batch.rs#L554-L558`](file:///home/zou/data/workspace/Aleph/src/memory/notes/ingest/ingestor/batch.rs#L554-L558) | REDUNDANT-ALLOCATION | Deeply clones JSON array `.cloned().unwrap_or_default()` in `enforce_link_contract` | `FIX-NOW` | Borrow slice or take from owned `json` |
| [`src/memory/notes/orientation/schema.rs#L47-L56`](file:///home/zou/data/workspace/Aleph/src/memory/notes/orientation/schema.rs#L47-L56) | REDUNDANT-ALLOCATION | `out.push_str(&format!("## {name}\n{section}\n\n"))` in `compact_for_prompt` | `FIX-NOW` | Use `write!(out, ...)` to eliminate intermediate format allocations |
| [`src/memory/notes/orientation/schema.rs#L78-L84`](file:///home/zou/data/workspace/Aleph/src/memory/notes/orientation/schema.rs#L78-L84) | REDUNDANT-ALLOCATION | `format!("\n## {name}\n")` allocated on every call to `extract_section` | `FIX-NOW` | Match substring without temporary string allocation |
| [`src/memory/notes/orientation/schema.rs#L86-L90`](file:///home/zou/data/workspace/Aleph/src/memory/notes/orientation/schema.rs#L86-L90) | DEAD-CODE / ACCIDENTAL-COMPLEXITY | Private duplicate `fn hash(s: &str) -> String` reinventing `sha2_hash` | `FIX-NOW` | Delegate to canonical `crate::memory::notes::indexer::sha2_hash` |
| [`src/memory/notes/orientation/index_md.rs#L60-L64`](file:///home/zou/data/workspace/Aleph/src/memory/notes/orientation/index_md.rs#L60-L64) | REDUNDANT-ALLOCATION | Collects category strings into `BTreeSet` in `write` to compute category count | `FIX-NOW` | Use `by_cat.len()` from renderer or `HashSet` |
| [`src/memory/notes/orientation/index_md.rs#L75-L78`](file:///home/zou/data/workspace/Aleph/src/memory/notes/orientation/index_md.rs#L75-L78) | REDUNDANT-ALLOCATION | Clones category string for every note entry in `render` | `FIX-NOW` | Key map by borrowed `&str`: `BTreeMap<&str, Vec<&NoteIndexEntry>>` |
| [`src/memory/notes/orientation/index_md.rs#L99-L112`](file:///home/zou/data/workspace/Aleph/src/memory/notes/orientation/index_md.rs#L99-L112) | REDUNDANT-ALLOCATION | `items.clone()` duplicates note slice for sorting; repeated `format!` in line loop | `FIX-NOW` | Sort in place in `by_cat` and use `write!` directly to `out` |
| [`src/memory/notes/orientation/index_md.rs#L195-L202`](file:///home/zou/data/workspace/Aleph/src/memory/notes/orientation/index_md.rs#L195-L202) | REDUNDANT-ALLOCATION | Allocates 3 intermediate strings in `sanitise_summary` for truncated summaries | `FIX-NOW` | Single-pass char collection with inline newline stripping and truncation |
| [`src/memory/notes/orientation/log_md.rs#L45-L54`](file:///home/zou/data/workspace/Aleph/src/memory/notes/orientation/log_md.rs#L45-L54) | REDUNDANT-ALLOCATION | Repeated `buf.push_str(&format!(...))` in `append` | `FIX-NOW` | Use `write!(buf, ...)` |
| [`src/memory/notes/orientation/log_md.rs#L76-L88`](file:///home/zou/data/workspace/Aleph/src/memory/notes/orientation/log_md.rs#L76-L88) | REDUNDANT-ALLOCATION | Full `log.md` string read (`tokio::fs::read_to_string`) on every single append | `FIX-CAREFULLY` | Check file size via `metadata().len()` or streaming line count |
| [`src/memory/notes/orientation/log_md.rs#L129-L147`](file:///home/zou/data/workspace/Aleph/src/memory/notes/orientation/log_md.rs#L129-L147) | REDUNDANT-ALLOCATION | Four separate string allocations per line in `sanitize_single_line` | `FIX-NOW` | Single-pass space-collapsing and trimming loop |
| [`src/memory/notes/orientation/obsidian_config.rs#L27`](file:///home/zou/data/workspace/Aleph/src/memory/notes/orientation/obsidian_config.rs#L27) | CONTROL-FLOW / INDIRECTION | Synchronous blocking `p.exists()` inside async function | `FIX-NOW` | Replace with `!tokio::fs::try_exists(&p).await.unwrap_or(false)` |
| [`src/memory/notes/orientation/overview_md.rs#L10-L42`](file:///home/zou/data/workspace/Aleph/src/memory/notes/orientation/overview_md.rs#L10-L42) | DEAD-CODE / ACCIDENTAL-COMPLEXITY | `OverviewMd` and `PurposeMd` share identical structure and logic (Shape 16) | `DEFER` | Preserve distinct public types; extract common internal helper in future refactor |
| [`src/memory/notes/query_filer/filer.rs#L74-L79`](file:///home/zou/data/workspace/Aleph/src/memory/notes/query_filer/filer.rs#L74-L79) | DEAD-CODE / ACCIDENTAL-COMPLEXITY | `query_hash` reimplements Sha256 hex digest instead of using `sha2_hash` | `FIX-NOW` | Delegate to canonical `sha2_hash(&normalized)` |
| [`src/memory/notes/query_filer/filer.rs#L117-L127`](file:///home/zou/data/workspace/Aleph/src/memory/notes/query_filer/filer.rs#L117-L127) | REDUNDANT-ALLOCATION | Intermediate `Vec<String>` and `.join("\n")` for `sources_list` | `FIX-NOW` | Format sources directly into `user_prompt` using `writeln!` |
| [`src/memory/notes/watcher.rs#L122-L125`](file:///home/zou/data/workspace/Aleph/src/memory/notes/watcher.rs#L122-L125) | REDUNDANT-ALLOCATION | Allocates a heap `Vec<&str>` on every debounced filesystem event in `classify` | `FIX-NOW` | Walk path components with an iterator (`rel.components()`) with 0 heap allocations |
| [`src/memory/notes/watcher.rs#L218-L255`](file:///home/zou/data/workspace/Aleph/src/memory/notes/watcher.rs#L218-L255) | ACCIDENTAL-COMPLEXITY | Double pass over `events` (count then collect) | `REJECT` | Intentional burst short-circuit pattern to avoid allocations during bulk changes |
| [`src/memory/notes/watcher.rs#L392-L411`](file:///home/zou/data/workspace/Aleph/src/memory/notes/watcher.rs#L392-L411) | CONTROL-FLOW / INDIRECTION | Synchronous `std::fs::read_dir(root)` in async `reconcile_batch` task | `FIX-CAREFULLY` | Use `tokio::fs::read_dir` or `spawn_blocking` |
| `src/memory/notes/indexer/tests.rs` | ACCIDENTAL-COMPLEXITY | Test scaffolding & mock fixtures | `REJECT` | Intentional unit/integration test coverage for NoteIndexer |
| `src/memory/notes/ingest/ingestor/tests.rs` | ACCIDENTAL-COMPLEXITY | Test scaffolding & mock fixtures | `REJECT` | Intentional integration tests for compound ingest pipeline |
| `src/memory/notes/watcher/lifecycle_tests.rs` | ACCIDENTAL-COMPLEXITY | Test scaffolding & mock fixtures | `REJECT` | Intentional lifecycle and shutdown verification tests |

---

## Detailed Findings by Sub-Module

### 1. Ingestion Pipeline (`src/memory/notes/ingest/`)

#### 1.1 `retrieve.rs`: Seed Note Content Cloning & Inefficient Filtering
- **Finding**: In lines 148–153, for each top-ranked candidate note, `seeds.iter().find(|s| s.path == path).map(|s| s.content.clone())` allocates a full `String` copy of the note's entire markdown text (frequently multiple kilobytes). This string is only used for an 800-character preview and the first bullet line.
- **Remediation (`FIX-NOW`)**: Borrow `s.content.as_str()` directly as `&str`. `full.chars().take(...)` and `first_body_bullet(full)` both operate seamlessly on `&str`.
- **Finding**: Lines 86–94 use `BTreeSet<String>` for path deduplication and clone `s.path` twice. `HashSet<String>` eliminates logarithmic node allocations.

#### 1.2 `ref_table.rs`: In-Place String Mutation Allocation
- **Finding**: In `resolve_field(&self, value: &mut String)` (line 101), `*value = path.clone()` drops the heap allocation already held by `value` and allocates a brand new buffer.
- **Remediation (`FIX-NOW`)**: Use `value.clone_from(path)` to reuse the existing string buffer.

#### 1.3 `ingestor/plan_parse.rs`: Element-Wise Plan Deserialization Clones
- **Finding**: In `parse_plan_lenient(json: serde_json::Value)` (lines 142–167), `arr.iter().filter_map(|op| serde_json::from_value::<PageOp>(op.clone()))` clones every `serde_json::Value` (which recursively clones nested HashMaps, Strings, and Arrays).
- **Remediation (`FIX-NOW`)**: Since `json` is passed by value (owned), take or destructure the `ops` array and iterate with `into_iter()`, moving each `op` value directly into `serde_json::from_value::<PageOp>(op)` with zero cloning.

#### 1.4 `apply.rs`: Origin Marker Stamp & Deduplication Allocations
- **Finding**: `ensure_origin_marker` (lines 156–168) allocates a new `String` via `line.to_string()` even when the origin regex matches. In line 344, this transformation is run on every new fact before filtering out existing facts, discarding allocations immediately.
- **Remediation (`FIX-NOW`)**: Return `Cow<'_, str>` from `ensure_origin_marker` and check existence before allocating.
- **Finding**: Line 611 uses `BTreeSet<String>` to deduplicate `report.touched_paths`. Switching to `HashSet<String>` avoids B-tree node allocations.

#### 1.5 `ingestor/helpers.rs` & `batch.rs`: Prompt String Formatting
- **Finding**: Repeated `out.push_str(&format!(...))` patterns in `build_user_prompt` (lines 133–169) and `enforce_link_contract` (lines 516–533).
- **Remediation (`FIX-NOW`)**: Import `std::fmt::Write` and use `write!(out, ...)` to stream formatted text directly into the pre-allocated string.

---

### 2. Orientation Document System (`src/memory/notes/orientation/`)

#### 2.1 `index_md.rs`: Renderer Vector and Map Allocations
- **Finding**: In `render` (lines 75–78), `BTreeMap<String, Vec<&NoteIndexEntry>>` clones `e.category` for every note entry. In lines 99–112, `items.clone()` allocates a new vector of references for each category just to sort by timestamp.
- **Remediation (`FIX-NOW`)**: Key by borrowed `&str` (`BTreeMap<&str, Vec<&NoteIndexEntry>>`) and sort `items` in-place within the mutable map entry.
- **Finding**: In `sanitise_summary` (lines 195–202), three intermediate strings are allocated when truncating summaries.
- **Remediation (`FIX-NOW`)**: Single-pass char filtering with character count bounding into a single pre-allocated `String`.

#### 2.2 `log_md.rs`: Full File Reads and Multi-Allocation Sanitization
- **Finding**: Line 76–88: `line_count()` reads the entire `log.md` into an owned string (`tokio::fs::read_to_string`) on every single log append via `rotate_if_needed()`. At 2000 lines, this allocates hundreds of kilobytes on each memory write.
- **Remediation (`FIX-CAREFULLY`)**: Inspect file metadata size or count newlines in chunks rather than reading the entire file into a `String`.
- **Finding**: `sanitize_single_line` (lines 129–147) allocates four separate `String`s via chained `.replace().replace()...` and `.trim().to_string()`.
- **Remediation (`FIX-NOW`)**: Perform space-collapsing and newline replacement in a single linear pass over chars into one pre-allocated buffer.

#### 2.3 `schema.rs`: Duplicate Hasher & Substring Formatting
- **Finding**: Lines 86–90 define a local `hash(s: &str) -> String` using Sha256 + hex::encode, duplicating `crate::memory::notes::indexer::sha2_hash`.
- **Remediation (`FIX-NOW`)**: Call canonical `sha2_hash`.
- **Finding**: `compact_for_prompt` (lines 47–56) and `extract_section` (lines 78–84) use temporary string allocations in loops and header searches.

#### 2.4 `obsidian_config.rs`: Blocking File Check in Async Task
- **Finding**: Line 27 calls synchronous `p.exists()`.
- **Remediation (`FIX-NOW`)**: Replace with async `tokio::fs::try_exists(&p).await.unwrap_or(false)`.

---

### 3. Query Filer (`src/memory/notes/query_filer/`)

#### 3.1 `filer.rs`: Duplicate Hasher and Prompt Assembly Vecs
- **Finding**: `query_hash` (lines 74–79) re-implements Sha256 hex digest instead of delegating to `sha2_hash(&normalized)`.
- **Remediation (`FIX-NOW`)**: Use canonical `sha2_hash`.
- **Finding**: Lines 117–127 map `synthesis.sources` to a `Vec<String>`, join with newlines, and format into `user_prompt`.
- **Remediation (`FIX-NOW`)**: Stream sources directly into `user_prompt` using `writeln!`.

---

### 4. File Watcher (`src/memory/notes/watcher.rs`)

#### 4.1 `classify()`: Per-Event Component Vector Allocation
- **Finding**: Lines 122–125 collect path components into a `Vec<&str>` (`rel.components().map(...).collect::<Option<Vec<_>>>()?`) for every incoming filesystem event.
- **Remediation (`FIX-NOW`)**: Step through components with an iterator (`let mut comps = rel.components();`) without heap-allocating a vector.

#### 4.2 Bulk Directory Reading
- **Finding**: Lines 392–411 use synchronous `std::fs::read_dir(root)` within an async task during bulk change reconcile.
- **Remediation (`FIX-CAREFULLY`)**: Use `tokio::fs::read_dir` to ensure the Tokio executor is never blocked during bulk filesystem reconciliations.

---

## Invariant Safety Pre-Flight

1. **Transactional Integrity**:
   - `CompoundApplyTx` (`apply.rs`) staging in `.tx/{tx_id}`, atomic writes, and ordered commit/rollback guarantees must remain strictly untouched.
2. **Deterministic Token Mapping**:
   - `RefTable` token grammar (`[P<n>]`) and out-of-range reference dropping must remain byte-for-byte identical to prevent LLM path drift.
3. **Database Schema & Vector Tables**:
   - SQLite tables (`notes_index`, `query_filed`, `graph_insights`) and vector indices remain unaltered.
4. **Obsidian Vault Compatibility**:
   - Generated markdown layout (`SCHEMA.md`, `index.md`, `log.md`, `.obsidian/` configuration) must preserve frontmatter fences and wikilink conventions (`[[category/title]]`).
