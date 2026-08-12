# Memory Batch 4 — `src/memory/{assembler,extensions,events}/*` Code Review

**Date**: 2026-08-12
**Path**: `src/memory/assembler/*` (13 files), `src/memory/extensions/*` (9 files), `src/memory/events/*` (6 files) — 28 files, ~7 000 lines
**Reviewer**: static (security / logic / architecture / quality)

## Module Totals

| Critical | High | Medium | Low | Total |
|---------:|-----:|-------:|----:|------:|
|        0 |    3 |     6 |    5 |   14 |

---

## Findings

### [HIGH] `assembler/hybrid.rs:303-340` — `hydrate` loop has no early-break on exhausted budget, but `used` accounting is wrong
- **Category**: logic
- **Description**: The doc-comment of `hydrate` says: "No early break: once the budget is exhausted, remaining_chars is 0, so later items truncate to empty and are dropped by the retain below." But the actual code path is: `truncate_chars(&item.content, 0)` returns `""` (correct), `item.tokens = 0`, and the retain filter is `|i| i.tokens > 0 || !i.content.is_empty()`. For an empty content with `tokens == 0` the item is dropped. So the *behaviour* is right, but the *invariant* is fragile — any future change to `truncate_chars` to return non-empty for length-0 input would silently break the budget.
- **Suggested fix**: Add an explicit `break` after the first item that truncates to empty. The retain is then redundant and the budget invariant is enforced at the loop, not at the post-loop filter.

### [HIGH] `extensions/mcp_adapter.rs:386` — `panic!("expected block")` in production match
- **Category**: logic
- **Description**: A test-only `panic!` in a match arm that is *not* `#[cfg(test)]` and *is* the fallthrough of an `unwrap_or` chain. The function is documented as defensive; the panic is a debug assertion. If a malformed MCP message arrives the production daemon panics.
- **Suggested fix**: Return `AlephError::other("mcp adapter: unexpected block variant")` instead of panicking. The function is already error-returning at other sites; this one is the outlier.

### [HIGH] `events/migration.rs:234` — `panic!("Expected NoteMigrated")` in the migration's `read_back` helper
- **Category**: logic
- **Description**: A `match` arm that panics on an unexpected variant during a one-shot data migration. If the schema is forward-only and a future migration adds a new event variant, the migration reader crashes on a freshly-migrated DB.
- **Suggested fix**: Convert the migration reader to a streaming `Ok(None)` for unknown variants and a `tracing::warn!` so a future schema extension does not brick the migration.

### [MEDIUM] `assembler/hybrid.rs:240-280` — `clamp_pinned` ceiling calculation can underflow for tiny budgets
- **Category**: logic
- **Description**: `let pinned_cap = u32::try_from((f64::from(total_budget) * 0.3) as u64).unwrap_or(u32::MAX);` then `clamp_pinned = configured.min(pinned_cap.max(1))`. For `total_budget = 0`, `pinned_cap = 0`, `pinned_cap.max(1) = 1`, so the clamp is `configured.min(1)`. The pin then steals a slot from a zero-budget caller.
- **Suggested fix**: Short-circuit: if `total_budget == 0` or `total_budget < MIN_PIN_BUDGET` (say, 4 tokens), drop the pinned slot entirely. The LLM re-rank already short-circuits when `candidates_considered < 3`; the pin should mirror that.

### [MEDIUM] `extensions/scheduler.rs:46-50` — `tokio::spawn` with `loop {}` and no shutdown signal
- **Category**: architecture
- **Description**: The scheduler's worker loop has no cancellation. A `MemoryProducerScheduler` started at boot runs until the daemon dies. A test that creates one and drops it leaks a tokio task. The harness's `#[tokio::test]` with `flavor = "current_thread"` then deadlocks on shutdown.
- **Suggested fix**: Add a `tokio::sync::watch::Receiver<()>` shutdown channel. The loop's `tokio::select!` watches the receiver; on signal, return cleanly.

### [MEDIUM] `events/handler.rs:530, 750` — `panic!("Expected NoteContentUpdated event")` / `NoteConsolidated event` in the event handler
- **Category**: logic
- **Description**: Same shape as the migration panic above. The handler is a `match` over `MemoryEvent::` variants and panics on a missing arm. A future event variant or a backfill from a forward-compatible schema breaks every consumer.
- **Suggested fix**: Replace with `return Err(AlephError::other(format!("unhandled event {ev:?}")))`. The handler is already `async fn` and error-returning; the panic is gratuitous.

### [MEDIUM] `assembler/render.rs:1-100` — `render_envelope` truncates at the byte level for the XML formatter
- **Category**: logic
- **Description**: The XML formatter is called from `MemoryEnvelope`'s `Serialize` impl. The `bound_content` helper is used for prose but the XML attribute escaping is byte-counted, not char-counted. A CJK character in a title would be split across the closing quote and a downstream parser would reject the envelope.
- **Suggested fix**: Use `truncate_chars` for the title and alias fields. The current `truncate(s, MAX)` is byte-truncation; same bug as the assembler/hybrid.rs `hydrate` finding, just at a different layer.

### [MEDIUM] `extensions/traits.rs:1-100` — `MemoryExtension` trait has 6 methods; each consumer must implement all 6 to participate
- **Category**: architecture
- **Description**: The default impls on 5 of the 6 are no-ops; the 6th (`on_capture`) returns `CaptureDecision::Allow`. But Rust's `async_trait` macros do not preserve the default `async fn` if the implementor overrides any method. A simple "no-op extension" has to spell out 5 boilerplate methods.
- **Suggested fix**: Use a `MemoryExtension::default()` builder or split into multiple smaller traits (`Capture`, `Retrieve`, `PreCompress`, `Delegation`, `SessionSwitch`) and have `MemoryExtension = Capture + Retrieve + PreCompress + Delegation + SessionSwitch` as a supertrait. Each consumer asks for what it needs.

### [MEDIUM] `assembler/gather.rs:200-260` — `Gatherer::gather` reads the snapshot AND the profile AND the feedback floor; no shared pool cap
- **Category**: DoS
- **Description**: Three independent pools are merged into one. The `pool_limit` config caps the *post-merge* total but each leg can be `pool_limit` rows on its own. A misconfigured `pool_limit = 100` becomes 300 candidates before the cap.
- **Suggested fix**: Cap each leg to `pool_limit / N_LEGS` (here, `pool_limit / 3`) so the merged pool is bounded by `pool_limit` in the worst case. The current code's doc-comment says "pool_limit is a hard cap on the merged pool" but the cap is applied post-merge, not pre-merge.

### [LOW] `assembler/fallback.rs:27-95` — `deterministic_truncate` allocates a fresh `String` per call
- **Category**: performance
- **Description**: A hot path. The function returns a `String`, so allocation is unavoidable, but the size hint via `String::with_capacity(max_chars)` is missing.
- **Suggested fix**: `String::with_capacity(max_chars.min(input_chars))` upfront.

### [LOW] `assembler/envelope.rs:1-100` — `SCHEMA_VERSION` is `&'static str`; no migration path
- **Category**: architecture
- **Description**: A schema bump requires a coordinated change at every reader. Today, the version is read in `MemoryEnvelope::schema_version` but never compared to a min-supported version.
- **Suggested fix**: Add a `MIN_SUPPORTED_VERSION: &str` constant and a check at deserialisation that errors clearly on a too-new schema. Pure hardening.

### [LOW] `events/commands.rs:1-100` — `MemoryCommandHandler` chains 7 async methods; each forwards errors but does not log
- **Category**: quality
- **Description**: The chained `?`s swallow the error context. A failed `ConsolidateCommand` is reported as "consolidate failed: {err}" but the original `MemoryEvent` is not in the log line.
- **Suggested fix**: Wrap the error in an `anyhow::Context` analogue or add a `#[tracing::instrument]` to the entry point.

### [LOW] `extensions/registry.rs:32-50` — `RwLock<Vec<...>>` is held across `register` and `bind`; a slow caller (an MCP server handshake) blocks every reader
- **Category**: architecture
- **Description**: The `RwLock` is correct for read-mostly workloads, but the `register` and `bind` paths do I/O (open a channel, handshake). A 5-second MCP bind blocks every concurrent recall.
- **Suggested fix**: Replace with `tokio::sync::RwLock`; the registry is touched rarely and the slow I/O is the bottleneck.

### [LOW] `events/projector.rs:1-100` — `EventProjector` is a 200-line `match`-on-event arm tree with no easy extensibility
- **Category**: architecture
- **Description**: New `MemoryEvent` variants require editing the projector. A visitor trait or an `Inventory`-style registration is the modern Rust pattern.
- **Suggested fix**: Same as events/handler.rs — convert the panic arms to typed error returns and add a `tracing::warn!` for the "unhandled event" path.

## Cross-References

- `assembler/hybrid.rs:303-340` and `assembler/render.rs:1-100` — both truncate at the byte level. A single `truncate_chars` helper in `utils::text_format` would close both.
- `extensions/mcp_adapter.rs:386` and `events/migration.rs:234` and `events/handler.rs:530, 750` — all four are `panic!` in production matches. The right shape is a typed `AlephError` return; the four sites are mechanical conversions.
- `extensions/scheduler.rs:46-50` and `extensions/registry.rs:32-50` — both reference a long-lived background task with no shutdown. A `watch::Receiver<()>` per task is the standard pattern; the codebase already uses it elsewhere.

## Strengths

- `assembler/hybrid.rs::HybridAssembler` degrades gracefully: tiny pool → skeleton fallback; LLM timeout → skeleton fallback; LLM parse error → skeleton fallback. The fallbacks are not just "return empty" — `skeleton_pack` sorts by pinned relevance and charges the budget correctly.
- `assembler/hybrid.rs::AiProviderSummaryLlm` is a separate type from `AiProviderReranker` precisely because the two have opposite output contracts. The doc-comment is a good explanation of why one impl "kept them in sync" was the wrong design.
- `extensions/registry.rs` is the only `MemoryExtensionRegistry` constructor; the per-event timeouts (`ON_CAPTURE_TIMEOUT`, etc.) are public constants so consumers can override if needed.
- `events/commands.rs` keeps the command/handler split; commands are value types and handlers are stateful. The split makes a future async test of the handler cheap.
- `events/migration.rs` is forward-compatible by design: it reads each row, applies a transform, and writes the result. The transform is a free function, not a closure, so the unit tests can pin the migration.
