# Multi-Provider Failover & Per-Agent Routing — Design Spec

**Date:** 2026-05-19
**Branch:** `feat/provider-failover`
**Status:** Phases 1–4 SHIPPED & verified — the multi-provider failover work is complete.

## Status (2026-05-19)

| Phase | State | Commit |
|---|---|---|
| 1 — Failover engine | ✅ shipped, 17 unit tests | `7b1a03bf7` |
| 2 — Config + wiring (`FailoverProvider` as `deps.llm`) | ✅ shipped, 69 lib + 7 bin tests green | `219da2ec4` |
| 3 — Per-agent `provider_hint` + subagent failover | ✅ shipped, new unit tests green | `b6f5c091a` |
| 4 — Dead-code cleanup (`fallback_llm`, `AgentModelConfig`) | ✅ shipped | `feat/provider-failover-phase4` |

**Delivered:** automatic provider **and** model failover when the default
provider/model fails — an ordered `[fallback_provider].chain`, model-level
fallback across each provider's `models[]`, a per-provider circuit breaker, and
rich error classification (the formerly-dead `llm_retry.rs` classifier and
`FailoverProvider` are now both live). Plus per-agent provider selection via
`AgentDef.provider_hint`. No harness changes — R10 holds.

**Phase 3 implementation notes:** the original plan assumed four `SubagentTool`
construction sites and the `named_providers` map. Investigation corrected both:
only **one** site is production (`gateway/execution_engine/run_loop.rs`) — the
rest are `#[cfg(test)]` — and the subagent path never consults
`named_providers`/`pick_llm`; it uses `SpawnerBase.provider`. That production
site also handed subagents a *bare* provider
(`provider_registry.default_provider()`), silently bypassing the Phase 1–2
failover chain — so Phase 3 fixes that bypass too. What shipped:
`build_failover_chain` now returns a `ProviderChain { default, agent_overrides }`;
`agent_overrides` maps each non-primary provider to a `FailoverProvider` that
pins it as primary then falls through the whole global chain, all sharing one
`FailoverHealth`. The chain is carried on `Orchestrator`
(`with_subagent_routing`); `run_loop.rs` reads it so **every** subagent runs on
the failover chain, and `AgentDef.provider_hint` selects an override. The
registry threads `SubagentTool` → `AgentRuntime`, resolved in
`AgentRuntime::spawn_subagent` (`provider_hint` → override, else default).

**Phase 4 implementation notes:** two coherent commits. (4a) Removed the
inert single-step `deps.fallback_llm` seam — the `HarnessDeps` field, the
`think.rs` Stage 5b retry arm, the `AgentHarnessRunner`/`SpawnerBase`/
`SubagentTool`/`AgentRuntime` fields + `with_fallback_llm` builders, and the
`stage5b-fallback` init seam (the contract is now **eight** events, not
nine). The `FailoverProvider` chain in `deps.llm` fully subsumes it. (4b)
Removed the half-dead `AgentModelConfig { primary, fallbacks }` — the type,
the `model_config` field on `AgentDefinition`/`AgentPatch`/`CreateAgentParams`,
and its TOML-write + CRUD + gateway plumbing. It round-tripped through config
but never reached an LLM call; `AgentDefinition` has no `deny_unknown_fields`,
so dropping the field silently ignores it in any legacy config file.

## 1. Problem

Aleph cannot automatically switch providers/models when the default one fails,
and cannot assign different providers to different agents. hermes-agent (the
reference super-assistant) does both: an ordered `{provider, model}` fallback
chain with error-classified recovery, and per-subagent provider override that
inherits the parent's chain.

The user hypothesis — *"infrastructure exists, just missing wiring"* — is
confirmed. Aleph built the failover engine **twice** and connected neither.

## 2. Gap Analysis (Aleph today)

| Component | Lines | Status |
|---|---|---|
| `providers/failover.rs` — `FailoverProvider`: 3-state circuit breaker, priority ordering, per-provider health + metrics, TOCTOU-safe acquire | 686 | **Dead.** Only a `pub use` export. `deps.rs` doc-comment tells callers to use it; nobody does. |
| `providers/llm_retry.rs` — `RetryVerdict`, `classify_error/exhausted/http`, `Retry-After` + `x-ratelimit-reset` header parsing, 429 model-vs-account discrimination, token-gap parsing | 803 | **Dead.** Zero callers outside its own tests. |
| `AgentModelConfig { primary, fallbacks }` | — | **Half-dead.** Round-trips through config CRUD + gateway, but never reaches a real LLM call. |
| `ProviderConfig.models: Vec<String>` | — | **Half-wired.** TOML accepts `models = ["a","b","c"]`; `default_model()` only ever returns `models[0]`. |

The **only live failover** is `harness/agent/think.rs` Stage 5b: a single
`deps.fallback_llm` retried **once** on `ErrorClass::Transient`. No chain, no
model-level fallback, no rich classification (a 429 does not reliably trip it —
only `Transient` does), no circuit memory.

**Missing entirely:** per-agent provider selection. `BrainRef`
(Default/Preferred/Strict) exists in `pick_llm`, but agent definitions carry no
provider field — every agent shares one global provider. Only `model` is
per-agent, via the `ModelOverrideProvider` decorator.

## 3. Guiding Principle

Consolidation + wiring, **not** new architecture. Reuse `FailoverProvider` as
the spine, fuse `llm_retry.rs` classification into it, delete the redundant
single-step path.

**Redline R10 compliance:** failover is a *provider decorator*. The harness loop
sees one `Arc<dyn AiProvider>` and never knows failover exists. No
recovery-strategy logic enters `src/harness/`. The decorator lives in
`src/providers/` — the correct home for the "Error/Recovery" Harness module.
This matches Aleph's idiomatic decorator-stacking pattern
(`MeteringProvider(ModelOverrideProvider(base))`).

## 4. Architecture

```
deps.llm  ─►  FailoverProvider  (decorator, src/providers/failover.rs)
                 ├─ primary slot   ← DefaultProviderHandle.current()  (hot-reload preserved)
                 ├─ fallback nodes ← [fallback_provider].chain         (static, boot-built)
                 ├─ model catalog  ← provider name → models[]          (boot snapshot)
                 ├─ circuit state  ← per-provider HealthState (Closed/Open/HalfOpen), shared Arc
                 ├─ classify       ← llm_retry::classify_error → RetryVerdict
                 └─ trace_sink     ← emits a fallback event for the UI ModelFallback stream
```

### 4.1 Candidate walk

Each `process()` call builds an ordered candidate list at call time:

1. **Primary** — `default_handle.current()`; its models from the boot model
   catalog (keyed by provider name, so hot-reloading the default still resolves
   a model list).
2. **Fallback nodes** — each `chain` provider, expanded across its `models[]`.
   Dedup: skip any fallback whose provider name equals the live primary.

Walk: classify each failure via `llm_retry::classify_error` →

| `RetryVerdict` | Action |
|---|---|
| `Retry { delay }` | backoff `delay`, retry **same** candidate, up to `max_retries` |
| `Fallback { reason }` | advance to next candidate; provider-level reasons (429/401/5xx-exhausted) trip the provider's circuit so its remaining models are skipped |
| `Fatal` | stop — a 400 will not be fixed by switching |
| `CompactAndRetry` | propagate the error — the harness context-compactor owns 413 |

`Ok` → mark provider healthy, return. All candidates exhausted → return the
last error.

### 4.2 Circuit breaker

Keep the existing `FailoverProvider` 3-state machine (Closed → Open → HalfOpen,
doubling cooldown capped at 10 min) and metrics. Key health state by **provider
name** in a shared `Arc<RwLock<HashMap<String, HealthState>>>` so every chain
(global + per-agent) sees the same provider-health picture: one provider's
rate-limit is known everywhere.

### 4.3 Hot-reload interaction

The primary slot reads `DefaultProviderHandle.current()` live on every call, so
UI-driven `set_default` still takes effect next turn. The model catalog is a
boot snapshot of all configured providers' `models[]` — adequate because
`set_default` can only target an already-configured provider.

## 5. Implementation Phases

Each phase is independently testable and committable.

### Phase 1 — Failover engine (`providers/failover.rs`, `providers/llm_retry.rs`)

- Refactor `FailoverProvider`: keep circuit-breaker state machine + metrics +
  TOCTOU-safe acquire; replace crude `is_non_retryable_error` /
  `matches!(RateLimitError)` with `llm_retry::classify_error`.
- Add the model-level inner loop (walk a provider's `models[]`).
- Constructor accepts pre-built nodes (`name`, `models`, `Arc<dyn AiProvider>`)
  + a shared health map — the orchestrator already builds providers; do not
  double-construct via `create_provider`.
- Gives `llm_retry.rs` its first real consumer.
- Unit tests: model-level walk, circuit trip on provider-level reason, `Fatal`
  short-circuit, `Retry` backoff, dedup, all-exhausted error.

### Phase 2 — Config + wiring (`config/types/phase6_wiring.rs`, `orchestrator/deps_builder.rs`, `bin/aleph-server/.../orchestrator_init.rs`, `orchestrator/harness_bridge*`)

- `FallbackProviderToml` gains `chain: Vec<String>`; single `provider` kept as
  a back-compat alias (`provider = "x"` ≡ `chain = ["x"]`).
- `build_fallback_llm` → `build_failover_chain(config, primary_key)` → builds
  the `FailoverProvider`, validating self-reference + unknown providers
  (warn-and-skip, mirroring current behavior).
- Wire the `FailoverProvider` as `deps.llm` for `BrainRef::Default`.
- **Remove the single-step Stage 5b path:** `deps.fallback_llm` field, the
  `think.rs` retry arm, `with_fallback_llm` across `runtime.rs` /
  `subagent_tool.rs` / `subagent_spawner`. The chain fully subsumes it —
  behavior preserved and strengthened. Shrinks `harness/deps.rs` + `think.rs`
  (net-positive for R10).
- Fallback events still reach the UI: the `FailoverProvider` emits a fallback
  trace event via `trace_sink` (same pattern `MeteringProvider` uses); the
  gateway's existing `FlowStreamEvent::ModelFallback` is sourced from it.
- `on_model_fallback` callback trait method is retained (harmless, tiny) but no
  longer driven from `think.rs`.

### Phase 3 — Per-agent provider + subagent failover (SHIPPED)

Files: `agents/types.rs`, `agents/loader.rs`, `orchestrator/deps_builder.rs`,
`orchestrator/dispatch.rs`, `agents/runtime.rs`, `agents/subagent_tool.rs`,
`bin/aleph-server/.../orchestrator_init.rs`, `gateway/execution_engine/run_loop.rs`.

- `AgentDef` gains `provider_hint: Option<String>` alongside `model_hint`
  (`#[serde(default)]` for back-compat); `loader.rs` parses it from frontmatter.
- `build_failover_chain` returns `ProviderChain { default, agent_overrides }`.
  Each `agent_overrides` entry is a `FailoverProvider` that pins one non-primary
  provider as primary, then adds a single fallback node = the whole global
  chain ("pin + fall through"). All chains share one `FailoverHealth`, so a
  provider's outage is visible across every chain. The primary gets no entry —
  hinting it is equivalent to no hint.
- The `ProviderChain` is carried on `Orchestrator` via `with_subagent_routing`.
  `run_loop.rs` reads `orchestrator.subagent_routing`: `chain.default.current()`
  becomes the subagent provider — **fixing a pre-existing bypass** where the
  gateway handed subagents a bare `provider_registry.default_provider()` — and
  `chain.agent_overrides` is threaded into `SubagentTool::with_provider_overrides`.
- `SubagentTool` → `AgentRuntime` carry the override map; `spawn_subagent`
  resolves `agent_def.provider_hint` → override (else the shared default).
- When `provider_hint` is unset, the subagent uses the global `FailoverProvider`
  unchanged.

### Phase 4 — Dead-code cleanup (SHIPPED)

Two commits.

- **4a — `fallback_llm`**: removed the inert single-step Stage 5b seam. The
  `HarnessDeps.fallback_llm` field, the `think.rs` transient-error retry arm,
  the `AgentHarnessRunner`/`SpawnerBase`/`SubagentTool`/`AgentRuntime` fields +
  `with_fallback_llm` builders, the `orchestrator_init.rs` `None` wiring, and
  the `stage5b-fallback` init seam. The init-seam contract is now **eight**
  events; `emit_init_seams` lost its `fallback_llm_configured` parameter. The
  `FailoverProvider` chain in `deps.llm` fully subsumes the single-step path —
  this also shrinks `src/harness/` (net-positive for R10).
- **4b — `AgentModelConfig`**: removed the half-dead `AgentModelConfig
  { primary, fallbacks }` type, the `model_config` field on `AgentDefinition`
  / `AgentPatch` / `CreateAgentParams`, and its TOML-write (`toml_ops.rs`,
  `crud.rs`) + gateway-handler plumbing. It round-tripped through config but
  never reached an LLM call; its independent-list semantics never matched the
  chosen "fall through global chain" model.

## 6. Scope Boundaries (YAGNI)

- `llm_retry::classify_http_error` + header-based `resolve_retry_delay` need raw
  HTTP headers, visible only to the protocol adapters — **not deleted**, but
  wiring them at the adapter layer is an explicit *follow-up*, not this cycle.
  `FailoverProvider` uses string-based `classify_error` on `AlephError`, which
  is what already works.
- No credential-pool / multi-API-key rotation — Aleph has no multi-key infra;
  out of scope.
- No new LLM calls, no intent classification — `FailoverProvider` is pure
  deterministic plumbing (consistent with R7: this is enabling infrastructure,
  not reasoning the model should do).

## 7. Success Criteria

- A primary-provider failure (429 / 5xx / network) transparently produces a
  successful response from a fallback provider, with no harness changes.
- A provider with `models = ["a","b"]` tries `b` after `a` fails with a
  model-level error (404).
- An agent with `provider_hint` runs on that provider and still fails over down
  the global chain.
- A 400 bad-request does **not** trigger pointless cross-provider retries.
- `cargo test -p alephcore --lib` green; no dead `fallback_llm` /
  `AgentModelConfig` references remain.
