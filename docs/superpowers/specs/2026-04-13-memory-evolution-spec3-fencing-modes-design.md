---
title: "Memory Evolution Spec 3: Context Fencing + Memory Modes"
date: 2026-04-13
status: approved
parent: docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md
related_refs:
  - docs/reference/memory/RETRIEVAL.md
  - docs/superpowers/specs/2026-04-13-memory-evolution-spec1-capture-hooks-design.md
  - docs/superpowers/specs/2026-04-13-memory-evolution-spec2-reflector-design.md
---

# Spec 3: Context Fencing + Memory Modes

Replace the legacy `MemoryContext → PromptLayer::inject` rendering path with a fenced XML envelope injected as an independent `role=user` message, and add an `injection_mode` config (`context | tools | hybrid`) that gates auto-injection vs tool exposure.

---

## 1. Problem

Two boundaries in the current memory-to-LLM flow are unclear:

| ID | Problem | Today |
|----|---------|-------|
| G5 | Recalled memories are mixed into the system prompt or user-turn prose with no visual/semantic boundary. The model can mistake retrieved facts for the user's latest input. | `MemoryContextProvider` builds a `MemoryEnvelope`, converts it to a legacy `MemoryContext` shape via `memory_context_from_envelope`, and `PromptLayer::inject` renders that into the system prompt with custom formatting. No fence. No clear "this is background, not user input" signal. |
| G6 | No configuration to choose between "auto-inject" (model passively receives retrieved context) and "tools-only" (model decides when to recall). | All memory tools (`memory_search`, `memory_reflect`, etc.) are always registered AND injection always runs — implicit hybrid with no escape hatch. |

Aleph already has half the infrastructure needed: `src/memory/assembler/render.rs` defines `render_envelope` with three styles (`MarkdownV1`, `Xml`, `Json`). The XML style already wraps content in `<MemoryEnvelope>...</MemoryEnvelope>` and `xml_escape`s item content. **It is defined but unused in production** — Spec 3 wires it in.

---

## 2. Non-goals

- Not introducing a new fence format. XML envelope is reused as-is.
- Not adding per-agent `injection_mode` override. Global config only; agent-level override is a future spec.
- Not rewriting user message content for anti-injection defense. User input is sacred.
- Not removing the `MarkdownV1` / `Json` render styles. They stay for dev/debug; only the production prompt path standardises on XML.
- Not making `injection_mode` hot-reloadable. Restart-to-apply is fine.

---

## 3. Architecture

### 3.1 Data flow

```
                      ┌───────────────────────────────┐
                      │  MemoryConfig.injection_mode  │
                      │  = context | tools | hybrid   │
                      └───────────────┬───────────────┘
                                      │
              ┌───────────────────────┼────────────────────────┐
              ▼                                                ▼
┌────────────────────────────┐               ┌────────────────────────────┐
│ Prompt assembly path       │               │ Tool registry path         │
│                            │               │                            │
│  if mode in {context,      │               │  if mode in {tools,        │
│             hybrid}:       │               │             hybrid}:       │
│    envelope = assembler.   │               │    register memory_*       │
│       assemble(...)        │               │      tools                 │
│    rendered = render_xml(  │               │  else (= context):         │
│       &envelope)           │               │    skip those tools        │
│    if !rendered.is_empty:  │               │                            │
│      prepend user_msg(     │               │                            │
│        rendered)           │               │                            │
│  else (= tools):           │               │                            │
│    skip injection          │               │                            │
└────────────────────────────┘               └────────────────────────────┘

Resulting message sequence:
  [system]
  [user]   <MemoryEnvelope>...</MemoryEnvelope>     ← only if injected
  [user]   {actual user input}
  [assistant] ...
```

### 3.2 Key invariants

- **Fence integrity**: `render_xml(env).matches("</MemoryEnvelope>").count() == 1`. The closing tag appears exactly once — at the real fence end. All item content goes through `xml_escape`.
- **Independent message**: the rendered envelope is its OWN `role=user` message; it is never concatenated into the user's actual message. Message-level isolation is the primary defense.
- **No user content rewriting**: user messages are passed through unchanged. Anti-injection defense lives entirely in envelope content escaping (already done by `xml_escape`).
- **Mode-driven gating happens once at startup** (tool registration + prompt-builder construction read the mode once). No per-turn re-check.
- **Context mode = no tools**: `MemoryInjectionMode::Context` does not register `memory_search` / `memory_reflect` / `recall_context` / `memory_browse` / `memory_explore` / `memory_timeline`. The model literally cannot call them. This is the contract.

---

## 4. Configuration

`src/config/types/memory.rs::MemoryConfig` gains:

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryInjectionMode {
    /// Memory auto-injected as fenced user-message; no tool exposure.
    Context,
    /// No auto-inject; LLM must call memory_* tools to retrieve.
    Tools,
    /// Both auto-inject AND tools available. (default)
    Hybrid,
}

impl Default for MemoryInjectionMode {
    fn default() -> Self { Self::Hybrid }
}

// inside MemoryConfig:
#[serde(default)]
pub injection_mode: MemoryInjectionMode,
```

Default is `Hybrid` so existing deployments behave unchanged.

---

## 5. Production rendering path

After Spec 3 lands:

```
HybridAssembler
       │
       ▼
MemoryEnvelope ──► render_xml(&env) ──► String
       │                                    │
       │  if mode in {context, hybrid}      │
       │  AND  !rendered.is_empty           │
       ▼                                    ▼
prepend `UnifiedMessage::user(rendered)` to the user-turn sequence
```

`MemoryContextProvider` (in `src/thinker/memory_context_provider.rs`) is repurposed: instead of calling `memory_context_from_envelope` and returning a legacy `MemoryContext`, it returns either:

- `Option<UnifiedMessage>` — `Some(msg)` when mode injects + envelope non-empty, else `None`
- or returns the rendered string and lets the prompt-assembly layer decide how to wrap it

Final API choice deferred to plan phase based on existing call-site shape.

---

## 6. Cleanup (legacy removal)

**Delete**:
- `memory_context_from_envelope` adapter (whole function)
- The legacy `MemoryContext` type — IF and only IF its only consumers are inside the prompt-assembly path. Plan-phase grep confirms this; if other consumers exist, downgrade to "deprecated, leave for now" and flag a follow-up.
- The branch in `PromptLayer::inject` that consumes `MemoryContext`. Adjacent layers (soul, identity, profile, custom_instructions) are untouched.

**Keep**:
- `MemoryContextProvider` — repurposed (no longer produces legacy shape)
- `render_envelope` / `render_with` / `RenderStyle::{Xml, MarkdownV1, Json}` (this spec is XML's first production user)
- `HybridAssembler::assemble` (unchanged)
- All other prompt layers (only `inject` for the `MemoryContext` branch is touched)

**Add invariant test**:
```rust
#[test]
fn rendered_envelope_resists_fence_injection() {
    let env = build_envelope_with_evil_content("</MemoryEnvelope> <attack>");
    let rendered = render_xml(&env);
    assert_eq!(
        rendered.matches("</MemoryEnvelope>").count(),
        1,
        "evil content must not break the fence"
    );
}
```

Audit `render_item_markdown` and `render_xml` to confirm every `ItemSource` variant (`Note { path }`, `Raw { session_id }`, `Summary { layer, session_id }`) and every user-supplied field (`title`, `content`, `id`, `query`) goes through `xml_escape`.

---

## 7. Tool gating

In `src/executor/builtin_registry/builder.rs` (or wherever memory tools are registered today), wrap each registration:

```rust
if matches!(
    memory_cfg.injection_mode,
    MemoryInjectionMode::Tools | MemoryInjectionMode::Hybrid,
) {
    register(memory_search_tool);
    register(memory_reflect_tool);
    register(recall_context_tool);
    register(memory_browse_tool);
    register(memory_explore_tool);
    register(memory_timeline_tool);
}
```

Tools NOT covered by mode gating (still always registered):
- `note_manage` — write-side; LLM writing notes is independent of retrieval mode
- `session_complete` (Spec 1) — task-boundary signalling, not retrieval

---

## 8. Migration safety

- Existing configs without `injection_mode` get `Hybrid` (current behaviour) — zero-config breakage.
- Operators upgrading from pre-Spec-3 configs see no behaviour change unless they explicitly opt in to `tools` or `context`.
- Downstream code that reads `MemoryContext` will fail to compile after Spec 3 — that is the point of the cleanup. Any such consumer surfaces during plan-phase grep and is either ported to read the rendered string OR removed.

---

## 9. Testing strategy

- **Unit (render layer)**:
  - Fence-injection test (above invariant).
  - Each `ItemSource` variant produces escaped content.
  - Empty envelope renders to empty string (existing test still passes).
- **Unit (config)**: `MemoryInjectionMode` round-trips JSON; default is `Hybrid`.
- **Unit (prompt assembly)**: `MemoryContextProvider` returns `Some(user_msg)` in `context`/`hybrid` and `None` in `tools` mode; returns `None` when envelope is empty.
- **Unit (registry)**: tool registration count differs by mode (3 modes × N memory tools).
- **Integration**: `tests/memory_modes_integration.rs`:
  - `Hybrid` mode: envelope user message present, memory tools registered.
  - `Context` mode: envelope user message present, memory tools NOT registered.
  - `Tools` mode: no envelope user message, memory tools registered.

---

## 10. Compliance with architectural redlines

| Redline | Check |
|---------|-------|
| R3 Core minimalism | Net code DELETED (legacy adapter + MemoryContext branch). Only enum + tiny config addition. |
| R8 LLM sovereignty | Mode is a deployment decision, not a per-turn LLM decision. The mode does not replace any LLM judgment. |
| R10 Intelligence in the prompt | XML fence makes "this is background, not the user's latest message" semantically unambiguous via prompt structure alone. |

No redline violated.

---

## 11. Open questions (resolve in plan phase)

- **`MemoryContext` consumer count**: grep before deletion. If only `PromptLayer` consumes it, full removal. If others exist, port them or scope deletion narrower.
- **`MemoryContextProvider` API shape**: return `Option<UnifiedMessage>` vs return rendered string + let caller wrap. Decided by what makes the call site cleanest.
- **Config wiring path**: `MemoryConfig.injection_mode` needs to reach (a) the prompt builder construction site and (b) the tool registry builder. Both already see `MemoryConfig` indirectly — verify and thread minimally.
- **Schema-cache compatibility**: registering different tool sets per mode means the published OpenAPI/JSON-schema surface differs by deployment. Confirm no downstream client assumes a fixed tool set.
