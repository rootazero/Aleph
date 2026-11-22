# Occam's Razor Review Brief: Media, Memory, and Metrics

## 1. Scope & Subsystem Invariants

This review targets `src/media`, `src/memory`, and `src/metrics` under Occam's Razor doctrine:
- **Simplicity is the removal of unnecessary concepts, not merely line count reduction.**
- Every abstraction, clone, intermediate allocation, and indirection must earn its existence.
- Non-negotiable invariants (public API surface, error semantics, database schema and migrations, concurrency properties, and async cancellation safety) must remain completely stable.

---

### Subsystem Invariants

#### 1. Media (`src/media`)
1. **Pipeline & Format Detection (`detect.rs`, `pipeline.rs`)**:
   - Magic bytes detection and MIME sniffing must preserve strict byte matches and fallback heuristics.
   - Media processing errors (`error.rs`) and limits (`policy.rs`, max size, timeout) must fail closed without crashing server tasks.
2. **Processors (`processors/`, `processor.rs`)**:
   - Image transcoding, audio extraction, document text extraction must maintain sandboxed error propagation.
   - Zero-allocation borrowing where slices suffice instead of copying raw payload bytes into temporary buffers.
3. **Caching & Whisper (`cache.rs`, `whisper.rs`, `transcription.rs`)**:
   - Cache keys, eviction algorithms (LRU / TTL), and file locks must remain deterministic.
   - Transcription result formats and streaming chunks must preserve external contracts.

#### 2. Metrics (`src/metrics`)
1. **Prometheus Registry & Counter Semantics (`src/metrics/mod.rs`)**:
   - Atomic counter increments and gauge updates must maintain correct atomic memory orderings (`Ordering::Relaxed` or `Ordering::SeqCst` as designed).
   - Metric names, label keys, and Prometheus text formatting must preserve standard formatting without breaking scraping pipelines.
   - Avoid transient heap allocations inside metric recording hot-paths.

#### 3. Memory (`src/memory`)
1. **Data Integrity & Storage (`store/sqlite/`, `store/markdown/`, `schema/`)**:
   - SQLite migrations, vector table triggers, transaction boundaries, and foreign key constraints must never be bypassed.
   - Reducers and projections must maintain fail-closed idempotency on duplicate events.
2. **Retrieval, Scoring & Assembly (`note_retrieval/`, `rerank/`, `assembler/`, `context/`)**:
   - Cosine similarity calculations, recency decay curves, reranking thresholds, and context window budgets must preserve mathematical invariants.
   - Avoid extraneous vector/string cloning inside note search ranking and prompt assembly loops.
3. **Dreaming & Evolution (`dreaming/`, `dreaming/stages/`, `dreaming/evolution/`)**:
   - Note consolidation, decay, drift detection, and knowledge graph recomputation must operate safely over background epochs.
   - Mutation gates and skill distillation must guarantee convergence and never corrupt persistent notes.
4. **Runtime & Concurrency (`runtime/`, `events/`, `reflector/`)**:
   - Task cancellation, event dispatch, session store updates, and mutex/RwLock critical sections must prevent lock inversion and deadlocks.

---

## 2. Methodology & Model Tiering

### Jev Tier (Classification, Filtering, Routing)
The **Jev tier** (lightweight fast-scanning tier using Gemini Flash / Flash-Lite equivalent capabilities):
1. **Candidate Sweep**: Scan assigned batch files for Occam's Razor anti-patterns:
   - Redundant allocations: repeated `format!`, unnecessary `.to_string()`, `.to_vec()`, `.clone()`.
   - Redundant clones on types that already implement `Copy` or where borrowed references `&str` / `&[T]` suffice.
   - Over-nested control flow: `if let Some(...) { if let Some(...) { ... } }` flattening.
   - Unused private helper functions, dead code, stale imports.
   - Unnecessary `Box<dyn ...>` or trait indirection where concrete types or generic parameters are simpler.
2. **Taxonomy Classification**: Classify candidate issues into:
   - `DEAD-CODE` / `ACCIDENTAL-COMPLEXITY`
   - `REDUNDANT-ALLOCATION`
   - `CONTROL-FLOW` / `INDIRECTION`
   - `PANIC-RISK` / `DEFENSIVE-CHECK`
   - `CORRECTNESS` / `INVARIANT-TIGHTENING`
3. **Filtering & Routing Decisions**:
   - Tag findings with route decisions: `FIX-NOW`, `FIX-CAREFULLY`, `DEFER`, `REJECT`.
   - Output structured classification matrices in `classification/<BATCH_ID>.class.md`.

### Deep Reasoning Tier (minimax-M3 / Invariant Analysis & Code Repair)
The **Deep Reasoning tier**:
1. **Semantic Invariant Verification**:
   - Verify that proposed simplifications preserve all public API contracts, error types, lifetime constraints, and concurrency semantics.
   - Cross-check architectural hub nodes and call chains using the Graphify semantic graph (`graphify-out/graph.json`).
2. **Surgical Patch Authoring**:
   - Apply fixes directly in the dedicated git worktree `/home/zou/data/workspace/Aleph-wt-media-memory-metrics`.
   - Preserve all existing tests and docstrings.
3. **Evidence-Based Documentation**:
   - Record detailed findings and remediation rationale in `findings/<BATCH_ID>.md`.
   - Commit directly per batch to git without PR and without interim cargo check.
