# Protocol Adapter Phase 2: Complete Migration Design

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Migrate Claude and Gemini providers to ProtocolAdapter architecture, achieving unified cloud API provider design.

**Architecture:** Extend the proven HttpProvider + ProtocolAdapter pattern from Phase 1 to Anthropic and Gemini protocols.

**Tech Stack:** Rust, tokio, reqwest, serde, async-trait, futures

---

## 1. Scope and Objectives

### Migration Scope

- ✅ Migrate: `ClaudeProvider` → `AnthropicProtocol` + `HttpProvider`
- ✅ Migrate: `GeminiProvider` → `GeminiProtocol` + `HttpProvider`
- ✅ Delete: `openai/provider.rs` (legacy, factory no longer uses it)
- ❌ Keep Native: `OllamaProvider` (local model, different use case)

### Expected Benefits

| Metric | Before | After | Change |
|--------|--------|-------|--------|
| claude.rs | 1,040 lines | 0 | -1,040 |
| gemini.rs | 663 lines | 0 | -663 |
| openai/provider.rs | 792 lines | 0 | -792 |
| AnthropicProtocol | 0 | ~400 lines | +400 |
| GeminiProtocol | 0 | ~400 lines | +400 |
| **Net Change** | | | **~-1,700 lines** |

### Migration Order

1. **Claude first** - Primary provider, validates architecture
2. **Gemini second** - Replicate successful pattern
3. **Cleanup last** - Delete old OpenAI provider

---

## 2. Architecture Design

### Protocol Adapter Hierarchy

```
┌─────────────────────────────────────────────────────────┐
│                    AiProvider Trait                      │
│         (process, process_with_image, name, color)       │
└─────────────────────────┬───────────────────────────────┘
                          │
          ┌───────────────┼───────────────┐
          │               │               │
          ▼               ▼               ▼
   ┌─────────────┐ ┌─────────────┐ ┌──────────────┐
   │ HttpProvider│ │ HttpProvider│ │OllamaProvider│
   │ + OpenAi    │ │ + Anthropic │ │   (native)   │
   │   Protocol  │ │   Protocol  │ │              │
   └─────────────┘ └─────────────┘ └──────────────┘
          │               │
          ▼               ▼
   ┌─────────────┐ ┌─────────────┐ ┌─────────────┐
   │ OpenAi      │ │ Anthropic   │ │ Gemini      │
   │ Protocol    │ │ Protocol    │ │ Protocol    │
   └─────────────┘ └─────────────┘ └─────────────┘
```

### File Structure After Migration

```
core/src/providers/
├── protocols/
│   ├── mod.rs           # Export all protocols
│   ├── openai.rs        # OpenAiProtocol (exists)
│   ├── anthropic.rs     # AnthropicProtocol (new)
│   └── gemini.rs        # GeminiProtocol (new)
├── http_provider.rs     # Generic HTTP container (exists)
├── presets.rs           # Vendor presets (extend for claude/gemini)
├── adapter.rs           # ProtocolAdapter trait (exists)
├── ollama.rs            # Keep native
├── openai/
│   ├── mod.rs           # Keep
│   ├── types.rs         # Keep (used by OpenAiProtocol)
│   └── request.rs       # Keep
├── claude.rs            # DELETE
└── gemini.rs            # DELETE
```

---

## 3. AnthropicProtocol Design

### Protocol Differences from OpenAI

| Feature | OpenAI | Anthropic |
|---------|--------|-----------|
| Auth Header | `Authorization: Bearer {key}` | `x-api-key: {key}` |
| Version Header | None | `anthropic-version: 2023-06-01` |
| System Prompt | messages array with role=system | Separate `system` field |
| Response Path | `choices[0].message.content` | `content[0].text` |
| Thinking | `reasoning_effort` (o1/o3) | `thinking.budget_tokens` |
| Stream Format | `{"choices":[{"delta":...}]}` | `{"type":"content_block_delta",...}` |

### Implementation Structure

```rust
pub struct AnthropicProtocol {
    client: Client,
}

impl ProtocolAdapter for AnthropicProtocol {
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
        is_streaming: bool
    ) -> Result<RequestBuilder> {
        // 1. Endpoint: {base_url}/v1/messages
        // 2. Headers: x-api-key, anthropic-version, content-type
        // 3. Body: model, messages, max_tokens, system, thinking
        // 4. Handle Extended Thinking (ThinkLevel → budget_tokens)
    }

    async fn parse_response(&self, response: Response) -> Result<String> {
        // Parse content[0].text or thinking block
    }

    async fn parse_stream(&self, response: Response) -> Result<BoxStream<...>> {
        // Parse content_block_delta events
    }

    fn name(&self) -> &'static str { "anthropic" }
}
```

### Preset Configuration

```rust
("claude", ProviderPreset {
    base_url: "https://api.anthropic.com",
    protocol: "anthropic",
    color: "#d97757"
})
("anthropic", ProviderPreset { ... }) // alias
```

---

## 4. GeminiProtocol Design

### Protocol Differences from OpenAI

| Feature | OpenAI | Gemini |
|---------|--------|--------|
| Auth | Header `Authorization` | Query param `?key={key}` |
| Endpoint | `/v1/chat/completions` | `/v1beta/models/{model}:generateContent` |
| Streaming | Same endpoint + `stream=true` | `:streamGenerateContent?alt=sse` |
| Messages | `messages: [{role, content}]` | `contents: [{role, parts}]` |
| System | `role: "system"` | `systemInstruction` field |
| Response | `choices[0].message.content` | `candidates[0].content.parts[0].text` |
| Thinking | `reasoning_effort` | `thinkingConfig.thinkingBudget` |

### Implementation Structure

```rust
pub struct GeminiProtocol {
    client: Client,
}

impl ProtocolAdapter for GeminiProtocol {
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
        is_streaming: bool
    ) -> Result<RequestBuilder> {
        // 1. Endpoint: {base_url}/v1beta/models/{model}:generateContent?key={key}
        //    Or: :streamGenerateContent?alt=sse&key={key}
        // 2. Body: contents, systemInstruction, generationConfig
        // 3. Handle Thinking (ThinkLevel → thinkingBudget)
    }

    async fn parse_response(&self, response: Response) -> Result<String> {
        // Parse candidates[0].content.parts[0].text
    }

    async fn parse_stream(&self, response: Response) -> Result<BoxStream<...>> {
        // Parse Gemini SSE format
    }

    fn name(&self) -> &'static str { "gemini" }
}
```

### Preset Configuration

```rust
("gemini", ProviderPreset {
    base_url: "https://generativelanguage.googleapis.com",
    protocol: "gemini",
    color: "#4285f4"
})
("google", ProviderPreset { ... }) // alias
```

---

## 5. Factory Updates

### Updated create_provider Logic

```rust
pub fn create_provider(name: &str, mut config: ProviderConfig) -> Result<Arc<dyn AiProvider>> {
    // 1. Apply preset (existing logic)
    if let Some(preset) = presets::get_preset(&name_lower) { ... }

    // 2. Route by protocol
    match config.protocol().as_str() {
        "openai" => {
            // HttpProvider + OpenAiProtocol (exists)
            let adapter = Arc::new(OpenAiProtocol::new(client));
            Ok(Arc::new(HttpProvider::new(name, config, adapter)?))
        }
        "anthropic" => {
            // HttpProvider + AnthropicProtocol (new)
            let adapter = Arc::new(AnthropicProtocol::new(client));
            Ok(Arc::new(HttpProvider::new(name, config, adapter)?))
        }
        "gemini" => {
            // HttpProvider + GeminiProtocol (new)
            let adapter = Arc::new(GeminiProtocol::new(client));
            Ok(Arc::new(HttpProvider::new(name, config, adapter)?))
        }
        "ollama" => {
            // Keep native OllamaProvider
            Ok(Arc::new(OllamaProvider::new(name, config)?))
        }
        unknown => Err(AetherError::invalid_config(...))
    }
}
```

### Backward Compatibility

- Config `provider_type: "claude"` continues to work
- `protocol()` method auto-infers: `claude` → `anthropic`
- Users need not modify any configuration

---

## 6. Testing Strategy

### Unit Tests (per Protocol)

- `test_build_request_basic` - Basic request construction
- `test_build_request_with_system_prompt` - System prompt handling
- `test_build_request_with_thinking` - Extended Thinking configuration
- `test_build_request_multimodal` - Image/attachment handling
- `test_parse_response` - Response parsing
- `test_parse_response_error` - Error response handling
- `test_parse_stream` - SSE stream parsing

### Integration Tests

- Reuse existing `providers::tests` factory tests
- `test_create_claude_provider` - Verify Claude creation
- `test_create_gemini_provider` - Verify Gemini creation

### Success Criteria

| Criterion | Verification |
|-----------|--------------|
| All existing tests pass | `cargo test -p aethecore` |
| Claude features complete | Extended Thinking, Vision, Streaming |
| Gemini features complete | Thinking, Vision, Streaming |
| Net code reduction | `git diff --stat main` shows -1,500+ lines |
| Backward compatible | Existing configs work unchanged |
| No performance regression | Request latency unchanged |

---

## 7. Migration Steps

### Phase 2a: Claude Migration

1. Implement `AnthropicProtocol` with tests
2. Add claude/anthropic presets
3. Update factory to route anthropic → HttpProvider
4. Verify all Claude tests pass
5. Delete `claude.rs`
6. Commit

### Phase 2b: Gemini Migration

7. Implement `GeminiProtocol` with tests
8. Add gemini/google presets
9. Update factory to route gemini → HttpProvider
10. Verify all Gemini tests pass
11. Delete `gemini.rs`
12. Commit

### Phase 2c: Cleanup

13. Delete `openai/provider.rs`
14. Update module documentation
15. Final verification
16. Commit

### Rollback Strategy

- Each Protocol is an independent commit, can be rolled back separately
- Git history preserves deleted files
- If issues arise, factory can quickly switch back to old implementation
