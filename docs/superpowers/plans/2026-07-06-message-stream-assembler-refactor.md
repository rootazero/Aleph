# Message Stream & Final Answer — Single Assembled-Message Reducer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give Aleph §4.7 a single owner of the assembled agent message — one reducer that feeds both the live `full_text` snapshot and the terminal `final_response` — closing the live-vs-final `<think>` leak (G4), unifying final-answer extraction (G2), and retiring the deprecated `ResponseChunk.content` alias (G1).

**Architecture:** Introduce `MessageAssembler` (kosong `merge_in_place` / pi `partial` pattern) in `src/gateway/message_assembly/`, backed by a multi-tag streaming scrubber generalized from `memory::StreamingContextScrubber`. Adopt it at the drain, the final-answer extraction atoms, the OpenAI-compat surface, and the `ReplyEmitter`. The instant/typewriter throttle (`plan_instant`) and channel `StreamingController` stay as orthogonal presentation layers on top.

**Tech Stack:** Rust (tokio + serde), existing gateway infra (`StreamingContextScrubber`, `sanitize_llm_output`, `plan_instant`). No new dependencies.

## Global Constraints

- **Branch isolation:** all work in a new git worktree via `superpowers:using-git-worktrees`; main is never touched directly.
- **cargo frugality (CLAUDE.md):** run per-module unit tests during dev (`cargo test -p alephcore <module>`); a single `cargo check -p alephcore --lib` before wrap-up. No full-suite runs.
- **No new deps / serde-only / tokio-only** (Tech Stack redline).
- **Redline scope:** changes confined to `src/gateway/` + the one shared scrubber in `src/memory/`; no `src/harness/` LOC added (R10).
- **Rust style:** rustfmt (100 col), clippy clean, `pub(crate)` over `pub`, no `unwrap()` in non-test code, lock poison handled via `unwrap_or_else(|e| e.into_inner())`.
- **Message-vs-presentation boundary (INVARIANT):** the assembler owns message content only. Fallback-model notice and runtime footer are appended to `finalize().answer` at the delivery layer — never fed into the assembler.

---

## File Structure

- **Create** `src/gateway/message_assembly/mod.rs` — module root; re-exports `MessageAssembler`, `AssembledMessage`.
- **Create** `src/gateway/message_assembly/assembler.rs` — the `MessageAssembler` reducer + `AssembledMessage`.
- **Create** `src/gateway/message_assembly/tests.rs` — unit tests (the anti-drift proof).
- **Modify** `src/memory/streaming_scrubber.rs` — add multi-tag `with_tag_set` constructor + multi-pair scan (backward-compatible; single-pair `with_tags`/`default` unchanged).
- **Modify** `src/gateway/mod.rs` — register `pub mod message_assembly;`.
- **Modify** `src/gateway/execution_engine/event_drain.rs` — `DrainState` holds a `MessageAssembler`; `ResponseChunk` drops `content:`.
- **Modify** `src/gateway/reply_emitter/extract.rs` — `sanitize_final_response`/`extract_final_response` delegate to `AssembledMessage`.
- **Modify** `src/gateway/event_emitter/types.rs` — delete `ResponseChunk.content` field.
- **Modify** `src/gateway/events/frame.rs` — align wire frame with the field decision (Task 5 gate).
- **Modify** `src/gateway/openai_api/completions/agent.rs` — read `delta` not `content`.
- **Modify** `src/gateway/reply_emitter/emitter/{mod.rs,streaming.rs,helpers.rs}` — `buffer`+`reasoning_buffer` → `Mutex<MessageAssembler>`.
- **Modify** every `StreamEvent::ResponseChunk { … content: … }` construction site to drop `content:` (enumerated in Task 5).

---

## Task 1: Multi-tag streaming scrubber

Generalize `memory::StreamingContextScrubber` to discard a *set* of tag pairs across delta boundaries (memory-context + reasoning + completion), keeping the single-pair constructors backward-compatible for existing memory callers.

**Files:**
- Modify: `src/memory/streaming_scrubber.rs`

**Interfaces:**
- Consumes: existing `find_ascii_ci`, `max_partial_suffix_ascii_ci` helpers (unchanged).
- Produces:
  - `StreamingContextScrubber::with_tag_set(pairs: &[(&str, &str)]) -> Self` — multi-pair discard scrubber.
  - `pub const DISCARD_TAG_PAIRS: &[(&str, &str)]` — the assembler's tag set.
  - unchanged: `default()`, `with_tags(open, close)`, `feed(&str) -> String`, `flush() -> String`, `reset()`, `in_span()`.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/memory/streaming_scrubber.rs`:

```rust
#[test]
fn tag_set_strips_think_and_memory_across_boundaries() {
    let mut s = StreamingContextScrubber::with_tag_set(&[
        ("<memory-context>", "</memory-context>"),
        ("<think>", "</think>"),
    ]);
    // <think> split across deltas, interleaved with a memory-context span.
    let a = s.feed("answer <thi");
    let b = s.feed("nk>hidden reasoning</think> and <memory-context>x</memory-context> tail");
    assert_eq!(format!("{a}{b}"), "answer  and  tail");
    assert_eq!(s.flush(), "");
    assert!(!s.in_span());
}

#[test]
fn tag_set_holds_ambiguous_open_prefix_until_disambiguated() {
    let mut s = StreamingContextScrubber::with_tag_set(DISCARD_TAG_PAIRS);
    // "<th" is a prefix of "<think>"/"<thinking>"/"<thought>" — must hold back.
    let v1 = s.feed("keep <th");
    let v2 = s.feed("ursday plans"); // not a tag
    assert_eq!(format!("{v1}{v2}"), "keep <thursday plans");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore streaming_scrubber::tests::tag_set -- --nocapture`
Expected: FAIL — `with_tag_set` / `DISCARD_TAG_PAIRS` not found.

- [ ] **Step 3: Implement multi-pair support**

Replace the single-pair fields and scan with a pair-set. In `src/memory/streaming_scrubber.rs`, change the struct and constructors:

```rust
/// The tag pairs the message assembler discards from the visible stream:
/// echoed memory framing, chain-of-thought, and loop completion markers.
/// All ASCII (byte-level scan safe). `<task-complete/>` is self-closing and
/// is handled by the finalize-time `sanitize_llm_output` pass, not here.
pub const DISCARD_TAG_PAIRS: &[(&str, &str)] = &[
    ("<memory-context>", "</memory-context>"),
    ("<think>", "</think>"),
    ("<thinking>", "</thinking>"),
    ("<thought>", "</thought>"),
    ("<antthinking>", "</antthinking>"),
    ("<completion-check>", "</completion-check>"),
];

#[derive(Debug, Clone)]
pub struct StreamingContextScrubber {
    /// Discard tag pairs (open, close). One entry for the single-tag ctors.
    pairs: Vec<(String, String)>,
    /// Index into `pairs` of the currently open span, or `None`.
    active: Option<usize>,
    /// Bytes held back across calls because they might be a tag prefix.
    buf: String,
}
```

Rewrite the constructors:

```rust
impl Default for StreamingContextScrubber {
    fn default() -> Self {
        Self::with_tags(DEFAULT_OPEN_TAG, DEFAULT_CLOSE_TAG)
    }
}

impl StreamingContextScrubber {
    /// Single-pair scrubber (existing memory callers).
    pub fn with_tags(open: impl Into<String>, close: impl Into<String>) -> Self {
        let open = open.into();
        let close = close.into();
        Self::with_tag_set(&[(open.as_str(), close.as_str())])
    }

    /// Multi-pair discard scrubber. Every pair's tags must be ASCII.
    pub fn with_tag_set(pairs: &[(&str, &str)]) -> Self {
        debug_assert!(!pairs.is_empty(), "at least one tag pair required");
        for (o, c) in pairs {
            debug_assert!(!o.is_empty() && !c.is_empty(), "tags must not be empty");
            debug_assert!(o.is_ascii() && c.is_ascii(), "tags must be ASCII");
        }
        Self {
            pairs: pairs.iter().map(|(o, c)| ((*o).to_string(), (*c).to_string())).collect(),
            active: None,
            buf: String::new(),
        }
    }

    pub fn reset(&mut self) {
        self.active = None;
        self.buf.clear();
    }

    #[must_use]
    pub const fn in_span(&self) -> bool {
        self.active.is_some()
    }
}
```

Rewrite `feed` to scan for the earliest of any open tag, and hold back the longest partial-suffix across all open tags:

```rust
    pub fn feed(&mut self, text: &str) -> String {
        if text.is_empty() {
            return String::new();
        }
        let mut work = std::mem::take(&mut self.buf);
        work.push_str(text);
        let work_bytes = work.as_bytes();
        let mut out = String::new();
        let mut cursor: usize = 0;

        loop {
            if let Some(active) = self.active {
                let close = self.pairs[active].1.as_bytes();
                match find_ascii_ci(&work_bytes[cursor..], close) {
                    Some(rel) => {
                        cursor = cursor + rel + close.len();
                        self.active = None;
                    }
                    None => {
                        let tail = max_partial_suffix_ascii_ci(&work_bytes[cursor..], close);
                        if tail > 0 {
                            self.buf = work[work.len() - tail..].to_string();
                        }
                        return out;
                    }
                }
            } else {
                // Earliest open tag among all pairs.
                let mut best: Option<(usize, usize)> = None; // (abs_idx, pair_idx)
                for (i, (open, _)) in self.pairs.iter().enumerate() {
                    if let Some(rel) = find_ascii_ci(&work_bytes[cursor..], open.as_bytes()) {
                        let abs = cursor + rel;
                        if best.is_none_or(|(b, _)| abs < b) {
                            best = Some((abs, i));
                        }
                    }
                }
                match best {
                    Some((abs, i)) => {
                        out.push_str(&work[cursor..abs]);
                        cursor = abs + self.pairs[i].0.len();
                        self.active = Some(i);
                    }
                    None => {
                        // Hold back the longest partial suffix matching any open tag.
                        let tail = self
                            .pairs
                            .iter()
                            .map(|(open, _)| {
                                max_partial_suffix_ascii_ci(&work_bytes[cursor..], open.as_bytes())
                            })
                            .max()
                            .unwrap_or(0);
                        if tail > 0 {
                            let split = work.len() - tail;
                            out.push_str(&work[cursor..split]);
                            self.buf = work[split..].to_string();
                        } else {
                            out.push_str(&work[cursor..]);
                        }
                        return out;
                    }
                }
            }
        }
    }
```

Update `flush` to use `active`:

```rust
    pub fn flush(&mut self) -> String {
        if self.active.is_some() {
            self.buf.clear();
            self.active = None;
            return String::new();
        }
        std::mem::take(&mut self.buf)
    }
```

- [ ] **Step 4: Run tests to verify pass (new + all pre-existing scrubber tests)**

Run: `cargo test -p alephcore streaming_scrubber`
Expected: PASS — the new `tag_set_*` tests and all pre-existing single-pair tests stay green (backward compatibility proven).

- [ ] **Step 5: Commit**

```bash
git add src/memory/streaming_scrubber.rs
git commit -m "memory: generalize streaming scrubber to a discard tag-set"
```

---

## Task 2: The `MessageAssembler` primitive

**Files:**
- Create: `src/gateway/message_assembly/mod.rs`
- Create: `src/gateway/message_assembly/assembler.rs`
- Create: `src/gateway/message_assembly/tests.rs`
- Modify: `src/gateway/mod.rs`

**Interfaces:**
- Consumes: `memory::StreamingContextScrubber::{with_tag_set, DISCARD_TAG_PAIRS, feed, flush}`; `reply_emitter`'s `sanitize_llm_output` — but to avoid a visibility cycle, Task 2 calls the sanitizer via a small local re-export. Use `crate::gateway::reply_emitter::sanitize_final_text` (added in Task 4). For Task 2 in isolation, `finalize` sanitization is inlined; Task 4 swaps it to the shared atom.
- Produces:
  - `MessageAssembler::new() -> Self`
  - `push_text_delta(&mut self, raw: &str) -> String` (visible slice to stream now)
  - `push_reasoning_delta(&mut self, raw: &str)`
  - `flush_boundary(&mut self) -> String` (drain held-back tail at tool/run boundary)
  - `snapshot(&self) -> &str` (== `ResponseChunk.full_text`)
  - `next_chunk_index(&mut self) -> u32`
  - `finalize(&mut self) -> AssembledMessage`
  - `struct AssembledMessage { answer: Option<String>, reasoning: Option<String> }`

- [ ] **Step 1: Write the failing tests**

Create `src/gateway/message_assembly/tests.rs`:

```rust
use super::MessageAssembler;

#[test]
fn snapshot_equals_finalized_answer_the_antidrift_invariant() {
    let mut a = MessageAssembler::new();
    let v1 = a.push_text_delta("Hello ");
    let v2 = a.push_text_delta("world");
    assert_eq!(format!("{v1}{v2}"), "Hello world");
    let snap = a.snapshot().to_string();
    let final_ans = a.finalize().answer.unwrap();
    assert_eq!(snap, final_ans, "live snapshot must equal terminal answer");
    assert_eq!(final_ans, "Hello world");
}

#[test]
fn inline_think_stripped_from_visible_across_deltas() {
    let mut a = MessageAssembler::new();
    let v1 = a.push_text_delta("answer <thi");
    let v2 = a.push_text_delta("nk>secret</think> done");
    assert_eq!(format!("{v1}{v2}"), "answer  done");
    assert_eq!(a.finalize().answer.as_deref(), Some("answer  done"));
}

#[test]
fn reasoning_deltas_route_to_reasoning_not_answer() {
    let mut a = MessageAssembler::new();
    a.push_text_delta("visible");
    a.push_reasoning_delta("step 1 ");
    a.push_reasoning_delta("step 2");
    let m = a.finalize();
    assert_eq!(m.answer.as_deref(), Some("visible"));
    assert_eq!(m.reasoning.as_deref(), Some("step 1 step 2"));
}

#[test]
fn think_only_turn_yields_no_answer() {
    let mut a = MessageAssembler::new();
    a.push_text_delta("<think>only thinking</think>");
    let m = a.finalize();
    assert_eq!(m.answer, None, "pure-reasoning turn delivers nothing");
}

#[test]
fn chunk_index_is_monotonic() {
    let mut a = MessageAssembler::new();
    assert_eq!(a.next_chunk_index(), 0);
    assert_eq!(a.next_chunk_index(), 1);
    assert_eq!(a.next_chunk_index(), 2);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore message_assembly`
Expected: FAIL — module does not exist.

- [ ] **Step 3: Implement the assembler**

Create `src/gateway/message_assembly/assembler.rs`:

```rust
//! The single owner of a run's assembled agent message.
//!
//! Ports the kosong `merge_in_place` / pi `partial` pattern: one reducer per
//! run/consumer that accumulates the deliverable answer and the reasoning,
//! stripping reasoning/completion/memory framing from the *live* visible
//! stream across delta boundaries (G4). `snapshot()` (the live `full_text`)
//! and `finalize().answer` (the terminal `final_response`) derive from the
//! same accumulator and can never disagree — the anti-drift invariant.

use crate::memory::streaming_scrubber::{StreamingContextScrubber, DISCARD_TAG_PAIRS};

/// The terminal, deliverable form of a run's assembled message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssembledMessage {
    /// Sanitized visible answer; `None` when nothing deliverable survives.
    pub answer: Option<String>,
    /// Accumulated reasoning; `None` when empty.
    pub reasoning: Option<String>,
}

/// Reducer over a run's streamed text + reasoning.
#[derive(Debug)]
pub struct MessageAssembler {
    visible: String,
    reasoning: String,
    chunk_index: u32,
    scrubber: StreamingContextScrubber,
}

impl Default for MessageAssembler {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageAssembler {
    #[must_use]
    pub fn new() -> Self {
        Self {
            visible: String::new(),
            reasoning: String::new(),
            chunk_index: 0,
            scrubber: StreamingContextScrubber::with_tag_set(DISCARD_TAG_PAIRS),
        }
    }

    /// Feed a raw text delta. Returns the cleaned, user-visible slice to stream
    /// now (empty if fully absorbed or held back as a partial-tag tail).
    pub fn push_text_delta(&mut self, raw: &str) -> String {
        let visible = self.scrubber.feed(raw);
        if !visible.is_empty() {
            self.visible.push_str(&visible);
        }
        visible
    }

    /// Feed a reasoning delta (from `FlowStreamEvent::Reasoning`).
    pub fn push_reasoning_delta(&mut self, raw: &str) {
        self.reasoning.push_str(raw);
    }

    /// Drain any held-back tag tail at a tool-call / run boundary.
    pub fn flush_boundary(&mut self) -> String {
        let tail = self.scrubber.flush();
        if !tail.is_empty() {
            self.visible.push_str(&tail);
        }
        tail
    }

    /// The live full visible snapshot — populates `ResponseChunk.full_text`.
    #[must_use]
    pub fn snapshot(&self) -> &str {
        &self.visible
    }

    /// Next monotonic chunk index.
    pub fn next_chunk_index(&mut self) -> u32 {
        let idx = self.chunk_index;
        self.chunk_index += 1;
        idx
    }

    /// Terminal answer + reasoning. Flushes any held tail first, then applies
    /// the idempotent final sanitizer (catches self-closing `<task-complete/>`
    /// and trailing incomplete directives the streaming scrubber leaves).
    pub fn finalize(&mut self) -> AssembledMessage {
        let _ = self.flush_boundary();
        let answer = finalize_sanitize(&self.visible);
        let reasoning = if self.reasoning.trim().is_empty() {
            None
        } else {
            Some(self.reasoning.clone())
        };
        AssembledMessage { answer, reasoning }
    }
}

/// Final sanitize pass. Inlined in Task 2; Task 4 replaces this body with a
/// delegation to `crate::gateway::reply_emitter::sanitize_final_text`.
fn finalize_sanitize(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
```

Create `src/gateway/message_assembly/mod.rs`:

```rust
//! Single assembled-message reducer (FEATURE_LOCATOR §4.7).
//!
//! One owner of "given the stream so far, what is the assembled visible answer
//! + reasoning" — reused by the drain, the final-answer extraction atoms, the
//! OpenAI-compat surface, and the `ReplyEmitter`, so the live bubble and the
//! persisted transcript can never drift.

mod assembler;

#[cfg(test)]
mod tests;

pub use assembler::{AssembledMessage, MessageAssembler};
```

- [ ] **Step 4: Register the module**

In `src/gateway/mod.rs`, add alongside the other `pub mod` declarations (keep alphabetical grouping with neighbors like `event_emitter` / `reply_emitter`):

```rust
pub mod message_assembly;
```

- [ ] **Step 5: Run tests to verify pass**

Run: `cargo test -p alephcore message_assembly`
Expected: PASS — all five tests green.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/message_assembly/ src/gateway/mod.rs
git commit -m "gateway: add MessageAssembler reducer (single assembled-message owner)"
```

---

## Task 3: Wire the drain to `MessageAssembler` (Slice 2 — closes G4)

Replace `DrainState.accumulated` + its inline scrubber with a `MessageAssembler`, so the live `ResponseChunk` stream is `<think>`-stripped and `full_text` is the assembler snapshot.

**Files:**
- Modify: `src/gateway/execution_engine/event_drain.rs`

**Interfaces:**
- Consumes: `MessageAssembler::{new, push_text_delta, push_reasoning_delta, snapshot, next_chunk_index, flush_boundary}`.
- Produces: unchanged `emit_flow_event` signature.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `event_drain.rs` (a `CollectingEventEmitter`-based test; follow the existing test style in that file):

```rust
#[tokio::test]
async fn inline_think_is_stripped_from_live_response_chunks() {
    let emitter: Arc<dyn EventEmitter> = Arc::new(
        crate::gateway::event_emitter::CollectingEventEmitter::new(),
    );
    let state = Arc::new(Mutex::new(DrainState::default()));
    // Deltas that inline a <think> block split across chunk boundaries.
    for d in ["Here is <thi", "nk>hidden</think> the answer"] {
        emit_flow_event(FlowStreamEvent::Delta(d.to_string()), &emitter, "r1", &state)
            .await
            .unwrap();
    }
    let collector = emitter
        .as_any() // if unavailable, downcast via a concrete Arc<CollectingEventEmitter> local
        .downcast_ref::<crate::gateway::event_emitter::CollectingEventEmitter>()
        .unwrap();
    let joined: String = collector
        .events()
        .await
        .into_iter()
        .filter_map(|e| match e {
            StreamEvent::ResponseChunk { delta, .. } => Some(delta),
            _ => None,
        })
        .collect();
    assert_eq!(joined, "Here is  the answer", "no raw <think> in the live stream");
}
```

> If `EventEmitter` has no `as_any`, bind a `let collector = Arc::new(CollectingEventEmitter::new());` and pass `collector.clone() as Arc<dyn EventEmitter>` into `emit_flow_event`, then read `collector.events()` directly — mirror whichever pattern the existing tests in this file already use.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore event_drain::tests::inline_think -- --nocapture`
Expected: FAIL — current drain streams the raw `<think>` block.

- [ ] **Step 3: Replace `DrainState` internals**

In `src/gateway/execution_engine/event_drain.rs`, replace the `DrainState` struct + `Default` impl (lines ~30-56) with:

```rust
#[derive(Debug, Default)]
pub(crate) struct DrainState {
    /// The single assembled-message reducer for this run. Owns the visible
    /// text accumulator (`full_text` source), the reasoning accumulator, the
    /// monotonic chunk index, and the cross-boundary discard scrubber that
    /// strips `<memory-context>` + reasoning/completion tags from the live
    /// stream (consolidated from the retired StreamingDeltaSink + inline
    /// StreamingContextScrubber).
    assembler: crate::gateway::message_assembly::MessageAssembler,
}
```

Replace the `FlowStreamEvent::Delta` arm (lines ~71-106) with:

```rust
        FlowStreamEvent::Delta(text) => {
            let emitted = {
                let mut s = state.lock().await;
                let visible = s.assembler.push_text_delta(&text);
                if visible.is_empty() {
                    None
                } else {
                    let full_text = s.assembler.snapshot().to_string();
                    let idx = s.assembler.next_chunk_index();
                    Some((visible, full_text, idx))
                }
            };
            if let Some((visible, full_text, idx)) = emitted {
                let seq = emitter.next_seq();
                emitter
                    .emit(StreamEvent::ResponseChunk {
                        run_id: run_id.to_string(),
                        seq,
                        delta: visible,
                        full_text,
                        chunk_index: idx,
                        is_final: false,
                        is_intermediate: false,
                    })
                    .await?;
            }
        }
```

> Note: the `content:` field is intentionally dropped here (Task 5 deletes the field). If Task 5 has not run yet, keep `content: visible.clone()` temporarily and let Task 5 remove it — but prefer running Task 5's field deletion in the same worktree before this compiles cleanly. Order is enforced by the plan: run Task 5 last only if you retain the temporary `content:`. **Recommended:** drop `content:` here and complete Task 5 immediately after so the crate compiles.

Update the `FlowStreamEvent::Reasoning` arm to also feed the assembler (so `finalize().reasoning` is populated) — after the existing `emitter.emit(StreamEvent::Reasoning{…})`, add before it:

```rust
        FlowStreamEvent::Reasoning(text) => {
            {
                let mut s = state.lock().await;
                s.assembler.push_reasoning_delta(&text);
            }
            let seq = emitter.next_seq();
            emitter
                .emit(StreamEvent::Reasoning {
                    run_id: run_id.to_string(),
                    seq,
                    content: text,
                    is_complete: false,
                })
                .await?;
        }
```

Update `flush_text_boundary` (used at `ToolCallStart`, ~line 128) to drain via the assembler. Find its body and replace the scrubber-drain + `accumulated` reset with:

```rust
        // Tool/text-iteration boundary: drain any held-back tag tail so a
        // partial tag doesn't straddle the boundary. The assembler keeps its
        // accumulated visible text across the boundary (the run's full answer),
        // matching the prior single-run full_text semantics.
        let tail = {
            let mut s = state.lock().await;
            s.assembler.flush_boundary()
        };
        if !tail.is_empty() {
            let (full_text, idx, seq) = {
                let mut s = state.lock().await;
                (s.assembler.snapshot().to_string(), s.assembler.next_chunk_index(), emitter.next_seq())
            };
            emitter
                .emit(StreamEvent::ResponseChunk {
                    run_id: run_id.to_string(),
                    seq,
                    delta: tail,
                    full_text,
                    chunk_index: idx,
                    is_final: false,
                    is_intermediate: false,
                })
                .await?;
        }
```

> Read the existing `flush_text_boundary` body first and preserve any behavior it has beyond the scrubber drain (e.g. `accumulated` reset semantics). The prior code reset `accumulated` at each tool boundary "so each iteration's running text starts fresh". Decide per existing tests: if a test asserts `full_text` resets per iteration, keep a `self.visible.clear()` equivalent by adding a `MessageAssembler::reset_iteration()` that clears `visible` but preserves reasoning + chunk_index; otherwise keep accumulation. Add that method to Task 2's assembler if the tests require it.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p alephcore event_drain`
Expected: PASS — new inline-think test green; existing `event_drain` tests green (adjust the per-iteration reset per the note above if one fails).

- [ ] **Step 5: Commit**

```bash
git add src/gateway/execution_engine/event_drain.rs src/gateway/message_assembly/
git commit -m "gateway: drain streams through MessageAssembler (strip inline <think> live, G4)"
```

---

## Task 4: Unify final-answer extraction (Slice 3 — closes G2)

Make `sanitize_final_response` / `extract_final_response` the shared atoms, expressed once, reused by cron/broadcast/fanout/telegram, and wire the assembler's `finalize` to the same sanitizer.

**Files:**
- Modify: `src/gateway/reply_emitter/extract.rs`
- Modify: `src/gateway/reply_emitter/mod.rs`
- Modify: `src/gateway/message_assembly/assembler.rs`

**Interfaces:**
- Produces: `pub(crate) fn sanitize_final_text(text: &str) -> Option<String>` in `reply_emitter` (the one atom). `sanitize_final_response` keeps its name/signature and calls it.
- Consumes (assembler): `crate::gateway::reply_emitter::sanitize_final_text`.

- [ ] **Step 1: Write the failing test**

Add to `src/gateway/message_assembly/tests.rs`:

```rust
#[test]
fn finalize_uses_shared_sanitizer_for_task_complete_marker() {
    let mut a = MessageAssembler::new();
    a.push_text_delta("done <task-complete/>");
    // The self-closing marker is caught by the shared final sanitizer,
    // not the streaming scrubber.
    assert_eq!(a.finalize().answer.as_deref(), Some("done"));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p alephcore message_assembly::tests::finalize_uses_shared`
Expected: FAIL — Task 2's inlined `finalize_sanitize` only trims; `<task-complete/>` survives.

- [ ] **Step 3: Expose the shared atom and delegate**

In `src/gateway/reply_emitter/extract.rs`, rename the atom's body into a `sanitize_final_text` fn and keep `sanitize_final_response` as its caller:

```rust
/// The single sanitize atom: raw run text → clean deliverable, or `None`.
#[must_use]
pub(crate) fn sanitize_final_text(text: &str) -> Option<String> {
    let sanitized = sanitize_llm_output(text);
    if sanitized.trim().is_empty() {
        None
    } else {
        Some(sanitized.into_owned())
    }
}

/// Back-compat name used by fanout/telegram/cron/broadcast.
#[must_use]
pub(crate) fn sanitize_final_response(text: &str) -> Option<String> {
    sanitize_final_text(text)
}
```

In `src/gateway/reply_emitter/mod.rs`, export it:

```rust
pub(crate) use extract::{extract_final_response, sanitize_final_response, sanitize_final_text};
```

In `src/gateway/message_assembly/assembler.rs`, replace the inlined `finalize_sanitize` with a delegation:

```rust
fn finalize_sanitize(text: &str) -> Option<String> {
    crate::gateway::reply_emitter::sanitize_final_text(text)
}
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p alephcore message_assembly && cargo test -p alephcore reply_emitter::extract`
Expected: PASS — the `<task-complete/>` test green; all existing `extract.rs` tests green.

- [ ] **Step 5: Commit**

```bash
git add src/gateway/reply_emitter/extract.rs src/gateway/reply_emitter/mod.rs src/gateway/message_assembly/assembler.rs
git commit -m "gateway: single final-answer sanitize atom shared by assembler + extraction"
```

---

## Task 5: Delete the `ResponseChunk.content` alias (Slice 4 — closes G1)

**Files:**
- Modify: `src/gateway/event_emitter/types.rs`
- Modify: `src/gateway/events/frame.rs`
- Modify: `src/gateway/openai_api/completions/agent.rs`
- Modify: every `ResponseChunk { … content: … }` construction site (enumerated below)

**Interfaces:**
- Produces: `StreamEvent::ResponseChunk` without a `content` field.

- [ ] **Step 1: Gate check — does the wire frame / panel read `content`?**

Run:
```bash
grep -rn "ResponseChunk" src/gateway/events/frame.rs
grep -rn '"content"\|\.content' panel/ 2>/dev/null | grep -i chunk
grep -rn "content" src/gateway/session_service 2>/dev/null | grep -i chunk
```
Expected: identify whether `GatewayEventFrame::ResponseChunk` carries `content` and whether the panel/replay deserializes it.
- **If the wire frame carries `content` and a consumer reads it:** keep the wire `content` field in `frame.rs` (serialization-only), but remove the internal `StreamEvent::ResponseChunk.content`; in the `From<StreamEvent> for GatewayEventFrame` mapping, set the wire `content` from `delta`. Document this in a comment.
- **If nothing reads the wire `content`:** delete it from `frame.rs` too.

- [ ] **Step 2: Migrate the OpenAI-compat reads to `delta`**

In `src/gateway/openai_api/completions/agent.rs`, change the two match arms:
- line ~82: `StreamEvent::ResponseChunk { content, .. }` → `StreamEvent::ResponseChunk { delta, .. }`, and update the body that used `content` to use `delta`.
- line ~458: `StreamEvent::ResponseChunk { content: chunk, .. }` → `StreamEvent::ResponseChunk { delta: chunk, .. }`.

- [ ] **Step 3: Delete the field**

In `src/gateway/event_emitter/types.rs`, delete the `content: String,` field (and its doc comment, lines ~112-114) from `StreamEvent::ResponseChunk`.

In `src/gateway/event_emitter/mod.rs::emit_response_chunk`, delete `content: delta.to_string(),`.

Remove `content:` from every construction site:
```bash
grep -rn "content:" src --include='*.rs' | grep -B2 -A2 "ResponseChunk" 
```
Known sites (verify + fix each): `event_drain.rs` (Delta arm, boundary flush), `instant_buffer.rs::final_chunk` + the two intermediate/marker constructions, `fast_path.rs:193`, `slash_command.rs:283`, `interfaces/telegram/streaming/orchestrator.rs:230,306`, `interfaces/feishu/feishu_outbound/streaming.rs`, `approval/operator_requester.rs` (if it builds `GatewayEventFrame::ResponseChunk` — that is the wire frame; handle per Step 1's decision), and any test constructors in `event_emitter/tests.rs`, `instant_buffer.rs` tests, `extract.rs` tests, `origin_fanout`/`team_fanout` tests.

- [ ] **Step 4: Compile + test**

Run: `cargo check -p alephcore --lib`
Expected: clean — no `content` references remain (compiler enumerates any missed site).
Run: `cargo test -p alephcore event_emitter && cargo test -p alephcore reply_emitter`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A src/gateway/
git commit -m "gateway: drop deprecated ResponseChunk.content alias (read delta)"
```

---

## Task 6: OpenAI-compat surface parity (Slice 5)

Ensure the OpenAI-compatible SSE surface streams assembler-cleaned `delta` (already true after Task 5) and its terminal frame carries no raw reasoning.

**Files:**
- Modify: `src/gateway/openai_api/completions/agent.rs`

**Interfaces:**
- Consumes: `StreamEvent::ResponseChunk { delta, .. }` (post-Task-5).

- [ ] **Step 1: Write the failing test**

Add an OpenAI-completions streaming test (mirror the existing test harness in that module) that feeds a `ResponseChunk { delta: "<think>x</think>visible", .. }`-equivalent produced by the drain and asserts the SSE payload contains `visible` and never `<think>`. If the drain already strips (Task 3), the test asserts the OpenAI surface forwards the already-clean `delta` verbatim (regression guard against a future re-introduction of raw `content`).

```rust
#[tokio::test]
async fn openai_stream_forwards_clean_delta_only() {
    // Build the agent SSE emitter, feed a ResponseChunk whose delta is the
    // assembler-cleaned text, assert the SSE frame carries exactly that delta.
    // (Follow the module's existing ChatCompletionChunk assertion pattern.)
}
```

- [ ] **Step 2: Run to verify it fails / passes**

Run: `cargo test -p alephcore openai_api::completions`
Expected: If it passes immediately, Task 5 already delivered parity — keep the test as a regression guard and note it in the commit. If it fails, fix the delta forwarding in the `ResponseChunk` arm.

- [ ] **Step 3: Confirm no terminal reasoning leak**

Inspect the `StreamEvent::RunComplete` arm (line ~128): it emits `finish_reason: stop` + usage + `[DONE]` and no text — correct (text already streamed as clean deltas). No change needed unless the test above requires the terminal frame to also carry a sanitized final; if so, source it from `summary.final_response` via `sanitize_final_response`.

- [ ] **Step 4: Commit**

```bash
git add src/gateway/openai_api/completions/agent.rs
git commit -m "gateway: openai-compat forwards assembler-clean delta (regression guard)"
```

---

## Task 7: `ReplyEmitter` adoption (Slice 6 — highest risk)

Replace `ReplyEmitter.buffer` + `reasoning_buffer` with a `Mutex<MessageAssembler>`; route delta/reasoning accumulation and finalize through it; keep `StreamingController`/native/voice/overflow/**notice+footer** as the presentation layer.

**Files:**
- Modify: `src/gateway/reply_emitter/emitter/mod.rs`
- Modify: `src/gateway/reply_emitter/emitter/streaming.rs`
- Modify: `src/gateway/reply_emitter/emitter/helpers.rs`

**Interfaces:**
- Consumes: `MessageAssembler::{push_text_delta, push_reasoning_delta, snapshot, finalize}`, `AssembledMessage`.
- Produces: unchanged `ReplyEmitter` public API.

- [ ] **Step 1: Read the full buffer weave first**

Run:
```bash
grep -n "self.buffer\|reasoning_buffer\|take_reasoning_buffer\|split_reasoning\|sanitize_llm_output\|send_to_channel" src/gateway/reply_emitter/emitter/streaming.rs src/gateway/reply_emitter/emitter/helpers.rs
```
Read every hit. Classify each site as **message content** (delta append, reasoning append, finalize) vs **presentation** (notice append `streaming.rs:365,375`, footer append `streaming.rs:401`, overflow split, native `stream_finalize`, voice/TTS). Presentation sites operate on `finalize().answer`, not the assembler.

- [ ] **Step 2: Write the failing regression test**

Add to `src/gateway/reply_emitter/tests.rs` a test asserting an inline-`<think>` response delivered to a channel arrives clean AND the fallback notice still appends after the answer (proving the message-vs-presentation boundary holds):

```rust
#[tokio::test]
async fn channel_reply_strips_inline_think_and_keeps_notice_ordering() {
    // Drive a ReplyEmitter with ResponseChunk deltas containing an inline
    // <think> block, a ModelResolved{is_fallback:true}, then RunComplete.
    // Assert the delivered text = clean answer + appended fallback notice,
    // in that order, with no <think>.
}
```

- [ ] **Step 3: Swap the fields**

In `src/gateway/reply_emitter/emitter/mod.rs`, replace:
```rust
    pub(crate) buffer: Mutex<String>,
```
and
```rust
    pub(crate) reasoning_buffer: Mutex<String>,
```
with a single:
```rust
    /// The single assembled-message reducer for this run (replaces the
    /// separate `buffer` + `reasoning_buffer`). Presentation decorations
    /// (fallback notice, runtime footer, overflow split) live OUTSIDE it.
    pub(crate) assembler: Mutex<crate::gateway::message_assembly::MessageAssembler>,
```
Update both constructors (`new`, `with_config`) to initialise `assembler: Mutex::new(MessageAssembler::new())` and drop the `buffer` / `reasoning_buffer` initialisers.

- [ ] **Step 4: Route accumulation + finalize through the assembler**

For each classified site from Step 1:
- `self.buffer.lock().await.push_str(&content)` (streaming.rs:106) → `self.assembler.lock().await.push_text_delta(&content);`
- reads of the buffered text for a mid-stream send (streaming.rs:239-240) → `self.assembler.lock().await.snapshot().to_string()` then existing sanitize (or drop the redundant sanitize since the snapshot is already clean).
- `*self.buffer.lock().await = text;` (streaming.rs:352, native path) — the native handler overwrites accumulated text; keep an assembler method `overwrite_visible(&mut self, text: String)` (add to Task 2 assembler) OR retain a small presentation-local `String` for the native path if it is not "the message" but a re-render. Prefer `overwrite_visible`.
- `take_reasoning_buffer` (helpers.rs:152) → return `self.assembler.lock().await.finalize().reasoning` at finalize, or expose `assembler.reasoning_snapshot()` for mid-run reads.
- RunComplete finalize (streaming.rs:306+): replace the `sanitize_llm_output(&raw)` + `split_reasoning` + `send_to_channel_with_reasoning` dance with:
```rust
    let AssembledMessage { answer, reasoning } = {
        let mut a = self.assembler.lock().await;
        a.finalize()
    };
    if let Some(answer) = answer {
        // presentation layer: append fallback notice + runtime footer HERE.
        self.send_to_channel_with_reasoning(&answer, reasoning.as_deref()).await;
    }
```
- `send_to_channel_with_reasoning` (helpers.rs:254): its internal `split_reasoning(content)` is now redundant (assembler already separated). Simplify to take the already-clean `answer` + `reasoning` and skip the re-split/re-sanitize.

- [ ] **Step 5: Run the reply_emitter suite**

Run: `cargo test -p alephcore reply_emitter`
Expected: PASS — new boundary test + all existing `reply_emitter/tests.rs` tests green. Fix regressions by preserving presentation ordering (notice/footer after `answer`), not by weakening the assembler.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/reply_emitter/
git commit -m "gateway: ReplyEmitter accumulates via MessageAssembler (retire dual buffers)"
```

---

## Task 8: Final verification & entropy sweep

**Files:** none (verification) + any dead-code deletions surfaced.

- [ ] **Step 1: Confirm no orphaned accumulation logic remains**

Run:
```bash
grep -rn "accumulated\|reasoning_buffer\|\.content\b" src/gateway --include='*.rs' | grep -iv "test\|// " | grep -i "chunk\|buffer\|response"
grep -rn "split_reasoning" src/gateway --include='*.rs'
```
Expected: no live `ResponseChunk.content` usage; `split_reasoning` referenced only where still genuinely needed (or removed if fully folded). Delete any now-dead `split_reasoning` / helper left with zero callers (熵减 ledger).

- [ ] **Step 2: Single compile gate**

Run: `cargo check -p alephcore --lib`
Expected: clean, zero warnings on touched files.

- [ ] **Step 3: Targeted test sweep**

Run: `cargo test -p alephcore message_assembly event_drain event_emitter reply_emitter streaming_scrubber`
Expected: all PASS.

- [ ] **Step 4: Format**

Run: `cargo fmt -p alephcore`
Expected: no diff on already-formatted code.

- [ ] **Step 5: Final commit (if fmt/dead-code changed anything)**

```bash
git add -A src/
git commit -m "gateway: entropy sweep — remove dead accumulation logic post-assembler"
```

---

## Self-review notes (author)

- **Spec coverage:** G1 → Task 5; G2 → Task 4; G3 (primitive + adoption) → Tasks 2/3/7; G4 → Tasks 1/3. Non-goals (G5, instant coalescing, StreamingController) untouched by design. ✔
- **Wire-compat gate:** Task 5 Step 1 explicitly forks on whether the panel/replay reads the wire `content` — no silent break. ✔
- **Message-vs-presentation invariant:** enforced in Task 7 (notices/footer outside the assembler), tested in Task 7 Step 2. ✔
- **Assembler API additions surfaced during wiring** (`reset_iteration`, `overwrite_visible`, `reasoning_snapshot`) are called out at their point of need (Tasks 3/7) — add them to Task 2's `MessageAssembler` when the consuming task requires them, with a unit test each.
