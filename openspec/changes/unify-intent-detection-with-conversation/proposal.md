# Unify Intent Detection with Conversation

## Summary

Refactor the intent detection system to use a universal AI-first approach integrated with multi-turn conversation. Every message (initial or follow-up) goes through AI intent detection to determine: (1) which capability to invoke, and (2) what information is missing and needs user clarification.

## Motivation

### Current Problems

1. **Fragmented Detection Logic**: The codebase has multiple detection paths:
   - `AiIntentDetector` (AI-powered, returns JSON)
   - `SmartTriggerDetector` (regex-based)
   - `IntentDetector` (legacy, weather-specific)
   - `process_with_ai_first` (AI decides capabilities inline)

2. **Weather-Specific Code**: The current intent detection has hardcoded weather patterns (`IntentType::Weather`, weather regex patterns) that should be generalized.

3. **Inconsistent Conversation Integration**: Multi-turn conversation (`start_conversation`, `continue_conversation`) bypasses the standard intent detection flow.

4. **No Universal Capability Discovery**: Tools are not automatically declared to the AI in a consistent manner.

### Desired Behavior

1. User sends any message (e.g., "今天天气怎么样")
2. AI receives the message along with all available capabilities (search, video, etc.)
3. AI returns structured JSON indicating:
   - Which capability to use (or none for general chat)
   - What parameters are needed but missing (e.g., location)
4. If missing parameters:
   - Halo shows clarification UI (select or text input)
   - User provides missing info
   - Process continues with augmented input
5. Capability executes (e.g., search runs)
6. AI generates final response with capability results
7. Response is pasted to target window
8. Halo shows continuation input for multi-turn
9. Each follow-up also goes through intent detection

## Scope

### In Scope

1. **Unified Intent Detection**: Single AI-based detection for all messages
2. **Generic Capability Declaration**: All capabilities declared dynamically to AI
3. **Clarification Integration**: Seamless use of existing `ClarificationRequest` system
4. **Multi-turn Integration**: Conversation methods use unified detection
5. **Memory Integration**: Memory context included in all requests
6. **Focus Management**: Cursor focus returns to original app after AI response

### Out of Scope

- MCP integration (reserved for future)
- Skill workflows (reserved for future)
- New capabilities beyond search/video
- UI redesign of Halo

## Technical Approach

### 1. Universal Capability Declaration

Extend `CapabilityRegistry` to generate a standardized declaration format that includes parameter requirements and examples. This declaration is sent with every AI request.

```rust
pub struct UniversalCapabilityDeclaration {
    pub id: String,
    pub name: String,
    pub description: String,
    pub parameters: Vec<ParameterDeclaration>,
    pub when_to_use: Vec<String>,
    pub examples: Vec<String>,
}

pub struct ParameterDeclaration {
    pub name: String,
    pub param_type: String,
    pub description: String,
    pub required: bool,
    pub clarification_prompt: Option<String>,
    pub suggestions: Option<Vec<String>>,
}
```

### 2. Universal AI Response Format

Standardize the AI response format to handle all cases:

```json
{
  "type": "direct" | "capability" | "clarification",

  // For type="direct": direct response text
  "response": "Here is my answer...",

  // For type="capability": capability invocation
  "capability": {
    "id": "search",
    "parameters": { "query": "北京天气" }
  },

  // For type="clarification": request user input
  "clarification": {
    "param_name": "location",
    "prompt": "请问您想查询哪个城市的天气？",
    "suggestions": ["北京", "上海", "深圳", "广州"],
    "input_type": "select" | "text"
  }
}
```

### 3. Unified Processing Flow

Consolidate all processing into a single flow:

```
User Input → UnifiedProcessor
    ↓
Build Capability Context (capabilities + memory)
    ↓
AI Call with capability-aware prompt
    ↓
Parse Response → direct | capability | clarification
    ↓
[If clarification needed]
    Show Halo clarification UI
    Wait for user input
    Augment input
    Retry from AI Call
    ↓
[If capability needed]
    Execute capability
    Make second AI call with results
    ↓
Return Response
```

### 4. Remove Legacy Code

Delete weather-specific intent detection:
- `IntentType::Weather` and related patterns
- Weather-specific regex in `SmartTriggerDetector`
- Weather-specific clarification templates

### 5. Focus Management

When AI response is ready and pasted to target window:
1. Activate original application window
2. Paste response
3. After paste completes, show Halo continuation input
4. Cursor auto-focuses in Halo input field
5. When user presses ESC, hide Halo and session ends

## Dependencies

- Existing `CapabilityRegistry` and `CapabilityDeclaration`
- Existing `ClarificationRequest/ClarificationResult` system
- Existing `ConversationManager` for multi-turn state
- Existing Halo UI components

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| AI response latency for clarification | Use fast lightweight model for intent detection |
| Parsing errors in AI JSON response | Robust fallback to direct response |
| Focus management race conditions | Use completion callbacks, proper async handling |

## Success Criteria

1. User can ask "今天天气怎么样" and get prompted for location automatically
2. Any capability (search, video) is invoked based on AI judgment, not hardcoded regex
3. Multi-turn conversation maintains context across turns
4. Each turn can trigger different capabilities as needed
5. Cursor focus behaves correctly: in Halo during input, in target app during response
