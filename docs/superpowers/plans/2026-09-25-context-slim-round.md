# Context-slim round — reasoning replay policy + tool-output ingress slimming (2026-09-25)

> Controller plan. Scan inputs (read-only, scratchpad of the controlling session):
> `scan-ref.md` (context-mode / pi-ctx), `scan-aleph-tooloutput.md` (G1–G12, E1–E7),
> `scan-aleph-reasoning.md` (R-G1–R-G15, probes P1–P5), `api-facts.md` (provider API facts, authoritative).
> Gap ids below refer to those files: **T-** = tool-output scan, **R-** = reasoning scan.

## 0. User decisions (2026-09-25, binding)

| # | Decision |
|---|---|
| U1 | **No live-provider probes.** Implement per official docs (`api-facts.md`); any vendor whose rule is uncertain keeps today's behaviour (conservative default = status quo, never "strip"). Real-machine QA uses mock providers. |
| U2 | Ingress reduction threshold: **4000 tokens per result, 32k per Think→Act turn** (was 8000 / 50k) — only after the persisted-original handle is fixed (B1). |
| U3 | Anthropic server-side context editing (`clear_tool_uses_20250919` / `clear_thinking_20251015`): **config option, default OFF**. When ON for a provider, local tool-result pruning yields on that provider (one problem, one answer). |
| U4 | Transient tail (reminders / recall / MoA advice deleted every turn): **no harness redesign this round**; stop the 400s with `drop_block` on binding Claude models; record the redesign as DEFER in FEATURE_LOCATOR. |

## 1. Pushback recorded (why "strip reasoning" is not the design)

pi-ctx style "keep last N reasoning messages" is **wrong for current providers**: DeepSeek thinking+tools
400s unless ALL prior `reasoning_content` is replayed; binding Claude models (Opus 5.5 / Fable 5.1) 400 on
any mid-history edit; moving the boundary every step re-bills the prompt cache every step. The real bloat
is elsewhere (Anthropic adapter copying thinking into every `tool_use.input`; unchanged tool output
≤ 8000 tokens re-sent every turn; double JSON encoding). The design is a **per-target reasoning replay
policy applied once, after failover picks the target**, plus **ingress-time** tool-output reduction
(never rewrite history afterwards).

## 2. Lines, ownership, merge protocol

- **Line A** — worktree `.claude/worktrees/context-slim`, branch `context-slim-round` (= integration branch).
  Owns: `src/providers/**`, `src/harness/**`, `src/session/events.rs`, `src/context/compact/**`,
  `src/providers/model_catalog/**`, `docs/reference/AGENT_SYSTEM.md`.
- **Line B** — worktree `.claude/worktrees/context-slim-b`, branch `context-slim-b`.
  Owns: `src/tool_output/**`, `src/tools/**`, `src/builtin_tools/**`, `src/context/retrieval/**`,
  `src/context/budget/**` (incl. `cheap_passes/`), `src/orchestrator/harness_bridge/context_estimate.rs`,
  `shared/protocol/**`, `src/gateway/**`, `interfaces/**`, `src/config/types/tools.rs`.
- Anything outside both lists: ask the controller first. **Line B never edits `src/harness/`**; harness
  follow-ups found by B are reported to the controller and folded into a Line A task.
- **Merge points** (controller performs, only when Line A is idle at a commit boundary):
  B batch committed → `git merge context-slim-b` into `context-slim-round` → Line A's next compile is the
  merged compile. Before starting a batch that needs A's work, Line B runs `git merge context-slim-round`.
- `FEATURE_LOCATOR.md` and other reference docs are edited **only in the final docs task** (avoid conflicts);
  each task reports its doc-worthy facts in its report file instead.

## 3. Tasks

### Line A
- **A1 · stop the bleeding (no probes needed)**
  - R-G1: gate the `reasoning_content` injection into `tool_use.input` (`anthropic/proto_impl.rs:149-235`)
    by host policy: first-party Anthropic / Bedrock / Vertex → never inject; other Anthropic-protocol hosts
    (Kimi/Moonshot, MiniMax, Kimi Coding, unknown base_url) → status quo. Reuse the existing host
    classification in `anthropic/provider_policy.rs` (no second classifier).
  - R-G3 (minimal): binding Claude models on the first-party API send
    `thinking.block_binding.prefix_mismatch_behavior: "drop_block"` + beta `thinking-binding-controls-2026-08-01`.
    Which models bind must be **derived from the model catalog** (a capability flag), not name-matched in the adapter.
  - R-G11 CUT `UnifiedMessage::from_provider_response`; R-G12 CUT the rotten `AGENT_SYSTEM.md` sections.
- **A2 · facts down, policy at the wire** (R-G6, R-G7, R-G5)
  - `harness/agent/prompt.rs` stops deciding: it emits every persisted thinking block with facts
    (current-user-turn or not, via existing `turn_id`; origin protocol when known). Harness line count must
    go **down** (`src/harness/tests/budget.rs::CEILING` only decreases; answer the 3 questions).
  - Revive `transform_messages(msgs, target)` (`providers/message.rs`) as the **single** pre-send choke point
    inside `HttpProvider` (after failover fan-out); delete the stale "reserved" comment/duplication.
  - New `ReasoningReplay` capability dimension, one table, next to existing capabilities; unknown targets
    default to today's behaviour for their protocol.
  - Persist signature/reasoning origin (serde-compatible with old logs); legacy entries with unknown origin
    keep today's behaviour.
- **A3 · connect the policies** (R-G2, R-G8, Anthropic <4.5)
  - OpenAI-compat: add `reasoning_content` to the wire message; DeepSeek native with tools → replay ALL
    prior turns' reasoning; Kimi-openai K2 thinking → replay (verify vendor doc first; if unverifiable keep
    status quo); GLM → status quo unless documented.
  - Inline `<think>…</think>` in historical assistant text (local Qwen3 / R1 distills) stripped for turns
    before the current user turn; MiniMax-M2 exempt. Machine-format text ⇒ regex allowed (P8).
  - Anthropic models where the API strips prior-turn thinking itself (pre-4.5, Haiku ≤ 4.5): drop signed
    thinking from turns before the current user turn (payload only; boundary moves once per user turn).
  - Foreign signatures never sent to a different protocol family.
- **A4 · summarizer input** (R-G10): all three compaction paths exclude thinking from summarizer input;
  fix `text_content()` doc. Note the one-time `hash_window` fingerprint invalidation.
- **A5 · one text accessor for tool results** (T-G1 + T-G3a): `ContentBlock` gets a single
  `as_model_text()` (String unwrapped, other JSON compact); every provider arm + `message.rs` readers +
  `moa/advisory_view.rs` use it. Gemini keeps object passthrough, wraps strings without re-encoding.
  One-time prefix-cache invalidation on upgrade is accepted (document it).
- **A6 · Anthropic context editing option (U3)**: config (default off) → `context_management.edits` +
  beta `context-management-2025-06-27`, first-party host + supporting models only; when enabled, local
  tool-result pruning yields for that provider. Scheduled **after B5 is merged** (touches preflight wiring).

### Line B
- **B1 · the handle works** (T-G2, T-G5, T-G12): persisted/indexed originals rendered line-preserving
  (typed envelopes expanded to text fields with real newlines), so `ctx_search` gets > 1 section;
  `ctx_search` can return a section body (token-bounded) and accepts several queries in one call;
  the marker only names tools the model can actually call.
- **B2 · one budget source** (T-G4 CONNECT, T-G9, T-G6, E7, non-harness T-G10 comment drift):
  tools' `max_result_tokens()` declarations become the only source; delete the name table (migrate its
  values into declarations); fix `clean_error_body` units; Layer-3 spill only evicts the just-recorded
  item and never books savings it did not make; resolve `[tools.search]` (CUT or CONNECT).
- **B3 · measurement** (T-G7, T-G8): `tokens_in_context` gets a real consumer; `context.breakdown` reports
  history tokens split by kind (tool results / reasoning / other) from the **one** existing estimator;
  wire keys live in `shared/protocol` and the handler constructs the response from them; every added
  field must be rendered by at least one client (TUI and/or Panel) or it is not added.
- **B4 · lower the gate + fetch by intent** (U2, E6): default 4000 tokens/result, 32k/turn; per-tool
  overrides only where the output *is* the artifact the model explicitly windowed (e.g. `file_read`),
  each justified in the report; `web_fetch` with `prompt` on a large page indexes it and returns the
  BM25-matched sections + handle (pure retrieval, R7-clean).
- **B5 · pruning on the production shape** (after A5 merged; T-G3, T-G11): cheap-pass tests use the
  `build_prompt` shape; `file_op_supersede` marker guard → `extract_persisted_ref`.
- **B6 · transport payload** (R-G14, R-G15): phone client stops receiving reasoning frames it drops;
  CUT the zero-producer `StreamEvent::ReasoningBlock`.

### Final (controller-dispatched)
- **C1 · real-machine QA** `qa/context_slim/run.sh`: mock DeepSeek that 400s without replayed
  `reasoning_content` when tools are present; wire capture proving no `reasoning_content` copies for a
  first-party-classified Anthropic target (or the closest reachable proof); large bash output → marker →
  `ctx_search` body retrieval. Register in `qa/README.md`.
- **C2 · verification set** (six commands in CLAUDE.md) on the integration branch.
- **C3 · docs**: FEATURE_LOCATOR new section + appendix E/D entries; TOOL_SYSTEM / MODEL_CATALOG /
  AGENT_SYSTEM updates; DEFER list (U4 transient-tail redesign, R-G4 redacted_thinking, R-G9 Responses
  historical items, R-G13 `display:"omitted"`, Anthropic text-only-turn thinking replay).

## 4. Execution protocol for every implementer

1. `cd` into your worktree; never touch the other worktree or the main checkout.
2. Cargo only via `bash <scratchpad>/cargo-bg.sh <log> '<cmd>'` then `bash <scratchpad>/wait-log.sh <log>`
   repeated **in the same turn** until `DONE` (never end your turn while a job runs; never use Monitor;
   never `run_in_background` then stop). The helper waits for ≥ 4 GiB available memory and holds the
   machine-global cargo lock.
3. Compile once per task batch; minimum per batch: `cargo test -p alephcore --lib <affected modules>`
   (and `--no-run` for the whole lib when public items changed).
4. TDD where behaviour changes: failing test first, then code; guards must be shown red by a mutation.
5. Report file (append incrementally): `<scratchpad>/report-<task>.md` — what changed, tests run with
   results, what was NOT done, doc-worthy facts for C3.
6. Commit at the end of each task: English `<scope>: <description>` + the Co-Authored-By trailer.
