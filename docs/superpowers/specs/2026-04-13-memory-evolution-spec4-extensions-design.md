---
title: "Memory Evolution Spec 4: Pluggable Memory Extensions"
date: 2026-04-13
status: approved
parent: docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md
related_refs:
  - docs/reference/PLUGIN_SYSTEM.md
  - docs/reference/EXTENSION_SYSTEM.md
  - docs/superpowers/specs/2026-04-13-memory-evolution-spec1-capture-hooks-design.md
  - docs/superpowers/specs/2026-04-13-memory-evolution-spec2-reflector-design.md
  - docs/superpowers/specs/2026-04-13-memory-evolution-spec3-fencing-modes-design.md
---

# Spec 4: Pluggable Memory Extensions

Introduce a `MemoryExtension` trait with three hook points (`on_retrieve` / `on_capture` / `produce`) so first-party Aleph code **and** third-party community plugins can enhance memory behaviour through a single, uniform registry. Third-party plugins run over the existing Aleph MCP plugin runtime; first-party extensions implement the trait directly in-process. No new runtime; no new security model beyond what MCP already provides.

---

## 1. Problem

The Spec 1–3 work built the memory stack's core functionality. Beyond that, there is legitimate demand for **third-party community extensions** — plus a clean path for Aleph itself to grow features without bolting them onto core modules:

- **Retrieval augmentation** (A): add items to the envelope from sources Aleph core doesn't know about (e.g., an Obsidian vault, an email archive, a git history).
- **Capture filtering** (C): transform / redact / block raw memories before persistence (e.g., PII redaction, language detection, compression preprocessing).
- **Ingestion sources** (F): produce raw memories on their own schedule from external systems (calendar events, browser history, incoming emails).

Without an extension point, every new capability fights for space in `alephcore` or grows by copy-paste. Plug-ability lets the core stay minimal (R3) and invites community contribution.

Aleph's existing plugin system already supports skills, agents, commands, hooks, and MCP servers — but has no `memory` category. Spec 4 adds that category.

---

## 2. Non-goals

- **Not WASM runtime for memory plugins.** MCP + in-process Rust is the full set. WASM is a possible Spec 5+ addition.
- **Not a new permission model.** MCP's tool-level access already defines what a plugin can touch.
- **Not rate limiting / quotas.** That's Gateway middleware territory, not the extension layer.
- **Not cross-Aleph sync.** The trait supports such a plugin, but Spec 4 doesn't ship one.
- **Not hot-reload.** Restart to reload plugin list.
- **Not a marketplace UI.** Existing `plugin install` CLI suffices.
- **Not a full suite of first-party extensions.** Spec 4 ships zero production features under the trait. One POC extension validates the plumbing; nothing more.

---

## 3. Architecture

### 3.1 Trait

`src/memory/extensions/mod.rs` (new):

```rust
use async_trait::async_trait;
use crate::error::AlephError;
use crate::memory::assembler::envelope::MemoryEnvelope;
use crate::memory::store::raw_memory::RawMemory;

pub struct RetrieveCtx {
    pub agent_id: String,
    pub namespace: crate::memory::namespace::NamespaceScope,
    pub query: String,
    pub session_id: Option<String>,
}

pub struct CaptureCtx {
    pub agent_id: String,
    pub namespace: crate::memory::namespace::NamespaceScope,
    pub session_id: Option<String>,
    /// Source of the raw memory (SessionCompressed, Transcript, PreCompress, …).
    pub source_hint: String,
}

pub struct ProduceCtx {
    pub agent_id: String,
    pub namespace: crate::memory::namespace::NamespaceScope,
    /// Monotonic tick count since Aleph started — lets plugins rate-limit
    /// or batch their own output.
    pub tick: u64,
}

#[derive(Debug, Clone)]
pub enum CaptureDecision {
    Allow,
    Block { reason: String },
}

#[async_trait]
pub trait MemoryExtension: Send + Sync {
    fn name(&self) -> &str;

    async fn on_retrieve(
        &self,
        _ctx: &RetrieveCtx,
        _envelope: &mut MemoryEnvelope,
    ) -> Result<(), AlephError> {
        Ok(())
    }

    async fn on_capture(
        &self,
        _ctx: &CaptureCtx,
        _raw: &mut RawMemory,
    ) -> Result<CaptureDecision, AlephError> {
        Ok(CaptureDecision::Allow)
    }

    async fn produce(
        &self,
        _ctx: &ProduceCtx,
    ) -> Result<Vec<RawMemory>, AlephError> {
        Ok(vec![])
    }
}
```

All three hooks have sensible default no-op implementations, so a plugin only implements what it cares about.

### 3.2 Registry + dispatch (Q3 = hybrid)

`src/memory/extensions/registry.rs`:

```rust
pub struct MemoryExtensionRegistry {
    extensions: Vec<Arc<dyn MemoryExtension>>,
}

impl MemoryExtensionRegistry {
    pub fn register(&mut self, ext: Arc<dyn MemoryExtension>) { ... }

    // on_retrieve: broadcast fan-out; each plugin sees an immutable copy
    // of the core envelope and returns its proposed additions; the registry
    // merges all additions into a new "Extension" slot on the original.
    pub async fn dispatch_on_retrieve(
        &self,
        ctx: &RetrieveCtx,
        envelope: &mut MemoryEnvelope,
    ) -> Result<(), AlephError>;

    // on_capture: chained pipeline, ordered by plugin manifest priority
    // (lower = earlier). Each plugin's output is the next plugin's input.
    // Any Block short-circuits and stops the chain.
    pub async fn dispatch_on_capture(
        &self,
        ctx: &CaptureCtx,
        raw: &mut RawMemory,
    ) -> Result<CaptureDecision, AlephError>;

    // produce: independent concurrent call; scheduler consumes the Vec
    // per-plugin results so one plugin's failure doesn't hide another's
    // successes.
    pub async fn dispatch_produce(
        &self,
        ctx: &ProduceCtx,
    ) -> Vec<(String /* plugin_name */, Result<Vec<RawMemory>, AlephError>)>;
}
```

### 3.3 Data flow

```
┌─ HybridAssembler::assemble → envelope ─┐
                                         │
                                         ▼
                       registry.dispatch_on_retrieve(ctx, &mut envelope)
                                         │
                                         ▼
                               render_with(envelope, Xml)
                                         │
                                         ▼
                           LayerInput::memory_user_message
                                         │
                                         ▼
                                  (into prompt)


┌─ raw_memory producer (Spec 1 hook, gateway transcript, …) ─┐
│                                                            │
│   RawMemory created                                        │
│                                                            │
└──┬──────────────────────────────────────────────────────────┘
   ▼
   registry.dispatch_on_capture(ctx, &mut raw) → Allow | Block
   ▼ (if Allow)
   raw_memory_store.insert_raw_memory(&raw)


┌─ MemoryProducerScheduler tick (every N seconds) ─┐
│                                                  │
│   registry.dispatch_produce(ctx) → Vec<results>  │
│                                                  │
│   for each Ok(raw_memories):                     │
│     for each raw:                                │
│       registry.dispatch_on_capture(ctx, raw)    │  ← produced memories
│       if Allow: raw_memory_store.insert_raw     │     still go through
│                                                  │     on_capture
└──────────────────────────────────────────────────┘
```

The `produce → on_capture → store` composition means filters apply uniformly regardless of who produced the memory (Spec 1 hook, gateway ingestion, or an F-type plugin).

---

## 4. Dual-path implementations

### 4.1 First-party (in-process)

Aleph's own code can implement the trait directly:

```rust
pub struct AutoTaggerExtension { ... }

#[async_trait]
impl MemoryExtension for AutoTaggerExtension {
    fn name(&self) -> &str { "aleph.auto-tagger" }

    async fn on_capture(&self, _ctx: &CaptureCtx, raw: &mut RawMemory) -> ... {
        // ... use LLM to add tags to raw.content metadata
        Ok(CaptureDecision::Allow)
    }
}
```

Registered at server startup alongside MCP adapters — no special handling in the registry.

### 4.2 Third-party (MCP adapter)

`src/memory/extensions/mcp_adapter.rs`:

```rust
pub struct McpMemoryExtension {
    name: String,
    client: Arc<McpClient>,
    priority: i32,
}

#[async_trait]
impl MemoryExtension for McpMemoryExtension {
    fn name(&self) -> &str { &self.name }

    async fn on_retrieve(
        &self,
        ctx: &RetrieveCtx,
        envelope: &mut MemoryEnvelope,
    ) -> Result<(), AlephError> {
        let req = json!({
            "agent_id": ctx.agent_id,
            "query": ctx.query,
            "envelope": envelope, // serde-serialized
        });
        let resp = timeout(
            Duration::from_secs(2),
            self.client.call_tool("memory.on_retrieve", req),
        ).await??;
        let additions: Vec<EnvelopeItem> = serde_json::from_value(resp["additions"].clone())?;
        merge_additions(envelope, additions);
        Ok(())
    }

    // on_capture + produce: analogous shape; JSON in / JSON out.
}
```

The adapter handles:
- JSON serialisation of request (ctx + current state)
- Timeout (per Q4 = 2s / 3s / 30s per hook)
- JSON deserialisation of response
- Error mapping (MCP errors / timeouts → `AlephError`)

---

## 5. MCP method schemas (third-party plugins implement these)

| Method | Request | Response |
|--------|---------|----------|
| `memory.on_retrieve` | `{ agent_id, namespace, query, session_id?, envelope }` | `{ additions: [EnvelopeItem] }` |
| `memory.on_capture` | `{ agent_id, namespace, session_id?, source_hint, raw: RawMemory }` | `{ decision: "allow" \| "block", reason?: string, modified?: RawMemory }` |
| `memory.produce` | `{ agent_id, namespace, tick }` | `{ raw_memories: [RawMemory] }` |

Plugins that don't implement a hook simply return `{}` (interpreted as no-op by the adapter).

---

## 6. Manifest extension

Existing Aleph plugin TOML gains an optional `[memory]` section:

```toml
[plugin]
name = "memory-obsidian"
version = "1.0.0"
type = "mcp"

[memory]
hooks = ["on_retrieve"]
# priority controls on_capture ordering (lower = earlier)
priority = 50
# produce-only fields — ignored if "produce" not in hooks:
produce_interval_seconds = 300
produce_on_capture_timeout = "allow"  # or "block"
```

The `src/extension/` manifest parser learns to recognise `[memory]`. Plugin loader creates an `McpMemoryExtension` and registers it with `MemoryExtensionRegistry`.

---

## 7. Timeouts + failure policies (Q4 = B)

| Hook | Per-plugin timeout | Failure policy |
|------|--------------------|----------------|
| `on_retrieve` | 2 s | Drop that plugin's contribution; warn log; envelope unchanged w.r.t. that plugin |
| `on_capture` | 3 s | Default = `Block { reason: "plugin X timed out" }` (write path fail-safe); manifest can override to `allow` per-plugin |
| `produce` | 30 s | Drop that tick's result; warn log; retry next tick; N consecutive failures → temporarily disable (threshold configurable; Plan picks default) |

Plugin errors never propagate to users. Metrics & logs only.

---

## 8. POC first-party extension

To validate the plumbing, Spec 4 ships **one** tiny first-party extension: `EnvelopeRelevanceFloorExtension`. Implements `on_retrieve` to drop items with `relevance < configured_floor`. Behaviour is a thin, regression-testable affirmation that the dispatch mechanism works end-to-end. Rationale: proves dispatch round-trips before any real plugin ever runs.

This extension is safe to keep in perpetuity or remove once another first-party extension lands.

---

## 9. `produce` scheduler

`src/memory/extensions/scheduler.rs` (new):

Independent tokio task, ticks on a short interval (e.g. 10 s). On each tick:

1. Ask the registry for plugins that declared `produce` in their manifest.
2. For each, check `last_run + interval ≤ now`. If yes, schedule a `dispatch_produce` call for that plugin.
3. Concurrent `dispatch_produce` across eligible plugins; 30 s timeout per plugin.
4. For each `Ok(Vec<RawMemory>)`: route every produced `RawMemory` through `dispatch_on_capture` then `raw_memory_store.insert_raw_memory`. So produced memories are filtered the same way as organic ones.
5. For each `Err`: increment a per-plugin failure counter; if ≥ threshold (Plan sets default, e.g. 5), disable plugin until restart or manual re-enable.

Plan-phase check: if Aleph already has a generic scheduler (cron service), piggyback instead of writing a new one.

---

## 10. Server wiring

At server startup (Plan phase determines exact file):

1. Plugin loader iterates manifest files, constructs `McpMemoryExtension` instances for each plugin with a `[memory]` section, and registers them.
2. First-party extensions (the POC one; future Aleph-built ones) are instantiated directly and registered.
3. `MemoryExtensionRegistry` handle is shared via `Arc` to:
   - `MemoryContextProvider` (for `on_retrieve` after `assemble`)
   - Every raw-memory producer site that currently calls `raw_memory_store.insert_raw_memory` (for `on_capture` interposition)
   - `MemoryProducerScheduler` (for `produce`)

---

## 11. Testing strategy

### 11.1 Unit
- `MemoryExtension` trait default implementations behave as no-ops.
- `MemoryExtensionRegistry::dispatch_on_retrieve`:
  - Zero extensions → envelope unchanged.
  - Two extensions → both contributions merged in an `Extension` slot.
  - One extension times out → the other's contribution still lands.
- `dispatch_on_capture`:
  - Chain order respects priority.
  - `Block` short-circuits the chain.
  - Timeout → configurable block / allow behaviour.
- `dispatch_produce`:
  - Returns per-plugin results.
  - One plugin failure doesn't affect another's success.

### 11.2 Integration
- `tests/memory_extensions_integration.rs`:
  - Register the POC `EnvelopeRelevanceFloorExtension`; run a full retrieve cycle; verify low-relevance items are dropped.
  - A dummy in-proc `ProducerExtension` that returns a canned `RawMemory`; verify the scheduler picks it up, runs `on_capture`, and the memory lands in `raw_memories` table.
  - A dummy in-proc `CaptureFilterExtension` that blocks memories matching a predicate; verify `raw_memories` table does NOT contain the blocked rows.

### 11.3 MCP adapter contract test
- Mock MCP client; verify `McpMemoryExtension` serialises/deserialises the three method envelopes correctly.

---

## 12. Compliance with architectural redlines

| Redline | Check |
|---------|-------|
| R3 Core minimalism | Extensions are out-of-process by default. Core adds a trait + registry + small scheduler. No heavy deps. |
| R8 LLM sovereignty | Extensions modify envelope / raw data, not LLM decisions. |
| R9 Everything is a tool | Plugin enable/disable surfaces through manifest today; `memory_extension_manage` tool is a possible future addition (not in Spec 4 scope). |
| R10 Intelligence in the prompt | Extensions add structured items to the envelope; the same XML fence from Spec 3 surrounds them. No new prompt-level primitives. |

No redline violated.

---

## 13. Open questions (Plan phase)

- **Existing scheduler reuse**: grep for a cron / tick service before authoring `MemoryProducerScheduler`.
- **MCP method schema exactness**: `RetrieveRequest` should include what exactly — budget hint? recent assistant message? Plan picks the minimum that lets plugins do useful work without exposing unnecessary state.
- **on_capture consecutive-failure threshold** default (3? 5? 10?).
- **Per-agent override** of extension enablement: basic support (global on/off) in Spec 4; per-agent selective disable is a future enhancement.
- **Backfill for existing envelopes** (pre-Spec-4): no backfill needed — extensions only see envelopes created after they're registered.
- **Hot-reload**: explicitly deferred (§2 non-goals).

---

## 14. What this unlocks

After Spec 4 lands, enabling/writing a new memory behaviour (third-party OR first-party) is an **additive** change: manifest + MCP server (or a new `impl MemoryExtension`). No churn to core. The roadmap-level vision of "community-enhanced Aleph memory" becomes a real path, and Aleph's own future memory improvements inherit the same extension-native posture.
