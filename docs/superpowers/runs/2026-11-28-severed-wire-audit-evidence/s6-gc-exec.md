## A. Findings

| ID | Form | Severity | Title | Producer | Consumer |
|----|------|----------|-------|----------|----------|
| SW-S6-GC-EXEC-1 | 1 | high | `GroupChatOrchestrator::with_database` has zero callers — the `group_chat_sessions` table is never written | `src/group_chat/orchestrator.rs:70` | none |
| SW-S6-GC-EXEC-2 | 1 | high | `GroupChatExecutor::with_database` has zero callers — the `group_chat_turns` table is never written | `src/group_chat/executor.rs:95` | none |
| SW-S6-GC-EXEC-3 | 1 | high | `StateDatabase::get_group_chat_turns` has zero callers — the read-back half of the persistence chain does not exist | `src/resilience/database/group_chat.rs:143` | none |
| SW-S6-GC-EXEC-4 | 1 | high | `GroupChatExecutor::with_provider_registry` has no production caller — per-persona provider resolution never happens | `src/group_chat/executor.rs:73` | none (tests only) |
| SW-S6-GC-EXEC-5 | 3 | medium | `[[personas]].provider` config knob is parsed and carried into `Persona.provider` but silently ignored in production | `src/config/types/group_chat.rs:127` | `src/group_chat/executor.rs:105` (unreachable branch) |
| SW-S6-GC-EXEC-6 | 1 | medium | `CoordinatorPlan.need_summary` is requested from the coordinator LLM and parsed, but never read — synthesis feature is dark | `src/group_chat/protocol.rs:286` | none |
| SW-S6-GC-EXEC-7 | 1 | medium | `GroupChatMessage.dropped_targets` is produced by the executor but dropped by every serializer — dropped-mention feedback never reaches the user | `src/group_chat/executor.rs:565` | none |
| SW-S6-GC-EXEC-8 | 1 | low | `GroupChatSession.source_channel` has no production reader (only the dead DB insert and tests) | `src/group_chat/session.rs:46` | none |

**Severity tally**: high 4 · medium 3 · low 1 · (UNCERTAIN 0)

---

### SW-S6-GC-EXEC-1 — `GroupChatOrchestrator::with_database` has zero callers — the `group_chat_sessions` table is never written
- **Form**: 1 (no consumer)
- **Producer**: `src/group_chat/orchestrator.rs:70` — `pub fn with_database(mut self, db: Arc<StateDatabase>) -> Self { self.db = Some(db); self }`, the only way to arm `GroupChatOrchestrator.db`.
- **Consumer**: **none found** — the two DB writes it gates (`src/group_chat/orchestrator.rs:195` `insert_group_chat_session`, `src/group_chat/orchestrator.rs:273` `update_group_chat_session_status`) live inside `if let Some(db) = &self.db`, and `self.db` is always `None` in production because the builder is never called.
- **Live caller search**:
  - `rg -n --glob '*.rs' 'with_database' .` → exactly two hits, both **definitions**:
    `./src/group_chat/executor.rs:95: pub fn with_database(mut self, db: Arc<StateDatabase>) -> Self {`
    `./src/group_chat/orchestrator.rs:70: pub fn with_database(mut self, db: Arc<StateDatabase>) -> Self {`
    (no call sites anywhere in the repo, including `src/bin/`, `crates/`, `desktop/`, `interfaces/`)
  - `rg -n --glob '*.rs' 'insert_group_chat_session|update_group_chat_session_status' .` → only `src/resilience/database/group_chat.rs:35`/`:74` (definitions) and the two gated call sites inside the orchestrator.
  - Production construction site: `src/bin/aleph-server/commands/start/mod.rs:3470` — `GroupChatOrchestrator::new(gc_config, &persona_configs);` — no `.with_database(...)` chained.
- **Symptom**: Group-chat sessions live only in RAM. A server restart loses every session; the `group_chat_sessions` table stays empty; any future "list my past group chats" / replay feature reads nothing. Not user-visible today because no reader exists either (see SW-S6-GC-EXEC-3).
- **Hypothesis**: CUT — the arm between "an `Arc<StateDatabase>` exists in the process" and "sessions are persisted" is missing; the producer side (`StateDatabase`) exists and the DB schema exists, but the wire is never connected.
- **Confidence**: high
- **Severity**: high

---

### SW-S6-GC-EXEC-2 — `GroupChatExecutor::with_database` has zero callers — the `group_chat_turns` table is never written
- **Form**: 1 (no consumer)
- **Producer**: `src/group_chat/executor.rs:95` — `pub fn with_database(mut self, db: Arc<StateDatabase>) -> Self { self.db = Some(db); self }`.
- **Consumer**: **none found** — `executor::persist_turn` (`src/group_chat/executor.rs:151-185`) starts with `let Some(db) = &self.db else { return; };` and the only DB write it reaches is `src/group_chat/executor.rs:167` `insert_group_chat_turn`. With `self.db == None`, `persist_turn` is an unconditional early return, so the executor's whole per-turn persistence path is dark.
- **Live caller search**:
  - `rg -n --glob '*.rs' 'with_database' .` → 2 definitions, 0 calls (see SW-S6-GC-EXEC-1).
  - `rg -n --glob '*.rs' 'insert_group_chat_turn' .` → `src/resilience/database/group_chat.rs:101` (definition) + `src/group_chat/executor.rs:167` (gated call). No other caller.
  - Executor construction site: `src/bin/aleph-server/commands/start/mod.rs:3485-3488` — `GroupChatExecutor::new(handle).with_coordinator_visible(coordinator_visible).with_history_window(history_window_rounds)` — no `.with_database(...)`.
- **Symptom**: Turns are never durable. `group_chat_turns` remains empty forever; the "persist a conversation turn to the database (best-effort)" doc at `src/group_chat/executor.rs:147` describes behavior that cannot happen in production. `persist_turn` is also not exercised by any test (no test in the slice builds an executor with a DB), so the path is neither live nor covered.
- **Hypothesis**: CUT
- **Confidence**: high
- **Severity**: high

---

### SW-S6-GC-EXEC-3 — `StateDatabase::get_group_chat_turns` has zero callers — the read-back half of the persistence chain does not exist
- **Form**: 1 (no consumer)
- **Producer**: `src/resilience/database/group_chat.rs:143` — `pub async fn get_group_chat_turns(&self, session_id: &str) -> Result<Vec<GroupChatTurn>, AlephError>` (SELECT … ORDER BY round ASC, sequence ASC).
- **Consumer**: **none found** — no code reads `group_chat_turns` back anywhere.
- **Live caller search**:
  - `rg -n --glob '*.rs' 'get_group_chat_turns' .` → only `src/resilience/database/group_chat.rs:143` (the definition). The second hit, `src/group_chat/session.rs:162`, is a **doc comment** ("replay orders by `(round, sequence)`") — not a call.
  - `rg -n --glob '*.rs' 'get_group_chat_session|delete_group_chat' .` → no hits (no session-row reader exists either).
  - The live history path is in-memory only: `src/gateway/handlers/group_chat.rs:451` reads `session.history` (the `Vec<GroupChatTurn>` on `GroupChatSession`), never the DB.
- **Symptom**: The whole write→read chain is one-way and one-ended. Even if SW-S6-GC-EXEC-1/2 were fixed by attaching a DB, no consumer would read the rows back — persisted turns would be write-only. The `ORDER BY round, sequence` replay contract the session doc relies on has no implementation.
- **Hypothesis**: CUT
- **Confidence**: high
- **Severity**: high

---

### SW-S6-GC-EXEC-4 — `GroupChatExecutor::with_provider_registry` has no production caller — per-persona provider resolution never happens
- **Form**: 1 (no consumer)
- **Producer**: `src/group_chat/executor.rs:73` — `pub fn with_provider_registry(mut self, registry: Arc<ProviderRegistry>) -> Self { self.provider_registry = Some(registry); self }`, the only writer of `self.provider_registry`.
- **Consumer**: **none found in production** — the only reader is `src/group_chat/executor.rs:105` `if let Some(ref registry) = self.provider_registry` inside `resolve_provider`; with `provider_registry == None` that branch is skipped entirely and `resolve_provider` always returns `self.default_provider.current()` (`src/group_chat/executor.rs:129`).
- **Live caller search**:
  - `rg -n --glob '*.rs' 'with_provider_registry' .` → `src/group_chat/executor.rs:73` (definition), `src/group_chat/executor.rs:1032` and `src/group_chat/executor.rs:1204` — both inside `#[cfg(test)] mod tests` (the file's test block starts at `src/group_chat/executor.rs:581`).
  - Production construction site: `src/bin/aleph-server/commands/start/mod.rs:3476-3488` — the registry **is** built (`create_provider_registry_from_env().ok().map(|reg| { let snapshot = reg.default_provider(); … })`) but only `reg.default_provider()` is used; `reg` itself is dropped and never handed to the executor.
- **Symptom**: Every persona in every group chat runs on the single default provider, no matter what the operator configured. The doc on `src/group_chat/executor.rs:64-69` ("When a persona specifies a `provider` field, the executor looks it up in this registry") is false in production.
- **Hypothesis**: CUT
- **Confidence**: high
- **Severity**: high

---

### SW-S6-GC-EXEC-5 — `[[personas]].provider` config knob is parsed and carried into `Persona.provider` but silently ignored in production
- **Form**: 3 (inert config)
- **Producer**: `src/config/types/group_chat.rs:127` — `pub provider: Option<String>` on `PersonaConfig`, documented as "Optional AI provider override (e.g., \"anthropic\", \"openai\")"; copied verbatim by `src/group_chat/persona.rs:76` `provider: cfg.provider.clone()` into the runtime `Persona`.
- **Consumer**: **unreachable** — the only reader of `Persona.provider` is `src/group_chat/executor.rs:105-129` `resolve_provider`, whose registry branch is dead (SW-S6-GC-EXEC-4). No other code reads `persona.provider` (`rg -n --glob '*.rs' '\.provider' src/group_chat/` returns only the executor branch and the config copy).
- **Live caller search**:
  - `rg -n --glob '*.rs' 'provider:' src/group_chat/` → `src/group_chat/persona.rs:76` (copy) and `src/group_chat/protocol.rs:72` (field decl).
  - `rg -n --glob '*.rs' 'resolve_provider' .` → `src/group_chat/executor.rs:105` (definition) + `src/group_chat/executor.rs:403` (the single call site in the persona loop). That call site resolves to `self.default_provider.current()` in production.
  - `src/group_chat/executor.rs:115-129` — the "persona provider not found in registry" warning is inside the `if let Some(ref registry)` block, so it **never fires** in production either. The misconfiguration is completely silent.
- **Symptom**: An operator writes `[[personas]] id = "arch" provider = "openai"` and the persona silently answers with the default provider. No log line, no error, no warning — the knob looks supported (it is in the config schema and the TOML docs) and is dead.
- **Hypothesis**: CUT
- **Confidence**: high
- **Severity**: medium

---

### SW-S6-GC-EXEC-6 — `CoordinatorPlan.need_summary` is requested from the coordinator LLM and parsed, but never read — the synthesis feature is dark
- **Form**: 1 (no consumer)
- **Producer**: `src/group_chat/protocol.rs:286` — `pub need_summary: bool` on `CoordinatorPlan`; the coordinator LLM is explicitly instructed to set it (`src/group_chat/coordinator.rs:93` — "Set need_summary to true if the topic is complex and benefits from synthesis."), and it is deserialized by `parse_coordinator_plan` (`src/group_chat/coordinator.rs:102`, plain `serde_json::from_str` into `CoordinatorPlan`).
- **Consumer**: **none found** — `src/group_chat/executor.rs` reads only `plan.respondents` (`sort_by_key(|r| r.order)`, `retain`, `truncate`) and never `plan.need_summary`; there is no summary round, no summary persona, and no summary message anywhere in the module.
- **Live caller search**:
  - `rg -n --glob '*.rs' 'need_summary' .` → `src/group_chat/protocol.rs:286` (field), `src/group_chat/coordinator.rs:93` (prompt), `src/group_chat/coordinator.rs:131` (`build_fallback_plan` sets `false`), `src/group_chat/coordinator.rs:305` (test asserting the prompt mentions it), plus JSON fixtures in `src/group_chat/executor.rs` tests. **No read of the parsed value anywhere.**
- **Symptom**: The coordinator says "this topic needs synthesis" and nothing happens — the round proceeds with exactly the respondents it listed and no summary is produced. The prompt asks the model for a flag the runtime ignores, so the model spends tokens on a decision with no effect.
- **Hypothesis**: CUT
- **Confidence**: high
- **Severity**: medium

---

### SW-S6-GC-EXEC-7 — `GroupChatMessage.dropped_targets` is produced by the executor but dropped by every serializer — dropped-mention feedback never reaches the user
- **Form**: 1 (no consumer)
- **Producer**: `src/group_chat/executor.rs:565` — the final message of a round carries `dropped_targets: dropped_for_this_msg` (populated at `src/group_chat/executor.rs:328` when a mentioned persona is not in the coordinator plan).
- **Consumer**: **none found** — the RPC serializer `message_to_json` (`src/gateway/handlers/group_chat.rs:73-83`) emits `session_id`, `speaker`, `content`, `round`, `sequence`, `is_final` — **not** `dropped_targets`; the channel serializer `send_group_chat_messages` (`src/gateway/inbound_router/group_chat_handler.rs:408-420`) formats only `speaker.name()` and `content`.
- **Live caller search**:
  - `rg -n --glob '*.rs' 'dropped_targets' .` → `src/group_chat/protocol.rs:234` (field decl), `src/group_chat/protocol.rs:386` (default), `src/group_chat/executor.rs:318/328/352/548/554/565` (produce + move). **No read outside the producer.**
  - The only visible signal today is the `tracing::warn!` at `src/group_chat/executor.rs:323-328`, i.e. log-only.
- **Symptom**: A user mentions a persona in `group_chat.mention` and the coordinator silently excludes it. The RPC reply and the channel reply look identical to a normal round — the user is told the persona was addressed when it was not. The code comment at `src/group_chat/executor.rs:544-550` says the list "is surfaced to the caller via the last `GroupChatMessage` returned", but no caller reads it.
- **Hypothesis**: CUT
- **Confidence**: high
- **Severity**: medium

---

### SW-S6-GC-EXEC-8 — `GroupChatSession.source_channel` has no production reader
- **Form**: 1 (no consumer)
- **Producer**: `src/group_chat/session.rs:46` — `pub source_channel: String`, set from the caller at construction (`src/group_chat/session.rs:115`) and threaded from the RPC handler (`src/gateway/handlers/group_chat.rs:132-136`, default `"rpc"`) and the channel handler (`src/gateway/inbound_router/group_chat_handler.rs:189-195`).
- **Consumer**: **none found** — `rg -n --glob '*.rs' '\.source_channel\b' src/group_chat/ src/gateway/handlers/group_chat.rs src/gateway/inbound_router/group_chat_handler.rs` returns only two **test** assertions (`src/group_chat/session.rs:322`, `src/group_chat/orchestrator.rs:418`). The only non-test use is `src/group_chat/orchestrator.rs:199` → `insert_group_chat_session(…, &source_channel, …)`, which is itself dead (SW-S6-GC-EXEC-1). `handle_list` (`src/gateway/handlers/group_chat.rs:396-404`) does not expose it.
- **Live caller search**: the two greps above; whole-repo `rg -n 'source_channel' src/` shows no other `GroupChatSession` field reader (the many hits are the unrelated `source_channel_id` on cron jobs).
- **Symptom**: none user-visible — the origin channel of a session is recorded but never surfaced, filtered on, or persisted in a reachable store.
- **Hypothesis**: CUT
- **Confidence**: high
- **Severity**: low

---

## B. Clean / verified symbols (no severed wire)

- `GroupChatExecutor::new` — production caller `src/bin/aleph-server/commands/start/mod.rs:3485`.
- `GroupChatExecutor::with_coordinator_visible` — production caller `src/bin/aleph-server/commands/start/mod.rs:3486` <- `gc_config.coordinator_visible` (`src/config/types/group_chat.rs:41`); consumed at `src/group_chat/executor.rs:336`.
- `GroupChatExecutor::with_history_window` — production caller `src/bin/aleph-server/commands/start/mod.rs:3487` <- `gc_config.history_window_rounds` (`src/config/types/group_chat.rs:54`); consumed at `src/group_chat/executor.rs:269`.
- `GroupChatExecutor::execute_round` — production callers `src/gateway/handlers/group_chat.rs:164` (`handle_start`), `src/gateway/handlers/group_chat.rs:272` (`handle_continue_with_targets`), `src/gateway/inbound_router/group_chat_handler.rs:235` and `:388`.
- `GroupChatOrchestrator::new` — production caller `src/bin/aleph-server/commands/start/mod.rs:3470`; `config` + `persona_configs` both reach the registry (`src/group_chat/orchestrator.rs:64`).
- `GroupChatOrchestrator::create_session` — production callers `src/gateway/handlers/group_chat.rs:147`, `src/gateway/inbound_router/group_chat_handler.rs:189`.
- `GroupChatOrchestrator::get_session` — production callers `src/gateway/handlers/group_chat.rs:226/332/424`, `src/gateway/inbound_router/group_chat_handler.rs:133/288/343`.
- `GroupChatOrchestrator::end_session` — production callers `src/gateway/handlers/group_chat.rs:261/347`, `src/gateway/inbound_router/group_chat_handler.rs:300/372`.
- `GroupChatOrchestrator::max_rounds` — production callers `src/gateway/handlers/group_chat.rs:236`, `src/gateway/inbound_router/group_chat_handler.rs:345`; the same value is stamped onto the session at `src/group_chat/orchestrator.rs:166`.
- `GroupChatOrchestrator::all_sessions` — production caller `src/gateway/handlers/group_chat.rs:367` (`handle_list`). (The doc comment at `src/group_chat/orchestrator.rs:318-323` claiming "only the `#[cfg(test)]` block in this module iterates `all_sessions`" is stale — see Section C.)
- `GroupChatSession::new` — production caller `src/group_chat/orchestrator.rs:154` only.
- `GroupChatSession::with_max_rounds` — production caller `src/group_chat/orchestrator.rs:166`.
- `GroupChatSession::max_no_commit_attempts` — production caller `src/group_chat/executor.rs:248`.
- `GroupChatSession::add_turn` — production callers `src/group_chat/executor.rs:503/509`; `pub(crate)`, so no external bypass of the Active/monotonic gates.
- `GroupChatSession::build_history_text_windowed` — production caller `src/group_chat/executor.rs:269`.
- `GroupChatSession::build_history_text` — `pub(crate)` helper; production callers `src/group_chat/session.rs:238` (the `window_rounds == 0` fall-through inside the windowed variant) and the `recent` loop at `src/group_chat/session.rs:261-262` — live.
- `GroupChatSession::end` — production callers `src/group_chat/orchestrator.rs:267`, `src/gateway/handlers/group_chat.rs:256/355`, `src/gateway/inbound_router/group_chat_handler.rs:305/366`.
- `GroupChatSession::owner_user_id` — read at `src/gateway/handlers/group_chat.rs:247/339/442` and `src/gateway/inbound_router/group_chat_handler.rs:437` via `visibility::stamped_owner_visible`.
- `GroupChatSession::source_session_key` — read at `src/gateway/inbound_router/group_chat_handler.rs:436` (`session_visible_to_conversation`).
- `GroupChatSession::cancel_token` — cloned at `src/group_chat/orchestrator.rs:182`, fired at `src/group_chat/orchestrator.rs:262`, observed at `src/group_chat/executor.rs:285/292/429/444`.
- `GroupChatSession::history` — read at `src/gateway/handlers/group_chat.rs:451`; `current_round` at `:401` and `:468`; `topic` at `:399`; `created_at` at `:403`.
- `GroupChatOrchestrator::remove_session` — private helper, production caller `src/group_chat/orchestrator.rs:253`.
- `GroupChatExecutor::resolve_provider` / `lock_provider_fallback_warned` — private, called at `src/group_chat/executor.rs:403` / `:114`. (The registry branch is dead — SW-S6-GC-EXEC-4 — but the function itself is reached.)
- `GroupChatExecutor::persist_turn` — private, called at `src/group_chat/executor.rs:530`; reached, but its body returns immediately at `:159` (SW-S6-GC-EXEC-2).
- Round-loop reachability: the whole round path is reachable from two production entry points — the Gateway RPC `group_chat.start` / `group_chat.continue` / `group_chat.mention` (registered at `src/bin/aleph-server/commands/start/builder/handlers/system.rs:129-176`) and the channel `/groupchat` slash command (`src/gateway/inbound_router/group_chat_handler.rs:19`). Every value the callers pass reaches its consumer: `targets` -> `src/group_chat/executor.rs:270` -> `build_coordinator_prompt` (`src/group_chat/coordinator.rs:30/74-80`); `history_window_rounds` -> `src/group_chat/executor.rs:269`; `coordinator_visible` -> `src/group_chat/executor.rs:336`; `max_rounds` -> `src/group_chat/orchestrator.rs:166` + `src/group_chat/executor.rs:232`; `persona.model` -> `src/group_chat/executor.rs:435`; `persona.thinking_level` -> `src/group_chat/executor.rs:411`; `persona.system_prompt` -> `src/group_chat/executor.rs:434`; `persona.provider` -> `src/group_chat/executor.rs:105` (reached, but the registry arm inside it is dark).
- RPC registration parity: the six `group_chat.*` methods registered by `register_group_chat_handlers` (`src/bin/aleph-server/commands/start/builder/handlers/system.rs:129-176`) exactly match the six real handlers in `src/gateway/handlers/group_chat.rs`; the placeholder registrations in `src/gateway/handlers/mod.rs:887-896` are overwritten at boot because `HandlerRegistry::register` uses `HashMap::insert` (`src/gateway/handlers/mod.rs:1259-1261`) — the real handlers win, so there is no ghost method and no NOT_FOUND. A registration-parity test exists at `src/gateway/handlers/group_chat.rs:697`.
- `GroupChatConfig::validate()` is reached in production (`src/config/validate.rs:524`), so `max_rounds == 0` / `max_personas_per_session == 0` are rejected before the orchestrator is built.

## C. New observations (not findings)

- **Stale doc, live symbol**: `src/group_chat/orchestrator.rs:318-323` says `all_sessions` is used only by the module's `#[cfg(test)]` block and was demoted to `pub(crate)` accordingly. It now has a production caller (`src/gateway/handlers/group_chat.rs:360`). The demotion itself is harmless (same crate), but the comment now misdescribes production behaviour.
- **Stale doc, dead symbol**: `src/gateway/handlers/group_chat.rs:5-6` says the `handle_xxx_placeholder` variants are "used in `HandlerRegistry::new()`" — that is true (`src/gateway/handlers/mod.rs:887-896`), but `RUNTIME_REQUIRED` ("requires GroupChatOrchestrator runtime - wire Gateway first") is therefore never seen by a real client in the wired server, since the boot-time registration overwrites all six.
- **No-client surface**: `rg -n 'group_chat\.' interfaces/ desktop/` returns nothing — the `group_chat.*` RPC methods have no Panel/TS client. The group-chat feature is reachable only via the JSON-RPC Gateway and the channel slash command. (Not a severed wire: the RPC methods are a public Gateway surface with live server-side handlers.)
- **Round-limit error semantics**: `src/group_chat/executor.rs:232-241` returns `GroupChatError::SessionInactive("round N exceeds max_rounds …")` for a budget overrun, and `src/group_chat/executor.rs:248-256` returns the same variant for the no-commit budget. Both are surfaced as generic `INTERNAL_ERROR` by the RPC handlers (`src/gateway/handlers/group_chat.rs:172/288`) and as "Round failed: …" on the channel path — the user cannot tell "budget exhausted" from "provider failed".
- **In-memory/DB asymmetry acknowledged in-tree**: `src/group_chat/executor.rs:519-525` notes that `session.current_round` is advanced before the persistence loop, so a cancel in between leaves the in-memory session one round ahead of `group_chat_turns`. Currently unobservable because `group_chat_turns` is never written (SW-S6-GC-EXEC-2).

## D. Coverage statement

**Files read in full (slice)**: `src/group_chat/executor.rs` (1324 lines, incl. its `#[cfg(test)]` block), `src/group_chat/session.rs` (433), `src/group_chat/orchestrator.rs` (629).

**Files read as far-end / seam (not in slice)**: `src/gateway/handlers/group_chat.rs` (real + placeholder handlers, `message_to_json`), `src/gateway/handlers/mod.rs:870-910` (placeholder registrations) + `:1240-1270` (`HandlerRegistry::register` overwrite semantics), `src/gateway/inbound_router/group_chat_handler.rs` (channel `/groupchat` path, `send_group_chat_messages`), `src/gateway/inbound_router/mod.rs` (`with_group_chat`, `group_chat_orch`/`group_chat_executor` fields), `src/bin/aleph-server/commands/start/mod.rs:3460-3500` (production wiring), `src/bin/aleph-server/commands/start/builder/handlers/system.rs:127-178` (`register_group_chat_handlers`), `src/bin/aleph-server/commands/start/builder/subsystems.rs:725-835` (inbound-router wiring), `src/resilience/database/group_chat.rs` (CRUD), `src/config/types/group_chat.rs` (config struct), `src/group_chat/persona.rs` (config→`Persona` copy), `src/group_chat/protocol.rs` (message/status/plan/error types), `src/config/validate.rs:524` (config validation).

**Lenses applied**: registration parity (RPC method names vs registered handlers, placeholder vs real), call-vs-handler parity (`with_database`, `with_provider_registry`, `insert_group_chat_*`, `get_group_chat_turns`), classifier-vs-handler parity (the `max_rounds` / `no_commit_attempts` budget gates vs the handlers that enforce them), event emit-vs-subscribe parity (not applicable — this slice has no bus/events), config-reader parity (all 4 `GroupChatConfig` fields + all 6 `PersonaConfig` fields), path/route parity (not applicable — no HTTP paths in this slice), stub sweep (`todo!`, `unimplemented!`, `RUNTIME_REQUIRED`, `Ok(None)` fallbacks).

**Seams not covered and why**:
- `src/group_chat/coordinator.rs` (prompt building / plan parsing) is **not** in the slice; it was read only for the `targets` and `need_summary` wires. A full stub sweep of its parse fallbacks is out of scope.
- `src/group_chat/channel.rs`, `src/group_chat/mod.rs`, `src/group_chat/protocol.rs` command-parser side (`DefaultGroupChatCommandParser`) were not audited for their own severed wires; only the executor/session/orchestrator surface is covered.
- The `StateDatabase` schema/migration side (`src/resilience/database/state_database/schema.rs:282`) was not verified — I confirmed the *Rust call sites* are dead, not that the tables exist or are migrated correctly.
- No runtime/dynamic verification was performed: the task forbids running `cargo`, so all findings are static. The claim "no production caller" for `with_database` rests on a whole-repo `rg` for the identifier, which cannot see a caller generated by a macro (`paste!`, `stringify!`, builder codegen). I found no such macro in this codebase — `rg -n 'with_database'` returns exactly the two definitions and no `macro_rules!`/`paste` usage near them — but a macro-generated call cannot be excluded with 100% certainty.
- `desktop/nsh-shell` and `interfaces/webchat` were checked only for `group_chat.*` RPC usage (none found); the desktop host's own RPC surface was not audited.
