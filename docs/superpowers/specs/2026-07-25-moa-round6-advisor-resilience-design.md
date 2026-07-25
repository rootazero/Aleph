# MoA Round-6 — Advisor-Side Resilience (§4.9)

Date: 2026-07-25
Worktree: `moa-round6-advisor-resilience`
Scope: `src/providers/moa/`, `src/providers/health.rs` (consumer only)

## 1. Why a round 6

Rounds 1–5 hardened the **activation / configuration / observability** axes of MoA
(arm-site authorization, scratch-config validation, one-shot restore CAS, prompt-cache
breakpoints, spend attribution, trace presentation). Round 5's 15-agent audit concluded
"zero dead code, already surpasses hermes" — for those axes it is correct.

This round attacks the axis none of them scanned: **what happens to the advisor fan-out
when an advisor misbehaves, or when the conversation gets long.** Every finding below is
reachable from a normal production configuration; none of them is caught by an existing
test.

Reference baseline: `docs/superpowers/specs/assets/2026-07-05-moa/moa-understand-hermes.md`
(line-verified distillation of hermes-agent's `agent/moa_loop.py` + `moa_config.py`;
the local hermes checkout at `/Volumes/TBU/Github/hermes-agent` is a 2026-06-04 snapshot
that predates the MoA feature — `git log --all -- '*moa*'` is empty there).

## 2. Findings

### G1 — Dead-advisor timeout amplification (HIGH, latency)

`fan_out::run_fan_out` unconditionally consults every advisor on every cache MISS, and
`MoaProvider` keeps no memory of prior outcomes. Under the default
`MoaFanout::PerIteration`, a hung or mis-keyed advisor burns the full
`advisor_timeout_secs` (default **120 s**) on **every tool iteration** before degrading to
its `[timeout after Ns]` note. A 20-step agentic run pays up to 40 minutes of pure dead
wall-clock, and each of those waits blocks the aggregator.

This does **not** contradict round-1's "no K-of-N racing" decision. That decision governs
the completion semantics of a *single* consultation (wait for all, per-advisor timeout
budget). G1 is about whether an advisor already proven dead *within this run* should be
re-consulted at all. Orthogonal.

### G2 — Advisors are blind to the acting agent's tool inventory (MEDIUM-HIGH, quality)

`ADVISOR_SYSTEM_PROMPT` asks each advisor for "concrete next steps and **tool-use
strategy**", but the advisory view only reveals a tool once it has already been called
(`[called tool: X]`). Tools never yet used are invisible. Advisors therefore invent tool
names or retreat to generic advice.

`MoaProvider::process` already owns the full `Vec<ToolDefinition>`
(`payload.tools.map(<[_]>::to_vec)`) and hands it only to the aggregator. This is a pure
wiring gap: the data is in hand and discarded. hermes has the same blindness, so closing
it is a surpass item.

### G3 — Advisory view has no total budget (MEDIUM, resilience)

`advisory_view` caps each tool result at `TOOL_RESULT_BUDGET` (4000 chars) but the view
itself grows without bound. The failure that matters is not cost (modern providers
auto-cache prefixes, and `mark_cache_breakpoints` already handles Anthropic) — it is
**overflowing the advisor's context window**. An advisor on a small-context model paired
with a large-context aggregator dies with a hard 4xx on every later iteration of a long
run: MoA silently stops working in exactly the scenario it exists for.

### G4 — Empty user turn escapes the advisory view (LOW-MEDIUM, robustness)

`build_advisory_view`'s `User` arm pushes unconditionally:

```rust
rendered.push(UnifiedMessage::user(text_of(content)));
```

while the `Assistant` arm guards with `if !parts.is_empty()`. When `text_of` yields `""`
(content vec empty, or a lone empty text block) the view carries an empty text block.
`anthropic/proto_impl.rs`'s `blocks.is_empty()` fallback only catches a wholly empty
content vec, not an empty *string* inside a block, so the request reaches the wire as
`MessageContent::Text { content: "" }` → HTTP 400 → **all advisors fail simultaneously**.
The same arm can also produce a fully empty view (`build_advisory_view(&[])` → `[]`),
which every provider rejects.

## 3. Design

### 3.1 G1 — run-scoped per-advisor circuit breaker

**Reuse, do not build.** `src/providers/health.rs::ProviderHealth` is a live,
tested circuit-breaker state machine (`Healthy` / `Degraded { cooldown_until,
consecutive_failures }` / `Unavailable { reason }`, exponential backoff 30 s → 300 s,
`is_usable()`), today consumed by `src/thinker/mod.rs`'s router. It also ships
`impl From<&AlephError> for Option<ProviderError>` — the exact transient/permanent
classifier MoA needs — with no consumer for that conversion path yet.

New module `src/providers/moa/advisor_health.rs` is a thin **policy** layer over it:

- One `ProviderHealth` per advisor slot, `Vec`-indexed, held in the run-scoped
  `MoaProvider` behind the existing sequential-access invariant.
- Before a fan-out, `usable()` yields the skip mask.
- After a fan-out, success → `record_success()`; timeout → `record_failure(Transient(Timeout))`;
  error → `From<&AlephError>` classification, defaulting to `Transient(ConnectionFailed)`
  when the error is not provider-level.
- **Run-level trip**: once a slot reaches `TRIP_AFTER_CONSECUTIVE_FAILURES` (3)
  consecutive failures, it is moved to `Unavailable` — the enum's own terminal state, so
  no new state machine — and is not probed again for the rest of the run. Cooldown alone
  would still re-pay the 120 s timeout roughly every 300 s.
- A new run builds a fresh `MoaProvider`, so health resets per run (self-healing).

**Slot preservation (user decision).** A skipped advisor keeps its slot and index. It
produces `AdvisorOutcome { label, text: "[skipped: <reason>]" }`, structurally identical
to the existing `[failed: …]` / `[timeout after Ns]` notes, so the aggregator can tell
"one advisor configured" apart from "three configured, two down", and advisor numbering
never shifts.

**Index-alignment invariant.** `MoaProvider::spend_event` indexes `self.advisors[idx]`
from `results.iter().enumerate()`. `run_fan_out` must therefore keep returning one
`AdvisorResult` per advisor slot in slot order — skipped slots yield a synthetic result,
never a filtered-out entry.

**Counting semantics.** `emit_fanout_events`'s `count` (the `i/n` display and
`MoaAggregating.advisor_count`) stays the total slot count. `spend_event`'s
`advisor_count` is documented as *consulted*, so it drops skipped slots.

### 3.2 G2 — advisor tool roster

`prompts::advisor_system_prompt(tools) -> Cow<'static, str>` returns the existing const
verbatim when there are no tools, otherwise appends a compact roster section.

Budget discipline (this text ships to every advisor on every fan-out):
- `name — first sentence of description`, each line capped at
  `ROSTER_LINE_BUDGET` (100 chars) with UTF-8-safe truncation;
- whole roster capped at `ROSTER_TOTAL_BUDGET` (1800 chars) with an honest
  `… (+N more tools)` tail;
- payload order preserved (already deterministic) so the system prefix stays
  byte-stable across iterations and prefix caches keep hitting.

Framing matters: the roster is labelled as *the acting agent's* tools, and the existing
"you cannot call tools" sentence stays, so the advisor reasons about strategy rather than
attempting calls.

### 3.3 G3 — advisory-view total budget

`advisory_view::apply_view_budget(&mut Vec<UnifiedMessage>)`, applied after the view is
built and **before** `view_signature` (so the cache key describes what is actually sent).

Mechanism: **shrink oldest messages, never drop them.** Walking newest → oldest, each
message keeps its full text while the running total is under budget; past that point older
messages are re-truncated head+tail through the existing `truncate_tool_result` at a small
per-message allowance, and the very oldest get a one-line stub. Message count, ordering and
role sequence are untouched — no risk of violating "first message must be `user`" or the
alternation rules, which a drop-based elision would have.

`ADVISORY_VIEW_BUDGET` is a module constant (mirroring `TOOL_RESULT_BUDGET`), not new
config surface: it is a guardrail against a hard failure, not a tuning knob.

### 3.4 G4 — empty-turn guard

- `User` arm: skip when `text_of(content)` is blank, mirroring the `Assistant` arm.
- Terminal guarantee: if the view ends up empty, emit the synthetic
  `ADVISORY_INSTRUCTION` user turn so advisors always receive at least one non-empty
  message.

## 4. Non-goals (settled in earlier rounds — do not re-litigate)

- K-of-N advisor racing (round-1 §2).
- Advisor streaming (round-5 D1: would require touching `src/harness/`, violating the R10
  line ratchet, for no user-visible gain).
- hermes' global persistent `active_preset` (round-1: replaced by session-scoped state).
- A new RPC for "which preset is armed on this session" (new surface, low value).

## 5. Test plan

| Test | Locks |
|---|---|
| `advisor_tripped_after_consecutive_failures_is_skipped` | G1: an always-failing advisor stops being called after the trip threshold |
| `tripped_advisor_keeps_its_slot_in_guidance` | G1: skipped slot present with `[skipped: …]`, index stable |
| `recovered_advisor_resets_health` | G1: success clears `Degraded` |
| `skipped_advisors_excluded_from_spend_count` | G1: `advisor_count` = consulted, display `count` = total |
| `roster_lists_tools_and_respects_budget` | G2: names present, total capped, `+N more` tail |
| `advisor_prompt_unchanged_without_tools` | G2: zero-tool path is byte-identical to today |
| `view_budget_shrinks_oldest_and_preserves_roles` | G3: message count/roles unchanged, total under budget |
| `view_budget_is_noop_under_budget` | G3: short conversations untouched |
| `empty_user_turn_is_dropped` | G4: no empty text block reaches the view |
| `all_empty_input_yields_instruction_turn` | G4: view never empty |

Plus the existing 34 MoA unit tests must stay green (`cargo test -p alephcore --lib moa`).

## 6. Entropy

- No new config fields, no new RPCs, no new trace events.
- `providers::health`'s `From<&AlephError> for Option<ProviderError>` gains its first
  consumer (it existed with tests but no caller).
- `run_fan_out`'s signature gains the skip mask; `emit_fanout_events`'s `count` semantics
  are documented against `spend_event`'s so the two never silently drift.
