# Severed-Wire Audit — Batch 5 (src/gateway)

**Scope:** `server/`, `transport/`, `security/`, `voice/`, `webhooks/`, `openai_api/`, `admin_api/`, `agent_env/`
**Date:** 2026-08-17
**Method:** PRODUCED − CONSUMED via `rg` across `src/`, `src/bin/`, `interfaces/`, `shared/`. Decision rule: no live consumer ⇒ CUT.

---

## Findings (12 total)

| # | ID | Module | Severity | Form | Decision | Verdict at a Glance |
|---|----|--------|----------|------|----------|---------------------|
| 1 | sw-gateway-41 | `gateway/transport/` | high | dead module | **CUT** | Entire `pub mod transport` re-exported by `gateway::mod.rs:27` but has zero consumers — bridge-IPC abstraction (Signal/WhatsApp) was planned but never implemented |
| 2 | sw-gateway-42 | `gateway/webhooks/` | critical | dead module | **CUT** | All 5 files of the external-service webhooks subsystem are orphaned — `create_router` (`handler.rs:122`) re-exported as `create_webhook_router` is never merged into the server; only sibling `webhook_receiver` is wired |
| 3 | sw-gateway-43 | `gateway/admin_api/agents.rs` | medium | dead router | **CUT** | `pub fn router()` not mounted in `admin_api::router` by explicit design (header comment lines 6–13) — three handlers have zero production callers |
| 4 | sw-gateway-44 | `gateway/webhooks/hmac.rs::generate_signature` | low | dead fn | **CUT** | Paired with `verify_signature` (also unused) — only `webhooks/` itself references either; identity module uses the unrelated `security/crypto::verify_signature` |
| 5 | sw-gateway-45 | `gateway/webhooks/template.rs::{render_template, extract_variables, TemplateContext}` | low | dead module | **CUT** | Only referenced inside `webhooks/handler.rs` (which itself is dead) — distinct from `providers/protocols::TemplateContext` |
| 6 | sw-gateway-46 | `gateway/voice/format.rs::build_format_prompt` | low | dead pub fn | **CUT** | `pub` visibility only to back `format_text`'s static `DEFAULT_PROMPT` — never called across the crate |
| 7 | sw-gateway-47 | `gateway/openai_api/stream.rs::{completion_id, now_timestamp}` | low | dead helpers | **DECIDE** | Module-internal SSE plumbing helpers; `pub` likely for test access. Could be downgraded to `pub(super)` |
| 8 | sw-gateway-48 | `gateway/voice/streaming/whisperlive.rs` | n/a | live | **CONNECT** | `WhisperLiveDecoder`/`WhisperLiveStream` constructed by `streaming/mod.rs::build_transcriber` (line 128), used through `relay::start_stream` |
| 9 | sw-gateway-49 | `gateway/voice/streaming/deepgram.rs` | n/a | live | **CONNECT** | Same dispatch path as WhisperLive via `build_transcriber` |
| 10 | sw-gateway-50 | `gateway/voice/{state, outbound, voice_mode, format, sanitize, local_provider, inbound, hallucination}` | n/a | live | **CONNECT** | Sanity sweep — all heavily consumed by `gateway/handlers/voice.rs`, `reply_emitter/`, `media/resolve`, `generation/factory`, `thinker/layers/voice_mode`, `inbound_router` |
| 11 | sw-gateway-51 | `gateway/security/{artifact_caps, canvas_caps, crypto, token_readonly, device_token_manager, shared_token, store::SecurityStore}` | n/a | live | **CONNECT** | All gated/encrypted surfaces actively used by `server/{artifact_route, canvas_asset_route}`, `handlers/{gateway_devices, connect, users}`, `identity/{artifact, keystore, verify}`, `cli/ipc_client`, `bin/aleph-server/commands/{secret, pair, identity, bootstrap_token}` |
| 12 | sw-gateway-52 | `gateway/openai_api/`, `gateway/admin_api/{resume, secrets}`, `gateway/server/`, `gateway/agent_env/` | n/a | live | **CONNECT** | Sanity sweep — `openai_routes`, `OpenAiApiState`, `admin_api::router`, `ArtifactRouteState`, `CanvasAssetRouteState`, `GatewayServer`, `AgentEnvStore`, etc. all consumed by `server::mod.rs`, `bin/aleph-server/commands/start/*` |

---

## Rationales per finding

### sw-gateway-41 — `gateway/transport/` entire module is dead

`pub mod transport;` is declared in `src/gateway/mod.rs:27`. The header on `src/gateway/transport/mod.rs` declares:
> "Transport layer for bridge process IPC ... [`unix_socket::UnixSocketTransport`] ... [`stdio::StdioTransport`]"

Searching `src/`, `src/bin/`, `interfaces/`, `shared/` for `gateway::transport`, `alephcore::gateway::transport`, `gateway::transport::stdio`, `gateway::transport::unix` yields **one** hit — a doc-comment in `src/gateway/transport/stdio.rs:22` itself. The only `StdioTransport` references in the codebase are in `mcp/transport/stdio.rs` and `acp/transport.rs` (entirely separate modules). No Signal/WhatsApp/IPC bridge adapter exists on HEAD to consume this.

**Proposed change:** delete `src/gateway/transport/` (4 files) and remove the `pub mod transport;` line in `src/gateway/mod.rs:27`.

**Risk:** low — `pub use stdio::StdioTransport;` in `transport/mod.rs:17` and `pub use traits::*;` (re-exporting `Transport`, `BridgeEvent`, `PairingEvent`, `AttachmentPayload`, `TransportError`) are the only externally-visible artifacts, none of which are reachable from elsewhere in the codebase.

**Verification:** after removal: `cargo check -p alephcore` should succeed. `rg "gateway::transport"` should produce no matches.

---

### sw-gateway-42 — `gateway/webhooks/` entire module is dead

A sibling module `src/gateway/webhook_receiver.rs` (NOT in this batch — line 7 banner: "Difference from webhooks Module") is what the server actually mounts:

```
src/gateway/server/mod.rs:851:
    router = router.merge(crate::gateway::webhook_receiver::WebhookReceiver::router(
```

`src/gateway/webhooks/handler.rs:122` defines `pub fn create_router(state: Arc<WebhookHandlerState>) -> Router` and `src/gateway/mod.rs:195` re-exports it as `create_webhook_router`. **No file merges that router into the server.** All the supporting types are self-referential:

- `WebhooksConfig` (`config.rs:190`) — searched globally, only used inside `webhooks/{config, handler}.rs`.
- `WebhookEndpointConfig` (`config.rs:38`) — same.
- `WebhookHandlerState` (`handler.rs:25`) — same.
- `WebhookProcessor` trait (`handler.rs:34`) — same.
- `WebhookRequest`/`Accepted`/`Rejected`/`Error` — same.
- `hmac.rs::{verify_signature, generate_signature}` — separate from `security/crypto::verify_signature` which is what `identity/{artifact, keystore, verify}.rs` actually use.
- `template.rs::{render_template, extract_variables, TemplateContext}` — distinct from `providers/protocols::TemplateContext` which is the actively-used templating type.

**Proposed change:** delete `src/gateway/webhooks/` (5 files), remove `pub mod webhooks;` line in `src/gateway/mod.rs:129`, and the `pub use webhooks::{…}` block at `src/gateway/mod.rs:194`.

**Risk:** medium — must verify no TOML config or external extension refers to the `[[webhooks]]` config schema. Quick check: `rg "\[\[webhooks\]\]"` returns only self-references inside `src/gateway/webhooks/mod.rs:25`. Safe to delete.

**Verification:** after removal: `cargo check -p alephcore`, then `rg "gateway::webhooks" src/ src/bin/ interfaces/ shared/` → 0 results.

---

### sw-gateway-43 — `admin_api/agents.rs` is intentionally dead

`src/gateway/admin_api/mod.rs` header lines 6–13:
> "`/v1/admin/agents` is intentionally NOT mounted: the three handlers (`POST /`, `PATCH /{id}`, `DELETE /{id}`) had zero production callers — `aleph agent create / update / delete` was never built"

`agents.rs:16` defines `pub fn router() -> Router<AdminApiState>`. The `mod.rs::router` only nests `/secrets` and `/resume` (lines 45–46), never `/agents`. The whole module is preserved for the "next iteration".

**Proposed change:** either (a) genuinely delete `agents.rs` and the `pub mod agents;` declaration until the CLI commands exist, or (b) keep but mark it `#[cfg(feature = "admin-agents")]` so the dead code does not appear in `cargo doc`/`rg` surveys. Prefer (a) until the CLI ships.

**Risk:** low — the file is on HEAD as future-stub by explicit author intent.

**Verification:** after deletion: `cargo check -p alephcore`.

---

### sw-gateway-44, 45, 46 — Smaller dead pub-items, all subsumed by 41/42/43

These exist only as constituents of the dead modules above, so they fall automatically when the parent module is deleted. No separate CUT needed — listed for completeness:

- `webhooks/hmac.rs::generate_signature` (line 238) — only `webhooks/handler.rs:151` (test) calls it.
- `webhooks/template.rs::{render_template, extract_variables, TemplateContext}` — only `webhooks/handler.rs:22,127` (and tests).
- `voice/format.rs::build_format_prompt` (line 49) — only `voice/format.rs` tests call it; `format_text` uses the constant `DEFAULT_PROMPT` directly.

---

### sw-gateway-47 — `openai_api/stream.rs` helpers

`completion_id()` (line 24) and `now_timestamp()` (line 30) are `pub fn` but only called inside `stream.rs` itself. `pub` is likely defensive for tests in adjacent modules. **DECIDE** rather than CUT — they're 1–3 line helpers, low blast radius, and tightening visibility is a separate cleanup.

**Proposed change (optional):** downgrade to `pub(super)` if no tests in other modules need them.

---

### sw-gateway-48, 49 — Streaming adapters are LIVE (not dead)

Despite the absence of named external constructors, both `deepgram::DeepgramStream::new(t)` and `whisperlive::WhisperLiveStream::new(t)` are constructed at `src/gateway/voice/streaming/mod.rs:128–132` inside `pub fn build_transcriber(t: StreamingTarget) -> Box<dyn StreamingTranscriber>`, which is called from `src/gateway/voice/streaming/relay.rs:151` inside `pub async fn start_stream`, which is called from `src/gateway/handlers/voice.rs:432`. Both adapters are reachable through the live `handle_stream_audio` route.

**No change.** CONNECT.

---

### sw-gateway-50, 51, 52 — Sanity sweeps (no findings)

I verified the following symbols all have live consumers:

| Symbol | Consumer |
|--------|----------|
| `voice::VoiceState` | `reply_emitter/emitter/{mod,helpers}.rs` |
| `voice::generate_tts[(_outcome)]` | `reply_emitter/emitter/helpers.rs:92` |
| `voice::VoiceTurnState::{set,get,new}` | `thinker/layers/voice_mode.rs:272,287,289,313,344,346` |
| `voice::format::format_text` | `gateway/handlers/voice.rs:147` |
| `voice::sanitize::sanitize_for_tts` | `voice/outbound.rs:167` |
| `voice::local_provider::{LocalTranscription, LocalVoiceProvider}` | `media/resolve.rs:75`, `generation/factory.rs:88` |
| `voice::hallucination::filter_transcript` | `voice/inbound/stt.rs:156` |
| `voice::inbound::{resolve_stt_source, transcribe_with_source, has_audio_attachment, process_inbound_voice}` | `gateway/handlers/voice.rs:16,77,90`, `inbound_router/mod.rs:117,281,633,643` |
| `security::{ArtifactCapabilities, CanvasCapabilities}` | `server/{artifact_route, canvas_asset_route}.rs` |
| `security::{DeviceTokenManager, SharedTokenManager, read_current_token_readonly}` | `gateway/handlers/{gateway_devices,connect,users}.rs`, `bin/aleph-server/commands/{secret,pair,identity,bootstrap_token}.rs`, `cli/ipc_client.rs:13,85` |
| `security::store::{SecurityStore, UserRole, UserStatus, OWNER_USER_ID}` | `server/{mod,handler}.rs`, `bin/aleph-server/commands/{secret,pair,identity,bootstrap_token}.rs`, `handlers/users.rs`, `projects/store.rs`, `executor/builtin_registry` |
| `openai_api::openai_routes`, `OpenAiApiState` | `gateway/server/mod.rs:15,760,777,1471` |
| `openai_api::completions::{passthrough, agent}::handle` | wired through `completions::handle` dispatcher |
| `openai_api::responses::{handle, sse::provider_deltas_to_responses_sse}` | wired through `router.rs:24` |
| `openai_api::embeddings::handle` | wired through `router.rs:23` |
| `admin_api::{router, secrets::SecretSummary, resume::ResumeResponse}` | `bin/aleph-server/commands/{start/mod, secret, resume}.rs` |
| `server::{GatewayServer, GatewayConfig, GatewaySharedState, ArtifactRouteState, CanvasAssetRouteState, handle_health, handle_ready, handle_metrics}` | `bin/aleph-server/commands/start/{mod, builder/*}.rs`, `src/cli/ipc_client.rs` |
| `agent_env::{AgentEnvStore, ActiveAgentEnv, AgentEnvContext, AgentEnvFilter, DEFAULT_AGENT}` | `builtin_tools/{workspace_manage, agent_manage/*, gateway_route}`, `inbound_router`, `executor/builtin_registry`, `executor/engine`, `memory`, `routing`, `utils/paths` |

---

## What was **not** checked (negative disclosure)

- **No reading of source files** — decisions were purely PRODUCED − CONSUMED symbol matching across `rg`. The audit may miss dead pub-items that ARE consumed by symbol but live on broken/dead paths.
- **No testing** of dynamic dispatch via `Box<dyn Trait>` (e.g. `Box<dyn WebhookProcessor>` is in scope, but no concrete impl is reachable from `bin/`, only from the dead `webhooks/handler.rs::tests`).
- **No check across `webchat/` style** `interfaces/*` deeper than top-level: relied on `rg` propagating through `interfaces/`.
- **No evaluation of `cargo build` after proposed CUTs** — verification is described but not executed (out of scope per "<60 tool calls" budget).
- **No check on `docs/`, `tests/`, `examples/`** for references to the deleted modules.
- **Did not examine `transport/traits.rs`** symbols (`Transport`, `BridgeEvent`, `PairingEvent`, `AttachmentPayload`, `TransportError`) individually — they fall with the whole module under sw-gateway-41.
- **`openai_api/stream.rs` `completion_id`/`now_timestamp`** marked DECIDE rather than CUT due to test-access ambiguity.
- **`security/store/{tests,types}.rs`** — `tests.rs` is `#[cfg(test)]`, never consumed externally; `types.rs` provides `DeviceUpsertData`/`DeviceRow`/`DeviceTokenRow` re-exported through `store/mod.rs:36`, so it's transitive-OK.
- **`voice/inbound/stt.rs::SttSource`/`SttConfig`** — verified live via `voice/inbound/provider.rs` test code plus `inbound_router/mod.rs:117`; CONNECT.
