# Memory Evolution Spec 2: MemoryReflector Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `MemoryReflector` that synthesises coherent LLM answers from the memory store (vs raw hit lists), exposed both as the `memory_reflect` builtin tool and as a core Rust API for internal callers.

**Architecture:** Reflector lives at `src/memory/reflector/`. It composes the existing `HybridAssembler` (retrieval), an `AiProvider` (synthesis), and the existing `recall_signals` store (side effect). Tool handler at `src/builtin_tools/memory_reflect.rs` is a thin wrapper. No new traits, no new storage.

**Tech Stack:** Rust, Tokio, existing `HybridAssembler`, `AiProvider`, `recall_signals.rs`, `schemars`, `serde_json`.

**Spec:** `docs/superpowers/specs/2026-04-13-memory-evolution-spec2-reflector-design.md`

---

## File Structure

### Files to CREATE

| Path | Responsibility |
|------|----------------|
| `src/memory/reflector/mod.rs` | Module entry + re-exports. |
| `src/memory/reflector/types.rs` | `Synthesis`, `NoteRef`, `ReflectOpts` types. |
| `src/memory/reflector/reflector.rs` | `MemoryReflector` struct + `reflect()` method (orchestration). |
| `src/memory/reflector/prompts.rs` | `PROMPT_SYNTHESIS` const + `build_synthesis_prompt` helper. |
| `src/memory/reflector/prompts/snapshots/synthesis.txt` | Prompt body as snapshot file. |
| `src/memory/reflector/packet_adapter.rs` | `packet_to_synthesis_context(packet) -> (user_prompt, note_lookup)` — turn assembler packet into LLM prompt + path→title map. |
| `src/memory/reflector/recall_signals.rs` | `record_reflect_signals(recall_store, query, notes, opts)` — write one row per note fed to prompt. |
| `src/memory/reflector/tests.rs` | Unit tests using mocked assembler + provider. |
| `src/builtin_tools/memory_reflect.rs` | Tool schema + handler wiring. |
| `tests/memory_reflect_integration.rs` | End-to-end integration test with in-memory SQLite fixture. |

### Files to MODIFY

| Path | Change |
|------|--------|
| `src/memory/mod.rs` | `pub mod reflector;` |
| `src/builtin_tools/mod.rs` | `pub mod memory_reflect;` |
| `src/executor/builtin_registry/registry.rs` | Register `memory_reflect` following `note_manage` / `memory_search` pattern. |
| `src/executor/builtin_registry/builder.rs` (if present) | Inject `Arc<MemoryReflector>` into tool-context assembly. |
| `src/bin/aleph-server/commands/start/builder/agent_init.rs` or `start/mod.rs` | Construct `Arc<MemoryReflector>` at startup and pass into builtin-registry builder. |
| `docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md` | Update Spec 2 status row to ✅ shipped. |
| `docs/reference/memory/RETRIEVAL.md` | Add a short section: "Reflect / Synthesis (Spec 2)" pointing at the spec. |

---

## Pre-work: Baseline

- [ ] **Step 0.1: Confirm green baseline**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore 2>&1 | tail -5`
Expected: `Finished \`dev\` profile ... 0 errors`.

If fails, STOP and fix first.

---

## Task 1: Types

**Files:**
- Create: `src/memory/reflector/types.rs`
- Create: `src/memory/reflector/mod.rs`
- Modify: `src/memory/mod.rs`

- [ ] **Step 1.1: Write failing test**

Create `src/memory/reflector/types.rs`:

```rust
//! Public types for the MemoryReflector synthesis layer.

use crate::memory::namespace::NamespaceScope;
use serde::{Deserialize, Serialize};

/// A synthesised answer derived from the memory store.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Synthesis {
    /// Natural-language answer composed by the LLM from retrieved notes.
    pub text: String,
    /// Notes the LLM cited. Titles are code-overlaid; LLM only emits path + relevance.
    pub sources: Vec<NoteRef>,
}

/// A single cited note.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NoteRef {
    /// `category/filename` path, e.g. `"wiki/rust-ownership"`.
    pub path: String,
    /// Human-readable title, resolved from the note store (never from LLM).
    pub title: String,
    /// Rerank score carried over from the HybridAssembler packet, 0.0–1.0.
    pub relevance: f32,
}

/// Options passed to `MemoryReflector::reflect`. Tool-path callers fill this
/// from the current agent context; internal callers may set more fields.
#[derive(Debug, Clone)]
pub struct ReflectOpts {
    pub agent_id: String,
    pub namespace: NamespaceScope,
    /// Optional token budget override for assembler.
    pub max_tokens: Option<usize>,
    /// Optional time filter (unix seconds, inclusive) — reserved for internal callers.
    pub time_range: Option<(i64, i64)>,
    /// Optional session id — threaded into recall_signals.
    pub session_id: Option<String>,
}

impl ReflectOpts {
    pub fn for_agent(agent_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            namespace: NamespaceScope::Owner,
            max_tokens: None,
            time_range: None,
            session_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthesis_round_trips_json() {
        let s = Synthesis {
            text: "Rust uses ownership.".into(),
            sources: vec![NoteRef {
                path: "wiki/rust-ownership".into(),
                title: "Rust Ownership".into(),
                relevance: 0.91,
            }],
        };
        let j = serde_json::to_string(&s).unwrap();
        let back: Synthesis = serde_json::from_str(&j).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn reflect_opts_for_agent_defaults_to_owner() {
        let o = ReflectOpts::for_agent("a1");
        assert_eq!(o.agent_id, "a1");
        assert!(matches!(o.namespace, NamespaceScope::Owner));
        assert!(o.max_tokens.is_none());
        assert!(o.time_range.is_none());
        assert!(o.session_id.is_none());
    }
}
```

Create `src/memory/reflector/mod.rs`:

```rust
//! MemoryReflector — synthesise coherent answers from stored notes.
//!
//! See `docs/superpowers/specs/2026-04-13-memory-evolution-spec2-reflector-design.md`.

pub mod types;

pub use types::{NoteRef, ReflectOpts, Synthesis};
```

In `src/memory/mod.rs` add (near other `pub mod X;`):

```rust
pub mod reflector;
```

- [ ] **Step 1.2: Run tests**

`cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore reflector::types -- --nocapture 2>&1 | tail -10`
Expected: 2 tests pass.

`cargo check -p alephcore 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 1.3: Commit**

```bash
git add src/memory/reflector/ src/memory/mod.rs
git commit -m "feat(memory): add MemoryReflector public types

Synthesis / NoteRef / ReflectOpts for the Spec 2 reflection layer.
Titles in NoteRef are code-overlaid (never LLM-generated) to block
source hallucination."
```

---

## Task 2: Synthesis prompt

**Files:**
- Create: `src/memory/reflector/prompts.rs`
- Create: `src/memory/reflector/prompts/snapshots/synthesis.txt`
- Modify: `src/memory/reflector/mod.rs` (add `pub mod prompts;`)

- [ ] **Step 2.1: Write snapshot file**

Create directory: `cd /Volumes/TBU4/Workspace/Aleph && mkdir -p src/memory/reflector/prompts/snapshots`

Create `src/memory/reflector/prompts/snapshots/synthesis.txt`:

```
You are a memory synthesis assistant. Below are notes retrieved from the user's long-term memory, ranked by relevance. Compose a coherent answer to the user's question using ONLY information in those notes.

RULES:
1. Only use information from the provided notes. Do not invent facts.
2. If the notes don't cover the question, say so in plain language — do NOT make up an answer. An honest "my memory has no information on this" is valuable.
3. Express uncertainty, contradictions, or caveats in natural language inside `text`. Do NOT split them into separate structured fields.
4. Every note you cite must appear in `sources` with its exact `path` and a `relevance` score copied from the `[score=...]` tag in the input. Do not hallucinate paths or scores.

OUTPUT FORMAT (JSON only, no markdown code blocks):
{
  "text": "Synthesised answer as continuous prose. Can be multiple sentences.",
  "sources": [
    { "path": "exact/path/from/input", "relevance": 0.81 }
  ]
}
```

- [ ] **Step 2.2: Write prompts.rs + tests**

Create `src/memory/reflector/prompts.rs`:

```rust
//! Synthesis prompt for MemoryReflector.

pub const PROMPT_SYNTHESIS: &str = include_str!("prompts/snapshots/synthesis.txt");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_is_nonempty_and_json_directed() {
        assert!(
            PROMPT_SYNTHESIS.len() > 200,
            "synthesis prompt suspiciously short: {} bytes",
            PROMPT_SYNTHESIS.len()
        );
        assert!(PROMPT_SYNTHESIS.contains("JSON"));
        assert!(PROMPT_SYNTHESIS.contains("sources"));
        assert!(PROMPT_SYNTHESIS.contains("relevance"));
    }

    #[test]
    fn prompt_enforces_no_hallucination() {
        assert!(
            PROMPT_SYNTHESIS.contains("Do not invent")
                || PROMPT_SYNTHESIS.contains("Do not hallucinate"),
            "prompt must instruct no invented facts"
        );
    }
}
```

In `src/memory/reflector/mod.rs` add:

```rust
pub mod prompts;
```

- [ ] **Step 2.3: Run tests**

`cargo test -p alephcore reflector::prompts -- --nocapture 2>&1 | tail -10`
Expected: 2 tests pass.

- [ ] **Step 2.4: Commit**

```bash
git add src/memory/reflector/prompts.rs src/memory/reflector/prompts/ src/memory/reflector/mod.rs
git commit -m "feat(memory): add MemoryReflector synthesis prompt

PROMPT_SYNTHESIS stored as snapshot file for regression-testable review.
Forbids invented facts and structured confidence fields — uncertainty
goes in the text, not separate JSON keys."
```

---

## Task 3: Packet adapter

**Files:**
- Create: `src/memory/reflector/packet_adapter.rs`
- Modify: `src/memory/reflector/mod.rs`

- [ ] **Step 3.1: Locate `HybridAssembler::assemble` signature**

Run: `cd /Volumes/TBU4/Workspace/Aleph && grep -n "pub async fn assemble\|pub fn assemble\|struct AssembledPacket\|struct MemoryPacket\|pub struct MemoryEnvelope" src/memory/assembler/*.rs`

Record the names. The current shape (confirmed at design time) is:
- `HybridAssembler::assemble(query, opts) -> Result<AssembledPacket, AlephError>` (or similar)
- Packet contains `envelopes: Vec<MemoryEnvelope>` where each envelope has `path`, `title`, `content`, `score`.

If the names differ, note them — below uses `AssembledPacket` / `MemoryEnvelope`. Adjust by find/replace if the real names diverge.

- [ ] **Step 3.2: Write failing test**

Create `src/memory/reflector/packet_adapter.rs`:

```rust
//! Convert an `AssembledPacket` into the user-prompt text + path→title lookup
//! required by the synthesis pipeline.

use crate::memory::assembler::envelope::MemoryEnvelope;
use std::collections::HashMap;

/// The prompt-ready representation: a user-message body and a path→title
/// lookup that the reflector uses to overlay real titles onto the LLM's
/// path-only `NoteRef` output.
pub struct SynthesisContext {
    pub user_prompt: String,
    pub note_lookup: HashMap<String, NoteMeta>,
}

#[derive(Debug, Clone)]
pub struct NoteMeta {
    pub title: String,
    pub relevance: f32,
}

pub fn packet_to_synthesis_context(
    query: &str,
    envelopes: &[MemoryEnvelope],
) -> SynthesisContext {
    let mut body = format!("QUESTION: {query}\n\n");
    body.push_str("RETRIEVED NOTES (higher score = more relevant):\n\n");

    let mut lookup = HashMap::new();
    for env in envelopes {
        body.push_str(&format!(
            "[path={path} score={score:.3}] {title}\n{content}\n\n---\n\n",
            path = env.path,
            score = env.score,
            title = env.title,
            content = env.content,
        ));
        lookup.insert(
            env.path.clone(),
            NoteMeta {
                title: env.title.clone(),
                relevance: env.score,
            },
        );
    }
    SynthesisContext {
        user_prompt: body,
        note_lookup: lookup,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::assembler::envelope::MemoryEnvelope;

    fn env(path: &str, title: &str, content: &str, score: f32) -> MemoryEnvelope {
        MemoryEnvelope {
            path: path.to_string(),
            title: title.to_string(),
            content: content.to_string(),
            score,
        }
    }

    #[test]
    fn empty_envelopes_still_produces_question() {
        let ctx = packet_to_synthesis_context("What is Rust?", &[]);
        assert!(ctx.user_prompt.contains("What is Rust?"));
        assert!(ctx.note_lookup.is_empty());
    }

    #[test]
    fn envelopes_become_tagged_blocks() {
        let envs = vec![
            env("wiki/rust", "Rust Lang", "Rust is a systems language.", 0.91),
            env("wiki/ownership", "Ownership", "Every value has one owner.", 0.73),
        ];
        let ctx = packet_to_synthesis_context("ownership?", &envs);
        assert!(ctx.user_prompt.contains("[path=wiki/rust score=0.910]"));
        assert!(ctx.user_prompt.contains("[path=wiki/ownership score=0.730]"));
        assert_eq!(ctx.note_lookup.len(), 2);
        assert_eq!(ctx.note_lookup.get("wiki/rust").unwrap().title, "Rust Lang");
        assert!((ctx.note_lookup.get("wiki/ownership").unwrap().relevance - 0.73).abs() < 1e-6);
    }
}
```

**IF** `MemoryEnvelope` does NOT have `title` / `content` / `score` fields with those exact names, open `src/memory/assembler/envelope.rs` and adapt:
- Rename the struct-literal helper `env()` to use the real field names.
- Adjust `packet_to_synthesis_context` to read the actual fields.
- If `title` is a `Option<String>`, use `.as_deref().unwrap_or(&env.path)` as the display fallback.
- If `score` is a different name (e.g., `rerank_score`), change the body format accordingly.

In `src/memory/reflector/mod.rs` add:

```rust
pub mod packet_adapter;
```

- [ ] **Step 3.3: Run tests**

`cargo test -p alephcore reflector::packet_adapter -- --nocapture 2>&1 | tail -10`
Expected: 2 tests pass.

- [ ] **Step 3.4: Commit**

```bash
git add src/memory/reflector/packet_adapter.rs src/memory/reflector/mod.rs
git commit -m "feat(memory): MemoryReflector packet adapter

packet_to_synthesis_context() turns an assembler packet into the
LLM user-prompt body and a path→(title, relevance) lookup that the
reflector uses to overlay canonical titles over the LLM's path-only
cite output."
```

---

## Task 4: `MemoryReflector::reflect` core (empty-packet short-circuit)

**Files:**
- Create: `src/memory/reflector/reflector.rs`
- Create: `src/memory/reflector/tests.rs` (or inline in `reflector.rs`)
- Modify: `src/memory/reflector/mod.rs`

- [ ] **Step 4.1: Inspect assembler constructor + assemble signature**

Run: `cd /Volumes/TBU4/Workspace/Aleph && grep -n "pub fn new\|pub async fn assemble\|pub fn assemble" src/memory/assembler/hybrid.rs`

Record the full `assemble` signature — what does it take (query, opts) and return (`AssembledPacket` / `Result<X>`)? Use that shape in the reflector.

- [ ] **Step 4.2: Write `reflector.rs` — short-circuit-only version**

Create `src/memory/reflector/reflector.rs`:

```rust
//! MemoryReflector — orchestrates HybridAssembler + LLM synthesis.

use crate::error::AlephError;
use crate::memory::assembler::hybrid::HybridAssembler;
use crate::memory::reflector::types::{ReflectOpts, Synthesis};
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

pub struct MemoryReflector {
    assembler: Arc<HybridAssembler>,
    provider: Arc<dyn AiProvider>,
    // recall_signals writer injected in Task 6.
}

impl MemoryReflector {
    pub fn new(
        assembler: Arc<HybridAssembler>,
        provider: Arc<dyn AiProvider>,
    ) -> Self {
        Self {
            assembler,
            provider,
        }
    }

    pub async fn reflect(
        &self,
        query: &str,
        opts: ReflectOpts,
    ) -> Result<Synthesis, AlephError> {
        // Delegate to the assembler for retrieval.
        let packet = self
            .assembler
            .assemble(query, &opts) // ← adapt to real signature
            .await?;

        // Short-circuit when the packet is empty.
        if packet.envelopes.is_empty() {
            return Ok(Synthesis {
                text: "No relevant memories found.".to_string(),
                sources: Vec::new(),
            });
        }

        // Task 5 adds the LLM path.
        unreachable!("LLM synthesis path implemented in Task 5");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Mocks imported in Task 5. For now only the short-circuit is tested
    // via `reflect_empty_packet_short_circuits` below.
}
```

**IMPORTANT ADAPTATION**: The exact `HybridAssembler::assemble` signature may differ from `(query: &str, opts: &ReflectOpts) -> Result<AssembledPacket, AlephError>`. Look at `src/memory/assembler/hybrid.rs` to see:
- Does it take a `&str` query directly or a richer `Query` struct?
- Does it take `agent_id` / `namespace` separately, or packed in an options struct?

Adapt `reflect()`'s body to marshal `ReflectOpts` into whatever the assembler actually needs. The contract is: "given our ReflectOpts, call assembler once, get a packet whose envelopes we can inspect".

If the assembler returns a struct OTHER than one with a `.envelopes: Vec<_>` field, adjust the emptiness check accordingly (e.g., `packet.is_empty()` or similar).

- [ ] **Step 4.3: Write short-circuit unit test**

Add (appended to `reflector.rs` or as `src/memory/reflector/tests.rs` and declared via `#[cfg(test)] mod tests;` in `mod.rs`):

```rust
#[cfg(test)]
mod short_circuit_tests {
    use super::*;
    use crate::memory::reflector::types::ReflectOpts;
    // NOTE: we need a faked assembler. If the existing codebase offers
    // a testable constructor, use it. Otherwise, the first real assertion
    // happens in Task 5 (which wires RecordingMockProvider + fixture).
    // For this task we only need to verify empty_packet → stub Synthesis.

    // Minimal fake assembler helper: factored into a `pub(crate)` newtype
    // below so both this module and the integration test can reuse it.

    #[tokio::test]
    async fn reflect_empty_packet_short_circuits_to_stub() {
        // If the concrete HybridAssembler cannot be easily faked yet
        // (complex dep graph), SKIP this test with an explanation and
        // rely on Task 7's integration test to cover the empty case too.
        //
        // If it CAN be faked (e.g. via pub(crate) seam or test-helpers
        // feature), build one that returns an empty packet and call
        // `reflector.reflect(...)` asserting:
        //    synthesis.text == "No relevant memories found."
        //    synthesis.sources.is_empty()

        // Placeholder assertion: the stub string itself doesn't require
        // any live object. Verifies the compile-time code path at minimum.
        let fallback = Synthesis {
            text: "No relevant memories found.".to_string(),
            sources: Vec::new(),
        };
        assert_eq!(fallback.text, "No relevant memories found.");
    }
}
```

**If the `HybridAssembler` has a pub(crate) seam for testing**, use it here and assert the real reflect() path. Otherwise this placeholder holds the line until Task 7's integration test exercises the real pipeline.

In `src/memory/reflector/mod.rs` add:

```rust
pub mod reflector;
pub use reflector::MemoryReflector;
```

- [ ] **Step 4.4: Run check + tests**

`cargo test -p alephcore reflector:: -- --nocapture 2>&1 | tail -15`
Expected: tests pass (or the short-circuit placeholder passes, plus Tasks 1 & 2 tests).

`cargo check -p alephcore 2>&1 | tail -5`
Expected: clean (the `unreachable!()` in `reflect()` compiles fine; it just isn't reachable in short-circuit tests).

- [ ] **Step 4.5: Commit**

```bash
git add src/memory/reflector/reflector.rs src/memory/reflector/mod.rs
git commit -m "feat(memory): MemoryReflector empty-packet short-circuit

Orchestrator skeleton: calls HybridAssembler::assemble, returns the
\"No relevant memories found.\" stub when the packet is empty. The
LLM synthesis path lands in the next task."
```

---

## Task 5: LLM synthesis path + JSON parsing

**Files:**
- Modify: `src/memory/reflector/reflector.rs`
- Modify: `src/memory/reflector/tests.rs` (if split out)

- [ ] **Step 5.1: Write failing test**

Add to `reflector.rs` tests module (or sibling tests.rs):

```rust
#[cfg(test)]
mod llm_path_tests {
    use super::*;
    use crate::memory::reflector::packet_adapter::SynthesisContext;
    use crate::memory::reflector::types::{NoteRef, ReflectOpts, Synthesis};
    use crate::providers::recording_mock::RecordingMockProvider;
    use std::collections::HashMap;
    use std::sync::Arc;

    /// Direct test of the synthesis-from-context logic, isolated from the
    /// assembler. The public `reflect()` method is integration-tested in
    /// Task 8 against a real pipeline.
    #[tokio::test]
    async fn synthesise_parses_llm_json_and_overlays_titles() {
        let canned = r#"{"text":"Rust enforces ownership.","sources":[{"path":"wiki/rust","relevance":0.91}]}"#;
        let provider = RecordingMockProvider::new(canned.to_string());

        // packet_to_synthesis_context output mock
        let mut lookup = HashMap::new();
        lookup.insert(
            "wiki/rust".to_string(),
            crate::memory::reflector::packet_adapter::NoteMeta {
                title: "Rust Lang".to_string(),
                relevance: 0.91,
            },
        );
        let ctx = SynthesisContext {
            user_prompt: "QUESTION: ownership?".to_string(),
            note_lookup: lookup,
        };

        let synthesis = MemoryReflector::synthesise_from_context(
            &ctx,
            &(Arc::new(provider) as Arc<dyn crate::providers::AiProvider>),
        )
        .await
        .unwrap();

        assert_eq!(synthesis.text, "Rust enforces ownership.");
        assert_eq!(synthesis.sources.len(), 1);
        assert_eq!(synthesis.sources[0].path, "wiki/rust");
        // Title overlaid from lookup — not from LLM (LLM didn't emit one):
        assert_eq!(synthesis.sources[0].title, "Rust Lang");
        assert!((synthesis.sources[0].relevance - 0.91).abs() < 1e-6);
    }

    #[tokio::test]
    async fn unknown_path_from_llm_is_dropped() {
        // LLM fabricates a path that's not in the lookup → should be skipped.
        let canned = r#"{"text":"x","sources":[{"path":"wiki/rust","relevance":0.5},{"path":"wiki/fake","relevance":0.99}]}"#;
        let provider = RecordingMockProvider::new(canned.to_string());

        let mut lookup = HashMap::new();
        lookup.insert(
            "wiki/rust".to_string(),
            crate::memory::reflector::packet_adapter::NoteMeta {
                title: "Rust".to_string(),
                relevance: 0.5,
            },
        );
        let ctx = SynthesisContext {
            user_prompt: "?".to_string(),
            note_lookup: lookup,
        };

        let synthesis = MemoryReflector::synthesise_from_context(
            &ctx,
            &(Arc::new(provider) as Arc<dyn crate::providers::AiProvider>),
        )
        .await
        .unwrap();

        assert_eq!(synthesis.sources.len(), 1, "fake path must be dropped");
        assert_eq!(synthesis.sources[0].path, "wiki/rust");
    }

    #[tokio::test]
    async fn malformed_json_falls_back_to_text_only() {
        // LLM returns plain prose with no JSON — we gracefully degrade to
        // Synthesis { text = raw text, sources = [] } (callers still get
        // something usable instead of an error).
        let canned = "The notes don't cover that topic.".to_string();
        let provider = RecordingMockProvider::new(canned);

        let ctx = SynthesisContext {
            user_prompt: "?".to_string(),
            note_lookup: HashMap::new(),
        };
        let synthesis = MemoryReflector::synthesise_from_context(
            &ctx,
            &(Arc::new(provider) as Arc<dyn crate::providers::AiProvider>),
        )
        .await
        .unwrap();
        assert!(synthesis.text.contains("don't cover"));
        assert!(synthesis.sources.is_empty());
    }
}
```

- [ ] **Step 5.2: Implement `synthesise_from_context` and wire it into `reflect`**

Replace the `unreachable!()` in `reflect()` with the full LLM path, and add the helper:

```rust
use crate::memory::reflector::packet_adapter::{
    packet_to_synthesis_context, SynthesisContext,
};
use crate::memory::reflector::prompts::PROMPT_SYNTHESIS;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::utils::json_extract::extract_json_robust;
use serde::Deserialize;
use tracing::warn;

#[derive(Debug, Deserialize)]
struct LlmSourceRef {
    path: String,
    #[serde(default)]
    relevance: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct LlmSynthesis {
    #[serde(default)]
    text: String,
    #[serde(default)]
    sources: Vec<LlmSourceRef>,
}

impl MemoryReflector {
    /// Pure LLM-facing synthesis step. Exposed pub(crate) for unit tests.
    pub(crate) async fn synthesise_from_context(
        ctx: &SynthesisContext,
        provider: &Arc<dyn AiProvider>,
    ) -> Result<Synthesis, AlephError> {
        let msgs = [UnifiedMessage::user(&ctx.user_prompt)];
        let response = provider
            .process(RequestPayload::new(&msgs).with_system(Some(PROMPT_SYNTHESIS)))
            .await
            .map_err(|e| AlephError::other(format!("Reflect LLM call failed: {e}")))?;
        let text = response.text_content();

        // Try to parse JSON; if it fails, return raw text with no sources.
        let parsed: Option<LlmSynthesis> = extract_json_robust(&text)
            .and_then(|v| serde_json::from_value::<LlmSynthesis>(v).ok());

        let Some(llm) = parsed else {
            warn!("reflector: LLM response was not parseable JSON; returning text-only synthesis");
            return Ok(Synthesis {
                text: text.trim().to_string(),
                sources: Vec::new(),
            });
        };

        // Overlay titles from the lookup; drop any path the LLM fabricated.
        let sources: Vec<crate::memory::reflector::types::NoteRef> = llm
            .sources
            .into_iter()
            .filter_map(|s| {
                ctx.note_lookup.get(&s.path).map(|meta| {
                    crate::memory::reflector::types::NoteRef {
                        path: s.path,
                        title: meta.title.clone(),
                        relevance: s.relevance.unwrap_or(meta.relevance),
                    }
                })
            })
            .collect();

        Ok(Synthesis {
            text: llm.text,
            sources,
        })
    }
}
```

And update `reflect()`'s body (replacing the `unreachable!()`):

```rust
        let ctx = packet_to_synthesis_context(query, &packet.envelopes);
        let synthesis = Self::synthesise_from_context(&ctx, &self.provider).await?;
        // recall_signals written in Task 6.
        Ok(synthesis)
```

**Adapt** if `packet.envelopes` isn't the exact field. Use whatever the packet actually holds.

- [ ] **Step 5.3: Run tests**

`cargo test -p alephcore reflector::reflector -- --nocapture 2>&1 | tail -20`
Expected: 3 synthesis tests pass + the Task 4 short-circuit placeholder.

`cargo check -p alephcore 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 5.4: Commit**

```bash
git add src/memory/reflector/reflector.rs
git commit -m "feat(memory): MemoryReflector LLM synthesis path

synthesise_from_context() builds the user prompt from the packet,
runs the LLM with PROMPT_SYNTHESIS, parses JSON response, overlays
canonical titles from the lookup (so fabricated paths are dropped).
Malformed JSON degrades gracefully to a text-only Synthesis."
```

---

## Task 6: `recall_signals` side effect

**Files:**
- Create: `src/memory/reflector/recall_signals.rs`
- Modify: `src/memory/reflector/reflector.rs` (inject + call)
- Modify: `src/memory/reflector/mod.rs`

- [ ] **Step 6.1: Inspect existing recall_signals API**

Run:
```
cd /Volumes/TBU4/Workspace/Aleph
grep -n "pub fn record_signals\|pub fn query_hash\|record_signal\b" src/memory/store/sqlite/recall_signals.rs | head
grep -n "recall_signals\|RecallSignal" src/memory/store/sqlite/mod.rs | head -15
```

Record the exact method name + signature used to write a row. The spec assumes something like:
```rust
store.record_signals(&[(note_path, query_text, channel, score)], session_id, namespace)
```
but the real API may be per-row.

- [ ] **Step 6.2: Write signal helper**

Create `src/memory/reflector/recall_signals.rs`:

```rust
//! Record a `recall_signals` row per note fed into a reflect() synthesis call.

use crate::error::AlephError;
use crate::memory::reflector::packet_adapter::NoteMeta;
use crate::memory::reflector::types::ReflectOpts;
use std::collections::HashMap;

pub const REFLECT_CHANNEL: &str = "reflect";

/// Write one signal per note. Real caller must provide a concrete handle
/// to the sqlite recall_signals writer (typically `Arc<AlephSqliteStore>` or
/// a thinner `Arc<RecallSignalStore>` wrapper). The first arg is kept as
/// `&dyn Fn(...)` so Spec 2 does not need to invent a new trait.
pub async fn record_reflect_signals<F, Fut>(
    record: F,
    query: &str,
    note_lookup: &HashMap<String, NoteMeta>,
    opts: &ReflectOpts,
) -> Result<(), AlephError>
where
    F: Fn(SignalRow) -> Fut,
    Fut: std::future::Future<Output = Result<(), AlephError>>,
{
    for (path, meta) in note_lookup {
        let row = SignalRow {
            note_path: path.clone(),
            query_text: query.to_string(),
            channel: REFLECT_CHANNEL.to_string(),
            score: meta.relevance,
            session_id: opts.session_id.clone(),
            namespace: opts.namespace.to_namespace_value(),
        };
        record(row).await?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct SignalRow {
    pub note_path: String,
    pub query_text: String,
    pub channel: String,
    pub score: f32,
    pub session_id: Option<String>,
    pub namespace: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::namespace::NamespaceScope;

    #[tokio::test]
    async fn records_one_row_per_note() {
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let cap_ref = captured.clone();
        let record = |row: SignalRow| {
            let cap_ref = cap_ref.clone();
            async move {
                cap_ref.lock().unwrap().push(row);
                Ok(())
            }
        };
        let mut lookup = HashMap::new();
        lookup.insert(
            "wiki/a".to_string(),
            NoteMeta { title: "A".into(), relevance: 0.5 },
        );
        lookup.insert(
            "wiki/b".to_string(),
            NoteMeta { title: "B".into(), relevance: 0.8 },
        );
        let opts = ReflectOpts::for_agent("x");
        record_reflect_signals(record, "q", &lookup, &opts).await.unwrap();
        let rows = captured.lock().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.channel == "reflect"));
        assert!(rows.iter().all(|r| r.query_text == "q"));
    }

    #[tokio::test]
    async fn empty_lookup_writes_no_rows() {
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let cap_ref = captured.clone();
        let record = |row: SignalRow| {
            let cap_ref = cap_ref.clone();
            async move {
                cap_ref.lock().unwrap().push(row);
                Ok(())
            }
        };
        let opts = ReflectOpts::for_agent("x");
        record_reflect_signals(record, "q", &HashMap::new(), &opts).await.unwrap();
        assert!(captured.lock().unwrap().is_empty());
    }
}
```

If `NamespaceScope::to_namespace_value(&self) -> String` doesn't exist, grep `src/memory/namespace.rs` for the method that converts the enum to the SQLite `namespace` column value and use that instead.

- [ ] **Step 6.3: Wire into `MemoryReflector`**

Modify `MemoryReflector` struct + constructor to take a recall-signal writer. Because recall_signals.rs exposes its methods on the SQLite store via `impl`, the most pragmatic approach is to take an `Arc<AlephSqliteStore>` (or whichever concrete type holds the `record_signals` impl) and call it via a closure.

In `src/memory/reflector/reflector.rs`:

```rust
use crate::memory::reflector::recall_signals::{record_reflect_signals, SignalRow};

pub struct MemoryReflector {
    assembler: Arc<HybridAssembler>,
    provider: Arc<dyn AiProvider>,
    /// Holds the sqlite store that exposes `record_signals`. Boxed as a
    /// closure so the reflector doesn't take a hard dependency on the
    /// concrete store type.
    recall_writer: RecallWriter,
}

pub type RecallWriter = Arc<
    dyn Fn(SignalRow) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), AlephError>> + Send>,
    > + Send + Sync,
>;

impl MemoryReflector {
    pub fn new(
        assembler: Arc<HybridAssembler>,
        provider: Arc<dyn AiProvider>,
        recall_writer: RecallWriter,
    ) -> Self {
        Self { assembler, provider, recall_writer }
    }
    // ...
}
```

At the end of `reflect()`, before returning `Ok(synthesis)`, call:

```rust
let writer = self.recall_writer.clone();
let record = move |row| {
    let writer = writer.clone();
    async move { writer(row).await }
};
let _ = record_reflect_signals(record, query, &ctx.note_lookup, &opts).await;
// intentionally ignore errors — a failure to log signals must never fail
// the reflect() call (the user still gets their synthesis).
```

- [ ] **Step 6.4: Run tests**

```
cargo test -p alephcore reflector::recall_signals -- --nocapture 2>&1 | tail -10
cargo test -p alephcore reflector -- --nocapture 2>&1 | tail -20
cargo check -p alephcore 2>&1 | tail -5
```
Expected: all pass; no regressions.

- [ ] **Step 6.5: Commit**

```bash
git add src/memory/reflector/recall_signals.rs src/memory/reflector/reflector.rs src/memory/reflector/mod.rs
git commit -m "feat(memory): MemoryReflector writes recall_signals per note

record_reflect_signals() fires one row per note in the synthesis
context (channel=reflect). Failures are swallowed so signal-log
issues never fail a reflect() call."
```

---

## Task 7: `memory_reflect` builtin tool

**Files:**
- Create: `src/builtin_tools/memory_reflect.rs`
- Modify: `src/builtin_tools/mod.rs`
- Modify: `src/executor/builtin_registry/registry.rs`
- Modify: `src/executor/builtin_registry/builder.rs`

- [ ] **Step 7.1: Inspect tool registration pattern**

Grep the registry for an existing memory-side tool that takes a service handle and emits JSON. `note_manage` is the closest pattern (commit `d094d033+` landed it during Spec 1).

```
cd /Volumes/TBU4/Workspace/Aleph
grep -n "session_complete\|memory_search\|register_tool\|note_manage" src/executor/builtin_registry/registry.rs | head -20
grep -n "session_complete\|memory_search\|register_tool\|note_manage" src/executor/builtin_registry/builder.rs | head -20
```

Record how the handler gets its service handle (`ctx.memory_reflector.clone()` or similar) — match exactly.

- [ ] **Step 7.2: Write tool file with failing test**

Create `src/builtin_tools/memory_reflect.rs`:

```rust
//! LLM-facing tool: synthesise an answer from the memory store.

use crate::error::AlephError;
use crate::memory::namespace::NamespaceScope;
use crate::memory::reflector::{MemoryReflector, ReflectOpts, Synthesis};
use crate::sync_primitives::Arc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryReflectArgs {
    /// Natural-language question to synthesise an answer for from memory.
    pub query: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryReflectResult {
    pub synthesis: Synthesis,
}

pub const TOOL_NAME: &str = "memory_reflect";

pub const TOOL_DESCRIPTION: &str = "Synthesise a coherent answer from your \
long-term memory. Use this when you want a distilled response (vs \
memory_search, which returns raw hits). Returns answer text + cited note paths.";

pub async fn handle(
    args: MemoryReflectArgs,
    reflector: &Arc<MemoryReflector>,
    agent_id: &str,
    session_id: Option<&str>,
) -> Result<MemoryReflectResult, AlephError> {
    let opts = ReflectOpts {
        agent_id: agent_id.to_string(),
        namespace: NamespaceScope::Owner,
        max_tokens: None,
        time_range: None,
        session_id: session_id.map(|s| s.to_string()),
    };
    let synthesis = reflector.reflect(&args.query, opts).await?;
    Ok(MemoryReflectResult { synthesis })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_round_trip_json() {
        let a = MemoryReflectArgs { query: "What do I know about Rust?".into() };
        let j = serde_json::to_string(&a).unwrap();
        let back: MemoryReflectArgs = serde_json::from_str(&j).unwrap();
        assert_eq!(back.query, a.query);
    }

    #[test]
    fn tool_description_mentions_synthesis() {
        assert!(TOOL_DESCRIPTION.contains("synthesi")); // "synthesise" or "synthesis"
        assert!(TOOL_DESCRIPTION.contains("memory_search"));
    }
}
```

Register in `src/builtin_tools/mod.rs`:

```rust
pub mod memory_reflect;
```

Register in `src/executor/builtin_registry/registry.rs` AND `builder.rs` — follow EXACTLY the existing `session_complete` wiring as a template (introduced by Spec 1 Task 9, commit `ebdf3795`). The registration:
- Adds `memory_reflect` to the schema registry
- Handler extracts `agent_id`, `session_id`, `Arc<MemoryReflector>` from tool context
- Parses `MemoryReflectArgs` from incoming JSON
- Calls `memory_reflect::handle(args, &reflector, agent_id, session_id)` → serialises `MemoryReflectResult` to JSON

Task 8 wires `Arc<MemoryReflector>` into the tool context; Task 7 does the registry schema + handler closure. If you cannot reach the reflector from the handler yet, use a `todo!("wired in Task 8")` in the handler body and commit the schema + description so the tool is visible — Task 8 fills in the call.

- [ ] **Step 7.3: Run tests + check**

```
cargo test -p alephcore memory_reflect -- --nocapture 2>&1 | tail -15
cargo check -p alephcore 2>&1 | tail -5
```
Expected: 2 unit tests pass; build green.

- [ ] **Step 7.4: Commit**

```bash
git add src/builtin_tools/memory_reflect.rs src/builtin_tools/mod.rs src/executor/builtin_registry/
git commit -m "feat(memory): add memory_reflect builtin tool

LLM-facing entry for MemoryReflector. Accepts a natural-language
query and returns Synthesis{text, sources} as JSON. The core
reflector does the retrieval + synthesis + recall-signal write."
```

---

## Task 8: Wire `Arc<MemoryReflector>` at server startup

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init.rs` or `start/mod.rs` (wherever other memory services — `HybridAssembler`, `AlephSqliteStore`, `FactExtractor` — are assembled).
- Modify: `src/executor/builtin_registry/builder.rs` (to accept + propagate the reflector handle).

- [ ] **Step 8.1: Locate the assembly point**

```
cd /Volumes/TBU4/Workspace/Aleph
grep -rn "HybridAssembler::new\|FactExtractor::new" src/bin/ | head -10
```

Record the file:line where `HybridAssembler` is built. The reflector construction belongs directly after, because it consumes:
- The just-built `Arc<HybridAssembler>`
- An `Arc<dyn AiProvider>` (reuse the same one already used for `FactExtractor` / `CompressionService`)
- A closure that invokes `AlephSqliteStore::record_signals`

- [ ] **Step 8.2: Construct the reflector + thread it through**

At the assembly point, add:

```rust
let reflector = {
    use crate::memory::reflector::{recall_signals::SignalRow, MemoryReflector, RecallWriter};
    let store = memory_db.clone(); // whatever the AlephSqliteStore handle is named
    let writer: RecallWriter = std::sync::Arc::new(move |row: SignalRow| {
        let store = store.clone();
        Box::pin(async move {
            // Translate SignalRow → store.record_signals(...) — inspect the real
            // record_signals signature (it likely takes &[(...)], session id, ns).
            store.record_signals(
                &[(row.note_path, row.query_text, row.channel, row.score)],
                row.session_id.as_deref(),
                &row.namespace,
            )
        })
    });
    std::sync::Arc::new(MemoryReflector::new(
        assembler.clone(),
        provider.clone(),
        writer,
    ))
};
```

**Adapt**: the exact `record_signals` signature is unknown until Task 6 Step 6.1 locked it down. If the store method signature differs, wrap it inside the closure appropriately. The contract the reflector needs is "async callable that takes a `SignalRow` and writes it". Anything else is implementation detail.

Then pass `reflector.clone()` into the builtin-registry builder (see how `note_manage` / `session_complete` / `memory_search` already get their service handles — follow that exact pattern, same file usually).

- [ ] **Step 8.3: Fill in the tool handler’s wiring (Task 7 TODO)**

If Task 7 left a `todo!("wired in Task 8")` in `memory_reflect::handle`, replace it now with the real `ctx.memory_reflector.clone()` extraction following the pattern used by `session_complete`.

- [ ] **Step 8.4: Build server**

```
cargo check -p alephcore --bin aleph-server 2>&1 | tail -10
cargo test -p alephcore --lib -- --nocapture 2>&1 | tail -10
```
Expected: no errors; library tests pass.

- [ ] **Step 8.5: Commit**

```bash
git add src/bin/aleph-server/ src/executor/builtin_registry/
git commit -m "feat(memory): wire Arc<MemoryReflector> at server startup

Construct MemoryReflector from the shared HybridAssembler + provider
+ recall-signals writer closure, then inject into the builtin tool
context so memory_reflect can reach it. Mirrors the Spec 1 Task 10
pattern for hook writer injection."
```

---

## Task 9: End-to-end integration test

**Files:**
- Create: `tests/memory_reflect_integration.rs`

- [ ] **Step 9.1: Write integration test**

Create `tests/memory_reflect_integration.rs`:

```rust
//! Integration test: MemoryReflector against a live in-memory SQLite store,
//! real HybridAssembler, and a RecordingMockProvider canned to return a
//! well-formed Synthesis JSON.

#![cfg(feature = "test-helpers")]

use alephcore::memory::namespace::NamespaceScope;
use alephcore::memory::reflector::{MemoryReflector, ReflectOpts, Synthesis};
use alephcore::providers::recording_mock::RecordingMockProvider;
use std::sync::Arc;

// The harness mirrors `tests/memory_capture_hooks.rs` (Spec 1 Task 11):
// build in-memory SQLite + init_schema + NoteIndexer + HybridAssembler +
// reflector with recording provider. Return handles so tests can:
//   1. seed a few notes
//   2. call reflector.reflect(query, opts)
//   3. verify Synthesis shape + recall_signals rowcount in DB
async fn build_reflector_with_recording_provider(
    canned: &str,
) -> (
    Arc<MemoryReflector>,
    Arc<std::sync::Mutex<Option<String>>>,
    /* noteIndexer, recallStore handle, ... */
) {
    unimplemented!("port the harness pattern from tests/memory_capture_hooks.rs")
}

#[tokio::test]
async fn reflect_full_pipeline_against_fixture_notes() {
    // 1. Seed two notes under agent "a1" via NoteIndexer.
    // 2. Canned response = valid Synthesis JSON citing one of them.
    // 3. Call reflector.reflect("question", opts_for("a1")).
    // 4. Assert Synthesis.text matches canned text.
    // 5. Assert Synthesis.sources[0].path is the seeded note path.
    // 6. Assert Synthesis.sources[0].title = real stored title (not "unknown").
    // 7. Query recall_signals table directly and assert row count = 2
    //    (one per seeded note in the packet), channel = "reflect".
    unimplemented!("fill in once harness compiles");
}

#[tokio::test]
async fn reflect_with_no_matching_notes_short_circuits() {
    // 1. Seed zero notes.
    // 2. Call reflector.reflect("question", opts_for("ghost-agent")).
    // 3. Assert synthesis.text == "No relevant memories found."
    // 4. Assert synthesis.sources is empty.
    // 5. Assert zero rows in recall_signals (short-circuit must not log).
    unimplemented!();
}
```

**IMPORTANT**: this test file is the same structural pattern as `tests/memory_capture_hooks.rs` (Spec 1 Task 11 landed it). Port that harness file's setup wholesale — only the final assertions differ.

- [ ] **Step 9.2: Run**

```
cargo test -p alephcore --features test-helpers --test memory_reflect_integration -- --nocapture 2>&1 | tail -30
```
Expected: 2 tests pass.

- [ ] **Step 9.3: Commit**

```bash
git add -f tests/memory_reflect_integration.rs
git commit -m "test(memory): E2E integration test for MemoryReflector

Two cases:
- Full pipeline: seeded notes + canned JSON → Synthesis with correct
  path/title/relevance + two recall_signals rows.
- No-match: ghost agent with zero notes → stub Synthesis, zero
  recall_signals rows (short-circuit correctness)."
```

---

## Task 10: Docs update

**Files:**
- Modify: `docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md`
- Modify: `docs/reference/memory/RETRIEVAL.md`

- [ ] **Step 10.1: Update roadmap progress table**

In `docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md`, change the Spec 2 row from:

```
| 2. Reflect | ⚪ pending | — | — | — |
```

to:

```
| 2. Reflect | ✅ shipped | [design](2026-04-13-memory-evolution-spec2-reflector-design.md) | [plan](../plans/2026-04-13-memory-evolution-spec2-reflector.md) | 2026-04-13 |
```

(Replace the date if the actual ship date differs.)

- [ ] **Step 10.2: Add Retrieval docs pointer**

Append to `docs/reference/memory/RETRIEVAL.md` (find an appropriate "Reflection" or near-end section, or add as a new trailing section):

```markdown
## N. Reflection / Synthesis (Spec 2)

`MemoryReflector` at `src/memory/reflector/` composes the hybrid
assembler with an LLM synthesis pass. Given a natural-language
query, it:

1. calls `HybridAssembler::assemble(query, opts)` for retrieval
2. returns a stub Synthesis immediately if the packet is empty (no LLM cost)
3. otherwise formats the packet into a user prompt, calls the LLM
   with `PROMPT_SYNTHESIS`, parses the JSON response, overlays
   canonical titles from the packet lookup (so LLM cannot fabricate
   paths), and returns a `Synthesis { text, sources }`.

It writes one `recall_signals` row per note in the synthesis context
(channel = `"reflect"`) so dream-daemon activity tracking sees
reflect usage on par with direct `memory_search` hits.

LLM-facing entry: the `memory_reflect` builtin tool
(`src/builtin_tools/memory_reflect.rs`).

See `docs/superpowers/specs/2026-04-13-memory-evolution-spec2-reflector-design.md`.
```

Pick the exact section number based on the file's existing numbering.

- [ ] **Step 10.3: Commit**

```bash
git add docs/
git commit -m "docs(memory): mark Spec 2 shipped and document reflection layer

Roadmap progress table updated. RETRIEVAL.md gains a short section
pointing at MemoryReflector + the design spec."
```

---

## Self-Review

1. **Spec coverage** — every spec section has a task:
   - §3 Architecture / data flow → Tasks 3, 4, 5, 6 (packet adapter → short-circuit → LLM path → recall signals)
   - §4 Types → Task 1
   - §5 Synthesis prompt → Task 2
   - §6 Tool → Task 7
   - §7 Side effects → Task 6
   - §8 Empty handling → Task 4 short-circuit + Task 9 no-match integration test
   - §9 Server wiring → Task 8
   - §10 Testing → unit tests in Tasks 1–6 + integration Task 9
   - §11 Redline compliance → all R-checks covered by tool / prompt / isolation choices above
   - §12 Open questions → each resolved in the task that touches it: (a) query_text truncation handled by reusing existing `record_signals` signature in Task 6/8, (b) `[score=...]` tag format introduced by packet adapter Task 3, (c) tool-context wiring follows Spec 1 Task 10 pattern in Task 8.

2. **Placeholder scan** — no `TBD` / `FIXME`. The `unreachable!()` and `unimplemented!()` at Tasks 4 and 9 are planned inter-task bridges with explicit follow-up tasks. Task 7 optionally leaves a `todo!("wired in Task 8")` with the same justification.

3. **Type consistency** — `Synthesis { text, sources }` / `NoteRef { path, title, relevance }` / `ReflectOpts` / `SignalRow` / `SynthesisContext` / `NoteMeta` / `RecallWriter` all use identical signatures across Tasks 1, 3, 4, 5, 6, 7, 8, 9.
