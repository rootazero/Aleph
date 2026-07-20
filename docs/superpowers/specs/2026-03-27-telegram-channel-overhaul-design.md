# Telegram Channel Overhaul — Design Spec

**Date**: 2026-03-27
**Status**: Approved
**Scope**: `src/gateway/interfaces/telegram/`

## Summary

Overhaul Aleph's Telegram channel implementation by (1) splitting the 1355-line `mod.rs` into 6 focused modules, then (2) upgrading 4 high-value dimensions: message chunking with HTML safety, access control policy system, network resilience with error classification, and real-time streaming. Learned from OpenClaw's production-grade Telegram plugin but adapted to Aleph's Rust architecture and design principles.

## Motivation

Comparison with OpenClaw revealed significant gaps:

| Dimension | Aleph (current) | OpenClaw | Gap severity |
|-----------|-----------------|----------|-------------|
| Streaming | Fake typewriter (edit after complete) | Real-time token streaming with draft API | High |
| Access control | Static allowlist + basic pairing | DM policy state machine + group policy + per-topic override | High |
| Network resilience | Basic watchdog + string-based error matching | Pre/post connect classification + precise 429 handling | Medium |
| Message chunking | Simple 3500-char split | HTML tag balancing + newline-first strategy | Medium |

## Approach: Incremental Refactor (B+C Hybrid)

Split first, upgrade on new structure. Each dimension independently testable and deployable.

## Section 1: Module Split

### Current structure

```
telegram/
├── mod.rs          (1355 lines — everything)
├── config.rs       (255 lines)
├── message_ops.rs  (276 lines)
└── group_chat.rs
```

### Target structure

```
telegram/
├── mod.rs          (~120 lines) — TelegramChannel struct + Channel trait impl delegation
├── config.rs       (extended)   — add DmPolicy/GroupPolicy enums
├── message_ops.rs  (unchanged)
├── group_chat.rs   (unchanged)
├── handlers.rs     (~200 lines) — message_handler + callback_handler closures
├── polling.rs      (~180 lines) — polling lifecycle + watchdog + auto-restart
├── access.rs       (~250 lines) — DM policy state machine + group policy + pairing
├── delivery.rs     (~300 lines) — HTML-safe chunking + retry loop + attachment send
└── streaming.rs    (~200 lines) — real-time streaming edit + typing control
```

### Code migration map

| Current location (lines) | Target module | Content |
|---|---|---|
| 145-358 | `handlers.rs` | `convert_message()` + `extract_attachments()` |
| 366-394 | `delivery.rs` | `ErrorClass` + `classify_error()` |
| 442-840 | `polling.rs` + `handlers.rs` | `start()` polling loop + handler closures |
| 856-1049 | `delivery.rs` | `send()` chunking + retry logic |
| 1051-1105 | `delivery.rs` | `send_typing()` + `react()` |
| 1120-1280 | `delivery.rs` | `send_attachment()` + `edit_message()` |

### Principles

- `mod.rs` is pure assembly: struct definition, `Channel` trait impl with one-line delegation to submodules
- Submodules expose `pub(crate)` interfaces, no cross-module calls (all coordination via mod.rs)
- Existing `config.rs` / `message_ops.rs` / `group_chat.rs` untouched — zero migration risk
- `ErrorClass` + `classify_error` move to `delivery.rs` (only used by send logic)

## Section 2: Access Control Upgrade

### New config types (config.rs)

```rust
/// DM access policy
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DmPolicy {
    /// Reject all DMs
    Disabled,
    /// Require pairing code (default)
    #[default]
    Pairing,
    /// Explicit allowlist only (no pairing)
    Allowlist,
    /// Allow all (dangerous, requires explicit config)
    Open,
}

/// Group access policy
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GroupPolicy {
    /// Reject all groups
    Disabled,
    /// Only allowlisted groups (default)
    #[default]
    Allowlist,
    /// Allow all groups (require @mention)
    Open,
}
```

### AccessController (access.rs)

```rust
pub enum AccessDecision {
    Allowed,
    NeedsPairing,
    Denied,
}

pub struct AccessController {
    config: TelegramConfig,
    runtime_users: Arc<RwLock<Vec<i64>>>,
    pairing_codes: Arc<RwLock<HashMap<String, PairingEntry>>>,
    prompt_times: Arc<RwLock<HashMap<i64, Instant>>>,
}

impl AccessController {
    pub async fn check_message(&self, user_id: i64, chat_id: i64, is_group: bool) -> AccessDecision;
    pub async fn try_pair(&self, user_id: i64, code: &str) -> PairingResult;
    pub async fn generate_code(&self) -> String;
    pub async fn list_codes(&self) -> Vec<(String, u64)>;
}
```

### Backward compatibility

- Old config `allowed_users: [123]` auto-maps to `dm_policy: allowlist` + `allowed_users: [123]`
- `allowed_users: []` (empty) maps to `dm_policy: pairing` (safe default — avoids accidental open access)
- New `dm_policy` field takes precedence when explicitly set
- To get open access, user must explicitly set `dm_policy: open`

### Not doing (YAGNI)

- Per-topic override — no multi-agent routing in Aleph yet
- Execution approvals — handled at core layer, not channel
- Multi-account — single bot sufficient

## Section 3: Message Chunking with HTML Safety

### Smart chunking algorithm (delivery.rs)

```rust
/// Split HTML text respecting tag integrity
pub fn split_html_safe(html: &str, max_len: usize) -> Vec<String>;

/// Track unclosed HTML tags (stack-based parser)
/// Only handles Telegram-supported tags: <b>, <i>, <s>, <code>, <pre>, <blockquote>
fn balance_html_tags(chunk: &str) -> (Vec<&str>, Vec<&str>);
```

### Split priority (highest to lowest)

1. Double newline `\n\n` (paragraph boundary)
2. Single newline `\n` (line boundary)
3. Space ` ` (word boundary)
4. Hard cut at char boundary (last resort, using `char_indices()` for UTF-8 safety)

### HTML tag balancing

After each split point:
- Append closing tags to current chunk tail
- Prepend opening tags to next chunk head

### Changes from current code

- **Replace**: `MessageFormatter::split()` call in Telegram send → `format()` to HTML first, then `split_html_safe()`
- **Keep**: `MessageFormatter::split()` itself (other channels may use it)
- **Current SPLIT_LIMIT=3500**: increase to 4000 (more room now that HTML integrity is preserved)

### Not doing (YAGNI)

- Media group buffering — single message processing sufficient
- Caption splitting — 1024 char caption limit rarely hit

## Section 4: Network Resilience

### Enhanced error classification (delivery.rs)

```rust
#[derive(Debug)]
pub enum ErrorClass {
    /// DNS/TCP failure — safe to retry, data never sent
    PreConnect,
    /// Timeout/reset — may have been sent, retry cautiously
    PostConnect,
    /// Telegram API rejection — don't retry, fallback to plain text
    Rejected(String),
    /// 429 rate limit — wait exact seconds then retry
    RateLimited(u64),
}

fn classify_error(err: &teloxide::RequestError) -> ErrorClass {
    match err {
        teloxide::RequestError::Api(api_err) => {
            // Use teloxide's ApiError enum for precise matching
            // ApiError::RetryAfter → exact seconds (not hardcoded 30s)
            // BotBlocked, ChatNotFound, etc. → Rejected
        }
        teloxide::RequestError::Network(reqwest_err) => {
            // reqwest's is_connect()/is_dns() → PreConnect
            // else → PostConnect
        }
        _ => ErrorClass::PostConnect,
    }
}
```

### Retry strategy matrix

| ErrorClass | Message send | Polling |
|---|---|---|
| PreConnect | Retry immediately (backoff 500ms×attempt) | Don't restart, wait next tick |
| PostConnect | Retry max 2 times (may already be delivered) | 3 consecutive → transport rebuild |
| Rejected | No retry, fallback plain text | 401 → stop polling |
| RateLimited(n) | sleep(n) then retry | sleep(n) then continue |

### Polling resilience (polling.rs)

New `PollingState` struct:
- `last_update_at: Instant` — passive stall detection (90s no updates → mark dirty)
- `consecutive_empty: u32` — track empty poll responses
- Health check interval and max failures unchanged (120s, 3)

### Rust advantages over OpenClaw

- `teloxide::ApiError` enum → pattern matching instead of string comparison
- `reqwest::Error::is_connect()` / `is_dns()` → type-safe pre/post classification
- No runtime cost for error classification (zero-allocation enum matching)

### Not doing (YAGNI)

- Webhook HMAC upgrade — webhook mode not in active use
- Proxy trust chain — self-hosted scenario
- apiThrottler adaptive rate limiting — teloxide handles this

## Section 5: Real-Time Streaming

### Core architecture change

**Current** (fake typewriter):
```
Tokens arrive → buffer accumulate → RunComplete → send_message → edit loop (+80 chars/300ms)
```

**New** (real-time):
```
Tokens arrive → push_chunk() → first threshold hit → send_message
             → more tokens → debounce timer → edit_message(full text)
             → ... repeat → RunComplete → final edit(complete text)
```

### StreamingController (streaming.rs) — pure logic, no IO

```rust
pub struct StreamingController {
    buffer: String,
    sent_message_id: Option<MessageId>,
    last_edit_at: Instant,
    last_edit_len: usize,
    config: StreamingConfig,
}

pub struct StreamingConfig {
    pub min_initial_chars: usize,    // default 30
    pub debounce_interval: Duration, // default 300ms
    pub enabled: bool,
}

pub enum StreamAction {
    Wait,
    SendInitial(String),   // First send
    Edit(String),           // Intermediate edit
    SendFinal(String),      // One-shot send (buffer never hit threshold)
    EditFinal(String),      // Final edit (ensure complete text)
    Done,                   // No action needed
}

impl StreamingController {
    pub fn push_chunk(&mut self, text: &str);
    pub fn poll_action(&mut self) -> StreamAction;
    pub fn finalize(&mut self) -> StreamAction;
}
```

### Integration with ReplyEmitter

Minimal change — replace buffer + typewriter loop with StreamingController:

- `on_event(TextDelta)`: `controller.push_chunk()` + `poll_action()` → dispatch SendInitial/Edit
- `on_event(RunComplete)`: `controller.finalize()` → dispatch SendFinal/EditFinal, then send attachments

### Key design decisions

1. **StreamingController is pure logic (no IO)** — buffer + timing only, no bot/channel reference. 100% unit testable
2. **Debounce 300ms** — Telegram's implicit `editMessageText` rate limit ~20/min, 300ms is safe and smooth
3. **min_initial_chars = 30** — avoid push notification showing "I" or "Let me"
4. **Fallback**: if channel doesn't support editing → auto-degrade to instant mode (SendFinal only)
5. **Delete old code**: `TYPEWRITER_CHARS_PER_STEP` constant and typewriter edit loop removed entirely

### Rust advantages

- `StreamAction` enum + exhaustive `match` — impossible to miss a state
- `StreamingController` has no async, no Arc — pure CPU logic, zero runtime overhead
- Single Mutex acquisition at `on_event` entry, synchronous operations within

## Implementation Order

1. **Module split** — pure refactor, zero behavior change, `cargo test` validates
2. **Message chunking** — modify `delivery.rs`, lowest risk, immediate quality improvement
3. **Access control** — new `access.rs`, strong independence, backward-compatible config
4. **Network resilience** — modify `polling.rs` + `delivery.rs` retry logic
5. **Real-time streaming** — modify `streaming.rs` + `ReplyEmitter`, highest impact, last

## Code to delete after completion

- `TYPEWRITER_CHARS_PER_STEP` constant in `reply_emitter.rs`
- Typewriter edit loop in `ReplyEmitter::on_event(RunComplete)`
- Old `ErrorClass` and `classify_error` with string matching (replaced by enum-based version)
- Scattered access control logic in `start()` message_handler closure (replaced by `AccessController`)

## Files affected

| File | Change type |
|------|------------|
| `telegram/mod.rs` | Shrink from 1355 → ~120 lines (delegation only) |
| `telegram/config.rs` | Extend with DmPolicy, GroupPolicy |
| `telegram/handlers.rs` | **New** — message/callback handlers extracted |
| `telegram/polling.rs` | **New** — polling lifecycle + enhanced watchdog |
| `telegram/access.rs` | **New** — AccessController + policy logic |
| `telegram/delivery.rs` | **New** — HTML-safe chunking + retry + attachments |
| `telegram/streaming.rs` | **New** — StreamingController |
| `gateway/reply_emitter.rs` | Integrate StreamingController, remove typewriter loop |
