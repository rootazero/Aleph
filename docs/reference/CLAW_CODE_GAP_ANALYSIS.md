# claw-code Gap Analysis — 2026-05-27

Reference project: `/Volumes/TBU4/Github/claw-code` (TS upstream + `rust/crates/`
runtime port).

## TL;DR

claw-code's `runtime` crate (47 files / ~38k LOC) is a **thick monolith** that
bundles harness + session + permissions + MCP + git + bash + lanes + tasks.
This is **fundamentally incompatible** with Aleph's R10 *thin harness* /
*dumb loop* philosophy and 4.2 of [HARNESS_PHILOSOPHY.md](HARNESS_PHILOSOPHY.md)
already partitions the 12 harness modules correctly across `src/`.

**Verdict**: do not port the runtime monolith. This document records each
considered borrow and either (a) the targeted port (one) or (b) the
justified decline (everything else). Future sessions should consult this
file before re-investigating.

## Audit Summary

| claw-code module | claw-code LOC | Considered for | Status | Reason |
|---|---|---|---|---|
| `trident.rs` stage 1 (supersede file-ops) | ~50 of 849 | `src/context/budget/cheap_passes/` | **PORTED + WIRED** → `cheap_passes/file_op_supersede.rs` (implements `PreflightStage`, registered in `orchestrator/harness_bridge.rs` alongside `ToolResultPruningStage` + `HistoricalImageStrippingStage`). Pure deterministic context reduction; R7-aligned. Safer than upstream (replaces tool-result body, never drops messages). |
| `trident.rs` stage 2 (collapse chatty runs) | ~150 | `src/context/compact/` | **DECLINED** | Aleph's `tool_aware_chunker` already handles semantic-unit-aware chunking. "Chatty" is a heuristic — R7 violation. |
| `trident.rs` stage 3 (cluster by cosine sim ≥0.6) | ~250 | `src/context/compact/` | **DECLINED** | Similarity threshold replaces LLM judgment — R7 violation. |
| `recovery_recipes.rs` (FailureScenario → nudge map) | 941 | `src/providers/llm_retry.rs` | **DECLINED** | Already covered by `llm_retry::classify` + [[harness-claude-code-parity]] (Halt verdict, max_output_tokens recovery, prompt_too_long classify). The remaining recipes (`StaleBranch`, `CompileRedCrossCrate`) are git-workflow-specific to claw-code's coordinator role; not applicable to Aleph (R6). |
| `summary_compression.rs` (line-budget post-processor) | 350 | `src/context/compact/summary_utils.rs` | **DECLINED** | Aleph's `summary_utils` is intentionally minimal (89 LOC) and the production compactor produces bounded summaries via the prompt template (R10/R7). Adding a deterministic post-processor would not change tokens enough to justify the entropy. |
| `bash_validation.rs` (intent classifier: ReadOnly/Write/Destructive/Network…) | 1004 | `src/sandbox/` | **DECLINED** | Aleph executes bash inside a sandbox (`SandboxOutput`, deny-by-default). Pre-classify by rules is exactly what R7 says LLM should decide; a parallel rule engine is duplicate gate + R7 violation. |
| `stale_branch.rs` / `branch_lock.rs` / `stale_base.rs` | ~600 | n/a | **DECLINED** | Aleph isn't a git coordinator. Its bus is JSON-RPC over Gateway, not git branches (R6). claw-code's branch-state guardrails make sense for their multi-agent coordination model; they have no role in Aleph. |
| `green_contract.rs` (TargetedTests/Package/Workspace/MergeReady levels) | ~300 | `src/verification/` | **DECLINED** | Aleph's verification surface is broader: `stop_hook_verifier` + `tool_loop_verifier` + `turn_verifier` + LLM-judge hooks. A 4-level test-pass enum would be a single, less expressive verifier amongst those already wired. |
| `approval_tokens.rs` (delegation hops + audit ledger) | 502 | `src/approval/` | **DECLINED** | claw-code's tokens solve **chain-of-custody delegation** for their multi-claw coordinator scenario (claw A grants, claw B consumes, audit trail records both). Aleph's R5 "AI comes to you" + R6 "one core many channels" optimises a different shape: single user, many notification channels. Approval flows via `src/approval/session_route.rs` (per-session routing) + `src/approval/callback_sink.rs` (direct prompt to the user's active channel), no delegated-token primitive needed. `ApprovalDelegationHop` would be dead structure at Aleph scale. |
| `policy_engine.rs` / `permission_enforcer.rs` / `permissions.rs` | 2249 | `src/security/`, `src/sandbox/` | **DECLINED** | Aleph splits permission across `security`, `sandbox`, `approval`, `pii` (HARNESS_PHILOSOPHY §4.2 row 9). Re-aggregating into one engine would violate R3 + the "4 domains stay separate" rule. |
| `mcp_*.rs` (~5k LOC across 7 files) | 5000 | `src/mcp/` | **DECLINED** | Aleph already has a complete MCP implementation. Wholesale comparison out of scope; no single concrete gap identified during scan. |
| `hooks.rs` (HookAbortSignal, HookProgressReporter) | 1116 | `src/extension/hooks.rs` | **DECLINED** | Aleph's hooks subsystem is mature (R8 surface, 13-RPC catalogue per [[cli-parity-r2]]). No specific missing primitive identified. |
| `lane_events.rs` (event topology + fingerprints) | 2561 | `src/gateway/` (GatewayEventFrame) | **DECLINED** | Aleph already implements gateway event broadcasting (see [[gateway-r5b]], [[teams-r2-workflow-canvas-bridge]]). Different event model; no merge benefit. |
| `worker_boot.rs` | 2444 | `src/bin/aleph-server/commands/start/` | **DECLINED** | Aleph's boot is per-component, see [BOOT_ASSEMBLY.md]. No claw-code-shaped worker-boot abstraction needed. |
| `task_packet.rs` / `task_registry.rs` / `team_cron_registry.rs` | 1900 | `src/tasks/`, `src/teams/` | **DECLINED** | Aleph's `tasks` + `teams` subsystems are mature post-[[teams-r2-workflow-canvas-bridge]] + [[cron-iter-cap-perseverance]]. No specific gap. |

## Aleph Harness Health Snapshot (2026-05-27)

`src/harness/` measured during this audit:

| File | total LOC | production LOC | role |
|---|---|---|---|
| `agent/think.rs` | 1294 | 1185 | run_turn_internal + race_llm_call + reactive_compact rescue + grace turn |
| `agent/act.rs` | 853 | 825 | tool dispatch (serial + parallel) + turn budget + emit_tool_{success,error} |
| `agent.rs` | 1207 | 764 | `AgentHarness` struct + Harness/SessionDriver impls + `run()` loop |
| `trace.rs` | 473 | 401 | LoopTraceEvent collection |
| `agent/prompt.rs` | 661 | 236 | build_prompt + parse_tool_use_block + resolve_tool_name |
| `deps.rs` | 286 | 220 | `HarnessDeps` DI + `StallTracker` |
| `trait_def.rs` | 193 | 166 | `Harness` trait + `HarnessError` + `TurnState` |
| `agent/guardrails.rs` | 123 | 123 | apply_input_guardrail + apply_tool_call_guardrail |
| `callback.rs` | 148 | 97 | `HarnessCallback` + `NoopHarnessCallback` |
| `chain_context.rs` | 158 | 96 | per-turn chain context |
| `trace_sink.rs` | 33 | 33 | `TraceSink` trait |
| `mod.rs` | 33 | 20 | re-exports |
| **Total** | **5462** | **4166** | — |

R10 budget: **12 files / ~4900 LOC**. Aleph harness is currently **12 files /
4166 prod LOC**, with **~734 LOC headroom**. The audit found no R10-violating
authority logic (intent classify, completeness judge, similarity scoring) and
no dead code. Every helper has documented R10 rationale; every constant
(`MAX_REACTIVE_COMPACT_ATTEMPTS = 1`, `MAX_VERIFIER_VETOS = 10`,
`GRACE_NUDGE_*`) is a single-write cap, not a policy decision.

**Conclusion**: harness熵减 (Track A) was not warranted this round.
Post-dissolution and post-[[harness-r2-minimalism]] the surface is healthy.

## What Got Ported

[`src/context/budget/cheap_passes/file_op_supersede.rs`](../../src/context/budget/cheap_passes/file_op_supersede.rs)
— file-op last-write-wins preflight stage. Implements
[`PreflightStage`](../../src/context/budget/preflight.rs) and is wired into the
production `PreflightPipeline` in
[`orchestrator/harness_bridge.rs`](../../src/orchestrator/harness_bridge.rs)
alongside `ToolResultPruningStage` and `HistoricalImageStrippingStage`. Runs
*before* the LLM compactor so token savings happen even when the compactor's
side-channel LLM call fails.

Safety guarantees beyond claw-code's stage1:

1. **No message deletion.** The earlier `ToolResult`'s *body* is replaced
   with a stub naming the superseding tool. The assistant message issuing
   the tool call is untouched — the model's reasoning chain stays intact.
2. **Pressure-gated.** Defaults to `PressureLevel::Preventive` (≥60%) before
   firing. Calm-pressure runs pay nothing.
3. **Fresh-tail respected.** `ctx.fresh_tail_count` shields recent messages
   from supersession.
4. **Error results never touched.** A failing read followed by a successful
   write keeps the error text — the model may need it to explain itself.
5. **Path canonicalisation is conservative.** Only `path` / `file_path` from
   arguments; no output-string heuristics (claw-code parses `path: ` lines
   from output — brittle).
6. **Deterministic estimate.** `confidence: 0.95` because the only drift
   between estimate and actual is the `chars / ratio` quantum.

Test coverage (13 tests):
- read-then-write supersedes the read
- last mutating op is preserved
- reads-only never supersede (no canonical state yet)
- different paths are independent
- pressure gate blocks below threshold
- fresh tail protects recent messages
- error results are never stubbed
- alias tool names (Read/Write/Edit/read_file/…) are classified
- estimate=0 when nothing obsolete
- estimate ≈ actual within 1-token rounding
- no match when the `path` argument is missing
- integration: supersede → pruning → image-strip run in the real 3-stage
  `PreflightPipeline` and their freed-token counts sum correctly

## References

- `/Volumes/TBU4/Github/claw-code/rust/crates/runtime/src/trident.rs`
- `/Volumes/TBU4/Github/claw-code/PHILOSOPHY.md` — claw-code's
  human-as-director / claws-as-labour model. Different from Aleph's
  one-core-many-channels (R6), but the runtime patterns above were the
  candidates of interest.
- [HARNESS_PHILOSOPHY.md](HARNESS_PHILOSOPHY.md) — Aleph's R10 article.
- [CODE_ORGANIZATION.md](CODE_ORGANIZATION.md) — where each of the 12
  harness modules lives.
