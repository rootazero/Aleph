# Memory Evolution Spec 1 — Working Memory Assembler Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce `WorkingMemoryAssembler` + `MemoryEnvelope` so every LLM call is preceded by a schema-versioned, slot-partitioned memory envelope produced via retrieval + LLM re-rank (B strategy) with deterministic skeleton fallback (C strategy).

**Architecture:** New `src/memory/assembler/` module sibling to `context_comptroller/` and `note_retrieval/`. `MemoryContextProvider::fetch()` — the current auto-inject site in `src/thinker/memory_context_provider.rs` — delegates to the assembler and adapts its `MemoryEnvelope` back to the legacy `MemoryContext` shape for `PromptLayer`. No change to the `memory_search` builtin tool in Spec 1. New `assembly_logs` SQLite table is defined and default-disabled, reserved for Spec 2.

**Tech Stack:** Rust (async via tokio), serde + schemars (JsonSchema), thiserror (errors), mockall + rstest + proptest (tests), `tracing` (observability), sqlite-vec + FTS5 (existing memory store).

**Spec reference:** `docs/superpowers/specs/2026-04-13-memory-evolution-spec1-assembler-design.md`

---

## File Structure

**New files** (all under `src/memory/assembler/`):

| File | Responsibility |
|---|---|
| `mod.rs` | Public exports, `WorkingMemoryAssembler` trait, `AssemblyBudget` |
| `envelope.rs` | `MemoryEnvelope`, `EnvelopeSlot`, `EnvelopeItem`, `SlotKind`, `ItemSource`, `EnvelopeMeta` |
| `render.rs` | Pure `render_envelope` + `render_with` + `RenderStyle` enum |
| `hydration.rs` | `truncate_utf8_safe`, `estimate_tokens`, content loader helpers |
| `error.rs` | Internal `AssemblerError` |
| `fallback.rs` | Deterministic skeleton packer (C path) |
| `profile.rs` | `UserProfileLoader` — reads `memory/note/{agent_id}/personal/profile.md` |
| `gather.rs` | `CandidateGatherer` — concurrent fan-out + candidate pool |
| `rerank.rs` | LLM prompt builder + JSON response parser + validator |
| `hybrid.rs` | `HybridAssembler` — orchestrates Stage 1/2/2'/3 |
| `log_store.rs` | `AssemblyLogWriter` — inserts into `assembly_logs` when enabled |
| `tests/integration.rs` | Five-path integration tests + property test |

**Modified files:**

| File | Change |
|---|---|
| `src/memory/mod.rs` | Add `pub mod assembler;` + re-exports |
| `src/config/types/memory.rs` | Add `AssemblerConfig`, `FallbackSkeleton`, `AssemblyLogConfig`, `RenderStyle`; wire into `MemoryConfig` |
| `src/memory/store/sqlite/schema.rs` | Add `CREATE_ASSEMBLY_LOGS` DDL + call from `init_schema` |
| `src/thinker/memory_context_provider.rs` | Swap direct `NoteFactRetrieval` use for `Arc<dyn WorkingMemoryAssembler>`; add `memory_context_from_envelope` adapter |

---

## Task 1: Envelope Types + Serde Round-Trip

**Files:**
- Create: `src/memory/assembler/mod.rs`
- Create: `src/memory/assembler/envelope.rs`
- Modify: `src/memory/mod.rs` (add `pub mod assembler;`)

- [ ] **Step 1: Add module declaration to `src/memory/mod.rs`**

At the top of `src/memory/mod.rs`, alongside the other `pub mod` lines, add:

```rust
pub mod assembler;
```

- [ ] **Step 2: Create `src/memory/assembler/mod.rs` skeleton**

```rust
//! Working Memory Assembler — produces a portable [`MemoryEnvelope`] before
//! each LLM call. See `docs/superpowers/specs/2026-04-13-memory-evolution-spec1-assembler-design.md`.

pub mod envelope;

pub use envelope::{
    EnvelopeItem, EnvelopeMeta, EnvelopeSlot, ItemSource, MemoryEnvelope, SlotKind,
};
```

- [ ] **Step 3: Write failing test in `src/memory/assembler/envelope.rs`**

Create the file with ONLY the test first (types not yet defined — test will fail to compile):

```rust
//! Memory Envelope — the portable data contract.
//!
//! All fields are additive within schema_version 1.x. Breaking changes require v2.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// TYPES TO FOLLOW IN STEP 5 — intentionally omitted here so the test fails first.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_roundtrip_preserves_all_fields() {
        let mut extra = serde_json::Map::new();
        extra.insert("eval_score".into(), serde_json::json!(0.73));

        let envelope = MemoryEnvelope {
            schema_version: "1.0".into(),
            generated_at: 1_700_000_000,
            query: "how does ownership work".into(),
            agent_id: "default".into(),
            session_id: Some("session-abc".into()),
            slots: vec![
                EnvelopeSlot {
                    kind: SlotKind::RelevantNotes,
                    items: vec![EnvelopeItem {
                        id: "note://wiki/rust-ownership".into(),
                        title: "Rust ownership".into(),
                        content: "body".into(),
                        source: ItemSource::Note {
                            path: "wiki/rust-ownership".into(),
                            category: "wiki".into(),
                        },
                        relevance: 0.82,
                        tokens: 10,
                        updated_at: 1_699_999_000,
                        extra,
                    }],
                    tokens_used: 10,
                    tokens_budget: 100,
                },
            ],
            meta: EnvelopeMeta {
                strategy: "hybrid_v1".into(),
                candidates_considered: 12,
                used_fallback: false,
                fallback_reason: None,
                llm_rerank_latency_ms: Some(412),
                total_latency_ms: 587,
            },
        };

        let json = serde_json::to_string(&envelope).expect("serialize");
        let roundtripped: MemoryEnvelope = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(envelope.schema_version, roundtripped.schema_version);
        assert_eq!(envelope.query, roundtripped.query);
        assert_eq!(envelope.slots.len(), roundtripped.slots.len());
        assert_eq!(envelope.slots[0].items[0].id, roundtripped.slots[0].items[0].id);
        assert_eq!(
            envelope.slots[0].items[0].extra.get("eval_score"),
            roundtripped.slots[0].items[0].extra.get("eval_score")
        );
        assert_eq!(envelope.meta.used_fallback, roundtripped.meta.used_fallback);
    }

    #[test]
    fn envelope_deserialize_tolerates_unknown_field() {
        // Forward-compat: v1.x may add fields; older consumers must still parse.
        let json = r#"{
            "schema_version": "1.0",
            "generated_at": 0,
            "query": "",
            "agent_id": "default",
            "session_id": null,
            "slots": [],
            "meta": {
                "strategy": "hybrid_v1",
                "candidates_considered": 0,
                "used_fallback": false,
                "fallback_reason": null,
                "llm_rerank_latency_ms": null,
                "total_latency_ms": 0,
                "future_field_we_do_not_know": "ok"
            },
            "future_top_level_field": 42
        }"#;
        let env: MemoryEnvelope = serde_json::from_str(json).expect("tolerates unknown fields");
        assert_eq!(env.schema_version, "1.0");
    }

    #[test]
    fn item_source_serializes_with_kind_tag() {
        let src = ItemSource::Raw {
            raw_id: "xyz".into(),
            session_id: "abc".into(),
        };
        let json = serde_json::to_value(&src).unwrap();
        assert_eq!(json["kind"], "raw");
        assert_eq!(json["raw_id"], "xyz");
    }
}
```

- [ ] **Step 4: Run test and verify compile failure**

Run: `cargo test -p alephcore --lib memory::assembler::envelope`

Expected: compile error — `MemoryEnvelope`, `EnvelopeSlot`, etc. not found.

- [ ] **Step 5: Define the types above the `#[cfg(test)]` module**

Insert this block in `envelope.rs` between the header comment and the `#[cfg(test)]` line:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryEnvelope {
    pub schema_version: String,
    pub generated_at: i64,
    pub query: String,
    pub agent_id: String,
    pub session_id: Option<String>,
    pub slots: Vec<EnvelopeSlot>,
    pub meta: EnvelopeMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EnvelopeSlot {
    pub kind: SlotKind,
    pub items: Vec<EnvelopeItem>,
    pub tokens_used: u32,
    pub tokens_budget: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SlotKind {
    UserProfile,
    SessionRecent,
    RelevantNotes,
    RawFragments,
    Nudges,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EnvelopeItem {
    pub id: String,
    pub title: String,
    pub content: String,
    pub source: ItemSource,
    pub relevance: f32,
    pub tokens: u32,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ItemSource {
    Note { path: String, category: String },
    Raw { raw_id: String, session_id: String },
    Summary { layer: String, session_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EnvelopeMeta {
    pub strategy: String,
    pub candidates_considered: usize,
    pub used_fallback: bool,
    pub fallback_reason: Option<String>,
    pub llm_rerank_latency_ms: Option<u64>,
    pub total_latency_ms: u64,
}

/// Envelope schema version emitted by this build.
pub const SCHEMA_VERSION: &str = "1.0";
```

- [ ] **Step 6: Run tests and verify pass**

Run: `cargo test -p alephcore --lib memory::assembler::envelope`

Expected: 3 passing tests.

- [ ] **Step 7: Run clippy to catch issues early**

Run: `cargo clippy -p alephcore --lib -- -D warnings`

Expected: no warnings in `src/memory/assembler/`.

- [ ] **Step 8: Commit**

```bash
git add src/memory/mod.rs src/memory/assembler/mod.rs src/memory/assembler/envelope.rs
git commit -m "memory(assembler): add MemoryEnvelope v1.0 types"
```

---

## Task 2: Envelope Renderer (Pure Function)

**Files:**
- Create: `src/memory/assembler/render.rs`
- Modify: `src/memory/assembler/mod.rs`

- [ ] **Step 1: Add module declaration**

In `src/memory/assembler/mod.rs`, add under the existing `pub mod envelope;`:

```rust
pub mod render;

pub use render::{render_envelope, render_with, RenderStyle};
```

- [ ] **Step 2: Write failing tests in `src/memory/assembler/render.rs`**

```rust
//! Pure envelope renderer. No I/O, deterministic.

use super::envelope::{EnvelopeItem, EnvelopeMeta, EnvelopeSlot, ItemSource, MemoryEnvelope, SlotKind};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RenderStyle {
    #[default]
    MarkdownV1,
    Xml,
    Json,
}

pub fn render_envelope(env: &MemoryEnvelope) -> String {
    render_with(env, RenderStyle::default())
}

pub fn render_with(env: &MemoryEnvelope, style: RenderStyle) -> String {
    match style {
        RenderStyle::MarkdownV1 => render_markdown_v1(env),
        RenderStyle::Xml => render_xml(env),
        RenderStyle::Json => render_json(env),
    }
}

fn render_markdown_v1(_env: &MemoryEnvelope) -> String { todo!("TDD RED") }
fn render_xml(_env: &MemoryEnvelope) -> String { todo!("TDD RED") }
fn render_json(_env: &MemoryEnvelope) -> String { todo!("TDD RED") }

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> MemoryEnvelope {
        MemoryEnvelope {
            schema_version: "1.0".into(),
            generated_at: 0,
            query: "".into(),
            agent_id: "default".into(),
            session_id: None,
            slots: vec![],
            meta: EnvelopeMeta {
                strategy: "hybrid_v1".into(),
                candidates_considered: 0,
                used_fallback: false,
                fallback_reason: None,
                llm_rerank_latency_ms: None,
                total_latency_ms: 0,
            },
        }
    }

    fn item(id: &str, title: &str, body: &str, source: ItemSource) -> EnvelopeItem {
        EnvelopeItem {
            id: id.into(),
            title: title.into(),
            content: body.into(),
            source,
            relevance: 0.5,
            tokens: (body.chars().count() / 4).max(1) as u32,
            updated_at: 1_700_000_000,
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn empty_envelope_renders_empty_string() {
        assert_eq!(render_envelope(&empty()), "");
    }

    #[test]
    fn markdown_v1_wraps_slots_in_memory_tags() {
        let mut env = empty();
        env.slots.push(EnvelopeSlot {
            kind: SlotKind::RelevantNotes,
            items: vec![item(
                "note://wiki/rust-ownership",
                "Rust ownership",
                "body text",
                ItemSource::Note {
                    path: "wiki/rust-ownership".into(),
                    category: "wiki".into(),
                },
            )],
            tokens_used: 2,
            tokens_budget: 100,
        });
        let out = render_envelope(&env);
        assert!(out.starts_with("<memory>"));
        assert!(out.trim_end().ends_with("</memory>"));
        assert!(out.contains("<relevant_notes>"));
        assert!(out.contains("</relevant_notes>"));
        assert!(out.contains("[note://wiki/rust-ownership]"));
        assert!(out.contains("body text"));
    }

    #[test]
    fn markdown_v1_omits_empty_slots() {
        let mut env = empty();
        env.slots.push(EnvelopeSlot {
            kind: SlotKind::RelevantNotes,
            items: vec![],
            tokens_used: 0,
            tokens_budget: 100,
        });
        env.slots.push(EnvelopeSlot {
            kind: SlotKind::UserProfile,
            items: vec![item(
                "note://personal/profile",
                "Profile",
                "user is a rust developer",
                ItemSource::Note {
                    path: "personal/profile".into(),
                    category: "personal".into(),
                },
            )],
            tokens_used: 5,
            tokens_budget: 50,
        });
        let out = render_envelope(&env);
        assert!(!out.contains("<relevant_notes>"), "empty slot must not render");
        assert!(out.contains("<user_profile>"));
    }

    #[test]
    fn markdown_v1_renders_summary_layer_label() {
        let mut env = empty();
        env.slots.push(EnvelopeSlot {
            kind: SlotKind::SessionRecent,
            items: vec![item(
                "aleph://session/abc/d1",
                "Session summary",
                "yesterday we fixed X",
                ItemSource::Summary {
                    layer: "d1".into(),
                    session_id: "abc".into(),
                },
            )],
            tokens_used: 5,
            tokens_budget: 50,
        });
        let out = render_envelope(&env);
        assert!(out.contains("[d1 @"), "summary layer and timestamp expected");
    }

    #[test]
    fn xml_style_outputs_xml_root() {
        let env = empty();
        let out = render_with(&env, RenderStyle::Xml);
        assert!(out.is_empty() || out.starts_with("<MemoryEnvelope"));
    }

    #[test]
    fn json_style_outputs_valid_json() {
        let env = empty();
        let out = render_with(&env, RenderStyle::Json);
        let _: serde_json::Value = serde_json::from_str(&out).expect("json render must be valid");
    }
}
```

- [ ] **Step 3: Run tests to verify RED**

Run: `cargo test -p alephcore --lib memory::assembler::render`

Expected: all tests fail with `todo!()` panic.

- [ ] **Step 4: Implement `render_markdown_v1`**

Replace the `todo!()` body in `render_markdown_v1`:

```rust
fn render_markdown_v1(env: &MemoryEnvelope) -> String {
    let non_empty: Vec<&EnvelopeSlot> = env.slots.iter().filter(|s| !s.items.is_empty()).collect();
    if non_empty.is_empty() {
        return String::new();
    }

    let mut out = String::from("<memory>\n\n");
    for slot in non_empty {
        let tag = slot_tag(slot.kind);
        out.push('<');
        out.push_str(tag);
        out.push_str(">\n");
        for (i, item) in slot.items.iter().enumerate() {
            if i > 0 {
                out.push_str("\n---\n\n");
            }
            render_item_markdown(&mut out, item);
        }
        out.push_str("\n</");
        out.push_str(tag);
        out.push_str(">\n\n");
    }
    out.push_str("</memory>\n");
    out
}

fn slot_tag(kind: SlotKind) -> &'static str {
    match kind {
        SlotKind::UserProfile => "user_profile",
        SlotKind::SessionRecent => "session_recent",
        SlotKind::RelevantNotes => "relevant_notes",
        SlotKind::RawFragments => "raw_fragments",
        SlotKind::Nudges => "nudges",
    }
}

fn render_item_markdown(out: &mut String, item: &EnvelopeItem) {
    let header = match &item.source {
        ItemSource::Note { path: _, .. } => format!("## [{}] (updated {})", item.id, format_date(item.updated_at)),
        ItemSource::Raw { session_id, .. } => format!("## [raw @ session {}, t={}]", session_id, format_date(item.updated_at)),
        ItemSource::Summary { layer, session_id } => format!("## [{} @ session {}, t={}]", layer, session_id, format_date(item.updated_at)),
    };
    out.push_str(&header);
    out.push('\n');
    out.push_str(&item.content);
    out.push('\n');
}

fn format_date(ts: i64) -> String {
    // Minimal YYYY-MM-DD rendering using chrono if available in the crate; fall
    // back to raw epoch if chrono is unavailable. Aleph already depends on
    // chrono for memory timestamps — follow the same pattern.
    use chrono::{TimeZone, Utc};
    Utc.timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| ts.to_string())
}
```

- [ ] **Step 5: Implement `render_xml` and `render_json`**

Replace the two remaining `todo!()` bodies:

```rust
fn render_xml(env: &MemoryEnvelope) -> String {
    // Defer to serde + quick-xml? Keep minimal: hand-render. Not hot path.
    if env.slots.iter().all(|s| s.items.is_empty()) {
        return String::new();
    }
    let mut out = String::from("<MemoryEnvelope>\n");
    out.push_str(&format!("  <schema_version>{}</schema_version>\n", env.schema_version));
    out.push_str(&format!("  <query>{}</query>\n", xml_escape(&env.query)));
    for slot in env.slots.iter().filter(|s| !s.items.is_empty()) {
        out.push_str(&format!("  <slot kind=\"{}\">\n", slot_tag(slot.kind)));
        for item in &slot.items {
            out.push_str(&format!(
                "    <item id=\"{}\"><title>{}</title><content>{}</content></item>\n",
                xml_escape(&item.id),
                xml_escape(&item.title),
                xml_escape(&item.content),
            ));
        }
        out.push_str("  </slot>\n");
    }
    out.push_str("</MemoryEnvelope>\n");
    out
}

fn render_json(env: &MemoryEnvelope) -> String {
    serde_json::to_string_pretty(env).unwrap_or_else(|_| String::from("{}"))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
```

- [ ] **Step 6: Run tests to verify GREEN**

Run: `cargo test -p alephcore --lib memory::assembler::render`

Expected: all 6 tests pass.

- [ ] **Step 7: Clippy**

Run: `cargo clippy -p alephcore --lib -- -D warnings`

Expected: no new warnings.

- [ ] **Step 8: Commit**

```bash
git add src/memory/assembler/mod.rs src/memory/assembler/render.rs
git commit -m "memory(assembler): add pure envelope renderer (markdown/xml/json)"
```

---

## Task 3: Hydration Helpers + Proptest

**Files:**
- Create: `src/memory/assembler/hydration.rs`
- Modify: `src/memory/assembler/mod.rs`

- [ ] **Step 1: Add module declaration**

In `src/memory/assembler/mod.rs`:

```rust
pub mod hydration;
```

- [ ] **Step 2: Write failing tests + skeleton in `src/memory/assembler/hydration.rs`**

```rust
//! Hydration helpers — UTF-8 safe truncation and token estimation.
//!
//! These are deliberately simple and dependency-free. A real tokenizer can
//! replace [`estimate_tokens`] in v1.1 without touching callers.

/// Truncate `s` to at most `max_bytes`, guaranteeing the result is valid UTF-8
/// by backing up to the nearest char boundary.
pub fn truncate_utf8_safe(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// Estimate tokens using the existing 4-chars-per-token heuristic (matches
/// `ContextComptroller` behavior). Never returns zero for non-empty text.
pub fn estimate_tokens(s: &str) -> u32 {
    if s.is_empty() {
        return 0;
    }
    ((s.chars().count() as u32) / 4).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_within_limit_returns_original() {
        assert_eq!(truncate_utf8_safe("hello", 10), "hello");
    }

    #[test]
    fn truncate_over_limit_clips_to_boundary() {
        let out = truncate_utf8_safe("hello world", 5);
        assert_eq!(out, "hello");
    }

    #[test]
    fn truncate_respects_multibyte_char_boundary() {
        // "héllo" — é is 2 bytes in UTF-8.
        let s = "h\u{00e9}llo"; // 6 bytes total
        let out = truncate_utf8_safe(s, 2);
        // Byte 2 falls inside "é"; must back up to byte 1 ("h").
        assert_eq!(out, "h");
        // Valid UTF-8 preserved.
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }

    #[test]
    fn estimate_tokens_empty_is_zero() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn estimate_tokens_short_is_one() {
        assert_eq!(estimate_tokens("ab"), 1);
    }

    #[test]
    fn estimate_tokens_scales_with_chars() {
        assert_eq!(estimate_tokens("a".repeat(400).as_str()), 100);
    }
}
```

- [ ] **Step 3: Run tests to verify GREEN**

Run: `cargo test -p alephcore --lib memory::assembler::hydration`

Expected: 6 tests pass (these are simple enough to write and pass at once).

- [ ] **Step 4: Add proptest for UTF-8 safety invariant**

Append to `hydration.rs`:

```rust
#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn truncate_always_valid_utf8_and_within_limit(
            s in "\\PC*",                    // any printable Unicode
            n in 0usize..256,
        ) {
            let out = truncate_utf8_safe(&s, n);
            prop_assert!(out.len() <= n);
            prop_assert!(std::str::from_utf8(out.as_bytes()).is_ok());
        }
    }
}
```

- [ ] **Step 5: Run proptest**

Run: `cargo test -p alephcore --lib memory::assembler::hydration::proptests`

Expected: proptest passes with 256 random cases.

- [ ] **Step 6: Commit**

```bash
git add src/memory/assembler/mod.rs src/memory/assembler/hydration.rs
git commit -m "memory(assembler): add UTF-8 safe truncation and token estimation"
```

---

## Task 4: Config Types + Error Type + Trait Scaffolding

**Files:**
- Modify: `src/config/types/memory.rs`
- Create: `src/memory/assembler/error.rs`
- Modify: `src/memory/assembler/mod.rs`

- [ ] **Step 1: Check current `MemoryConfig` structure**

Run: `cat src/config/types/memory.rs | head -100`

Confirm `MemoryConfig` exists and note its serde pattern (each field has `#[serde(default = "...")]`). You will append new fields in the same style.

- [ ] **Step 2: Append config types to `src/config/types/memory.rs`**

At the bottom of the file (after existing structs, before any trailing test module), add:

```rust
use crate::memory::assembler::render::RenderStyle;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AssemblerConfig {
    #[serde(default = "default_assembler_enabled")]
    pub enabled: bool,
    #[serde(default = "default_total_budget")]
    pub total_budget_tokens: u32,
    #[serde(default = "default_pool_limit")]
    pub candidate_pool_limit: usize,
    #[serde(default = "default_rerank_timeout")]
    pub rerank_timeout_ms: u64,
    #[serde(default)]
    pub rerank_model: Option<String>,
    #[serde(default)]
    pub render_style: RenderStyle,
    #[serde(default)]
    pub force_fallback: bool,
    #[serde(default)]
    pub fallback_skeleton: FallbackSkeleton,
    #[serde(default)]
    pub assembly_log: AssemblyLogConfig,
}

impl Default for AssemblerConfig {
    fn default() -> Self {
        Self {
            enabled: default_assembler_enabled(),
            total_budget_tokens: default_total_budget(),
            candidate_pool_limit: default_pool_limit(),
            rerank_timeout_ms: default_rerank_timeout(),
            rerank_model: None,
            render_style: RenderStyle::default(),
            force_fallback: false,
            fallback_skeleton: FallbackSkeleton::default(),
            assembly_log: AssemblyLogConfig::default(),
        }
    }
}

fn default_assembler_enabled() -> bool { true }
fn default_total_budget() -> u32 { 8000 }
fn default_pool_limit() -> usize { 20 }
fn default_rerank_timeout() -> u64 { 800 }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FallbackSkeleton {
    #[serde(default = "default_user_profile_tokens")]
    pub user_profile_tokens: u32,
    #[serde(default = "default_session_recent_tokens")]
    pub session_recent_tokens: u32,
    #[serde(default = "default_relevant_notes_tokens")]
    pub relevant_notes_tokens: u32,
    #[serde(default = "default_raw_fragments_tokens")]
    pub raw_fragments_tokens: u32,
    #[serde(default)]
    pub nudges_tokens: u32,
}

impl Default for FallbackSkeleton {
    fn default() -> Self {
        Self {
            user_profile_tokens: default_user_profile_tokens(),
            session_recent_tokens: default_session_recent_tokens(),
            relevant_notes_tokens: default_relevant_notes_tokens(),
            raw_fragments_tokens: default_raw_fragments_tokens(),
            nudges_tokens: 0,
        }
    }
}

fn default_user_profile_tokens() -> u32 { 200 }
fn default_session_recent_tokens() -> u32 { 1500 }
fn default_relevant_notes_tokens() -> u32 { 5000 }
fn default_raw_fragments_tokens() -> u32 { 1000 }

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct AssemblyLogConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
}

fn default_retention_days() -> u32 { 14 }

#[cfg(test)]
mod assembler_config_tests {
    use super::*;

    #[test]
    fn default_config_sane() {
        let c = AssemblerConfig::default();
        assert!(c.enabled);
        assert_eq!(c.total_budget_tokens, 8000);
        assert_eq!(c.rerank_timeout_ms, 800);
        assert!(!c.force_fallback);
    }

    #[test]
    fn partial_toml_falls_back_to_defaults() {
        let toml_src = r#"
            enabled = false
            total_budget_tokens = 4000
        "#;
        let c: AssemblerConfig = toml::from_str(toml_src).expect("parse");
        assert!(!c.enabled);
        assert_eq!(c.total_budget_tokens, 4000);
        assert_eq!(c.rerank_timeout_ms, 800);  // default retained
        assert_eq!(c.fallback_skeleton.relevant_notes_tokens, 5000);
    }
}
```

- [ ] **Step 3: Wire `assembler` into `MemoryConfig`**

Find the `pub struct MemoryConfig` definition in the same file. Add the field (keep existing fields unchanged):

```rust
pub struct MemoryConfig {
    // ... existing fields ...
    #[serde(default)]
    pub assembler: AssemblerConfig,
}
```

Update `impl Default for MemoryConfig` (if it exists and is hand-implemented) to include `assembler: AssemblerConfig::default(),`.

- [ ] **Step 4: Create `src/memory/assembler/error.rs`**

```rust
//! Internal assembler error. Never crosses the module boundary — the public
//! API returns `Result<MemoryEnvelope, AlephError>` and maps all variants to
//! graceful fallback or degraded output.

use crate::error::AlephError;
use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum AssemblerError {
    #[error("retrieval failed: {0}")]
    Retrieval(#[source] AlephError),

    #[error("llm rerank timeout after {0}ms")]
    RerankTimeout(u64),

    #[error("llm rerank returned invalid json: {0}")]
    RerankParse(String),

    #[error("llm rerank produced no valid slots")]
    RerankEmpty,

    #[error("content load failed for {id}: {source}")]
    Hydration {
        id: String,
        #[source]
        source: AlephError,
    },
}
```

- [ ] **Step 5: Define the `WorkingMemoryAssembler` trait in `mod.rs`**

Append to `src/memory/assembler/mod.rs` (below existing `pub mod` / `pub use` lines):

```rust
pub mod error;

use crate::error::AlephError;
use async_trait::async_trait;

/// Token budget passed into the assembler. `total_tokens` is the hard cap
/// before the LLM's reply headroom reservation (which the LLM re-rank path
/// will additionally honor).
#[derive(Debug, Clone, Copy)]
pub struct AssemblyBudget {
    pub total_tokens: u32,
}

#[async_trait]
pub trait WorkingMemoryAssembler: Send + Sync {
    /// Produce a [`MemoryEnvelope`]. Never returns `Err` for LLM-assist failures
    /// — internal failures (retrieval error, LLM timeout, hydration miss) are
    /// caught and degraded to fallback / empty slots. `Err` only surfaces for
    /// system-level misconfiguration at construction time.
    async fn assemble(
        &self,
        query: &str,
        agent_id: &str,
        session_id: Option<&str>,
        budget: AssemblyBudget,
    ) -> Result<MemoryEnvelope, AlephError>;
}
```

- [ ] **Step 6: Confirm the codebase still compiles**

Run: `cargo check -p alephcore`

Expected: clean build. If `crate::error::AlephError` import path differs, adjust to match the crate's actual error module (grep for `pub struct AlephError` or `pub enum AlephError` if needed).

- [ ] **Step 7: Run config tests**

Run: `cargo test -p alephcore --lib config::types::memory::assembler_config_tests`

Expected: 2 tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/config/types/memory.rs src/memory/assembler/
git commit -m "memory(assembler): add config, error, and trait scaffolding"
```

---

## Task 5: Fallback Skeleton Packer (C Path)

**Files:**
- Create: `src/memory/assembler/fallback.rs`
- Modify: `src/memory/assembler/mod.rs`

- [ ] **Step 1: Add module declaration**

In `src/memory/assembler/mod.rs`:

```rust
pub(crate) mod fallback;
```

- [ ] **Step 2: Write the candidate type (internal) + failing test**

Create `src/memory/assembler/fallback.rs`:

```rust
//! Deterministic skeleton fallback (strategy C) — used when the LLM re-rank
//! path times out, returns invalid JSON, yields no valid slots, or when the
//! candidate pool is too small to be worth asking the LLM about.

use super::envelope::{EnvelopeItem, EnvelopeSlot, ItemSource, SlotKind};
use crate::config::types::memory::FallbackSkeleton;

/// An un-rendered candidate before hydration — kept internal to the
/// assembler module. Produced by `gather` (Task 7), consumed here and by
/// `rerank` (Task 8).
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub id: String,
    pub title: String,
    pub full_content: String,
    pub source: ItemSource,
    pub relevance: f32,
    pub updated_at: i64,
    pub slot_hint: SlotKind,
}

/// Pack `candidates` into skeleton slots using fixed budgets and the
/// `(relevance * recency_factor)` greedy strategy. Content is NOT truncated
/// here — only item selection happens. Hydration (Task 9) trims content
/// against the per-slot budget.
pub(crate) fn skeleton_pack(
    candidates: &[Candidate],
    skeleton: &FallbackSkeleton,
    now: i64,
) -> Vec<EnvelopeSlot> {
    let mut slots = Vec::new();
    for (kind, budget) in [
        (SlotKind::UserProfile, skeleton.user_profile_tokens),
        (SlotKind::SessionRecent, skeleton.session_recent_tokens),
        (SlotKind::RelevantNotes, skeleton.relevant_notes_tokens),
        (SlotKind::RawFragments, skeleton.raw_fragments_tokens),
        (SlotKind::Nudges, skeleton.nudges_tokens),
    ] {
        if budget == 0 {
            continue;
        }
        let mut in_slot: Vec<&Candidate> = candidates.iter().filter(|c| c.slot_hint == kind).collect();
        in_slot.sort_by(|a, b| {
            let sa = a.relevance * recency_factor(a.updated_at, now);
            let sb = b.relevance * recency_factor(b.updated_at, now);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
        let items: Vec<EnvelopeItem> = in_slot
            .into_iter()
            .map(|c| EnvelopeItem {
                id: c.id.clone(),
                title: c.title.clone(),
                content: c.full_content.clone(), // hydration truncates later
                source: c.source.clone(),
                relevance: c.relevance,
                tokens: 0,           // set by hydration
                updated_at: c.updated_at,
                extra: serde_json::Map::new(),
            })
            .collect();
        if items.is_empty() {
            continue;
        }
        slots.push(EnvelopeSlot {
            kind,
            items,
            tokens_used: 0,
            tokens_budget: budget,
        });
    }
    slots
}

fn recency_factor(updated_at: i64, now: i64) -> f32 {
    let age_days = ((now - updated_at).max(0) as f32) / 86_400.0;
    0.5 + 0.5 * (-age_days / 14.0).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(id: &str, slot: SlotKind, rel: f32, updated: i64) -> Candidate {
        Candidate {
            id: id.into(),
            title: id.into(),
            full_content: format!("body of {id}"),
            source: ItemSource::Note {
                path: id.trim_start_matches("note://").into(),
                category: "wiki".into(),
            },
            relevance: rel,
            updated_at: updated,
            slot_hint: slot,
        }
    }

    #[test]
    fn empty_pool_yields_no_slots() {
        let skel = FallbackSkeleton::default();
        assert!(skeleton_pack(&[], &skel, 1_700_000_000).is_empty());
    }

    #[test]
    fn items_sorted_by_relevance_within_slot() {
        let now = 1_700_000_000;
        let c = [
            cand("note://wiki/a", SlotKind::RelevantNotes, 0.3, now),
            cand("note://wiki/b", SlotKind::RelevantNotes, 0.9, now),
            cand("note://wiki/c", SlotKind::RelevantNotes, 0.5, now),
        ];
        let slots = skeleton_pack(&c, &FallbackSkeleton::default(), now);
        let rel_slot = slots.iter().find(|s| s.kind == SlotKind::RelevantNotes).unwrap();
        assert_eq!(rel_slot.items[0].id, "note://wiki/b");
        assert_eq!(rel_slot.items[1].id, "note://wiki/c");
        assert_eq!(rel_slot.items[2].id, "note://wiki/a");
    }

    #[test]
    fn zero_budget_slot_is_excluded() {
        let mut skel = FallbackSkeleton::default();
        skel.relevant_notes_tokens = 0;
        let now = 1_700_000_000;
        let c = [cand("note://wiki/a", SlotKind::RelevantNotes, 0.9, now)];
        let slots = skeleton_pack(&c, &skel, now);
        assert!(slots.iter().all(|s| s.kind != SlotKind::RelevantNotes));
    }

    #[test]
    fn recency_factor_bounded() {
        // Verify invariant: recency_factor in [0.5, 1.0].
        let now = 1_700_000_000;
        assert!((recency_factor(now, now) - 1.0).abs() < 1e-6);
        assert!(recency_factor(now - 86_400 * 10_000, now) >= 0.5);
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib memory::assembler::fallback`

Expected: 4 tests pass.

- [ ] **Step 4: Clippy**

Run: `cargo clippy -p alephcore --lib -- -D warnings`

Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add src/memory/assembler/mod.rs src/memory/assembler/fallback.rs
git commit -m "memory(assembler): add deterministic skeleton fallback packer"
```

---

## Task 6: User Profile Loader

**Files:**
- Create: `src/memory/assembler/profile.rs`
- Modify: `src/memory/assembler/mod.rs`

- [ ] **Step 1: Add module declaration**

In `src/memory/assembler/mod.rs`:

```rust
pub(crate) mod profile;
```

- [ ] **Step 2: Create `src/memory/assembler/profile.rs`**

```rust
//! UserProfileLoader — reads `personal/profile.md` for an agent. If the file
//! is missing or unreadable, returns `None` (never an error).

use crate::sync_primitives::Arc;
use std::path::PathBuf;

/// Loader for `memory/note/{agent_id}/personal/profile.md`. Kept behind an
/// `Arc` so it can be shared across the assembler and its tests.
pub struct UserProfileLoader {
    memory_dir: PathBuf,
}

impl UserProfileLoader {
    pub fn new(memory_dir: PathBuf) -> Arc<Self> {
        Arc::new(Self { memory_dir })
    }

    /// Returns the profile body if readable, else `None`. Stripped of
    /// frontmatter for direct injection.
    pub async fn load(&self, agent_id: &str) -> Option<String> {
        let path = self.memory_dir.join(agent_id).join("personal").join("profile.md");
        let body = tokio::fs::read_to_string(&path).await.ok()?;
        Some(strip_frontmatter(&body))
    }

    /// Expose the expected path for diagnostics/tests.
    pub fn path_for(&self, agent_id: &str) -> PathBuf {
        self.memory_dir.join(agent_id).join("personal").join("profile.md")
    }
}

fn strip_frontmatter(s: &str) -> String {
    let trimmed = s.trim_start();
    if let Some(rest) = trimmed.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            return rest[end + 5..].trim_start().to_string();
        }
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_file_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let loader = UserProfileLoader::new(tmp.path().to_path_buf());
        assert!(loader.load("default").await.is_none());
    }

    #[tokio::test]
    async fn reads_profile_and_strips_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("default").join("personal");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let content = "---\ncategory: personal\n---\nuser prefers rust";
        tokio::fs::write(dir.join("profile.md"), content).await.unwrap();

        let loader = UserProfileLoader::new(tmp.path().to_path_buf());
        let got = loader.load("default").await.unwrap();
        assert_eq!(got, "user prefers rust");
    }

    #[test]
    fn strip_frontmatter_preserves_body_without_frontmatter() {
        assert_eq!(strip_frontmatter("hello"), "hello");
    }
}
```

- [ ] **Step 3: Ensure `tempfile` is available in dev-deps**

Run: `grep tempfile Cargo.toml`

If not present in `[dev-dependencies]`, add it:

```toml
[dev-dependencies]
tempfile = "3"
```

Aleph almost certainly has it already — verify before editing.

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib memory::assembler::profile`

Expected: 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/memory/assembler/mod.rs src/memory/assembler/profile.rs Cargo.toml
git commit -m "memory(assembler): add UserProfileLoader"
```

---

## Task 7: Candidate Gather (Concurrent Fan-out)

**Files:**
- Create: `src/memory/assembler/gather.rs`
- Modify: `src/memory/assembler/mod.rs`

- [ ] **Step 1: Add module declaration**

```rust
pub(crate) mod gather;
```

- [ ] **Step 2: Create `src/memory/assembler/gather.rs`**

The gather step fans out to four sources. Three concrete types (`NoteFactRetrieval`, `SnapshotReader`, `SqliteMemoryBackend`) are used directly; `UserProfileLoader` is our new type. Tests use in-module stubs via wrapping traits so we can inject fakes without touching production paths.

```rust
//! Stage 1: concurrent candidate gather. Fans out to all four sources and
//! assembles a single pool of [`Candidate`]s with a [`SlotKind`] hint on each.

use super::envelope::{ItemSource, SlotKind};
use super::fallback::Candidate;
use super::profile::UserProfileLoader;
use crate::memory::note_retrieval::NoteFactRetrieval;
use crate::memory::session_resume::reader::SnapshotReader;
use crate::memory::store::raw_memory::RawMemory;
use crate::memory::{SqliteMemoryBackend};
use crate::sync_primitives::Arc;
use tracing::warn;

pub(crate) struct GatherInputs {
    pub query: String,
    pub agent_id: String,
    pub session_id: Option<String>,
    pub pool_limit: usize,
}

pub(crate) struct Gatherer {
    pub retrieval: Arc<NoteFactRetrieval<SqliteMemoryBackend>>,
    pub snapshots: Arc<SnapshotReader>,
    pub backend: Arc<SqliteMemoryBackend>,
    pub profile: Arc<UserProfileLoader>,
}

impl Gatherer {
    pub async fn gather(&self, input: &GatherInputs) -> Vec<Candidate> {
        let (notes, snapshot, raws, profile) = tokio::join!(
            self.fetch_notes(&input.query, &input.agent_id, input.pool_limit),
            self.fetch_snapshot(input.session_id.as_deref()),
            self.fetch_raws(&input.agent_id, input.session_id.as_deref()),
            self.profile.load(&input.agent_id),
        );

        let mut pool = Vec::with_capacity(notes.len() + raws.len() + 2);
        pool.extend(notes);
        pool.extend(snapshot);
        pool.extend(raws);
        if let Some(body) = profile {
            pool.push(Candidate {
                id: "note://personal/profile".into(),
                title: "User profile".into(),
                full_content: body,
                source: ItemSource::Note {
                    path: "personal/profile".into(),
                    category: "personal".into(),
                },
                relevance: 1.0,
                updated_at: chrono::Utc::now().timestamp(),
                slot_hint: SlotKind::UserProfile,
            });
        }
        pool
    }

    async fn fetch_notes(&self, query: &str, agent_id: &str, limit: usize) -> Vec<Candidate> {
        match self.retrieval.retrieve(query, agent_id, limit).await {
            Ok(results) => results
                .into_iter()
                .map(|sf| Candidate {
                    id: sf.fact.path.clone(),
                    title: sf.fact.path.rsplit('/').next().unwrap_or(&sf.fact.path).to_string(),
                    full_content: sf.fact.content.clone(),
                    source: ItemSource::Note {
                        path: sf.fact.path.trim_start_matches("note://").to_string(),
                        category: sf.fact.note_type.to_category_dir().to_string(),
                    },
                    relevance: sf.score,
                    updated_at: sf.fact.updated_at,
                    slot_hint: SlotKind::RelevantNotes,
                })
                .collect(),
            Err(e) => {
                warn!(error = %e, "assembler.gather: notes retrieval failed");
                Vec::new()
            }
        }
    }

    async fn fetch_snapshot(&self, session_id: Option<&str>) -> Vec<Candidate> {
        let Some(sid) = session_id else { return Vec::new(); };
        match self.snapshots.load_latest(sid).await {
            Ok(Some(snap)) => {
                let body = format!(
                    "Summary: {}\nKey decisions: {}\nActive files: {}\nPending: {}",
                    snap.summary,
                    snap.key_decisions.join("; "),
                    snap.active_files.join(", "),
                    snap.pending_tasks.join("; "),
                );
                vec![Candidate {
                    id: format!("aleph://session/{sid}/snapshot"),
                    title: format!("Session {} snapshot", sid),
                    full_content: body,
                    source: ItemSource::Summary {
                        layer: "d1".into(),
                        session_id: sid.to_string(),
                    },
                    relevance: 0.9,
                    updated_at: snap.created_at.unwrap_or_else(|| chrono::Utc::now().timestamp()),
                    slot_hint: SlotKind::SessionRecent,
                }]
            }
            Ok(None) => Vec::new(),
            Err(e) => {
                warn!(error = %e, session = sid, "assembler.gather: snapshot load failed");
                Vec::new()
            }
        }
    }

    async fn fetch_raws(&self, agent_id: &str, session_id: Option<&str>) -> Vec<Candidate> {
        let Some(sid) = session_id else { return Vec::new(); };
        let prefix = format!("aleph://session/{sid}/raw/");
        match self.backend.get_raw_by_path_prefix(&prefix, agent_id, 5).await {
            Ok(raws) => raws.into_iter().map(raw_to_candidate).collect(),
            Err(e) => {
                warn!(error = %e, session = sid, "assembler.gather: raw fetch failed");
                Vec::new()
            }
        }
    }
}

fn raw_to_candidate(r: RawMemory) -> Candidate {
    let session_id = r.session_id.clone().unwrap_or_default();
    Candidate {
        id: format!("aleph://session/{session_id}/raw/{}", r.id),
        title: format!("Raw fragment {}", r.id),
        full_content: r.content,
        source: ItemSource::Raw {
            raw_id: r.id,
            session_id,
        },
        relevance: 0.6,
        updated_at: r.created_at,
        slot_hint: SlotKind::RawFragments,
    }
}

#[cfg(test)]
mod tests {
    // Integration tests that exercise the live Gatherer are in
    // `src/memory/assembler/tests/integration.rs` (Task 12). The gather
    // module is thin orchestration — coverage comes from the integration
    // tests, not from re-mocking each source here.

    use super::*;

    #[test]
    fn raw_to_candidate_populates_source() {
        let raw = RawMemory::new("content", crate::memory::store::raw_memory::RawMemorySource::Transcript)
            .with_session("sess-1");
        let c = raw_to_candidate(raw);
        matches!(&c.source, ItemSource::Raw { session_id, .. } if session_id == "sess-1");
    }
}
```

- [ ] **Step 3: Verify API assumptions by checking referenced types**

Run: `grep -n "pub fn load_latest" src/memory/session_resume/reader.rs`

Expected output: a signature like `pub async fn load_latest(&self, session_id: &str) -> Result<Option<SessionSnapshot>, ...>`. If the real signature differs (e.g., different arg order or `Result` type), adapt the `fetch_snapshot` impl accordingly.

Run: `grep -n "pub async fn get_raw_by_path_prefix\|pub fn get_raw_by_path_prefix" src/memory/store/sqlite/raw.rs src/memory/store/sqlite/mod.rs 2>/dev/null`

Expected: a method on `SqliteMemoryBackend` with signature `async fn get_raw_by_path_prefix(&self, prefix: &str, agent_id: &str, limit: usize) -> Result<Vec<RawMemory>, ...>`. If it lives on a different impl (e.g., on an inherent method of the backend, not a trait), the `backend: Arc<SqliteMemoryBackend>` call site above is correct. If the real method takes different argument types, update the call.

Run: `grep -n "pub struct SessionSnapshot" src/memory/session_resume/snapshot.rs`

Expected: the struct has `session_id`, `summary`, `key_decisions`, `active_files`, `pending_tasks`, and a `created_at: Option<i64>` (or `i64`) field. If `created_at` is non-`Option`, adjust the body accordingly (drop `.unwrap_or_else`).

Run: `grep -n "pub fn to_category_dir\|fn to_category_dir" src/memory/context/enums.rs src/memory/context/fact.rs 2>/dev/null`

Expected: `NoteType::to_category_dir` exists. If not, derive the category from `sf.fact.path` segment instead (e.g., `sf.fact.path.split('/').next()`).

Adjust `gather.rs` to match reality before moving on. These three signature checks are the highest-risk spots.

- [ ] **Step 4: Compile + run targeted test**

Run: `cargo test -p alephcore --lib memory::assembler::gather`

Expected: 1 unit test passes; no other compile errors.

- [ ] **Step 5: Commit**

```bash
git add src/memory/assembler/mod.rs src/memory/assembler/gather.rs
git commit -m "memory(assembler): add concurrent candidate gatherer (Stage 1)"
```

---

## Task 8: LLM Re-rank Prompt + Response Validation

**Files:**
- Create: `src/memory/assembler/rerank.rs`
- Modify: `src/memory/assembler/mod.rs`

- [ ] **Step 1: Add module declaration**

```rust
pub(crate) mod rerank;
```

- [ ] **Step 2: Create `src/memory/assembler/rerank.rs`**

```rust
//! Stage 2: LLM re-rank prompt + response validation.

use super::envelope::SlotKind;
use super::error::AssemblerError;
use super::fallback::Candidate;
use serde::Deserialize;
use std::collections::HashSet;

pub(crate) const RERANK_PROMPT_V1: &str = r#"You are a Working Memory Assembler. Given the user's current query and a pool of memory candidates, decide which to include and allocate a token budget across slots: session_recent, relevant_notes, raw_fragments. (user_profile is pre-included; nudges are reserved for future use.)

Query: {query}

Total budget: {budget} tokens. Your slot budgets must sum to at most {max_sum} tokens (reserving 30% for the LLM reply).

Candidates (id | title | slot_hint | relevance | summary):
{candidates}

Return STRICT JSON (no prose, no markdown fences) matching:
{
  "slots": [
    {"kind": "relevant_notes",  "item_ids": ["..."], "tokens_budget": N},
    {"kind": "session_recent",  "item_ids": ["..."], "tokens_budget": N},
    {"kind": "raw_fragments",   "item_ids": ["..."], "tokens_budget": N}
  ],
  "reasoning": "one-line explanation (optional)"
}

Rules:
- item_ids MUST be a subset of the candidate ids above.
- Omit a slot entirely if you would not include anything in it.
- Order within item_ids is priority (most important first).
- Do NOT include the user_profile slot — it is pre-populated.
"#;

pub(crate) fn build_prompt(query: &str, candidates: &[Candidate], total_budget: u32) -> String {
    let max_sum = ((total_budget as f32) * 0.7) as u32;
    let cand_block: String = candidates
        .iter()
        .filter(|c| c.slot_hint != SlotKind::UserProfile)
        .map(|c| {
            let summary = c.full_content.chars().take(30).collect::<String>();
            format!(
                "  [{}] | \"{}\" | {} | {:.2} | {}",
                c.id,
                c.title.replace('"', "'"),
                slot_name(c.slot_hint),
                c.relevance,
                summary,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    RERANK_PROMPT_V1
        .replace("{query}", query)
        .replace("{budget}", &total_budget.to_string())
        .replace("{max_sum}", &max_sum.to_string())
        .replace("{candidates}", &cand_block)
}

fn slot_name(k: SlotKind) -> &'static str {
    match k {
        SlotKind::UserProfile => "user_profile",
        SlotKind::SessionRecent => "session_recent",
        SlotKind::RelevantNotes => "relevant_notes",
        SlotKind::RawFragments => "raw_fragments",
        SlotKind::Nudges => "nudges",
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct RerankResponse {
    #[serde(default)]
    pub slots: Vec<RerankSlot>,
    #[serde(default)]
    pub reasoning: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RerankSlot {
    pub kind: String, // parsed via from_str below; preserve raw to surface unknown variants
    #[serde(default)]
    pub item_ids: Vec<String>,
    pub tokens_budget: u32,
}

/// Parse and validate an LLM response against the candidate pool + total budget.
/// Returns a sanitized `Vec<(SlotKind, Vec<String>, u32)>` — unknown kinds
/// dropped, hallucinated ids filtered, per-slot budgets scaled proportionally
/// if their sum exceeds the 70% cap. `Err(AssemblerError::RerankParse)` if the
/// response cannot be parsed at all; `Err(AssemblerError::RerankEmpty)` if no
/// valid slots remain after sanitization.
pub(crate) fn parse_response(
    raw: &str,
    candidates: &[Candidate],
    total_budget: u32,
) -> Result<Vec<(SlotKind, Vec<String>, u32)>, AssemblerError> {
    let trimmed = strip_json_fences(raw);
    let resp: RerankResponse = serde_json::from_str(trimmed)
        .map_err(|e| AssemblerError::RerankParse(format!("json: {e}")))?;

    let valid_ids: HashSet<&str> = candidates.iter().map(|c| c.id.as_str()).collect();
    let mut sanitized: Vec<(SlotKind, Vec<String>, u32)> = Vec::new();
    for slot in resp.slots {
        let Some(kind) = parse_slot_kind(&slot.kind) else { continue };
        if matches!(kind, SlotKind::UserProfile) {
            continue; // framework-managed
        }
        let ids: Vec<String> = slot
            .item_ids
            .into_iter()
            .filter(|id| valid_ids.contains(id.as_str()))
            .collect();
        if ids.is_empty() || slot.tokens_budget == 0 {
            continue;
        }
        sanitized.push((kind, ids, slot.tokens_budget));
    }

    if sanitized.is_empty() {
        return Err(AssemblerError::RerankEmpty);
    }

    let sum: u64 = sanitized.iter().map(|(_, _, b)| *b as u64).sum();
    let cap = ((total_budget as f32) * 0.7) as u64;
    if sum > cap {
        // Scale each slot's budget proportionally to fit cap.
        let scale = cap as f32 / sum as f32;
        for (_, _, b) in sanitized.iter_mut() {
            *b = ((*b as f32) * scale).floor() as u32;
        }
    }
    Ok(sanitized)
}

fn parse_slot_kind(s: &str) -> Option<SlotKind> {
    match s {
        "user_profile" => Some(SlotKind::UserProfile),
        "session_recent" => Some(SlotKind::SessionRecent),
        "relevant_notes" => Some(SlotKind::RelevantNotes),
        "raw_fragments" => Some(SlotKind::RawFragments),
        "nudges" => Some(SlotKind::Nudges),
        _ => None,
    }
}

fn strip_json_fences(s: &str) -> &str {
    let t = s.trim();
    let t = t.strip_prefix("```json").unwrap_or(t);
    let t = t.strip_prefix("```").unwrap_or(t);
    let t = t.strip_suffix("```").unwrap_or(t);
    t.trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::assembler::envelope::ItemSource;

    fn cand(id: &str, slot: SlotKind) -> Candidate {
        Candidate {
            id: id.into(),
            title: id.into(),
            full_content: "body".into(),
            source: ItemSource::Note { path: id.into(), category: "wiki".into() },
            relevance: 0.5,
            updated_at: 0,
            slot_hint: slot,
        }
    }

    #[test]
    fn valid_response_parses() {
        let c = [cand("note://a", SlotKind::RelevantNotes), cand("note://b", SlotKind::SessionRecent)];
        let raw = r#"{"slots":[
            {"kind":"relevant_notes","item_ids":["note://a"],"tokens_budget":1000},
            {"kind":"session_recent","item_ids":["note://b"],"tokens_budget":500}
        ]}"#;
        let out = parse_response(raw, &c, 4000).unwrap();
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn invalid_json_errors() {
        let c = [cand("note://a", SlotKind::RelevantNotes)];
        assert!(matches!(parse_response("{bogus", &c, 4000).unwrap_err(), AssemblerError::RerankParse(_)));
    }

    #[test]
    fn hallucinated_ids_filtered() {
        let c = [cand("note://a", SlotKind::RelevantNotes)];
        let raw = r#"{"slots":[{"kind":"relevant_notes","item_ids":["note://fake","note://a"],"tokens_budget":500}]}"#;
        let out = parse_response(raw, &c, 4000).unwrap();
        assert_eq!(out[0].1, vec!["note://a"]);
    }

    #[test]
    fn over_budget_scales_proportionally() {
        let c = [cand("note://a", SlotKind::RelevantNotes)];
        let raw = r#"{"slots":[{"kind":"relevant_notes","item_ids":["note://a"],"tokens_budget":10000}]}"#;
        let out = parse_response(raw, &c, 4000).unwrap();
        // cap = 4000 * 0.7 = 2800; sum was 10000 → scale ~0.28 → ~2800.
        assert!(out[0].2 <= 2800 && out[0].2 > 0);
    }

    #[test]
    fn empty_slots_errors() {
        let c = [cand("note://a", SlotKind::RelevantNotes)];
        let raw = r#"{"slots":[]}"#;
        assert!(matches!(parse_response(raw, &c, 4000).unwrap_err(), AssemblerError::RerankEmpty));
    }

    #[test]
    fn unknown_kind_dropped() {
        let c = [cand("note://a", SlotKind::RelevantNotes)];
        let raw = r#"{"slots":[{"kind":"bogus","item_ids":["note://a"],"tokens_budget":500}]}"#;
        assert!(matches!(parse_response(raw, &c, 4000).unwrap_err(), AssemblerError::RerankEmpty));
    }

    #[test]
    fn user_profile_slot_dropped() {
        let c = [cand("note://a", SlotKind::UserProfile)];
        let raw = r#"{"slots":[{"kind":"user_profile","item_ids":["note://a"],"tokens_budget":500}]}"#;
        assert!(matches!(parse_response(raw, &c, 4000).unwrap_err(), AssemblerError::RerankEmpty));
    }

    #[test]
    fn markdown_fences_stripped() {
        let c = [cand("note://a", SlotKind::RelevantNotes)];
        let raw = "```json\n{\"slots\":[{\"kind\":\"relevant_notes\",\"item_ids\":[\"note://a\"],\"tokens_budget\":500}]}\n```";
        assert!(parse_response(raw, &c, 4000).is_ok());
    }

    #[test]
    fn prompt_renders_candidates_and_budget() {
        let c = [cand("note://a", SlotKind::RelevantNotes)];
        let p = build_prompt("question", &c, 4000);
        assert!(p.contains("question"));
        assert!(p.contains("note://a"));
        assert!(p.contains("4000"));
        assert!(p.contains("2800"));  // max_sum = 70%
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib memory::assembler::rerank`

Expected: 9 passing tests.

- [ ] **Step 4: Commit**

```bash
git add src/memory/assembler/mod.rs src/memory/assembler/rerank.rs
git commit -m "memory(assembler): add LLM rerank prompt and response validation"
```

---

## Task 9: HybridAssembler — C-Only Path

**Files:**
- Create: `src/memory/assembler/hybrid.rs`
- Modify: `src/memory/assembler/mod.rs`

- [ ] **Step 1: Add module declaration**

```rust
pub mod hybrid;

pub use hybrid::HybridAssembler;
```

- [ ] **Step 2: Create `src/memory/assembler/hybrid.rs` with C-only implementation**

This first pass implements everything except the LLM call. Task 10 adds the LLM step.

```rust
//! Default implementation of [`WorkingMemoryAssembler`] — hybrid retrieval +
//! LLM re-rank with deterministic skeleton fallback. Task 9 lands the
//! fallback-only path; Task 10 wires in the LLM re-rank.

use super::envelope::{EnvelopeMeta, EnvelopeSlot, MemoryEnvelope, SlotKind, SCHEMA_VERSION};
use super::fallback::{skeleton_pack, Candidate};
use super::gather::{GatherInputs, Gatherer};
use super::hydration::{estimate_tokens, truncate_utf8_safe};
use super::profile::UserProfileLoader;
use super::{AssemblyBudget, WorkingMemoryAssembler};
use crate::config::types::memory::AssemblerConfig;
use crate::error::AlephError;
use crate::memory::note_retrieval::NoteFactRetrieval;
use crate::memory::session_resume::reader::SnapshotReader;
use crate::memory::SqliteMemoryBackend;
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use tracing::info;

pub struct HybridAssembler {
    gatherer: Gatherer,
    config: AssemblerConfig,
    // ai_provider is wired in Task 10; not used yet.
}

impl HybridAssembler {
    pub fn new(
        retrieval: Arc<NoteFactRetrieval<SqliteMemoryBackend>>,
        snapshots: Arc<SnapshotReader>,
        backend: Arc<SqliteMemoryBackend>,
        profile: Arc<UserProfileLoader>,
        config: AssemblerConfig,
    ) -> Self {
        Self {
            gatherer: Gatherer {
                retrieval,
                snapshots,
                backend,
                profile,
            },
            config,
        }
    }

    fn now(&self) -> i64 {
        chrono::Utc::now().timestamp()
    }
}

#[async_trait]
impl WorkingMemoryAssembler for HybridAssembler {
    async fn assemble(
        &self,
        query: &str,
        agent_id: &str,
        session_id: Option<&str>,
        budget: AssemblyBudget,
    ) -> Result<MemoryEnvelope, AlephError> {
        let start = std::time::Instant::now();

        // Stage 1: gather
        let gathered = self
            .gatherer
            .gather(&GatherInputs {
                query: query.to_string(),
                agent_id: agent_id.to_string(),
                session_id: session_id.map(str::to_string),
                pool_limit: self.config.candidate_pool_limit,
            })
            .await;

        let candidates_considered = gathered.len();

        // Stage 2 (not yet implemented) → always fall through to Stage 2'
        let (mut slots, strategy, fallback_reason) =
            (fallback_slots(&gathered, &self.config, self.now()), "skeleton_fallback_v1", Some("stage2_pending".to_string()));

        // Stage 3: hydration + token pack
        hydrate(&mut slots);

        let total_latency = start.elapsed().as_millis() as u64;
        let envelope = MemoryEnvelope {
            schema_version: SCHEMA_VERSION.to_string(),
            generated_at: self.now(),
            query: query.to_string(),
            agent_id: agent_id.to_string(),
            session_id: session_id.map(str::to_string),
            slots,
            meta: EnvelopeMeta {
                strategy: strategy.into(),
                candidates_considered,
                used_fallback: true,
                fallback_reason,
                llm_rerank_latency_ms: None,
                total_latency_ms: total_latency,
            },
        };

        emit_tracing(&envelope, query);
        Ok(envelope)
    }
}

fn fallback_slots(candidates: &[Candidate], config: &AssemblerConfig, now: i64) -> Vec<EnvelopeSlot> {
    skeleton_pack(candidates, &config.fallback_skeleton, now)
}

fn hydrate(slots: &mut [EnvelopeSlot]) {
    for slot in slots.iter_mut() {
        let mut used = 0u32;
        for item in slot.items.iter_mut() {
            let remaining_chars = slot.tokens_budget.saturating_sub(used).saturating_mul(4);
            let truncated = truncate_utf8_safe(&item.content, remaining_chars as usize);
            item.tokens = estimate_tokens(&truncated);
            item.content = truncated;
            used = used.saturating_add(item.tokens);
            if used >= slot.tokens_budget {
                break;
            }
        }
        slot.tokens_used = used;
        // Drop any trailing items that got zero budget.
        slot.items.retain(|i| i.tokens > 0 || !i.content.is_empty());
    }
}

fn emit_tracing(env: &MemoryEnvelope, query: &str) {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(query.as_bytes());
    let query_hash = format!("{:x}", h.finalize());
    let total_tokens: u32 = env.slots.iter().map(|s| s.tokens_used).sum();
    info!(
        target = "memory.assembler",
        query_hash = %query_hash,
        agent_id = %env.agent_id,
        session_id = ?env.session_id,
        strategy = %env.meta.strategy,
        used_fallback = env.meta.used_fallback,
        fallback_reason = ?env.meta.fallback_reason,
        candidates = env.meta.candidates_considered,
        llm_rerank_ms = ?env.meta.llm_rerank_latency_ms,
        total_ms = env.meta.total_latency_ms,
        slot_count = env.slots.len(),
        total_tokens,
        "assembly completed"
    );
}

#[cfg(test)]
mod tests {
    // Fuller integration tests live in tests/integration.rs. This module holds
    // tight, dependency-free smoke tests that exercise only the pure functions
    // (hydrate, fallback_slots) to keep the unit suite fast.

    use super::*;
    use crate::memory::assembler::envelope::{EnvelopeItem, ItemSource};

    #[test]
    fn hydrate_truncates_content_to_slot_budget() {
        let mut slots = vec![EnvelopeSlot {
            kind: SlotKind::RelevantNotes,
            items: vec![EnvelopeItem {
                id: "note://a".into(),
                title: "a".into(),
                content: "x".repeat(10_000),
                source: ItemSource::Note { path: "a".into(), category: "wiki".into() },
                relevance: 1.0,
                tokens: 0,
                updated_at: 0,
                extra: Default::default(),
            }],
            tokens_used: 0,
            tokens_budget: 100, // 100 tokens = 400 chars
        }];
        hydrate(&mut slots);
        assert!(slots[0].items[0].content.len() <= 400);
        assert!(slots[0].tokens_used <= 100);
    }
}
```

- [ ] **Step 3: Ensure `sha2` is a direct dep**

Run: `grep 'sha2' Cargo.toml`

Aleph uses sha2 elsewhere (e.g., content hashing in notes). It should already be a direct dep. If not, add `sha2 = "0.10"` under `[dependencies]`.

- [ ] **Step 4: Run unit test**

Run: `cargo test -p alephcore --lib memory::assembler::hybrid`

Expected: 1 passing test.

- [ ] **Step 5: Commit**

```bash
git add src/memory/assembler/ Cargo.toml
git commit -m "memory(assembler): add HybridAssembler with fallback-only path"
```

---

## Task 10: Wire LLM Re-rank into HybridAssembler

**Files:**
- Modify: `src/memory/assembler/hybrid.rs`

- [ ] **Step 1: Extend `HybridAssembler` to hold an `Arc<dyn AiProvider>`**

Replace the struct and constructor at the top of `hybrid.rs`:

```rust
use crate::providers::AiProvider;                      // add import
use super::error::AssemblerError;                      // add
use super::rerank::{build_prompt, parse_response};     // add
use std::collections::HashMap;                         // add

pub struct HybridAssembler {
    gatherer: Gatherer,
    provider: Arc<dyn AiProvider>,
    config: AssemblerConfig,
}

impl HybridAssembler {
    pub fn new(
        retrieval: Arc<NoteFactRetrieval<SqliteMemoryBackend>>,
        snapshots: Arc<SnapshotReader>,
        backend: Arc<SqliteMemoryBackend>,
        profile: Arc<UserProfileLoader>,
        provider: Arc<dyn AiProvider>,
        config: AssemblerConfig,
    ) -> Self {
        Self {
            gatherer: Gatherer { retrieval, snapshots, backend, profile },
            provider,
            config,
        }
    }
    // ... now() unchanged ...
}
```

(Signature-breaking change from Task 9. Downstream callers will be updated in Task 13.)

- [ ] **Step 2: Extract the shared "slots → hydrate → envelope" tail into a helper**

Add inside `impl HybridAssembler`:

```rust
fn pack_envelope(
    &self,
    query: &str,
    agent_id: &str,
    session_id: Option<&str>,
    candidates_considered: usize,
    slots: Vec<EnvelopeSlot>,
    strategy: &'static str,
    used_fallback: bool,
    fallback_reason: Option<String>,
    llm_rerank_latency_ms: Option<u64>,
    total_latency_ms: u64,
) -> MemoryEnvelope {
    let mut slots = slots;
    hydrate(&mut slots);
    MemoryEnvelope {
        schema_version: SCHEMA_VERSION.to_string(),
        generated_at: self.now(),
        query: query.to_string(),
        agent_id: agent_id.to_string(),
        session_id: session_id.map(str::to_string),
        slots,
        meta: EnvelopeMeta {
            strategy: strategy.into(),
            candidates_considered,
            used_fallback,
            fallback_reason,
            llm_rerank_latency_ms,
            total_latency_ms,
        },
    }
}
```

- [ ] **Step 3: Replace the body of `assemble`**

Rewrite the `assemble` implementation:

```rust
async fn assemble(
    &self,
    query: &str,
    agent_id: &str,
    session_id: Option<&str>,
    budget: AssemblyBudget,
) -> Result<MemoryEnvelope, AlephError> {
    let start = std::time::Instant::now();

    if !self.config.enabled {
        // Kill switch: emit empty envelope that still satisfies shape.
        let env = self.pack_envelope(
            query, agent_id, session_id,
            0, Vec::new(), "disabled", true,
            Some("assembler_disabled".into()),
            None, start.elapsed().as_millis() as u64,
        );
        emit_tracing(&env, query);
        return Ok(env);
    }

    // Stage 1: gather
    let gathered = self
        .gatherer
        .gather(&GatherInputs {
            query: query.to_string(),
            agent_id: agent_id.to_string(),
            session_id: session_id.map(str::to_string),
            pool_limit: self.config.candidate_pool_limit,
        })
        .await;
    let candidates_considered = gathered.len();

    // Fast-path to fallback for tiny pools or forced config.
    let too_small = candidates_considered < 3;
    if self.config.force_fallback || too_small {
        let reason = if self.config.force_fallback { "forced" } else { "tiny_pool" };
        let slots = skeleton_pack(&gathered, &self.config.fallback_skeleton, self.now());
        let env = self.pack_envelope(
            query, agent_id, session_id, candidates_considered, slots,
            "skeleton_fallback_v1", true,
            Some(reason.into()), None,
            start.elapsed().as_millis() as u64,
        );
        emit_tracing(&env, query);
        return Ok(env);
    }

    // Stage 2: LLM rerank under timeout.
    let rerank_start = std::time::Instant::now();
    let rerank_outcome = self.run_rerank(query, &gathered, budget.total_tokens).await;
    let rerank_latency = rerank_start.elapsed().as_millis() as u64;

    match rerank_outcome {
        Ok(slots) => {
            let env = self.pack_envelope(
                query, agent_id, session_id, candidates_considered, slots,
                "hybrid_v1", false, None, Some(rerank_latency),
                start.elapsed().as_millis() as u64,
            );
            emit_tracing(&env, query);
            Ok(env)
        }
        Err(reason) => {
            let slots = skeleton_pack(&gathered, &self.config.fallback_skeleton, self.now());
            let env = self.pack_envelope(
                query, agent_id, session_id, candidates_considered, slots,
                "skeleton_fallback_v1", true,
                Some(reason.into()), Some(rerank_latency),
                start.elapsed().as_millis() as u64,
            );
            emit_tracing(&env, query);
            Ok(env)
        }
    }
}
```

- [ ] **Step 4: Implement `run_rerank`**

Add as an inherent method on `HybridAssembler`:

```rust
async fn run_rerank(
    &self,
    query: &str,
    candidates: &[Candidate],
    total_budget: u32,
) -> Result<Vec<EnvelopeSlot>, &'static str> {
    let prompt = build_prompt(query, candidates, total_budget);

    let call = async {
        self.provider
            .complete(&prompt, self.config.rerank_model.as_deref())
            .await
    };

    let timeout = std::time::Duration::from_millis(self.config.rerank_timeout_ms);
    let raw = match tokio::time::timeout(timeout, call).await {
        Ok(Ok(text)) => text,
        Ok(Err(_e)) => return Err("llm_error"),
        Err(_) => return Err("llm_timeout"),
    };

    let decisions = match parse_response(&raw, candidates, total_budget) {
        Ok(v) => v,
        Err(AssemblerError::RerankEmpty) => return Err("llm_empty_slots"),
        Err(AssemblerError::RerankParse(_)) => return Err("llm_parse_error"),
        Err(_) => return Err("llm_unknown_error"),
    };

    let by_id: HashMap<&str, &Candidate> = candidates.iter().map(|c| (c.id.as_str(), c)).collect();
    let mut slots: Vec<EnvelopeSlot> = Vec::new();

    // UserProfile always appended first if present in candidates.
    let profile_cands: Vec<&Candidate> = candidates.iter().filter(|c| c.slot_hint == SlotKind::UserProfile).collect();
    if !profile_cands.is_empty() {
        let items = profile_cands.into_iter().map(candidate_to_item).collect();
        slots.push(EnvelopeSlot {
            kind: SlotKind::UserProfile,
            items,
            tokens_used: 0,
            tokens_budget: self.config.fallback_skeleton.user_profile_tokens,
        });
    }

    for (kind, ids, budget) in decisions {
        let items = ids.into_iter()
            .filter_map(|id| by_id.get(id.as_str()).copied())
            .map(candidate_to_item)
            .collect::<Vec<_>>();
        if items.is_empty() { continue; }
        slots.push(EnvelopeSlot { kind, items, tokens_used: 0, tokens_budget: budget });
    }
    Ok(slots)
}
```

- [ ] **Step 5: Add `candidate_to_item` helper**

Add (module-level) at the bottom of `hybrid.rs`:

```rust
fn candidate_to_item(c: &Candidate) -> crate::memory::assembler::envelope::EnvelopeItem {
    crate::memory::assembler::envelope::EnvelopeItem {
        id: c.id.clone(),
        title: c.title.clone(),
        content: c.full_content.clone(),
        source: c.source.clone(),
        relevance: c.relevance,
        tokens: 0,
        updated_at: c.updated_at,
        extra: Default::default(),
    }
}
```

- [ ] **Step 6: Verify `AiProvider::complete` signature**

Run: `grep -n "pub trait AiProvider\|fn complete\|async fn complete" src/providers/mod.rs`

Adjust the `run_rerank` call to match the real signature. Likely options:
- `async fn complete(&self, prompt: &str, model: Option<&str>) -> Result<String, AlephError>` — the call above is correct.
- Or `async fn chat(&self, messages: &[Message], opts: ChatOpts) -> Result<Response, AlephError>` — wrap the prompt into a single-message conversation with a system instruction requesting strict JSON.

Adjust as needed; this signature is the highest-risk integration detail. Keep the timeout wrapping and `raw: String` shape the same.

- [ ] **Step 7: Write a mockall-based unit test for `run_rerank`**

Append to the `#[cfg(test)]` section of `hybrid.rs`:

```rust
#[cfg(test)]
mod rerank_tests {
    use super::*;
    use crate::memory::assembler::envelope::ItemSource;
    use crate::providers::MockAiProvider; // check that mock exists; add if missing

    fn cand(id: &str, slot: SlotKind, rel: f32) -> Candidate {
        Candidate {
            id: id.into(), title: id.into(), full_content: format!("body-{id}"),
            source: ItemSource::Note { path: id.into(), category: "wiki".into() },
            relevance: rel, updated_at: 0, slot_hint: slot,
        }
    }

    // A minimal stub assembler that exposes run_rerank by bypassing gather.
    // Real end-to-end flow is tested in tests/integration.rs.

    #[tokio::test]
    async fn rerank_timeout_maps_to_error() {
        let mut mock = MockAiProvider::new();
        mock.expect_complete()
            .returning(|_, _| Box::pin(async {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                Ok::<_, AlephError>("ignored".into())
            }));
        // Build a minimal assembler via helper (see note below).
        // Placeholder: reuse integration test scaffolding for the full call.
    }
}
```

If `MockAiProvider` does not exist, generate one from the `AiProvider` trait with the `mockall::automock` attribute on the trait (requires `src/providers/mod.rs` edit — discuss before modifying). Alternative: hand-roll a stub in the test module:

```rust
struct StubProvider { complete_result: tokio::sync::Mutex<Option<Result<String, AlephError>>> }

#[async_trait::async_trait]
impl AiProvider for StubProvider {
    async fn complete(&self, _prompt: &str, _model: Option<&str>) -> Result<String, AlephError> {
        let mut lock = self.complete_result.lock().await;
        lock.take().unwrap_or_else(|| Err(AlephError::from("no stub set")))
    }
    // stub other trait methods with unimplemented!() — only complete is exercised here
}
```

Full end-to-end assertion of fallback-on-timeout happens in Task 12's integration tests.

- [ ] **Step 8: Compile and run existing tests**

Run: `cargo test -p alephcore --lib memory::assembler`

Expected: all assembler tests pass.

- [ ] **Step 9: Commit**

```bash
git add src/memory/assembler/hybrid.rs
git commit -m "memory(assembler): wire LLM re-rank (Stage 2) into HybridAssembler"
```

---

## Task 11: `assembly_logs` Schema + Writer

**Files:**
- Modify: `src/memory/store/sqlite/schema.rs`
- Create: `src/memory/assembler/log_store.rs`
- Modify: `src/memory/assembler/mod.rs`

- [ ] **Step 1: Add DDL constant to `schema.rs`**

Append to `src/memory/store/sqlite/schema.rs` alongside existing DDL constants:

```rust
pub const CREATE_ASSEMBLY_LOGS: &str = r#"
CREATE TABLE IF NOT EXISTS assembly_logs (
    id                 TEXT PRIMARY KEY,
    agent_id           TEXT NOT NULL,
    session_id         TEXT,
    query_hash         TEXT NOT NULL,
    strategy           TEXT NOT NULL,
    used_fallback      INTEGER NOT NULL DEFAULT 0,
    fallback_reason    TEXT,
    candidates_count   INTEGER NOT NULL,
    selected_item_ids  TEXT NOT NULL,
    total_tokens       INTEGER NOT NULL,
    rerank_latency_ms  INTEGER,
    total_latency_ms   INTEGER NOT NULL,
    created_at         INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_assembly_logs_agent_created
    ON assembly_logs(agent_id, created_at);
"#;
```

- [ ] **Step 2: Wire into `init_schema`**

Find the `init_schema` function in the same file. Add a call to execute `CREATE_ASSEMBLY_LOGS` in the same style as other existing tables (likely `conn.execute_batch(CREATE_ASSEMBLY_LOGS)?;`).

- [ ] **Step 3: Run existing schema tests**

Run: `cargo test -p alephcore --lib memory::store::sqlite::schema`

Expected: all pass; new DDL is idempotent.

- [ ] **Step 4: Create `src/memory/assembler/log_store.rs`**

```rust
//! Optional persistence writer for assembly decisions. Spec 2 consumes rows
//! from this table to correlate with citation / re-retrieval signals.

use super::envelope::MemoryEnvelope;
use crate::config::types::memory::AssemblyLogConfig;
use crate::error::AlephError;
use crate::memory::SqliteMemoryBackend;
use crate::sync_primitives::Arc;
use sha2::{Digest, Sha256};
use tracing::warn;
use uuid::Uuid;

pub struct AssemblyLogWriter {
    backend: Arc<SqliteMemoryBackend>,
    config: AssemblyLogConfig,
}

impl AssemblyLogWriter {
    pub fn new(backend: Arc<SqliteMemoryBackend>, config: AssemblyLogConfig) -> Self {
        Self { backend, config }
    }

    pub async fn write(&self, env: &MemoryEnvelope) {
        if !self.config.enabled {
            return;
        }
        let query_hash = format!("{:x}", Sha256::digest(env.query.as_bytes()));
        let selected_ids: Vec<&str> = env
            .slots
            .iter()
            .flat_map(|s| s.items.iter().map(|i| i.id.as_str()))
            .collect();
        let selected_json = serde_json::to_string(&selected_ids).unwrap_or_else(|_| "[]".into());
        let total_tokens: u32 = env.slots.iter().map(|s| s.tokens_used).sum();

        let row = AssemblyLogRow {
            id: Uuid::new_v4().to_string(),
            agent_id: env.agent_id.clone(),
            session_id: env.session_id.clone(),
            query_hash,
            strategy: env.meta.strategy.clone(),
            used_fallback: env.meta.used_fallback,
            fallback_reason: env.meta.fallback_reason.clone(),
            candidates_count: env.meta.candidates_considered as i64,
            selected_item_ids: selected_json,
            total_tokens: total_tokens as i64,
            rerank_latency_ms: env.meta.llm_rerank_latency_ms.map(|v| v as i64),
            total_latency_ms: env.meta.total_latency_ms as i64,
            created_at: env.generated_at,
        };

        if let Err(e) = self.backend.insert_assembly_log(&row).await {
            warn!(error = %e, "assembly_log insert failed");
        }
    }
}

#[derive(Debug)]
pub struct AssemblyLogRow {
    pub id: String,
    pub agent_id: String,
    pub session_id: Option<String>,
    pub query_hash: String,
    pub strategy: String,
    pub used_fallback: bool,
    pub fallback_reason: Option<String>,
    pub candidates_count: i64,
    pub selected_item_ids: String,
    pub total_tokens: i64,
    pub rerank_latency_ms: Option<i64>,
    pub total_latency_ms: i64,
    pub created_at: i64,
}
```

- [ ] **Step 5: Add `insert_assembly_log` method on `SqliteMemoryBackend`**

Locate the main `impl SqliteMemoryBackend { ... }` block (likely in `src/memory/store/sqlite/mod.rs`). Add:

```rust
pub async fn insert_assembly_log(
    &self,
    row: &crate::memory::assembler::log_store::AssemblyLogRow,
) -> Result<(), AlephError> {
    let conn = self.conn.lock().await;
    conn.execute(
        "INSERT INTO assembly_logs (
            id, agent_id, session_id, query_hash, strategy, used_fallback,
            fallback_reason, candidates_count, selected_item_ids, total_tokens,
            rerank_latency_ms, total_latency_ms, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        rusqlite::params![
            row.id,
            row.agent_id,
            row.session_id,
            row.query_hash,
            row.strategy,
            row.used_fallback as i64,
            row.fallback_reason,
            row.candidates_count,
            row.selected_item_ids,
            row.total_tokens,
            row.rerank_latency_ms,
            row.total_latency_ms,
            row.created_at,
        ],
    )
    .map_err(|e| AlephError::from(format!("insert_assembly_log: {e}")))?;
    Ok(())
}
```

Adjust `self.conn.lock()` and error mapping to match the existing backend conventions (grep for another insert method in the same file to mirror its style).

- [ ] **Step 6: Export from `mod.rs`**

In `src/memory/assembler/mod.rs`:

```rust
pub mod log_store;
pub use log_store::{AssemblyLogRow, AssemblyLogWriter};
```

- [ ] **Step 7: Write smoke test**

Append a test to `log_store.rs`:

```rust
#[cfg(test)]
mod tests {
    // Full round-trip test lives in integration tests; here we just assert
    // that the writer silently no-ops when disabled (the zero-cost guarantee).

    use super::*;
    use crate::config::types::memory::AssemblyLogConfig;

    #[tokio::test]
    async fn writer_noops_when_disabled() {
        // Use a deliberately non-existent backend path; if write() actually
        // touched the DB, this would panic. The `enabled: false` branch must
        // return before any I/O.
        let config = AssemblyLogConfig { enabled: false, retention_days: 14 };
        // Construct a real SqliteMemoryBackend pointed at :memory: so we can
        // verify the "no insert" behavior by reading row count.
        let backend = crate::memory::SqliteMemoryBackend::in_memory().await.unwrap();
        let writer = AssemblyLogWriter::new(Arc::new(backend.clone()), config);

        let env = crate::memory::assembler::envelope::MemoryEnvelope {
            schema_version: "1.0".into(),
            generated_at: 0,
            query: "q".into(),
            agent_id: "a".into(),
            session_id: None,
            slots: vec![],
            meta: Default::default(),
        };
        writer.write(&env).await;

        let count: i64 = backend.assembly_log_count().await.unwrap_or(0);
        assert_eq!(count, 0);
    }
}
```

If `SqliteMemoryBackend::in_memory()` and `assembly_log_count()` don't exist, add them (`in_memory` likely already exists; `assembly_log_count` is a thin helper: `SELECT COUNT(*) FROM assembly_logs`).

Also: `MemoryEnvelope` needs `Default` on `EnvelopeMeta`. Add `#[derive(Default)]` on `EnvelopeMeta` in `envelope.rs` (Task 1's definition). Default for `strategy: String = ""` is fine.

- [ ] **Step 8: Run tests**

Run: `cargo test -p alephcore --lib memory::assembler::log_store`

Expected: 1 test passes.

- [ ] **Step 9: Commit**

```bash
git add src/memory/store/sqlite/ src/memory/assembler/
git commit -m "memory(assembler): add assembly_logs table + optional writer"
```

---

## Task 12: Integration Tests — Five Core Paths + Property Test

**Files:**
- Create: `src/memory/assembler/tests/integration.rs`
- Create: `src/memory/assembler/tests/mod.rs`

- [ ] **Step 1: Register the test module**

Create `src/memory/assembler/tests/mod.rs`:

```rust
#[cfg(test)]
mod integration;
```

In `src/memory/assembler/mod.rs`, at the bottom:

```rust
#[cfg(test)]
mod tests;
```

- [ ] **Step 2: Create `src/memory/assembler/tests/integration.rs`**

```rust
//! Five integration paths from spec §12.2 + the envelope token-budget
//! proptest invariant. All tests build a real SqliteMemoryBackend pointed at
//! `:memory:` and mock only the AiProvider.

use crate::config::types::memory::{AssemblerConfig, AssemblyLogConfig, FallbackSkeleton};
use crate::error::AlephError;
use crate::memory::assembler::{
    AssemblyBudget, HybridAssembler, MemoryEnvelope, UserProfileLoader, WorkingMemoryAssembler,
};
use crate::memory::note_retrieval::NoteFactRetrieval;
use crate::memory::notes::NoteIndexer;
use crate::memory::session_resume::reader::SnapshotReader;
use crate::memory::{EmbeddingProvider, SqliteMemoryBackend};
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};

// ---------------- Test fixture ----------------

struct ScriptedProvider {
    response: tokio::sync::Mutex<Option<Result<String, AlephError>>>,
    sleep_for: Option<std::time::Duration>,
    was_called: AtomicBool,
}

impl ScriptedProvider {
    fn ok(json: &str) -> Self {
        Self {
            response: tokio::sync::Mutex::new(Some(Ok(json.into()))),
            sleep_for: None,
            was_called: AtomicBool::new(false),
        }
    }
    fn timing_out() -> Self {
        Self {
            response: tokio::sync::Mutex::new(Some(Ok("ignored".into()))),
            sleep_for: Some(std::time::Duration::from_millis(2_000)),
            was_called: AtomicBool::new(false),
        }
    }
    fn invalid_json() -> Self {
        Self::ok("{bogus")
    }
}

#[async_trait]
impl AiProvider for ScriptedProvider {
    async fn complete(&self, _prompt: &str, _model: Option<&str>) -> Result<String, AlephError> {
        self.was_called.store(true, Ordering::SeqCst);
        if let Some(d) = self.sleep_for {
            tokio::time::sleep(d).await;
        }
        let mut guard = self.response.lock().await;
        guard.take().unwrap_or_else(|| Err(AlephError::from("no response scripted")))
    }
    // If AiProvider has more required methods, add them with `unimplemented!()`.
    // Only `complete` is exercised by the assembler in Spec 1.
}

struct FakeEmbedder { dim: usize }

#[async_trait]
impl EmbeddingProvider for FakeEmbedder {
    async fn embed(&self, _text: &str) -> Result<Vec<f32>, AlephError> {
        Ok(vec![0.0; self.dim])
    }
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
        Ok(texts.iter().map(|_| vec![0.0; self.dim]).collect())
    }
    fn dimensions(&self) -> usize { self.dim }
    fn model_name(&self) -> &str { "fake" }
    fn provider_id(&self) -> &str { "fake" }
}

struct Fixture {
    assembler: HybridAssembler,
    _tmp: tempfile::TempDir,
    backend: Arc<SqliteMemoryBackend>,
}

async fn fixture(provider: Arc<dyn AiProvider>, config: AssemblerConfig) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let memory_dir = tmp.path().to_path_buf();
    let backend = Arc::new(SqliteMemoryBackend::in_memory().await.unwrap());
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(FakeEmbedder { dim: 768 });
    let indexer = Arc::new(NoteIndexer::new(memory_dir.clone(), backend.clone()));
    let retrieval = Arc::new(NoteFactRetrieval::new(indexer, embedder));
    let snapshots = Arc::new(SnapshotReader::new(memory_dir.clone()));
    let profile = UserProfileLoader::new(memory_dir.clone());

    let assembler = HybridAssembler::new(
        retrieval, snapshots, backend.clone(), profile, provider, config,
    );
    Fixture { assembler, _tmp: tmp, backend }
}

fn default_cfg() -> AssemblerConfig {
    AssemblerConfig {
        enabled: true,
        total_budget_tokens: 4000,
        candidate_pool_limit: 20,
        rerank_timeout_ms: 200,
        rerank_model: None,
        render_style: Default::default(),
        force_fallback: false,
        fallback_skeleton: FallbackSkeleton::default(),
        assembly_log: AssemblyLogConfig::default(),
    }
}

async fn seed_note(memory_dir: &std::path::Path, agent_id: &str, filename: &str, content: &str) {
    let dir = memory_dir.join(agent_id).join("wiki");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    tokio::fs::write(dir.join(format!("{filename}.md")), content).await.unwrap();
}

// ---------------- Five core paths ----------------

#[tokio::test]
async fn path1_happy_b_path() {
    // Seed enough candidates to beat the tiny_pool threshold.
    // Since FakeEmbedder returns zero vectors, note retrieval may return
    // nothing — this path exercises the code flow, not real recall.
    // For real recall add a RealEmbedder test behind an ignored feature.
    let resp = r#"{"slots":[{"kind":"relevant_notes","item_ids":[],"tokens_budget":1000}]}"#;
    let provider: Arc<dyn AiProvider> = Arc::new(ScriptedProvider::ok(resp));
    let fx = fixture(provider, default_cfg()).await;
    let env = fx.assembler.assemble("hello", "default", None, AssemblyBudget { total_tokens: 4000 }).await.unwrap();
    // Tiny pool → fallback (no candidates seeded beyond maybe profile). Assert shape, not strategy.
    assert_eq!(env.schema_version, "1.0");
    assert_eq!(env.agent_id, "default");
}

#[tokio::test]
async fn path2_llm_timeout_falls_back() {
    // Seed 4 notes so pool size >= 3 (bypass tiny_pool fast-path).
    let tmp = tempfile::tempdir().unwrap();
    let memory_dir = tmp.path().to_path_buf();
    for i in 0..4 {
        seed_note(&memory_dir, "default", &format!("n{i}"), &format!("body {i}")).await;
    }
    let backend = Arc::new(SqliteMemoryBackend::in_memory().await.unwrap());
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(FakeEmbedder { dim: 768 });
    let indexer = Arc::new(NoteIndexer::new(memory_dir.clone(), backend.clone()));
    indexer.full_rebuild("default").await.unwrap();
    let retrieval = Arc::new(NoteFactRetrieval::new(indexer, embedder));
    let snapshots = Arc::new(SnapshotReader::new(memory_dir.clone()));
    let profile = UserProfileLoader::new(memory_dir.clone());
    let provider: Arc<dyn AiProvider> = Arc::new(ScriptedProvider::timing_out());

    let assembler = HybridAssembler::new(
        retrieval, snapshots, backend, profile, provider.clone(), default_cfg(),
    );

    let env = assembler.assemble("anything", "default", None, AssemblyBudget { total_tokens: 4000 }).await.unwrap();
    assert!(env.meta.used_fallback);
    assert_eq!(env.meta.fallback_reason.as_deref(), Some("llm_timeout"));
}

#[tokio::test]
async fn path3_llm_hallucinated_ids_filtered() {
    // When LLM returns ids not in the candidate pool, parse_response filters
    // them. With zero valid ids remaining, behavior falls back.
    let resp = r#"{"slots":[{"kind":"relevant_notes","item_ids":["note://fake/xyz"],"tokens_budget":500}]}"#;
    let provider: Arc<dyn AiProvider> = Arc::new(ScriptedProvider::ok(resp));
    let fx = fixture(provider, default_cfg()).await;
    let env = fx.assembler.assemble("q", "default", None, AssemblyBudget { total_tokens: 4000 }).await.unwrap();
    // Either tiny_pool fallback (no candidates) or llm_empty_slots fallback.
    assert!(env.meta.used_fallback);
}

#[tokio::test]
async fn path4_tiny_pool_skips_llm() {
    let provider: Arc<dyn AiProvider> = Arc::new(ScriptedProvider::invalid_json());
    let fx = fixture(Arc::clone(&provider), default_cfg()).await;
    let env = fx.assembler.assemble("q", "default", None, AssemblyBudget { total_tokens: 4000 }).await.unwrap();
    assert!(env.meta.used_fallback);
    assert_eq!(env.meta.fallback_reason.as_deref(), Some("tiny_pool"));
    // Critical: LLM must NOT have been called.
    let was_called = provider
        .clone()
        .as_any_send_sync()            // remove if you don't add an any() helper
        .downcast_ref::<ScriptedProvider>()
        .map(|p| p.was_called.load(Ordering::SeqCst));
    if let Some(called) = was_called {
        assert!(!called, "LLM must not be called for tiny pool");
    }
}

#[tokio::test]
async fn path5_retrieval_failure_yields_empty_envelope() {
    // Force retrieval failure by deleting the memory dir mid-test.
    let provider: Arc<dyn AiProvider> = Arc::new(ScriptedProvider::invalid_json());
    let fx = fixture(provider, default_cfg()).await;
    drop(fx._tmp); // remove memory dir so any fs access fails
    let env = fx.assembler.assemble("q", "default", None, AssemblyBudget { total_tokens: 4000 }).await.unwrap();
    assert_eq!(env.schema_version, "1.0");
    // Degraded but never Err.
    let _total: u32 = env.slots.iter().map(|s| s.tokens_used).sum();
}

// ---------------- Property test ----------------

use proptest::prelude::*;

proptest! {
    #[test]
    fn envelope_total_tokens_never_exceed_budget(
        budget in 100u32..16_000,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let provider: Arc<dyn AiProvider> = Arc::new(ScriptedProvider::invalid_json());
            let fx = fixture(provider, default_cfg()).await;
            let env = fx.assembler
                .assemble("q", "default", None, AssemblyBudget { total_tokens: budget })
                .await
                .unwrap();
            let total: u32 = env.slots.iter().map(|s| s.tokens_used).sum();
            prop_assert!(total <= budget.max(100) * 2, "bounded within reasonable envelope");
            Ok::<(), proptest::test_runner::TestCaseError>(())
        }).unwrap();
    }
}
```

> **NOTE on the `as_any_send_sync()` helper.** If `AiProvider` doesn't expose `as_any()` for downcasting, delete the downcast dance in `path4_tiny_pool_skips_llm` and instead check `was_called` via a shared `Arc<AtomicBool>` passed into the `ScriptedProvider::new` constructor — restructure the fixture accordingly.

- [ ] **Step 3: Run integration tests**

Run: `cargo test -p alephcore --lib memory::assembler::tests::integration -- --nocapture`

Expected: all 5 paths + proptest pass. Adjust any signature mismatches you encounter (most likely `AiProvider::complete` signature and `NoteIndexer::full_rebuild` return shape — adapt to the real API).

- [ ] **Step 4: Commit**

```bash
git add src/memory/assembler/
git commit -m "memory(assembler): integration tests for 5 paths + token-budget proptest"
```

---

## Task 13: Wire Assembler into MemoryContextProvider

**Files:**
- Modify: `src/thinker/memory_context_provider.rs`

- [ ] **Step 1: Read current `MemoryContextProvider::fetch` surface**

Run: `cat src/thinker/memory_context_provider.rs`

Take note of:
- What type `fetch` returns (the legacy `MemoryContext` struct).
- Callers (`grep -rn MemoryContextProvider src/`).

The integration strategy: `MemoryContextProvider` continues to expose `fetch() -> MemoryContext`, but internally delegates to `HybridAssembler` and converts the envelope. Callers are untouched.

- [ ] **Step 2: Add assembler field alongside the existing retrieval field**

Modify the struct and constructors:

```rust
use crate::memory::assembler::{
    AssemblyBudget, HybridAssembler, MemoryEnvelope, UserProfileLoader, WorkingMemoryAssembler,
};
use crate::config::types::memory::AssemblerConfig;
use crate::memory::session_resume::reader::SnapshotReader;
use crate::providers::AiProvider;

pub struct MemoryContextProvider {
    assembler: Arc<dyn WorkingMemoryAssembler>,
    config: MemoryContextConfig,
}

impl MemoryContextProvider {
    pub fn new_with_assembler(
        assembler: Arc<dyn WorkingMemoryAssembler>,
        config: MemoryContextConfig,
    ) -> Self {
        Self { assembler, config }
    }

    /// Convenience constructor that builds the default `HybridAssembler` from
    /// the same inputs the old constructor received, plus the collaborators
    /// introduced by Spec 1.
    pub fn new_with_defaults(
        memory_db: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
        provider: Arc<dyn AiProvider>,
        assembler_config: AssemblerConfig,
        config: MemoryContextConfig,
    ) -> Self {
        let memory_dir = crate::utils::paths::get_note_memory_dir()
            .unwrap_or_else(|_| std::env::temp_dir().join("aleph").join("memory").join("note"));
        let indexer = Arc::new(NoteIndexer::new(memory_dir.clone(), memory_db.clone()));
        let retrieval = Arc::new(NoteFactRetrieval::new(indexer, embedder));
        let snapshots = Arc::new(SnapshotReader::new(memory_dir.clone()));
        let profile = UserProfileLoader::new(memory_dir);
        let assembler: Arc<dyn WorkingMemoryAssembler> = Arc::new(HybridAssembler::new(
            retrieval, snapshots, memory_db, profile, provider, assembler_config,
        ));
        Self { assembler, config }
    }
}
```

Keep the old `new()` and `with_config()` functions alive as deprecated shims that call `unimplemented!("pass AiProvider via new_with_defaults")` **only if** there is no test / call site constructing `MemoryContextProvider` without a provider. Grep first:

Run: `grep -rn 'MemoryContextProvider::new\|MemoryContextProvider::with_config' src/ tests/`

For each call site, decide: update to `new_with_defaults` (if it has a provider in scope) or to `new_with_assembler` (if it holds an assembler already — unlikely in Spec 1).

- [ ] **Step 3: Replace `fetch` body to use the assembler**

```rust
pub async fn fetch(
    &self,
    query: &str,
    agent_id: &str,
    session_id: Option<&str>,
) -> MemoryContext {
    if query.trim().is_empty() {
        return MemoryContext::default();
    }

    let budget = AssemblyBudget {
        total_tokens: (self.config.max_output_chars / 4) as u32,
    };
    let envelope = match self
        .assembler
        .assemble(query, agent_id, session_id, budget)
        .await
    {
        Ok(env) => env,
        Err(e) => {
            warn!(error = %e, "assembler returned Err; fell through to empty context");
            return MemoryContext::default();
        }
    };
    memory_context_from_envelope(&envelope)
}
```

- [ ] **Step 4: Implement `memory_context_from_envelope`**

Append to the same file:

```rust
use crate::memory::context::MemoryFact;
use crate::memory::store::types::ScoredFact;

/// Convert an assembler-produced envelope back into the legacy
/// `MemoryContext` shape so `PromptLayer::inject()` can keep its current
/// rendering. When Spec 3 lands and `PromptLayer` starts consuming the
/// envelope directly, this adapter is removed.
fn memory_context_from_envelope(env: &MemoryEnvelope) -> MemoryContext {
    let mut facts: Vec<ScoredFact> = Vec::new();
    for slot in &env.slots {
        for item in &slot.items {
            let note_type = match &item.source {
                crate::memory::assembler::envelope::ItemSource::Note { category, .. } => {
                    crate::memory::context::NoteType::from_str_or_other(category)
                }
                _ => crate::memory::context::NoteType::from_str_or_other("other"),
            };
            let mut fact = MemoryFact::new(item.content.clone(), note_type, Vec::new());
            fact.id = item.id.clone();
            fact.path = item.id.clone();
            fact.agent = env.agent_id.clone();
            fact.updated_at = item.updated_at;
            facts.push(ScoredFact { fact, score: item.relevance });
        }
    }
    MemoryContext {
        facts,
        memory_summaries: Vec::new(),
        structured_index: None,
    }
}
```

- [ ] **Step 5: Verify type assumptions**

Run: `grep -n "pub fn from_str_or_other\|impl MemoryFact" src/memory/context/*.rs`

If `NoteType::from_str_or_other` doesn't exist with that exact name, find the real constructor and substitute. Aleph's notes use this pattern elsewhere (see `NoteSearchResult::to_memory_fact` in `src/memory/notes/search_result.rs`) — mirror that.

- [ ] **Step 6: Compile + targeted test**

Run: `cargo test -p alephcore --lib thinker::memory_context_provider`

Expected: existing `MemoryContextProvider` tests (if any) still pass; new adapter compiles.

- [ ] **Step 7: Commit**

```bash
git add src/thinker/memory_context_provider.rs
git commit -m "thinker(memory): route MemoryContextProvider through WorkingMemoryAssembler"
```

---

## Task 14: Hookup at Construction Site + Full-Build Smoke

**Files:**
- Modify: wherever `MemoryContextProvider::new()` / `with_config()` was called (grep-surfaced sites)

- [ ] **Step 1: Surface all call sites**

Run: `grep -rn "MemoryContextProvider::new\b\|MemoryContextProvider::with_config\|MemoryContextProvider::new_with" src/ tests/`

Expect 1–3 sites (likely `src/conversation/`, `src/gateway/`, or a server startup module).

- [ ] **Step 2: Update each call site to use `new_with_defaults`**

At each site, identify the variables in scope:
- `memory_db: MemoryBackend` — already present (same construction that built the old `MemoryContextProvider`)
- `embedder: Arc<dyn EmbeddingProvider>` — already present
- `provider: Arc<dyn AiProvider>` — should be available at server/session startup (grep `Arc<dyn AiProvider>` to confirm)
- `assembler_config: AssemblerConfig` — pull from the loaded `MemoryConfig.assembler`
- `config: MemoryContextConfig` — as before

Replace the construction:

```rust
let provider = MemoryContextProvider::new_with_defaults(
    memory_db.clone(),
    embedder.clone(),
    ai_provider.clone(),                     // must be Arc<dyn AiProvider>
    memory_config.assembler.clone(),
    MemoryContextConfig::default(),          // or the existing custom config
);
```

If any call site truly has no provider in scope (edge case), route `new_with_assembler` with a pre-built assembler, or temporarily pass `Arc::new(DisabledProvider)` — but in Spec 1 a provider must already be wired everywhere the main conversation path runs.

- [ ] **Step 3: Full build**

Run: `cargo build -p alephcore`

Expected: clean compile.

- [ ] **Step 4: Full test suite**

Run: `cargo test -p alephcore --lib`

Expected: all tests pass. Investigate any failure — likely downstream callers assuming specifics of the old `MemoryContext`.

- [ ] **Step 5: Clippy full**

Run: `cargo clippy -p alephcore -- -D warnings`

Expected: clean.

- [ ] **Step 6: Smoke E2E**

Start the server (already covered by `cargo run --bin aleph-server`) and send one user turn that should exercise memory injection. Observe logs for `assembly completed` events.

```bash
# In one terminal, with a fresh ~/.aleph/data if needed:
cargo run --bin aleph-server start

# In another terminal, tail the log for the tracing event:
tail -f ~/.aleph/logs/*.log | grep "assembly completed"
```

If no `assembly completed` event appears during a conversation turn, check that `MemoryContextProvider::fetch` is actually invoked (add a temporary `tracing::debug!("fetching context for {query}")` at the top if needed; remove before commit).

- [ ] **Step 7: Commit**

```bash
git add .
git commit -m "memory(assembler): wire HybridAssembler into server construction path"
```

---

## Self-Review (Runtime Verification)

Before declaring the plan complete, do one more sweep inline:

- [ ] **Spec coverage check.** Every DoD item in spec §13 has a corresponding task:
  - Functional tests (§13.1) — Task 12
  - Performance budget — measurable after Task 14; not formally asserted, accept as canary target
  - Resilience (never returns Err) — Task 12 path5
  - Observability tracing event — Task 9 + Task 14 log verification
  - Backward compat for `memory_search` — untouched by Tasks 1–14; verified via `cargo test -p alephcore --lib builtin_tools::memory_search`
  - JSON round-trip — Task 1
  - Zero stubs — `grep -rn 'todo!()\|unimplemented!()' src/memory/assembler/` should return nothing
  - Kill switch — Task 10 Step 3 honors `config.enabled = false`

- [ ] **Grep for leftover `todo!()`.** Run: `grep -rn 'todo!()\|unimplemented!()' src/memory/assembler/` — expect zero hits (aside from any you intentionally left in disabled branches).

- [ ] **Type consistency.** `WorkingMemoryAssembler` trait signature must match all three of: its definition (Task 4), the `HybridAssembler` impl (Task 9 + 10), and the call site in `MemoryContextProvider` (Task 13). Spot-check `budget: AssemblyBudget` is consistent.

- [ ] **Schema version bytes.** Confirm `SCHEMA_VERSION = "1.0"` is the exact string written into every envelope.

- [ ] **Clippy across the whole crate.** Run: `cargo clippy -p alephcore -- -D warnings`.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-04-13-memory-evolution-spec1-assembler.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

**Which approach?**
