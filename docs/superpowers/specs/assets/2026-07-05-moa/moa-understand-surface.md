# Aleph COMMAND + CONFIG + OBSERVABILITY surface for a hermes-style MoA port

All paths relative to `/Volumes/TBU4/Workspace/Aleph`.

## 1. Slash commands

**Key fact: in Aleph, slash commands are not a separate subsystem — they are surface sugar over tools (redline R8).** There is no per-command handler table like hermes'; `/x args` resolves against the unified tool catalog and either executes the tool directly (fast path) or falls through to a full agent run.

### Definition & parsing
- `src/command/mod.rs:1-17` — command module = completion/registry only. Sources: builtin tools, MCP tools, user prompt rules, skills. Exposed to UIs via `commands.list` (`src/gateway/handlers/commands.rs`).
- `src/command/parser.rs:89-108` — `CommandParser::parse_async(input)`: strips `/`, delegates to `ToolCatalog::resolve_command` (`src/tool_metadata/registry/mod.rs:316`), returns `ParsedCommand { source_type, command_name, tool_id, arguments, context }`.
- `src/command/parser.rs:34-70` — `CommandContext` enum: `Builtin { tool_name }` / `Mcp` / `Skill` / `Custom` / `None`.
- Builtin tools become slash commands at boot: `src/bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs:41` → `ToolCatalog::register_builtin_tools` (`src/tool_metadata/registry/mod.rs:133`), seeded from the static catalog `BUILTIN_TOOL_DEFINITIONS` (`src/executor/builtin_registry/definitions.rs:68+`).

### Routing — two entry paths
**Channel path (Telegram/Slack/...)**: `src/gateway/inbound_router/mod.rs:776-928`
- Hard intercepts before agent dispatch: `/btw` (`:779-784`), `/stop|/abort` (`:790-792`).
- Unified interception (`:794-859`): `parse_async` → per-channel access gate (`slash_command_gate`, `:808-836`) → special cases `/groupchat` (`:837`), `/new`/`session_new` (`:840`) → `serialize_parsed_command` (`src/gateway/inbound_router/command_handler.rs:55-111`) produces a **mode JSON** (`{"type":"direct_tool"|"skill"|"mcp"|"custom", ...}`) passed as run metadata into `execute_for_context_with_metadata` (`:856`).
- Fallbacks: namespace inline keyboard (`:877-903`), unknown-command suggestions (`:911`).

**Panel/CLI path**: `src/gateway/execution_engine/slash_command.rs`
- `try_resolve_slash_command` (`:43-92`): `/name args` → if `tool_registry.get_tool(name)` exists, builds `{"type":"direct_tool","tool_id":name,"args":args}` mode JSON. Shorthand aliases table `SHORTHAND_ALIASES` (`:24-30`).
- `execute_slash_command_fast_path` (`:99-173`, "L0 fast path"): `direct_tool` executes the tool immediately via `tool_registry.execute_tool` (`:203`), streams the result as `Reasoning` + final `ResponseChunk` events (`:193-240`) — **bypassing the LLM entirely**. `skill`/`mcp`/`custom` return `ExecutionError::Fallthrough` → full agent loop.
- Generic arg mapping `build_tool_arguments` (`:252-325`): per-tool field maps + `--key value` CLI parser.

### ⚠️ Critical precedent for /moa
`slash_command.rs:75-77`: `/loop` and `/goal` are **explicitly excluded from the L0 fast path** because (a) their post-run continuation hooks only fire from a full agent run, and (b) the LLM must map free-text args onto structured schemas. A `/moa on|off|preset X` toggle is a pure state write with a trivial schema — it CAN run on the fast path (instant, zero LLM cost). If MoA args are free-text, follow the /loop pattern and exclude it.

### How a command flips per-session state the next turn reads
Exemplar: `select_model` (the exact mechanism MoA-toggle needs):
1. Tool reads the ambient session from the task-local turn context: `src/builtin_tools/select_model.rs:70-82` (`current_turn_context()` → `ctx.session_key`; task-local set by the tool dispatcher, `src/tools/turn_context.rs`).
2. Writes a **process-global, session-keyed map**: `src/providers/session_model_handle.rs:30-60` — `OnceLock<RwLock<HashMap<String /*session key*/, SessionModelPref>>>` with `set_/get_/clear_session_model`. In-memory by design (soft UX state, resets on restart). Doc comment says it "mirrors `route_handle`" — this is the established idiom.
3. Read at **next run construction**: `src/orchestrator/harness_bridge/runner_impl.rs:101-122` — Step 3 "brain pick" precedence: ① session `select_model` pick → ② agent's configured `provider_hint`/`model_hint` pin → ③ flow `BrainRef` via `pick_llm`. Picks ①/② wrap the base provider in `ModelOverrideProvider` (`:118`). **This exact site is where a per-session MoA flag would swap in a `MoAProvider` wrapper.**

Second exemplar: `/loop` → `LoopRegistry` per-session loop state (`src/looping/mod.rs:52`, process-global via `init_global`/`global()` `:265-277`); tool at `src/builtin_tools/loop_manage.rs`; re-firing happens in the execution engine's post-run continuation hook (`gateway/execution_engine/execute.rs`).

### One-shot (next-turn-only) pattern
There is **no generic "apply-to-next-turn-then-auto-restore" middleware**. Two existing idioms:
- **Request-scoped override (true one-shot)**: `ModelOverride` — `src/gateway/model_override.rs:17-23` ("Both variants are **per-turn** overrides"). Panel attaches it to a single `chat.send` (`src/gateway/handlers/chat.rs:60-62,204`; `agent.rs:71-73,357`) → `RunRequest.model_override` (`src/gateway/execution_engine/mod.rs:236-241`) → applied once in `run_loop/inner.rs:478-500` (short-circuits `resolve_with_fallback`). Dies with the run; nothing to restore. A one-shot `/moa` ("MoA for next turn only") = a field on `RunRequest` / chat.send params, not session state.
- **Ephemeral session**: `/btw` runs in `SessionKey::ephemeral` — no persistence, no context pollution (`src/gateway/inbound_router/command_handler.rs:175-205`).
- Sticky-until-cleared = the session-keyed map above (select_model), with an explicit `clear` action.

## 2. Config

### config.toml → typed structs
- Root struct `Config`: `src/config/structs.rs:54-287`. Every TOML section is one serde field with `#[serde(default)]` (+ often `skip_serializing_if`); `Default` impl `:417-484`. Section types live in `src/config/types/` (one file per domain, re-exported through `types/mod.rs`, imported at `structs.rs:5-16`). All types derive `Serialize/Deserialize/JsonSchema` (schemars — feeds `config/schema.rs`).
- Loading: `src/config/load.rs:15-19` (`~/.aleph/config.toml`), `Config::load()` `:181`. Overrides in separate files: `presets_override` (`~/.aleph/presets.toml`), `prompts_override`, `defaults_override` (`structs.rs:274-286`).
- LLM-driven config editing already exists (R8): `SelfConfigTool` (`src/builtin_tools/self_config.rs:1-20`) reads/updates arbitrary config.toml sections through `ConfigPatcher` (`src/config/patcher.rs`). A new `[moa]` section is automatically manageable via it; a dedicated `moa` tool is still nicer UX.

### Where `[moa]` fits
Add `src/config/types/moa.rs` with `MoaConfig`, register in `types/mod.rs`, add field `pub moa: MoaConfig` (or `Option<MoaConfig>` per the `strategy`/`guardrails` opt-in pattern, `structs.rs:222-246`) + `Default` entry.

Analogous existing shapes:
- `[execution]` — `ExecutionConfig`, `src/config/types/execution.rs:8-101`: flat knobs, `fn default_*()` const fns, serde-missing-fields tests (`:124-137`). Best template for `[moa] enabled/max_...` style knobs.
- **Named presets of {provider, model} slots** — two direct precedents:
  - `providers: HashMap<String, ProviderConfig>` (`structs.rs:69`; `ProviderConfig` at `src/config/types/provider.rs:74-133` with `models: Vec<String>`, `protocol`, `base_url`, vault-backed `api_key` with `skip_serializing`).
  - `profiles: HashMap<String, ProfileConfig>` (`structs.rs:171`).
  So `[moa.presets.<name>]` → `presets: HashMap<String, MoaPreset>` where `MoaPreset { slots: Vec<MoaSlot> }` and `MoaSlot { provider: Option<String>, model: String }`. Note `ModelOverride::Qualified {provider, model}` / `Raw {model}` (`src/gateway/model_override.rs:37-53`) is already the typed (provider, model) pair with exactly the resolve semantics needed per slot — reuse it or mirror its shape. Slot→provider resolution precedent: `runner_impl.rs:112-121` (`named_providers.get(p)` else default chain, then stamp model via `ModelOverrideProvider`).

## 3. Per-session mutable state

- **Durable transcript** = session event log (see §5).
- **Soft cross-turn state** = process-global `OnceLock<RwLock<HashMap<session_key_string, T>>>` keyed by `SessionKey::to_key_string()` — `session_model_handle.rs` is the canonical exemplar; `route_handle` and `LoopRegistry` are siblings. Written by tools under `TURN_CONTEXT`, read at run construction in `harness_bridge/runner_impl.rs`. Intended to reset on daemon restart.
- **Model switch mid-session**: two mechanisms — (a) LLM-initiated `select_model` tool (sticky per session, "takes effect from the next turn", `select_model.rs:54-59`), paired with `list_models` discovery tool (`src/builtin_tools/list_models.rs`); (b) user-initiated per-turn picker override on chat.send (§1). There is **no `/model` slash command** per se — `select_model` being a builtin tool means `/select_model` resolves anyway via the catalog.
- Per-session run exclusivity: `SessionRunRegistry` (`src/gateway/execution_engine/session_run_registry.rs`) — one run per session at a time; MoA state writes won't race a concurrent run on the same session.

## 4. Display/events — the pattern a `moa.reference` event should follow

Two parallel pipes run from the loop to frontends:

**Pipe A — streaming callback (text/tools/gauge):**
`HarnessCallback` (`src/harness/callback.rs:21-79`: `on_delta`, `on_reasoning`, `on_tool_call_start/done`, `on_context_usage`, `on_complete_with_outcome`) → `BroadcastCallback` (`src/orchestrator/harness_bridge/callback.rs:33-119`) → `FlowStreamEvent` (`src/orchestrator/dispatch.rs:39-70`: Delta/Reasoning/ToolCallStart/Done/ContextGauge/SafetyBlock/Complete) → gateway drain `emit_flow_event` (`src/gateway/execution_engine/event_drain.rs:64+`) → `StreamEvent` (`src/gateway/event_emitter/types.rs:51-219`) → `GatewayEventFrame` (`src/gateway/events/frame.rs:23+`), which maps to bus topic (`topic_name()` `:435-473`, e.g. `agent.context.gauge`) and WS JSON-RPC notification method (`stream_method()` `:484-504`, e.g. `stream.context_gauge`) → webchat panel dispatcher `interfaces/webchat/src/platform/wide/views/chat/events.rs:336-509` (match on event type: `"context_gauge"` `:464`, `"agent_trace"` `:376`, ...).

**Pipe B — structured trace (the right one for `moa.reference`):**
`TraceSink` trait (`src/harness/trace_sink.rs:12-25`, MUST NOT block; mpsc-backed in prod) carries `LoopTraceEvent` (`src/harness/trace.rs:24-125`, `#[non_exhaustive]`, serde-tagged snake_case). Gateway wraps the persistence sink with `AgentTraceEmitSink` (`src/gateway/execution_engine/agent_trace_emit_sink.rs:60-98`) — a decorator that mirrors whitelisted variants (`is_step_event`, `:45-55`) to the wire as `StreamEvent::AgentTrace { run_id, seq, event }` (`event_emitter/types.rs:97-101`) via non-blocking mpsc + spawned drain, while always forwarding to the inner (persistence) sink. Wire form is the protocol mirror `aleph_protocol::AgentTraceEvent` (conversion `trace.rs:205-336`). Panel consumes under `"agent_trace"` (`events.rs:376-384` → `apply_trace_event`).

**Recipe for `moa.reference`:**
1. Add a `LoopTraceEvent::MoaReference { slot, provider, model, ... }` variant (enum is `#[non_exhaustive]` precisely for this; `trace.rs:12-20`) + mirror variant in `aleph_protocol::AgentTraceEvent` + `From` arm.
2. Emit it from the MoA provider wrapper through the run's `trace_sink` — the exact pattern of `MeteringProvider` (`src/providers/metering.rs:20`), which is constructed per run with `(inner_provider, trace_sink, "root")` at `harness_bridge/runner_impl.rs:127-129` and emits `LoopTraceEvent::ProviderUsage` per LLM call (`trace.rs:98-105`). This keeps `src/harness/` untouched (R10: zero new files/LOC in harness; the sink is already threaded).
3. Whitelist the new variant in `is_step_event` (`agent_trace_emit_sink.rs:45`) so it reaches the panel; add a branch in the panel's `apply_trace_event`.
4. It persists to `task_traces` for free through the inner sink (§5).

Per-LLM-call event precedent (cadence identical to per-reference): context gauge chain — `think.rs:823-825` → `callback.rs:87-98` → `dispatch.rs:61` → `event_drain.rs:164-171` → `types.rs:128` → `frame.rs:493` (`stream.context_gauge`) → panel `events.rs:464-470`. Note channel surfaces drop gauge-class events (`src/gateway/reply_emitter/emitter/streaming.rs:597`) — panel-only display is the norm for meta events.

## 5. Transcripts / where a MoA turn-trace lives

Two persistence layers:
- **Canonical conversation transcript** — append-only session event log: `SessionEvent` (`src/session/events.rs:122-293`: `UserMessage`/`AssistantMessage` (+`thinking` field on `MessageContent` `:54-61`)/`LlmCallStarted`/`LlmCallEnded`/`ToolCallRequested`/`ToolResult`/`ToolError`/`AssistantRunMeta`/...), stored in SQLite `session_events` with `(session_id, seq)` PK via `SessionEventStore` (`src/session/store.rs:37+`, includes BM25 search); tool-call trace helper `src/session/tool_trace.rs:36+` wires dispatch→events.
- **Observability run trace** — `task_traces` table in `StateDatabase`: every `LoopTraceEvent` for a run is persisted via `TracePersistence` (`src/gateway/execution_engine/callback.rs:6-13`, wired per-run at `run_loop/inner.rs:443`) fed by `GatewayTraceSink` (`trace_sink_adapter.rs:46-72`); replayed to the panel via `trace.by_runs` / `trace.list` / `trace.get` RPCs (`src/gateway/handlers/trace_replay.rs:22-47`, registered `handlers/mod.rs:566-570`) keyed by `run_id`.

**Recommendation**: an optional MoA turn-trace (full advisor inputs/outputs) belongs in `task_traces` — it is observability, not conversation. Emit as `LoopTraceEvent` variant(s) (heavy payloads can be persisted-but-not-wire-forwarded by simply not whitelisting them in `is_step_event`), and it becomes replayable through the existing `trace.by_runs` panel timeline with zero new RPC surface. Only aggregate/reference-level data should also ride the wire.

## 6. Per-turn cost/usage accounting

- **Per-LLM-call fold (harness)**: `src/harness/agent/think.rs:320-324` (`account_intermediate_tokens` — every billed round-trip counted exactly once) and `:814` (`accumulate_token_breakdown`); occupancy → `callback.on_context_usage` `:823-825` (from `TokenUsage::context_occupancy_tokens`, `harness/agent.rs:306`).
- **Per-call provider usage events**: `MeteringProvider` (`src/providers/metering.rs:20`) wraps the root provider per run (`harness_bridge/runner_impl.rs:127-129`) → `LoopTraceEvent::ProviderUsage { agent_id, input/output/cache_read/cache_creation/thinking_tokens }` (`trace.rs:98-105`), labelled `"root"` or subagent id. **For MoA: wrap each advisor slot's provider in its own `MeteringProvider` labelled e.g. `"moa:<slot>"` — per-advisor usage then flows into the existing trace/persistence/metrics machinery with zero new accounting code.**
- **Run totals**: `FlowOutcome` (`dispatch.rs:72-121`): `total_tokens`, `token_breakdown`, `estimated_cost: Option<pricing::CostEstimate>`, `context_tokens`, `context_window`, `tool_timeline` → drained into `RunSummary` at `event_drain.rs:312-358` (`RunSummary` struct `event_emitter/types.rs:358-391` incl. `estimated_cost_usd`, gauge fields) → `stream.run_complete`.
- **Cross-reload persistence of gauge**: `SessionEvent::AssistantRunMeta` (`events.rs:192-199`) emitted post-run at `execute.rs:526-548`, projected onto the assistant message row by `src/gateway/session_projector.rs:146,353`.
- **Ops metrics RPCs**: `gateway.metrics.lanes` / `gateway.metrics.run_concurrency` (`src/gateway/handlers/gateway_metrics.rs:22-39`).

## 7. Builtin tool registration (R8 route for MoA config management)

- Trait: `AlephTool` (docs `docs/reference/TOOL_SYSTEM.md:60-110`) — `const NAME/DESCRIPTION`, typed `Args: JsonSchema` / `Output`, `call()`; blanket `AlephToolDyn` for dynamic dispatch. Concrete exemplars: `select_model.rs` (52 lines of logic), `loop_manage.rs` (action-enum tool: `Start/Stop/Status/Update` — the ideal shape for a `moa` tool: `On/Off/Preset/Status`).
- Registration is a 3-touch pattern:
  1. `src/builtin_tools/moa_manage.rs` implementing `AlephTool`, re-export in `builtin_tools/mod.rs`.
  2. Catalog entry in `BUILTIN_TOOL_DEFINITIONS` (`src/executor/builtin_registry/definitions.rs:68+`; e.g. `select_model` at `:220`) — this is what makes it advertise to the LLM AND auto-appear as `/moa` in the command tree.
  3. Constructor arm in `create_tool_boxed` (`definitions.rs:908` — `"select_model" => Some(Box::new(SelectModelTool))`). Tools needing runtime deps return `None` here and are built per-call inside `BuiltinToolRegistry::execute_tool` (pattern documented `definitions.rs:922-949`).
- Progress surfacing from inside tools: `notify_tool_start` / `notify_tool_result` (`builtin_tools/mod.rs:301,314`).
- Per R8, config presets should ALSO be manageable conversationally; `SelfConfigTool` + `ConfigPatcher` already give generic section access, so a dedicated `moa` tool only needs the session-state toggle + preset selection (writing a `session_moa_handle` map à la `session_model_handle`), not TOML editing.

## Synthesis — minimal-footprint MoA mapping

| hermes MoA piece | Aleph home |
|---|---|
| `/moa` slash command | builtin `moa` tool (definitions.rs 3-touch) — auto-exposed as `/moa`; fast-path-eligible if args are structured, else exclude like `/loop` (`slash_command.rs:75-77`) |
| per-turn MoA mode flag | sticky: `session_moa_handle` map (clone of `providers/session_model_handle.rs`); one-shot: field on `RunRequest`/chat.send (clone of `model_override`) |
| config presets `{provider, model}[]` | `[moa]` section → `src/config/types/moa.rs`, `presets: HashMap<String, MoaPreset>`; slot shape mirrors `ModelOverride::Qualified` |
| activation point per turn | `harness_bridge/runner_impl.rs:80-129` Step 3 brain pick — wrap resolved provider in `MoAProvider` (like `ModelOverrideProvider`/`MeteringProvider` wrapping) — zero harness changes (R10) |
| per-reference display events | new `LoopTraceEvent` variant emitted via `trace_sink` from the provider wrapper (MeteringProvider pattern), whitelisted in `is_step_event`, panel branch in `apply_trace_event` |
| turn traces (advisor I/O) | `task_traces` via `TracePersistence`; replay via `trace.by_runs` |
| per-advisor cost | one `MeteringProvider` per slot with label `moa:<slot>` → existing `ProviderUsage` accounting |