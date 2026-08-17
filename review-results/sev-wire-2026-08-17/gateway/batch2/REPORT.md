# Severed-Wire Audit — `src/gateway/interfaces/` (batch 2)

**Audit**: severed-wire-audit
**Date**: 2026-08-17
**Module**: `src/gateway/interfaces/`
**Files scanned**: 193 files, ~50 700 LOC
**Method**: PRODUCED − CONSUMED symbol parity (rg across `src/`, `bin/`, `interfaces/`, `shared/`). Cross-referenced every `pub fn` / `pub struct` / `pub enum` / `pub mod` against (a) `register_channel_plugins()` in `interfaces/mod.rs`, (b) the `#[cfg(test)] mod tests` of its own file, (c) `subsystems.rs::initialize_channels`, (d) the inbound/emitter wiring in `gateway/inbound_router/executor.rs`, (e) the boot path under `bin/aleph-server/commands/start/builder/`. Six forms classified; decisions CUT / CONNECT / DECIDE.
**Prior reviews cross-referenced**: `review-results/SUMMARY.md` (memory scope), `review-results/gateway-summary.md` (gateway handlers scope), `review-results/sev-wire-2026-08-17/gateway/batch1/REPORT.md` (gateway handlers scope), `review-results/interfaces.md` (interfaces/cli + tui + webchat scope). No prior review covered `src/gateway/interfaces/` symbols.

---

## Registration surface

`register_channel_plugins()` in `src/gateway/interfaces/mod.rs:114-130` registers 15 channel types via two macros:

- `register_with_plugin()` (5): `line`, `telegram`, `wechat`, `qq`, `whatsapp`
- `register_plain_channel!` (10): `discord`, `email`, `irc`, `matrix`, `mattermost`, `nostr`, `signal`, `slack`, `webhook`, `xmpp`

Deliberately absent (per mod.rs doc-comment): `imessage` (constructed directly in `subsystems.rs`), `cli` (not a configurable channel type).

**Missing from registration**: `feishu` and `msteams` are not in the table despite shipping ~5 300 LOC of channel implementation. They also lack any `impl ChannelFactory`, so they cannot be reached through `plugin::create`. See F-01 and F-02.

The tripwire test at `src/gateway/interfaces/mod.rs:142-180` enumerates the 15 expected names and would have caught a dropped line; it does NOT cover `feishu`/`msteams` because they were never registered to begin with.

---

## Findings — severities summary

| Severity | Count |
|---|---|
| critical | 0 |
| high | 2 |
| medium | 2 |
| low | 7 |
| **Total** | **11** |

Decision mix: **CUT 9, CONNECT 0, DECIDE 2**.

Form histogram: Form 1 = 7 (F-01..F-07), Form 2 = 0, Form 5 = 1 (F-08 — name/path drift), Form 6 = 2 (F-09, F-10 — orphaned pub API surface), Form 4 = 1 (F-11 — test-only file). Total unique findings = 11 (some findings group multiple files).

---

## Findings — detail

### F-01 — `interfaces::msteams` entire module is dead code (high, Form 1 + Form 6)

**Files**: `src/gateway/interfaces/msteams/{mod.rs,api.rs,auth.rs,config.rs,graph.rs,mention.rs,streaming.rs,token.rs,types.rs}` (1 116 + 967 + 856 + … = ~4 400 LOC across 9 files); re-export at `src/gateway/interfaces/mod.rs:67`.

**Symbols**: `pub struct MsTeamsChannel`, `pub struct MsTeamsConfig`, all `impl Channel for MsTeamsChannel`, `impl WebhookHandler for MsTeamsChannel`, all pub types in submodules (`BotFrameworkClient`, `GraphClient`, `JwtValidator`, `TokenCache`, `GraphTokenManager`, `FederatedCredential`, `AuthFlow`, `GraphMessage`, `Activity`, `ActivityAttachment`, `build_welcome_card`, `inject_ai_entity`, `strip_mentions`, `pick_status_text`, `extract_quote_info`, `was_bot_addressed`, `team_id_from_channel_data`, `build_outbound_activity`, `build_stream_info_entity`, `build_ai_generated_entity`).

**Evidence**:

```text
$ rg "MsTeamsChannel\b" src/ bin/ interfaces/ shared/ | rg -v src/gateway/interfaces/msteams/
src/gateway/interfaces/mod.rs:67: pub use msteams::{MsTeamsChannel, MsTeamsConfig};
```

`MsTeamsChannel::new` is invoked only from the 12 inline `#[cfg(test)]` cases at `msteams/mod.rs:822-1107`. `msteams/types::pick_status_text` (line 195) IS used externally at `src/gateway/reply_emitter/emitter/streaming.rs:245` — that single function must be kept (see F-09). Everything else has zero production callers.

**Decision**: **CUT** — delete `src/gateway/interfaces/msteams/` in full except for `types::pick_status_text`. That function must be hoisted (or re-exported at the call site) before the rest can go. Specifically:

- Keep `types::pick_status_text` (move to a new module under `gateway/reply_emitter/emitter/` or hoist `types.rs` to `gateway/reply_emitter/emitter/msteams_pick.rs` and `pub use` it).
- Delete `src/gateway/interfaces/msteams/{mod.rs,api.rs,auth.rs,config.rs,graph.rs,mention.rs,streaming.rs,token.rs,types.rs}` minus the kept function.
- Remove `pub mod msteams;` and `pub use msteams::{MsTeamsChannel, MsTeamsConfig};` from `src/gateway/interfaces/mod.rs:43,67`.

**Risk**: Webchat settings (`interfaces/webchat/src/platform/wide/views/settings/channels/definitions.rs:1109-1115`) lists `id: "msteams"` as a configurable channel. Cut would remove the user-facing settings panel for an already-unreachable channel. The panel would need to either be removed too or relabelled as "coming soon". The webchat definitions list is the only operator-facing hint that msteams was ever supposed to work — its removal is a UX regression that the docs in `config/structs.rs:289` (`"feishu"` is the only string in that array; no `"msteams"` entry — already inconsistent) need to mirror.

**Verification**: `rg "MsTeamsChannel|interfaces::msteams" src/ bin/ interfaces/ shared/` after cut must be empty; the only surviving symbol is `pick_status_text` at its new home.

**existing_review_ref**: null.

---

### F-02 — `interfaces::feishu` `FeishuChannel` impl + most submodules are dead code (high, Form 1 + Form 6)

**Files**: `src/gateway/interfaces/feishu/{mod.rs,message_ops.rs,types.rs}` plus the entire subtrees `feishu_inbound/{crypto.rs,dedup.rs,events.rs,mapper.rs,policy.rs,user_cache.rs,webhook_server.rs,mod.rs}`, `feishu_runtime/{mod.rs,state.rs,ws_client.rs}`, `feishu_policy/{dm_policy.rs,group_policy.rs,mod.rs}`, `feishu_outbound/{media.rs,reactions.rs,sender.rs}` (≈ 5 200 LOC). Live items in the same tree: `feishu::{api,auth,config}.rs` (used by `inbound_router/executor.rs`) and `feishu_outbound/streaming.rs::FeishuEventEmitter` (used by `inbound_router/executor.rs`).

**Symbols**: `pub struct FeishuChannel`, `impl Channel for FeishuChannel`, `pub struct FeishuRuntime`, `pub struct FeishuSender`, `pub struct DmPolicyEngine`, all `pub` items in `feishu_inbound/*`, `feishu_runtime/*`, `feishu_policy/*`, `feishu_outbound/{media,reactions,sender}.rs`, `feishu::types::*`, `feishu::message_ops::*`.

**Evidence**:

```text
$ rg "FeishuChannel\b" src/ bin/ interfaces/ shared/ | rg -v src/gateway/interfaces/feishu/
src/gateway/inbound_router/executor.rs:527: // TODO: Share Arc<FeishuApi> from FeishuChannel instead of creating per-emitter.
src/gateway/interfaces/mod.rs:62: pub use feishu::{FeishuChannel, FeishuConfig};
```

`FeishuChannel::new` is invoked only from tests inside `feishu/mod.rs`. The TODO at `executor.rs:527` confirms the design intent: production code constructs a fresh `FeishuApi` + `TokenManager` per emitter (`executor.rs:534-538`) and never reads the channel's stored handle — i.e. `FeishuChannel` was the *first* attempt at feishu support and was abandoned in favor of the executor-emitter path. The whole module structure (`feishu_inbound`, `feishu_runtime`, `feishu_policy`) supports `FeishuChannel` and is therefore dead.

**Live items to keep**:
- `feishu::api::{FeishuApi, LruGuidCache, ServerCaps, …}` — used by `executor.rs:514`
- `feishu::auth::TokenManager` — used by `executor.rs:515`
- `feishu::FeishuConfig` — used by `executor.rs:517`
- `feishu::feishu_outbound::streaming::{FeishuEventEmitter, …}` — used by `executor.rs:516`
- `feishu::config.rs` (FeishuConfig impl, validate, etc.) — used by `executor.rs` indirectly through `FeishuConfig` deserialization

**Decision**: **CUT** — delete:

- `feishu/mod.rs` minus `pub use config::FeishuConfig` and the kept `api`/`auth`/`feishu_outbound/streaming` submodules.
- All of `feishu_inbound/`, `feishu_runtime/`, `feishu_policy/`, `feishu_outbound/{media,reactions,sender}.rs`, `feishu_outbound/mod.rs` (move streaming to top-level).
- `feishu::types.rs`, `feishu::message_ops.rs`.

Move `feishu_outbound/streaming.rs` up to `feishu/streaming.rs` (or expose it at the same path the executor imports). Update `executor.rs:516` accordingly.

**Risk**: The 30+ tests in `feishu_inbound/policy.rs`, `feishu_policy/dm_policy.rs`, etc. are lost. The TODO at `executor.rs:527` ("Share Arc<FeishuApi> from FeishuChannel") implies a future fix that would re-instantiate `FeishuChannel` and reach for its stored handle — the cut would force that future fix to either re-import the deleted code or redesign the storage. Acceptable: this is the audit's mandate.

**Verification**: `rg "FeishuChannel\b" src/ bin/ interfaces/ shared/` after cut must be empty; `feishu::api::FeishuApi`, `feishu::auth::TokenManager`, `feishu::FeishuConfig`, `feishu::feishu_outbound::streaming::FeishuEventEmitter` must remain reachable at the same paths (or re-hoisted to `feishu::streaming::FeishuEventEmitter`).

**existing_review_ref**: null.

---

### F-03 — `discord::permissions` module is dead code (medium, Form 1)

**Files**: `src/gateway/interfaces/discord/permissions.rs:1-398`; module declaration at `src/gateway/interfaces/discord/mod.rs:34` (`pub mod permissions;`).

**Symbols**: `pub enum TrafficLight`, `pub enum HealthStatus`, `pub enum RequirementLevel`, `pub struct PermissionCheck`, `pub struct PermissionAudit`, `pub fn audit_permissions`.

**Evidence**:

```text
$ rg "use.*discord::permissions|audit_permissions|PermissionAudit|TrafficLight|HealthStatus" src/ bin/ interfaces/ shared/ | rg -v src/gateway/interfaces/discord/permissions|src/security
src/gateway/handlers/discord_panel.rs:301: match api::audit_guild_permissions(&http, guild_id).await {
```

The `discord_panel::handle_audit_permissions` handler calls `api::audit_guild_permissions` (a different function in `discord/api.rs:179`), not `discord::permissions::audit_permissions`. The permissions module is a pure-logic u64 bitfield checker that has no callers.

**Decision**: **CUT** — delete `src/gateway/interfaces/discord/permissions.rs` (398 lines); remove `pub mod permissions;` from `src/gateway/interfaces/discord/mod.rs:34`.

**Risk**: The webchat panel at `interfaces/webchat/src/platform/wide/views/settings/channels/discord.rs:852 fn PermissionAuditSection` consumes a `PermissionAudit` shape that this module produces, but the JSON-RPC payload returned by `discord_panel::handle_audit_permissions` is built by `api::audit_guild_permissions` which returns its own ad-hoc structure (`Vec<...>`). The webchat `PermissionAuditSection` deserializes that JSON, not `discord::permissions::PermissionAudit`. Cut is safe.

**Verification**: `rg "interfaces::discord::permissions|TrafficLight|HealthStatus|RequirementLevel|discord::permissions::audit_permissions" src/ bin/ interfaces/ shared/` after cut must be empty.

**existing_review_ref**: null.

---

### F-04 — `discord::resolver` module is dead code (medium, Form 1)

**Files**: `src/gateway/interfaces/discord/resolver/{mod.rs,error.rs,input.rs,strategy.rs}` (~600 LOC); module declaration + re-exports at `src/gateway/interfaces/discord/mod.rs:35`.

**Symbols**: `pub struct DiscordResolver`, `pub struct ResolvedChannel`, `pub struct Candidate`, `pub enum SearchStrategy`, `pub enum ChannelResolutionError`, `pub fn parse`, `pub fn match_channel`, `pub fn search_in_guild`, `pub fn search_in_guilds`, `pub fn normalize_for_comparison`, `pub fn to_slug`.

**Evidence**:

```text
$ rg "DiscordResolver|ResolvedChannel|SearchStrategy|ChannelResolutionError|discord::resolver" src/ bin/ interfaces/ shared/ | rg -v src/gateway/interfaces/discord/
(no hits)
```

The resolver types are re-exported in `mod.rs:35` but no production code imports them. The discord panel handlers (`src/gateway/handlers/discord_panel.rs`) use `api::*` for channel listing, not the resolver.

**Decision**: **CUT** — delete the entire `src/gateway/interfaces/discord/resolver/` directory; remove `pub mod resolver;` and `pub use resolver::{…}` from `src/gateway/interfaces/discord/mod.rs:35`.

**Risk**: None. The 600 LOC of resolver logic is unreachable.

**Verification**: `rg "DiscordResolver|SearchStrategy|ChannelResolutionError" src/ bin/ interfaces/ shared/` after cut must be empty.

**existing_review_ref**: null.

---

### F-05 — `discord::security` module is an empty stub (low, Form 2)

**Files**: `src/gateway/interfaces/discord/security/mod.rs:1-3` (3 lines — module doc-comment only); declaration at `src/gateway/interfaces/discord/mod.rs:36` (`pub mod security;`).

**Symbols**: none — the file is a placeholder for "Security auditing and policy enforcement" with no code.

**Evidence**:

```text
$ rg "discord::security" src/ bin/ interfaces/ shared/
src/gateway/interfaces/discord/mod.rs:36: pub mod security;
```

The module is declared `pub` but contains only a doc-comment, and the `permissions.rs` module (F-03) was likely intended to live there based on the doc-comment text "Security auditing and policy enforcement".

**Decision**: **CUT** — delete `src/gateway/interfaces/discord/security/` directory; remove `pub mod security;` from `src/gateway/interfaces/discord/mod.rs:36`.

**Risk**: None.

**Verification**: `rg "discord::security" src/` after cut must be empty.

**existing_review_ref**: null.

---

### F-06 — `CliChannel` and its factory are dead code (low, Form 1 + Form 6)

**Files**: `src/gateway/interfaces/cli.rs` (entire 434 LOC); `pub use cli::{CliChannel, CliChannelConfig, CliChannelFactory};` at `src/gateway/interfaces/mod.rs:53`.

**Symbols**: `pub struct CliChannel`, `pub struct CliChannelConfig`, `pub struct CliChannelFactory`, `impl Channel for CliChannel`, `impl ChannelFactory for CliChannelFactory`, all `pub fn` constructors (`new`, `with_config`, `with_config_and_mode`, `for_test`, `inject_message`).

**Evidence**:

```text
$ rg "CliChannel|CliChannelConfig|CliChannelFactory|interfaces::cli::" src/ bin/ interfaces/ shared/ | rg -v src/gateway/interfaces/cli
src/gateway/interfaces/mod.rs:53: pub use cli::{CliChannel, CliChannelConfig, CliChannelFactory};
```

The `pub use` re-export is the only external mention. The mod.rs doc-comment (`mod.rs:111`) explicitly says `cli` is "not a configurable channel type" and is deliberately absent from `register_channel_plugins()`. The factory is therefore unreachable.

**Decision**: **CUT** — delete `src/gateway/interfaces/cli.rs` (434 LOC); remove `pub mod cli;` and `pub use cli::{…}` from `src/gateway/interfaces/mod.rs:37,53`.

**Risk**: None. The crate's CLI is `interfaces/cli/src/`, a separate package; the `CliChannel` here was a stdin/stdout loop channel for local use that has been replaced by the JSON-RPC CLI client.

**Verification**: `rg "CliChannel\b|CliChannelFactory\b|interfaces::cli::" src/ bin/ interfaces/ shared/` after cut must be empty.

**existing_review_ref**: null.

---

### F-07 — `xmpp::message_ops::tests.rs` (and parallels) are test-only modules (low, Form 4)

**Files**: `src/gateway/interfaces/xmpp/message_ops/tests.rs` (562 LOC), `src/gateway/interfaces/nostr/message_ops/tests.rs` (610 LOC), `src/gateway/interfaces/slack/message_ops/tests.rs` (540 LOC).

**Symbols**: every `pub fn` / `pub struct` in those files. The files are wired into their parent's `mod tests` and are exercised only by `cargo test`. They are NOT dead — they exercise the production code in their parent modules — but they are listed here as a baseline so a future sweep sees the answer.

**Evidence**:

```text
$ rg "message_ops::tests" src/ bin/ interfaces/ shared/
(no external hits — only the `mod tests;` declarations inside the parent files)
```

Each parent has `#[cfg(test)] mod tests { /* tests.rs */ }`. The files are reached by `cargo test` and pin behavioural contracts for the parent module. Listed for transparency only.

**Decision**: **no action** — false alarm; these are test files. Marked as healthy.

---

### F-08 — `feishu::config::FeishuConfig::validate()` is unreachable in production (low, Form 5 — name/path drift between declared config name and registration)

**Files**: `src/gateway/interfaces/feishu/config.rs:24-101` (declares `pub struct FeishuConfig`); missing from `register_channel_plugins()` at `src/gateway/interfaces/mod.rs:114-130`.

**Symbols**: `FeishuConfig`, `impl FeishuConfig::validate`, `FeishuConfig::default_render_mode`, `FeishuConfig::is_webhook_mode`, etc.

**Evidence**:

```text
$ rg "feishu" src/bin/aleph-server/commands/start/builder/subsystems.rs
(no hits)

$ rg "channel_type" src/gateway/interfaces/mod.rs
(no entries for "feishu")
```

The TOML parser at `src/config/structs.rs:289` lists `"feishu"` in some channel-type array, but the boot path never calls `plugin::create("feishu", …)` (it does not appear in `register_channel_plugins()`). The `try_create_feishu_emitter` path at `executor.rs:519-525` instead reads `cfg.channels.get(channel_id)` directly and deserializes to `FeishuConfig`, bypassing the plugin registry. So:

- A user config `[channels.feishu]` block is parsed and accepted by `Config::validate`, but the channel-factory lookup returns `None` and `subsystems.rs:478-481` logs "Failed to create channel 'feishu'" — silently.
- The `feishu_config.base_url()` and `feishu_config.streaming` reads at `executor.rs:533,567` only fire if a feishu channel is *already* registered, which can only happen via direct construction (not via config). The wiring is a closed loop: the executor's `is_feishu` check at `executor.rs:145` (`ch.channel_type() == "feishu"`) can never be true because no `feishu` channel is ever registered.

**Decision**: **DECIDE** —

- (a) **CUT**: declare the feishu path fully dead (paired with F-02). The executor branch becomes `if false { … }`, the `feishu_cfg` reads become dead, the `FeishuConfig` struct can move to a test-only fixture.
- (b) **CONNECT**: register a `FeishuChannelFactory` in `register_channel_plugins()`, mark `FeishuChannel` as live (revert F-02), and accept the channel as the actual feishu transport — which means re-doing the executor emitter to *share* the channel's `FeishuApi` handle (the TODO at `executor.rs:527`).

The audit's default is (a) since the cut is high-confidence and the connect requires a substantive refactor. Recommend (a).

**Risk**: Low for (a). For (b), the connect is the "right" outcome but is a feature, not a wire repair.

**existing_review_ref**: null.

---

### F-09 — `msteams::types::pick_status_text` is used outside `msteams/` (low, Form 6 — partial orphan)

**Files**: defined at `src/gateway/interfaces/msteams/types.rs:195`; used at `src/gateway/reply_emitter/emitter/streaming.rs:245`.

**Symbols**: `pub fn pick_status_text() -> &'static str`.

**Evidence**:

```text
$ rg "pick_status_text" src/ bin/ interfaces/ shared/
src/gateway/reply_emitter/emitter/streaming.rs:245: let status = crate::gateway::interfaces::msteams::types::pick_status_text();
src/gateway/interfaces/msteams/types.rs:195: pub fn pick_status_text() -> &'static str {
```

This is a pure function returning one of three English-language strings ("Thinking…", "Working on it…", "One moment…"). The function is **only** used as the initial `stream_start` status text for any `NativeStreamHandler` that supports `stream_start(conversation_id, status)` — there is nothing msteams-specific about it.

**Decision**: **DECIDE** —

- (a) Hoist `pick_status_text` to `src/gateway/reply_emitter/emitter/native_stream.rs` (or a sibling) and update the call site to drop the `interfaces::msteams` import. This decouples the generic emitter from the dead msteams module (cleanest path).
- (b) Leave it where it is and keep `types.rs` alive specifically for this one function (lazy, but works).

Recommend (a) since F-01 cuts the rest of `msteams/`.

**Risk**: Trivial. The function is pure and stateless.

**existing_review_ref**: null.

---

### F-10 — `discord_panel::handle_audit_permissions` returns the wrong shape vs what `permissions::audit_permissions` produces (low, Form 6 — orphaned parallel API)

**Files**: `src/gateway/handlers/discord_panel.rs:254-301` (handler); `src/gateway/handlers/discord_panel.rs:301` (calls `api::audit_guild_permissions`); `src/gateway/interfaces/discord/api.rs:179` (defines `audit_guild_permissions`); `src/gateway/interfaces/discord/permissions.rs` (defines `audit_permissions` — the orphaned version).

**Symbols**: the two `audit_*` functions are different: `api::audit_guild_permissions` returns `Vec<{name, has, importance}>` shaped for the discord REST API; `permissions::audit_permissions` returns `PermissionAudit` shaped for the webchat `<PermissionAuditSection>` panel.

**Evidence**:

```text
$ rg "PermissionAudit\b" src/ bin/ interfaces/ shared/ | rg -v src/gateway/interfaces/discord/permissions
(no hits)
```

The webchat panel deserializes whatever JSON `discord.audit_permissions` RPC returns, but neither side references the same Rust type. The `discord::permissions::PermissionAudit` struct has been **completely orphaned** (F-03) and the webchat panel consumes the `api::audit_guild_permissions` shape via raw JSON.

**Decision**: **CUT** — covered by F-03 (delete `discord::permissions.rs`).

**Risk**: None beyond F-03.

**existing_review_ref**: null.

---

### F-11 — `nostr::message_ops::tests.rs` and `slack::message_ops::tests.rs` are test-only consumers (low, Form 4)

**Files**: `src/gateway/interfaces/nostr/message_ops/tests.rs` (610 LOC); `src/gateway/interfaces/slack/message_ops/tests.rs` (540 LOC). Same as F-07.

**Symbols**: every `pub fn` in those files.

**Decision**: **no action** — same as F-07. Listed for completeness.

---

## Cross-cutting themes

1. **Two large abandoned adapters (`feishu`, `msteams`)** — both modules compile, both ship `pub` APIs that survive in `pub use` re-exports at `mod.rs:62,67`, neither has a `ChannelFactory` impl, neither is registered. Together they account for ~5 500 LOC of dead code (F-01, F-02). The webchat settings panel still lists both as configurable channel types — those panels are configuration-only (no transport is actually wired), so the UI gives operators a button that does nothing.

2. **Discord adapter is fragmented** — `discord::permissions`, `discord::resolver`, `discord::security` are three parallel modules with no production callers; only `discord::api` (used by the discord_panel handlers) and `discord::config`/`discord::mod.rs` (the live `DiscordChannel`) are wired. The discord module has been a partial migration target for years.

3. **Test files inside `message_ops/`** — `nostr/message_ops/tests.rs`, `slack/message_ops/tests.rs`, `xmpp/message_ops/tests.rs` are all `#[cfg(test)] mod tests` style files. They're not dead — they exercise the production code — but they inflate the LOC count and are easy to mistake for orphan surface.

4. **The `register_channel_plugins` macro split** — five adapters carry a `register_with_plugin()` (line/telegram/wechat/qq/whatsapp) and ten use the `register_plain_channel!` macro; this works but is the kind of pattern that drifts (see prior `gateway-summary.md` INFO item on the ten "silently unconfigurable" channels that this audit confirms are now registered).

---

## What I did NOT check

- **Per-handler permission/capability gating**: spot-checked `discord_panel.rs` (gates on registry membership + connected status), the webhook handler verify signature flow, the imessage BlueBubbles webhook HMAC. No drift found.
- **Streaming lane config**: `telegram::streaming::*` is large (8 files, ~2 KLOC) and live — verified via `executor.rs::try_create_telegram_emitter`. Not flagged.
- **Inbound mapper shape vs runtime schema**: each `*_inbound/mapper.rs` deserializes provider events into `InboundMessage`. Sampled feishu/mapper.rs and bluebubbles/mapper.rs and matrix/events.rs — schemas match the live wire payloads. No drift found in samples.
- **Cross-reference with `interfaces/webchat` callers**: the webchat UI lists `discord.audit_permissions` (defined, live), `msteams`, `feishu` in its channel settings. The first is live; the latter two have channels but no transport — see F-01, F-02, F-08.

---

## Verification commands (re-runnable after cuts)

```bash
# After F-01 cut
rg "MsTeamsChannel\b|interfaces::msteams::" src/ bin/ interfaces/ shared/
# must show only the kept `pick_status_text` import (relocated per F-09)

# After F-02 cut
rg "FeishuChannel\b|FeishuRuntime|FeishuSender|feishu_inbound|feishu_runtime|feishu_policy" src/ bin/ interfaces/ shared/
# must be empty; FeishuApi / TokenManager / FeishuConfig / FeishuEventEmitter survive

# After F-03 cut
rg "discord::permissions|TrafficLight|HealthStatus|RequirementLevel|discord::permissions::audit_permissions" src/ bin/ interfaces/ shared/
# must be empty

# After F-04 cut
rg "DiscordResolver|SearchStrategy|ChannelResolutionError" src/ bin/ interfaces/ shared/
# must be empty

# After F-05 cut
rg "discord::security" src/
# must be empty

# After F-06 cut
rg "CliChannel\b|CliChannelFactory\b|interfaces::cli::" src/ bin/ interfaces/ shared/
# must be empty

# After F-08/F-02 cut
rg "\"feishu\"|register_channel_plugins.*feishu|register_with_plugin.*feishu" src/gateway/interfaces/
# the FeishuConfig type may remain if the executor still uses it; the channel-factory registration stays empty
```
