# Stage J Reassessment Kickoff Prompt

> **Intended for use:** 2026-05-23 (or later). Authored 2026-05-09 immediately after Stage J-pre ship.
>
> **How to use:** open a new Claude Code session at `/Volumes/TBU4/Workspace/Aleph` (Aleph project root) and paste **this entire file** as your first message. The session's Claude executes the steps below autonomously and ends in either:
> - Path A: a Stage J implementation plan + SDD execution kicked off; OR
> - Path B: roadmap updated to mark Stage J ❌ Indefinitely Deferred + brief retro committed.

---

## Read this first (no skipping)

You are Claude Code resuming work on **Aleph**, a self-hosted personal AI assistant (Rust core + multi-channel architecture). Project root: `/Volumes/TBU4/Workspace/Aleph`. Branch: `main` (single-branch model — commit directly).

**Your mission this session:** decide whether to implement **Stage J** (fork-subagent prompt cache reuse) of the subagent uplift roadmap, based on the cache-token observability data that **Stage J-pre** has been silently collecting since 2026-05-09. Then either ship Stage J via SDD, or close it out with a retro.

**Hard project rules (architectural redlines)** — never violate:
- **R10 thin-harness redline**: `git diff f891cc71b -- src/harness/agent.rs | wc -l` MUST be `0`. Harness file count `ls src/harness/*.rs | wc -l` MUST be `10`. (`f891cc71b` is the Stage I closure SHA — Stage J-pre preserved it through 9 commits, so will you.)
- All other redlines (R1–R9) per `/Volumes/TBU4/Workspace/Aleph/CLAUDE.md`. Worth re-reading.

**Project conventions:**
- 中文 conversation, English code/comments, English commit messages
- `<scope>: <description>` commit format, no Claude attribution
- Atomic per-task commits

**Skill availability:** assume `superpowers:brainstorming`, `superpowers:writing-plans`, `superpowers:subagent-driven-development` are available (Aleph uses them throughout the subagent-uplift series).

---

## Background — what's already shipped

The subagent-uplift roadmap's master spec is at:
```
/Volumes/TBU4/Workspace/Aleph/docs/superpowers/specs/2026-05-08-subagent-uplift-roadmap-design.md
```

By 2026-05-09 the following had shipped on `main`:

| Phase | Stage | Closure SHA | What |
|---|---|---|---|
| P1 | A–D | (multiple) | HarnessDeps fields + recursion guard + LaneScheduler + cancellation tests |
| P2 | E–G | (multiple) | File-system agents + skill-ops integration + agent-tool spawn |
| P3 | H | `64f322a03` | Worktree isolation primitive |
| P3 | I | `f891cc71b` | Per-agent MCP scope |
| P3 | J-pre | `fd673742c` | **Cache observability pipeline** — see below |

**Stage J-pre commit chain** (`8f06fe341..fd673742c`, 10 commits including plan baseline) shipped at 2026-05-09:

| SHA | Description |
|---|---|
| `8f06fe341` | docs: Stage J-pre implementation plan |
| `21061a9f2` | providers: add `cache_creation_tokens` to `TokenUsage` |
| `d430949cc` | providers/anthropic: parse `cache_creation_input_tokens` (non-streaming) |
| `158e068d5` | providers/anthropic: stream protocol extracts `message_start` usage + `cache_creation` |
| `949db92c4` | harness/trace + protocol: add `ProviderUsage` schema variant |
| `ac8c9042f` | providers: `MeteringProvider` decorator emits `ProviderUsage` trace |
| `9fef0b178` | agents/spawner: wrap subagent provider with `MeteringProvider` |
| `cfd2c3954` | aleph-server/orchestrator: wrap root provider with `MeteringProvider` |
| `c56c5d014` | tests: cache_observability_smoke (3/3 pass) |
| `fd673742c` | docs: Stage J-pre shipped |

**What Stage J-pre actually does:** every LLM call from either the root harness or a spawned subagent flows through `src/providers/metering.rs::MeteringProvider`, which after each `process()` call emits a `LoopTraceEvent::ProviderUsage { agent_id, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, thinking_tokens }` event into the harness's `TraceSink`. The label `agent_id` is `"root"` for the top-level harness or the subagent's `agent_def.id` when emitted from within a spawned subagent.

**What Stage J-pre does NOT do** (deliberately deferred until your decision today):
- Implement the actual fork-prompt branch in `subagent_spawner` (the `inherit_parent_prompt: bool` AgentDef flag)
- Surface a cost dashboard
- Persist `ProviderUsage` events to a queryable store (this is a known gap — see Step 0)

---

## Step 0 — Data availability check (CRITICAL FIRST STEP)

The whole reassessment hinges on having ≥2 weeks of `ProviderUsage` event data. **First confirm the data actually exists somewhere queryable.** The TraceSink is mpsc-backed at the gateway path; if no consumer subscribed and persisted events, the data may have evaporated.

Run these in parallel:

```bash
# 1. Check whether any sink writes ProviderUsage events to logs / files
grep -rn "ProviderUsage\|provider_usage" /Users/zouguojun/.aleph/ 2>/dev/null | head -20

# 2. Check Aleph's own data directory for any trace persistence
ls -la /Users/zouguojun/.aleph/data/ 2>/dev/null
find /Users/zouguojun/.aleph/data/ -name "*trace*" -o -name "*.log" -o -name "*.jsonl" 2>/dev/null | head -20

# 3. Check if there's a SQLite trace table
find /Users/zouguojun/.aleph/data/ -name "*.db" -o -name "*.sqlite*" 2>/dev/null | head -5

# 4. Check if any test or dev run captured them
ls -la /Volumes/TBU4/Workspace/Aleph/.omc/state/ 2>/dev/null | head -10
find /Volumes/TBU4/Workspace/Aleph -name "*provider_usage*" -not -path "*/target/*" -not -path "*/.git/*" 2>/dev/null | head -20

# 5. Confirm the ship code is still live (no regressions since 2026-05-09)
git log --oneline fd673742c..HEAD | head -20
git diff f891cc71b -- src/harness/agent.rs | wc -l   # must still be 0
```

### Decision branch on Step 0

| Outcome | Action |
|---|---|
| **Data is queryable** (logs/SQLite/JSONL with ProviderUsage events) | Proceed to **Step 1** |
| **No data found** (sink never persisted, or user didn't run server during the window) | Skip to **Path B (NO-GO with caveat)** — document the data gap as the reason |
| **Code regressed** (R10 violation, or J-pre was reverted) | **STOP** — report to user, do not proceed |

If the data exists but in an unexpected location, ask the user once with the actual paths you found. Don't guess.

---

## Step 1 — Extract and aggregate ProviderUsage events

**If Step 0 shows data is queryable**, write a one-shot script (Bash + jq, or a tiny Rust binary at `tools/cache-stats/`) that produces this CSV/table:

```
agent_id            | calls | sum_input | sum_output | sum_cache_read | sum_cache_create | hit_ratio
--------------------|-------|-----------|------------|----------------|------------------|----------
root                | 1234  | 5,000,000 |  120,000   |   3,800,000    |     180,000      |   0.76
subagent-research   |   45  |   200,000 |    8,000   |      80,000    |       4,000      |   0.40
subagent-pr-review  |   23  |   100,000 |    4,000   |      35,000    |       2,000      |   0.35
... (all distinct agent_ids)
```

`hit_ratio = sum_cache_read / (sum_input + sum_cache_read)` (the fraction of input tokens served from cache).

Save the raw aggregated table at `docs/superpowers/runs/2026-05-23-stage-j-cache-stats.md` with:
- the time window (first event timestamp → last event timestamp)
- the source location (file path / SQLite query you used)
- the per-agent table above
- count of subagent_ids with N ≥ 5 calls (the ones with statistically meaningful data)

**Total session count target:** ideally ≥ 50 subagent spawns across the window. If fewer, the data is too thin — flag this in the report and either widen the window or proceed with a "high-uncertainty" GO/NO-GO. Don't fabricate.

---

## Step 2 — Compute decision metrics

From the aggregated table compute:

1. **`subagent_hit_ratio_avg`** — mean `hit_ratio` across all subagent_ids with N ≥ 5 calls
2. **`subagent_input_token_share`** — `sum(subagent.input_tokens) / sum(all.input_tokens)` — how much of total token cost subagents represent
3. **`subagent_call_volume`** — total subagent calls in the window
4. **`projected_fork_savings`** — `subagent_input_token_share * (1 - subagent_hit_ratio_avg) * 0.9`
   - Logic: fork branch claims to push `subagent_hit_ratio_avg` toward `0.9+` (Anthropic cache hit on prefix). If subagents already cache-hit at 0.7+, the marginal savings are small.

---

## Step 3 — Apply the decision tree

```
                    projected_fork_savings (% of total token cost)
                    ┌────────────────────────────────────────┐
                    │   <5%       5-15%        >15%          │
                    │                                        │
subagent_call_      │  NO-GO     UNCERTAIN     GO            │
volume ≥ 50         │            (caveat)                    │
                    │                                        │
                    │  NO-GO     NO-GO         UNCERTAIN     │
volume <50          │           (insufficient (proceed if    │
                    │             data)        savings model │
                    │                          robust)       │
                    └────────────────────────────────────────┘
```

**Tie-breaker for UNCERTAIN cells:** ask the user once with the metrics in hand. Don't autoplay this; it's their cost call.

**Hard "always NO-GO" gates** (override the table above):
- claude-code upstream changed fork API in a breaking way in the last 2 weeks (check: `gh search code --repo anthropics/claude-code "forkSubagent"` recent commits)
- Anthropic prompt cache pricing/behavior changed (check Anthropic API docs / changelog)
- A regression in J-pre data shows `cache_creation_tokens` always 0 (would mean no cache writes happening — then there's nothing to share via fork either)

---

## Path A — GO (proceed to Stage J implementation)

If the decision tree outputs GO:

1. **Brainstorm sub-step (skipped — design already exists)**
   The Stage J solution sketch + acceptance criteria are locked in master spec § 1.2 Stage J (lines ~594-642 of `docs/superpowers/specs/2026-05-08-subagent-uplift-roadmap-design.md`). Do NOT re-brainstorm — this would duplicate work and risk drift. Proceed directly to writing-plans.

2. **Write plan** using `superpowers:writing-plans` skill. Input:
   - master spec § 1.2 Stage J (locked solution sketch)
   - Step 1 metrics report (justifies the cost-savings model and the threshold for the cost-regression test)
   - The existing P3 design doc § 0 Q3 decision frame
   Output: `docs/superpowers/plans/2026-05-23-subagent-uplift-stage-j-plan.md`. Honor master spec § 1.2 "Allowed seams":
   - `AgentDef::inherit_parent_prompt: bool` (default `false`, opt-in only)
   - subagent_spawner internal fork branch ≤ 150 lines
   - Placeholder tool result generator ≤ 50 lines
   - Don't modify `PromptBuilder`
   - ≥ 1 real consumer (write a fork agent definition)

3. **Execute via** `superpowers:subagent-driven-development` (autonomous, fresh implementer per task, atomic commits, R10 verifier per task).

4. **Stage J acceptance criteria** (from master spec, must verify before closure):
   - Functional: fork agent first-round prompt prefix byte-equal to parent's
   - Trace shows `cache_read_input_tokens > 0` on first turn (real-LLM verification)
   - ≥ 2 unit tests + ≥ 1 integration + ≥ 1 cost regression
   - Cost regression: N concurrent fork subagents total cost < N × non_fork_cost × 0.5
   - Build/lint/test green
   - R10: `agent.rs` 0 diff vs `f891cc71b`, harness file count 10

5. **Closure**: update master roadmap top-of-file with `✅ P3 Stage J Shipped: <hash> on <date>` line; update Stage J section `**Status**` line.

---

## Path B — NO-GO (close out Stage J)

If the decision tree outputs NO-GO (or DATA-GAP causes mandatory NO-GO):

1. **Write a brief retro** at `docs/superpowers/runs/2026-05-23-stage-j-no-go-retro.md` (~50 lines) covering:
   - Window analyzed (start → end)
   - Sample size (subagent calls)
   - Key metrics from Step 2
   - The decision tree cell that triggered NO-GO
   - Specific number: "fork-branch projected savings = X% of total token cost; threshold for GO was 15%"
   - Whether the data was sufficient or thin
   - Conditions under which Stage J should be re-opened (e.g., "if subagent call volume grows 10x, re-run this analysis")

2. **Update the roadmap** `docs/superpowers/specs/2026-05-08-subagent-uplift-roadmap-design.md`:
   - Top-of-file: add `❌ P3 Stage J Indefinitely Deferred: <date> based on 2-week trace data — see docs/superpowers/runs/2026-05-23-stage-j-no-go-retro.md`
   - Stage J entry `**Status**` line: append `· Indefinitely deferred per 2-week reassessment 2026-05-23`
   - § 5 Out-of-scope table: keep Stage J entry, update "何时重审" column to point at the retro

3. **DO NOT remove `MeteringProvider` or the `ProviderUsage` trace event.** They are independently useful (cost telemetry, observability for future decisions). Removing them would be premature.

4. **One commit**:
   ```
   docs: Stage J indefinitely deferred per 2026-05-23 reassessment
   
   2-week trace data shows projected fork-branch savings at X% of total
   token cost, below the 15% GO threshold. See retro for details.
   ```

---

## Path C — INSUFFICIENT DATA caveat

If Step 0 found no data (or Step 1 yielded < 50 subagent calls), the cleanest move is **NO-GO with explicit data-gap caveat** (Path B with the retro making this explicit). Do NOT proceed to GO based on speculation — Stage J is high-risk + R10 soft-pass; the precondition for it is *evidence*, not enthusiasm.

If the user *insists* on proceeding without data, treat that as overriding the design's precondition (master spec § 1.2 line 642 explicit reassessment requirement). In that case:
- Document explicitly in the plan that the design's evidence-based precondition was overridden by user directive
- Reduce scope: ship behind a feature flag / opt-in agent definition only
- Add a strict cost regression test that fails CI if savings < 30% (forcing the data to validate the design after the fact)

---

## Known follow-ups carried over from prior stages (read but don't auto-do)

These are documented gaps from earlier stages. Touch them ONLY if Path A's plan needs to (don't expand scope unilaterally):

1. **`runtime.rs::AgentRuntime` plugin_registry threading** (Stage I, T11 deferred) — runtime-spawned subagents using `mcp_servers` fail-loud until `AgentRuntime` is threaded with the registry.

2. **Stage I I-T5 real Drop-leak coverage** — `/bin/cat` doesn't speak MCP, so test takes early-return path. Real coverage needs an MCP-speaking test binary or a `pub(crate) InlineMcpHandle::new_for_test`.

3. **`AgentRuntime::fallback_llm` field doesn't go through MeteringProvider** (Stage J-pre, T7 deferred) — fallback calls don't emit `ProviderUsage`. If this is biasing the Step 1 numbers low, fix it FIRST before the reassessment (a 1-task pre-flight: same wrap pattern as `harness_bridge.rs`, label `"root-fallback"`).

4. **`McpScope::tools()` async tool surfacing** (Stage I, T8 deferred) — only reference-projected globals are surfaced. Stage J's fork branch may need full subagent tool surface; reassess during Path A planning.

5. **`CLAUDE.md` R10 text** says "9 files / ~1500 lines" but actual is 10 files. Cosmetic, fix opportunistically.

---

## Reference documents (absolute paths)

| Doc | Path |
|---|---|
| Project rules | `/Volumes/TBU4/Workspace/Aleph/CLAUDE.md` |
| Master roadmap (subagent uplift) | `/Volumes/TBU4/Workspace/Aleph/docs/superpowers/specs/2026-05-08-subagent-uplift-roadmap-design.md` |
| P3 design (Stage H + I + J context) | `/Volumes/TBU4/Workspace/Aleph/docs/superpowers/specs/2026-05-09-subagent-uplift-p3-design.md` |
| P3 Stage I plan (executed) | `/Volumes/TBU4/Workspace/Aleph/docs/superpowers/plans/2026-05-09-subagent-uplift-p3-stage-i-plan.md` |
| Stage J-pre plan (executed) | `/Volumes/TBU4/Workspace/Aleph/docs/superpowers/plans/2026-05-09-subagent-uplift-stage-j-pre-plan.md` |
| Multi-agent reference docs (cache observability section) | `/Volumes/TBU4/Workspace/Aleph/docs/reference/MULTI_AGENT_SYSTEM.md` |
| Harness philosophy | `/Volumes/TBU4/Workspace/Aleph/docs/reference/HARNESS_PHILOSOPHY.md` |

## Key code paths (absolute)

| Path | Role |
|---|---|
| `src/providers/metering.rs` | `MeteringProvider` decorator (Stage J-pre) |
| `src/providers/adapter.rs:268-280` | `TokenUsage` struct |
| `src/providers/anthropic/types.rs:192-201` | `AnthropicUsage` struct |
| `src/providers/protocols/anthropic.rs:822-889` | Streaming protocol usage extraction |
| `src/harness/trace.rs:63-76` | `LoopTraceEvent::ProviderUsage` variant |
| `shared/protocol/src/events.rs:283-302` | `AgentTraceEvent::ProviderUsage` mirror |
| `src/agents/subagent_spawner.rs:271-285` | Subagent MeteringProvider wrap site (label = `agent_def.id`) |
| `src/orchestrator/harness_bridge.rs:150-162` | Root MeteringProvider wrap site (label = `"root"`) |
| `tests/cache_observability_smoke.rs` | End-to-end smoke (3 tests) |
| `src/harness/agent.rs` | **R10 redline target** — `git diff f891cc71b -- $this | wc -l` MUST be `0` |

---

## Final checklist before you act

- [ ] Read this entire file (you're doing it now)
- [ ] Run Step 0 data availability check
- [ ] Branch on Step 0 outcome
- [ ] If data exists: run Step 1, compute Step 2 metrics, apply Step 3 decision tree
- [ ] Path A or Path B per outcome
- [ ] Honor R10 throughout (every commit verifies `git diff f891cc71b -- src/harness/agent.rs | wc -l` = 0)
- [ ] One closure commit at end (Path A: roadmap shipped line; Path B: deferred line)
- [ ] Report final SHA + decision summary to user

You're authorized to operate autonomously through brainstorming/writing-plans/SDD without per-task confirmation, matching the cadence Stage J-pre used. Pause only for: BLOCKED status you can't resolve, R10 violation surfaced unexpectedly, or genuine ambiguity in the data Step 1 produces.

Begin with Step 0.
