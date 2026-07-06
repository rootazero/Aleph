# Message Stream & Final Answer — Single Assembled-Message Reducer (Design)

- **Date**: 2026-07-06
- **Subsystem**: FEATURE_LOCATOR §4.7 消息流与最终答案汇总 (Message Stream & Final Answer)
- **Type**: 深度架构重构 + 连线修复 + 熵减 + bug 修复
- **Depth decision (locked)**: 架构重构 + 连线修复 (defer pure-UX throttle enhancements). ReplyEmitter **included** in the refactor.
- **Reference projects (gap analysis)**: codex (Rust), kimi-cli/kosong (Python), pi (TypeScript).

---

## 1. Problem statement

Aleph's §4.7 works, but is the outlier among the three reference agents on one structural axis: **the assembled agent message has no single owner.** The same streamed text is re-accumulated in ≥4 places, and the terminal "final answer" is recovered by scanning an event log via **two divergent idioms**. Every past "fanout leaked `<think>` / live bubble drifted from persisted transcript" fix patched a *symptom* of that missing single source.

### Current architecture (as-read)

- **Producer/reducer**: `orchestrator::dispatch::FlowStreamEvent` (`Delta`/`Reasoning`/`ToolCallStart`/…) → `execution_engine/event_drain.rs::DrainState` accumulates post-scrub `visible` deltas into `full_text`, stamps `chunk_index`, emits `StreamEvent::ResponseChunk`.
- **Vocabulary**: `StreamEvent` (`event_emitter/types.rs`, 15 variants) → `GatewayEventFrame` (`events/frame.rs`) wire form.
- **Emitters**: `GatewayEventEmitter` (bus + inline instant/typewriter via shared `plan_instant`); decorators `InstantBufferingEmitter`, `OriginFanoutEmitter`, `TeamFanoutEmitter`; `ReplyEmitter` (inbound channel path, its own `buffer`+`reasoning_buffer`+`StreamingController`).
- **Final answer (no dedicated table)**: `event_drain.rs::build_run_summary` → `RunSummary.final_response` → terminal `RunComplete`. Recovery via `reply_emitter/extract.rs::extract_final_response` (log-scan, newest `RunComplete` else concat deltas, sanitized) + atom `sanitize_final_response` + `sanitize.rs::sanitize_llm_output` (code-block-aware `<think>`/`completion-check`/`task-complete` stripper) + `split_reasoning`.

### Gap analysis vs references

| Axis | Aleph (current) | codex | kimi/kosong | pi |
|---|---|---|---|---|
| Delta → final | ❌ no single owner: 4 accumulators + log-scan with **2 idioms** | server-authoritative item + `last_agent_message` | pure reducer `merge_in_place` → `Message` | mutable snapshot handed out as `partial`, `result()` |
| Reasoning split | structural at `FlowStreamEvent` **+ post-hoc regex** net | distinct event families + tag strip | distinct `ThinkPart`; `extract_text()` | distinct `ThinkingContent` + `phase` |
| Sanitization | at **final only**; live deltas only memory-scrubbed | streaming parser strips tags **across delta boundaries** | structural | structural |
| Chunk snapshot | `ResponseChunk.full_text` exists (≈ pi `partial`) but **tripled** with `delta` + deprecated `content` | deltas display-only | n/a | every event carries `partial` |

**Unifying lesson**: one owner of the assembled message (codex trusts server item; kimi runs a type-driven reducer; pi mutates one snapshot). Port the kosong `merge_in_place` / pi `partial` pattern to Rust as one shared reducer primitive.

---

## 2. Gaps to close (mapped to protocol verbs)

- **G1 · 熵减** — `ResponseChunk.content` (deprecated `delta` alias) is still *read* by the OpenAI-compat API (`openai_api/completions/agent.rs:82,458`). Collapse 3 text fields → 2 (`delta` + `full_text`).
- **G2 · 连线/bugfix** — Two final-answer recovery idioms: cron/broadcast use richer `extract_final_response` (delta-concat fallback + sanitize); fanout/telegram/openai read raw `summary.final_response` (no fallback, inconsistent sanitize). Route **every** surface delivery through one finalize atom.
- **G3 · 架构重构 (centerpiece)** — Introduce one shared assembled-message reducer, reused by every accumulation site, feeding both the live `full_text` snapshot and the terminal `final_response`. Structurally retires the drift/leak bug class.
- **G4 · bugfix/连线 (CONFIRMED real)** — Live-stream vs final-answer sanitization asymmetry. Some models inline `<think>…</think>` inside `TextDelta` (proof: channel path defensively calls `split_reasoning` on the buffered content, `reply_emitter/emitter/helpers.rs:263`). The drain path streams `visible` deltas that are only `<memory-context>`-scrubbed (`event_drain.rs:80`), never `<think>`-stripped — so the live Panel bubble / OpenAI SSE leaks raw reasoning, then `RunComplete` retro-cleans it. Fix = codex-style cross-delta-boundary tag stripping in the streaming scrubber.

### Non-goals (explicitly deferred)

- **G5 enhancements** (YAGNI for this pass): codex adaptive catch-up chunking; pi `phase: final_answer|commentary` discriminator; kimi think-only→error guard (Aleph already returns `None` for pure-thinking turns).
- **Instant/typewriter coalescing** (`plan_instant`, `InstantBufferingEmitter`) stays as the orthogonal *throttle* layer downstream of the assembler — **not** merged in (would be the wrong abstraction).
- **`StreamingController`** (channel debounced edits) stays as the channel throttle; only its text *accumulation* delegates to the assembler.

---

## 3. Architecture — the `MessageAssembler` primitive

New home: `src/gateway/message_assembly/`. Pure logic, per-consumer instance (kosong pattern: shared logic, each consumer owns its pending buffer — not one global runtime instance).

```rust
/// Owns the running assembled agent message for one run/consumer.
pub struct MessageAssembler {
    visible: String,            // deliverable answer, accumulated
    reasoning: String,          // reasoning (from Reasoning events + extracted inline <think>)
    chunk_index: u32,
    scrubber: StreamingTagScrubber, // generalized StreamingContextScrubber
}

pub struct AssembledMessage {
    pub answer: Option<String>,    // sanitized visible text; None if nothing deliverable
    pub reasoning: Option<String>,
}

impl MessageAssembler {
    /// Feed a raw text delta. Returns the cleaned user-visible slice to stream
    /// now (empty if absorbed / held back as a partial-tag tail). Strips
    /// <memory-context> AND reasoning/completion tags across delta boundaries
    /// (G4), routing extracted <think> content into `reasoning`.
    pub fn push_text_delta(&mut self, raw: &str) -> VisibleDelta;
    pub fn push_reasoning_delta(&mut self, raw: &str);
    /// Live full snapshot — the ResponseChunk.full_text field.
    pub fn snapshot(&self) -> &str;
    /// Boundary flush (tool call / run end): drain held-back tag tail.
    pub fn flush_boundary(&mut self) -> VisibleDelta;
    /// Terminal answer, sanitized. Consumes self.
    pub fn finalize(self) -> AssembledMessage;
}
```

**Invariant (the anti-drift guarantee):** `snapshot()` (live `full_text`) and `finalize().answer` (terminal `final_response`) derive from the same accumulator — they can never disagree.

`StreamingTagScrubber` = `StreamingContextScrubber` generalized to hold back partial tags across deltas for `<memory-context>` **and** reasoning/completion tags. Reuses the existing proven hold-back-tail mechanism (连线优先).

### Message vs presentation boundary (important)

The assembler owns **message content only**. Delivery-time decorations stay outside it:
- Fallback-model notice (`reply_emitter/emitter/streaming.rs:365,375`)
- Runtime-metadata footer (`streaming.rs:401`, `runtime_footer.rs`)

These are appended to `finalize().answer` at the presentation layer, never fed into the assembler.

---

## 4. Wiring & deletion plan (connect-first, ordered)

Each slice compiles and is testable before the next.

| Slice | Change | Closes | Risk |
|---|---|---|---|
| **1 · primitive** | New `gateway/message_assembly/`: `MessageAssembler` + `AssembledMessage` + `StreamingTagScrubber` (generalize `StreamingContextScrubber`). Pure, unit-tested, unwired. | — | low |
| **2 · drain** | `DrainState.accumulated` + inline scrubber → one `MessageAssembler`. `ResponseChunk` streams assembler-cleaned `visible` (live `<think>` stripped) + `full_text = snapshot()`. | G4 | med |
| **3 · final unify** | `extract.rs` re-expressed over `AssembledMessage::finalize` logic; log-scan reducer (cron/broadcast) + fanout/telegram/openai all share the one finalize+fallback atom. | G2 | low |
| **4 · kill alias** | Migrate OpenAI reads (`agent.rs:82,458`) to `delta`; delete `ResponseChunk.content` + every `content:` populate site (event_drain, instant_buffer, fast_path, slash_command, telegram, feishu…). | G1 | med* |
| **5 · openai** | Stream `delta`, emit assembler-cleaned text, sanitize terminal. | G1+G2+G4 | med |
| **6 · ReplyEmitter** | `buffer`+`reasoning_buffer` → `Mutex<MessageAssembler>`; `push_str`/`take_reasoning_buffer`/`split_reasoning`+`sanitize_llm_output` → assembler `push`/`finalize`. Keep `StreamingController`/native/voice/overflow/**notice+footer** as presentation on `finalize().answer`. | 熵减 | **high** |

**\*Slice 4 gating check**: `ResponseChunk.content` also exists on the wire frame (`events/frame.rs:60`). Before deleting, verify whether the Panel or the persisted replay store reads `content`. If so, either migrate the panel to `delta` or retain a serialization-only wire alias while dropping the internal field. **No silent wire break.**

### 熵减 ledger (deleted)

- `ResponseChunk.content` field (internal; wire alias per Slice-4 check)
- `DrainState.accumulated` + its standalone scrubber wiring
- `ReplyEmitter.buffer` + `reasoning_buffer` (folded into the assembler)
- the divergent final-extraction idiom (fanout/telegram/openai reading raw `summary.final_response`)
- standalone `split_reasoning` / per-call `sanitize_llm_output` sites once folded (kept only if still referenced by tests)

---

## 5. Testing & verification

- **New unit tests (the primitive carries the proof):**
  - cross-boundary tag split (`<thi`|`nk>…</thi`|`nk>`) — the G4 core
  - `<memory-context>` strip across deltas
  - **`snapshot() == finalize().answer` invariant** (anti-drift)
  - reasoning routing (inline `<think>` → `reasoning`, not `answer`)
  - think-only turn → `answer: None`
- **Regression (must stay green, no rewrite-to-pass):** `event_emitter/tests.rs`, `reply_emitter/tests.rs`, `extract.rs` tests, `origin_fanout`/`team_fanout` tests, `instant_buffer` tests.
- **cargo frugality (per CLAUDE.md):** unit-test the primitive during dev; a single `cargo check --lib` before wrap-up. No full-suite runs.

### Risk hotspots

1. **ReplyEmitter message-vs-presentation boundary** — notices/footer must NOT enter the assembler (Slice 6).
2. **Instant-mode ordering** — assembler is strictly upstream of `plan_instant`; no double-accumulate / re-sanitize.
3. **OpenAI behavior change** — now strips inline `<think>` live; intended, but visible.
4. **Wire compat** — the Slice-4 `content` frame check.

---

## 6. Constraints & process

- **Branch isolation** (protocol redline): all code in a new git worktree; main untouched.
- **Redline conformance**: change is confined to `src/gateway/` (I/O + presentation boundary, R4) and reuses existing infra (StreamingContextScrubber, plan_instant, extract atom). No harness LOC added (R10 budget unaffected). No new deps (serde-only, tokio-only — unchanged).
- **Terminal deliverable of this design**: an implementation plan via `writing-plans`, then execution. No code is written during brainstorming.
