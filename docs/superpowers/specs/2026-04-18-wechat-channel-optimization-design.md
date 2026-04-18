# WeChat Channel Optimization Design

**Date:** 2026-04-18
**Status:** Draft (pending review)
**Scope:** Fix critical defects and achieve feature parity with hermes-agent WeChat channel
**Related:** `docs/superpowers/specs/2026-04-16-wechat-channel-design.md` (original design)

---

## 1. Problem Statement

The current Aleph WeChat channel implementation has **critical functional gaps** that make it unusable in production. While the module structure exists, several core features are either broken (AES decryption) or entirely missing (message deduplication, context token management, media handling).

### 1.1 Critical Defects (Broken Features)

| # | Component | Defect | Impact |
|---|-----------|--------|--------|
| 1 | `media.rs` | `aes128_ecb_decrypt()` is a no-op stub — returns ciphertext as-is | All media downloads (images, voice, files, video) are corrupted |
| 2 | `runtime.rs` | `WeChatChannel.start()` does not call `runtime.start()` | Poll loop never starts — **no inbound messages are received** |
| 3 | `runtime.rs` | Fixed 100ms sleep, no error backoff strategy | CPU waste + API pressure on errors |
| 4 | `runtime.rs` | Session expired error (`-14`) not handled | Infinite retry loop after session expiry |
| 5 | `runtime.rs` | No message deduplication | Duplicate message processing, duplicate replies |

### 1.2 High-Priority Missing Features

| # | Feature | hermes-agent | Aleph Current |
|---|---------|-------------|---------------|
| 6 | Media inbound mapping | Full (image/video/voice/file download + cache) | Text only |
| 7 | Context token extraction/storage | Auto-extract + disk persistence | Not implemented |
| 8 | Message chunking/splitting | Configurable (compact vs per-line) | No splitting |
| 9 | Markdown conversion | Full conversion (headers, tables, links) | Strips markdown |
| 10 | Typing indicator | `send_typing()` with ticket cache | Empty implementation |
| 11 | QR login | Complete interactive flow | Returns error |
| 12 | `max_message_length` | 4000 (correct) | 2000 (incorrect) |

### 1.3 Medium-Priority Issues

| # | Issue | Current | Desired |
|---|-------|---------|---------|
| 13 | Error type | `String` everywhere | Structured `thiserror` enum |
| 14 | HTTP client reuse | New `Client` per `get_updates()` | Shared `Client` with timeout |
| 15 | `sync_buf.rs` persistence path | Hardcoded empty string `hermes_home` | Use actual Aleph data dir |
| 16 | `random_wechat_uin()` | Uses `unwrap()` | Proper error handling |

---

## 2. Goals

### 2.1 Primary Goal
Achieve **functional parity** with hermes-agent's WeChat channel (`gateway/platforms/weixin.py`), adapted to Aleph's Rust architecture and Channel trait abstraction.

### 2.2 Success Criteria

- [ ] All media types (image, voice, video, file) can be received and decrypted
- [ ] Inbound messages are deduplicated (no duplicate processing)
- [ ] Context tokens are extracted, stored, and persisted across restarts
- [ ] Outbound messages > 4000 chars are split correctly
- [ ] Markdown is converted to WeChat-friendly format (not stripped)
- [ ] Session expiry is handled gracefully (10-min pause, not infinite retry)
- [ ] Error backoff: 3 consecutive failures trigger 30s backoff
- [ ] `send_typing()` works with typing ticket caching
- [ ] `max_message_length` corrected to 4000
- [ ] All existing unit tests pass; new tests added for fixed features
- [ ] `cargo clippy -p alephcore -- -D warnings` passes

---

## 3. Architecture

### 3.1 Current Architecture (Broken)

```text
┌─────────────────────────────────────────────┐
│           WeChatChannel                     │
│  start() → creates runtime (NOT started)    │
│  send()  → runtime.send_message()           │
└─────────────────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────┐
│         WeChatRuntime                       │
│  - NOT started by Channel                   │
│  - 100ms fixed sleep                        │
│  - No error backoff                         │
│  - No deduplication                         │
└─────────────────────────────────────────────┘
```

### 3.2 Target Architecture

```text
┌─────────────────────────────────────────────────────────────┐
│                    ChannelRegistry                            │
└─────────────────────┬───────────────────────────────────────┘
                      │ create_channel("wechat")
                      ▼
┌─────────────────────────────────────────────────────────────┐
│                 WeChatChannel                                │
│  ┌─────────────────────────────────────────────────────┐   │
│  │              WeChatRuntime                          │   │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────────────┐  │   │
│  │  │ ILinkApi │  │PollingMgr│  │ MediaProcessor   │  │   │
│  │  │(HTTP)    │  │(backoff) │  │(AES-128 decrypt) │  │   │
│  │  └──────────┘  └──────────┘  └──────────────────┘  │   │
│  └─────────────────────────────────────────────────────┘   │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐     │
│  │ContextTokenStore│  │SyncBufManager│  │TypingTicketCache│  │
│  │(persistent)   │  │(incremental) │  │(600s TTL)    │     │
│  └──────────────┘  └──────────────┘  └──────────────┘     │
│  ┌──────────────┐  ┌──────────────┐                        │
│  │DedupCache    │  │MarkdownConverter│                     │
│  │(300s TTL)    │  │(full conversion)│                     │
│  └──────────────┘  └──────────────┘                        │
└─────────────────────────────────────────────────────────────┘
```

### 3.3 Message Flow

```text
Inbound:
  iLink API → get_updates() → PollingManager
                                    │
                                    ▼
                            MessageDeduplicator
                                    │
                                    ▼
                            should_accept_message()
                                    │
                                    ▼
                            map_message_to_inbound()
                            (extract text + media)
                                    │
                                    ▼
                            ContextTokenStore::set()
                            TypingTicketCache::warm()
                                    │
                                    ▼
                            InboundMessage → EventBus

Outbound:
  OutboundMessage → markdown_to_wechat()
                         │
                         ▼
                  split_text_for_delivery()
                         │
                         ▼
                  ContextTokenStore::get()
                         │
                         ▼
                  send_message() → iLink API
```

---

## 4. Module Changes

### 4.1 `mod.rs` — WeChatChannel

**Changes:**
- Fix `start()` to call `runtime.start(sender).await`
- Fix `max_message_length` from 2000 to 4000
- Implement `send_typing()` using runtime
- Integrate context_token lookup in `send()`

**Before:**
```rust
pub async fn start(&mut self) -> ChannelResult<()> {
    // ...
    let runtime = Arc::new(runtime::WeChatRuntime::new(
        self.config.clone(),
        auth::ContextTokenStore::new(""),
    ));
    self.runtime = Some(runtime.clone());
    // MISSING: runtime.start() call
    // ...
}
```

**After:**
```rust
pub async fn start(&mut self) -> ChannelResult<()> {
    // ...
    let token_store = auth::ContextTokenStore::new(&self.config.data_dir);
    token_store.restore(&self.config.account_id).await;
    
    let runtime = Arc::new(runtime::WeChatRuntime::new(
        self.config.clone(),
        token_store,
    ));
    
    let sender = self.channel_state.inbound_sender();
    runtime.start(sender).await;
    self.runtime = Some(runtime.clone());
    // ...
}
```

### 4.2 `runtime.rs` — Polling Loop

**Changes:**
- Add `MessageDeduplicator` (300s TTL)
- Add consecutive failure counter with backoff
- Handle session expiry (`-14`) with 10-min pause
- Support dynamic timeout from server response
- Fix fixed 100ms sleep to use adaptive delay
- Add media download integration

**New constants:**
```rust
const MAX_CONSECUTIVE_FAILURES: u32 = 3;
const RETRY_DELAY_SECONDS: u64 = 2;
const BACKOFF_DELAY_SECONDS: u64 = 30;
const SESSION_EXPIRED_ERRCODE: i32 = -14;
const MESSAGE_DEDUP_TTL_SECONDS: u64 = 300;
```

**New backoff logic:**
```rust
match self.api.get_updates(...).await {
    Ok(resp) => {
        consecutive_failures = 0;
        // ... process messages
    }
    Err(e) => {
        consecutive_failures += 1;
        let delay = if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
            BACKOFF_DELAY_SECONDS
        } else {
            RETRY_DELAY_SECONDS
        };
        sleep(Duration::from_secs(delay)).await;
        if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
            consecutive_failures = 0;
        }
    }
}
```

### 4.3 `media.rs` — AES Decryption

**Changes:**
- Implement actual AES-128-ECB decryption using `aes` crate
- Remove no-op stub

**Implementation:**
```rust
use aes::cipher::{BlockDecrypt, KeyInit};
use aes::Aes128;
use cipher::block_padding::Pkcs7;
use cipher::BlockSizeUser;

type Aes128Ecb = ecb::Decryptor<Aes128>;

pub fn aes128_ecb_decrypt(ciphertext: &[u8], key: &[u8]) -> Result<Vec<u8>, String> {
    if ciphertext.is_empty() {
        return Ok(Vec::new());
    }
    
    let key_arr: [u8; 16] = key.try_into()
        .map_err(|_| "AES key must be 16 bytes")?;
    
    let mut buf = ciphertext.to_vec();
    let decryptor = Aes128Ecb::new_from_slice(&key_arr)
        .map_err(|e| format!("Failed to create decryptor: {}", e))?;
    
    let plaintext = decryptor.decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|e| format!("Decryption failed: {}", e))?;
    
    Ok(plaintext.to_vec())
}
```

### 4.4 `inbound/mapper.rs` — Media Handling

**Changes:**
- Add media extraction from `MessageItem::Image/Voice/Video/File`
- Download media via `media::download_and_decrypt_media()`
- Populate `InboundMessage.attachments` with MIME types
- Extract context_token from message

**New function:**
```rust
async fn extract_media(
    items: &[MessageItem],
    config: &WeChatConfig,
) -> (Vec<String>, Vec<String>) {
    // Download each media item, return (paths, mime_types)
}
```

### 4.5 `auth.rs` — Context Token Store

**Changes:**
- Fix `restore()` to use actual data directory (not hardcoded `hermes_home`)
- Integrate with Aleph's data directory path

**New helper:**
```rust
fn resolve_data_dir() -> PathBuf {
    // Use ~/.aleph/ or ALEPH_DATA_DIR env var
}
```

### 4.6 `outbound/markdown.rs` — Full Conversion

**Changes:**
- Rewrite to match hermes-agent's conversion rules:
  - `# Title` → `【Title】`
  - `## Title` → `**Title**`
  - `[text](url)` → `text (url)`
  - Tables → list format
  - Code blocks preserved
- Remove simple stripping logic

### 4.7 `outbound/mapper.rs` — Message Splitting

**Changes:**
- Add `split_text_for_delivery()` with compact and per-line modes
- Integrate with `markdown.rs` conversion
- Support chunk delay and retry config

### 4.8 `api.rs` — Error Types

**Changes:**
- Introduce `WeChatApiError` enum instead of `String`
- Share `reqwest::Client` instead of creating new one per call

```rust
#[derive(Debug, thiserror::Error)]
pub enum WeChatApiError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    
    #[error("JSON parse error: {0}")]
    Parse(#[from] serde_json::Error),
    
    #[error("API error: ret={ret}, errcode={errcode:?}, errmsg={errmsg:?}")]
    Api { ret: i32, errcode: Option<i32>, errmsg: Option<String> },
    
    #[error("Session expired")]
    SessionExpired,
}
```

---

## 5. Data Types

### 5.1 New Types

```rust
// runtime.rs
pub struct MessageDeduplicator {
    cache: RwLock<HashMap<String, Instant>>,
    ttl: Duration,
}

// api.rs  
pub enum WeChatApiError { ... }

// outbound/markdown.rs
pub struct MarkdownConverter;
```

### 5.2 Modified Types

```rust
// config.rs — add data_dir
pub struct WeChatConfig {
    // ... existing fields ...
    #[serde(default = "default_data_dir")]
    pub data_dir: String,
}

// types.rs — add Serialize to MessageItem
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(into = "MessageItemHelper")]
pub enum MessageItem { ... }
```

---

## 6. Error Handling

### 6.1 Retryable vs Fatal Errors

| Error | Retryable | Action |
|-------|-----------|--------|
| Network timeout | Yes | Exponential backoff |
| HTTP 5xx | Yes | Retry with backoff |
| Session expired (-14) | Yes (after 10min) | Pause 10min, reset counter |
| HTTP 4xx | No | Log error, skip |
| Invalid token | No | Set fatal error |

### 6.2 Backoff Strategy

```
Failure 1: wait 2s
Failure 2: wait 2s  
Failure 3: wait 30s (backoff)
Failure 4: wait 2s (counter reset)
...
```

---

## 7. Testing Strategy

### 7.1 Unit Tests (per module)

| Module | Tests |
|--------|-------|
| `media.rs` | AES decrypt with known vectors, PKCS7 padding edge cases |
| `runtime.rs` | Mock ILinkApi, test backoff logic, dedup TTL expiry |
| `inbound/mapper.rs` | Media extraction, chat type guessing, text concatenation |
| `outbound/markdown.rs` | Header conversion, table conversion, link rewriting |
| `outbound/mapper.rs` | Message splitting at 4000 char boundary |
| `auth.rs` | Token store persistence across instances |

### 7.2 Integration Tests

- Mock iLink API server (using `wiremock` or `mockito`)
- Test full inbound → outbound roundtrip
- Test session expiry recovery

---

## 8. Migration Plan

### Phase 1: Critical Fixes (Day 1-2)
1. Fix `mod.rs` to start runtime
2. Implement AES decryption in `media.rs`
3. Add error backoff to `runtime.rs`
4. Fix `max_message_length` to 4000

### Phase 2: Feature Completion (Day 3-5)
5. Add message deduplication
6. Implement media inbound mapping
7. Add context token extraction/storage
8. Rewrite markdown conversion
9. Implement message splitting

### Phase 3: Polish (Day 6-7)
10. Implement `send_typing()` with ticket cache
11. Implement QR login flow (interactive CLI)
12. Add structured error types
13. Write integration tests
14. Run clippy, fix warnings

---

## 9. Dependencies

### New crates needed:
```toml
aes = "0.8"
cipher = "0.4"
ecb = "0.1"  # for AES-ECB mode
```

### Existing crates to use:
- `reqwest` (already in project)
- `tokio` (already in project)
- `serde` (already in project)
- `thiserror` (already in project)

---

## 10. Reference

- hermes-agent Weixin: `/Volumes/TBU4/Github/hermes-agent/gateway/platforms/weixin.py`
- Aleph Channel trait: `/Volumes/TBU4/Workspace/Aleph/src/gateway/channel.rs`
- Original design: `/Volumes/TBU4/Workspace/Aleph/docs/superpowers/specs/2026-04-16-wechat-channel-design.md`
