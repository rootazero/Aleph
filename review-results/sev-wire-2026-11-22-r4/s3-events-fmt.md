# Severed-Wire Audit — Round 4, Slice S3: gateway events / formatter / surface

- **Repo:** `/home/zou/data/workspace/Aleph` (branch `main`, HEAD `2969ce7aa`)
- **Date:** 2026-11-22
- **Mode:** STATIC audit of CURRENT on-disk code. **No repository file was modified.**
- **Slice:** `src/gateway/event_emitter/**`, `src/gateway/events/**`, `src/gateway/reply_emitter/**`, `src/gateway/formatter/**`, `src/gateway/surface/**`, `src/gateway/event_bus.rs`, `src/gateway/event_scope.rs`, `src/gateway/event_visibility.rs`, `src/gateway/capability_events.rs`, `src/gateway/capability_ui.rs` + the client/projection side (`interfaces/webchat/`, `desktop/shell/`, `shared/`).

## Round-3 cross-check (no overlap)

`git diff --stat 3bba257c7~1..2969ce7aa` restricted to the slice is **empty** — round 3 landed no change in this slice, so every finding below is "what round 3 missed".
Round 3 deleted `src/gateway/interfaces/feishu/feishu_inbound/events.rs` — a Feishu WebSocket frame parser, **not** a `GatewayEventFrame` producer, so no producer was lost there.
Round-3 report §4 (`docs/superpowers/runs/2026-11-21-severed-wire-audit-extension-fetch-gateway.md:161-190`) already-fixed list contains no item in this slice's scope.

---

## Summary table

| ID | Form | Area | Disposition | Confidence |
|----|------|------|-------------|-----------|
| W-01 | 5 | Panel activity feed topic vocabulary `agent.*` vs `run.*` | CONNECT | high |
| W-02 | 1 | 4 `channel.*` `GatewayEventFrame` variants have no producer | CUT | high |
| W-03 | 1 | 2 `pairing.*` `GatewayEventFrame` variants have no producer | CONNECT | high |
| W-04 | 1 | `EventEmitter::emit_run_complete` — zero callers repo-wide | CUT | high |
| W-05 | 1 | `emit_tool_start/update/end`, `emit_reasoning` — test-only callers | CUT | high |
| W-06 | 3 | `ReplyEmitterConfig::buffer_threshold` — dead knob | CUT | high |
| W-07 | 3 | `ReplyEmitterConfig::voice_enabled` — written, never read | CUT | high |
| W-08 | 1 | `MarkupFormat::DiscordMarkdown` / `markdown_to_discord` — no production consumer | CUT | high |
| W-09 | 1 | `alerts.**` topic family — Panel subscribes, no core producer | DECIDE | medium |
| W-10 | 5 | `node.connected` — Panel arm + doc, no producer | CONNECT | high |
| W-11 | 1 | `capability.event` / `capability.event.status` — produced, zero client consumer | DARK | medium |
| W-12 | 3 | `event_scope.rs` `"guest."` prefix rule guards a topic family that has no producer | CUT | high |

**Counts:** CONNECT 3 · CUT 6 · DECIDE 1 · DARK 1 → **12 candidates**.

---

### W-01 — Panel activity feed matches a topic vocabulary that the core never publishes
- **Form:** 5 (name / path drift)
- **Producer:** `src/gateway/events/frame.rs:846-868` — `GatewayEventFrame::stream_method()`; the doc contract at `src/gateway/events/frame.rs:841-843` states: *"The frontend subscribes to `stream.*` and converts the method prefix from `stream.` to `run.` for internal event dispatch."* The core publishes stream frames as JSON-RPC notifications whose `method` is `stream.<x>` (`src/gateway/event_bus.rs:445` uses `frame.topic_name()` **only in the non-stream branch**), so the client-visible topic is `run.<x>`.
- **Consumer:** `interfaces/webchat/src/platform/wide/views/home.rs:35-38`, `:51`, `:55`, `:89`, `:92` — `classify_topic` / `summarize_event` match the literals `"agent.response.chunk"`, `"agent.reasoning"`, `"agent.tool.start"`, `"agent.tool.update"`, `"agent.run.error"`, `"agent.run.complete"`. Also `interfaces/webchat/src/platform/wide/views/home.rs:77` matches `"session.updated"`, and `interfaces/webchat/src/components/dashboard_sidebar.rs:24` uses `alert_key="agent.trace"`.
- **Live caller search:**
  - `rg -n --glob '*.rs' -o '"agent\.[a-z_.]*"' interfaces/webchat/src/` → exactly 9 hits, all in `home.rs` (35,36,37,38,51,55,89,92) + `dashboard_sidebar.rs:24`.
  - `rg -n --glob '*.rs' -o '"run\.[a-z_.]*"' interfaces/webchat/src/` → `context.rs:52`, `chat/events.rs:984` (`"run.run_accepted"`), `chat_sidebar.rs:953,988`, `agent_trace.rs:70` (`"run.agent_trace"`), `usage.rs:122`.
  - `rg -n --glob '*.rs' 'topic_name\(\)' src/` → only `event_bus.rs:445` (non-stream branch), `server/handler.rs` (tests), `event_visibility.rs:3306` (test). **No producer ever emits an `agent.*` topic.**
- **Symptom:** the dashboard activity feed never shows a run failure as `Danger` and never shows a run completion as `Success` — both fall through to `_ => Some(ActivitySeverity::Info)`. Worse, the deliberate chatter-suppression filter (the four `None` arms meant to drop per-chunk reasoning/tool bursts) **does not fire**, so `run.response_chunk` / `run.reasoning` / `run.tool_start` / `run.tool_update` rows land in the 30-row ring buffer and push real events out — exactly the failure the doc comment at `home.rs:30-33` says the filter exists to prevent. `home.rs:217`'s own comment ("streaming chatter is dropped by `classify_topic`") is now false.
- **Hypothesis:** **CONNECT** — change the Panel literals to the `run.*` vocabulary the protocol doc already mandates (`run.response_chunk`, `run.reasoning`, `run.tool_start`, `run.tool_update`, `run.run_error`, `run.run_complete`, `run.session_updated`); the core side needs no change.
- **Confidence:** high

---

### W-02 — The four `channel.*` frame variants have no producer anywhere
- **Form:** 1 (no consumer of a produced symbol ⇒ here: no producer of a declared symbol)
- **Producer:** `src/gateway/events/frame.rs:254` `ChannelMessage`, `:259` `ChannelTyping`, `:263` `ChannelStatusChanged`, `:267` `ChannelError` (declaration) + their `topic_name()` arms at `src/gateway/events/frame.rs:801-804`.
- **Consumer:** `interfaces/webchat/src/platform/wide/views/home.rs:42` (`"channel.typing"`→`None`), `:51` (`"channel.error"`→`Danger`), `:80` (`"channel.message"`→summary), `:83` (`"channel.status"`→summary); `src/gateway/event_visibility.rs:674-680` classifies them; `src/gateway/event_scope.rs` has no rule for `channel.`.
- **Live caller search:**
  - `rg -n --glob '*.rs' 'GatewayEventFrame::ChannelError' src/ interfaces/ shared/` → `src/gateway/event_visibility.rs:3494` (match arm) and `:3728` (test). No construction site.
  - `rg -n --glob '*.rs' -F 'GatewayEventFrame::ChannelMessage' …` → only the enum decl, `topic_name()`, visibility match arms and `frame.rs` tests.
  - `rg -n --glob '*.rs' -F '"channel.message"' src/` → no `publish*` site.
  - `crate::gateway::channel::ChannelError` (a *different* type in `src/gateway/channel/`) is the only `ChannelError` construction in the tree — it is **not** this frame.
- **Symptom:** four typed variants of the "Unified Typed Channel Event Bus" are permanently dark: the Panel's channel-activity rows never appear, and the `channel.error` → `Danger` severity never fires. The bus looks complete on the type side and silently drops the whole channel plane.
- **Hypothesis:** **CUT** — the channel interfaces deliver outbound directly (`src/gateway/interfaces/*/message_ops.rs`), so a *bus* projection of channel traffic has no defined source; either (a) delete the 4 variants + the Panel arms, or (b) add a producer in the inbound/outbound channel path. If no channel-plane producer is planned, CUT; recommend CUT unless the channel-plane product intent exists.
- **Confidence:** high

---

### W-03 — The two `pairing.*` frame variants have no producer; `handlers/pairing.rs` has no event bus at all
- **Form:** 1
- **Producer:** `src/gateway/events/frame.rs:275` `PairingRequested`, `:278` `PairingCompleted` (declaration) + `topic_name()` arms `src/gateway/events/frame.rs:806-807`.
- **Consumer:** `interfaces/webchat/src/platform/wide/views/home.rs:53` (`"pairing.requested"`→`Warning`), `:55` (`"pairing.completed"`→`Success`), `:101-102` (summaries); `src/gateway/event_visibility.rs:674` classifies them as `Global`; `src/gateway/event_scope.rs:108` declares the operator-only `"pairing."` prefix rule.
- **Live caller search:**
  - `rg -n --glob '*.rs' 'GatewayEventFrame::Pairing' src/ interfaces/ shared/` → only enum decl / `topic_name()` / `event_visibility.rs:3496,3736` (arm + test). **No construction site.**
  - `rg -n --glob '*.rs' -i 'bus|publish|event' src/gateway/handlers/pairing.rs` → **zero hits**: the pairing handler (`handle_list` / `handle_approve` / `handle_reject` / `handle_approved_list` / `handle_revoke`) has no `GatewayEventBus` handle and never publishes.
  - `rg -n --glob '*.rs' -F '"pairing.requested"' src/` → no `publish*` site.
  - The only pairing-shaped producer is `src/gateway/inbound_router/permission.rs:178` `self.send_pairing_request(&ctx)` — a *channel* pairing code sent as a chat message, semantically different from the device-pairing frame.
- **Symptom:** the operator never sees a pairing request/completion in the dashboard activity feed, although the type contract, the visibility classification, the operator-only scope rule, and the Panel's severity arms are all already in place. The `pairing.` scope rule guards a topic family that is never published.
- **Hypothesis:** **CONNECT** — `src/gateway/handlers/pairing.rs` should publish `PairingRequested` / `PairingCompleted` frames on the bus when a device pairing request is created / approved / rejected; all downstream arms (Panel, visibility, scope) already exist. This is the cheapest of the frame findings: only the producer is missing.
- **Confidence:** high

---

### W-04 — `EventEmitter::emit_run_complete` has zero callers in the whole repository
- **Form:** 1 (no consumer)
- **Producer:** `src/gateway/event_emitter/mod.rs:147` — `async fn emit_run_complete(&self, run_id: &str, summary: RunSummary, duration_ms: u64)`.
- **Consumer:** none found.
- **Live caller search:**
  - `rg -n --glob '*.rs' 'emit_run_complete' src/ interfaces/ shared/ desktop/` → `src/gateway/event_emitter/mod.rs:147` (definition) and one doc mention at `src/gateway/execution_engine/helpers.rs:529`. **No call site, not even in tests.**
- **Symptom:** dead public API on the `EventEmitter` trait; the run-completion event is instead published directly by the production drain path (`src/gateway/execution_engine/simple.rs:265` → `StreamEvent::RunComplete`), so the trait method is a second, unused spelling of the same wire.
- **Hypothesis:** **CUT** — remove `emit_run_complete` from the trait (or route production through it and delete the direct `StreamEvent::RunComplete` publish, but that is a bigger change with no behavioural gain).
- **Confidence:** high

---

### W-05 — `emit_tool_start` / `emit_tool_update` / `emit_tool_end` / `emit_reasoning` are exercised only by the crate's own tests
- **Form:** 1 (test-only ⇒ severed for production)
- **Producer:** `src/gateway/event_emitter/mod.rs:64` `emit_tool_start`, `:81` `emit_tool_update`, `:97` `emit_tool_end`, `:48` `emit_reasoning`.
- **Consumer:** none in production.
- **Live caller search:**
  - `rg -n --glob '*.rs' 'emit_tool_start|emit_tool_update|emit_tool_end' src/` → only `src/gateway/event_emitter/tests.rs:54,62,65` (plus the definitions).
  - `rg -n --glob '*.rs' 'emit_reasoning' src/` → `src/gateway/event_emitter/tests.rs:10,11,33,34,35`. Note `src/harness/agent/think.rs:1113,1562` call `self.emit_reasoning(...)`, but that is a **different method** — `src/harness/agent/think.rs:465` `fn emit_reasoning(&self, iteration: usize, thinking: Option<&str>)` — not this trait method.
  - Production publishes the same events through a different path: `src/gateway/execution_engine/event_drain.rs:126` `StreamEvent::ToolStart`, `src/gateway/execution_engine/simple.rs:265` `StreamEvent::RunComplete`.
- **Symptom:** four trait methods that look like the canonical event-emission API are dark in production; the real emission path bypasses them. A reader following the trait would wire new events through a dead API.
- **Hypothesis:** **CUT** — the trait's tool/reasoning methods are a duplicate spelling of the `StreamEvent` drain path; delete them (and their tests) or make production call them. Production currently works, so CUT is the low-risk branch.
- **Confidence:** high

---

### W-06 — `ReplyEmitterConfig::buffer_threshold` is a dead knob
- **Form:** 3 (inert config)
- **Producer:** `src/gateway/reply_emitter/config.rs:9` `pub buffer_threshold: usize`, default `500` at `src/gateway/reply_emitter/config.rs:99`.
- **Consumer:** none in production.
- **Live caller search:**
  - `rg -n --glob '*.rs' 'buffer_threshold' src/ interfaces/ shared/` → `src/gateway/reply_emitter/config.rs:9` (field), `:99` (default), and `src/gateway/reply_emitter/tests.rs:310,404,421` (tests only). **No production read.**
  - The streaming path thresholds on `min_initial_chars` + `debounce_ms` instead (`src/gateway/reply_emitter/emitter/streaming.rs`).
- **Symptom:** a documented-looking tuning knob that changes nothing; an operator who reads the config default and tries to tune "buffer threshold" gets silence.
- **Hypothesis:** **CUT** — delete the field and its default; if a buffer threshold is genuinely wanted, it must be wired into the streaming decision.
- **Confidence:** high

---

### W-07 — `ReplyEmitterConfig::voice_enabled` is written by production but never read
- **Form:** 3 (inert config — write-only field)
- **Producer:** `src/gateway/reply_emitter/config.rs:17` `pub voice_enabled: bool`, default `false` at `:101`; **written** at `src/gateway/inbound_router/executor.rs:133` (`reply_config.voice_enabled = voice_enabled;`).
- **Consumer:** none.
- **Live caller search:**
  - `rg -n --glob '*.rs' 'voice_enabled' src/ interfaces/ shared/` → `config.rs:17`, `config.rs:101`, `executor.rs:133` (write), `executor.rs:109` (the **local** variable — a different binding), `executor.rs:408` (uses the local `voice_enabled`, not the config field), `src/gateway/reply_emitter/tests.rs:406`.
  - `src/gateway/reply_emitter/emitter/helpers.rs:28-53` `should_voice()` reads `config.voice_reply_hint` + the live channel registry voice state — **not** `voice_enabled`.
- **Symptom:** the reply emitter's voice decision ignores the very flag production sets one line above; a config-driven "voice disabled" has no effect. The write at `executor.rs:133` looks authoritative but is inert.
- **Hypothesis:** **CUT** — delete the field and the write, or make `should_voice()` consult it. Since the local variable at `executor.rs:109` already carries the truth into `executor.rs:408`, the config field is redundant: CUT.
- **Confidence:** high

---

### W-08 — `MarkupFormat::DiscordMarkdown` / `markdown_to_discord` have no production consumer
- **Form:** 1 (no consumer)
- **Producer:** `src/gateway/formatter/mod.rs:58` `MarkupFormat::DiscordMarkdown`; `src/gateway/formatter/markdown_to_platform.rs:180` `markdown_to_discord` (a pure identity `text.to_string()`).
- **Consumer:** none in production.
- **Live caller search:**
  - `rg -n --glob '*.rs' 'MarkupFormat::DiscordMarkdown' src/ interfaces/ shared/ desktop/ | grep -v 'formatter/tests.rs' | grep -v 'formatter/mod.rs'` → **zero hits**.
  - `rg -n --glob '*.rs' 'MarkupFormat::(TelegramHtml|PlainText|Markdown|SlackMrkdwn|IrcFormatting)' … | grep -v tests | grep -v 'formatter/mod.rs'` → all live: telegram `delivery.rs:436,481,1064,1165,1196`; xmpp `message_ops/ops.rs:152`; signal `message_ops.rs:249`; irc `message_ops.rs:341`; matrix/mattermost/slack likewise.
  - The Discord interface (`src/gateway/interfaces/discord/`) never calls `MessageFormatter`.
- **Symptom:** a whole markup dialect exists with a 100-line test suite but no shipping consumer — the Discord surface either ships raw markdown or a parallel converter; the two ends disagree and neither knows.
- **Hypothesis:** **CUT** — delete `DiscordMarkdown` + `markdown_to_discord` and its tests, unless Discord is meant to adopt the shared formatter (then CONNECT it at the Discord send path).
- **Confidence:** high

---

### W-09 — The Panel subscribes to the `alerts.**` topic family; no core producer exists
- **Form:** 1 (no producer for a subscribed topic family) — also a producer-side form-3 style dead knob
- **Producer:** none found.
- **Consumer:** `interfaces/webchat/src/context.rs:550` `const BASE_TOPICS: [&str; 3] = ["config.**", "alerts.**", "approval.**"]`; `interfaces/webchat/src/context.rs:2755` `if event.topic.starts_with("alerts.")` (live handler that parses `severity` / `count` / `message` and updates the sidebar alert map); `interfaces/webchat/src/context.rs:2811` `self.subscribe_topic("alerts.**")`; `interfaces/webchat/src/context.rs:3803` replay seed entry `"alerts.**"`.
- **Live caller search:**
  - `rg -n --glob '*.rs' 'alerts\.' src/ interfaces/ shared/ desktop/` → **no producer**; the only core-side hits are a test string `src/gateway/server/handler.rs:101` (`"alerts.system"`) and cron test job names.
  - `src/gateway/event_visibility.rs:440` `classify()` has **no** `"alerts."` arm ⇒ a hypothetical producer would fall through to `_ => return None` ⇒ `session_identity_of` would treat it as `Global` (deliverable). The seam is severed on the **producer** side only; the delivery path would work if a producer existed.
  - The Panel's *initial* alert state does work — `interfaces/webchat/src/context.rs` `load_initial_alerts()` calls RPC `"health"` (`src/gateway/handlers/mod.rs:322` registers it) and `"memory.stats"` (`src/gateway/method_census.rs:308` `Class::Open`) — so only the **live-update** path is dark.
- **Symptom:** the notification centre / sidebar alert badges never update live; alerts only reflect the state at page load. The `alerts.**` subscription is a live subscription that never fires.
- **Hypothesis:** **DECIDE** — the consumer side is fully built; either a core alert publisher is planned (CONNECT — e.g. `src/gateway/handlers/health.rs` publishing `alerts.system.health` on status change) or the topic family is a leftover (CUT — remove the subscription + handler). Evidence that would settle it: a design note naming the intended `alerts.*` producer; I found none in `docs/`.
- **Confidence:** medium

---

### W-10 — `node.connected` has no producer; the Panel arm and its doc still claim it does
- **Form:** 5 (name / path drift: stale half of a topic pair)
- **Producer:** none for `node.connected`. The only node-plane producer is `src/cluster/enrollment.rs:789` — `"node.disconnected"`.
- **Consumer:** `interfaces/webchat/src/platform/wide/views/settings/network/cluster.rs:183` `if evt.topic == "node.connected" || evt.topic == "node.disconnected"` (the `node.connected` half is dead); subscription `cluster.rs:179` `state.subscribe_topic("node.**")`.
- **Live caller search:**
  - `rg -n --glob '*.rs' -F 'node.connected' src/ interfaces/ shared/` → `cluster.rs:8` (doc comment: *"`node.connected` / `node.disconnected` have been broadcasting on the event bus all along"*), `cluster.rs:172` (comment), `cluster.rs:183` (the arm), `src/gateway/event_scope.rs:504,564,586` (scope-rule tests / fixtures), `src/gateway/method_admin.rs:131` (comment). **No `publish*` site.**
  - `rg -n --glob '*.rs' '"node' src/cluster/` → `src/cluster/enrollment.rs:789` (`"node.disconnected"`), `src/cluster/node_approval.rs:164` (an RPC **method** `"node.approval.request"`, not a bus topic).
- **Symptom:** the fleet view's "connected" row can never light up; the doc comment asserting both topics broadcast "all along" is false and will keep misleading future readers.
- **Hypothesis:** **CONNECT** — publish `node.connected` at node enrollment (`src/cluster/enrollment.rs` already holds the bus handle at the `node.disconnected` site, `:789`), because the Panel subscribes `node.**` and the doc at `cluster.rs:8,172` already promises the behaviour. The alternative (CUT) is to delete the `node.connected` arm and fix the stale doc.
- **Confidence:** high

---

### W-11 — `capability.event` / `capability.event.status` are published in production with no client consumer
- **Form:** 1 (produced symbol with zero real consumers)
- **Producer:** `src/gateway/capability_events.rs:115` `bus.publish_json(&TopicEvent::new("capability.event", data))` and `:119-120` `TopicEvent::new("capability.event.status", json!({"state":"lagged","incomplete":true}))`, inside `CapabilityEventBridge::start` (`src/gateway/capability_events.rs:108`).
- **Consumer:** none.
- **Live caller search:**
  - `rg -n --glob '*.rs' -F 'capability.event' interfaces/ desktop/ shared/` → **zero hits**. The Panel only knows `"capabilities.ui.display"` (`interfaces/webchat/src/panel/capability_surface.rs:9`), `"capabilities.ui.ack/attach/detach/submit_input"`, and `"capabilities.changed"` (`interfaces/webchat/src/components/right_pane/slot.rs:16`).
  - Producer side is live: `src/bin/aleph-server/commands/start/capability_init.rs:25` calls `CapabilityEventBridge::start(...)`, wired via `src/bin/aleph-server/commands/start/builder/startup_resources.rs:15,57`.
  - Core-side references only: `src/gateway/server/connection/mod.rs:1650,1652` (the per-connection forward filter), `src/gateway/event_visibility.rs:451` (`SessionIdentity::CapabilityStatus`).
  - The only in-process subscriber of `InMemoryEventStream` in production is the bridge itself; `build_capability_events()` (`src/bin/aleph-server/commands/start/capability_init.rs:18`) builds the stream with `InMemoryEventStream::default()` — **no persistence sink**.
- **Symptom:** the capability audit ledger is forwarded onto the gateway bus and then dropped: the audience-stamping, visibility classification and per-connection admission machinery for `capability.event` are fully implemented but no client ever reads the topic, and nothing persists the records.
- **Hypothesis:** **DARK** — the plan that created it (`docs/superpowers/plans/2026-11-07-phase7-i3-production-integration.md:72`) defines `capability.event` as the *immutable audit record* topic; it is plausibly intended for external audit tooling rather than a first-party client. Either add an audit sink (CONNECT) or mark the topic explicitly dark so the forward-filter special-cases at `connection/mod.rs:1650` are not mistaken for a live path. **UNCERTAIN** — evidence that would settle it: a doc or design decision naming the intended consumer of `capability.event`; I found only the plan line above.
- **Confidence:** medium

---

### W-12 — `event_scope.rs` `"guest."` prefix rule guards a topic family with no producer
- **Form:** 3 (inert rule / dead knob)
- **Producer:** none.
- **Consumer:** `src/gateway/event_scope.rs:109` — `("guest.".to_string(), vec!["admin".to_string()])` in `default_rules()`.
- **Live caller search:**
  - `rg -n --glob '*.rs' -F '"guest.' src/ interfaces/ shared/ desktop/` → **only** `src/gateway/event_scope.rs:109` (the rule) and `src/gateway/event_scope.rs:370,459,501` (its own tests). **No producer of any `guest.*` topic.**
  - `src/gateway/event_visibility.rs:440` `classify()` has no `"guest."` arm either.
- **Symptom:** the operator-only prefix rule advertises a guest plane that does not exist; a reader concludes the bus carries guest events. Harmless at runtime (it can only over-restrict a topic nobody publishes) but it is a false contract in a file whose whole purpose is to be the authority on who may receive what.
- **Hypothesis:** **CUT** — delete the `"guest."` rule (and its test fixtures) unless a guest plane is planned. Note: the sibling `"pairing."` rule at `src/gateway/event_scope.rs:108` is *also* currently dark, but it becomes live the moment W-03 is connected — keep that one.
- **Confidence:** high

---

## Things checked and found CONNECTED (not findings)

- `GatewayEvent` (`src/gateway/event_bus.rs:70-72`) has exactly one variant (`ConfigChanged`); `publish_gateway_event` (`:519`) is called by 8 config handlers with a correct `section` (e.g. `src/gateway/handlers/config/general_config.rs:143` → `Some("general")`, `acp_config.rs:134` → `Some("acp")`).
- `surface.notify` → `interfaces/webchat/src/app/shell_bridge.rs:10` and `desktop/shell/src/notify.rs:46`; `surface.approval` → `desktop/shell/src/notify.rs:41`. `DesktopSurface` is registered at `src/bin/aleph-server/commands/start/mod.rs:3790-3798`, and `r5_router::run` is spawned there.
- `gateway.token.rotated` / `gateway.device.revoked` → consumed internally by `src/gateway/server/connection/forward.rs` (`is_token_rotated_frame`, `rotated_should_close_remote`, `device_revoked_id`); producers `src/gateway/server/handler.rs:59,123`.
- `voice.transcribe.delta` → produced `src/gateway/voice/streaming/relay.rs:306,319`, consumed `interfaces/webchat/src/platform/wide/views/voice/mod.rs:229`.
- `run.subagent_tree` → produced `src/gateway/subagent_tree_relay.rs:50`; Panel `team.*` / `stream.*` subscriptions all have producers.
- `capabilities.ui.*` RPCs: registered `src/gateway/server/mod.rs:875-915`, called `interfaces/webchat/src/panel/capability_surface.rs:96,133,176,191,216,233`. `capabilities.ui.invoke` is **explicitly documented-dark** at `src/gateway/server/mod.rs:886-900` and is *not* re-reported here.
- `audience_allows` (`src/gateway/surface/delivery.rs:80`) **is** used in production: `src/gateway/server/connection/mod.rs:1629,1660`. `SurfaceKind::from_opt_str` is used at `src/gateway/server/connection/mod.rs:1072`.
- `reply_emitter::sanitize` / `extract` helpers all have production callers (`extract.rs:14`, `emitter/helpers.rs:4,73,409,430,446`, `emitter/streaming.rs:5,51,185,211,283,310,373,396`); the `ReplyEmitter` field census (`native_stream_state`, `fallback_info`, `model_label`, `reasoning_buffer`, `generation_registry`, `generation_config`, `native_disabled`) shows production reads/writes for every field.
- `capability_ui.rs` handlers (`handle_attach/invoke/ack/submit_input/detach` at `:284,:306,:327`) are real.
- Panel topic subscriptions with live producers: `projects.changed`, `team.*.task.*` (`src/agents/swarm/tasks/store/mod.rs:94`), `team.changed`, `acp.sessions.changed` (`src/bin/aleph-server/commands/start/mod.rs:1267`), `cron.job.changed`, `heartbeat.task.changed`, `workspace.changed`, `config.**`, `approval.**`, `memory.reembed.*`, `node.**` (partially — see W-10), `voice.transcribe.delta`.
- **No** `TODO` / `todo!` / `unimplemented!` / `FIXME` anywhere in the slice.

---

## Seams NOT covered (and why)

1. **`src/gateway/event_visibility.rs` (4399 lines) was NOT read line-by-line.** Only `classify()` (`:440`), `session_identity_of()` (`:805`) and the pinned tests were inspected. Its ~4000 lines are mostly exhaustive per-topic permission tests; a full pass was out of budget. *Evidence that would settle the rest:* a per-topic table diffing every `classify` arm against every producer topic.
2. **`src/gateway/event_emitter/instant_buffer.rs` (594), `redacting.rs` (872), `origin_fanout.rs` (513), `team_fanout.rs` (533)** were checked only for *caller presence* of their public entry points, not for internal field-level wiring. Their `pub(super)` items are `dead_code`-linted, so a truly unreferenced item would not survive compilation — but a *mis-wired* internal (e.g. a redactor that redacts nothing) would not be caught by that lint and was not audited.
3. **`src/gateway/formatter/helpers.rs` (749 lines)** was checked for caller presence only, not for per-rule correctness (e.g. escaping edge cases).
4. **The Panel side is out of this repo's scope for code, but present as `interfaces/webchat/`** (package name `aleph-panel`). I audited the *topic vocabulary* and *subscription* side of the Panel, not its rendering logic.
5. **`desktop/shell/`** was checked only for `surface.notify` / `surface.approval` consumers.
6. **The `shared/client` + CLI/TUI path** was checked for `stream.*` handling only (`shared/client/src/connection.rs:166-190` → `Inbound::Event(StreamEvent)`, i.e. typed events, not topics); it does not consume topics, so no topic drift applies there.
7. **Round-4 sibling slices** (other modules) were not reviewed; findings are limited to the slice named in the task.
8. **No test suite was executed** (audit is static). No `cargo check`/`cargo test` was run, and **no repository file was modified** — the report lives only under `/tmp/sev-r4/reports/`.
