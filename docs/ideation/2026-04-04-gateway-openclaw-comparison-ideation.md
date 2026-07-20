---
date: 2026-04-04
topic: gateway-openclaw-comparison
focus: Compare OpenClaw gateway implementation, identify Aleph deficiencies, propose improvements that leverage Rust advantages
---

# Ideation: Gateway Architecture — Learn from OpenClaw, Surpass OpenClaw

## Codebase Context

### OpenClaw Gateway Strengths
- Protocol-first design: AJV schema validation, versioned wire format (v3), min/max negotiation at handshake
- Event sequence numbers (`seq`) + state versioning (`stateVersion`) for gap detection and resync
- Streaming text merging: intelligent delta merging with `chatDeltaLastBroadcastLen`
- Idempotency key support for side-effecting operations
- Preauth budget tracking to limit resource exhaustion before authentication
- Plugin-based channel system: channels are plugin contracts, not hardcoded
- Channel health monitoring with automatic recovery policies
- Exec approval manager: decoupled, async human-in-the-loop with audit logging
- Session DAG threading: parent ID chains for conversation branching
- 74+ server method handlers via central dispatch with role-based RBAC

### Aleph Gateway Strengths
- Trait-based abstractions (Channel, EventEmitter, ExecutionAdapter) — clean low coupling
- SessionKey enum making illegal states unrepresentable (Main, PerPeer, Task, Ephemeral)
- Pure StreamingController state machine decoupled from I/O
- Topic-based event filtering with glob patterns (GatewayEventBus)
- DashMap-based lock-free per-identity rate limiting
- SQLite persistence with FTS5 search, session compaction via LLM
- 12+ channel implementations with ChannelCapabilities
- Isolated AgentInstance execution environments
- Hot reload via ConfigWatcher

### Aleph Gateway Gaps (vs OpenClaw)
- No protocol versioning or schema validation at wire level
- Broadcast channel (tokio 1024 buffer) silently drops events for slow subscribers
- No idempotency key support — duplicate WebSocket messages replay
- No event sequence numbers for gap detection or replay
- No preauth budget tracking
- Channels are compile-time linked, not dynamically loadable
- No message middleware/hook framework (pre/post processing)
- Session compaction blocks agent processing (synchronous LLM call)
- Exec approval system exists but no UI integration
- No conversation branching (DAG)

## Ranked Ideas

### 1. Sequenced Event Stream with Gap Detection & Replay
**Description:** Add global monotonic `seq: u64` to every `TopicEvent`. Clients detect gaps and request backfill via `events.backfill` RPC. Server maintains bounded ring buffer per run. Existing `StateDatabase.get_events_since_seq()` and `get_events_in_range()` provide the storage layer — just needs wiring to WebSocket path.
**Rationale:** Closes the biggest reliability gap. Infrastructure already partially exists. Synergizes with active trace development work.
**Downsides:** Ring buffer adds memory. Backfill RPC needs client-side support.
**Confidence:** 85%
**Complexity:** Medium
**Status:** Explored (selected for brainstorm)

### 2. Protocol Versioning + Compile-Time Schema Generation
**Description:** Explicit protocol version in handshake with bidirectional negotiation. All request/response types derive `schemars::JsonSchema` at compile time. Rust type system IS the schema — zero drift possible.
**Rationale:** Foundation for all future protocol changes. Compile-time schema is a Rust-only advantage over OpenClaw's runtime AJV.
**Downsides:** Requires modifying connection handshake and params parsing.
**Confidence:** 90%
**Complexity:** Medium
**Status:** Unexplored

### 3. Backpressure-Aware Event Bus with Disk Spillover
**Description:** Keep broadcast channel for fast path; when subscriber lags, spill to SQLite per-connection ring buffer. Subscriber catches up from disk before rejoining live. Leverages existing `agent_events` table gap-fill pattern.
**Rationale:** Solves `handler.rs:606` lagged event warning. WebChat panel missing 1024 streaming events = broken trace display.
**Downsides:** SQLite write I/O increase. Only triggers on actual lag.
**Confidence:** 80%
**Complexity:** Medium-High
**Status:** Unexplored

### 4. Tower-Style Handler Middleware Pipeline
**Description:** Replace flat 60+ handler registry with layered middleware pipeline. Cross-cutting concerns (auth, logging, metrics, rate-limiting, validation) as composable Layers that auto-wrap every handler.
**Rationale:** Highest compound leverage. Every new handler -30-50% code. Every cross-cutting improvement applies globally with single registration.
**Downsides:** Major refactor of handler dispatch path.
**Confidence:** 85%
**Complexity:** High
**Status:** Unexplored

### 5. Unified Typed Channel Event Bus
**Description:** Replace `mpsc::Sender<InboundMessage>` with `ChannelEvent` enum: `MessageReceived`, `MessageEdited`, `MessageDeleted`, `ReactionAdded`, `UserTyping`, `PresenceChanged`, `CallbackQueryReceived`. Consumers subscribe by event type.
**Rationale:** Prerequisite for all future channel enhancements. Currently channels can only emit "new message" — Discord/Slack/Telegram rich events are invisible to core.
**Downsides:** Changes fundamental event flow, touches all channel implementations.
**Confidence:** 80%
**Complexity:** High
**Status:** Unexplored

### 6. Causal Trace Context Propagation
**Description:** Lightweight `TraceContext` (trace_id + parent_span_id + causation chain) flowing through handlers, events, tool calls, LLM invocations. Stored in SQLite. Propagated via tokio task-local.
**Rationale:** Active trace file development in working tree needs this. Without causal context, trace views require heuristic event ordering reconstruction.
**Downsides:** Context propagation overhead (minimal with task-local).
**Confidence:** 85%
**Complexity:** Medium
**Status:** Unexplored

### 7. Self-Routing Gateway as LLM Tool
**Description:** Gateway routing, channel config, and delivery policies exposed as LLM-callable tools. LLM can: route responses differently per channel, throttle delivery, fork sessions to different devices.
**Rationale:** Ultimate expression of R9 (Everything is a Tool). No competitor does this. Gateway is the last infrastructure piece outside LLM control.
**Downsides:** Security boundaries need careful design. Which configs are mutable needs definition.
**Confidence:** 75%
**Complexity:** Medium-High
**Status:** Unexplored

---

*The following ideas were added in Round 2 (deep OpenClaw comparison), complementing Round 1 survivors:*

### 8. Two-Phase Tool Exec Approval with LLM-Routed Escalation
**Description:** Implement request→broadcast→resolve approval flow for dangerous tool executions. Key differentiator from OpenClaw: the LLM (not hardcoded rules) judges what needs approval via system prompt escalation criteria. Gateway holds execution in pending state, broadcasts to originating channel (or designated approval channel), resolves on allow-once/allow-always/deny within configurable timeout.
**Rationale:** Perfect alignment with R8 (LLM Sovereignty) and R10 (Intelligence Lives in Prompt). OpenClaw hardcodes approval rules; Aleph lets the LLM reason about risk. Existing `exec_approvals.rs` (498 lines) provides foundation but lacks UI integration and LLM-driven escalation.
**Downsides:** Requires prompt engineering for escalation criteria and fallback for LLM misjudgment.
**Confidence:** 88%
**Complexity:** Medium
**Status:** Unexplored

### 9. Compile-Time RPC Schema Registry with Blanket Dispatch
**Description:** Each RPC method defined as unit struct implementing sealed `RpcMethod` trait with associated types Params/Result (both derive `schemars::JsonSchema`). Auto-registration via `inventory`/`linkme` crate. Dispatcher auto-deserializes, routes, serializes — eliminates hand-written match arms in `handlers/mod.rs` (~900 lines). Adding a new RPC = one struct + impl.
**Rationale:** Solves concrete maintainability bottleneck. Schema and code can never diverge (compile-time guarantee). Every new handler auto-registers without touching the dispatcher. OpenClaw's 74+ handlers require manual wiring; this pattern makes wiring automatic.
**Downsides:** Major refactor of handler registration mechanism. `inventory`/`linkme` crates add build-time magic.
**Confidence:** 85%
**Complexity:** High
**Status:** Unexplored

### 10. Channel Health Heartbeat with Auto-Recovery Policy
**Description:** Each channel adapter reports periodic heartbeat metrics (latency, error rate, last successful send/receive). Lightweight HealthMonitor applies configurable recovery policies: exponential backoff reconnect, circuit-breaker (open/half-open/closed), graceful degrade to queue-and-retry. Policies declared per-channel in config, not hardcoded.
**Rationale:** Fills real operational blind spot — when Telegram polling stops or Discord websocket drops, current system has zero visibility. OpenClaw solves this with `channel-health-monitor.ts`. ChannelRegistry currently has no health check loop.
**Downsides:** Each channel needs health check interface implementation.
**Confidence:** 82%
**Complexity:** Medium
**Status:** Unexplored

### 11. Typestate-Driven Request Pipeline
**Description:** Replace runtime stage flow with compile-time typestates: `Parsed<R>` → `Authenticated<R>` → `LaneAcquired<R>` → `Executed<R>`. Each stage is zero-sized wrapper carrying proof of previous stage via PhantomData. Compiler rejects code paths that skip authentication or lane acquisition.
**Rationale:** Eliminates an entire class of security bugs (skipped auth paths) at compile time with zero runtime cost. TypeScript fundamentally cannot enforce call-order invariants. Complements Tower Middleware (Round 1 #4): Tower does runtime composition, typestates enforce compile-time invariants within a handler path.
**Downsides:** Requires refactoring request processing type signatures throughout the pipeline.
**Confidence:** 80%
**Complexity:** Medium-High
**Status:** Unexplored

### 12. Channel-Scoped Approval Delegation
**Description:** Allow approval requests to route to a different channel than the originator. Telegram bot interactions can route high-risk approvals to webchat admin panel; CLI dangerous commands can notify via Telegram for mobile approval. Routing configured as a tool (`set_approval_route`) per R9 — user speaks "route all shell approvals to my Telegram".
**Rationale:** Solves real UX problem — approvals must reach the user wherever they are. Aleph's multi-interface architecture (webchat, CLI, Telegram) already has gateway plumbing but no cross-channel approval routing. Complements #8 (Approval Escalation).
**Downsides:** Requires cross-channel message routing mechanism.
**Confidence:** 78%
**Complexity:** Medium
**Status:** Unexplored

### 13. Session Lifecycle State Machine with Event Emission
**Description:** Model each session as formal state machine: Created → Active → Suspended → Resumed → Archived → Tombstoned. Transitions emit typed events through EventEmitter. Suspension captures agent state snapshot for warm restore (not cold restart).
**Rationale:** Structural gap — `HttpSessionManager` has Create/Validate/Revoke but no formal lifecycle states. System cannot distinguish "user closed tab" from "user done forever", forcing conservative resource holding. OpenClaw tracks created/started/stopped/archived as first-class events.
**Downsides:** Needs clear semantic definition for each state and transition triggers.
**Confidence:** 78%
**Complexity:** Medium
**Status:** Unexplored

### 14. Client Send-Queue Pressure Detection and Graceful Shed
**Description:** Monitor each WebSocket connection's outbound buffer depth. Above configurable high-water mark (256KB), drop non-essential delta events for that subscriber (keep control events: stream_end, error). Sustained >5s above mark → close with code 1008 + resume cursor in close payload. Prevents slow consumer (Telegram bridge, backgrounded tab) from causing memory bloat.
**Rationale:** Complements Backpressure Event Bus (Round 1 #3) at per-client delivery layer. OpenClaw checks `bufferedAmount > MAX_BUFFERED_BYTES` and closes slow clients. Aleph writes to SplitSink without inspecting underlying buffer.
**Downsides:** tokio-tungstenite doesn't expose bufferedAmount directly; needs sink wrapper or poll_ready semantics.
**Confidence:** 75%
**Complexity:** Medium
**Status:** Unexplored

### 15. Per-Client Delta Cursor with Dedup Guard
**Description:** Track `last_broadcast_offset` (UTF-8 safe byte offset) per WebSocket client. Before sending delta chunk, compare against cursor to skip already-delivered text. Direct complement to Sequenced Event Stream (Round 1 #1) — sequence numbers handle event-level gaps, delta cursors handle intra-event text overlap on reconnect.
**Rationale:** Without per-client tracking, backfill-on-Lagged re-sends full events but has no mechanism to trim overlap with already-rendered text, causing visible stutter or repeated paragraphs.
**Downsides:** Minimal per-client memory overhead.
**Confidence:** 75%
**Complexity:** Low-Medium
**Status:** Unexplored

## Rejection Summary

### Round 1

| # | Idea | Reason Rejected |
|---|------|-----------------|
| 1 | Compile-Time Channel Fusion | Too extreme, prevents dynamic behavior |
| 2 | Content-Addressed Sessions (Merkle) | Massive overhaul, unclear benefit for personal assistant |
| 3 | Session Linear Resource Tokens | Too academic, no concrete use case |
| 4 | Binary Protocol (Cap'n Proto) | Premature optimization, breaks debuggability |
| 5 | Edge WASM Gateway | Self-hosted scenario, no CDN edge use case |
| 6 | Subscription GC on Disconnect | Trivial bug fix, not ideation-level |
| 7 | Health Endpoint Enhancement | Routine engineering |
| 8 | Agent Capability Typestate | No real problem driving it yet |
| 9 | Event Scope Watermark | Covered by stronger sequenced event stream idea |
| 10 | Compile-Time Capability Marker Traits | Would overcomplicate trait hierarchy |
| 11 | Pull-Based Channels | Overkill message broker for self-hosted |
| 12 | Speculative Execution | Tool purity categorization too hard to implement safely |
| 13 | Bidirectional Context Streaming | Beyond gateway improvement scope |
| 14 | Declarative Formatter + Outbound Adaptation | Too incremental for ideation |
| 15 | Channel Capability Negotiation | Covered by protocol handshake idea |
| 16 | Queryable Session Schema | Incremental improvement |
| 17 | Channel Test Harness | Routine test infrastructure |
| 18 | Graceful Shutdown | Routine engineering |
| 19 | Preauth Budget | Important but small scope |
| 20 | Rate Limiter + Queue Depth | Good but not transformative |
| 21 | LLM as Rate Limiter | Implementation path unclear |
| 22 | WASM Channel Plugins | 12+ compiled channels sufficient for now |
| 23 | Session Inheritance | CLAUDE.md already mentions Memory Prompt |
| 24 | Zero-Copy Streaming (Pinned Buffer) | Premature optimization |
| 25 | Compile-Time Handler Dispatch Macro | Overlaps with more flexible middleware pipeline |

### Round 2

| # | Idea | Reason Rejected |
|---|------|-----------------|
| 1 | Throttled Delta Coalescing | StreamingDeltaSink + MessageCoalescer already implement this |
| 2 | Final Flush Barrier Before Stream-End | StreamingDeltaSink::Done already flushes explicitly |
| 3 | Differential Compression | Single-user local network, compression gains negligible |
| 4 | Output-Mode Rendering Hints | OutputMode enum + ChannelCapabilities already exist |
| 5 | Speculative Preflight Delta | First-token latency dominated by LLM, not gateway |
| 6 | Zero-Copy Ring Buffer (broadcast) | 1-5 clients, premature optimization (P6 YAGNI) |
| 7 | Session DAG Fork-on-Diverge | No user-facing workflow requires branching today (P6 YAGNI) |
| 8 | Device Token Registry | DeviceStore already implements this |
| 9 | Session Projection Snapshots | Overlaps with Sequenced Event Stream (Round 1 #1) |
| 10 | Session Dehydration to Cold Storage | Single-user memory footprint trivial |
| 11 | Optimistic Session Resume with Conflict | Single user, no concurrent write conflicts |
| 12 | Session Inheritance for Tasks | InterAgentPolicy already handles context propagation |
| 13 | Heartbeat Presence with Grace Periods | handlers/heartbeat.rs (792 lines) already implements this |
| 14 | Preauth Connection Budget | LAN-only scenario, not public-facing service |
| 15 | Atomic Allow-Once Tokens | Single-user trust model, over-engineered |
| 16 | Scoped Capability Tokens | Device permissions already support wildcards |
| 17 | LLM-Narrated Security Events | Monitoring UX, not architecture improvement |
| 18 | Approval Policy as Prompt Fragment | Overlaps with #8 (LLM-Routed Escalation) |
| 19 | Mutual TLS for Inter-Process | YAGNI, single-machine deployment |
| 20 | Zero-Copy Arc\<Bytes\> Fan-Out | 1-5 clients, immeasurable benefit |
| 21 | Compile-Time Topic Routing (phf) | enum match already O(1), 14 variants don't need hash tables |
| 22 | Affine-Type Session Tokens | Use-once semantics contradict how sessions work |
| 23 | Lock-Free Credit Counters | Overlaps with Backpressure Event Bus (Round 1 #3) |
| 24 | Sealed Capability Algebra | ChannelCapabilities struct already sufficient |
| 25 | Structured Concurrency TaskSet | Overlaps with Causal Trace Context (Round 1 #6) |
| 26 | Channel Capability Negotiation | ChannelCapabilities already declared at registration |
| 27 | Webhook MCP Server | Webhook system already exists (~1500 lines) |
| 28 | Channel Account Snapshots | No concrete missing state identified |
| 29 | WASM Channel Plugins | Massive complexity, violates R3 (Core Minimalism) |
| 30 | Channel Lifecycle Event Hooks | Need covered by #10 (Channel Health) |
| 31 | Per-Channel Agent Override | Existing routing_rules already support this |

## Session Log
- 2026-04-04: Initial ideation — 48 candidates generated (6 agents × ~8 ideas), 32 unique after dedupe, 7 survived filtering
- 2026-04-04: Selected #1 (Sequenced Event Stream) for brainstorm
- 2026-04-04: Round 2 deep dive — 39 new candidates from 5 agents (streaming, session, security, Rust-native, channel), 8 survived adversarial filtering. Total: 15 ranked ideas across 2 rounds
