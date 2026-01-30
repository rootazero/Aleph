# Tasks: Block Streaming & Auth Profiles

## 1. Block Streaming - Fence Awareness

### 1.1 Markdown Fence Parsing
- [x] Implement `FenceSpan` struct with start/end/marker/indent/language
- [x] Implement `parse_fence_spans()` line-by-line parser
- [x] Add `is_safe_fence_break()` utility
- [x] Unit tests for fence parsing edge cases

### 1.2 Fence-Aware Chunking
- [x] Add `FenceSplit` struct (close_line, reopen_line)
- [x] Update `BlockReplyChunker` to use fence spans
- [x] Implement fence close/reopen on forced breaks
- [x] Unit tests for fence splitting scenarios

### 1.3 Block Coalescer
- [x] Implement `BlockCoalescer` with idle timeout
- [x] Add `CoalescingConfig` (min_chars, max_chars, idle_ms, joiner)
- [x] Tokio timer integration for idle flush
- [x] Unit tests for coalescing behavior

### 1.4 Configuration
- [ ] Add `StreamingConfig` to gateway config
- [ ] Channel-specific overrides support
- [ ] Default values matching Moltbot (min=800, max=1200, idle=1000ms)

## 2. Auth Profiles - API Key Rotation

### 2.1 Data Model
- [x] Define `AuthProfileCredential` enum (ApiKey, Token, OAuth)
- [x] Define `ProfileUsageStats` struct
- [x] Define `AuthProfileStore` struct
- [x] JSON serialization tests

### 2.2 Cooldown Algorithm
- [x] Implement `calculate_cooldown_ms()` (base-5 exponential)
- [x] Implement `calculate_billing_cooldown_ms()` (base-2 exponential)
- [x] 24-hour failure window tracking
- [x] Unit tests for backoff sequences

### 2.3 Profile Ordering
- [x] Implement `resolve_auth_profile_order()`
- [x] Partition: available vs in-cooldown
- [x] Round-robin by last_used
- [x] Unit tests for ordering scenarios

### 2.4 Storage
- [ ] File storage: `~/.aether/agent:{id}/auth-profiles.json`
- [ ] File locking (fs2)
- [ ] Atomic writes

### 2.5 Runtime Integration
- [x] Integrate with `ProviderRegistry` (via `AuthProfileProviderRegistry`)
- [x] Error classification (401/402/403/429/timeout)
- [x] Profile rotation on failure (via cooldown system)
- [x] Mark good/failure hooks

## 3. Tests
- [ ] Integration test: streaming with code blocks
- [ ] Integration test: API key rotation on 429

## 4. Documentation
- [ ] Update design.md with algorithms
- [ ] Configuration examples

---

## Implementation Summary

### Commits

1. `feat(streaming): add fence-aware block chunking` - Markdown fence parsing + BlockReplyChunker with fence awareness
2. `feat(streaming): add block coalescer with idle timeout` - BlockCoalescer for message batching
3. `feat(providers): add auth profile management for API key rotation` - Core data model, cooldown algorithms, profile ordering
4. `feat(providers): add AuthProfileProviderRegistry for API key rotation` - ProviderRegistry integration

### Test Coverage

- **Markdown Fences**: 8 tests for fence parsing
- **Block Chunker**: 10 tests for fence-aware chunking
- **Block Coalescer**: 16 tests for coalescing behavior
- **Auth Profiles**: 17 tests for data model and algorithms
- **Auth Registry**: 6 tests for registry operations

**Total: 57 tests passing**

### Files Created/Modified

- `core/src/markdown/fences.rs` - Fence parsing utilities
- `core/src/markdown/mod.rs` - Module exports
- `core/src/thinking/streaming/block_reply_chunker.rs` - Enhanced with fence awareness
- `core/src/thinking/streaming/block_coalescer.rs` - New coalescer implementation
- `core/src/thinking/streaming/mod.rs` - Added exports
- `core/src/providers/auth_profiles.rs` - Auth profile data model
- `core/src/providers/auth_profile_registry.rs` - ProviderRegistry integration
- `core/src/providers/mod.rs` - Added exports
- `core/src/lib.rs` - Added markdown module
