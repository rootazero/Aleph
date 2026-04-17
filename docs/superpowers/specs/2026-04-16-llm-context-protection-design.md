# Aleph LLM Context Protection — Phase 2 Design

> Date: 2026-04-16  
> Scope: Phase 2 of Runtime Security roadmap. Builds on Phase 1 (`RuntimeSecurityGuard`) to protect LLM context from session identifier leakage, standardize media placeholders, and add platform-aware PII policies.

---

## 1. Goal

Ensure **LLM never sees raw session identifiers** (session keys, sender IDs, channel IDs), **media is represented via structured placeholders** when not sent as native blocks, and **PII policies can vary per messaging platform** (Telegram vs Discord vs CLI, etc.).

---

## 2. Scope

### In Scope
- `ContextIdHasher` — deterministic short-hash for session identifiers in prompts.
- `InboundContext::format_for_prompt()` integration with hasher.
- `MediaPlaceholder` + `MediaRegistry` — structured `{{media:TYPE:ID}}` placeholders.
- `MediaProcessor` updates to emit unified placeholders for non-native media.
- `PrivacyConfig` extension for `platform_policies`.
- `PiiEngine` extension for platform-aware filtering.
- `RuntimeSecurityGuard` integration of platform/context protection.

### Out of Scope
- Encryption of session identifiers at rest (already handled by state layer).
- New vision models or audio codecs.
- UI changes for privacy settings.

---

## 3. Architecture

### 3.1 Session Identifier Protection

**Problem**: `InboundContext::format_for_prompt()` currently emits raw strings like:
```
Session: tg:dm:123
Sender: u123
Channel: telegram | group_chat
```
These expose internal routing keys and user IDs to the LLM.

**Solution**: `ContextIdHasher`
- Deterministic SHA-256 truncated to 8 hex chars.
- Produces `ctx:a1b2c3d4` style tokens.
- Same input → same hash (allows LLM to track continuity across turns without revealing real IDs).
- Applied to:
  - `session.session_key`
  - `sender.id` (when no `display_name` is present, or optionally always)
  - `reply_to` message IDs

**Mount Point**: `InboundContext::format_for_prompt()` gains an optional `redact_ids: bool` parameter (default `true`).

### 3.2 Media Placeholder Enhancement

**Problem**: Current fallback texts are ad-hoc:
- `[Attachment: name (mime)]`
- `[Image: description unavailable]`
- `[Voice message transcript]: "..."`

These are inconsistent and hard to track in leak detection.

**Solution**: Structured placeholders
- `{{media:image:<short_hash>}}`
- `{{media:audio:<short_hash>}}`
- `{{media:file:<short_hash>}}`

`MediaRegistry` maps `short_hash` → `MediaRecord { original_name, mime_type, description }`.
`MediaProcessor` emits these placeholders instead of free-form text for non-native blocks.
When vision is supported, `ContentBlock::Image` is still emitted natively (no placeholder needed).

### 3.3 Platform-Aware PII Policies

**Problem**: `PrivacyConfig` is global. Telegram phone numbers are sensitive, but Discord usernames are public. A single global policy is too coarse.

**Solution**: Add per-platform overrides.

```rust
pub struct PlatformPiiPolicy {
    pub pii_filtering: Option<bool>,          // override master switch
    pub id_card: Option<PiiAction>,
    pub phone: Option<PiiAction>,
    pub email: Option<PiiAction>,
    pub api_key: Option<PiiAction>,
    pub exclude_providers: Option<Vec<String>>,
}
```

`PrivacyConfig` gains:
```rust
pub platform_policies: HashMap<String, PlatformPiiPolicy>,
```

`PiiEngine` gains:
```rust
pub fn filter_with_platform(&self, text: &str, platform: Option<&str>) -> FilterResult
pub fn is_platform_excluded(&self, platform: Option<&str>, provider: &str) -> bool
```

Resolution order (most specific wins):
1. Platform override (if platform matches and field is `Some`)
2. Global config default

---

## 4. Component Details

### 4.1 `ContextIdHasher`

**File**: `src/security/context_id_hasher.rs`

```rust
pub struct ContextIdHasher;

impl ContextIdHasher {
    pub fn hash(input: &str) -> String {
        let digest = sha2::Sha256::digest(input.as_bytes());
        format!("ctx:{:08x}", u32::from_be_bytes(digest[0..4].try_into().unwrap()))
    }
}
```

### 4.2 `MediaPlaceholder`

**File**: `src/media/placeholder.rs`

```rust
pub enum MediaPlaceholderType {
    Image,
    Audio,
    File,
}

pub struct MediaPlaceholder {
    pub ty: MediaPlaceholderType,
    pub id: String,
}

impl MediaPlaceholder {
    pub fn to_text(&self) -> String {
        format!("{{{{media:{}:{}}}}}", self.ty.as_str(), self.id)
    }
}
```

### 4.3 `PlatformPiiPolicy`

**File**: `src/config/types/privacy.rs` (extend existing)

### 4.4 `PiiEngine` Extensions

**File**: `src/pii/engine.rs` (extend existing)

---

## 5. Data Flow

### Outbound (Agent Loop → LLM)
1. `InboundContext::format_for_prompt()` hashes session IDs via `ContextIdHasher`.
2. `MediaProcessor::process()` emits native `ContentBlock::Image` for vision, or `MediaPlaceholder` text for fallbacks.
3. `RuntimeSecurityGuard::process_outbound()` receives platform name from `SecurityContext`.
4. PII filtering uses `filter_with_platform()` to apply platform-specific rules.
5. Rest of Phase 1 pipeline (leak detection, sanitization, secret injection) proceeds unchanged.

### Inbound (LLM → Agent Loop)
- Inbound leak detection remains unchanged from Phase 1.
- No session identifiers exist in inbound text (they're only in outbound prompts).

---

## 6. File Changes

| File | Action | Purpose |
|------|--------|---------|
| `src/security/context_id_hasher.rs` | Create | SHA-256 short hasher for session IDs |
| `src/security/mod.rs` | Modify | Export `context_id_hasher` module |
| `src/thinker/inbound_context.rs` | Modify | Use `ContextIdHasher` in `format_for_prompt()` |
| `src/media/placeholder.rs` | Create | `MediaPlaceholder`, `MediaRegistry` |
| `src/media/mod.rs` | Modify | Export `placeholder` module |
| `src/media/processor.rs` | Modify | Emit structured placeholders for non-native media |
| `src/config/types/privacy.rs` | Modify | Add `PlatformPiiPolicy` and `platform_policies` field |
| `src/pii/engine.rs` | Modify | Add `filter_with_platform`, `is_platform_excluded` |
| `src/security/runtime_guard.rs` | Modify | Pass `platform_name` to PII engine |
| `tests/security_integration.rs` | Modify | Add Phase 2 integration tests |

---

## 7. Testing Strategy

### Unit Tests
- `context_id_hasher::tests::test_deterministic_hash` — same input → same output.
- `inbound_context::tests::test_prompt_redacts_session_key` — `format_for_prompt` no longer contains raw `tg:dm:123`.
- `media::placeholder::tests::test_roundtrip` — placeholder serialization/deserialization.
- `media::processor::tests::test_unsupported_mime_placeholder` — PDF produces `{{media:file:...}}`.
- `pii::engine::tests::test_platform_override` — Telegram platform overrides phone rule.
- `privacy_config::tests::test_platform_policy_deserialization` — TOML parsing of `platform_policies`.

### Integration Tests
- `security_integration::test_platform_aware_pii_filters_phone_on_telegram` — outbound text with phone number is blocked only when platform policy says so.
- `security_integration::test_session_id_not_in_prompt` — full agent-loop turn shows no raw session key in assembled prompt.

---

## 8. Success Criteria

- `cargo check -p alephcore --lib` passes with zero new warnings.
- `cargo test -p alephcore --lib` passes; all new unit tests pass.
- `cargo test -p alephcore --test security_integration` passes.
- `InboundContext::format_for_prompt()` never emits raw `session_key` when redaction is enabled.
- `MediaProcessor` emits structured `{{media:...}}` placeholders for all non-native media.
- `PiiEngine` correctly resolves platform overrides per the precedence rules.

---

## 9. Roadmap Context

- **Phase 1** ✅: `RuntimeSecurityGuard` orchestrates secret injection, PII, sanitization, leak detection.
- **Phase 2** (this design): LLM Context Protection — hash/redact session identifiers, enhance media placeholders, platform-aware PII.
- **Phase 3** (future): Audit & Leak Detection Hardening — unified security events, tracing spans, comprehensive leak coverage.
