# Severed-wire audit — `src/extension`, `src/fetch`, `src/gateway`

- Date: 2026-11-21 → 2026-11-22
- Repo: `/home/zou/data/workspace/Aleph`, branch `main`
- Baseline at audit time: `da970869d`; final state of this round: see *Verification*
- Scope: `src/extension/**` (106 files / 60 839 lines), `src/fetch/**` (5 files / 502 lines),
  `src/gateway/**` (718 files / 368 219 lines)
- Mode: static review with **no diff supplied** — the audit looked for *severed wires*
  (a producer and a consumer that exist but are not connected), not for regressions.

---

## 1. Method

### 1.1 Slices and waves

The three modules were cut into 10 read-only slices and audited by parallel `sev-auditor` subagents
(one slice per child, each child writing a report file and returning a ≤40-line summary). A slice
whose provider quota died was re-run; nothing was silently dropped.

| slice | scope | report | findings |
|---|---|---|---|
| ext-a | extension registration / manifest / marketplace / mcp_config | `ext-a.md` | 11 |
| ext-b | extension hooks / lifecycle / effects / capability | `ext-b.md` | 5 |
| fetch-runtime | `src/fetch` + extension runtime / panel_publication / types | `fetch-runtime.md` | 14 |
| gw-b1 | `src/gateway/handlers/**` + `admin_api/**` | `gw-b1.md` | 3 |
| gw-b2 | execution_engine / middleware / inbound_router / busy_queue / btw / delivery_queue | `gw-b2.md` | 8 |
| gw-b3 | session_manager / session_store / server / security / hot_reload / link / visibility | `gw-b3.md` | 16 |
| if-c1 | telegram / discord / slack | `if-c1.md` | 20 |
| if-c2 | feishu / whatsapp / imessage / wechat | `if-c2.md` | 21 |
| if-c3 | voice_call / sms / line / matrix / xmpp / nostr | `if-c3.md` | 17 |
| if-c4 | irc / signal / email / mattermost / qq / webhook | `if-c4.md` | 12 |

**127 candidates** in total. The parent agent additionally ran its own mechanical sweeps
(config-field readers, dead-symbol census, shadow-config comparison) and read the code behind every
candidate before acting on it.

`src/gateway/telecom/**` + `src/gateway/pty/**` were assigned to a `gw-d1` slice whose auditor died on
a provider quota and was **not** re-run to completion; the pty surface it covered was partly reached
through `gw-b1` (see *What was NOT done*).

### 1.2 Triage

Every candidate was classified **CONNECT / CUT / DECIDE / DARK** against the triage playbook
(`references/triage-playbook.md`):

- no live caller ⇒ **CUT**;
- painless wire ⇒ **CUT**;
- a real product fork ⇒ **DECIDE** (escalated to the operator, never silently chosen);
- a gap that is *documented as intentional* ⇒ **DARK** (left alone on purpose).

A **read-before-write** rule applied: every symbol was grepped repo-wide (`src/`, `crates/`,
`interfaces/`, `shared/`, `desktop/`, `tests/`, `mobile/`, `benches/`, `bindings/`, `examples/`,
`plugins/`, `tools/`) before it was deleted. Four candidates were **rejected as false positives** by
that rule — including `DeliveryQueue::pending_for_conversation` (live at `delivery_queue.rs:2227`) and
`AgentInstanceConfig::max_loops` / `max_tokens` (live in `agent_manage/info.rs` and
`run_loop/inner.rs:316`).

### 1.3 Isolation and delivery

Each module was fixed in its own git worktree (`wire-extension`, `wire-fetch`, `wire-gateway`), then
merged into `main` with `--no-ff`. Later slices were fixed by dedicated `sev-fixer` subagents, each in
its own worktree and branch (`fix-if-c1`, `fix-if-c3`, `fix-if-c4`, `fix-gateway-rest`,
`fix-if-c2`, `fix-gw-b1`), each required to verify the evidence, commit, and write a report.

---

## 2. Where Jev was used

`jev_evaluate` was used only for **classification / triage**, never for code reading or editing:

| batch | question | outcome |
|---|---|---|
| gateway config | shadow `GatewayConfig.sandbox`/`.tools` — cut or keep? | **CUT 0.98** |
| gateway dead code | batch of dead helpers — cut or keep? | **DARK 0.67 / CUT 0.22** (conf 0.5) |
| gateway config | inert config fields — cut, connect or keep dark? | **CUT 0.60 / CONNECT 0.24 / DARK 0.16** (conf 0.4) |
| gateway pair-loop | severed `bot_loop_protection` — cut or rewire? | **CUT 0.91 / CONNECT-PER-CHANNEL 0.04** (conf 0.88) |
| ext-a ×10 | EXTA-01 … EXTA-11 — connect / cut / dark | CONNECT 0.97 · EXTA-02 0.49 (low) · EXTA-04 0.55 (low) · EXTA-05 1.00 · EXTA-06 0.95 · EXTA-07 0.95 · EXTA-08 0.51 (low) · EXTA-09 CUT 0.82 · EXTA-10 DARK 1.00 · EXTA-11 CUT 0.90 |

**Jev was over-ruled where the code was self-evident.** It systematically over-scores DECIDE, so a
candidate whose two ends contradicted each other in the source (a comment that promised a wire the code
did not implement) was decided by reading the code, not by the score. The two lowest-confidence batches
(config fields at conf 0.4, dead code at conf 0.5) were exactly the ones where the code, not the
score, decided.

Complex reasoning (reading a slice, proving a candidate, writing the fix) went to the reasoning
models (`minimax-cn/MiniMax-M3`, `kimi-coding/k3.1-pro`, `deepseek-flash`).

---

## 3. Operator decisions

Five forks were product decisions, not code decisions; each was escalated rather than guessed.

| # | question | decision |
|---|---|---|
| 1 | the severed pair-loop guard | **per-channel lookup** — the router holds a `HashMap<channel_id, PairLoopGuardConfig>` and `pair_loop_check` looks the config up by `msg.channel_id`, falling back to the global default; the existing per-channel operator key became a real reader |
| 2 | `[fetch].enabled` / `default_provider` / `fallback_providers` | **CUT all three** — nothing read them; keep the per-backend `base_url`/`api_key`/`timeout`/test |
| 3 | a plugin agent's `mode:` key | **the plugin side must error loudly** (like the disk loader's `ForbiddenSystemField`), not drop it |
| 4 | the whole `src/gateway/link` subsystem (`LinkManager` / `LinkConfig` / `~/.aleph/links/*.yaml`) | **delete it** — no live path existed |
| 5 | the extension manifest display metadata (author / homepage / repository / license / keywords) | **delete the fields** — they were parsed by three dialects and read by nobody |

---

## 4. Findings and dispositions

### 4.1 Configuration that lied to the operator

- **`GatewayConfig` was a shadow copy of the app config.** `src/gateway/config.rs` re-declared
  `sandbox` and `tools` while the live types are `crate::sandbox::SandboxConfig`
  (`src/sandbox/config.rs:330`) and `crate::config::types::tools::ToolsConfig`
  (`src/config/types/tools.rs:115`). Both are deserialized from the *same* operator TOML, so
  `[sandbox.docker_image]`, `[tools.chrome]`, `[cron]`, `[webhook]` were silently ignored while the
  file claimed they were set. → **CUT** the shadow fields and their types.
- **The canonical `[[bindings]]` shape could not be parsed.** The app config declares
  `bindings: Vec<RouteBinding>` (`src/config/structs.rs:197`); the gateway copy declared
  `bindings: HashMap<String, String>` (`src/gateway/config.rs:57`). The same file was loaded through
  both, so the canonical array-of-tables form aborted startup. → **CUT** the gateway copy;
  `handle_reload` / `handle_validate` now read the app-level binding count.
- **`[fetch]` carried three routing knobs that no request path consulted.** The agent-facing
  `web_fetch` path deliberately does not route through a fetch provider (BT-D-R4-22), so the knobs
  only ever told the operator a routing decision was being made. → **CUT** (decision 2); the boot
  warn now names the backends that exist.
- **`ChannelAccessConfig::bot_loop_protection` was read by nobody** while the guard it describes ran
  with hard-coded defaults. → **CONNECT** (decision 1).
- **Inert per-interface knobs** (telegram `polling_interval_secs`, signal `poll_interval_secs` /
  `send_typing`, LINE `send_typing` / `coalescing`, voice-call `inbound_enabled` /
  `outbound_enabled`, discord `ThreadConfig`, wechat's three send knobs, email `use_tls=false`, …)
  → **CUT** (with the Panel rows that would have written them) or **CONNECT** to the code they
  claimed to configure.

### 4.2 Silently dropped or swallowed behaviour

- **The whatsapp `Pairing` policy logged "needs pairing" and delivered anyway** (default policy at
  `channel_policy.rs:255`, `whatsapp/mod.rs:247-251`); `approve_pairing` had no caller and
  `paired_numbers` was always empty. → **CONNECT**, fail closed.
- **Six feishu event types were parsed and then dropped** by the mapper's `_ => None`, hidden by an
  `#[allow(dead_code)]`. → **CONNECT or CUT per event type**.
- **The telegram multi-account selection always picked the first account** — the `.or_else` fallback
  made the explicit lookup unreachable, so a second tenant was answered by the first bot identity.
  → **CONNECT** (explicit resolution; ambiguous ⇒ configuration error).
- **Discord `clarify:<n>` buttons did nothing** (only `AckOnly` was handled) while telegram's
  equivalent worked. → **CONNECT**.
- **The approval card kept its keyboard after the decision** — the slot-api contract requires
  `keyboard=None` on resolve. → **CONNECT**.
- **`send_typing=false` could not stop the generic 4 s typing loop** (the loop cannot see per-channel
  config). → **CONNECT** (honour the flag in the per-channel limbs).
- **The `AgentEnd` hook's `update_output:` was accepted and thrown away**, and the graceful
  `BeforeAgentStart` stop text never reached the channel. → **CONNECT** (the run output is already
  delivered, so the hook now warns instead of pretending; the stop text is emitted as a chunk).
- **A fast-path run skipped the steering rescue** that the normal path performs. → **CONNECT**
  (`post_run_steering_rescue`).
- **The backpressure rejection answered with `id: null`**, leaving the client's pending promise
  hanging until its own timeout. → **CONNECT** (the request id travels with the rejection).
- **A dropped request leaked its in-flight slot.** If the response future was dropped (client
  disconnect, task abort), `requests_in_flight` was never decremented and the request state stayed
  `Processing` forever — after enough disconnects the backpressure threshold trips permanently.
  → **CONNECT** (`RequestGuard` releases the slot and marks the request cancelled on drop), and
  `RequestState::AwaitingResponse` finally has a writer.

### 4.3 Code that existed but was unreachable

Dead modules, files, functions, trait methods and struct fields were cut — after a repo-wide grep
proved no caller: `src/gateway/http_server.rs` (the manual static-file path; the live one is
`serve_webchat_on_listener`), `src/gateway/link/**`, telegram's `audit.rs` (a write-only audit ring)
and `sticker.rs` (a cache with no producer), discord's `draft.rs` / `group_policy.rs` /
`session_key.rs` / `resolver/**` / `reconnect.rs`, whatsapp's `message.rs` / `types.rs`,
wechat's `media.rs`, matrix's `convert_timeline_event`, the extension's async
`parse_manifest_from_dir`, the registry's `stats()` / `get_hooks_for_event` / `clear_diagnostics`,
`SessionStore::count_by_state` / `list_by_state` / `set_error` / `stop`,
`SessionMetadata::auto_reset_at`, `PluginRegistry::get_agent`, the manifest's display metadata, and
about forty smaller helpers across `execution_engine`, `busy_queue`, `capability_ui`,
`announce_delivery`, `credential_planner`, `source_census`, `visibility`, `event_emitter/**`, …

### 4.4 Deliberately dark wires (left alone)

- `src/fetch` is **documented-dark**: the `FetchProvider` trait exists for the `fetch_config.test`
  path only, and `web_fetch` does not route through it. The module says so; the audit left it alone.
- `src/gateway/method_visibility.rs` is an **audit trail, not a dispatch gate** — its module doc says
  so, and its accessor `treatment_of` exists for the pinned tests. Left alone, with a guard that keeps
  the wording.
- `ContextLifecycleEvent::SessionShutdown` has no emitter; the seam is reserved for a host-owned
  shutdown. Documented instead of "fixed".
- `hot_reload`'s `ReloadMode` / `should_hot_reload` do not hot-reload anything; the doc now says what
  the reload path actually does (the PII engine is rebuilt and `config.reloaded` is published; agents,
  models and channels are not reconfigured).

---

## 5. What changed

| area | change |
|---|---|
| config | shadow `GatewayConfig.sandbox`/`.tools`/`.bindings` cut; the `[fetch]` section knobs cut; the app-level binding count is what `config.reload`/`config.validate` report |
| pair-loop | `PairLoopGuardConfig::from_channel_config` reads the operator key; the router resolves the guard config **per channel** and the boot wiring registers each channel's override |
| extension | manifest display metadata cut; the plugin agent `mode:` key now fails loudly; the config-schema budget has **one** owner (`config_validation.rs`), and `hub_catalog.rs` calls it; the CLI (`aleph plugin validate`) now compiles the `config_schema` and checks the entry file; plugin validation warnings reach the operator; the context-grant audit is readable through `context_status` |
| interfaces | telegram multi-account selection, `clarify:` buttons, approval-keyboard removal, `send_typing`, streaming `preserve_formatting`; discord `application_id`/doctor lie cut, `reconnect.rs` deleted, group policy made explicit; slack `api_base` honoured and 429 classified as rate-limited; whatsapp pairing fail-closed + group context injected; feishu account resolution + sender names; wechat `cdn_base_url` + the three send knobs; email real IMAP pre-flight, TLS enforced, subject threading, fail-closed attachments; signal/mattermost/irc/qq/webhook attachment and rate-limit handling; matrix upload filename + direct-vs-group classification; nostr kind-4 ciphertext no longer reaches the agent; LINE reply-token capture cut |
| execution | the fast path performs the steering rescue; the `AgentEnd` hook warns instead of pretending to rewrite delivered output; a graceful hook stop reaches the channel |
| store | the file session backend implements `set_source_channel` (it was a silent no-op, so every file-backed session lost its origin) |
| gateway metrics | the dropped-request leak is closed; `AwaitingResponse` has a writer; the backpressure rejection carries the request id |
| delivery queue | the module doc no longer credits a stamp that production does not write |

---

## 6. Guards

Three new grep-level guards, wired into `just check-wiring` (now 14 guards):

| guard | what it pins |
|---|---|
| `scripts/extension_wire_audit.py` | the cut manifest/registry wires stay cut; the plugin agent `mode:` key stays loud; the config-schema budget keeps exactly one owner; the CLI keeps validating the schema; the manifest cache stays documented |
| `scripts/fetch_wire_audit.py` | the section-level `[fetch]` routing knobs stay cut; the section keeps its backends; the boot warn keeps its producer; the `web_fetch` / provider split stays documented-dark |
| `scripts/gateway_wire_audit.py` | the deleted modules stay deleted; the shadow config fields stay gone; the pair-loop override stays connected per channel; the file backend keeps `set_source_channel`; the metrics guard keeps the drop path; `method_visibility` stays documented as a non-gate |

Each guard was mutation-tested: breaking the `mode:` rejection, re-adding the `[fetch].enabled` knob,
removing the per-channel pair-loop lookup and removing the drop-cancel were each caught, and the
guards returned to green after the revert.

---

## 7. Verification

- `cargo check -p alephcore -p aleph-panel --all-targets -j 4` — see *Verification record* below.
- `python3 scripts/{extension,fetch,gateway}_wire_audit.py` → PASS.
- `just check-wiring` — the three new guards are green; the pre-existing `config_wire_audit.py`
  false positive (`TimeWindowConfig`, `src/config/types/policies/capability_constraints.rs:24`) is
  untouched by this round.

---

## 8. What was NOT done

- **`src/gateway/telecom/**` and `src/gateway/pty/**` were not fully audited.** The `gw-d1` slice died
  on a provider quota and was not re-run; only the pty handler path was reached through `gw-b1`.
- **No test suite was run.** The user asked for one unified compile check instead; the behavioural
  changes (pairing fail-closed, email pre-flight, telegram account selection, the matrix/nostr
  routing changes) are therefore **compile-verified, not behaviour-verified**.
- **The Panel is a separate repository.** The Rust-side DTO changes (`fetch_config` lost
  `enabled`/`default_provider`; the deleted per-channel knobs) must be mirrored by the Panel before it
  is deployed — the Panel rows for the removed knobs were removed in this repo, but the Panel's own
  fetch toggle is not here.
- **`gw-b3-F1` (multi-tenant routing / `LinkConfig.routing`) and the feishu multi-account fork were
  decided as "fail closed", not as a routing feature.** Building a per-tenant routing story is a
  separate piece of work.
- **The audit did not look at the `interfaces/webchat` Panel TypeScript** beyond the channel-settings
  definitions that the config changes touched.
- **Provider quotas truncated the sweep**: `kimi-coding/k3.1-pro` (5-hour limit) and
  `minimax-cn/MiniMax-M3` (token-plan limit) each killed slices mid-run; every killed slice was
  re-run, but the re-runs happened on a different model, so a second opinion on the same slice is
  missing.

---

## 9. Verification record

_(filled in below)_
