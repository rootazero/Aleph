# Design: Enhance Routing Rule System

## Overview

This document details the technical design for enhancing Aether's routing rule system to support context-aware matching, per-rule system prompts, and clear fallback behavior.

## Architecture

### Current Architecture (Before Change)

```
User Input (Clipboard Content)
    ↓
Router::route(&str)
    ↓
Match against rules (regex)
    ↓
Return (Provider, Option<SystemPrompt>)
    ↓
AI Provider processes with prompt
```

**Limitations**:
- Only matches clipboard content
- Unclear system prompt handling
- Ambiguous rule ordering
- Unclear fallback behavior

### New Architecture (After Change)

```
Window Context + Clipboard Content
    ↓
Build Context String: "[AppName] WindowTitle\nContent"
    ↓
Router::route(&str)
    ↓
Match rules in order (first match wins)
    ↓
IF match:
    Return (Provider, Rule's SystemPrompt OR Provider's Default)
ELSE:
    Return (DefaultProvider, Provider's Default)
    ↓
AI Provider processes with final prompt
```

**Improvements**:
- Full context awareness
- Clear system prompt priority
- Explicit first-match-stops logic
- Defined fallback behavior

## Component Design

### 1. Context String Builder

**Location**: `Aether/core/src/core.rs`

**Method**: `build_routing_context(window_context: &CapturedContext, clipboard: &str) -> String`

**Implementation**:
```rust
fn build_routing_context(
    window_context: &CapturedContext,
    clipboard_content: &str
) -> String {
    // Format: "[AppName] WindowTitle\nClipboardContent"
    let app_name = extract_app_name(&window_context.bundle_id);
    format!(
        "[{}] {}\n{}",
        app_name,
        window_context.window_title,
        clipboard_content
    )
}

fn extract_app_name(bundle_id: &str) -> &str {
    // Extract app name from bundle ID
    // e.g., "com.apple.Notes" -> "Notes"
    bundle_id.split('.').last().unwrap_or("Unknown")
}
```

**Example Outputs**:
```
[Notes] Meeting Notes.txt
Discuss Q1 roadmap for Aether project

[VSCode] main.rs - Aether
fn process_clipboard() { ... }

[WeChat] Chat with Alice
帮我翻译这段话
```

### 2. Router Enhancement

**Location**: `Aether/core/src/router/mod.rs`

#### 2.1 Updated `Router::route()` Method

**Old Signature**:
```rust
pub fn route(&self, input: &str) -> Option<(&dyn AiProvider, Option<&str>)>
```

**New Signature** (same, but semantic change):
```rust
pub fn route(&self, context: &str) -> Option<(&dyn AiProvider, Option<&str>)>
```

**Implementation Changes**:
```rust
pub fn route(&self, context: &str) -> Option<(&dyn AiProvider, Option<&str>)> {
    debug!(context_length = context.len(), "Starting route decision with full context");

    // Iterate rules in order (first match wins)
    for (index, rule) in self.rules.iter().enumerate() {
        if rule.matches(context) {
            if let Some(provider) = self.providers.get(rule.provider_name()) {
                info!(
                    rule_index = index,
                    provider = %rule.provider_name(),
                    has_custom_prompt = rule.system_prompt().is_some(),
                    "Rule matched (first-match-stops)"
                );

                // Return provider with rule's system prompt (if exists)
                return Some((provider.as_ref(), rule.system_prompt()));
            }
        }
    }

    // No rule matched, fall back to default provider
    debug!("No rule matched, using fallback");
    self.default_provider
        .as_ref()
        .and_then(|name| self.providers.get(name))
        .map(|provider| {
            info!(
                provider = %provider.name(),
                "Using default provider (no rule match)"
            );
            (provider.as_ref(), None) // Default provider uses its own prompt
        })
}
```

**Key Changes**:
1. Renamed parameter from `input` to `context` (semantic clarity)
2. Added `first-match-stops` comment in logs
3. Explicit fallback logic with clear logging
4. Rule's `system_prompt()` has priority over provider default

#### 2.2 System Prompt Priority Logic

**Priority Order** (highest to lowest):
1. **Rule's `system_prompt`**: If rule defines it, use it
2. **Provider's default prompt**: If rule has no prompt, provider may have one
3. **None**: If neither has a prompt

**Implementation** (in `AiProvider::process()`):
```rust
async fn process(
    &self,
    user_input: &str,
    system_prompt_override: Option<&str>
) -> Result<String> {
    let final_prompt = system_prompt_override
        .or(self.default_system_prompt.as_deref());

    // Use final_prompt in API call
    // ...
}
```

### 3. Configuration Updates

#### 3.1 Rule Management API

**Location**: `Aether/core/src/config/mod.rs`

**New Methods**:
```rust
impl Config {
    /// Add a new rule at the top (highest priority)
    pub fn add_rule_at_top(&mut self, rule: RoutingRuleConfig) {
        self.rules.insert(0, rule);
    }

    /// Remove rule at index
    pub fn remove_rule(&mut self, index: usize) -> Result<()> {
        if index < self.rules.len() {
            self.rules.remove(index);
            Ok(())
        } else {
            Err(AetherError::invalid_config(
                format!("Rule index {} out of bounds", index)
            ))
        }
    }

    /// Move rule from one index to another
    pub fn move_rule(&mut self, from: usize, to: usize) -> Result<()> {
        if from >= self.rules.len() || to >= self.rules.len() {
            return Err(AetherError::invalid_config("Invalid rule indices"));
        }
        let rule = self.rules.remove(from);
        self.rules.insert(to, rule);
        Ok(())
    }

    /// Get rule at index
    pub fn get_rule(&self, index: usize) -> Option<&RoutingRuleConfig> {
        self.rules.get(index)
    }
}
```

#### 3.2 Enhanced Validation

**Location**: `Aether/core/src/config/mod.rs`

**Addition to `Config::validate()`**:
```rust
// Warn if default_provider is missing
if self.general.default_provider.is_none() {
    warn!(
        "No default_provider configured. \
         Requests will fail if no routing rule matches."
    );
}

// Warn if rules list is empty
if self.rules.is_empty() {
    warn!(
        "No routing rules configured. \
         All requests will use default_provider (if set)."
    );
}
```

### 4. UniFFI Interface Updates

**Location**: `Aether/core/src/aether.udl`

**Current `CapturedContext` Dictionary**:
```idl
dictionary CapturedContext {
    string bundle_id;
    string window_title;
};
```

**No changes needed** - already has required fields.

**Swift Side**:
The Swift code in `AppDelegate.swift` already captures window context. We just need to ensure it's passed to Rust before routing.

### 5. Integration with Memory Module

**Important**: Context matching happens BEFORE memory retrieval.

**Flow**:
```
1. Capture window context + clipboard
2. Build context string
3. Route to provider (select which AI to use)
4. Retrieve relevant memories (if enabled)
5. Augment prompt with memories
6. Send to selected provider
```

This ensures routing is fast (no embedding inference needed) and memory augmentation happens after provider selection.

## Data Flow

### Complete Request Flow

```
┌─────────────────────────────────────────────────────────────┐
│ 1. User presses hotkey in Notes app                         │
│    Window: "[Notes] Meeting Notes.txt"                      │
│    Clipboard: "Discuss Q1 roadmap"                          │
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│ 2. Swift captures window context                            │
│    CapturedContext {                                         │
│      bundle_id: "com.apple.Notes",                          │
│      window_title: "Meeting Notes.txt"                      │
│    }                                                         │
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│ 3. Rust builds routing context                              │
│    "[Notes] Meeting Notes.txt\nDiscuss Q1 roadmap"         │
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│ 4. Router matches rules (first-match-stops)                 │
│    Rule 1: ^/code    ❌ No match                            │
│    Rule 2: ^\[Notes\] ✅ Match! → Claude + Custom Prompt   │
│    Rule 3: .*        ⏭️  Skipped (already matched)          │
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│ 5. Selected: Claude provider                                │
│    System Prompt: "You are a meeting notes assistant."      │
│    (from Rule 2's system_prompt field)                      │
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│ 6. Memory retrieval (if enabled)                            │
│    Search for similar past interactions                     │
│    Append to prompt                                          │
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│ 7. Send to Claude API                                       │
│    POST https://api.anthropic.com/v1/messages               │
│    {                                                         │
│      system: "You are a meeting notes assistant.",          │
│      messages: [{ user: "Discuss Q1 roadmap" }]             │
│    }                                                         │
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│ 8. Response processed and pasted                            │
└─────────────────────────────────────────────────────────────┘
```

## Configuration Examples

### Example 1: Window Context Matching

```toml
# Match by application context
[[rules]]
regex = "^\\[VSCode\\]"
provider = "claude"
system_prompt = "You are a senior software engineer. Provide concise, production-ready code."

[[rules]]
regex = "^\\[Notes\\]"
provider = "openai"
system_prompt = "You are a helpful writing assistant. Help organize and improve notes."

[[rules]]
regex = "^\\[WeChat\\]"
provider = "deepseek"
system_prompt = "You are a translation assistant. Translate between English and Chinese naturally."
```

### Example 2: Content Prefix Matching

```toml
# Match by content prefix (traditional routing)
[[rules]]
regex = "/code"
provider = "claude"
system_prompt = "You are a coding expert."

[[rules]]
regex = "/draw"
provider = "openai"
system_prompt = "You are DALL-E. Generate image descriptions."

[[rules]]
regex = "/translate"
provider = "deepseek"
system_prompt = "You are a professional translator."
```

### Example 3: Hybrid Matching

```toml
# Combine window context + content pattern
[[rules]]
regex = "^\\[VSCode\\].*TODO"
provider = "claude"
system_prompt = "You are a task planning assistant. Help break down TODOs into actionable steps."

[[rules]]
regex = "^\\[Mail\\].*meeting"
provider = "openai"
system_prompt = "You are a meeting scheduler. Extract key meeting details."

# Catch-all fallback (should be last rule)
[[rules]]
regex = ".*"
provider = "openai"
system_prompt = "You are a helpful AI assistant."
```

### Example 4: No Rules (Pure Default)

```toml
[general]
default_provider = "claude"

# No rules defined - all requests go to default provider
```

## Edge Cases and Error Handling

### Edge Case 1: Empty Window Context

**Scenario**: User triggers hotkey in an app without window title
- Window context: `[AppName] `
- Clipboard: `User input`
- **Result**: Context string is `[AppName] \nUser input`
- **Behavior**: Rules can still match `[AppName]` or clipboard content

### Edge Case 2: Empty Clipboard

**Scenario**: User triggers hotkey with nothing in clipboard
- Window context: `[Notes] Document.txt`
- Clipboard: `` (empty)
- **Result**: Context string is `[Notes] Document.txt\n`
- **Behavior**: Rules can match window context only

### Edge Case 3: No Rules Match and No Default Provider

**Scenario**: No rules match and `default_provider` is not configured
- **Result**: `Router::route()` returns `None`
- **Error**: `AetherError::NoProviderAvailable`
- **User Experience**: Error notification shown, no AI processing

### Edge Case 4: Rule Matches but Provider Disabled

**Scenario**: Rule matches but selected provider is `enabled = false`
- **Current Behavior**: Providers are filtered at initialization
- **Result**: Provider won't be in `providers` HashMap
- **Fallback**: Falls through to next rule or default provider

### Edge Case 5: Very Long Context String

**Scenario**: Window title + clipboard content exceeds 10,000 chars
- **Mitigation**: No limit on routing (regex matching is fast)
- **Note**: AI provider may have token limits (handled separately)

## Performance Considerations

### Regex Matching Performance

**Expected Performance**:
- 10 rules: < 0.5ms
- 20 rules: < 1ms
- 50 rules: < 2ms

**Optimization Strategies** (if needed in future):
1. **Prefix Trie**: For simple prefix patterns like `^/code`
2. **Compiled Regex Cache**: Already done (regex compiled at init)
3. **Rule Reordering**: Put most common matches first

**Measurement**:
```rust
let start = std::time::Instant::now();
let result = router.route(&context);
let duration = start.elapsed();
debug!(duration_us = duration.as_micros(), "Routing completed");
```

### Memory Impact

**Context String Size**:
- Average: ~500 bytes (typical window title + clipboard)
- Maximum: ~10KB (large clipboard content)
- **Impact**: Negligible (allocated on stack or short-lived heap)

**Rule Storage**:
- 10 rules × ~200 bytes = ~2KB
- 50 rules × ~200 bytes = ~10KB
- **Impact**: Negligible

## Testing Strategy

### Unit Tests

**Test File**: `Aether/core/src/router/mod.rs`

**Test Cases**:
```rust
#[test]
fn test_route_with_window_context() {
    // Context: "[VSCode] main.rs\nfn main() {}"
    // Rule: ^\\[VSCode\\]
    // Expected: Match Rule 1
}

#[test]
fn test_route_first_match_stops() {
    // Context: "hello world"
    // Rule 1: "hello" → Provider A
    // Rule 2: "world" → Provider B
    // Expected: Provider A (first match wins)
}

#[test]
fn test_route_fallback_to_default() {
    // Context: "no match"
    // Rules: None match
    // Default: OpenAI
    // Expected: OpenAI provider
}

#[test]
fn test_route_system_prompt_priority() {
    // Rule with custom prompt
    // Expected: Return rule's prompt, not provider's
}
```

### Integration Tests

**Test File**: `Aether/core/src/core.rs`

**Test Cases**:
```rust
#[tokio::test]
async fn test_end_to_end_routing() {
    // 1. Create AetherCore with config
    // 2. Set window context
    // 3. Set clipboard content
    // 4. Call process_clipboard()
    // 5. Verify correct provider selected
    // 6. Verify correct system prompt used
}
```

### Manual Tests

**Scenario 1**: Notes App
- Open Notes app
- Copy text
- Press hotkey
- Verify: Matches `[Notes]` rule

**Scenario 2**: VSCode
- Open VSCode
- Copy code snippet
- Press hotkey
- Verify: Matches `[VSCode]` rule

**Scenario 3**: No Match
- Open unknown app
- Copy random text
- Press hotkey
- Verify: Uses default provider

## Migration Guide

### For Existing Users

**Current Config**:
```toml
[[rules]]
regex = "^/code"
provider = "claude"
```

**Migration**: No changes needed! The new system is backward compatible.
- Old rules that match clipboard content still work
- Just add window context patterns if desired

**Recommended Enhancements**:
```toml
# Old rule (still works)
[[rules]]
regex = "^/code"
provider = "claude"

# New rule (more specific)
[[rules]]
regex = "^\\[VSCode\\]"
provider = "claude"
system_prompt = "You are a coding expert."
```

### For New Users

**Recommended Starter Config**:
```toml
[general]
default_provider = "openai"

# App-specific routing
[[rules]]
regex = "^\\[VSCode\\]|^\\[Cursor\\]"
provider = "claude"
system_prompt = "You are a senior software engineer."

[[rules]]
regex = "^\\[Notes\\]|^\\[Bear\\]"
provider = "openai"
system_prompt = "You are a writing assistant."

# Content-based routing
[[rules]]
regex = "/translate"
provider = "deepseek"
system_prompt = "You are a translator."

# Catch-all (optional)
[[rules]]
regex = ".*"
provider = "openai"
```

## Open Questions

### Q1: Should we limit window context length?

**Options**:
1. No limit (current design)
2. Limit to 200 chars
3. Limit to 500 chars

**Recommendation**: No limit for now. Monitor performance and add limit if needed.

### Q2: Should we support multiple context sources?

**Future Enhancement**: Could add more context sources
- Browser tab title
- Active file path
- Selected text (before cut)

**Recommendation**: Out of scope for this change. Can add in future.

### Q3: Should we provide a rule testing tool?

**Potential CLI Command**:
```bash
aether test-route "[Notes] Doc.txt\nHello world"
# Output: Matched Rule 2 (^\\[Notes\\]) → Provider: openai
```

**Recommendation**: Nice-to-have, but not blocking. Can add later.

## Conclusion

This design provides:
1. ✅ Context-aware routing (window + clipboard)
2. ✅ Clear system prompt priority (rule > provider > none)
3. ✅ Explicit first-match-stops logic
4. ✅ Well-defined fallback behavior
5. ✅ Backward compatibility with existing configs
6. ✅ Easy rule management API
7. ✅ Comprehensive testing strategy

The implementation is straightforward and builds on existing infrastructure with minimal changes.
