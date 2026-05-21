# REPL Agent Control Panel — Design Spec

**Date:** 2026-05-21
**Status:** Draft (awaiting user review)
**Scope:** Wire 6 new slash commands into Aleph TUI by connecting existing backend capabilities; minimal surgical backend additions.
**Inspiration:** hermes-agent CLI (`/usage`, `/compress`, `/stop`, `/undo`, `/retry`, tool progress modes), but with Aleph's Rust + JSON-RPC architecture.

---

## 1. Motivation

The current Aleph TUI exposes only 6 local slash commands (`/clear /verbose /help /quit /replays /replay`); everything else is forwarded verbatim to the gateway as a prompt. The agent-control surface that hermes-agent's interactive REPL provides — live token + cost display, manual compaction, run abort, undo/retry, tool progress mode — is absent **despite the backend already implementing most of these capabilities**. Users must drop out of the TUI and run CLI subcommands (`aleph session compact`, `aleph chat-control abort`, …) for operations that should be a single keystroke inside chat.

This spec closes the **wiring gap**, not the capability gap. Per CLAUDE.md R10 (thin harness, dumb loop), we add **no new harness logic and no new abstractions**; we route existing RPC endpoints to new TUI keystrokes.

---

## 2. Goals & Non-Goals

### Goals
- Add 6 new local slash commands to the TUI: `/usage`, `/compress`, `/stop`, `/undo`, `/retry`, `/tools`.
- Surface per-turn token breakdown + cumulative cost estimate in the status bar / `/usage` panel.
- Allow user to truncate the conversation to the last N turns (`/undo` = pop the most recent user+assistant pair).
- Add one new SessionStore trait method (`truncate_messages`) and one new RPC (`session.truncate`).
- Forward `ToolProgress` events from harness through the Gateway stream so the TUI can render tool execution progress at four verbosity levels.
- Clean up the now-unused `CompactionOrchestrator` (flagged for deletion in MEMORY.md history-compression spec) if confirmed dead.

### Non-Goals
- **No mid-run user message injection** (`/steer "..."`). Requires harness message queue — separate cycle, possibly never (would violate R10).
- **No inline diff rendering** for file-write tool results. Separate UX cycle.
- **No light/dark theme detection**. Separate cycle.
- **No interactive session picker** (curses-style fuzzy browser). Reuses existing `session list`.
- **No `--max-turns` / `--toolsets` / `--skills` CLI flags on `aleph ask`**. Separate cycle (different surface).

---

## 3. User-Facing Behavior

### `/usage`
```
Tokens — this turn: in=4 213, out=1 087, total=5 300
         session: in=18 902, out=4 412, total=23 314
Cost estimate (kimi-for-coding): $0.012 (in) + $0.022 (out) = $0.034
```
Cost shows `n/a` if the active provider has no pricing entry.

### `/compress`
```
Compacting session…
Before: 87 messages, ~42 100 tokens
After:  12 messages, ~6 800 tokens (saved 35 300, 83.8%)
```
Errors propagate as a system message: `Compact failed: <reason>`.

### `/stop`
```
Run aborted (run_id=…)
```
No-op (and a polite message) if no run is active.

### `/undo`
Pops the **last user+assistant turn pair**. If the last turn was a streaming run still in flight, fail with a hint to `/stop` first.
```
Reverted last turn (-2 messages, ~1 200 tokens).
```

### `/retry`
Composite: `/undo` + re-submit the popped user message verbatim.
```
Re-running last turn…
[new assistant response streams]
```

### `/tools off|new|all|verbose`
Switches the **client-side** display filter for `StreamEvent::ToolProgress`:
- `off` — never show
- `new` — show only the first event per `tool_call_id` (default, current behavior on the new event path)
- `all` — show every `Status` event, suppress `PartialOutput`
- `verbose` — show every event including streaming partial output

Echo confirmation: `Tool progress: verbose`. Mode is persisted in `AppState` only (not on disk); resets per TUI launch. Default: `new`.

The status bar gains a single character indicator: `T:n` / `T:a` / `T:v` / `T:-`.

---

## 4. Architecture

### 4.1 Layers Touched

```
┌─────────────────────────────────────────────┐
│ interfaces/tui                              │
│  - slash.rs: enum + catalog + parse_input   │
│  - mod.rs: execute_local_command            │
│  - app.rs: state for run_id / mode / cost   │
│  - widgets/status_bar.rs: tool-progress glyph│
│  - widgets/usage_panel.rs (NEW): /usage view│
└──────────────────┬──────────────────────────┘
                   │ JSON-RPC over WebSocket
                   ▼
┌─────────────────────────────────────────────┐
│ shared/client                               │
│  - AlephClient: thin wrapper, adds          │
│    session.truncate RPC method              │
└──────────────────┬──────────────────────────┘
                   │
                   ▼
┌─────────────────────────────────────────────┐
│ src/gateway                                 │
│  - handlers/session/db_handlers/modify.rs:  │
│    + handle_session_truncate                │
│  - handlers/mod.rs: register session.truncate│
│  - streaming: forward ToolProgress to clients│
└──────────────────┬──────────────────────────┘
                   │
                   ▼
┌─────────────────────────────────────────────┐
│ src/gateway/session_store                   │
│  - mod.rs (trait): + truncate_messages      │
│  - file_backend/mod.rs (impl)               │
│  - sqlite_backend/mod.rs (impl)             │
│ src/teams/sessions/store.rs (impl)          │
│ test mocks: InMemorySessionStore (impl stub)│
└─────────────────────────────────────────────┘
```

### 4.2 Backend Additions (Surgical)

**Addition 1 — `SessionStore::truncate_messages`**
```rust
// src/gateway/session_store/mod.rs
async fn truncate_messages(
    &self,
    key: &SessionKey,
    keep_count: usize,  // keep the FIRST n messages; drop the rest
) -> Result<TruncateStats, SessionStoreError>;

pub struct TruncateStats {
    pub messages_removed: usize,
    pub tokens_removed_estimate: u64,
}
```
Semantics: deletes the **last** `(total_count - keep_count)` messages by ascending message id. Returns `(0, 0)` if `keep_count >= total_count`. Errors if session does not exist.

**Addition 2 — RPC `session.truncate`**

`src/gateway/handlers/session/db_handlers/modify.rs`:
```rust
#[derive(Deserialize)]
struct TruncateParams { session_id: SessionId, keep_count: usize }
#[derive(Serialize)]
struct TruncateResult { messages_removed: usize, tokens_removed_estimate: u64 }

pub async fn handle_session_truncate(req: …) -> Result<TruncateResult, …>
```
Registered in `handlers/mod.rs` next to `session.compact`.

**Addition 3 — Forward `ToolProgress` to client stream**

Two-step:
1. `src/thinker/streaming/events.rs`: add `StreamEvent::ToolProgress { tool_call_id: String, phase: ToolProgressPhase, data: serde_json::Value }` where `ToolProgressPhase ∈ { Started, PartialOutput, Completed, Failed }`.
2. Wire the existing harness `ToolProgressCallback` (currently mpsc-only, internal) to also push into the active `StreamingSession`'s event channel. Look for the producer in `src/builtin_tools/mod.rs` / `src/tools/execution_context.rs` and add a second sink.

No new abstraction — just a fan-out at the existing emission point.

### 4.3 TUI Additions

**`interfaces/tui/src/tui/slash.rs`** — extend `LocalCommand`:
```rust
pub enum LocalCommand {
    // existing …
    Usage,
    Compress,
    Stop,
    Undo,
    Retry,
    ToolsMode(ToolProgressMode),
}

pub enum ToolProgressMode { Off, New, All, Verbose }
```

Extend `LOCAL_COMMAND_CATALOG` and `parse_input` accordingly. `/tools` with no arg prints current mode + valid values.

**`interfaces/tui/src/tui/mod.rs`** — extend `execute_local_command`:

```rust
LocalCommand::Usage => {
    let usage = client.call::<_, SessionUsage>("session.usage", Some(…)).await?;
    let cost = estimate_cost(&usage, &state.active_provider());  // pure fn
    state.show_usage_panel(usage, cost);
}
LocalCommand::Compress => {
    let r = client.call::<_, CompactResult>("session.compact", Some(…)).await?;
    state.add_system_message(format_compact(r));
}
LocalCommand::Stop => {
    if let Some(run_id) = state.active_run_id() {
        client.call::<_, ()>("chat.abort", Some(json!({"run_id": run_id}))).await?;
    } else {
        state.add_system_message("No active run.");
    }
}
LocalCommand::Undo => {
    if state.active_run_id().is_some() {
        state.add_system_message("Stop the active run first (/stop)."); return;
    }
    let history_len = state.message_count();
    if history_len < 2 { state.add_system_message("Nothing to undo."); return; }
    let keep = history_len.saturating_sub(2);
    let r = client.call::<_, TruncateResult>("session.truncate",
        Some(json!({"session_id": sid, "keep_count": keep}))).await?;
    state.reload_history().await?;
    state.add_system_message(format_undo(r));
}
LocalCommand::Retry => {
    // pop last user msg before truncating
    let last_user = state.last_user_message();
    if last_user.is_none() { state.add_system_message("Nothing to retry."); return; }
    execute_local_command(state, LocalCommand::Undo).await?;
    // resend
    state.submit_prompt(last_user.unwrap()).await?;
}
LocalCommand::ToolsMode(m) => {
    state.tool_progress_mode = m;
    state.add_system_message(format!("Tool progress: {:?}", m));
}
```

**`interfaces/tui/src/tui/app.rs`** — add to `AppState`:
- `active_run_id: Option<String>` — set on `Action::GatewayCommand` send, cleared on `RunComplete`
- `tool_progress_mode: ToolProgressMode` — default `New`
- `last_turn_usage: Option<TokenUsage>` — updated on `AssistantComplete`
- `session_usage_cache: SessionUsage` — refreshed by `/usage`

**Cost estimation helper** — `interfaces/tui/src/tui/cost.rs` (NEW, ~50 LOC):
```rust
pub fn estimate_cost(usage: &SessionUsage, provider: &str) -> Option<f64>;
```
Reads pricing table from a small static map (initially hardcoded for the 4 providers Aleph supports today: Anthropic, OpenAI, kimi-for-coding, T8Star). Adding a pricing table file is **out of scope** (would need a separate spec); hardcoded is honest and easy to grep.

### 4.4 Dead Code Cleanup

Per MEMORY.md "History Compression Wiring" entry, `CompactionOrchestrator` was "flagged for next-cycle deletion." Verify during implementation:
1. `rg "CompactionOrchestrator"` — confirm zero non-test references.
2. If zero, delete the type + its module file + any factory wiring.
3. If still referenced (e.g. by an internal test), defer to a follow-up note.

If `tool_progress_callback.rs` (or similar) has stale unwired code, delete the dead branches.

---

## 5. Data Flow

### 5.1 `/usage` Round Trip
```
User types /usage
  → slash.rs parses to LocalCommand::Usage
  → execute_local_command calls client.session_usage(session_id)
  → Gateway handler reads SessionStore.get_metadata + sums message usage
  → returns SessionUsage{ input, output, total, message_count, by_provider }
  → TUI computes cost via lookup_pricing(provider)
  → renders to dedicated UsagePanel widget overlay (dismiss on any key)
```

### 5.2 `/compress`
```
User types /compress
  → client.session_compact(session_id, strategy=KeepLastN{n:20})
  → Gateway invokes ContextCompactor (existing)
  → returns CompactResult { before_count, after_count, tokens_saved }
  → TUI renders one-line summary as a system message
  → TUI triggers full history reload (existing client method)
```

### 5.3 `/undo` + `/retry`
```
/undo:
  Guard: active_run_id is None
  Compute: keep_count = history_len - 2
  RPC session.truncate
  Reload history
  Show "Reverted last turn"

/retry:
  Capture last_user_message
  Call /undo recursively
  Resend captured prompt via existing submit path
```

### 5.4 Tool Progress Display
```
Harness emits ToolProgress (existing internal channel)
  → New fan-out: also push to StreamingSession.events
  → WebSocket frame to client
  → TUI app.rs handle_gateway_event sees StreamEvent::ToolProgress
  → Filter by tool_progress_mode (off/new/all/verbose)
  → Append to chat as a styled "tool" line (existing system-message style)
```

---

## 6. Errors & Edge Cases

| Case | Behavior |
|---|---|
| `/usage` while session has 0 messages | Show "0 tokens recorded yet." |
| Unknown provider for cost | Show `cost: n/a (no pricing for <name>)` |
| `/compress` mid-stream | Reject with "Wait for current turn to finish (/stop first)." |
| `/stop` when no run | "No active run." (not an error) |
| `/undo` with 0 or 1 message | "Nothing to undo." |
| `/undo` while streaming | "Stop the active run first (/stop)." |
| `/retry` with no prior user message | "Nothing to retry." |
| `/tools` invalid arg | Print usage hint: `/tools off|new|all|verbose` |
| `session.truncate` keep_count > total | Return `(0, 0)` not error |
| RPC failure on any command | System message with error; TUI does not crash |

---

## 7. Testing

### Unit
- `interfaces/tui/src/tui/slash.rs`: parse table-driven test for each new command + invalid args
- `interfaces/tui/src/tui/cost.rs`: known input/output pairs for each supported provider
- `src/gateway/session_store/{file_backend,sqlite_backend}/mod.rs`: `truncate_messages` correctness — keep_count={0, mid, exact, over}
- `src/gateway/handlers/session/db_handlers/modify.rs`: `handle_session_truncate` happy path + missing session error

### Integration
- `tests/session_truncate_e2e.rs` (NEW): start in-memory gateway, append 6 messages, truncate to 4, assert count + ids
- `tests/tui_slash_integration.rs` (NEW, harness mock): simulate user typing `/undo` → assert SessionStore state transitions

### Manual
- `just dev` → open TUI → run a real conversation → exercise each slash command → confirm UI feedback

### Regression
- `cargo test -p alephcore --lib` must remain green
- `just clippy` must not regress (baseline is dirty per MEMORY.md fmt+clippy entry; track only new warnings)

---

## 8. Rollout

### Phases
1. **Spec approval** (this doc → user review)
2. **EnterWorktree** named `repl-control-panel`
3. **Phase 1: Pure-wire commands** — `/usage /compress /stop /tools` + cost helper. Commit + test.
4. **Phase 2: SessionStore truncate + /undo + /retry** — new trait method, 4 impls, RPC, TUI dispatch. Commit + test.
5. **Phase 3: Tool progress fan-out** — emit `StreamEvent::ToolProgress`, TUI subscriber, status bar glyph. Commit + test.
6. **Phase 4: Dead code purge** — drop `CompactionOrchestrator` if confirmed orphaned; drop any other dead tool-progress code paths.
7. **Phase 5: Merge to main** — squash if cleanly separable, otherwise keep 5 commits.

### Compatibility
- All additions are additive; no existing API breaks.
- New RPC `session.truncate` is independent of existing RPCs; older clients ignore it.
- New `StreamEvent::ToolProgress` is a new enum variant; older clients deserialize via `#[serde(other)]` default (verify this is the existing pattern in `streaming/events.rs`).

### Risks
- **Pricing drift**: hardcoded pricing will go stale. Mitigation: comment with last-known-good date + a follow-up issue to add a YAML config.
- **truncate_messages ordering bug**: deleting by id might leave gaps in `ROWID` — verify behavior against `session_messages.message_id` column ordering, not insertion order.
- **Tool progress flood**: `verbose` mode could spam the chat for long-running tools. Mitigation: rate-limit at 1 event per 250ms per `tool_call_id` on the client side.

---

## 9. Out of Scope (Explicit)

- Mid-run user-message injection (`/steer`)
- Inline diff renderer for file write tools
- OSC 11 terminal-theme detection
- Curses-style fuzzy session picker
- `--max-turns / --toolsets / --skills` CLI flags
- A pricing config file (hardcoded for now)
- Cost projection / budget alerts (future)

---

## 10. Acceptance Criteria

A reviewer can merge this when:
- [ ] All 6 new slash commands work end-to-end against a real `aleph-server`
- [ ] `cargo test -p alephcore --lib` green
- [ ] `cargo test -p aleph-tui` green
- [ ] New unit tests cover `truncate_messages` boundary cases (0, mid, exact, over)
- [ ] `CompactionOrchestrator` either deleted (with confirmation) or its remaining users documented
- [ ] No new `clippy` warnings beyond baseline
- [ ] MEMORY.md updated with a `project_repl_control_panel.md` entry on merge
