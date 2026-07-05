# Aleph Provider Layer Map — for a Virtual MoA Provider Design

All paths relative to `/Volumes/TBU4/Workspace/Aleph`. Line numbers verified against working tree on 2026-07-05 (branch `main`).

---

## 1. The `AiProvider` trait and request/response shapes

### 1.1 Trait definition — `src/providers/mod.rs:233-295`

```rust
pub trait AiProvider: Send + Sync {
    /// Core method — process a request and return structured response
    fn process<'a>(
        &'a self,
        payload: adapter::RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>>;

    fn name(&self) -> &str;
    fn color(&self) -> &str;

    fn supports_native_tools(&self) -> bool { false }

    /// Protocol name for model behavior resolution ("openai", "anthropic", ...)
    fn protocol(&self) -> Cow<'_, str> { Cow::Borrowed("unknown") }

    /// Explicit config override for model-behavior directives
    fn model_behavior_override(&self) -> Option<Cow<'_, str>> { None }

    /// Self-identified governance behavior (e.g. Kimi/Minimax → "strict")
    fn behavior_hint(&self) -> Option<Cow<'_, str>> { None }

    /// Best-effort id of the model this provider would serve next.
    /// Keys per-model lookups: context-window (gauge denominator) + pricing.
    fn serving_model_hint(&self) -> Option<Cow<'_, str>> { None }

    /// Downcast to HttpProvider for streaming access.
    fn as_http_provider(&self) -> Option<&http_provider::HttpProvider> { None }
}
```

Key facts:
- **Not `async_trait`** — hand-rolled `Pin<Box<dyn Future>>` with lifetime `'a` tied to `&self` and the borrowed payload. Wrappers that rebuild the payload per attempt must own the borrowed fields first (see `FailoverProvider::process`, `src/providers/failover/provider.rs:597-618`, which does `payload.messages.to_vec()` etc.).
- There is **no streaming method on the trait**. Streaming is reached only by downcast: `as_http_provider()` → `HttpProvider::execute_streaming` / `stream_raw`. Only `HttpProvider` returns `Some(self)` (`src/providers/http_provider.rs:652-654`); `MeteringProvider` delegates (`src/providers/metering.rs:114-116`); `FailoverProvider` and `ModelOverrideProvider` do **not** implement it → default `None`.
- All wrapper methods (`protocol`/`behavior_hint`/`serving_model_hint`) are expected to delegate to the "live primary" — every existing wrapper does.

### 1.2 `RequestPayload` — `src/providers/adapter.rs:37-70`

```rust
pub struct RequestPayload<'a> {
    pub messages: &'a [UnifiedMessage],
    pub system_prompt: Option<&'a str>,
    pub system_blocks: Option<&'a [SystemPromptPart]>,   // stable/dynamic cache split (Anthropic prompt-cache)
    pub tools: Option<&'a [ToolDefinition]>,             // crate::tool_metadata::ToolDefinition
    pub think_level: Option<ThinkLevel>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub tool_choice: Option<ToolChoice>,                 // Auto | Required | Specific(String) | None
    pub model: Option<String>,                           // per-request model override — beats provider config
    pub metadata: Option<HashMap<String, String>>,       // e.g. "session_id" (drives hook attribution + cache keys)
}
```

Builder methods `with_system/with_system_blocks/with_tools/with_think_level/with_temperature/with_max_tokens/with_tool_choice/with_model/with_metadata` at adapter.rs:90-165. `payload.model` is the single lever every model-pinning wrapper uses.

### 1.3 `ProviderResponse` — `src/providers/adapter.rs:240-265`

```rust
pub struct ProviderResponse {
    pub text: Option<String>,
    pub tool_calls: Vec<NativeToolCall>,           // { id, name, arguments: Value, thought_signature: Option<String> }
    pub thinking: Option<String>,
    pub thinking_signature: Option<String>,        // Anthropic signed-thinking replay token
    pub stop_reason: StopReason,                   // EndTurn | ToolUse | MaxTokens | ContextWindowExceeded | StopSequence | PauseTurn | Refusal | Sensitive | Unknown
    pub usage: Option<TokenUsage>,
    pub truncated_tool_call: Option<String>,       // mid-stream truncation diagnostic → promoted to retryable error
}
```

### 1.4 `TokenUsage` / `TokenCost` — `src/providers/adapter.rs:354-491`

```rust
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: Option<u32>,
    pub cache_creation_tokens: Option<u32>,
    pub thinking_tokens: Option<u32>,
    pub cost: Option<TokenCost>,       // { input_cost_per_million, output_cost_per_million }
}
```
Provider-shape-aware helpers: `cache_hit_ratio()` (adapter.rs:411), `prompt_tokens_total()` (adapter.rs:450), `context_occupancy_tokens()` (adapter.rs:480) — the last one is the live gauge numerator (prompt + generated, thinking folded without double count). `merge_usage` exists in `src/providers/delta.rs` (used at http_provider.rs:528) for accumulating multiple `Usage` deltas — reusable for merging advisor usages.

### 1.5 Protocol layer (below `AiProvider`)

`ProtocolAdapter` trait — `src/providers/adapter.rs:176-229`: `build_request(&payload, &ProviderConfig) -> reqwest::RequestBuilder`, `stream_deltas(reqwest::Response) -> BoxStream<Result<ProviderDelta>>` (stream-first, no non-streaming parse path), `supports_native_tools`, `normalize_model_id`. Registered in a global `ProtocolRegistry` (openai / anthropic / gemini / openai-responses / codex / dynamic YAML protocols). `HttpProvider` = `name + ProviderConfig + Arc<dyn ProtocolAdapter>`.

---

## 2. Registration and selection

### 2.1 Factory — `create_provider(name, ProviderConfig)` at `src/providers/mod.rs:174-222`

Pure function, callable anywhere at runtime: applies preset defaults (`presets::get_preset` — base_url/protocol/color; `src/providers/presets/`), resolves the protocol adapter from the global `ProtocolRegistry`, returns `Arc<HttpProvider>` (or native `OllamaProvider` / `MockProvider`). Returns `Arc<dyn AiProvider>`.

### 2.2 Boot-time registry — two layers

1. **`MultiProviderRegistry`** (`src/thinker/mod.rs:223-379`) — name→provider map with a hot-swappable default (`set_default` :270, `default_provider` :345, `register` :244, RwLock interior mutability). Built at boot in `src/bin/aleph-server/commands/start/builder/agent_init/provider_registry.rs:23-155`: iterates `config.providers` (`[providers.<name>]` in aleph.toml), **hydrates `api_key` from the vault** (`shared_token_mgr.get_secret("ai:<name>")`, :30-48), skips unkeyed/disabled providers, calls `create_provider`. It implements `DefaultProviderHandle` so `current()` reads through the RwLock each turn (hot-reload of `set_default`).

2. **Failover chain** — `build_failover_chain(config, primary_provider_key, default_provider, escalation_approval, route_handle) -> ProviderChain` at `src/orchestrator/deps_builder.rs:228-409`, called from `src/bin/aleph-server/commands/start/orchestrator_init.rs:199-226`.

```rust
pub struct ProviderChain {                                   // deps_builder.rs:200
    pub default: Arc<dyn DefaultProviderHandle>,             // global FailoverProvider wrapped in StaticDefault
    pub agent_overrides: HashMap<String, Arc<dyn AiProvider>>, // per-provider pin chains (pin + fall-through global)
    pub observability: RouteObservability,
}
```
   - Every non-primary configured provider is built once via `create_provider` (deps_builder.rs:281-296) and reused for both the fallback list and per-name pin chains.
   - `agent_overrides` become the harness's **`named_providers`** (orchestrator_init.rs:226) — so `BrainRef::Strict/Preferred` and `select_model(provider=…)` resolve to a route-shaped, circuit-broken chain, never a raw provider.

### 2.3 Per-run selection — the exact resolution path

`DefaultProviderHandle` trait — `src/providers/default_handle.rs:21-41`:
```rust
pub trait DefaultProviderHandle: Send + Sync {
    fn current(&self) -> Arc<dyn AiProvider>;
    fn provider_names(&self) -> Vec<String> { Vec::new() }
    fn provider_by_name(&self, _name: &str) -> Option<Arc<dyn AiProvider>> { None }
}
```

`pick_llm` — `src/orchestrator/harness_bridge/llm.rs:19-52`:
```rust
pub(super) fn pick_llm(
    brain: &BrainRef,                                     // Default | Preferred{provider} | Strict{provider, model: Option<String>}
    default_provider: &Arc<dyn DefaultProviderHandle>,
    named: &HashMap<String, Arc<dyn AiProvider>>,
) -> Result<Arc<dyn AiProvider>, FlowError>
```
`Strict` with a model wraps in `ModelOverrideProvider`. `BrainRef` is at `src/orchestrator/flow_spec.rs:68`.

**Selection granularity is per-RUN** (one run = one Think→Act loop for one user turn), resolved in `AgentHarnessRunner::run` Step 3 — `src/orchestrator/harness_bridge/runner_impl.rs:101-133`. Precedence:
1. **Session pick** (`select_model` tool): `session_model_handle::get_session_model(&session_pref_key)` — a process-global `OnceLock<RwLock<HashMap<String, SessionModelPref{provider: Option<String>, model: String}>>>` (`src/providers/session_model_handle.rs:22-60`), written by `src/builtin_tools/select_model.rs`.
2. **Agent pin**: `agent_registry.get(agent).model_hint` (+ `provider_hint`).
3. **Flow brain**: `pick_llm(&spec.brain, ...)`.

For (1)/(2): base = `named_providers.get(provider)` else `default_provider.current()`, then `Arc::new(ModelOverrideProvider::new(base, model))` (runner_impl.rs:112-122). Then **always** wrapped: `MeteringProvider::new(llm, trace_sink, "root")` (runner_impl.rs:127-129).

The resolved provider is injected as **`HarnessDeps.llm: Arc<dyn AiProvider>`** (`src/harness/deps.rs:35`), with the doc note: *"Provider-tier failover … is layered inside this AiProvider … the harness sees one provider and never knows failover exists (R10)."* **This is exactly the seam a virtual MoA provider slots into — the harness is provider-shape-blind by design.**

### 2.4 Where the harness calls the provider

`src/harness/agent/think.rs`:
- Primary call gate at :576-586:
```rust
let provider_streams = may_stream_deltas(
    self.deps.guardrails.as_deref(),
    self.deps.llm.as_http_provider().is_some(),   // ← streaming requires the HTTP downcast seam
);
let primary_call = if provider_streams {
    self.stream_llm_call(payload, callback, parent_cancel, started).await?
} else {
    self.race_llm_call(self.deps.llm.process(payload), parent_cancel, started).await?
};
```
- All retries/rescues (`empty-response` :643, `max_tokens resume` :728, reactive compaction, grace turn :1703) go through non-streamed `self.deps.llm.process(...)`.
- Every call is raced against cancel + `turn_timeout` (`race_llm_call`, think.rs:1560-1585). **A MoA facade's fan-out must fit inside one `turn_timeout` (default recommendation 300s) — the harness wraps the whole `process()` future.**
- Subagents get the identical stack: `src/agents/subagent_spawner/mod.rs:361-371` (`ModelOverrideProvider` per-spawn pin + `MeteringProvider` with the child agent_id).

---

## 3. Existing composite/facade providers — the patterns to follow

There are **four production wrapper `AiProvider`s** (plus the retry helpers). `src/resilience/` is database resilience only; `src/routing/` is the VESR routing-experience memory (recall text, not provider selection) — neither contains provider wrappers.

| Wrapper | File | What it adds | Delegation style |
|---|---|---|---|
| `FailoverProvider` | `src/providers/failover/provider.rs:51-909` | ordered provider chain, per-model fallback, circuit breaker, 429 cooldowns, load-balancing, route-tier gating, in-place retry w/ backoff+jitter | metadata methods read `primary.current()` live (:869-909); `process` owns payload fields, rebuilds `RequestPayload` per attempt (:597-732); **no `as_http_provider`** → suppresses live deltas by design |
| `MeteringProvider` | `src/providers/metering.rs:20-117` | after each `process`, logs usage + emits `LoopTraceEvent::ProviderUsage{agent_id, input/output/cache/thinking}` to `TraceSink`; feeds global `CacheMonitor` | delegates every metadata method verbatim, **including `as_http_provider`** (:114) |
| `ModelOverrideProvider` | `src/providers/model_override_provider.rs:27-79` | stamps `payload.model = Some(self.model)` unconditionally; `serving_model_hint` returns the stamped model (:76-78) | delegates everything else; no `as_http_provider` |
| `AuthProfileProviderRegistry` | `src/providers/auth_profile_registry.rs` | multi-credential rotation for one vendor (profile ordering, cooldown, mark_success/failure) | registry-shaped, builds providers per credential via `create_provider` |

Retry helpers (not wrappers): `src/providers/retry.rs` (`retry_with_backoff`, `apply_jitter`), `src/providers/llm_retry.rs` (`backoff_delay`, `is_transient_overload`), `src/providers/failover/decision.rs` (`decide(&err, attempt, max) -> RetrySame|NextModel|RateLimited|NextProvider|Stop`).

**Production provider stack per run (main agent):**
`MeteringProvider("root") → [ModelOverrideProvider(pinned model)] → FailoverProvider(global) → primary HttpProvider | fallback HttpProviders`.

The hermes-MoAClient-equivalent shape in Aleph terms: a `MoaProvider` struct implementing `AiProvider`, holding `advisors: Vec<Arc<dyn AiProvider>>` + `aggregator: Arc<dyn AiProvider>`, inserted where `ModelOverrideProvider` is today (between Metering and Failover, or wrapping per-advisor chains from `named_providers`). `FailoverProvider` is the canonical example of a fan-out-shaped `process()` that owns its payload and re-issues sub-requests.

---

## 4. Usage + cost accounting per call

### 4.1 Where usage is recorded

Three parallel channels, all fed from `ProviderResponse.usage`:

1. **Harness run counters** — the harness folds the *surviving* response's usage once per turn: `account_intermediate_tokens` (think.rs:320-324) for discarded intermediate calls (empty retries, max-token partials, grace turns — each explicitly counted), and `accumulate_token_breakdown` (`src/harness/agent.rs:377-401`) which updates the cumulative `TokenBreakdown` **and** snapshots `last_turn_usage` (drives `last_turn_context_tokens()` → gauge). `turn_token_total` (agent.rs:1050-1060) = input+output+cache_read+cache_creation.
   `TokenBreakdown { input, output, cache_read, cache_creation, reasoning }` — `src/orchestrator/dispatch.rs:293-299`.

2. **Trace events** — `MeteringProvider` emits `LoopTraceEvent::ProviderUsage { agent_id, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, thinking_tokens }` per `process()` call (metering.rs:71-80) into the per-run `TraceSink` (gateway persistence + `OutcomeObserver` for routing experience).

3. **Extension hooks (cost meter)** — inside `HttpProvider::execute`/`stream_raw`, `PreApiRequest`/`PostApiRequest` global observer hooks fire with env `PROVIDER_NAME, MODEL, PROTOCOL, STREAMING, INPUT_TOKENS, OUTPUT_TOKENS, CACHE_*, THINKING_TOKENS, COST_USD` (http_provider.rs:404-418, :484-552, `append_usage_env` :590-605). **This channel is per-HTTP-call and carries the actual model — it already accounts heterogeneous models correctly.**

### 4.2 Cost estimation

`src/pricing.rs` — static in-repo price table, prefix-matched on canonicalised model id, with long-context tiers:
- `rate_card(provider, model) -> Option<RateCard>` (:739) — also feeds `FailoverProvider` cost-aware routing.
- `estimate(provider, model, &TokenBreakdown) -> CostEstimate { usd, status: Complete|PartialMissingPrice|Unknown, provider, model }` (:779-803).

The run-level estimate is computed **once, post-run**, in runner_impl.rs:636-645: `pricing::estimate(&provider_name, &gauge_model, &token_breakdown)` — one `(provider, model)` pair applied to the whole run's cumulative breakdown. `gauge_model` is resolved pre-loop (runner_impl.rs:196-201): pinned model, else `llm.serving_model_hint()`, else provider name.

### 4.3 Can advisor calls on different models be priced at their own rates today?

- **Run-level `FlowOutcome.estimated_cost`: NO.** Single (provider, model) × cumulative breakdown. If a MoA facade merges advisor usage into the returned `response.usage`, those tokens get priced at the *aggregator's* rate and inflate the context gauge (`context_occupancy_tokens` treats them as prompt occupancy). **Recommendation: return only the aggregator's usage on `ProviderResponse.usage`.**
- **Per-call channels: YES, already.** (a) The extension cost-meter hooks fire per HTTP call with the true `MODEL` + `COST_USD`; (b) each advisor can be wrapped in its own `MeteringProvider::new(advisor, sink, "moa-advisor:<model>")` → distinct `ProviderUsage` trace events per advisor (the subagent spawner already uses per-agent-id metering exactly this way); (c) `pricing::rate_card`/`estimate` are pure functions the facade can call per advisor with a one-call `TokenBreakdown` if it wants to attach an aggregate MoA cost figure.

---

## 5. Streaming end-to-end, and MoA feasibility

### 5.1 The pipeline

1. **Adapter**: `ProtocolAdapter::stream_deltas(response) -> BoxStream<Result<ProviderDelta>>` — every protocol is stream-first.
   `ProviderDelta` (`src/providers/delta.rs:35-89`): `TextDelta | ThinkingDelta | ThinkingSignatureDelta | ToolCallStart{id,name,signature} | ToolCallArgDelta | ToolCallArgsComplete | ToolCallEnd | Usage(TokenUsage) | Done(StopReason) | Error(String)`.
2. **HttpProvider**: `execute(payload, Option<&dyn DeltaSink>)`; `process()` = `execute(payload, None)` (http_provider.rs:608-613); `execute_streaming(payload, &dyn DeltaSink) -> ProviderResponse` (:449-455) — full pipeline (cost hooks, error promotion, truncation check, validation, inbound leak scan) **plus** live delta forwarding; `stream_raw(payload) -> BoxStream` (:461-555) — raw stream for `AiProviderBridge`/`LoopProvider` consumers.
   `DeltaSink` (delta.rs:563): `async fn on_delta(&self, delta: &ProviderDelta)`. `DeltaCollector` accumulates deltas → `ProviderResponse`. `response_to_delta_stream` (delta.rs:632) bridges a completed response into the delta-stream shape (ordering: thinking → signature → text → tool calls → usage → done).
3. **Harness**: `stream_llm_call` (think.rs:1606-1628) — only when `deps.llm.as_http_provider()` is `Some` AND no output guardrail (`may_stream_deltas`, think.rs:576). `CallbackSink` (think.rs:29-47) forwards only `TextDelta`→`callback.on_delta` and `ThinkingDelta`→`on_reasoning`; tool-call deltas are folded by the collector.
4. **Gateway**: `HarnessCallback` (`src/harness/callback.rs:21-53`: `on_delta`, `on_reasoning`, `on_tool_call_start/done`, `on_context_usage(context_tokens, total_tokens)`, `on_safety_block`, `on_complete`) → `BroadcastCallback` → `FlowStreamEvent` broadcast (`src/orchestrator/dispatch.rs:39-70`: `Delta | Reasoning | ToolCallStart | ToolCallDone | ContextGauge{context_tokens, context_window, total_tokens} | SafetyBlock | Complete(FlowOutcome)`).

### 5.2 Critical finding: live per-token streaming is already OFF on the production main-agent path

`FailoverProvider` and `ModelOverrideProvider` do **not** implement `as_http_provider` (default `None`); `MeteringProvider` merely delegates. Since the production `deps.llm` is `Metering(→[ModelOverride]→Failover)`, `as_http_provider()` is `None` → `provider_streams == false` → every turn goes through one-shot `process()`, and text reaches the panel as **one `on_delta` per turn** (think.rs emits the full turn text once). Live token-by-token streaming only occurs when `deps.llm` is a bare `HttpProvider` (or Metering-wrapped one).

### 5.3 Consequences for the MoA facade

- **Do NOT implement `as_http_provider` on the facade by forwarding the aggregator's `HttpProvider`.** think.rs:1613-1623 calls `http.execute_streaming(payload, &sink)` directly on the downcast result — this would bypass the facade's `process()` entirely, so advisors would never run. The facade must return `None` (the default).
- Returning `None` gives behavior **identical to today's production failover path**: the aggregator's answer arrives as a single per-turn `on_delta`. "Run non-streamed advisor calls, then return the aggregator's response" works trivially inside `process()` — call each advisor's `process()` under `futures::future::join_all` (respecting the outer `turn_timeout`), build the aggregation prompt, call `aggregator.process(...)`, return its `ProviderResponse` (tool_calls, stop_reason, thinking, usage all intact — the harness Act phase consumes them normally).
- If genuinely-live aggregator streaming is wanted later, the seams are: (a) have the facade internally call `aggregator.as_http_provider().execute_streaming(payload, sink)` — but no sink reaches `process()` today; (b) add an optional trait method (e.g. `process_with_sink(payload, Option<&dyn DeltaSink>)` defaulting to `process`) — a trait change mirroring `HttpProvider::execute`'s existing signature; (c) accept the no-live-delta status quo (recommended first cut; the production path already lives there).

---

## 6. Credentials / base_url / api_mode resolution (hermes `resolve_runtime_provider` equivalent)

### 6.1 Config shape

`ProviderConfig` — `src/config/types/provider.rs:74-242`. Relevant fields: `protocol: Option<String>` (adapter selection; `.protocol()` defaults `"openai"`), `api_key: Option<String>` (**runtime-only**: `#[serde(skip_serializing)]`, never persisted — "populated from encrypted vault", :79-82), `models: Vec<String>` (`default_model()` = first entry, :276), `base_url: Option<String>`, `timeout_seconds`, `stream_idle_timeout_secs`, `cache_retention`, `max_tokens/context_window/temperature/...`, `model_behavior`, `service_tier`, `effort`.

### 6.2 Credential resolution — vault key scheme `"ai:<provider_name>"`

- Canonical key fn: `provider_vault_key(name) -> format!("ai:{name}")` — `src/providers/probe.rs:30-32`.
- Boot hydration: `provider_registry.rs:30-48` — `hydrate(name, cfg)` clones the config and fills `api_key` from `shared_token_mgr.get_secret("ai:<name>")` if empty; `has_key` gates registration.
- Runtime hydration examples: config patcher healthcheck (`src/config/patcher.rs:419`), diagnostics connectivity check (`src/diagnostics/checks/providers_connectivity.rs:53`), voice (`src/gateway/voice/format.rs:126`).
- Presets fill base_url/protocol when unset: `presets::get_preset(name)` inside `create_provider` (mod.rs:181-195); `resolve_provider_from_model` maps bare model ids → preset provider names.

### 6.3 Can code instantiate a SECOND provider (different model/provider) at runtime? — YES, three proven patterns

1. **Clone-primary-swap-model** (same vendor, cheaper/different model) — `build_cheap_summary_provider` (`src/orchestrator/deps_builder.rs:878-940`): `let mut cheap_cfg = base.clone(); cheap_cfg.models = vec![summary_model]; create_provider(primary_key, cheap_cfg)`. Config clone carries the already-hydrated `api_key`/base_url/protocol/timeouts. Same pattern in `build_strategy_planner_provider` (:966+). Consumed as `AgentHarnessRunner.cheap_provider` → `ContextCompactor::with_cheap_provider` (runner_impl.rs:286-288) — **an existing production case of a second, differently-modeled provider used mid-run alongside the session's main provider.**
2. **Resolve from the pin-chain registry** (different vendor, failover for free) — `named_providers: HashMap<String, Arc<dyn AiProvider>>` holds one route-shaped `FailoverProvider` per configured provider; wrap in `ModelOverrideProvider::new(chain, model)` to pin the model — exactly what runner_impl.rs:112-122 and `select_model` do. **This is the recommended advisor-resolution path for MoA** (circuit breaker, cooldowns, tier gating inherited).
3. **Fresh on-demand build** — `probe_provider(label, resolved_config)` (`src/providers/probe.rs:37-66`) builds a throwaway provider per call from a vault-hydrated `ProviderConfig`.

"api_mode" equivalent: Aleph keys everything off `ProviderConfig.protocol` → `ProtocolRegistry` adapter (chat vs responses vs anthropic vs gemini) — no separate mode field.

---

## 7. Design-relevant checklist for the `MoaProvider` facade

- Implement `AiProvider`; **delegate `name/color/protocol/supports_native_tools/model_behavior_override/behavior_hint/serving_model_hint` to the aggregator** — `resolve_behavior(llm.as_ref())` (runner_impl.rs:331) picks the prompt behavior family from these, `serving_model_hint` keys the gauge window + pricing (runner_impl.rs:196-208), `supports_native_tools` gates tool extraction. The aggregator IS the acting model, so all identity surfaces must be its.
- Keep `as_http_provider()` = default `None` (see §5.3 — forwarding it bypasses fan-out).
- In `process()`: own the payload fields (`FailoverProvider` pattern, provider.rs:601-611), fan out advisor `process()` calls with `join_all` (fail-soft per advisor — drop failures, proceed with survivors; total failure → plain aggregator call), then call aggregator with the MoA-augmented payload. Return the aggregator's `ProviderResponse` unchanged (its `usage` keeps the gauge and pricing honest).
- Advisor accounting: wrap each advisor in `MeteringProvider::new(advisor, sink, "moa-advisor:<n>")` if per-advisor trace events are wanted; the extension cost-meter hooks fire automatically per HTTP call with correct MODEL/COST_USD.
- Insertion point: runner_impl.rs Step 3 (:112-129), between the model-directive resolution and the `MeteringProvider("root")` wrap — the same seam `ModelOverrideProvider` occupies. Advisors resolved via `named_providers` + `ModelOverrideProvider`, or `create_provider` on a vault-hydrated config clone.
- Constraint: entire fan-out + aggregation must complete inside `deps.turn_timeout` (think.rs:1569-1577) and respect cancellation (the outer `race_llm_call` select drops the whole future on cancel — advisor sub-futures are dropped with it, which is safe since `reqwest` futures cancel on drop).
- Payload note for advisors: `RequestPayload.tools` should likely be stripped (`ToolChoice::None` / `tools: None`) for advisor calls if advisors are text-consultants only; `system_blocks` (Anthropic cache split) only benefits the aggregator whose prefix repeats across turns — advisors with rewritten prompts will not cache-hit.
