# S5 — Severed-wire audit: `src/group_chat/` API slice

- **Slice**: `src/group_chat/mod.rs`, `src/group_chat/channel.rs`, `src/group_chat/coordinator.rs`, `src/group_chat/protocol.rs`, `src/group_chat/persona.rs`
- **Repo / HEAD**: `/home/zou/data/workspace/Aleph` @ `1b959fdf4` ("Merge origin/main with the Prism Panel integration")
- **Mode**: READ-ONLY. No file was edited, no git command mutating state was run, no cargo command was run.
- **Adjacent files read for far-end confirmation**: `src/group_chat/executor.rs`, `src/group_chat/orchestrator.rs`, `src/group_chat/session.rs`, `src/gateway/handlers/group_chat.rs`, `src/gateway/inbound_router/group_chat_handler.rs`, `src/gateway/handlers/mod.rs`, `src/gateway/server/mod.rs`, `src/config/types/group_chat.rs`, `src/config/validate.rs`, `src/bin/aleph-server/commands/start/mod.rs`, `src/bin/aleph-server/commands/start/builder/handlers/system.rs`, `crates/aleph-slot-api/src/ai.rs`.

## A. Findings

| ID | Form | Severity | Title | Producer | Consumer |
|----|------|----------|-------|----------|----------|
| SW-S5-GC-API-1 | 3 | medium | Group-chat persistence is dark: neither `with_database` builder is ever called, so all four DB CRUD calls are unreachable | `src/group_chat/orchestrator.rs:70`, `src/group_chat/executor.rs:95` | none found |
| SW-S5-GC-API-2 | 3 | medium | `Persona.provider` / `PersonaConfig.provider` is inert: the executor's provider registry is `None` in production | `src/group_chat/persona.rs:70` → `src/group_chat/protocol.rs:72` | `src/group_chat/executor.rs:105` (dead in production) |
| SW-S5-GC-API-3 | 3 | medium | `CoordinatorPlan.need_summary` is requested from the LLM, parsed, and never read | `src/group_chat/coordinator.rs:89`, `src/group_chat/protocol.rs:286` | none found |
| SW-S5-GC-API-4 | 5 | medium | `GroupChatMessage.dropped_targets` is produced by the executor and omitted by the only outbound serializer | `src/group_chat/executor.rs:318`, `src/group_chat/protocol.rs:234` | `src/gateway/handlers/group_chat.rs:73` (omits the field) |
| SW-S5-GC-API-5 | 1 | medium | The whole `group_chat.*` RPC surface is registered but has no in-repo client | `src/bin/aleph-server/commands/start/builder/handlers/system.rs:137` | none found |
| SW-S5-GC-API-6 | 5 | low | `Persona::validate` rejects `thinking_level = "off"` / `"minimal"` although the far end accepts them | `src/group_chat/protocol.rs:52`, `src/group_chat/protocol.rs:133` | `crates/aleph-slot-api/src/ai.rs:208` |
| SW-S5-GC-API-7 | 2 | low | The `group_chat.*` placeholder stubs stay live when no provider key is configured and misreport the cause | `src/gateway/handlers/mod.rs:887` | `src/gateway/handlers/group_chat.rs:478` |
| SW-S5-GC-API-8 | 5 | low | `channel.rs` advertises a channel-adapter trait + per-channel renderer that exist nowhere; rendering is hardcoded in the inbound router | `src/group_chat/channel.rs:1-7`, `src/group_chat/channel.rs:22-23` | `src/gateway/inbound_router/group_chat_handler.rs:414` |
| SW-S5-GC-API-9 | 1 | low | `GroupChatRequest` / `GroupChatMessage` carry a serde + JsonSchema wire contract that no peer ever reads or writes | `src/group_chat/protocol.rs:167`, `src/group_chat/protocol.rs:211` | none found |

**Severity tally**: high 0 · medium 5 · low 4 · (UNCERTAIN 0)

### SW-S5-GC-API-1 — Group-chat persistence is dark: neither `with_database` builder is ever called
- **Form**: 3 (inert config / severed persistence wire) — also Form 1 for the two builders themselves
- **Producer**: `src/group_chat/orchestrator.rs:70` — `pub fn with_database(mut self, db: Arc<StateDatabase>) -> Self`; `src/group_chat/executor.rs:95` — same builder on the executor. Both are the *only* writers of the private `db: Option<Arc<StateDatabase>>` field (`src/group_chat/orchestrator.rs:49`, `src/group_chat/executor.rs:36`).
- **Consumer**: **none found**. Every DB call in the module is gated on `self.db`:
  - `src/group_chat/orchestrator.rs:194` `db.insert_group_chat_session(...)` ← `if let Some(db) = &self.db`
  - `src/group_chat/orchestrator.rs:273` `db.update_group_chat_session_status(&session_id, GroupChatStatus::Ended.as_str())` ← `if let Some(db) = &self.db`
  - `src/group_chat/executor.rs:167` `insert_group_chat_turn(...)` ← reached only through `persist_turn`, which is called at `src/group_chat/executor.rs:531` under the executor's `db` check.
  - The four DB primitives themselves live at `src/resilience/database/group_chat.rs:35,74,101,143` and each has exactly one production caller — all of them inside the never-taken `Some(db)` arms.
- **Live caller search**:
  - `rg -n --glob '*.rs' 'with_database' src/ tests/` → **only the two definitions** (`src/group_chat/executor.rs:95`, `src/group_chat/orchestrator.rs:70`). Zero call sites, zero test call sites.
  - `rg -n --glob '*.rs' 'GroupChatOrchestrator::new|GroupChatExecutor::new' src/ tests/` → production: `src/bin/aleph-server/commands/start/mod.rs:3470` `GroupChatOrchestrator::new(gc_config, &persona_configs)` and `src/bin/aleph-server/commands/start/mod.rs:3485` `GroupChatExecutor::new(handle).with_coordinator_visible(..).with_history_window(..)` — neither chains `.with_database(..)`. All other hits are `#[cfg(test)]` constructions inside `src/group_chat/orchestrator.rs` and `src/group_chat/executor.rs`.
  - `rg -n --glob '*.rs' 'insert_group_chat_session|update_group_chat_session_status|insert_group_chat_turn|get_group_chat_turns' src/` → 4 definitions + 3 in-module call sites, all under the `db` guard.
- **Symptom**: A group-chat session lives only in the orchestrator's in-memory `HashMap` (`src/group_chat/orchestrator.rs:54`). Restarting `aleph-server` destroys every session and its transcript; `group_chat.history` (`src/gateway/handlers/group_chat.rs:413`) answers from `session.history` only, so a restarted server cannot serve history for a session it no longer holds. The `state` DB tables for group chat stay empty forever. No log line says so — the `tracing::warn!` branches (`orchestrator.rs:200`, `orchestrator.rs:276`) only fire when a DB call *fails*, and the call is never attempted.
- **Hypothesis**: **CUT** — the builder exists and the DB layer exists, but the middle arm (startup → builder) is missing. (Prior audit `review-results/severed-wire-2026-09-04-g5g/group_chat-raw.json` finding `sw-group_chat-1-1` reported the same two inert builders on 2026-09-04; it is **still open at HEAD**.)
- **Confidence**: high
- **Severity**: medium

### SW-S5-GC-API-2 — `Persona.provider` / `PersonaConfig.provider` is inert in production
- **Form**: 3 (inert config)
- **Producer**: `src/config/types/group_chat.rs:115` — `pub provider: Option<String>` on `PersonaConfig` (documented as "Optional AI provider override (e.g., \"anthropic\", \"openai\")", example at `src/config/types/group_chat.rs:99-104`) → copied at `src/group_chat/persona.rs:70` (`provider: cfg.provider.clone()`) → stored on `src/group_chat/protocol.rs:72` (`pub provider: Option<String>`).
- **Consumer**: `src/group_chat/executor.rs:105` — `if let Some(ref provider_name) = persona.provider { if let Some(ref registry) = self.provider_registry { … } }`. The inner arm is **never entered in production**: `self.provider_registry` is `Option<Arc<ProviderRegistry>>` (`src/group_chat/executor.rs:34`), initialised to `None` at `src/group_chat/executor.rs:58`, and only settable through `with_provider_registry` (`src/group_chat/executor.rs:73`). The executor therefore always falls through to `self.default_provider.current()` (`src/group_chat/executor.rs:129`).
- **Live caller search**:
  - `rg -n --glob '*.rs' 'with_provider_registry|provider_registry' .` → the only two call sites of `with_provider_registry` are `src/group_chat/executor.rs:1032` and `src/group_chat/executor.rs:1204`, both **inside `#[cfg(test)] mod tests` (which begins at `src/group_chat/executor.rs:577-578`)**. Every other hit is an unrelated `ProviderRegistry` (`src/capability/provider_registry.rs`, `src/gateway/provider_factory.rs:191`, `src/tools/registry.rs`).
  - `rg -n --glob '*.rs' 'GroupChatExecutor::new' src/` → production `src/bin/aleph-server/commands/start/mod.rs:3485`; the builder chain there is `.with_coordinator_visible(coordinator_visible).with_history_window(history_window_rounds)` — **no** `.with_provider_registry(..)`, even though the very same block creates a registry via `create_provider_registry_from_env()` (`src/bin/aleph-server/commands/start/mod.rs:3476`) and only keeps `reg.default_provider()` (`src/bin/aleph-server/commands/start/mod.rs:3480-3482`).
  - `rg -n --glob '*.rs' '\bprovider\b' src/group_chat/` → the only non-test read of `persona.provider` is `src/group_chat/executor.rs:105`.
- **Symptom**: An operator writes `[[personas]] provider = "anthropic"` (or `"openai"`) for a persona. The persona silently runs on the **default** provider instead. No warning is emitted: the fallback warning at `src/group_chat/executor.rs:120-127` lives *inside* the `if let Some(ref registry)` arm, so the "provider not found in registry" message can never fire either — the misconfiguration is completely silent. Contrast with the sibling field `model`, which *is* honoured (`src/group_chat/executor.rs:438` `.with_model(persona.model.clone())`, real impl at `crates/aleph-slot-api/src/ai.rs:180`), and `thinking_level`, which *is* honoured (`src/group_chat/executor.rs:411`).
- **Hypothesis**: **CUT** — the middle arm (startup → `with_provider_registry`) is missing. (Prior audit `sw-group_chat-2-1` on 2026-09-04 called this "a designed feature … but the wiring from startup to executor is missing"; **still open at HEAD**.) Whether the feature should live at all is a product call → **DECIDE** on scope, **CUT** on the wiring.
- **Confidence**: high
- **Severity**: medium

### SW-S5-GC-API-3 — `CoordinatorPlan.need_summary` is requested from the LLM, parsed, and never read
- **Form**: 3 (inert field — a produced value with no consumer)
- **Producer**:
  - `src/group_chat/coordinator.rs:89` — the coordinator prompt instructs the model: `{"respondents":[…],"need_summary":bool}`, and `src/group_chat/coordinator.rs:93` — "Set need_summary to true if the topic is complex and benefits from synthesis."
  - `src/group_chat/protocol.rs:286` — `pub need_summary: bool` on `CoordinatorPlan` (serde `default` at `src/group_chat/protocol.rs:281-284`).
  - `src/group_chat/coordinator.rs:106` — `parse_coordinator_plan` deserialises it via `serde_json::from_str` (`src/group_chat/coordinator.rs:104`).
  - `src/group_chat/coordinator.rs:131` — the fallback plan always sets it to `false`.
- **Consumer**: **none found**.
- **Live caller search**:
  - `rg -n --glob '*.rs' 'need_summary' .` → 32 hits. Every hit is either (a) the definition (`src/group_chat/protocol.rs:286`), (b) the prompt text / fallback constant (`src/group_chat/coordinator.rs:89,93,131`), (c) a JSON *string literal inside a test fixture* (`src/group_chat/executor.rs:664,711,806,859,885,918,978,1021,1060,1101,1163,1241,1289,1291,1292`), or (d) an assertion inside `#[cfg(test)]` (`src/group_chat/coordinator.rs:331,340,357,385`).
  - The single production consumer of the plan is `src/group_chat/executor.rs:301` (`parse_coordinator_plan(&coordinator_raw)`), which then reads `plan.respondents` (`src/group_chat/executor.rs:321`) and never `plan.need_summary`.
- **Symptom**: The coordinator is explicitly asked to flag a round that "benefits from synthesis", the flag is faithfully parsed, and then it is dropped on the floor. A user who asks a complex multi-persona question and gets a round of opinions gets **no summary**, and there is no log line, no message, and no error to explain why. The feature is *documented as existing* on the struct itself (`src/group_chat/protocol.rs:279-282`: "Whether a summary should be generated after all respondents have spoken").
- **Hypothesis**: **CUT** — the consumer (summary generation) was never written; the field is the orphaned half of the contract.
- **Confidence**: high
- **Severity**: medium

### SW-S5-GC-API-4 — `GroupChatMessage.dropped_targets` is produced by the executor and omitted by the only outbound serializer
- **Form**: 5 (name/field drift: the hand-written JSON shadow of the struct dropped a field the struct carries)
- **Producer**:
  - `src/group_chat/executor.rs:318-330` — the executor computes the mentions the coordinator's plan dropped and pushes them into `dropped_targets`, warning per drop (`src/group_chat/executor.rs:322-326`).
  - `src/group_chat/executor.rs:554-565` — the list is attached to the **final** message of the round (`std::mem::take(&mut dropped_targets)` at `src/group_chat/executor.rs:554`).
  - `src/group_chat/protocol.rs:225-234` — the field and its contract: "Mentions that were NOT included in the coordinator's plan … Populated by the executor on the LAST message of the round … Keeping the data on every message (rather than as a side-channel field) keeps the streaming consumer's view self-contained: when it sees the final message it can both finalize the round AND surface the dropped list without re-reading prior frames."
- **Consumer**: **none found**. The two outbound paths are:
  - `src/gateway/handlers/group_chat.rs:73-80` — `fn message_to_json(msg: &GroupChatMessage) -> Value` hand-builds the JSON with `session_id`, `speaker`, `content`, `round`, `sequence`, `is_final` — **no `dropped_targets`**. Used at `src/gateway/handlers/group_chat.rs:166` and `:276` (the `group_chat.start` and `group_chat.continue` responses). Note this manual serializer also shadows the derived `Serialize` on the struct, which *would* have carried the field.
  - `src/gateway/inbound_router/group_chat_handler.rs:411-415` — the channel renderer formats `format!("**[{}]**: {}", gc_msg.speaker.name(), gc_msg.content)` and likewise omits it.
  - `rg -n --glob '*.rs' 'dropped_targets' .` → 8 hits: the definition (`src/group_chat/protocol.rs:234`), the test literal (`src/group_chat/protocol.rs:386`), and the executor's producer sites (`src/group_chat/executor.rs:318,328,352,554,565`). **Zero consumer hits.**
- **Symptom**: A user writes `/groupchat mention <sid> @expert please weigh in`. If the coordinator's plan omits `@expert`, the executor logs a warning (`src/group_chat/executor.rs:322`) and records it on the final message — and the record is then dropped by both serializers. The user sees the other personas answer and **no indication that their mention was ignored**. The doc comment at `src/group_chat/protocol.rs:225-232` promises a consumer-visible dropped list that does not exist.
  - Worse edge case: when the coordinator returns a plan with **zero** respondents, `messages` is empty (`src/group_chat/executor.rs:533-566` only iterates `prepared`), so the round returns `[]` and `dropped_targets` is never attached to anything at all. The RPC then answers `{"session_id": …, "messages": []}` (`src/gateway/handlers/group_chat.rs:167-171`).
- **Hypothesis**: **CUT** — the producer arm exists, the consumer arm (serialise the field / render it) does not.
- **Confidence**: high
- **Severity**: medium
