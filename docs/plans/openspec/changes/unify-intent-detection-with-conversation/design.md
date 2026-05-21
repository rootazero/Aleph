# Design: Unified Intent Detection with Conversation

## Design Principles

1. **Low Coupling, High Cohesion**: Minimize dependencies, maximize module focus
2. **Reuse Over Rewrite**: Leverage existing infrastructure
3. **No Dead Code**: Remove unused legacy code, don't add parallel paths
4. **Modify In Place**: Extend existing methods, don't create redundant abstractions

## What We Already Have (Reuse)

| Component | Location | Status |
|-----------|----------|--------|
| `CapabilityDeclaration` | `capability/declaration.rs` | Use as-is |
| `CapabilityRequest` | `capability/request.rs` | Use as-is |
| `ClarificationInfo` | `capability/request.rs` | Use as-is |
| `AiResponse` enum | `capability/request.rs` | Already has Direct/Capability/Clarification |
| `ResponseParser` | `capability/response_parser.rs` | Use as-is |
| `ClarificationRequest` | `clarification/mod.rs` | Use as-is |
| `process_with_ai_first` | `core.rs` | **Modify** - already handles capabilities |
| `on_clarification_needed` | `event_handler.rs` | Use as-is |

## What Needs to Change

### 1. Modify `process_with_ai_first` (core.rs)

The existing method already handles `AiResponse::NeedsClarification`. We need to:

1. **Improve clarification handling**: Currently it recursively calls itself. Make the loop explicit for clarity.
2. **Ensure it's called by conversation methods**: Already done in our previous fix.

```rust
// Current flow (already exists):
AiResponse::NeedsClarification(info) => {
    let clarification_request = /* convert info to ClarificationRequest */;
    let result = self.event_handler.on_clarification_needed(clarification_request);

    if result.is_success() {
        // Augment input and reprocess
        let augmented_input = format!("{}\n\n用户补充: {}", input, value);
        return self.process_with_ai_first(augmented_input, context, start_time);
    }
}
```

**No new code needed here** - it already works correctly.

### 2. Remove Legacy Intent Detection Code

Delete these files/modules that are now redundant:

| File | Reason |
|------|--------|
| `intent/patterns.rs` | Weather-specific patterns, replaced by AI detection |
| `intent/smart_trigger.rs` | Regex-based triggers, replaced by AI detection |
| `IntentDetector` struct | Legacy detector, no longer needed |
| `IntentType::Weather` | Hardcoded intent type, AI handles generically |

Keep:
- `intent/ai_detector.rs` - Useful for standalone intent detection if needed
- `intent/mod.rs` - Simplified to just re-export what's needed

### 3. Simplify Config (config/mod.rs)

Remove `ai_first` flag - AI-first is now the only mode:

```toml
[smart_flow.intent_detection]
enabled = true           # Keep: enable/disable intent detection
# ai_first = true        # Remove: always use AI-first now
use_ai = true            # Keep: for compatibility
confidence_threshold = 0.7
```

### 4. Focus Management (Swift)

**HaloWindow.swift** - Add focus return after paste:

```swift
// After AI response is pasted to target window
func onResponsePasted() {
    // Return focus to original app (already happens via paste)

    // Show continuation input after brief delay
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) {
        self.showConversationContinuationInput()
        self.inputField.becomeFirstResponder()  // Auto-focus
    }
}
```

## Processing Flow (Simplified)

```
User Input
    │
    ▼
start_conversation() / continue_conversation()
    │
    └──► process_with_ai_first()  [EXISTING METHOD]
            │
            ├── Build capability-aware prompt
            ├── Include memory context (if enabled)
            ├── AI call returns AiResponse
            │
            ├── [Direct] → Return response
            │
            ├── [Capability] → Execute → Second AI call → Return
            │
            └── [Clarification] → on_clarification_needed()
                    │
                    ├── User provides value
                    ├── Augment input
                    └── Retry (loop)
```

## Code Changes Summary

| File | Action | Description |
|------|--------|-------------|
| `core.rs` | MODIFY | Remove `ai_first` check in conversation methods (already done) |
| `intent/patterns.rs` | DELETE | Weather-specific patterns |
| `intent/smart_trigger.rs` | DELETE | Regex-based triggers |
| `intent/mod.rs` | SIMPLIFY | Remove legacy exports |
| `config/mod.rs` | SIMPLIFY | Remove `ai_first` field |
| `HaloWindow.swift` | MODIFY | Add auto-focus for continuation input |

## Lines of Code Impact

- **Remove**: ~800 lines (patterns.rs ~300, smart_trigger.rs ~400, legacy IntentDetector ~100)
- **Add**: ~20 lines (Swift focus handling)
- **Modify**: ~50 lines (simplify config, clean up imports)

**Net**: -700 lines

## Testing

1. **Existing tests continue to work**: `process_with_ai_first` tests, `ResponseParser` tests
2. **Remove legacy tests**: Tests for `IntentDetector`, `SmartTriggerDetector`
3. **Add manual test**: "今天天气怎么样" → clarification prompt → select city → search executes
