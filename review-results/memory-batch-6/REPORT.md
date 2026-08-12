# Memory Batch 6 — `src/memory/{note_retrieval,reflector,rerank,curated,compression,context,context_comptroller,tool_signal_sink}/*` Code Review

**Date**: 2026-08-12
**Path**: 8 submodules + 1 top-level file, ~6 000 lines
**Reviewer**: static (security / logic / architecture / quality)

## Module Totals

| Critical | High | Medium | Low | Total |
|---------:|-----:|-------:|----:|------:|
|        0 |    3 |     6 |    5 |   14 |

---

## Findings

### [HIGH] `note_retrieval/mod.rs:200-280` — `NoteFactRetrieval` is `Send + Sync` but holds `Arc<dyn EmbeddingProvider>` whose trait is not `Sync` (depending on impl)
- **Category**: architecture / safety
- **Description**: `NoteFactRetrieval` is `pub struct` and is shared across the agent harness. The `embedder: Option<Arc<dyn EmbeddingProvider>>` field requires the trait to be `Send + Sync` (the `Arc` is, but the inner `dyn` must be). The `EmbeddingProvider` trait declares `async fn` methods; depending on the impl (e.g. a non-Sync HTTP client) the `Arc<dyn EmbeddingProvider>` may not be `Send + Sync`. Today's `DefaultEmbeddingProvider` is OK, but a future provider implementation that wraps a non-Sync client would silently fail to compile in a different generic context, not at the type's definition site.
- **Suggested fix**: Add explicit `Send + Sync` bounds on the `EmbeddingProvider` trait (and `RerankProvider`) and a `static_assertion!` (via `static_assertions` crate or a const fn) that the default impls are `Send + Sync`. Pure hardening.

### [HIGH] `compression/service.rs:401, 635, 699` — three `tokio::spawn` sites with no `JoinSet` and no shutdown
- **Category**: architecture
- **Description**: The compression service fires three kinds of background tasks: LLM pre-compress, post-turn consolidator, and bucket flush. Each is fire-and-forget. A long-running service that is cancelled (e.g. an admin RPC `compression.cancel`) leaves the spawned tasks running and writing raw memories.
- **Suggested fix**: Wrap each spawn site in a `JoinSet` owned by the `CompressionService`. The `cancel` admin RPC aborts the set.

### [HIGH] `curated/store.rs:24, 131` — `tokio::sync::Mutex<()>` is the I/O gate but `read` / `write` methods are `async fn` and hold the gate across awaits
- **Category**: architecture
- **Description**: `curated::Store::write` takes the I/O gate and then calls a `tokio::fs` write. The gate is held for the full write duration. Multiple concurrent writes serialise on the gate; a slow disk (an NFS mount, a 5400 RPM USB drive) holds the gate for many seconds. The semaphore-style pattern would let N writes proceed in parallel.
- **Suggested fix**: Replace `Mutex<()>` with `Semaphore::new(MAX_CONCURRENT_WRITES)` (e.g. 4). The semaphore is held only for the critical section; the actual write proceeds without serialisation.

### [MEDIUM] `reflector/recall_signals.rs:69, 94, 108` — `cap_ref.lock().unwrap().push(row)` in production capture path
- **Category**: logic
- **Description**: Three call sites take a `Mutex<Vec<Row>>` and push without error handling. The mutex is not poisoned in practice (the only failing op is the push), but a `expect("recall_signals lock")` is more explicit.
- **Suggested fix**: Use `.lock().expect("recall_signals: lock poisoned")` for explicit intent; today the `.unwrap()` works but the panic message is uninformative.

### [MEDIUM] `context/compact/tool_aware_chunker.rs` — chunker has no max-segment cap; a 100 MB message yields a single segment
- **Category**: DoS
- **Description**: The chunker parses semantic units and chunks by token count. A user pastes a 100 MB file as one user message; the chunker produces one segment of 100 MB and the post-turn compress loop iterates once over it. The summary call has a 32 K input cap, so the model is given 32 K of the 100 MB; the rest is lost.
- **Suggested fix**: Cap each segment at `MAX_SEGMENT_CHARS = 200_000` (≈ 50 K tokens). The chunker splits a too-large segment into N segments, each with its own summary.

### [MEDIUM] `compression/scheduler.rs:1-100` — `CompressionScheduler` has no per-agent budget; one heavy agent starves the rest
- **Category**: architecture
- **Description**: A single scheduler tick fans out across every agent. A user with 10 agents sees each agent's compression run sequentially (or in a single fan-out), and the agents share the LLM quota. The LLM is the bottleneck; without per-agent budget the slowest agent is the cycle time.
- **Suggested fix**: Per-agent token budget. The scheduler tracks `tokens_used` per agent per cycle and skips an agent when the budget is exhausted.

### [MEDIUM] `rerank/mod.rs:1-100` — `RerankProvider` trait has no retry policy; provider timeouts are caller-side
- **Category**: architecture
- **Description**: Each rerank backend (Cohere, Jina, Voyage, etc.) has a `timeout_ms` config and a single `send` call. A transient 503 from the provider surfaces as a hard error to the caller, which then falls back to the skeleton pack. A 1-second retry on 5xx would dramatically reduce fallback rate.
- **Suggested fix**: Add `MAX_RETRIES = 2` and `RETRY_BACKOFF_MS = 250` at the `RerankProvider` trait level. The backend impls can opt in via a default method.

### [MEDIUM] `context/compact/mod.rs:1-100` — `CompactStrategy` enum matched without a fallthrough
- **Category**: quality
- **Description**: Same shape as `dreaming::strategy.rs::DreamStrategy`. New variants added without updating the match produce a `match _ => unreachable!()` site.
- **Suggested fix**: Same — add an `Other` catch-all that logs a `tracing::warn!` and returns `CompactStrategy::Skip`.

### [MEDIUM] `context_comptroller/comptroller.rs:1-100` — token budget is `u32`; a misconfigured `total_tokens = u32::MAX` causes a multi-million-token envelope
- **Category**: logic
- **Description**: `ComptrollerConfig::total_tokens: u32` is the assembly budget. A misconfigured TOML (`total_tokens = 4294967295`) is parsed as `u32::MAX` and the assembler builds a 4-billion-token envelope — well past the LLM's context window. The downstream LLM rejects the request, but the assemble work (token counting, sorting, hydrating) is wasted.
- **Suggested fix**: Validate `total_tokens` at config load: `if total_tokens > MAX_REASONABLE_TOKENS { return Err(...) }`. A constant `MAX_REASONABLE_TOKENS = 200_000` is well past any real model.

### [LOW] `curated/legacy.rs:1-100` — `curated::legacy::*` has no callers
- **Category**: architecture
- **Description**: The `legacy` submodule was the pre-`SnapshotWriter` API. A grep finds 0 callers; the `pub use` re-exports in `curated/mod.rs` are dead.
- **Suggested fix**: Remove the `pub use` and gate the module with `#[cfg(feature = "legacy-curated")]`. Pure R10 YAGNI.

### [LOW] `rerank/provider.rs:1-100` — `RerankConfig::rerank_model: Option<String>` has no `Default`
- **Category**: quality
- **Description**: Each backend falls back to its own hardcoded default. The `RerankConfig::default()` returns `None`; the actual default is applied at backend-construction time. The shape is correct but the `Option<String>` confuses readers who expect a `String`.
- **Suggested fix**: Either document the `None == backend default` contract or add a `pub const fn model_or_default() -> &str` helper that returns the hardcoded fallback.

### [LOW] `compression/source_prompts.rs:1-100` — `build_compression_prompt` allocates a `Vec<String>` then joins
- **Category**: performance
- **Description**: The function builds a `Vec<String>` per call and `.join("\n")`s. For a 1 K-line message the allocation is 1 K `String`s. A single `String` with `push_str` is cheaper.
- **Suggested fix**: Refactor to a `String` accumulator with `push_str`.

### [LOW] `note_retrieval/scoring.rs:1-100` — MMR lambda is hardcoded at 0.5
- **Category**: architecture
- **Description**: The MMR diversity pass uses `lambda = 0.5` regardless of the agent's preferences. A user who wants pure relevance (e.g. a code-search use case) cannot disable diversity.
- **Suggested fix**: Expose `lambda` as a config knob on `RetrievalScoringConfig`.

### [LOW] `context/enums.rs:1-100` — `CognitiveLayer::Memory` / `Working` / `Episodic` are matched but the doc lists 4 layers
- **Category**: documentation
- **Description**: The enum has 3 variants; the docstring says "4 layers". Drift.
- **Suggested fix**: Reconcile the count.

## Cross-References

- `compression/service.rs:401, 635, 699` and `session_compactor/post_turn_compress.rs:130-180` and `session_search_summary/synthesizer.rs:560-580` — all three are `tokio::spawn` fan-out sites without `JoinSet`. A `join_set::spawn` helper would close all three.
- `curated/store.rs:24, 131` and `extensions/registry.rs:32-50` — both use a sync `Mutex`/`RwLock` around I/O. Replacing with the async equivalent is a sweeping change but the right one.
- `compression/scheduler.rs` and `note_retrieval/mod.rs:200-280` — both fan out across agents. The per-agent budget pattern is the same.

## Strengths

- `context/compact/tool_aware_chunker.rs` correctly distinguishes tool calls from prose and preserves tool-result pairs across chunk boundaries. The semantic-unit parser is the right shape.
- `context_comptroller/comptroller.rs` is the single chokepoint for token-budget enforcement. The downstream `HybridAssembler` reads the same budget.
- `note_retrieval/scoring.rs` keeps the recency + reinforcement + MMR passes in three small named functions. The shape is right; a future recency / MMR config change is a one-liner.
- `reflector/recall_signals.rs` is the only signal-capture site. The `cap_ref` pattern is correct; the `unwrap()` is a code-smell but the panic is informative.
- `rerank/mod.rs` has 6 backend impls behind a single trait. Adding a 7th is a self-contained file.
- `curated/store.rs` uses atomic file writes (temp + fsync + rename) for durability. The shape is the same as `session_resume/writer.rs` — consistent.
