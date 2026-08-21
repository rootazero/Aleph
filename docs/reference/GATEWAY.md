# Gateway System

> WebSocket control plane, JSON-RPC protocol, and multi-channel messaging

---

## Overview

The Gateway is Aleph's control plane, providing:
- WebSocket server for real-time communication
- JSON-RPC 2.0 protocol for structured requests
- Multi-interface message routing (Telegram, Discord, iMessage, CLI)
- Event distribution and streaming
- Session management and persistence

**Location**: `src/gateway/`

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        Gateway Server                            │
│                  ws://127.0.0.1:18790/ws                        │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────────┐     ┌──────────────┐     ┌──────────────┐    │
│  │   Inbound    │     │   Handler    │     │   Outbound   │    │
│  │   Router     │ ──▶ │   Registry   │ ──▶ │   Emitter    │    │
│  │              │     │              │     │              │    │
│  │ • Parse req  │     │ • Route      │     │ • Stream     │    │
│  │ • Validate   │     │ • Execute    │     │ • Events     │    │
│  └──────────────┘     └──────────────┘     └──────────────┘    │
│                                                                  │
│  ┌──────────────┐     ┌──────────────┐     ┌──────────────┐    │
│  │   Session    │     │    Event     │     │  Interface   │    │
│  │   Manager    │     │     Bus      │     │   Registry   │    │
│  │              │     │              │     │              │    │
│  │ • SQLite     │     │ • Pub/Sub    │     │ • Telegram   │    │
│  │ • Compaction │     │ • Topics     │     │ • Discord    │    │
│  │ • History    │     │ • Subscribe  │     │ • iMessage   │    │
│  └──────────────┘     └──────────────┘     └──────────────┘    │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## JSON-RPC Protocol

### Message Format

**Request (Client → Gateway)**:
```json
{
  "jsonrpc": "2.0",
  "id": "uuid-xxx",
  "method": "agent.run",
  "params": {
    "message": "Hello",
    "session_key": "agent:main:main"
  }
}
```

**Response (Gateway → Client)**:
```json
{
  "jsonrpc": "2.0",
  "id": "uuid-xxx",
  "result": {
    "run_id": "run-123",
    "status": "running"
  }
}
```

**Event (Gateway → Client)**:
```json
{
  "jsonrpc": "2.0",
  "method": "event",
  "params": {
    "topic": "stream.chunk",
    "data": {
      "run_id": "run-123",
      "content": "Hello! How can I help you?"
    }
  }
}
```

---

## RPC Methods

### Agent Methods

| Method | Description | Parameters |
|--------|-------------|------------|
| `agent.run` | Start agent execution | `message`, `session_key`, `thinking?`, `model?` |
| `agent.status` | Get run status | `run_id` |
| `agent.cancel` | Cancel running agent | `run_id` |
| `agent.abort` | Force abort | `run_id` |
| `agent.resume` | Re-trigger this session's interrupted run (see [On-demand resume](#on-demand-resume)) | `session_key` |

#### On-demand resume

`ResumeCoordinator` scans for interrupted runs once, at boot. That covers the
case it was built for and nothing else: a run interrupted while the daemon kept
running, or a candidate skipped after a transient store error, had no second
trigger and no way for anyone to ask for one. `agent.resume` and
`POST /v1/admin/resume` (which `aleph-server resume <session-key>` calls) are
that ask.

Both land in `handlers::resume::resume_named_session` →
`ResumeCoordinator::resume_session` → `resume_from_markers`, which the boot
scan also calls. Every judgement — recency filter, crash-loop cap,
crash-boundary repair, concurrency permit — is made once. **Change a resume
criterion in `resume_from_markers` or you have built a second, weaker resume
wearing the same word.**

Four things worth knowing before touching it:

- **`resume_session` deliberately ignores `[resume] enabled`.** That switch
  governs whether the daemon resumes things nobody asked it to. An operator
  naming a session has already made the decision it exists to defer, and
  silently ignoring an explicit request is the kind of no-op that reads as a
  broken feature. Consequently `set_global_resume_coordinator` must be called
  **outside** the `enabled` branch at boot — a handle installed under a
  narrower condition than its consumers fails closed on exactly the
  deployments that need the manual verb most, and its only symptom is a
  rejection.
- **Visibility is `KeyChecked`, and it is worth different amounts per
  transport.** Over JSON-RPC the caller identity is scoped around
  `process_request`, so `visibility::session_visible` compares against a real
  actor and an invisible session gets the byte-identical `not_found` a missing
  one gets. Over `/v1/admin` there is no such scope, `visible_owner_filter()`
  is `None`, and the gate admits everything — which is the trust model working
  (that route is bearer-authenticated with the operator's shared token, and an
  operator sees every session). The check lives in the shared body so this
  reasoning is stated once rather than re-derived per transport.
- **Lane is `Execute`, set explicitly.** `agent.resume` starts agent
  execution; it just takes its input from the session log instead of the
  request. The `.resume` suffix matches no heuristic token, so without the
  `lane.rs::override_for` entry it defaults to `Mutate` and a burst of resumes
  competes on the generic mutation lane instead of the run-concurrency budget.
- **The CLI is IPC-only, with no local fallback.** Resuming means re-entering
  the harness with the session's provider, tools and workspace. A `LockOrIpc`
  local half would either do nothing or stand a second runtime beside the
  singleton, so `aleph-server resume` uses `run_no_lock` + `forward_to_server`
  and says so when no server is running.

- **One resume per session at a time.** `repair_boundary` is a read-then-append,
  so two concurrent resumes of one session both compute the same repair set and
  both append it — leaving one `call_id` answered by two `tool_result`s, which
  the provider rejects on every later turn of that session. The boot scan never
  exposed this (it walks sessions in a sequential loop); the on-demand face
  does, and it can collide with the boot scan itself, which is spawned while the
  gateway is already serving requests. `ResumeCoordinator.in_flight` claims the
  session before anything reads the log; a collision returns `busy` rather than
  proceeding. `already_resuming` is checked **first** when deriving the status
  word, because a busy report has every other counter at zero and would
  otherwise render as `no_runs` — telling the operator a session has no history
  at the moment it is being resumed.

Status vocabulary (same on both surfaces): `resumed` · `already_resuming` ·
`already_finished`
(scanned, newest marker was a `RunFinished`) · `no_runs` (the session has no
run markers at all — an answer, not a failure) · `abandoned` (too old, or the
crash-loop cap tripped) · `not_resumed` (interrupted, but the boundary repair
or the re-trigger failed; the server log has the reason).

**Crash-boundary wording is part of this contract.** A dangling
`ToolCallRequested` is answered with `boundary_repair_text`, which states that
the outcome is **unknown** — not that the call failed. `ToolCallRequested` is
persisted immediately before dispatch, and the two things that can still stop a
call after that point (a guardrail `Block`, an approval denial) each write
their own answer event, so "requested, never answered" means the call reached
or passed the dispatch line and its side effects may have landed. The previous
text read as a verdict, and the rational response to a failed call is to issue
it again.

### Session Methods

| Method | Description | Parameters |
|--------|-------------|------------|
| `session.get` | Get session info | `session_key` |
| `session.list` | List all sessions | `filter?` |
| `session.history` | Get message history | `session_key`, `limit?` |
| `session.compact` | Summarize older turns and drop them from the live context (`context::compact::manual`; soft-retires the event log, deletes nothing) | `session_key`, `instructions?` |
| `session.delete` | Delete session | `session_key` |

### Config Methods

| Method | Description | Parameters |
|--------|-------------|------------|
| `config.get` | Get current config | - |
| `config.patch` | Partial update | `patch` (JSON Merge Patch) |
| `config.apply` | Full replace | `config` |
| `config.reload` | Reload from file | - |

### Event Methods

| Method | Description | Parameters |
|--------|-------------|------------|
| `events.subscribe` | Subscribe to topic | `pattern` (glob) |
| `events.unsubscribe` | Unsubscribe | `pattern` |
| `events.list` | List subscriptions | - |

### Memory Methods

| Method | Description | Parameters |
|--------|-------------|------------|
| `memory.store` | Store fact | `content`, `metadata?` |
| `memory.search` | Search facts | `query`, `limit?` |
| `memory.delete` | Delete fact | `fact_id` |
| `memory.stats` | Get statistics | - |

### Browser Methods (CDP)

| Method | Description | Parameters |
|--------|-------------|------------|
| `browser.navigate` | Go to URL | `url` |
| `browser.click` | Click element | `selector` |
| `browser.type` | Type text | `selector`, `text` |
| `browser.screenshot` | Take screenshot | `selector?` |
| `browser.evaluate` | Run JavaScript | `script` |

### Other Methods

| Domain | Methods |
|--------|---------|
| `connect` | LAN-trust handshake (no auth; always `operator`) |
| `pairing.*` | `list`, `approve`, `reject` — **channel** sender approval (iMessage/Telegram unknown senders), not device auth |
| `interface.*` | `status`, `config` |
| `mcp.*` | `start`, `stop`, `list`, `call` |
| `plugins.*` | `install`, `uninstall`, `list`, `enable`, `disable` |
| `skills.*` | `list`, `install`, `activate` |
| `runs.*` | `list`, `status`, `wait`, `queue` |
| `models.*` | `list`, `config` |
| `generation.*` | `image`, `video` |
| `cron.*` | `list`, `add`, `remove`, `run` |

---

## Event Topics

Subscribe to events using glob patterns:

| Pattern | Events |
|---------|--------|
| `stream.*` | All streaming events |
| `stream.chunk` | Text chunks |
| `stream.agent_trace` | Structured loop-originated execution trace |
| `stream.tool_start` | Tool execution start |
| `stream.tool_end` | Tool execution end |
| `agent.*` | Agent lifecycle events |
| `agent.started` | Run started |
| `agent.completed` | Run completed |
| `agent.error` | Run error |
| `session.*` | Session events |
| `config.*` | Configuration changes |

---

## Interfaces

**Location**: `src/gateway/interfaces/`

### Interface Trait

```rust
#[async_trait]
pub trait Interface: Send + Sync {
    fn name(&self) -> &str;

    async fn start(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;

    async fn send_message(
        &self,
        target: &InterfaceTarget,
        message: &str,
    ) -> Result<()>;

    fn is_running(&self) -> bool;
}
```

### Available Interfaces

| Interface | Feature Flag | Description |
|-----------|--------------|-------------|
| CLI | `cli` | Command-line interface |
| Telegram | `telegram` | Telegram Bot API |
| Discord | `discord` | Discord Bot |
| iMessage | always compiled | Apple iMessage — two transports: **Local** (chat.db poll + AppleScript, macOS-only) and **BlueBubbles** (REST + webhook, any OS). See `src/gateway/interfaces/imessage/`. |
| WebChat | `gateway` | Built-in web chat |

### Factory registration is what makes a channel configurable

`handlers::channel::create_channel_from_config` resolves a configured
`[channels.<type>]` entry through the plugin table in
`interfaces/plugin.rs`, and returns `None` for a type that is not in it —
after which `initialize_channels` logs `Failed to create channel` and continues.
**A `ChannelFactory` that is not registered in `interfaces::register_channel_plugins`
is therefore unreachable, however complete it is.**

The table landed 2026-04-05. Channels added after it registered themselves in the
same commit; the ten that predate it (Slack, Discord, Matrix, Mattermost, Signal,
IRC, Nostr, XMPP, Email, Webhook) were never back-filled and were silently
unconfigurable until 2026-07-26. `imessage` and `cli` are deliberately absent —
iMessage is constructed directly in `initialize_channels`, which `continue`s before
consulting the table, and CLI is not a configurable channel type.

New adapters must be added to `register_channel_plugins` by hand.
`every_configurable_channel_type_is_registered` pins the current set against
regression, but it cannot enumerate `impl ChannelFactory`, so it will not catch a
*future* adapter that forgets to register — adding the name to that list is the
same manual step as the registration.

### Self-healing needs intent, not just status

`ChannelHealthMonitor` sweeps for wedged channels and restarts them in place.
Its predicate reads **two** facts, because one is not enough:

- `ChannelRegistry`'s `DesiredChannelState` — `Running` after a successful
  `start_channel` / `restart_channel`, `Stopped` after `stop_channel` or before
  any start. Only the layer that serves start/stop knows this.
- The live `ChannelStatus`, plus staleness.

`ChannelStatus::Disconnected` means three unrelated things — never started,
stopped on purpose, transport died — so a status-only predicate must either
resurrect channels the operator stopped or never rescue a dead socket. The old
predicate chose the latter (`status == Error` alone) and was consequently a
**no-op for exactly the long-lived socket channels the monitor was written
for**: `discord`, `irc` and `xmpp` all write `Disconnected` when their
connection task exits, and Discord assigned `Error` and then unconditionally
overwrote it on the next line. Registered, budgeted, tested, never fired.

The predicate is now "supposed to be running AND down AND stale", where down
covers `Error`, `Disconnected` and a hung `Connecting`. `Connected`-but-silent
is still excluded — a quiet Slack workspace is not a broken one, and Aleph has
no separate transport-liveness signal to tell them apart (openclaw restarts on
`stale-socket` only because it tracks `lastTransportActivityAt`). `Disabled` is
excluded as configuration. Mapped from openclaw's `isManagedAccount` +
`lifecycle` in `gateway/channel-health-policy.ts`.

**Adapter rule**: do not unconditionally overwrite `status` on a connection
task's exit path. That one line erases the failure the line above just
recorded, blinding both `channels.list` and the monitor.

### Addressing: channel vs conversation

Three different things get called "channel". Keep them apart when reading an error:

| Term | Type | Example | Where it comes from |
|------|------|---------|---------------------|
| **channel** — the transport | `ChannelId` | `"slack"` | `[channels.*]` config, registered into `ChannelRegistry` |
| **conversation** — the room | `ConversationId` | `"C0A1B2C3"` | opaque platform handle |
| **capability** — what this transport can do | `ChannelCapabilities` | `reactions: true` | the adapter's own `capabilities()` |

`OutboundMessage` needs a `ConversationId`, and until 2026-07-26 the only source of
one was an *inbound* message — so the agent could only ever reply where it had been
spoken to. `Channel::list_conversations(query, limit) -> ConversationPage` closes
that: it is the trait's only read, and it reads **routing metadata only** (name, id,
`is_member`), never message content. That line is deliberate — content fetched by a
*pull* would arrive with none of the access control that `inbound_router::check_permission`
(dm/group policy, pairing, allowlists) applies to *pushed* messages.

`ConversationPage` is `{ conversations, warnings }` rather than a bare `Vec` because a
roster lookup has a real **partial** outcome: an app granted `channels:read` but not
`users:read` can list every channel and no people. A bare `Vec` can only say "no match",
which would have the model report that a person does not exist when the truth is that it
was never allowed to look. Slack therefore fails the call only when **both** sweeps fail;
one failing degrades and names the reason. The same field carries page-cap truncation, so
"we stopped looking" is never mistaken for "not in the roster".

Model-facing, this is the read-only `channel_directory` tool feeding `channel_message`.
They are two tools on purpose: `ToolFacts::idempotent` is keyed on the tool **name**
(`registry_adapter::READ_ONLY_TOOLS`), so folding a lookup into non-idempotent
`channel_message` would gate it under the `Ask` exec tier — and a tier never widens,
so there would be no way back.

Slack implements it in `interfaces/slack/directory.rs::ConversationDirectory`
(`conversations.list` + `users.list`, cursor-paginated, 15-minute TTL cache, hard page
cap). The cap is not cosmetic: `ChannelRegistry` holds the channel's **read guard**
across the adapter call, so an unbounded sweep would block the write lock that
`stop_channel` / `restart_channel` need.

### Capability flags are promises

Each `ChannelCapabilities` bool claims the matching optional `Channel` method works.
An adapter that sets one **must** override that method: the default bodies now return
`ChannelError::UnsupportedFeature` naming the adapter, where they used to return
`Ok(())` and let the caller report a success that never happened. Six shipped adapters
were in exactly that state — `msteams.reactions` and `whatsapp.deletion` made
`channel_message` answer `delivered: true` for a no-op. Pinned by
`declared_but_unimplemented_optional_methods_fail_loudly` in `channel.rs`.

### Interface Configuration

```json5
{
  "interfaces": {
    "telegram": {
      "token": "BOT_TOKEN",
      "allowFrom": ["+1234567890"],
      "groups": {
        "*": { "requireMention": true }
      }
    },
    "discord": {
      "token": "BOT_TOKEN",
      "guilds": ["guild-id-1"]
    }
  }
}
```

---

## Durable Outbound Delivery Queue

**Location**: `src/gateway/delivery_queue.rs` (store + drain loop),
`src/gateway/channel_registry.rs` (enqueue-on-failure + accessors),
`src/builtin_tools/channel_outbox.rs` (operator/model surface).
**Config**: `[gateway.delivery_queue]`. **Opt-out**: no store attached ⇒
byte-identical pre-queue behaviour.

`ChannelRegistry::send` retries only `RateLimited`, in memory, for a bounded
window. Every other *definitely-not-delivered* failure — above all
`NotConnected` while a channel reconnects — used to drop the message, and
nothing survived a restart. For an assistant whose core promise is proactive
push (R5), a lost Daemon notification is a silent correctness failure.

### The three-way outcome, not two

Delivery has three endings, and the queue used to model two:

| Ending | Classifier | Action |
|---|---|---|
| Reported, definitely not delivered (`NotConnected` / `RateLimited`) | `should_enqueue` | persist, retry with backoff |
| Reported, may already be on the wire (`SendFailed` / `Internal`) | `terminal_reason` → `Ambiguous` | never retried; dead-lettered for inspection |
| **Not reported at all — the process vanished mid-send** | in-flight stamp | never retried; dead-lettered as `UnknownOutcome` |

The third row is why `mark_inflight` stamps a row *before* the send crosses the
transport boundary and `reconcile_inflight` runs once per process (from
`spawn_drain`, at the moment it wins the drainer slot). Without it, a daemon
that exits between a successful send and `mark_delivered` leaves a pending,
already-due row that the next boot replays — a duplicate. Mapped from
openclaw's `markDeliveryPlatformSendAttemptStarted` /
`needsUnknownSendReconciliation`; Aleph skips its multi-process lease because
the single-drainer invariant (`try_claim_drainer`) makes it unnecessary.

### Ordering is per conversation

`drain_once` groups a claimed batch by `(channel, conversation)`, preserves
claim order, and stops a conversation at its first non-success. Grouping alone
is not enough across ticks: the failed head backs off into the future while its
followers stay due, so the next tick would claim a follower, never see the
head, and deliver it first — permanently reordering the chat. The head's
backoff is therefore carried to the rest of its conversation
(`defer_conversation`, over an indexed `conversation_id` column backfilled at
open). Different conversations never block each other.

That covered only the *queued* half. A live `send` that succeeded while an
older message for the same conversation was still serving out a backoff arrived
first — permanently, and up to `max_backoff` early. `send` now flushes that
conversation's backlog ahead of itself (`flush_conversation`), which keeps the
return contract byte-for-byte: the caller still gets a real `SendResult` or the
exact error it would have got before. The rejected alternative was admitting
live sends into the queue, which changes what every caller observes (a queued
message has no `SendResult`) and converts a reordering bug into head-of-line
*unavailability* — one wedged record would hold up its conversation until it
dead-lettered, roughly 45 minutes at the default curve.

Three constraints make the flush safe:

| Constraint | Why |
|---|---|
| `claim_conversation` ignores `next_attempt_at` | the record a live send overtakes is precisely the one sitting in the future |
| `AttemptMode::Inline` never settles a *transient* failure (`clear_inflight` only, `attempts` / `next_attempt_at` untouched) | an opportunistic probe must not spend the backlog's retry budget — ten user messages would otherwise burn the head's ten attempts and dead-letter it far ahead of the configured curve. Ambiguous *terminal* failures are still settled: handing a possibly-on-the-wire send back for another attempt is how at-most-once dies |
| `drain_gate` serializes flush against the drain loop; `try_lock`, never blocking | both claims are unleased SELECTs — this is exactly the "a second drainer is introduced" case `claim_due` warns about. A process-local mutex suffices because the single-drainer invariant already is process-local. Standing down when the drainer holds it beats blocking a user-facing send behind a whole tick |

Bounded by `inline_flush_max` (default 8, `0` disables; deliberately not floored
at 1). A backlog deeper than the cap is flushed up to the cap and the overtake
is logged, not silently accepted.

### Dead letters carry their own replay-safety

`redrive_dead_letters` used to rest on "every dead letter is duplicate-safe by
construction", which was true only while exhausted transient retries were the
sole producer. With terminal failures and interrupted attempts also landing
there, safety is per record: `DeadLetterReason::replay_safe` (single source in
Rust, projected into SQL by `replay_safe_tokens`). Redrive moves `Exhausted` /
`Permanent` and reports the rest as `skipped_unsafe`.

Redrive also respects the live-queue bound by **moving fewer records**, never
by evicting live ones. The previous implementation moved everything then
trimmed by oldest `created_at` — and since redriven rows carry `created_at =
now`, the rows evicted were the genuinely-older *pending* deliveries.

### Bounds

- `max_queue_len` — row count, oldest-first eviction, both tables.
- `max_payload_bytes` — serialized size. `OutboundMessage` carries
  `Attachment`s that may hold inline `data: Vec<u8>`, so a row-count cap alone
  lets a few media pushes to a wedged channel grow `delivery.db` without bound.
  Over-cap payloads are dead-lettered (`PayloadTooLarge`), not silently dropped.

### A durable row must not outlive its media

Every local `Attachment.path` Aleph produces points **inside the OS temp
directory** — `media::cache` refuses to attach a local path from anywhere else
(arbitrary-file exfiltration guard) and TTS writes there too. So the one thing
this queue exists to promise, "it survives a restart", was false for any row
carrying media: a path attachment serializes to a couple of hundred bytes,
sails past `max_payload_bytes`, and replays as a message whose media is gone.
The byte cap cannot see this — it is measuring the reference, not the file.

`take_media_custody` (called from `maybe_enqueue`, the single admission
chokepoint, and only on the failure path) inlines the bytes into
`Attachment.data` — the branch every adapter already prefers over `path`.
Custody lives **in the row**, not in a spool directory: the row stays
self-contained, so eviction, dead-lettering, redrive and the byte cap keep
working unchanged and there is no second lifecycle to garbage-collect. openclaw
needs its `delivery-queue-media-spool.ts` (stage table, `.part` publishes, 24h
orphan grace, prune sweep) because it is queue-first — every message goes
through the queue, so the copy must be cheap and out of band. Aleph queues only
on failure, so it can afford the simplest shape. Do not port the spool.

Both refusals leave the attachment exactly as the caller wrote it, so a queued
row is never *less* deliverable than before: an unreadable file (already gone,
permissions) has nothing to inline, and bytes that would push the payload over
`max_payload_bytes` are better queued as a path that *might* still be there
than dead-lettered on the spot. Both are logged. The path's file name moves
into `filename` on the way in, because inline bytes carry no name of their own.

### Surfaces

- `channels.list` → `delivery_queue`: counts only (depth, due, oldest age,
  per-channel, dead-lettered, dead-lettered-replayable). Panel reads this.
- `channel_outbox` tool → `status` / `dead_letters` / `redrive`. This is the
  **only** production consumer of `recent_dead_letters` / `redrive_dead_letters`;
  the `channels.dead_letters` / `channels.redrive_dead_letters` RPCs were
  removed as orphans and must not be revived — no client ever called them, and
  the consumer that exists is the model (R8).

---

## Busy Input & Wait Lane

**Location**: `src/gateway/busy_queue/` (lane + delivery loop + config)

Exactly one run may be in flight per session (`execution_engine::SessionRunRegistry`).
A message that arrives while its session is busy is routed by the originating
channel's declared `BusyInputMode`:

| Mode | Behaviour |
|---|---|
| `Steer` (default) | Injected into the live event log; the running loop consumes it at its next turn boundary. |
| `Interrupt` | Cancels the session's run **and its delegated child runs**, then the message restarts as a fresh run via the lane. |
| `Queue` | Leaves the running task alone; the message waits in the lane. |

Anything that cannot be delivered inline joins its session's **FIFO wait lane**.
All three surfaces share the one lane — the inbound router (channels) and both
`aleph-server` RPC handlers (`agent.run`, `chat.send`, via
`busy_queue::spawn_queued_run`) call `busy_queue::register` on the arrival path
and `busy_queue::deliver_with_ticket` inside the spawned delivery task.

Invariants worth preserving:

- **Ticket is taken synchronously on the arrival path**, before the delivery task
  is spawned. Registering inside the task makes lane order follow task
  scheduling instead of arrival order.
- **The lane is a waiting room, not a run registry.** `deliver_with_ticket`
  holds its ticket across the whole `attempt()`, and `attempt()` *is* the agent
  run — so `SessionRunRegistry::try_claim` calls `busy_queue::mark_admitted` to
  withdraw the ticket the moment the run is admitted (the exact mirror of
  `release` → `notify_slot_free`). Without it the running message sits at the
  head of its own lane for the run's entire lifetime, every follow-up parks
  behind the very run it wants to change, and `Steer` / `Interrupt` — which only
  mean anything *while* a sibling runs — silently degrade to `Queue`. The same
  root cause made `/stop` count the message it was stopping among the "queued
  messages dropped" and inflated `busy_queue.total_waiting` by one per busy
  session. FIFO constrains only the messages that are still waiting.
- **Waiters do not poll.** They park on a per-session `Notify` fired by
  `SessionRunRegistry::release` (the authoritative slot-free edge),
  `mark_admitted` (the symmetric "the lane just got shorter" edge), and ticket
  departures. `busy_queue_wake_fallback_secs` is a missed-signal safety net, not
  the delivery latency.
- **`TicketGuard` is the only way in or out.** Its `Drop` is load-bearing: a
  panic while holding the front ticket would otherwise wedge the lane until
  daemon restart.
- **A stale or unknown ticket fails open** — the engine's gate is the real
  authority, so the worst case is one redundant delivery attempt.
- **Report a failure once.** `DeliveryOutcome::Executed(_)` means the run's own
  emitter already sent a `RunError`; only the never-ran outcomes are the
  caller's to report.

Stopping has two granularities: `/stop` purges the whole session lane
(`busy_queue::purge`, wired only in `command_handler::handle_stop` — the
`Interrupt` mode depends on the lane to restart its own message, so
`cancel_session` must not purge), while `chat.abort` reaches a single queued
message by `run_id` (`busy_queue::cancel_queued_run`, wired in
`AgentRunManager::cancel_run` — a queued run has no `active_runs` entry, so the
engine's own cancel cannot see it).

Knobs live in `[execution]`: `busy_queue_max_per_session` (32),
`busy_queue_max_wait_secs` (1800), `busy_queue_wake_fallback_secs` (30),
`max_pending_steering` (16). Backlog is observable via
`gateway.metrics.run_concurrency` → `busy_queue`.

### A side question gets its own lane, and the arrival layers must agree

`/btw <question>` runs as an ordinary turn on a session **derived** from the
conversation it was typed in, so that it can be answered while the main run
keeps going. That promise is entirely a routing property, and routing it is not
one chokepoint — it is **one writer plus one query**, because the lane is
claimed before the engine is ever entered.

**The writer** is `execution_engine::stamp_btw`
(`execution_engine/slash_command.rs:67`) — parser-free, idempotent, and the only
thing that ever sets `btw::BTW_METADATA_KEY`. It runs at three places, all of
them *before* the busy lane: the inbound router's execute path
(`inbound_router/executor.rs:451`), inside `stamp_slash_mode`
(`slash_command.rs:98`, whose first statement it is) which the `agent.run` and
`chat.send` run-start handlers call before `busy_queue::spawn_queued_run`
(`src/bin/aleph-server/server_init.rs:272,466`; the guard
`every_run_start_handler_stamps_the_slash_mode_before_the_busy_lane` is phrased
over that file's handlers and inherits nothing outside it), and as the first
statement of `ExecutionEngine::execute` (`execution_engine/execute.rs:374`).

*Stamped after the lane gate, every `/btw` sent during a run is folded into that
run as steering text* — `steering::carries_more_than_text` reads the same key to
decide that a message must be redelivered as its own run rather than injected
into a running sibling.

**The query** is `btw::execution_session(addressed_to, metadata)`
(`gateway/btw/mod.rs:142`), deliberately a query and not a mutation, because two
layers that are not adjacent both need the answer:

- `busy_queue::register_run` (`busy_queue/mod.rs:238`) keys the lane on it, so a
  side question waits in its own lane rather than behind the very run it is
  asking about;
- `ExecutionEngine::execute`'s `redirect_to_side_session`
  (`execute.rs:98`, called at `:381`) moves the request onto the derived key
  **before `admit_run`** (`:458`), so the engine's per-session slot claim and the
  busy-input policy both see the side session.

Asking twice is free; *writing* twice would derive the side key of the side key
and land the run where neither layer named. The idempotence is
`btw::is_side_key` (`btw/mod.rs:203`), and it is load-bearing beyond the two
arrival layers: `steering::build_steering_rescue_request` re-enters `execute()`
with the metadata *and* the already-derived key cloned, so a completed side
question with an unanswered steering burst is a third ask.

Channels reach that path through an explicit claim at
`inbound_router/mod.rs:872`, placed **above** the unified slash interception and
keeping both things the turn needs — the `/btw` prefix (so `stamp_btw` fires) and
the conversation's own session key (so `execution_session` has something to
derive from). It has to be a claim rather than a fall-through because both paths
below it are wrong in their own way: the unified interception routes anything the
shared `CommandParser` resolves into the engine's slash fast path, which
dispatches on the raw `ToolRegistry` and never builds the `ScopedToolService` the
read-only ceiling lives in; and anything it does *not* resolve reaches
`try_send_unknown_command_help`, which answers "did you mean …?" and returns
without running the agent — `suggest_commands("btw", 3)` returns
`["session_new"]` today (alias `new`, edit distance 2), so the fall-through is
broken now, not latently.

`/stop` in the main conversation reaches the side question too:
`ExecutionEngine::cancel_session` (`execution_engine/engine.rs:549`) walks
`btw::side_session_of` (`:557`), which returns `None` for an already-derived key
so the walk is one level deep by construction. Full account, including the
read-only ceiling and the retirement surfaces, in
[FEATURE_LOCATOR §4.14](FEATURE_LOCATOR.md#414-btw-侧问派生的只读侧会话-side-questions--2026-08-20).

## Many clients, one thread

A session key names a **conversation**, not a viewer. Any number of clients can
be watching the same one — two Panel tabs, a Panel and the TUI, every member of
a project room, plus whatever channel or cron job started the turn. There is no
`attach` verb and deliberately none: delivery is `topic + per-connection
visibility projection`, and an attachment table would be a second source of
truth for a question `event_visibility` already answers. What that model owes
its clients instead is two things, both of which were missing until 2026-08-10
(full account in [FEATURE_LOCATOR §6.9](FEATURE_LOCATOR.md#69-多端共享一条线程--重连与崩溃后的状态重建-multi-client-thread-sharing--post-reconnect-state-rebuild)):

- **Every frame must carry enough identity to be routed by a client that did
  not start the run.** `RunAccepted` has always carried `session_key`; what was
  missing was a client using it instead of guessing "the conversation in front
  of the user". A frame that names a session no local view is showing must be
  **dropped**, never redirected.
- **A client joining mid-turn needs a pointer to the turn in flight.**
  `chat.history` answers it: the response carries `active_run` — the run id
  currently claiming this session's slot in the `SessionRunRegistry`, or `null`.
  It rides on that response rather than a method of its own so the transcript
  and the live pointer are one snapshot, and so it inherits the visibility gate
  `handle_history` has already passed instead of opening a second one. Sourced
  through `ExecutionAdapter::active_run_for_session` (default `None`), because
  the registry is the only table that sees runs from *every* interface —
  `AgentRunManager::active_runs` holds Panel-started runs only.

**Turn-end announcements go out on both terminal arms.** `SessionUpdated` is
what every other surface re-hydrates on, and a run that failed, timed out or
was cancelled still moved the transcript (the harness appends the user message
*before* dispatch). `ExecutionEngine::announce_turn_end` is the single
derivation both arms of `execute()` call; a source-level guard pins that the
failure path is one of them.

## Session Routing

**Location**: `src/routing/session_key.rs`

### Session Key Variants

| Variant | Format | Use Case |
|---------|--------|----------|
| **Main** | `agent:main:main` | Cross-channel shared session |
| **DirectMessage** | `agent:main:telegram:dm:user123` | Per-user DM |
| **Group** | `agent:main:discord:group:guild-id` | Group/channel chat |
| **Task** | `agent:main:cron:daily-summary` | Cron jobs, webhooks |
| **Subagent** | `subagent:agent:main:translator` | Sub-agent delegation |
| **Ephemeral** | `agent:main:ephemeral:uuid` | Temporary, no persistence |

### DM Scope Strategies

```rust
pub enum DmScope {
    Main,           // All DMs share main session
    PerPeer,        // Isolated per user (default)
    PerChannelPeer, // Isolated per channel + user
}
```

---

## Session Manager

**Location**: `src/gateway/session_manager.rs`

### Storage Schema

```sql
CREATE TABLE sessions (
    session_key TEXT PRIMARY KEY,
    messages TEXT,           -- JSON array
    created_at INTEGER,
    updated_at INTEGER,
    message_count INTEGER,
    token_count INTEGER
);

CREATE TABLE session_metadata (
    session_key TEXT PRIMARY KEY,
    agent_id TEXT,
    channel TEXT,
    last_compaction INTEGER
);
```

### Compaction

When session exceeds token threshold:

1. Extract key facts from old messages
2. Store facts in memory system
3. Replace old messages with summary
4. Update token count

---

## Security

**Network boundary + Gateway token**: the trust boundary is the network
boundary, gated by a Gateway-token login wall. The default bind is
`127.0.0.1` (loopback only); loopback is the zero-config operator and needs
no token. Set `[gateway] host = "0.0.0.0"` to open the LAN — a remote device
can then reach the socket but is **walled until it presents a valid
credential** at `connect`. Authorization is resolved by
`connect::resolve_connect_auth` in priority order: (1) loopback ⇒ operator;
(2) a valid **device token** (`aleph-dt-*`, long-lived, bound to a paired
device, SHA-256-hashed at rest); (3) a valid **bootstrap ticket**
(`aleph-bt-*`, 5-min single-use, exchanged for a fresh device token during
onboarding); (4) the legacy shared **Gateway token** (`aleph-<uuid>`,
`SharedTokenManager`, HMAC-hashed, constant-time verified). A valid
credential = full operator authority (identical to local); a missing/invalid
one is walled — the WS dispatch refuses every method but `connect`.
Revocation is token rotation (`gateway.token.rotate`, which also force-closes
live remote sockets) or per-device revoke (`gateway.devices.revoke`, which
drops that device's live sessions to the login wall and then closes their
sockets with WS 4001 `device_revoked`) — both effective immediately, not at the
next handshake. The WS
Origin check (`src/gateway/origin_policy.rs`) additionally blocks public web
pages from cross-origin-driving the local daemon. See
[SECURITY.md#auth-ux](SECURITY.md#auth-ux) for the full model.

### Connect handshake

The first frame on a `/ws` connection must be `connect`. Loopback carries no
credential (zero-config operator). A remote connection presents one of
`device_token`, `bootstrap_ticket`, or `token` (the legacy shared Gateway
token) in `connect` params; `resolve_connect_auth`
(`src/gateway/handlers/connect.rs`) stamps the resolved role (`operator` when
authorized, else `guest`) onto the connection state, and the response echoes
`role` / `authorized` / `needs_token`. A bootstrap-ticket exchange also
returns a freshly minted `device_token` the client persists for subsequent
reconnects. A rejected remote `connect` is recorded in the security audit log
(`AuditEventType::AuthFailure`, bounded by the `Auth`-scope rate limiter).

```json
{
  "method": "connect",
  "params": {
    "minProtocol": 1,
    "maxProtocol": 1,
    "client": {
      "id": "macos-app",
      "version": "1.0.0",
      "platform": "macos"
    },
    "device_token": "aleph-dt-…",
    "bootstrap_ticket": "aleph-bt-…"
  }
}
```

> Loopback clients omit the credential fields entirely. A remote client sends
> `bootstrap_ticket` on first pairing (receiving a `device_token` back), then
> `device_token` on every reconnect. `token` (the legacy shared Gateway token)
> is accepted as a fallback.
>
> `device_id` is **client-asserted** and the `devices` table is one namespace
> shared with cluster nodes, so the exchange refuses a `device_id` that already
> names a non-Panel device (and `cluster::admit_node` refuses the mirror case).
> Without that guard one ticket buys an operator token the Panel roster cannot
> see and no revoke path can reach.

---

## Hot Reload

**Location**: `src/gateway/hot_reload.rs`

Configuration changes are detected via file watcher:

```
~/.aleph/config.json modified
    │
    ▼
┌─────────────────────────────────┐
│ Debounce (500ms)                │
└─────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────┐
│ Parse new config                │
│ Validate against schema         │
└─────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────┐
│ Apply changes                   │
│ • Restart affected interfaces    │
│ • Update routing rules          │
│ • Emit config.changed event     │
└─────────────────────────────────┘
```

---

## HTTP Server

**Location**: `src/gateway/http_server.rs`

Alongside WebSocket, Gateway serves:
- Static files (WebChat UI)
- Liveness probe (`/health`) and readiness probe (`/ready`)
- Metrics endpoint (`/metrics`) — Prometheus text exposition (v0.0.4) of
  request-lifecycle counters, connection gauges, rate-limiter pressure, and a
  request-duration histogram (`aleph_gateway_request_duration_ms`, fed by the
  per-request `elapsed_ms` the metrics middleware already measures); exports
  only aggregate counts (no payloads/secrets), unauthenticated like the probes.
  Implemented in `src/gateway/server/metrics_endpoint.rs` +
  `src/gateway/middleware/latency.rs`.

Abuse protection at WS upgrade: besides the global `max_connections` cap, a
per-IP concurrent-connection cap (`gateway.max_connections_per_ip`, default 64,
`0` disables, loopback exempt) bounds slot-exhaustion — a remote peer
opening many idle sockets.

### Reverse proxies (no X-Forwarded-For resolution)

The IP-keyed abuse protections (per-IP connection cap, rate limiter, `Auth`-scope
lockout) key off the **raw socket peer address** (`peer_addr.ip()`, verbatim).
`X-Forwarded-For` / trusted-proxy resolution was removed with the LAN-trust
revert and is **not** reinstated — a `trusted_proxies` key in config is a
silently-ignored legacy field, and there is no `src/gateway/trusted_proxy.rs`.
Keeping the loopback check on the raw peer is deliberate: it means `is_loopback`
(the zero-config-operator grant) can never be forged by a spoofed `X-Forwarded-For`
header.

The trade-off: when the gateway is fronted by a reverse proxy, every client
collapses to the proxy's socket address, so the per-IP protections bound the
*proxy* rather than individual clients. Terminate client-identity trust upstream
(the proxy) if you need per-client limits, and treat the Gateway token as the
transport auth. (Restoring fail-closed, allowlist-gated trusted-proxy XFF
resolution — never letting a forwarded header influence `is_loopback` — is
tracked as a future enhancement.)

### Method-level authorization

The connection-level barrier is the **login wall**: a remote connection that has
not presented a valid Gateway credential is `guest` and may only issue `connect`
(§Connect handshake). Once authorized it is `operator`, identical to local —
there is no finer per-RPC operator-vs-guest tier on the Panel surface. A separate
classifier survives at the *channel* tool-dispatch tier
(`src/gateway/method_authz.rs`, consumed by `ScopedToolService`): the
`inbound_router` caps a chat-tier channel (Telegram/Slack, default `guest`) so it
cannot run Aleph's self-config tools. Limiting *what an agent may do* (vs *who may
connect*) is the job of the per-channel tool-permission layer, orthogonal to
connection trust.

### Distributed-trace correlation

Each JSON-RPC request resolves a [W3C `traceparent`](https://www.w3.org/TR/trace-context/):
an inbound `params.traceparent` is honoured (its trace id adopted), otherwise a
fresh 128-bit root trace is minted. The dispatch chokepoint opens a `tracing`
span carrying `trace_id`/`span_id`, and the response echoes a `traceparent`
naming the server's span as the parent so a multi-hop call graph stitches
together. This is a lightweight propagation layer (`src/gateway/trace_context.rs`),
**not** an OpenTelemetry integration — the OTel SDK would violate core
minimalism (R3) for what is, given Aleph's own trace persistence and `tracing`
logging, a correlation feature.

> Note: the JSON-RPC middleware chain is built once at server construction and
> cloned per connection. Building it per-connection previously reinstalled the
> global request-state registry on every connect, zeroing the `/metrics`
> request-lifecycle counters and undercounting in-flight requests.

### Channel webhook ingestion

Channels that receive over HTTP POST (`generic webhook`, and future ones)
return a handler from `Channel::webhook_handler()`. `build_router()` registers
**one constant route** — `POST /webhook/{*rest}` — whose state is the shared
`WebhookMountTable`. `ChannelRegistry` owns that table and is its only writer.

- **Mounting follows the registry, not boot.** `start_channel` /
  `restart_channel` mount; `stop_channel` / `unregister` / `register` /
  `create_channel` unmount. So `channel.stop` and `channel.delete` really do
  remove the endpoint (404, not 503), and a channel created at runtime is
  reachable without restarting the daemon. The earlier version built the route
  table once at boot: `stop` returned `"stopped"` while the endpoint kept
  answering 200 and driving agent runs, because the route held its own
  `Arc<Handler>` clone. The forwarder never exiting has a separate,
  mount-independent cause: the forwarder task captures a `channel_arc` clone
  by move and holds it for its entire — infinite — lifetime
  (`channel_registry.rs` `start_message_forwarder`). That keeps the channel
  instance alive, which keeps `ChannelState`'s original `Sender` alive, so
  `RecvError::Closed` is structurally unreachable for any started channel,
  whether or not a mount exists.
- **⚠️ `restart_channel` does not go through `stop_channel`/`start_channel`.**
  It calls `channel.stop()` + `channel.start()` directly, so it carries its own
  mount refresh. A hook set that only covers start/stop leaves the pre-restart
  handler clone in the table forever.
- **`path` must be under `/webhook/`**, enforced by
  `WebhookChannelConfig::validate()` and again by `WebhookMountTable::mount()`
  (one predicate, `is_mountable_path`). Because operator-writable paths never
  enter axum's route table, a bad path can no longer panic `Router::merge` at
  boot, and can no longer shadow a Panel SPA path (`path = "/settings"` used to
  turn `GET /settings` into 405). `RESERVED_ROUTE_PREFIXES` existed only to
  guard that boot panic and was withdrawn with it.
- **⚠️ matchit does not panic on `/webhook/foo` next to `/webhook/{*rest}`** —
  the more specific static route just wins. A future gateway route under this
  prefix would therefore *silently* steal a channel's webhook path. The guard
  is a source scan in `server/mod.rs`'s own tests
  (`build_router_registers_no_second_route_under_the_webhook_prefix`); axum
  cannot be asked what is in its route table.
- **Two channels, one path** → the lower `channel_id` keeps the route, warned
  with both ids. Deterministic on purpose: `start_all` iterates a HashMap, so
  arrival order would make route ownership a per-boot coin flip. The loser is
  only warned — it still reports `Connected` in `channels.list` while being
  deaf, and the `channel.start` RPC still answers `{"status":"started"}`:
  `mount()`'s refusal `bool` is discarded at both call sites, so the operator
  sees the `warn!` naming both ids but the RPC caller is told nothing. For
  that one case this is a small step backwards from the deleted
  `"restart_required"` receipt. Recorded limit, not a fix — threading the
  refusal into the receipt changes an RPC response shape and needs its own
  round.
- **One port.** Webhook traffic rides the gateway's own listener, so it
  inherits `[gateway] host`, TLS, and `SecurityHeadersLayer`. `WebhookReceiver`
  deliberately owns no listener — the version that bound `0.0.0.0` itself would
  have opened a LAN surface regardless of the configured host.
- **Auth is per-handler HMAC**, not the login wall — an external platform
  cannot present a device token. Same posture as `/health`, `/metrics`, `/a2a`:
  no transport-level auth, no rate limiter (that lives in `MiddlewareChain`,
  on the JSON-RPC/WS path only). The signature also binds no timestamp or
  nonce (unlike Stripe/GitHub's `t=…,v1=…`), so replay protection is
  incidental — it comes only from inbound dedup at
  `src/gateway/inbound_router/dedup.rs`, whose window is **5 minutes**; a
  captured signed request replayed after that re-triggers an agent run. This
  is posture, not a known gap requiring action.
- **Check order is deliberate**: lookup → 404, signature → 403, channel status
  → 503, then parse and forward. Signature comes *before* status so an
  unauthenticated caller learns only whether a mount exists at that path,
  never the mounted channel's status.
  The status check is depth only, for a channel that moved itself to
  `Error`/`Connecting` without any RPC; `try_read` fails **open** on
  contention, because a momentary write-lock holder is not evidence the
  channel is down.
- ⚠️ The sink is the channel's **own** `ChannelState::sender()`, not the
  registry's. Going direct to the registry bypasses
  `start_message_forwarder`, the only place inbound traffic stamps
  `health.record_event()` — the channel would receive while health monitoring
  reported it dead.

---

## See Also

- [Architecture](ARCHITECTURE.md) - System overview
- [Agent System](AGENT_SYSTEM.md) - Agent loop
- [Security](SECURITY.md) - Exec approval system
