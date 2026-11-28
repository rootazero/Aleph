# S8 — Cross-boundary severed-wire audit: group_chat (gateway ↔ channel ↔ boot)

**Slice**: `src/gateway/inbound_router/group_chat_handler.rs`, `src/gateway/handlers/group_chat.rs`,
`src/gateway/handlers/mod.rs` (group_chat registration parts),
`src/bin/aleph-server/commands/start/**` (group_chat wiring parts).
**Repo HEAD**: `1b959fdf4`. **Mode**: read-only (no edits, no cargo).
**Adjacent sibling reports** (already cover the group_chat *library* internals, so this report
deliberately does not re-report them): `s5-gc-api.md` (SW-S5-GC-API-1..4),
`s6-gc-exec.md` (SW-S6-GC-EXEC-1..8). Findings **1, 4, 5** below overlap those reports and are
marked `DUP` so the parent can de-duplicate; the *cross-boundary* consequence is stated in each.

---

## A. Findings

| ID | Form | Severity | Title | Producer | Consumer |
|----|------|----------|-------|----------|----------|
| SW-S8-CROSS-GROUPCHAT-1 | 1 | high | `GroupChatOrchestrator::with_database` + `GroupChatExecutor::with_database` are never called at boot → the whole `group_chat_sessions`/`group_chat_turns` persistence chain is dark | `src/group_chat/orchestrator.rs:70`, `src/group_chat/executor.rs:95` | none (boot wiring `src/bin/aleph-server/commands/start/mod.rs:3470` omits it) |
| SW-S8-CROSS-GROUPCHAT-2 | 2 | medium | The six `group_chat.*` placeholder handlers stay registered when the executor is absent → production RPCs answer "requires GroupChatOrchestrator runtime - wire Gateway first" | `src/gateway/handlers/mod.rs:887-895` | `src/gateway/handlers/group_chat.rs:479-525` (placeholders) — never overridden on the no-provider path |
| SW-S8-CROSS-GROUPCHAT-3 | 1 | medium | No in-repo client calls any `group_chat.*` RPC — the whole JSON-RPC surface is reachable only through the generic `aleph gateway call` passthrough | `src/bin/aleph-server/commands/start/builder/handlers/system.rs:137-170` | none found (only `src/bin/aleph-server/cli.rs:345` generic passthrough) |
| SW-S8-CROSS-GROUPCHAT-4 | 1 | medium | `GroupChatMessage.dropped_targets` is produced by the executor and omitted by the only outbound serializer | `src/group_chat/executor.rs:565` | `src/gateway/handlers/group_chat.rs:73-86` (omits it); `src/gateway/inbound_router/group_chat_handler.rs:408-420` (omits it) |
| SW-S8-CROSS-GROUPCHAT-5 | 3 | low | `GroupChatSession.source_channel` is written on every session and read by nobody in production | `src/group_chat/session.rs:46` | none (only `src/group_chat/session.rs:322-323` and `src/group_chat/orchestrator.rs:418-419`, both tests) |
| SW-S8-CROSS-GROUPCHAT-6 | 5 / 1 | medium | `/groupchat end` with no session id is documented but rejected by the parser → the handler's "end the active session" branch is unreachable | `src/group_chat/channel.rs:20` (doc), `src/group_chat/channel.rs:50-58` (parser) | `src/gateway/inbound_router/group_chat_handler.rs:271-278` (dead branch) |
| SW-S8-CROSS-GROUPCHAT-7 | 2 | medium | The unified `/groupchat` intercept swallows any command the group-chat parser does not recognise — no reply, no error, no agent fallback | `src/gateway/inbound_router/mod.rs:1047-1050` | `src/gateway/inbound_router/group_chat_handler.rs:18-41` (returns `Ok(())` at `:40`) |
| SW-S8-CROSS-GROUPCHAT-8 | 5 | medium | The catalogued usage of `builtin:groupchat` (`/groupchat start <personas> [topic]`) does not match the parser grammar, so the advertised invocation is silently swallowed | `src/tool_metadata/registry/registration.rs:106-114` | `src/group_chat/channel.rs:131-189` (requires `--preset` / `--role`) |
| SW-S8-CROSS-GROUPCHAT-9 | 3 | medium | `SCOPED_METHODS` / `treatment_of` — the table that records the per-user filtering ruling for the five addressed `group_chat.*` methods — has no production consumer | `src/gateway/method_visibility.rs:530-660` | none (only `src/gateway/method_visibility.rs` `#[cfg(test)] mod tests`) |
| SW-S8-CROSS-GROUPCHAT-10 | 5 | low | `group_chat.list` returns the session id under the key `id`, while `group_chat.continue/mention/history/end` require `session_id` — the list→act handoff needs a key rename the contract never states | `src/gateway/handlers/group_chat.rs:398` (`"id"`) | `src/gateway/handlers/group_chat.rs:209, 302, 310, 414` (`"session_id"`) |
| SW-S8-CROSS-GROUPCHAT-11 | 3 | low | `group_chat.start` is listed in `MEMBER_CARVE_OUTS`, but the `group_chat.` family is not in `ADMIN_PREFIXES` → the carve-out is inert (the whole family is member-open by default) | `src/gateway/method_admin.rs:935` | `src/gateway/method_admin.rs:107-348` (`ADMIN_PREFIXES` has no `group_chat.`) |

**Severity tally**: high 1 · medium 7 · low 3 · (UNCERTAIN 0)

---

### SW-S8-CROSS-GROUPCHAT-1 — Group-chat persistence chain is dark end-to-end (boot wiring omits `.with_database`)

- **Form**: 1 (no consumer) — with a 2 (stub far-end) face at the persistence call sites.
- **Producer**: `src/group_chat/orchestrator.rs:70` — `pub fn with_database(mut self, db: Arc<StateDatabase>) -> Self`
  and `src/group_chat/executor.rs:95` — `pub fn with_database(mut self, db: Arc<StateDatabase>) -> Self`.
- **Consumer**: **none found**. The production constructor is
  `src/bin/aleph-server/commands/start/mod.rs:3470` — `let orchestrator = GroupChatOrchestrator::new(gc_config, &persona_configs);`
  — with **no** `.with_database(...)`; the executor is built at
  `src/bin/aleph-server/commands/start/mod.rs:3485-3489` — `GroupChatExecutor::new(handle).with_coordinator_visible(..).with_history_window(..)` — also with no `.with_database(...)`.
- **Live caller search**:
  - `rg -n 'with_database' --glob '*.rs' .` → exactly two hits, both the *definitions*:
    `src/group_chat/executor.rs:95`, `src/group_chat/orchestrator.rs:70`. **Zero call sites.**
  - `rg -n 'insert_group_chat_session|update_group_chat_session_status' --glob '*.rs' .` → only
    `src/group_chat/orchestrator.rs:195` and `:273`, both inside `if let Some(db) = &self.db { … }`
    (orchestrator.rs:193 / :271) — unreachable because `db` is always `None`.
  - `rg -n 'insert_group_chat_turn|get_group_chat_turns' --glob '*.rs' .` → `insert_group_chat_turn` is
    called only from `src/group_chat/executor.rs:167`, inside `persist_turn`, whose first statement is
    `let Some(db) = &self.db else { return };` (`src/group_chat/executor.rs:159`) — unreachable.
    `get_group_chat_turns` (`src/resilience/database/group_chat.rs:143`) has **zero** callers repo-wide
    (the only other mention is a doc comment at `src/group_chat/session.rs:162`).
  - The tables themselves exist and are created on every boot:
    `src/resilience/database/state_database/schema.rs:278` — `CREATE TABLE IF NOT EXISTS group_chat_sessions`
    (and `group_chat_turns` at `:294`).
- **Symptom**: a group chat is pure in-process state. `group_chat.history` / `group_chat.list` see only
  sessions created since the last restart; the `/groupchat` channel path and the RPC path diverge after a
  restart; `group_chat_sessions` / `group_chat_turns` are created in SQLite and never contain a row.
  There is no user-visible error — the feature silently loses its history.
- **Hypothesis**: **CUT** — the producer (the persistence builders + the SQLite tables) exists, the
  consumer (the boot wiring) omits the call. *DUP: also reported as SW-S5-GC-API-1 /
  SW-S6-GC-EXEC-1/2/3; the cross-boundary contribution is that the cut is in
  `src/bin/aleph-server/commands/start/mod.rs:3460-3501`, i.e. the boot wiring, not in the library.*
- **Confidence**: high.
- **Severity**: high.

---

### SW-S8-CROSS-GROUPCHAT-2 — The placeholder handlers remain the live handlers whenever the executor is absent

- **Form**: 2 (stub far-end).
- **Producer**: `src/gateway/handlers/mod.rs:887-895` — `HandlerRegistry::new()` registers
  `group_chat.start` / `.continue` / `.mention` / `.end` / `.list` / `.history` to
  `group_chat::handle_*_placeholder`.
- **Consumer**: `src/gateway/handlers/group_chat.rs:477` —
  `const RUNTIME_REQUIRED: &str = "requires GroupChatOrchestrator runtime - wire Gateway first";`
  used by `handle_start_placeholder` (`:479`), `handle_continue_placeholder` (`:487`),
  `handle_mention_placeholder` (`:495`), `handle_end_placeholder` (`:503`),
  `handle_list_placeholder` (`:511`), `handle_history_placeholder` (`:519`).
  The real handlers (`group_chat::handle_start` … `handle_history`) are registered **only** at
  `src/bin/aleph-server/commands/start/builder/handlers/system.rs:129-173`, and that function is
  called **only** from `src/bin/aleph-server/commands/start/mod.rs:3495`, guarded by
  `if let Some(ref executor) = gc_executor` (`mod.rs:3494`).
- **Live caller search**:
  - `rg -n 'handle_start_placeholder|handle_continue_placeholder|handle_mention_placeholder|handle_end_placeholder|handle_list_placeholder|handle_history_placeholder' .`
    → only the definitions (`src/gateway/handlers/group_chat.rs:479-519`) and the six
    `registry.register(...)` lines (`src/gateway/handlers/mod.rs:887-895`).
  - `rg -n 'group_chat::handle_' .` → **zero** hits outside the definition file; the real handlers are
    reached through the `group_chat_handlers::` alias
    (`src/bin/aleph-server/commands/start/builder/handlers/mod.rs:15`:
    `use alephcore::gateway::handlers::group_chat as group_chat_handlers;`).
  - `rg -n 'register_group_chat_handlers' .` → `src/bin/aleph-server/commands/start/mod.rs:37` (import),
    `:3495` (the single call), `src/bin/aleph-server/commands/start/builder/handlers/system.rs:129` (definition).
  - `rg -n 'fn can_create_provider_from_env' -A 25 src/gateway/provider_factory.rs` → `:211-220`: true
    only when `ANTHROPIC_API_KEY` or `OPENAI_API_KEY` is set and non-empty.
  - The placeholders really are installed in production: `GatewayServer` builds its registry with
    `handlers: Arc::new(HandlerRegistry::new())` (`src/gateway/server/mod.rs:561` and `:628`), and
    `HandlerRegistry::new()` is the function that registers the six placeholders.
  - Override mechanics confirmed: `HandlerRegistry::register` is a `HashMap::insert`
    (`src/gateway/handlers/mod.rs:1250-1259`), so the override *would* win — it is simply not executed
    on the no-provider path.
  - The same condition also gates the channel path: `src/bin/aleph-server/commands/start/builder/subsystems.rs:826-833`
    installs the orchestrator on the router only `if let Some(executor) = group_chat_executor`, so with no
    provider key the RPC path serves the stub error while the channel path is dark too — two different
    symptoms for one missing prerequisite.
- **Symptom**: on a deployment whose provider is configured through the **file** config (not env
  keys), `aleph gateway call group_chat.list` answers
  `INTERNAL_ERROR "group_chat.list requires GroupChatOrchestrator runtime - wire Gateway first"` —
  a message that blames an already-completed wiring step. The operator cannot tell "group chat is
  disabled" from "the Gateway was never wired".
- **Hypothesis**: **CUT** — the real far end exists but the arm that installs it is conditional, and the
  stub is left as the production answer. Secondary smell: group chat is the only subsystem in this boot
  file whose provider is taken from **env only**
  (`src/bin/aleph-server/commands/start/mod.rs:3475-3481`), even though the file-config provider
  registry is built elsewhere in the same boot (`src/bin/aleph-server/commands/start/builder/agent_init/provider_registry.rs`).
- **Confidence**: high.
- **Severity**: medium.

---

### SW-S8-CROSS-GROUPCHAT-3 — No in-repo client calls any `group_chat.*` RPC

- **Form**: 1 (no consumer).
- **Producer**: the six registrations
  `src/bin/aleph-server/commands/start/builder/handlers/system.rs:137-170`
  (`group_chat.start`, `.continue`, `.mention`, `.end`, `.list`, `.history`) plus the placeholder
  registrations `src/gateway/handlers/mod.rs:887-895`; the methods are also *advertised* in the
  startup banner `src/bin/aleph-server/commands/start/builder/handlers/system.rs:177-182`.
- **Consumer**: **none found** other than the generic passthrough.
- **Live caller search**:
  - `rg -n --glob '!docs/**' --glob '!review-results/**' --glob '!archive/**' --glob '!graphify-out/**' --glob '!target/**' --glob '!*.log' 'group_chat\.' .`
    → hits only in `src/config/validate.rs:524,526` (config validation), the two registration files
    above, `src/gateway/method_admin.rs:935`, `src/gateway/method_visibility.rs:650-654`
    (policy table rows; the remaining hits in that file are doc comments at `:288`, `:298`, `:433`,
    `:466`, `:753`), and the banner prints. **No call site of any shape.**
  - `rg -n 'group_chat|groupchat' shared/ --glob '*.rs' --glob '*.ts'` → **no hits**; `rg -n 'group_chat|groupchat' src/mcp/` → **no hits**.
  - The Panel's "group chat" UI is a *different* feature: `interfaces/webchat/src/platform/wide/views/chat/team_compose.rs`
    calls `TeamsApi::create` (the `teams.*` family), and
    `interfaces/webchat/src/components/chat_sidebar.rs:1691` only renders an i18n label
    (`chat.group_chats_count`) for team groups.
  - The only generic entry point is `aleph gateway call <method> -p '<json>'`
    (`src/bin/aleph-server/cli.rs:345-360`, `GatewayAction::Call`).
- **Symptom**: none user-visible; the surface is "present but dark" for the shipped product. An
  operator reading the startup banner (`Group Chat methods: group_chat.start : Start a new group chat
  session …`) sees six advertised RPCs that no shipped client uses.
- **Hypothesis**: **CONNECT-with-a-caveat / DECIDE** — the JSON-RPC surface is a public server API and
  an operator can drive it via `aleph gateway call`; but no first-party client (Panel, TUI, CLI
  subcommand, MCP bridge) calls it. Whether this is "dead API" or "API for external clients" is a
  product decision; the audit records the fact, not the verdict.
- **Confidence**: high (the absence is confirmed); **verdict** UNCERTAIN — evidence that would settle
  it: a shipped client (Panel/TS) or an operator runbook that calls `group_chat.*`.
- **Severity**: medium.

---

### SW-S8-CROSS-GROUPCHAT-4 — `dropped_targets` never reaches any consumer (DUP of SW-S5-GC-API-4 / SW-S6-GC-EXEC-7)

- **Form**: 1 (no consumer).
- **Producer**: `src/group_chat/executor.rs:565` — `dropped_targets: dropped_for_this_msg` on the
  final message of a round (and `src/group_chat/executor.rs:352` for the coordinator message).
  The field's contract is stated at `src/group_chat/protocol.rs:224-234`: *"Populated by the executor on
  the LAST message of the round (`is_final == true`) … keeps the streaming consumer's view
  self-contained: when it sees the final message it can both finalize the round AND surface the
  dropped list without re-reading prior frames."*
- **Consumer**: **none found**. The only outbound serializers are
  `src/gateway/handlers/group_chat.rs:73-86` (`message_to_json` → `session_id`, `speaker`, `content`,
  `round`, `sequence`, `is_final` — **no `dropped_targets`**) and
  `src/gateway/inbound_router/group_chat_handler.rs:408-420`
  (`format!("**[{}]**: {}", gc_msg.speaker.name(), gc_msg.content)` — also no `is_final`, no
  `dropped_targets`).
- **Live caller search**:
  - `rg -n 'GroupChatMessage' --glob '*.rs' .` → `src/gateway/inbound_router/group_chat_handler.rs:411`
    and `src/gateway/handlers/group_chat.rs:38,73` are the **only** two production consumers of the
    struct; both drop the field.
  - `rg -n 'dropped_targets' --glob '*.rs' .` → `src/group_chat/protocol.rs:234` (definition),
    `:386` (test fixture), `src/group_chat/executor.rs:318,328,352,548,554,565` (producer). No consumer.
- **Symptom**: a `/groupchat mention` that names a persona the coordinator silently excluded produces
  **no** user-visible feedback — only a `tracing::warn!`
  (`src/group_chat/executor.rs:322-331`). The RPC client sees a normal round and cannot tell that its
  mention was dropped, despite the field existing precisely for that.
- **Hypothesis**: **CUT** — the producer exists and the serializer that should carry it omits it.
- **Confidence**: high.
- **Severity**: medium. (Overlaps SW-S5-GC-API-4 / SW-S6-GC-EXEC-7.)

---

### SW-S8-CROSS-GROUPCHAT-5 — `GroupChatSession.source_channel` has no production reader (DUP of SW-S6-GC-EXEC-8)

- **Form**: 3 (inert field) / 1 (no consumer).
- **Producer**: `src/group_chat/session.rs:46` — `pub source_channel: String`, set at
  `src/group_chat/session.rs:115`; carried in from the caller at
  `src/group_chat/orchestrator.rs:158` and persisted at `src/group_chat/orchestrator.rs:198` (dead, see
  finding 1). The RPC path lets the **client** choose it:
  `src/gateway/handlers/group_chat.rs:132-136` — `params.get("source_channel") … .unwrap_or("rpc")`.
- **Consumer**: **none found**. The channel path keys on `source_session_key`
  (`src/gateway/inbound_router/group_chat_handler.rs:436`:
  `session.source_session_key == expected_key`), never on `source_channel`.
- **Live caller search**:
  - `rg -n 'source_channel|source_session_key' src/group_chat/ src/gateway/` → reads of
    `source_channel` occur only at `src/group_chat/session.rs:322-323` and
    `src/group_chat/orchestrator.rs:418-419` — **both inside `#[cfg(test)]` modules**
    (`assert_eq!(session.source_channel, "telegram")`).
  - `rg -n 'source_channel' --glob '*.rs' .` → the remaining hits are the unrelated
    `shared/protocol/src/auth.rs` `AuthContext.source_channel`, the cron
    `source_channel_id`, and the session-store `set_session_source_channel` — different concepts.
- **Symptom**: none user-visible; a client-supplied value is accepted, stored, and never surfaced
  (not in `group_chat.list`, not in `group_chat.history`, not on the wire).
- **Hypothesis**: **CUT**.
- **Confidence**: high.
- **Severity**: low. (Overlaps SW-S6-GC-EXEC-8.)

---

### SW-S8-CROSS-GROUPCHAT-6 — `/groupchat end` (no id) is documented, rejected by the parser, and leaves the handler branch dead

- **Form**: 5 (name/path drift between doc and parser) + 1 (dead branch on the consumer side).
- **Producer**: `src/group_chat/channel.rs:20` — the parser's own doc advertises
  `/groupchat end [session_id]` (the brackets say the id is optional), and
  `src/gateway/inbound_router/group_chat_handler.rs:270` — the handler doc says
  *"Determine which session to end: explicit session_id or the active one"*. The parser,
  however, returns `None` for an empty argument list:
  `src/group_chat/channel.rs:50-58` — `"end" => { if args.is_empty() { None } else { Some(GroupChatRequest::End { session_id: args.to_string() }) } }`.
- **Consumer**: `src/gateway/inbound_router/group_chat_handler.rs:271-278` —
  `let session_id = if session_id_hint.is_empty() { …sessions.get(conversation_key)… } else { session_id_hint.to_string() };`
  — the "end the active session in this conversation" branch.
- **Live caller search**:
  - `rg -n 'GroupChatRequest' --glob '*.rs' .` → the **only** producer of
    `GroupChatRequest::End` in the whole repo is `src/group_chat/channel.rs:56`, and it is reached
    only when `args` is non-empty. (`src/group_chat/protocol.rs:439-451` only constructs
    `Start`/`Continue` inside a test.) Therefore `session_id_hint` can never be `""`.
  - `rg -n 'parse_group_chat_command' --glob '*.rs' .` → called from
    `src/gateway/inbound_router/group_chat_handler.rs:53` only (plus tests inside
    `src/group_chat/channel.rs`).
- **Symptom**: `/groupchat end` typed in a chat channel is **not** an error and **not** an end — it is
  silently swallowed (see finding 7); the only working spelling is
  `/groupchat end <session_id>`. Worse: the start reply tells the user to type exactly the rejected
  form — `src/gateway/inbound_router/group_chat_handler.rs:223-229`:
  `"🎭 Group chat started!\nParticipants: {}{}\n\nSend messages to continue, /groupchat end to finish."` —
  it prints participants and topic but **not** the session id, so the user has no id to type either.
- **Hypothesis**: **CUT** — the producer cannot produce the value the consumer's branch needs; and the
  id the branch needs is never shown to the user in the first place.
- **Confidence**: high.
- **Severity**: medium.

---

### SW-S8-CROSS-GROUPCHAT-7 — The unified `/groupchat` intercept swallows unrecognised commands

- **Form**: 2 (stub far-end: a handler that returns success without doing anything).
- **Producer**: `src/gateway/inbound_router/mod.rs:1047-1051` —
  `if parsed.command_name == "groupchat" { return self.handle_groupchat_command(&ctx.message).await; }`
  — the unified slash-command parser resolves *any* `/groupchat …` to the `groupchat` command name
  (the command is catalogued at `src/tool_metadata/registry/registration.rs:106-117`).
- **Consumer**: `src/gateway/inbound_router/group_chat_handler.rs:18-41` —
  `handle_groupchat_command` returns `Ok(())` at `:40` when `try_handle_group_chat` returns `None`
  (i.e. the group-chat parser did not recognise the text, `:24`), and also when
  `self.group_chat_orch.is_none()` (`:23`).
- **Live caller search**:
  - `rg -n 'handle_groupchat_command' --glob '*.rs' .` → two call sites:
    `src/gateway/inbound_router/mod.rs:1051` (the unified branch — **unconditional** on
    `group_chat_orch.is_some()`) and `src/gateway/inbound_router/mod.rs:1132` (the fallback branch —
    correctly guarded by `slash_text.starts_with("/groupchat") && self.group_chat_orch.is_some()`).
  - `rg -n 'try_handle_group_chat' --glob '*.rs' .` → `src/gateway/inbound_router/mod.rs:1144`
    (guarded) and `src/gateway/inbound_router/group_chat_handler.rs:24` (inside the helper).
  - `src/gateway/inbound_router/group_chat_handler.rs:61` —
    `if let Some(request) = parser.parse_group_chat_command(&msg.text)` → `None` when the text is not
    one of `start` / `continue` / `mention` / `end` with valid arguments
    (`src/group_chat/channel.rs:48-60`, `:180-182`).
- **Symptom**: a user who types `/groupchat start alice,bob Topic` (or any `/groupchat` the parser
  rejects) gets **no reply at all** — the message is consumed by the router and never reaches the
  agent. The same happens for `/groupchat` when group chat is **disabled**: the unified branch is
  not guarded by `group_chat_orch.is_some()` (unlike the fallback at `:1131`, which is), so it
  intercepts, the handler finds no orchestrator, and returns `Ok(())` at
  `src/gateway/inbound_router/group_chat_handler.rs:40` — the user's message disappears with no
  reply and no fallback to the agent.
- **Hypothesis**: **CUT** — the far end returns "handled, nothing to say".
- **Confidence**: high.
- **Severity**: medium.

---

### SW-S8-CROSS-GROUPCHAT-8 — The catalogued `builtin:groupchat` usage contradicts the parser grammar

- **Form**: 5 (name/path drift between the advertised command surface and the parser).
- **Producer**: `src/tool_metadata/registry/registration.rs:106-114` —
  `UnifiedTool::new("builtin:groupchat", "groupchat", "Start, end, or manage a multi-persona group chat", …)`
  `.with_usage("/groupchat start <personas> [topic]")` `.with_param_hint("[personas]")`.
- **Consumer**: `src/group_chat/channel.rs:17` (the real grammar) and
  `src/group_chat/channel.rs:131-189` (`parse_start_command`):
  personas must come from `--preset id1,id2` or `--role "Name: prompt"`; `--topic` must be a flag;
  **any bare positional token becomes part of the initial message**; and
  `src/group_chat/channel.rs:180-182` — `if personas.is_empty() { return None; }`.
- **Live caller search**:
  - `rg -n '"groupchat"' --glob '*.rs' .` → `src/tool_metadata/registry/registration.rs:107` (the
    tool id) / `:108` (the command name) and `src/gateway/inbound_router/mod.rs:1047` (the dispatch). The XMPP hits
    (`src/gateway/interfaces/xmpp/message_ops/ops.rs:58` `msg.msg_type == "groupchat"`) are the XMPP
    MUC message *type*, a different concept.
  - `rg -n 'parse_start_command|--preset|--role' --glob '*.rs' src/group_chat/` → the only parser;
    no other producer of `PersonaSource::Preset` from free text.
- **Symptom**: a user who follows the tool's own usage hint —
  `/groupchat start alice,bob Topic` — produces **no personas**, the parser returns `None`, and (per
  finding 7) the message is swallowed with no reply. The same input with the real grammar
  (`/groupchat start --preset alice,bob --topic "Topic"`) works. The advertised `[topic]` positional is
  silently reinterpreted as the *initial message* when personas are given by flag.
- **Hypothesis**: **CUT** — two ends of one wire disagree on the literal key/grammar.
- **Confidence**: high.
- **Severity**: medium.

---

### SW-S8-CROSS-GROUPCHAT-9 — The `SCOPED_METHODS` visibility table has no production consumer

- **Form**: 3 (inert config/table).
- **Producer**: `src/gateway/method_visibility.rs:530` — `pub const SCOPED_METHODS: &[(&str, Treatment)]`,
  with the group-chat rulings at `:650` (`("group_chat.list", Treatment::ListFiltered)`) and
  `:651-654` (`group_chat.continue` / `.mention` / `.history` / `.end` → `Treatment::KeyChecked`);
  the accessor is `src/gateway/method_visibility.rs:807` — `pub fn treatment_of(method: &str) -> Option<Treatment>`.
- **Consumer**: **none found in production**. The only reads are inside
  `src/gateway/method_visibility.rs`'s own `#[cfg(test)] mod tests` (`:852`, `:860`, `:885`, `:899`,
  `:904`, `:916`, `:928`, `:950`, `:964`, `:976`, `:983`, `:992-993`, `:1003`, `:1013-1020`,
  `:1029-1044`, `:1233-1238`, `:1256`, `:1311`, `:1334-1337`).
- **Live caller search**:
  - `rg -n 'treatment_of' --glob '*.rs' .` → every hit is inside `src/gateway/method_visibility.rs`;
    the non-test hits are the definition (`:807`) and doc mentions (`:363`, `:476`).
  - `rg -n 'SCOPED_METHODS|ORG_SHARED_REASONS|method_visibility::' --glob '*.rs' .` → the only
    consumers are `src/gateway/method_visibility.rs` itself.
  - The **actual** per-user enforcement is done ad hoc inside the handlers via
    `crate::gateway::visibility::stamped_owner_visible`:
    `src/gateway/handlers/group_chat.rs:247` (continue/mention), `:339` (end), `:382` (list),
    `:442` (history), and `src/gateway/inbound_router/group_chat_handler.rs:437` (channel path).
- **Symptom**: none user-visible today (the handlers do enforce the owner check), but the table reads
  like the authoritative policy and is not one: deleting a `group_chat.*` row from `SCOPED_METHODS`
  would only fail a test, never change runtime behaviour. The module is *partly* honest about this —
  `src/gateway/method_visibility.rs:437` calls the list "a curated pin, not a generated one" and
  `:825-827` "Curated pin, not a second source of truth" — which is why this is reported as an
  observation-grade finding rather than a break.
- **Hypothesis**: **DECIDE** — either promote the table to a real chokepoint or rename it to make the
  pin-only nature obvious at the call sites that rely on hand-written checks.
- **Confidence**: medium (the absence of a production consumer is high-confidence; whether that is a
  defect is a design judgement).
- **Severity**: medium (low if the pin role is the accepted design).

---

### SW-S8-CROSS-GROUPCHAT-10 — `group_chat.list` returns `id`, the addressed methods require `session_id`

- **Form**: 5 (name drift between the producer of the id and the consumer of it).
- **Producer**: `src/gateway/handlers/group_chat.rs:397-404` — the list response builds
  `sessions_json.push(json!({ "id": s.id, "topic": s.topic, "participants": participants, "current_round": s.current_round, "status": s.status.as_str(), "created_at": s.created_at }));`
- **Consumer**: the four addressed handlers read the id from the **`session_id`** parameter:
  `src/gateway/handlers/group_chat.rs:209` (continue/mention), `:302` (mention targets,
  `extract_str_array(&request, "targets")`), `:310` (end), `:414` (history) — all
  `extract_str(&request, "session_id")`.
  `group_chat.start` returns `"session_id"` (`src/gateway/handlers/group_chat.rs:170`, `:185`),
  `group_chat.continue`/`.mention` return it at `:280`, and `group_chat.history` at `:466`.
- **Live caller search**:
  - `rg -n '"session_id"|"id"' src/gateway/handlers/group_chat.rs` → the two spellings coexist in the
    same module; `extract_str(&request, "session_id")` is the only id lookup on the consumer side.
  - `rg -n --glob '!docs/**' --glob '!review-results/**' 'group_chat\.' .` → no in-repo client to
    disambiguate (see finding 3), so no contract file pins the spelling.
- **Symptom**: a client that does `group_chat.list` → pick a session → `group_chat.continue` must
  rename `id` → `session_id` itself; forgetting it yields `INVALID_PARAMS "Missing session_id"`.
  The channel path does not use `list` at all (it keys on its own `active_group_sessions` map), so the
  drift is invisible to the only first-party consumer.
- **Hypothesis**: **DECIDE** — pick one spelling and state it in the RPC contract.
- **Confidence**: medium.
- **Severity**: low.

---

### SW-S8-CROSS-GROUPCHAT-11 — `group_chat.start`'s member carve-out is inert (the family is not admin-gated)

- **Form**: 3 (inert config entry).
- **Producer**: `src/gateway/method_admin.rs:935` — `"group_chat.start",` inside the
  `MEMBER_CARVE_OUTS` list, which the module doc (`:25-29`) defines as "a family is admin iff it
  prefix-matches `ADMIN_PREFIXES` and is not listed in `MEMBER_CARVE_OUTS`".
- **Consumer**: `src/gateway/method_admin.rs:107-348` — `ADMIN_PREFIXES` (and `ADMIN_METHODS` at
  `:501-`) contain **no** `group_chat.` entry (the only `group_chat` string in the whole file is the
  carve-out row at `:935`), so `method_requires_admin` (`:545-553`) returns `false` for the whole
  `group_chat.*` family and the carve-out for `group_chat.start` never changes any outcome.
- **Live caller search**:
  - `rg -n 'group_chat' src/gateway/method_admin.rs` → a single hit, `:935` (the carve-out).
  - The assertion loop at `src/gateway/method_admin.rs:927-960` asserts
    `!method_requires_admin(m)` for each carve-out member — which passes trivially for a family that
    is not admin-gated in the first place, so the test cannot detect the redundancy.
- **Symptom**: none user-visible — but the audit table reads as if `group_chat.start` needed a
  deliberate member carve-out, while in fact the *whole* family (`group_chat.continue`, `.mention`,
  `.end`, `.list`, `.history`) is member-open with per-user filtering delegated to the handlers'
  hand-written `stamped_owner_visible` checks (finding 9).
- **Hypothesis**: **DECIDE** — either the carve-out is stale, or the family's admin ruling is the
  intended one and the redundant row should go.
- **Confidence**: medium (the redundancy is confirmed; whether it is intended is not stated anywhere).
- **Severity**: low.

---

## B. Clean / verified symbols (no severed wire)

- `src/gateway/handlers/mod.rs:1250-1259` — `HandlerRegistry::register` is a `HashMap::insert`, so the
  boot-time `register_group_chat_handlers` override genuinely **replaces** the placeholder for
  `group_chat.*` (verified at `src/bin/aleph-server/commands/start/builder/handlers/system.rs:129-173`
  + `src/gateway/handlers/mod.rs:887-895`). The override-wins question is answered *yes* — the problem
  is only that the override is conditional (finding 2).
- The **same** orchestrator and executor instances serve both surfaces:
  `src/bin/aleph-server/commands/start/mod.rs:3462-3501` builds `(shared_orch, gc_executor)` once and
  `src/bin/aleph-server/commands/start/mod.rs:3992-3999` passes those exact values into
  `initialize_inbound_router`, which installs them at
  `src/bin/aleph-server/commands/start/builder/subsystems.rs:828-829`
  (`inbound_router.with_group_chat(group_chat_orch, executor)`). No second instance, no placeholder.
- `GroupChatConfig::coordinator_visible` → `src/bin/aleph-server/commands/start/mod.rs:3467,3486` →
  `GroupChatExecutor::with_coordinator_visible` → read at `src/group_chat/executor.rs:336`. CONNECTED.
- `GroupChatConfig::history_window_rounds` → `src/bin/aleph-server/commands/start/mod.rs:3468,3487` →
  `with_history_window` → read at `src/group_chat/executor.rs:269`. CONNECTED.
- `GroupChatConfig::max_personas_per_session` → `src/group_chat/orchestrator.rs:98-102`. CONNECTED.
- `GroupChatConfig::max_rounds` → `src/group_chat/orchestrator.rs:306` (`max_rounds()`) → used by both
  the RPC handler (`src/gateway/handlers/group_chat.rs:236`) and the channel handler
  (`src/gateway/inbound_router/group_chat_handler.rs:352`). CONNECTED.
- `PersonaConfig` (`[[personas]]`, `src/config/structs.rs:138`) → `app_cfg.personas.clone()` at
  `src/bin/aleph-server/commands/start/mod.rs:3465` → `GroupChatOrchestrator::new(gc_config, &persona_configs)`
  → `PersonaRegistry::from_configs`. CONNECTED (preset personas resolve).
- `GroupChatConfig::validate` → `src/config/validate.rs:519-526`. CONNECTED.
- `GroupChatRequest::Start/Continue/Mention` → `src/group_chat/channel.rs:71,97,184` →
  `src/gateway/inbound_router/group_chat_handler.rs:63,87,103`. CONNECTED (except `End`'s empty-id
  case, finding 6).
- `src/gateway/inbound_router/group_chat_handler.rs:437` — `session_visible_to_conversation` reuses the
  shared predicate `crate::gateway::visibility::stamped_owner_visible` rather than re-deriving
  membership. CONNECTED (single predicate).
- `src/gateway/handlers/group_chat.rs:247,339,382,442` — the four addressed RPC methods each perform
  their own owner check and each reuse its own "not found" wording, so a foreign session is
  indistinguishable from a missing one. CONNECTED.

---

## C. New observations (not findings)

1. **The group-chat provider is env-only.** `src/bin/aleph-server/commands/start/mod.rs:3475` gates the
   executor on `can_create_provider_from_env()` (`src/gateway/provider_factory.rs:211-220`), i.e. on
   `ANTHROPIC_API_KEY` / `OPENAI_API_KEY`. The file-config provider registry is built elsewhere in the
   same boot
   (`src/bin/aleph-server/commands/start/builder/agent_init/provider_registry.rs`) but is **not** used
   for group chat, even though the code comment at `mod.rs:3479-3482` anticipates a "file-config
   webchat path". Consequence: a file-config deployment has group chat switched off, which is the
   precondition for finding 2.
2. **Two parallel session stores.** `InboundMessageRouter::active_group_sessions`
   (`src/gateway/inbound_router/mod.rs:112`) is a second map, keyed `channel:conversation → session_id`,
   that exists only for the channel path. The orchestrator's own map is the real store; the RPC path
   never touches the router's map, so an RPC-created session is invisible to the channel path and vice
   versa. The channel path self-heals stale entries (`group_chat_handler.rs:155-163`), so this is
   duplication rather than a break — but it is a second source of truth for "is this conversation in a
   group chat?".
3. **The channel never shows the session id.** The start reply
   (`src/gateway/inbound_router/group_chat_handler.rs:223-229`) prints participants and topic but not
   the `session_id`, while `/groupchat continue` and `/groupchat end <id>` both require it. Combined
   with finding 6 this makes the addressed channel commands effectively unusable.
4. **The channel path formats away the round metadata.** `send_group_chat_messages`
   (`src/gateway/inbound_router/group_chat_handler.rs:408-420`) renders only `**[speaker]**: content`,
   dropping `round`, `sequence` and `is_final`, so a channel user cannot tell where one round ends and
   the next begins. Same root cause as finding 4's serializer omission.
5. **`group_chat.*` is not in `ADMIN_PREFIXES`** (see finding 11), so all six methods are callable by a
   member principal; the per-user filtering depends entirely on the handler-local
   `stamped_owner_visible` calls. `method_census.rs` classifies the six as `Class::Open`
   (`src/gateway/method_census.rs:257-262`) — consistent — but that census is a **test-only** module
   (the whole file is `#[cfg(test)] pub(crate) mod tests { … }`, `src/gateway/method_census.rs:36-37`),
   so it is a pin, not a runtime guard.

6. **The unified `/groupchat` branch and its fallback disagree about the same guard.**
   `src/gateway/inbound_router/mod.rs:1047-1051` intercepts `parsed.command_name == "groupchat"`
   unconditionally; the fallback at `:1130-1133` is guarded by
   `slash_text.starts_with("/groupchat") && self.group_chat_orch.is_some()`. The catalog entry that
   makes `command_name == "groupchat"` resolve is installed in production
   (`src/bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs:40` calls
   `tool_catalog.register_builtin_tools().await`, and `src/tool_metadata/registry/registration.rs:106-117`
   registers `builtin:groupchat`), so the **unguarded** branch is the one that always runs and the
   guarded fallback is effectively dead for `/groupchat`. The guard the author wrote on the fallback
   was never written on the branch that actually fires.
7. **A client-supplied string is what the channel path trusts as "this session belongs to this
   conversation".** `src/gateway/handlers/group_chat.rs:132-141` lets the RPC caller choose
   `source_channel` and `source_session_key` freely (default `"rpc:direct"`), while
   `src/gateway/inbound_router/group_chat_handler.rs:430-438` uses `session.source_session_key ==
   "{channel}:{conversation}"` as the conversation-ownership proof for an addressed session id (the
   "bearer token" at `:350-354`). The independent owner gate
   (`crate::gateway::visibility::stamped_owner_visible`, `src/gateway/visibility.rs:122-127`, where
   `owner_or_legacy(None)` reads as the legacy owner id) is what actually limits the damage, and the
   session id is a UUID that must be shared — so the practical impact is low, but the *proof* the
   channel path relies on is a value another path lets the client choose. **Observation, not a
   finding**: the owner gate is a real gate and no privilege escalation was found.

---

## D. Coverage statement

**Files read in full or by targeted range**:
`src/gateway/inbound_router/group_chat_handler.rs` (all 438 lines),
`src/gateway/handlers/group_chat.rs` (all 707 lines),
`src/gateway/handlers/mod.rs` (registration + `register`/`register_with_voice_context` ranges),
`src/bin/aleph-server/commands/start/builder/handlers/system.rs` (`register_group_chat_handlers`),
`src/bin/aleph-server/commands/start/builder/handlers/mod.rs` (`register_handler!` macro),
`src/bin/aleph-server/commands/start/mod.rs` (lines 3440-3520, 3990-4010),
`src/bin/aleph-server/commands/start/builder/subsystems.rs` (lines 700-850),
`src/group_chat/{channel,orchestrator,executor,protocol,persona,session}.rs` (the parts named by the
findings), `src/gateway/{visibility,method_admin,method_visibility,method_census}.rs`,
`src/gateway/provider_factory.rs`, `src/resilience/database/group_chat.rs`,
`src/resilience/database/state_database/schema.rs`, `src/tool_metadata/registry/registration.rs`,
`src/config/types/group_chat.rs`.

**Lenses applied**: registration parity (6 `group_chat.*` names × 3 registration sites);
call-vs-handler parity (whole-repo grep for `group_chat\.`); classifier-vs-handler parity
(`ADMIN_PREFIXES` / `MEMBER_CARVE_OUTS` vs the six methods, and `SCOPED_METHODS` vs the handlers);
event emit-vs-subscribe parity (**not applicable** — group chat emits no bus events; the executor
returns messages by value, and the round result is not published to `GlobalBus`);
config-reader parity (`GroupChatConfig` × 4 fields, `PersonaConfig` × 5 fields, `[[personas]]`);
path/route parity (6 RPC names vs the CLI's `aleph gateway call` generic passthrough); stub sweep
(`RUNTIME_REQUIRED`, `Ok(())` early returns, `db: None` guards).

**Seams NOT covered and why**:
- `src/group_chat/coordinator.rs` (420 lines) — coordinator prompt building / plan parsing; it is the
  *producer* side of the round and its output is consumed inside the same module. Only its
  `need_summary` surface was cross-checked (already reported by s5/s6).
- `src/gateway/inbound_router/mod.rs` outside the group-chat branches — the general dispatch pipeline
  (pairing, policy, coalescing) was read only where it touches `/groupchat`.
- The TSX/Leptos Panel and the TUI — searched (`rg 'group_chat|groupchat' shared/ interfaces/`) and
  found to have **no** group-chat client; the Panel's "group chat" is the `teams.*` feature.
- `graphify-out/graph.json` — used for orientation only (the graph predates HEAD); every claim above
  was re-verified with `rg` against the working tree.
- Runtime behaviour (no `cargo check` / `cargo test` was run, per the read-only constraint): the
  findings are all static reachability arguments over the current source.

**Explicitly not checked**: the `state_database` schema's migration path for `group_chat_sessions.owner_user_id`
(`src/resilience/database/migration.rs:435-475`) beyond confirming the column exists; the
`GroupChatExecutor::provider_fallback_warned` dedup set; the `max_no_commit_attempts` budget.
