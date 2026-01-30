# Tasks: Block Streaming & Auth Profiles

## 1. Block Streaming - Fence Awareness

### 1.1 Markdown Fence Parsing
- [ ] Implement `FenceSpan` struct with start/end/marker/indent/language
- [ ] Implement `parse_fence_spans()` line-by-line parser
- [ ] Add `is_safe_fence_break()` utility
- [ ] Unit tests for fence parsing edge cases

### 1.2 Fence-Aware Chunking
- [ ] Add `FenceSplit` struct (close_line, reopen_line)
- [ ] Update `BlockReplyChunker` to use fence spans
- [ ] Implement fence close/reopen on forced breaks
- [ ] Unit tests for fence splitting scenarios

### 1.3 Block Coalescer
- [ ] Implement `BlockCoalescer` with idle timeout
- [ ] Add `CoalescingConfig` (min_chars, max_chars, idle_ms, joiner)
- [ ] Tokio timer integration for idle flush
- [ ] Unit tests for coalescing behavior

### 1.4 Configuration
- [ ] Add `StreamingConfig` to gateway config
- [ ] Channel-specific overrides support
- [ ] Default values matching Moltbot (min=800, max=1200, idle=1000ms)

## 2. Auth Profiles - API Key Rotation

### 2.1 Data Model
- [ ] Define `AuthProfileCredential` enum (ApiKey, Token, OAuth)
- [ ] Define `ProfileUsageStats` struct
- [ ] Define `AuthProfileStore` struct
- [ ] JSON serialization tests

### 2.2 Cooldown Algorithm
- [ ] Implement `calculate_cooldown_ms()` (base-5 exponential)
- [ ] Implement `calculate_billing_cooldown_ms()` (base-2 exponential)
- [ ] 24-hour failure window tracking
- [ ] Unit tests for backoff sequences

### 2.3 Profile Ordering
- [ ] Implement `resolve_auth_profile_order()`
- [ ] Partition: available vs in-cooldown
- [ ] Round-robin by last_used
- [ ] Unit tests for ordering scenarios

### 2.4 Storage
- [ ] File storage: `~/.aether/agent:{id}/auth-profiles.json`
- [ ] File locking (fs2)
- [ ] Atomic writes

### 2.5 Runtime Integration
- [ ] Integrate with `ExecutionEngine`
- [ ] Error classification (401/402/403/429/timeout)
- [ ] Profile rotation on failure
- [ ] Mark good/failure hooks

## 3. Tests
- [ ] Integration test: streaming with code blocks
- [ ] Integration test: API key rotation on 429

## 4. Documentation
- [ ] Update design.md with algorithms
- [ ] Configuration examples
