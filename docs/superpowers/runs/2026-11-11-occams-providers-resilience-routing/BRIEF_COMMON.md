# Occam's Razor Review Brief: Providers, Resilience, and Routing

## 1. Scope & Subsystem Invariants

This review targets `src/providers`, `src/resilience`, and `src/routing` under Occam's Razor doctrine:
- **Simplicity is the removal of unnecessary concepts, not merely line count reduction.**
- Every abstraction, clone, intermediate allocation, and indirection must earn its existence.
- Non-negotiable invariants (public API surface, error semantics, database schema, concurrency properties, and async cancellation safety) must remain completely stable.

---

### Subsystem Invariants

#### 1. Routing (`src/routing`)
1. **Session Key Resolution & Normalization (`session_key.rs`, `resolve.rs`)**:
   - Session key syntax, delimiter parsing (`:`, `/`), and prefix/channel discrimination must remain strictly backwards-compatible.
   - Fail-closed route resolution: Unrecognized channels or malformed keys must return well-typed resolution errors or fallbacks.
2. **Overlay & Identity Links (`overlay.rs`, `identity_links.rs`)**:
   - User identity aliasing, link traversal, and overlay precedence must prevent loop cycles and preserve deterministic link ordering.

#### 2. Resilience (`src/resilience`)
1. **Database Schema & Migrations (`database/migration.rs`, `database/state_database/`)**:
   - SQLite migration integrity: Never mutate already-applied schema definitions or table layouts.
   - Transactional isolation: Write transactions must ensure atomic rollback on errors.
   - Prepared statements and query parameters must be cached/reused where beneficial without leaking unbounded state.
2. **Traces, Memory Events & Task Queuing (`database/traces.rs`, `database/memory_events.rs`, `database/tasks.rs`)**:
   - Task states and transition invariants (`Pending` -> `Running` -> `Completed`/`Failed`) must not be bypassed.
   - Cursor pagination and chunk offsets must handle boundary conditions accurately.

#### 3. Providers (`src/providers`)
1. **Protocol Adapters & Wire Formats (`protocols/`, `anthropic/`, `gemini/`, `openai/`)**:
   - Streaming SSE decoding: Byte chunk buffering, UTF-8 boundary assembly, and newline frame splitting must be strictly zero-allocation where possible.
   - Request serialization: Wire payloads for OpenAI, Anthropic, Gemini, Ollama, Codex must match upstream provider specifications verbatim.
   - Tool calling / structured output schema translations must preserve schema constraints without dropping properties.
2. **Model Catalog, Presets & Failover (`model_catalog/`, `presets/`, `failover/`)**:
   - Failover provider health tracking, circuit breaker half-open probes, and cooldown timers must preserve thread-safety and lock ordering.
   - Capability flags (`supports_vision`, `supports_tools`, `supports_thinking`) in catalog records must not be inadvertently dropped or altered.
3. **Core Provider Traits, Metering & Delta (`adapter.rs`, `metering.rs`, `delta.rs`, `http_provider.rs`)**:
   - Provider handle lifetimes, `Clone` semantics for client handles, and shared HTTP client connection pooling (`reqwest::Client`) must be preserved.
   - Token metering and usage aggregation must avoid numerical overflow and remain accurate.

---

## 2. Methodology & Model Tiering

### Jev Tier (Classification, Filtering, Routing)
The **Jev tier** (lightweight fast-scanning tier using Flash models):
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
3. **Smell Routing**: Assign each finding an action:
   - `FIX-NOW`: High-confidence, zero-risk simplification or correctness fix.
   - `FIX-CAREFULLY`: Nuanced simplification requiring careful verification of invariants.
   - `DEFER`: Architectural changes that require cross-crate updates.
   - `REJECT`: False positive or intentional design choice.

### Deep Reasoning Tier (Invariant Analysis & Code Authoring)
The **deep reasoning tier** (Senior Reviewer & Code Mutation):
1. **Semantic Invariant Verification**: Cross-reference each proposed fix with system invariants and Redlines (R1-R10).
2. **Surgical Code Fixes**: Apply direct mutations in the worktree workspace preserving all contracts.
3. **Direct Commit**: Commit each batch independently with conventional commit message format.
4. **Compilation Safety**: No interim cargo checks; unified `CARGO_BUILD_JOBS=2 cargo check --jobs 2` after all batches complete.
