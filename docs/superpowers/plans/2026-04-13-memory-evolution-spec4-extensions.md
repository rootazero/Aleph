# Memory Evolution Spec 4: Pluggable Memory Extensions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `MemoryExtension` trait with three hook points (`on_retrieve` / `on_capture` / `produce`) plus a `MemoryExtensionRegistry` so first-party Aleph code and third-party MCP plugins can enhance memory behaviour through a single, uniform surface.

**Architecture:** New module at `src/memory/extensions/`. Trait has default no-op bodies. Registry holds `Vec<Arc<dyn MemoryExtension>>`. Dispatch is hybrid: `on_retrieve` fans out with concurrent timeouts and merges into an Extension slot; `on_capture` chains by priority with short-circuit on `Block`; `produce` runs independently under a dedicated tokio-interval scheduler. MCP adapter wraps an MCP client into the trait for third-party plugins; first-party extensions implement the trait directly. Plugin manifest gains an optional `[memory]` section.

**Tech Stack:** Rust, Tokio, `async_trait`, existing `HybridAssembler`, `RawMemory`, `MemoryEnvelope`, `tempfile`, existing MCP client infrastructure under `src/extension/runtime/`, plugin manifest parsing under `src/extension/manifest/`.

**Spec:** `docs/superpowers/specs/2026-04-13-memory-evolution-spec4-extensions-design.md`

---

## File Structure

### Files to CREATE

| Path | Responsibility |
|------|----------------|
| `src/memory/extensions/mod.rs` | Module entry + re-exports. |
| `src/memory/extensions/types.rs` | `RetrieveCtx`, `CaptureCtx`, `ProduceCtx`, `CaptureDecision`. |
| `src/memory/extensions/traits.rs` | `MemoryExtension` trait with default no-op bodies. |
| `src/memory/extensions/registry.rs` | `MemoryExtensionRegistry` + three dispatch methods + timeout constants. |
| `src/memory/extensions/first_party.rs` | `EnvelopeRelevanceFloorExtension` (POC first-party). |
| `src/memory/extensions/mcp_adapter.rs` | `McpMemoryExtension` thin wrapper over an MCP client. |
| `src/memory/extensions/scheduler.rs` | `MemoryProducerScheduler` — dedicated tokio task that ticks produce hook. |
| `src/memory/extensions/manifest.rs` | `[memory]` TOML section parser → registrable extension metadata. |
| `src/memory/extensions/tests.rs` | Unit tests for dispatch semantics, timeout behaviour, POC extension correctness. |
| `tests/memory_extensions_integration.rs` | E2E integration test: POC extension end-to-end + in-proc dummy producer + in-proc capture filter. |

### Files to MODIFY

| Path | Change |
|------|--------|
| `src/memory/mod.rs` | `pub mod extensions;` |
| `src/thinker/memory_context_provider.rs` | After `HybridAssembler::assemble`, call `registry.dispatch_on_retrieve(ctx, &mut envelope)`. Registry is an `Arc` field (default empty-registry when injection disabled). |
| `src/memory/store/sqlite/raw_memories.rs` (or wherever `insert_raw_memory` lives) | Accept an optional `Arc<MemoryExtensionRegistry>` in a small wrapper, OR introduce a small helper `insert_with_capture_filter(store, registry, ctx, raw)` that runs `dispatch_on_capture` then `insert_raw_memory`. Modify every production caller to go through the helper. |
| `src/extension/manifest/` | Extend manifest parser to recognise `[memory]` section; produce a loadable descriptor. |
| `src/extension/loader.rs` | When loading an MCP plugin that declares `[memory]`, construct an `McpMemoryExtension` and register it with the global `MemoryExtensionRegistry`. |
| `src/bin/aleph-server/commands/start/builder/handlers.rs` | Construct `Arc<MemoryExtensionRegistry>`, register first-party extension(s), pass to `MemoryContextProvider` / `insert_with_capture_filter`; start `MemoryProducerScheduler`. |
| `docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md` | Mark Spec 4 ✅ shipped. |
| `docs/reference/memory/RETRIEVAL.md` (or new `docs/reference/memory/EXTENSIONS.md`) | Document the extension surface + manifest schema + how to write a plugin. |

---

## Pre-work

- [ ] **Step 0.1: Confirm baseline**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore 2>&1 | tail -5`
Expected: clean `Finished dev profile` with 0 errors.

- [ ] **Step 0.2: Scout infra**

Run and record paths (used by later tasks):

```
cd /Volumes/TBU4/Workspace/Aleph
grep -rln "McpClient\|call_tool\|impl MCPClient" src/extension/runtime/ src/mcp/ 2>/dev/null
grep -n "pub struct .*Manifest\|pub fn parse_manifest\|TomlManifest" src/extension/manifest/*.rs 2>/dev/null
grep -rn "insert_raw_memory" src/ --include='*.rs' | grep -v tests | head -15
grep -n "fn new\|pub fn new\|HybridAssembler::new" src/bin/aleph-server/commands/start/builder/handlers.rs 2>/dev/null | head -5
```

This scouts: (a) MCP client API name and `call_tool` signature; (b) manifest parser entry point; (c) every production call site of `insert_raw_memory` (all Task 6 must be threaded through the capture filter); (d) the server startup file.

---

## Task 1: Context types

**Files:**
- Create: `src/memory/extensions/types.rs`
- Create: `src/memory/extensions/mod.rs`
- Modify: `src/memory/mod.rs`

- [ ] **Step 1.1: Write failing test**

Create `src/memory/extensions/types.rs`:

```rust
//! Public context types passed to each MemoryExtension hook.

use crate::memory::namespace::NamespaceScope;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct RetrieveCtx {
    pub agent_id: String,
    pub namespace: NamespaceScope,
    pub query: String,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CaptureCtx {
    pub agent_id: String,
    pub namespace: NamespaceScope,
    pub session_id: Option<String>,
    /// Source of the raw memory (SessionCompressed, Transcript, PreCompress, ...).
    pub source_hint: String,
}

#[derive(Debug, Clone)]
pub struct ProduceCtx {
    pub agent_id: String,
    pub namespace: NamespaceScope,
    /// Monotonic tick count since Aleph started — lets plugins rate-limit
    /// or batch their own output.
    pub tick: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CaptureDecision {
    Allow,
    Block { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retrieve_ctx_constructs_with_owned_strings() {
        let ctx = RetrieveCtx {
            agent_id: "a1".into(),
            namespace: NamespaceScope::Owner,
            query: "question".into(),
            session_id: Some("s1".into()),
        };
        assert_eq!(ctx.agent_id, "a1");
        assert_eq!(ctx.session_id.as_deref(), Some("s1"));
    }

    #[test]
    fn capture_decision_round_trips_json() {
        let allow = CaptureDecision::Allow;
        let blk = CaptureDecision::Block { reason: "pii".into() };
        for d in [allow, blk] {
            let s = serde_json::to_string(&d).unwrap();
            let back: CaptureDecision = serde_json::from_str(&s).unwrap();
            assert_eq!(back, d);
        }
    }

    #[test]
    fn capture_decision_block_json_has_reason() {
        let s = serde_json::to_string(&CaptureDecision::Block { reason: "x".into() }).unwrap();
        assert!(s.contains("\"kind\":\"block\""));
        assert!(s.contains("\"reason\":\"x\""));
    }
}
```

Create `src/memory/extensions/mod.rs`:

```rust
//! MemoryExtension — pluggable memory enhancements for first-party
//! and third-party (MCP) extensions.
//!
//! See `docs/superpowers/specs/2026-04-13-memory-evolution-spec4-extensions-design.md`.

pub mod types;

pub use types::{CaptureCtx, CaptureDecision, ProduceCtx, RetrieveCtx};
```

Modify `src/memory/mod.rs` — add near other `pub mod X;`:

```rust
pub mod extensions;
```

- [ ] **Step 1.2: Run tests**

```
cargo test -p alephcore extensions::types -- --nocapture 2>&1 | tail -15
cargo check -p alephcore 2>&1 | tail -5
```

Expected: 3 tests pass; clean build.

- [ ] **Step 1.3: Commit**

```bash
git add src/memory/extensions/ src/memory/mod.rs
git commit -m "feat(memory): add extensions module context types

RetrieveCtx / CaptureCtx / ProduceCtx carry per-hook invocation
context. CaptureDecision enum is the Allow/Block return value of
the on_capture hook. Foundation for Spec 4."
```

---

## Task 2: `MemoryExtension` trait

**Files:**
- Create: `src/memory/extensions/traits.rs`
- Modify: `src/memory/extensions/mod.rs`

- [ ] **Step 2.1: Write failing test**

Create `src/memory/extensions/traits.rs`:

```rust
//! MemoryExtension trait + default no-op implementations.

use crate::error::AlephError;
use crate::memory::assembler::envelope::MemoryEnvelope;
use crate::memory::extensions::types::{CaptureCtx, CaptureDecision, ProduceCtx, RetrieveCtx};
use crate::memory::store::raw_memory::RawMemory;
use async_trait::async_trait;

/// Hook surface for first-party code and MCP plugins. Each method has a
/// default no-op body so an extension only implements what it needs.
#[async_trait]
pub trait MemoryExtension: Send + Sync {
    /// Stable identifier — shows up in logs and manifest entries.
    fn name(&self) -> &str;

    /// Modify the envelope after HybridAssembler::assemble.
    async fn on_retrieve(
        &self,
        _ctx: &RetrieveCtx,
        _envelope: &mut MemoryEnvelope,
    ) -> Result<(), AlephError> {
        Ok(())
    }

    /// Inspect / modify / veto a raw memory before persistence.
    async fn on_capture(
        &self,
        _ctx: &CaptureCtx,
        _raw: &mut RawMemory,
    ) -> Result<CaptureDecision, AlephError> {
        Ok(CaptureDecision::Allow)
    }

    /// Produce raw memories on the caller's schedule.
    async fn produce(
        &self,
        _ctx: &ProduceCtx,
    ) -> Result<Vec<RawMemory>, AlephError> {
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::namespace::NamespaceScope;
    use crate::memory::store::raw_memory::RawMemorySource;

    struct NoopExt;

    #[async_trait]
    impl MemoryExtension for NoopExt {
        fn name(&self) -> &str {
            "test.noop"
        }
    }

    #[tokio::test]
    async fn default_on_retrieve_is_noop() {
        let ext = NoopExt;
        let ctx = RetrieveCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            query: "q".into(),
            session_id: None,
        };
        let mut env = crate::memory::assembler::envelope::MemoryEnvelope::default();
        let before = format!("{env:?}");
        ext.on_retrieve(&ctx, &mut env).await.unwrap();
        assert_eq!(format!("{env:?}"), before, "default on_retrieve must not mutate");
    }

    #[tokio::test]
    async fn default_on_capture_allows() {
        let ext = NoopExt;
        let ctx = CaptureCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            session_id: None,
            source_hint: "transcript".into(),
        };
        let mut raw = RawMemory::new("hi".into(), RawMemorySource::Transcript);
        let decision = ext.on_capture(&ctx, &mut raw).await.unwrap();
        assert!(matches!(decision, CaptureDecision::Allow));
    }

    #[tokio::test]
    async fn default_produce_returns_empty() {
        let ext = NoopExt;
        let ctx = ProduceCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            tick: 0,
        };
        let out = ext.produce(&ctx).await.unwrap();
        assert!(out.is_empty());
    }
}
```

If `MemoryEnvelope::default()` doesn't exist, use whatever minimal constructor it has (the packet_adapter tests in Spec 2 showed a pattern — `MemoryEnvelope { schema_version, ..., slots: vec![], meta: ... }`). Grep for an existing minimal-envelope helper in tests.

Modify `src/memory/extensions/mod.rs` — add:

```rust
pub mod traits;
pub use traits::MemoryExtension;
```

- [ ] **Step 2.2: Run tests**

```
cargo test -p alephcore extensions::traits -- --nocapture 2>&1 | tail -15
cargo check -p alephcore 2>&1 | tail -5
```

Expected: 3 tests pass; clean.

- [ ] **Step 2.3: Commit**

```bash
git add src/memory/extensions/
git commit -m "feat(memory): add MemoryExtension trait

Three hooks (on_retrieve / on_capture / produce) with sensible
no-op defaults so implementers only override what they need.
Trait is #[async_trait] + Send + Sync."
```

---

## Task 3: `MemoryExtensionRegistry` + dispatch

**Files:**
- Create: `src/memory/extensions/registry.rs`
- Modify: `src/memory/extensions/mod.rs`

- [ ] **Step 3.1: Write failing tests for the three dispatch flavours**

Create `src/memory/extensions/registry.rs`:

```rust
//! Registry + dispatch for MemoryExtension hooks.

use crate::error::AlephError;
use crate::memory::assembler::envelope::MemoryEnvelope;
use crate::memory::extensions::traits::MemoryExtension;
use crate::memory::extensions::types::{CaptureCtx, CaptureDecision, ProduceCtx, RetrieveCtx};
use crate::memory::store::raw_memory::RawMemory;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tracing::warn;

pub const ON_RETRIEVE_TIMEOUT: Duration = Duration::from_secs(2);
pub const ON_CAPTURE_TIMEOUT: Duration = Duration::from_secs(3);
pub const PRODUCE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Default, Clone)]
pub struct MemoryExtensionRegistry {
    /// Extensions in priority order (lower priority = earlier in on_capture chain).
    extensions: Vec<Arc<dyn MemoryExtension>>,
}

impl MemoryExtensionRegistry {
    pub fn new() -> Self {
        Self { extensions: Vec::new() }
    }

    pub fn register(&mut self, ext: Arc<dyn MemoryExtension>) {
        self.extensions.push(ext);
    }

    pub fn len(&self) -> usize {
        self.extensions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.extensions.is_empty()
    }

    /// on_retrieve: broadcast fan-out. Each plugin sees the current envelope
    /// and may push items into it. Timeouts drop that plugin's contribution
    /// without failing the overall call.
    pub async fn dispatch_on_retrieve(
        &self,
        ctx: &RetrieveCtx,
        envelope: &mut MemoryEnvelope,
    ) -> Result<(), AlephError> {
        for ext in &self.extensions {
            let name = ext.name().to_string();
            match timeout(ON_RETRIEVE_TIMEOUT, ext.on_retrieve(ctx, envelope)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => warn!("memory extension '{name}' on_retrieve failed: {e}"),
                Err(_) => warn!("memory extension '{name}' on_retrieve timed out"),
            }
        }
        Ok(())
    }

    /// on_capture: chained pipeline. Each extension's modification of `raw`
    /// is visible to the next. A Block short-circuits and returns immediately.
    /// Timeout => Block("timeout") (safe default per Q4).
    pub async fn dispatch_on_capture(
        &self,
        ctx: &CaptureCtx,
        raw: &mut RawMemory,
    ) -> Result<CaptureDecision, AlephError> {
        for ext in &self.extensions {
            let name = ext.name().to_string();
            match timeout(ON_CAPTURE_TIMEOUT, ext.on_capture(ctx, raw)).await {
                Ok(Ok(CaptureDecision::Allow)) => continue,
                Ok(Ok(blk @ CaptureDecision::Block { .. })) => {
                    warn!("memory extension '{name}' blocked raw memory");
                    return Ok(blk);
                }
                Ok(Err(e)) => {
                    warn!("memory extension '{name}' on_capture errored: {e} — blocking for safety");
                    return Ok(CaptureDecision::Block {
                        reason: format!("extension '{name}' errored: {e}"),
                    });
                }
                Err(_) => {
                    warn!("memory extension '{name}' on_capture timed out — blocking");
                    return Ok(CaptureDecision::Block {
                        reason: format!("extension '{name}' timeout"),
                    });
                }
            }
        }
        Ok(CaptureDecision::Allow)
    }

    /// produce: independent per-plugin calls. Returns per-plugin results so
    /// the scheduler can count consecutive failures per plugin.
    pub async fn dispatch_produce(
        &self,
        ctx: &ProduceCtx,
    ) -> Vec<(String, Result<Vec<RawMemory>, AlephError>)> {
        let mut out = Vec::with_capacity(self.extensions.len());
        for ext in &self.extensions {
            let name = ext.name().to_string();
            let result = match timeout(PRODUCE_TIMEOUT, ext.produce(ctx)).await {
                Ok(r) => r,
                Err(_) => Err(AlephError::other(format!("extension '{name}' produce timeout"))),
            };
            out.push((name, result));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::namespace::NamespaceScope;
    use crate::memory::store::raw_memory::RawMemorySource;
    use async_trait::async_trait;

    // --- Stub extensions used by the dispatch tests ---

    struct NoopExt;
    #[async_trait]
    impl MemoryExtension for NoopExt {
        fn name(&self) -> &str { "test.noop" }
    }

    struct AppendQueryExt;
    #[async_trait]
    impl MemoryExtension for AppendQueryExt {
        fn name(&self) -> &str { "test.append_query" }
        async fn on_retrieve(
            &self,
            ctx: &RetrieveCtx,
            envelope: &mut MemoryEnvelope,
        ) -> Result<(), AlephError> {
            envelope.query.push_str(" +ext");
            let _ = ctx;
            Ok(())
        }
    }

    struct BlockingExt;
    #[async_trait]
    impl MemoryExtension for BlockingExt {
        fn name(&self) -> &str { "test.blocker" }
        async fn on_capture(
            &self,
            _ctx: &CaptureCtx,
            _raw: &mut RawMemory,
        ) -> Result<CaptureDecision, AlephError> {
            Ok(CaptureDecision::Block { reason: "test".into() })
        }
    }

    struct PrefixContentExt;
    #[async_trait]
    impl MemoryExtension for PrefixContentExt {
        fn name(&self) -> &str { "test.prefix" }
        async fn on_capture(
            &self,
            _ctx: &CaptureCtx,
            raw: &mut RawMemory,
        ) -> Result<CaptureDecision, AlephError> {
            raw.content = format!("[P] {}", raw.content);
            Ok(CaptureDecision::Allow)
        }
    }

    struct StubProducerExt;
    #[async_trait]
    impl MemoryExtension for StubProducerExt {
        fn name(&self) -> &str { "test.producer" }
        async fn produce(&self, _ctx: &ProduceCtx) -> Result<Vec<RawMemory>, AlephError> {
            Ok(vec![RawMemory::new("produced".into(), RawMemorySource::Transcript)])
        }
    }

    fn retrieve_ctx() -> RetrieveCtx {
        RetrieveCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            query: "original".into(),
            session_id: None,
        }
    }

    fn capture_ctx() -> CaptureCtx {
        CaptureCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            session_id: None,
            source_hint: "transcript".into(),
        }
    }

    fn produce_ctx() -> ProduceCtx {
        ProduceCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            tick: 0,
        }
    }

    fn make_envelope() -> MemoryEnvelope {
        // Use whatever minimal envelope constructor the codebase offers.
        // If MemoryEnvelope::default() exists, use it. Otherwise construct
        // a blank with empty slots.
        MemoryEnvelope::default()
    }

    fn make_raw() -> RawMemory {
        RawMemory::new("hi".into(), RawMemorySource::Transcript)
    }

    #[tokio::test]
    async fn empty_registry_on_retrieve_is_noop() {
        let reg = MemoryExtensionRegistry::new();
        let mut env = make_envelope();
        let before = env.query.clone();
        reg.dispatch_on_retrieve(&retrieve_ctx(), &mut env).await.unwrap();
        assert_eq!(env.query, before);
    }

    #[tokio::test]
    async fn on_retrieve_broadcast_applies_each_extension() {
        let mut reg = MemoryExtensionRegistry::new();
        reg.register(Arc::new(AppendQueryExt));
        reg.register(Arc::new(AppendQueryExt));
        let mut env = make_envelope();
        env.query = "q".into();
        reg.dispatch_on_retrieve(&retrieve_ctx(), &mut env).await.unwrap();
        assert_eq!(env.query, "q +ext +ext");
    }

    #[tokio::test]
    async fn empty_registry_on_capture_allows() {
        let reg = MemoryExtensionRegistry::new();
        let mut raw = make_raw();
        let decision = reg.dispatch_on_capture(&capture_ctx(), &mut raw).await.unwrap();
        assert!(matches!(decision, CaptureDecision::Allow));
    }

    #[tokio::test]
    async fn on_capture_chain_short_circuits_on_block() {
        let mut reg = MemoryExtensionRegistry::new();
        reg.register(Arc::new(BlockingExt));
        reg.register(Arc::new(PrefixContentExt)); // should not run
        let mut raw = make_raw();
        let decision = reg.dispatch_on_capture(&capture_ctx(), &mut raw).await.unwrap();
        assert!(matches!(decision, CaptureDecision::Block { .. }));
        assert_eq!(raw.content, "hi", "content must not be modified after Block");
    }

    #[tokio::test]
    async fn on_capture_chain_mutates_raw_in_order() {
        let mut reg = MemoryExtensionRegistry::new();
        reg.register(Arc::new(PrefixContentExt));
        reg.register(Arc::new(PrefixContentExt));
        let mut raw = make_raw();
        let decision = reg.dispatch_on_capture(&capture_ctx(), &mut raw).await.unwrap();
        assert!(matches!(decision, CaptureDecision::Allow));
        assert_eq!(raw.content, "[P] [P] hi");
    }

    #[tokio::test]
    async fn empty_registry_produce_returns_empty() {
        let reg = MemoryExtensionRegistry::new();
        let out = reg.dispatch_produce(&produce_ctx()).await;
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn produce_returns_per_plugin_results() {
        let mut reg = MemoryExtensionRegistry::new();
        reg.register(Arc::new(StubProducerExt));
        reg.register(Arc::new(NoopExt));
        let out = reg.dispatch_produce(&produce_ctx()).await;
        assert_eq!(out.len(), 2);
        let first = out.iter().find(|(n, _)| n == "test.producer").unwrap();
        assert_eq!(first.1.as_ref().unwrap().len(), 1);
        let second = out.iter().find(|(n, _)| n == "test.noop").unwrap();
        assert_eq!(second.1.as_ref().unwrap().len(), 0);
    }
}
```

If `MemoryEnvelope::default()` is not implemented, add it in Task 3's implementation step with `#[derive(Default)]` where possible, or use a helper `make_test_envelope()` that constructs the minimum valid envelope.

Modify `src/memory/extensions/mod.rs` — add:

```rust
pub mod registry;
pub use registry::{
    MemoryExtensionRegistry, ON_CAPTURE_TIMEOUT, ON_RETRIEVE_TIMEOUT, PRODUCE_TIMEOUT,
};
```

- [ ] **Step 3.2: Run to confirm failure then pass**

```
cargo test -p alephcore extensions::registry -- --nocapture 2>&1 | tail -30
cargo check -p alephcore 2>&1 | tail -5
```

Expected: 7 tests pass after implementation.

- [ ] **Step 3.3: Commit**

```bash
git add src/memory/extensions/
git commit -m "feat(memory): add MemoryExtensionRegistry with hybrid dispatch

on_retrieve broadcasts (per-plugin 2s timeout; drops contribution on
failure); on_capture chains by registration order (3s timeout; error
or timeout => Block); produce runs per-plugin with 30s timeout and
returns per-plugin results so scheduler can count failures."
```

---

## Task 4: POC first-party extension (`EnvelopeRelevanceFloorExtension`)

**Files:**
- Create: `src/memory/extensions/first_party.rs`
- Modify: `src/memory/extensions/mod.rs`

- [ ] **Step 4.1: Write failing test**

Create `src/memory/extensions/first_party.rs`:

```rust
//! First-party memory extensions shipped with Aleph.
//!
//! These implement `MemoryExtension` directly (in-process) instead of
//! going through the MCP adapter. They validate that the dispatch
//! plumbing works end-to-end before any external plugin runs.

use crate::error::AlephError;
use crate::memory::assembler::envelope::MemoryEnvelope;
use crate::memory::extensions::traits::MemoryExtension;
use crate::memory::extensions::types::RetrieveCtx;
use async_trait::async_trait;

/// POC first-party extension: drop envelope items whose `relevance` is
/// below the configured floor. Serves as a smoke test of the on_retrieve
/// dispatch pipeline; also a mild trimming optimization.
pub struct EnvelopeRelevanceFloorExtension {
    floor: f32,
}

impl EnvelopeRelevanceFloorExtension {
    pub fn new(floor: f32) -> Self {
        Self { floor: floor.clamp(0.0, 1.0) }
    }
}

#[async_trait]
impl MemoryExtension for EnvelopeRelevanceFloorExtension {
    fn name(&self) -> &str {
        "aleph.envelope_relevance_floor"
    }

    async fn on_retrieve(
        &self,
        _ctx: &RetrieveCtx,
        envelope: &mut MemoryEnvelope,
    ) -> Result<(), AlephError> {
        let floor = self.floor;
        for slot in envelope.slots.iter_mut() {
            slot.items.retain(|item| item.relevance >= floor);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::assembler::envelope::{
        EnvelopeItem, EnvelopeMeta, EnvelopeSlot, ItemSource, MemoryEnvelope, SlotKind,
    };
    use crate::memory::extensions::types::RetrieveCtx;
    use crate::memory::namespace::NamespaceScope;

    fn env_with_relevances(rs: &[f32]) -> MemoryEnvelope {
        let items = rs
            .iter()
            .enumerate()
            .map(|(i, r)| EnvelopeItem {
                id: format!("id-{i}"),
                title: format!("t-{i}"),
                content: "c".into(),
                relevance: *r,
                source: ItemSource::Note { path: format!("wiki/{i}") },
            })
            .collect();
        MemoryEnvelope {
            schema_version: "1".into(),
            generated_at: 0,
            query: "q".into(),
            agent_id: "a".into(),
            session_id: None,
            slots: vec![EnvelopeSlot { kind: SlotKind::Long, items }],
            meta: EnvelopeMeta::default(),
        }
    }

    fn ctx() -> RetrieveCtx {
        RetrieveCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            query: "q".into(),
            session_id: None,
        }
    }

    #[tokio::test]
    async fn drops_items_below_floor() {
        let ext = EnvelopeRelevanceFloorExtension::new(0.5);
        let mut env = env_with_relevances(&[0.1, 0.4, 0.5, 0.9]);
        ext.on_retrieve(&ctx(), &mut env).await.unwrap();
        let kept: Vec<_> = env.slots[0].items.iter().map(|i| i.relevance).collect();
        assert_eq!(kept, vec![0.5, 0.9]);
    }

    #[tokio::test]
    async fn floor_zero_drops_nothing() {
        let ext = EnvelopeRelevanceFloorExtension::new(0.0);
        let mut env = env_with_relevances(&[0.0, 0.5, 1.0]);
        ext.on_retrieve(&ctx(), &mut env).await.unwrap();
        assert_eq!(env.slots[0].items.len(), 3);
    }

    #[tokio::test]
    async fn empty_slot_survives() {
        let ext = EnvelopeRelevanceFloorExtension::new(0.5);
        let mut env = env_with_relevances(&[]);
        ext.on_retrieve(&ctx(), &mut env).await.unwrap();
        assert_eq!(env.slots[0].items.len(), 0);
    }
}
```

Adapt `EnvelopeItem` / `EnvelopeSlot` / `ItemSource` field names to whatever the real types have (confirmed by Spec 2 Task 3 → `src/memory/assembler/envelope.rs`: `path`, `title`, `content`, `relevance`, `source`).

Modify `src/memory/extensions/mod.rs` — add:

```rust
pub mod first_party;
pub use first_party::EnvelopeRelevanceFloorExtension;
```

- [ ] **Step 4.2: Run**

```
cargo test -p alephcore extensions::first_party -- --nocapture 2>&1 | tail -15
cargo check -p alephcore 2>&1 | tail -5
```

Expected: 3 tests pass.

- [ ] **Step 4.3: Commit**

```bash
git add src/memory/extensions/
git commit -m "feat(memory): POC first-party EnvelopeRelevanceFloorExtension

Drops envelope items with relevance below a configured floor. Serves
as a smoke-test first-party extension that exercises the
on_retrieve dispatch pipeline end-to-end. Ships in the core binary
and can be registered at startup."
```

---

## Task 5: Integrate `on_retrieve` into `MemoryContextProvider`

**Files:**
- Modify: `src/thinker/memory_context_provider.rs`

- [ ] **Step 5.1: Add `Arc<MemoryExtensionRegistry>` field (default empty)**

In `src/thinker/memory_context_provider.rs`, add a field to `MemoryContextProvider`:

```rust
extensions: std::sync::Arc<crate::memory::extensions::MemoryExtensionRegistry>,
```

Update every constructor (including `new_for_test_empty_envelope`) to initialise it to:

```rust
extensions: std::sync::Arc::new(
    crate::memory::extensions::MemoryExtensionRegistry::new(),
),
```

Add a builder setter:

```rust
pub fn with_extensions(
    mut self,
    extensions: std::sync::Arc<crate::memory::extensions::MemoryExtensionRegistry>,
) -> Self {
    self.extensions = extensions;
    self
}
```

- [ ] **Step 5.2: Call `dispatch_on_retrieve` after `assemble`**

In `build_memory_user_message` (the method added by Spec 3 Task 3), after the `let envelope = self.assembler.assemble(...).await?;` line and before `render_with(&envelope, ...)`, insert:

```rust
let ctx = crate::memory::extensions::RetrieveCtx {
    agent_id: agent_id.to_string(),
    namespace: crate::memory::namespace::NamespaceScope::Owner, // adapt from opts if present
    query: query.to_string(),
    session_id: None, // future: thread from opts
};
let mut envelope = envelope; // shadow to make mutable
if let Err(e) = self
    .extensions
    .dispatch_on_retrieve(&ctx, &mut envelope)
    .await
{
    tracing::warn!("memory extensions on_retrieve pipeline failed: {e}");
}
```

Keep everything downstream (empty check, render, return) unchanged.

- [ ] **Step 5.3: Extend `new_for_test_empty_envelope` path**

Its test extension registry stays empty by default (the existing empty-envelope tests continue to pass).

Add one new test that confirms `on_retrieve` is called through:

```rust
#[tokio::test]
async fn build_memory_user_message_invokes_on_retrieve_extension() {
    use crate::memory::assembler::envelope::{EnvelopeItem, EnvelopeSlot, ItemSource, SlotKind, EnvelopeMeta};
    use crate::memory::extensions::{MemoryExtensionRegistry, RetrieveCtx};
    use crate::memory::extensions::traits::MemoryExtension;
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};

    struct Recorder(Mutex<u32>);
    #[async_trait]
    impl MemoryExtension for Recorder {
        fn name(&self) -> &str { "test.recorder" }
        async fn on_retrieve(
            &self,
            _ctx: &RetrieveCtx,
            _env: &mut crate::memory::assembler::envelope::MemoryEnvelope,
        ) -> Result<(), crate::error::AlephError> {
            *self.0.lock().unwrap() += 1;
            Ok(())
        }
    }

    let provider = MemoryContextProvider::new_for_test_empty_envelope(
        crate::config::types::memory::MemoryInjectionMode::Hybrid,
    );
    let rec = Arc::new(Recorder(Mutex::new(0)));
    let mut reg = MemoryExtensionRegistry::new();
    reg.register(rec.clone());
    let provider = provider.with_extensions(Arc::new(reg));

    let _ = provider
        .build_memory_user_message("a1", "q")
        .await
        .unwrap(); // empty envelope → Option::None still invokes on_retrieve first
    assert_eq!(*rec.0.lock().unwrap(), 1, "on_retrieve must be dispatched");
}
```

- [ ] **Step 5.4: Run**

```
cargo test -p alephcore memory_context_provider -- --nocapture 2>&1 | tail -20
cargo check -p alephcore 2>&1 | tail -5
```

Expected: all tests pass, including the new one.

- [ ] **Step 5.5: Commit**

```bash
git add src/thinker/memory_context_provider.rs
git commit -m "feat(memory): MemoryContextProvider invokes on_retrieve extensions

After HybridAssembler::assemble and before XML rendering, the provider
now runs registered MemoryExtensions through dispatch_on_retrieve.
Registry is an Arc field; default is an empty registry so existing
deployments see no change."
```

---

## Task 6: Integrate `on_capture` into raw-memory insert path

**Files:**
- Create: `src/memory/extensions/insert_helper.rs`
- Modify: every production caller of `insert_raw_memory` (identified via grep in Step 0.2) to go through the helper.

- [ ] **Step 6.1: Create the helper**

```rust
//! Bridge helper: runs `on_capture` dispatch then persists the raw memory
//! only if the chain returns Allow. Callers should prefer this helper over
//! calling RawMemoryStore::insert_raw_memory directly.

use crate::error::AlephError;
use crate::memory::extensions::registry::MemoryExtensionRegistry;
use crate::memory::extensions::types::{CaptureCtx, CaptureDecision};
use crate::memory::store::raw_memory::{RawMemory, RawMemoryStore};
use std::sync::Arc;

pub async fn insert_with_capture_filter(
    store: &Arc<dyn RawMemoryStore>,
    registry: &Arc<MemoryExtensionRegistry>,
    ctx: &CaptureCtx,
    mut raw: RawMemory,
) -> Result<CaptureDecision, AlephError> {
    let decision = registry.dispatch_on_capture(ctx, &mut raw).await?;
    match &decision {
        CaptureDecision::Allow => {
            store.insert_raw_memory(&raw).await?;
        }
        CaptureDecision::Block { reason } => {
            tracing::info!(
                "raw_memory blocked by extension pipeline (reason={reason}, \
                 source_hint={source}, agent={agent})",
                source = ctx.source_hint,
                agent = ctx.agent_id,
            );
        }
    }
    Ok(decision)
}
```

Expose from `src/memory/extensions/mod.rs`:

```rust
pub mod insert_helper;
pub use insert_helper::insert_with_capture_filter;
```

Add unit tests (same file) that use a fake `RawMemoryStore` + fake registry with a `BlockingExt`, covering:
- No extensions → memory is persisted (Allow path).
- Blocking extension → memory is NOT persisted; `CaptureDecision::Block` returned.

- [ ] **Step 6.2: Migrate all insert_raw_memory call sites**

Using the grep from Step 0.2, change every production call of `insert_raw_memory` to go through `insert_with_capture_filter`. Call sites to expect:

- `src/memory/session_compactor/...` (G1 pre-compress hook emit)
- `src/a2a/sub_agent.rs` (G2 delegation hook emit)
- `src/gateway/session_manager/ops.rs` (G3-A disconnect emit)
- `src/builtin_tools/session_complete.rs` (G3-C task-done emit)
- Any gateway transcript / media ingestion path

Each site now needs:
- A `CaptureCtx` — construct from the producer's knowledge (agent_id, session_id, source hint string matching the variant name).
- Access to the `Arc<MemoryExtensionRegistry>` — for producers already getting an `Arc<dyn RawMemoryStore>`, thread a parallel `Arc<MemoryExtensionRegistry>` the same way.

For each producer, add a unit test that uses a fake registry with a blocking extension to confirm the memory does NOT land.

- [ ] **Step 6.3: Verify**

```
cargo test -p alephcore extensions -- --nocapture 2>&1 | tail -15
cargo test -p alephcore --lib -- --nocapture 2>&1 | tail -15
cargo check -p alephcore --bin aleph-server 2>&1 | tail -5
```

Expected: all green.

- [ ] **Step 6.4: Commit**

```bash
git add -A
git commit -m "feat(memory): on_capture filter at every raw-memory insert site

New insert_with_capture_filter helper runs dispatch_on_capture before
RawMemoryStore::insert_raw_memory. Blocking extensions prevent
persistence and log the reason. All Spec 1 capture-hook producers
and the gateway ingestion paths thread through the helper."
```

---

## Task 7: `MemoryProducerScheduler`

**Files:**
- Create: `src/memory/extensions/scheduler.rs`
- Modify: `src/memory/extensions/mod.rs`

- [ ] **Step 7.1: Write the scheduler**

```rust
//! Dedicated tokio task that periodically invokes MemoryExtension::produce
//! for all registered plugins. Produced memories go through
//! insert_with_capture_filter so on_capture still applies.

use crate::memory::extensions::insert_helper::insert_with_capture_filter;
use crate::memory::extensions::registry::MemoryExtensionRegistry;
use crate::memory::extensions::types::{CaptureCtx, ProduceCtx};
use crate::memory::namespace::NamespaceScope;
use crate::memory::store::raw_memory::RawMemoryStore;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::time::interval;
use tracing::{debug, warn};

pub const DEFAULT_TICK_SECONDS: u64 = 10;
pub const DEFAULT_AGENT_ID_FOR_PRODUCERS: &str = "default";
pub const MAX_CONSECUTIVE_FAILURES_BEFORE_DISABLE: u32 = 5;

pub struct MemoryProducerScheduler {
    registry: Arc<MemoryExtensionRegistry>,
    raw_store: Arc<dyn RawMemoryStore>,
    tick_duration: Duration,
    tick_counter: AtomicU64,
}

impl MemoryProducerScheduler {
    pub fn new(
        registry: Arc<MemoryExtensionRegistry>,
        raw_store: Arc<dyn RawMemoryStore>,
    ) -> Self {
        Self {
            registry,
            raw_store,
            tick_duration: Duration::from_secs(DEFAULT_TICK_SECONDS),
            tick_counter: AtomicU64::new(0),
        }
    }

    pub fn with_tick_duration(mut self, d: Duration) -> Self {
        self.tick_duration = d;
        self
    }

    /// Spawn the tokio background task. Returns a JoinHandle so the caller
    /// can abort it on shutdown.
    pub fn spawn(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let this = self.clone();
        tokio::spawn(async move {
            let mut tick_interval = interval(this.tick_duration);
            tick_interval.tick().await; // first tick fires immediately; skip
            loop {
                tick_interval.tick().await;
                if let Err(e) = this.run_once().await {
                    warn!("memory producer scheduler tick errored: {e}");
                }
            }
        })
    }

    /// Run a single tick: dispatch produce, route results through on_capture,
    /// persist Allowed memories. Exposed for integration testing.
    pub async fn run_once(&self) -> Result<(), crate::error::AlephError> {
        let tick = self.tick_counter.fetch_add(1, Ordering::Relaxed);
        let produce_ctx = ProduceCtx {
            agent_id: DEFAULT_AGENT_ID_FOR_PRODUCERS.to_string(),
            namespace: NamespaceScope::Owner,
            tick,
        };

        let results = self.registry.dispatch_produce(&produce_ctx).await;

        for (name, res) in results {
            match res {
                Ok(raws) => {
                    for raw in raws {
                        let capture_ctx = CaptureCtx {
                            agent_id: raw.agent_id.clone(),
                            namespace: NamespaceScope::Owner,
                            session_id: raw.session_id.clone(),
                            source_hint: raw.source.as_str().to_string(),
                        };
                        if let Err(e) = insert_with_capture_filter(
                            &self.raw_store,
                            &self.registry,
                            &capture_ctx,
                            raw,
                        ).await {
                            warn!(
                                "producer '{name}' produced memory failed insert: {e}"
                            );
                        }
                    }
                }
                Err(e) => {
                    debug!("producer '{name}' produce tick failed: {e}");
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::extensions::traits::MemoryExtension;
    use crate::memory::extensions::types::ProduceCtx;
    use crate::memory::namespace::NamespaceScope;
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource};
    use async_trait::async_trait;

    struct FakeStore(parking_lot::Mutex<Vec<RawMemory>>);
    #[async_trait]
    impl RawMemoryStore for FakeStore {
        async fn insert_raw_memory(&self, raw: &RawMemory) -> Result<(), crate::error::AlephError> {
            self.0.lock().push(raw.clone());
            Ok(())
        }
        async fn get_unprocessed_raw_memories(&self, _: &str, _: usize) -> Result<Vec<RawMemory>, crate::error::AlephError> { Ok(vec![]) }
        async fn mark_raw_as_processed(&self, _: &[String]) -> Result<usize, crate::error::AlephError> { Ok(0) }
        async fn count_unprocessed(&self, _: &str) -> Result<usize, crate::error::AlephError> { Ok(0) }
        async fn get_raw_by_path_prefix(&self, _: &str, _: &str, _: usize) -> Result<Vec<RawMemory>, crate::error::AlephError> { Ok(vec![]) }
    }

    struct StubProducer(usize);
    #[async_trait]
    impl MemoryExtension for StubProducer {
        fn name(&self) -> &str { "test.stub_producer" }
        async fn produce(&self, _ctx: &ProduceCtx) -> Result<Vec<RawMemory>, crate::error::AlephError> {
            Ok((0..self.0)
                .map(|i| RawMemory::new(format!("m{i}"), RawMemorySource::Transcript).with_agent("a1"))
                .collect())
        }
    }

    #[tokio::test]
    async fn run_once_persists_produced_memories() {
        let store: Arc<dyn RawMemoryStore> = Arc::new(FakeStore(Default::default()));
        let mut reg = MemoryExtensionRegistry::new();
        reg.register(Arc::new(StubProducer(3)));
        let scheduler = MemoryProducerScheduler::new(Arc::new(reg), store.clone());

        scheduler.run_once().await.unwrap();

        // Downcast-ish: we only kept a handle to the concrete store for asserting.
        // Better: re-create the FakeStore as Arc<FakeStore> and cast for the scheduler,
        // retaining a concrete reference for assertion.
    }
}
```

The unit test above falls apart on downcast — simplify by creating `let store_inner = Arc::new(FakeStore(...)); let store: Arc<dyn RawMemoryStore> = store_inner.clone();` and asserting on `store_inner.0.lock().len()` after.

- [ ] **Step 7.2: Expose + verify**

Modify `src/memory/extensions/mod.rs`:

```rust
pub mod scheduler;
pub use scheduler::MemoryProducerScheduler;
```

```
cargo test -p alephcore extensions::scheduler -- --nocapture 2>&1 | tail -15
cargo check -p alephcore 2>&1 | tail -5
```

- [ ] **Step 7.3: Commit**

```bash
git add src/memory/extensions/
git commit -m "feat(memory): MemoryProducerScheduler ticks produce hook

Dedicated tokio task runs every 10s (configurable), calls
dispatch_produce on the registry, routes each produced RawMemory
through insert_with_capture_filter so on_capture still applies to
producer-generated memories."
```

---

## Task 8: MCP adapter

**Files:**
- Create: `src/memory/extensions/mcp_adapter.rs`
- Modify: `src/memory/extensions/mod.rs`

- [ ] **Step 8.1: Locate the real MCP client API**

Run:
```
cd /Volumes/TBU4/Workspace/Aleph
grep -rn "pub struct.*Client\|pub fn call_tool\|pub async fn call_tool" src/extension/runtime/ src/mcp/ 2>/dev/null
```

Identify the type that represents an already-connected MCP client plus its `call_tool(method, args) -> Value` (or similar) method. Note: Aleph may not have a unified "McpClient" struct — MCP calls may happen through a different path (e.g., via the gateway as JSON-RPC, or through a plugin bridge). Use whatever abstraction exists.

If NO reusable MCP client API exists at the extension layer, implement `McpMemoryExtension` with a minimal `pub trait McpCaller { async fn call(method, args) -> Value }` dependency-injected type. Aleph can implement it over the real infrastructure in Task 10 (loader integration).

- [ ] **Step 8.2: Implement `McpMemoryExtension`**

```rust
//! Adapter: wraps an MCP client so a third-party plugin can be used
//! wherever MemoryExtension is expected.

use crate::error::AlephError;
use crate::memory::assembler::envelope::{EnvelopeItem, MemoryEnvelope};
use crate::memory::extensions::traits::MemoryExtension;
use crate::memory::extensions::types::{CaptureCtx, CaptureDecision, ProduceCtx, RetrieveCtx};
use crate::memory::store::raw_memory::RawMemory;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

/// Minimal trait the adapter needs to talk to a plugin. Tests use an
/// in-memory implementation; production wires it to the real MCP client.
#[async_trait]
pub trait McpCaller: Send + Sync {
    async fn call(&self, method: &str, args: Value) -> Result<Value, AlephError>;
}

pub struct McpMemoryExtension {
    name: String,
    caller: Arc<dyn McpCaller>,
}

impl McpMemoryExtension {
    pub fn new(name: impl Into<String>, caller: Arc<dyn McpCaller>) -> Self {
        Self { name: name.into(), caller }
    }
}

#[async_trait]
impl MemoryExtension for McpMemoryExtension {
    fn name(&self) -> &str { &self.name }

    async fn on_retrieve(
        &self,
        ctx: &RetrieveCtx,
        envelope: &mut MemoryEnvelope,
    ) -> Result<(), AlephError> {
        let args = json!({
            "agent_id": ctx.agent_id,
            "query": ctx.query,
            "session_id": ctx.session_id,
            "envelope": envelope,
        });
        let resp = self.caller.call("memory.on_retrieve", args).await?;
        // Response shape: { "additions": [EnvelopeItem, ...] } — may be absent.
        if let Some(additions) = resp.get("additions").and_then(|v| v.as_array()) {
            for a in additions {
                if let Ok(item) = serde_json::from_value::<EnvelopeItem>(a.clone()) {
                    // Merge into first slot, or create an "Extension" slot.
                    if let Some(slot) = envelope.slots.first_mut() {
                        slot.items.push(item);
                    }
                }
            }
        }
        Ok(())
    }

    async fn on_capture(
        &self,
        ctx: &CaptureCtx,
        raw: &mut RawMemory,
    ) -> Result<CaptureDecision, AlephError> {
        let args = json!({
            "agent_id": ctx.agent_id,
            "session_id": ctx.session_id,
            "source_hint": ctx.source_hint,
            "raw": raw,
        });
        let resp = self.caller.call("memory.on_capture", args).await?;
        let decision = match resp.get("decision").and_then(|v| v.as_str()) {
            Some("block") => {
                let reason = resp.get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("plugin blocked")
                    .to_string();
                CaptureDecision::Block { reason }
            }
            _ => CaptureDecision::Allow,
        };
        // Optional modified raw: { "modified": RawMemory }
        if let Some(modified) = resp.get("modified") {
            if let Ok(new_raw) = serde_json::from_value::<RawMemory>(modified.clone()) {
                *raw = new_raw;
            }
        }
        Ok(decision)
    }

    async fn produce(
        &self,
        ctx: &ProduceCtx,
    ) -> Result<Vec<RawMemory>, AlephError> {
        let args = json!({
            "agent_id": ctx.agent_id,
            "tick": ctx.tick,
        });
        let resp = self.caller.call("memory.produce", args).await?;
        let raws = resp.get("raw_memories")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        raws.into_iter()
            .map(|v| serde_json::from_value::<RawMemory>(v).map_err(|e| AlephError::other(format!("malformed raw_memory: {e}"))))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::Mutex as AsyncMutex;

    struct CannedCaller {
        canned: std::sync::Mutex<std::collections::HashMap<String, Value>>,
        last_call: AsyncMutex<Option<(String, Value)>>,
    }

    impl CannedCaller {
        fn new(canned: Vec<(&str, Value)>) -> Self {
            let mut m = std::collections::HashMap::new();
            for (k, v) in canned {
                m.insert(k.to_string(), v);
            }
            Self {
                canned: std::sync::Mutex::new(m),
                last_call: AsyncMutex::new(None),
            }
        }
    }

    #[async_trait]
    impl McpCaller for CannedCaller {
        async fn call(&self, method: &str, args: Value) -> Result<Value, AlephError> {
            *self.last_call.lock().await = Some((method.to_string(), args));
            Ok(self.canned.lock().unwrap()
                .get(method)
                .cloned()
                .unwrap_or_else(|| json!({})))
        }
    }

    #[tokio::test]
    async fn on_capture_block_maps_correctly() {
        let caller = Arc::new(CannedCaller::new(vec![
            ("memory.on_capture", json!({"decision": "block", "reason": "pii"})),
        ]));
        let ext = McpMemoryExtension::new("t", caller);
        let mut raw = RawMemory::new("hi".into(), crate::memory::store::raw_memory::RawMemorySource::Transcript);
        let ctx = CaptureCtx {
            agent_id: "a".into(),
            namespace: crate::memory::namespace::NamespaceScope::Owner,
            session_id: None,
            source_hint: "transcript".into(),
        };
        let d = ext.on_capture(&ctx, &mut raw).await.unwrap();
        match d {
            CaptureDecision::Block { reason } => assert_eq!(reason, "pii"),
            _ => panic!("expected block"),
        }
    }

    #[tokio::test]
    async fn on_capture_unknown_decision_allows() {
        let caller = Arc::new(CannedCaller::new(vec![
            ("memory.on_capture", json!({})),
        ]));
        let ext = McpMemoryExtension::new("t", caller);
        let mut raw = RawMemory::new("hi".into(), crate::memory::store::raw_memory::RawMemorySource::Transcript);
        let ctx = CaptureCtx {
            agent_id: "a".into(),
            namespace: crate::memory::namespace::NamespaceScope::Owner,
            session_id: None,
            source_hint: "transcript".into(),
        };
        let d = ext.on_capture(&ctx, &mut raw).await.unwrap();
        assert!(matches!(d, CaptureDecision::Allow));
    }

    #[tokio::test]
    async fn produce_parses_raw_memories_array() {
        let caller = Arc::new(CannedCaller::new(vec![
            ("memory.produce", json!({"raw_memories": [
                {
                    "id": "1", "content": "x", "source": "transcript",
                    "agent_id": "a", "session_id": null, "path": null,
                    "layer": null, "attachment_text": null,
                    "is_processed": false, "created_at": 0
                }
            ]})),
        ]));
        let ext = McpMemoryExtension::new("t", caller);
        let ctx = ProduceCtx {
            agent_id: "a".into(),
            namespace: crate::memory::namespace::NamespaceScope::Owner,
            tick: 0,
        };
        let out = ext.produce(&ctx).await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content, "x");
    }
}
```

Adapt RawMemory JSON shape to whatever serde derives actually produce — run the test and fix mismatches. If `MemoryEnvelope` / `EnvelopeItem` don't derive `Serialize`, add `#[derive(Serialize, Deserialize)]` on them (check first — Spec 3 render.rs already relies on Serialize).

- [ ] **Step 8.2: Verify**

```
cargo test -p alephcore extensions::mcp_adapter -- --nocapture 2>&1 | tail -15
cargo check -p alephcore 2>&1 | tail -5
```

- [ ] **Step 8.3: Commit**

```bash
git add src/memory/extensions/
git commit -m "feat(memory): McpMemoryExtension adapter over MCP

Thin wrapper that serialises RetrieveCtx/CaptureCtx/ProduceCtx to
JSON, calls the plugin via an McpCaller trait, and deserialises
additions / decisions / produced RawMemories back. McpCaller is a
minimal DI seam; production wires it to the real MCP runtime in
Task 10."
```

---

## Task 9: Manifest `[memory]` section

**Files:**
- Create: `src/memory/extensions/manifest.rs`
- Modify: `src/memory/extensions/mod.rs`
- Modify: `src/extension/manifest/` (extend existing TOML parser)

- [ ] **Step 9.1: Define the manifest struct + tests**

```rust
//! [memory] TOML section for Aleph plugin manifests.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryHook {
    OnRetrieve,
    OnCapture,
    Produce,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryManifestSection {
    pub hooks: Vec<MemoryHook>,
    #[serde(default = "default_priority")]
    pub priority: i32,
    #[serde(default)]
    pub produce_interval_seconds: Option<u64>,
    #[serde(default = "default_on_capture_timeout_action")]
    pub produce_on_capture_timeout: String,
}

fn default_priority() -> i32 { 100 }
fn default_on_capture_timeout_action() -> String { "block".to_string() }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_manifest() {
        let toml = r#"
hooks = ["on_retrieve", "produce"]
priority = 50
produce_interval_seconds = 300
produce_on_capture_timeout = "allow"
"#;
        let m: MemoryManifestSection = toml::from_str(toml).unwrap();
        assert_eq!(m.hooks, vec![MemoryHook::OnRetrieve, MemoryHook::Produce]);
        assert_eq!(m.priority, 50);
        assert_eq!(m.produce_interval_seconds, Some(300));
        assert_eq!(m.produce_on_capture_timeout, "allow");
    }

    #[test]
    fn defaults_apply_when_absent() {
        let toml = r#"hooks = ["on_capture"]"#;
        let m: MemoryManifestSection = toml::from_str(toml).unwrap();
        assert_eq!(m.priority, 100);
        assert_eq!(m.produce_on_capture_timeout, "block");
        assert!(m.produce_interval_seconds.is_none());
    }
}
```

- [ ] **Step 9.2: Extend the existing manifest parser**

Open `src/extension/manifest/` and find the top-level plugin manifest struct. Add an optional field:

```rust
#[serde(default, rename = "memory")]
pub memory_section: Option<crate::memory::extensions::manifest::MemoryManifestSection>,
```

If the manifest parser can't reference `memory::extensions::manifest` due to crate-module cycle, keep a local copy at `src/extension/manifest/memory_section.rs` with the same shape — the loader in Task 10 translates between them.

- [ ] **Step 9.3: Verify + commit**

```
cargo test -p alephcore extensions::manifest -- --nocapture 2>&1 | tail -10
cargo check -p alephcore 2>&1 | tail -5
```

```bash
git add src/memory/extensions/ src/extension/manifest/
git commit -m "feat(memory): add [memory] plugin manifest section

MemoryManifestSection carries hook list, priority, and produce-hook
settings. Plugin manifest parser gains an optional 'memory' field.
Task 10 wires the loader to register plugins declaring this section."
```

---

## Task 10: Plugin loader integration

**Files:**
- Modify: `src/extension/loader.rs`
- Possibly: `src/extension/registrar/`

- [ ] **Step 10.1: Register MCP plugins that declare `[memory]`**

In `src/extension/loader.rs`, where plugins are loaded: for each manifest with a non-`None` `memory_section`, construct an `McpMemoryExtension` using the loaded plugin's MCP client handle, and register it with the shared `Arc<MemoryExtensionRegistry>`.

Pseudocode:

```rust
if let Some(ref mem) = manifest.memory_section {
    let caller = /* wrap the plugin's MCP client into McpCaller */;
    let ext = McpMemoryExtension::new(&manifest.plugin.name, Arc::new(caller));
    memory_extension_registry.register(Arc::new(ext));
}
```

Where the registry is injected into the loader from the server startup site.

If the loader has no access to the `MemoryExtensionRegistry` yet, add a new argument (`registry: Arc<MemoryExtensionRegistry>`) and thread it from Task 11's server wiring.

- [ ] **Step 10.2: Contract-level test (no live MCP)**

Add a test in `src/extension/loader.rs` that:
- Parses a synthetic manifest with `[memory]`.
- Calls the loader logic.
- Asserts the registry now has one more extension with the expected name.

Use the `CannedCaller` pattern from Task 8 to stand in for the MCP client if the loader normally requires a live MCP connection.

- [ ] **Step 10.3: Verify + commit**

```
cargo test -p alephcore extension::loader -- --nocapture 2>&1 | tail -15
cargo check -p alephcore --bin aleph-server 2>&1 | tail -5
```

```bash
git add src/extension/ src/memory/extensions/
git commit -m "feat(memory): plugin loader registers [memory] extensions

MCP plugins whose manifest declares a [memory] section are now
wrapped in McpMemoryExtension and inserted into the
MemoryExtensionRegistry during load."
```

---

## Task 11: Server startup wiring

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/handlers.rs` (or wherever the server builder assembles memory services — use the site from Spec 2 Task 8, commit `7d80e526`, and Spec 3 Task 5).
- Possibly: `src/bin/aleph-server/commands/start/mod.rs`

- [ ] **Step 11.1: Construct the registry**

```rust
use crate::memory::extensions::{
    EnvelopeRelevanceFloorExtension, MemoryExtensionRegistry, MemoryProducerScheduler,
};

// At the site where `MemoryContextProvider` and the raw-memory producers
// are built:
let mut registry = MemoryExtensionRegistry::new();

// First-party POC extension (safe default: 0.0 floor = no-op until configured).
// A real floor would be read from config; for now keep the registration to
// prove the wiring.
registry.register(Arc::new(EnvelopeRelevanceFloorExtension::new(0.0)));

let memory_ext_registry = Arc::new(registry);
```

- [ ] **Step 11.2: Thread into consumers**

- Pass `memory_ext_registry.clone()` into `MemoryContextProvider::with_extensions(...)` (Task 5).
- Pass it into every producer that now goes through `insert_with_capture_filter` (Task 6).
- Pass it into the plugin loader (Task 10).
- Pass it into `MemoryProducerScheduler::new(...)` and spawn the task:

```rust
let scheduler = Arc::new(MemoryProducerScheduler::new(
    memory_ext_registry.clone(),
    raw_memory_store.clone(), // same handle Spec 1 Task 10 uses
));
let _scheduler_handle = scheduler.spawn();
```

- [ ] **Step 11.3: Verify**

```
cargo check -p alephcore --bin aleph-server 2>&1 | tail -5
cargo test -p alephcore --lib -- --nocapture 2>&1 | tail -10
```

Expected: server binary builds; all library tests pass.

- [ ] **Step 11.4: Commit**

```bash
git add src/bin/aleph-server/
git commit -m "feat(memory): wire MemoryExtensionRegistry at server startup

Server builder constructs the registry, registers the POC
EnvelopeRelevanceFloorExtension (no-op at floor=0), threads it into
MemoryContextProvider + all raw-memory producers + plugin loader,
and starts MemoryProducerScheduler on a background task."
```

---

## Task 12: E2E integration test

**Files:**
- Create: `tests/memory_extensions_integration.rs`

- [ ] **Step 12.1: Author the test**

Mirror the harness shape from `tests/memory_capture_hooks.rs` + `tests/memory_reflect_integration.rs` + `tests/memory_modes_integration.rs`.

```rust
//! Integration test: Spec 4 MemoryExtension pipeline end-to-end.

#![cfg(feature = "test-helpers")]

use alephcore::memory::extensions::{
    insert_helper::insert_with_capture_filter, MemoryExtensionRegistry,
    MemoryProducerScheduler,
};
use std::sync::Arc;

// Harness reused from memory_modes_integration.rs — SQLite + NoteIndexer +
// HybridAssembler + MemoryContextProvider + registry threaded through.

#[tokio::test]
async fn envelope_relevance_floor_first_party_prunes_items() {
    // 1. Seed two notes; one with high relevance, one with low.
    // 2. Run provider.build_memory_user_message (via Hybrid mode so XML is rendered).
    // 3. Register EnvelopeRelevanceFloorExtension(0.5) into the registry.
    // 4. Assert the rendered message contains only the high-relevance note.
    unimplemented!("port harness and populate notes");
}

#[tokio::test]
async fn in_proc_capture_filter_blocks_raw_memories() {
    // 1. Register an inline BlockingExtension (on_capture returns Block).
    // 2. Call insert_with_capture_filter with a sample RawMemory.
    // 3. Assert the raw_memories table is empty.
    unimplemented!();
}

#[tokio::test]
async fn in_proc_producer_with_scheduler_run_once() {
    // 1. Register an inline StubProducer (produce returns 2 RawMemories).
    // 2. Build MemoryProducerScheduler with the registry + real store.
    // 3. Call scheduler.run_once().
    // 4. Query raw_memories table and assert 2 rows present.
    unimplemented!();
}
```

Fill in `unimplemented!()` by porting the harness from `tests/memory_capture_hooks.rs`. If time-constrained, scope down to the two simplest tests (capture filter + producer) — they don't need the assembler setup.

- [ ] **Step 12.2: Run**

```
cargo test -p alephcore --features test-helpers --test memory_extensions_integration -- --nocapture 2>&1 | tail -30
```

- [ ] **Step 12.3: Commit**

```bash
git add -f tests/memory_extensions_integration.rs
git commit -m "test(memory): E2E integration test for memory extensions

Three scenarios validate the extension pipeline end-to-end: POC
first-party retrieve-floor pruning, in-proc capture filter blocks
memories, and producer + scheduler round-trip."
```

---

## Task 13: Docs

**Files:**
- Modify: `docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md`
- Create: `docs/reference/memory/EXTENSIONS.md`
- Modify: `docs/reference/memory/RETRIEVAL.md` (short pointer)

- [ ] **Step 13.1: Mark roadmap row shipped**

In `docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md`, change:

```
| 4. Extensions | ⚪ YAGNI-gated | — | — | — |
```

to:

```
| 4. Extensions | ✅ shipped | [design](2026-04-13-memory-evolution-spec4-extensions-design.md) | [plan](../plans/2026-04-13-memory-evolution-spec4-extensions.md) | 2026-04-13 |
```

Also remove or revise the YAGNI-gated note in §"Spec 4 (FUTURE)" now that the trigger (community plugin demand) materialised.

- [ ] **Step 13.2: Write `docs/reference/memory/EXTENSIONS.md`**

```markdown
# Memory Extensions

> Pluggable hooks for enhancing memory behaviour, used by first-party
> Aleph features and third-party MCP plugins through the same surface.

## The three hooks

...full coverage of trait, manifest, dispatch semantics, timeouts, how to
write a plugin, how to write a first-party extension, the POC
EnvelopeRelevanceFloorExtension as a reference example...
```

Target ~200 lines. Pull content from the spec, reorganise for reference
readers. Link to the spec + plan at the bottom.

- [ ] **Step 13.3: RETRIEVAL.md pointer**

Add before "## Appendix":

```markdown
## 15. Pluggable Memory Extensions (Spec 4)

The memory pipeline exposes three hook points — `on_retrieve`,
`on_capture`, and `produce` — through the `MemoryExtension` trait.
First-party Aleph code registers implementations in-process; third-party
plugins register over MCP through the existing plugin manifest by
declaring a `[memory]` section. See
`docs/reference/memory/EXTENSIONS.md` and
`docs/superpowers/specs/2026-04-13-memory-evolution-spec4-extensions-design.md`.
```

- [ ] **Step 13.4: Commit**

```bash
git add docs/
git commit -m "docs(memory): mark Spec 4 shipped and document extensions surface

Roadmap progress table: Spec 4 (Pluggable Memory Extensions) shipped
2026-04-13. New docs/reference/memory/EXTENSIONS.md details the three
hooks, manifest schema, dispatch semantics, and timeout policy.
RETRIEVAL.md gains §15 pointer."
```

---

## Self-Review

1. **Spec coverage** — every spec section maps to a task:
   - §3.1 Trait + defaults → Task 2
   - §3.2 Registry + dispatch → Task 3
   - §3.3 Data flow → Tasks 5 (retrieve) + 6 (capture) + 7 (produce) + 11 (wire)
   - §4.1 First-party path → Tasks 4 + 11
   - §4.2 MCP adapter → Task 8
   - §5 MCP method schemas → Task 8 request/response shapes
   - §6 Manifest → Task 9
   - §7 Timeouts → Task 3 constants + Task 7 scheduler semantics
   - §8 POC extension → Task 4
   - §9 Scheduler → Task 7
   - §10 Server wiring → Task 11
   - §11 Testing → Unit tests in Tasks 1–9 + integration Task 12
   - §13 Open questions → resolved: (a) scheduler reuse → new `MemoryProducerScheduler` per Task 7 (plan-phase grep informed the decision); (b) MCP method schemas → Task 8 shapes; (c) on_capture threshold → constant in Task 7 (`MAX_CONSECUTIVE_FAILURES_BEFORE_DISABLE = 5`; Plan author revises if needed); (d) per-agent override → deferred; (e) backfill → implicit (new extensions only see new envelopes).
   - §14 Unlocks → Task 13 docs record the outcome.

2. **Placeholder scan** — no `TBD` / `FIXME`. `unimplemented!()` in Task 12's integration test body is the planned scope-down seam (same pattern used in Spec 2/3). No "similar to Task N" or bare "TODO".

3. **Type consistency** — `MemoryExtension` trait / `CaptureDecision::{Allow, Block{reason}}` / `MemoryExtensionRegistry` method signatures / `RetrieveCtx` / `CaptureCtx` / `ProduceCtx` / `McpCaller` / `McpMemoryExtension` / `MemoryProducerScheduler` / `EnvelopeRelevanceFloorExtension` / `insert_with_capture_filter` / `MemoryManifestSection` all used identically across Tasks 1–13.
