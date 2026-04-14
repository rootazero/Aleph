# Memory Extensions

> Pluggable hook surface for Aleph's memory system. First-party Aleph code and third-party MCP plugins both plug in through the same `MemoryExtension` trait, unified by `MemoryExtensionRegistry`.

## 1. Three hooks

| Hook | When fires | Signature | Purpose |
|------|-----------|-----------|---------|
| `on_retrieve` | After `HybridAssembler::assemble`, before XML rendering | `async fn(&self, ctx: &RetrieveCtx, envelope: &mut MemoryEnvelope) -> Result<()>` | Augment / filter / reorder the retrieved envelope |
| `on_capture` | Before every `RawMemoryStore::insert_raw_memory` | `async fn(&self, ctx: &CaptureCtx, raw: &mut RawMemory) -> Result<CaptureDecision>` | Inspect / redact / block raw memories before persistence |
| `produce` | Dedicated scheduler tick (default 10s) | `async fn(&self, ctx: &ProduceCtx) -> Result<Vec<RawMemory>>` | Produce raw memories from external sources (calendar, email, git, etc.) |

All three have no-op defaults — plugins override only what they need.

## 2. Dispatch semantics

`MemoryExtensionRegistry` applies each hook with a different strategy:

- **`on_retrieve` — sequential broadcast**: every registered extension sees the envelope in registration order. Per-plugin **2-second** timeout. Plugin errors/timeouts drop that plugin's contribution; the core envelope continues.
- **`on_capture` — chained pipeline**: each extension's mutation of `raw` is visible to the next. A `Block` short-circuits the chain. Per-plugin **3-second** timeout. Timeout or error => **`Block` (fail-safe for write path)**.
- **`produce` — per-plugin parallel**: each plugin is called independently; per-plugin **30-second** timeout; results returned as `Vec<(name, Result<Vec<RawMemory>>)>` so the scheduler can track per-plugin failures.

Timeouts and failure policies are defined by constants in `src/memory/extensions/registry.rs`: `ON_RETRIEVE_TIMEOUT`, `ON_CAPTURE_TIMEOUT`, `PRODUCE_TIMEOUT`.

## 3. Dual-path implementation

| Path | How | Performance |
|------|-----|-------------|
| **First-party (in-process)** | Implement `MemoryExtension` directly in a Rust module (e.g., `src/memory/extensions/first_party.rs`) | Zero IPC overhead |
| **Third-party (MCP)** | Write an MCP server exposing `memory.on_retrieve` / `memory.on_capture` / `memory.produce` tool methods; declare `[memory]` in plugin manifest | JSON-RPC round-trip |

Both register to the **same** `MemoryExtensionRegistry` and dispatch uniformly. Error isolation is identical.

## 4. Manifest `[memory]` section

Third-party plugins declare hooks in their TOML manifest:

```toml
[plugin]
name = "memory-obsidian"
version = "1.0.0"
type = "mcp"

[memory]
hooks = ["on_retrieve"]                  # which hooks this plugin implements
priority = 50                            # on_capture chain order (lower = earlier; default 100)
produce_interval_seconds = 300           # only relevant if "produce" is in hooks
on_capture_timeout_action = "block"      # "block" (fail-safe, default) or "allow"
```

Parsed by `src/memory/extensions/manifest.rs::MemoryManifestSection`. The plugin loader registers an `McpMemoryExtension` in the registry when this section is present.

## 5. MCP method schemas

Third-party plugins expose these three MCP tool methods:

| Method | Request | Response |
|--------|---------|----------|
| `memory.on_retrieve` | `{ agent_id, query, session_id?, envelope }` | `{ additions: [EnvelopeItem] }` (optional) |
| `memory.on_capture` | `{ agent_id, session_id?, source_hint, raw }` | `{ decision: "allow"\|"block", reason?, modified?: RawMemory }` |
| `memory.produce` | `{ agent_id, tick }` | `{ raw_memories: [RawMemory] }` |

Plugins that don't implement a hook simply return `{}` — the adapter treats missing fields as no-op.

## 6. Writing a first-party extension

```rust
use crate::memory::extensions::traits::MemoryExtension;
use crate::memory::extensions::types::RetrieveCtx;
use crate::memory::assembler::envelope::MemoryEnvelope;
use async_trait::async_trait;

pub struct MyExtension;

#[async_trait]
impl MemoryExtension for MyExtension {
    fn name(&self) -> &str { "aleph.my_extension" }

    async fn on_retrieve(
        &self,
        _ctx: &RetrieveCtx,
        envelope: &mut MemoryEnvelope,
    ) -> Result<(), AlephError> {
        // Modify envelope here.
        Ok(())
    }
}
```

Register at server startup:

```rust
memory_ext_registry.register(Arc::new(MyExtension));
```

The reference POC implementation is `EnvelopeRelevanceFloorExtension` in `src/memory/extensions/first_party.rs` — drops items with `relevance < floor`. Registered at server startup with `floor = 0.0` (no-op by default).

## 7. Produce scheduler

`MemoryProducerScheduler` is a dedicated tokio task that ticks every `DEFAULT_TICK_SECONDS` (10s). Each tick:

1. Calls `registry.dispatch_produce(&ctx)` → gets `Vec<(plugin_name, Result<Vec<RawMemory>>)>`.
2. For each `Ok(Vec<RawMemory>)`: routes every memory through `insert_with_capture_filter` — so **producer-generated memories still pass through `on_capture`** — before persisting.
3. Failures are logged but don't stop the tick; next tick retries.

Implementation: `src/memory/extensions/scheduler.rs`.

## 8. Capture filter helper

All six production raw-memory producers (from Spec 1 / earlier) now use `insert_with_capture_filter(&store, &registry, &ctx, raw)` instead of direct `store.insert_raw_memory(&raw)`. The helper:

1. Runs `registry.dispatch_on_capture(&ctx, &mut raw)` — the full chain.
2. If result is `Allow`: calls `store.insert_raw_memory(&raw)`.
3. If result is `Block`: logs the reason, does NOT persist.

Implementation: `src/memory/extensions/insert_helper.rs`.

## 9. Testing strategy

Unit tests cover each layer:

- `src/memory/extensions/types.rs` — `CaptureDecision` JSON round-trip
- `src/memory/extensions/traits.rs` — defaults + object-safety
- `src/memory/extensions/registry.rs` — all three dispatch shapes (broadcast / chain / independent)
- `src/memory/extensions/first_party.rs` — POC pruning logic
- `src/memory/extensions/insert_helper.rs` — Allow/Block persistence gating
- `src/memory/extensions/scheduler.rs` — run_once produce → capture → persist
- `src/memory/extensions/mcp_adapter.rs` — JSON schema round-trips with canned caller
- `src/memory/extensions/manifest.rs` — TOML parsing + defaults

Integration tests at `tests/memory_extensions_integration.rs` validate the full pipeline with feature-flag `test-helpers`.

## 10. Non-goals (explicit)

- **No WASM runtime** — MCP + in-process Rust only.
- **No per-plugin permission model** — MCP's existing tool-level access governs what plugins can touch.
- **No rate limiting / quotas** — Gateway middleware territory.
- **No hot-reload** — restart to apply new plugins.

## 11. Related documents

- Design: `docs/superpowers/specs/2026-04-13-memory-evolution-spec4-extensions-design.md`
- Plan: `docs/superpowers/plans/2026-04-13-memory-evolution-spec4-extensions.md`
- Parent roadmap: `docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md`
- Retrieval flow: `docs/reference/memory/RETRIEVAL.md` §15 (pointer)
