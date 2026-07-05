# hermes-agent MOA Module — Periphery & Integration Surface

All paths relative to `/Volumes/TBU4/Github/hermes-agent`. Line numbers verified against the working tree on 2026-07-05.

---

## 1. Config schema — `hermes_cli/moa_config.py` (277 lines)

### Top-level config shape (`config.yaml` key `moa`)

```yaml
moa:
  default_preset: <name>        # preset used by /moa one-shot & bare `--provider moa`
  active_preset: <name|"">      # "" = off; validated to exist, else cleared
  save_traces: true|false       # read by agent/moa_trace.py (NOT normalized here)
  trace_dir: <path>             # optional trace dir override (also read only by moa_trace.py)
  presets:
    <preset-name>:
      enabled: true|false
      reference_models: [{provider, model}, ...]
      aggregator: {provider, model}
      reference_temperature: float|null
      aggregator_temperature: float|null
      max_tokens: int
      reference_max_tokens: int|null
      fanout: per_iteration|user_turn
```

### Per-preset fields, defaults, validation (`_normalize_preset`, lines 108–148; `_default_preset`, lines 93–105)

| Field | Default | Coercion / validation |
|---|---|---|
| `enabled` | `True` | `bool(raw.get("enabled", True))` (line 126) |
| `reference_models` | `DEFAULT_MOA_REFERENCE_MODELS` = `[{openai-codex, gpt-5.5}, {openrouter, deepseek/deepseek-v4-pro}]` (lines 13–16) | Non-list degrades: bare dict is wrapped, other types → `[]` (lines 112–117); each slot cleaned by `_clean_slot`; empty result → defaults (line 121) |
| `aggregator` | `DEFAULT_MOA_AGGREGATOR` = `{openrouter, anthropic/claude-opus-4.8}` (lines 18–21) | `_clean_slot` or default (line 123) |
| `reference_temperature` | `None` | `_coerce_float_or_none` (lines 24–37): None/""/invalid → `None` = "omit param, provider default" |
| `aggregator_temperature` | `None` | same |
| `max_tokens` | `4096` | `_coerce_int` (lines 40–49): invalid → default, accepts float strings via `int(float(v))` |
| `reference_max_tokens` | `None` (= uncapped, preserves prior behavior) | `_coerce_int_or_none` (lines 52–67): non-positive/invalid → `None`. Caps ONLY advisor output, never the acting aggregator (comment lines 132–139; latency rationale: turn latency ~0.88 correlated with advisor output tokens) |
| `fanout` | `"per_iteration"` | `_coerce_fanout` (lines 70–73): only `{"per_iteration","user_turn"}` allowed, else default. `per_iteration` = advisors re-run every tool iteration; `user_turn` = advisors run once per user turn (original MoA shape), comment lines 141–147 |

### Slot validation (`_clean_slot`, lines 76–90)
- Slot must be a dict with non-empty `provider` AND `model` (whitespace-stripped), else dropped (→ falls back to preset defaults).
- **Recursion guard**: `provider == "moa"` (case-insensitive) is rejected at save/normalize time so a MoA preset can never be a reference or aggregator slot of another MoA preset (lines 83–90). Runtime guards in `moa_loop.py` exist too but only surface mid-turn; this makes the invalid slot unsaveable.

### Preset resolution/naming (`normalize_moa_config`, lines 151–196)
- `presets` dict: keys stripped; empty-name presets dropped (lines 160–166).
- **Legacy compat**: if no `presets`, the flat top-level `moa` dict itself becomes preset `"default"` (`DEFAULT_MOA_PRESET_NAME`, line 11) (lines 168–170).
- `default_preset`: must exist among presets, else falls back to first preset key; if still missing, a full `_default_preset()` is synthesized (lines 172–176).
- `active_preset`: cleared to `""` unless it names an existing preset (lines 178–180).
- Returns BOTH the presets map and a **flattened compatibility view** of the *default* preset at top level (`reference_models`, `aggregator`, temperatures, `max_tokens`, `reference_max_tokens`, `fanout`, `enabled`) for dashboard/desktop callers (lines 182–196).

### Other API
- `list_moa_presets(config)` → preset names (lines 199–201).
- `resolve_moa_preset(config, name=None)` → deep copy of preset; name defaults to `default_preset`; raises `KeyError` on unknown (lines 204–210).
- `exact_moa_preset_name(config, text)` (lines 213–233): exact-match only, and **only for `enabled: true` presets**. Used by the implicit `/model <preset>` PATH B switch (see §4.8); a disabled preset must not hijack a plain model switch whose name collides (issue #55187). Explicit `--provider moa` still reaches disabled presets.
- `set_active_moa_preset(config, name)` (lines 236–242): validates name exists (KeyError otherwise); `""`/None clears.
- **One-shot marker codec** (lines 245–272): `encode_moa_turn(prompt, config, preset)` → `"__HERMES_MOA_TURN_V1__" + urlsafe_b64(JSON{prompt, config: resolved preset})` (marker constant `MOA_MARKER_PREFIX`, line 10). `decode_moa_turn(message)` → `(prompt, normalized_preset|None)`; non-marker or corrupt payload returns the message unchanged with `None`. `build_moa_turn_prompt` is a thin alias (lines 270–272). This is the legacy "frontends that can only send text" path; still decoded in the conversation loop (§4.4).
- `moa_usage()` (lines 275–276): usage string — `/moa <prompt>` runs one prompt through default preset then restores; session switch is via the model picker.

---

## 2. CLI command surface — `hermes_cli/moa_cmd.py` (135 lines)

This file is the **top-level `hermes moa` shell command** (NOT the `/moa` slash command — that lives in cli.py/gateway/tui_gateway, §4.3). Wired in `hermes_cli/main.py:12719–12732`:

- `hermes moa list` (alias `ls`, also the default with no subcommand) → `_print_config` (lines 63–76): prints default preset, `active_preset` (or `(off)`), and each preset's reference slots + aggregator; `*` marks the default.
- `hermes moa configure [name]` (alias `config`) (lines 88–114): interactive loop — picks ≥1 reference slots then one aggregator via `_pick_slot` (curses radiolist with plain-stdin fallback, `_prompt_choice` lines 12–26); provider/model options come from `build_models_payload(load_picker_context(), include_unconfigured=True, ..., max_models=200)` (lines 29–41). Saves via `cfg["moa"] = normalize_moa_config(moa)`; `moa.setdefault("default_preset", preset_name)` makes the first configured preset the default. Note: configure only edits slots — temperatures/max_tokens/fanout/save_traces are config-file-only knobs.
- `hermes moa delete <name>` (alias `rm`) (lines 116–133): refuses to delete the last preset; if the deleted preset was `default_preset`, first remaining preset becomes default; clears `active_preset` if it pointed at the deleted one.
- Unknown subcommand → `SystemExit` (line 135).
- `RuntimeError("No configured model providers found. Run `hermes model` first.")` if picker context has no providers (line 46).

Blocked in Hermes Console (hosted): `hermes_cli/console_engine.py:1318` (`moa` in `blocked_top`). On Slack, `/moa` slash mode is gated behind `/hermes moa` (`hermes_cli/commands.py:1166`, `_SLACK_VIA_HERMES_ONLY = {"credits","billing","moa","debug"}`). Slash command definition: `hermes_cli/commands.py:116` — `CommandDef("moa", "Run one prompt through the default Mixture of Agents preset, then restore your model", "Session", ...)`.

### `/moa` slash semantics (uniform across all three frontends)
`/moa` is **one-shot sugar only**: `/moa <prompt>` runs that single prompt through the **default** preset, then restores the prior model. Bare `/moa` prints `moa_usage()` and changes nothing. The argument is ALWAYS a prompt — even if it exactly matches a preset name (pinned by `tests/cli/test_moa_command.py::test_moa_arg_is_always_one_shot_prompt`). **Persistent** switching is done via the model picker / `/model <preset>` / `/model <preset> --provider moa`, where MoA appears as a virtual provider (§4.7–4.8).

---

## 3. Trace format — `agent/moa_trace.py` (167 lines)

- **Gate**: opt-in via config `moa.save_traces` (read lazily per cache-MISS turn, `_traces_enabled_and_dir` lines 37–57). Off by default; when off the only overhead is a config read. `moa.trace_dir` overrides the location.
- **Location**: `<hermes_home>/moa-traces/<sanitized_session_id>.jsonl` — one JSON line appended per MoA turn that actually ran the reference fan-out (a cache MISS in `MoAChatCompletions.create`). Session id sanitized to `[alnum-_.]` (lines 60–64); missing id → `"unknown-session"`.
- **Not conversation history**: explicitly a side-channel; never enters the messages table or replay (module docstring lines 13–17) because advisory side-calls would corrupt role alternation.
- **Record shape** (`save_moa_turn`, lines 97–167):
```json
{
  "ts": <epoch float>,
  "session_id": ...,
  "preset": <preset name>,
  "references": [            // one per reference slot, from _RefAccounting via _slot_trace (lines 67–94)
    {"label", "model", "provider", "temperature",
     "input_messages": <FULL messages array the reference received (system+advisory view)>,
     "output": <full reference output>,
     "usage": {input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens},
     "cost_usd", "cost_status", "cost_source"}
  ],
  "aggregator": {
    "label", "model", "provider", "temperature",
    "input_messages": <exact aggregator messages incl. injected reference-context guidance block>,
    "output": <aggregator text or null>,
    "streamed": bool,
    "output_location": "inline" | "inline_from_stream" | "assistant_message_in_session_db"
  }
}
```
- **`output_location` tri-state** (lines 130–138, 154–162): non-streaming aggregator → captured inline (`"inline"`); streaming → captured after the fact from the caller's resolved assistant text passed as `aggregator_output_fallback` through `consume_and_save_trace` (`"inline_from_stream"`); if that resolved text was unavailable → `null` output pointing at the session DB (`"assistant_message_in_session_db"`).
- **Best-effort**: any write failure is logged at debug and swallowed — tracing must never break a live turn (lines 166–167). `json.dumps(..., default=str)` guards non-serializable objects.

---

## 4. Integration points (exact anchors)

### 4.1 MoAClient instantiation — two sites only
1. **Fresh agent build** — `agent/agent_init.py:722–770`: `elif agent.provider == "moa":` → pins `agent.api_mode = "chat_completions"` (line 724), installs the `_moa_reference_relay` closure (lines 733–760) that forwards facade events `moa.reference` / `moa.aggregating` into `agent.tool_progress_callback` (adding kwargs `moa_index`/`moa_count` and `moa_ref_count`), then `agent.client = MoAClient(agent.model or "default", reference_callback=_moa_reference_relay)` (lines 762–765). Sets `api_key = "moa-virtual-provider"`, `base_url = "moa://local"` (lines 767–768). **`agent.model` carries the preset name** — the MoAClient facade resolves the preset from config itself.
2. **Live in-place switch** — `agent/agent_runtime_helpers.py:1801–1819` (`switch_model`): same pinning; `agent.client = MoAClient(agent.model or "default")` (line 1819) — **NOTE: no `reference_callback` on this path** (the relay only exists on the init path; switch-path facade uses whatever default callback wiring MoAClient has). Comment block (lines 1804–1814) documents why `api_mode` MUST be re-pinned to `chat_completions`: `determine_api_mode("moa", ...)` may leave the aggregator's transport (`codex_responses`/`anthropic_messages`), which would dispatch `client.responses.create` against the facade (which has no `.responses`) → 404 on `moa://local` → silent fallback to a reference model (issues #54259/#54669).

Virtual-provider plumbing elsewhere:
- `run_agent.py:4037–4038` (`_create_request_openai_client`): `if self.provider == "moa": return primary_client` — never builds a per-request HTTP client for the facade.
- `agent/chat_completion_helpers.py:281–283`: same rule on the chat-completions call path ("Do not rebuild a request-local client").
- `run_agent.py:5711–5734` (`run_conversation` forwarder): threads the optional `moa_config` kwarg into `agent.conversation_loop.run_conversation`.

### 4.2 Gateway `/moa` one-shot + restore — `gateway/run.py`
- **Busy guard**: `gateway/run.py:9108–9109` — if an agent is running, `/moa` is rejected: "Agent is running — wait or /stop first, then run /moa."
- **Flip**: `gateway/run.py:9615–9648`. Bare `/moa` → `moa_usage()` (line 9628). Otherwise: `event.text = <prompt>`; saves the prior per-session override into `event._moa_restore_override = self._session_model_overrides.get(_quick_key)` (line 9637); installs `self._session_model_overrides[_quick_key] = {"provider":"moa","model":<default_preset>,"base_url":"moa://local","api_key":"moa-virtual-provider","api_mode":"chat_completions"}` (lines 9638–9644); `self._evict_cached_agent(_quick_key)` (line 9645) so the next agent build sees the override and constructs a MoAClient via agent_init; sets `event._moa_disable_after_turn = True` (line 9646). Then **falls through** to the normal message path (the prompt runs as an ordinary turn on the MoA-overridden agent).
- **Restore**: `gateway/run.py:9919–9928` — `self._restore_moa_one_shot(event, _quick_key)` runs in the message handler's `finally`, guaranteeing revert on success, exception, AND interrupt (comment: restore data lives on the per-turn event object; a try-block restore would leak the override permanently on a raising turn — every later message would silently fan out through MoA). Helper `_restore_moa_one_shot` at `gateway/run.py:9938–9957`: no-op unless `event._moa_disable_after_turn`; `_moa_restore_override is None` → `pop` the override entirely, else reinstate it; then `_evict_cached_agent(quick_key)` again so the following turn rebuilds on the restored model.

### 4.3 TUI gateway `/moa` one-shot + restore — `tui_gateway/server.py`
- Command list registration: line 11195. Handler: `tui_gateway/server.py:11479–11537`. Bare arg → `_err(4004, moa_usage())`; no session → `_err(4001)`. Saves `session["moa_one_shot_restore"] = {"override": session.get("model_override"), "model": agent.model, "provider": agent.provider}` (lines 11499–11503). Two branches:
  - **Live agent** (lines 11504–11516): real in-place switch via `_apply_model_switch(sid, session, f"{preset} --provider moa", confirm_expensive_model=False, pin_session_override=True)` — #53444: setting `session["model_override"]` alone never switched an already-built agent. Failure pops the restore stash and errors `5030`.
  - **No agent yet** (lines 11517–11527): just sets `session["model_override"]` to the moa virtual-provider dict; consumed by the first lazy build.
  - Returns `{"type":"send", "notice": "MoA one-shot queued with preset ...; previous model will be restored after this turn.", "message": arg}` — the client then sends the prompt as a normal turn.
- **Restore** (lines 8626–8663), immediately after `agent.run_conversation(...)` returns: pops `moa_one_shot_restore`; restores `session["model_override"]` (pop if prior was None); and because the one-shot did a real `switch_model`, undoing it goes back **through `_apply_model_switch`** with the recorded `model --provider provider` (resetting the override alone would leave the live client pinned to MoA). Restore failure only logs a warning. (Note: unlike the gateway's `finally`, this restore sits on the normal return path of the runner function.)

### 4.4 Interactive CLI `/moa` one-shot — `cli.py`
- Flip: `cli.py:8758–8793`. Stashes the full identity (`requested_provider/provider/model/api_key/base_url/api_mode`) into `self._pending_moa_restore_model` (lines 8776–8783), sets `provider="moa"`, `model=<default_preset>`, `api_key="moa-virtual-provider"`, `base_url="moa://local"`, `api_mode="chat_completions"`, **`self.agent = None`** (forces rebuild via agent_init → MoAClient), `_pending_moa_disable_after_turn = True`, and queues the prompt as `_pending_agent_seed` (line 8792).
- Restore: `cli.py:12206–12213` right after `run_conversation` returns — copies non-None stashed keys back onto self and sets `self.agent = None` again.
- Legacy marker path: `cli.py:12193–12204` passes `_pending_moa_config` (if any) as `moa_config=` into `run_conversation`; and `agent/conversation_loop.py:550–561` decodes the `__HERMES_MOA_TURN_V1__` marker when `moa_config is None` — replacing `user_message` with the decoded prompt and setting `persist_user_message` so the marker never reaches transcripts. Pinned by `tests/cli/test_moa_command.py::test_decode_legacy_encoded_moa_turn_still_works`.

### 4.5 Non-virtual-provider fan-out injection — `agent/conversation_loop.py:848–869`
When `moa_config` (dict) is passed to `run_conversation` (the legacy/one-shot-marker path, NOT the virtual-provider path), the loop calls `aggregate_moa_context(...)` from `agent/moa_loop.py` with the preset's slots/temperatures/`reference_max_tokens`, and appends the returned advisory context onto the **last user message** content. Failure → warning log, turn proceeds without MoA.

### 4.6 Display events (moa.reference / moa.aggregating)
- Source: MoAClient facade emits; relayed via `agent.tool_progress_callback` by the `agent_init.py:733–760` closure.
- **TUI gateway** — `tui_gateway/server.py:3446–3463` (`_on_tool_progress`): `moa.reference` → `_emit("moa.reference", sid, {"label", "text", ["index"], ["count"]})` (index/count only when `moa_index`/`moa_count` kwargs present); `moa.aggregating` → `_emit("moa.aggregating", sid, {"aggregator": name})`. Gated by `_tool_progress_enabled(sid)` (line 3433). Rendered by the Ink/desktop client as labelled thinking-style blocks before the aggregator's answer.
- **Interactive CLI** — `cli.py:10815–10838`: `moa.reference` prints a dim `┊ ◇ Reference i/n — <label>` header + the text via the reasoning-preview helper; `moa.aggregating` sets spinner text `◆ aggregating (<aggregator>)`. Both display-only, never enter history.
- **Gateway** (chat platforms): no dedicated renderer found under `gateway/` — the events flow through the generic tool-progress channel (references reach platform surfaces only where a progress consumer exists).

### 4.7 Cost/usage accounting — `agent/conversation_loop.py:1942–2069`
- After each response with usage: `aggregator_usage = canonical_usage` retained separately (line 1953) so advisor tokens are **not priced at the aggregator's rate**.
- `consume_reference_usage()` duck-typed off `agent.client` (`hasattr` check, line 1961): returns `(_ref_usage: CanonicalUsage|None, _moa_ref_cost: float|None)`; the token buckets are folded into the turn's *reported* counts (`canonical_usage + _ref_usage`, line 1965) so advisor spend (usually the bulk of the turn) is visible; consume-semantics clear the accumulator once per turn. Defensive try/except → debug log.
- Trace flush hook (lines 1976–1986): `consume_and_save_trace(agent.session_id, aggregator_output_fallback=<agent._current_streamed_assistant_text or None>)` — no-op for non-MoA clients / when tracing off.
- **Aggregator pricing via `last_aggregator_slot`** (lines 2038–2062): on the MoA path `agent.model/provider` are the virtual preset name + `"moa"` with no pricing entry, so `estimate_usage_cost` would return None and silently drop the aggregator's own spend (~50% undercount). Reads `_moa_client.last_aggregator_slot` (`{model, provider, [base_url]}` populated by `MoAChatCompletions.create`, moa_loop.py:799) and prices `aggregator_usage` at the REAL model/provider/base_url. Advisor cost `_moa_ref_cost` (already priced per-advisor at each advisor's OWN rate inside the facade) is added on top (lines 2065–2069). MoAClient property mirrors: `agent/moa_loop.py:1031–1045` (`consume_reference_usage`, `last_aggregator_slot` delegate to `chat.completions`).

### 4.8 Model picker / model switch integration
- Virtual provider row builder: `hermes_cli/inventory.py:445–473` `_moa_provider_row` — `{"slug":"moa","name":"Mixture of Agents","models": <preset names>, "source":"virtual","authenticated":True,"auth_type":"virtual","warning":"Aggregator acts as the selected model; references provide analysis before each call."}`; returns None when no presets. Prepended to the inventory rows at `inventory.py:166–168`; excluded from unconfigured-append dedup at line 216.
- Picker plumbing: `hermes_cli/model_switch.py:2318–2333` `_prepend_moa_picker_provider` + `list_picker_providers(include_moa=...)` (lines 2345–2377). Gateway model picker passes `include_moa=True` at `gateway/slash_commands.py:1474`.
- `switch_model` resolution: explicit `--provider moa` with no model defaults to the config `default_preset` (`model_switch.py:852–857`). **PATH B** (no explicit provider, `model_switch.py:938–962`): `exact_moa_preset_name` check runs BEFORE alias resolution — a bare `/model <preset>` that exactly matches an *enabled* preset pivots to `target_provider="moa"` (`resolved_moa_preset=True` skips alias/fallback steps).
- CLI `hermes model` interactive flow: selecting the `moa` provider routes to `_model_flow_moa` (`hermes_cli/main.py:623`, `3074–3075`).

### 4.9 Aggregator-identity resolution for infrastructure
- **Context length**: `agent/model_metadata.py:1922–1947` — provider `"moa"` means `model` is a preset name; resolves the preset's aggregator slot (when its provider isn't itself `moa`) and continues context-length lookup against the real aggregator model.
- **Auxiliary client**: `agent/auxiliary_client.py:3801–3829` — aux-client resolution for `main_provider == "moa"` resolves the preset to its aggregator slot (real provider+model), explicitly discarding the virtual `moa://local` base_url / placeholder key.
- `hermes_state.py`: **zero MoA references** (grep confirmed) — MoA holds no daemon-level state; all one-shot state lives on per-turn event objects / session dicts / CLI instance attrs.

---

## 5. Behavioral contract pinned by tests

### tests/hermes_cli/test_moa_config.py (283 lines, 20 tests) — config invariants
- Empty/absent config normalizes to a single `default` preset with the built-in slots; flattened top-level view mirrors the default preset.
- Named presets preserved verbatim; legacy flat config becomes the `default` preset.
- Tolerance: non-numeric temperature/max_tokens values, non-list `reference_models`, bare-dict `reference_models` (wrapped), numeric strings and float strings all coerce without crashing.
- Preset name matching is exact, never fuzzy; `exact_moa_preset_name` skips `enabled: false` presets but allows enabled ones (implicit `/model` switch safety, #55187).
- `set_active_moa_preset` validates existence (KeyError on unknown; empty clears).
- `resolve_moa_preset` returns the requested named model set; `build_moa_turn_prompt` encodes a decodable one-shot payload for a named preset.
- `provider: moa` slots rejected at normalize time — as reference, as aggregator, and case-insensitively (falls back to defaults).
- `reference_max_tokens`: defaults None (uncapped), positive value preserved, invalid → None, numeric string coerced, present in flattened view.

### tests/cli/test_moa_command.py (82 lines, 4 tests) — `/moa` slash semantics
- Bare `/moa` = usage only; no provider switch, no seed, no disable-after-turn flag.
- Any argument — **even one equal to a preset name** — is a one-shot prompt through the DEFAULT preset (`provider="moa"`, `model="default"`, `_pending_moa_disable_after_turn=True`, prompt in `_pending_agent_seed`).
- `_pending_moa_restore_model` records the pre-MoA identity (provider ≠ "moa").
- Legacy `__HERMES_MOA_TURN_V1__` encoded turns still decode to `(prompt, normalized preset config)`.

### tests/gateway/test_moa_one_shot_restore.py (103 lines, 4 tests) — gateway restore
Exercises the real `GatewayRunner._restore_moa_one_shot`:
- One-shot turn restores the prior per-session model override, and evicts the cached agent.
- Prior override `None` → the MoA override is removed outright (not replaced).
- Non-one-shot turns (no `_moa_disable_after_turn`) touch nothing and evict nothing.
- The restore fires from a `finally` — a raising turn still reverts the override (the regression: restore in `try` leaked the MoA override permanently).

### tests/run_agent/test_moa_loop_mode.py (1061 lines, 27 tests) — core loop contract
Routing/identity:
- Virtual provider: the **aggregator is the actor** (its response is the turn's answer); runtime provider resolution uses the virtual endpoint (`moa://local`).
- Reference + aggregator slots are called via their provider's REAL runtime (routed through `resolve_runtime_provider`); codex slots keep codex identity (not demoted to custom chat-completions endpoints); provider-backed slots survive aux resolution; anthropic OAuth slots keep the provider branch; a slot whose provider can't resolve still attempts the call with fallback runtime.
- MoA must NOT inject an output cap on reference or aggregator calls (unless `reference_max_tokens` configured).
Advisory view construction:
- Reference messages: system prompt dropped, tool calls + results rendered as text; view never ends on an assistant turn (no prefill); large tool results truncated head+tail; a fresh user turn ends on that user message; each reference call gets the advisory-role system prompt prepended; facade passes trimmed messages to references.
Fan-out behavior:
- `enabled: false` preset skips references entirely; references run **in parallel** (delegate-batch semantics); facade emits each `moa.reference` then one `moa.aggregating`; the reference cache is invalidated (advisors re-run) on a new tool result AND on a genuinely new user turn.
Accounting:
- Each reference captures per-advisor `CanonicalUsage` + priced cost; `create()` sums advisor usage+cost once per turn and `consume_reference_usage` clears it; `CanonicalUsage.__add__` sums per bucket.
Guidance placement:
- In an agentic tool loop the reference guidance block lands at the END of the prompt; in plain chat it merges into the trailing user message (still at end).
Tracing:
- `save_traces` on → full turn (references + aggregator I/O) written to JSONL; off (default) → nothing written.

### tests/run_agent/test_moa_streaming.py (221 lines, 7 tests) — streaming contract
- `create(stream=True)` runs references first, then returns the aggregator's RAW streaming iterator (acting model output streams to the user); forwards `stream_read_timeout` and respects caller `stream_options`; does not forward the timeout when not streaming; `stream=False` path stays byte-identical to the original.
- `call_llm` stream mode returns the raw stream and skips response validation; non-stream still validates.

### tests/agent/test_moa_aggregator_cost_slot.py (100 lines, 2 tests)
- After `create()`, `last_aggregator_slot` carries the REAL aggregator model/provider (never the virtual preset name); MoAClient exposes it (property delegation). Regression guard for the ~50% cost undercount.

### tests/agent/test_moa_slot_api_mode.py (75 lines, 4 tests) — issue #54379
- `_slot_runtime` propagates the resolved `api_mode` into `call_llm` (e.g. Copilot GPT-5.x → `codex_responses`); omits it when absent/empty; `call_llm` accepts the `api_mode` kwarg.

### tests/agent/test_moa_switch_api_mode.py (84 lines, parametrized) — issues #54259/#54669
- `switch_model` to `provider=moa` pins `agent.api_mode = "chat_completions"` regardless of incoming transport (`codex_responses`/`anthropic_messages`/`chat_completions`/empty), so the primary call always dispatches through `MoAClient.chat.completions` and never `.responses.create` against `moa://local`.

### tests/agent/test_moa_trace_streamed_capture.py (155 lines, 5 tests)
- Streamed aggregator output captured from `aggregator_output_fallback` → `output_location: "inline_from_stream"`; non-streaming prefers the inline capture over fallback (`"inline"`); streamed with no fallback → `output: null`, `"assistant_message_in_session_db"`; empty-string fallback treated as missing; `_pending_trace` cleared after flush (no duplicate records). Real file I/O against temp HERMES_HOME.

### tests/tui_gateway/test_moa_reference_emit.py (98 lines, 3 tests)
- `_on_tool_progress("moa.reference", label, text, None, moa_index=i, moa_count=n)` → exactly one `_emit("moa.reference", sid, {label, text, index, count})`; without index kwargs the `index`/`count` keys are omitted (not null); `moa.aggregating` relays `{"aggregator": <label>}`.

---

## 6. Design-relevant observations (raw notes)

- **Two MoA activation shapes coexist**: (a) the *virtual provider* (`provider="moa"`, model=preset name, MoAClient facade owns fan-out inside `chat.completions.create`) — the modern path used by picker switches and all three `/moa` one-shot implementations; (b) the *moa_config kwarg / encoded-marker* path (`run_conversation(moa_config=...)` → `aggregate_moa_context` appended to the last user message, conversation_loop.py:848–869) — legacy, still decoded for backward compat.
- **One-shot restore is implemented three times with three different mechanisms**: gateway = per-session override map + event attrs + `finally`; TUI gateway = session dict stash + real in-place switch/unswitch (no `finally` — restore is on the normal return path with warn-only failure); CLI = instance-attribute stash + agent teardown. All three converge on the same user-visible contract (default preset, restore after turn) but the leak-proofness guarantees differ (gateway strongest).
- Duck-typed integration seam: conversation loop discovers MoA purely via `hasattr(client, "consume_reference_usage")` / `consume_and_save_trace` / `getattr(client, "last_aggregator_slot")` — no imports of moa_loop on the hot path.
- The recursion ban (`provider != "moa"` in slots) is enforced at THREE layers: config normalize (`_clean_slot`), runtime skip/raise in moa_loop, and aggregator-identity resolvers checking `agg_provider.lower() != "moa"` (model_metadata.py:1938, auxiliary_client.py:3818).
- `save_traces`/`trace_dir` are deliberately NOT part of `normalize_moa_config`'s output — read raw by `moa_trace.py` only.