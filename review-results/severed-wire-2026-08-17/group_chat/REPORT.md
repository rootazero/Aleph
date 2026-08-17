# Severed-Wire Audit — src/group_chat

- Audit: severed-wire-2026-08-17 (PRODUCED–CONSUMED symbol parity via `rg`)
- Module: `src/group_chat` (8 files, 3,286 LOC)
- Working tree: `/home/zou/data/workspace/Aleph/.worktrees/review-fix-2026-08-17`
- Prior review (re-verified against current code): `review-results/group_chat.md` (header date 2026-08-12; cited as `existing_review_ref`)
- READ-ONLY: no source files modified; no cargo run.

## Method

1. Read all 8 module files fully.
2. Enumerated the public surface: `rg -n "^\s*pub" src/group_chat/` (61 pub items).
3. For each candidate symbol, ran `rg -n "<symbol>" src/ interfaces/ shared/` (this repo's `bin/` is `src/bin/`, covered by the `src/` sweep) and classified every hit as production / `#[cfg(test)]` / definition.
4. Traced the wiring path: `src/bin/aleph-server/commands/start/mod.rs` (orchestrator/executor construction) → `builder/handlers/system.rs` (RPC registration) → `src/gateway/handlers/group_chat.rs` + `src/gateway/inbound_router/group_chat_handler.rs` (consumers).
5. Checked the DB seam: `src/resilience/database/group_chat.rs` + `state_database/schema.rs` + `migration.rs`.

A symbol is "live" only if a production code path reaches it. Test-only consumers do not count.

## Findings

| ID | Severity | Form | Produced | Decision |
|----|----------|------|----------|----------|
| sw-gc-1 | high | 2/1 | `with_database` (orchestrator + executor) | CONNECT |
| sw-gc-2 | medium | 1 | DB read APIs `get_group_chat_turns` / `list_active_group_chats` / `get_group_chat_session_owner` | DECIDE |
| sw-gc-3 | medium | 4 | `GroupChatExecutor::with_provider_registry` | CONNECT |
| sw-gc-4 | low | 4/1 | `GroupChatRequest::Continue` / `Mention` variants | DECIDE |
| sw-gc-5 | low | 1/6 | `RenderedContent` / `ContentFormat` (+ constructors) | CUT |
| sw-gc-6 | low | 1 | `GroupChatError::SessionNotFound` | CUT |
| sw-gc-7 | low | 4 | `PersonaRegistry::len` / `is_empty` | CUT |
| sw-gc-8 | low | 4 | `impl FromStr for GroupChatStatus` | CUT |

Totals: **8 findings** — 1 high / 2 medium / 5 low; 4 CUT / 2 CONNECT / 2 DECIDE.

---

## sw-gc-1 — `with_database` never wired: entire group_chat DB persistence layer severed (high, CONNECT)

**Produced:**
- `GroupChatOrchestrator::with_database` — `src/group_chat/orchestrator.rs:49`
- `GroupChatExecutor::with_database` — `src/group_chat/executor.rs:80`

**Evidence — zero callers repo-wide:**

```
$ rg -n "with_database" src/ interfaces/ shared/
src/group_chat/executor.rs:80:    pub fn with_database(mut self, db: Arc<StateDatabase>) -> Self {
src/group_chat/orchestrator.rs:49:    pub fn with_database(mut self, db: Arc<StateDatabase>) -> Self {
```

Not even test code calls it. The only production construction sites:

```
$ rg -n "GroupChatOrchestrator::new" src/ | rg -v "src/group_chat/|handlers/group_chat.rs"
src/bin/aleph-server/commands/start/mod.rs:2713:        let orchestrator = GroupChatOrchestrator::new(gc_config, &persona_configs);
$ rg -n "GroupChatExecutor::new" src/ | rg -v "src/group_chat/executor.rs"
src/bin/aleph-server/commands/start/mod.rs:2728:                    GroupChatExecutor::new(handle).with_coordinator_visible(coordinator_visible),
```

Both constructors leave `db: None` (`orchestrator.rs:44`, `executor.rs:54`), and the builder never calls `with_database`.

**Consequence — every persistence path is unreachable at runtime:**
- `orchestrator.rs:140-152` — `create_session`'s `insert_group_chat_session` branch: guarded by `if let Some(db) = &self.db` (line 140).
- `orchestrator.rs:204-213` — `end_session`'s `update_group_chat_session_status` branch: guarded at line 204.
- `executor.rs:99-138` — `persist_turn` (definition at line 117) early-returns at line 125: `let Some(db) = self.db.clone() else { return };`. `insert_group_chat_turn` (executor.rs:136) never runs.

The DB-side functions are reachable only through those unwired branches:
- `insert_group_chat_session` — `src/resilience/database/group_chat.rs:44` (sole caller: orchestrator.rs:141)
- `insert_group_chat_turn` — `group_chat.rs:124` (sole caller: executor.rs:136)
- `update_group_chat_session_status` — `group_chat.rs:101` (sole caller: orchestrator.rs:206)

The tables and migration exist (`state_database/schema.rs:242` `group_chat_sessions`, `:256` `group_chat_turns`; `migration.rs:420` `migrate_add_group_chat_owner`) — the schema is created at boot, then never written. `GroupChatSession::source_channel` / `source_session_key` (session.rs:44,46) are likewise write-only in production: their only readers are the inert DB insert and tests.

**Prior-review cross-check:** `review-results/group_chat.md` flagged (high) that `insert_group_chat_session` never persisted `owner_user_id` and the schema had no column. Current code fixed that at the SQL layer — the column + migration exist and the insert passes `?7 = owner_user_id` (group_chat.rs:44-73, migration.rs:420-459). But because `with_database` is unwired, that fix is moot: **nothing** is ever written to either table, so sessions and turns survive only in memory and vanish on restart. `group_chat.history` (handlers/group_chat.rs:410-460) serves only the live in-memory session.

**Decision: CONNECT.** In `src/bin/aleph-server/commands/start/mod.rs` (~2713), where the `StateDatabase` is available, pass it through:

```rust
let orchestrator = GroupChatOrchestrator::new(gc_config, &persona_configs)
    .with_database(state_db_arc.clone());
// and in the executor branch (~2728):
GroupChatExecutor::new(handle)
    .with_coordinator_visible(coordinator_visible)
    .with_database(state_db_arc.clone())
```

Verify the `state_db` variable name at that construction site first (the builder initializes the DB earlier in the file). Risk: DB writes become active — check the `spawn_blocking` persist path (executor.rs:117-138) under load; it is currently dead code and never exercised by tests (no test calls `with_database` either), so wiring it exposes it to real SQLite contention for the first time.

---

## sw-gc-2 — DB read APIs with zero consumers (medium, DECIDE)

**Produced (all `pub fn` on `StateDatabase` in `src/resilience/database/group_chat.rs`):**
- `get_group_chat_turns` — `group_chat.rs:159` (row struct `GroupChatTurn` — `group_chat.rs:12`)
- `list_active_group_chats` — `group_chat.rs:195` (row struct `GroupChatSessionSummary` — `group_chat.rs:24`)
- `get_group_chat_session_owner` — `group_chat.rs:81`

**Evidence:**

```
$ rg -n "get_group_chat_turns" src/ interfaces/ shared/
src/resilience/database/group_chat.rs:159:    pub fn get_group_chat_turns(&self, session_id: &str) -> Result<Vec<GroupChatTurn>, AlephError> {
src/group_chat/session.rs:99:    /// out-of-order row would float to the top of `get_group_chat_turns`.
$ rg -n "list_active_group_chats" src/ interfaces/ shared/
src/resilience/database/group_chat.rs:195:    pub fn list_active_group_chats(&self) -> Result<Vec<GroupChatSessionSummary>, AlephError> {
$ rg -n "get_group_chat_session_owner" src/ interfaces/ shared/
src/resilience/database/group_chat.rs:81:    pub fn get_group_chat_session_owner(
```

The `session.rs:99` hit is a doc-comment mention, not a call. No consumer exists anywhere — not even in tests (the DB module has no test module).

**Rationale:** these are the read half of a persistence contract whose write half is also currently inert (sw-gc-1). If sw-gc-1 is connected, these become the replay/list surface — but nothing would still call them (no resume/replay path for group chats exists; `get_group_chat_turns`' ORDER BY round,sequence replay was referenced in session.rs's `add_turn` regression test rationale but no production reader was built). Options: (a) connect them into a future resume/replay feature, or (b) CUT the three read APIs + the two row structs. Keep `insert_group_chat_session` / `insert_group_chat_turn` / `update_group_chat_session_status` regardless — sw-gc-1 makes them live.

**Decision: DECIDE** — the intent (replay/visibility read-back) is plausible but unimplemented; do not delete until sw-gc-1 is resolved.

---

## sw-gc-3 — `with_provider_registry` test-only: per-persona `provider` override is an inert config knob (medium, CONNECT)

**Produced:** `GroupChatExecutor::with_provider_registry` — `src/group_chat/executor.rs:66`.

**Evidence — production never calls it:**

```
$ rg -n "with_provider_registry" src/ interfaces/ shared/
src/group_chat/executor.rs:66:    pub fn with_provider_registry(mut self, registry: Arc<ProviderRegistry>) -> Self {
src/group_chat/executor.rs:897:        .with_provider_registry(Arc::new(registry));
src/group_chat/executor.rs:1069:                .with_provider_registry(Arc::new(registry));
```

Lines 897 and 1069 are both inside `#[cfg(test)] mod tests` (tests `test_per_persona_provider_resolution`, `test_resolve_provider_warn_is_deduped`). The production builder (`start/mod.rs:2718-2729`) constructs the executor with only `.with_coordinator_visible(...)`.

**Consequence:** `resolve_provider` (executor.rs:89-114) always takes the `else` branch → `self.default_provider.current()`. The `Persona.provider` / `PersonaConfig.provider` fields (protocol.rs:67, config/types/group_chat.rs:105-110) parse and propagate (`persona_from_config`, persona.rs:78-88) but are silently ignored at runtime — an inert config knob the operator can set with zero effect. The `provider_fallback_warned` dedupe set (executor.rs:44-46, the subject of a fixed prior-review medium finding) can never fill.

**Wiring note (type seam):** `with_provider_registry` takes `Arc<crate::providers::ProviderRegistry>` (concrete struct, `src/providers/registry.rs:35`), while the production default-provider path holds `MultiProviderRegistry` (`src/thinker/mod.rs:202`, used at `start/mod.rs:1511`/`1755`) which implements the **trait** `crate::thinker::ProviderRegistry` (`thinker/mod.rs:55`) — a different, same-named type. The builder's group_chat section (start/mod.rs:2718-2729) only snapshots `reg.default_provider()` into a `StaticDefault` and discards the registry. CONNECT requires either adapting `MultiProviderRegistry` (or its `provider_names()`) into the `providers::ProviderRegistry` shape, or widening the executor's registry parameter to the trait.

**Decision: CONNECT** — hand the executor a real registry (and ideally the live `MultiProviderRegistry` as the default handle) so `Persona.provider` overrides take effect. Until then, the field is dead config surface.

---

## sw-gc-4 — `GroupChatRequest::Continue` / `Mention`: variants produced only in tests; router match arms unreachable (low, DECIDE)

**Produced:** `GroupChatRequest::Continue` — `src/group_chat/protocol.rs:136`; `GroupChatRequest::Mention` — `protocol.rs:143`.

**Evidence:**

```
$ rg -n "GroupChatRequest::" src/ interfaces/ shared/
src/group_chat/channel.rs:41:            Some(GroupChatRequest::End { session_id })
src/group_chat/channel.rs:105:    Some(GroupChatRequest::Start {
src/group_chat/protocol.rs:470:        let start = GroupChatRequest::Start {        # #[cfg(test)]
src/group_chat/protocol.rs:477:        let cont = GroupChatRequest::Continue {      # #[cfg(test)]
src/gateway/inbound_router/group_chat_handler.rs:62:                GroupChatRequest::Start {
src/gateway/inbound_router/group_chat_handler.rs:80:                GroupChatRequest::End { session_id } => {
src/gateway/inbound_router/group_chat_handler.rs:86:                GroupChatRequest::Continue {
src/gateway/inbound_router/group_chat_handler.rs:102:                GroupChatRequest::Mention {
```

`parse_group_chat_command` (channel.rs:28-52, doc at 14-20) emits **only** `Start` (line 105) and `End` (line 41). The router's `Continue` (group_chat_handler.rs:86-97) and `Mention` (102-112) match arms are therefore unreachable — no production code can produce those variants. The RPC layer (`handlers/group_chat.rs`) never touches `GroupChatRequest` at all: `handle_continue`/`handle_mention` (lines 191, 294) parse params directly and call `handle_continue_with_targets`.

**Options:** (a) CUT the two variants + the two router arms — RPC continue/mention already cover the functionality and the channel auto-routes plain messages to the active session (group_chat_handler.rs:113-150); (b) CONNECT by extending the parser with `/groupchat continue <id> …` / `mention` syntax — redundant with the auto-routing, and the parser lacks the conversation→session map needed to resolve bare mentions.

**Decision: DECIDE** — the variants carry `Serialize`/`Deserialize`/`JsonSchema` derives (protocol.rs:125-165), so removal changes a serializable contract surface; the router arms themselves are provably dead and safe to CUT independently.

---

## sw-gc-5 — `RenderedContent` / `ContentFormat`: dead code + orphaned pub API (low, CUT)

**Produced:** `ContentFormat` — `src/group_chat/protocol.rs:232`; `RenderedContent` — `protocol.rs:245`; constructors `markdown`/`plain`/`html` — `protocol.rs:256/265/274`. Re-exported wholesale via `pub use protocol::*` (`mod.rs:17`).

**Evidence:**

```
$ rg -n "RenderedContent" src/ interfaces/ shared/
src/group_chat/protocol.rs:226:// ContentFormat / RenderedContent
src/group_chat/protocol.rs:245:pub struct RenderedContent {
src/group_chat/protocol.rs:254:impl RenderedContent {
src/group_chat/protocol.rs:488:        let md = RenderedContent::markdown("# Hello");   # #[cfg(test)]
src/group_chat/protocol.rs:493:        let plain = RenderedContent::plain("Hello world"); # #[cfg(test)]
src/group_chat/protocol.rs:498:        let html = RenderedContent::html("<h1>Hello</h1>"); # #[cfg(test)]
$ rg -n "ContentFormat" src/ | rg "group_chat"
src/group_chat/protocol.rs:232:pub enum ContentFormat {
src/group_chat/protocol.rs:249:    pub format: ContentFormat,
src/group_chat/protocol.rs:259:            format: ContentFormat::Markdown,
src/group_chat/protocol.rs:268:            format: ContentFormat::Plain,
src/group_chat/protocol.rs:277:            format: ContentFormat::Html,
src/group_chat/protocol.rs:490/495/500:  # #[cfg(test)] asserts
```

(The `builtin_tools/pdf_generate` `ContentFormat` hits are a different, unrelated type.) No channel adapter uses these types — `DefaultGroupChatCommandParser` is the only channel-module symbol with a production consumer, and it returns `GroupChatRequest`, not rendered content. The module doc (`channel.rs:3-6`: "allow different communication channels … to render group chat messages in their native format") describes a rendering path that does not exist — form 5 residue folded in here.

**Removal:** delete `protocol.rs:226-277` (section header, `ContentFormat`, `RenderedContent`, impl) and the test `test_rendered_content_creation` (protocol.rs:487-502). Risk: none — no production or test code outside the module references them (verified above).

**Decision: CUT.**

---

## sw-gc-6 — `GroupChatError::SessionNotFound`: never constructed, never matched (low, CUT)

**Produced:** `src/group_chat/protocol.rs:345`.

**Evidence:**

```
$ rg -n "GroupChatError::SessionNotFound|SessionNotFound\(" src/ | rg -v wizard
src/group_chat/protocol.rs:345:    SessionNotFound(String),
$ rg -n "GroupChatError::" src/ bin/ interfaces/ shared/ | rg -v "src/group_chat/"
(no matches — no consumer outside the module)
```

Within the module, every other variant is constructed somewhere (PersonaNotFound: persona.rs:65, executor.rs; TooManyPersonas/InvalidPersona: orchestrator.rs; CoordinatorPlanParseError: coordinator.rs; PersonaInvocationFailed/ProviderUnavailable/SessionInactive: executor.rs) — `SessionNotFound` is constructed nowhere and matched nowhere. The RPC/channel layers use their own literal "Session not found" strings (handlers/group_chat.rs:229 etc.), not this variant.

**Removal:** delete `protocol.rs:345-347`. Risk: none.

**Decision: CUT.**

---

## sw-gc-7 — `PersonaRegistry::len` / `is_empty`: test-only (low, CUT)

**Produced:** `len` — `src/group_chat/persona.rs:46`; `is_empty` — `persona.rs:52`. (`get` at persona.rs:40 is live — called internally by `resolve` at persona.rs:64, and `from_configs`/`resolve` are consumed by the orchestrator.)

**Evidence:**

```
$ rg -n "registry\.len\(\)|registry\.is_empty\(\)|PersonaRegistry" src/ | rg "persona.rs"
src/group_chat/persona.rs:46:    pub fn len(&self) -> usize {
src/group_chat/persona.rs:52:    pub fn is_empty(&self) -> bool {
src/group_chat/persona.rs:100:        assert_eq!(registry.len(), 2);     # #[cfg(test)]
src/group_chat/persona.rs:101:        assert!(!registry.is_empty());     # #[cfg(test)]
```

The orchestrator holds `persona_registry: PersonaRegistry` privately (orchestrator.rs:31) and never calls `len`/`is_empty` (only `resolve`, via `create_session`). No other production code reaches the registry.

**Removal:** delete `persona.rs:46-54` (len + is_empty) and their test assertions at persona.rs:100-101. Risk: none.

**Decision: CUT.**

---

## sw-gc-8 — `impl FromStr for GroupChatStatus`: test-only (low, CUT)

**Produced:** `src/group_chat/protocol.rs:213-221`.

**Evidence:**

```
$ rg -n "parse::<GroupChatStatus>|GroupChatStatus::from_str" src/ interfaces/ shared/
src/group_chat/protocol.rs:422:            "active".parse::<GroupChatStatus>().unwrap(),   # #[cfg(test)]
src/group_chat/protocol.rs:426:            "ended".parse::<GroupChatStatus>().unwrap(),    # #[cfg(test)]
src/group_chat/protocol.rs:431:        assert!("unknown".parse::<GroupChatStatus>().is_err()); # #[cfg(test)]
src/group_chat/protocol.rs:432:        assert!("".parse::<GroupChatStatus>().is_err());      # #[cfg(test)]
```

The live half of the status contract is `as_str` (protocol.rs:199-205; consumers: handlers/group_chat.rs:399, orchestrator.rs:206) and `Display` (protocol.rs:207-211). The DB layer writes `"active"`/`"ended"` strings via `as_str`; nothing ever parses a status string back (no DB status read exists — see sw-gc-2). The `FromStr` impl has zero production consumers and no doc/contract promise.

**Removal:** delete `protocol.rs:213-223` (FromStr impl; the Display impl above it is live — keep) plus the four from_str test lines (422, 426, 431, 432). Risk: none.

**Decision: CUT.**

---

## Checked and found LIVE (no finding)

| Symbol | Consumer (production) |
|--------|-----------------------|
| `DefaultGroupChatCommandParser::parse_group_chat_command` | inbound_router/group_chat_handler.rs:57-60 |
| `GroupChatExecutor::new` / `execute_round` / `with_coordinator_visible` | start/mod.rs:2728; router + RPC handlers (execute_round: executor.rs:186) |
| `GroupChatOrchestrator::new` / `create_session` / `get_session` / `end_session` / `max_rounds` / `all_sessions` | start/mod.rs:2713; router + RPC handlers (handlers/group_chat.rs:92-402) |
| `coordinator::{build_coordinator_prompt, parse_coordinator_plan, build_fallback_plan, build_persona_prompt}` | executor.rs:26-29, called in `execute_round` |
| `Persona::validate` | orchestrator.rs:create_session inline-validation |
| `PersonaRegistry::from_configs` / `get` / `resolve` | orchestrator.rs:40, 66+ (get via resolve) |
| `Speaker::name` | router send_group_chat_messages, handlers message_to_json, session.build_history_text |
| `GroupChatMessage` / `Persona` / `PersonaSource` / `GroupChatStatus::as_str` | RPC handlers + router |
| `GroupChatSession` fields (`owner_user_id`, `created_at`, `topic`, `participants`, `history`, `current_round`, `status`) | visibility checks (handlers/group_chat.rs:236, 321, 378), handle_list/handle_history |
| `GroupChatTurn` (session.rs) + `timestamp` | session history + handle_history (handlers/group_chat.rs:438-441) |
| `SharedSession` type alias | orchestrator API signatures, consumed via get_session/end_session returns |
| `GroupChatConfig` knobs (`max_personas_per_session`, `max_rounds`, `coordinator_visible`) | orchestrator.rs:93, 229; start/mod.rs:2726 |
| `insert_group_chat_session` / `insert_group_chat_turn` / `update_group_chat_session_status` | call sites exist (orchestrator.rs:141, executor.rs:136, orchestrator.rs:206) — but unreachable at runtime until sw-gc-1 is connected |

## Deliberately skipped / noted

- **No `cargo` runs** (protocol constraint). A symbol that would fail compilation under `cargo check` (form 3, stale references) cannot be positively identified statically; I found no textual evidence of renamed-symbol references, and none of the ~40 cross-module consumers reference a symbol I could not locate.
- **`coordinator_visible` raw-JSON UX** and **uuid dependency (R3)**: flagged in the prior review, deliberately unfixed there; not severed-wire issues — out of scope here.
- **`GroupChatSession::source_channel` / `source_session_key` write-only in production**: folded into sw-gc-1's rationale (they are persistence inputs, not dead API).
- **Two same-named `ProviderRegistry` types** (struct in `providers/registry.rs:35` vs trait in `thinker/mod.rs:55`): noted as the type seam blocking sw-gc-3's CONNECT; a form-5-adjacent naming smell, tracked with that finding rather than standalone.
- **Prior-review fix verification:** the executor mid-round rollback (critical) and warn-dedupe (medium) fixes are present in current code (executor.rs:186+ staging, 44-46 dedupe set, regression tests at executor.rs:962 and 1021). The `owner_user_id` SQL fix is present but moot until sw-gc-1 — see finding.
