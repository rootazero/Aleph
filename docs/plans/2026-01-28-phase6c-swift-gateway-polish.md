# Phase 6C: Swift Gateway Client Polish

**Date**: 2026-01-28
**Status**: In Progress
**Duration**: 1-2 weeks

---

## Current State Analysis

### Already Implemented ✅

| Component | File | Status |
|-----------|------|--------|
| WebSocket Client | GatewayClient.swift | Complete (448 lines) |
| Protocol Models | ProtocolModels.swift | Complete (442 lines) |
| Event Stream Manager | EventStreamManager.swift | Complete (300 lines) |
| Gateway Process Manager | GatewayManager.swift | Complete (310 lines) |
| Multi-Turn Adapter | GatewayMultiTurnAdapter.swift | Partial (242 lines) |

**Total: ~1,742 lines of Swift code**

### Key Gaps Identified

| Gap | Priority | Impact |
|-----|----------|--------|
| Request timeout | High | Requests hang indefinitely |
| User question UI | High | AskUser events ignored |
| Reconnection state recovery | Medium | Streaming interrupted |
| Part-driven UI rendering | Medium | No structured display |
| Token usage display | Low | Metrics not shown |

---

## Implementation Tasks

### Task 1: Request Timeout & Error Handling

Add timeout to RPC calls and improve error handling.

**Changes to GatewayClient.swift:**

```swift
// Add timeout parameter
func call<T: Decodable>(
    method: String,
    params: [String: Any]? = nil,
    timeout: TimeInterval = 30.0
) async throws -> T

// Add timeout task
let timeoutTask = Task {
    try await Task.sleep(nanoseconds: UInt64(timeout * 1_000_000_000))
    throw GatewayError.timeout(method: method)
}
```

### Task 2: Exponential Backoff Improvements

Enhance reconnection with state tracking.

**Reconnection Strategy:**
```
Attempt 1: 1s delay
Attempt 2: 2s delay
Attempt 3: 4s delay
Attempt 4: 8s delay
Attempt 5+: 30s delay (max)
```

**State Recovery:**
- Track pending requests before disconnect
- Re-subscribe to event topics after reconnect
- Resume active runs if possible

### Task 3: User Question UI Flow

Implement AskUser event handling with modal dialog.

**Flow:**
```
Gateway → AskUserEvent → Show modal → User selects → Send answer → Resume
```

**Components:**
1. `AskUserView.swift` - SwiftUI modal for question display
2. `answer()` RPC method in GatewayClient
3. Integration in GatewayMultiTurnAdapter

### Task 4: Part-Driven Event Mapping

Map Gateway events to UI parts properly.

**Event → Part Mapping:**

| Event | Part Type | UI Component |
|-------|-----------|--------------|
| reasoning | ReasoningPart | ReasoningPartView |
| toolStart/End | ToolCallPart | ToolCallPartView |
| responseChunk | TextPart | MessageBubbleView |
| runComplete | CompletionPart | (status update) |

### Task 5: Token Usage Display

Show token consumption in UI.

**RunSummary fields:**
- `totalTokens` → Status bar indicator
- `toolCalls` → Tool summary
- `loops` → Iteration count

---

## File Changes Summary

### Modified Files

| File | Changes |
|------|---------|
| GatewayClient.swift | Add timeout, improve error handling |
| GatewayMultiTurnAdapter.swift | Add user question handling, part mapping |
| UnifiedConversationViewModel.swift | Add token display, part updates |
| ProtocolModels.swift | Add answer request type |

### New Files

| File | Purpose |
|------|---------|
| AskUserView.swift | User question modal dialog |

---

## Implementation Order

1. **Task 1**: Request timeout (Day 1)
2. **Task 2**: Reconnection improvements (Day 2)
3. **Task 3**: User question UI (Day 3-4)
4. **Task 4**: Part-driven mapping (Day 5-6)
5. **Task 5**: Token display (Day 7)

---

## Success Criteria

- [x] RPC calls timeout after 30s by default
- [x] Reconnection uses exponential backoff
- [x] AskUser events show modal dialog
- [x] User answers sent back to Gateway
- [x] Reasoning/Tool events render as Parts
- [x] Token usage visible in UI

---

## Implementation Progress (2026-01-28)

### Completed

**Task 1: Request Timeout & Error Handling** - DONE
- Added `defaultRequestTimeout` to GatewayClientConfig (default 30s)
- Updated `call()` and `sendRequest()` with timeout parameter
- Implemented timeout using TaskGroup racing pattern
- Enhanced GatewayError with detailed timeout info (method, duration)

**Task 2: Exponential Backoff** - ALREADY IMPLEMENTED
- Reconnection already uses exponential backoff (1s → 30s max)
- Added state recovery methods to GatewayMultiTurnAdapter

**Task 3: User Question UI Flow** - DONE
- Created `AskUserView.swift` (300+ lines) with:
  - Multi-question support with header/options
  - Single-select and multi-select modes
  - "Other" custom input option
  - SwiftUI modal presentation
- Enhanced `AskUserEvent` with `questionId`, `questions` array
- Added `UserQuestion`, `QuestionOption` structs
- Added `AnswerParams`, `CancelParams`, `SubscribeParams` types
- GatewayClient: `answer()`, `cancelRun()`, `subscribe()`, `unsubscribe()` methods
- GatewayMultiTurnAdapter: `submitAnswer()`, `cancelQuestion()` methods
- UnifiedConversationViewModel: Gateway question integration

**Task 4: Part-Driven Event Mapping** - DONE
- GatewayMultiTurnAdapter now maintains `parts: [MessagePart]`
- MessagePart enum: `.text`, `.reasoning`, `.toolCall`, `.askUser`
- Event → Part mapping:
  - reasoning → ReasoningPart (accumulated, streaming)
  - toolStart → ToolCallPart (.running)
  - toolEnd → ToolCallPart (.success/.error)
  - responseChunk → TextPart (accumulated)
  - askUser → AskUserPart
- `updateOrAddPart()` for in-place updates by ID

**Task 5: Token Usage Display** - DONE
- UnifiedConversationViewModel: token usage properties
  - `totalTokens`, `toolCallCount`, `loopCount`
  - `tokenUsageDisplay` computed property with formatting
  - `updateTokenUsage(from: RunSummary)`
- GatewayMultiTurnAdapter: `runSummary` published property

### Files Modified

| File | Changes |
|------|---------|
| GatewayClient.swift | +timeout, +answer(), +cancelRun(), +subscribe() |
| GatewayMultiTurnAdapter.swift | +parts array, +AskUser handling, +Part-driven updates |
| ProtocolModels.swift | +AskUserEvent enhanced, +AnswerParams, +UserQuestion |
| UnifiedConversationViewModel.swift | +token usage, +Gateway question methods |

### New Files

| File | Purpose |
|------|---------|
| AskUserView.swift | SwiftUI modal for user questions |

---

## Testing Plan

### Manual Testing

1. Kill Gateway during request → verify timeout
2. Disconnect network → verify reconnection
3. Trigger AskUser event → verify modal appears
4. Long conversation → verify token count

### Integration Testing

1. Mock Gateway server for controlled event sequences
2. Test reconnection scenarios
3. Test concurrent requests
